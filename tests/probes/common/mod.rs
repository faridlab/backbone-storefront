//! Shared harness: one DISPOSABLE scratch database per probe,
//! FAIL-HARD (the sibling module suites' contract, verbatim in shape).
//!
//! The suite never runs against a shared database (and NEVER against
//! the live dev database on 5432): each probe mints
//! `stf_probe_<marker>_<hex>` on the local scratch Postgres
//! (127.0.0.1:5433 — the pinned scratch container), applies THIS
//! module's migrations plus the pinned sibling checkouts' migrations
//! (website, selling, payment-gateway — the tables the module's SQL
//! touches), runs, and drops the database.
//!
//! FAIL-HARD CONTRACT: a probe that cannot reach its scratch database
//! PANICS — [`TestDb::new`] refuses to return `None`, and [`skipped`]
//! panics on principle. A green suite means the behaviors were
//! exercised, not that they were unreachable.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

/// The scratch Postgres every probe database is born on and dropped
/// from. 127.0.0.1:5433 — the pinned scratch container, NEVER a live
/// service database.
pub const SCRATCH_ADMIN_URL: &str = "postgres://postgres:postgres@127.0.0.1:5433/postgres";

/// The probe host the StubSurface binds.
pub const PROBE_HOST: &str = "probe.example.test";

fn admin_url() -> String {
    std::env::var("STOREFRONT_TEST_ADMIN_URL").unwrap_or_else(|_| SCRATCH_ADMIN_URL.into())
}

/// The fail-hard skip: reaching this is a FAILURE, never a green tick.
pub fn skipped(reason: &str) -> ! {
    panic!("VACUOUS SKIP IS A FAILURE: {reason}");
}

/// One disposable scratch database, migrations applied. Panics (never
/// returns `None`) when the scratch Postgres is unreachable.
pub struct TestDb {
    pub pool: PgPool,
    name: String,
    admin: PgPool,
}

impl TestDb {
    pub async fn new(marker: &str) -> Self {
        Self::boot(marker, &[]).await
    }

    /// The probe database for probes that ALSO touch the inventory
    /// checkout's tables (the collect registry's warehouse fence reads
    /// `inventory.warehouses`): applies backbone-inventory's migrations
    /// on top of the standard sibling set.
    pub async fn new_with_inventory(marker: &str) -> Self {
        Self::boot(marker, &["backbone-inventory"]).await
    }

    async fn boot(marker: &str, extra_siblings: &[&str]) -> Self {
        let url = admin_url();
        let admin = match PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("PROBE-FAIL: {marker}: admin connect to {url} failed: {e}");
                skipped(&format!("scratch Postgres unreachable: {e}"));
            }
        };
        let suffix: String = Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();
        let name = format!("stf_probe_{marker}_{suffix}");
        // Disposable by construction: a stale DB of the same name goes
        // first.
        if let Err(e) = sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#))
            .execute(&admin)
            .await
        {
            eprintln!("PROBE-FAIL: {marker}: pre-drop of {name} failed: {e}");
            skipped(&format!("scratch pre-drop failed: {e}"));
        }
        if let Err(e) = sqlx::query(&format!(r#"CREATE DATABASE "{name}""#))
            .execute(&admin)
            .await
        {
            eprintln!("PROBE-FAIL: {marker}: create database {name} failed: {e}");
            skipped(&format!("scratch create failed: {e}"));
        }
        let db_url = match url.rfind('/') {
            Some(i) => format!("{}{}", &url[..=i], name),
            None => url.clone(),
        };
        let pool = match PgPoolOptions::new()
            .max_connections(16)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&db_url)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("PROBE-FAIL: {marker}: connect to {db_url} failed: {e}");
                skipped(&format!("scratch connect failed: {e}"));
            }
        };
        if let Err(what) = apply_module_migrations(&pool, marker).await {
            skipped(&what);
        }
        if let Err(what) = apply_sibling_migrations(&pool, marker, extra_siblings).await {
            skipped(&what);
        }
        Self { pool, name, admin }
    }

    /// Explicit teardown: drop the scratch database entirely.
    pub async fn dispose(self) {
        self.drop_db().await;
    }

    async fn drop_db(&self) {
        // FORCE: the connected probe pool may still hold an idle
        // session.
        let _ = sqlx::query(&format!(
            r#"DROP DATABASE IF EXISTS "{}" WITH (FORCE)"#,
            self.name
        ))
        .execute(&self.admin)
        .await;
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let name = self.name.clone();
        let url = admin_url();
        // Leak-guard teardown for panicking probes; dispose() is the
        // happy path.
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async move {
                    if let Ok(admin) = sqlx::PgPool::connect(&url).await {
                        let _ = sqlx::query(&format!(
                            r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#
                        ))
                        .execute(&admin)
                        .await;
                    }
                });
            }
        });
    }
}

