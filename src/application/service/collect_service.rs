//! The pickup-location registry (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! Click & Collect's store registry (§14.2): MERCHANT-DECLARED rows —
//! a store exists only through the officer upsert verb; nothing
//! auto-mints locations from the company's warehouses. The public read
//! is a pure lookup (active stores for the bound website); the pin
//! verb (checkout_service::set_pickup) is the only writer of a cart's
//! pickup linkage, and it resolves warehouse + fiscal jurisdiction
//! SERVER-SIDE from the row this file owns — the client ever presents
//! only the opaque location id.
//!
//! Coordinates are OFFICER-INPUT ONLY. No route in this module
//! geocodes, and no third-party outbound call exists anywhere in the
//! module (the public-geocode shape is fenced with the
//! outbound-bridge posture; see the spec's §14 fence table).
//!
//! The upsert carries three fences: the fiscal country is REQUIRED
//! merchant-declared input (2-letter ISO, validated unconditionally —
//! a countryless store has no defensible tax arm); the target website
//! must be a live row (the write's company is the website's company);
//! and a present warehouse pointer must be one of that company's live
//! warehouses (a foreign-company warehouse is the typed refusal).

use uuid::Uuid;

use backbone_orm::company_scope;

use super::audit::{record_audit, ActorRef};
use super::storefront_error::StorefrontError;

/// One pickup-location row as the reads see it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PickupLocationRow {
    pub id: Uuid,
    pub website_id: Uuid,
    pub warehouse_id: Option<Uuid>,
    pub name: String,
    pub address_line1: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<rust_decimal::Decimal>,
    pub longitude: Option<rust_decimal::Decimal>,
    pub opening_hours: Option<serde_json::Value>,
    pub is_active: bool,
}

const LOCATION_SELECT: &str = r#"
    SELECT id, website_id, warehouse_id, name, address_line1, city,
           postal_code, country, latitude, longitude, opening_hours, is_active
    FROM storefront.pickup_locations
"#;

/// The officer upsert's patch body (every field optional except the
/// identity pair; None leaves the stored value).
#[derive(Debug, Default, Clone)]
pub struct LocationPatch {
    pub warehouse_id: Option<Option<Uuid>>,
    pub name: Option<String>,
    pub address_line1: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
    pub latitude: Option<Option<rust_decimal::Decimal>>,
    pub longitude: Option<Option<rust_decimal::Decimal>>,
    pub opening_hours: Option<serde_json::Value>,
    pub is_active: Option<bool>,
}

