use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionSource {
    Server,
    #[default]
    SQLite,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineRenderer {
    List,
    #[default]
    VirtualList,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidecarHiddenTabBehavior {
    #[default]
    Keep,
    Discard,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceSettings {
    pub mention_source: SuggestionSource,
    pub hashtag_source: SuggestionSource,
    #[serde(default)]
    pub timeline_renderer: TimelineRenderer,
    #[serde(default)]
    pub sidecar_hidden_tab_behavior: SidecarHiddenTabBehavior,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sidecar_hidden_tab_behavior_keeps_existing_webviews() {
        let settings: PerformanceSettings = serde_json::from_value(serde_json::json!({
            "mention_source": "SQLite",
            "hashtag_source": "SQLite",
            "timeline_renderer": "VirtualList"
        }))
        .expect("deserialize legacy performance settings");

        assert_eq!(
            settings.sidecar_hidden_tab_behavior,
            SidecarHiddenTabBehavior::Keep
        );
    }
}
