//! The catalog read port (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The storefront takes NO cargo edge to the catalog module: every
//! catalog read goes through this port and the HOST wires the adapter
//! over its own catalog handles (the composing-service posture — the
//! same shape as website's principal verifier port).
//!
//! FAIL-CLOSED: the module ships only the refusing default. Unwired,
//! every catalog read answers "nothing is available" — the listing
//! reads serve an empty page and line mutations refuse with the typed
//! publish-gate 404. No catalog row is ever guessed.

use uuid::Uuid;

/// The port's typed refusal (unwired adapter, transport failure).
#[derive(Debug, Clone)]
pub struct CatalogPortError {
    pub code: String,
    pub message: String,
}

/// The catalog facts the publish gate and the pricing mapping need for
/// one item: the sale-blocking status read, the display name, and the
/// two pricing dimensions promo rules match on.
#[derive(Debug, Clone)]
pub struct ItemSnapshot {
    pub item_id: Uuid,
    /// The item's lifecycle status, verbatim from the catalog
    /// (`active`, `archived`, …). The publish gate accepts `active`
    /// only.
    pub status: String,
    pub name: String,
    pub item_group_id: Option<Uuid>,
    pub brand_id: Option<Uuid>,
    /// The item's primary group name (the derived category tree's
    /// label source).
    pub item_group_name: Option<String>,
}

impl ItemSnapshot {
    /// The publish gate's catalog arm: the item must be `active`.
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}

/// Read-only catalog access, host-wired. Every method is a pure read.
#[async_trait::async_trait]
pub trait CatalogReadPort: Send + Sync {
    /// One item's snapshot; `None` when the catalog carries no such
    /// item (a missing item fails the publish gate the same way an
    /// inactive one does — the closed-door shape).
    async fn item_snapshot(
        &self,
        company_id: Uuid,
        item_id: Uuid,
    ) -> Result<Option<ItemSnapshot>, CatalogPortError>;

    /// Batch arm of the same read (listing pages and cart re-pricing
    /// resolve every visible item in one call). Missing items are
    /// simply absent from the vector.
    async fn item_snapshots(
        &self,
        company_id: Uuid,
        item_ids: &[Uuid],
    ) -> Result<Vec<ItemSnapshot>, CatalogPortError>;
}

/// The refusing default: no catalog read ever succeeds. Fail-closed —
/// an unwired catalog means an empty storefront, never an ungated one.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefusingCatalogReadPort;

impl RefusingCatalogReadPort {
    fn refused() -> CatalogPortError {
        CatalogPortError {
            code: "catalog_port_unwired".into(),
            message: "no catalog read adapter is installed".into(),
        }
    }
}

#[async_trait::async_trait]
impl CatalogReadPort for RefusingCatalogReadPort {
    async fn item_snapshot(
        &self,
        _company_id: Uuid,
        _item_id: Uuid,
    ) -> Result<Option<ItemSnapshot>, CatalogPortError> {
        Err(Self::refused())
    }

    async fn item_snapshots(
        &self,
        _company_id: Uuid,
        _item_ids: &[Uuid],
    ) -> Result<Vec<ItemSnapshot>, CatalogPortError> {
        Err(Self::refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unwired_port_refuses_every_read() {
        let port = RefusingCatalogReadPort;
        let id = Uuid::new_v4();
        assert!(port.item_snapshot(Uuid::new_v4(), id).await.is_err());
        assert!(port.item_snapshots(Uuid::new_v4(), &[id]).await.is_err());
    }
}
