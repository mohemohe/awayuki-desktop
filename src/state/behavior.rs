use gpui::Global;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorSettings {
    #[serde(default)]
    pub unified_timeline: bool,
}

impl Default for BehaviorSettings {
    fn default() -> Self {
        Self {
            unified_timeline: false,
        }
    }
}

impl Global for BehaviorSettings {}
