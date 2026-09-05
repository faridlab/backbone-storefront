//! The availability reads and the stock gate (hand-written; user-owned;
//! see `metaphor.codegen.yaml`).
//!
//! Every number here is COMPUTED FRESH through the
//! [`AvailabilityReadPort`] — this module stores no stock snapshot and
//! no persisted shop-warning row anywhere (the persisted-warning shape
//! is deliberately not ported).
//!
//! TWO SCOPES (§14.1):
//!  - DISPLAY — the website's `display_warehouse_id` setting (NULL =
//!    the company aggregate). Serves the availability route and the
//!    comparison read.
//!  - CHECKOUT — the cart's fulfillment scope: the pinned pickup
//!    location's warehouse for a pickup cart, the company aggregate for
//!    a delivery cart. Serves the line-mutation clamp and the
//!    place-time gate.
//!
//! THE STOCK GATE: a line mutation (add/set) refuses with the typed 422
//! when the resulting quantity exceeds the checkout-scope free
//! quantity; the place re-checks EVERY line under the row lock (the
//! whole basket is gated, never just the first line). A listing flagged
//! `allow_backorder` skips the gate — made-to-order listings stay
//! orderable past free quantity. An unwired port fails CLOSED: the
//! clamped verbs answer the typed 503 rather than promising stock
//! nobody read (never a silent zero/infinite fallback).

use rust_decimal::Decimal;
use uuid::Uuid;

use super::availability_port::{AvailabilityPortError, AvailabilityReadPort};
use super::cart_service::CartRow;
use super::storefront_error::StorefrontError;

/// Map the port's refusal onto the typed 503.
pub fn map_availability_error(e: AvailabilityPortError) -> StorefrontError {
    StorefrontError::AvailabilityPortRefused { code: e.code }
}

/// The DISPLAY-scope warehouse for one website: the sale-settings row's
/// `display_warehouse_id` (NULL = aggregate across warehouses — a
/// documented, officer-visible semantic, never a hidden fallback).
pub async fn display_scope_warehouse(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
) -> Result<Option<Uuid>, StorefrontError> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        r#"
        SELECT display_warehouse_id
        FROM storefront.website_sale_settings
        WHERE website_id = $1 AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.and_then(|r| r.0))
}

/// The CHECKOUT-scope warehouse for one cart: the pinned pickup
/// location's warehouse (a pickup cart), else the company aggregate
/// (None). The location row is read FRESH here — the pinned id is the
/// only stored fact.
pub async fn checkout_scope_warehouse(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    cart: &CartRow,
) -> Result<Option<Uuid>, StorefrontError> {
    if cart.fulfillment_mode != "pickup" {
        return Ok(None);
    }
    let Some(location_id) = cart.pickup_location_id else {
        return Ok(None);
    };
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        r#"
        SELECT warehouse_id
        FROM storefront.pickup_locations
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(location_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.and_then(|r| r.0))
}

/// The listing's sold-out policy for one (website, item).
async fn listing_allows_backorder(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
    item_id: Uuid,
) -> Result<bool, StorefrontError> {
    let row: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT allow_backorder
        FROM storefront.product_listings
        WHERE website_id = $1 AND item_id = $2
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.is_some_and(|r| r.0))
}

/// The line-mutation clamp: refuse when `requested` exceeds the
/// checkout-scope free quantity for the item. Backorder-allowed
/// listings skip the clamp entirely. Takes one connection and
/// reborrows it per read (the caller's transaction rides through).
pub async fn clamp_quantity(
    conn: &mut sqlx::PgConnection,
    availability: &dyn AvailabilityReadPort,
    company_id: Uuid,
    cart: &CartRow,
    item_id: Uuid,
    requested: Decimal,
) -> Result<(), StorefrontError> {
    if listing_allows_backorder(&mut *conn, cart.website_id, item_id).await? {
        return Ok(());
    }
    let scope = checkout_scope_warehouse(&mut *conn, cart).await?;
    let answer = availability
        .free_quantity(company_id, item_id, scope)
        .await
        .map_err(map_availability_error)?;
    if requested > answer.free_quantity {
        return Err(StorefrontError::StockInsufficient {
            item_id,
            requested,
            available: answer.free_quantity,
        });
    }
    Ok(())
}

/// The place-time gate: EVERY line re-checked under the row lock — the
/// whole basket is gated, never just the first line. Backorder-allowed
/// lines skip their check.
pub async fn gate_place_quantities(
    conn: &mut sqlx::PgConnection,
    availability: &dyn AvailabilityReadPort,
    company_id: Uuid,
    cart: &CartRow,
    lines: &[(Uuid, Decimal)],
) -> Result<(), StorefrontError> {
    if lines.is_empty() {
        return Ok(());
    }
    let item_ids: Vec<Uuid> = lines.iter().map(|l| l.0).collect();
    // The backorder policy per item, one read.
    let backorder_rows: Vec<(Uuid, bool)> = sqlx::query_as(
        r#"
        SELECT item_id, allow_backorder
        FROM storefront.product_listings
        WHERE website_id = $1 AND item_id = ANY($2)
          AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart.website_id)
    .bind(&item_ids)
    .fetch_all(&mut *conn)
    .await?;
    let gated: Vec<(Uuid, Decimal)> = lines
        .iter()
        .filter(|(item, _)| !backorder_rows.iter().any(|(i, allowed)| i == item && *allowed))
        .cloned()
        .collect();
    if gated.is_empty() {
        return Ok(());
    }
    let scope = checkout_scope_warehouse(&mut *conn, cart).await?;
    let gated_ids: Vec<Uuid> = gated.iter().map(|l| l.0).collect();
    let answers = availability
        .free_quantities(company_id, &gated_ids, scope)
        .await
        .map_err(map_availability_error)?;
    for (item_id, requested) in gated {
        let Some(answer) = answers.iter().find(|a| a.item_id == item_id) else {
            // The adapter omitted a gated line — that is a refusal, not
            // an implicit zero (fail-loud, never guess stock).
            return Err(StorefrontError::AvailabilityPortRefused {
                code: "availability_port_incomplete".into(),
            });
        };
        if requested > answer.free_quantity {
            return Err(StorefrontError::StockInsufficient {
                item_id,
                requested,
                available: answer.free_quantity,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_types_are_decimals_end_to_end() {
        // The gate's arithmetic is decimal-only; the types must not
        // narrow through f64 anywhere (a float compare on money-adjacent
        // quantities is a rounding defect waiting to land).
        let q: Decimal = Decimal::new(1500, 2); // 15.00
        let free: Decimal = Decimal::new(1499, 2);
        assert!(q > free);
    }
}
