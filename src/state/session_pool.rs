use std::collections::HashMap;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;

/// All currently signed-in sessions, indexed by `acct`. Mirrors
/// `SessionManager` but exposed as a GPUI Global so panels can route
/// outgoing actions to the correct backend without holding a reference to
/// the workspace. Used in unified-timeline mode: a Bluesky status arriving
/// in a panel whose primary is Mastodon must hit the user's Bluesky
/// session, not whatever happens to be active in the account switcher.
#[derive(Clone, Default)]
pub struct SessionPool {
    pub sessions: HashMap<String, ApiClient>,
}

impl SessionPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_pairs<I>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (String, ApiClient)>,
    {
        Self {
            sessions: pairs.into_iter().collect(),
        }
    }

    pub fn find_by_domain(&self, domain: &str) -> Option<ApiClient> {
        self.sessions
            .values()
            .find(|c| c.domain() == domain)
            .cloned()
    }

    pub fn find_by_kind(&self, kind: ServerKind) -> Option<ApiClient> {
        self.sessions.values().find(|c| c.kind() == kind).cloned()
    }
}
