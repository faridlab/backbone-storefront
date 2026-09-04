-- =============================================================================
-- storefront — core schema teardown (001_storefront_core_schema DOWN).
--
-- Drops the storefront schema outright (CASCADE covers tables, indexes,
-- functions, triggers) and then the four public enum types. Reverse of the
-- UP file; safe to run only with the schema's data truly disposable.
-- =============================================================================

DROP SCHEMA IF EXISTS storefront CASCADE;

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_audit_event') THEN
        DROP TYPE storefront_audit_event;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_cart_state') THEN
        DROP TYPE storefront_cart_state;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_checkout_state') THEN
        DROP TYPE storefront_checkout_state;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_type WHERE typname = 'storefront_access_gate') THEN
        DROP TYPE storefront_access_gate;
    END IF;
END
$$;
