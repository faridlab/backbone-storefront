//! Gate 13 (§9.1): installs are inert. A fresh database, this module's
//! migrations applied (plus the sibling checkouts the compose stack
//! mounts): EVERY table in every sibling schema holds ZERO rows — the
//! module's bring-up writes nothing outside schema `storefront`, and
//! nothing inside it either (schema only, no seed rows).

use super::common::TestDb;

#[tokio::test]
async fn module_bringup_writes_zero_rows_anywhere() {
    let db = TestDb::new("inert").await;
    let schemas = ["storefront", "website", "selling", "payment_gateway"];
    for schema in schemas {
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
        )
        .bind(schema)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert!(
            !tables.is_empty(),
            "schema {schema} must exist after bring-up (migrations applied)"
        );
        for (table,) in &tables {
            let rows: i64 = sqlx::query_scalar(&format!(
                r#"SELECT count(*) FROM "{schema}"."{table}""#
            ))
            .fetch_one(&db.pool)
            .await
            .unwrap();
            assert_eq!(
                rows, 0,
                "bring-up wrote {rows} rows into {schema}.{table} — installs are inert"
            );
        }
    }
    db.dispose().await;
}
