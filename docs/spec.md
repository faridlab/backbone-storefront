# backbone-storefront — the module specification (server-authoritative eCommerce core)

This document is the build contract for the `backbone-storefront` module: every
decision in it is frozen. An implementer executes this file without re-deciding
anything; where a choice was deliberately left to a later increment, the file
says so explicitly and names the increment's re-entry condition. The upstream
reference is the Odoo website eCommerce cohort (`docs/odoo/website/ecommerce/`
in this repository, register IDs EC-1..28 + EC-1b/EC-3b/EC-27b + EC-R1..18);
every deviation from upstream behavior is recorded with its reason. The
governing product-side records are the website pillar section
`docs/plan/08-pillar-website.md` §WB-6 (:174-198) and DoD (:253-278), the W8
pass ledger row P1 (`docs/plan/w8-passes.md:17`), the register floor
(`docs/plan/w8-register-deltas.md` §WB-6 :62-126 + the inheritance rows
:56-59), and the P0 council's design gate C7 with its chair clarification
(`docs/council/2026-09-03-module-w8-p0-open.md:61`).

Identity, in one line each:

- **Crate** `backbone-storefront`, version `0.1.0`, Postgres **schema
  `storefront`**, HTTP mount base **`/api/v1/storefront`** (verified
  unoccupied in the host tree — §0; the module itself mounts nothing — the
  host nests the exported routers). Naming ruling: the P0 council, the ruled
  pass map, and the register's STOREFRONT-TRUST row all name this module "the
  storefront"; the register cohort header's transcribed module label
  `ecommerce` (`w8-register-deltas.md:65-66`) is the Odoo addon name, not a
  naming ruling — superseded here, the register rows keep their EC prefixes
  either way.
- **A NEW module consuming the existing export surface — NOT a selling
  increment** (council condition C7, binding on this spec). The cart +
  website + promo glue lives HERE, never inside selling. Zero deviations are
  taken: the P0 export census found no order-model gap, and this spec found
  none either (§1.4 records the two places a selling change was considered
  and rejected).
- **Consumed exports** (each verified first-hand at the pinned tags; the full
  census with anchors is §1): selling's write verbs + the cart-pricing port
  DTOs; website's exported `WebsiteSurface` trait; portal's `PortalUserId`
  principal type; payment-gateway's transaction create + verified-settle
  verbs. Promo is consumed ONLY through selling's `CartPricingPort` as
  composed by the host's `PromoCartPricingAdapter` — never directly.
