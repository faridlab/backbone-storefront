//! The pricing mapping (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The frozen website→port dimension mapping (§5.2 of the module
//! spec): selling's `CartPricingPort` carries NO website dimension, so
//! per-website pricing lands by mapping:
//!
//! | port field | resolution |
//! |---|---|
//! | `company_id` | the website's company (total, stored pairing) |
//! | `customer_id` | the cart's billing party once captured, else `None` (the guest party is NEVER passed — promo rules must not key on a shared synthetic customer) |
//! | `customer_group_id` | the billing party's explicit segment ELSE the website's default segment — how "per-website pricelist" lands on the port's EXISTING group dimension |
//! | `coupon_code` | the cart's presented code, straight through |
//! | per-line `list_price` | this website's `product_prices` row |
//! | tax | NOT a port field — fiscal resolution is order-level (§5.3, the tax port) |
//!
//! The port instance is the HOST-composed adapter over promo (one
//! adapter, two consumers — never a second mapping; the module takes no
//! promo edge). [`price_cart`] is THE single derivation: display reads
//! and the place-time mint call the same function, so the locked total
//! and the rendered total can never diverge by construction.
//!
//! CACHE POSTURE (§5.4, frozen): P1 adds NO price-resolution cache —
//! every cart read re-derives through the port. Any FUTURE cache MUST
//! key per company and invalidate per company;
//! [`invalidate_pricing_cache_for_company`] is the seam that contract
//! hangs on (a no-op today BY DESIGN).

use std::collections::HashMap;

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_selling::application::service::selling_cart_pricing::{
    CartPriceLine, CartPriceRequest, CartPricingPort,
};

use super::cart_service::{CartLineRow, CartRow};
use super::catalog_read_port::CatalogReadPort;
use super::party_write_port::PartyWritePort;
use super::storefront_error::StorefrontError;

/// The per-company cache-invalidation seam. P1: NO cache exists — the
/// call is an explicit no-op that records the contract (any future
/// price-resolution cache plugs in HERE and may invalidate for exactly
/// one company; whole-registry/global clearing is banned by the module
/// spec's cache posture).
pub fn invalidate_pricing_cache_for_company(_company_id: Uuid) {
    // Deliberately empty: no cache exists this pass. The signature is
    // the contract — per company, never registry-wide.
}

/// The website's sale-settings row as the pricing and gate reads use it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SaleSettingsRow {
    pub id: Uuid,
    pub website_id: Uuid,
    pub access_gate: String,
    pub default_customer_group_id: Option<Uuid>,
    pub guest_party_id: Uuid,
    pub recovery_template_ref: Option<String>,
}

