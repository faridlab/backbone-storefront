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

pub mod abandoned;
pub mod common;
pub mod coupon_discipline;
pub mod exclusions;
pub mod express_determinism;
pub mod free_arm;
pub mod identity_determinism;
pub mod installs_inert;
pub mod mutating_gets;
pub mod pricing_mapping;
pub mod publish_gate;
pub mod row_lock;
pub mod settle_confirm;