- **Two hard exclusions** (P3's, not ours): `SellingWriteService::with_sink`
  (`selling_write_service.rs:419` — the sale-seam arming hook, DIT #304) is
  never called by this module or its host wiring; the events intake router
  `event_intake_routes` (`public_routes.rs:136-140`, "mounted by the host
  only when the funnel arms") is never nested by this pass — the chair
  clarification recorded with C7: it is the P3 funnel.
- **Company fence**: none declared on the storefront's own tables (ADR-0014
  posture 4, the website posture — the security axis is the verb surface plus
  the identity binding). Every table carries `website_id`; the company is
  derived through `website.websites.company_id` (NOT NULL,
  `website_service.rs:56`). Selling's own company fence (ADR-0008,
  `company_scope::bind_company_on` at `selling_order.rs:75-77`) governs every
  order write.
- **No durable events**: the module stages no outbox rows and subscribes to
  none. The settlement→confirm leg is a host-side consumer over the gateway's
  existing `GatewayTransactionSettled` seam event (§7.4), not a module bus.
- **Zero crons**: abandoned carts are a derived read; recovery is an explicit
  verb (§8). The host's `outbox_schemas` sets get NO `storefront` entry.

Family conventions this module follows verbatim (verified against
backbone-website v0.1.0 and backbone-blog v0.1.0):

- Schema/model YAML is the source of truth; every hand-written file is
  declared under `user_owned:` in `metaphor.codegen.yaml` in the same change
  that lands it.
- Migrations carry NO `GRANT`s (owner-role DDL). The composing host re-runs
  `apps/serpa-service/scripts/rls_app_role.sql` as owner after
  `metaphor migration run-all` (the dev-migration grant rider, DIT #250).
- Cross-module references are LOGICAL: indexed uuid columns with
  `@exclude_from_foreign_key_check`, never a `FOREIGN KEY` constraint across
  schema boundaries. References that stay INSIDE schema `storefront` are real
  FKs.
- The clippy bar is a gate-time CLI flag:
  `cargo clippy --all-targets -- -D clippy::expect_used` → EXIT=0.
- Probes are fail-hard on the scratch Postgres (`127.0.0.1:5433`,
  `postgres`/`postgres`, overridable); a missing scratch server panics the
  suite. The live dev database on 5432 is never touched by tests.

---

## 0. Pre-flight gates (run before any schema work; exit codes quoted)

1. `grep -c "/api/v1/storefront" apps/serpa-service/src/main.rs` → `0`
   (verified 2026-09-04; the nest is absent).
2. No migration in the module family creates schema `storefront`
   (`grep -rln "CREATE SCHEMA.*storefront" modules/*/migrations/` → no
   output; verified 2026-09-04).
3. The four consumed trees are tag-equal and clean at the pinned refs:
   selling `v0.9.1`, website `v0.1.0`, portal `v0.2.0`, payment-gateway
   `v0.3.3` (the P0 hygiene result; drift heals per DIT #307, never by
   re-tagging).
4. `metaphor schema undeclared` on the host → 0 (the standing hygiene bar).

---

## 1. Module boundary (the C7 design gate) — the consumed-export census

### 1.1 The ruling

C7 (council record :61, reproduced in the P1 row `w8-passes.md:17`): the
storefront is a NEW module consuming the existing export surface — NOT a
selling increment; a selling increment is admissible ONLY where this spec
names an order-model gap the P0 export census shows missing. Grounds:
the host seam `apps/serpa-service/src/infrastructure/seams/promo_cart.rs:23-26`
reserves the priced-cart mount for a consumer outside
selling ("no HTTP verb mounts `create_sales_order_priced` yet; the adapter is
composed and held ready … it is the only shape a priced cart consumer may
mount"), and the module cross-domain rule bans cart+website+promo glue inside
selling. **This spec takes zero deviations** — every consumed capability
below exists today at the cited anchor.

### 1.2 The consumed exports (all anchors verified first-hand at the pinned tags)

| export | anchor | what the storefront does with it |
|---|---|---|
| `SellingWriteService::create_sales_order_priced` | `modules/backbone-selling/src/application/service/selling_order.rs:122` | Mints the cart's sales order at place time with promo-resolved per-line nets (§7.2). THE reserved consumer the census names. |
| `SellingWriteService::confirm_sales_order` | `selling_order.rs:270` | The paid-heal and free-cart confirm driver (§7.4, §7.5). |
| `SellingWriteService::cancel_sales_order` | `selling_order.rs:559` | The checkout-cancel mirror (§7.6). |
| `SellingWriteService::sales_order_ref` | `selling_order.rs:523` | Order-state reads for checkout views. |
| `CartPricingPort` + `CartPriceRequest`/`PricedCart` DTOs | `selling_cart_pricing.rs:29-35` (request), `:74-76` (trait) | The whole pricing wire. The storefront builds `CartPriceRequest` (company/customer/group/coupon/lines) and reads `PricedCart` for both display reads and the place-time mint. |
| promo `resolve_cart` — consumed ONLY through the host adapter | `modules/backbone-promo/src/application/service/promo_cart.rs:395` (impl), composed at host `apps/serpa-service/src/infrastructure/seams/promo_cart.rs:38-106` | Never a direct edge. The adapter is the only meeting place (the host seam's reservation, `seams/promo_cart.rs:23-26`). |
| `WebsiteSurface` trait | `website_surface.rs:33-63` | `resolve_website_by_host` (public-request binding), `company_allowlist` (principal-scoped reads), `track_visit` (session heartbeat). Website's own Cargo.toml anticipates exactly this consumption ("storefront surfaces consume THIS module's exported trait surface"). |
| `WebsitePrincipal` / `WebsitePrincipalVerifier` | `principal_port.rs:15-18` | The authenticated-principal arm of cart identity (§2). |
| website publish contract | W8-SEAM-CONTRACT clause 3: `PUBLISH_FENCED_FIELDS` `page_service.rs:33`, `publish` :324 / `unpublish` :346 | The precedent this spec's product-publish verbs copy (§4.2): publish fields excluded from every patch whitelist, explicit verbs the only writers. |
| `PortalUserId` | `backbone-portal/src/exports/types.rs:263` | The principal key on carts (website uses it in `WebsitePrincipal` and does not re-export it — the storefront names the type, hence the portal edge). |
| `GatewayTransactionService` create (the plain-CRUD tx path) | `gateway_transaction_service.rs:28-33` | Creates the PENDING gateway transaction row at place, INSIDE the checkout row lock (§7.3) — the checkout-side lock that does not exist today is added HERE, around this call. |
| `GatewayWriteService::settle_transaction` / `settle_by_provider_tx_verified` | `gateway_write_service.rs:148` / `:386` | NOT called by the storefront — the webhook ingest + host ACL own settling. Read-side dependency: the money gate (`authority_gross != recorded gross` → `InvalidMoney`, `:425-431`) is what the row lock makes hold. |
| `GatewayTransactionSettled` seam event → host ACL | `gateway_events.rs` → host `seams/gateway_settlement.rs` (status-aware dedup, exactly-once post) | The settlement→confirm consumer rides the SAME existing consumption the P0 recipe names for OrderPaid (§7.4). |

### 1.3 Edges the module does NOT take

- No edge to promo (the adapter is host-composed; the forbidden
  selling→promo edge stays forbidden storefront→promo for the same reason).
- No edge to events (nothing of events is consumed this pass; the intake
  router exclusion above).
- No edge to catalog, party, or tax: reads/writes on those go through
  module-defined ports the HOST wires (the POS posture — "a composing service
  wires … behind its ports"). The storefront defines `CatalogReadPort` (item
  identity/status/uom/group reads), `PartyWritePort` (guest-party bootstrap +
  billing-party mint), and `TaxResolvePort` (company + jurisdiction → tax
  template + rate). This keeps the sibling edges at exactly four (§10.4).

### 1.4 The deviation record (two selling changes considered and rejected, one admitted)

1. **Per-line tax partitioning through the priced-cart port.** Promo's
   `PriceQuery.tax_key` is an opaque per-line tax-group partition key
   (`promo_ports.rs:39-42`) and the host adapter passes `tax_key: None`
   (`promo_cart.rs:73-76`, "a missing key means single-group allocation").
   Surfacing it would require adding a field to selling's
   `CartPriceRequest` — a selling increment. REJECTED: selling's order model
   carries exactly one order-level `tax_rate`
   (`NewCartSalesOrder.tax_rate`, `selling_write_service.rs:135`;
   `sales_order.model.yaml:80-85`), so a per-line tax split would EXCEED the
   order model — that is an order-model change, not a gap the census missed.
   Fiscal resolution therefore lands order-level (§5.4).
2. **An update/re-price verb on existing priced orders.** Cart re-pricing
   before place is a pure port read; the priced ORDER is minted once at place
   (§7.2). Selling needs no new verb for either.
3. **The carrier field on `NewCartSalesOrder` — ADMITTED (the one
   sanctioned selling increment).** The shopper picks a delivery carrier
   before placing; `set_delivery` stamps it on the cart (§6) and the mint
   must carry it onto the order or the choice is silently dropped at place.
   `NewSalesOrder` already carries `delivery_carrier_id` (validated against
   the company's carrier registry at create), so the priced create path
   accepting the same field is not a new selling CAPABILITY — it is the
   plain and priced create paths converging on one validation shape. This
   is the narrow form of the gate's escape hatch (a selling increment is
   admissible where this spec records why the storefront cannot own the
   behavior): the carrier is selling master data, validated by selling's
   registry verbs, and a storefront-side carrier column would fork that
   registry. The increment rides selling's `v0.9.2` release; the storefront
   passes `cart.delivery_carrier_id` at the mint (§7.2) and the place probe
   asserts the carrier lands on the minted order.

---

## 2. Identity and the cart lifecycle (EC-15/EC-25 anti-spec; EC-R13)

### 2.1 The identity ladder

A cart is bound to exactly one REAL identity, which is one of:

- **Session identity** — a website visitor row (`website.visitors`, per-website,
  random 32-byte base64url access token, GDPR GC + erasure verbs already
  built). The storefront NEVER mints visitor rows (EC-21): the token arrives
  from the webapp's existing website session; product and cart reads only
  READ by token. The one minting verb is `POST /public/cart` (§6), which
  requires an already-existing visitor token and refuses otherwise (typed
  401, never a silent mint).
- **Authenticated identity** — a portal principal verified through website's
  `WebsitePrincipalVerifier` (`principal_port.rs`), i.e. `PortalUserId` +
  email. Login does not silently move carts (§2.3).

### 2.2 Deterministic create

`POST /public/cart` creates the identity's open cart:

- The open-cart invariant is a PARTIAL UNIQUE: `UNIQUE(visitor_id) WHERE
  state = 'open'` (§3.4). Concurrent creates race to exactly one winner
  (`INSERT … ON CONFLICT DO NOTHING` then select the surviving row) — no
  `limit=1` without ordering (EC-15's adoption shape), no check-then-act.
- No GET ever creates a cart. The cart read returns `204`/empty for a
  token with no cart — the webapp calls the create verb explicitly.

### 2.3 Deterministic adopt (login reconciliation, the EC-R13 anti-spec inverted)

At login bind (`POST /public/session/bind`), the storefront ANSWERS A QUERY
and the client ADOPTS EXPLICITLY:

- The bind verb verifies the portal principal against the CURRENT visitor
  token and returns the principal's most recent open cart — selected with an
  explicit total order (`state='open' AND portal_user_id = $principal ORDER
  BY updated_at DESC, id DESC LIMIT 1`) — plus that cart's ownership proof
  token. It mutates nothing.
- `POST /public/cart/adopt` is the only mover: it re-binds that cart to the
  current visitor lineage AND stamps `portal_user_id`, refusing with a typed
  409 when the current visitor already holds an open cart (the partial
  unique enforces it — no silent merge, no arbitrary abandoned-draft
  adoption: a cart is adoptable ONLY through its `portal_user_id` linkage,
  never through "some draft exists").
- A cart owned by a DIFFERENT live identity is never returned, never
  adoptable (probe §13.4).

### 2.4 The cart's storage grain (the mapping decision)

The pillar words it "the cart is a draft selling order bound to a real
identity" (`08-pillar-website.md:179-181`). This spec implements that as a
LIFECYCLE equivalence, not a storage identity:

- Cart state (identity binding, lines, coupon, carrier, checkout link) lives
  in `storefront.carts` / `storefront.cart_lines`.
- The `selling.sales_orders` row is minted ONCE, at place, by
  `create_sales_order_priced` (§7.2) — the exact reserved consumer shape
  (`promo_cart.rs:23-26`). Before place there IS no order row.

Why (recorded per the spec-discipline rule): upstream's abandoned-cart
problem class (EC-11/EC-15/EC-25) exists precisely because carts ARE order
rows there — anonymous browsing consumes order numbers, sudo'd resolvers
adopt arbitrary drafts, and lifecycle flags rot. Minting selling rows only
at place (a) keeps selling's number space clean, (b) makes the priced-cart
port's create verb the one and only order entry, and (c) makes every cart
read a server re-derivation (ADR-0022) over storefront-owned rows. The
floor rows' normative content — deterministic create/adopt, identity-bound,
no arbitrary adoption, no session-key cross-layer contract — holds verbatim
(EC-25's "9 session keys" die: the ONLY client-held secret is the visitor
token; quantities live server-side only).

---

## 3. Table set (schema `storefront`)

Timestamps/actors ride the shared `Metadata` jsonb block (the family shape).
All cross-schema refs are LOGICAL (uuid, indexed,
`@exclude_from_foreign_key_check`); intra-schema refs are real FKs.

### 3.1 `storefront.product_listings` — the per-website publish pairing (EC-1, EC-R5)

| column | type | notes |
|---|---|---|
| `id` | uuid PK | |
| `website_id` | uuid NOT NULL | FK `storefront.websites_ref`? no — LOGICAL ref `website.websites(id)`, indexed |
| `item_id` | uuid NOT NULL | LOGICAL ref `catalog.items(id)`, indexed |
| `sale_ok` | bool NOT NULL DEFAULT false | the merchandising sale-eligibility flag (catalog carries none — verified: `item.model.yaml` has status only, `:146-149`) |
| `is_published` | bool NOT NULL DEFAULT false | writable ONLY through the publish/unpublish verbs (§4.2), excluded from every patch whitelist |
| `sequence` | int NOT NULL DEFAULT 10 | hand-set ordering (EC-6's race-prone computed ribbon/sequence dies) |
| `media_urls` | jsonb NOT NULL DEFAULT `'[]'` | ordered array of `{url, alt?}` — webapp/object-storage URLs only; a `data:` URI is REJECTED at write validation (EC-22) |
| `metadata` | jsonb | shared audit block |

Partial unique (live rows): `UNIQUE(website_id, item_id)`.

### 3.2 `storefront.product_prices` — the per-website price rows (EC-1b)

`id` PK, `website_id` NOT NULL (logical), `item_id` NOT NULL (logical),
`list_price` decimal(18,2) NOT NULL non-negative, `compare_at_price`
decimal(18,2) NULL (the merchandising compare-at display; NULL = not shown),
`currency` char(3) NOT NULL DEFAULT `'IDR'`, `metadata`. Partial unique
(website_id, item_id) live. THE base-price source: no module in the tree
owns a product list price (catalog: "NO prices" — `item.model.yaml:7-8`), so
the per-website pricelist's BASE arm lands here; rules/discounts on top stay
promo's (§5).

### 3.3 `storefront.website_sale_settings` — one row per website

`id` PK, `website_id` NOT NULL UNIQUE (logical), `access_gate` enum
`storefront_access_gate` NOT NULL DEFAULT `'open'` (`open` | `members_only`
— EC-R6), `default_customer_group_id` uuid NULL (the per-website pricing
segment, §5.2), `guest_party_id` uuid NOT NULL (the designated public
customer party anonymous carts ride — minted by the settings bootstrap
verb through `PartyWritePort`; the website `public_user_id` bootstrap
pattern), `recovery_template_ref` text NULL (the per-website recovery
message template reference, EC-12), `metadata`.

### 3.4 `storefront.carts` — the cart spine

| column | type | notes |
|---|---|---|
| `id` | uuid PK | |
| `website_id` | uuid NOT NULL | logical ref `website.websites(id)` |
| `visitor_id` | uuid NOT NULL | logical ref `website.visitors(id)` — the session lineage |
| `portal_user_id` | uuid? | logical ref `portal.portal_users(id)` — stamped at explicit adopt/bind |
| `party_id` | uuid? | logical ref `party.parties(id)` — the billing identity the placed order will ride; NULL until billing capture (§6) |
| `state` | `storefront_cart_state` | `open` \| `placed` \| `closed` \| `cancelled` |
| `coupon_code` | text? | the presented code (case-folded); applied only via the POST verb (EC-R16) |
| `delivery_carrier_id` | uuid? | logical ref selling's carriers; set only under the row lock (§7.1) |
| `placed_at` | timestamptz? | |
| `metadata` | jsonb | |

Partial unique: `UNIQUE(visitor_id) WHERE state = 'open'` (§2.2). Index
`(portal_user_id) WHERE state = 'open'`, `(website_id, state)`.
**This row is the checkout row-lock target** (§7.1).

### 3.5 `storefront.cart_lines`

`id` PK, `cart_id` uuid NOT NULL FK `storefront.carts(id)` CASCADE,
`item_id` uuid NOT NULL (logical), `quantity` decimal(18,4) NOT NULL
positive, `metadata`. Index `(cart_id)`. No stored prices — every read
re-derives through the port (ADR-0022; EC-7's fused-render pricing dies).

### 3.6 `storefront.shopper_parties` — the deterministic email→party map (EC-23)

`id` PK, `company_id` uuid NOT NULL, `email_normalized` text NOT NULL,
`party_id` uuid NOT NULL (logical), `metadata`.
`UNIQUE(company_id, email_normalized)` — resolution is
`INSERT … ON CONFLICT DO NOTHING RETURNING`, on conflict SELECT by the
unique key: race-free, no check-then-act, no string-containment placeholder
detection (the map IS the record; parties created through it are first-class
customers, never "placeholders"). Deliberately scoped to the storefront's
own map — no sudo-search over all parties. (party's own `party_emails`
carries a global `UNIQUE(email)` — `party_email.model.yaml:61-65` — noted as
the upstream fact; the storefront does not rely on it for dedup.)

### 3.7 `storefront.checkout_sessions` — the place record

`id` PK, `cart_id` uuid NOT NULL FK CASCADE, `website_id` NOT NULL,
`sales_order_id` uuid? (logical — stamped by the place verb),
`gateway_transaction_id` uuid? (logical ref gateway transactions),
`provider_code` text?, `provider_reference` text? (`stf-{checkout_id}` — the
storefront-minted reference the provider reports back),
`amount_total` decimal(18,2) NOT NULL (the locked final total),
`state` `storefront_checkout_state` (`pending_payment` | `confirmed_free` |
`settled` | `failed` | `cancelled`), `placed_at`, `settled_at`, `metadata`.
`UNIQUE(gateway_transaction_id)`. Index `(cart_id)`.

### 3.8 `storefront.recovery_invites` — the explicit recovery record (EC-12/13)

`id` PK, `cart_id` uuid NOT NULL FK CASCADE, `template_ref` text NOT NULL
(the per-website template used), `notified_at` timestamptz?,
`delivery_state` text NOT NULL (`pending`|`sent`|`unwired`), `metadata`.
Audit-stamp only — eligibility NEVER derives from these rows (§8.3).

### 3.9 `storefront.storefront_audit_log`

The portal-shape audit vocabulary: `cart_created`, `cart_adopted`,
`line_added`, `line_updated`, `line_removed`, `coupon_applied`,
`coupon_removed`, `billing_set`, `delivery_set`, `cart_placed`,
`checkout_confirmed_free`, `checkout_settled_confirmed`, `cart_cancelled`,
`recovery_sent`, `listing_published`, `listing_unpublished`, `price_set`,
`settings_set`. Append-only, service-emitted.

### 3.10 RLS posture — none declared

`rowsecurity = false` on every `storefront.*` table (ADR-0014 posture 4).
Cart/checkout reads are gated by the identity binding at the service layer
(visitor token or verified portal principal); officer verbs sit behind the
host's `company_auth` + `ModuleWriteGate::new(pool, "storefront")`.

---

## 4. Publish-gated product reads (PRICING-PUBLISH, EC-R5; EC-21/EC-22; EC-6/EC-R6)

### 4.1 The ONE domain contract

A product is storefront-visible on website W iff:

```
storefront.product_listings(website_id = W, item_id) live row exists
  AND sale_ok = true
  AND is_published = true
  AND catalog item status = 'active' (via CatalogReadPort)
```

This is a CODE contract in the read verbs (EC-R5's port shape — "the ACTUAL
catalog boundary … in code"), not an ACL rule. Upstream's EC-1 observation
(publish state scattered across mixins) inverts into: the website publish
PAIRING for products lives in exactly one table (`product_listings`), written
only by its verbs. The product entity is NOT enrolled in website's
generic-vs-specific resolver — that fold is page-scoped by design
(`specificity.rs:1-22` documents the ONE resolver; its row shape is
`ResolvedPage`, `:67-80`; the resolver imports only `Page` — verified), and
this spec does not enroll it: the per-website grain here is the listing row
itself.

### 4.2 Publish verbs (the website publish contract copied)

`publish` / `unpublish` are the ONLY writers of `is_published`
(`PUBLISH_FENCED_FIELDS` posture, `page_service.rs:33` precedent):
`is_published` is excluded from every generic patch whitelist (typed 422 +
audit `publish_refused` on attempt). Publish NEVER couples from any other
write — no category onchange (EC-5/EC-R9 dead), no media write, no settings
save.

### 4.3 Read-surface closures

- **No visitor mint on reads** (EC-21): catalog/category/cart reads never
  INSERT into `website.visitors` — probe-asserted by row count (§13.5).
- **No data-URI smuggling** (EC-22): `media_urls` validation rejects
  `data:` URIs at write; unpublished/other-website listings return NO media
  on any read (structural — the media live on the listing row that the gate
  already filtered).
- **No variant-flooding surface** (EC-3): the public catalog read serves
  stored catalog rows only; no public verb generates variant combinations.
- **Server-side sort vocabulary** (EC-27b): listing sort accepts exactly
  `relevance` | `newest` | `price_asc` | `price_desc` | `name_asc` — a
  validated enum mapped to fixed SQL order arms, never a formatted string.
- **B2B access gate** (EC-6/EC-R6, the port-vs-fence call): PORTED MINIMAL —
  `website_sale_settings.access_gate = 'members_only'` makes every public
  catalog/cart/checkout verb require a verified portal principal (401
  otherwise). No sitemap exists this pass to suppress; when one lands, the
  gate must suppress it too (recorded as the landing condition).

---

## 5. Pricing — the website-to-port dimension mapping (the pass's decision step)

### 5.1 The port's shape (what must be mapped)

`CartPriceRequest` carries `company_id`, `customer_id?`, `customer_group_id?`,
`coupon_code?`, `lines[]` (`selling_cart_pricing.rs:29-35`). There is NO
website dimension and NO per-line tax key — the census anchor this decision
owns. Promo underneath matches rules on company + item/group/brand +
customer/customer_group + coupon (`promo_ports.rs:17-47`,
`pricing_rule.model.yaml`).

### 5.2 The mapping (frozen)

| port field | resolution | why |
|---|---|---|
| `company_id` | `website.company_id` (via `WebsiteSurface::resolve_website_by_host` → `WebsiteView.company_id`, `website_service.rs:56`) | The website→company pairing is total and stored (NOT NULL); it is the only company a website's cart may book. |
| `customer_id` | the cart's `party_id` once billing is captured; before that `None` (the guest party is NOT passed — promo rules must not key on a shared synthetic customer) | Keeps anonymous pricing honest; the guest party exists for the ORDER's customer column, not for rule matching. |
| `customer_group_id` | **resolution order**: the billing party's explicit segment (set at billing capture from the shopper-parties map / portal linkage) ELSE `website_sale_settings.default_customer_group_id` (the per-website pricing segment) | This is how "per-website pricelist" lands WITHOUT a promo increment: the website's default segment rides the port's EXISTING group dimension (a real promo rule dimension — `PriceQuery.customer_group_id`, `promo_ports.rs:32`). Two websites with different segments price the same catalog differently; promo stays website-free. |
| `coupon_code` | the cart's presented code | Straight through; redemption stays promo's (`commit_coupon_redemption` at order commit — selling's priced path already rides this). |
| `list_price` per line | `storefront.product_prices(website_id, item_id).list_price` | §3.2 — the base arm of the per-website pricelist. |
| `tax_key` | **not set — stays `None`** | §1.4 rejection #1: surfacing it is a selling port-DTO change (banned increment); single-group allocation is the adapter's documented behavior (`promo_cart.rs:73-76`). |

### 5.3 Fiscal resolution (order-level, by construction)

The storefront resolves ONE tax template per cart through `TaxResolvePort`
(company tax settings + the delivery jurisdiction from the captured billing
address) and applies its rate as `NewCartSalesOrder.tax_rate` — the order
model's own single-rate grain (`selling_write_service.rs:135`). No
fiscal-position entity exists anywhere in the tree (verified: party,
billing, tax schemas carry none), and creating one is NOT this module's
move — recorded as the named re-entry: a fiscal-position model, when
product demands it, lands in the tax domain with its own spec.

### 5.4 Cache posture (EC-8, EC-3b, EC-R15)

Promo resolves FRESH on every call — no cache exists in promo (verified:
no cache module in its tree). The posture this spec freezes:

- P1 adds NO price-resolution cache. Every cart read re-derives through the
  port (ADR-0022; consistent with the computed-fresh family posture).
- Any FUTURE cache over resolved prices MUST be keyed per company (the
  website's company is always in the key — never implicit, resolving EC-3b's
  implicit-website-cache-key class structurally) and invalidated per company
  on pricing-config writes — whole-registry/global clearing is banned
  (EC-8/EC-R15's thundering-herd lever named dead in advance).

### 5.5 The B2B gate's pricing arm

Under `access_gate = 'members_only'`, anonymous pricing reads 401 before the
port call — no pricing oracle for walled stores.

---

## 6. Routes — the full verb table (CART-CHECKOUT, ADR-0019; EC-17/EC-27)

Every mutation is a **POST**. No GET writes anything — the mutating-GET
harness (§11.3) proves it per route. The public tree nests BARE (its own
gates: hostname binding via `resolve_website_by_host`, identity token or
principal, per-identity + per-IP fixed-window throttles on the write verbs —
the website public posture). The admin tree nests behind the host's
`company_auth` + `ModuleWriteGate::new(pool, "storefront")`.

### 6.1 Public routes (all under `/api/v1/storefront`)

| method + path | contract (one line) |
|---|---|
| `GET /public/catalog` | Publish-gated product listing for the bound website (paged; closed sort vocabulary §4.3; pure read). |
| `GET /public/catalog/{item_id}` | Publish-gated product detail incl. price row + media refs; unpublished/other-website → typed 404; no visitor mint. |
| `GET /public/categories` | Category tree derived FRESH from published listings' catalog groups (computed per read; no stored category state). |
| `GET /public/cart` | The token's open cart + freshly port-resolved pricing (subtotal, per-line nets, order discounts); ZERO writes on any path (no line purging — EC-17/EC-R8's cart-page GET purge is dead). |
| `POST /public/cart` | Create the identity's open cart (deterministic §2.2; requires an existing visitor token; idempotent per identity). |
| `POST /public/cart/lines` | Add a line (validates the §4.1 gate + a live price row exist; typed refusals otherwise). |
| `POST /public/cart/lines/{line_id}` | Set a line's quantity (positive decimal; typed refusal on the §4.1 gate failing at mutation time — never a silent unlink). |
| `POST /public/cart/lines/{line_id}/remove` | Remove a line. |
| `POST /public/cart/coupon` | Apply a coupon code (case-folded; the ONLY code surface — a POST, EC-R16; no code search/listing exists). |
| `POST /public/cart/coupon/remove` | Clear the coupon. |
| `POST /public/cart/billing` | Capture billing identity: deterministic shopper-parties resolve-or-create (§3.6), stamp `party_id`, and RE-PRICE EXPLICITLY in the same verb (EC-R10's silent ripple inverted — the response carries the new totals; a fiscal re-resolution may change them). |
| `POST /public/cart/delivery` | Set the delivery carrier — validated against the company's carriers (the `carrier_id_or_refuse` posture, `selling_order.rs:68-72` — clean 404, never an FK 500) — UNDER the cart row lock (§7.1). |
| `POST /public/session/bind` | Verify the portal principal against the current visitor lineage; ANSWERS the adoptable-cart query; mutates nothing (§2.3). |
| `POST /public/cart/adopt` | Explicitly adopt the principal's own prior open cart (typed 409 when the visitor already holds one; foreign carts unadoptable). |
| `POST /public/checkout` | PLACE (§7): the row-locked critical section — final port price → mint the priced sales order → pending gateway tx OR free-confirm. |
| `GET /public/checkout/{checkout_id}` | Checkout + order state view (derived read; zero writes). |
| `POST /public/express` | Express checkout (EC-23): one verb = deterministic billing capture + place; same locks, same refusals. |
| `GET /public/abandoned` | The identity's derived abandoned carts (§8; own carts only — never another identity's). |
| `POST /public/cart/{cart_id}/recover` | Recovery re-bind of the identity's OWN abandoned cart to the current session (explicit; ownership-checked; typed refusals). |

### 6.2 Admin/officer routes (behind `company_auth` + the write gate)

| method + path | contract |
|---|---|
| `GET /admin/listings` | Officer listing read (all states) for the company's websites. |
| `POST /admin/listings` | Create/update a listing row (sale_ok, sequence, media; `is_published` NOT patchable here — typed 422). |
| `POST /admin/listings/{id}/publish` | The publish verb (§4.2; the only `is_published=true` writer). |
| `POST /admin/listings/{id}/unpublish` | The unpublish verb. |
| `POST /admin/prices` | Set the per-website price row (list + compare-at + currency). |
| `GET /admin/settings/{website_id}` | Read the website's sale settings. |
| `POST /admin/settings/{website_id}` | Set settings (segment, gate, recovery template; bootstraps the guest party on first set via `PartyWritePort`; per-website row — EC-R18's every-website fan-out is dead by grain). |
| `GET /admin/abandoned-carts` | Officer derived read (computed fresh §8; company-scoped). |
| `POST /admin/abandoned-carts/{cart_id}/send-recovery` | The explicit recovery send (per-website template honored EC-12; eligibility computed fresh EC-13; delivery via the host notifier port — unwired = typed visible `unwired` state, never silent). |
| `GET /admin/checkouts` | Checkout reads for officer/support views. |

---

## 7. Checkout: the row lock and the payment pipeline (EC-24, CART-CHECKOUT)

### 7.1 The row lock (the NEW work the census names)

`GatewayTransactionService` is a plain `GenericCrudService`
(`gateway_transaction_service.rs:28-33`) — the substrate's guarded
transitions + exactly-once settle exist on the SETTLE side only; the
tx-CREATION path has no discipline of its own. This module adds it:

- **Locked row**: `storefront.carts` (the checkout's own spine row).
- **Taken**: `SELECT … FOR UPDATE` at the top of the critical sections of
  BOTH `POST /public/checkout` (place) and `POST /public/cart/delivery`
  (delivery change) — the two verbs that can change what the payment
  charges. Also inside the lock scope: `POST /public/express` (it IS a
  place) and `POST /public/cart/billing` (fiscal re-resolution can move the
  tax arm).
- **Released**: at transaction end (commit/rollback) — lock scope is one
  DB transaction, no cross-request locks.
- **Why no unlocked delivery-change window remains** (EC-24's exact
  complaint): a concurrent delivery change either (a) completes before
  place takes the lock — place re-prices under the lock and records the
  FINAL total on the checkout session and the gateway tx row — or (b)
  blocks until place commits, then finds `state='placed'` and takes the
  typed 409 refusal. There is no interleaving in which a tx row exists with
  a total that a subsequent carrier change can invalidate.
- **Belt-and-braces**: the gateway's verified settle money gate
  (`authority_gross != recorded gross` → `InvalidMoney`,
  `gateway_write_service.rs:425-431`) re-checks the locked amount against
  the provider's authority numbers at settle time.

### 7.2 Place (the priced-order mint)

Inside the cart row lock, one DB transaction:

1. Re-derive final pricing through `CartPricingPort::price_cart` (§5.2
   mapping; `at = now`).
2. Resolve the fiscal rate (§5.3).
3. `create_sales_order_priced(NewCartSalesOrder{…}, pricing)` — the order is
   born DRAFT with promo-conserved totals (`selling_order.rs:122`;
   `NewCartSalesOrder` at `selling_write_service.rs:125-138`; customer =
   the captured billing party — billing capture is REQUIRED before place
   for paid carts, typed 409 otherwise).
4. `state='placed'`, `placed_at=now()`, checkout session row with
   `amount_total` = the locked total.

### 7.3 Payment arm (paid carts)

- Provider selection: the company's active provider row (payment-gateway
  providers are company-scoped). The per-WEBSITE provider pivot is FENCED to
  P4 (WM-R15's row) — recorded, not designed here.
- The pending gateway transaction row is created via
  `GatewayTransactionService` create INSIDE the lock, with
  `provider_transaction_id = 'stf-{checkout_id}'` (the storefront-minted
  reference the provider will report — the settle path resolves rows by
  `(provider_code, provider_transaction_id)`, `gateway_write_service.rs:357-365`),
  `gross_amount = amount_total`, `company_id = website.company_id`.
- The webapp renders the provider's payment UX from the provider config +
  reference (QRIS string / VA number / redirect URL — provider-side data the
  host exposes through the gateway provider read); the storefront itself
  never talks to the provider outbound (the gateway's codecs are
  NOTIFICATION codecs — `gateway_codecs.rs:192-240` — there is no outbound
  initiate port in the substrate, and none is invented here).

### 7.4 Settlement → confirm (the paid heal)

Settling stays the substrate's: verified webhook → ingest →
`settle_by_provider_tx_verified` (money-gated, exactly-once transition) →
`GatewayTransactionSettled` → the host ACL posts the payment entry
(`seams/gateway_settlement.rs`). THIS pass adds the storefront's host-side
settlement consumer on the same existing consumption the P0 recipe names for
OrderPaid: when a settled tx's id matches a checkout session, the consumer
(1) stamps the session `settled`, (2) calls `confirm_sales_order` on the
session's order — idempotent: the gateway's transition gate settles once,
and selling's confirm refuses non-draft orders (`NotDraft`), so a redelivery
can never double-confirm. The order stays DRAFT+awaiting-payment until
settlement — an UNPAID order is never auto-confirmed (the pay-on-site
posture, WSC-25, cited forward to its P2 row).

### 7.5 Free arm

`amount_total == 0`: no gateway row; the place verb calls
`confirm_sales_order` directly and stamps `confirmed_free`. (The order was
already minted priced — the free arm differs only in skipping the payment
wait.)

### 7.6 Cancel mirror

`cancel_sales_order` when a placed-but-unsettled checkout is cancelled
(shopper cancel verb or officer void); the checkout session stamps
`cancelled`; a later settlement for a cancelled checkout lands on the
gateway's existing reversal path (`reverse_transaction`,
`gateway_write_service.rs:264`) — recorded as the operator's reconciliation
verb, not an automatic refund.

---

## 8. Abandoned carts (EC-14/EC-16; EC-11/EC-R11/EC-R12/EC-13)

### 8.1 Derived read — nothing stored, nothing cron'd

A cart is "abandoned" iff `state='open' AND updated_at < now() -
interval '<STOREFRONT_ABANDONED_AFTER_HOURS> hours'`, computed FRESH at
read (one tz-aware `now()` per query — EC-11's naive-utcnow and per-call
recomputation drift die). No stored flag, no cron flips anything, zero
cron rows exist for this module.

### 8.2 ONE delay constant

`STOREFRONT_ABANDONED_AFTER_HOURS` (default `1`) — the single knob; EC-16's
10.0/1.0/0.0 divergence is impossible by construction (one constant, one
read site). Declared in BOTH env templates (the standing rider).

### 8.3 Recovery is explicit, ordered, and never permanently flagged

- The derived read is ORDERED (`updated_at DESC, id DESC`) — EC-14's
  first-match-wins over unordered `search()` dies.
- Shopper recovery: `POST /public/cart/{cart_id}/recover` — ownership-checked
  (the cart's own visitor lineage or portal linkage), re-binds to the
  current session.
- Officer recovery: `POST /admin/abandoned-carts/{cart_id}/send-recovery` —
  one explicit cart, the per-website template (EC-12 fixed), eligibility
  computed FRESH per call: a shopper who gains an email later still
  qualifies (EC-13 fixed — `recovery_invites.notified_at` is an audit
  stamp, never an eligibility input).
- No batch cron, no hardcoded fallback template, no try/except-aborts-batch
  shape (EC-R12's three defects all die with the batch itself).

---

## 9. Installs inert + the GMC fence (INSTALLS-INERT; EC-9/EC-10/EC-19/EC-20/EC-22; owner ruling)

### 9.1 Inert by construction

The module HAS no install/uninstall hooks: nothing deactivates another
module's rules (EC-9/EC-R4 dead), nothing flips DB-wide defaults or group
grants (EC-10/EC-R7 dead), nothing widens account/tax model reads
(EC-19 dead — the storefront reads tax templates through its own port
company-scoped, and mounts no public account surface). The first officer
verbs (settings/prices/listings) are the ONLY state-changing entry points,
each an explicit authenticated POST.

### 9.2 The GMC feed — FENCED (owner ruling 2026-09-04)

Register fence row text (for the register seat to transcribe verbatim):

> **website_sale_gmc_feed — FENCED (owner ruling 2026-09-04): on-demand,
> nothing ported in the eCommerce core pass.** Re-entry spec must carry:
> Tier A token + rotation + rate limit (EC-20's bearer-forever public
> consteq shape banned — pillar :192-193), unpublished-product data-URI
> closure (EC-22, pillar :194-196), per-website availability posture
> (WSC-R5's companions override), and the feed's own trust-table row.
> Pillar anchor: `08-pillar-website.md:194-196`.

Nothing in this module renders feeds, mints feed tokens, or serves
`/gmc.xml`-shaped routes.

---

## 10. Migrations, crons, config, Cargo.toml

### 10.1 Migration list (raw-SQL runner order; names + one-liners are the contract)

1. `001_storefront_core_schema` — the nine tables + enums
   (`storefront_cart_state`, `storefront_checkout_state`,
   `storefront_access_gate`; created UNQUALIFIED in `public`, prefixed,
   census-checked).
2. `002_storefront_hardening` — partial uniques: live listing
   `(website_id, item_id)`, live price `(website_id, item_id)`, open cart
   `(visitor_id) WHERE state='open'`, `shopper_parties (company_id,
   email_normalized)`, `checkout_sessions (gateway_transaction_id)`,
   settings `(website_id)`.
3. `003_storefront_audit_triggers` — `created_at`/`updated_at` stamping
   (the portal `add_audit_triggers` shape).

Standing rider: after `metaphor migration run-all`, the host re-runs
`apps/serpa-service/scripts/rls_app_role.sql` AS OWNER or the app role is
locked out of schema `storefront` (DIT #250's lesson).

### 10.2 Crons — none

Zero cron rows; zero `posture:` declarations needed (absence is the
truthful posture). Visitor GC stays website's own job.

### 10.3 Config knobs (host-declared in BOTH `deployment/.env.dev.example`
AND `deployment/.env.prod.example` — compose fails fast)

- `STOREFRONT_ABANDONED_AFTER_HOURS` (default `1`) — the ONE abandonment
  delay (§8.2).
- `STOREFRONT_MAX_CART_LINES` (default `100`) — per-cart line bound (typed
  422 on exceed; a cheap honest DoS bound beside the edge throttles).

### 10.4 Cargo.toml dependency block semantics

- Framework: `backbone-core` (postgres), `backbone-orm`, `backbone-auth`,
  `backbone-rate-limit`, `backbone-messaging` — all at git tag `v2.7.11`
  (single-rev policy).
- Sibling edges — exactly four, each justified by a NAMED type (§1):
  `backbone-selling` tag `v0.9.1` (write verbs + port DTOs),
  `backbone-website` tag `v0.1.0` (`WebsiteSurface`/`WebsitePrincipal`),
  `backbone-portal` tag `v0.2.0` (`PortalUserId`),
  `backbone-payment-gateway` tag `v0.3.3` (tx create DTOs + settle
  types). promo/events/catalog/party/tax: NO edge (ports + host adapters,
  §1.3) — recorded in-comment so no later seat adds them casually (the
  website precedent).
- Host compose: a `seams/storefront_compose.rs` mirroring
  `website_compose.rs` — public router nested bare at
  `/api/v1/storefront`, admin router behind `company_auth` + the write
  gate, the settlement→confirm consumer beside the existing gateway ACL
  consumer, the three ports (`CatalogReadPort`, `PartyWritePort`,
  `TaxResolvePort`) wired over the host's module handles, and the priced
  cart handed the SAME composed `PromoCartPricingAdapter`
  (`seams/promo_cart.rs:38`) — one adapter instance, two consumers
  (selling-side tests + the storefront), never a second mapping.

---

## 11. The STOREFRONT-TRUST surface table (opened this pass; the harness seat fills the rulings)

### 11.1 The surfaces this pass exposes

| # | surface | input trust question | ruling |
|---|---|---|---|
| T-1 | Public catalog/category reads (§4) | may a query string bypass the publish gate or mint identity? | OPEN — harness |
| T-2 | Public cart verbs (§6.1) | which fields of a cart mutation may the client pin (never prices/identity)? | OPEN — harness |
| T-3 | Billing capture + shopper-parties resolution (§3.6) | email → party: what may the client assert (name only, never ids)? | OPEN — harness |
| T-4 | Coupon apply (POST) | code enumeration oracle? (uniform refusals — the WSC-45 lesson named forward) | OPEN — harness |
| T-5 | Place / express (§7) | amount integrity (client never sends an amount), lock coverage | OPEN — harness |
| T-6 | Settlement→confirm consumer (§7.4) | webhook-driven; what does it trust (only the substrate's settled row)? | OPEN — harness |
| T-7 | Recovery verbs (§8.3) | ownership proof; notify port unwired posture | OPEN — harness |
| T-8 | Officer merchandising verbs (§6.2) | write-gate scope; publish-field patch ban | OPEN — harness |

### 11.2 Ruling axes (ADR-0022)

Per surface: who re-derives (server), what the client payload may pin,
mutation verb + method, throttle posture, and the ADR-0022 citation. The
close pass completes the table per surface (pillar DoD :264-265).

### 11.3 The mutating-GET harness (NEW work, on the composed-router probe pattern)

A host-side probe that, for EVERY route in §6: issues the GET forms with
malicious/ambient query params (cart-purge shapes, coupon-redeem shapes,
carrier-switch shapes, pricelist-flip shapes — the EC-17/EC-27 census
classes), then asserts byte-identical table state (row counts + checksums
over `storefront.*`, `website.visitors`, `selling.sales_orders`,
`promo.*`, gateway tx rows) — zero writes on every GET, EXIT=0. This is
the harness the P0 census named absent; it lands with this module and
outlives it (P2/P3 routes extend the same harness).

---

## 12. Register dispositions (the P1 floor rows this spec owns)

Every row of `w8-register-deltas.md` §WB-6 (:78-126) + the four inheritance
rows (:56-59). Dispositions here are the SPEC's; the audited State flip
happens at the pass's register re-audit.

| flag | disposition in this spec |
|---|---|
| EC-1 | **ported-as-contract** — §3.1/§4.1: the publish pairing is ONE table + a code contract (the mixin scatter dies) |
| EC-1b | **ported-minus-ribbons** — §3.1/§3.2/§6.1: merchandising surface = listing + price (base/compare-at) + derived category tree; media = webapp URLs; NO ribbons M2M; variants are catalog's own (`item_variant.model.yaml`) |
| EC-2 | **inverted** — §3/§10.1: real partial uniques everywhere (the constraint-mindset default dies) |
| EC-3 | **not-ported-by-decision** — §4.3: no public variant-combination surface exists |
| EC-3b | **resolved-structurally** — §5.4: no implicit cache key exists; any future cache keys the company explicitly |
| EC-4 / EC-R8 | **inverted** — §6.1: no purge path; gate failures at mutation time are typed refusals; the cart-page GET purge is doubly dead (§11.3) |
| EC-5 / EC-R9 | **dead** — §4.2: publish never couples from any other write |
| EC-6 | **not-ported** — no ribbons; sequence is hand-set (§3.1) |
| EC-7 | **inverted** — §3.5/§6.1: no stored line prices; every read re-derives server-side; rendering never prices |
| EC-8 / EC-R15 | **declared** — §5.4: computed fresh; future caches per-company keyed + invalidated, global clearing banned |
| EC-9 / EC-10 / EC-19 / EC-R4 / EC-R7 | **dead-by-construction** — §9.1: no install hooks exist at all |
| EC-11 / EC-R11 | **ported-corrected** — §8.1: derived lifecycle, one tz-aware instant, fresh per read |
| EC-12 | **fixed** — §3.3/§8.3: per-website template honored on the explicit verb |
| EC-13 | **fixed** — §8.3: eligibility computed fresh; notified_at is audit-only |
| EC-14 | **dead** — §8.3: ordered reads + explicit per-cart verbs (no first-match-wins anywhere) |
| EC-15 / EC-25 / EC-R13 | **the anti-spec, honored** — §2: partial-unique create, explicit adopt, one client secret (the visitor token), no arbitrary drafts |
| EC-16 | **fixed** — §8.2: ONE constant, ONE read site |
| EC-17 / EC-27 | **dead** — §6 + §11.3: all mutations POST; the harness proves every GET |
| EC-18 | **not-ported** — no public config route; category/merchandising options are officer settings behind the gate (§6.2) |
| EC-20 / EC-R17 | **fenced-with-the-feed** — §9.2 (owner ruling; landing conditions recorded) |
| EC-21 | **dead** — §4.3 + probe §13.5: reads never touch the visitor table |
| EC-22 | **closed** — §4.3: `data:` URIs rejected at write; unpublished rows return no media on any read |
| EC-23 | **inverted** — §3.6/§6.1: deterministic map resolution, no placeholder detection, no check-then-act |
| EC-24 | **ported-closed** — §7.1: the cart row lock covers place AND delivery/billing; no unlocked window |
| EC-26 | **dead** — no carousel engine; the paged listing endpoints are the whole merchandising read (the webapp composes) |
| EC-27b | **ported** — §4.3: closed sort vocabulary, server-mapped order arms |
| EC-28 | **dead** — no external RPC exists; no failure-degrading silence possible |
| EC-R1 | **ported** — §4.1 (the read gate IS the domain contract) |
| EC-R2 | **not-ported** — §6.1: categories derive from published listings over catalog's own tree; no bypass_access SQL |
| EC-R3 | **not-ported-by-decision** — §5: no global rule replacement exists; scoping is native (website→company) |
| EC-R5 | **ported** — §4.1 (the ONE code contract) |
| EC-R6 | **ported-minimal** — §3.3/§4.3 (access_gate; sitemap-suppression named as the landing condition) |
| EC-R10 | **inverted** — §6.1: billing set = one explicit verb, repoint + re-price atomically |
| EC-R12 | **dead-with-the-batch** — §8.3 |
| EC-R14 | **not-ported** — no combo UI quantity-clamp recursion; bundle nets arrive as the port's reward lines (§7.2); clamps are typed refusals |
| EC-R16 | **dead** — §6.1: the coupon POST is the only code surface; no hidden-field search exists |
| EC-R18 | **dead-by-grain** — §6.2: settings are per-website rows; no set_values fan-out |
| CART-CHECKOUT | **owned** — §6 + §7 |
| PRICING-PUBLISH | **owned** — §4 + §5 |
| INSTALLS-INERT | **owned** — §9.1 |
| STOREFRONT-TRUST | **opened** — §11 (skeleton + axes; rulings land with the harness seat + the close pass) |

---

## 13. The probe suite (fail-hard; every gate a verified exit code)

1. **Schema gates**: `metaphor schema generate --force` EXIT=0, byte-stable
   re-run, `metaphor lint check` EXIT=0; `metaphor schema undeclared` → 0
   on the host after compose.
2. **Deterministic create/adopt** (§2): N concurrent `POST /public/cart`
   for one identity → exactly one open cart; adopt-refusal family (foreign
   cart, already-open visitor) → typed 409s; an abandoned cart of ANOTHER
   identity is returned to no one and adoptable by no one.
3. **The row lock** (§7.1): concurrent `delivery` + `checkout` pairs
   against one cart → every trial ends either fully-old-carrier or
   fully-new, the checkout's `amount_total` always equals the minted
   order's total, and the gateway tx gross equals both; zero torn trials.
4. **Publish gate** (§4): unpublished / other-website / `sale_ok=false` /
   inactive-item listings → typed 404 on detail and absent from listing;
   `is_published` patch attempts → typed 422 + `publish_refused` audit row;
   only the verbs flip it.
5. **No-mint reads** (EC-21): catalog/cart GETs leave `website.visitors`
   row-count byte-identical.
6. **Mutating-GET harness** (§11.3): every §6 GET route, adversarial param
   family, full-table checksums → zero writes, EXIT=0.
7. **Pricing mapping** (§5.2): two websites, different default segments,
   same catalog → different nets through the SAME adapter; guest carts pass
   `customer_id=None`; billing capture re-prices exactly once, in-verb.
8. **Coupon discipline**: apply is POST-only; GET forms never redeem;
   uniform refusal text (no enumeration oracle).
9. **Express determinism** (EC-23): parallel expresses with one email →
   exactly one `shopper_parties` row, one party, one order.
10. **Settle→confirm** (§7.4): simulated verified settlement for the
    `stf-` reference → order confirmed exactly once across a redelivered
    webhook (transition gate + `NotDraft` double guard).
11. **Free arm** (§7.5): zero-total place → `confirmed_free`, no gateway
    row; unpaid paid-carts NEVER confirm (assert `draft` after place).
12. **Abandoned** (§8): derived read flips exactly at the one constant;
   zero cron rows exist for the module; recovery eligibility survives a
   later-gained email (no permanent flag).
13. **Installs inert** (§9.1): module bring-up writes zero rows outside
   schema `storefront` (checksum probe over sibling schemas).
14. **Exclusions**: `event_intake_routes` absent from the composed host
   router (route-absence probe); zero references to `with_sink` /
   `SellingEventSink` in the module and its compose seam (grep gate,
   EXIT=0 on absence — the DIT #304 boundary).
15. **Live wire (dev, after the train)**: `/health` 200;
   `/api/v1/storefront/admin/…` unauth → 401; the guest flow
   browse → cart → place → settle-simulated → confirmed, proven through
   the API (the pillar's probe-driven posture :264-267).
16. **Riders**: migrations as owner + `rls_app_role.sql` as owner; app
   role USAGE on schema `storefront` verified; both env templates carry
   both knobs; clippy `-D clippy::expect_used` EXIT=0; pin probe —
   `backbone-storefront` resolves exactly once, host-declared, tag-equal
   with ls-remote at the tagged cut; version == tag verified inside the
   tag-cut step.

---

*End of specification. The build seat scaffolds the module tree around this
file's contract; the harness seat fills the §11 rulings and drives §13; the
register seat transcribes §9.2's fence row and re-audits §12 by ID. Nothing
outside §2–§10 is built before its named increment.*
