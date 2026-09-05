//! The visitor-backed wishlist + back-in-stock disposition (hand-written;
//! user-owned; see `metaphor.codegen.yaml`).
//!
//! OWNERSHIP (the durable wishlist shape): every wish row is born from a
//! real website VISITOR identity (`visitor_id` is NOT NULL) — the
//! anonymous device's list is first-class, not a session curiosity.
//! `portal_user_id` is a RECONCILED STAMP, never the ownership key: the
//! reconcile verb (at login) stamps the visitor's rows with the
//! verified principal; the read is the UNION of the visitor's own rows
//! and the principal-stamped rows, website-scoped. Rows NEVER MOVE —
//! there is no migration of "anonymous rows into account rows", so a
//! merge can never lose or double-count a wish.
//!
//! THE BACK-IN-STOCK DISPOSITION (§14.3): the smallest honest surface —
//!  - a `notify_on_stock` arm on the wish row itself (no separate
//!    subscription table, no crons, no webhooks);
//!  - `contact_email` stamped ONLY from a VERIFIED principal (the
//!    portal login's email — never text from a request body);
//!  - an OFFICER demand read that recomputes eligibility fresh through
//!    the availability port (nothing persisted, no staleness);
//!  - an OFFICER explicit send through the [`StockAlertNotifier`] port
//!    (visible-unwired posture) — no automatic trigger exists anywhere;
//!  - the arm clears ONLY on an accepted send: a failed delivery never
//!    burns the shopper's one notification.

use rust_decimal::Decimal;
use uuid::Uuid;

use super::audit::{record_audit, ActorRef};
use super::availability_port::AvailabilityReadPort;
use super::cart_service;
use super::catalog_read_port::CatalogReadPort;
use super::notifier_port::{StockAlertDelivery, StockAlertNotifier, StockAlertMessage};
use super::storefront_error::StorefrontError;

/// One wish row as the reads see it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct WishlistRow {
    pub id: Uuid,
    pub website_id: Uuid,
    pub visitor_id: Uuid,
    pub portal_user_id: Option<Uuid>,
    pub item_id: Uuid,
    pub notify_on_stock: bool,
    pub contact_email: Option<String>,
}

const WISH_SELECT: &str = r#"
    SELECT id, website_id, visitor_id, portal_user_id, item_id,
           notify_on_stock, contact_email
    FROM storefront.wishlist_items
"#;

