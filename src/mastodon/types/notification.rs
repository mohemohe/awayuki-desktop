use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::account::Account;
use super::status::Status;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    #[serde(rename = "type")]
    pub notification_type: NotificationType,
    pub created_at: DateTime<Utc>,
    pub account: Account,
    #[serde(default)]
    pub status: Option<Status>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    Mention,
    Reblog,
    Favourite,
    Follow,
    FollowRequest,
    Poll,
    Status,
    Update,
    #[serde(rename = "admin.sign_up")]
    AdminSignUp,
    #[serde(rename = "admin.report")]
    AdminReport,
    #[serde(other)]
    Unknown,
}

impl NotificationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Reblog => "reblog",
            Self::Favourite => "favourite",
            Self::Follow => "follow",
            Self::FollowRequest => "follow_request",
            Self::Poll => "poll",
            Self::Status => "status",
            Self::Update => "update",
            Self::AdminSignUp => "admin.sign_up",
            Self::AdminReport => "admin.report",
            Self::Unknown => "unknown",
        }
    }
}