/// Apply one directory's `.up.sql` migrations in sorted order with a
/// raw SQL file runner. The module's own files and the sibling
/// checkouts' files are self-contained (no cross-schema FOREIGN KEYs),
/// so directory order carries no constraint between them.
async fn apply_migration_dir(
    pool: &PgPool,
    marker: &str,
    dir: &str,
    what: &str,
) -> Result<(), String> {
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".up.sql"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(e) => {
            return Err(format!(
                "PROBE-FAIL: {marker}: cannot read {what} at {dir}: {e} \
                 (the sibling checkout is expected beside this crate)"
            ))
        }
    };
    files.sort();
    if files.is_empty() {
        return Err(format!(
            "PROBE-FAIL: {marker}: no .up.sql migrations found in {what} at {dir}"
        ));
    }
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| format!("PROBE-FAIL: {marker}: cannot acquire pool conn: {e}"))?;
    for file in files {
        let sql = std::fs::read_to_string(&file)
            .map_err(|e| format!("PROBE-FAIL: {marker}: cannot read {}: {e}", file.display()))?;
        if let Err(e) = sqlx::raw_sql(&sql).execute(&mut *conn).await {
            return Err(format!(
                "PROBE-FAIL: {marker}: {what} migration {} failed: {e}",
                file.display()
            ));
        }
    }
    Ok(())
}

/// Apply this module's migrations.
async fn apply_module_migrations(pool: &PgPool, marker: &str) -> Result<(), String> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    apply_migration_dir(pool, marker, &format!("{manifest}/migrations"), "module").await
}

/// The pinned sibling checkouts whose tables the module's SQL touches
/// (the composing-service truth: website for host/visitor identity,
/// selling for orders + carriers, payment-gateway for providers +
/// transactions). Overridable for hermetic runs via
/// `STOREFRONT_TEST_MODULES_DIR` (a directory holding the three
/// checkouts); default is this crate's siblings in the modules tree.
async fn apply_sibling_migrations(
    pool: &PgPool,
    marker: &str,
    extra_siblings: &[&str],
) -> Result<(), String> {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let root = std::env::var("STOREFRONT_TEST_MODULES_DIR")
        .unwrap_or_else(|_| format!("{manifest}/.."));
    let mut siblings = vec![
        "backbone-website",
        "backbone-selling",
        "backbone-payment-gateway",
    ];
    siblings.extend_from_slice(extra_siblings);
    for sibling in siblings {
        apply_migration_dir(
            pool,
            marker,
            &format!("{root}/{sibling}/migrations"),
            sibling,
        )
        .await?;
    }
    Ok(())
}

// ── shared stubs ───────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rust_decimal::Decimal;

use backbone_storefront::application::service::availability_port::{
    AvailabilityPortError, AvailabilityReadPort, ItemAvailability,
};
use backbone_storefront::application::service::catalog_read_port::{
    CatalogPortError, CatalogReadPort, ItemSnapshot,
};
use backbone_storefront::application::service::notifier_port::{
    RecoveryDelivery, RecoveryMessage, RecoveryNotifier, StockAlertDelivery, StockAlertMessage,
    StockAlertNotifier,
};
use backbone_storefront::application::service::party_write_port::{
    PartyPortError, PartyWritePort,
};
use backbone_storefront::application::service::tax_resolve_port::{TaxPortError, TaxResolvePort};

/// The stub catalog: a programmed item table. Active by default; a
/// probe can archive an item to close the gate's catalog arm.
#[derive(Default)]
pub struct StubCatalog {
    items: Mutex<HashMap<Uuid, ItemSnapshot>>,
}

impl StubCatalog {
    pub fn with(items: Vec<ItemSnapshot>) -> Self {
        Self {
            items: Mutex::new(items.into_iter().map(|i| (i.item_id, i)).collect()),
        }
    }

    pub fn add(&self, item: ItemSnapshot) {
        if let Ok(mut items) = self.items.lock() {
            items.insert(item.item_id, item);
        }
    }

    /// Close the gate's catalog arm for one item (status → archived).
    pub fn archive(&self, item_id: Uuid) {
        if let Ok(mut items) = self.items.lock() {
            if let Some(item) = items.get_mut(&item_id) {
                item.status = "archived".into();
            }
        }
    }
}

fn catalog_refused() -> CatalogPortError {
    CatalogPortError {
        code: "catalog_port_unwired".into(),
        message: "no catalog read adapter is installed".into(),
    }
}