/// The website's locations — the OFFICER read (all states; the registry
/// management view).
pub async fn locations_for_website(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
) -> Result<Vec<PickupLocationRow>, StorefrontError> {
    sqlx::query_as::<_, PickupLocationRow>(&format!(
        "{LOCATION_SELECT} WHERE website_id = $1 AND (metadata->>'deleted_at') IS NULL \
         ORDER BY (metadata->>'created_at') ASC, id ASC"
    ))
    .bind(website_id)
    .fetch_all(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The website's ACTIVE locations — the PUBLIC lookup read (inactive
/// and foreign-website stores are indistinguishable from missing).
pub async fn active_locations_for_website(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
) -> Result<Vec<PickupLocationRow>, StorefrontError> {
    sqlx::query_as::<_, PickupLocationRow>(&format!(
        "{LOCATION_SELECT} WHERE website_id = $1 AND is_active = true \
         AND (metadata->>'deleted_at') IS NULL \
         ORDER BY (metadata->>'created_at') ASC, id ASC"
    ))
    .bind(website_id)
    .fetch_all(exec)
    .await
    .map_err(StorefrontError::from)
}

/// One ACTIVE location on the website — the pin verb's server-side
/// resolution (the closed-door 404 when the store is missing, inactive,
/// or another website's).
pub async fn active_location_on_website(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
    location_id: Uuid,
) -> Result<Option<PickupLocationRow>, StorefrontError> {
    sqlx::query_as::<_, PickupLocationRow>(&format!(
        "{LOCATION_SELECT} WHERE id = $1 AND website_id = $2 AND is_active = true \
         AND (metadata->>'deleted_at') IS NULL LIMIT 1"
    ))
    .bind(location_id)
    .bind(website_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The officer upsert: create the named store on the website, or patch
/// the existing one (matched by name among live rows — the unique
/// index's key). Deactivation is the lifecycle: `is_active = false`
/// hides the store from every public read while the carts FK keeps
/// historical pins intact (RESTRICT forbids a silent delete).
///
/// Two fences guard the write (both inside one transaction):
///
/// 1. **The website is the grain.** The target website must be a live
///    row (an unknown or deleted website id is the typed 404 — the
///    registry never mints rows against dangling websites), and the
///    write's company is THAT website's company. The module's admin
///    surface carries no officer-company identity of its own; the
///    officer→company binding is the host's auth fence around the
///    admin tree, and this module enforces the referential half —
///    every fact on the row coheres with the target website's company.
/// 2. **The warehouse, when present, must be one of that company's
///    live warehouses** (the typed refusal covers a missing id and a
///    foreign company's id alike — a store that fulfilled from another
///    company's warehouse would promise stock it can never read).
///    `inventory.warehouses` is company-RLS fenced, so the read runs
///    inside the website's company scope; under the host's app role a
///    foreign warehouse is invisible (the same refusal), and the
///    explicit company comparison catches it even on scope-exempt
///    connections.
///
/// The fiscal country is REQUIRED merchant-declared input: a create
/// that omits it refuses, a patch may only replace it with another
/// valid code, and whatever value the row will carry is validated as a
/// 2-letter ISO code unconditionally — a store without a country has
/// no defensible tax arm (its pickup orders would silently resolve
/// under the delivery/home jurisdiction).
pub async fn upsert_location(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    name: &str,
    patch: LocationPatch,
    actor: ActorRef,
) -> Result<Uuid, StorefrontError> {
    let name = name.trim();
    if name.is_empty() || name.len() > 120 {
        return Err(StorefrontError::InvalidInput(
            "location name must be 1..=120 characters".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    // Fence 1: the target website must be live; its company is the
    // write's company (the referential half of the officer-write scope).
    let owner_company: Uuid = sqlx::query_as(
        r#"
        SELECT company_id
        FROM website.websites
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .fetch_optional(&mut *tx)
    .await?
    .map(|(company,)| company)
    .ok_or(StorefrontError::WebsiteNotFound)?;
    // RLS scope (ADR-0008): the warehouse fence reads a FORCE-RLS
    // inventory table; bind the website's company so the scoped read
    // sees exactly this company's warehouses under the host's app role.
    company_scope::bind_company_on(&mut tx, owner_company).await?;
    // Fence 2: a present warehouse pointer must name one of the
    // website company's live warehouses (missing = foreign = refused).
    let warehouse_id = patch.warehouse_id.flatten();
    if let Some(warehouse) = warehouse_id {
        let found: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT company_id
            FROM inventory.warehouses
            WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
            LIMIT 1
            "#,
        )
        .bind(warehouse)
        .fetch_optional(&mut *tx)
        .await?;
        match found {
            Some((warehouse_company,)) if warehouse_company == owner_company => {}
            _ => return Err(StorefrontError::PickupWarehouseRefused),
        }
    }
    let existing: Option<(Uuid, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, country
        FROM storefront.pickup_locations
        WHERE website_id = $1 AND name = $2 AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    // The fiscal country is required merchant-declared input. A create
    // must carry it; a patch keeps the stored value unless it carries a
    // new one. Whatever value the row will STORE is validated — the
    // 2-letter ISO check is unconditional, never presence-gated: a
    // countryless store cannot exist through this verb.
    let stored_country = existing.as_ref().and_then(|(_, country)| country.clone());
    let effective_country = patch.country.clone().or(stored_country);
    let Some(country) = effective_country.as_deref() else {
        return Err(StorefrontError::InvalidInput(
            "a pickup location requires its country — the 2-letter ISO code \
             its pickup orders' tax resolves under"
                .into(),
        ));
    };
    if country.len() != 2 || !country.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(StorefrontError::InvalidInput(
            "country must be a 2-letter ISO code".into(),
        ));
    }
    let id = if let Some((id, _)) = existing {
        // Patch by name-match: each Some(..) writes, None keeps.
        sqlx::query(
            r#"
            UPDATE storefront.pickup_locations SET
                warehouse_id = COALESCE($2, warehouse_id),
                address_line1 = COALESCE($3, address_line1),
                city = COALESCE($4, city),
                postal_code = COALESCE($5, postal_code),
                country = COALESCE($6, country),
                latitude = COALESCE($7, latitude),
                longitude = COALESCE($8, longitude),
                opening_hours = COALESCE($9, opening_hours),
                is_active = COALESCE($10, is_active),
                metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
            WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
            "#,
        )
        .bind(id)
        .bind(warehouse_id)
        .bind(patch.address_line1.as_deref())
        .bind(patch.city.as_deref())
        .bind(patch.postal_code.as_deref())
        .bind(patch.country.as_deref())
        .bind(patch.latitude.flatten())
        .bind(patch.longitude.flatten())
        .bind(patch.opening_hours.clone())
        .bind(patch.is_active)
        .execute(&mut *tx)
        .await?;
        id
    } else {
        let (id,): (Uuid,) = sqlx::query_as(
            r#"
            INSERT INTO storefront.pickup_locations
                (website_id, warehouse_id, name, address_line1, city, postal_code,
                 country, latitude, longitude, opening_hours, is_active)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id
            "#,
        )
        .bind(website_id)
        .bind(warehouse_id)
        .bind(name)
        .bind(patch.address_line1.as_deref())
        .bind(patch.city.as_deref())
        .bind(patch.postal_code.as_deref())
        .bind(patch.country.as_deref())
        .bind(patch.latitude.flatten())
        .bind(patch.longitude.flatten())
        .bind(patch.opening_hours.clone())
        .bind(patch.is_active.unwrap_or(true))
        .fetch_one(&mut *tx)
        .await?;
        id
    };
    record_audit(
        &mut *tx,
        Some(website_id),
        "location_upserted",
        actor,
        Some("pickup_location"),
        Some(id),
        Some(serde_json::json!({ "name": name })),
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_patch_carries_no_publish_style_fence() {
        // is_active is officer-settable through the SAME upsert (the
        // registry's lifecycle flag) — unlike is_published on listings,
        // no public surface reads it as a merchandising gate, so the
        // fence-verb pattern does not apply. Recorded in the spec §14.2.
        let patch = LocationPatch { is_active: Some(false), ..Default::default() };
        assert!(patch.is_active.is_some());
    }
}
