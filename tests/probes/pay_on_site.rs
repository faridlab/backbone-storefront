//! The pay-on-site lane (spec §14.2): the THIRD checkout arm. The
//! on-site place mints the order DRAFT with NO gateway row and NOTHING
//! auto-confirms — not even a zero-total cart (the lane check runs
//! before the free arm; a free on-site pickup is still a store
//! collection to confirm by hand). ONLY the officer confirm-pickup
//! verb settles, exactly once.

use rust_decimal::Decimal;
use uuid::Uuid;

use super::common::{post, seed_listing, seed_visitor, Probe};

/// One shopper's cart pinned to a store, ready for the on-site place.
/// Returns (token, cart's website, pickup location id).
async fn pickup_cart(
    probe: &Probe,
    name: &str,
    price: Decimal,
) -> (String, Uuid) {
    let pool = probe.pool.clone();
    let site = probe.view.id;
    let item = seed_listing(&pool, &probe.catalog, site, name, price, true).await;
    probe.availability.stock(item, Decimal::new(100, 0));
    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let body = format!("{{\"item_id\": \"{item}\", \"quantity\": 1}}");
    let (status, _) = post(&probe.public, "/public/cart/lines", Some(&token), &body).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (status, _) = post(
        &probe.public,
        "/public/cart/billing",
        Some(&token),
        "{\"email\": \"collector@shop.test\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Pay Here Store\", \
              \"city\": \"Jakarta\", \"country\": \"ID\"}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let location: Uuid = json["location_id"].as_str().unwrap().parse().unwrap();
    let (status, _) = post(
        &probe.public,
        "/public/cart/pickup",
        Some(&token),
        &format!("{{\"location_id\": \"{location}\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    (token, location)
}

#[tokio::test]
async fn on_site_never_auto_confirms_and_the_officer_settles_exactly_once() {
    let probe = Probe::boot("ponsite").await;
    let pool = probe.pool.clone();
    let (token, _location) = pickup_cart(&probe, "Collect Paid", Decimal::new(12000, 2)).await;

    let (status, json) = post(
        &probe.public,
        "/public/checkout/on-site",
        Some(&token),
        "{\"notes\": \"will pay at the counter\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(json["state"], "pending_pickup");
    assert!(
        json["gateway_transaction_id"].is_null(),
        "the on-site lane mints NO gateway row"
    );
    assert!(json["provider_code"].is_null());
    let checkout_id: Uuid = json["checkout_id"].as_str().unwrap().parse().unwrap();
    let order_id: Uuid = json["sales_order_id"].as_str().unwrap().parse().unwrap();

    let (status_,): (String,) =
        sqlx::query_as("SELECT status::text FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status_, "draft", "an unpaid on-site order stays DRAFT");
    let gateway_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_gateway.gateway_transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(gateway_rows, 0, "no gateway row exists anywhere for the on-site lane");

    // ONLY the officer confirm settles — and exactly once.
    let (status, json) = post(
        &probe.admin,
        &format!("/admin/checkouts/{checkout_id}/confirm-pickup"),
        None,
        "{\"payment_reference\": \"cash-drawer-7\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["state"], "settled");
    assert!(json["settled_at"].is_string(), "the settle stamp landed");
    let (confirmed,): (String,) =
        sqlx::query_as("SELECT status::text FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(confirmed, "to_deliver_and_bill", "the confirm left draft like every lane");

    // The redrive: a second confirm is the typed state refusal, and it
    // stamps nothing new.
    let (status, json) = post(
        &probe.admin,
        &format!("/admin/checkouts/{checkout_id}/confirm-pickup"),
        None,
        "{\"payment_reference\": \"double-tap\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert_eq!(json["code"], "storefront_checkout_state_refused");
    let pickups: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM storefront.storefront_audit_log \
         WHERE event = 'pickup_confirmed' AND subject_id = $1",
    )
    .bind(checkout_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pickups, 1, "the confirm lands exactly once");
    probe.dispose().await;
}

#[tokio::test]
async fn a_delivery_cart_cannot_take_the_on_site_lane() {
    let probe = Probe::boot("ponsitelane").await;
    let pool = probe.pool.clone();
    let site = probe.view.id;

    // A delivery cart: item, billing, carrier — but NO pickup pin.
    let item = seed_listing(
        &pool,
        &probe.catalog,
        site,
        "Shipped Thing",
        Decimal::new(12000, 2),
        true,
    )
    .await;
    probe.availability.stock(item, Decimal::new(100, 0));
    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let body = format!("{{\"item_id\": \"{item}\", \"quantity\": 1}}");
    let (status, _) = post(&probe.public, "/public/cart/lines", Some(&token), &body).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (status, _) = post(
        &probe.public,
        "/public/cart/billing",
        Some(&token),
        "{\"email\": \"shipme@shop.test\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, json) = post(&probe.public, "/public/checkout/on-site", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json["code"], "storefront_pickup_mode_required",
        "a shipping cart cannot promise payment at a store"
    );
    let orders: i64 = sqlx::query_scalar("SELECT count(*) FROM selling.sales_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(orders, 0, "the refused lane mints nothing");
    probe.dispose().await;
}

#[tokio::test]
async fn a_zero_total_on_site_cart_is_still_pending_pickup() {
    let probe = Probe::boot("ponsitefree").await;
    let pool = probe.pool.clone();
    let (token, _location) = pickup_cart(&probe, "Free Sample", Decimal::ZERO).await;

    // The lane check runs BEFORE the free arm: a zero-total on-site
    // cart must NOT grow a confirm-on-place shortcut — the store still
    // has to hand it over, and the officer still confirms that.
    let (status, json) = post(&probe.public, "/public/checkout/on-site", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(
        json["state"], "pending_pickup",
        "the on-site lane never auto-confirms, zero total included"
    );
    let order_id: Uuid = json["sales_order_id"].as_str().unwrap().parse().unwrap();
    let (status_,): (String,) =
        sqlx::query_as("SELECT status::text FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status_, "draft");
    let gateway_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM payment_gateway.gateway_transactions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(gateway_rows, 0);
    probe.dispose().await;
}
