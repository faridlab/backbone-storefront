-- Migration: storefront hardening — the partial unique fences.
--
-- Hand-written DDL (the family posture: the generator emits plain tables;
-- the partial uniques are the DB fences the spec's invariants lean on).
-- Every constraint keys on the LIVE scope ((metadata->>'deleted_at') IS
-- NULL) exactly like the sibling modules' hardening migrations.
--
-- NO GRANTs — owner-role DDL; the composing host re-runs
-- apps/serpa-service/scripts/rls_app_role.sql as owner after the
-- migration runner (the dev-migration grant rider).

-- One live listing row per (website, item) — the publish pairing's grain.
CREATE UNIQUE INDEX IF NOT EXISTS idx_product_listings_website_item_live
    ON storefront.product_listings (website_id, item_id)
    WHERE (metadata->>'deleted_at') IS NULL;

-- One live price row per (website, item) — the base-price lookup's grain.
CREATE UNIQUE INDEX IF NOT EXISTS idx_product_prices_website_item_live
    ON storefront.product_prices (website_id, item_id)
    WHERE (metadata->>'deleted_at') IS NULL;

-- ONE OPEN CART PER VISITOR — the deterministic-create invariant: concurrent
-- creates race to exactly one winner through this constraint (no
-- check-then-act anywhere in the verbs).
CREATE UNIQUE INDEX IF NOT EXISTS idx_carts_open_per_visitor
    ON storefront.carts (visitor_id)
    WHERE state = 'open' AND (metadata->>'deleted_at') IS NULL;

-- The adopt query's total order is backed by an index: the principal's most
-- recent open cart resolves (updated_at DESC, id DESC) without a scan.
CREATE INDEX IF NOT EXISTS idx_carts_open_per_portal_user
    ON storefront.carts (portal_user_id, (metadata->>'updated_at') DESC, id DESC)
    WHERE state = 'open' AND (metadata->>'deleted_at') IS NULL;

-- One live party binding per (company, normalized email) — the deterministic
-- shopper-party resolution's ON CONFLICT target.
CREATE UNIQUE INDEX IF NOT EXISTS idx_shopper_parties_company_email_live
    ON storefront.shopper_parties (company_id, email_normalized)
    WHERE (metadata->>'deleted_at') IS NULL;

-- One live checkout session per gateway transaction — the settlement
-- consumer's resolution key (NULL rows are excluded by the predicate).
CREATE UNIQUE INDEX IF NOT EXISTS idx_checkout_sessions_gateway_tx_live
    ON storefront.checkout_sessions (gateway_transaction_id)
    WHERE gateway_transaction_id IS NOT NULL AND (metadata->>'deleted_at') IS NULL;

-- One live settings row per website.
CREATE UNIQUE INDEX IF NOT EXISTS idx_website_sale_settings_website_live
    ON storefront.website_sale_settings (website_id)
    WHERE (metadata->>'deleted_at') IS NULL;

-- The abandoned-carts derived read (state='open' AND stale updated_at) —
-- computed fresh per query; the index serves the per-website officer read.
CREATE INDEX IF NOT EXISTS idx_carts_open_updated_at
    ON storefront.carts (website_id, (metadata->>'updated_at'))
    WHERE state = 'open' AND (metadata->>'deleted_at') IS NULL;
