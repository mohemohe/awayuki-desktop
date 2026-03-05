use gpui::Global;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSettings {
    pub mention_source: SuggestionSource,
    pub hashtag_source: SuggestionSource,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            mention_source: SuggestionSource::SQLite,
            hashtag_source: SuggestionSource::SQLite,
        }
    }
}

impl Global for PerformanceSettings {}
