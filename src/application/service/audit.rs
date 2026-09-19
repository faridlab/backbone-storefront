//! The audit-stamp helper (hand-written; user-owned; see
//! `metaphor.codegen.yaml`).
//!
//! The website module's `record_audit` shape, verbatim: append-only,
//! service-emitted, typed enum event, optional actor/subject/detail.
//! Every mutating verb in this module stamps exactly one row — the
//! officer audit read and the publish-refusal probe both read this
//! table.

use backbone_orm::org_scope;
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

/// Stamp one audit row from a verb that holds a pool rather than a transaction.
///
/// The trail is org-fenced, and the fence reads session variables that only
/// exist on a connection somebody bound them on. A bare `pool` acquire is a
/// different connection from the one the request's scope was opened on, so the
/// row it writes carries no unit and the fence refuses it, which rolls back the
/// business write that triggered the audit. The officer sees an internal error
/// and nothing saved.
///
/// So this opens a short transaction, relays the caller's ambient scope onto
/// it, and writes there. A transaction is required rather than incidental: the
/// scope binder sets its variables LOCAL, and outside a transaction they are
/// gone before the next statement runs.
///
/// Outside any request scope (host consumers, bootstraps) nothing is bound and
/// the write behaves exactly as it did before.
///
/// Prefer [`record_audit`] with the mutation's own transaction wherever the verb
/// has one: only then does the audit row die with the write it describes.
#[allow(clippy::too_many_arguments)]
pub async fn record_audit_on_pool(
    pool: &sqlx::PgPool,
    website_id: Option<Uuid>,
    event: &str,
    actor: ActorRef,
    subject_type: Option<&str>,
    subject_id: Option<Uuid>,
    detail: Option<serde_json::Value>,
) -> Result<(), StorefrontError> {
    let mut tx = pool.begin().await?;
    if let Some(scope) = org_scope::current_org_scope() {
        org_scope::bind_org_scope_on(&mut tx, &scope).await?;
    }
    record_audit(&mut *tx, website_id, event, actor, subject_type, subject_id, detail).await?;
    tx.commit().await?;
    Ok(())
}

/// Open a transaction that carries the caller's ambient org scope.
///
/// Every verb in this module that audits inside its own transaction needs this:
/// the shared audit trail is org-fenced, and the fence reads session variables
/// that live on ONE connection. A transaction opened straight off the pool is a
/// different connection from the one the request's scope was opened on, so the
/// audit row it writes carries no unit, the fence refuses it, and the refusal
/// rolls back the business write that triggered the audit.
///
/// Outside any request scope (host consumers, bootstraps) nothing is bound and
/// the transaction behaves exactly as `pool.begin()` did.
pub async fn begin_scoped(
    pool: &sqlx::PgPool,
) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, StorefrontError> {
    let mut tx = pool.begin().await?;
    if let Some(scope) = org_scope::current_org_scope() {
        org_scope::bind_org_scope_on(&mut tx, &scope).await?;
    }
    Ok(tx)
}
