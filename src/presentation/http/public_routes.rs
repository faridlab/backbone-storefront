//! The storefront module's PUBLIC route surface (hand-written;
//! user-owned; see `metaphor.codegen.yaml`).
//!
//! The module DOES NOT SELF-MOUNT: it exports
//! [`storefront_public_routes`], a plain `axum::Router` the composing
//! host nests BARE at `/api/v1/storefront` — its own gates are the
//! fence. NOTHING here touches the events intake surface (that router
//! belongs to the funnel increment and stays exported-unmounted).
//!
//! The gates, in order, on EVERY verb: (1) hostname binding through
//! website's exported `WebsiteSurface::resolve_website_by_host` (no
//! fallback; miss = the typed 404), (2) the identity ladder — the
//! visitor token (read-only; the storefront never mints visitor rows)
//! and/or the verified portal principal through website's exported
//! principal port, (3) the members_only arm (§4.3 — when the website's
//! settings arm the B2B gate, every public verb requires a verified
//! principal, 401 otherwise), (4) per-identity + per-IP fixed windows
//! on the write verbs.
//!
//! EVERY MUTATION IS A POST (§6): the route table carries no GET that
//! writes anything; checkout reads are derived views. The mutating-GET
//! probe family targets this table.
//!
//! The visitor token arrives as the `x-storefront-token` header — the
//! same `access_token` the website module's visitor heartbeat minted;
//! the storefront reads it, never writes it (§4.3's no-mint closure).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use backbone_selling::application::service::CartPricingPort;
// Website is consumed through its exported surface only (the sanctioned
// cross-module lane — the same one blog uses): hostname binding, the
// principal port, and the normalized-host helper.
use backbone_website::exports::{
    normalize_host, RefusingPrincipalVerifier, WebsitePrincipal, WebsitePrincipalVerifier,
    WebsiteSurface, WebsiteView,
};

use crate::application::service::availability_port::{
    AvailabilityReadPort, RefusingAvailabilityReadPort,
};
use crate::application::service::availability_service;
use crate::application::service::cart_service::{self, CartRow};
use crate::application::service::catalog_read_port::{CatalogReadPort, RefusingCatalogReadPort};
use crate::application::service::catalog_service::{self, SortKind};
use crate::application::service::checkout_service::{self, CheckoutDeps};
use crate::application::service::collect_service;
use crate::application::service::party_write_port::{PartyWritePort, RefusingPartyWritePort};
use crate::application::service::pricing_service::{self, members_only, PricedCartView};
use crate::application::service::recovery_service;
use crate::application::service::storefront_error::StorefrontError;
use crate::application::service::tax_resolve_port::{RefusingTaxResolvePort, TaxResolvePort};
use crate::application::service::wishlist_service;

/// The header carrying the visitor token (read-only identity).
pub const VISITOR_TOKEN_HEADER: &str = "x-storefront-token";

/// The env var declaring whether this module's traffic arrives through
/// a trusted reverse proxy (`true` → the RIGHTMOST `X-Forwarded-For`
/// hop is the caller's address for the per-IP throttle arm; anything
/// else ignores the forwarded header entirely — every hop is then
/// client-supplied text).
pub const TRUSTED_PROXY_ENV: &str = "STOREFRONT_TRUSTED_PROXY";

// ── the fixed-window throttle (per-identity + per-IP, fail-closed) ──────────

/// In-memory fixed-window throttle (per key). Windows are wall-clock
/// buckets measured from the first hit inside the window; a poisoned
/// lock fails CLOSED into the live map (never wedges the verb).
#[derive(Debug, Default)]
pub struct FixedWindows {
    inner: Mutex<HashMap<String, (u64, u64)>>, // key -> (window_start_unix, count)
}

impl FixedWindows {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a hit; returns false when the key is over budget in the
    /// current window.
    pub fn allow(&self, key: &str, max: u64, window_secs: u64) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let Ok(mut guard) = self.inner.lock() else {
            return false; // fail closed
        };
        match guard.get_mut(key) {
            Some((start, count)) => {
                if now.saturating_sub(*start) >= window_secs {
                    *start = now;
                    *count = 0;
                }
            }
            None => {
                guard.insert(key.to_string(), (now, 0));
            }
        }
        let Some((_, count)) = guard.get_mut(key) else {
            return false;
        };
        if *count >= max {
            return false;
        }
        *count += 1;
        true
    }
}

/// The per-verb throttle budgets (hits per `WINDOW_SECS`).
const WRITE_BUDGET: u64 = 60;
const CHECKOUT_BUDGET: u64 = 12;
const WINDOW_SECS: u64 = 60;

/// The env var overriding the comparison read's server-side cap.
pub const COMPARE_CAP_ENV: &str = "STOREFRONT_COMPARE_CAP";

/// The comparison read's SERVER-SIDE cap (the client cannot raise it):
/// default 4, env-tunable, hard-clamped to 1..=20 — the read's fan-out
/// (a gated detail + a fresh availability read per row) stays bounded
/// whatever the query string says.
pub fn compare_cap() -> usize {
    std::env::var(COMPARE_CAP_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 20)
}

// ── state ────────────────────────────────────────────────────────────────────