#[async_trait]
impl CatalogReadPort for StubCatalog {
    async fn item_snapshot(
        &self,
        _company_id: Uuid,
        item_id: Uuid,
    ) -> Result<Option<ItemSnapshot>, CatalogPortError> {
        Ok(self.items.lock().ok().and_then(|i| i.get(&item_id).cloned()))
    }

    async fn item_snapshots(
        &self,
        _company_id: Uuid,
        item_ids: &[Uuid],
    ) -> Result<Vec<ItemSnapshot>, CatalogPortError> {
        // Refuse loudly when never programmed — an empty stub is a
        // misconfigured probe, not an empty store.
        let items = self.items.lock().map_err(|_| catalog_refused())?;
        if items.is_empty() {
            return Err(catalog_refused());
        }
        Ok(item_ids.iter().filter_map(|id| items.get(id).cloned()).collect())
    }
}

/// The stub party port: idempotent mint per (company, email) — the
/// adapter contract the port's docs state. Probes program explicit
/// party segments for the pricing-mapping arm.
pub struct StubParty {
    mints: Mutex<HashMap<(Uuid, String), Uuid>>,
    segments: Mutex<HashMap<Uuid, Uuid>>,
    mint_calls: AtomicUsize,
}

impl StubParty {
    pub fn new() -> Self {
        Self {
            mints: Mutex::new(HashMap::new()),
            segments: Mutex::new(HashMap::new()),
            mint_calls: AtomicUsize::new(0),
        }
    }

    /// Program one party's explicit customer segment.
    pub fn segment(&self, party_id: Uuid, group_id: Uuid) {
        if let Ok(mut segments) = self.segments.lock() {
            segments.insert(party_id, group_id);
        }
    }

    /// The party a (company, email) mint resolved to, when one ran.
    pub fn minted_party(&self, company_id: Uuid, email: &str) -> Option<Uuid> {
        self.mints
            .lock()
            .ok()
            .and_then(|m| m.get(&(company_id, email.into())).copied())
    }

    /// How many mint CALLS ran (the map dedupes the outcomes).
    pub fn mint_calls(&self) -> usize {
        self.mint_calls.load(Ordering::SeqCst)
    }
}

impl Default for StubParty {
    fn default() -> Self {
        Self::new()
    }
}

fn party_refused() -> PartyPortError {
    PartyPortError {
        code: "party_port_unwired".into(),
        message: "no party write adapter is installed".into(),
    }
}

#[async_trait]
impl PartyWritePort for StubParty {
    async fn mint_customer_party(
        &self,
        company_id: Uuid,
        email_normalized: &str,
        _name: Option<&str>,
    ) -> Result<Uuid, PartyPortError> {
        self.mint_calls.fetch_add(1, Ordering::SeqCst);
        let key = (company_id, email_normalized.to_string());
        if let Ok(mut mints) = self.mints.lock() {
            if let Some(known) = mints.get(&key) {
                return Ok(*known);
            }
            let minted = Uuid::new_v4();
            mints.insert(key, minted);
            Ok(minted)
        } else {
            Err(party_refused())
        }
    }

    async fn mint_guest_party(&self, company_id: Uuid) -> Result<Uuid, PartyPortError> {
        self.mint_customer_party(company_id, "guest@invalid.test", None)
            .await
    }

    async fn party_segment(
        &self,
        _company_id: Uuid,
        party_id: Uuid,
    ) -> Result<Option<Uuid>, PartyPortError> {
        Ok(self.segments.lock().ok().and_then(|s| s.get(&party_id).copied()))
    }
}

/// The stub tax resolver: one fixed rate.
pub struct StubTax(pub Decimal);

#[async_trait]
impl TaxResolvePort for StubTax {
    async fn resolve_rate(
        &self,
        _company_id: Uuid,
        _delivery_jurisdiction: Option<&str>,
    ) -> Result<Decimal, TaxPortError> {
        Ok(self.0)
    }
}

/// The RECORDING tax resolver: one rate per jurisdiction, `None` being
/// the company-home arm, and every call's jurisdiction recorded — the
/// fiscal-pin probe asserts WHICH arm each place resolved under (the
/// distinction a fixed-rate stub can never see). An unprogrammed
/// jurisdiction REFUSES: a place resolving somewhere the probe did not
/// arm must explode the probe, not silently borrow the home rate.
pub struct RecordingTax {
    home_rate: Decimal,
    keyed: HashMap<String, Decimal>,
    pub calls: Mutex<Vec<Option<String>>>,
}

