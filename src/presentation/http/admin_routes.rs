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
use crate::application::service::catalog_service;
use crate::application::service::checkout_service;
use crate::application::service::notifier_port::{RecoveryNotifier, UnwiredRecoveryNotifier};
use crate::application::service::pricing_service::settings_for;
use crate::application::service::recovery_service;
use crate::application::service::storefront_error::StorefrontError;

/// The request extension carrying the acting officer id (the host's
/// company_auth bridge inserts it after authentication).
#[derive(Debug, Clone, Copy)]
pub struct StorefrontActor(pub Uuid);

/// The module's admin state — the pool plus the two host-wired ports.
#[derive(Clone)]
pub struct StorefrontAdminState {
    pub pool: sqlx::PgPool,
    notifier: Arc<dyn RecoveryNotifier>,
    party: Arc<dyn crate::application::service::party_write_port::PartyWritePort>,
}

impl StorefrontAdminState {
    /// Compose with the FAIL-CLOSED defaults: the UNWIRED notifier
    /// (a recovery send then records the visible `unwired` state —
    /// loud, never silent) and the refusing party port (a first-set
    /// settings save that must mint the guest party refuses with the
    /// typed 503 until the host wires the adapter — never a silent
    /// zero-uuid guest arm).
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            notifier: Arc::new(UnwiredRecoveryNotifier),
            party: Arc::new(crate::application::service::party_write_port::RefusingPartyWritePort),
        }
    }

    /// The notifier install seam (the host bridges its mailer here).
    pub fn install_notifier(&mut self, notifier: Arc<dyn RecoveryNotifier>) {
        self.notifier = notifier;
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
}

#[derive(Debug, Deserialize)]
struct CartPath {
    cart_id: Uuid,
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
            })),
        )
            .into_response(),
        Ok(None) => err_response(StorefrontError::SettingsNotFound),
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

/// The exported officer router — EXACTLY the §6.2 table (10 rows,
/// every mutation a POST). The host mounts it behind `company_auth`
/// with the module write gate as the innermost route layer.
pub fn storefront_admin_routes(state: StorefrontAdminState) -> Router {
    Router::new()
        .route("/admin/listings", get(listings_read).post(listings_upsert))
        .route("/admin/listings/:id/publish", post(listing_publish))
        .route("/admin/listings/:id/unpublish", post(listing_unpublish))
        .route("/admin/prices", post(prices_set))
        .route("/admin/settings/:website_id", get(settings_read).post(settings_set))
        .route("/admin/abandoned-carts", get(abandoned_read))
        .route("/admin/abandoned-carts/:cart_id/send-recovery", post(abandoned_send_recovery))
        .route("/admin/checkouts", get(checkouts_read))
        .with_state(state)
}
