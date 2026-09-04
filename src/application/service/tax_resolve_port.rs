//! The tax resolve port (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! Order-level fiscal resolution (§5.3 of the module spec): the
//! storefront resolves ONE tax rate per cart — company tax settings +
//! the delivery jurisdiction from the captured billing address — and
//! applies it as the priced order's single `tax_rate`. No
//! fiscal-position entity exists anywhere in the tree and none is
//! invented here; when product demand needs one it lands in the tax
//! domain with its own spec.
//!
//! No cargo edge to the tax module: the HOST wires this adapter over
//! its own tax handles. FAIL-CLOSED: the refusing default makes every
//! place refuse with the typed 503 — a checkout never books under an
//! unknown fiscal rate (a zero-rate fallback would be a silent fiscal
//! lie, not a default).

use rust_decimal::Decimal;
use uuid::Uuid;

/// The port's typed refusal (unwired adapter, transport failure, or the
/// tax domain refused).
#[derive(Debug, Clone)]
pub struct TaxPortError {
    pub code: String,
    pub message: String,
}

/// Order-level tax resolution, host-wired.
#[async_trait::async_trait]
pub trait TaxResolvePort: Send + Sync {
    /// Resolve the ONE tax rate a placed cart's order carries. The
    /// jurisdiction hint is the delivery address's country/region when
    /// billing captured one (`None` for a billing without an address
    /// — the adapter falls back to the company's home rate, the same
    /// arm Odoo's default fiscal position takes).
    async fn resolve_rate(
        &self,
        company_id: Uuid,
        delivery_jurisdiction: Option<&str>,
    ) -> Result<Decimal, TaxPortError>;
}

/// The refusing default: every resolution refuses. Installed until the
/// host wires an adapter — place and express read the typed 503, never
/// a zero rate.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefusingTaxResolvePort;

impl RefusingTaxResolvePort {
    fn refused() -> TaxPortError {
        TaxPortError {
            code: "tax_port_unwired".into(),
            message: "no tax resolve adapter is installed".into(),
        }
    }
}

#[async_trait::async_trait]
impl TaxResolvePort for RefusingTaxResolvePort {
    async fn resolve_rate(
        &self,
        _company_id: Uuid,
        _delivery_jurisdiction: Option<&str>,
    ) -> Result<Decimal, TaxPortError> {
        Err(Self::refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unwired_port_refuses_every_resolution() {
        let port = RefusingTaxResolvePort;
        assert!(port.resolve_rate(Uuid::new_v4(), None).await.is_err());
        assert!(port.resolve_rate(Uuid::new_v4(), Some("ID")).await.is_err());
    }
}
