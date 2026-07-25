use std::collections::HashSet;

use serde::{Deserialize, Serialize};

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
