//! Gate 3 (§7.1): the checkout row lock. Concurrent delivery + place
//! pairs against ONE cart: exactly one place wins, every trial ends
//! fully-old-carrier or fully-new (no torn state), and the winner's
//! `amount_total` equals the minted order's total AND the gateway
//! transaction's gross and net — one total, three witnesses.

use futures::future::join_all;
use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_orm::company_scope::with_company_scope;
use backbone_storefront::application::service::cart_service;
use backbone_storefront::application::service::checkout_service::{self, CheckoutDeps};

use super::common::{
    seed_carrier, seed_listing, seed_provider, seed_visitor, StubAvailability, StubCatalog, StubParty, StubPricing,
    StubTax, TestDb,
};

fn dec(units: i64, cents: u32) -> Decimal {
    Decimal::new(units * 100 + cents as i64, 2)
}

#[tokio::test]
async fn concurrent_delivery_and_place_never_tear_and_totals_conserve() {
    let db = TestDb::new("rowlock").await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let view = super::common::seed_website(&pool, "Lock Store", company).await;
    let (visitor, _token) = seed_visitor(&pool, view.id).await;
    let carrier_a = seed_carrier(&pool, company, "probe-a").await;
    let carrier_b = seed_carrier(&pool, company, "probe-b").await;
    seed_provider(&pool, company).await;

    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let item = seed_listing(&pool, &catalog, view.id, "Widget", dec(150, 0), true).await;

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

    // One cart, two lines of qty 1 each: the conserved total is 300.
    let cart = cart_service::create_cart(&pool, view.id, visitor)
        .await
        .unwrap()
        .cart;
    with_company_scope(Some(company), cart_service::add_line(
        &pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE,
    ))
    .await
    .unwrap();
    with_company_scope(Some(company), cart_service::add_line(
        &pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE,
    ))
    .await
    .unwrap();
    // Billing captured first so plain place has its party.
    with_company_scope(
        Some(company),
        checkout_service::capture_billing(&deps, company, cart.id, "buyer@lock.test", Some("Buyer")),
    )
    .await
    .unwrap();

    // SIX concurrent pairs: each pair races one delivery change against
    // one place. Exactly one place can ever succeed (the loser reads
    // state != 'open' UNDER the lock).
    let mut trials: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = (String, bool)> + Send>>> =
        Vec::new();
    for i in 0..6 {
        let deps_delivery = deps.clone();
        let deps_place = deps.clone();
        let carrier = if i % 2 == 0 { carrier_a } else { carrier_b };
        trials.push(Box::pin(async move {
            let outcome = with_company_scope(
                Some(company),
                checkout_service::set_delivery(&deps_delivery, company, cart.id, carrier),
            )
            .await;
            (format!("delivery-{i}"), outcome.is_ok())
        }));
        trials.push(Box::pin(async move {
            let outcome = with_company_scope(
                Some(company),
                checkout_service::place(&deps_place, company, cart.id, None, None),
            )
            .await;
            (format!("place-{i}"), outcome.is_ok())
        }));
    }
    let outcomes = join_all(trials).await;
    let placed: Vec<_> = outcomes
        .iter()
        .filter(|(name, _)| name.starts_with("place"))
        .collect();
    let place_wins = placed.iter().filter(|(_, won)| *won).count();
    assert_eq!(place_wins, 1, "exactly one place wins, got {placed:?}");

    // The winner's session: exactly one checkout row for the cart.
    let (_checkout_id, order_id, gateway_id, amount_total): (Uuid, Option<Uuid>, Option<Uuid>, Decimal) =
        sqlx::query_as(
            "SELECT id, sales_order_id, gateway_transaction_id, amount_total \
             FROM storefront.checkout_sessions WHERE cart_id = $1",
        )
        .bind(cart.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(order_id.is_some(), "the place minted its order");
    assert!(gateway_id.is_some(), "the paid arm minted its gateway tx");

    // Witness 1: the session's locked amount.
    assert_eq!(amount_total, dec(300, 0), "amount_total is the locked total");

    // Witness 2: the minted order's own total.
    let (order_total,): (Decimal,) =
        sqlx::query_as("SELECT total FROM selling.sales_orders WHERE id = $1")
            .bind(order_id.unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(order_total, dec(300, 0), "the order total equals the session");

    // Witness 3: the gateway transaction's gross and net.
    let (gross, net): (Decimal, Decimal) = sqlx::query_as(
        "SELECT gross_amount, net_amount \
         FROM payment_gateway.gateway_transactions WHERE id = $1",
    )
    .bind(gateway_id.unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(gross, dec(300, 0), "gateway gross equals the locked total");
    assert_eq!(net, dec(300, 0), "gateway net equals the locked total");

    // No torn trial: the cart is placed exactly once, its carrier is
    // one of the two racers' values (or null when delivery never won),
    // and exactly one order row exists for the company.
    let (state, carrier): (String, Option<Uuid>) = sqlx::query_as(
        "SELECT state::text, delivery_carrier_id FROM storefront.carts WHERE id = $1",
    )
    .bind(cart.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "placed");
    assert!(
        carrier.is_none() || carrier == Some(carrier_a) || carrier == Some(carrier_b),
        "carrier is a whole racer value, never torn: {carrier:?}"
    );
    let orders = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM selling.sales_orders WHERE company_id = $1",
    )
    .bind(company)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orders, 1, "no duplicate orders escaped the lock");

    let sessions = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.checkout_sessions WHERE cart_id = $1",
    )
    .bind(cart.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sessions, 1, "no duplicate sessions escaped the lock");
    db.dispose().await;
}

#[tokio::test]
async fn place_refuses_a_closed_cart_under_the_lock() {
    let db = TestDb::new("lockclosed").await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let view = super::common::seed_website(&pool, "Closed Store", company).await;
    let (visitor, _token) = seed_visitor(&pool, view.id).await;
    seed_provider(&pool, company).await;

    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let item = seed_listing(&pool, &catalog, view.id, "Gadget", dec(90, 0), true).await;
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

    let cart = cart_service::create_cart(&pool, view.id, visitor)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(&pool, catalog.as_ref(), availability.as_ref(), company, &cart, item, Decimal::ONE)
        .await
        .unwrap();
    checkout_service::capture_billing(&deps, company, cart.id, "x@y.test", None)
        .await
        .unwrap();
    checkout_service::place(&deps, company, cart.id, None, None)
        .await
        .unwrap();

    // The second place reads the identity-scoped 404 (the deterministic
    // closed door — whatever interleaving a loser lands in, one answer);
    // the late delivery reads the typed 409 that PROVES the window closed.
    // Neither mints a second order.
    let err = checkout_service::place(&deps, company, cart.id, None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            backbone_storefront::application::service::storefront_error::StorefrontError::CartNotFound { .. }
        ),
        "got {err:?}"
    );
    let carrier = seed_carrier(&pool, company, "late").await;
    let err = checkout_service::set_delivery(&deps, company, cart.id, carrier)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            backbone_storefront::application::service::storefront_error::StorefrontError::CartNotOpen { .. }
        ),
        "got {err:?}"
    );
    let orders = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM selling.sales_orders WHERE company_id = $1",
    )
    .bind(company)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orders, 1);
    db.dispose().await;
}
