//! Gate 10 (§7.4): settle → confirm, exactly-once. A simulated
//! verified settlement for the `stf-` reference confirms the order
//! once and stamps the session `settled` once — a REDLIVERED webhook
//! no-ops; the `NotDraft` double guard survives the crash window (the
//! order confirmed but the stamp missing); a settlement for an
//! unbound transaction is none of this consumer's business.

use chrono::Utc;
use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_payment_gateway::application::service::GatewayTransactionSettled;
use backbone_selling::application::service::{
    NoServiceCatalog, NoServiceDelivery, NoStockFulfillmentPort, NoUnitCostPort,
    SellingWriteService,
};
use backbone_storefront::application::service::cart_service;
use backbone_storefront::application::service::checkout_service::{self, CheckoutDeps};

use super::common::{
    seed_listing, seed_provider, seed_visitor, StubAvailability, StubCatalog, StubParty, StubPricing, StubTax, TestDb,
};

async fn paid_checkout(
    db: &TestDb,
    marker: &str,
) -> (
    std::sync::Arc<CheckoutDeps>,
    Uuid,
    Uuid,
    std::sync::Arc<StubParty>,
    std::sync::Arc<StubCatalog>,
) {
    let pool = &db.pool;
    let company = Uuid::new_v4();
    let view = super::common::seed_website(pool, &format!("Settle {marker}"), company).await;
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
        "Settle Item",
        Decimal::new(12000, 2),
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
        Some(("settler@pay.test".into(), None)),
        None,
    )
    .await
    .unwrap();
    (deps, company, checkout.id, party, catalog)
}

fn settled_event(checkout: &checkout_service::CheckoutRow, company: Uuid, party: Option<Uuid>) -> GatewayTransactionSettled {
    GatewayTransactionSettled {
        gateway_transaction_id: checkout.gateway_transaction_id.unwrap(),
        company_id: company,
        provider_code: "manual".into(),
        provider_transaction_id: checkout.provider_reference.clone().unwrap(),
        direction: "receive".into(),
        party_type: Some("customer".into()),
        party_id: party,
        gross_amount: checkout.amount_total,
        fee_amount: Decimal::ZERO,
        net_amount: checkout.amount_total,
        currency: "IDR".into(),
        settled_at: Utc::now(),
        reference_no: None,
    }
}

#[tokio::test]
async fn settlement_confirms_once_across_a_redelivered_webhook() {
    let db = TestDb::new("settle").await;
    let (deps, company, checkout_id, party, _catalog) = paid_checkout(&db, "One").await;
    let pool = &db.pool;

    let checkout = checkout_service::checkout_by_id(pool, checkout_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkout.state, "pending_payment");
    let order_id = checkout.sales_order_id.unwrap();

    // Unpaid paid-carts NEVER confirm before settlement.
    let (status,): (String,) =
        sqlx::query_as("SELECT status::text FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "draft");

    // First delivery: confirm + stamp.
    let event = settled_event(&checkout, company, party.minted_party(company, "settler@pay.test"));
    let consumed = checkout_service::consume_settlement(&deps, &event).await.unwrap();
    assert!(consumed.is_some());
    let (status,): (String,) =
        sqlx::query_as("SELECT status::text FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        status, "to_deliver_and_bill",
        "settlement confirmed the order (selling's confirm leaves draft; zero watermarks roll up to to_deliver_and_bill)"
    );

    // REDLIVERY of the same webhook: the state flip no-ops.
    let again = checkout_service::consume_settlement(&deps, &event).await.unwrap();
    assert_eq!(
        again.unwrap().state,
        "settled",
        "the redelivery reads the settled state"
    );
    let audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.storefront_audit_log \
         WHERE event = 'checkout_settled_confirmed' AND subject_id = $1",
    )
    .bind(checkout_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(audits, 1, "the confirm stamp lands exactly once");
    let (status,): (String,) =
        sqlx::query_as("SELECT status::text FROM selling.sales_orders WHERE id = $1")
            .bind(order_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(status, "to_deliver_and_bill", "still left-draft, once");
    db.dispose().await;
}

#[tokio::test]
async fn the_notdraft_double_guard_survives_the_crash_window() {
    // The crash window: the confirm committed, the stamp did not. A
    // retry finds a NOT-DRAFT order — tolerated, not fatal — and
    // stamps the session settled.
    let db = TestDb::new("crashwindow").await;
    let (deps, company, checkout_id, party, _catalog) = paid_checkout(&db, "Crash").await;
    let pool = &db.pool;
    let checkout = checkout_service::checkout_by_id(pool, checkout_id)
        .await
        .unwrap()
        .unwrap();
    let order_id = checkout.sales_order_id.unwrap();

    // The window: the order is confirmed OUT of band (selling's own
    // verb — the operator's crash-window retry would do the same).
    let selling = SellingWriteService::new(pool.clone());
    selling
        .confirm_sales_order(
            order_id,
            company,
            &NoUnitCostPort,
            &NoStockFulfillmentPort,
            &NoServiceCatalog,
            &NoServiceDelivery,
        )
        .await
        .unwrap();

    let event = settled_event(&checkout, company, party.minted_party(company, "settler@pay.test"));
    let consumed = checkout_service::consume_settlement(&deps, &event)
        .await
        .unwrap_or_else(|e| panic!("the NotDraft guard must be tolerated, got {e:?}"));
    assert_eq!(consumed.unwrap().state, "settled");
    let stamped = checkout_service::checkout_by_id(pool, checkout_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stamped.state, "settled", "the stamp recovered");
    db.dispose().await;
}

#[tokio::test]
async fn an_unbound_settlement_is_not_this_consumers_lane() {
    let db = TestDb::new("unbound").await;
    let (deps, company, checkout_id, _party, _catalog) = paid_checkout(&db, "Unbound").await;
    let pool = &db.pool;
    let checkout = checkout_service::checkout_by_id(pool, checkout_id)
        .await
        .unwrap()
        .unwrap();

    // A settlement for a transaction no checkout binds (an alien
    // gateway tx id) answers Ok(None) — other traffic on the gateway
    // is not consumed.
    let mut alien = settled_event(&checkout, company, None);
    alien.gateway_transaction_id = Uuid::new_v4();
    let consumed = checkout_service::consume_settlement(&deps, &alien).await.unwrap();
    assert!(consumed.is_none(), "the alien settlement is a no-op");

    // And the REAL checkout is untouched by the alien event.
    let still = checkout_service::checkout_by_id(pool, checkout_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.state, "pending_payment");
    db.dispose().await;
}