impl RecordingTax {
    /// The home arm's rate plus the explicitly armed jurisdictions.
    pub fn new(home_rate: Decimal, keyed: Vec<(String, Decimal)>) -> Self {
        Self {
            home_rate,
            keyed: keyed.into_iter().collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Did one place resolve under exactly this jurisdiction?
    pub fn saw(&self, jurisdiction: Option<&str>) -> bool {
        self.calls
            .lock()
            .map(|c| c.iter().any(|j| j.as_deref() == jurisdiction))
            .unwrap_or(false)
    }

    /// How many resolutions ran.
    pub fn call_count(&self) -> usize {
        self.calls.lock().map(|c| c.len()).unwrap_or(0)
    }
}

#[async_trait]
impl TaxResolvePort for RecordingTax {
    async fn resolve_rate(
        &self,
        _company_id: Uuid,
        delivery_jurisdiction: Option<&str>,
    ) -> Result<Decimal, TaxPortError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(delivery_jurisdiction.map(str::to_string));
        }
        match delivery_jurisdiction {
            None => Ok(self.home_rate),
            Some(code) => self.keyed.get(code).copied().ok_or_else(|| TaxPortError {
                code: "probe_jurisdiction_not_armed".into(),
                message: format!("the recording stub has no rate armed for {code}"),
            }),
        }
    }
}

use backbone_selling::application::service::selling_cart_pricing::{
    CartPricingError, CartPricingPort, CartPriceRequest, PricedCart, PricedCartLine,
    PricedRewardLine,
};

/// The stub cart-pricing adapter: prices every line as
/// `list_price × factor(customer_group_id)`, conserving
/// `total == Σ net_line_total` exactly (the port's own conservation
/// contract). Probes program one factor per segment; `None` (no
/// segment) has its own. Records every request for the mapping
/// assertions. Reward lines are programmable (the coupon-claim probe
/// arms a "buy X get Y" grant) and are NEVER priced into the total —
/// the port's own reward contract.
pub struct StubPricing {
    factors: HashMap<Option<Uuid>, Decimal>,
    pub requests: Mutex<Vec<CartPriceRequest>>,
    rewards: Mutex<Vec<PricedRewardLine>>,
}

impl StubPricing {
    /// factor(None) and the explicit per-segment factors.
    pub fn new(default_factor: Decimal, per_segment: Vec<(Uuid, Decimal)>) -> Self {
        let mut factors = HashMap::new();
        factors.insert(None, default_factor);
        for (group, factor) in per_segment {
            factors.insert(Some(group), factor);
        }
        Self {
            factors,
            requests: Mutex::new(Vec::new()),
            rewards: Mutex::new(Vec::new()),
        }
    }

    /// Arm a reward grant: every priced answer carries these reward
    /// lines (the loyalty probe's "buy X get Y" shape).
    pub fn arm_rewards(&self, rewards: Vec<(Uuid, Decimal)>) {
        if let Ok(mut armed) = self.rewards.lock() {
            *armed = rewards
                .into_iter()
                .map(|(item_id, quantity)| PricedRewardLine { item_id, quantity })
                .collect();
        }
    }

    fn factor_for(&self, group: Option<Uuid>) -> Decimal {
        self.factors.get(&group).copied().unwrap_or(Decimal::ONE)
    }
}

#[async_trait]
impl CartPricingPort for StubPricing {
    async fn price_cart(&self, req: &CartPriceRequest) -> Result<PricedCart, CartPricingError> {
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(req.clone());
        }
        let factor = self.factor_for(req.customer_group_id);
        let mut lines = Vec::new();
        let mut total = Decimal::ZERO;
        for line in &req.lines {
            let unit = (line.list_price * factor).round_dp(2);
            let net = (unit * line.quantity).round_dp(2);
            total += net;
            lines.push(PricedCartLine {
                line_ref: line.line_ref,
                unit_price: unit,
                net_line_total: net,
            });
        }
        let reward_lines = self
            .rewards
            .lock()
            .map(|armed| armed.clone())
            .unwrap_or_default();
        Ok(PricedCart {
            lines,
            reward_lines,
            total,
        })
    }
}

/// The stub recovery notifier: every send is accepted and recorded.
#[derive(Default)]
pub struct StubNotifier {
    pub messages: Mutex<Vec<String>>,
}

#[async_trait]
impl RecoveryNotifier for StubNotifier {
    async fn send_recovery(&self, message: &RecoveryMessage<'_>) -> Result<RecoveryDelivery, String> {
        if let Ok(mut messages) = self.messages.lock() {
            messages.push(format!(
                "{} -> {} ({})",
                message.template_ref, message.to_address, message.cart_id
            ));
        }
        Ok(RecoveryDelivery::Sent)
    }
}

