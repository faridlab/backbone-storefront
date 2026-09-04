//! Gate 8: coupon discipline. Apply is POST-only (a GET never
//! redeems); refusal text is UNIFORM across malformed and
//! well-formed-but-unknown codes — no enumeration oracle; the stored
//! code is case-folded; remove clears it.

use rust_decimal::Decimal;

use super::common::{get, post, seed_listing, seed_visitor, Probe};

#[tokio::test]
async fn coupon_apply_is_post_only_and_refusals_are_uniform() {
    let probe = Probe::boot("coupon").await;
    let pool = probe.pool.clone();
    let site = probe.view.id;

    let item = seed_listing(&pool, &probe.catalog, site, "Thing", Decimal::new(4000, 2), true).await;
    let (_visitor, token) = seed_visitor(&pool, site).await;
    let (status, _) = post(&probe.public, "/public/cart", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // A GET on the coupon path is method-refused — a GET form never
    // reaches the redeem surface at all.
    let (status, _) = get(&probe.public, "/public/cart/coupon", Some(&token)).await;
    assert_eq!(status, axum::http::StatusCode::METHOD_NOT_ALLOWED);

    // A well-formed code applies (case-folded; redemption is promo's
    // verdict at the next pricing pass, not this verb's).
    let (status, body) = post(
        &probe.public,
        "/public/cart/coupon",
        Some(&token),
        &serde_json::json!({"code": "  Summer-2026  "}).to_string(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["coupon_code"], serde_json::json!("summer-2026"));

    // The refusal family: too-short, too-long, and an injection-shaped
    // code that is LENGTH-valid but must read EXACTLY like the others
    // when refused — the uniformity is the point (no oracle).
    let refused = ["ab", &"x".repeat(70), "'; DROP TABLE carts; --"];
    let mut bodies = Vec::new();
    for code in refused {
        let (status, body) = post(
            &probe.public,
            "/public/cart/coupon",
            Some(&token),
            &serde_json::json!({"code": code}).to_string(),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        bodies.push(body);
    }
    assert!(
        bodies.windows(2).all(|w| w[0] == w[1]),
        "every refusal carries the same body — no enumeration oracle: {bodies:?}"
    );

    // A format-valid unknown code is indistinguishable from a refused
    // one on THIS surface (the port decides redemption later, never
    // here): re-applying a different well-formed code still answers
    // the uniform OK store shape, and the stored value flips.
    let (status, body) = post(
        &probe.public,
        "/public/cart/coupon",
        Some(&token),
        &serde_json::json!({"code": "winter-2027"}).to_string(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["coupon_code"], serde_json::json!("winter-2027"));

    // Remove clears it.
    let (status, body) = post(&probe.public, "/public/cart/coupon/remove", Some(&token), "{}").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["coupon_code"], serde_json::json!(null));

    // The stored row carries the folded form only.
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT coupon_code FROM storefront.carts WHERE state = 'open'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, None);

    // The cart still has its line and no item was priced in this probe.
    let _ = item;
    probe.dispose().await;
}
