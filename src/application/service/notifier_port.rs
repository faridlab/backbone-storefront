//! The outbound notifier ports (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! Two ports live here, both following the same posture: the module
//! owns the trait + the UNWIRED default; the host installs the adapter
//! over its own mailer.
//!
//! 1. **RecoveryNotifier** — the explicit recovery send's delivery arm
//!    (§8.3).
//! 2. **StockAlertNotifier** — the officer's back-in-stock send (§14.3).
//!
//! UNWIRED IS VISIBLE, NEVER SILENT: an uncomposed port does not refuse
//! the send and does not drop it — the verb still records its audit row
//! and answers `delivery_state = "unwired"`, a loud typed state the
//! officer sees. Only a wired adapter that ACCEPTED the delivery stamps
//! `sent`.

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

// ── the back-in-stock notifier (§14.3) ─────────────────────────────────────
//
// The SAME host-wired posture as recovery: the module owns the trait +
// the unwired default, the host installs the adapter over its own
// mailer. The officer's explicit stock-alert send rides this port; no
// cron, no webhook, and no automatic trigger exists anywhere in the
// module — a waitlisted shopper hears about restock only when an
// officer decided to say so.

/// One stock-alert delivery attempt's outcome, as the adapter reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockAlertDelivery {
    /// The notifier accepted the message for delivery.
    Sent,
    /// No notifier adapter is composed — the attempt is recorded, the
    /// delivery state stays visibly `unwired`.
    Unwired,
}

impl StockAlertDelivery {
    /// The `delivery_state` label the stock-alert audit row carries.
    pub fn state_label(&self) -> &'static str {
        match self {
            StockAlertDelivery::Sent => "sent",
            StockAlertDelivery::Unwired => "unwired",
        }
    }

    /// Whether the outcome counts as an ACCEPTED send for the arm's
    /// bookkeeping (both `Sent` and the visible `Unwired` clear the
    /// wait flags; only a transport `Err` leaves them armed — a failed
    /// send must never burn the shopper's one notification).
    pub fn is_accepted(&self) -> bool {
        matches!(self, StockAlertDelivery::Sent | StockAlertDelivery::Unwired)
    }
}

/// One back-in-stock message, fully resolved by the caller — the
/// adapter adds no policy (and never a stock number: the message says
/// "back in stock", not "N units left").
#[derive(Debug, Clone)]
pub struct StockAlertMessage<'a> {
    pub website_id: uuid::Uuid,
    pub item_id: uuid::Uuid,
    /// The listing's display name at send time (resolved from the
    /// catalog port by the verb, never trusted from the request).
    pub item_name: &'a str,
    pub to_address: &'a str,
    /// The website's name (the message's sender context).
    pub website_name: &'a str,
}

/// Back-in-stock delivery, host-wired.
#[async_trait::async_trait]
pub trait StockAlertNotifier: Send + Sync {
    /// Deliver one stock alert. A transport refusal is an `Err` (the
    /// verb records the failed attempt and leaves the wait flags
    /// armed); the unwired default is NOT an error — it is the visible
    /// `Unwired` outcome, accepted for bookkeeping.
    async fn send_stock_alert(&self, message: &StockAlertMessage<'_>) -> Result<StockAlertDelivery, String>;
}

/// The unwired default: every send reports the visible `Unwired`
/// outcome — recorded, loud, never a silent drop.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnwiredStockAlertNotifier;

#[async_trait::async_trait]
impl StockAlertNotifier for UnwiredStockAlertNotifier {
    async fn send_stock_alert(&self, _message: &StockAlertMessage<'_>) -> Result<StockAlertDelivery, String> {
        tracing::warn!(
            target: "storefront.stock_alert",
            item_id = %_message.item_id,
            "stock alert notifier port is unwired — the send is recorded with the visible 'unwired' state"
        );
        Ok(StockAlertDelivery::Unwired)
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

    #[tokio::test]
    async fn unwired_stock_notifier_reports_the_visible_state() {
        let notifier = UnwiredStockAlertNotifier;
        let msg = StockAlertMessage {
            website_id: Uuid::new_v4(),
            item_id: Uuid::new_v4(),
            item_name: "Widget",
            to_address: "shopper@example.test",
            website_name: "Site",
        };
        let outcome = notifier.send_stock_alert(&msg).await.unwrap_or(StockAlertDelivery::Unwired);
        assert_eq!(outcome, StockAlertDelivery::Unwired);
        assert_eq!(outcome.state_label(), "unwired");
        // The visible-unwired outcome ACCEPTS the send for bookkeeping
        // purposes (the arm was recorded loudly) — only a transport Err
        // leaves the wait flags untouched.
        assert!(outcome.is_accepted());
    }
}
