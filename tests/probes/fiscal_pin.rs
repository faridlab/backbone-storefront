//! The fiscal pin (spec §14.2): a pickup cart's place resolves its tax
//! jurisdiction through the STORE'S country, while a delivery cart
//! keeps the home arm (no jurisdiction). The recording tax stub keys
//! DIFFERENT rates to the two arms and refuses any jurisdiction it was
//! not armed for, so an inverted pin — a pickup order resolving under
//! the home arm — cannot produce a green run: the recorded
//! jurisdiction AND the minted order's tax rate both have to name the
//! arm that produced them. A pinned store with NO country refuses the
//! place with the typed fiscal guard: the home arm is unreachable for
//! pickup orders at the code level too, not only through the NOT NULL
//! column.

use std::sync::Arc;

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_storefront::presentation::http::{
    storefront_public_routes, StorefrontPublicState,
};

use super::common::{
    post, seed_listing, seed_pickup_location, seed_provider, seed_visitor, seed_website,
    RecordingTax, StubAvailability, StubCatalog, StubParty, StubPricing, StubSurface, TestDb,
};

/// The store's country, deliberately NOT the company's home
/// jurisdiction: the home arm is what the stub answers for `None`, and
/// the two arms carry different rates (8% vs 11%) so a wrong-arm
/// resolution fails on the order's rate, not just on the recording.
const STORE_COUNTRY: &str = "SG";
fn store_rate() -> Decimal {
    Decimal::new(8, 2) // 0.08
}
fn home_rate() -> Decimal {
    Decimal::new(11, 2) // 0.11
}

struct FiscalRig {
    public: axum::Router,
    pool: sqlx::PgPool,
    site: backbone_website::exports::WebsiteView,
    catalog: Arc<StubCatalog>,
    availability: Arc<StubAvailability>,
    tax: Arc<RecordingTax>,
    _db: TestDb,
}

async fn rig(marker: &str) -> FiscalRig {
    let db = TestDb::new(marker).await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let site = seed_website(&pool, "Fiscal Store", company).await;
    let catalog = Arc::new(StubCatalog::default());
    let party = Arc::new(StubParty::new());
    let tax = Arc::new(RecordingTax::new(
        home_rate(),
        vec![(STORE_COUNTRY.to_string(), store_rate())],
    ));
    let pricing = Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let availability = Arc::new(StubAvailability::new());
    let public = storefront_public_routes(StorefrontPublicState::compose(
        pool.clone(),
        Arc::new(StubSurface::binding(site.clone())),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    ));
    // The delivery place runs the paid (gateway) arm; it needs the
    // company's active provider row.
    seed_provider(&pool, company).await;
    FiscalRig {
        public,
        pool,
        site,
        catalog,
        availability,
        tax,
        _db: db,
    }
}

impl FiscalRig {
    async fn dispose(self) {
        self._db.dispose().await;
    }
}

