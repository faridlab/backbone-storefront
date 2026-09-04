//! The storefront probe suite (spec §13, the in-module gates).
//!
//! Fail-hard by contract: a probe that cannot reach its scratch
//! database PANICS — a green run means the behaviors were exercised,
//! never that they were unreachable. See `probes/common` for the
//! harness and `probes/mod.rs` for the gate map.

mod probes;
