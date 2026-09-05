//! The checkout critical sections (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! THE ROW LOCK (§7.1): the checkout's own spine row —
//! `storefront.carts` — is taken `SELECT .. FOR UPDATE` at the top of
//! every critical section that can change what a payment charges:
//! place (`POST /public/checkout`), delivery change, billing capture,
//! and express (it IS a place). Lock scope is ONE database
//! transaction; released at commit/rollback; no cross-request locks.
//! A concurrent delivery change therefore either completes before
//! place takes the lock (place re-prices under the lock and records the
//! FINAL total) or blocks until place commits and then reads
//! `state='placed'` and takes the typed 409 — there is NO interleaving
//! in which a pending transaction row exists with a total a later
//! carrier change could invalidate. The gateway's verified-settle
//! money gate re-checks the locked amount against the provider's
//! authority numbers at settle time (belt and braces).
//!
//! PLACE (§7.2), inside the lock: re-derive final pricing through the
//! SAME single derivation the display reads use → resolve the
//! order-level fiscal rate through the tax port → mint the priced
//! order via selling's `create_sales_order_priced` (the order is born
//! DRAFT; the totals are promo-conserved) → record the checkout
//! session with the LOCKED total → arm payment (paid) or confirm
//! directly (free arm).
//!
//! The gateway transaction row is created through the gateway's PLAIN
//! CRUD service (it stays plain CRUD — no change there) INSIDE the
//! lock scope, with the storefront-minted reference `stf-{checkout_id}`
//! as `provider_transaction_id`.
//!
//! SETTLEMENT → CONFIRM (§7.4): settling stays the substrate's
//! (verified webhook → ingest → money-gated, exactly-once transition);
//! [`consume_settlement`] is the storefront's idempotent consumer the
//! host bridge calls on the settled seam event — first delivery stamps
//! `settled` and confirms the order; a redelivery no-ops (the
//! transition already happened; selling's own not-draft refusal is the
//! second guard).
//!
//! The module NEVER consumes selling's event-sink arming constructor —
//! the service is composed with its default tracing publisher and none
//! of its outbound events drive storefront behavior.

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_orm::company_scope;
use backbone_payment_gateway::application::service::GatewayTransactionSettled;
use backbone_payment_gateway::presentation::dto::CreateGatewayTransactionDto;
use backbone_payment_gateway::GatewayTransactionService;
use backbone_selling::application::service::{
    CartOrderLine, CartPricingPort, NewCartSalesOrder, NoServiceCatalog, NoServiceDelivery,
    NoStockFulfillmentPort, NoUnitCostPort, SellingError, SellingWriteService,
};

use super::audit::{record_audit, ActorRef};
use super::availability_port::AvailabilityReadPort;
use super::availability_service;
use super::cart_service::{self, CartRow};
use super::catalog_read_port::CatalogReadPort;
use super::party_write_port::PartyWritePort;
use super::pricing_service;
use super::storefront_error::StorefrontError;
use super::tax_resolve_port::TaxResolvePort;

/// The checkout session row as reads and the consumer see it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CheckoutRow {
    pub id: Uuid,
    pub cart_id: Uuid,
    pub website_id: Uuid,
    pub sales_order_id: Option<Uuid>,
    pub gateway_transaction_id: Option<Uuid>,
    pub provider_code: Option<String>,
    pub provider_reference: Option<String>,
    pub amount_total: Decimal,
    pub state: String,
    pub pickup_location_id: Option<Uuid>,
    pub placed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub settled_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Column-qualified: the admin join pairs `checkout_sessions` with
/// `website.websites`, where a bare `id` would be an ambiguous-column
/// error — every column names its table so single-table and joined
/// reads share one select.
const CHECKOUT_SELECT: &str = r#"
    SELECT checkout_sessions.id, checkout_sessions.cart_id, checkout_sessions.website_id,
           checkout_sessions.sales_order_id, checkout_sessions.gateway_transaction_id,
           checkout_sessions.provider_code, checkout_sessions.provider_reference,
           checkout_sessions.amount_total,
           checkout_sessions.state::text AS state, checkout_sessions.pickup_location_id,
           checkout_sessions.placed_at, checkout_sessions.settled_at
    FROM storefront.checkout_sessions
"#;

