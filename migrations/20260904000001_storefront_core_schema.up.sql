-- =============================================================================
-- storefront — core schema (migration 1 of 3: 001_storefront_core_schema).
--
-- Consolidated by hand from the generator's per-table DDL (same statements,
-- one file) so the migration contract stays exactly THREE named files:
--   001_storefront_core_schema    — enums, schema, nine tables, indexes, FKs
--   002_storefront_hardening      — partial unique indexes (the DB fences)
--   003_storefront_audit_triggers — metadata stamping triggers
--
-- SHAPE NOTES (source: docs/spec.md §3 / §10.1):
--  - Cross-schema refs (website_id, item_id, party_id, portal_user_id,
--    sales_order_id, gateway_transaction_id, ...) are LOGICAL: plain indexed
--    uuid columns, never FOREIGN KEY across schema boundaries (the workspace
--    posture; no cross-module FK exists anywhere in the tree).
--  - Intra-schema refs are REAL FKs added as trailing ALTER TABLE ... ON
--    DELETE CASCADE statements: cart_lines.cart_id, checkout_sessions.cart_id,
--    recovery_invites.cart_id -> carts(id). A cart's lines, checkouts, and
--    recovery stamps are the cart's own state.
--  - NO GRANTs here: the host re-runs its role/RLS grant script as owner
--    after applying migrations (permission denials otherwise, per the
--    workspace runbook).
--  - No RLS policies: the module declares company_fence: none; visibility is
--    the verb surface (publish gate + identity binding), not row fencing.
-- =============================================================================

-- -----------------------------------------------------------------------------
-- ENUM TYPES
-- -----------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_audit_event') THEN
        CREATE TYPE storefront_audit_event AS ENUM ('cart_created', 'cart_adopted', 'line_added', 'line_updated', 'line_removed', 'coupon_applied', 'coupon_removed', 'billing_set', 'delivery_set', 'cart_placed', 'checkout_confirmed_free', 'checkout_settled_confirmed', 'cart_cancelled', 'recovery_sent', 'listing_upserted', 'listing_published', 'listing_unpublished', 'publish_refused', 'price_set', 'settings_set');
    END IF;
END
$$;

-- Create storefront_cart_state enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_cart_state') THEN
        CREATE TYPE storefront_cart_state AS ENUM ('open', 'placed', 'closed', 'cancelled');
    END IF;
END
$$;

-- Create storefront_checkout_state enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_checkout_state') THEN
        CREATE TYPE storefront_checkout_state AS ENUM ('pending_payment', 'confirmed_free', 'settled', 'failed', 'cancelled');
    END IF;
END
$$;

-- Create storefront_access_gate enum type
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_access_gate') THEN
        CREATE TYPE storefront_access_gate AS ENUM ('open', 'members_only');
    END IF;
END
$$;

-- -----------------------------------------------------------------------------
-- TABLE: carts
-- -----------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_cart_state') THEN
        CREATE TYPE storefront_cart_state AS ENUM ('open', 'placed', 'closed', 'cancelled');
    END IF;
