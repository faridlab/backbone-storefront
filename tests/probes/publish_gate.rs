//! Gate 4 (§4): the publish gate. Every closed door — unpublished,
//! `sale_ok=false`, inactive catalog item, missing price row,
//! other-website listing — reads the SAME typed refusal on detail and
//! is structurally absent from the listing; `is_published` is fenced
//! out of the upsert (422 + `publish_refused` audit); only the
//! publish/unpublish verbs flip the flag.

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_storefront::application::service::catalog_service::{self, SortKind};
use backbone_storefront::application::service::storefront_error::StorefrontError;

use super::common::{get, post, seed_listing, Probe};

fn dec(units: i64) -> Decimal {
    Decimal::new(units * 100, 2)
}

#[tokio::test]
async fn every_closed_door_reads_the_same_typed_refusal() {
    let probe = Probe::boot("gate").await;
    let pool = &probe.pool;
    let company = probe.company_id;
    let site = probe.view.id;

    // The five closed doors + the one open door.
    let visible = seed_listing(pool, &probe.catalog, site, "Visible", dec(100), true).await;

    let unpublished = seed_listing(pool, &probe.catalog, site, "Unpublished", dec(100), false).await;

    // sale_ok=false with is_published=true: the gate's conjunct.
    let not_saleable = seed_listing(pool, &probe.catalog, site, "NotSaleable", dec(100), true).await;
    sqlx_close_sale_ok(pool, site, not_saleable).await;

    let inactive = seed_listing(pool, &probe.catalog, site, "Inactive", dec(100), true).await;
    probe.catalog.archive(inactive);

    let unpriced = seed_listing(pool, &probe.catalog, site, "Unpriced", dec(100), true).await;
    sqlx_drop_price(pool, site, unpriced).await;

    // The other-website door: listed + published + priced on site B.
    let site_b = super::common::seed_website(pool, "Site B", company).await.id;
    let foreign = seed_listing(pool, &probe.catalog, site_b, "Foreign", dec(100), true).await;

    // Listing: ONLY the visible item, on either door shape.
    let rows = catalog_service::public_listings(
        pool,
        probe.catalog.as_ref(),
        company,
        site,
        None,
        SortKind::Relevance,
        1,
        50,
    )
    .await
    .unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["Visible"], "every closed door is absent");

    // Detail: every closed door answers the SAME variant.
    let mut refusals = 0;
    for item in [unpublished, not_saleable, inactive, unpriced, foreign] {
        let err = catalog_service::public_detail(
            pool,
            probe.catalog.as_ref(),
            company,
            site,
            item,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, StorefrontError::PublishGateRefused),
            "closed door must read the typed refusal, got {err:?} for {item}"
        );
        refusals += 1;
    }
    assert_eq!(refusals, 5);

    // The open door reads fine.
    let open = catalog_service::public_detail(pool, probe.catalog.as_ref(), company, site, visible)
        .await
        .unwrap();
    assert_eq!(open.name, "Visible");

    // Route level: the closed doors are 404s, byte-identical bodies.
    let (status_visible, body_visible) = get(&probe.public, &format!("/public/catalog/{visible}"), None).await;
    assert_eq!(status_visible, axum::http::StatusCode::OK);
    let (status_closed, body_closed) = get(&probe.public, &format!("/public/catalog/{unpublished}"), None).await;
    assert_eq!(status_closed, axum::http::StatusCode::NOT_FOUND);
    let (_, body_closed2) = get(&probe.public, &format!("/public/catalog/{inactive}"), None).await;
    assert_eq!(
        body_closed, body_closed2,
        "the closed-door 404 carries no door-identity oracle"
    );
    let _ = body_visible;
    probe.dispose().await;
}

#[tokio::test]
async fn is_published_is_fenced_out_of_the_upsert() {
    let probe = Probe::boot("fence").await;
    let pool = &probe.pool;
    let site = probe.view.id;
    let item = Uuid::new_v4();

    // A body carrying is_published: the typed 422, BEFORE any write.
    let body = serde_json::json!({
        "website_id": site,
        "item_id": item,
        "sale_ok": true,
        "sequence": 5,
        "media_urls": ["https://cdn.example.test/x.jpg"],
        "is_published": true,
    });
    let (status, json) = post(&probe.admin, "/admin/listings", None, &body.to_string()).await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY, "{json}");
    assert_eq!(json["code"], "storefront_field_not_patchable", "{json}");

    // The refusal is audited exactly once.
    let audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.storefront_audit_log \
         WHERE event = 'publish_refused' AND subject_id = $1",
    )
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "the refusal leaves its audit row");

    // And NO listing row was written by the refused verb.
    let listings = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.product_listings WHERE item_id = $1",
    )
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(listings, 0);

    // Without the field: the upsert lands UNPUBLISHED (a fresh row is
    // born closed; only the verb opens it).
    let body = serde_json::json!({
        "website_id": site,
        "item_id": item,
        "sale_ok": true,
        "media_urls": ["https://cdn.example.test/x.jpg"],
    });
    let (status, json) = post(&probe.admin, "/admin/listings", None, &body.to_string()).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{json}");
    let listing_id: Uuid = serde_json::from_value(json["listing_id"].clone()).unwrap();
    let published = sqlx::query_scalar::<_, bool>(
        "SELECT is_published FROM storefront.product_listings WHERE id = $1",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(!published, "a fresh listing is born unpublished");

    // Only the verbs flip it, and the flips are guarded + audited.
    let (status, _) = post(
        &probe.admin,
        &format!("/admin/listings/{listing_id}/publish"),
        None,
        "{}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let published = sqlx::query_scalar::<_, bool>(
        "SELECT is_published FROM storefront.product_listings WHERE id = $1",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(published);

    // Re-publish (already published): the guarded 404, no second
    // audit row.
    let (status, _) = post(
        &probe.admin,
        &format!("/admin/listings/{listing_id}/publish"),
        None,
        "{}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    let audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.storefront_audit_log \
         WHERE event = 'listing_published' AND subject_id = $1",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "the publish verb stamps exactly once");

    let (status, _) = post(
        &probe.admin,
        &format!("/admin/listings/{listing_id}/unpublish"),
        None,
        "{}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    probe.dispose().await;
}

// ── raw close-door helpers (the doors the verbs cannot produce) ────────────

async fn sqlx_close_sale_ok(pool: &sqlx::PgPool, website_id: Uuid, item_id: Uuid) {
    sqlx::query(
        "UPDATE storefront.product_listings SET sale_ok = false \
         WHERE website_id = $1 AND item_id = $2",
    )
    .bind(website_id)
    .bind(item_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn sqlx_drop_price(pool: &sqlx::PgPool, website_id: Uuid, item_id: Uuid) {
    sqlx::query(
        "DELETE FROM storefront.product_prices \
         WHERE website_id = $1 AND item_id = $2",
    )
    .bind(website_id)
    .bind(item_id)
    .execute(pool)
    .await
    .unwrap();
}
