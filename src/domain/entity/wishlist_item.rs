use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for WishlistItem
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WishlistItemId(pub Uuid);

impl WishlistItemId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for WishlistItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for WishlistItemId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for WishlistItemId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<WishlistItemId> for Uuid {
    fn from(id: WishlistItemId) -> Self { id.0 }
}

impl AsRef<Uuid> for WishlistItemId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for WishlistItemId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WishlistItem {
    pub id: Uuid,
    pub website_id: Uuid,
    pub visitor_id: Uuid,
    pub portal_user_id: Option<Uuid>,
    pub item_id: Uuid,
    pub notify_on_stock: bool,
    pub contact_email: Option<String>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl WishlistItem {
    /// Create a builder for WishlistItem
    pub fn builder() -> WishlistItemBuilder {
        <WishlistItemBuilder as Default>::default()
    }

    /// Create a new WishlistItem with required fields
    pub fn new(website_id: Uuid, visitor_id: Uuid, item_id: Uuid, notify_on_stock: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            website_id,
            visitor_id,
            portal_user_id: None,
            item_id,
            notify_on_stock,
            contact_email: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> WishlistItemId {
        WishlistItemId(self.id)
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

    /// Set the contact_email field (chainable)
    pub fn with_contact_email(mut self, value: String) -> Self {
        self.contact_email = Some(value);
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
                "item_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.item_id = v; }
                }
                "notify_on_stock" => {
                    if let Ok(v) = serde_json::from_value(value) { self.notify_on_stock = v; }
                }
                "contact_email" => {
                    if let Ok(v) = serde_json::from_value(value) { self.contact_email = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for WishlistItem {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "WishlistItem"
    }
}

impl backbone_core::PersistentEntity for WishlistItem {
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

impl backbone_orm::EntityRepoMeta for WishlistItem {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("visitor_id".to_string(), "uuid".to_string());
        m.insert("portal_user_id".to_string(), "uuid".to_string());
        m.insert("item_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builder for WishlistItem entity
///
/// Provides a fluent API for constructing WishlistItem instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct WishlistItemBuilder {
    website_id: Option<Uuid>,
    visitor_id: Option<Uuid>,
    portal_user_id: Option<Uuid>,
    item_id: Option<Uuid>,
    notify_on_stock: Option<bool>,
    contact_email: Option<String>,
}

impl WishlistItemBuilder {
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

    /// Set the item_id field (required)
    pub fn item_id(mut self, value: Uuid) -> Self {
        self.item_id = Some(value);
        self
    }

    /// Set the notify_on_stock field (default: `false`)
    pub fn notify_on_stock(mut self, value: bool) -> Self {
        self.notify_on_stock = Some(value);
        self
    }

    /// Set the contact_email field (optional)
    pub fn contact_email(mut self, value: String) -> Self {
        self.contact_email = Some(value);
        self
    }

    /// Build the WishlistItem entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<WishlistItem, String> {
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let visitor_id = self.visitor_id.ok_or_else(|| "visitor_id is required".to_string())?;
        let item_id = self.item_id.ok_or_else(|| "item_id is required".to_string())?;

        Ok(WishlistItem {
            id: Uuid::new_v4(),
            website_id,
            visitor_id,
            portal_user_id: self.portal_user_id,
            item_id,
            notify_on_stock: self.notify_on_stock.unwrap_or(false),
            contact_email: self.contact_email,
            metadata: AuditMetadata::default(),
        })
    }
}