/// One checkout by id (live row).
pub async fn checkout_by_id(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    checkout_id: Uuid,
) -> Result<Option<CheckoutRow>, StorefrontError> {
    sqlx::query_as::<_, CheckoutRow>(&format!(
        "{CHECKOUT_SELECT} WHERE id = $1 AND (metadata->>'deleted_at') IS NULL"
    ))
    .bind(checkout_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The live checkout bound to one gateway transaction — the settlement
/// consumer's resolution key (partial unique among live rows).
async fn checkout_by_gateway_tx(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    gateway_transaction_id: Uuid,
) -> Result<Option<CheckoutRow>, StorefrontError> {
    sqlx::query_as::<_, CheckoutRow>(&format!(
        "{CHECKOUT_SELECT} WHERE gateway_transaction_id = $1 \
         AND (metadata->>'deleted_at') IS NULL LIMIT 1"
    ))
    .bind(gateway_transaction_id)
    .fetch_optional(exec)
    .await
    .map_err(StorefrontError::from)
}

/// Map selling's refusals onto the storefront's typed shapes.
fn map_selling_error(e: SellingError) -> StorefrontError {
    match e {
        SellingError::EmptyDocument => StorefrontError::EmptyCart,
        SellingError::OrderNotFound(id) => StorefrontError::NotFound(format!("order {id}")),
        SellingError::NotDraft(current) => {
            // The idempotency double guard: a redelivered settlement
            // finds the order already confirmed — that is success, not
            // refusal. Callers that expect it match before mapping.
            StorefrontError::Guarded(format!("order no longer draft: {current}"))
        }
        SellingError::InvalidTransition { verb, current } => {
            StorefrontError::Guarded(format!("{verb} refused in state {current}"))
        }
        SellingError::PricingRejected { code, .. } => {
            StorefrontError::PricingRefused { code }
        }
        other => StorefrontError::Internal(format!("selling refused: {other}")),
    }
}

/// The company's active payment provider row (company-scoped; the
/// per-WEBSITE provider pivot is a later increment's row — recorded,
/// not designed here).
struct ProviderRow {
    id: Uuid,
    code: String,
}

async fn active_provider(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    company_id: Uuid,
) -> Result<Option<ProviderRow>, StorefrontError> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, code::text
        FROM payment_gateway.payment_gateway_providers
        WHERE company_id = $1 AND status = 'active'
          AND (metadata->>'deleted_at') IS NULL
        ORDER BY code ASC
        LIMIT 1
        "#,
    )
    .bind(company_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.map(|(id, code)| ProviderRow { id, code }))
}

/// The carrier-ownership check (the clean-404 posture, mirrored from
/// selling's create-time validation): the carrier must be one of THIS
/// company's live carriers.
pub async fn carrier_owned_by_company(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    company_id: Uuid,
    carrier_id: Uuid,
) -> Result<bool, StorefrontError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT 1::int8
        FROM selling.delivery_carriers
        WHERE id = $1 AND company_id = $2 AND active = true
          AND (metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(carrier_id)
    .bind(company_id)
    .fetch_optional(exec)
    .await?;
    Ok(row.is_some())
}

/// THE LOCKED CART READ: `SELECT .. FOR UPDATE` on the cart row. Must
/// run FIRST inside the critical section's transaction; everything
/// after it runs holding the row lock.
async fn lock_cart(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cart_id: Uuid,
) -> Result<CartRow, StorefrontError> {
    sqlx::query_as::<_, CartRow>(&format!(
        "{} WHERE id = $1 AND (metadata->>'deleted_at') IS NULL FOR UPDATE",
        cart_service::CART_SELECT
    ))
    .bind(cart_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::RowNotFound => StorefrontError::CartNotFound,
        other => StorefrontError::Db(other),
    })
}

/// The deps every checkout critical section holds. The pricing and
/// availability ports are HOST-composed adapters; the gateway service
/// stays the plain CRUD service. The availability port is REQUIRED for
/// a functional checkout: the place-time stock gate fails closed
/// without it (the typed 503 — a place never promises stock nobody
/// read).
pub struct CheckoutDeps {
    pub pool: sqlx::PgPool,
    pub catalog: std::sync::Arc<dyn CatalogReadPort>,
    pub party: std::sync::Arc<dyn PartyWritePort>,
    pub tax: std::sync::Arc<dyn TaxResolvePort>,
    pub pricing: std::sync::Arc<dyn CartPricingPort>,
    pub availability: std::sync::Arc<dyn AvailabilityReadPort>,
    pub selling: std::sync::Arc<SellingWriteService>,
    pub gateway: GatewayTransactionService,
}

impl CheckoutDeps {
    /// Compose over one pool: selling's write service with its DEFAULT
    /// tracing publisher (the module never arms an alternate event
    /// destination), the gateway's plain transaction service.
    pub fn new(
        pool: sqlx::PgPool,
        catalog: std::sync::Arc<dyn CatalogReadPort>,
        party: std::sync::Arc<dyn PartyWritePort>,
        tax: std::sync::Arc<dyn TaxResolvePort>,
        pricing: std::sync::Arc<dyn CartPricingPort>,
        availability: std::sync::Arc<dyn AvailabilityReadPort>,
    ) -> Self {
        Self {
            selling: std::sync::Arc::new(SellingWriteService::new(pool.clone())),
            gateway: gateway_service(&pool),
            pool,
            catalog,
            party,
            tax,
            pricing,
            availability,
        }
    }
}

/// The gateway's plain transaction service (CRUD only — the substrate
/// keeps its guarded transitions on the settle side; this module adds
/// the checkout-side lock around CREATION, which stays plain here).
pub fn gateway_service(pool: &sqlx::PgPool) -> GatewayTransactionService {
    GatewayTransactionService::with_repository(std::sync::Arc::new(
        backbone_payment_gateway::GatewayTransactionRepository::new(pool.clone()),
    ))
}