/// The stub stock-alert notifier: sends are recorded (addresses +
/// item ids — the probe asserts exactly who was told what); a probe
/// can arm a transport FAILURE to prove the arm survives a refused
/// send.
#[derive(Default)]
pub struct StubStockNotifier {
    pub sent: Mutex<Vec<(Uuid, String)>>, // (item_id, to_address)
    pub fail_all: std::sync::atomic::AtomicBool,
}

impl StubStockNotifier {
    /// Make every subsequent send fail at the transport (Err).
    pub fn fail_transport(&self) {
        self.fail_all.store(true, Ordering::SeqCst);
    }

    /// Did one item's alert reach one address?
    pub fn told(&self, item_id: Uuid, address: &str) -> bool {
        self.sent
            .lock()
            .map(|s| s.iter().any(|(i, a)| *i == item_id && a == address))
            .unwrap_or(false)
    }
}

#[async_trait]
impl StockAlertNotifier for StubStockNotifier {
    async fn send_stock_alert(&self, message: &StockAlertMessage<'_>) -> Result<StockAlertDelivery, String> {
        if self.fail_all.load(Ordering::SeqCst) {
            return Err("probe-armed transport failure".into());
        }
        if let Ok(mut sent) = self.sent.lock() {
            sent.push((message.item_id, message.to_address.to_string()));
        }
        Ok(StockAlertDelivery::Sent)
    }
}

/// The stub availability adapter: a programmed free-quantity table per
/// (item, warehouse scope). `None` scope (the company aggregate) and
/// explicit warehouses are separate rows; an unprogrammed item REFUSES
/// (fail-loud, never an implicit zero); a probe can arm a global
/// refusal to prove the fail-closed 503 family.
pub struct StubAvailability {
    table: Mutex<HashMap<(Uuid, Option<Uuid>), Decimal>>,
    refuse_all: std::sync::atomic::AtomicBool,
}

impl StubAvailability {
    /// An empty table — items must be stocked explicitly.
    pub fn new() -> Self {
        Self::default()
    }

    /// Program the AGGREGATE (no-warehouse) free quantity for one item.
    pub fn stock(&self, item_id: Uuid, free: Decimal) {
        self.scoped_stock(item_id, None, free);
    }

    /// Program one (item, warehouse) pair's free quantity.
    pub fn scoped_stock(&self, item_id: Uuid, warehouse: Option<Uuid>, free: Decimal) {
        if let Ok(mut table) = self.table.lock() {
            table.insert((item_id, warehouse), free);
        }
    }

    /// Arm the port-wide refusal (the unwired-port posture).
    pub fn refuse_everything(&self) {
        self.refuse_all.store(true, Ordering::SeqCst);
    }
}

