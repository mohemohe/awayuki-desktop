use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MediaSource {
    #[default]
    Local,
    Remote,
}

impl MediaSource {
    pub const ALL: [MediaSource; 2] = [MediaSource::Local, MediaSource::Remote];

    pub fn label(&self) -> &'static str {
        match self {
            MediaSource::Local => "Local instance",
            MediaSource::Remote => "Remote (original)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmationSettings {
    pub confirm_boost: bool,
    pub confirm_favourite: bool,
    pub confirm_follow: bool,
    pub confirm_unfollow: bool,
    #[serde(default)]
    pub media_source: MediaSource,
    #[serde(default)]
    pub translate_enabled: bool,
    #[serde(default)]
    pub auto_translate_enabled: bool,
}

impl Default for ConfirmationSettings {
    fn default() -> Self {
        Self {
            confirm_boost: true,
            confirm_favourite: true,
            confirm_follow: true,
            confirm_unfollow: true,
            media_source: MediaSource::Local,
            translate_enabled: false,
            auto_translate_enabled: false,
        }
    }
}
