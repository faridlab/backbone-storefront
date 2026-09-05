//! The back-in-stock disposition (spec §14.3): the smallest honest
//! surface. The arm rides the wish row itself; the contact address is
//! stamped ONLY from a verified principal (never a request body); the
//! officer demand read recomputes eligibility FRESH through the
//! availability port; the officer EXPLICIT send refuses while the item
//! is still out of stock, sends on restock, and clears the arm ONLY on
//! an accepted delivery — a transport failure leaves the shopper's
//! one notification armed.

use std::sync::Arc;

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_portal::exports::PortalUserId;
use backbone_storefront::presentation::http::{
    storefront_admin_routes, storefront_public_routes, StorefrontAdminState, StorefrontPublicState,
};
use backbone_website::exports::WebsitePrincipal;

use super::common::{
    get, post, post_dual, seed_listing, seed_visitor, seed_website, StubAvailability, StubCatalog,
    StubParty, StubPricing, StubStockNotifier, StubSurface, StubTax, StubVerifier, TestDb,
};

const BEARER: &str = "probe-bearer";
const EMAIL: &str = "waiter@account.test";

/// The stock-wait rig: a public router that verifies bearers (the arm
/// stamps its contact from the principal) + the admin router with the
/// catalog/availability/notifier ports installed — the officer's two
/// surfaces.
struct WaitRig {
    public: axum::Router,
    admin: axum::Router,
    pool: sqlx::PgPool,
    site: Uuid,
    catalog: Arc<StubCatalog>,
    availability: Arc<StubAvailability>,
    stock_notifier: Arc<StubStockNotifier>,
    _db: TestDb,
}

async fn rig(marker: &str) -> WaitRig {
    let db = TestDb::new(marker).await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let site = seed_website(&pool, "Wait Store", company).await;
    let catalog = Arc::new(StubCatalog::default());
    let availability = Arc::new(StubAvailability::new());
    let stock_notifier = Arc::new(StubStockNotifier::default());
    let mut public_state = StorefrontPublicState::compose(
        pool.clone(),
        Arc::new(StubSurface::binding(site.clone())),
        catalog.clone(),
        Arc::new(StubParty::new()),
        Arc::new(StubTax(Decimal::ZERO)),
        Arc::new(StubPricing::new(Decimal::ONE, Vec::new())),
        availability.clone(),
    );
    public_state.install_principal_verifier(Arc::new(StubVerifier {
        token: BEARER.into(),
        principal: WebsitePrincipal {
            user_id: PortalUserId::from(Uuid::new_v4()),
            email: EMAIL.into(),
        },
    }));
    let mut admin_state = StorefrontAdminState::new(pool.clone());
    admin_state.install_catalog_port(catalog.clone());
    admin_state.install_availability_port(availability.clone());
    admin_state.install_stock_notifier(stock_notifier.clone());
    WaitRig {
        public: storefront_public_routes(public_state),
        admin: storefront_admin_routes(admin_state),
        pool,
        site: site.id,
        catalog,
        availability,
        stock_notifier,
        _db: db,
    }
}

/// Read one JSON field as a decimal whether serde wrote it as a string
/// or a bare number (the wire contract is the value, not the quoting).
fn dec(v: &serde_json::Value) -> Decimal {
    use std::str::FromStr;
    if let Some(s) = v.as_str() {
        return Decimal::from_str(s).unwrap_or_else(|_| panic!("not a decimal string: {s}"));
    }
    if let Some(n) = v.as_i64() {
        return Decimal::from(n);
    }
    panic!("not a decimal field: {v}");
}

