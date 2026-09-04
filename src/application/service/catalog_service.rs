//! The merchandising surface (hand-written; user-owned; see
//! `metaphor.codegen.yaml`): publish-gated public reads (§4), the
//! officer merchandising verbs (§6.2), and the closed sort vocabulary
//! (§4.3).
//!
//! THE ONE DOMAIN CONTRACT (§4.1) — a product is storefront-visible on
//! website W iff a LIVE `product_listings(website_id, item_id)` row
//! exists AND `sale_ok` AND `is_published` AND the catalog item is
//! active through the port. The contract lives HERE, in the read
//! verbs' SQL + the port check — never in an ACL rule, never in a
//! resolver enrollment (the product entity is NOT enrolled in
//! website's page-scoped resolver; the listing row IS the
//! per-website grain).
//!
//! CLOSED SORT VOCABULARY (§4.3): `relevance | newest | price_asc |
//! price_desc | name_asc` — a validated enum, each arm a fixed
//! comparator, never a client-supplied order expression.
//!
//! Publish verbs (§4.2) copy the website publish contract: publish /
//! unpublish are the ONLY writers of `is_published`; the field is
//! excluded from every patch whitelist (the typed 422 + the
//! `publish_refused` audit row live at the route + verb boundary).

use rust_decimal::Decimal;
use uuid::Uuid;

use super::audit::{record_audit, ActorRef};
use super::catalog_read_port::CatalogReadPort;
use super::party_write_port::PartyWritePort;
use super::pricing_service::settings_for;
use super::storefront_error::StorefrontError;

/// The page-size bound the listing read enforces (a cheap honest cap —
/// the read is gate-filtered and catalog-bounded already).
const MAX_PAGE_SIZE: i64 = 100;
/// The gated-row fetch bound: the listing read materializes at most
/// this many gated rows before the Rust-side name filter/sort/page.
/// P1 merchandising sets are far smaller; the bound keeps the read
/// finite on any input.
const MAX_FETCH_ROWS: i64 = 1000;

// ── the closed sort vocabulary ───────────────────────────────────────────────

/// The listing sort's closed value set (§4.3) — validated at parse,
/// mapped to fixed comparators, never a formatted string into SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKind {
    Relevance,
    Newest,
    PriceAsc,
    PriceDesc,
    NameAsc,
}

impl SortKind {
    /// Parse the closed vocabulary; anything else is the typed 422 at
    /// the route (this parser answers `None`, the route refuses).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "relevance" => Some(SortKind::Relevance),
            "newest" => Some(SortKind::Newest),
            "price_asc" => Some(SortKind::PriceAsc),
            "price_desc" => Some(SortKind::PriceDesc),
            "name_asc" => Some(SortKind::NameAsc),
            _ => None,
        }
    }
}

// ── public reads ─────────────────────────────────────────────────────────────

