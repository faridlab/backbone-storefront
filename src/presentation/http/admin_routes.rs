//! The storefront module's ADMIN/officer route surface (hand-written;
//! user-owned; see `metaphor.codegen.yaml`).
//!
//! The module DOES NOT SELF-MOUNT and DOES NOT SELF-GATE: it exports
//! [`storefront_admin_routes`], a plain `axum::Router` the composing
//! host nests under the schema name BEHIND `company_auth`, with
//! `ModuleWriteGate::new(pool, "storefront")` as the INNERMOST
//! `route_layer` (the foundation-ext pattern verbatim: the write gate
//! innermost, company_auth outside). Authority names resolve through
//! the host gate: `write:storefront` (POST), `delete:storefront`
//! (DELETE), `admin:storefront` / `ADMIN` / `*:*` supersets.
//!
//! The acting OFFICER id arrives through the [`StorefrontActor`]
//! request extension (the host's company_auth bridge inserts it);
//! without it the verbs run as the system actor — never a public
//! principal.
//!
//! THE PUBLISH FENCE (§4.2): `is_published` is excluded from every
//! patch whitelist — a body carrying it takes the typed 422 AND a
//! `publish_refused` audit row; only the explicit publish/unpublish
//! verbs write the flag.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::Extensions,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::application::service::audit::{record_audit, ActorRef};
use crate::application::service::availability_port::{
    AvailabilityReadPort, RefusingAvailabilityReadPort,
};
use crate::application::service::catalog_read_port::{CatalogReadPort, RefusingCatalogReadPort};
use crate::application::service::catalog_service;
use crate::application::service::checkout_service::{self, CheckoutDeps};
use crate::application::service::collect_service::{self, LocationPatch};
use crate::application::service::notifier_port::{
    RecoveryNotifier, StockAlertNotifier, UnwiredRecoveryNotifier, UnwiredStockAlertNotifier,
};
use crate::application::service::pricing_service::settings_for;
use crate::application::service::recovery_service;
use crate::application::service::storefront_error::StorefrontError;
use crate::application::service::wishlist_service;

/// The request extension carrying the acting officer id (the host's
/// company_auth bridge inserts it after authentication).
#[derive(Debug, Clone, Copy)]
pub struct StorefrontActor(pub Uuid);

/// The module's admin state — the pool plus the host-wired ports.
#[derive(Clone)]
pub struct StorefrontAdminState {
    pub pool: sqlx::PgPool,
    notifier: Arc<dyn RecoveryNotifier>,
    stock_notifier: Arc<dyn StockAlertNotifier>,
    party: Arc<dyn crate::application::service::party_write_port::PartyWritePort>,
    catalog: Arc<dyn CatalogReadPort>,
    availability: Arc<dyn AvailabilityReadPort>,
    checkout: Arc<CheckoutDeps>,
}

impl StorefrontAdminState {
    /// Compose with the FAIL-CLOSED defaults: the UNWIRED notifiers
    /// (a recovery or stock-alert send then records the visible
    /// `unwired` state — loud, never silent), the refusing party /
    /// catalog / availability ports (verbs that need them refuse with
    /// the typed 503 until the host wires the adapters), and checkout
    /// deps over those same refusing defaults (the confirm-pickup verb
    /// needs none of the refused arms — it mints nothing).
    pub fn new(pool: sqlx::PgPool) -> Self {
        let checkout = Arc::new(CheckoutDeps::new(
            pool.clone(),
            Arc::new(RefusingCatalogReadPort),
            Arc::new(crate::application::service::party_write_port::RefusingPartyWritePort),
            Arc::new(crate::application::service::tax_resolve_port::RefusingTaxResolvePort),
            Arc::new(AdminRefusingPricing),
            Arc::new(RefusingAvailabilityReadPort),
        ));
        Self {
            pool,
            notifier: Arc::new(UnwiredRecoveryNotifier),
            stock_notifier: Arc::new(UnwiredStockAlertNotifier),
            party: Arc::new(crate::application::service::party_write_port::RefusingPartyWritePort),
            catalog: Arc::new(RefusingCatalogReadPort),
            availability: Arc::new(RefusingAvailabilityReadPort),
            checkout,
        }
    }

    /// The notifier install seam (the host bridges its mailer here).
    pub fn install_notifier(&mut self, notifier: Arc<dyn RecoveryNotifier>) {
        self.notifier = notifier;
    }

    /// The stock-alert notifier install seam (the host bridges its
    /// mailer here — the back-in-stock send's delivery arm).
    pub fn install_stock_notifier(&mut self, notifier: Arc<dyn StockAlertNotifier>) {
        self.stock_notifier = notifier;
    }

