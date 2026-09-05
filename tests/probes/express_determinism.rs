//! Gate 9 (EC-23): express determinism. Parallel expresses with ONE
//! email against ONE cart: the row lock admits exactly one place, and
//! the (company, email) party map admits exactly one row / one party /
//! one order — the double-click shopper can never double-place.

use futures::future::join_all;
use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_storefront::application::service::cart_service;
use backbone_storefront::application::service::checkout_service::{self, CheckoutDeps};

use super::common::{
    seed_listing, seed_provider, seed_visitor, StubAvailability, StubCatalog, StubParty, StubPricing, StubTax, TestDb,
};

#[tokio::test]
async fn parallel_expresses_double_click_safe_one_email() {
    let db = TestDb::new("express").await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let view = super::common::seed_website(&pool, "Express Store", company).await;
    let (visitor, _token) = seed_visitor(&pool, view.id).await;
    seed_provider(&pool, company).await;

    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let item = seed_listing(
        &pool,
        &catalog,
        view.id,
        "Express Item",
        Decimal::new(7500, 2),
        true,
    )
    .await;
    let availability = std::sync::Arc::new(StubAvailability::new());
    availability.stock(item, Decimal::new(10000, 0));
    let deps = std::sync::Arc::new(CheckoutDeps::new(
        pool.clone(),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    ));

    let cart = cart_service::create_cart(&pool, view.id, visitor)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(&pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE)
        .await
        .unwrap();

    // SIX parallel expresses, one email, one cart.
    let email = "double@click.test";
    let attempts = join_all((0..6).map(|_| {
        let deps = deps.clone();
        async move {
            checkout_service::place(
                &deps,
                company,
                cart.id,
                Some((email.to_string(), None)),
                None,
            )
            .await
        }
    }))
    .await;

    let wins = attempts.iter().filter(|a| a.is_ok()).count();
    assert_eq!(wins, 1, "the lock admits exactly one express");
    for loss in attempts.iter().filter(|a| a.is_err()) {
        let err = format!("{:?}", loss.as_ref().err());
        assert!(
            err.contains("CartNotFound"),
            "the losers read the deterministic closed-door refusal, got {err}"
        );
    }

    // ONE shopper-parties map row, ONE party behind it.
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT party_id FROM storefront.shopper_parties \
         WHERE company_id = $1 AND email_normalized = $2",
    )
    .bind(company)
    .bind(email)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "the map row is unique");
    let party_id = rows[0].0;

    // ONE order, riding that exact party as its customer.
    let (customer,): (Option<Uuid>,) = sqlx::query_as(
        "SELECT customer_id FROM selling.sales_orders WHERE company_id = $1",
    )
    .bind(company)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        customer,
        Some(party_id),
        "the single order's customer is the single resolved party"
    );

    // ONE checkout session, placed.
    let sessions = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.checkout_sessions WHERE cart_id = $1",
    )
    .bind(cart.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sessions, 1);
    db.dispose().await;
}

#[tokio::test]
async fn two_shoppers_one_email_resolve_one_party() {
    // The companion arm: DIFFERENT carts (two visitors), same email —
    // the map's race-free insert binds ONE party for the company; each
    // cart still places its own order (two shoppers, two carts, two
    // orders, one identity).
    let db = TestDb::new("expressmap").await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let view = super::common::seed_website(&pool, "Map Store", company).await;
    let (visitor_a, _ta) = seed_visitor(&pool, view.id).await;
    let (visitor_b, _tb) = seed_visitor(&pool, view.id).await;
    seed_provider(&pool, company).await;

    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let item = seed_listing(&pool, &catalog, view.id, "Shared", Decimal::new(2500, 2), true).await;
    let availability = std::sync::Arc::new(StubAvailability::new());
    availability.stock(item, Decimal::new(10000, 0));
    let deps = std::sync::Arc::new(CheckoutDeps::new(
        pool.clone(),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    ));

    for visitor in [visitor_a, visitor_b] {
        let cart = cart_service::create_cart(&pool, view.id, visitor)
            .await
            .unwrap()
            .cart;
        cart_service::add_line(&pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE)
            .await
            .unwrap();
        let placed = checkout_service::place(
            &deps,
            company,
            cart.id,
            Some(("same@shopper.test".into(), None)),
            None,
        )
        .await;
        assert!(placed.is_ok(), "{:?}", placed.err());
    }

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT party_id FROM storefront.shopper_parties \
         WHERE company_id = $1 AND email_normalized = 'same@shopper.test'",
    )
    .bind(company)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "one map row for one email");
    let party_id = rows[0].0;
    let customers: Vec<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT customer_id FROM selling.sales_orders WHERE company_id = $1",
    )
    .bind(company)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(customers.len(), 2, "two shoppers, two orders");
    assert!(
        customers.iter().all(|c| c.0 == Some(party_id)),
        "both orders ride the one resolved party"
    );
    db.dispose().await;
}
