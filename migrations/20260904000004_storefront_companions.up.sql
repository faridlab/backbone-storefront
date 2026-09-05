-- =============================================================================
-- storefront — the shopper-companions increment (migration 4: companions).
--
-- Hand-written DDL (the family posture: migrations are the hand-consolidated
-- contract; the schema/models YAMLs are the model source of truth and carry
-- the same fields).
--
-- SHAPE NOTES (source: docs/spec.md §14, the companions increment):
--  - Cross-schema refs (website_id, visitor_id, warehouse_id, item_id,
--    portal_user_id) stay LOGICAL: plain indexed uuid columns, no FOREIGN
--    KEY across schema boundaries (the workspace posture).
--  - pickup_locations: the merchant-declared store registry. NEVER
--    auto-minted from warehouses — a store exists only as this explicit
--    row (no "link every warehouse" surface anywhere).
--  - pickup_locations.country is NOT NULL: the store's country is the
--    fiscal jurisdiction of every pickup order it mints, so it is
--    required merchant-declared input (2-letter ISO, validated at the
--    officer upsert). A countryless store has no defensible tax arm —
--    it would silently resolve pickup tax under the delivery/home
--    jurisdiction — so neither the column nor the upsert may leave it
--    empty, and the place verb keeps a typed refusal as the code-level
--    guard behind the constraint.
--  - wishlist_items: the visitor-backed wishlist. visitor_id is NOT NULL —
--    every wish is born from a real website visitor identity; the portal
--    link is a reconciled STAMP (set by the reconcile verb at login),
--    never the ownership key. UNIQUE (website_id, visitor_id, item_id) on
--    live rows is the idempotent-add arbiter; the read is the UNION of the
--    visitor's own rows and the principal-stamped rows, website-scoped —
--    no row ever moves between websites.
--  - carts.fulfillment_mode / pickup_location_id: the Click & Collect pin.
--    The client presents ONLY the opaque pickup_location id; the verb
--    resolves warehouse + address server-side from this module's own row.
--    The intra-schema FK to pickup_locations is ON DELETE RESTRICT (a
--    store with carts pinned to it must be deactivated, never deleted —
--    the deviation from the CASCADE convention is deliberate: a silent
--    store drop would orphan pickup pins).
--  - checkout_state gains 'pending_pickup' — the pay-on-site lane: the
--    order mints DRAFT, NO gateway row, and NOTHING auto-confirms; only
--    the officer confirm-pickup verb (after the store took payment)
--    confirms and stamps settled.
--  - website_sale_settings.display_warehouse_id: the availability pivot.
--    NULL = aggregate across warehouses (a documented, officer-visible
--    semantic — never a hidden footgun); the value scopes display reads.
--  - product_listings.allow_backorder: the per-listing sold-out policy.
--    false (default): the stock gate refuses line adds and places when
--    free quantity runs out; true: the listing stays orderable (made to
--    order) and the stock gate skips it.
--  - ALTER TYPE ... ADD VALUE: safe here in autocommit DDL runs; the new
--    enum values are never USED inside this same file.
--  - NO GRANTs — the host re-runs its role/RLS grant script as owner
--    after applying migrations.
-- =============================================================================

-- New checkout lane + new audit events.
ALTER TYPE storefront_checkout_state ADD VALUE IF NOT EXISTS 'pending_pickup';

ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'pickup_set';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'fulfillment_mode_reset';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'checkout_pending_pickup';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'pickup_confirmed';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'location_upserted';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'wishlist_added';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'wishlist_removed';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'wishlist_reconciled';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'stock_notify_armed';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'stock_alert_sent';
ALTER TYPE storefront_audit_event ADD VALUE IF NOT EXISTS 'listing_backorder_set';

