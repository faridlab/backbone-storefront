//! The cart verbs (hand-written; user-owned; see `metaphor.codegen.yaml`).
//!
//! The generated CRUD alias first (the module's mod tree re-exports
//! `CartService`; the alias keeps that compile), then the hand verbs.
//!
//! The identity spine (§2 of the module spec):
//!
//! - **Deterministic create** — `INSERT .. ON CONFLICT DO NOTHING` with
//!   the partial unique `UNIQUE(visitor_id) WHERE state='open'` as the
//!   arbiter, then the surviving row is selected. Concurrent creates
//!   race to exactly one winner; no check-then-act anywhere.
//! - **Deterministic adopt** — the bind verb ANSWERS the principal's
//!   most-recent-open-cart query (explicit total order) and mutates
//!   nothing; `adopt` is the only mover, re-binding lineage and
//!   stamping `portal_user_id`, refusing with the typed 409 when the
//!   visitor already holds an open cart. Foreign carts are never
//!   returned, never adoptable.
//! - **No stored prices** — a line is (item, quantity); every priced
//!   read goes through the pricing service (§5). Line mutations
//!   re-check the publish gate AT MUTATION TIME with typed refusals.
//! - **The visitor token is read-only identity** — the storefront never
//!   mints visitor rows; `visitor_by_token` is a read on the website
//!   schema's own table (the logical-ref posture: uuid read, no FK).
//!
//! This file holds the cart-table verbs; the checkout critical sections
//! (row lock, place, settlement) live in `checkout_service.rs`.

use uuid::Uuid;

use backbone_core::GenericCrudService;
use crate::domain::entity::Cart;
use crate::infrastructure::persistence::CartRepository;
use crate::presentation::dto::{CreateCartDto, UpdateCartDto};

/// Generated CRUD alias (the generator skipped emitting this file
/// because it is user-owned; the alias keeps mod.rs's re-export
/// compiling unchanged).
pub type CartService = GenericCrudService<
    Cart,
    CreateCartDto,
    UpdateCartDto,
    CartRepository,
>;

use super::audit::{record_audit, ActorRef};
use super::availability_port::AvailabilityReadPort;
use super::availability_service;
use super::catalog_read_port::{CatalogReadPort, ItemSnapshot};
use super::party_write_port::PartyWritePort;
use super::storefront_error::StorefrontError;

/// The per-cart line bound's env knob (default 100). Read per call so
/// probes see a deterministic value without process-global state.
pub const MAX_CART_LINES_ENV: &str = "STOREFRONT_MAX_CART_LINES";

/// The default per-cart line bound when the env knob is unset or
/// unparseable (an unparseable value falls back, never wedges the
/// verb).
pub const DEFAULT_MAX_CART_LINES: i64 = 100;

/// The per-cart line bound in effect.
pub fn max_cart_lines() -> i64 {
    std::env::var(MAX_CART_LINES_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_CART_LINES)
}

/// Normalize a shopper email: trim + lowercase (the deterministic map
/// key's shape — every resolution and lookup runs on this form only).
pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_lowercase()
}

// ── row views ───────────────────────────────────────────────────────────────

/// One cart row as the verbs read it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CartRow {
    pub id: Uuid,
    pub website_id: Uuid,
    pub visitor_id: Uuid,
    pub portal_user_id: Option<Uuid>,
    pub party_id: Option<Uuid>,
    pub state: String,
    pub coupon_code: Option<String>,
    pub delivery_carrier_id: Option<Uuid>,
    pub fulfillment_mode: String,
    pub pickup_location_id: Option<Uuid>,
    pub placed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// One cart line row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CartLineRow {
    pub id: Uuid,
    pub cart_id: Uuid,
    pub item_id: Uuid,
    pub quantity: rust_decimal::Decimal,
}

// ── identity ────────────────────────────────────────────────────────────────