    /// The party-port install seam (the host bridges its party module
    /// handle here; the settings verb's guest-party bootstrap needs
    /// it).
    pub fn install_party_port(
        &mut self,
        party: Arc<dyn crate::application::service::party_write_port::PartyWritePort>,
    ) {
        self.party = party;
    }

    /// The catalog-port install seam (the host bridges its catalog
    /// module handle here; the stock-alert send resolves item names
    /// through it).
    pub fn install_catalog_port(&mut self, catalog: Arc<dyn CatalogReadPort>) {
        self.catalog = catalog;
    }

    /// The availability-port install seam (the host bridges its
    /// inventory/manufacturing composition here; the back-in-stock
    /// reads and sends derive fresh eligibility through it).
    pub fn install_availability_port(&mut self, availability: Arc<dyn AvailabilityReadPort>) {
        self.availability = availability;
    }
}

/// The pricing-port default for the admin state's checkout deps: the
/// officer tree mints no carts, so every price request refuses — loud,
/// never a zero-total fallback.
struct AdminRefusingPricing;

#[async_trait::async_trait]
impl backbone_selling::application::service::selling_cart_pricing::CartPricingPort
    for AdminRefusingPricing
{
    async fn price_cart(
        &self,
        _req: &backbone_selling::application::service::selling_cart_pricing::CartPriceRequest,
    ) -> Result<
        backbone_selling::application::service::selling_cart_pricing::PricedCart,
        backbone_selling::application::service::selling_cart_pricing::CartPricingError,
    > {
        Err(
            backbone_selling::application::service::selling_cart_pricing::CartPricingError {
                code: "pricing_port_unwired".into(),
                message: "the admin tree prices no carts".into(),
            },
        )
    }
}

fn actor_of(extensions: &Extensions) -> ActorRef {
    match extensions.get::<StorefrontActor>() {
        Some(StorefrontActor(id)) => ActorRef::officer(*id),
        None => ActorRef::system(),
    }
}

fn err_response(err: StorefrontError) -> Response {
    use axum::http::StatusCode;
    let status =
        StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = match &err {
        StorefrontError::Db(e) => {
            tracing::error!(error = ?e, "storefront admin route internal error");
            json!({"error": "internal error", "code": err.code()})
        }
        StorefrontError::Internal(msg) => {
            tracing::error!(reason = %msg, "storefront admin route internal error");
            json!({"error": "internal error", "code": err.code()})
        }
        other => json!({"error": other.to_string(), "code": err.code()}),
    };
    (status, Json(body)).into_response()
}

// ── request DTOs ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CompanyQuery {
    company_id: Uuid,
}

