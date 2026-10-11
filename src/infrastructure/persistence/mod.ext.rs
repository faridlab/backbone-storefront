// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

/// Bind the ambient organization scope onto a connection this module opened
/// itself. Every write transaction the module owns calls this right after
/// `begin`, so the org fence (once the composition declares this schema
/// scoped) sees the caller's entitlements instead of refusing the write at
/// the kind guard. A no-op while no ambient scope exists (seeder and
/// host-consumer paths), which keeps the module correct before the fence
/// lands.
pub(crate) async fn relay_ambient_scope(
    conn: &mut sqlx::PgConnection,
) -> Result<(), sqlx::Error> {
    if let Some(scope) = backbone_orm::org_scope::current_org_scope() {
        backbone_orm::org_scope::bind_org_scope_on(conn, &scope).await?;
    }
    Ok(())
}