/// The locked delivery change (§7.1): validate the carrier against the
/// company's registry, stamp it UNDER THE LOCK. A cart that left `open`
/// while blocked on the lock takes the typed 409 here.
pub async fn set_delivery(
    deps: &CheckoutDeps,
    company_id: Uuid,
    cart_id: Uuid,
    carrier_id: Uuid,
) -> Result<CartRow, StorefrontError> {
    let mut tx = deps.pool.begin().await?;
    let cart = lock_cart(&mut tx, cart_id).await?;
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    // RLS scope (ADR-0008): the carrier-registry read below targets a
    // FORCE-RLS selling table; the company predicate alone does not
    // fence it — bind the company onto this transaction so the read
    // resolves on the scoped session.
    company_scope::bind_company_on(&mut tx, company_id).await?;
    if !carrier_owned_by_company(&mut *tx, company_id, carrier_id).await? {
        return Err(StorefrontError::CarrierNotFound);
    }
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET delivery_carrier_id = $2,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1
        "#,
    )
    .bind(cart_id)
    .bind(carrier_id)
    .execute(&mut *tx)
    .await?;
    record_audit(
        &mut *tx,
        Some(cart.website_id),
        "delivery_set",
        ActorRef::visitor(cart.visitor_id),
        Some("cart"),
        Some(cart_id),
        Some(serde_json::json!({ "delivery_carrier_id": carrier_id })),
    )
    .await?;
    tx.commit().await?;
    cart_service::cart_by_id(&deps.pool, cart_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("cart vanished after delivery set".into()))
}

/// The locked CLICK & COLLECT PIN (§14.2): the client presents ONLY the
/// opaque location id — the store row (its warehouse, its address, its
/// fiscal country) resolves SERVER-SIDE here, validated against the
/// cart's own website (a foreign or inactive store is the closed-door
/// 404). The pin does NOT touch `delivery_carrier_id` (the carrier
/// stays the delivery verb's lane; a lookup switches no carrier and
/// this verb mints nothing).
pub async fn set_pickup(
    deps: &CheckoutDeps,
    company_id: Uuid,
    cart_id: Uuid,
    location_id: Uuid,
) -> Result<(CartRow, super::collect_service::PickupLocationRow), StorefrontError> {
    let mut tx = deps.pool.begin().await?;
    let cart = lock_cart(&mut tx, cart_id).await?;
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    company_scope::bind_company_on(&mut tx, company_id).await?;
    let location = super::collect_service::active_location_on_website(
        &mut *tx,
        cart.website_id,
        location_id,
    )
    .await?
    .ok_or(StorefrontError::PickupLocationNotFound)?;
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET fulfillment_mode = 'pickup', pickup_location_id = $2,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1
        "#,
    )
    .bind(cart_id)
    .bind(location_id)
    .execute(&mut *tx)
    .await?;
    record_audit(
        &mut *tx,
        Some(cart.website_id),
        "pickup_set",
        ActorRef::visitor(cart.visitor_id),
        Some("cart"),
        Some(cart_id),
        Some(serde_json::json!({ "pickup_location_id": location_id })),
    )
    .await?;
    tx.commit().await?;
    let stamped = cart_service::cart_by_id(&deps.pool, cart_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("cart vanished after pickup set".into()))?;
    Ok((stamped, location))
}

/// The locked lane RESET (§14.2): back to delivery — the pickup pin
/// clears (the store linkage on the cart goes away; nothing else
/// moves, and the carrier stays whatever the delivery verb set).
pub async fn reset_fulfillment(
    deps: &CheckoutDeps,
    cart_id: Uuid,
) -> Result<CartRow, StorefrontError> {
    let mut tx = deps.pool.begin().await?;
    let cart = lock_cart(&mut tx, cart_id).await?;
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET fulfillment_mode = 'delivery', pickup_location_id = NULL,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1
        "#,
    )
    .bind(cart_id)
    .execute(&mut *tx)
    .await?;
    record_audit(
        &mut *tx,
        Some(cart.website_id),
        "fulfillment_mode_reset",
        ActorRef::visitor(cart.visitor_id),
        Some("cart"),
        Some(cart_id),
        None,
    )
    .await?;
    tx.commit().await?;
    cart_service::cart_by_id(&deps.pool, cart_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("cart vanished after lane reset".into()))
}

/// The locked billing capture (§7.1): fiscal re-resolution can move the
/// tax arm, so capture runs inside the lock. The re-price the response
/// carries happens EXPLICITLY in the same verb (the caller re-derives
/// through the single pricing derivation after this returns).
pub async fn capture_billing(
    deps: &CheckoutDeps,
    company_id: Uuid,
    cart_id: Uuid,
    email_normalized: &str,
    name: Option<&str>,
) -> Result<CartRow, StorefrontError> {
    let mut tx = deps.pool.begin().await?;
    let cart = lock_cart(&mut tx, cart_id).await?;
    if cart.state != "open" {
        return Err(StorefrontError::CartNotOpen { state: cart.state.clone() });
    }
    // Same ADR-0008 bind as the other locked verbs: the party port's
    // cross-schema reads on this transaction run fenced to the cart's
    // company.
    company_scope::bind_company_on(&mut tx, company_id).await?;
    let party_id = cart_service::resolve_shopper_party(
        &mut *tx,
        deps.party.as_ref(),
        company_id,
        email_normalized,
        name,
    )
    .await?;
    cart_service::stamp_billing_party(
        &mut *tx,
        cart_id,
        cart.website_id,
        cart.visitor_id,
        party_id,
    )
    .await?;
    tx.commit().await?;
    cart_service::cart_by_id(&deps.pool, cart_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("cart vanished after billing capture".into()))
}

