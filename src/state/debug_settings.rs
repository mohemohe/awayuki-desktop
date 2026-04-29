use gpui::Global;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugSettings {
    #[serde(default)]
    pub logging_enabled: bool,
}

impl Default for DebugSettings {
    fn default() -> Self {
        Self {
            logging_enabled: false,
        }
    }
}

impl Global for DebugSettings {}
