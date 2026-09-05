//! Gate 12 (§8): abandonment is DERIVED. The read flips exactly at the
//! one delay constant (nothing is stored); zero cron rows exist for
//! the module; recovery eligibility is computed fresh per call (an
//! email gained LATER still qualifies — `notified_at` is a stamp,
//! never a flag); the send honors the per-website template with NO
//! fallback.

use rust_decimal::Decimal;
use uuid::Uuid;

use backbone_storefront::application::service::audit::ActorRef;
use backbone_storefront::application::service::cart_service;
use backbone_storefront::application::service::catalog_service::{self, SettingsPatch};
use backbone_storefront::application::service::checkout_service::{self, CheckoutDeps};
use backbone_storefront::application::service::recovery_service;

use super::common::{
    backdate_cart, seed_listing, seed_visitor, StubAvailability, StubCatalog, StubNotifier, StubParty, StubPricing,
    StubTax, TestDb,
};

async fn backdate(pool: &sqlx::PgPool, cart_id: Uuid, minutes: i64) {
    backdate_cart(pool, cart_id, minutes).await;
}

struct Rig {
    db: TestDb,
    company: Uuid,
    website: backbone_website::exports::WebsiteView,
    deps: std::sync::Arc<CheckoutDeps>,
    notifier: std::sync::Arc<StubNotifier>,
    party: std::sync::Arc<StubParty>,
    catalog: std::sync::Arc<StubCatalog>,
    availability: std::sync::Arc<StubAvailability>,
}

async fn rig(marker: &str) -> Rig {
    let db = TestDb::new(marker).await;
    let pool = db.pool.clone();
    let company = Uuid::new_v4();
    let view = super::common::seed_website(&pool, "Abandoned Store", company).await;
    let catalog = std::sync::Arc::new(StubCatalog::default());
    let party = std::sync::Arc::new(StubParty::new());
    let tax = std::sync::Arc::new(StubTax(Decimal::ZERO));
    let pricing = std::sync::Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
    let availability = std::sync::Arc::new(StubAvailability::new());
    let deps = std::sync::Arc::new(CheckoutDeps::new(
        pool.clone(),
        catalog.clone(),
        party.clone(),
        tax.clone(),
        pricing.clone(),
        availability.clone(),
    ));
    // The recovery template exists on the settings row.
    catalog_service::set_settings(
        &pool,
        party.as_ref(),
        view.id,
        SettingsPatch {
            access_gate: "open".into(),
            default_customer_group_id: None,
            recovery_template_ref: Some("recovery/default-v1".into()),
            display_warehouse_id: None,
        },
        ActorRef::system(),
    )
    .await
    .unwrap();
    Rig {
        db,
        company,
        website: view,
        deps,
        notifier: std::sync::Arc::new(StubNotifier::default()),
        party,
        catalog,
        availability,
    }
}

#[tokio::test]
async fn the_derived_read_flips_exactly_at_the_one_constant() {
    let rig = rig("flip").await;
    let pool = &rig.db.pool;
    let (visitor, _token) = seed_visitor(pool, rig.website.id).await;
    let item = seed_listing(pool, &rig.catalog, rig.website.id, "Clock", Decimal::new(1000, 2), true).await;
    rig.availability.stock(item, Decimal::new(10000, 0));

    let cart = cart_service::create_cart(pool, rig.website.id, visitor)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(pool, rig.catalog.as_ref(), rig.availability.as_ref(), rig.company, &cart, item, Decimal::ONE)
        .await
        .unwrap();

    let hours = recovery_service::abandoned_after_hours();
    assert!(hours > 0, "the constant reads a sane window");

    // One minute INSIDE the window: not abandoned.
    backdate(pool, cart.id, hours * 60 - 1).await;
    assert!(
        !recovery_service::cart_is_abandoned(pool, cart.id, hours)
            .await
            .unwrap(),
        "inside the window the cart is just idle"
    );
    let rows = recovery_service::abandoned_carts_for_company(pool, rig.company, hours)
        .await
        .unwrap();
    assert!(rows.is_empty(), "the derived read agrees");

    // One minute PAST the window: abandoned — with NOTHING stored.
    backdate(pool, cart.id, hours * 60 + 1).await;
    assert!(
        recovery_service::cart_is_abandoned(pool, cart.id, hours)
            .await
            .unwrap(),
        "past the window the read flips"
    );
    let rows = recovery_service::abandoned_carts_for_company(pool, rig.company, hours)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].line_count, 1, "the projection carries the lines");

    // Any cart mutation (the clock's only input) flips it BACK.
    cart_service::touch_cart(pool, cart.id).await.unwrap();
    assert!(
        !recovery_service::cart_is_abandoned(pool, cart.id, hours)
            .await
            .unwrap(),
        "a touch un-flips the read — nothing was ever stored"
    );
    rig.db.dispose().await;
}