/// Resolve a presented visitor token to its visitor row id on this
/// website. READ ONLY — the storefront never mints visitors (the typed
/// 401 on miss, never a silent create). The token is the ONLY
/// client-held secret on the session arm.
pub async fn visitor_by_token(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
    token: &str,
) -> Result<Option<Uuid>, StorefrontError> {
    if token.is_empty() {
        return Ok(None);
    }
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id
        FROM website.visitors
        WHERE access_token = $1 AND website_id = $2
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(token)
    .bind(website_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|r| r.0))
}

// ── single-row reads ────────────────────────────────────────────────────────

pub(crate) const CART_SELECT: &str = r#"
    SELECT id, website_id, visitor_id, portal_user_id, party_id,
           state::text AS state, coupon_code, delivery_carrier_id,
           fulfillment_mode, pickup_location_id, placed_at
    FROM storefront.carts
"#;

/// The visitor's open cart (at most one — the partial unique's read
/// arm). Ordered deterministically so even a hypothetical duplicate
/// reads the same winner on every call.
pub async fn open_cart_for_visitor(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    visitor_id: Uuid,
) -> Result<Option<CartRow>, StorefrontError> {
    sqlx::query_as::<_, CartRow>(&format!(
        "{CART_SELECT} WHERE visitor_id = $1 AND state = 'open' \
         AND (metadata->>'deleted_at') IS NULL \
         ORDER BY (metadata->>'updated_at') DESC, id DESC LIMIT 1"
    ))
    .bind(visitor_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// One cart by id (any state) — the ownership checks' read.
pub async fn cart_by_id(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    cart_id: Uuid,
) -> Result<Option<CartRow>, StorefrontError> {
    sqlx::query_as::<_, CartRow>(&format!(
        "{CART_SELECT} WHERE id = $1 AND (metadata->>'deleted_at') IS NULL"
    ))
    .bind(cart_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The principal's most recent open cart — the bind query's ONE row,
/// explicit total order (`updated_at DESC, id DESC`); the
/// `portal_user_id` predicate is the ownership fence (a different
/// identity's cart is never returned).
pub async fn most_recent_open_cart_for_principal(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    portal_user_id: Uuid,
) -> Result<Option<CartRow>, StorefrontError> {
    sqlx::query_as::<_, CartRow>(&format!(
        "{CART_SELECT} WHERE portal_user_id = $1 AND state = 'open' \
         AND (metadata->>'deleted_at') IS NULL \
         ORDER BY (metadata->>'updated_at') DESC, id DESC LIMIT 1"
    ))
    .bind(portal_user_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The visitor's most recent cart in ANY state — the locked delivery
/// verb's fallback arm: when no open cart exists, a placed one still has
/// to reach the in-lock state gate so the caller reads the typed not-open
/// refusal (§7.1(b)'s closed-window proof) instead of a bare 404 that
/// would hide the closed window.
pub async fn latest_cart_for_visitor(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    visitor_id: Uuid,
) -> Result<Option<CartRow>, StorefrontError> {
    sqlx::query_as::<_, CartRow>(&format!(
        "{CART_SELECT} WHERE visitor_id = $1 \
         AND (metadata->>'deleted_at') IS NULL \
         ORDER BY (metadata->>'updated_at') DESC, id DESC LIMIT 1"
    ))
    .bind(visitor_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The principal-arm twin of [`latest_cart_for_visitor`].
pub async fn latest_cart_for_principal(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    portal_user_id: Uuid,
) -> Result<Option<CartRow>, StorefrontError> {
    sqlx::query_as::<_, CartRow>(&format!(
        "{CART_SELECT} WHERE portal_user_id = $1 \
         AND (metadata->>'deleted_at') IS NULL \
         ORDER BY (metadata->>'updated_at') DESC, id DESC LIMIT 1"
    ))
    .bind(portal_user_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The cart's lines, ordered by insertion (deterministic reads).
pub async fn lines_of(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    cart_id: Uuid,
) -> Result<Vec<CartLineRow>, StorefrontError> {
    sqlx::query_as::<_, CartLineRow>(
        r#"
        SELECT id, cart_id, item_id, quantity
        FROM storefront.cart_lines
        WHERE cart_id = $1 AND (metadata->>'deleted_at') IS NULL
        ORDER BY (metadata->>'created_at') ASC, id ASC
        "#,
    )
    .bind(cart_id)
    .fetch_all(exec)
    .await
    .map_err(StorefrontError::from)
}

// ── deterministic create ────────────────────────────────────────────────────

/// The create outcome: the surviving open cart and whether THIS call is
/// the one that created it (the audit stamps `cart_created` only on the
/// winner — a losing racer reads `created = false` and the same row).
pub struct CreatedCart {
    pub cart: CartRow,
    pub created: bool,
}

/// Create the visitor's open cart — deterministic under concurrency:
/// the insert lands `ON CONFLICT DO NOTHING` (the partial unique
/// `UNIQUE(visitor_id) WHERE state='open'` is the arbiter) and the
/// surviving row is then selected. Exactly one winner, ever.
pub async fn create_cart(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    visitor_id: Uuid,
) -> Result<CreatedCart, StorefrontError> {
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO storefront.carts (id, website_id, visitor_id, state)
        VALUES (gen_random_uuid(), $1, $2, 'open')
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(website_id)
    .bind(visitor_id)
    .fetch_optional(pool)
    .await?;
    let created = inserted.is_some();
    if let Some((cart_id,)) = inserted {
        record_audit(
            pool,
            Some(website_id),
            "cart_created",
            ActorRef::visitor(visitor_id),
            Some("cart"),
            Some(cart_id),
            None,
        )
        .await?;
    }
    let cart = open_cart_for_visitor(pool, visitor_id)
        .await?
        .ok_or(StorefrontError::Internal("open cart vanished after create".into()))?;
    Ok(CreatedCart { cart, created })
}

// ── the publish gate (mutation-time arm) ────────────────────────────────────

/// The mutation-time gate's facts — everything the pricing mapping
/// needs for one merchandised item.
pub struct GatedListing {
    pub listing_id: Uuid,
    pub item_id: Uuid,
    pub sequence: i32,
    pub media_urls: serde_json::Value,
    pub list_price: rust_decimal::Decimal,
    pub currency: String,
    pub snapshot: ItemSnapshot,
}

/// Check the publish gate for one (website, item): a LIVE listing row
/// with `sale_ok` AND `is_published`, a LIVE price row, and an ACTIVE
/// catalog item (port read). Every closed-door shape — unpublished,
/// no listing on this website, `sale_ok=false`, inactive or missing
/// catalog item, no live price row — answers the same typed refusal.
pub async fn gated_listing(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    catalog: &dyn CatalogReadPort,
    company_id: Uuid,
    website_id: Uuid,
    item_id: Uuid,
) -> Result<GatedListing, StorefrontError> {
    let listing: Option<(Uuid, i32, serde_json::Value, rust_decimal::Decimal, String)> =
        sqlx::query_as(
            r#"
            SELECT l.id, l.sequence, l.media_urls, p.list_price, p.currency
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
    let (listing_id, sequence, media_urls, list_price, currency) =
        listing.ok_or(StorefrontError::PublishGateRefused)?;
    let snapshot = catalog
        .item_snapshot(company_id, item_id)
        .await
        .map_err(|e| StorefrontError::CatalogPortRefused { code: e.code })?
        .ok_or(StorefrontError::PublishGateRefused)?;
    if !snapshot.is_active() {
        return Err(StorefrontError::PublishGateRefused);
    }
    Ok(GatedListing {
        listing_id,
        item_id,
        sequence,
        media_urls,
        list_price,
        currency,
        snapshot,
    })
}

// ── line verbs ──────────────────────────────────────────────────────────────

/// Add a line (same-item adds FOLD onto the existing line). The publish
/// gate AND the stock clamp re-check HERE, at mutation time; a closed
/// door or a short warehouse refuses the add, never silently drops or
/// clamps it.
pub async fn add_line(
    pool: &sqlx::PgPool,
    catalog: &dyn CatalogReadPort,
    availability: &dyn AvailabilityReadPort,
    company_id: Uuid,
    cart: &CartRow,
    item_id: Uuid,
    quantity: rust_decimal::Decimal,
) -> Result<CartLineRow, StorefrontError> {
    if quantity <= rust_decimal::Decimal::ZERO {
        return Err(StorefrontError::InvalidQuantity);
    }
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    // The gate first — a closed-door item never touches the cart.
    gated_listing(pool, catalog, company_id, cart.website_id, item_id).await?;
    let existing = lines_of(pool, cart.id).await?;
    if let Some(line) = existing.iter().find(|l| l.item_id == item_id) {
        // Fold: a repeat item grows its existing line.
        let next = line.quantity + quantity;
        return set_line_quantity(pool, catalog, availability, company_id, cart, line.id, next)
            .await;
    }
    if (existing.len() as i64) >= max_cart_lines() {
        return Err(StorefrontError::LineLimitExceeded);
    }
    // The stock clamp on the RESULTING quantity (checkout scope,
    // computed fresh; a backorder-allowed listing skips it).
    let mut clamp_conn = pool.acquire().await?;
    availability_service::clamp_quantity(
        &mut clamp_conn,
        availability,
        company_id,
        cart,
        item_id,
        quantity,
    )
    .await?;
    drop(clamp_conn);
    let row: (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO storefront.cart_lines (id, cart_id, item_id, quantity)
        VALUES (gen_random_uuid(), $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(cart.id)
    .bind(item_id)
    .bind(quantity)
    .fetch_one(pool)
    .await?;
    // Touch the cart's updated_at — the abandonment clock rides it.
    touch_cart(pool, cart.id).await?;
    record_audit(
        pool,
        Some(cart.website_id),
        "line_added",
        ActorRef::visitor(cart.visitor_id),
        Some("cart_line"),
        Some(row.0),
        Some(serde_json::json!({ "item_id": item_id, "quantity": quantity })),
    )
    .await?;
    Ok(CartLineRow { id: row.0, cart_id: cart.id, item_id, quantity })
}

/// Set a line's quantity (positive decimal only). The publish gate AND
/// the stock clamp re-check at mutation time — an item that left the
/// gate or ran past the warehouse's free quantity since the add refuses
/// the SET, never silently unlinks the line.
pub async fn set_line_quantity(
    pool: &sqlx::PgPool,
    catalog: &dyn CatalogReadPort,
    availability: &dyn AvailabilityReadPort,
    company_id: Uuid,
    cart: &CartRow,
    line_id: Uuid,
    quantity: rust_decimal::Decimal,
) -> Result<CartLineRow, StorefrontError> {
    if quantity <= rust_decimal::Decimal::ZERO {
        return Err(StorefrontError::InvalidQuantity);
    }
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    let line = lines_of(pool, cart.id)
        .await?
        .into_iter()
        .find(|l| l.id == line_id)
        .ok_or(StorefrontError::LineNotFound)?;
    // Mutation-time gate re-check on this line's item.
    gated_listing(pool, catalog, company_id, cart.website_id, line.item_id).await?;
    // The stock clamp on the requested quantity (checkout scope,
    // computed fresh; a backorder-allowed listing skips it).
    let mut clamp_conn = pool.acquire().await?;
    availability_service::clamp_quantity(
        &mut clamp_conn,
        availability,
        company_id,
        cart,
        line.item_id,
        quantity,
    )
    .await?;
    drop(clamp_conn);
    sqlx::query(
        r#"
        UPDATE storefront.cart_lines
        SET quantity = $3,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND cart_id = $2 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(line_id)
    .bind(cart.id)
    .bind(quantity)
    .execute(pool)
    .await?;
    touch_cart(pool, cart.id).await?;
    record_audit(
        pool,
        Some(cart.website_id),
        "line_updated",
        ActorRef::visitor(cart.visitor_id),
        Some("cart_line"),
        Some(line_id),
        Some(serde_json::json!({ "quantity": quantity })),
    )
    .await?;
    Ok(CartLineRow { id: line_id, cart_id: cart.id, item_id: line.item_id, quantity })
}

/// Remove a line (soft delete — the audit trail keeps the row).
pub async fn remove_line(
    pool: &sqlx::PgPool,
    cart: &CartRow,
    line_id: Uuid,
) -> Result<(), StorefrontError> {
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    let outcome = sqlx::query(
        r#"
        UPDATE storefront.cart_lines
        SET metadata = jsonb_set(metadata, '{deleted_at}', to_jsonb(now()))
        WHERE id = $1 AND cart_id = $2 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(line_id)
    .bind(cart.id)
    .execute(pool)
    .await?;
    if outcome.rows_affected() == 0 {
        return Err(StorefrontError::LineNotFound);
    }
    touch_cart(pool, cart.id).await?;
    record_audit(
        pool,
        Some(cart.website_id),
        "line_removed",
        ActorRef::visitor(cart.visitor_id),
        Some("cart_line"),
        Some(line_id),
        None,
    )
    .await?;
    Ok(())
}

/// Bump the cart's `updated_at` (the abandonment clock's only input —
/// every cart mutation rides through here or the checkout verbs).
pub async fn touch_cart(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    cart_id: Uuid,
) -> Result<(), StorefrontError> {
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1
        "#,
    )
    .bind(cart_id)
    .execute(exec)
    .await?;
    Ok(())
}

// ── coupon ──────────────────────────────────────────────────────────────────

/// Apply a coupon code (the ONLY code surface — this verb). The code is
/// case-folded and format-checked HERE; whether it REDEEMS is promo's
/// verdict at the next pricing pass (uniform refusal text either way —
/// no enumeration oracle).
pub async fn apply_coupon(
    pool: &sqlx::PgPool,
    cart: &CartRow,
    code: &str,
) -> Result<String, StorefrontError> {
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    let folded = code.trim().to_lowercase();
    if folded.len() < 3 || folded.len() > 64 {
        return Err(StorefrontError::CouponRefused);
    }
    // The format fence: codes are [a-z0-9_-] only — anything else
    // (spaces inside, quotes, semicolons, injection shapes) refuses
    // with the same uniform 422, before any storage.
    if !folded.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(StorefrontError::CouponRefused);
    }
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET coupon_code = $2,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'open' AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart.id)
    .bind(&folded)
    .execute(pool)
    .await?;
    record_audit(
        pool,
        Some(cart.website_id),
        "coupon_applied",
        ActorRef::visitor(cart.visitor_id),
        Some("cart"),
        Some(cart.id),
        None,
    )
    .await?;
    Ok(folded)
}

/// Clear the coupon.
pub async fn remove_coupon(pool: &sqlx::PgPool, cart: &CartRow) -> Result<(), StorefrontError> {
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET coupon_code = NULL,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'open' AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart.id)
    .execute(pool)
    .await?;
    record_audit(
        pool,
        Some(cart.website_id),
        "coupon_removed",
        ActorRef::visitor(cart.visitor_id),
        Some("cart"),
        Some(cart.id),
        None,
    )
    .await?;
    Ok(())
}

// ── billing capture (the shopper-party resolve-or-create) ──────────────────

/// The deterministic (company, normalized email) → party resolution.
/// The hardening partial unique `UNIQUE(company_id, email_normalized)`
/// on live rows is the arbiter: the map miss mints through the party
/// port, the insert lands `ON CONFLICT DO NOTHING`, and a lost race
/// re-selects the winner's row. Race-free by construction — two
/// concurrent expresses with one email resolve the SAME map row and
/// mint at most one party. Runs on a CONNECTION the caller holds (the
/// checkout row-lock transaction for the billing and express verbs).
pub async fn resolve_shopper_party(
    conn: &mut sqlx::PgConnection,
    party_port: &dyn PartyWritePort,
    company_id: Uuid,
    email_normalized: &str,
    name: Option<&str>,
) -> Result<Uuid, StorefrontError> {
    // Fast path: the map already binds this shopper.
    let known: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT party_id
        FROM storefront.shopper_parties
        WHERE company_id = $1 AND email_normalized = $2
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(company_id)
    .bind(email_normalized)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((party_id,)) = known {
        return Ok(party_id);
    }
    // Map miss: mint the first-class customer party, then race the
    // insert (another capture may have won in between — the map row
    // wins either way).
    let minted = party_port
        .mint_customer_party(company_id, email_normalized, name)
        .await
        .map_err(|e| StorefrontError::PartyPortRefused { code: e.code })?;
    let raced: Option<(Uuid,)> = sqlx::query_as(
        r#"
        INSERT INTO storefront.shopper_parties (id, company_id, email_normalized, party_id)
        VALUES (gen_random_uuid(), $1, $2, $3)
        ON CONFLICT DO NOTHING
        RETURNING party_id
        "#,
    )
    .bind(company_id)
    .bind(email_normalized)
    .bind(minted)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some((party_id,)) = raced {
        return Ok(party_id);
    }
    // Lost the race: the winner's row IS the binding.
    let winner: (Uuid,) = sqlx::query_as(
        r#"
        SELECT party_id
        FROM storefront.shopper_parties
        WHERE company_id = $1 AND email_normalized = $2
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(company_id)
    .bind(email_normalized)
    .fetch_one(&mut *conn)
    .await?;
    Ok(winner.0)
}

/// Stamp the cart's billing party (inside the caller's lock scope).
pub async fn stamp_billing_party(
    conn: &mut sqlx::PgConnection,
    cart_id: Uuid,
    website_id: Uuid,
    visitor_id: Uuid,
    party_id: Uuid,
) -> Result<(), StorefrontError> {
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET party_id = $2,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'open' AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart_id)
    .bind(party_id)
    .execute(&mut *conn)
    .await?;
    record_audit(
        &mut *conn,
        Some(website_id),
        "billing_set",
        ActorRef::visitor(visitor_id),
        Some("cart"),
        Some(cart_id),
        None,
    )
    .await?;
    Ok(())
}

// ── adopt / bind (the login reconciliation) ─────────────────────────────────

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Standard base64url, no padding (the ticket's encoding — hand-rolled
/// so the module carries no encoding dependency for one informational
/// string).
fn base64url_no_pad(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).map_or(0, |b| *b as u32);
        let b2 = chunk.get(2).map_or(0, |b| *b as u32);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64URL_ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(BASE64URL_ALPHABET[(triple >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL_ALPHABET[(triple >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL_ALPHABET[triple as usize & 63] as char);
        }
    }
    out
}

/// The ownership-proof ticket the bind verb hands the client: a
/// base64url claim over `cart_id:portal_user_id`. INFORMATIONAL ONLY —
/// never a capability: the adopt verb re-verifies everything
/// server-side against the verified principal (the ticket carries no
/// authority to lose or forge).
pub fn adoption_ticket(cart_id: Uuid, portal_user_id: Uuid) -> String {
    let claim = format!("{cart_id}:{portal_user_id}");
    base64url_no_pad(claim.as_bytes())
}

/// The partial unique `idx_carts_open_per_visitor` is the DB's arbiter
/// for "one open cart per visitor": a racing adopt/recover that loses
/// the read-then-update interleaving surfaces as a unique violation on
/// the UPDATE — map it onto the typed 409 the checked path already
/// returns, never an untyped 500.
fn map_open_cart_race(e: sqlx::Error) -> StorefrontError {
    if e.as_database_error().is_some_and(|d| d.is_unique_violation()) {
        StorefrontError::OpenCartExists
    } else {
        StorefrontError::from(e)
    }
}

/// Adopt: the ONLY mover of a principal's open cart onto the current
/// visitor lineage. Refuses with the typed 409 when the visitor already
/// holds an open cart (no silent merge); a cart without THIS
/// principal's linkage is unadoptable (foreign carts never move).
pub async fn adopt_cart(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    visitor_id: Uuid,
    portal_user_id: Uuid,
    cart_id: Uuid,
) -> Result<CartRow, StorefrontError> {
    let target = cart_by_id(pool, cart_id)
        .await?
        .ok_or(StorefrontError::CartNotFound)?;
    if target.portal_user_id != Some(portal_user_id) || target.state != "open" {
        return Err(StorefrontError::CartNotAdoptable);
    }
    if let Some(existing) = open_cart_for_visitor(pool, visitor_id).await? {
        if existing.id != cart_id {
            return Err(StorefrontError::OpenCartExists);
        }
    }
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET visitor_id = $2, portal_user_id = $3,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'open' AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart_id)
    .bind(visitor_id)
    .bind(portal_user_id)
    .execute(pool)
    .await
    .map_err(map_open_cart_race)?;
    record_audit(
        pool,
        Some(website_id),
        "cart_adopted",
        ActorRef::visitor(visitor_id),
        Some("cart"),
        Some(cart_id),
        None,
    )
    .await?;
    cart_by_id(pool, cart_id)
        .await?
        .ok_or(StorefrontError::Internal("adopted cart vanished".into()))
}

// ── recovery re-bind ────────────────────────────────────────────────────────

/// Recover the identity's OWN abandoned cart onto the current session:
/// ownership is checked against the cart's lineage (its visitor or its
/// principal linkage) — a foreign cart answers the closed-door 404.
pub async fn recover_cart(
    pool: &sqlx::PgPool,
    cart_id: Uuid,
    visitor_id: Uuid,
    portal_user_id: Option<Uuid>,
) -> Result<CartRow, StorefrontError> {
    let cart = cart_by_id(pool, cart_id)
        .await?
        .ok_or(StorefrontError::CartNotFound)?;
    let owned = cart.visitor_id == visitor_id
        || (portal_user_id.is_some() && cart.portal_user_id == portal_user_id);
    if !owned {
        // A foreign cart is indistinguishable from a missing one on
        // the public tree.
        return Err(StorefrontError::CartNotFound);
    }
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    if cart.visitor_id == visitor_id {
        return Ok(cart); // already this session's cart — idempotent
    }
    if let Some(existing) = open_cart_for_visitor(pool, visitor_id).await? {
        if existing.id != cart_id {
            return Err(StorefrontError::OpenCartExists);
        }
    }
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET visitor_id = $2,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'open' AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart_id)
    .bind(visitor_id)
    .execute(pool)
    .await
    .map_err(map_open_cart_race)?;
    cart_by_id(pool, cart_id)
        .await?
        .ok_or(StorefrontError::Internal("recovered cart vanished".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_normalization_is_trim_and_lowercase() {
        assert_eq!(normalize_email("  Shopper@Example.COM "), "shopper@example.com");
    }

    #[test]
    fn line_bound_reads_the_knob_with_a_safe_default() {
        // Unset in a clean process; the default must hold either way.
        let bound = max_cart_lines();
        assert!(bound > 0);
        assert_eq!(DEFAULT_MAX_CART_LINES, 100);
    }

    #[test]
    fn adoption_ticket_is_informational_and_deterministic() {
        let cart = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap_or_default();
        let user = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap_or_default();
        assert_eq!(adoption_ticket(cart, user), adoption_ticket(cart, user));
        assert_ne!(adoption_ticket(cart, user), adoption_ticket(user, cart));
    }

    #[test]
    fn base64url_has_no_padding_and_known_vectors() {
        assert_eq!(base64url_no_pad(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url_no_pad(b"foob"), "Zm9vYg");
        assert_eq!(base64url_no_pad(b"fooba"), "Zm9vYmE");
    }
}
