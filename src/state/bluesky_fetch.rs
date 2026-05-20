use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_BLUESKY_FETCH_INTERVAL_SECONDS: u64 = 30;
pub const BLUESKY_FETCH_INTERVAL_SECONDS: [u64; 6] = [10, 15, 30, 60, 120, 300];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlueskyFetchSettings {
    #[serde(default)]
    pub intervals_by_acct: HashMap<String, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
}

impl BlueskyFetchSettings {
    pub fn normalized(mut self) -> Self {
        self.intervals_by_acct
            .retain(|_, seconds| BLUESKY_FETCH_INTERVAL_SECONDS.contains(seconds));
        if !self
            .interval_seconds
            .is_some_and(|seconds| BLUESKY_FETCH_INTERVAL_SECONDS.contains(&seconds))
        {
            self.interval_seconds = None;
        }
        self
    }

    pub fn interval_for_acct(&self, acct: &str) -> u64 {
        self.intervals_by_acct
            .get(acct)
            .copied()
            .or(self.interval_seconds)
            .filter(|seconds| BLUESKY_FETCH_INTERVAL_SECONDS.contains(seconds))
            .unwrap_or(DEFAULT_BLUESKY_FETCH_INTERVAL_SECONDS)
    }
}

impl Default for BlueskyFetchSettings {
    fn default() -> Self {
        Self {
            intervals_by_acct: HashMap::new(),
            interval_seconds: None,
        }
    }
}