/// One shopper with a billed one-line cart; returns (visitor token,
/// item id). The item is stocked generously so the place-time stock
/// gate never intercepts the fiscal arm under test.
async fn billed_shopper(rig: &FiscalRig, name: &str, price: Decimal) -> (String, Uuid) {
    let item = seed_listing(&rig.pool, &rig.catalog, rig.site.id, name, price, true).await;
    rig.availability.stock(item, Decimal::new(100, 0));
    let (_visitor, token) = seed_visitor(&rig.pool, rig.site.id).await;
    let (status, _) = post(&rig.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (status, _) = post(
        &rig.public,
        "/public/cart/lines",
        Some(&token),
        &format!("{{\"item_id\": \"{item}\", \"quantity\": 1}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (status, _) = post(
        &rig.public,
        "/public/cart/billing",
        Some(&token),
        "{\"email\": \"fiscal@shop.test\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    (token, item)
}

async fn order_tax_rate(pool: &sqlx::PgPool, order_id: Uuid) -> Decimal {
    let (rate,): (Decimal,) =
        sqlx::query_as("SELECT tax_rate FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(pool)
            .await
            .unwrap();
    rate
}

#[tokio::test]
async fn the_pickup_place_pins_tax_to_the_store_country_and_delivery_to_home() {
    let rig = rig("fiscalpin").await;

    // ── the PICKUP order: pinned store, on-site lane ────────────────
    let (token, _item) =
        billed_shopper(&rig, "Border Widget", Decimal::new(10000, 2)).await;
    let store =
        seed_pickup_location(&rig.pool, rig.site.id, "Border Store", None, STORE_COUNTRY, true)
            .await;
    let (status, _) = post(
        &rig.public,
        "/public/cart/pickup",
        Some(&token),
        &format!("{{\"location_id\": \"{store}\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (status, json) = post(&rig.public, "/public/checkout/on-site", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(json["state"], "pending_pickup");
    let pickup_order: Uuid = json["sales_order_id"].as_str().unwrap().parse().unwrap();
    // The pickup place resolved under the STORE'S country — and never
    // the home arm.
    assert!(
        rig.tax.saw(Some(STORE_COUNTRY)),
        "the pickup place must resolve tax under the store's country"
    );
    assert!(
        !rig.tax.saw(None),
        "the pickup place must never touch the home arm"
    );
    assert_eq!(
        order_tax_rate(&rig.pool, pickup_order).await,
        store_rate(),
        "the pickup order carries the store-country rate"
    );

    // ── the DELIVERY order: no pin, the plain online lane ───────────
    let (token, _item) =
        billed_shopper(&rig, "Home Widget", Decimal::new(10000, 2)).await;
    let (status, json) = post(&rig.public, "/public/checkout", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let delivery_order: Uuid = json["sales_order_id"].as_str().unwrap().parse().unwrap();
    assert!(
        rig.tax.saw(None),
        "the delivery place resolves under the home arm (no jurisdiction)"
    );
    assert_eq!(rig.tax.call_count(), 2, "exactly one resolution per order");
    assert_eq!(
        order_tax_rate(&rig.pool, delivery_order).await,
        home_rate(),
        "the delivery order carries the home rate"
    );
    assert_ne!(
        order_tax_rate(&rig.pool, pickup_order).await,
        order_tax_rate(&rig.pool, delivery_order).await,
        "the two orders carry different tax arms — the pin is observable on the orders themselves"
    );
    rig.dispose().await;
}

#[tokio::test]
async fn a_countryless_pinned_store_refuses_the_place_with_the_typed_fiscal_guard() {
    let rig = rig("fiscalnull").await;

    // The column is NOT NULL and the upsert requires the country, so a
    // countryless row cannot be born through the schema. The probe
    // relaxes the constraint ON ITS OWN DISPOSABLE DATABASE to prove
    // the CODE-level guard stands independently of the column: a
    // legacy or bypass-written row still refuses at place, loudly.
    sqlx::query("ALTER TABLE storefront.pickup_locations ALTER COLUMN country DROP NOT NULL")
        .execute(&rig.pool)
        .await
        .unwrap();
    let (store,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO storefront.pickup_locations (website_id, name, is_active)
        VALUES ($1, 'Countryless Store', true)
        RETURNING id
        "#,
    )
    .bind(rig.site.id)
    .fetch_one(&rig.pool)
    .await
    .unwrap();

    let (token, _item) =
        billed_shopper(&rig, "Orphan Widget", Decimal::new(10000, 2)).await;
    let (status, _) = post(
        &rig.public,
        "/public/cart/pickup",
        Some(&token),
        &format!("{{\"location_id\": \"{store}\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "the pin itself accepts the row");

    // The place refuses with the typed fiscal guard — never a silent
    // fallback to the home jurisdiction.
    let (status, json) = post(&rig.public, "/public/checkout/on-site", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["code"], "storefront_pickup_country_missing");

    // Nothing was minted and the tax port was never reached: the home
    // arm is unreachable for pickup orders even in this bypass state.
    let orders: i64 = sqlx::query_scalar("SELECT count(*) FROM selling.sales_orders")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(orders, 0, "the refused place mints no order");
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM storefront.checkout_sessions")
        .fetch_one(&rig.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "the refused place writes no checkout session");
    assert_eq!(
        rig.tax.call_count(),
        0,
        "resolve_rate is never called for a countryless pickup place — the home arm is unreachable"
    );
    rig.dispose().await;
}
