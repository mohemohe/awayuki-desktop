use serde::{Deserialize, Serialize};

/// Event received from the streaming API
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Update(String),
    Notification(String),
    Delete(String),
    FiltersChanged,
    StatusUpdate(String),
    /// Connection generation changed. Downstream must refresh a snapshot
    /// before treating following deltas as gap-free.
    Resync,
    Unknown(String, String),
}

/// Type of stream to subscribe to
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamType {
    User,
    UserNotification,
    Public,
    PublicLocal,
    PublicRemote,
    Hashtag(String),
    HashtagLocal(String),
    List(String),
    Direct,
}

impl StreamType {
    pub fn stream_param(&self) -> &str {
        match self {
            Self::User => "user",
            Self::UserNotification => "user:notification",
            Self::Public => "public",
            Self::PublicLocal => "public:local",
            Self::PublicRemote => "public:remote",
            Self::Hashtag(_) => "hashtag",
            Self::HashtagLocal(_) => "hashtag:local",
            Self::List(_) => "list",
            Self::Direct => "direct",
        }
    }

    pub fn extra_param(&self) -> Option<(&str, &str)> {
        match self {
            Self::Hashtag(tag) | Self::HashtagLocal(tag) => Some(("tag", tag)),
            Self::List(id) => Some(("list", id)),
            _ => None,
        }
    }
}
