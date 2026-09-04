//! Gate 7 (§5.2): the pricing mapping. Two websites with different
//! default segments, one shared catalog, ONE pricing adapter: the
//! nets differ per website; guest carts pass `customer_id=None`; a
//! billing capture re-prices in-verb (the response carries the new
//! arm's price, not the caller's stale view).

use std::sync::Arc;

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_storefront::application::service::audit::ActorRef;
use backbone_storefront::application::service::cart_service;
use backbone_storefront::application::service::catalog_service::{self, SettingsPatch};
use backbone_storefront::application::service::party_write_port::PartyWritePort;
use backbone_storefront::application::service::pricing_service;

use super::common::{
    seed_listing, seed_visitor, post, StubCatalog, StubParty, StubPricing, StubSurface, StubTax,
    TestDb,
};

fn dec(units: i64, cents: u32) -> Decimal {
    Decimal::new(units * 100 + cents as i64, 2)
}

#[tokio::test]
async fn two_websites_price_differently_through_one_adapter() {
    let db = TestDb::new("pricing").await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let site_a = super::common::seed_website(&pool, "Site A", company).await;
    let site_b = super::common::seed_website(&pool, "Site B", company).await;
    let group_a = Uuid::new_v4();
    let group_b = Uuid::new_v4();

    let catalog = Arc::new(StubCatalog::default());
    let party = Arc::new(StubParty::new());
    let tax = Arc::new(StubTax(Decimal::ZERO));
    // ONE adapter: no-segment 1.0x, segment A 0.8x, segment B 0.5x.
    let pricing = Arc::new(StubPricing::new(
        Decimal::ONE,
        vec![(group_a, dec(0, 80)), (group_b, dec(0, 50))],
    ));

    // Each website defaults its guest pricing to its own segment.
    for (site, group) in [(site_a.id, group_a), (site_b.id, group_b)] {
        catalog_service::set_settings(
            &pool,
            party.as_ref(),
            site,
            SettingsPatch {
                access_gate: "open".into(),
                default_customer_group_id: Some(group),
                recovery_template_ref: None,
            },
            ActorRef::system(),
        )
        .await
        .unwrap();
    }

    // ONE catalog item, merchandised + priced 100 on BOTH websites.
    let item = seed_listing(&pool, &catalog, site_a.id, "Shared", dec(100, 0), true).await;
    let listing_b = catalog_service::upsert_listing(
        &pool,
        site_b.id,
        item,
        true,
        10,
        serde_json::json!(["https://cdn.example.test/shared.jpg"]),
        ActorRef::system(),
    )
    .await
    .unwrap();
    catalog_service::set_price(
        &pool,
        site_b.id,
        item,
        dec(100, 0),
        None,
        "IDR",
        ActorRef::system(),
    )
    .await
    .unwrap();
    catalog_service::publish_listing(&pool, site_b.id, listing_b, ActorRef::system())
        .await
        .unwrap();

    // One guest cart per website, one line each.
    let (visitor_a, token_a) = seed_visitor(&pool, site_a.id).await;
    let (visitor_b, _token_b) = seed_visitor(&pool, site_b.id).await;
    let cart_a = cart_service::create_cart(&pool, site_a.id, visitor_a)
        .await
        .unwrap()
        .cart;
    let cart_b = cart_service::create_cart(&pool, site_b.id, visitor_b)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(&pool, catalog.as_ref(), company, &cart_a, item, Decimal::ONE)
        .await
        .unwrap();
    cart_service::add_line(&pool, catalog.as_ref(), company, &cart_b, item, Decimal::ONE)
        .await
        .unwrap();

    // The SAME derivation, the SAME adapter instance, two websites.
    let mut conn = pool.acquire().await.unwrap();
    let lines_a = cart_service::lines_of(&mut *conn, cart_a.id).await.unwrap();
    let view_a = pricing_service::price_cart(
        &mut conn,
        catalog.as_ref(),
        party.as_ref(),
        pricing.as_ref(),
        company,
        &cart_a,
        &lines_a,
    )
    .await
    .unwrap();
    let lines_b = cart_service::lines_of(&mut *conn, cart_b.id).await.unwrap();
    let view_b = pricing_service::price_cart(
        &mut conn,
        catalog.as_ref(),
        party.as_ref(),
        pricing.as_ref(),
        company,
        &cart_b,
        &lines_b,
    )
    .await
    .unwrap();
    assert_eq!(view_a.subtotal, dec(80, 0), "site A nets its segment's price");
    assert_eq!(view_b.subtotal, dec(50, 0), "site B nets its own, different");
    assert_eq!(
        view_a.customer_group_id,
        Some(group_a),
        "the mapping resolved site A's default segment"
    );
    assert_eq!(view_b.customer_group_id, Some(group_b));

    // Guest carts pass customer_id=None into the port (the anonymous
    // shape, verbatim).
    let requests = pricing.requests.lock().unwrap();
    assert!(
        requests.iter().all(|r| r.customer_id.is_none()),
        "guest carts carry no customer id"
    );
    drop(requests);

    // ── the billing re-price, in-verb, at the route ───────────────────
    // The billing party carries segment B's group — the fiscal
    // re-resolution moves the arm, and the SAME POST response carries
    // the new price (never a stale view). Pre-mint the party and
    // program its segment so the arm is deterministic.
    let minted = party
        .mint_customer_party(company, "vip@example.com", None)
        .await
        .unwrap();
    party.segment(minted, group_b);
    let state = backbone_storefront::presentation::http::StorefrontPublicState::compose(
        pool.clone(),
        Arc::new(StubSurface::binding(site_a.clone())),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
    );
    let app = backbone_storefront::presentation::http::storefront_public_routes(state);
    let (status, body) = post(
        &app,
        "/public/cart/billing",
        Some(&token_a),
        &serde_json::json!({"email": "VIP@Example.COM", "name": "VIP"}).to_string(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(
        body["subtotal"], serde_json::json!(dec(50, 0)),
        "the billing verb re-priced in-verb to the party arm: {}",
        body["subtotal"]
    );
    assert_eq!(
        body["lines"][0]["unit_price"], serde_json::json!(dec(50, 0)),
        "the line carries the new unit: {}",
        body["lines"][0]["unit_price"]
    );
    assert_eq!(
        body["billing_party_id"], serde_json::json!(minted),
        "the email normalized before the map hit"
    );
    db.dispose().await;
}
