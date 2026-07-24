use serde::{Deserialize, Serialize};

/// Identifies the fediverse server software backing an account / server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServerKind {
    #[default]
    Mastodon,
    Paon,
    Misskey,
    Bluesky,
}

impl ServerKind {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Mastodon => "mastodon",
            Self::Paon => "paon",
            Self::Misskey => "misskey",
            Self::Bluesky => "bluesky",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "paon" => Self::Paon,
            "misskey" => Self::Misskey,
            "bluesky" => Self::Bluesky,
            _ => Self::Mastodon,
        }
    }

    pub fn is_mastodon_compatible(&self) -> bool {
        matches!(self, Self::Mastodon | Self::Paon)
    }
}