/// The payment lane a place runs under (§14.2). `Online` is the P1
/// pair (free arm / paid arm). `OnSite` is the Click & Collect
/// pay-at-store lane: the order mints DRAFT, NO gateway row is
/// created, and NOTHING auto-confirms — the officer confirm-pickup
/// verb (after the store physically took payment) is the only
/// confirmer. A zero-total on-site place takes the SAME lane (one
/// uniform lane beats mixed arms inside a lane).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentLane {
    Online,
    OnSite,
}

/// PLACE — the ONLINE lane (the §7.2/§7.3/§7.5 critical section): free
/// arm on a zero locked total, paid arm otherwise. See
/// [`place_with_lane`] for the ordered work inside the lock.
pub async fn place(
    deps: &CheckoutDeps,
    company_id: Uuid,
    cart_id: Uuid,
    billing: Option<(String, Option<String>)>,
    notes: Option<String>,
) -> Result<CheckoutRow, StorefrontError> {
    place_with_lane(deps, company_id, cart_id, billing, notes, PaymentLane::Online).await
}

/// PLACE, PAY ON SITE (§14.2) — the third checkout lane. Requires a
/// pickup cart (the typed 422 on a delivery cart — a shipping order
/// cannot promise payment at a store); the order mints DRAFT, the
/// session records `pending_pickup` with NO gateway row, and NOTHING
/// auto-confirms. An unpaid on-site order stays draft until the store
/// takes payment and the officer confirm-pickup verb runs (or the
/// cancel mirror retires it).
pub async fn place_on_site(
    deps: &CheckoutDeps,
    company_id: Uuid,
    cart_id: Uuid,
    billing: Option<(String, Option<String>)>,
    notes: Option<String>,
) -> Result<CheckoutRow, StorefrontError> {
    place_with_lane(deps, company_id, cart_id, billing, notes, PaymentLane::OnSite).await
}

