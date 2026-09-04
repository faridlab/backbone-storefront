use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::StorefrontAccessGate;
use super::AuditMetadata;

/// Strongly-typed ID for WebsiteSaleSetting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebsiteSaleSettingId(pub Uuid);

impl WebsiteSaleSettingId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for WebsiteSaleSettingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for WebsiteSaleSettingId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for WebsiteSaleSettingId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<WebsiteSaleSettingId> for Uuid {
    fn from(id: WebsiteSaleSettingId) -> Self { id.0 }
}

impl AsRef<Uuid> for WebsiteSaleSettingId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for WebsiteSaleSettingId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WebsiteSaleSetting {
    pub id: Uuid,
    pub website_id: Uuid,
    pub access_gate: StorefrontAccessGate,
    pub default_customer_group_id: Option<Uuid>,
    pub guest_party_id: Uuid,
    pub recovery_template_ref: Option<String>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl WebsiteSaleSetting {
    /// Create a builder for WebsiteSaleSetting
    pub fn builder() -> WebsiteSaleSettingBuilder {
        <WebsiteSaleSettingBuilder as Default>::default()
    }

    /// Create a new WebsiteSaleSetting with required fields
    pub fn new(website_id: Uuid, access_gate: StorefrontAccessGate, guest_party_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            website_id,
            access_gate,
            default_customer_group_id: None,
            guest_party_id,
            recovery_template_ref: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> WebsiteSaleSettingId {
        WebsiteSaleSettingId(self.id)
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

    /// Set the default_customer_group_id field (chainable)
    pub fn with_default_customer_group_id(mut self, value: Uuid) -> Self {
        self.default_customer_group_id = Some(value);
        self
    }

    /// Set the recovery_template_ref field (chainable)
    pub fn with_recovery_template_ref(mut self, value: String) -> Self {
        self.recovery_template_ref = Some(value);
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
                "access_gate" => {
                    if let Ok(v) = serde_json::from_value(value) { self.access_gate = v; }
                }
                "default_customer_group_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.default_customer_group_id = v; }
                }
                "guest_party_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.guest_party_id = v; }
                }
                "recovery_template_ref" => {
                    if let Ok(v) = serde_json::from_value(value) { self.recovery_template_ref = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for WebsiteSaleSetting {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "WebsiteSaleSetting"
    }
}

impl backbone_core::PersistentEntity for WebsiteSaleSetting {
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

impl backbone_orm::EntityRepoMeta for WebsiteSaleSetting {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("default_customer_group_id".to_string(), "uuid".to_string());
        m.insert("guest_party_id".to_string(), "uuid".to_string());
        m.insert("access_gate".to_string(), "storefront_access_gate".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
}

/// Builder for WebsiteSaleSetting entity
///
/// Provides a fluent API for constructing WebsiteSaleSetting instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct WebsiteSaleSettingBuilder {
    website_id: Option<Uuid>,
    access_gate: Option<StorefrontAccessGate>,
    default_customer_group_id: Option<Uuid>,
    guest_party_id: Option<Uuid>,
    recovery_template_ref: Option<String>,
}

impl WebsiteSaleSettingBuilder {
    /// Set the website_id field (required)
    pub fn website_id(mut self, value: Uuid) -> Self {
        self.website_id = Some(value);
        self
    }

    /// Set the access_gate field (default: `StorefrontAccessGate::default()`)
    pub fn access_gate(mut self, value: StorefrontAccessGate) -> Self {
        self.access_gate = Some(value);
        self
    }

    /// Set the default_customer_group_id field (optional)
    pub fn default_customer_group_id(mut self, value: Uuid) -> Self {
        self.default_customer_group_id = Some(value);
        self
    }

    /// Set the guest_party_id field (required)
    pub fn guest_party_id(mut self, value: Uuid) -> Self {
        self.guest_party_id = Some(value);
        self
    }

    /// Set the recovery_template_ref field (optional)
    pub fn recovery_template_ref(mut self, value: String) -> Self {
        self.recovery_template_ref = Some(value);
        self
    }

    /// Build the WebsiteSaleSetting entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<WebsiteSaleSetting, String> {
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let guest_party_id = self.guest_party_id.ok_or_else(|| "guest_party_id is required".to_string())?;

        Ok(WebsiteSaleSetting {
            id: Uuid::new_v4(),
            website_id,
            access_gate: self.access_gate.unwrap_or_default(),
            default_customer_group_id: self.default_customer_group_id,
            guest_party_id,
            recovery_template_ref: self.recovery_template_ref,
            metadata: AuditMetadata::default(),
        })
    }
}
