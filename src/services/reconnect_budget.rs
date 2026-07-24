//! Process-local reconnect admission control shared by protocol adapters.
//!
//! Persistent connections remain account-scoped, but connection attempts for
//! the same server are staggered so a network outage cannot create a burst.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SERVER_RECONNECT_GAP: Duration = Duration::from_millis(250);
const MAX_RECONNECT_BACKOFF_SECONDS: u64 = 60;

/// Exponential delay for consecutive connection-establishment failures.
///
/// A successfully established streaming session must call [`Self::reset`].
/// Otherwise a later, unrelated socket reset inherits old failures and leaves
/// the timeline disconnected for up to a minute.
pub struct ReconnectBackoff {
    next_delay_seconds: u64,
    attempt: u64,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            next_delay_seconds: 1,
            attempt: 0,
        }
    }
}

impl ReconnectBackoff {
    pub fn reset(&mut self) {
        self.next_delay_seconds = 1;
        self.attempt = 0;
    }

    pub fn next_delay(&mut self, server: &str) -> Duration {
        let delay = reconnect_delay(self.next_delay_seconds, server, self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        self.next_delay_seconds = (self.next_delay_seconds * 2).min(MAX_RECONNECT_BACKOFF_SECONDS);
        delay
    }
}

fn reconnect_delay(base_seconds: u64, server: &str, attempt: u64) -> Duration {
    let hash = server
        .bytes()
        .fold(0xcbf29ce484222325u64 ^ attempt, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    Duration::from_secs(base_seconds) + Duration::from_millis(hash % 1_000)
}

struct ReconnectBudget {
    gap: Duration,
    next_slots: Mutex<HashMap<String, Instant>>,
}

impl ReconnectBudget {
    fn new(gap: Duration) -> Self {
        Self {
            gap,
            next_slots: Mutex::new(HashMap::new()),
        }
    }

    fn reserve_at(&self, server: &str, now: Instant) -> Duration {
        let mut slots = self
            .next_slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slots.retain(|_, next| *next > now);
        let slot = slots.get(server).copied().unwrap_or(now).max(now);
        slots.insert(server.to_string(), slot + self.gap);
        slot.saturating_duration_since(now)
    }
}

fn reconnect_budget() -> &'static ReconnectBudget {
    static BUDGET: OnceLock<ReconnectBudget> = OnceLock::new();
    BUDGET.get_or_init(|| ReconnectBudget::new(SERVER_RECONNECT_GAP))
}

pub async fn wait_for_server_slot(server: &str) {
    let delay = reconnect_budget().reserve_at(server, Instant::now());
    if !delay.is_zero() {
        tracing::debug!(server, ?delay, "Waiting for server reconnect budget");
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn established_connection_resets_accumulated_backoff() {
        let mut backoff = ReconnectBackoff::default();

        assert_eq!(backoff.next_delay("wss://example.test").as_secs(), 1);
        assert_eq!(backoff.next_delay("wss://example.test").as_secs(), 2);
        assert_eq!(backoff.next_delay("wss://example.test").as_secs(), 4);

        backoff.reset();

        assert_eq!(backoff.next_delay("wss://example.test").as_secs(), 1);
        assert_eq!(backoff.next_delay("wss://example.test").as_secs(), 2);
    }

    #[test]
    fn reconnect_delay_has_bounded_deterministic_jitter() {
        let first = reconnect_delay(8, "wss://example.test", 3);
        assert_eq!(first, reconnect_delay(8, "wss://example.test", 3));
        assert!(first >= Duration::from_secs(8));
        assert!(first < Duration::from_secs(9));
        assert_ne!(first, reconnect_delay(8, "wss://example.test", 4));
    }

    #[test]
    fn same_server_attempts_are_staggered_but_other_servers_are_independent() {
        let budget = ReconnectBudget::new(Duration::from_millis(250));
        let now = Instant::now();

        assert_eq!(budget.reserve_at("shared.example", now), Duration::ZERO);
        assert_eq!(
            budget.reserve_at("shared.example", now),
            Duration::from_millis(250)
        );
        assert_eq!(budget.reserve_at("other.example", now), Duration::ZERO);
        assert_eq!(
            budget.reserve_at("shared.example", now + Duration::from_secs(1)),
            Duration::ZERO
        );
    }
}
