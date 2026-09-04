use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "storefront_cart_state", rename_all = "snake_case")]
pub enum StorefrontCartState {
    Open,
    Placed,
    Closed,
    Cancelled,
}

impl std::fmt::Display for StorefrontCartState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Placed => write!(f, "placed"),
            Self::Closed => write!(f, "closed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FromStr for StorefrontCartState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "open" => Ok(Self::Open),
            "placed" => Ok(Self::Placed),
            "closed" => Ok(Self::Closed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("Unknown StorefrontCartState variant: {}", s)),
        }
    }
}

impl Default for StorefrontCartState {
    fn default() -> Self {
        Self::Open
    }
}