/// The listing upsert body. `is_published` is DECODED (so a body
/// carrying it is refused loudly, not silently dropped) but it is
/// FENCED — never a parameter of the upsert verb.
#[derive(Debug, Deserialize)]
struct UpsertListingBody {
    website_id: Uuid,
    item_id: Uuid,
    sale_ok: bool,
    #[serde(default)]
    sequence: Option<i32>,
    #[serde(default)]
    media_urls: Option<serde_json::Value>,
    #[serde(default)]
    is_published: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ListingPath {
    id: Uuid,
}

#[derive(Debug, Deserialize)]
struct SetPriceBody {
    website_id: Uuid,
    item_id: Uuid,
    list_price: rust_decimal::Decimal,
    #[serde(default)]
    compare_at_price: Option<rust_decimal::Decimal>,
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebsitePath {
    website_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct SetSettingsBody {
    access_gate: String,
    #[serde(default)]
    default_customer_group_id: Option<Uuid>,
    #[serde(default)]
    recovery_template_ref: Option<String>,
    #[serde(default)]
    display_warehouse_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct CartPath {
    cart_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct WebsiteQuery {
    website_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct UpsertLocationBody {
    website_id: Uuid,
    name: String,
    #[serde(default)]
    warehouse_id: Option<Uuid>,
    #[serde(default)]
    address_line1: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    postal_code: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    latitude: Option<rust_decimal::Decimal>,
    #[serde(default)]
    longitude: Option<rust_decimal::Decimal>,
    #[serde(default)]
    opening_hours: Option<serde_json::Value>,
    #[serde(default)]
    is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct CheckoutPath {
    checkout_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct ConfirmPickupBody {
    #[serde(default)]
    payment_reference: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StockSendBody {
    website_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct SetBackorderBody {
    website_id: Uuid,
    item_id: Uuid,
    allow_backorder: bool,
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn listings_read(
    State(state): State<StorefrontAdminState>,
    Query(q): Query<CompanyQuery>,
) -> Response {
    match catalog_service::admin_listings(&state.pool, q.company_id).await {
        Ok(rows) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "listings": rows.iter().map(|r| json!({
                    "id": r.id,
                    "website_id": r.website_id,
                    "item_id": r.item_id,
                    "sale_ok": r.sale_ok,
                    "is_published": r.is_published,
                    "sequence": r.sequence,
                    "media_urls": r.media_urls,
                    "list_price": r.list_price,
                    "compare_at_price": r.compare_at_price,
                    "currency": r.currency,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

async fn listings_upsert(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Json(body): Json<UpsertListingBody>,
) -> Response {
    // THE PUBLISH FENCE: `is_published` is writable ONLY through the
    // publish/unpublish verbs — a patch attempt is the typed 422 plus
    // the `publish_refused` audit row (the website contract copied).
    if body.is_published.is_some() {
        let _ = record_audit(
            &state.pool,
            Some(body.website_id),
            "publish_refused",
            actor_of(&extensions),
            Some("product_listing"),
            Some(body.item_id),
            Some(json!({ "reason": "is_published is not patchable", "verb": "listing_upsert" })),
        )
        .await;
        return err_response(StorefrontError::FieldNotPatchable {
            field: "is_published".into(),
            verb: "listing_upsert".into(),
        });
    }
    match catalog_service::upsert_listing(
        &state.pool,
        body.website_id,
        body.item_id,
        body.sale_ok,
        body.sequence.unwrap_or(10),
        body.media_urls.clone().unwrap_or_else(|| json!([])),
        actor_of(&extensions),
    )
    .await
    {
        Ok(id) => (
            axum::http::StatusCode::OK,
            Json(json!({ "listing_id": id, "is_published": "unchanged (publish verb only)" })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The listing row's website (the publish verbs' audit scope).
async fn website_of_listing(
    pool: &sqlx::PgPool,
    listing_id: Uuid,
) -> Result<Uuid, StorefrontError> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT website_id FROM storefront.product_listings WHERE id = $1 LIMIT 1")
            .bind(listing_id)
            .fetch_optional(pool)
            .await
            .map_err(StorefrontError::from)?;
    row.map(|r| r.0).ok_or(StorefrontError::NotFound("listing".into()))
}

async fn listing_publish(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Path(listing): Path<ListingPath>,
) -> Response {
    let website_id = match website_of_listing(&state.pool, listing.id).await {
        Ok(w) => w,
        Err(e) => return err_response(e),
    };
    match catalog_service::publish_listing(&state.pool, website_id, listing.id, actor_of(&extensions))
        .await
    {
        Ok(()) => (axum::http::StatusCode::OK, Json(json!({ "published": true }))).into_response(),
        Err(e) => err_response(e),
    }
}

async fn listing_unpublish(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Path(listing): Path<ListingPath>,
) -> Response {
    let website_id = match website_of_listing(&state.pool, listing.id).await {
        Ok(w) => w,
        Err(e) => return err_response(e),
    };
    match catalog_service::unpublish_listing(
        &state.pool,
        website_id,
        listing.id,
        actor_of(&extensions),
    )
    .await
    {
        Ok(()) => (axum::http::StatusCode::OK, Json(json!({ "published": false }))).into_response(),
        Err(e) => err_response(e),
    }
}

async fn prices_set(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Json(body): Json<SetPriceBody>,
) -> Response {
    match catalog_service::set_price(
        &state.pool,
        body.website_id,
        body.item_id,
        body.list_price,
        body.compare_at_price,
        body.currency.as_deref().unwrap_or("IDR"),
        actor_of(&extensions),
    )
    .await
    {
        Ok(id) => (
            axum::http::StatusCode::OK,
            Json(json!({ "price_id": id })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

async fn settings_set(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Path(path): Path<WebsitePath>,
    Json(body): Json<SetSettingsBody>,
) -> Response {
    match catalog_service::set_settings(
        &state.pool,
        state.party.as_ref(),
        path.website_id,
        catalog_service::SettingsPatch {
            access_gate: body.access_gate,
            default_customer_group_id: body.default_customer_group_id,
            recovery_template_ref: body.recovery_template_ref,
            display_warehouse_id: body.display_warehouse_id,
        },
        actor_of(&extensions),
    )
    .await
    {
        Ok(id) => (
            axum::http::StatusCode::OK,
            Json(json!({ "settings_id": id })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The settings read carries the display scope (NULL = aggregate).
async fn settings_read(
    State(state): State<StorefrontAdminState>,
    Path(path): Path<WebsitePath>,
) -> Response {
    match settings_for(&state.pool, path.website_id).await {
        Ok(Some(row)) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "website_id": row.website_id,
                "access_gate": row.access_gate,
                "default_customer_group_id": row.default_customer_group_id,
                "guest_party_id": row.guest_party_id,
                "recovery_template_ref": row.recovery_template_ref,
                "display_warehouse_id": row.display_warehouse_id,
            })),
        )
            .into_response(),
        Ok(None) => err_response(StorefrontError::SettingsNotFound),
        Err(e) => err_response(e),
    }
}

async fn abandoned_read(
    State(state): State<StorefrontAdminState>,
    Query(q): Query<CompanyQuery>,
) -> Response {
    match recovery_service::abandoned_carts_for_company(
        &state.pool,
        q.company_id,
        recovery_service::abandoned_after_hours(),
    )
    .await
    {
        Ok(carts) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "abandoned_after_hours": recovery_service::abandoned_after_hours(),
                "carts": carts.iter().map(|c| json!({
                    "cart_id": c.id,
                    "website_id": c.website_id,
                    "state": c.state,
                    "updated_at": c.updated_at,
                    "line_count": c.line_count,
                    "party_id": c.party_id,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

async fn abandoned_send_recovery(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Path(path): Path<CartPath>,
) -> Response {
    match recovery_service::send_recovery(&state.pool, state.notifier.as_ref(), path.cart_id, actor_of(&extensions))
        .await
    {
        Ok(delivery_state) => (
            axum::http::StatusCode::OK,
            Json(json!({ "delivery_state": delivery_state })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

async fn checkouts_read(
    State(state): State<StorefrontAdminState>,
    Query(q): Query<CompanyQuery>,
) -> Response {
    match checkout_service::admin_checkouts(&state.pool, q.company_id, 200).await {
        Ok(rows) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "checkouts": rows.iter().map(|c| json!({
                    "id": c.id,
                    "cart_id": c.cart_id,
                    "website_id": c.website_id,
                    "sales_order_id": c.sales_order_id,
                    "gateway_transaction_id": c.gateway_transaction_id,
                    "provider_code": c.provider_code,
                    "amount_total": c.amount_total,
                    "state": c.state,
                    "placed_at": c.placed_at,
                    "settled_at": c.settled_at,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

// ── Click & Collect registry (§14.2) ───────────────────────────────────────

/// The registry read (all states — the management view; the public
/// lookup is the active-only arm).
async fn locations_read(
    State(state): State<StorefrontAdminState>,
    Query(q): Query<WebsiteQuery>,
) -> Response {
    match collect_service::locations_for_website(&state.pool, q.website_id).await {
        Ok(rows) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "locations": rows.iter().map(|l| json!({
                    "location_id": l.id,
                    "website_id": l.website_id,
                    "warehouse_id": l.warehouse_id,
                    "name": l.name,
                    "address_line1": l.address_line1,
                    "city": l.city,
                    "postal_code": l.postal_code,
                    "country": l.country,
                    "latitude": l.latitude,
                    "longitude": l.longitude,
                    "opening_hours": l.opening_hours,
                    "is_active": l.is_active,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The registry upsert: a store exists ONLY through this verb —
/// nothing auto-mints locations from the company's warehouses, and no
/// client JSON pins warehouse or fiscal facts (the row the officer
/// writes here is the row the pin verb resolves against).
async fn locations_upsert(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Json(body): Json<UpsertLocationBody>,
) -> Response {
    match collect_service::upsert_location(
        &state.pool,
        body.website_id,
        &body.name,
        LocationPatch {
            // Absent = keep the stored value (clearing a warehouse or a
            // coordinate is the deactivation path, not a null write).
            warehouse_id: body.warehouse_id.map(Some),
            name: None,
            address_line1: body.address_line1,
            city: body.city,
            postal_code: body.postal_code,
            country: body.country,
            latitude: body.latitude.map(Some),
            longitude: body.longitude.map(Some),
            opening_hours: body.opening_hours,
            is_active: body.is_active,
        },
        actor_of(&extensions),
    )
    .await
    {
        Ok(id) => (
            axum::http::StatusCode::OK,
            Json(json!({ "location_id": id, "name": body.name })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The officer CONFIRM for a pay-on-site checkout: the store took the
/// physical payment; ONLY this verb settles the pending_pickup lane
/// (the lane itself never auto-confirms). The optional receipt note
/// stamps the audit trail.
async fn checkout_confirm_pickup(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Path(path): Path<CheckoutPath>,
    Json(body): Json<ConfirmPickupBody>,
) -> Response {
    match checkout_service::confirm_pickup(
        &state.checkout,
        path.checkout_id,
        body.payment_reference.as_deref(),
        actor_of(&extensions),
    )
    .await
    {
        Ok(checkout) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "checkout_id": checkout.id,
                "state": checkout.state,
                "settled_at": checkout.settled_at,
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

// ── the sold-out policy + back-in-stock surface (§14.1/§14.3) ─────────────

/// Set the listing's sold-out policy: whether the stock gate skips
/// this listing (made-to-order stays orderable past free quantity).
async fn listing_backorder_set(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Json(body): Json<SetBackorderBody>,
) -> Response {
    match catalog_service::set_listing_backorder(
        &state.pool,
        body.website_id,
        body.item_id,
        body.allow_backorder,
        actor_of(&extensions),
    )
    .await
    {
        Ok(id) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "listing_id": id,
                "item_id": body.item_id,
                "allow_backorder": body.allow_backorder,
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The armed-demand read: per-item wait counts with FRESH display-scope
/// availability (nothing persisted — the officer sees the truth of the
/// moment). Needs the company for the port read; the website carries it.
async fn stock_wait_read_route(
    State(state): State<StorefrontAdminState>,
    Query(q): Query<WebsiteQuery>,
) -> Response {
    let (company_id,): (Uuid,) = match sqlx::query_as(
        r#"
        SELECT company_id FROM website.websites
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(q.website_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(e) => return err_response(StorefrontError::from(e)),
    };
    match wishlist_service::stock_wait_read(
        &state.pool,
        state.availability.as_ref(),
        company_id,
        q.website_id,
    )
    .await
    {
        Ok(rows) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "website_id": q.website_id,
                "items": rows.iter().map(|r| json!({
                    "item_id": r.item_id,
                    "armed": r.armed,
                    "with_address": r.with_address,
                    "free_quantity": r.free_quantity,
                    "eligible": r.eligible,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The officer EXPLICIT back-in-stock send: one item, every armed and
/// addressable wish. Refuses unless the item is actually back in stock
/// (the fresh port read — an alert for a still-out item would be a
/// lie). Accepted sends (or the visible unwired state) clear their
/// arms; transport failures leave them armed.
async fn stock_alert_send(
    State(state): State<StorefrontAdminState>,
    extensions: Extensions,
    Path(item_id): Path<Uuid>,
    Json(body): Json<StockSendBody>,
) -> Response {
    let (company_id,): (Uuid,) = match sqlx::query_as(
        r#"
        SELECT company_id FROM website.websites
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(body.website_id)
    .fetch_one(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(e) => return err_response(StorefrontError::from(e)),
    };
    match wishlist_service::send_stock_alerts(
        &state.pool,
        state.catalog.as_ref(),
        state.availability.as_ref(),
        state.stock_notifier.as_ref(),
        company_id,
        body.website_id,
        item_id,
        actor_of(&extensions),
    )
    .await
    {
        Ok(summary) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "item_id": summary.item_id,
                "attempted": summary.attempted,
                "sent": summary.sent,
                "skipped_no_address": summary.skipped_no_address,
                "failed": summary.failed,
                "delivery_state": summary.delivery_state,
            })),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

/// The exported officer router — EXACTLY the §6.2 table (every
/// mutation a POST). The host mounts it behind `company_auth`
/// with the module write gate as the innermost route layer.
pub fn storefront_admin_routes(state: StorefrontAdminState) -> Router {
    Router::new()
        .route("/admin/listings", get(listings_read).post(listings_upsert))
        .route("/admin/listings/:id/publish", post(listing_publish))
        .route("/admin/listings/:id/unpublish", post(listing_unpublish))
        .route("/admin/listings/backorder", post(listing_backorder_set))
        .route("/admin/prices", post(prices_set))
        .route("/admin/settings/:website_id", get(settings_read).post(settings_set))
        .route("/admin/abandoned-carts", get(abandoned_read))
        .route("/admin/abandoned-carts/:cart_id/send-recovery", post(abandoned_send_recovery))
        .route("/admin/checkouts", get(checkouts_read))
        .route("/admin/checkouts/:checkout_id/confirm-pickup", post(checkout_confirm_pickup))
        .route("/admin/collect/locations", get(locations_read).post(locations_upsert))
        .route("/admin/stock-wait", get(stock_wait_read_route))
        .route("/admin/stock-wait/:item_id/send", post(stock_alert_send))
        .with_state(state)
}
