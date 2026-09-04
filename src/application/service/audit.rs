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
    sqlx::query(
        r#"
        INSERT INTO storefront.storefront_audit_log
            (id, website_id, event, actor, subject_type, subject_id, detail, occurred_at)
        VALUES (gen_random_uuid(), $1, $2::storefront_audit_event, $3, $4, $5, $6, now())
        "#,
    )
    .bind(website_id)
    .bind(event)
    .bind(actor.stamp())
    .bind(subject_type)
    .bind(subject_id)
    .bind(detail)
    .execute(exec)
    .await?;
    Ok(())
}
