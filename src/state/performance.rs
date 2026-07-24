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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceSettings {
    pub mention_source: SuggestionSource,
    pub hashtag_source: SuggestionSource,
    #[serde(default)]
    pub timeline_renderer: TimelineRenderer,
}
