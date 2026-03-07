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
            confirm_boost: true,
            confirm_favourite: true,
            confirm_follow: true,
            confirm_unfollow: true,
        }
    }
}

impl Global for ConfirmationSettings {}
