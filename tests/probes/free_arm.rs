//! Gate 11 (§7.5): the two place arms. A zero-total place takes the
//! FREE arm — `confirmed_free`, order confirmed at place, NO gateway
//! row; a paid place stays `pending_payment` with its order DRAFT (an
//! unpaid order never confirms) and its gateway row pending.

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_storefront::application::service::cart_service;
use backbone_storefront::application::service::checkout_service::{self, CheckoutDeps};

use super::common::{
    seed_listing, seed_provider, seed_visitor, StubAvailability, StubCatalog, StubParty, StubPricing, StubTax, TestDb,
};

#[tokio::test]
async fn zero_total_places_free_with_no_gateway_row() {
    let db = TestDb::new("free").await;
    let pool = &db.pool;
    let company = Uuid::new_v4();
    let view = super::common::seed_website(pool, "Free Store", company).await;
    let (visitor, _token) = seed_visitor(pool, view.id).await;
    seed_provider(pool, company).await;

    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let item = seed_listing(pool, &catalog, view.id, "Freebie", Decimal::ZERO, true).await;
    let availability = std::sync::Arc::new(StubAvailability::new());
    availability.stock(item, Decimal::new(10000, 0));
    let deps = CheckoutDeps::new(
        pool.clone(),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    );

    let cart = cart_service::create_cart(pool, view.id, visitor)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE)
        .await
        .unwrap();
    let checkout = checkout_service::place(
        &deps,
        company,
        cart.id,
        Some(("free@shopper.test".into(), None)),
        None,
    )
    .await
    .unwrap();

    assert_eq!(checkout.state, "confirmed_free", "the free arm at place");
    assert!(
        checkout.gateway_transaction_id.is_none(),
        "the free arm mints NO gateway row"
    );
    let gateway_rows = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM payment_gateway.gateway_transactions",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(gateway_rows, 0, "no gateway row anywhere");

    // The free order confirmed AT place.
    let (status,): (String,) = sqlx::query_as(
        "SELECT status::text FROM selling.sales_orders WHERE id = $1",
    )
    .bind(checkout.sales_order_id.unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    // Selling's confirm vocabulary: "confirmed" IS the left-draft family
    // (to_deliver_and_bill at zero watermarks) — there is no literal
    // 'confirmed' status value.
    assert_eq!(status, "to_deliver_and_bill");
    db.dispose().await;
}

#[tokio::test]
async fn a_paid_place_stays_draft_until_settlement() {
    let db = TestDb::new("paid").await;
    let pool = &db.pool;
    let company = Uuid::new_v4();
    let view = super::common::seed_website(pool, "Paid Store", company).await;
    let (visitor, _token) = seed_visitor(pool, view.id).await;
    seed_provider(pool, company).await;

    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let item = seed_listing(
        pool,
        &catalog,
        view.id,
        "Priced",
        Decimal::new(17500, 2),
        true,
    )
    .await;
    let availability = std::sync::Arc::new(StubAvailability::new());
    availability.stock(item, Decimal::new(10000, 0));
    let deps = CheckoutDeps::new(
        pool.clone(),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    );

    let cart = cart_service::create_cart(pool, view.id, visitor)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE)
        .await
        .unwrap();
    let checkout = checkout_service::place(
        &deps,
        company,
        cart.id,
        Some(("paid@shopper.test".into(), None)),
        None,
    )
    .await
    .unwrap();

    assert_eq!(checkout.state, "pending_payment", "the paid arm at place");
    assert!(checkout.gateway_transaction_id.is_some());

    // NEVER auto-confirmed: the order stays draft until the settle
    // consumer confirms it.
    let (status,): (String,) = sqlx::query_as(
        "SELECT status::text FROM selling.sales_orders WHERE id = $1",
    )
    .bind(checkout.sales_order_id.unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(status, "draft", "unpaid paid-carts never confirm");

    // The gateway row exists, pending both ways.
    let (state, posting): (String, String) = sqlx::query_as(
        "SELECT status::text, posting_state::text \
         FROM payment_gateway.gateway_transactions WHERE id = $1",
    )
    .bind(checkout.gateway_transaction_id.unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(state, "pending");
    assert_eq!(posting, "pending");
    db.dispose().await;
}