/// PLACE — the row-locked critical section shared by both lanes. With
/// `billing`, this is the EXPRESS verb (deterministic capture + place,
/// one lock scope); without, plain checkout (billing must already be
/// captured — the typed 409 otherwise).
///
/// Order of work inside the lock: price → stock gate (checkout scope,
/// computed fresh) → fiscal rate (the pickup store's country when the
/// cart is pinned — server-derived, never client JSON) → mint the
/// priced order → locked total → session row → the lane's arm (online:
/// pending gateway transaction inside the lock scope, or the free
/// arm's direct confirm; on-site: `pending_pickup`, no gateway row,
/// no confirm). The mint and the gateway create run on their own
/// pool transactions INSIDE the lock scope (they are single-verb
/// substrate calls); the cart's `state='placed'` flip and the session
/// row commit in the OUTER locked transaction, so a failed mint leaves
/// the cart open and NO session row exists.
async fn place_with_lane(
    deps: &CheckoutDeps,
    company_id: Uuid,
    cart_id: Uuid,
    billing: Option<(String, Option<String>)>,
    notes: Option<String>,
    lane: PaymentLane,
) -> Result<CheckoutRow, StorefrontError> {
    let mut tx = deps.pool.begin().await?;
    let cart = lock_cart(&mut tx, cart_id).await?;
    if cart.state != "open" {
        // The closed door, not the typed not-open refusal: a racing place
        // that loses the lock must read ONE deterministic answer (the
        // identity-scoped 404), whatever interleaving it landed in — the
        // not-open code is reserved for the verbs that PROVE the closed
        // window (delivery's §7.1(b) refusal).
        return Err(StorefrontError::CartNotFound);
    }
    // The Click & Collect invariants, under the lock (§14.2): the
    // on-site lane exists ONLY for pickup carts; a pickup cart's pinned
    // store must still be live here (the store row is BOTH the
    // fulfillment origin's resolver and the fiscal source — its country
    // is the jurisdiction the tax port resolves under, server-derived;
    // the client never sends a warehouse or a jurisdiction). A live
    // store with NO country is a typed refusal, never a fallback: the
    // only arm a countryless store could reach is the delivery/home
    // jurisdiction, which would mint pickup tax under the wrong country
    // silently. The column is NOT NULL and the upsert requires the
    // country, so this guard is the code-level defense behind those
    // fences (a legacy or bypass-written row still refuses here).
    if lane == PaymentLane::OnSite && cart.fulfillment_mode != "pickup" {
        return Err(StorefrontError::PickupModeRequired);
    }
    let (pickup_country, pickup_pin) = if cart.fulfillment_mode == "pickup" {
        let Some(location_id) = cart.pickup_location_id else {
            return Err(StorefrontError::PickupLocationNotFound);
        };
        let row: Option<(Option<String>,)> = sqlx::query_as(
            r#"
            SELECT country
            FROM storefront.pickup_locations
            WHERE id = $1 AND is_active = true
              AND (metadata->>'deleted_at') IS NULL
            LIMIT 1
            "#,
        )
        .bind(location_id)
        .fetch_optional(&mut *tx)
        .await?;
        match row {
            Some((Some(country),)) => (Some(country), Some(location_id)),
            // Unreachable by construction (NOT NULL column + required
            // upsert) — and refused loudly regardless, so
            // resolve_rate(company_id, None) — the home arm — can never
            // carry a pickup order.
            Some((None,)) => return Err(StorefrontError::PickupCountryMissing),
            None => return Err(StorefrontError::PickupLocationNotFound),
        }
    } else {
        (None, None)
    };
    // RLS scope (ADR-0008): bind the cart's company onto the locked
    // transaction, so every cross-schema read executed ON this
    // transaction (the carrier-registry check in the delivery verbs,
    // the active-provider read below) is fenced to this company. The
    // storefront's own tables are deliberately predicate-only (no RLS),
    // so the bind costs them nothing and scopes only the fenced reads.
    company_scope::bind_company_on(&mut tx, company_id).await?;

    // Express arm: deterministic billing capture INSIDE the same lock.
    let party_id = match billing {
        Some((email, name)) => {
            let email = cart_service::normalize_email(&email);
            if email.is_empty() || !email.contains('@') {
                return Err(StorefrontError::InvalidInput("a valid email is required".into()));
            }
            let party_id = cart_service::resolve_shopper_party(
                &mut *tx,
                deps.party.as_ref(),
                company_id,
                &email,
                name.as_deref(),
            )
            .await?;
            cart_service::stamp_billing_party(
                &mut *tx,
                cart_id,
                cart.website_id,
                cart.visitor_id,
                party_id,
            )
            .await?;
            party_id
        }
        None => cart.party_id.ok_or(StorefrontError::BillingRequired)?,
    };

    let lines = cart_service::lines_of(&mut *tx, cart_id).await?;
    if lines.is_empty() {
        return Err(StorefrontError::EmptyCart);
    }

    // (1) Final pricing through the SINGLE derivation, under the lock.
    let priced = pricing_service::price_cart(
        &mut *tx,
        deps.catalog.as_ref(),
        deps.party.as_ref(),
        deps.pricing.as_ref(),
        company_id,
        &cart,
        &lines,
    )
    .await?;
    if priced.unavailable_count > 0 {
        // A line's item left the gate since its mutation — the place
        // refuses; the read surface has been showing the line as
        // unavailable the whole time.
        return Err(StorefrontError::PublishGateRefused);
    }

    // (1b) The place-time stock gate (§14.1): EVERY line re-checked in
    // the cart's checkout scope (the pinned store's warehouse for a
    // pickup cart, the company aggregate otherwise), computed fresh
    // through the port. Backorder-allowed listings skip their check;
    // an unwired port fails the whole place closed (the typed 503).
    let line_pairs: Vec<(Uuid, Decimal)> =
        lines.iter().map(|l| (l.item_id, l.quantity)).collect();
    availability_service::gate_place_quantities(
        &mut *tx,
        deps.availability.as_ref(),
        company_id,
        &cart,
        &line_pairs,
    )
    .await?;

    // (2) Order-level fiscal rate (§5.3). A pickup cart resolves under
    // the STORE's country (the server-side fiscal pin); a delivery
    // cart passes no jurisdiction (the adapter's home-rate arm).
    let tax_rate = deps
        .tax
        .resolve_rate(company_id, pickup_country.as_deref())
        .await
        .map_err(|e| StorefrontError::TaxPortRefused { code: e.code })?;

    // (3) Mint the priced order (born DRAFT; promo-conserved totals),
    // (read back the locked total), and (4)/(5) arm payment — all inside
    // a request-dedicated company scope. These legs touch FORCE-RLS
    // tables through the substrate services (selling's priced mint and
    // its read-back pre-pass, the gateway's provider read and pending
    // transaction insert, the free arm's confirm pre-reads); the ORM's
    // scoped helpers bind them to the scoped connection so the fenced
    // tables accept them. Unscoped, the public connection carries no
    // `app.company_id` and every one of those legs fails closed — the
    // mint would commit on its own self-scoped transaction while the
    // read-back goes blind and 404s beside a committed draft order.
    let (
        checkout_id,
        order_id,
        amount_total,
        (state, gateway_transaction_id, provider_code, provider_reference),
    ) = company_scope::with_request_scope(&deps.pool, company_id, async {
        let checkout_id = Uuid::new_v4();
        let order_number = format!("STF-{checkout_id}");
        let order_lines: Vec<CartOrderLine> = priced
            .lines
            .iter()
            .map(|l| CartOrderLine {
                item_id: l.item_id,
                // The group/brand dimensions were consumed by the pricing
                // request; the mint carries the same resolvable facts.
                item_group_id: None,
                brand_id: None,
                revenue_account_id: None,
                description: Some(l.name.clone()),
                list_price: l.list_price,
                quantity: l.quantity,
            })
            .collect();
        let order_id = deps
            .selling
            .create_sales_order_priced(
                NewCartSalesOrder {
                    order_number,
                    // The carrier the shopper chose under the checkout lock
                    // (set_delivery stamps it on the cart); the mint carries
                    // it onto the order so the fulfillment chain inherits it.
                    delivery_carrier_id: cart.delivery_carrier_id,
                    company_id,
                    branch_id: None,
                    customer_id: party_id,
                    customer_group_id: priced.customer_group_id,
                    coupon_code: cart.coupon_code.clone(),
                    order_date: chrono::Utc::now().date_naive(),
                    delivery_date: None,
                    currency: Some(priced.currency.clone()),
                    tax_rate,
                    notes,
                    lines: order_lines,
                },
                deps.pricing.as_ref(),
            )
            .await
            .map_err(map_selling_error)?;

        // The LOCKED total is the minted order's own total (authoritative —
        // it is what the gateway gross and the checkout session must both
        // equal).
        let order_ref = deps
            .selling
            .sales_order_ref(order_id)
            .await
            .map_err(map_selling_error)?;
        let amount_total = order_ref.grand_total;

        // (4)/(5) The arms, decided by the lane first, then the locked
        // total.
        let arms = if lane == PaymentLane::OnSite {
            // PAY-ON-SITE LANE (§14.2): the order stays DRAFT — no
            // gateway row, no confirm, no exception for a zero total.
            // The store takes payment physically; the officer
            // confirm-pickup verb (the only confirmer) runs after.
            record_audit(
                &mut *tx,
                Some(cart.website_id),
                "checkout_pending_pickup",
                ActorRef::visitor(cart.visitor_id),
                Some("checkout"),
                Some(checkout_id),
                Some(serde_json::json!({ "amount_total": amount_total })),
            )
            .await?;
            ("pending_pickup", None, None, None)
        } else if amount_total == Decimal::ZERO {
            // FREE ARM (§7.5): no gateway row; the order confirms at place.
            deps.selling
                .confirm_sales_order(
                    order_id,
                    company_id,
                    &NoUnitCostPort,
                    &NoStockFulfillmentPort,
                    &NoServiceCatalog,
                    &NoServiceDelivery,
                )
                .await
                .map_err(map_selling_error)?;
            record_audit(
                &mut *tx,
                Some(cart.website_id),
                "checkout_confirmed_free",
                ActorRef::visitor(cart.visitor_id),
                Some("checkout"),
                Some(checkout_id),
                None,
            )
            .await?;
            ("confirmed_free", None, None, None)
        } else {
            // PAID ARM (§7.3): the company's active provider mints the
            // PENDING gateway transaction INSIDE the lock scope, keyed by
            // the storefront-minted reference.
            let provider = active_provider(&mut *tx, company_id)
                .await?
                .ok_or(StorefrontError::ProviderUnavailable)?;
            let reference = format!("stf-{checkout_id}");
            let dto = CreateGatewayTransactionDto {
                company_id,
                provider_id: provider.id,
                provider_code: parse_provider_code(&provider.code),
                provider_transaction_id: reference.clone(),
                direction: backbone_payment_gateway::GatewayDirection::Receive,
                party_type: Some(backbone_payment_gateway::GatewayPartyType::Customer),
                party_id: Some(party_id),
                gross_amount: amount_total,
                fee_amount: Decimal::ZERO,
                net_amount: amount_total,
                currency: priced.currency.clone(),
                status: backbone_payment_gateway::GatewayTransactionStatus::Pending,
                posting_state: backbone_payment_gateway::GatewayPostingState::Pending,
                payment_entry_id: None,
                fee_post_id: None,
                settled_at: None,
                reference_no: None,
                raw_payload: None,
            };
            let gateway_tx = deps
                .gateway
                .create(dto)
                .await
                .map_err(|e| StorefrontError::Internal(format!("gateway create refused: {e}")))?;
            (
                "pending_payment",
                Some(gateway_tx.id),
                Some(provider.code),
                Some(reference),
            )
        };
        Ok::<_, StorefrontError>((
            checkout_id,
            order_id,
            amount_total,
            arms,
        ))
    })
    .await
    .map_err(StorefrontError::from)??;

    // The session row + the cart's placement flip, in the OUTER locked
    // transaction (a failure above leaves the cart open and no session).
    // The pickup pin rides the session (the fulfillment chain's record
    // of WHICH store this order collects from).
    sqlx::query(
        r#"
        INSERT INTO storefront.checkout_sessions
            (id, cart_id, website_id, sales_order_id, gateway_transaction_id,
             provider_code, provider_reference, amount_total, state,
             pickup_location_id, placed_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::storefront_checkout_state, $10, now())
        "#,
    )
    .bind(checkout_id)
    .bind(cart_id)
    .bind(cart.website_id)
    .bind(order_id)
    .bind(gateway_transaction_id)
    .bind(provider_code)
    .bind(provider_reference)
    .bind(amount_total)
    .bind(state)
    .bind(pickup_pin)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET state = 'placed', placed_at = now(),
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1
        "#,
    )
    .bind(cart_id)
    .execute(&mut *tx)
    .await?;
    record_audit(
        &mut *tx,
        Some(cart.website_id),
        "cart_placed",
        ActorRef::visitor(cart.visitor_id),
        Some("checkout"),
        Some(checkout_id),
        Some(serde_json::json!({ "amount_total": amount_total, "state": state })),
    )
    .await?;
    tx.commit().await?;

    checkout_by_id(&deps.pool, checkout_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("checkout vanished after place".into()))
}

