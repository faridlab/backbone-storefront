-- Down: storefront hardening — drop the partial unique fences.
DROP INDEX IF EXISTS idx_carts_open_updated_at;
DROP INDEX IF EXISTS idx_website_sale_settings_website_live;
DROP INDEX IF EXISTS idx_checkout_sessions_gateway_tx_live;
DROP INDEX IF EXISTS idx_shopper_parties_company_email_live;
DROP INDEX IF EXISTS idx_carts_open_per_portal_user;
DROP INDEX IF EXISTS idx_carts_open_per_visitor;
DROP INDEX IF EXISTS idx_product_prices_website_item_live;
DROP INDEX IF EXISTS idx_product_listings_website_item_live;
