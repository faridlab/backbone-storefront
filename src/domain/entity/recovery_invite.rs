use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use super::AuditMetadata;

/// Strongly-typed ID for RecoveryInvite
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecoveryInviteId(pub Uuid);

impl RecoveryInviteId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for RecoveryInviteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for RecoveryInviteId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for RecoveryInviteId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<RecoveryInviteId> for Uuid {
    fn from(id: RecoveryInviteId) -> Self { id.0 }
}

impl AsRef<Uuid> for RecoveryInviteId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for RecoveryInviteId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RecoveryInvite {
    pub id: Uuid,
    pub cart_id: Uuid,
    pub template_ref: String,
    pub notified_at: Option<DateTime<Utc>>,
    pub delivery_state: String,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl RecoveryInvite {
    /// Create a builder for RecoveryInvite
    pub fn builder() -> RecoveryInviteBuilder {
        <RecoveryInviteBuilder as Default>::default()
    }

    /// Create a new RecoveryInvite with required fields
    pub fn new(cart_id: Uuid, template_ref: String, delivery_state: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            cart_id,
            template_ref,
            notified_at: None,
            delivery_state,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> RecoveryInviteId {
        RecoveryInviteId(self.id)
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

    /// Set the notified_at field (chainable)
    pub fn with_notified_at(mut self, value: DateTime<Utc>) -> Self {
        self.notified_at = Some(value);
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
                "template_ref" => {
                    if let Ok(v) = serde_json::from_value(value) { self.template_ref = v; }
                }
                "notified_at" => {
                    if let Ok(v) = serde_json::from_value(value) { self.notified_at = v; }
                }
                "delivery_state" => {
                    if let Ok(v) = serde_json::from_value(value) { self.delivery_state = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for RecoveryInvite {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "RecoveryInvite"
    }
}

impl backbone_core::PersistentEntity for RecoveryInvite {
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

impl backbone_orm::EntityRepoMeta for RecoveryInvite {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("cart_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["template_ref", "delivery_state"]
    }
    fn relations() -> &'static [(&'static str, &'static str, &'static str)] {
        &[("cart", "carts", "cartId")]
    }
}

/// Builder for RecoveryInvite entity
///
/// Provides a fluent API for constructing RecoveryInvite instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct RecoveryInviteBuilder {
    cart_id: Option<Uuid>,
    template_ref: Option<String>,
    notified_at: Option<DateTime<Utc>>,
    delivery_state: Option<String>,
}

impl RecoveryInviteBuilder {
    /// Set the cart_id field (required)
    pub fn cart_id(mut self, value: Uuid) -> Self {
        self.cart_id = Some(value);
        self
    }

    /// Set the template_ref field (required)
    pub fn template_ref(mut self, value: String) -> Self {
        self.template_ref = Some(value);
        self
    }

    /// Set the notified_at field (optional)
    pub fn notified_at(mut self, value: DateTime<Utc>) -> Self {
        self.notified_at = Some(value);
        self
    }

    /// Set the delivery_state field (default: `Default::default()`)
    pub fn delivery_state(mut self, value: String) -> Self {
        self.delivery_state = Some(value);
        self
    }

    /// Build the RecoveryInvite entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<RecoveryInvite, String> {
        let cart_id = self.cart_id.ok_or_else(|| "cart_id is required".to_string())?;
        let template_ref = self.template_ref.ok_or_else(|| "template_ref is required".to_string())?;

        Ok(RecoveryInvite {
            id: Uuid::new_v4(),
            cart_id,
            template_ref,
            notified_at: self.notified_at,
            delivery_state: self.delivery_state.unwrap_or_default(),
            metadata: AuditMetadata::default(),
        })
    }
}
