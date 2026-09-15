//! The audit-stamp helper (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The website module's `record_audit` shape, verbatim: append-only,
//! service-emitted, typed enum event, optional actor/subject/detail.
//! Every mutating verb in this module stamps exactly one row — the
//! officer audit read and the publish-refusal probe both read this
//! table.

use uuid::Uuid;

use super::storefront_error::StorefrontError;

/// The acting principal an audit row attributes: an officer id, a
/// visitor id, or the system actor.
#[derive(Debug, Clone, Copy)]
pub struct ActorRef(pub Option<Uuid>);

impl ActorRef {
    /// An officer principal (the admin tree's request extension).
    pub fn officer(id: Uuid) -> Self {
        ActorRef(Some(id))
    }

    /// A shopper session (the visitor lineage a public verb ran under).
    pub fn visitor(id: Uuid) -> Self {
        ActorRef(Some(id))
    }

    /// The system actor (host-side consumers, bootstraps).
    pub fn system() -> Self {
        ActorRef(None)
    }

    fn stamp(self) -> Option<Uuid> {
        self.0
    }
}

/// Stamp one append-only audit row. `event` must be a member of the
/// `storefront_audit_event` enum vocabulary (the DB casts it).
pub async fn record_audit(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Option<Uuid>,
    event: &str,
    actor: ActorRef,
    subject_type: Option<&str>,
    subject_id: Option<Uuid>,
    detail: Option<serde_json::Value>,
) -> Result<(), StorefrontError> {
    // Consolidated onto `auditlog.audit_trails`. `website_id` has no column
    // there, so it folds into the diff payload rather than being dropped — it
    // is the one piece of context this module's own table carried that the
    // shared shape does not.
    let changed = match (website_id, detail) {
        (Some(w), Some(serde_json::Value::Object(mut o))) => {
            o.insert("website_id".into(), serde_json::Value::String(w.to_string()));
            Some(serde_json::Value::Object(o))
        }
        (Some(w), None) => Some(serde_json::json!({ "website_id": w.to_string() })),
        (None, d) => d,
        (Some(w), Some(d)) => Some(serde_json::json!({ "website_id": w.to_string(), "detail": d })),
    };
    backbone_auditlog::application::service::append(
        exec,
        backbone_auditlog::application::service::AuditEvent {
            event_type: backbone_auditlog::domain::entity::AuditEventType::DataChange,
            action: event.to_string(),
            subject_type: subject_type.map(|s| {
                if s.contains('.') { s.to_string() } else { format!("storefront.{s}") }
            }),
            subject_id: subject_id.map(|id| id.to_string()),
            changed,
            reason: None,
            status: backbone_auditlog::domain::entity::AuditStatus::Success,
            actor: actor.stamp().map(|id| id.to_string()),
        },
    )
    .await?;
    Ok(())
}