impl Default for StubAvailability {
    fn default() -> Self {
        Self {
            table: Mutex::new(HashMap::new()),
            refuse_all: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

fn availability_refused() -> AvailabilityPortError {
    AvailabilityPortError {
        code: "availability_port_unwired".into(),
        message: "no availability adapter is installed".into(),
    }
}

#[async_trait]
impl AvailabilityReadPort for StubAvailability {
    async fn free_quantity(
        &self,
        _company_id: Uuid,
        item_id: Uuid,
        warehouse_id: Option<Uuid>,
    ) -> Result<ItemAvailability, AvailabilityPortError> {
        if self.refuse_all.load(Ordering::SeqCst) {
            return Err(availability_refused());
        }
        self.table
            .lock()
            .map_err(|_| availability_refused())?
            .get(&(item_id, warehouse_id))
            .map(|free| ItemAvailability {
                item_id,
                free_quantity: *free,
                kit_exploded: false,
            })
            .ok_or_else(availability_refused)
    }

    async fn free_quantities(
        &self,
        company_id: Uuid,
        item_ids: &[Uuid],
        warehouse_id: Option<Uuid>,
    ) -> Result<Vec<ItemAvailability>, AvailabilityPortError> {
        let mut out = Vec::with_capacity(item_ids.len());
        for item_id in item_ids {
            out.push(self.free_quantity(company_id, *item_id, warehouse_id).await?);
        }
        Ok(out)
    }
}

// ── the website-surface / principal stubs (the blog suite's shape) ─────────

pub use backbone_website::exports::normalize_host;

use backbone_website::exports::{
    MenuNode, PublicPage, SessionFacts, SweepSummary, WebsiteError, WebsitePrincipal,
    WebsitePrincipalVerifier, WebsiteResult, WebsiteView,
};

/// The stub website surface: binds ONE host to ONE website view; every
/// other surface verb is a no-op (the storefront only resolves hosts).
pub struct StubSurface {
    pub view: WebsiteView,
}

impl StubSurface {
    pub fn binding(view: WebsiteView) -> Self {
        Self { view }
    }
}

#[async_trait]
impl backbone_website::exports::WebsiteSurface for StubSurface {
    async fn resolve_website_by_host(&self, host: &str) -> WebsiteResult<WebsiteView> {
        if normalize_host(host).eq_ignore_ascii_case(PROBE_HOST) {
            Ok(self.view.clone())
        } else {
            Err(WebsiteError::WebsiteNotResolved)
        }
    }

    async fn visible_page(
        &self,
        _website_id: Uuid,
        _url: &str,
        _principal: Option<&WebsitePrincipal>,
    ) -> WebsiteResult<Option<PublicPage>> {
        Ok(None)
    }

    async fn menu_tree_visible(
        &self,
        _website_id: Uuid,
        _principal: Option<&WebsitePrincipal>,
    ) -> WebsiteResult<Vec<MenuNode>> {
        Ok(Vec::new())
    }

    async fn redirect_answer(
        &self,
        _website_id: Uuid,
        _url: &str,
    ) -> Option<backbone_website::application::service::lang_matcher::RedirectAnswer> {
        None
    }

    async fn record_redirect(
        &self,
        _website_id: Uuid,
        _url_from: &str,
        _url_to: &str,
        _kind: backbone_website::exports::RedirectKind,
    ) -> WebsiteResult<()> {
        Ok(())
    }

    async fn company_allowlist(
        &self,
        _principal: &WebsitePrincipal,
        _website_id: Uuid,
    ) -> Vec<Uuid> {
        Vec::new()
    }

    async fn track_visit(
        &self,
        _website_id: Uuid,
        _session: &SessionFacts<'_>,
        _url: &str,
        _page_key: Option<&str>,
    ) -> WebsiteResult<()> {
        Ok(())
    }

    async fn sweep_visitors(&self) -> WebsiteResult<SweepSummary> {
        Ok(SweepSummary::default())
    }
}

/// The stub bearer verifier: exactly one token verifies to exactly one
/// principal.
pub struct StubVerifier {
    pub token: String,
    pub principal: WebsitePrincipal,
}

impl WebsitePrincipalVerifier for StubVerifier {
    fn verify<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<WebsitePrincipal>> + Send + 'a>>
    {
        Box::pin(async move {
            if token == self.token {
                Some(self.principal.clone())
            } else {
                None
            }
        })
    }
}

// ── seeds (raw rows in the sibling schemas the module reads) ───────────────

/// Insert one website row; return the surface's view of it.
pub async fn seed_website(pool: &sqlx::PgPool, name: &str, company_id: Uuid) -> WebsiteView {
    let id = Uuid::new_v4();
    let public_user_id = Uuid::new_v4();
    // One domainless live website is the whole allowance (the sibling's
    // COALESCE(domain,'') unique) — every seeded site carries its own
    // probe domain derived from its id.
    let domain = format!("{}.probe.test", id.simple());
    sqlx::query(
        r#"
        INSERT INTO website.websites (id, name, domain, company_id, public_user_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(id)
    .bind(name)
    .bind(&domain)
    .bind(company_id)
    .bind(public_user_id)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed website failed: {e}"));
    WebsiteView {
        id,
        name: name.into(),
        domain: Some(domain),
        company_id,
        public_user_id,
        default_lang_code: "en".into(),
        homepage_url: "/".into(),
        robots_txt: None,
        social_links: None,
        contact_recipients: Vec::new(),
        sequence: 10,
    }
}

/// Backdate a cart's `metadata->updated_at` past the abandonment
/// window. The carts BEFORE UPDATE trigger stamps `now()` on every
/// write, so the backdate runs with triggers parked for the scope of
/// ONE transaction (`session_replication_role = replica`, the
/// superuser-owned scratch fixture) — the stamping trigger is the
/// system under test's clock, and the probe rewinds only the clock.
pub async fn backdate_cart(pool: &sqlx::PgPool, cart_id: Uuid, minutes: i64) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('session_replication_role', 'replica', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        r#"
        UPDATE storefront.carts
        SET metadata = jsonb_set(metadata, '{updated_at}',
             to_jsonb(now() - make_interval(mins => $2::int)))
        WHERE id = $1
        "#,
    )
    .bind(cart_id)
    .bind(minutes)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

/// Mint one visitor row (the storefront never mints these — the probe
/// seeds the row the website module would have created); returns
/// (visitor id, access token).
pub async fn seed_visitor(pool: &sqlx::PgPool, website_id: Uuid) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let token = format!("stf-probe-{}", Uuid::new_v4().simple());
    sqlx::query(
        r#"
        INSERT INTO website.visitors (id, website_id, access_token, digest)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(id)
    .bind(website_id)
    .bind(&token)
    .bind(format!("digest-{token}"))
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("seed visitor failed: {e}"));
    (id, token)
}

/// Seed one active delivery carrier owned by the company.
pub async fn seed_carrier(pool: &sqlx::PgPool, company_id: Uuid, name: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO selling.delivery_carriers (company_id, name, active)
        VALUES ($1, $2, true)
        RETURNING id
        "#,
    )
    .bind(company_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("seed carrier failed: {e}"));
    id
}

/// Seed the company's active gateway provider.
pub async fn seed_provider(pool: &sqlx::PgPool, company_id: Uuid) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO payment_gateway.payment_gateway_providers
            (code, company_id, display_name)
        VALUES ('manual', $1, 'probe manual')
        RETURNING id
        "#,
    )
    .bind(company_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("seed provider failed: {e}"));
    id
}

/// One fully-gated merchandised item: catalog stub entry + listing +
/// price row on the website. Returns the item id.
pub async fn seed_listing(
    pool: &sqlx::PgPool,
    catalog: &StubCatalog,
    website_id: Uuid,
    name: &str,
    list_price: Decimal,
    published: bool,
) -> Uuid {
    use backbone_storefront::application::service::audit::ActorRef;
    use backbone_storefront::application::service::catalog_service;
    let item_id = Uuid::new_v4();
    catalog.add(ItemSnapshot {
        item_id,
        status: "active".into(),
        name: name.into(),
        item_group_id: None,
        brand_id: None,
        item_group_name: None,
    });
    let listing_id = catalog_service::upsert_listing(
        pool,
        website_id,
        item_id,
        true,
        10,
        serde_json::json!(["https://cdn.example.test/item.jpg"]),
        ActorRef::system(),
    )
    .await
    .unwrap_or_else(|e| panic!("seed listing failed: {e}"));
    catalog_service::set_price(
        pool,
        website_id,
        item_id,
        list_price,
        None,
        "IDR",
        ActorRef::system(),
    )
    .await
    .unwrap_or_else(|e| panic!("seed price failed: {e}"));
    if published {
        catalog_service::publish_listing(pool, website_id, listing_id, ActorRef::system())
            .await
            .unwrap_or_else(|e| panic!("seed publish failed: {e}"));
    }
    item_id
}

/// Seed one live inventory warehouse owned by the company (the row the
/// collect registry's warehouse fence validates against; requires the
/// probe database booted with `TestDb::new_with_inventory`).
pub async fn seed_warehouse(
    pool: &sqlx::PgPool,
    company_id: Uuid,
    code: &str,
) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO inventory.warehouses (company_id, code, name)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(company_id)
    .bind(code)
    .bind(format!("probe warehouse {code}"))
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("seed warehouse failed: {e}"));
    id
}

