use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "storefront_access_gate", rename_all = "snake_case")]
pub enum StorefrontAccessGate {
    Open,
    MembersOnly,
}

impl std::fmt::Display for StorefrontAccessGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::MembersOnly => write!(f, "members_only"),
        }
    }
}

impl FromStr for StorefrontAccessGate {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "members_only" => Ok(Self::MembersOnly),
            _ => Err(format!("Unknown StorefrontAccessGate variant: {}", s)),
        }
    }
}

impl Default for StorefrontAccessGate {
    fn default() -> Self {
        Self::Open
    }
}
