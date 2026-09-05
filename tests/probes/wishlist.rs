//! The wishlist MERGE probe (spec §14.3, the register's durable-wishlist
//! shape): every wish row is born from a VISITOR identity, the portal
//! link is a reconciled STAMP (never the ownership key), rows NEVER
//! move — the union read is what carries a device's list into the
//! account view. A second device of the same principal sees the union
//! while the first device's rows stay exactly where they were born;
//! removal is scoped to the caller's visible rows; and the merge is
//! WEBSITE-BLIND-PROOF: the same principal's wishes on another website
//! never leak into this website's view (the session shape's
//! website-blind merge is the anti-spec this module does not port).

use std::sync::Arc;

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_portal::exports::PortalUserId;
use backbone_storefront::presentation::http::{storefront_public_routes, StorefrontPublicState};
use backbone_website::exports::WebsitePrincipal;

use super::common::{
    post, post_dual, seed_listing, seed_visitor, seed_website, send_dual, StubAvailability,
    StubCatalog, StubParty, StubPricing, StubSurface, StubTax, StubVerifier, TestDb,
};

const BEARER: &str = "probe-bearer";
const PRINCIPAL_EMAIL: &str = "merger@account.test";

/// One website + one principal-verifying public router (the login-time
/// reconcile and arm verbs need BOTH rungs; the boot probe's default
/// verifier refuses every bearer).
struct WishRig {
    public: axum::Router,
    pool: sqlx::PgPool,
    site: backbone_website::exports::WebsiteView,
    catalog: Arc<StubCatalog>,
    principal_user: Uuid,
    _db: TestDb,
}

async fn rig(marker: &str) -> WishRig {
    let db = TestDb::new(marker).await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let site = seed_website(&pool, "Wish Store", company).await;
    let catalog = Arc::new(StubCatalog::default());
    let party = Arc::new(StubParty::new());
    let tax = Arc::new(StubTax(Decimal::ZERO));
    let pricing = Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let availability = Arc::new(StubAvailability::new());
    let mut state = StorefrontPublicState::compose(
        pool.clone(),
        Arc::new(StubSurface::binding(site.clone())),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    );
    let principal_user = Uuid::new_v4();
    state.install_principal_verifier(Arc::new(StubVerifier {
        token: BEARER.into(),
        principal: WebsitePrincipal {
            user_id: PortalUserId::from(principal_user),
            email: PRINCIPAL_EMAIL.into(),
        },
    }));
    WishRig {
        public: storefront_public_routes(state),
        pool,
        site,
        catalog,
        principal_user,
        _db: db,
    }
}

async fn wish(app: &axum::Router, token: &str, item: Uuid) -> Uuid {
    let (status, json) = post(app, "/public/wishlist", Some(token), &format!("{{\"item_id\": \"{item}\"}}")).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    json["wishlist_item_id"].as_str().unwrap().parse().unwrap()
}