/// The officer demand read, parsed: (item_id, armed, with_address,
/// free_quantity, eligible) for the FIRST row.
async fn wait_row(rig: &WaitRig) -> serde_json::Value {
    let (status, bytes) = get(
        &rig.admin,
        &format!("/admin/stock-wait?website_id={}", rig.site),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["items"].as_array().unwrap().first().cloned().unwrap()
}

async fn send(rig: &WaitRig, item: Uuid) -> (axum::http::StatusCode, serde_json::Value) {
    post(
        &rig.admin,
        &format!("/admin/stock-wait/{item}/send"),
        None,
        &format!("{{\"website_id\": \"{}\"}}", rig.site),
    )
    .await
}

#[tokio::test]
async fn the_wait_discharges_only_on_an_accepted_send() {
    let rig = rig("stockwait").await;
    let pool = &rig.pool;

    // A sold-out item — the port ANSWERS zero (programmed), the honest
    // demand read needs a real number, not a refusal.
    let item = seed_listing(pool, &rig.catalog, rig.site, "Sold-Out Lamp", Decimal::new(5000, 2), true).await;
    rig.availability.stock(item, Decimal::ZERO);

    // A logged-in shopper wishes the item and arms the wait: the arm
    // stamps the VERIFIED principal address (the only writer besides
    // reconcile).
    let (_visitor, token) = seed_visitor(pool, rig.site).await;
    let (status, _) = post(&rig.public, "/public/wishlist", Some(&token), &format!("{{\"item_id\": \"{item}\"}}")).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (status, json) = post_dual(&rig.public, &format!("/public/wishlist/{item}/notify"), Some(&token), Some(BEARER), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["notify_on_stock"], true);
    let stamped: Option<String> = sqlx::query_scalar(
        "SELECT contact_email FROM storefront.wishlist_items \
         WHERE website_id = $1 AND item_id = $2 AND (metadata->>'deleted_at') IS NULL",
    )
    .bind(rig.site)
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(stamped.as_deref(), Some(EMAIL), "the arm carries the verified principal address");

    // The officer demand read: armed, addressable, and INELIGIBLE (the
    // fresh availability read says zero — nothing persisted anywhere).
    let row = wait_row(&rig).await;
    assert_eq!(row["armed"], 1);
    assert_eq!(row["with_address"], 1);
    assert_eq!(dec(&row["free_quantity"]), Decimal::ZERO);
    assert_eq!(row["eligible"], false);

    // The send REFUSES while the item is still out of stock — an alert
    // for a still-out item would be a lie.
    let (status, json) = send(&rig, item).await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["code"], "storefront_guarded_refusal");
    assert!(!rig.stock_notifier.told(item, EMAIL), "nothing was delivered");

    // RESTOCK: the send discharges — accepted by the notifier, the arm
    // clears, the audit carries the send.
    rig.availability.stock(item, Decimal::new(5, 0));
    let (status, json) = send(&rig, item).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["attempted"], 1);
    assert_eq!(json["sent"], 1);
    assert_eq!(json["failed"], 0);
    assert_eq!(json["delivery_state"], "sent");
    assert!(rig.stock_notifier.told(item, EMAIL), "the verified address was told");
    let (status, bytes) = get(&rig.admin, &format!("/admin/stock-wait?website_id={}", rig.site), None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let cleared: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        cleared["items"].as_array().unwrap().is_empty(),
        "the accepted send cleared the arm (no armed demand left)"
    );

    // A FAILED transport never burns the wait: re-arm, fail, send — the
    // attempt is recorded failed and the arm STAYS.
    let (status, _) = post_dual(&rig.public, &format!("/public/wishlist/{item}/notify"), Some(&token), Some(BEARER), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    rig.stock_notifier.fail_transport();
    let (status, json) = send(&rig, item).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["sent"], 0);
    assert_eq!(json["failed"], 1);
    assert_eq!(json["delivery_state"], "none");
    let row = wait_row(&rig).await;
    assert_eq!(row["armed"], 1, "the transport failure left the arm set");

    // An address-less arm (an anonymous shopper who never reconciled)
    // is counted, skipped by the send, and stays armed for the next
    // officer pass after a login.
    let (_anon, anon_token) = seed_visitor(pool, rig.site).await;
    let (status, _) = post(&rig.public, "/public/wishlist", Some(&anon_token), &format!("{{\"item_id\": \"{item}\"}}")).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (status, _) = post(&rig.public, &format!("/public/wishlist/{item}/notify"), Some(&anon_token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let row = wait_row(&rig).await;
    assert_eq!(row["armed"], 2);
    assert_eq!(row["with_address"], 1);

    // Arming never creates a row: an item not on the caller's wishlist
    // answers the typed 404.
    let stranger = seed_listing(pool, &rig.catalog, rig.site, "Never Wished", Decimal::new(100, 2), true).await;
    let (status, json) = post(&rig.public, &format!("/public/wishlist/{stranger}/notify"), Some(&anon_token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "storefront_wishlist_item_not_found");

    rig._db.dispose().await;
}
