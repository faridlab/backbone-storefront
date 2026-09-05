//! The probe modules, one file per spec §13 gate family (the
//! host/harness-seat gates — schema tooling, live dev wire, pin
//! probes — live outside the module and are recorded in the module's
//! README instead).
//!
//! Gate map (spec §13):
//! - `identity_determinism`  — gate 2 (deterministic create/adopt)
//! - `row_lock`              — gate 3 (the checkout row lock)
//! - `publish_gate`          — gate 4 (the publish gate)
//! - `mutating_gets`         — gates 5 + 6 (no-mint reads, the
//!                             mutating-GET harness)
//! - `pricing_mapping`       — gate 7 (per-website pricing)
//! - `coupon_discipline`     — gate 8 (coupon POST-only)
//! - `express_determinism`   — gate 9 (parallel expresses)
//! - `settle_confirm`        — gate 10 (settle→confirm, exactly-once)
//! - `free_arm`              — gate 11 (free vs paid arms)
//! - `abandoned`             — gate 12 (derived abandonment)
//! - `installs_inert`        — gate 13 (bring-up writes nothing)
//! - `exclusions`            — gate 14 (the module's exclusions)
//!
//! Shopper-companions gates (spec §14.6):
//! - `availability_gate`     — the fail-closed stock gate (unwired 503,
//!                             the typed 422 clamp, backorder skip,
//!                             the place-time whole-basket check)
//! - `collect`               — Click & Collect (registry upsert with
//!                             its warehouse/website fences, the pure
//!                             lookup, the server-side pin, reset)
//! - `fiscal_pin`            — the pickup fiscal pin (a pickup place
//!                             resolves tax under the store's country,
//!                             a delivery place under the home arm; a
//!                             countryless pinned store refuses the
//!                             place with the typed fiscal guard)
//! - `pay_on_site`           — the third checkout lane (pending_pickup,
//!                             never auto-confirmed, officer confirm)
//! - `wishlist`              — the visitor-backed wishlist + merge
//! - `stock_wait`            — the back-in-stock arm + officer send

pub mod abandoned;
pub mod availability_gate;
pub mod collect;
pub mod common;
pub mod coupon_discipline;
pub mod exclusions;
pub mod express_determinism;
pub mod fiscal_pin;
pub mod free_arm;
pub mod identity_determinism;
pub mod installs_inert;
pub mod mutating_gets;
pub mod pay_on_site;
pub mod pricing_mapping;
pub mod publish_gate;
pub mod row_lock;
pub mod settle_confirm;
pub mod stock_wait;
pub mod wishlist;
