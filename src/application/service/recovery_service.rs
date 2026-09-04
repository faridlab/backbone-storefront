//! Abandoned-cart recovery (hand-written; user-owned; see
//! `metaphor.codegen.yaml`): the DERIVED lifecycle read (§8.1), the ONE
//! delay constant (§8.2), and the explicit recovery verbs (§8.3).
//!
//! NOTHING IS STORED about abandonment: a cart is abandoned iff
//! `state='open' AND updated_at < now() - interval '<hours> hours'`,
//! computed FRESH at read with ONE tz-aware `now()` per query. No
//! stored flag, no cron flips anything (zero cron rows exist for this
//! module), and `recovery_invites.notified_at` is an audit stamp —
//! NEVER an eligibility input (a shopper who gains an email later
//! still qualifies).
//!
//! ONE DELAY CONSTANT: `STOREFRONT_ABANDONED_AFTER_HOURS` (default 1)
//! is the single knob; the divergence class dies by construction (one
//! constant, one read site).
//!
//! ORDERING: every derived read is ordered `updated_at DESC, id DESC`
//! (deterministic; no first-match-wins over an unordered scan).

use uuid::Uuid;

use super::audit::{record_audit, ActorRef};
use super::notifier_port::{RecoveryMessage, RecoveryNotifier};
use super::pricing_service::settings_for;
use super::storefront_error::StorefrontError;

/// The abandonment window's env knob (default 1 hour) — the ONE delay
/// constant (§8.2), declared in BOTH env templates.
pub const ABANDONED_AFTER_HOURS_ENV: &str = "STOREFRONT_ABANDONED_AFTER_HOURS";

/// The default abandonment window when the knob is unset or
/// unparseable (an unparseable value falls back to the default, never
/// wedges the reads).
pub const DEFAULT_ABANDONED_AFTER_HOURS: i64 = 1;

/// The abandonment window in effect (hours).
pub fn abandoned_after_hours() -> i64 {
    std::env::var(ABANDONED_AFTER_HOURS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_ABANDONED_AFTER_HOURS)
}

/// One derived abandoned-cart row (the read's projection).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AbandonedCartRow {
    pub id: Uuid,
    pub website_id: Uuid,
    pub visitor_id: Uuid,
    pub portal_user_id: Option<Uuid>,
    pub party_id: Option<Uuid>,
    pub state: String,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    pub line_count: i64,
}

/// The abandonment predicate's SQL — ONE tz-aware `now()` per query,
/// the window from the one constant, the order deterministic.
const ABANDONED_ORDER: &str =
    " ORDER BY (c.metadata->>'updated_at') DESC, c.id DESC ";

async fn abandoned_carts_where(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    where_sql: &str,
    binds: Vec<uuid::Uuid>,
    hours: i64,
) -> Result<Vec<AbandonedCartRow>, StorefrontError> {
    // `where_sql` is a compile-time constant in this file (never
    // client input); the binds are typed uuids.
    let sql = format!(
        r#"
        SELECT c.id, c.website_id, c.visitor_id, c.portal_user_id, c.party_id,
               c.state::text AS state,
               (c.metadata->>'updated_at')::timestamptz AS updated_at,
               (
                   SELECT count(*)
                   FROM storefront.cart_lines cl
                   WHERE cl.cart_id = c.id AND (cl.metadata->>'deleted_at') IS NULL
               ) AS line_count
        FROM storefront.carts c
        WHERE c.state = 'open'
          AND (c.metadata->>'deleted_at') IS NULL
          AND (c.metadata->>'updated_at')::timestamptz
                < now() - make_interval(hours => ${hours_idx}::int)
          AND ({where_sql})
        {ABANDONED_ORDER}
        "#,
        hours_idx = binds.len() + 1,
    );
    let mut q = sqlx::query_as::<_, AbandonedCartRow>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    q.bind(hours).fetch_all(exec).await.map_err(StorefrontError::from)
}

/// The company's derived abandoned carts (the officer read, computed
/// fresh; company scope via the website pairing).
pub async fn abandoned_carts_for_company(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    company_id: Uuid,
    hours: i64,
) -> Result<Vec<AbandonedCartRow>, StorefrontError> {
    abandoned_carts_where(
        exec,
        "EXISTS (SELECT 1 FROM website.websites w \
          WHERE w.id = c.website_id AND w.company_id = $1 \
            AND (w.metadata->>'deleted_at') IS NULL)",
        vec![company_id],
        hours,
    )
    .await
}

/// The identity's OWN derived abandoned carts (the shopper read — the
/// ownership fence is in the predicate: the visitor lineage OR the
/// principal linkage; another identity's carts are structurally
/// absent).
pub async fn abandoned_carts_for_identity(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    visitor_id: Uuid,
    portal_user_id: Option<Uuid>,
    hours: i64,
) -> Result<Vec<AbandonedCartRow>, StorefrontError> {
    match portal_user_id {
        Some(pid) => abandoned_carts_where(
            exec,
            "(c.visitor_id = $1 OR c.portal_user_id = $2)",
            vec![visitor_id, pid],
            hours,
        )
        .await,
        None => {
            abandoned_carts_where(exec, "c.visitor_id = $1", vec![visitor_id], hours).await
        }
    }
}