async fn read_count(app: &axum::Router, token: Option<&str>, bearer: Option<&str>) -> usize {
    let (status, bytes) = send_dual(app, "GET", "/public/wishlist", token, bearer, None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["items"].as_array().unwrap().len()
}

async fn live_rows(pool: &sqlx::PgPool, sql: &str, key: Uuid) -> i64 {
    let n = sqlx::query_scalar::<_, i64>(sql)
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap();
    n
}

#[tokio::test]
async fn the_union_read_merges_without_moving_a_single_row() {
    let rig = rig("wishmerge").await;
    let site = rig.site.id;
    let pool = &rig.pool;

    let a1 = seed_listing(pool, &rig.catalog, site, "Wish A1", Decimal::new(1000, 2), true).await;
    let a2 = seed_listing(pool, &rig.catalog, site, "Wish A2", Decimal::new(1000, 2), true).await;
    let a3 = seed_listing(pool, &rig.catalog, site, "Wish A3", Decimal::new(1000, 2), true).await;
    let d4 = seed_listing(pool, &rig.catalog, site, "Wish D4", Decimal::new(1000, 2), true).await;
    let e5 = seed_listing(pool, &rig.catalog, site, "Wish E5", Decimal::new(1000, 2), true).await;

    // Device A (anonymous): three wishes, all born on A's visitor id.
    let (visitor_a, token_a) = seed_visitor(pool, site).await;
    let row_a1 = wish(&rig.public, &token_a, a1).await;
    let row_a2 = wish(&rig.public, &token_a, a2).await;
    let _row_a3 = wish(&rig.public, &token_a, a3).await;
    assert_eq!(read_count(&rig.public, Some(&token_a), None).await, 3);

    // LOGIN on device A: the reconcile STAMPS the visitor's live rows —
    // it moves nothing and creates nothing.
    let (status, json) = post_dual(&rig.public, "/public/wishlist/reconcile", Some(&token_a), Some(BEARER), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["reconciled"], 3);
    let stamped_both: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM storefront.wishlist_items \
         WHERE portal_user_id = $1 AND contact_email = $2 AND (metadata->>'deleted_at') IS NULL",
    )
    .bind(rig.principal_user)
    .bind(PRINCIPAL_EMAIL)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(stamped_both, 3, "all three of A's rows carry the principal stamp + verified address");

    // Device B (a second device of the SAME principal, fresh visitor):
    // two more wishes, then the same login.
    let (visitor_b, token_b) = seed_visitor(pool, site).await;
    wish(&rig.public, &token_b, d4).await;
    wish(&rig.public, &token_b, e5).await;
    let (status, json) = post_dual(&rig.public, "/public/wishlist/reconcile", Some(&token_b), Some(BEARER), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["reconciled"], 2);

    // THE MERGE READ: device B sees the union (2 + 3 = 5); the
    // device-local read (no bearer) still sees only its own 2.
    assert_eq!(read_count(&rig.public, Some(&token_b), None).await, 2);
    assert_eq!(read_count(&rig.public, Some(&token_b), Some(BEARER)).await, 5);
    // Device A's union is the SAME five: its own 3 (already stamped)
    // plus B's principal-stamped 2 — the merge is symmetric because it
    // is a READ union, not a row migration. B's rows never became A's
    // rows (the ownership counts below prove that); both devices merely
    // see the one account's whole list.
    assert_eq!(read_count(&rig.public, Some(&token_a), Some(BEARER)).await, 5);

    // ROWS NEVER MOVED: the per-visitor row counts are exactly what
    // each device added — no "anonymous rows migrated into the account".
    let rows_a = live_rows(
        pool,
        "SELECT count(*) FROM storefront.wishlist_items \
         WHERE visitor_id = $1 AND (metadata->>'deleted_at') IS NULL",
        visitor_a,
    )
    .await;
    let rows_b = live_rows(
        pool,
        "SELECT count(*) FROM storefront.wishlist_items \
         WHERE visitor_id = $1 AND (metadata->>'deleted_at') IS NULL",
        visitor_b,
    )
    .await;
    assert_eq!((rows_a, rows_b), (3, 2), "no row changed hands between devices");

    // REMOVE-BY-PRINCIPAL from device B can retire a row device A
    // added (it is in B's principal-visible set) — the soft delete, not
    // a move.
    let (status, json) = post_dual(
        &rig.public,
        &format!("/public/wishlist/{a1}/remove"),
        Some(&token_b),
        Some(BEARER),
        "{}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["removed"], true);
    let a1_state: Option<String> = sqlx::query_scalar(
        "SELECT metadata->>'deleted_at' FROM storefront.wishlist_items WHERE id = $1",
    )
    .bind(row_a1)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(a1_state.is_some(), "the retired row is soft-deleted, still on device A's visitor id");

    // A FOREIGN visitor (no principal) sees nothing to remove: B's row
    // is indistinguishable from a missing one.
    let (_visitor_c, token_c) = seed_visitor(pool, site).await;
    let (status, json) = post(&rig.public, &format!("/public/wishlist/{d4}/remove"), Some(&token_c), "{}").await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "storefront_wishlist_item_not_found");

    // Reconcile refuses without BOTH rungs.
    let (status, json) = post(&rig.public, "/public/wishlist/reconcile", Some(&token_a), "{}").await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(json["code"], "storefront_principal_required");

    // IDEMPOTENT ADD: re-wishing an item the visitor already holds
    // answers the SAME row (the partial-unique arbiter), minting none.
    let again = wish(&rig.public, &token_a, a2).await;
    assert_eq!(again, row_a2, "the idempotent add returns the existing row id");
    let total_live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM storefront.wishlist_items \
         WHERE website_id = $1 AND (metadata->>'deleted_at') IS NULL",
    )
    .bind(site)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(total_live, 4, "3 + 2 rows less the one soft-deleted, none minted by re-adds");

    // The closed door: an unpublished item never enters a wishlist.
    let hidden = seed_listing(pool, &rig.catalog, site, "Wish Hidden", Decimal::new(1000, 2), false).await;
    let (status, json) = post(&rig.public, "/public/wishlist", Some(&token_a), &format!("{{\"item_id\": \"{hidden}\"}}")).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "storefront_publish_gate_refused");

    rig._db.dispose().await;
}