/// The shared public state (cheap-to-clone handles; the pool rides
/// along for the verbs that open their own lock transactions).
#[derive(Clone)]
pub struct StorefrontPublicState {
    pub pool: sqlx::PgPool,
    /// Hostname binding — website's exported surface (the spec's named
    /// resolution path), so the storefront and the website resolve the
    /// same host to the same row.
    pub surface: Arc<dyn WebsiteSurface>,
    principal_port: Arc<dyn WebsitePrincipalVerifier>,
    pub catalog: Arc<dyn CatalogReadPort>,
    pub party: Arc<dyn PartyWritePort>,
    pub tax: Arc<dyn TaxResolvePort>,
    pub pricing: Arc<dyn CartPricingPort>,
    pub availability: Arc<dyn AvailabilityReadPort>,
    pub checkout: Arc<CheckoutDeps>,
    throttle: Arc<FixedWindows>,
    trusted_proxy: bool,
}

impl StorefrontPublicState {
    /// Compose over explicit handles — the host's one-call wiring (the
    /// SAME `CartPricingPort` adapter selling uses; one adapter, two
    /// consumers; the availability adapter rides over the host's
    /// inventory/manufacturing composition).
    pub fn compose(
        pool: sqlx::PgPool,
        surface: Arc<dyn WebsiteSurface>,
        catalog: Arc<dyn CatalogReadPort>,
        party: Arc<dyn PartyWritePort>,
        tax: Arc<dyn TaxResolvePort>,
        pricing: Arc<dyn CartPricingPort>,
        availability: Arc<dyn AvailabilityReadPort>,
    ) -> Self {
        let checkout = Arc::new(CheckoutDeps::new(
            pool.clone(),
            catalog.clone(),
            party.clone(),
            tax.clone(),
            pricing.clone(),
            availability.clone(),
        ));
        Self {
            pool,
            surface,
            principal_port: Arc::new(RefusingPrincipalVerifier),
            catalog,
            party,
            tax,
            pricing,
            availability,
            checkout,
            throttle: Arc::new(FixedWindows::new()),
            trusted_proxy: trusted_proxy_from_env(),
        }
    }

    /// [`Self::compose`] with the FAIL-CLOSED defaults: website's own
    /// Pg surface over the pool, the refusing catalog/party/tax/
    /// availability ports (an uncomposed module refuses loudly, never
    /// silently functional), and a pricing port that refuses every cart.
    pub fn from_env(pool: sqlx::PgPool) -> Self {
        let surface = Arc::new(backbone_website::exports::PgWebsiteSurface::new(
            pool.clone(),
            std::env::var("WEBSITE_VISITOR_PEPPER").unwrap_or_default(),
        ));
        Self::compose(
            pool,
            surface,
            Arc::new(RefusingCatalogReadPort),
            Arc::new(RefusingPartyWritePort),
            Arc::new(RefusingTaxResolvePort),
            Arc::new(RefusingCartPricing),
            Arc::new(RefusingAvailabilityReadPort),
        )
    }

    /// The principal-port install seam (the host bridges portal's
    /// verification surface here; unwired, no bearer verifies — the
    /// bind/adopt/members_only arms stay closed, never fail-open).
    pub fn install_principal_verifier(&mut self, verifier: Arc<dyn WebsitePrincipalVerifier>) {
        self.principal_port = verifier;
    }

    async fn principal(&self, headers: &HeaderMap) -> Option<WebsitePrincipal> {
        let presented = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        self.principal_port.verify(presented).await
    }
}

/// The pricing-port default for [`StorefrontPublicState::from_env`]:
/// every cart prices as refused — loud, never a zero-total fallback.
struct RefusingCartPricing;

#[async_trait::async_trait]
impl CartPricingPort for RefusingCartPricing {
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
                message: "no pricing adapter is composed".into(),
            },
        )
    }
}

/// The trusted-proxy posture from [`TRUSTED_PROXY_ENV`]: tolerant truth
/// arms the proxy posture; unset or any other value keeps direct
/// connections.
fn trusted_proxy_from_env() -> bool {
    std::env::var(TRUSTED_PROXY_ENV)
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"))
        .unwrap_or(false)
}

/// The caller address for the per-IP throttle arm only (never
/// authorization): `trusted_proxy` → the RIGHTMOST forwarded hop;
/// otherwise the socket address with the header ignored entirely.
fn caller_ip(
    headers: &HeaderMap,
    remote_addr: Option<std::net::SocketAddr>,
    trusted_proxy: bool,
) -> String {
    let socket = remote_addr.map(|a| a.to_string());
    if trusted_proxy {
        if let Some(hop) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return hop.to_string();
        }
    }
    socket.unwrap_or_else(|| "unknown".to_string())
}

// ── the shared gates ─────────────────────────────────────────────────────────

/// Map a typed service error to its HTTP shape (status + machine code;
/// the rate arm carries `Retry-After`; internal shapes never leak
/// their text).
pub fn storefront_error_response(err: StorefrontError) -> Response {
    let status = StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if let StorefrontError::RateLimited { retry_after_seconds } = &err {
        return (
            status,
            [
                ("retry-after", retry_after_seconds.to_string()),
                ("content-type", "application/json".into()),
            ],
            Json(json!({
                "error": err.to_string(),
                "code": err.code(),
                "retry_after_seconds": retry_after_seconds,
            })),
        )
            .into_response();
    }
    let body = match &err {
        StorefrontError::Db(e) => {
            tracing::error!(error = ?e, "storefront public route internal error");
            json!({"error": "internal error", "code": err.code()})
        }
        StorefrontError::Internal(msg) => {
            tracing::error!(reason = %msg, "storefront public route internal error");
            json!({"error": "internal error", "code": err.code()})
        }
        other => json!({"error": other.to_string(), "code": err.code()}),
    };
    (status, Json(body)).into_response()
}

