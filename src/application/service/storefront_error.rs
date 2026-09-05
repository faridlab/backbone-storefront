//! The storefront module's typed error surface (hand-written; user-owned;
//! see `metaphor.codegen.yaml`).
//!
//! Two rules shape every variant (the website error posture):
//!
//! 1. **Loud refusals, never silent degradation.** Every refusal is a
//!    TYPED failure with a stable machine code and an HTTP status — an
//!    unknown visitor token never silently mints one, an unwired port
//!    refuses loudly, and lifecycle violations name their state.
//! 2. **Existence is hidden on the public tree.** Unpublished and
//!    off-website listings share ONE not-found shape (404); a public
//!    caller cannot distinguish "no such product" from "not for sale".
//!    Uniform refusal text on the coupon arm (no enumeration oracle).

use thiserror::Error;
use uuid::Uuid;

/// The module error enum.
#[derive(Debug, Error)]
pub enum StorefrontError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("internal error: {0}")]
    Internal(String),

    // ── request binding / website resolution ───────────────────────────────

    /// The request's hostname binds to no live website (the loud 404;
    /// no fallback website exists).
    #[error("no live website is bound to this hostname")]
    WebsiteNotResolved,

    /// A public verb required a website that does not exist or belongs
    /// to another company (officer verbs).
    #[error("no such website")]
    WebsiteNotFound,

    // ── identity (the §2.1 ladder) ─────────────────────────────────────────

    /// The presented visitor token matches no live visitor row on this
    /// website. The storefront NEVER mints visitors — the typed 401, not
    /// a silent create.
    #[error("a valid visitor token is required")]
    VisitorTokenRequired,

    /// A members_only store (or the bind verb) requires a verified
    /// portal principal and none verified.
    #[error("a verified portal principal is required")]
    PrincipalRequired,

    // ── cart lifecycle ─────────────────────────────────────────────────────

    /// The identity's cart does not exist (read verbs answer the empty
    /// shape instead; mutation verbs carry the typed 404).
    #[error("no such cart for this identity")]
    CartNotFound,

    /// The cart is no longer open — placed/cancelled/closed carts
    /// refuse every mutation (the typed 409; the lock's post-state).
    #[error("this cart is no longer open (state: {state})")]
    CartNotOpen { state: String },

    /// The visitor already holds an open cart — adoption refuses (the
    /// partial unique's application-level mirror; no silent merge).
    #[error("this visitor already holds an open cart")]
    OpenCartExists,

    /// The named cart is not adoptable by this principal: it carries no
    /// portal linkage to them (a foreign cart is never adoptable).
    #[error("this cart is not linked to this principal")]
    CartNotAdoptable,

    /// The cart's line does not exist.
    #[error("no such line on this cart")]
    LineNotFound,

    /// The per-cart line bound (`STOREFRONT_MAX_CART_LINES`) fired.
    #[error("this cart is at its line bound")]
    LineLimitExceeded,

    /// A line quantity outside the positive decimal(18,4) domain.
    #[error("quantity must be a positive decimal")]
    InvalidQuantity,

    // ── the publish gate (§4) ──────────────────────────────────────────────

    /// The item failed the publish gate at mutation time (no live
    /// listing, sale_ok false, unpublished, inactive item, or no live
    /// price row). Uniform on the public tree: the detail read's 404
    /// and this mutation refusal share the closed-door shape.
    #[error("this item is not available on this website")]
    PublishGateRefused,

    /// A generic patch carried a publish-fenced field — the
    /// publish/unpublish verbs are the only writers.
    #[error("the {field} field moves only through its dedicated verb: {verb}")]
    FieldNotPatchable { field: &'static str, verb: &'static str },

    /// A sort parameter outside the closed vocabulary.
    #[error("sort must be one of the closed vocabulary")]
    InvalidSort,

    /// A `data:` URI reached the media validation (write refused; the
    /// object-storage-only contract).
    #[error("media urls must be object-storage references, never embedded payloads")]
    DataUriRefused,

    // ── pricing / coupon ───────────────────────────────────────────────────

    /// The pricing port refused the basket (a bad coupon code folds
    /// here — uniform text, no enumeration oracle).
    #[error("pricing refused this cart: {code}")]
    PricingRefused { code: String },

    /// The coupon apply verb's uniform refusal (malformed/empty code).
    #[error("coupon code not accepted")]
    CouponRefused,

    // ── checkout (§7) ──────────────────────────────────────────────────────

    /// Place requires a captured billing party first (the typed 409).
    #[error("billing must be captured before checkout")]
    BillingRequired,

    /// Place on a cart with no lines.
    #[error("cannot check out an empty cart")]
    EmptyCart,

    /// The company's carrier lookup missed (clean 404, never an FK 500).
    #[error("no such delivery carrier for this company")]
    CarrierNotFound,

    /// The checkout session does not exist (or belongs to another
    /// identity — the shared closed-door shape).
    #[error("no such checkout")]
    CheckoutNotFound,

    /// The checkout is in a state that refuses the requested move
    /// (settle on a cancelled checkout, cancel on a settled one).
    #[error("this checkout refuses this move (state: {state})")]
    CheckoutStateRefused { state: String },

    /// The company has no active payment provider row — the paid arm
    /// cannot mint a pending transaction.
    #[error("no active payment provider is configured for this company")]
    ProviderUnavailable,

    // ── fiscal port ────────────────────────────────────────────────────────

    /// The tax port is unwired or refused — a place never books under
    /// an unknown fiscal rate.
    #[error("tax resolution is unavailable: {code}")]
    TaxPortRefused { code: String },

    /// The party port is unwired or refused (billing capture, settings
    /// bootstrap).
    #[error("party resolution is unavailable: {code}")]
    PartyPortRefused { code: String },

    /// The catalog port is unwired (officer listing writes refuse
    /// rather than write rows the read gate can never honor).
    #[error("catalog reads are unavailable: {code}")]
    CatalogPortRefused { code: String },

    // ── availability / stock gate (§14.1) ──────────────────────────────────

    /// The availability port is unwired or refused — a clamped line
    /// mutation or a place never promises stock it did not read
    /// (fail-closed, never a zero/infinite fallback).
    #[error("availability reads are unavailable: {code}")]
    AvailabilityPortRefused { code: String },

    /// The stock gate fired: the requested quantity exceeds the
    /// checkout-scope free quantity (computed fresh at mutation time).
    #[error("insufficient stock for item {item_id}: requested {requested}, available {available}")]
    StockInsufficient {
        item_id: Uuid,
        requested: rust_decimal::Decimal,
        available: rust_decimal::Decimal,
    },

    // ── Click & Collect (§14.2) ────────────────────────────────────────────

    /// The pickup location does not exist, is inactive, or belongs to
    /// another website (the shared closed-door shape on the public
    /// tree).
    #[error("no such pickup location")]
    PickupLocationNotFound,

    /// A pinned pickup store carries no fiscal country. The store's
    /// country is the jurisdiction a pickup order's tax resolves under;
    /// without it the only reachable arm would be the delivery/home
    /// jurisdiction — a silent wrong-country tax, never a fallback. The
    /// place refuses loudly instead (the code-level guard standing
    /// behind the NOT NULL column).
    #[error("the pickup store has no fiscal country; a pickup order cannot resolve tax without it")]
    PickupCountryMissing,

    /// The registry upsert's warehouse pointer does not name one of the
    /// target website's company's warehouses (missing, deleted, or
    /// another company's). A store that fulfilled from a foreign
    /// company's warehouse would promise stock it can never read.
    #[error("the pickup warehouse must belong to this website's company")]
    PickupWarehouseRefused,

    /// The on-site payment lane is only offered to a cart in pickup
    /// mode — a shipping cart cannot promise payment at a store.
    #[error("pay on site requires a pickup cart")]
    PickupModeRequired,

    /// An unknown payment lane reached the checkout body (the closed
    /// vocabulary is online | on_site).
    #[error("payment lane must be 'online' or 'on_site'")]
    InvalidPaymentLane,

    // ── wishlist / comparison (§14.3/§14.4) ────────────────────────────────

    /// The identity's wishlist carries no such item (foreign rows are
    /// indistinguishable from missing ones — never a silent success).
    #[error("no such wishlist item")]
    WishlistItemNotFound,

    /// The comparison read's server-side cap fired.
    #[error("comparison is capped at {cap} items (sent {requested})")]
    ComparisonCapExceeded { cap: usize, requested: usize },

    // ── settings / recovery (§8) ───────────────────────────────────────────

    /// The website carries no sale-settings row yet (set it first).
    #[error("no sale settings exist for this website")]
    SettingsNotFound,

    /// The settings row carries no recovery template — there is no
    /// hardcoded fallback template anywhere.
    #[error("this website has no recovery template configured")]
    RecoveryTemplateRequired,

    /// The cart has no contactable address on file (no billing email,
    /// no portal principal email).
    #[error("this cart has no contact address on file")]
    NoContactAddress,

    // ── throttling ─────────────────────────────────────────────────────────

    /// A fixed-window throttle fired on a write verb.
    #[error("rate limited; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: i64 },

    // ── generic shapes (officer verbs) ─────────────────────────────────────

    /// Input validation refusal.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A requested record does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// A write was refused by a recorded guard.
    #[error("refused: {0}")]
    Guarded(String),
}