-- -----------------------------------------------------------------------------
-- TABLE: pickup_locations (the merchant-declared Click & Collect registry)
-- -----------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS storefront.pickup_locations (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID NOT NULL,
    warehouse_id UUID,
    name TEXT NOT NULL,
    address_line1 TEXT,
    city TEXT,
    postal_code TEXT,
    country TEXT NOT NULL,
    latitude NUMERIC(9, 6),
    longitude NUMERIC(9, 6),
    opening_hours JSONB,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

CREATE INDEX IF NOT EXISTS idx_pickup_locations_website_active ON storefront.pickup_locations (website_id, is_active);

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_pickup_locations_metadata_gin ON storefront.pickup_locations USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_pickup_locations_metadata_deleted_at ON storefront.pickup_locations ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_pickup_locations_metadata_created_at ON storefront.pickup_locations ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_pickup_locations_metadata_updated_at ON storefront.pickup_locations ((metadata->>'updated_at'));

-- One live store name per website (officer-facing determinism).
CREATE UNIQUE INDEX IF NOT EXISTS idx_pickup_locations_website_name_live
    ON storefront.pickup_locations (website_id, name)
    WHERE (metadata->>'deleted_at') IS NULL;

-- -----------------------------------------------------------------------------
-- TABLE: wishlist_items (the visitor-backed wishlist)
-- -----------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS storefront.wishlist_items (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    website_id UUID NOT NULL,
    visitor_id UUID NOT NULL,
    portal_user_id UUID,
    item_id UUID NOT NULL,
    notify_on_stock BOOLEAN NOT NULL DEFAULT FALSE,
    contact_email TEXT,
    metadata JSONB NOT NULL DEFAULT '{"created_at":null,"updated_at":null,"deleted_at":null,"created_by":null,"updated_by":null,"deleted_by":null}'::jsonb,
    PRIMARY KEY (id)
);

-- The idempotent-add arbiter: one live wish per (website, visitor, item).
CREATE UNIQUE INDEX IF NOT EXISTS idx_wishlist_items_website_visitor_item_live
    ON storefront.wishlist_items (website_id, visitor_id, item_id)
    WHERE (metadata->>'deleted_at') IS NULL;

-- The principal-stamped read arm (the union read's second half).
CREATE INDEX IF NOT EXISTS idx_wishlist_items_website_portal_item
    ON storefront.wishlist_items (website_id, portal_user_id, item_id)
    WHERE (metadata->>'deleted_at') IS NULL;

-- The back-in-stock demand read (armed wishes per item).
CREATE INDEX IF NOT EXISTS idx_wishlist_items_stock_wait
    ON storefront.wishlist_items (website_id, item_id)
    WHERE notify_on_stock = TRUE AND (metadata->>'deleted_at') IS NULL;

-- GIN index for audit metadata JSONB queries
CREATE INDEX IF NOT EXISTS idx_wishlist_items_metadata_gin ON storefront.wishlist_items USING GIN (metadata);
CREATE INDEX IF NOT EXISTS idx_wishlist_items_metadata_deleted_at ON storefront.wishlist_items ((metadata->>'deleted_at'));
CREATE INDEX IF NOT EXISTS idx_wishlist_items_metadata_created_at ON storefront.wishlist_items ((metadata->>'created_at'));
CREATE INDEX IF NOT EXISTS idx_wishlist_items_metadata_updated_at ON storefront.wishlist_items ((metadata->>'updated_at'));

-- -----------------------------------------------------------------------------
-- COLUMN ADDITIONS (carts, checkout_sessions, settings, listings)
-- -----------------------------------------------------------------------------

ALTER TABLE storefront.carts
    ADD COLUMN IF NOT EXISTS fulfillment_mode TEXT NOT NULL DEFAULT 'delivery'
        CHECK (fulfillment_mode IN ('delivery', 'pickup')),
    ADD COLUMN IF NOT EXISTS pickup_location_id UUID;

ALTER TABLE storefront.carts
    ADD CONSTRAINT fk_carts_pickup_location_id
    FOREIGN KEY (pickup_location_id) REFERENCES storefront.pickup_locations (id)
    ON DELETE RESTRICT;

ALTER TABLE storefront.checkout_sessions
    ADD COLUMN IF NOT EXISTS pickup_location_id UUID;

ALTER TABLE storefront.website_sale_settings
    ADD COLUMN IF NOT EXISTS display_warehouse_id UUID;

ALTER TABLE storefront.product_listings
    ADD COLUMN IF NOT EXISTS allow_backorder BOOLEAN NOT NULL DEFAULT FALSE;

-- -----------------------------------------------------------------------------
-- AUDIT TRIGGERS for the new tables (the migration-3 shape)
-- -----------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION storefront.pickup_locations_audit_timestamp() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{created_at}', to_jsonb(NOW()));
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    ELSIF TG_OP = 'UPDATE' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS pickup_locations_insert_audit ON storefront.pickup_locations;
CREATE TRIGGER pickup_locations_insert_audit BEFORE INSERT ON storefront.pickup_locations
    FOR EACH ROW EXECUTE FUNCTION storefront.pickup_locations_audit_timestamp();

DROP TRIGGER IF EXISTS pickup_locations_update_audit ON storefront.pickup_locations;
CREATE TRIGGER pickup_locations_update_audit BEFORE UPDATE ON storefront.pickup_locations
    FOR EACH ROW EXECUTE FUNCTION storefront.pickup_locations_audit_timestamp();

CREATE OR REPLACE FUNCTION storefront.wishlist_items_audit_timestamp() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{created_at}', to_jsonb(NOW()));
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    ELSIF TG_OP = 'UPDATE' THEN
        NEW.metadata = jsonb_set(NEW.metadata::jsonb, '{updated_at}', to_jsonb(NOW()));
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS wishlist_items_insert_audit ON storefront.wishlist_items;
CREATE TRIGGER wishlist_items_insert_audit BEFORE INSERT ON storefront.wishlist_items
    FOR EACH ROW EXECUTE FUNCTION storefront.wishlist_items_audit_timestamp();

DROP TRIGGER IF EXISTS wishlist_items_update_audit ON storefront.wishlist_items;
CREATE TRIGGER wishlist_items_update_audit BEFORE UPDATE ON storefront.wishlist_items
    FOR EACH ROW EXECUTE FUNCTION storefront.wishlist_items_audit_timestamp();