#[tokio::test]
async fn the_merge_never_leaks_across_websites() {
    let rig = rig("wishsite").await;
    let pool = &rig.pool;

    // A SECOND website bound by its own router over the same database:
    // the same principal (same bearer) shops both stores.
    let site2 = seed_website(pool, "Wish Sister Store", Uuid::new_v4()).await;
    let mut state2 = StorefrontPublicState::compose(
        pool.clone(),
        Arc::new(StubSurface::binding(site2.clone())),
        rig.catalog.clone(),
        Arc::new(StubParty::new()),
        Arc::new(StubTax(Decimal::ZERO)),
        Arc::new(StubPricing::new(Decimal::ONE, Vec::new())),
        Arc::new(StubAvailability::new()),
    );
    state2.install_principal_verifier(Arc::new(StubVerifier {
        token: BEARER.into(),
        principal: WebsitePrincipal {
            user_id: PortalUserId::from(rig.principal_user),
            email: PRINCIPAL_EMAIL.into(),
        },
    }));
    let public2 = storefront_public_routes(state2);

    let on1 = seed_listing(pool, &rig.catalog, rig.site.id, "Home Item", Decimal::new(1000, 2), true).await;
    let on2 = seed_listing(pool, &rig.catalog, site2.id, "Sister Item", Decimal::new(1000, 2), true).await;

    // The principal wishes + reconciles on EACH website with a distinct
    // visitor identity (the two storefronts mint their own tokens).
    let (_v1, token1) = seed_visitor(pool, rig.site.id).await;
    wish(&rig.public, &token1, on1).await;
    let (status, json) = post_dual(&rig.public, "/public/wishlist/reconcile", Some(&token1), Some(BEARER), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["reconciled"], 1);
    let (_v2, token2) = seed_visitor(pool, site2.id).await;
    wish(&public2, &token2, on2).await;
    let (status, json) = post_dual(&public2, "/public/wishlist/reconcile", Some(&token2), Some(BEARER), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["reconciled"], 1);

    // The same principal, both websites stamped — yet each website's
    // union read shows ONLY its own rows (the read is website-scoped;
    // a principal's cross-store wishes never bleed across storefronts).
    assert_eq!(read_count(&rig.public, Some(&token1), Some(BEARER)).await, 1);
    assert_eq!(read_count(&public2, Some(&token2), Some(BEARER)).await, 1);

    // And the stamp table itself stays website-partitioned.
    let per_site: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT website_id, count(*) FROM storefront.wishlist_items \
         WHERE portal_user_id = $1 AND (metadata->>'deleted_at') IS NULL \
         GROUP BY website_id",
    )
    .bind(rig.principal_user)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(per_site.len(), 2, "one stamped row per website, no shared row");
    rig._db.dispose().await;
}
