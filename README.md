# backbone-storefront

The website storefront module: public catalog + cart + checkout verbs and the
officer merchandising/recovery verbs that serve a website's shop. It is a NEW
module that CONSUMES the existing export surface — it is not a selling
increment, it never mounts the events intake router, and it never touches
selling's alternate event-destination constructor (the composition boundary:
this module arms nothing on the events side).

Authoritative spec: `docs/spec.md`.

## The four surfaces

1. **Checkout row lock** — every checkout-side mutation (billing capture,
   delivery change, place, the free confirm) runs under
   `SELECT ... FOR UPDATE` on `storefront.carts`. The gateway transaction
   service stays plain CRUD: the lock, not the gateway, serializes the
   checkout. Concurrent places against one cart admit exactly one winner;
   the winner's locked total equals the minted order's total and the gateway
   transaction's gross/net (one total, three witnesses).
2. **Publish-gated product reads** — a listing is publicly visible only when
   `sale_ok` AND `is_published` AND the catalog item is live AND a live price
   row exists. The gate is enforced in the domain (the same predicate guards
   the public reads and re-checks at cart-mutation time), not in a route
   filter. Every closed door answers the same typed 404 body (no oracle).
3. **Per-website pricing** — prices resolve through the `CartPricingPort`
   (selling's pricing contract) per website customer segment; totals are
   conserved (`unit = round(list × factor)`, `net = round(unit × qty)`,
   `total = Σ net`). A billing capture re-prices in-verb: the response
   carries the new arm's price, never the caller's stale view.
4. **Abandoned carts as derived reads** — nothing is stored. The read flips
   at ONE delay constant (`STOREFRONT_ABANDONED_AFTER_HOURS`, default 1h)
   computed from the cart's last touch. Recovery is an explicit officer verb
   with the per-website template honored (no hardcoded fallback) and
   eligibility re-computed fresh per call.

## Route trees

- `presentation/http/public_routes.rs` — the public tree (catalog reads,
  cart verbs, checkout). The host nests it bare of company_auth at
  `/api/v1/storefront`; visitor identity rides the website visitor token.
- `presentation/http/admin_routes.rs` — the officer tree (settings,
  listings, prices, abandoned reads, recovery, checkout reads). The host
  mounts it behind company_auth with the module-write gate keyed
  `storefront`.

Every mutating verb is POST. Checkout GETs never mutate (probe-verified:
table checksums identical before/after the whole adversarial GET family).

## Composition knobs

| Knob | Default | Meaning |
|---|---|---|
| `STOREFRONT_ABANDONED_AFTER_HOURS` | 1 | Hours since last touch at which the derived abandoned-carts read flips |
| `STOREFRONT_MAX_CART_LINES` | 100 | Per-cart line cap the add-line verb enforces |

Both are declared in the workspace deployment contracts
(`deployment/.env.dev.example`, `apps/serpa-service/.env.prod.example`).

## Host wiring (the compose seam — documented, not implemented here)

The module exports ports, not adapters: `CatalogReadPort`, `PartyWritePort`,
`TaxResolvePort`, `CartPricingPort` (re-exported from selling),
`RecoveryNotifier`, plus the website `WebsiteSurface` and principal-verifier
bindings. The composing host owns one seam file (the blog/website precedent:
`seams/storefront_compose.rs` in the host service) that:

- builds `StorefrontPublicState::compose(pool, website_surface, catalog,
  party, tax, pricing)` for the public router and
  `StorefrontAdminState` for the officer router;
- installs its own adapters over its module handles (catalog = the product
  module, party = sapiens/parties, tax = the tax module, pricing = selling's
  CartPricingPort, notifier = the mail composition);
- mounts the settlement consumer (`consume_settlement`) on the payment
  gateway's `GatewayTransactionSettled` event — the module exposes the
  consumer; the host owns the subscription.

Nothing here mounts the events intake router or arms an event sink.

## Migrations

Three hand-consolidated files (regen never re-emits per-table migrations
into this tree): core schema, hardening partial uniques, audit triggers.
No GRANTs — the composing host re-runs its RLS app-role grant script after
the migration runner (owner-role DDL posture shared with the siblings).

## Tests

```bash
cargo test -p backbone-storefront
```

The probe suite mints a disposable scratch database per test on the scratch
Postgres (default `postgres://postgres:postgres@127.0.0.1:5433/postgres`,
override `STOREFRONT_TEST_ADMIN_URL`), applies this module's migrations
plus the website/selling/payment-gateway sibling migrations, and fails hard
when the scratch server is missing — no vacuous skips. Probe batteries:
identity determinism, the row lock, the publish gate, per-website pricing,
coupon discipline, express determinism, mutating-GET immutability, free vs
paid arms, settlement idempotence, abandonment derivation, install-time
inertness, and the exclusion scans (no events edge, no intake route, no
alternate event-destination literals in the source).
