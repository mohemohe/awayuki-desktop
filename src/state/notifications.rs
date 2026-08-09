use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationSound {
    #[default]
    Default,
    Silent,
    Message,
    Mail,
    Reminder,
}

impl NotificationSound {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Default" => Some(Self::Default),
            "Silent" => Some(Self::Silent),
            "Message" => Some(Self::Message),
            "Mail" => Some(Self::Mail),
            "Reminder" => Some(Self::Reminder),
            _ => None,
        }
    }

    pub fn native_name(self) -> Option<&'static str> {
        match self {
            Self::Silent => None,
            Self::Default => Some("Default"),
            #[cfg(target_os = "windows")]
            Self::Message => Some("IM"),
            #[cfg(target_os = "windows")]
            Self::Mail => Some("Mail"),
            #[cfg(target_os = "windows")]
            Self::Reminder => Some("Reminder"),
            #[cfg(target_os = "macos")]
            Self::Message => Some("Glass"),
            #[cfg(target_os = "macos")]
            Self::Mail => Some("Ping"),
            #[cfg(target_os = "macos")]
            Self::Reminder => Some("Purr"),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Message => Some("message-new-instant"),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Mail => Some("message-new-email"),
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Reminder => Some("alarm-clock-elapsed"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationPreferences {
    #[serde(default)]
    pub default_sound: NotificationSound,
}

/// Application-side list of accounts whose desktop notifications are suppressed.
/// Suppressed notifications are still shown in the Notification timeline;
/// only the OS-level desktop toast is skipped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NotificationSuppressionList {
    #[serde(default)]
    pub suppressed_accts: HashSet<String>,
}

impl NotificationSuppressionList {
    pub fn is_suppressed(&self, acct: &str) -> bool {
        self.suppressed_accts.contains(acct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_values_round_trip_and_reject_unknown_values() {
        for value in ["Default", "Silent", "Message", "Mail", "Reminder"] {
            let sound = NotificationSound::parse(value).unwrap();
            assert_eq!(
                serde_json::to_string(&sound).unwrap(),
                format!("\"{value}\"")
            );
        }
        assert_eq!(NotificationSound::parse("Alarm10"), None);
        assert_eq!(NotificationSound::Silent.native_name(), None);
    }
}
