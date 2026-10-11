// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod audit;
pub mod availability_port;
pub mod availability_service;
pub mod catalog_read_port;
pub mod catalog_service;
pub mod checkout_service;
pub mod collect_service;
pub mod notifier_port;
pub mod party_write_port;
pub mod pricing_service;
pub mod recovery_service;
pub mod storefront_error;
pub mod tax_resolve_port;
pub mod wishlist_service;
pub use audit::{record_audit, ActorRef};
pub use availability_port::{
    AvailabilityPortError, AvailabilityReadPort, ItemAvailability, RefusingAvailabilityReadPort,
};
pub use availability_service::map_availability_error;
pub use catalog_read_port::{
    CatalogPortError, CatalogReadPort, ItemSnapshot, RefusingCatalogReadPort,
};
pub use catalog_service::{
    admin_listings, public_categories, public_detail, public_listings, publish_listing,
    set_listing_backorder, set_price, set_settings, unpublish_listing, upsert_listing,
    AdminListingRow, DerivedCategory, PublicListing, SettingsPatch, SortKind,
};
pub use checkout_service::{
    cancel_checkout, capture_billing, checkout_by_id, confirm_pickup, consume_settlement,
    order_state_of, place, place_on_site, reset_fulfillment, set_delivery, set_pickup,
    CheckoutDeps, CheckoutRow, PaymentLane,
};
pub use collect_service::{
    active_location_on_website, active_locations_for_website, locations_for_website,
    upsert_location, LocationPatch, PickupLocationRow,
};
pub use notifier_port::{
    RecoveryDelivery, RecoveryMessage, RecoveryNotifier, StockAlertDelivery, StockAlertMessage,
    StockAlertNotifier, UnwiredRecoveryNotifier, UnwiredStockAlertNotifier,
};
pub use party_write_port::{PartyPortError, PartyWritePort, RefusingPartyWritePort};
pub use pricing_service::{
    invalidate_pricing_cache_for_company, members_only, price_cart, settings_for, DisplayLine,
    PricedCartView, RewardLineView, SaleSettingsRow,
};
pub use recovery_service::{
    abandoned_after_hours, abandoned_carts_for_company, abandoned_carts_for_identity,
    cart_is_abandoned, send_recovery, AbandonedCartRow, ABANDONED_AFTER_HOURS_ENV,
    DEFAULT_ABANDONED_AFTER_HOURS,
};
pub use storefront_error::StorefrontError;
pub use tax_resolve_port::{RefusingTaxResolvePort, TaxPortError, TaxResolvePort};
pub use wishlist_service::{
    add as wishlist_add, arm_notify as wishlist_arm_notify, reconcile as wishlist_reconcile,
    remove as wishlist_remove, send_stock_alerts, stock_wait_read, wishlist_for, StockAlertSummary,
    StockWaitItem, WishlistRow,
};
