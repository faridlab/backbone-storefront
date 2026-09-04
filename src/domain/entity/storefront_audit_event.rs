use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "storefront_audit_event", rename_all = "snake_case")]
pub enum StorefrontAuditEvent {
    CartCreated,
    CartAdopted,
    LineAdded,
    LineUpdated,
    LineRemoved,
    CouponApplied,
    CouponRemoved,
    BillingSet,
    DeliverySet,
    CartPlaced,
    CheckoutConfirmedFree,
    CheckoutSettledConfirmed,
    CartCancelled,
    RecoverySent,
    ListingPublished,
    ListingUnpublished,
    PublishRefused,
    PriceSet,
    SettingsSet,
}

impl std::fmt::Display for StorefrontAuditEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CartCreated => write!(f, "cart_created"),
            Self::CartAdopted => write!(f, "cart_adopted"),
            Self::LineAdded => write!(f, "line_added"),
            Self::LineUpdated => write!(f, "line_updated"),
            Self::LineRemoved => write!(f, "line_removed"),
            Self::CouponApplied => write!(f, "coupon_applied"),
            Self::CouponRemoved => write!(f, "coupon_removed"),
            Self::BillingSet => write!(f, "billing_set"),
            Self::DeliverySet => write!(f, "delivery_set"),
            Self::CartPlaced => write!(f, "cart_placed"),
            Self::CheckoutConfirmedFree => write!(f, "checkout_confirmed_free"),
            Self::CheckoutSettledConfirmed => write!(f, "checkout_settled_confirmed"),
            Self::CartCancelled => write!(f, "cart_cancelled"),
            Self::RecoverySent => write!(f, "recovery_sent"),
            Self::ListingPublished => write!(f, "listing_published"),
            Self::ListingUnpublished => write!(f, "listing_unpublished"),
            Self::PublishRefused => write!(f, "publish_refused"),
            Self::PriceSet => write!(f, "price_set"),
            Self::SettingsSet => write!(f, "settings_set"),
        }
    }
}

impl FromStr for StorefrontAuditEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "cart_created" => Ok(Self::CartCreated),
            "cart_adopted" => Ok(Self::CartAdopted),
            "line_added" => Ok(Self::LineAdded),
            "line_updated" => Ok(Self::LineUpdated),
            "line_removed" => Ok(Self::LineRemoved),
            "coupon_applied" => Ok(Self::CouponApplied),
            "coupon_removed" => Ok(Self::CouponRemoved),
            "billing_set" => Ok(Self::BillingSet),
            "delivery_set" => Ok(Self::DeliverySet),
            "cart_placed" => Ok(Self::CartPlaced),
            "checkout_confirmed_free" => Ok(Self::CheckoutConfirmedFree),
            "checkout_settled_confirmed" => Ok(Self::CheckoutSettledConfirmed),
            "cart_cancelled" => Ok(Self::CartCancelled),
            "recovery_sent" => Ok(Self::RecoverySent),
            "listing_published" => Ok(Self::ListingPublished),
            "listing_unpublished" => Ok(Self::ListingUnpublished),
            "publish_refused" => Ok(Self::PublishRefused),
            "price_set" => Ok(Self::PriceSet),
            "settings_set" => Ok(Self::SettingsSet),
            _ => Err(format!("Unknown StorefrontAuditEvent variant: {}", s)),
        }
    }
}

impl Default for StorefrontAuditEvent {
    fn default() -> Self {
        Self::CartCreated
    }
}