END
$$;

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.carts (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID NOT NULL,
    visitor_id UUID NOT NULL,
    portal_user_id UUID,
    party_id UUID,
    state storefront_cart_state NOT NULL DEFAULT 'open',
    coupon_code TEXT,
    delivery_carrier_id UUID,
    placed_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_carts_website_id_state ON storefront.carts (website_id, state);

CREATE INDEX IF NOT EXISTS idx_carts_visitor_id ON storefront.carts (visitor_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_carts_metadata_gin ON storefront.carts USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_carts_metadata_deleted_at ON storefront.carts ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_carts_metadata_created_at ON storefront.carts ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_carts_metadata_updated_at ON storefront.carts ((metadata->>'updated_at'));

-- -----------------------------------------------------------------------------
-- TABLE: cart_lines
-- -----------------------------------------------------------------------------

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.cart_lines (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    cart_id UUID NOT NULL,
    item_id UUID NOT NULL,
    quantity NUMERIC(18, 4) NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_cart_lines_cart_id_item_id ON storefront.cart_lines (cart_id, item_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_cart_lines_metadata_gin ON storefront.cart_lines USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_cart_lines_metadata_deleted_at ON storefront.cart_lines ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_cart_lines_metadata_created_at ON storefront.cart_lines ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_cart_lines_metadata_updated_at ON storefront.cart_lines ((metadata->>'updated_at'));

-- Inline foreign key constraints (forward + self refs)
ALTER TABLE storefront.cart_lines ADD CONSTRAINT fk_cart_lines_cart_id FOREIGN KEY (cart_id) REFERENCES storefront.carts (id) ON DELETE CASCADE;

-- -----------------------------------------------------------------------------
-- TABLE: checkout_sessions
-- -----------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_checkout_state') THEN
        CREATE TYPE storefront_checkout_state AS ENUM ('pending_payment', 'confirmed_free', 'settled', 'failed', 'cancelled');
    END IF;
END
$$;

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.checkout_sessions (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    cart_id UUID NOT NULL,
    website_id UUID NOT NULL,
    sales_order_id UUID,
    gateway_transaction_id UUID,
    provider_code TEXT,
    provider_reference TEXT,
    amount_total NUMERIC(18, 2) NOT NULL CHECK (amount_total >= 0),
    state storefront_checkout_state NOT NULL DEFAULT 'pending_payment',
    placed_at TIMESTAMPTZ,
    settled_at TIMESTAMPTZ,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_checkout_sessions_cart_id ON storefront.checkout_sessions (cart_id);

CREATE INDEX IF NOT EXISTS idx_checkout_sessions_website_id_state ON storefront.checkout_sessions (website_id, state);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_checkout_sessions_metadata_gin ON storefront.checkout_sessions USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_checkout_sessions_metadata_deleted_at ON storefront.checkout_sessions ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_checkout_sessions_metadata_created_at ON storefront.checkout_sessions ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_checkout_sessions_metadata_updated_at ON storefront.checkout_sessions ((metadata->>'updated_at'));

-- Inline foreign key constraints (forward + self refs)
ALTER TABLE storefront.checkout_sessions ADD CONSTRAINT fk_checkout_sessions_cart_id FOREIGN KEY (cart_id) REFERENCES storefront.carts (id) ON DELETE CASCADE;

-- -----------------------------------------------------------------------------
-- TABLE: product_listings
-- -----------------------------------------------------------------------------

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.product_listings (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID NOT NULL,
    item_id UUID NOT NULL,
    sale_ok BOOLEAN NOT NULL DEFAULT FALSE,
    is_published BOOLEAN NOT NULL DEFAULT FALSE,
    sequence INTEGER NOT NULL DEFAULT 10,
    media_urls JSONB NOT NULL DEFAULT '[]'::jsonb,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_product_listings_website_id_is_published_sequence ON storefront.product_listings (website_id, is_published, sequence);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_product_listings_metadata_gin ON storefront.product_listings USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_product_listings_metadata_deleted_at ON storefront.product_listings ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_product_listings_metadata_created_at ON storefront.product_listings ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_product_listings_metadata_updated_at ON storefront.product_listings ((metadata->>'updated_at'));

-- -----------------------------------------------------------------------------
-- TABLE: product_prices
-- -----------------------------------------------------------------------------

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.product_prices (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID NOT NULL,
    item_id UUID NOT NULL,
    list_price NUMERIC(18, 2) NOT NULL CHECK (list_price >= 0),
    compare_at_price NUMERIC(18, 2),
    currency TEXT NOT NULL DEFAULT 'IDR',
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_product_prices_website_id_item_id ON storefront.product_prices (website_id, item_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_product_prices_metadata_gin ON storefront.product_prices USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_product_prices_metadata_deleted_at ON storefront.product_prices ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_product_prices_metadata_created_at ON storefront.product_prices ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_product_prices_metadata_updated_at ON storefront.product_prices ((metadata->>'updated_at'));

-- -----------------------------------------------------------------------------
-- TABLE: recovery_invites
-- -----------------------------------------------------------------------------

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.recovery_invites (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    cart_id UUID NOT NULL,
    template_ref TEXT NOT NULL,
    notified_at TIMESTAMPTZ,
    delivery_state TEXT NOT NULL DEFAULT 'pending',
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_recovery_invites_cart_id ON storefront.recovery_invites (cart_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_recovery_invites_metadata_gin ON storefront.recovery_invites USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_recovery_invites_metadata_deleted_at ON storefront.recovery_invites ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_recovery_invites_metadata_created_at ON storefront.recovery_invites ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_recovery_invites_metadata_updated_at ON storefront.recovery_invites ((metadata->>'updated_at'));

-- Inline foreign key constraints (forward + self refs)
ALTER TABLE storefront.recovery_invites ADD CONSTRAINT fk_recovery_invites_cart_id FOREIGN KEY (cart_id) REFERENCES storefront.carts (id) ON DELETE CASCADE;

-- -----------------------------------------------------------------------------
-- TABLE: shopper_parties
-- -----------------------------------------------------------------------------

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.shopper_parties (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    company_id UUID NOT NULL,
    email_normalized TEXT NOT NULL,
    party_id UUID NOT NULL,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_shopper_parties_company_id_email_normalized ON storefront.shopper_parties (company_id, email_normalized);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_shopper_parties_metadata_gin ON storefront.shopper_parties USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_shopper_parties_metadata_deleted_at ON storefront.shopper_parties ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_shopper_parties_metadata_created_at ON storefront.shopper_parties ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_shopper_parties_metadata_updated_at ON storefront.shopper_parties ((metadata->>'updated_at'));

-- -----------------------------------------------------------------------------
-- TABLE: storefront_audit_log
-- -----------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_audit_event') THEN
        CREATE TYPE storefront_audit_event AS ENUM ('cart_created', 'cart_adopted', 'line_added', 'line_updated', 'line_removed', 'coupon_applied', 'coupon_removed', 'billing_set', 'delivery_set', 'cart_placed', 'checkout_confirmed_free', 'checkout_settled_confirmed', 'cart_cancelled', 'recovery_sent', 'listing_upserted', 'listing_published', 'listing_unpublished', 'publish_refused', 'price_set', 'settings_set');
    END IF;
END
$$;

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.storefront_audit_log (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID,
    event storefront_audit_event NOT NULL,
    actor UUID,
    subject_type TEXT,
    subject_id UUID,
    detail JSONB,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_storefront_audit_log_event_occurred_at ON storefront.storefront_audit_log (event, occurred_at);

-- -----------------------------------------------------------------------------
-- TABLE: website_sale_settings
-- -----------------------------------------------------------------------------

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_access_gate') THEN
        CREATE TYPE storefront_access_gate AS ENUM ('open', 'members_only');
    END IF;
END
$$;

CREATE SCHEMA IF NOT EXISTS storefront;

CREATE TABLE IF NOT EXISTS storefront.website_sale_settings (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID NOT NULL,
    access_gate storefront_access_gate NOT NULL DEFAULT 'open',
    default_customer_group_id UUID,
    guest_party_id UUID NOT NULL,
    recovery_template_ref TEXT,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_website_sale_settings_website_id ON storefront.website_sale_settings (website_id);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_website_sale_settings_metadata_gin ON storefront.website_sale_settings USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_website_sale_settings_metadata_deleted_at ON storefront.website_sale_settings ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_website_sale_settings_metadata_created_at ON storefront.website_sale_settings ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_website_sale_settings_metadata_updated_at ON storefront.website_sale_settings ((metadata->>'updated_at'));
