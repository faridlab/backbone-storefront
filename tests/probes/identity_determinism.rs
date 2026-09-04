//! Gate 2 (§2): deterministic create/adopt. N concurrent creates for
//! one identity leave exactly ONE open cart; the adopt-refusal family
//! answers typed refusals; another identity's abandoned cart is
//! returned to no one and adoptable by no one.

use futures::future::join_all;
use uuid::Uuid;

use backbone_storefront::application::service::cart_service::{self};
use backbone_storefront::application::service::recovery_service;
use backbone_storefront::application::service::storefront_error::StorefrontError;

use super::common::{seed_visitor, TestDb};

#[tokio::test]
async fn concurrent_creates_leave_exactly_one_open_cart() {
    let db = TestDb::new("identity").await;
    let pool = &db.pool;
    let company = Uuid::new_v4();
    let view = super::common::seed_website(pool, "Identity Store", company).await;
    let (visitor, _token) = seed_visitor(pool, view.id).await;

    let creates = join_all((0..8).map(|_| cart_service::create_cart(pool, view.id, visitor))).await;
    for (i, outcome) in creates.iter().enumerate() {
        assert!(outcome.is_ok(), "create {i} failed: {:?}", outcome.as_ref().err());
    }
    let ids: std::collections::HashSet<Uuid> =
        creates.iter().map(|c| c.as_ref().unwrap().cart.id).collect();
    assert_eq!(ids.len(), 1, "the racers must all read the SAME cart");
    let winners = creates.iter().filter(|c| c.as_ref().unwrap().created).count();
    assert_eq!(winners, 1, "exactly one racer created the row");

    let open = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.carts \
         WHERE visitor_id = $1 AND state = 'open' \
           AND (metadata->>'deleted_at') IS NULL",
    )
    .bind(visitor)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(open, 1, "one open cart per visitor, enforced");

    // The audit row stamps cart_created exactly once (the winner only).
    let audits = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM storefront.storefront_audit_log \
         WHERE event = 'cart_created' AND subject_id = $1",
    )
    .bind(*ids.iter().next().unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    db.dispose().await;
}

#[tokio::test]
async fn the_adopt_refusal_family_is_typed() {
    let db = TestDb::new("adopt").await;
    let pool = &db.pool;
    let company = Uuid::new_v4();
    let view = super::common::seed_website(pool, "Adopt Store", company).await;
    let (visitor_a, _token_a) = seed_visitor(pool, view.id).await;
    let (visitor_b, _token_b) = seed_visitor(pool, view.id).await;
    let principal_a = Uuid::new_v4();
    let principal_b = Uuid::new_v4();

    // principal A's open, principal-linked cart.
    let cart = cart_service::create_cart(pool, view.id, visitor_a)
        .await
        .unwrap()
        .cart;
    sqlx::query(
        "UPDATE storefront.carts SET portal_user_id = $2 \
         WHERE id = $1",
    )
    .bind(cart.id)
    .bind(principal_a)
    .execute(pool)
    .await
    .unwrap();

    // Foreign principal: the cart is unadoptable by B (the ownership
    // fence is the portal linkage).
    let err = cart_service::adopt_cart(pool, view.id, visitor_b, principal_b, cart.id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StorefrontError::CartNotAdoptable),
        "foreign principal must read the typed refusal, got {err:?}"
    );

    // B already holds an open cart: adoption of A's cart refuses (no
    // silent merge) even though B's principal is... still foreign, so
    // first give B's own cart B's principal linkage, then try to adopt
    // A's cart.
    let cart_b = cart_service::create_cart(pool, view.id, visitor_b)
        .await
        .unwrap()
        .cart;
    sqlx::query("UPDATE storefront.carts SET portal_user_id = $2 WHERE id = $1")
        .bind(cart_b.id)
        .bind(principal_b)
        .execute(pool)
        .await
        .unwrap();
    let err = cart_service::adopt_cart(pool, view.id, visitor_b, principal_b, cart.id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StorefrontError::CartNotAdoptable),
        "a foreign cart stays unadoptable, got {err:?}"
    );

    // The already-open family: A logs in on a fresh visitor session
    // that already holds an open cart — adoption refuses with the
    // typed 409 (OpenCartExists), never a merge.
    let (visitor_a2, _token_a2) = seed_visitor(pool, view.id).await;
    cart_service::create_cart(pool, view.id, visitor_a2).await.unwrap();
    let err = cart_service::adopt_cart(pool, view.id, visitor_a2, principal_a, cart.id)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StorefrontError::OpenCartExists),
        "an open visitor cart must block adoption, got {err:?}"
    );

    // The happy arm: a fresh session with NO open cart adopts A's cart.
    let (visitor_a3, _token_a3) = seed_visitor(pool, view.id).await;
    let adopted = cart_service::adopt_cart(pool, view.id, visitor_a3, principal_a, cart.id)
        .await
        .unwrap();
    assert_eq!(adopted.id, cart.id);
    assert_eq!(adopted.visitor_id, visitor_a3);
    db.dispose().await;
}

#[tokio::test]
async fn another_identity_abandoned_cart_is_returned_to_no_one() {
    let db = TestDb::new("abandonedfence").await;
    let pool = &db.pool;
    let company = Uuid::new_v4();
    let view = super::common::seed_website(pool, "Fence Store", company).await;
    let (owner, _owner_token) = seed_visitor(pool, view.id).await;
    let (stranger, _stranger_token) = seed_visitor(pool, view.id).await;

    let cart = cart_service::create_cart(pool, view.id, owner)
        .await
        .unwrap()
        .cart;
    // Backdate past the default 1h window (the trigger-safe helper).
    super::common::backdate_cart(pool, cart.id, 180).await;

    let hours = recovery_service::abandoned_after_hours();
    let seen = recovery_service::abandoned_carts_for_identity(pool, stranger, None, hours)
        .await
        .unwrap();
    assert!(seen.is_empty(), "the stranger's derived read must be empty");
    let own = recovery_service::abandoned_carts_for_identity(pool, owner, None, hours)
        .await
        .unwrap();
    assert_eq!(own.len(), 1, "the owner still sees their cart");

    // Recovery re-bind: the stranger's attempt is the closed-door 404
    // (indistinguishable from a missing cart).
    let err = cart_service::recover_cart(pool, cart.id, stranger, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, StorefrontError::CartNotFound),
        "a foreign recover must read the closed-door 404, got {err:?}"
    );
    db.dispose().await;
}
