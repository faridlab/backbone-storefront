//! The recovery notifier port (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The explicit recovery send's delivery arm. The module owns the
//! trait + the UNWIRED default; the host installs the adapter over its
//! own mailer.
//!
//! UNWIRED IS VISIBLE, NEVER SILENT: an uncomposed port does not refuse
//! the send and does not drop it — the recovery verb still records its
//! audit row and answers `delivery_state = "unwired"`, a loud typed
//! state the officer sees (§8.3). Only a wired adapter that ACCEPTED
//! the delivery stamps `sent`.

/// One recovery delivery attempt's outcome, as the adapter reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDelivery {
    /// The notifier accepted the message for delivery.
    Sent,
    /// No notifier adapter is composed — the attempt is recorded, the
    /// delivery state stays visibly `unwired`.
    Unwired,
}

impl RecoveryDelivery {
    /// The `delivery_state` label the recovery audit row carries.
    pub fn state_label(&self) -> &'static str {
        match self {
            RecoveryDelivery::Sent => "sent",
            RecoveryDelivery::Unwired => "unwired",
        }
    }
}

/// One recovery message, fully resolved by the caller (template ref,
/// contact address, cart reference) — the adapter adds no policy.
#[derive(Debug, Clone)]
pub struct RecoveryMessage<'a> {
    pub template_ref: &'a str,
    pub to_address: &'a str,
    pub cart_id: uuid::Uuid,
    /// The recovering website's name (the message's sender context).
    pub website_name: &'a str,
}

/// Recovery delivery, host-wired.
#[async_trait::async_trait]
pub trait RecoveryNotifier: Send + Sync {
    /// Deliver one recovery message. A transport refusal is an `Err`
    /// (the verb then records `pending` — the loud retry state); the
    /// unwired default is NOT an error, it is the visible `Unwired`
    /// outcome.
    async fn send_recovery(&self, message: &RecoveryMessage<'_>) -> Result<RecoveryDelivery, String>;
}

/// The unwired default: every send reports the visible `Unwired`
/// outcome — recorded, loud, never a silent drop.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnwiredRecoveryNotifier;

#[async_trait::async_trait]
impl RecoveryNotifier for UnwiredRecoveryNotifier {
    async fn send_recovery(&self, _message: &RecoveryMessage<'_>) -> Result<RecoveryDelivery, String> {
        tracing::warn!(
            target: "storefront.recovery",
            cart_id = %_message.cart_id,
            "recovery notifier port is unwired — the send is recorded with the visible 'unwired' state"
        );
        Ok(RecoveryDelivery::Unwired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn unwired_notifier_reports_the_visible_state() {
        let notifier = UnwiredRecoveryNotifier;
        let msg = RecoveryMessage {
            template_ref: "tpl-a",
            to_address: "shopper@example.test",
            cart_id: Uuid::new_v4(),
            website_name: "Site",
        };
        let outcome = notifier.send_recovery(&msg).await.unwrap_or(RecoveryDelivery::Unwired);
        assert_eq!(outcome, RecoveryDelivery::Unwired);
        assert_eq!(outcome.state_label(), "unwired");
    }
}
