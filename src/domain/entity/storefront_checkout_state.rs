use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "storefront_checkout_state", rename_all = "snake_case")]
pub enum StorefrontCheckoutState {
    PendingPayment,
    ConfirmedFree,
    Settled,
    Failed,
    Cancelled,
}

impl std::fmt::Display for StorefrontCheckoutState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingPayment => write!(f, "pending_payment"),
            Self::ConfirmedFree => write!(f, "confirmed_free"),
            Self::Settled => write!(f, "settled"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FromStr for StorefrontCheckoutState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending_payment" => Ok(Self::PendingPayment),
            "confirmed_free" => Ok(Self::ConfirmedFree),
            "settled" => Ok(Self::Settled),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("Unknown StorefrontCheckoutState variant: {}", s)),
        }
    }
}

impl Default for StorefrontCheckoutState {
    fn default() -> Self {
        Self::PendingPayment
    }
}
