use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for ProductListing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProductListingId(pub Uuid);

impl ProductListingId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for ProductListingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for ProductListingId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for ProductListingId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<ProductListingId> for Uuid {
    fn from(id: ProductListingId) -> Self { id.0 }
}

impl AsRef<Uuid> for ProductListingId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for ProductListingId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProductListing {
    pub id: Uuid,
    pub website_id: Uuid,
    pub item_id: Uuid,
    pub sale_ok: bool,
    pub is_published: bool,
    pub sequence: i32,
    pub media_urls: serde_json::Value,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl ProductListing {
    /// Create a builder for ProductListing
    pub fn builder() -> ProductListingBuilder {
        <ProductListingBuilder as Default>::default()
    }

    /// Create a new ProductListing with required fields
    pub fn new(website_id: Uuid, item_id: Uuid, sale_ok: bool, is_published: bool, sequence: i32, media_urls: serde_json::Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            website_id,
            item_id,
            sale_ok,
            is_published,
            sequence,
            media_urls,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> ProductListingId {
        ProductListingId(self.id)
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
                "sale_ok" => {
                    if let Ok(v) = serde_json::from_value(value) { self.sale_ok = v; }
                }
                "is_published" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_published = v; }
                }
                "sequence" => {
                    if let Ok(v) = serde_json::from_value(value) { self.sequence = v; }
                }
                "media_urls" => {
                    if let Ok(v) = serde_json::from_value(value) { self.media_urls = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for ProductListing {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "ProductListing"
    }
}

impl backbone_core::PersistentEntity for ProductListing {
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

impl backbone_orm::EntityRepoMeta for ProductListing {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("item_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builder for ProductListing entity
///
/// Provides a fluent API for constructing ProductListing instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct ProductListingBuilder {
    website_id: Option<Uuid>,
    item_id: Option<Uuid>,
    sale_ok: Option<bool>,
    is_published: Option<bool>,
    sequence: Option<i32>,
    media_urls: Option<serde_json::Value>,
}

impl ProductListingBuilder {
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

    /// Set the sale_ok field (default: `false`)
    pub fn sale_ok(mut self, value: bool) -> Self {
        self.sale_ok = Some(value);
        self
    }

    /// Set the is_published field (default: `false`)
    pub fn is_published(mut self, value: bool) -> Self {
        self.is_published = Some(value);
        self
    }

    /// Set the sequence field (default: `10`)
    pub fn sequence(mut self, value: i32) -> Self {
        self.sequence = Some(value);
        self
    }

    /// Set the media_urls field (default: `serde_json::json!([])`)
    pub fn media_urls(mut self, value: serde_json::Value) -> Self {
        self.media_urls = Some(value);
        self
    }

    /// Build the ProductListing entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<ProductListing, String> {
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let item_id = self.item_id.ok_or_else(|| "item_id is required".to_string())?;

        Ok(ProductListing {
            id: Uuid::new_v4(),
            website_id,
            item_id,
            sale_ok: self.sale_ok.unwrap_or(false),
            is_published: self.is_published.unwrap_or(false),
            sequence: self.sequence.unwrap_or(10),
            media_urls: self.media_urls.unwrap_or(serde_json::json!([])),
            metadata: AuditMetadata::default(),
        })
    }
}
