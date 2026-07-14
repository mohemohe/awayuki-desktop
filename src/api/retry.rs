use std::collections::HashMap;
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::domain::adapter_error::{AdapterError, AdapterErrorCode};

const MAX_ATTEMPTS: u32 = 3;
const BASE_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(60);
const SERVER_RETRY_SPACING: Duration = Duration::from_millis(250);
const MAX_TRACKED_SERVERS: usize = 256;

static SERVER_BUDGETS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

/// Retry only protocol-neutral read failures which are safe to repeat. The
/// caller owns the future, so dropping it on an operation cancellation also
/// drops an in-progress request or retry sleep immediately.
pub async fn idempotent<T, F, Fut>(
    server: &str,
    operation: &'static str,
    mut request: F,
) -> Result<T, AdapterError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AdapterError>>,
{
    let mut attempt = 1;
    loop {
        match request().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let Some(delay) = retry_delay(&error, operation, attempt) else {
                    return Err(error);
                };
                if attempt >= MAX_ATTEMPTS {
                    return Err(error);
                }
                crate::observability::observe_http_retry();
                tokio::time::sleep(reserve_server_retry(server, delay)).await;
                attempt += 1;
            }
        }
    }
}

fn retry_delay(error: &AdapterError, operation: &str, attempt: u32) -> Option<Duration> {
    if !matches!(
        error.code,
        AdapterErrorCode::RateLimited | AdapterErrorCode::Timeout | AdapterErrorCode::Transport
    ) {
        return None;
    }
    let exponential = BASE_DELAY
        .saturating_mul(1u32 << attempt.saturating_sub(1).min(8))
        .min(MAX_DELAY);
    let header_delay = error
        .retry_after_seconds
        .map(Duration::from_secs)
        .unwrap_or_default()
        .min(MAX_DELAY);
    let jitter_window_ms = exponential.as_millis().min(250) as u64;
    let jitter = Duration::from_millis(
        stable_hash(operation.as_bytes(), attempt) % (jitter_window_ms.saturating_add(1)),
    );
    Some(
        exponential
            .max(header_delay)
            .saturating_add(jitter)
            .min(MAX_DELAY),
    )
}

fn reserve_server_retry(server: &str, requested_delay: Duration) -> Duration {
    let now = Instant::now();
    let server = server.to_lowercase();
    let budgets = SERVER_BUDGETS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut budgets = budgets.lock().unwrap_or_else(|error| error.into_inner());
    budgets.retain(|_, next| *next > now);
    if budgets.len() >= MAX_TRACKED_SERVERS && !budgets.contains_key(&server) {
        if let Some(oldest) = budgets
            .iter()
            .min_by_key(|(_, next)| **next)
            .map(|(server, _)| server.clone())
        {
            budgets.remove(&oldest);
        }
    }
    let requested_at = now + requested_delay;
    let reserved_at = budgets
        .get(&server)
        .copied()
        .unwrap_or(now)
        .max(requested_at);
    budgets.insert(server, reserved_at + SERVER_RETRY_SPACING);
    reserved_at.saturating_duration_since(now)
}

fn stable_hash(bytes: &[u8], attempt: u32) -> u64 {
    let mut hash = 0xcbf29ce484222325u64 ^ u64::from(attempt);
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::domain::adapter_error::AdapterSource;

    use super::*;

    fn error(code: AdapterErrorCode, retry_after_seconds: Option<u64>) -> AdapterError {
        AdapterError::new(
            code,
            AdapterSource::ActivityPub,
            retry_after_seconds,
            io::Error::other("fixture"),
        )
    }

    #[test]
    fn retry_policy_is_read_failure_and_retry_after_aware() {
        assert!(retry_delay(&error(AdapterErrorCode::Timeout, None), "home", 1).is_some());
        assert!(retry_delay(&error(AdapterErrorCode::Transport, None), "home", 1).is_some());
        let delay = retry_delay(&error(AdapterErrorCode::RateLimited, Some(12)), "home", 1)
            .expect("rate limit delay");
        assert!(delay >= Duration::from_secs(12));
        assert!(retry_delay(&error(AdapterErrorCode::Unauthorized, None), "home", 1).is_none());
        assert!(retry_delay(&error(AdapterErrorCode::InvalidResponse, None), "home", 1).is_none());
    }

    #[test]
    fn same_server_retries_are_spaced_and_registry_is_bounded() {
        let first = reserve_server_retry("example.test", Duration::ZERO);
        let second = reserve_server_retry("example.test", Duration::ZERO);
        assert!(second >= first);
        for index in 0..(MAX_TRACKED_SERVERS + 20) {
            reserve_server_retry(&format!("server-{index}.test"), Duration::from_secs(1));
        }
        let budgets = SERVER_BUDGETS
            .get()
            .expect("retry budgets")
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert!(budgets.len() <= MAX_TRACKED_SERVERS);
    }

    #[tokio::test]
    async fn idempotent_request_retries_transient_failures_at_most_three_times() {
        let attempts = AtomicUsize::new(0);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            idempotent("retry-fixture.test", "home", || {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                async move {
                    if attempt < MAX_ATTEMPTS as usize {
                        Err(error(AdapterErrorCode::Transport, None))
                    } else {
                        Ok("ok")
                    }
                }
            }),
        )
        .await
        .expect("bounded retry duration")
        .expect("third attempt succeeds");

        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), MAX_ATTEMPTS as usize);
    }
}
