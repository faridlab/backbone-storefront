//! The party write port (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! No cargo edge to the party module: party MINTS and segment reads go
//! through this port and the HOST wires the adapter over its own party
//! handles. Two consumers:
//!
//! - **Billing capture / express checkout** — the deterministic
//!   email→party map resolves-or-CREATES through
//!   [`PartyWritePort::mint_customer_party] on first sight of a
//!   shopper email. Parties minted here are first-class customers,
//!   never "placeholders".
//! - **Settings bootstrap** — the guest party (the designated public
//!   customer an anonymous placed order rides) is minted once per
//!   website through [`PartyWritePort::mint_guest_party`], mirroring
//!   website's public-principal bootstrap.
//!
//! FAIL-CLOSED: the refusing default makes billing capture and the
//! settings bootstrap refuse with the typed 503 — never a synthetic
//! party id, never a silent skip.

use uuid::Uuid;

/// The port's typed refusal (unwired adapter, transport failure, or the
/// party domain refused the mint).
#[derive(Debug, Clone)]
pub struct PartyPortError {
    pub code: String,
    pub message: String,
}

/// Party mint + segment reads, host-wired.
#[async_trait::async_trait]
pub trait PartyWritePort: Send + Sync {
    /// Mint (or resolve) a first-class CUSTOMER party for one shopper
    /// email on a company. Idempotent per (company, email) is the
    /// adapter's contract — the deterministic map row in
    /// `storefront.shopper_parties` is the storefront's own record of
    /// the resolution, and the race-free INSERT..ON CONFLICT there
    /// means this is only ever called for a map miss.
    async fn mint_customer_party(
        &self,
        company_id: Uuid,
        email_normalized: &str,
        name: Option<&str>,
    ) -> Result<Uuid, PartyPortError>;

    /// Mint (or resolve) the designated GUEST party for one website's
    /// company — the public customer anonymous placed orders ride (the
    /// settings bootstrap's one-time mint; the settings row keeps the
    /// resolved id).
    async fn mint_guest_party(&self, company_id: Uuid) -> Result<Uuid, PartyPortError>;

    /// The billing party's explicit customer segment, when it carries
    /// one — the pricing mapping's first arm (party segment ELSE the
    /// website's default segment).
    async fn party_segment(
        &self,
        company_id: Uuid,
        party_id: Uuid,
    ) -> Result<Option<Uuid>, PartyPortError>;
}

/// The refusing default: every mint and segment read refuses. Installed
/// until the host wires an adapter — billing capture and the settings
/// bootstrap read the typed 503, never a synthetic party.
#[derive(Debug, Default, Clone, Copy)]
pub struct RefusingPartyWritePort;

impl RefusingPartyWritePort {
    fn refused() -> PartyPortError {
        PartyPortError {
            code: "party_port_unwired".into(),
            message: "no party write adapter is installed".into(),
        }
    }
}

#[async_trait::async_trait]
impl PartyWritePort for RefusingPartyWritePort {
    async fn mint_customer_party(
        &self,
        _company_id: Uuid,
        _email_normalized: &str,
        _name: Option<&str>,
    ) -> Result<Uuid, PartyPortError> {
        Err(Self::refused())
    }

    async fn mint_guest_party(&self, _company_id: Uuid) -> Result<Uuid, PartyPortError> {
        Err(Self::refused())
    }

    async fn party_segment(
        &self,
        _company_id: Uuid,
        _party_id: Uuid,
    ) -> Result<Option<Uuid>, PartyPortError> {
        Err(Self::refused())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unwired_port_refuses_every_arm() {
        let port = RefusingPartyWritePort;
        assert!(port.mint_customer_party(Uuid::new_v4(), "a@b.c", None).await.is_err());
        assert!(port.mint_guest_party(Uuid::new_v4()).await.is_err());
        assert!(port.party_segment(Uuid::new_v4(), Uuid::new_v4()).await.is_err());
    }
}
