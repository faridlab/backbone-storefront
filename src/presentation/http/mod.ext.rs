// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod admin_routes;
pub mod public_routes;
pub use admin_routes::{storefront_admin_routes, StorefrontAdminState, StorefrontActor};
pub use public_routes::{
    storefront_error_response, storefront_public_routes, StorefrontPublicState, VISITOR_TOKEN_HEADER,
};
