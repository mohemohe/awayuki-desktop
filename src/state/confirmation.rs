use gpui::Global;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationSettings {
    pub confirm_boost: bool,
    pub confirm_favourite: bool,
    pub confirm_follow: bool,
    pub confirm_unfollow: bool,
}

impl Default for ConfirmationSettings {
    fn default() -> Self {
        Self {
            confirm_boost: false,
            confirm_favourite: false,
            confirm_follow: false,
            confirm_unfollow: false,
        }
    }
}

impl Global for ConfirmationSettings {}
