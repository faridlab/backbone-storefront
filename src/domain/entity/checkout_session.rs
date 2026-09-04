use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::StorefrontCheckoutState;
use super::AuditMetadata;

/// Strongly-typed ID for CheckoutSession
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckoutSessionId(pub Uuid);

impl CheckoutSessionId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for CheckoutSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for CheckoutSessionId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for CheckoutSessionId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<CheckoutSessionId> for Uuid {
    fn from(id: CheckoutSessionId) -> Self { id.0 }
}

impl AsRef<Uuid> for CheckoutSessionId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for CheckoutSessionId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CheckoutSession {
    pub id: Uuid,
    pub cart_id: Uuid,
    pub website_id: Uuid,
    pub sales_order_id: Option<Uuid>,
    pub gateway_transaction_id: Option<Uuid>,
    pub provider_code: Option<String>,
    pub provider_reference: Option<String>,
    pub amount_total: Decimal,
    pub state: StorefrontCheckoutState,
    pub placed_at: Option<DateTime<Utc>>,
    pub settled_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl CheckoutSession {
    /// Create a builder for CheckoutSession
    pub fn builder() -> CheckoutSessionBuilder {
        <CheckoutSessionBuilder as Default>::default()
    }

    /// Create a new CheckoutSession with required fields
    pub fn new(cart_id: Uuid, website_id: Uuid, amount_total: Decimal, state: StorefrontCheckoutState) -> Self {
        Self {
            id: Uuid::new_v4(),
            cart_id,
            website_id,
            sales_order_id: None,
            gateway_transaction_id: None,
            provider_code: None,
            provider_reference: None,
            amount_total,
            state,
            placed_at: None,
            settled_at: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> CheckoutSessionId {
        CheckoutSessionId(self.id)
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

    /// Set the sales_order_id field (chainable)
    pub fn with_sales_order_id(mut self, value: Uuid) -> Self {
        self.sales_order_id = Some(value);
        self
    }

    /// Set the gateway_transaction_id field (chainable)
    pub fn with_gateway_transaction_id(mut self, value: Uuid) -> Self {
        self.gateway_transaction_id = Some(value);
        self
    }

    /// Set the provider_code field (chainable)
    pub fn with_provider_code(mut self, value: String) -> Self {
        self.provider_code = Some(value);
        self
    }

    /// Set the provider_reference field (chainable)
    pub fn with_provider_reference(mut self, value: String) -> Self {
        self.provider_reference = Some(value);
        self
    }

    /// Set the placed_at field (chainable)
    pub fn with_placed_at(mut self, value: DateTime<Utc>) -> Self {
        self.placed_at = Some(value);
        self
    }

    /// Set the settled_at field (chainable)
    pub fn with_settled_at(mut self, value: DateTime<Utc>) -> Self {
        self.settled_at = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "cart_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.cart_id = v; }
                }
                "website_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.website_id = v; }
                }
                "sales_order_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.sales_order_id = v; }
                }
                "gateway_transaction_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.gateway_transaction_id = v; }
                }
                "provider_code" => {
                    if let Ok(v) = serde_json::from_value(value) { self.provider_code = v; }
                }
                "provider_reference" => {
                    if let Ok(v) = serde_json::from_value(value) { self.provider_reference = v; }
                }
                "amount_total" => {
                    if let Ok(v) = serde_json::from_value(value) { self.amount_total = v; }
                }
                "state" => {
                    if let Ok(v) = serde_json::from_value(value) { self.state = v; }
                }
                "placed_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.placed_at = v; }
                }
                "settled_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.settled_at = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for CheckoutSession {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "CheckoutSession"
    }
}

impl backbone_core::PersistentEntity for CheckoutSession {
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

impl backbone_orm::EntityRepoMeta for CheckoutSession {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("cart_id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("sales_order_id".to_string(), "uuid".to_string());
        m.insert("gateway_transaction_id".to_string(), "uuid".to_string());
        m.insert("state".to_string(), "storefront_checkout_state".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("cart", "carts", "cartId")]
    }
}

/// Builder for CheckoutSession entity
///
/// Provides a fluent API for constructing CheckoutSession instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct CheckoutSessionBuilder {
    cart_id: Option<Uuid>,
    website_id: Option<Uuid>,
    sales_order_id: Option<Uuid>,
    gateway_transaction_id: Option<Uuid>,
    provider_code: Option<String>,
    provider_reference: Option<String>,
    amount_total: Option<Decimal>,
    state: Option<StorefrontCheckoutState>,
    placed_at: Option<DateTime<Utc>>,
    settled_at: Option<DateTime<Utc>>,
}

impl CheckoutSessionBuilder {
    /// Set the cart_id field (required)
    pub fn cart_id(mut self, value: Uuid) -> Self {
        self.cart_id = Some(value);
        self
    }

    /// Set the website_id field (required)
    pub fn website_id(mut self, value: Uuid) -> Self {
        self.website_id = Some(value);
        self
    }

    /// Set the sales_order_id field (optional)
    pub fn sales_order_id(mut self, value: Uuid) -> Self {
        self.sales_order_id = Some(value);
        self
    }

    /// Set the gateway_transaction_id field (optional)
    pub fn gateway_transaction_id(mut self, value: Uuid) -> Self {
        self.gateway_transaction_id = Some(value);
        self
    }

    /// Set the provider_code field (optional)
    pub fn provider_code(mut self, value: String) -> Self {
        self.provider_code = Some(value);
        self
    }

    /// Set the provider_reference field (optional)
    pub fn provider_reference(mut self, value: String) -> Self {
        self.provider_reference = Some(value);
        self
    }

    /// Set the amount_total field (required)
    pub fn amount_total(mut self, value: Decimal) -> Self {
        self.amount_total = Some(value);
        self
    }

    /// Set the state field (default: `StorefrontCheckoutState::default()`)
    pub fn state(mut self, value: StorefrontCheckoutState) -> Self {
        self.state = Some(value);
        self
    }

    /// Set the placed_at field (optional)
    pub fn placed_at(mut self, value: DateTime<Utc>) -> Self {
        self.placed_at = Some(value);
        self
    }

    /// Set the settled_at field (optional)
    pub fn settled_at(mut self, value: DateTime<Utc>) -> Self {
        self.settled_at = Some(value);
        self
    }

    /// Build the CheckoutSession entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<CheckoutSession, String> {
        let cart_id = self.cart_id.ok_or_else(|| "cart_id is required".to_string())?;
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let amount_total = self.amount_total.ok_or_else(|| "amount_total is required".to_string())?;

        Ok(CheckoutSession {
            id: Uuid::new_v4(),
            cart_id,
            website_id,
            sales_order_id: self.sales_order_id,
            gateway_transaction_id: self.gateway_transaction_id,
            provider_code: self.provider_code,
            provider_reference: self.provider_reference,
            amount_total,
            state: self.state.unwrap_or_default(),
            placed_at: self.placed_at,
            settled_at: self.settled_at,
            metadata: AuditMetadata::default(),
        })
    }
}
