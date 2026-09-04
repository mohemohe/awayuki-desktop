mod executor;
mod fetcher;
mod manager;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub use manager::PluginManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginHook {
    BeforeCreatePost,
    AfterCreatePost,
    BeforeBoost,
    AfterBoost,
    BeforeFavorite,
    AfterFavorite,
    BeforeBookmark,
    AfterBookmark,
    BeforeDeletePost,
    AfterDeletePost,
}

impl PluginHook {
    pub const ALL: [Self; 10] = [
        Self::BeforeCreatePost,
        Self::AfterCreatePost,
        Self::BeforeBoost,
        Self::AfterBoost,
        Self::BeforeFavorite,
        Self::AfterFavorite,
        Self::BeforeBookmark,
        Self::AfterBookmark,
        Self::BeforeDeletePost,
        Self::AfterDeletePost,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeCreatePost => "beforeCreatePost",
            Self::AfterCreatePost => "afterCreatePost",
            Self::BeforeBoost => "beforeBoost",
            Self::AfterBoost => "afterBoost",
            Self::BeforeFavorite => "beforeFavorite",
            Self::AfterFavorite => "afterFavorite",
            Self::BeforeBookmark => "beforeBookmark",
            Self::AfterBookmark => "afterBookmark",
            Self::BeforeDeletePost => "beforeDeletePost",
            Self::AfterDeletePost => "afterDeletePost",
        }
    }
}

/// Identifies the exact plugin-set revision used for a before-hook decision.
///
/// The fields are intentionally private: callers obtain a token from
/// [`PluginManager::hook_token`] and can only pass it back to
/// [`PluginManager::run_hook_checked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginHookToken {
    revision: u64,
    hook: PluginHook,
}

impl PluginHookToken {
    pub(super) const fn new(revision: u64, hook: PluginHook) -> Self {
        Self { revision, hook }
    }

    pub(super) fn matches(self, revision: u64, hook: PluginHook) -> bool {
        self.revision == revision && self.hook == hook
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    Loaded,
    Unloaded,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginLogLevel {
    Trace,
    Debug,
    Log,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginLogEntry {
    pub timestamp: String,
    pub level: PluginLogLevel,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub file_name: String,
    pub path: String,
    pub version: Option<u32>,
    pub state: PluginState,
    pub generation: u64,
    pub error: Option<String>,
    pub logs: Vec<PluginLogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeButtonDescriptor {
    pub plugin_id: String,
    pub button_id: String,
    pub generation: u64,
    pub icon: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSnapshot {
    pub directory: String,
    pub revision: u64,
    pub plugins: Vec<PluginInfo>,
    pub compose_buttons: Vec<ComposeButtonDescriptor>,
}
