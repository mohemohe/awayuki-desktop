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

    /// Toggle suppression for the given acct. Returns the new state (true = now suppressed).
    pub fn toggle(&mut self, acct: &str) -> bool {
        if self.suppressed_accts.remove(acct) {
            false
        } else {
            self.suppressed_accts.insert(acct.to_string());
            true
        }
    }
}