/// Gate (1): hostname binding — the Host header (normalized) → the
/// live website through the exported surface. Miss → the typed 404; NO
/// fallback to any first website.
async fn bound_website(
    state: &StorefrontPublicState,
    headers: &HeaderMap,
) -> Result<WebsiteView, Response> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    match state.surface.resolve_website_by_host(&normalize_host(host)).await {
        Ok(view) => Ok(view),
        Err(_) => Err(storefront_error_response(StorefrontError::WebsiteNotResolved)),
    }
}

/// Gates (2)+(3): the identity ladder and the members_only arm. Under
/// `members_only`, every public verb requires the verified principal.
async fn gate_identity(
    state: &StorefrontPublicState,
    headers: &HeaderMap,
    website: &WebsiteView,
) -> Result<(Option<Uuid>, Option<WebsitePrincipal>), Response> {
    let principal = state.principal(headers).await;
    if members_only(&state.pool, website.id).await.unwrap_or(false) && principal.is_none() {
        return Err(storefront_error_response(StorefrontError::PrincipalRequired));
    }
    let token = headers
        .get(VISITOR_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let visitor = match token {
        Some(t) => match cart_service::visitor_by_token(&state.pool, website.id, t).await {
            Ok(v) => v,
            Err(e) => return Err(storefront_error_response(e)),
        },
        None => None,
    };
    Ok((visitor, principal))
}

/// Gate (4): the write-verb throttle — per identity (visitor or
/// principal) AND per IP, both must pass.
fn gate_throttle(
    state: &StorefrontPublicState,
    verb: &str,
    visitor: Option<Uuid>,
    principal: Option<&WebsitePrincipal>,
    ip: &str,
    budget: u64,
) -> Result<(), Response> {
    let identity = visitor
        .map(|v| v.to_string())
        .or_else(|| principal.map(|p| p.user_uuid().to_string()))
        .unwrap_or_else(|| "anonymous".into());
    let ok = state.throttle.allow(&format!("{verb}:id:{identity}"), budget, WINDOW_SECS)
        && state.throttle.allow(&format!("{verb}:ip:{ip}"), budget, WINDOW_SECS);
    if ok {
        Ok(())
    } else {
        Err(storefront_error_response(StorefrontError::RateLimited {
            retry_after_seconds: WINDOW_SECS as i64,
        }))
    }
}

/// The visitor's open cart (the cart verbs' identity fence — a token
/// with no open cart is the typed 404, never another identity's cart).
async fn own_open_cart(
    state: &StorefrontPublicState,
    visitor: Option<Uuid>,
    principal: Option<&WebsitePrincipal>,
) -> Result<CartRow, Response> {
    if let Some(v) = visitor {
        if let Ok(Some(cart)) = cart_service::open_cart_for_visitor(&state.pool, v).await {
            return Ok(cart);
        }
    }
    if let Some(p) = principal {
        if let Ok(Some(cart)) =
            cart_service::most_recent_open_cart_for_principal(&state.pool, p.user_uuid()).await
        {
            return Ok(cart);
        }
    }
    Err(storefront_error_response(StorefrontError::CartNotFound))
}

/// The locked delivery verb's cart resolution: the identity's open cart,
/// or — when none is open — the identity's most recent cart in any
/// state, so the in-lock state gate answers with the typed not-open
/// refusal (§7.1(b)'s closed-window proof) instead of a bare 404 that
/// would hide the closed window entirely.
async fn own_cart_or_recent(
    state: &StorefrontPublicState,
    visitor: Option<Uuid>,
    principal: Option<&WebsitePrincipal>,
) -> Result<CartRow, Response> {
    if let Ok(cart) = own_open_cart(state, visitor, principal).await {
        return Ok(cart);
    }
    if let Some(v) = visitor {
        if let Ok(Some(cart)) = cart_service::latest_cart_for_visitor(&state.pool, v).await {
            return Ok(cart);
        }
    }
    if let Some(p) = principal {
        if let Ok(Some(cart)) =
            cart_service::latest_cart_for_principal(&state.pool, p.user_uuid()).await
        {
            return Ok(cart);
        }
    }
    Err(storefront_error_response(StorefrontError::CartNotFound))
}

/// Serialize the priced cart view (the §5.2 derivation's answer).
fn priced_cart_json(cart: &CartRow, view: &PricedCartView) -> serde_json::Value {
    json!({
        "cart_id": cart.id,
        "state": cart.state,
        "coupon_code": cart.coupon_code,
        "delivery_carrier_id": cart.delivery_carrier_id,
        "billing_party_id": cart.party_id,
        "fulfillment_mode": cart.fulfillment_mode,
        "pickup_location_id": cart.pickup_location_id,
        "subtotal": view.subtotal,
        "currency": view.currency,
        "customer_group_id": view.customer_group_id,
        "unavailable_count": view.unavailable_count,
        "reward_lines": view.reward_lines.iter().map(|r| json!({
            "item_id": r.item_id,
            "name": r.name,
            "quantity": r.quantity,
        })).collect::<Vec<_>>(),
        "lines": view.lines.iter().map(|l| json!({
            "line_id": l.line_id,
            "item_id": l.item_id,
            "name": l.name,
            "quantity": l.quantity,
            "list_price": l.list_price,
            "unit_price": l.unit_price,
            "net_line_total": l.net_line_total,
            "unavailable": l.unavailable,
        })).collect::<Vec<_>>(),
    })
}

/// Price one cart through the SINGLE derivation (a read-only helper —
/// zero writes; the conn is acquired, never a transaction).
async fn price_view(
    state: &StorefrontPublicState,
    company_id: Uuid,
    cart: &CartRow,
) -> Result<PricedCartView, Response> {
    let lines = match cart_service::lines_of(&state.pool, cart.id).await {
        Ok(l) => l,
        Err(e) => return Err(storefront_error_response(e)),
    };
    let mut conn = match state.pool.acquire().await {
        Ok(c) => c,
        Err(e) => return Err(storefront_error_response(StorefrontError::Db(e))),
    };
    match pricing_service::price_cart(
        &mut conn,
        state.catalog.as_ref(),
        state.party.as_ref(),
        state.pricing.as_ref(),
        company_id,
        cart,
        &lines,
    )
    .await
    {
        Ok(view) => Ok(view),
        Err(e) => Err(storefront_error_response(e)),
    }
}

// ── request DTOs (typed allowlists; unknown keys dropped) ───────────────────

#[derive(Debug, Deserialize)]
struct CatalogQuery {
    q: Option<String>,
    sort: Option<String>,
    page: Option<i64>,
    page_size: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AddLineBody {
    item_id: Uuid,
    quantity: rust_decimal::Decimal,
}

#[derive(Debug, Deserialize)]
struct SetQuantityBody {
    quantity: rust_decimal::Decimal,
}

#[derive(Debug, Deserialize)]
struct CouponBody {
    code: String,
}

#[derive(Debug, Deserialize)]
struct BillingBody {
    email: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeliveryBody {
    carrier_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct AdoptBody {
    cart_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct PlaceBody {
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpressBody {
    email: String,
    name: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PickupBody {
    location_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct WishlistAddBody {
    item_id: Uuid,
}

// ── handlers ────────────────────────────────────────────────────────────────

async fn catalog_list(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
    Query(q): Query<CatalogQuery>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_identity(&state, &headers, &website).await {
        return resp;
    }
    let sort = match q.sort.as_deref() {
        None => SortKind::Relevance,
        Some(raw) => match SortKind::parse(raw) {
            Some(s) => s,
            None => {
                return storefront_error_response(StorefrontError::InvalidSort);
            }
        },
    };
    match catalog_service::public_listings(
        &state.pool,
        state.catalog.as_ref(),
        website.company_id,
        website.id,
        q.q.as_deref(),
        sort,
        q.page.unwrap_or(1),
        q.page_size.unwrap_or(20),
    )
    .await
    {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({
                "website_id": website.id,
                "items": rows.iter().map(|r| json!({
                    "listing_id": r.listing_id,
                    "item_id": r.item_id,
                    "name": r.name,
                    "sequence": r.sequence,
                    "media_urls": r.media_urls,
                    "list_price": r.list_price,
                    "compare_at_price": r.compare_at_price,
                    "currency": r.currency,
                    "created_at": r.created_at,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn catalog_detail(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_identity(&state, &headers, &website).await {
        return resp;
    }
    match catalog_service::public_detail(
        &state.pool,
        state.catalog.as_ref(),
        website.company_id,
        website.id,
        item_id,
    )
    .await
    {
        Ok(item) => (
            StatusCode::OK,
            Json(json!({
                "listing_id": item.listing_id,
                "item_id": item.item_id,
                "name": item.name,
                "sequence": item.sequence,
                "media_urls": item.media_urls,
                "list_price": item.list_price,
                "compare_at_price": item.compare_at_price,
                "currency": item.currency,
                "created_at": item.created_at,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn catalog_categories(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_identity(&state, &headers, &website).await {
        return resp;
    }
    match catalog_service::public_categories(
        &state.pool,
        state.catalog.as_ref(),
        website.company_id,
        website.id,
    )
    .await
    {
        Ok(categories) => (
            StatusCode::OK,
            Json(json!({
                "website_id": website.id,
                "categories": categories.iter().map(|c| json!({
                    "group_id": c.group_id,
                    "name": c.name,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_read(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(_visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    // ZERO writes on every path: the read derives; it never purges,
    // never touches the abandonment clock, never mints identity.
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    match price_view(&state, website.company_id, &cart).await {
        Ok(view) => (StatusCode::OK, Json(priced_cart_json(&cart, &view))).into_response(),
        Err(resp) => resp,
    }
}

async fn cart_create(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    // Requires an EXISTING visitor token — the storefront never mints
    // visitor rows (§4.3); a missing/unknown token is the typed 401.
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) =
        gate_throttle(&state, "cart_create", Some(visitor_id), principal.as_ref(), &ip, WRITE_BUDGET)
    {
        return resp;
    }
    match cart_service::create_cart(&state.pool, website.id, visitor_id).await {
        Ok(created) => (
            StatusCode::OK,
            Json(json!({
                "cart_id": created.cart.id,
                "state": created.cart.state,
                "created": created.created,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_add_line(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<AddLineBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "line_add",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match cart_service::add_line(
        &state.pool,
        state.catalog.as_ref(),
        state.availability.as_ref(),
        website.company_id,
        &cart,
        body.item_id,
        body.quantity,
    )
    .await
    {
        Ok(line) => (
            StatusCode::CREATED,
            Json(json!({
                "line_id": line.id,
                "item_id": line.item_id,
                "quantity": line.quantity,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_set_line(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Path(line_id): Path<Uuid>,
    Json(body): Json<SetQuantityBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "line_set",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match cart_service::set_line_quantity(
        &state.pool,
        state.catalog.as_ref(),
        state.availability.as_ref(),
        website.company_id,
        &cart,
        line_id,
        body.quantity,
    )
    .await
    {
        Ok(line) => (
            StatusCode::OK,
            Json(json!({
                "line_id": line.id,
                "item_id": line.item_id,
                "quantity": line.quantity,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_remove_line(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Path(line_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "line_remove",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match cart_service::remove_line(&state.pool, &cart, line_id).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "removed": true }))).into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_apply_coupon(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<CouponBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "coupon_apply",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    // Uniform refusal text either way (no enumeration oracle): a
    // well-formed-but-unknown code and a malformed one read the same.
    match cart_service::apply_coupon(&state.pool, &cart, &body.code).await {
        Ok(code) => (StatusCode::OK, Json(json!({ "coupon_code": code }))).into_response(),
        Err(_) => storefront_error_response(StorefrontError::CouponRefused),
    }
}

async fn cart_remove_coupon(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "coupon_remove",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match cart_service::remove_coupon(&state.pool, &cart).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "coupon_code": null }))).into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_billing(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<BillingBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "billing",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    let email = cart_service::normalize_email(&body.email);
    // The verb RE-PRICES in the same response (the explicit ripple —
    // a fiscal re-resolution may change the totals).
    let stamped = match checkout_service::capture_billing(
        &state.checkout,
        website.company_id,
        cart.id,
        &email,
        body.name.as_deref(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => return storefront_error_response(e),
    };
    match price_view(&state, website.company_id, &stamped).await {
        Ok(view) => {
            (StatusCode::OK, Json(priced_cart_json(&stamped, &view))).into_response()
        }
        Err(resp) => resp,
    }
}

async fn cart_delivery(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<DeliveryBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_cart_or_recent(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "delivery",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match checkout_service::set_delivery(
        &state.checkout,
        website.company_id,
        cart.id,
        body.carrier_id,
    )
    .await
    {
        Ok(stamped) => (
            StatusCode::OK,
            Json(json!({
                "cart_id": stamped.id,
                "delivery_carrier_id": stamped.delivery_carrier_id,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

/// `POST /public/session/bind` — mutates NOTHING: verify the portal
/// principal against the current visitor lineage and ANSWER the
/// adoptable-cart query (§2.3). The answer carries the informational
/// ticket (never a capability — adopt re-verifies server-side).
async fn session_bind(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(principal) = principal else {
        return storefront_error_response(StorefrontError::PrincipalRequired);
    };
    let portal_user_id = principal.user_uuid();
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(&state, "bind", visitor, Some(&principal), &ip, WRITE_BUDGET) {
        return resp;
    }
    match cart_service::most_recent_open_cart_for_principal(&state.pool, portal_user_id).await {
        Ok(Some(cart)) => (
            StatusCode::OK,
            Json(json!({
                "adoptive": true,
                "cart_id": cart.id,
                "adoption_ticket": cart_service::adoption_ticket(cart.id, portal_user_id),
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::OK,
            Json(json!({ "adoptive": false, "cart_id": null, "adoption_ticket": null })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_adopt(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<AdoptBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(principal) = principal else {
        return storefront_error_response(StorefrontError::PrincipalRequired);
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) =
        gate_throttle(&state, "adopt", Some(visitor_id), Some(&principal), &ip, WRITE_BUDGET)
    {
        return resp;
    }
    match cart_service::adopt_cart(
        &state.pool,
        website.id,
        visitor_id,
        principal.user_uuid(),
        body.cart_id,
    )
    .await
    {
        Ok(cart) => (
            StatusCode::OK,
            Json(json!({
                "cart_id": cart.id,
                "state": cart.state,
                "portal_user_id": cart.portal_user_id,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn checkout_place(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<PlaceBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "checkout",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        CHECKOUT_BUDGET,
    ) {
        return resp;
    }
    // The client sends NO amount — the total derives under the lock.
    match checkout_service::place(
        &state.checkout,
        website.company_id,
        cart.id,
        None,
        body.notes,
    )
    .await
    {
        Ok(checkout) => (StatusCode::CREATED, Json(checkout_json(&checkout))).into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn checkout_read(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
    Path(checkout_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    // Ownership: the checkout's own cart lineage (visitor OR principal
    // linkage); a foreign checkout is indistinguishable from a missing
    // one. A pure derived read — zero writes.
    let checkout = match checkout_service::checkout_by_id(&state.pool, checkout_id).await {
        Ok(Some(c)) if c.website_id == website.id => c,
        Ok(_) => return storefront_error_response(StorefrontError::CheckoutNotFound),
        Err(e) => return storefront_error_response(e),
    };
    let cart = match cart_service::cart_by_id(&state.pool, checkout.cart_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return storefront_error_response(StorefrontError::CheckoutNotFound),
        Err(e) => return storefront_error_response(e),
    };
    let owned = visitor.is_some_and(|v| v == cart.visitor_id)
        || principal.as_ref().is_some_and(|p| Some(p.user_uuid()) == cart.portal_user_id);
    if !owned {
        return storefront_error_response(StorefrontError::CheckoutNotFound);
    }
    let order_state = match checkout.sales_order_id {
        Some(order_id) => checkout_service::order_state_of(&state.pool, website.company_id, order_id)
            .await
            .unwrap_or(None),
        None => None,
    };
    let mut body = checkout_json(&checkout);
    if let Some((status, total, currency)) = order_state {
        body["order_status"] = json!(status);
        body["order_total"] = json!(total);
        body["order_currency"] = json!(currency);
    }
    (StatusCode::OK, Json(body)).into_response()
}

async fn express_checkout(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<ExpressBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "express",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        CHECKOUT_BUDGET,
    ) {
        return resp;
    }
    // ONE verb = deterministic billing capture + place, ONE lock
    // scope (§6.1's express row); the client still sends no amount.
    match checkout_service::place(
        &state.checkout,
        website.company_id,
        cart.id,
        Some((body.email, body.name)),
        body.notes,
    )
    .await
    {
        Ok(checkout) => (StatusCode::CREATED, Json(checkout_json(&checkout))).into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn abandoned_read(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    if visitor.is_none() && principal.is_none() {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    }
    // Own carts only — the identity fence is in the read's predicate.
    match recovery_service::abandoned_carts_for_identity(
        &state.pool,
        visitor.unwrap_or_else(Uuid::new_v4),
        principal.map(|p| p.user_uuid()),
        recovery_service::abandoned_after_hours(),
    )
    .await
    {
        Ok(carts) => (
            StatusCode::OK,
            Json(json!({
                "carts": carts.iter().map(|c| json!({
                    "cart_id": c.id,
                    "website_id": c.website_id,
                    "state": c.state,
                    "updated_at": c.updated_at,
                    "line_count": c.line_count,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn cart_recover(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Path(cart_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "recover",
        Some(visitor_id),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match cart_service::recover_cart(
        &state.pool,
        cart_id,
        visitor_id,
        principal.map(|p| p.user_uuid()),
    )
    .await
    {
        Ok(cart) => (
            StatusCode::OK,
            Json(json!({ "cart_id": cart.id, "state": cart.state })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

// ── Click & Collect (§14.2) ────────────────────────────────────────────────

/// The PUBLIC store lookup: active locations for the bound website. A
/// PURE READ — it switches NO carrier, mints NO cart, writes nothing;
/// the pin verb is the only writer of a cart's pickup linkage. The
/// warehouse id never leaves the server (it is an inventory pointer
/// the pin resolves server-side; the shopper sees a store, not a
/// warehouse).
async fn collect_locations(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_identity(&state, &headers, &website).await {
        return resp;
    }
    match collect_service::active_locations_for_website(&state.pool, website.id).await {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({
                "website_id": website.id,
                "locations": rows.iter().map(|l| json!({
                    "location_id": l.id,
                    "name": l.name,
                    "address_line1": l.address_line1,
                    "city": l.city,
                    "postal_code": l.postal_code,
                    "country": l.country,
                    "latitude": l.latitude,
                    "longitude": l.longitude,
                    "opening_hours": l.opening_hours,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

/// The pickup PIN: the client presents ONLY the opaque location id;
/// warehouse, address, and fiscal country resolve server-side inside
/// the verb. Delivery carriers are untouched (a pickup cart simply has
/// none until it resets to delivery).
async fn cart_pickup(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<PickupBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "pickup_set",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match checkout_service::set_pickup(&state.checkout, website.company_id, cart.id, body.location_id)
        .await
    {
        Ok((cart, location)) => (
            StatusCode::OK,
            Json(json!({
                "cart_id": cart.id,
                "fulfillment_mode": cart.fulfillment_mode,
                "pickup_location_id": location.id,
                "location_name": location.name,
                "delivery_carrier_id": cart.delivery_carrier_id,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

/// The lane RESET: back to delivery. The pickup linkage clears; the
/// carrier stays whatever the delivery verb last set (possibly none —
/// place re-checks the delivery lane's own requirement).
async fn cart_pickup_reset(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "pickup_reset",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match checkout_service::reset_fulfillment(&state.checkout, cart.id).await {
        Ok(cart) => (
            StatusCode::OK,
            Json(json!({
                "cart_id": cart.id,
                "fulfillment_mode": cart.fulfillment_mode,
                "pickup_location_id": cart.pickup_location_id,
                "delivery_carrier_id": cart.delivery_carrier_id,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

/// The pay-on-site lane's PLACE: the THIRD checkout arm. The order
/// mints DRAFT with NO gateway row and NOTHING auto-confirms — the
/// answer is `pending_pickup` until an officer confirms the store took
/// the money. Requires a pickup-mode cart (a shipping cart cannot
/// promise payment at a store).
async fn checkout_place_on_site(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<PlaceBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let cart = match own_open_cart(&state, visitor, principal.as_ref()).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "checkout",
        visitor.or(Some(cart.visitor_id)),
        principal.as_ref(),
        &ip,
        CHECKOUT_BUDGET,
    ) {
        return resp;
    }
    match checkout_service::place_on_site(&state.checkout, website.company_id, cart.id, None, body.notes)
        .await
    {
        Ok(checkout) => (StatusCode::CREATED, Json(checkout_json(&checkout))).into_response(),
        Err(e) => storefront_error_response(e),
    }
}

// ── availability + comparison (§14.1/§14.4) ────────────────────────────────

/// The DISPLAY-scope availability read for one item: the publish gate
/// first (a closed-door item's stock is not a public fact), then the
/// fresh port read under the website's display scope. Fail-loud: an
/// unwired availability port refuses with the typed 503 rather than
/// inventing a number.
async fn availability_read(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_identity(&state, &headers, &website).await {
        return resp;
    }
    // The closed door first: no listing, no number.
    if let Err(e) =
        cart_service::gated_listing(&state.pool, state.catalog.as_ref(), website.company_id, website.id, item_id)
            .await
    {
        return storefront_error_response(e);
    }
    let scope = match availability_service::display_scope_warehouse(&state.pool, website.id).await {
        Ok(s) => s,
        Err(e) => return storefront_error_response(e),
    };
    match state
        .availability
        .free_quantity(website.company_id, item_id, scope)
        .await
    {
        Ok(answer) => (
            StatusCode::OK,
            Json(json!({
                "item_id": item_id,
                "free_quantity": answer.free_quantity,
                "kit_exploded": answer.kit_exploded,
                "warehouse_id": scope,
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(availability_service::map_availability_error(e)),
    }
}

/// The comparison read: stateless, publish-gate-filtered, capped
/// SERVER-SIDE (the client cannot raise the cap — an oversized request
/// is the typed 422, never a truncated answer that silently drops
/// rows). Availability badges ride the same fresh display-scope read.
async fn compare_read(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    if let Err(resp) = gate_identity(&state, &headers, &website).await {
        return resp;
    }
    // Repeated `item_id=` keys, order-preserving, deduplicated — UUIDs
    // need no percent-decoding.
    let mut item_ids: Vec<Uuid> = Vec::new();
    for pair in raw.as_deref().unwrap_or("").split('&') {
        let Some(value) = pair.strip_prefix("item_id=") else {
            continue;
        };
        match Uuid::parse_str(value) {
            Ok(id) if !item_ids.contains(&id) => item_ids.push(id),
            Ok(_) => {}
            Err(_) => {
                return storefront_error_response(StorefrontError::InvalidInput(
                    "item_id must be a uuid".into(),
                ))
            }
        }
    }
    let cap = compare_cap();
    if item_ids.len() > cap {
        return storefront_error_response(StorefrontError::ComparisonCapExceeded {
            cap,
            requested: item_ids.len(),
        });
    }
    if item_ids.is_empty() {
        return storefront_error_response(StorefrontError::InvalidInput(
            "at least one item_id is required".into(),
        ));
    }
    // Gate + detail per item (bounded by the cap); a closed-door item
    // is simply ABSENT (the shared closed-door shape — the comparison
    // never reveals an unpublished item's existence).
    let mut rows = Vec::with_capacity(item_ids.len());
    let mut gated_ids = Vec::with_capacity(item_ids.len());
    for item_id in &item_ids {
        match catalog_service::public_detail(
            &state.pool,
            state.catalog.as_ref(),
            website.company_id,
            website.id,
            *item_id,
        )
        .await
        {
            Ok(item) => {
                gated_ids.push(*item_id);
                rows.push((item, None::<rust_decimal::Decimal>));
            }
            Err(StorefrontError::PublishGateRefused) => continue,
            Err(e) => return storefront_error_response(e),
        }
    }
    if !gated_ids.is_empty() {
        let scope =
            match availability_service::display_scope_warehouse(&state.pool, website.id).await {
                Ok(s) => s,
                Err(e) => return storefront_error_response(e),
            };
        let answers = match state
            .availability
            .free_quantities(website.company_id, &gated_ids, scope)
            .await
        {
            Ok(a) => a,
            Err(e) => return storefront_error_response(availability_service::map_availability_error(e)),
        };
        for row in rows.iter_mut() {
            if let Some(answer) = answers.iter().find(|a| a.item_id == row.0.item_id) {
                row.1 = Some(answer.free_quantity);
            }
        }
    }
    (
        StatusCode::OK,
        Json(json!({
            "website_id": website.id,
            "cap": cap,
            "items": rows.iter().map(|(item, free)| json!({
                "item_id": item.item_id,
                "name": item.name,
                "media_urls": item.media_urls,
                "list_price": item.list_price,
                "compare_at_price": item.compare_at_price,
                "currency": item.currency,
                "free_quantity": free,
            })).collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

// ── wishlist (§14.3) ────────────────────────────────────────────────────────

/// The union read: the visitor's own rows plus the verified
/// principal's stamped rows, website-scoped. Derived — zero writes.
async fn wishlist_read(
    State(state): State<StorefrontPublicState>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    match wishlist_service::wishlist_for(
        &state.pool,
        website.id,
        visitor_id,
        principal.map(|p| p.user_uuid()),
    )
    .await
    {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({
                "website_id": website.id,
                "items": rows.iter().map(|r| json!({
                    "item_id": r.item_id,
                    "notify_on_stock": r.notify_on_stock,
                })).collect::<Vec<_>>(),
            })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn wishlist_add(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Json(body): Json<WishlistAddBody>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) =
        gate_throttle(&state, "wishlist_add", Some(visitor_id), principal.as_ref(), &ip, WRITE_BUDGET)
    {
        return resp;
    }
    match wishlist_service::add(
        &state.pool,
        state.catalog.as_ref(),
        website.company_id,
        website.id,
        visitor_id,
        body.item_id,
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(json!({ "wishlist_item_id": id, "item_id": body.item_id })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

async fn wishlist_remove(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "wishlist_remove",
        Some(visitor_id),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match wishlist_service::remove(
        &state.pool,
        website.id,
        visitor_id,
        principal.map(|p| p.user_uuid()),
        item_id,
    )
    .await
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "removed": true, "item_id": item_id }))).into_response(),
        Err(e) => storefront_error_response(e),
    }
}

/// The login-time reconcile: stamps the visitor's live rows with the
/// verified principal (rows never move — the stamp is what the union
/// read follows). Requires BOTH rungs: a visitor token AND a verified
/// principal.
async fn wishlist_reconcile(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let Some(principal) = principal else {
        return storefront_error_response(StorefrontError::PrincipalRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "wishlist_reconcile",
        Some(visitor_id),
        Some(&principal),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match wishlist_service::reconcile(
        &state.pool,
        website.id,
        visitor_id,
        principal.user_uuid(),
        &principal.email,
    )
    .await
    {
        Ok(stamped) => (
            StatusCode::OK,
            Json(json!({ "reconciled": stamped, "visitor_id": visitor_id })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

/// Arm the back-in-stock wait on a wish the caller already holds. A
/// verified principal present at arm time refreshes the contact stamp
/// (the verified-email-only rule's one other writer — no request body
/// address is ever accepted).
async fn wishlist_notify(
    State(state): State<StorefrontPublicState>,
    connect_info: Option<ConnectInfo<std::net::SocketAddr>>,
    headers: HeaderMap,
    Path(item_id): Path<Uuid>,
) -> Response {
    let website = match bound_website(&state, &headers).await {
        Ok(w) => w,
        Err(resp) => return resp,
    };
    let (visitor, principal) = match gate_identity(&state, &headers, &website).await {
        Ok(pair) => pair,
        Err(resp) => return resp,
    };
    let Some(visitor_id) = visitor else {
        return storefront_error_response(StorefrontError::VisitorTokenRequired);
    };
    let ip = caller_ip(&headers, connect_info.map(|c| c.0), state.trusted_proxy);
    if let Err(resp) = gate_throttle(
        &state,
        "wishlist_notify",
        Some(visitor_id),
        principal.as_ref(),
        &ip,
        WRITE_BUDGET,
    ) {
        return resp;
    }
    match wishlist_service::arm_notify(
        &state.pool,
        website.id,
        visitor_id,
        item_id,
        principal.map(|p| (p.user_uuid(), p.email)),
    )
    .await
    {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "notify_on_stock": true, "item_id": item_id })),
        )
            .into_response(),
        Err(e) => storefront_error_response(e),
    }
}

fn checkout_json(checkout: &checkout_service::CheckoutRow) -> serde_json::Value {
    json!({
        "checkout_id": checkout.id,
        "cart_id": checkout.cart_id,
        "website_id": checkout.website_id,
        "sales_order_id": checkout.sales_order_id,
        "gateway_transaction_id": checkout.gateway_transaction_id,
        "provider_code": checkout.provider_code,
        "provider_reference": checkout.provider_reference,
        "amount_total": checkout.amount_total,
        "state": checkout.state,
        "pickup_location_id": checkout.pickup_location_id,
        "placed_at": checkout.placed_at,
        "settled_at": checkout.settled_at,
    })
}

/// The exported public router — EXACTLY the §6.1 table; nothing else
/// answers unauthenticated. Every mutating verb is a POST.
pub fn storefront_public_routes(state: StorefrontPublicState) -> Router {
    Router::new()
        .route("/public/catalog", get(catalog_list))
        .route("/public/catalog/:item_id", get(catalog_detail))
        .route("/public/categories", get(catalog_categories))
        .route("/public/availability/:item_id", get(availability_read))
        .route("/public/compare", get(compare_read))
        .route("/public/collect/locations", get(collect_locations))
        .route("/public/cart", get(cart_read).post(cart_create))
        .route("/public/cart/lines", post(cart_add_line))
        .route("/public/cart/lines/:line_id", post(cart_set_line))
        .route("/public/cart/lines/:line_id/remove", post(cart_remove_line))
        .route("/public/cart/coupon", post(cart_apply_coupon))
        .route("/public/cart/coupon/remove", post(cart_remove_coupon))
        .route("/public/cart/billing", post(cart_billing))
        .route("/public/cart/delivery", post(cart_delivery))
        .route("/public/cart/pickup", post(cart_pickup))
        .route("/public/cart/pickup/reset", post(cart_pickup_reset))
        .route("/public/session/bind", post(session_bind))
        .route("/public/cart/adopt", post(cart_adopt))
        .route("/public/checkout", post(checkout_place))
        .route("/public/checkout/on-site", post(checkout_place_on_site))
        .route("/public/checkout/:checkout_id", get(checkout_read))
        .route("/public/express", post(express_checkout))
        .route("/public/abandoned", get(abandoned_read))
        .route("/public/cart/:cart_id/recover", post(cart_recover))
        .route("/public/wishlist", get(wishlist_read).post(wishlist_add))
        .route("/public/wishlist/reconcile", post(wishlist_reconcile))
        .route("/public/wishlist/:item_id/remove", post(wishlist_remove))
        .route("/public/wishlist/:item_id/notify", post(wishlist_notify))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_window_budgets_fail_closed() {
        let windows = FixedWindows::new();
        for i in 0..3 {
            assert!(windows.allow("k", 3, 60), "hit {i} must pass");
        }
        assert!(!windows.allow("k", 3, 60), "the 4th hit must refuse");
        assert!(windows.allow("other", 3, 60), "keys are independent");
    }

    #[test]
    fn visitor_token_header_is_stable() {
        assert_eq!(VISITOR_TOKEN_HEADER, "x-storefront-token");
    }
}