#[tokio::test]
async fn recovery_sends_and_a_later_gained_email_still_qualifies() {
    let rig = rig("recover").await;
    let pool = &rig.db.pool;
    let (visitor, _token) = seed_visitor(pool, rig.website.id).await;
    let item = seed_listing(pool, &rig.catalog, rig.website.id, "Lamp", Decimal::new(2000, 2), true).await;
    rig.availability.stock(item, Decimal::new(10000, 0));

    let cart = cart_service::create_cart(pool, rig.website.id, visitor)
        .await
        .unwrap()
        .cart;
    cart_service::add_line(pool, rig.catalog.as_ref(), rig.availability.as_ref(), rig.company, &cart, item, Decimal::ONE)
        .await
        .unwrap();
    backdate(pool, cart.id, 180).await;

    // No billing captured yet: the typed refusal (no contact address).
    let err = recovery_service::send_recovery(pool, rig.notifier.as_ref(), cart.id, ActorRef::officer(Uuid::new_v4()))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            backbone_storefront::application::service::storefront_error::StorefrontError::NoContactAddress
        ),
        "got {err:?}"
    );
    let invites = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.recovery_invites WHERE cart_id = $1",
    )
    .bind(cart.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(invites, 0, "a refused send records nothing");

    // The email arrives LATER (billing capture on the abandoned cart).
    checkout_service::capture_billing(
        &rig.deps,
        rig.company,
        cart.id,
        "sleeper@shopper.test",
        None,
    )
    .await
    .unwrap();
    // capture_billing touched the clock — backdate again past the
    // window (the shopper is STILL abandoned).
    backdate(pool, cart.id, 180).await;

    // Eligibility survived: fresh per call, no permanent flag.
    let label = recovery_service::send_recovery(
        pool,
        rig.notifier.as_ref(),
        cart.id,
        ActorRef::officer(Uuid::new_v4()),
    )
    .await
    .unwrap();
    assert_eq!(label, "sent");
    let (template, delivery): (String, String) = sqlx::query_as(
        "SELECT template_ref, delivery_state \
         FROM storefront.recovery_invites WHERE cart_id = $1",
    )
    .bind(cart.id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(template, "recovery/default-v1", "the website's template, no fallback");
    assert_eq!(delivery, "sent");
    assert_eq!(rig.notifier.messages.lock().unwrap().len(), 1);

    // A SECOND send also succeeds — notified_at is a stamp, never an
    // eligibility input.
    let label = recovery_service::send_recovery(
        pool,
        rig.notifier.as_ref(),
        cart.id,
        ActorRef::officer(Uuid::new_v4()),
    )
    .await
    .unwrap();
    assert_eq!(label, "sent", "no one-shot flag exists");
    rig.db.dispose().await;
}

#[tokio::test]
async fn zero_cron_rows_exist_for_the_module() {
    // Structurally: no migration in the module ships cron DDL.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let dir = format!("{manifest}/migrations");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            let sql = std::fs::read_to_string(&path).unwrap();
            assert!(
                !sql.to_lowercase().contains("cron"),
                "migration {} must not schedule anything",
                path.display()
            );
        }
    }
    // At runtime (best effort — a scratch Postgres without pg_cron
    // proves the point by having no scheduler at all): when a cron
    // catalog exists, no job may reference storefront.
    let db = TestDb::new("cron").await;
    let has_cron: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'cron' AND table_name = 'job')",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    if has_cron {
        let jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM cron.job \
             WHERE command ILIKE '%storefront%' OR database ILIKE '%storefront%'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(jobs, 0, "no cron job may touch the module");
    }
    db.dispose().await;
}
