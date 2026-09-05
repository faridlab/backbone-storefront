-- Down: storefront companions — drop the increment's tables, columns,
-- triggers, and indexes. ENUM VALUES CANNOT BE DROPPED in PostgreSQL:
-- 'pending_pickup' on storefront_checkout_state and the new
-- storefront_audit_event values stay on the type (harmless — no live row
-- carries them after the tables/columns drop; a true reversal recreates
-- the types, which is a destructive event outside a plain down run).

DROP TRIGGER IF EXISTS wishlist_items_update_audit ON storefront.wishlist_items;
DROP TRIGGER IF EXISTS wishlist_items_insert_audit ON storefront.wishlist_items;
DROP FUNCTION IF EXISTS storefront.wishlist_items_audit_timestamp();

DROP TRIGGER IF EXISTS pickup_locations_update_audit ON storefront.pickup_locations;
DROP TRIGGER IF EXISTS pickup_locations_insert_audit ON storefront.pickup_locations;
DROP FUNCTION IF EXISTS storefront.pickup_locations_audit_timestamp();

ALTER TABLE storefront.product_listings
    DROP COLUMN IF EXISTS allow_backorder;

ALTER TABLE storefront.website_sale_settings
    DROP COLUMN IF EXISTS display_warehouse_id;

ALTER TABLE storefront.checkout_sessions
    DROP COLUMN IF EXISTS pickup_location_id;

ALTER TABLE storefront.carts
    DROP CONSTRAINT IF EXISTS fk_carts_pickup_location_id;
ALTER TABLE storefront.carts
    DROP COLUMN IF EXISTS pickup_location_id;
ALTER TABLE storefront.carts
    DROP COLUMN IF EXISTS fulfillment_mode;

DROP TABLE IF EXISTS storefront.wishlist_items;
DROP TABLE IF EXISTS storefront.pickup_locations;