impl StorefrontError {
    /// The HTTP status the route layer maps this error to.
    pub fn http_status(&self) -> u16 {
        match self {
            StorefrontError::Db(_) | StorefrontError::Internal(_) => 500,
            StorefrontError::WebsiteNotResolved => 404,
            StorefrontError::WebsiteNotFound => 404,
            StorefrontError::VisitorTokenRequired => 401,
            StorefrontError::PrincipalRequired => 401,
            StorefrontError::CartNotFound => 404,
            StorefrontError::CartNotOpen { .. } => 409,
            StorefrontError::OpenCartExists => 409,
            StorefrontError::CartNotAdoptable => 409,
            StorefrontError::LineNotFound => 404,
            StorefrontError::LineLimitExceeded => 422,
            StorefrontError::InvalidQuantity => 422,
            StorefrontError::PublishGateRefused => 404,
            StorefrontError::FieldNotPatchable { .. } => 422,
            StorefrontError::InvalidSort => 422,
            StorefrontError::DataUriRefused => 422,
            StorefrontError::PricingRefused { .. } => 422,
            StorefrontError::CouponRefused => 422,
            StorefrontError::BillingRequired => 409,
            StorefrontError::EmptyCart => 422,
            StorefrontError::CarrierNotFound => 404,
            StorefrontError::CheckoutNotFound => 404,
            StorefrontError::CheckoutStateRefused { .. } => 409,
            StorefrontError::ProviderUnavailable => 503,
            StorefrontError::TaxPortRefused { .. } => 503,
            StorefrontError::PartyPortRefused { .. } => 503,
            StorefrontError::CatalogPortRefused { .. } => 503,
            StorefrontError::AvailabilityPortRefused { .. } => 503,
            StorefrontError::StockInsufficient { .. } => 422,
            StorefrontError::PickupLocationNotFound => 404,
            StorefrontError::PickupCountryMissing => 422,
            StorefrontError::PickupWarehouseRefused => 422,
            StorefrontError::PickupModeRequired => 422,
            StorefrontError::InvalidPaymentLane => 422,
            StorefrontError::WishlistItemNotFound => 404,
            StorefrontError::ComparisonCapExceeded { .. } => 422,
            StorefrontError::SettingsNotFound => 404,
            StorefrontError::RecoveryTemplateRequired => 422,
            StorefrontError::NoContactAddress => 422,
            StorefrontError::RateLimited { .. } => 429,
            StorefrontError::InvalidInput(_) => 400,
            StorefrontError::NotFound(_) => 404,
            StorefrontError::Guarded(_) => 422,
        }
    }