fn parse_provider_code(code: &str) -> backbone_payment_gateway::GatewayProviderCode {
    match code {
        "midtrans" => backbone_payment_gateway::GatewayProviderCode::Midtrans,
        "xendit" => backbone_payment_gateway::GatewayProviderCode::Xendit,
        "doku" => backbone_payment_gateway::GatewayProviderCode::Doku,
        "stripe" => backbone_payment_gateway::GatewayProviderCode::Stripe,
        _ => backbone_payment_gateway::GatewayProviderCode::Manual,
    }
}

/// SETTLEMENT → CONFIRM (§7.4) — the idempotent consumer the host
/// bridge calls when the gateway's settled seam event arrives for a
/// transaction. First delivery: confirm the order (still draft — an
/// UNPAID order is never auto-confirmed before settlement), then stamp
/// the session `settled`. A redelivery finds `state='settled'` and
/// no-ops; selling's not-draft refusal is the second guard if the
/// stamp and the confirm ever race a crash between them (confirm
/// first, stamp second, so the crash window leaves a confirmable order
/// and an idempotent retry — never a confirmed order that cannot be
/// re-stamped).
///
/// Returns `Ok(None)` when the transaction binds no storefront
/// checkout (other transactions exist on the gateway; that is not this
/// consumer's lane).
pub async fn consume_settlement(
    deps: &CheckoutDeps,
    event: &GatewayTransactionSettled,
) -> Result<Option<CheckoutRow>, StorefrontError> {
    let Some(mut checkout) = checkout_by_gateway_tx(&deps.pool, event.gateway_transaction_id).await?
    else {
        return Ok(None);
    };
    if checkout.state == "settled" {
        return Ok(Some(checkout)); // redelivery — exactly-once by the state flip
    }
    if checkout.state != "pending_payment" {
        // A cancelled/failed checkout's late settlement lands on the
        // gateway's existing reversal path — the operator's
        // reconciliation verb, never an automatic refund.
        return Ok(Some(checkout));
    }
    let Some(order_id) = checkout.sales_order_id else {
        return Err(StorefrontError::Internal(
            "pending checkout carries no order id".into(),
        ));
    };
    // The website→company pairing is total and stored; the order's own
    // company is the confirm scope.
    let (company_id,): (Uuid,) = sqlx::query_as(
        r#"
        SELECT w.company_id
        FROM website.websites w
        WHERE w.id = $1 AND (w.metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(checkout.website_id)
    .fetch_one(&deps.pool)
    .await?;
    // RLS scope (ADR-0008): confirm's pre-reads touch FORCE-RLS selling
    // tables; the scoped helpers bind them to this company on a
    // request-dedicated connection (the webhook caller carries none).
    let confirmed = company_scope::with_request_scope(&deps.pool, company_id, async {
        deps.selling
            .confirm_sales_order(
                order_id,
                company_id,
                &NoUnitCostPort,
                &NoStockFulfillmentPort,
                &NoServiceCatalog,
                &NoServiceDelivery,
            )
            .await
    })
    .await
    .map_err(StorefrontError::from)?;
    match confirmed
    {
        Ok(()) => {}
        // The double guard: already confirmed by a crash-window retry —
        // proceed to stamp.
        Err(SellingError::NotDraft(_)) => {}
        Err(e) => return Err(map_selling_error(e)),
    }
    let stamped = sqlx::query(
        r#"
        UPDATE storefront.checkout_sessions
        SET state = 'settled', settled_at = now(),
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'pending_payment'
          AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(checkout.id)
    .execute(&deps.pool)
    .await?;
    if stamped.rows_affected() > 0 {
        record_audit(
            &deps.pool,
            Some(checkout.website_id),
            "checkout_settled_confirmed",
            ActorRef::system(),
            Some("checkout"),
            Some(checkout.id),
            None,
        )
        .await?;
    }
    checkout.state = "settled".into();
    checkout.settled_at = Some(chrono::Utc::now());
    Ok(Some(checkout))
}

/// The CANCEL MIRROR (§7.6) — a service-level verb (no public route in
/// the route table): a placed-but-unsettled checkout cancels its order
/// and stamps the session `cancelled`. The host wires shopper-cancel
/// and officer-void consumers over it; a LATER settlement for a
/// cancelled checkout rides the gateway's existing reversal path (the
/// operator's reconciliation, not an automatic refund).
pub async fn cancel_checkout(
    deps: &CheckoutDeps,
    checkout_id: Uuid,
) -> Result<CheckoutRow, StorefrontError> {
    let checkout = checkout_by_id(&deps.pool, checkout_id)
        .await?
        .ok_or(StorefrontError::CheckoutNotFound)?;
    if matches!(checkout.state.as_str(), "settled" | "cancelled" | "failed") {
        return Err(StorefrontError::CheckoutStateRefused { state: checkout.state.clone() });
    }
    let Some(order_id) = checkout.sales_order_id else {
        return Err(StorefrontError::Internal("checkout carries no order id".into()));
    };
    let (company_id,): (Uuid,) = sqlx::query_as(
        r#"
        SELECT w.company_id
        FROM website.websites w
        WHERE w.id = $1 AND (w.metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(checkout.website_id)
    .fetch_one(&deps.pool)
    .await?;
    // RLS scope (ADR-0008): the cancel verb's pre-reads touch FORCE-RLS
    // selling tables — same scoped wrap as the settlement confirm.
    company_scope::with_request_scope(&deps.pool, company_id, async {
        deps.selling
            .cancel_sales_order(order_id, company_id, &NoStockFulfillmentPort)
            .await
    })
    .await
    .map_err(StorefrontError::from)?
    .map_err(map_selling_error)?;
    sqlx::query(
        r#"
        UPDATE storefront.checkout_sessions
        SET state = 'cancelled',
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(checkout_id)
    .execute(&deps.pool)
    .await?;
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET state = 'cancelled',
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE id = $1
        "#,
    )
    .bind(checkout.cart_id)
    .execute(&deps.pool)
    .await?;
    record_audit(
        &deps.pool,
        Some(checkout.website_id),
        "cart_cancelled",
        ActorRef::system(),
        Some("checkout"),
        Some(checkout_id),
        None,
    )
    .await?;
    checkout_by_id(&deps.pool, checkout_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("checkout vanished after cancel".into()))
}

/// The OFFICER CONFIRM for pay-on-site (§14.2): the store collected the
/// physical payment, so the officer — and ONLY the officer — flips a
/// `pending_pickup` checkout to settled and confirms its order. This is
/// the sole confirmer of the on-site lane: the lane itself NEVER
/// auto-confirms (an unpaid order can only become an order someone
/// physically paid for at the counter, recorded here after the fact).
/// `payment_reference` is the officer's free-text receipt note (a
/// hand-written receipt number, a cash-drawer id) — optional, stamped
/// into the session's metadata for the audit trail.
pub async fn confirm_pickup(
    deps: &CheckoutDeps,
    checkout_id: Uuid,
    payment_reference: Option<&str>,
    actor: ActorRef,
) -> Result<CheckoutRow, StorefrontError> {
    let checkout = checkout_by_id(&deps.pool, checkout_id)
        .await?
        .ok_or(StorefrontError::CheckoutNotFound)?;
    if checkout.state != "pending_pickup" {
        return Err(StorefrontError::CheckoutStateRefused { state: checkout.state.clone() });
    }
    let Some(order_id) = checkout.sales_order_id else {
        return Err(StorefrontError::Internal("checkout carries no order id".into()));
    };
    let (company_id,): (Uuid,) = sqlx::query_as(
        r#"
        SELECT w.company_id
        FROM website.websites w
        WHERE w.id = $1 AND (w.metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(checkout.website_id)
    .fetch_one(&deps.pool)
    .await?;
    // RLS scope (ADR-0008): the confirm's pre-reads touch FORCE-RLS
    // selling tables — same scoped wrap as the settlement confirm.
    let confirmed = company_scope::with_request_scope(&deps.pool, company_id, async {
        deps.selling
            .confirm_sales_order(
                order_id,
                company_id,
                &NoUnitCostPort,
                &NoStockFulfillmentPort,
                &NoServiceCatalog,
                &NoServiceDelivery,
            )
            .await
    })
    .await
    .map_err(StorefrontError::from)?;
    match confirmed {
        Ok(()) => {}
        // Double guard: already confirmed by a crash-window retry —
        // proceed to stamp.
        Err(SellingError::NotDraft(_)) => {}
        Err(e) => return Err(map_selling_error(e)),
    }
    let stamped = sqlx::query(
        r#"
        UPDATE storefront.checkout_sessions
        SET state = 'settled', settled_at = now(),
            metadata = jsonb_set(
                jsonb_set(metadata, '{paid_on_site}', 'true'::jsonb),
                '{updated_at}', to_jsonb(now()))
        WHERE id = $1 AND state = 'pending_pickup'
          AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(checkout.id)
    .execute(&deps.pool)
    .await?;
    if stamped.rows_affected() > 0 {
        record_audit(
            &deps.pool,
            Some(checkout.website_id),
            "pickup_confirmed",
            actor,
            Some("checkout"),
            Some(checkout.id),
            Some(serde_json::json!({
                "payment_reference": payment_reference,
                "paid_on_site": true,
            })),
        )
        .await?;
    }
    checkout_by_id(&deps.pool, checkout_id)
        .await?
        .ok_or_else(|| StorefrontError::Internal("checkout vanished after pickup confirm".into()))
}

/// The checkout view's order-state read (read-only on the selling
/// table — the logical-ref posture). `selling.sales_orders` is
/// FORCE-RLS: the read rides the scoped fetch helper under the
/// cart-company's task-local scope, so the public connection (which
/// carries no company) still resolves the row; without it the
/// decoration silently drops.
pub async fn order_state_of(
    pool: &sqlx::PgPool,
    company_id: Uuid,
    order_id: Uuid,
) -> Result<Option<(String, rust_decimal::Decimal, String)>, StorefrontError> {
    company_scope::with_company_scope(Some(company_id), async {
        company_scope::fetch_optional_scoped::<(String, rust_decimal::Decimal, String)>(
            pool,
            sqlx::query_as::<_, (String, rust_decimal::Decimal, String)>(
                r#"
                SELECT status::text, total, currency
                FROM selling.sales_orders
                WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
                LIMIT 1
                "#,
            )
            .bind(order_id),
        )
        .await
        .map_err(StorefrontError::from)
    })
    .await
}

/// The officer/support checkout read (§6.2): the company's checkout
/// sessions newest-first, company scope via the website pairing. A
/// pure derived read.
pub async fn admin_checkouts(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    company_id: Uuid,
    limit: i64,
) -> Result<Vec<CheckoutRow>, StorefrontError> {
    sqlx::query_as::<_, CheckoutRow>(&format!(
        "{CHECKOUT_SELECT} \
         JOIN website.websites w ON w.id = checkout_sessions.website_id \
          AND (w.metadata->>'deleted_at') IS NULL \
         WHERE w.company_id = $1 AND (checkout_sessions.metadata->>'deleted_at') IS NULL \
         ORDER BY (checkout_sessions.metadata->>'created_at') DESC NULLS LAST, \
                  checkout_sessions.id DESC \
         LIMIT $2"
    ))
    .bind(company_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(exec)
    .await
    .map_err(StorefrontError::from)
}
