//! Click & Collect (spec §14.2): the store lookup is a PURE read — no
//! carrier switch, no cart mint (the whole-table checksum proof) — the
//! pin verb is the only writer of a cart's pickup linkage, the
//! warehouse id never leaves the server, and the server-side resolution
//! is a closed door (404) for missing, inactive, or foreign-website
//! stores.

use rust_decimal::Decimal;
use uuid::Uuid;

use super::common::{
    get, post, seed_carrier, seed_listing, seed_pickup_location, seed_warehouse, seed_website,
    seed_visitor, table_checksum, Probe, TestDb, CHECKSUMMED_TABLES,
};

/// Every checksummed table, in order (the mutating-GET proof's shape).
async fn checksums(pool: &sqlx::PgPool) -> Vec<(i64, String)> {
    let mut out = Vec::new();
    for table in CHECKSUMMED_TABLES {
        out.push(table_checksum(pool, table).await);
    }
    out
}

#[tokio::test]
async fn the_lookup_is_pure_and_the_pin_resolves_server_side() {
    let probe = Probe::over(TestDb::new_with_inventory("collect").await).await;
    let pool = probe.pool.clone();
    let site = probe.view.id;
    let company = probe.company_id;

    // Live shopper state: an item, a visitor, an open cart with one
    // line, and a DELIVERY carrier already chosen (the fact the pure
    // lookup must not disturb).
    let item = seed_listing(
        &pool,
        &probe.catalog,
        site,
        "Collect Widget",
        Decimal::new(10000, 2),
        true,
    )
    .await;
    probe.availability.stock(item, Decimal::new(10000, 0));
    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let body = format!("{{\"item_id\": \"{item}\", \"quantity\": 1}}");
    let (status, _) = post(&probe.public, "/public/cart/lines", Some(&token), &body).await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let carrier = seed_carrier(&pool, company, "Probe Express").await;
    let (status, json) = post(
        &probe.public,
        "/public/cart/delivery",
        Some(&token),
        &format!("{{\"carrier_id\": \"{carrier}\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["delivery_carrier_id"].as_str(), Some(carrier.to_string().as_str()));

    // The registry exists ONLY through the officer upsert verb: two
    // stores, one active and one deactivated (the lifecycle shape).
    // The warehouse is a REAL row of this company's inventory — the
    // upsert validates the pointer against the owning company.
    let warehouse = seed_warehouse(&pool, company, "COLLECT-01").await;
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Flagship Store\", \
              \"warehouse_id\": \"{warehouse}\", \"address_line1\": \"1 Jalan Probe\", \
              \"city\": \"Jakarta\", \"country\": \"ID\"}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let flagship: Uuid = json["location_id"].as_str().unwrap().parse().unwrap();
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Closed Kiosk\", \
              \"city\": \"Bandung\", \"country\": \"ID\", \"is_active\": false}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let kiosk: Uuid = json["location_id"].as_str().unwrap().parse().unwrap();

    // The OFFICER read sees both stores and their warehouse pointers.
    let (status, bytes) = get(
        &probe.admin,
        &format!("/admin/collect/locations?website_id={site}"),
        None,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let officer_rows = json["locations"].as_array().unwrap();
    assert_eq!(officer_rows.len(), 2, "the registry read carries all states");
    assert!(
        officer_rows
            .iter()
            .any(|l| l["location_id"].as_str() == Some(flagship.to_string().as_str())
                && l["warehouse_id"].as_str() == Some(warehouse.to_string().as_str())),
        "the officer sees the warehouse pointer"
    );

    // ── the PURE lookup, checksummed ─────────────────────────────────
    let before = checksums(&pool).await;
    let (status, bytes) = get(&probe.public, "/public/collect/locations", Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = json["locations"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "the public lookup lists ACTIVE stores only");
    assert_eq!(rows[0]["location_id"].as_str(), Some(flagship.to_string().as_str()));
    assert_eq!(rows[0]["name"].as_str(), Some("Flagship Store"));
    assert!(
        rows[0].get("warehouse_id").is_none(),
        "the warehouse id NEVER leaves the server — the shopper sees a store, not a warehouse"
    );
    let after = checksums(&pool).await;
    for (table, (b, a)) in CHECKSUMMED_TABLES.iter().zip(before.iter().zip(after.iter())) {
        assert_eq!(b, a, "the store lookup wrote to {table}");
    }

    // ── the PIN: the client presents ONLY the opaque location id ────
    let (status, json) = post(
        &probe.public,
        "/public/cart/pickup",
        Some(&token),
        &format!("{{\"location_id\": \"{flagship}\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["fulfillment_mode"], "pickup");
    assert_eq!(json["pickup_location_id"].as_str(), Some(flagship.to_string().as_str()));
    assert_eq!(json["location_name"].as_str(), Some("Flagship Store"));
    assert_eq!(
        json["delivery_carrier_id"].as_str(),
        Some(carrier.to_string().as_str()),
        "the pin switches NO carrier — the mode carries the fulfillment, not the carrier"
    );

    // Closed doors: the deactivated store, another website's store, and
    // an unknown id are all the same typed 404 (the server-side
    // resolution is website- and liveness-scoped).
    let other = seed_website(&pool, "Other Chain", company).await;
    let foreign =
        seed_pickup_location(&pool, other.id, "Foreign Store", None, "ID", true).await;
    for location_id in [kiosk, foreign, Uuid::new_v4()] {
        let (status, json) = post(
            &probe.public,
            "/public/cart/pickup",
            Some(&token),
            &format!("{{\"location_id\": \"{location_id}\"}}"),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
        assert_eq!(json["code"], "storefront_pickup_location_not_found");
    }

    // The cart survived the refused pins (still pinned to the flagship).
    let (mode, pinned, carrier_now): (String, Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT fulfillment_mode, pickup_location_id, delivery_carrier_id \
         FROM storefront.carts WHERE website_id = $1",
    )
    .bind(site)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(mode, "pickup");
    assert_eq!(pinned, Some(flagship));
    assert_eq!(carrier_now, Some(carrier));

    // The RESET returns the cart to delivery; the carrier stays
    // whatever the delivery verb last set.
    let (status, json) = post(&probe.public, "/public/cart/pickup/reset", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["fulfillment_mode"], "delivery");
    assert!(json["pickup_location_id"].is_null());
    assert_eq!(json["delivery_carrier_id"].as_str(), Some(carrier.to_string().as_str()));
    probe.dispose().await;
}

/// The registry upsert's fences (the officer-write grain): a warehouse
/// pointer must name one of the TARGET WEBSITE's company's live
/// warehouses (a foreign company's — or a missing — id is the typed
/// refusal), the target website itself must be a live row (an unknown
/// id is the typed 404, never a registry row against a dangling
/// website), and the write against another company's website with that
/// company's OWN warehouse is the recorded accepted grain — the module
/// scopes the write to the website's company; the officer→company
/// binding is the host's auth fence around the admin tree (spec §14.2).
/// The fiscal country is required merchant-declared input here too: no
/// create without it, no code outside the 2-letter ISO shape.
#[tokio::test]
async fn the_upsert_fences_its_warehouse_and_website_grain() {
    let probe = Probe::over(TestDb::new_with_inventory("collectfence").await).await;
    let pool = probe.pool.clone();
    let site = probe.view.id;
    let company = probe.company_id;

    // The company's own warehouse is the only pointer this website's
    // stores may fulfill from.
    let own = seed_warehouse(&pool, company, "OWN-01").await;
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Valid Store\", \
              \"warehouse_id\": \"{own}\", \"country\": \"ID\"}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let valid: Uuid = json["location_id"].as_str().unwrap().parse().unwrap();

    // A FOREIGN company's warehouse: the typed refusal (a store that
    // fulfilled from it would promise stock it can never read).
    let other = seed_website(&pool, "Other Company", Uuid::new_v4()).await;
    let foreign_warehouse = seed_warehouse(&pool, other.company_id, "FOREIGN-01").await;
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Smuggled Store\", \
              \"warehouse_id\": \"{foreign_warehouse}\", \"country\": \"ID\"}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["code"], "storefront_pickup_warehouse_refused");

    // A warehouse id that names nothing is the SAME uniform refusal.
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Ghost Store\", \
              \"warehouse_id\": \"{}\", \"country\": \"ID\"}}",
            Uuid::new_v4()
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["code"], "storefront_pickup_warehouse_refused");

    // The target website must be a live row — an unknown id is the
    // typed 404, never a registry row against a dangling website.
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{}\", \"name\": \"Nowhere Store\", \"country\": \"ID\"}}",
            Uuid::new_v4()
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert_eq!(json["code"], "storefront_website_not_found");

    // The recorded officer-write grain: the write scopes to the TARGET
    // WEBSITE's company. Another company's website + that company's own
    // warehouse is coherent and accepted — the module carries no
    // officer-company identity of its own; which officers may reach the
    // admin tree at all is the host's auth fence (spec §14.2 records
    // the decision).
    let (status, _) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{}\", \"name\": \"Their Own Store\", \
              \"warehouse_id\": \"{foreign_warehouse}\", \"country\": \"SG\"}}",
            other.id
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // The fiscal country is required merchant-declared input: a create
    // without one refuses, and the 2-letter ISO shape is the only
    // accepted form.
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!("{{\"website_id\": \"{site}\", \"name\": \"No Country Store\"}}"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], "storefront_invalid_input");
    let (status, json) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Bad Code Store\", \"country\": \"IDN\"}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert_eq!(json["code"], "storefront_invalid_input");

    // A coherent patch may move the country to another valid code.
    let (status, _) = post(
        &probe.admin,
        "/admin/collect/locations",
        None,
        &format!(
            "{{\"website_id\": \"{site}\", \"name\": \"Valid Store\", \"country\": \"SG\"}}"
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (country, warehouse): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT country, warehouse_id FROM storefront.pickup_locations WHERE id = $1",
    )
    .bind(valid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(country, "SG");
    assert_eq!(warehouse, Some(own));

    // Only the two coherent rows exist: every refused shape wrote
    // nothing.
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM storefront.pickup_locations \
         WHERE (metadata->>'deleted_at') IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 2, "only the two coherent stores were written");
    probe.dispose().await;
}
