use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::StorefrontCartState;
use super::AuditMetadata;

/// Strongly-typed ID for Cart
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CartId(pub Uuid);

impl CartId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for CartId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for CartId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for CartId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<CartId> for Uuid {
    fn from(id: CartId) -> Self { id.0 }
}

impl AsRef<Uuid> for CartId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for CartId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Cart {
    pub id: Uuid,
    pub website_id: Uuid,
    pub visitor_id: Uuid,
    pub portal_user_id: Option<Uuid>,
    pub party_id: Option<Uuid>,
    pub state: StorefrontCartState,
    pub coupon_code: Option<String>,
    pub delivery_carrier_id: Option<Uuid>,
    pub placed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl Cart {
    /// Create a builder for Cart
    pub fn builder() -> CartBuilder {
        <CartBuilder as Default>::default()
    }

    /// Create a new Cart with required fields
    pub fn new(website_id: Uuid, visitor_id: Uuid, state: StorefrontCartState) -> Self {
        Self {
            id: Uuid::new_v4(),
            website_id,
            visitor_id,
            portal_user_id: None,
            party_id: None,
            state,
            coupon_code: None,
            delivery_carrier_id: None,
            placed_at: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> CartId {
        CartId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the portal_user_id field (chainable)
    pub fn with_portal_user_id(mut self, value: Uuid) -> Self {
        self.portal_user_id = Some(value);
        self
    }

    /// Set the party_id field (chainable)
    pub fn with_party_id(mut self, value: Uuid) -> Self {
        self.party_id = Some(value);
        self
    }

    /// Set the coupon_code field (chainable)
    pub fn with_coupon_code(mut self, value: String) -> Self {
        self.coupon_code = Some(value);
        self
    }

    /// Set the delivery_carrier_id field (chainable)
    pub fn with_delivery_carrier_id(mut self, value: Uuid) -> Self {
        self.delivery_carrier_id = Some(value);
        self
    }

    /// Set the placed_at field (chainable)
    pub fn with_placed_at(mut self, value: DateTime<Utc>) -> Self {
        self.placed_at = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "website_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.website_id = v; }
                }
                "visitor_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.visitor_id = v; }
                }
                "portal_user_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.portal_user_id = v; }
                }
                "party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.party_id = v; }
                }
                "state" => {
                    if let Ok(v) = serde_json::from_value(value) { self.state = v; }
                }
                "coupon_code" => {
                    if let Ok(v) = serde_json::from_value(value) { self.coupon_code = v; }
                }
                "delivery_carrier_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.delivery_carrier_id = v; }
                }
                "placed_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.placed_at = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for Cart {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "Cart"
    }
}

impl backbone_core::PersistentEntity for Cart {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for Cart {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("visitor_id".to_string(), "uuid".to_string());
        m.insert("portal_user_id".to_string(), "uuid".to_string());
        m.insert("party_id".to_string(), "uuid".to_string());
        m.insert("delivery_carrier_id".to_string(), "uuid".to_string());
        m.insert("state".to_string(), "storefront_cart_state".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builder for Cart entity
///
/// Provides a fluent API for constructing Cart instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct CartBuilder {
    website_id: Option<Uuid>,
    visitor_id: Option<Uuid>,
    portal_user_id: Option<Uuid>,
    party_id: Option<Uuid>,
    state: Option<StorefrontCartState>,
    coupon_code: Option<String>,
    delivery_carrier_id: Option<Uuid>,
    placed_at: Option<DateTime<Utc>>,
}

impl CartBuilder {
    /// Set the website_id field (required)
    pub fn website_id(mut self, value: Uuid) -> Self {
        self.website_id = Some(value);
        self
    }

    /// Set the visitor_id field (required)
    pub fn visitor_id(mut self, value: Uuid) -> Self {
        self.visitor_id = Some(value);
        self
    }

    /// Set the portal_user_id field (optional)
    pub fn portal_user_id(mut self, value: Uuid) -> Self {
        self.portal_user_id = Some(value);
        self
    }

    /// Set the party_id field (optional)
    pub fn party_id(mut self, value: Uuid) -> Self {
        self.party_id = Some(value);
        self
    }

    /// Set the state field (default: `StorefrontCartState::default()`)
    pub fn state(mut self, value: StorefrontCartState) -> Self {
        self.state = Some(value);
        self
    }

    /// Set the coupon_code field (optional)
    pub fn coupon_code(mut self, value: String) -> Self {
        self.coupon_code = Some(value);
        self
    }

    /// Set the delivery_carrier_id field (optional)
    pub fn delivery_carrier_id(mut self, value: Uuid) -> Self {
        self.delivery_carrier_id = Some(value);
        self
    }

    /// Set the placed_at field (optional)
    pub fn placed_at(mut self, value: DateTime<Utc>) -> Self {
        self.placed_at = Some(value);
        self
    }

    /// Build the Cart entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<Cart, String> {
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let visitor_id = self.visitor_id.ok_or_else(|| "visitor_id is required".to_string())?;

        Ok(Cart {
            id: Uuid::new_v4(),
            website_id,
            visitor_id,
            portal_user_id: self.portal_user_id,
            party_id: self.party_id,
            state: self.state.unwrap_or_default(),
            coupon_code: self.coupon_code,
            delivery_carrier_id: self.delivery_carrier_id,
            placed_at: self.placed_at,
            metadata: AuditMetadata::default(),
        })
    }
}