    /// The stable machine code the route layer emits.
    pub fn code(&self) -> &'static str {
        match self {
            StorefrontError::Db(_) => "storefront_internal_error",
            StorefrontError::Internal(_) => "storefront_internal_error",
            StorefrontError::WebsiteNotResolved => "storefront_website_not_resolved",
            StorefrontError::WebsiteNotFound => "storefront_website_not_found",
            StorefrontError::VisitorTokenRequired => "storefront_visitor_token_required",
            StorefrontError::PrincipalRequired => "storefront_principal_required",
            StorefrontError::CartNotFound => "storefront_cart_not_found",
            StorefrontError::CartNotOpen { .. } => "storefront_cart_not_open",
            StorefrontError::OpenCartExists => "storefront_open_cart_exists",
            StorefrontError::CartNotAdoptable => "storefront_cart_not_adoptable",
            StorefrontError::LineNotFound => "storefront_line_not_found",
            StorefrontError::LineLimitExceeded => "storefront_line_limit_exceeded",
            StorefrontError::InvalidQuantity => "storefront_invalid_quantity",
            StorefrontError::PublishGateRefused => "storefront_publish_gate_refused",
            StorefrontError::FieldNotPatchable { .. } => "storefront_field_not_patchable",
            StorefrontError::InvalidSort => "storefront_invalid_sort",
            StorefrontError::DataUriRefused => "storefront_data_uri_refused",
            StorefrontError::PricingRefused { .. } => "storefront_pricing_refused",
            StorefrontError::CouponRefused => "storefront_coupon_refused",
            StorefrontError::BillingRequired => "storefront_billing_required",
            StorefrontError::EmptyCart => "storefront_empty_cart",
            StorefrontError::CarrierNotFound => "storefront_carrier_not_found",
            StorefrontError::CheckoutNotFound => "storefront_checkout_not_found",
            StorefrontError::CheckoutStateRefused { .. } => "storefront_checkout_state_refused",
            StorefrontError::ProviderUnavailable => "storefront_provider_unavailable",
            StorefrontError::TaxPortRefused { .. } => "storefront_tax_port_refused",
            StorefrontError::PartyPortRefused { .. } => "storefront_party_port_refused",
            StorefrontError::CatalogPortRefused { .. } => "storefront_catalog_port_refused",
            StorefrontError::AvailabilityPortRefused { .. } => "storefront_availability_port_refused",
            StorefrontError::StockInsufficient { .. } => "storefront_stock_insufficient",
            StorefrontError::PickupLocationNotFound => "storefront_pickup_location_not_found",
            StorefrontError::PickupCountryMissing => "storefront_pickup_country_missing",
            StorefrontError::PickupWarehouseRefused => "storefront_pickup_warehouse_refused",
            StorefrontError::PickupModeRequired => "storefront_pickup_mode_required",
            StorefrontError::InvalidPaymentLane => "storefront_invalid_payment_lane",
            StorefrontError::WishlistItemNotFound => "storefront_wishlist_item_not_found",
            StorefrontError::ComparisonCapExceeded { .. } => "storefront_comparison_cap_exceeded",
            StorefrontError::SettingsNotFound => "storefront_settings_not_found",
            StorefrontError::RecoveryTemplateRequired => "storefront_recovery_template_required",
            StorefrontError::NoContactAddress => "storefront_no_contact_address",
            StorefrontError::RateLimited { .. } => "storefront_rate_limited",
            StorefrontError::InvalidInput(_) => "storefront_invalid_input",
            StorefrontError::NotFound(_) => "storefront_not_found",
            StorefrontError::Guarded(_) => "storefront_guarded_refusal",
        }
    }
}

