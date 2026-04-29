use serde::{Deserialize, Serialize};

/// Identifies the fediverse server software backing an account / server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServerKind {
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

    #[allow(dead_code)]
    pub fn is_mastodon_compatible(&self) -> bool {
        matches!(self, Self::Mastodon | Self::Paon)
    }

    /// Best-effort guess of which backend an account ID belongs to, based on
    /// the ID's format alone. Returns `None` when the shape is ambiguous
    /// (Mastodon and Misskey both use opaque short strings).
    ///
    /// Used to repair cross-kind references in unified-timeline mode: if a
    /// Bluesky DID surfaces in a panel whose primary account is Mastodon, the
    /// caller should pick a Bluesky session for the lookup instead of sending
    /// the DID to a Mastodon server (which 404s with "Record not found").
    pub fn detect_from_account_id(id: &str) -> Option<Self> {
        if id.starts_with("did:") {
            Some(Self::Bluesky)
        } else {
            None
        }
    }

    /// Best-effort guess from a status ID. Bluesky's status IDs are AT-URIs
    /// (`at://did:plc:.../app.bsky.feed.post/rkey`) or our repost wrapper
    /// (`repost:did:plc:.../<at-uri>`).
    pub fn detect_from_status_id(id: &str) -> Option<Self> {
        if id.starts_with("at://") || id.starts_with("repost:did:") {
            Some(Self::Bluesky)
        } else {
            None
        }
    }
}

impl Default for ServerKind {
    fn default() -> Self {
        Self::Mastodon
    }
}