/// Seed one merchant-declared pickup location on the website (the
/// officer upsert verb's row, written directly for fixture brevity —
/// the collect probe drives the VERB itself). Returns the location id.
pub async fn seed_pickup_location(
    pool: &sqlx::PgPool,
    website_id: Uuid,
    name: &str,
    warehouse_id: Option<Uuid>,
    country: &str,
    active: bool,
) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        r#"
        INSERT INTO storefront.pickup_locations
            (website_id, warehouse_id, name, country, is_active)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(website_id)
    .bind(warehouse_id)
    .bind(name)
    .bind(country)
    .bind(active)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("seed pickup location failed: {e}"));
    id
}

// ── the composed probe stack (one pool, one stub set, both routers) ────────

use backbone_storefront::presentation::http::{
    storefront_admin_routes, storefront_public_routes, StorefrontAdminState, StorefrontPublicState,
};

/// The whole probe stack over one scratch database: the stub ports and
/// both exported routers, sharing ONE pricing adapter (the spec's
/// one-adapter-two-consumers posture). Holds the TestDb alive for the
/// probe's lifetime (its Drop is the leak-guard, its dispose the happy
/// teardown).
pub struct Probe {
    pub pool: sqlx::PgPool,
    pub company_id: Uuid,
    pub view: WebsiteView,
    pub catalog: Arc<StubCatalog>,
    pub party: Arc<StubParty>,
    pub tax: Arc<StubTax>,
    pub pricing: Arc<StubPricing>,
    pub notifier: Arc<StubNotifier>,
    pub availability: Arc<StubAvailability>,
    pub stock_notifier: Arc<StubStockNotifier>,
    pub public: axum::Router,
    pub admin: axum::Router,
    _db: TestDb,
}