/// The website's live sale-settings row, if one exists.
pub async fn settings_for(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
) -> Result<Option<SaleSettingsRow>, StorefrontError> {
    sqlx::query_as::<_, SaleSettingsRow>(
        r#"
        SELECT id, website_id, access_gate::text AS access_gate,
               default_customer_group_id, guest_party_id, recovery_template_ref
        FROM storefront.website_sale_settings
        WHERE website_id = $1 AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(website_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The members_only gate's read: `true` when the website's settings arm
/// the B2B gate (§4.3 — every public verb then requires a verified
/// principal; anonymous pricing reads 401 BEFORE any port call, so a
/// walled store has no pricing oracle).
pub async fn members_only(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
) -> Result<bool, StorefrontError> {
    Ok(settings_for(exec, website_id)
        .await?
        .map(|s| s.access_gate == "members_only")
        .unwrap_or(false))
}

/// Resolve the pricing request's group dimension (§5.2 resolution
/// order): the billing party's explicit segment ELSE the website's
/// default segment. Deterministic per call; a party without a segment
/// falls to the website default.
pub async fn resolve_group_id(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    party_port: &dyn PartyWritePort,
    company_id: Uuid,
    website_id: Uuid,
    party_id: Option<Uuid>,
) -> Result<Option<Uuid>, StorefrontError> {
    let settings_group = settings_for(exec, website_id)
        .await?
        .and_then(|s| s.default_customer_group_id);
    match party_id {
        None => Ok(settings_group),
        Some(party) => {
            let segment = party_port
                .party_segment(company_id, party)
                .await
                .map_err(|e| StorefrontError::PartyPortRefused { code: e.code })?;
            Ok(segment.or(settings_group))
        }
    }
}

/// One line priced for display: the port's verdict or the unavailable
/// flag (a line whose item left the gate between mutations — kept
/// VISIBLE, never silently dropped, never charged).
#[derive(Debug, Clone)]
pub struct DisplayLine {
    pub line_id: Uuid,
    pub item_id: Uuid,
    pub name: String,
    pub quantity: Decimal,
    pub list_price: Decimal,
    pub unit_price: Option<Decimal>,
    pub net_line_total: Option<Decimal>,
    pub unavailable: bool,
}

/// The freshly derived priced-cart view every priced read serves
/// (display reads and the place-time mint use the SAME derivation).
#[derive(Debug, Clone)]
pub struct PricedCartView {
    pub cart_id: Uuid,
    pub lines: Vec<DisplayLine>,
    /// Σ net_line_total over the AVAILABLE lines — equals the port's
    /// `total` (promo conserves it exactly).
    pub subtotal: Decimal,
    pub currency: String,
    pub customer_group_id: Option<Uuid>,
    pub coupon_code: Option<String>,
    /// The priced request's unavailable-line count (0 on the happy
    /// path; >0 means the place verb will refuse — surfaced so reads
    /// can render the state honestly).
    pub unavailable_count: usize,
}

/// Price one cart through the port — THE single §5.2 derivation.
/// Lines whose item fails the price/dimension resolution come back
/// flagged `unavailable` (visible, uncharged); a place with any
/// unavailable line refuses.
pub async fn price_cart(
    conn: &mut sqlx::PgConnection,
    catalog: &dyn CatalogReadPort,
    party_port: &dyn PartyWritePort,
    pricing: &dyn CartPricingPort,
    company_id: Uuid,
    cart: &CartRow,
    lines: &[CartLineRow],
) -> Result<PricedCartView, StorefrontError> {
    // Per-line price rows for THIS website (the per-website pricelist's
    // base arm).
    let item_ids: Vec<Uuid> = lines.iter().map(|l| l.item_id).collect();
    let price_rows: Vec<(Uuid, Decimal, String)> = sqlx::query_as(
        r#"
        SELECT item_id, list_price, currency
        FROM storefront.product_prices
        WHERE website_id = $1 AND item_id = ANY($2)
          AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(cart.website_id)
    .bind(&item_ids)
    .fetch_all(&mut *conn)
    .await?;
    let prices: HashMap<Uuid, (Decimal, String)> =
        price_rows.into_iter().map(|(i, p, c)| (i, (p, c))).collect();

    // Catalog dimensions (the group/brand arms the pricing rules match
    // on) + display names.
    let snapshots = catalog
        .item_snapshots(company_id, &item_ids)
        .await
        .map_err(|e| StorefrontError::CatalogPortRefused { code: e.code })?;
    let by_item: HashMap<Uuid, _> = snapshots.into_iter().map(|s| (s.item_id, s)).collect();

    // The group dimension (party segment ELSE website default).
    let group_id =
        resolve_group_id(&mut *conn, party_port, company_id, cart.website_id, cart.party_id)
            .await?;

    // Assemble the request over the RESOLVABLE lines, remembering the
    // display order so the port's verdict folds back exactly.
    let mut req_lines: Vec<CartPriceLine> = Vec::with_capacity(lines.len());
    let mut display: Vec<DisplayLine> = Vec::with_capacity(lines.len());
    let mut available_slots: Vec<usize> = Vec::with_capacity(lines.len());
    let mut currency = String::new();
    for line in lines {
        let snapshot = by_item.get(&line.item_id);
        let Some((list_price, row_currency)) = prices.get(&line.item_id) else {
            display.push(DisplayLine {
                line_id: line.id,
                item_id: line.item_id,
                name: snapshot.map_or_else(String::new, |s| s.name.clone()),
                quantity: line.quantity,
                list_price: Decimal::ZERO,
                unit_price: None,
                net_line_total: None,
                unavailable: true,
            });
            continue;
        };
        if currency.is_empty() {
            currency = row_currency.clone();
        }
        available_slots.push(display.len());
        req_lines.push(CartPriceLine {
            line_ref: Uuid::new_v4(),
            item_id: line.item_id,
            item_group_id: snapshot.and_then(|s| s.item_group_id),
            brand_id: snapshot.and_then(|s| s.brand_id),
            list_price: *list_price,
            quantity: line.quantity,
        });
        display.push(DisplayLine {
            line_id: line.id,
            item_id: line.item_id,
            name: snapshot.map_or_else(String::new, |s| s.name.clone()),
            quantity: line.quantity,
            list_price: *list_price,
            unit_price: None,
            net_line_total: None,
            unavailable: false,
        });
    }

    let unavailable_count = display.iter().filter(|d| d.unavailable).count();
    if req_lines.is_empty() {
        return Ok(PricedCartView {
            cart_id: cart.id,
            lines: display,
            subtotal: Decimal::ZERO,
            currency: or_idr(currency),
            customer_group_id: group_id,
            coupon_code: cart.coupon_code.clone(),
            unavailable_count,
        });
    }

    let req = CartPriceRequest {
        company_id,
        // The guest party is never passed (§5.2): before billing
        // capture there is no customer dimension at all.
        customer_id: cart.party_id,
        customer_group_id: group_id,
        coupon_code: cart.coupon_code.clone(),
        lines: req_lines,
    };
    let priced = pricing
        .price_cart(&req)
        .await
        .map_err(|e| StorefrontError::PricingRefused { code: e.code })?;

    // Fold the verdict back by request order (built in display order
    // over the available lines — the counts must match exactly).
    if priced.lines.len() != available_slots.len() {
        return Err(StorefrontError::Internal(
            "pricing port returned a different line count than requested".into(),
        ));
    }
    let mut subtotal = Decimal::ZERO;
    for (slot, priced_line) in available_slots.iter().zip(priced.lines.iter()) {
        let idx = *slot;
        display[idx].unit_price = Some(priced_line.unit_price);
        display[idx].net_line_total = Some(priced_line.net_line_total);
        subtotal += priced_line.net_line_total;
    }

    Ok(PricedCartView {
        cart_id: cart.id,
        lines: display,
        subtotal,
        currency: or_idr(currency),
        customer_group_id: group_id,
        coupon_code: cart.coupon_code.clone(),
        unavailable_count,
    })
}

fn or_idr(currency: String) -> String {
    if currency.is_empty() { "IDR".to_string() } else { currency }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_invalidation_seam_is_per_company_and_inert() {
        // The no-op is the P1 contract; the call must simply succeed.
        invalidate_pricing_cache_for_company(Uuid::new_v4());
    }
}