/// Map a Postgres unique-violation (23505) on this module's constraint
/// names to the typed conflict arm; anything else stays a DB error.
pub fn map_unique_violation(err: sqlx::Error) -> StorefrontError {
    if let sqlx::Error::Database(db) = &err {
        if db.code().as_deref() == Some("23505") {
            let constraint = db
                .constraint()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            return match constraint.as_str() {
                "idx_carts_open_per_visitor" | "idx_carts_open_per_portal_user" => {
                    StorefrontError::OpenCartExists
                }
                "idx_shopper_parties_company_email_live" => {
                    // The resolve-or-create race lost: the survivor row
                    // IS the binding — callers re-select by the key.
                    StorefrontError::Internal(constraint)
                }
                _ => StorefrontError::Internal(constraint),
            };
        }
    }
    StorefrontError::Db(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn lifecycle_and_identity_refusals_map_to_409() {
        assert_eq!(StorefrontError::OpenCartExists.http_status(), 409);
        assert_eq!(StorefrontError::CartNotAdoptable.http_status(), 409);
        assert_eq!(
            StorefrontError::CartNotOpen { state: "placed".into() }.http_status(),
            409
        );
        assert_eq!(StorefrontError::BillingRequired.http_status(), 409);
    }

    #[test]
    fn closed_door_shapes_share_404() {
        assert_eq!(StorefrontError::PublishGateRefused.http_status(), 404);
        assert_eq!(StorefrontError::CheckoutNotFound.http_status(), 404);
        assert_eq!(StorefrontError::CartNotFound.http_status(), 404);
    }

    #[test]
    fn unwired_ports_refuse_loudly_with_503() {
        assert_eq!(
            StorefrontError::TaxPortRefused { code: "unwired".into() }.http_status(),
            503
        );
        assert_eq!(
            StorefrontError::PartyPortRefused { code: "unwired".into() }.http_status(),
            503
        );
        assert_eq!(
            StorefrontError::AvailabilityPortRefused { code: "unwired".into() }.http_status(),
            503
        );
    }

    #[test]
    fn companions_refusals_map_to_their_typed_statuses() {
        use rust_decimal::Decimal;
        assert_eq!(
            StorefrontError::StockInsufficient {
                item_id: Uuid::new_v4(),
                requested: Decimal::TWO,
                available: Decimal::ONE,
            }
            .http_status(),
            422
        );
        assert_eq!(StorefrontError::PickupLocationNotFound.http_status(), 404);
        assert_eq!(StorefrontError::PickupCountryMissing.http_status(), 422);
        assert_eq!(
            StorefrontError::PickupCountryMissing.code(),
            "storefront_pickup_country_missing"
        );
        assert_eq!(StorefrontError::PickupWarehouseRefused.http_status(), 422);
        assert_eq!(
            StorefrontError::PickupWarehouseRefused.code(),
            "storefront_pickup_warehouse_refused"
        );
        assert_eq!(StorefrontError::PickupModeRequired.http_status(), 422);
        assert_eq!(StorefrontError::InvalidPaymentLane.http_status(), 422);
        assert_eq!(StorefrontError::WishlistItemNotFound.http_status(), 404);
        assert_eq!(
            StorefrontError::ComparisonCapExceeded { cap: 4, requested: 5 }.http_status(),
            422
        );
    }
}
