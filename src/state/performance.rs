use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionSource {
    Server,
    SQLite,
}

impl SuggestionSource {
    pub const ALL: [SuggestionSource; 2] = [SuggestionSource::Server, SuggestionSource::SQLite];

    pub fn label(&self) -> &'static str {
        match self {
            SuggestionSource::Server => "Server",
            SuggestionSource::SQLite => "SQLite",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineRenderer {
    List,
    VirtualList,
}

impl TimelineRenderer {
    pub const ALL: [TimelineRenderer; 2] = [TimelineRenderer::List, TimelineRenderer::VirtualList];

    pub fn label(&self) -> &'static str {
        match self {
            TimelineRenderer::List => "List",
            TimelineRenderer::VirtualList => "VirtualList",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSettings {
    pub mention_source: SuggestionSource,
    pub hashtag_source: SuggestionSource,
    #[serde(default)]
    pub timeline_renderer: TimelineRenderer,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            mention_source: SuggestionSource::SQLite,
            hashtag_source: SuggestionSource::SQLite,
            timeline_renderer: TimelineRenderer::VirtualList,
        }
    }
}

impl Default for TimelineRenderer {
    fn default() -> Self {
        TimelineRenderer::VirtualList
    }
}