/// One published listing as the public read serves it.
#[derive(Debug, Clone)]
pub struct PublicListing {
    pub listing_id: Uuid,
    pub item_id: Uuid,
    pub name: String,
    pub sequence: i32,
    pub media_urls: serde_json::Value,
    pub list_price: Decimal,
    pub compare_at_price: Option<Decimal>,
    pub currency: String,
    /// The listing row's creation stamp — the `newest` sort arm's key
    /// (and an honest fact for the response).
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// One gated row's SQL facts (the gate's own columns only — the name
/// joins in through the catalog port, never a selling read).
#[derive(Debug, Clone, sqlx::FromRow)]
struct GatedRow {
    listing_id: Uuid,
    item_id: Uuid,
    sequence: i32,
    media_urls: serde_json::Value,
    list_price: Decimal,
    compare_at_price: Option<Decimal>,
    currency: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

const GATED_ROWS_SQL: &str = r#"
    SELECT l.id AS listing_id, l.item_id, l.sequence, l.media_urls,
           p.list_price, p.compare_at_price, p.currency,
           (l.metadata->>'created_at')::timestamptz AS created_at
    FROM storefront.product_listings l
    JOIN storefront.product_prices p
      ON p.website_id = l.website_id AND p.item_id = l.item_id
     AND (p.metadata->>'deleted_at') IS NULL
    WHERE l.website_id = $1
      AND l.sale_ok = true
      AND l.is_published = true
      AND (l.metadata->>'deleted_at') IS NULL
    LIMIT $2
"#;

/// The publish-gated listing (§4.1 gate in the SQL; active-item check
/// through the port; the closed sort vocabulary; Rust-side paging).
/// Unpublished / other-website / `sale_ok=false` / no-live-price rows
/// are structurally absent — every closed door is the same absence.
pub async fn public_listings(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    catalog: &dyn CatalogReadPort,
    company_id: Uuid,
    website_id: Uuid,
    q: Option<&str>,
    sort: SortKind,
    page: i64,
    page_size: i64,
) -> Result<Vec<PublicListing>, StorefrontError> {
    let rows: Vec<GatedRow> =
        sqlx::query_as::<_, GatedRow>(GATED_ROWS_SQL)
            .bind(website_id)
            .bind(MAX_FETCH_ROWS)
            .fetch_all(exec)
            .await?;
    let item_ids: Vec<Uuid> = rows.iter().map(|r| r.item_id).collect();
    let snapshots = catalog
        .item_snapshots(company_id, &item_ids)
        .await
        .map_err(|e| StorefrontError::CatalogPortRefused { code: e.code })?;
    let names: std::collections::HashMap<Uuid, String> = snapshots
        .into_iter()
        .filter(|s| s.is_active())
        .map(|s| (s.item_id, s.name))
        .collect();

    let needle = q.map(|s| s.trim().to_lowercase());
    let mut out: Vec<PublicListing> = rows
        .into_iter()
        .filter_map(|r| {
            // The active-item arm of the gate (port-checked).
            let name = names.get(&r.item_id)?.clone();
            if let Some(n) = &needle {
                if !name.to_lowercase().contains(n.as_str()) {
                    return None;
                }
            }
            Some(PublicListing {
                listing_id: r.listing_id,
                item_id: r.item_id,
                name,
                sequence: r.sequence,
                media_urls: r.media_urls,
                list_price: r.list_price,
                compare_at_price: r.compare_at_price,
                currency: r.currency,
                created_at: r.created_at,
            })
        })
        .collect();

    // Each arm a FIXED comparator pair (the closed vocabulary's point);
    // the id tiebreak makes every ordering deterministic.
    let oldest = chrono::DateTime::<chrono::Utc>::MIN_UTC;
    out.sort_by(|a, b| match sort {
        SortKind::Relevance => (a.sequence, a.item_id).cmp(&(b.sequence, b.item_id)),
        SortKind::NameAsc => {
            (a.name.to_lowercase(), a.item_id).cmp(&(b.name.to_lowercase(), b.item_id))
        }
        SortKind::PriceAsc => (a.list_price, a.item_id).cmp(&(b.list_price, b.item_id)),
        SortKind::PriceDesc => (b.list_price, a.item_id).cmp(&(a.list_price, b.item_id)),
        SortKind::Newest => (
            b.created_at.unwrap_or(oldest),
            b.item_id,
        )
            .cmp(&(a.created_at.unwrap_or(oldest), a.item_id)),
    });

    let page = page.max(1);
    let page_size = page_size.clamp(1, MAX_PAGE_SIZE);
    let start = ((page - 1) * page_size) as usize;
    Ok(out.into_iter().skip(start).take(page_size as usize).collect())
}

/// The publish-gated product detail: the §4.1 gate plus the price row
/// and media refs; every closed-door shape — unpublished,
/// other-website, `sale_ok=false`, inactive item, no live price row —
/// answers the SAME typed 404 (no door-probing oracle).
pub async fn public_detail(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    catalog: &dyn CatalogReadPort,
    company_id: Uuid,
    website_id: Uuid,
    item_id: Uuid,
) -> Result<PublicListing, StorefrontError> {
    let row: Option<GatedRow> = sqlx::query_as::<_, GatedRow>(
        r#"
        SELECT l.id AS listing_id, l.item_id, l.sequence, l.media_urls,
               p.list_price, p.compare_at_price, p.currency,
               (l.metadata->>'created_at')::timestamptz AS created_at
        FROM storefront.product_listings l
        JOIN storefront.product_prices p
          ON p.website_id = l.website_id AND p.item_id = l.item_id
         AND (p.metadata->>'deleted_at') IS NULL
        WHERE l.website_id = $1 AND l.item_id = $2
          AND l.sale_ok = true AND l.is_published = true
          AND (l.metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .fetch_optional(exec)
    .await?;
    let row = row.ok_or(StorefrontError::PublishGateRefused)?;
    let snapshot = catalog
        .item_snapshot(company_id, item_id)
        .await
        .map_err(|e| StorefrontError::CatalogPortRefused { code: e.code })?
        .ok_or(StorefrontError::PublishGateRefused)?;
    if !snapshot.is_active() {
        return Err(StorefrontError::PublishGateRefused);
    }
    Ok(PublicListing {
        listing_id: row.listing_id,
        item_id: row.item_id,
        name: snapshot.name,
        sequence: row.sequence,
        media_urls: row.media_urls,
        list_price: row.list_price,
        compare_at_price: row.compare_at_price,
        currency: row.currency,
        created_at: row.created_at,
    })
}

/// One derived category: the catalog group's id and display name.
#[derive(Debug, Clone)]
pub struct DerivedCategory {
    pub group_id: Uuid,
    pub name: String,
}

/// The category tree derived FRESH from the PUBLISHED listings' catalog
/// groups (§6.1): no stored category state exists anywhere in this
/// module — every read recomputes from the gate's survivors. The
/// catalog port carries no parent dimension, so the P1 grain is the
/// flat ordered group list (a hierarchy lands with a port dimension,
/// not a stored table).
pub async fn public_categories(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    catalog: &dyn CatalogReadPort,
    company_id: Uuid,
    website_id: Uuid,
) -> Result<Vec<DerivedCategory>, StorefrontError> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT l.item_id
        FROM storefront.product_listings l
        JOIN storefront.product_prices p
          ON p.website_id = l.website_id AND p.item_id = l.item_id
         AND (p.metadata->>'deleted_at') IS NULL
        WHERE l.website_id = $1
          AND l.sale_ok = true AND l.is_published = true
          AND (l.metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(website_id)
    .fetch_all(exec)
    .await?;
    let item_ids: Vec<Uuid> = rows.into_iter().map(|r| r.0).collect();
    let snapshots = catalog
        .item_snapshots(company_id, &item_ids)
        .await
        .map_err(|e| StorefrontError::CatalogPortRefused { code: e.code })?;
    let mut seen: Vec<DerivedCategory> = Vec::new();
    for s in snapshots {
        if !s.is_active() {
            continue;
        }
        if let Some(gid) = s.item_group_id {
            if !seen.iter().any(|c| c.group_id == gid) {
                seen.push(DerivedCategory {
                    group_id: gid,
                    name: s.item_group_name.clone().unwrap_or_else(|| gid.to_string()),
                });
            }
        }
    }
    seen.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(seen)
}

// ── officer reads ────────────────────────────────────────────────────────────

/// One listing row as the officer read serves it (ALL states — the
/// officer sees the unpublished and unsaleable rows too).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminListingRow {
    pub id: Uuid,
    pub website_id: Uuid,
    pub item_id: Uuid,
    pub sale_ok: bool,
    pub is_published: bool,
    pub sequence: i32,
    pub media_urls: serde_json::Value,
    pub list_price: Option<Decimal>,
    pub compare_at_price: Option<Decimal>,
    pub currency: Option<String>,
}

/// The officer listing read: every listing row (all states) across the
/// COMPANY's websites (company scope via the website pairing), each
/// with its price row when one exists.
pub async fn admin_listings(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    company_id: Uuid,
) -> Result<Vec<AdminListingRow>, StorefrontError> {
    sqlx::query_as::<_, AdminListingRow>(
        r#"
        SELECT l.id, l.website_id, l.item_id, l.sale_ok, l.is_published,
               l.sequence, l.media_urls,
               p.list_price, p.compare_at_price, p.currency
        FROM storefront.product_listings l
        JOIN website.websites w
          ON w.id = l.website_id AND (w.metadata->>'deleted_at') IS NULL
        LEFT JOIN storefront.product_prices p
          ON p.website_id = l.website_id AND p.item_id = l.item_id
         AND (p.metadata->>'deleted_at') IS NULL
        WHERE w.company_id = $1 AND (l.metadata->>'deleted_at') IS NULL
        ORDER BY l.website_id, l.sequence, l.id
        "#,
    )
    .bind(company_id)
    .fetch_all(exec)
    .await
    .map_err(StorefrontError::from)
}

// ── officer verbs ────────────────────────────────────────────────────────────

/// Reject `data:` URIs anywhere in the media list (EC-22: the smuggle
/// vector dies at write; the array shape itself is the officer's to
/// order — strings only, every string must be a non-data URL).
fn validate_media_urls(media: &serde_json::Value) -> Result<(), StorefrontError> {
    let serde_json::Value::Array(entries) = media else {
        return Err(StorefrontError::InvalidInput(
            "media_urls must be a JSON array of URL strings".into(),
        ));
    };
    for entry in entries {
        let serde_json::Value::String(s) = entry else {
            return Err(StorefrontError::InvalidInput(
                "media_urls entries must be strings".into(),
            ));
        };
        let folded = s.trim().to_lowercase();
        if folded.starts_with("data:") {
            return Err(StorefrontError::DataUriRefused);
        }
        if s.trim().is_empty() {
            return Err(StorefrontError::InvalidInput(
                "media_urls entries must be non-empty".into(),
            ));
        }
    }
    Ok(())
}

/// The listing upsert (create-or-update on the website/item pairing).
/// `is_published` is NOT a parameter of this verb by construction —
/// only the publish/unpublish verbs write it; the route refuses a
/// patch body carrying it with the typed 422 + `publish_refused`
/// audit row before this function runs.
pub async fn upsert_listing(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    item_id: Uuid,
    sale_ok: bool,
    sequence: i32,
    media_urls: serde_json::Value,
    actor: ActorRef,
) -> Result<Uuid, StorefrontError> {
    validate_media_urls(&media_urls)?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM storefront.product_listings
        WHERE website_id = $1 AND item_id = $2
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let listing_id = match existing {
        Some((id,)) => {
            sqlx::query(
                r#"
                UPDATE storefront.product_listings
                SET sale_ok = $3, sequence = $4, media_urls = $5,
                    metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
                WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
                "#,
            )
            .bind(id)
            .bind(website_id)
            .bind(sale_ok)
            .bind(sequence)
            .bind(&media_urls)
            .execute(pool)
            .await?;
            id
        }
        None => {
            let row: (Uuid,) = sqlx::query_as(
                r#"
                INSERT INTO storefront.product_listings
                    (id, website_id, item_id, sale_ok, is_published, sequence, media_urls)
                VALUES (gen_random_uuid(), $1, $2, $3, false, $4, $5)
                RETURNING id
                "#,
            )
            .bind(website_id)
            .bind(item_id)
            .bind(sale_ok)
            .bind(sequence)
            .bind(&media_urls)
            .fetch_one(pool)
            .await?;
            row.0
        }
    };
    record_audit(
        pool,
        Some(website_id),
        "listing_upserted",
        actor,
        Some("product_listing"),
        Some(listing_id),
        Some(serde_json::json!({ "sale_ok": sale_ok, "sequence": sequence })),
    )
    .await?;
    Ok(listing_id)
}

/// THE publish verb — one of the only two writers of `is_published`.
/// Publishing without a live price row succeeds (the row is writable
/// ahead of merchandising); the READ gate still withholds visibility
/// until a price row exists — the gate is conjunctive by contract.
pub async fn publish_listing(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    listing_id: Uuid,
    actor: ActorRef,
) -> Result<(), StorefrontError> {
    let stamped = sqlx::query(
        r#"
        UPDATE storefront.product_listings
        SET is_published = true,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
          AND is_published = false
        "#,
    )
    .bind(listing_id)
    .execute(pool)
    .await?;
    if stamped.rows_affected() == 0 {
        // Missing or already published — the typed 404 keeps the
        // closed-door posture (no existence probe).
        return Err(StorefrontError::NotFound("listing".into()));
    }
    record_audit(
        pool,
        Some(website_id),
        "listing_published",
        actor,
        Some("product_listing"),
        Some(listing_id),
        None,
    )
    .await?;
    Ok(())
}

/// THE unpublish verb — the other `is_published` writer.
pub async fn unpublish_listing(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    listing_id: Uuid,
    actor: ActorRef,
) -> Result<(), StorefrontError> {
    let stamped = sqlx::query(
        r#"
        UPDATE storefront.product_listings
        SET is_published = false,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
          AND is_published = true
        "#,
    )
    .bind(listing_id)
    .execute(pool)
    .await?;
    if stamped.rows_affected() == 0 {
        return Err(StorefrontError::NotFound("listing".into()));
    }
    record_audit(
        pool,
        Some(website_id),
        "listing_unpublished",
        actor,
        Some("product_listing"),
        Some(listing_id),
        None,
    )
    .await?;
    Ok(())
}

/// Set the per-website price row (upsert on the website/item pairing).
pub async fn set_price(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    item_id: Uuid,
    list_price: Decimal,
    compare_at_price: Option<Decimal>,
    currency: &str,
    actor: ActorRef,
) -> Result<Uuid, StorefrontError> {
    if list_price < Decimal::ZERO {
        return Err(StorefrontError::InvalidInput(
            "list_price must be zero or positive".into(),
        ));
    }
    let currency = currency.trim().to_uppercase();
    if currency.len() < 3 || currency.len() > 3 {
        return Err(StorefrontError::InvalidInput(
            "currency must be a 3-letter code".into(),
        ));
    }
    let existing: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM storefront.product_prices
        WHERE website_id = $1 AND item_id = $2
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let price_id = match existing {
        Some((id,)) => {
            sqlx::query(
                r#"
                UPDATE storefront.product_prices
                SET list_price = $3, compare_at_price = $4, currency = $5,
                    metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
                WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
                "#,
            )
            .bind(id)
            .bind(website_id)
            .bind(list_price)
            .bind(compare_at_price)
            .bind(&currency)
            .execute(pool)
            .await?;
            id
        }
        None => {
            let row: (Uuid,) = sqlx::query_as(
                r#"
                INSERT INTO storefront.product_prices
                    (id, website_id, item_id, list_price, compare_at_price, currency)
                VALUES (gen_random_uuid(), $1, $2, $3, $4, $5)
                RETURNING id
                "#,
            )
            .bind(website_id)
            .bind(item_id)
            .bind(list_price)
            .bind(compare_at_price)
            .bind(&currency)
            .fetch_one(pool)
            .await?;
            row.0
        }
    };
    record_audit(
        pool,
        Some(website_id),
        "price_set",
        actor,
        Some("product_price"),
        Some(price_id),
        Some(serde_json::json!({ "list_price": list_price, "currency": currency })),
    )
    .await?;
    Ok(price_id)
}

/// The sale-settings arm the settings verb writes.
pub struct SettingsPatch {
    pub access_gate: String,
    pub default_customer_group_id: Option<Uuid>,
    pub recovery_template_ref: Option<String>,
}

/// Set the website's sale settings (one row per website — the grain IS
/// the fence; no every-website fan-out exists). The FIRST set
/// bootstraps the designated public guest party through the party
/// port; later sets reuse the stored id (the guest party is the
/// ORDER's customer arm for never-billed carts, never a pricing
/// dimension — §5.2).
pub async fn set_settings(
    pool: &sqlx::PgPool,
    party_port: &dyn PartyWritePort,
    website_id: Uuid,
    patch: SettingsPatch,
    actor: ActorRef,
) -> Result<Uuid, StorefrontError> {
    if !matches!(patch.access_gate.as_str(), "open" | "members_only") {
        return Err(StorefrontError::InvalidInput(
            "access_gate must be 'open' or 'members_only'".into(),
        ));
    }
    // The website's company scopes the party mint.
    let (company_id,): (Uuid,) = sqlx::query_as(
        r#"
        SELECT company_id
        FROM website.websites
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(website_id)
    .fetch_one(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::RowNotFound => StorefrontError::WebsiteNotFound,
        other => StorefrontError::Db(other),
    })?;
    let existing = settings_for(pool, website_id).await?;
    let settings_id = match existing {
        Some(row) => {
            sqlx::query(
                r#"
                UPDATE storefront.website_sale_settings
                SET access_gate = $2::storefront_access_gate,
                    default_customer_group_id = $3,
                    recovery_template_ref = $4,
                    metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
                WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
                "#,
            )
            .bind(row.id)
            .bind(&patch.access_gate)
            .bind(patch.default_customer_group_id)
            .bind(&patch.recovery_template_ref)
            .execute(pool)
            .await?;
            row.id
        }
        None => {
            let guest_party_id = party_port
                .mint_guest_party(company_id)
                .await
                .map_err(|e| StorefrontError::PartyPortRefused { code: e.code })?;
            let row: (Uuid,) = sqlx::query_as(
                r#"
                INSERT INTO storefront.website_sale_settings
                    (id, website_id, access_gate, default_customer_group_id,
                     guest_party_id, recovery_template_ref)
                VALUES (gen_random_uuid(), $1, $2::storefront_access_gate, $3, $4, $5)
                RETURNING id
                "#,
            )
            .bind(website_id)
            .bind(&patch.access_gate)
            .bind(patch.default_customer_group_id)
            .bind(guest_party_id)
            .bind(&patch.recovery_template_ref)
            .fetch_one(pool)
            .await?;
            row.0
        }
    };
    record_audit(
        pool,
        Some(website_id),
        "settings_set",
        actor,
        Some("website_sale_settings"),
        Some(settings_id),
        Some(serde_json::json!({ "access_gate": patch.access_gate })),
    )
    .await?;
    Ok(settings_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_vocabulary_is_closed() {
        assert_eq!(SortKind::parse("relevance"), Some(SortKind::Relevance));
        assert_eq!(SortKind::parse("NEWEST"), Some(SortKind::Newest));
        assert_eq!(SortKind::parse("price_asc"), Some(SortKind::PriceAsc));
        assert_eq!(SortKind::parse("price_desc"), Some(SortKind::PriceDesc));
        assert_eq!(SortKind::parse(" name_asc "), Some(SortKind::NameAsc));
        // Everything outside the closed set refuses — including the
        // SQL-injection-shaped strings the anti-spec family probes.
        for probe in [
            "", "random", "updated_at; DROP TABLE x", "price", "name_desc",
            "1; SELECT pg_sleep(10)",
        ] {
            assert_eq!(SortKind::parse(probe), None, "probe {probe:?} must refuse");
        }
    }

    #[test]
    fn data_uris_refuse_in_any_position() {
        let media = serde_json::json!(["https://cdn.example.test/a.jpg", "data:text/html,x"]);
        assert!(matches!(
            validate_media_urls(&media),
            Err(StorefrontError::DataUriRefused)
        ));
        let ok = serde_json::json!(["https://cdn.example.test/a.jpg"]);
        assert!(validate_media_urls(&ok).is_ok());
        let not_array = serde_json::json!({"url": "data:text/html,x"});
        assert!(matches!(
            validate_media_urls(&not_array),
            Err(StorefrontError::InvalidInput(_))
        ));
    }
}