/// Fresh eligibility for ONE cart (the send verb's own gate — never
/// derived from a stored flag): open, live, past the window.
pub async fn cart_is_abandoned(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    cart_id: Uuid,
    hours: i64,
) -> Result<bool, StorefrontError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT 1::int8
        FROM storefront.carts c
        WHERE c.id = $1 AND c.state = 'open'
          AND (c.metadata->>'deleted_at') IS NULL
          AND (c.metadata->>'updated_at')::timestamptz
                < now() - make_interval(hours => $2::int)
        LIMIT 1
        "#,
    )
    .bind(cart_id)
    .bind(hours)
    .fetch_optional(exec)
    .await?;
    Ok(row.is_some())
}

/// The explicit officer recovery send (§8.3): ONE cart, the
/// per-website template honored (NO hardcoded fallback — a website
/// without a configured template refuses with the typed 422),
/// eligibility computed FRESH per call, delivery through the notifier
/// port (UNWIRED is the visible typed state, never a silent drop).
pub async fn send_recovery(
    pool: &sqlx::PgPool,
    notifier: &dyn RecoveryNotifier,
    cart_id: Uuid,
    officer: ActorRef,
) -> Result<String, StorefrontError> {
    let hours = abandoned_after_hours();
    if !cart_is_abandoned(pool, cart_id, hours).await? {
        return Err(StorefrontError::Guarded(
            "cart does not satisfy the abandonment window".into(),
        ));
    }
    let cart: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        r#"
        SELECT c.website_id, c.party_id
        FROM storefront.carts c
        WHERE c.id = $1 AND (c.metadata->>'deleted_at') IS NULL
        LIMIT 1
        "#,
    )
    .bind(cart_id)
    .fetch_optional(pool)
    .await?;
    let Some((website_id, party_id)) = cart else {
        return Err(StorefrontError::CartNotFound);
    };
    // (1) The template: the website's own row, no fallback exists.
    let settings = settings_for(pool, website_id)
        .await?
        .ok_or(StorefrontError::SettingsNotFound)?;
    let template_ref = settings
        .recovery_template_ref
        .clone()
        .filter(|t| !t.trim().is_empty())
        .ok_or(StorefrontError::RecoveryTemplateRequired)?;
    // (2) The contact address: the shopper-parties map's reverse
    // lookup on the billing party (a cart without billing captured has
    // no address to send to — the typed refusal).
    let Some(party_id) = party_id else {
        return Err(StorefrontError::NoContactAddress);
    };
    let address: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT sp.email_normalized
        FROM storefront.shopper_parties sp
        WHERE sp.party_id = $1 AND (sp.metadata->>'deleted_at') IS NULL
        ORDER BY (sp.metadata->>'created_at') DESC NULLS LAST, sp.id ASC
        LIMIT 1
        "#,
    )
    .bind(party_id)
    .fetch_optional(pool)
    .await?;
    let Some((to_address,)) = address else {
        return Err(StorefrontError::NoContactAddress);
    };
    // (3) The website's display name (the message's sender context).
    let (website_name,): (String,) = sqlx::query_as(
        r#"
        SELECT name FROM website.websites
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(website_id)
    .fetch_one(pool)
    .await
    .map_err(|e| match e {
        sqlx::Error::RowNotFound => StorefrontError::WebsiteNotFound,
        other => StorefrontError::Db(other),
    })?;
    // (4) Deliver through the port; the outcome is visible either way:
    // `sent` (the adapter accepted), `unwired` (no adapter composed —
    // the loud typed state), or `pending` (a transport refusal — the
    // retry state, never a silent drop, never a fake `sent`).
    let message = RecoveryMessage {
        template_ref: &template_ref,
        to_address: &to_address,
        cart_id,
        website_name: &website_name,
    };
    let label = match notifier.send_recovery(&message).await {
        Ok(delivery) => delivery.state_label().to_string(),
        Err(err) => {
            tracing::warn!(
                target: "storefront.recovery",
                cart_id = %cart_id,
                error = %err,
                "recovery delivery refused by the notifier port — recorded as pending"
            );
            "pending".to_string()
        }
    };
    // (5) The audit-stamp row (notified_at is a stamp, NEVER an
    // eligibility input) + the audit event.
    sqlx::query(
        r#"
        INSERT INTO storefront.recovery_invites
            (id, cart_id, template_ref, notified_at, delivery_state)
        VALUES (gen_random_uuid(), $1, $2, now(), $3)
        "#,
    )
    .bind(cart_id)
    .bind(&template_ref)
    .bind(&label)
    .execute(pool)
    .await?;
    record_audit(
        pool,
        Some(website_id),
        "recovery_sent",
        officer,
        Some("cart"),
        Some(cart_id),
        Some(serde_json::json!({ "delivery_state": label })),
    )
    .await?;
    Ok(label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandonment_window_reads_the_one_knob_with_a_safe_default() {
        let hours = abandoned_after_hours();
        assert!(hours > 0);
        assert_eq!(DEFAULT_ABANDONED_AFTER_HOURS, 1);
        assert_eq!(ABANDONED_AFTER_HOURS_ENV, "STOREFRONT_ABANDONED_AFTER_HOURS");
    }
}
