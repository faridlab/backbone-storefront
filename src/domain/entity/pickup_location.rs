use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;
use super::AuditMetadata;

/// Strongly-typed ID for PickupLocation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PickupLocationId(pub Uuid);

impl PickupLocationId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PickupLocationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PickupLocationId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PickupLocationId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PickupLocationId> for Uuid {
    fn from(id: PickupLocationId) -> Self { id.0 }
}

impl AsRef<Uuid> for PickupLocationId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PickupLocationId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PickupLocation {
    pub id: Uuid,
    pub website_id: Uuid,
    pub warehouse_id: Option<Uuid>,
    pub name: String,
    pub address_line1: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
    pub country: String,
    pub latitude: Option<Decimal>,
    pub longitude: Option<Decimal>,
    pub opening_hours: Option<serde_json::Value>,
    pub is_active: bool,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PickupLocation {
    /// Create a builder for PickupLocation
    pub fn builder() -> PickupLocationBuilder {
        <PickupLocationBuilder as Default>::default()
    }

    /// Create a new PickupLocation with required fields
    pub fn new(website_id: Uuid, name: String, country: String, is_active: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            website_id,
            warehouse_id: None,
            name,
            address_line1: None,
            city: None,
            postal_code: None,
            country,
            latitude: None,
            longitude: None,
            opening_hours: None,
            is_active,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PickupLocationId {
        PickupLocationId(self.id)
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

    /// Set the warehouse_id field (chainable)
    pub fn with_warehouse_id(mut self, value: Uuid) -> Self {
        self.warehouse_id = Some(value);
        self
    }

    /// Set the address_line1 field (chainable)
    pub fn with_address_line1(mut self, value: String) -> Self {
        self.address_line1 = Some(value);
        self
    }

    /// Set the city field (chainable)
    pub fn with_city(mut self, value: String) -> Self {
        self.city = Some(value);
        self
    }

    /// Set the postal_code field (chainable)
    pub fn with_postal_code(mut self, value: String) -> Self {
        self.postal_code = Some(value);
        self
    }

    /// Set the latitude field (chainable)
    pub fn with_latitude(mut self, value: Decimal) -> Self {
        self.latitude = Some(value);
        self
    }

    /// Set the longitude field (chainable)
    pub fn with_longitude(mut self, value: Decimal) -> Self {
        self.longitude = Some(value);
        self
    }

    /// Set the opening_hours field (chainable)
    pub fn with_opening_hours(mut self, value: serde_json::Value) -> Self {
        self.opening_hours = Some(value);
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
                "warehouse_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.warehouse_id = v; }
                }
                "name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.name = v; }
                }
                "address_line1" => {
                    if let Ok(v) = serde_json::from_value(value) { self.address_line1 = v; }
                }
                "city" => {
                    if let Ok(v) = serde_json::from_value(value) { self.city = v; }
                }
                "postal_code" => {
                    if let Ok(v) = serde_json::from_value(value) { self.postal_code = v; }
                }
                "country" => {
                    if let Ok(v) = serde_json::from_value(value) { self.country = v; }
                }
                "latitude" => {
                    if let Ok(v) = serde_json::from_value(value) { self.latitude = v; }
                }
                "longitude" => {
                    if let Ok(v) = serde_json::from_value(value) { self.longitude = v; }
                }
                "opening_hours" => {
                    if let Ok(v) = serde_json::from_value(value) { self.opening_hours = v; }
                }
                "is_active" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_active = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PickupLocation {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PickupLocation"
    }
}

impl backbone_core::PersistentEntity for PickupLocation {
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

impl backbone_orm::EntityRepoMeta for PickupLocation {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("website_id".to_string(), "uuid".to_string());
        m.insert("warehouse_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["name", "country"]
    }
}

/// Builder for PickupLocation entity
///
/// Provides a fluent API for constructing PickupLocation instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PickupLocationBuilder {
    website_id: Option<Uuid>,
    warehouse_id: Option<Uuid>,
    name: Option<String>,
    address_line1: Option<String>,
    city: Option<String>,
    postal_code: Option<String>,
    country: Option<String>,
    latitude: Option<Decimal>,
    longitude: Option<Decimal>,
    opening_hours: Option<serde_json::Value>,
    is_active: Option<bool>,
}

impl PickupLocationBuilder {
    /// Set the website_id field (required)
    pub fn website_id(mut self, value: Uuid) -> Self {
        self.website_id = Some(value);
        self
    }

    /// Set the warehouse_id field (optional)
    pub fn warehouse_id(mut self, value: Uuid) -> Self {
        self.warehouse_id = Some(value);
        self
    }

    /// Set the name field (required)
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the address_line1 field (optional)
    pub fn address_line1(mut self, value: String) -> Self {
        self.address_line1 = Some(value);
        self
    }

    /// Set the city field (optional)
    pub fn city(mut self, value: String) -> Self {
        self.city = Some(value);
        self
    }

    /// Set the postal_code field (optional)
    pub fn postal_code(mut self, value: String) -> Self {
        self.postal_code = Some(value);
        self
    }

    /// Set the country field (required)
    pub fn country(mut self, value: String) -> Self {
        self.country = Some(value);
        self
    }

    /// Set the latitude field (optional)
    pub fn latitude(mut self, value: Decimal) -> Self {
        self.latitude = Some(value);
        self
    }

    /// Set the longitude field (optional)
    pub fn longitude(mut self, value: Decimal) -> Self {
        self.longitude = Some(value);
        self
    }

    /// Set the opening_hours field (optional)
    pub fn opening_hours(mut self, value: serde_json::Value) -> Self {
        self.opening_hours = Some(value);
        self
    }

    /// Set the is_active field (default: `true`)
    pub fn is_active(mut self, value: bool) -> Self {
        self.is_active = Some(value);
        self
    }

    /// Build the PickupLocation entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PickupLocation, String> {
        let website_id = self.website_id.ok_or_else(|| "website_id is required".to_string())?;
        let name = self.name.ok_or_else(|| "name is required".to_string())?;
        let country = self.country.ok_or_else(|| "country is required".to_string())?;

        Ok(PickupLocation {
            id: Uuid::new_v4(),
            website_id,
            warehouse_id: self.warehouse_id,
            name,
            address_line1: self.address_line1,
            city: self.city,
            postal_code: self.postal_code,
            country,
            latitude: self.latitude,
            longitude: self.longitude,
            opening_hours: self.opening_hours,
            is_active: self.is_active.unwrap_or(true),
            metadata: AuditMetadata::default(),
        })
    }
}
