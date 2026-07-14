//! Process-local reconnect admission control shared by protocol adapters.
//!
//! Persistent connections remain account-scoped, but connection attempts for
//! the same server are staggered so a network outage cannot create a burst.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SERVER_RECONNECT_GAP: Duration = Duration::from_millis(250);

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
