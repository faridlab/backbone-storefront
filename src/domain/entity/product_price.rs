use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;
use super::AuditMetadata;

/// Strongly-typed ID for ProductPrice
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProductPriceId(pub Uuid);

impl ProductPriceId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for ProductPriceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ProductPriceId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for ProductPriceId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<ProductPriceId> for Uuid {
    fn from(id: ProductPriceId) -> Self { id.0 }
}

impl AsRef<Uuid> for ProductPriceId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for ProductPriceId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProductPrice {
    pub id: Uuid,
    pub website_id: Uuid,
    pub item_id: Uuid,
    pub list_price: Decimal,
    pub compare_at_price: Option<Decimal>,
    pub currency: String,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl ProductPrice {
    /// Create a builder for ProductPrice
    pub fn builder() -> ProductPriceBuilder {
        <ProductPriceBuilder as Default>::default()
    }

    /// Create a new ProductPrice with required fields
    pub fn new(website_id: Uuid, item_id: Uuid, list_price: Decimal, currency: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            website_id,
            item_id,
            list_price,
            compare_at_price: None,
            currency,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> ProductPriceId {
        ProductPriceId(self.id)
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

    /// Set the compare_at_price field (chainable)
    pub fn with_compare_at_price(mut self, value: Decimal) -> Self {
        self.compare_at_price = Some(value);
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
                "item_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.item_id = v; }
                }
                "list_price" => {
                    if let Ok(v) = serde_json::from_value(value) { self.list_price = v; }
                }
                "compare_at_price" => {
                    if let Ok(v) = serde_json::from_value(value) { self.compare_at_price = v; }
                }
                "currency" => {
                    if let Ok(v) = serde_json::from_value(value) { self.currency = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for ProductPrice {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "ProductPrice"
    }
}

impl backbone_core::PersistentEntity for ProductPrice {
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

impl backbone_orm::EntityRepoMeta for ProductPrice {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("item_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["currency"]
    }
}

/// Builder for ProductPrice entity
///
/// Provides a fluent API for constructing ProductPrice instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct ProductPriceBuilder {
    website_id: Option<Uuid>,
    item_id: Option<Uuid>,
    list_price: Option<Decimal>,
    compare_at_price: Option<Decimal>,
    currency: Option<String>,
}

impl ProductPriceBuilder {
    /// Set the website_id field (required)
    pub fn website_id(mut self, value: Uuid) -> Self {
        self.website_id = Some(value);
        self
    }

    /// Set the item_id field (required)
    pub fn item_id(mut self, value: Uuid) -> Self {
        self.item_id = Some(value);
        self
    }

    /// Set the list_price field (required)
    pub fn list_price(mut self, value: Decimal) -> Self {
        self.list_price = Some(value);
        self
    }

    /// Set the compare_at_price field (optional)
    pub fn compare_at_price(mut self, value: Decimal) -> Self {
        self.compare_at_price = Some(value);
        self
    }

    /// Set the currency field (default: `"IDR".to_string()`)
    pub fn currency(mut self, value: String) -> Self {
        self.currency = Some(value);
        self
    }

    /// Build the ProductPrice entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<ProductPrice, String> {
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let item_id = self.item_id.ok_or_else(|| "item_id is required".to_string())?;
        let list_price = self.list_price.ok_or_else(|| "list_price is required".to_string())?;

        Ok(ProductPrice {
            id: Uuid::new_v4(),
            website_id,
            item_id,
            list_price,
            compare_at_price: self.compare_at_price,
            currency: self.currency.unwrap_or("IDR".to_string()),
            metadata: AuditMetadata::default(),
        })
    }
}