impl Probe {
    /// Boot a probe with a seeded website + the default stub set
    /// (pricing factor 1.0, tax 0.0, availability programmed generous)
    /// and both routers composed.
    pub async fn boot(marker: &str) -> Self {
        Self::over(TestDb::new(marker).await).await
    }

    /// Compose over an existing scratch database (probes that need the
    /// raw TestDb handle first).
    pub async fn over(db: TestDb) -> Self {
        let pool = db.pool.clone();
        let company_id = Uuid::new_v4();
        let view = seed_website(&pool, "Probe Store", company_id).await;
        let catalog = Arc::new(StubCatalog::default());
        let party = Arc::new(StubParty::new());
        let tax = Arc::new(StubTax(Decimal::ZERO));
        let pricing = Arc::new(StubPricing::new(Decimal::ONE, Vec::new()));
        let notifier = Arc::new(StubNotifier::default());
        let availability = Arc::new(StubAvailability::new());
        let stock_notifier = Arc::new(StubStockNotifier::default());
        let surface = Arc::new(StubSurface::binding(view.clone()));
        let public = storefront_public_routes(StorefrontPublicState::compose(
            pool.clone(),
            surface,
            catalog.clone(),
            party.clone(),
            tax.clone(),
            pricing.clone(),
            availability.clone(),
        ));
        let mut admin_state = StorefrontAdminState::new(pool.clone());
        admin_state.install_party_port(party.clone());
        admin_state.install_notifier(notifier.clone());
        admin_state.install_catalog_port(catalog.clone());
        admin_state.install_availability_port(availability.clone());
        admin_state.install_stock_notifier(stock_notifier.clone());
        let admin = storefront_admin_routes(admin_state);
        Self {
            pool,
            company_id,
            view,
            catalog,
            party,
            tax,
            pricing,
            notifier,
            availability,
            stock_notifier,
            public,
            admin,
            _db: db,
        }
    }

    /// Explicit teardown: drop the scratch database.
    pub async fn dispose(self) {
        self._db.dispose().await;
    }
}

// ── HTTP helpers (axum oneshot — no server, no sockets) ────────────────────

use axum::body::Body;
use axum::http::{header, Request, StatusCode};

/// Fire one request at a router; return (status, body bytes).
pub async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, PROBE_HOST);
    if let Some(token) = token {
        builder = builder.header("x-storefront-token", token);
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    use tower::ServiceExt;
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

/// Fire one request carrying BOTH identities: the visitor token
/// header AND a verified-principal bearer (the reconciled shopper
/// shape — reconcile, arm, and the union read ride this pair).
pub async fn send_dual(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    bearer: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, PROBE_HOST);
    if let Some(token) = token {
        builder = builder.header("x-storefront-token", token);
    }
    if let Some(bearer) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let request = match body {
        Some(json) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    use tower::ServiceExt;
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, bytes)
}

/// A POST with the visitor token AND the principal bearer.
pub async fn post_dual(
    app: &axum::Router,
    path: &str,
    token: Option<&str>,
    bearer: Option<&str>,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = send_dual(app, "POST", path, token, bearer, Some(body)).await;
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// A GET with the probe visitor token (the common shopper shape).
pub async fn get(app: &axum::Router, path: &str, token: Option<&str>) -> (StatusCode, Vec<u8>) {
    send(app, "GET", path, token, None).await
}

/// A POST with a JSON body.
pub async fn post(
    app: &axum::Router,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = send(app, "POST", path, token, Some(body)).await;
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

// ── table checksums (the mutating-GET harness's proof) ─────────────────────

/// A whole-table checksum: row count + md5 over every row's text
/// representation, ordered by id (any write flips it).
pub async fn table_checksum(pool: &sqlx::PgPool, table: &str) -> (i64, String) {
    let row: (i64, String) = sqlx::query_as(&format!(
        r#"
        SELECT count(*)::bigint,
               coalesce(md5(string_agg(t::text, '' ORDER BY t.id)), 'empty')
            AS digest
        FROM {table} t
        "#,
    ))
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("checksum over {table} failed: {e}"));
    row
}

/// Every table the module can write, plus the sibling identity tables
/// the mutating-GET harness must prove untouched.
pub const CHECKSUMMED_TABLES: &[&str] = &[
    "storefront.carts",
    "storefront.cart_lines",
    "storefront.checkout_sessions",
    "storefront.product_listings",
    "storefront.product_prices",
    "storefront.website_sale_settings",
    "storefront.shopper_parties",
    "storefront.recovery_invites",
    "storefront.storefront_audit_log",
    "storefront.pickup_locations",
    "storefront.wishlist_items",
    "website.websites",
    "website.visitors",
    "selling.sales_orders",
    "selling.sales_order_items",
    "payment_gateway.gateway_transactions",
];
