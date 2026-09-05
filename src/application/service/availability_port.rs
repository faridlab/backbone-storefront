//! The availability read port (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! Free-quantity reads for display and checkout (§14 of the module spec):
//! the storefront NEVER computes stock itself and NEVER persists a stock
//! snapshot — every number on every surface is computed fresh through
//! this port at read/mutation time (no persisted shop-warning rows
//! anywhere in this module).
//!
//! TWO SCOPES, ONE PORT:
//!  - DISPLAY: `GET /public/availability/{item}` and the comparison read
//!    answer the website's display scope — the website sale-settings
//!    `display_warehouse_id` (NULL = the company's aggregate across
//!    warehouses, a documented officer-visible semantic).
//!  - CHECKOUT: the line-mutation clamp and the place-time gate answer
//!    the cart's fulfillment scope — the pinned pickup location's
//!    warehouse for a pickup cart, the company aggregate for a delivery
//!    cart.
//!
//! THE MRP BRIDGE: for kit/BOM-backed items the adapter answers the
//! explode-through arithmetic (the minimum over component free quantity
//! in the SAME warehouse scope) and flags `kit_exploded`. The storefront
//! never performs the arithmetic — the host composes one adapter over
//! the inventory module's availability reads plus the manufacturing
//! module's BOM explode, so kits and plain items always read through ONE
//! consistent, publish-gated surface (no anonymous stock oracle for
//! arbitrary product ids and no second sudo'd inconsistent path).
//!
//! FAIL-CLOSED: the refusing default makes every clamped mutation and
//! every place refuse with the typed 503 — an unwired availability
//! adapter means the store cannot promise stock, and it says so loudly
//! (never a silent zero-stock or infinite-stock fallback).

use rust_decimal::Decimal;
use uuid::Uuid;

/// The port's typed refusal (unwired adapter, transport failure, or the
/// inventory domain refused).
#[derive(Debug, Clone)]
pub struct AvailabilityPortError {
    pub code: String,
    pub message: String,
}

/// One item's free-to-promise quantity in the requested scope.
#[derive(Debug, Clone)]
pub struct ItemAvailability {
    pub item_id: Uuid,
    /// Free quantity (on-hand minus reserved) in the requested warehouse
    /// scope, computed fresh by the adapter.
    pub free_quantity: Decimal,
    /// True when the adapter answered through kit explode-through
    /// arithmetic (the number is the minimum over the kit's components).
    pub kit_exploded: bool,
}

impl ItemAvailability {
    /// A simple in-scope quantity (non-kit shape).
    pub fn plain(item_id: Uuid, free_quantity: Decimal) -> Self {
        Self { item_id, free_quantity, kit_exploded: false }
    }
}

/// Availability reads, host-wired over the inventory module's
/// availability service (+ the manufacturing module's BOM explode for
/// kit items).
#[async_trait::async_trait]
pub trait AvailabilityReadPort: Send + Sync {
    /// One item's free quantity. `warehouse_id = None` reads the
    /// company's aggregate across warehouses; a value scopes the read to
    /// that warehouse.
    async fn free_quantity(
        &self,
        company_id: Uuid,
        item_id: Uuid,
        warehouse_id: Option<Uuid>,
    ) -> Result<ItemAvailability, AvailabilityPortError>;

    /// The batch form (display reads: the availability route and the
    /// comparison read) — one adapter round for the whole item set.
    async fn free_quantities(
        &self,
        company_id: Uuid,
        item_ids: &[Uuid],
        warehouse_id: Option<Uuid>,
    ) -> Result<Vec<ItemAvailability>, AvailabilityPortError>;
}

/// The refusing default: every read refuses. Installed until the host
/// wires an adapter — clamped line mutations and places read the typed
/// 503, never a fallback number.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefusingAvailabilityReadPort;

impl RefusingAvailabilityReadPort {
    fn refused() -> AvailabilityPortError {
        AvailabilityPortError {
            code: "availability_port_unwired".into(),
            message: "no availability read adapter is installed".into(),
        }
    }
}

#[async_trait::async_trait]
impl AvailabilityReadPort for RefusingAvailabilityReadPort {
    async fn free_quantity(
        &self,
        _company_id: Uuid,
        _item_id: Uuid,
        _warehouse_id: Option<Uuid>,
    ) -> Result<ItemAvailability, AvailabilityPortError> {
        Err(Self::refused())
    }

    async fn free_quantities(
        &self,
        _company_id: Uuid,
        _item_ids: &[Uuid],
        _warehouse_id: Option<Uuid>,
    ) -> Result<Vec<ItemAvailability>, AvailabilityPortError> {
        Err(Self::refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unwired_port_refuses_every_read() {
        let port = RefusingAvailabilityReadPort;
        assert!(port.free_quantity(Uuid::new_v4(), Uuid::new_v4(), None).await.is_err());
        assert!(port
            .free_quantity(Uuid::new_v4(), Uuid::new_v4(), Some(Uuid::new_v4()))
            .await
            .is_err());
        assert!(port
            .free_quantities(Uuid::new_v4(), &[], None)
            .await
            .is_err());
    }
}
