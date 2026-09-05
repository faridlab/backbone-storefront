//! The companions' stock gate (spec §14.1): the availability port is
//! the ONLY stock oracle, read FRESH at line-mutation time and again at
//! place time — no snapshot is ever stored. Fail-closed (an item the
//! port cannot answer for refuses with the typed 503, never a guessed
//! number), clamped per mutation, re-checked for the WHOLE basket at
//! place under the row lock, with the per-listing backorder escape
//! hatch for made-to-order listings.

use rust_decimal::Decimal;

use super::common::{get, post, seed_carrier, seed_listing, seed_provider, seed_visitor, Probe};

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

#[tokio::test]
async fn the_port_is_the_only_oracle_and_it_fails_closed() {
    let probe = Probe::boot("availfail").await;
    let pool = probe.pool.clone();
    let site = probe.view.id;

    let item =
        seed_listing(&pool, &probe.catalog, site, "Gated Widget", Decimal::new(10000, 2), true)
            .await;
    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // FAIL-CLOSED: the port was never programmed for this item — the
    // add refuses with the typed 503. The module never invents a stock
    // number and never silently allows the line.
    let body = format!("{{\"item_id\": \"{item}\", \"quantity\": 1}}");
    let (status, json) = post(&probe.public, "/public/cart/lines", Some(&token), &body).await;
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["code"], "storefront_availability_port_refused");

    // The DISPLAY read refuses with the same shape — no number anywhere.
    let (status, bytes) =
        get(&probe.public, &format!("/public/availability/{item}"), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["code"], "storefront_availability_port_refused");

    // PROGRAMMED: stock 2. The clamp refuses qty 3 (typed 422) and
    // accepts qty 2.
    probe.availability.stock(item, Decimal::new(2, 0));
    let over = format!("{{\"item_id\": \"{item}\", \"quantity\": 3}}");
    let (status, json) = post(&probe.public, "/public/cart/lines", Some(&token), &over).await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["code"], "storefront_stock_insufficient");
    let fits = format!("{{\"item_id\": \"{item}\", \"quantity\": 2}}");
    let (status, _) = post(&probe.public, "/public/cart/lines", Some(&token), &fits).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);

    // The DISPLAY read answers under the same port, FRESH: the number
    // is the one the adapter holds right now, not a stored snapshot.
    let (status, bytes) =
        get(&probe.public, &format!("/public/availability/{item}"), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(dec(&json["free_quantity"]), Decimal::new(2, 0));
    assert_eq!(json["kit_exploded"], false, "a plain item carries no kit bridge flag");

    // A closed-door item's stock is not a public fact: unpublished
    // listings answer the availability read with the 404, never a
    // number — even with the port programmed.
    let hidden = seed_listing(
        &pool,
        &probe.catalog,
        site,
        "Unlisted Widget",
        Decimal::new(10000, 2),
        false,
    )
    .await;
    probe.availability.stock(hidden, Decimal::new(9, 0));
    let (status, _) =
        get(&probe.public, &format!("/public/availability/{hidden}"), Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);

    // BACKORDER: the officer flag lifts the clamp — a made-to-order
    // listing stays orderable past free quantity.
    let (status, json) = post(
        &probe.admin,
        "/admin/listings/backorder",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"item_id\": \"{item}\", \"allow_backorder\": true}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["allow_backorder"], true);
    let (_made_visitor, made_token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&made_token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let deep = format!("{{\"item_id\": \"{item}\", \"quantity\": 99}}");
    let (status, _) = post(&probe.public, "/public/cart/lines", Some(&made_token), &deep).await;
    assert_eq!(
        status,
        axum::http::StatusCode::CREATED,
        "allow_backorder skips the mutation-time clamp"
    );
    probe.dispose().await;
}

#[tokio::test]
async fn the_place_gate_rechecks_every_line_under_the_lock() {
    let probe = Probe::boot("availplace").await;
    let pool = probe.pool.clone();
    let site = probe.view.id;
    let company = probe.company_id;

    // Two items, both comfortably in stock at add time.
    let item_a = seed_listing(
        &pool,
        &probe.catalog,
        site,
        "Place A",
        Decimal::new(10000, 2),
        true,
    )
    .await;
    let item_b = seed_listing(
        &pool,
        &probe.catalog,
        site,
        "Place B",
        Decimal::new(6000, 2),
        true,
    )
    .await;
    probe.availability.stock(item_a, Decimal::new(10, 0));
    probe.availability.stock(item_b, Decimal::new(2, 0));

    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    for (item, qty) in [(item_a, 1), (item_b, 2)] {
        let body = format!("{{\"item_id\": \"{item}\", \"quantity\": {qty}}}");
        let (status, _) = post(&probe.public, "/public/cart/lines", Some(&token), &body).await;
        assert_eq!(status, axum::http::StatusCode::CREATED);
    }
    let (status, _) = post(
        &probe.public,
        "/public/cart/billing",
        Some(&token),
        "{\"email\": \"placer@shop.test\"}",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let carrier = seed_carrier(&pool, company, "Probe Express").await;
    let (status, _) = post(
        &probe.public,
        "/public/cart/delivery",
        Some(&token),
        &format!("{{\"carrier_id\": \"{carrier}\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    seed_provider(&pool, company).await;

    // Stock DROPS between the add and the place (the real-world race:
    // another shopper bought the last units). The add passed; the
    // place-time gate re-reads EVERY line under the lock and refuses —
    // the mutation-time clamp alone is never the promise.
    probe.availability.stock(item_b, Decimal::ZERO);
    let (status, json) = post(&probe.public, "/public/checkout", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["code"], "storefront_stock_insufficient");

    // Nothing was minted by the refused place.
    let orders: i64 = sqlx::query_scalar("SELECT count(*) FROM selling.sales_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(orders, 0, "a refused place mints nothing");

    // Restock: the SAME cart now places cleanly (the gate is a fresh
    // read, not a sticky refusal).
    probe.availability.stock(item_b, Decimal::new(2, 0));
    let (status, json) = post(&probe.public, "/public/checkout", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    assert_eq!(json["state"], "pending_payment", "a paid online cart arms the gateway lane");
    probe.dispose().await;
}
