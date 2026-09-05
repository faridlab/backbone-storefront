//! Gates 5 + 6 (§11.3): no-mint reads and the mutating-GET harness.
//! Every GET route is fired with an adversarial parameter family while
//! full-table checksums run before and after — zero writes anywhere,
//! `website.visitors` byte-stable (the storefront never mints
//! identity). The POST-only discipline is proven by method: a GET on
//! every mutating path answers 405.

use rust_decimal::Decimal;
use uuid::Uuid;

use super::common::{
    get, seed_listing, seed_pickup_location, seed_visitor, table_checksum, Probe,
    CHECKSUMMED_TABLES,
};

async fn checksums(pool: &sqlx::PgPool) -> Vec<(i64, String)> {
    let mut out = Vec::new();
    for table in CHECKSUMMED_TABLES {
        out.push(table_checksum(pool, table).await);
    }
    out
}

#[tokio::test]
async fn every_get_route_leaves_every_table_untouched() {
    let probe = Probe::boot("gets").await;
    let pool = probe.pool.clone();
    let site = probe.view.id;

    // Live state for the GETs to read: a published item, a visitor
    // session, an open cart with one line.
    let item = seed_listing(&pool, &probe.catalog, site, "Widget", Decimal::new(10000, 2), true).await;
    probe.availability.stock(item, Decimal::new(10000, 0));
    seed_pickup_location(&pool, site, "Probe Store Flagship", None, "ID", true).await;
    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = super::common::post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let body = format!("{{\"item_id\": \"{item}\", \"quantity\": 1}}");
    let (status, _) =
        super::common::post(&probe.public, "/public/cart/lines", Some(&token), &body).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);

    let before = checksums(&pool).await;

    // The adversarial GET family across every §6.1 GET route. Allowed
    // answers: 200 (answered), 400/401/404/422 (typed refusals) —
    // never 405 (that is the POST-only discipline below) and never
    // 500. The WRITE proof is the checksum pair.
    let stranger = Uuid::new_v4();
    let get_family: Vec<(String, bool)> = vec![
        ("/public/catalog?q=%27%29%3B--&sort=newest&page=1&page_size=20".into(), false),
        ("/public/catalog?sort=price_desc".into(), false),
        ("/public/catalog?q=widget&sort=name_asc&page=2&page_size=5".into(), false),
        ("/public/catalog?sort=relevance&page=-9&page_size=99999".into(), false),
        ("/public/catalog?sort=%3BDROP%20TABLE%20carts%3B--".into(), false),
        ("/public/categories".into(), false),
        (format!("/public/catalog/{item}"), false),
        (format!("/public/catalog/{stranger}"), false),
        ("/public/cart".into(), true),
        (format!("/public/checkout/{stranger}"), true),
        ("/public/abandoned".into(), true),
        // The companions' GET family (stateless reads, the same
        // no-write proof): availability, comparison, the pickup store
        // lookup, and the wishlist read.
        (format!("/public/availability/{item}"), true),
        (format!("/public/availability/{stranger}"), true),
        (format!("/public/compare?item_id={item}&item_id={stranger}"), true),
        ("/public/compare".into(), true),
        ("/public/collect/locations".into(), true),
        ("/public/wishlist".into(), true),
    ];
    for (path, with_token) in &get_family {
        let (status, body) = get(
            &probe.public,
            path,
            with_token.then_some(token.as_str()),
        )
        .await;
        assert_ne!(
            status,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "GET {path} blew up: {}",
            String::from_utf8_lossy(&body)
        );
        assert_ne!(
            status,
            axum::http::StatusCode::METHOD_NOT_ALLOWED,
            "GET {path} method-refused"
        );
        assert!(
            [200, 400, 401, 404, 422].contains(&status.as_u16()),
            "GET {path} answered {status}"
        );
    }

    // The no-mint arm: however the reads answered, the visitors table
    // (and every other table) is byte-identical.
    let after = checksums(&pool).await;
    for (table, (b, a)) in CHECKSUMMED_TABLES.iter().zip(before.iter().zip(after.iter())) {
        assert_eq!(b, a, "GET family wrote to {table}");
    }
    let visitors = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM website.visitors WHERE website_id = $1",
    )
    .bind(site)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(visitors, 1, "reads never mint visitor rows");
    probe.dispose().await;
}

#[tokio::test]
async fn mutating_paths_answer_405_to_get() {
    let probe = Probe::boot("m405").await;
    let site = probe.view.id;
    let (_visitor, token) = seed_visitor(&probe.pool, site).await;
    let any = Uuid::new_v4();

    let mutating_paths = [
        "/public/cart/lines",
        &format!("/public/cart/lines/{any}"),
        &format!("/public/cart/lines/{any}/remove"),
        "/public/cart/coupon",
        "/public/cart/coupon/remove",
        "/public/cart/billing",
        "/public/cart/delivery",
        "/public/session/bind",
        "/public/cart/adopt",
        "/public/checkout",
        "/public/express",
        &format!("/public/cart/{any}/recover"),
        "/public/cart/pickup",
        "/public/cart/pickup/reset",
        "/public/checkout/on-site",
        "/public/wishlist/reconcile",
        &format!("/public/wishlist/{any}/remove"),
        &format!("/public/wishlist/{any}/notify"),
    ];
    for path in mutating_paths {
        let (status, _) = get(&probe.public, path, Some(&token)).await;
        assert_eq!(
            status,
            axum::http::StatusCode::METHOD_NOT_ALLOWED,
            "GET {path} must be method-refused"
        );
    }
    // /public/cart carries a legitimate GET (create is POST) — it must
    // NOT method-refuse.
    let (status, _) = get(&probe.public, "/public/cart", Some(&token)).await;
    assert_ne!(status, axum::http::StatusCode::METHOD_NOT_ALLOWED);
    probe.dispose().await;
}