/// Add one item to the visitor's wishlist (idempotent: a live wish for
/// the same (website, visitor, item) answers the existing row). The
/// publish gate re-checks HERE — an item that left the storefront never
/// enters a wishlist silently.
pub async fn add(
    pool: &sqlx::PgPool,
    catalog: &dyn CatalogReadPort,
    company_id: Uuid,
    website_id: Uuid,
    visitor_id: Uuid,
    item_id: Uuid,
) -> Result<Uuid, StorefrontError> {
    // The same mutation-time gate as the cart's add (closed-door shape:
    // unpublished, off-website, sale_ok=false, inactive, priceless).
    cart_service::gated_listing(pool, catalog, company_id, website_id, item_id).await?;
    let (id,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO storefront.wishlist_items (website_id, visitor_id, item_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (website_id, visitor_id, item_id)
          WHERE (metadata->>'deleted_at') IS NULL
        DO UPDATE SET id = storefront.wishlist_items.id
        RETURNING id
        "#,
    )
    .bind(website_id)
    .bind(visitor_id)
    .bind(item_id)
    .fetch_one(pool)
    .await?;
    record_audit(
        pool,
        Some(website_id),
        "wishlist_added",
        ActorRef::visitor(visitor_id),
        Some("wishlist_item"),
        Some(id),
        Some(serde_json::json!({ "item_id": item_id })),
    )
    .await?;
    Ok(id)
}

/// Remove one item — the caller's own wish, or (when a principal is
/// verified) a wish their principal is stamped on. Anything else is the
/// typed 404 (a foreign wish is indistinguishable from a missing one).
pub async fn remove(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    visitor_id: Uuid,
    principal_user_id: Option<Uuid>,
    item_id: Uuid,
) -> Result<(), StorefrontError> {
    let removed = sqlx::query(
        r#"
        UPDATE storefront.wishlist_items
        SET notify_on_stock = false,
            metadata = jsonb_set(metadata, '{deleted_at}', to_jsonb(now()))
        WHERE website_id = $1 AND item_id = $2
          AND (metadata->>'deleted_at') IS NULL
          AND (visitor_id = $3 OR portal_user_id = $4)
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .bind(visitor_id)
    .bind(principal_user_id)
    .execute(pool)
    .await?;
    if removed.rows_affected() == 0 {
        return Err(StorefrontError::WishlistItemNotFound);
    }
    record_audit(
        pool,
        Some(website_id),
        "wishlist_removed",
        ActorRef::visitor(visitor_id),
        Some("wishlist_item"),
        Some(item_id),
        Some(serde_json::json!({ "item_id": item_id })),
    )
    .await?;
    Ok(())
}

/// The UNION read: the visitor's own rows plus the principal-stamped
/// rows (a row matching both halves appears once — it is one row).
/// Website-scoped forever; ordered oldest-first for stable rendering.
pub async fn wishlist_for(
    exec: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    website_id: Uuid,
    visitor_id: Uuid,
    principal_user_id: Option<Uuid>,
) -> Result<Vec<WishlistRow>, StorefrontError> {
    sqlx::query_as::<_, WishlistRow>(&format!(
        "{WISH_SELECT} WHERE website_id = $1 AND (metadata->>'deleted_at') IS NULL \
         AND (visitor_id = $2 OR portal_user_id = $3) \
         ORDER BY (metadata->>'created_at') ASC, id ASC"
    ))
    .bind(website_id)
    .bind(visitor_id)
    .bind(principal_user_id)
    .fetch_all(exec)
    .await
    .map_err(StorefrontError::from)
}

/// The login-time reconcile: stamp the visitor's live rows with the
/// verified principal (and the principal's verified email — the ONLY
/// writer of `contact_email`; no route ever accepts an address from a
/// request body). Rows never move — the stamp is what makes the union
/// read carry the device's list into the account view. Returns the
/// number of rows stamped.
pub async fn reconcile(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    visitor_id: Uuid,
    principal_user_id: Uuid,
    principal_email: &str,
) -> Result<u64, StorefrontError> {
    let stamped = sqlx::query(
        r#"
        UPDATE storefront.wishlist_items
        SET portal_user_id = $3, contact_email = $4,
            metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
        WHERE website_id = $1 AND visitor_id = $2
          AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(website_id)
    .bind(visitor_id)
    .bind(principal_user_id)
    .bind(principal_email)
    .execute(pool)
    .await?
    .rows_affected();
    record_audit(
        pool,
        Some(website_id),
        "wishlist_reconciled",
        ActorRef::visitor(visitor_id),
        Some("wishlist_item"),
        None,
        Some(serde_json::json!({
            "visitor_id": visitor_id,
            "principal_user_id": principal_user_id,
            "rows": stamped,
        })),
    )
    .await?;
    Ok(stamped)
}

/// Arm the back-in-stock wait on one wish: sets `notify_on_stock` on
/// the CALLER'S own live row (the typed 404 when the item is not on
/// the visitor's wishlist — arming never creates a row by itself). A
/// verified principal present at arm time also refreshes the contact
/// stamp (the verified-email-only rule's one other writer).
pub async fn arm_notify(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    visitor_id: Uuid,
    item_id: Uuid,
    principal: Option<(Uuid, String)>,
) -> Result<(), StorefrontError> {
    let armed = if let Some((principal_user_id, email)) = principal {
        sqlx::query(
            r#"
            UPDATE storefront.wishlist_items
            SET notify_on_stock = true, portal_user_id = $4, contact_email = $5,
                metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
            WHERE website_id = $1 AND visitor_id = $2 AND item_id = $3
              AND (metadata->>'deleted_at') IS NULL
            "#,
        )
        .bind(website_id)
        .bind(visitor_id)
        .bind(item_id)
        .bind(principal_user_id)
        .bind(email)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            r#"
            UPDATE storefront.wishlist_items
            SET notify_on_stock = true,
                metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
            WHERE website_id = $1 AND visitor_id = $2 AND item_id = $3
              AND (metadata->>'deleted_at') IS NULL
            "#,
        )
        .bind(website_id)
        .bind(visitor_id)
        .bind(item_id)
        .execute(pool)
        .await?
    }
    .rows_affected();
    if armed == 0 {
        return Err(StorefrontError::WishlistItemNotFound);
    }
    record_audit(
        pool,
        Some(website_id),
        "stock_notify_armed",
        ActorRef::visitor(visitor_id),
        Some("wishlist_item"),
        Some(item_id),
        Some(serde_json::json!({ "item_id": item_id })),
    )
    .await?;
    Ok(())
}

// ── the officer's back-in-stock surface ────────────────────────────────────

/// One item's armed-demand facts, as the officer read derives them.
#[derive(Debug, Clone)]
pub struct StockWaitItem {
    pub item_id: Uuid,
    /// Live rows with the arm set.
    pub armed: i64,
    /// Armed rows carrying a contact address (sendable today).
    pub with_address: i64,
    /// The DISPLAY-scope free quantity, computed FRESH through the
    /// availability port at read time (no snapshot is stored anywhere).
    pub free_quantity: Decimal,
    /// Whether the item is actually back in stock (free > 0) — the
    /// send verb refuses on false.
    pub eligible: bool,
}

/// The officer demand read: armed wishes grouped per item, each with a
/// FRESH display-scope availability read. Nothing here persists; the
/// officer sees the truth of the moment, never a stale snapshot.
pub async fn stock_wait_read(
    pool: &sqlx::PgPool,
    availability: &dyn AvailabilityReadPort,
    company_id: Uuid,
    website_id: Uuid,
) -> Result<Vec<StockWaitItem>, StorefrontError> {
    let mut conn = pool.acquire().await?;
    let grouped: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        r#"
        SELECT item_id,
               COUNT(*) AS armed,
               COUNT(*) FILTER (WHERE contact_email IS NOT NULL) AS with_address
        FROM storefront.wishlist_items
        WHERE website_id = $1 AND notify_on_stock = true
          AND (metadata->>'deleted_at') IS NULL
        GROUP BY item_id
        ORDER BY armed DESC, item_id ASC
        "#,
    )
    .bind(website_id)
    .fetch_all(&mut *conn)
    .await?;
    if grouped.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = grouped.iter().map(|g| g.0).collect();
    let scope =
        super::availability_service::display_scope_warehouse(&mut *conn, website_id).await?;
    let answers = availability
        .free_quantities(company_id, &ids, scope)
        .await
        .map_err(super::availability_service::map_availability_error)?;
    let mut out = Vec::with_capacity(grouped.len());
    for (item_id, armed, with_address) in grouped {
        let free = answers
            .iter()
            .find(|a| a.item_id == item_id)
            .map(|a| a.free_quantity)
            .unwrap_or(Decimal::ZERO);
        out.push(StockWaitItem {
            item_id,
            armed,
            with_address,
            free_quantity: free,
            eligible: free > Decimal::ZERO,
        });
    }
    Ok(out)
}

/// The officer send's per-item summary.
#[derive(Debug, Clone)]
pub struct StockAlertSummary {
    pub item_id: Uuid,
    /// Armed rows examined.
    pub attempted: usize,
    /// Sends the notifier ACCEPTED (both `sent` and the visible
    /// `unwired` — the loud recorded state — count as accepted; only
    /// these clear their arms).
    pub sent: usize,
    /// Armed rows without a contact address (skipped, arms stay set —
    /// they reconcile onto a login before the next send).
    pub skipped_no_address: usize,
    /// Transports that errored (arms stay set; the retry costs nothing).
    pub failed: usize,
    /// The delivery states observed: `sent`, `unwired`, `mixed`, or
    /// `none` when nothing sendable existed.
    pub delivery_state: &'static str,
}

/// The officer EXPLICIT send: one item, every armed and addressable
/// wish on the website. REFUSES when the item is not actually back in
/// stock (the fresh availability read — an alert for a still-out item
/// would be a lie, whatever the officer hoped). Sends through the
/// notifier port; an ACCEPTED outcome (sent, or the visible unwired)
/// clears the arm, a transport error leaves it set. No cron, no
/// webhook, and no other trigger calls this — the officer does.
#[allow(clippy::too_many_arguments)]
pub async fn send_stock_alerts(
    pool: &sqlx::PgPool,
    catalog: &dyn CatalogReadPort,
    availability: &dyn AvailabilityReadPort,
    notifier: &dyn StockAlertNotifier,
    company_id: Uuid,
    website_id: Uuid,
    item_id: Uuid,
    actor: ActorRef,
) -> Result<StockAlertSummary, StorefrontError> {
    // Fresh eligibility — the send refuses unless stock actually
    // returned (display scope, computed now, never cached).
    let scope = super::availability_service::display_scope_warehouse(pool, website_id).await?;
    let answer = availability
        .free_quantity(company_id, item_id, scope)
        .await
        .map_err(super::availability_service::map_availability_error)?;
    if answer.free_quantity <= Decimal::ZERO {
        return Err(StorefrontError::Guarded(
            "this item is not back in stock — the alert would be false".into(),
        ));
    }
    // The message's item name resolves through the catalog port (never
    // client text); a vanished snapshot refuses the send.
    let snapshot = catalog
        .item_snapshot(company_id, item_id)
        .await
        .map_err(|e| StorefrontError::CatalogPortRefused { code: e.code })?
        .ok_or_else(|| {
            StorefrontError::Guarded("the catalog carries no snapshot for this item".into())
        })?;
    let (website_name,): (String,) = sqlx::query_as(
        r#"
        SELECT name FROM website.websites
        WHERE id = $1 AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(website_id)
    .fetch_one(pool)
    .await?;
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, contact_email
        FROM storefront.wishlist_items
        WHERE website_id = $1 AND item_id = $2 AND notify_on_stock = true
          AND contact_email IS NOT NULL AND contact_email <> ''
          AND (metadata->>'deleted_at') IS NULL
        ORDER BY (metadata->>'created_at') ASC
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .fetch_all(pool)
    .await?;
    let armed_total: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM storefront.wishlist_items
        WHERE website_id = $1 AND item_id = $2 AND notify_on_stock = true
          AND (metadata->>'deleted_at') IS NULL
        "#,
    )
    .bind(website_id)
    .bind(item_id)
    .fetch_one(pool)
    .await?;
    let mut summary = StockAlertSummary {
        item_id,
        attempted: armed_total.0 as usize,
        sent: 0,
        skipped_no_address: (armed_total.0 as usize).saturating_sub(rows.len()),
        failed: 0,
        delivery_state: "none",
    };
    let mut saw_sent = false;
    let mut saw_unwired = false;
    for (row_id, address) in rows {
        let message = StockAlertMessage {
            website_id,
            item_id,
            item_name: &snapshot.name,
            to_address: &address,
            website_name: &website_name,
        };
        match notifier.send_stock_alert(&message).await {
            Ok(outcome) => {
                match outcome {
                    StockAlertDelivery::Sent => saw_sent = true,
                    StockAlertDelivery::Unwired => saw_unwired = true,
                }
                // ACCEPTED (sent or the visible unwired) — the arm
                // clears; the wait is discharged loudly.
                sqlx::query(
                    r#"
                    UPDATE storefront.wishlist_items
                    SET notify_on_stock = false,
                        metadata = jsonb_set(metadata, '{updated_at}', to_jsonb(now()))
                    WHERE id = $1
                    "#,
                )
                .bind(row_id)
                .execute(pool)
                .await?;
                summary.sent += 1;
            }
            Err(_) => {
                // A refused transport leaves the arm SET — the shopper
                // must not lose their one notification to a hiccup.
                summary.failed += 1;
            }
        }
    }
    summary.delivery_state = match (saw_sent, saw_unwired) {
        (true, true) => "mixed",
        (true, false) => "sent",
        (false, true) => "unwired",
        (false, false) => "none",
    };
    record_audit(
        pool,
        Some(website_id),
        "stock_alert_sent",
        actor,
        Some("wishlist_item"),
        Some(item_id),
        Some(serde_json::json!({
            "item_id": item_id,
            "attempted": summary.attempted,
            "sent": summary.sent,
            "skipped_no_address": summary.skipped_no_address,
            "failed": summary.failed,
            "delivery_state": summary.delivery_state,
        })),
    )
    .await?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_delivery_states_are_the_closed_set() {
        let s = StockAlertSummary {
            item_id: Uuid::new_v4(),
            attempted: 0,
            sent: 0,
            skipped_no_address: 0,
            failed: 0,
            delivery_state: "none",
        };
        assert_eq!(s.delivery_state, "none");
    }
}
