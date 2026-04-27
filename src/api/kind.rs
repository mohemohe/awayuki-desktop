use serde::{Deserialize, Serialize};

/// Identifies the fediverse server software backing an account / server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServerKind {
    Mastodon,
    Paon,
    Misskey,
}

impl ServerKind {
    pub fn as_db_str(&self) -> &'static str {
        match self {
            Self::Mastodon => "mastodon",
            Self::Paon => "paon",
            Self::Misskey => "misskey",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "paon" => Self::Paon,
            "misskey" => Self::Misskey,
            _ => Self::Mastodon,
        }
    }

    #[allow(dead_code)]
    pub fn is_mastodon_compatible(&self) -> bool {
        matches!(self, Self::Mastodon | Self::Paon)
    }
}

impl Default for ServerKind {
    fn default() -> Self {
        Self::Mastodon
    }
}
