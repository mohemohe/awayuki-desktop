use std::sync::{Arc, Mutex};

use tokio::sync::watch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupState {
    Initializing,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct StartupControl {
    state: StartupState,
    worker_running: bool,
}

/// Coordinates the one background initializer shared by all IPC requests.
///
/// The mutex makes `Failed -> Initializing + worker_running` one atomic state
/// transition. A retry can therefore never enqueue two migration/session
/// workers, even when the UI double-clicks the Retry button.
#[derive(Clone)]
pub struct StartupGate {
    sender: watch::Sender<StartupState>,
    control: Arc<Mutex<StartupControl>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RetryStartError {
    #[error("application initialization is already running")]
    AlreadyRunning,
    #[error("application initialization has not failed")]
    NotFailed,
}

impl StartupGate {
    pub fn new() -> Self {
        let state = StartupState::Initializing;
        let (sender, _receiver) = watch::channel(state.clone());
        Self {
            sender,
            control: Arc::new(Mutex::new(StartupControl {
                state,
                worker_running: false,
            })),
        }
    }

    /// Claims the initial worker exactly once.
    pub fn begin_initialization(&self) -> bool {
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.worker_running || control.state != StartupState::Initializing {
            return false;
        }
        control.worker_running = true;
        true
    }

    /// Atomically claims a retry worker and resets the observable state.
    pub fn begin_retry(&self) -> Result<(), RetryStartError> {
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if control.worker_running {
            return Err(RetryStartError::AlreadyRunning);
        }
        if !matches!(control.state, StartupState::Failed(_)) {
            return Err(RetryStartError::NotFailed);
        }
        control.state = StartupState::Initializing;
        control.worker_running = true;
        self.sender.send_replace(StartupState::Initializing);
        Ok(())
    }

    pub fn mark_ready(&self) {
        self.finish(StartupState::Ready);
    }

    pub fn mark_failed(&self, message: impl Into<String>) {
        self.finish(StartupState::Failed(message.into()));
    }

    fn finish(&self, state: StartupState) {
        let mut control = self
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        control.state = state.clone();
        control.worker_running = false;
        self.sender.send_replace(state);
    }

    pub async fn wait_until_ready(&self) -> Result<(), String> {
        let mut receiver = self.sender.subscribe();
        loop {
            match receiver.borrow().clone() {
                StartupState::Initializing => {}
                StartupState::Ready => return Ok(()),
                StartupState::Failed(message) => return Err(message),
            }

            if receiver.changed().await.is_err() {
                return Err("Application initialization stopped unexpectedly".to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    #[test]
    fn retry_requires_a_failed_initialization() {
        let gate = StartupGate::new();
        assert!(gate.begin_initialization());
        assert_eq!(gate.begin_retry(), Err(RetryStartError::AlreadyRunning));
        gate.mark_ready();
        assert_eq!(gate.begin_retry(), Err(RetryStartError::NotFailed));
    }

    #[test]
    fn concurrent_retries_claim_exactly_one_worker() {
        let gate = StartupGate::new();
        assert!(gate.begin_initialization());
        gate.mark_failed("migration failed");

        let gate = Arc::new(gate);
        let barrier = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let gate = Arc::clone(&gate);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    gate.begin_retry()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();

        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("retry worker panicked"))
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(RetryStartError::AlreadyRunning))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn a_failed_gate_can_be_retried_until_ready() {
        let gate = StartupGate::new();
        assert!(gate.begin_initialization());
        gate.mark_failed("first attempt");
        assert_eq!(gate.wait_until_ready().await, Err("first attempt".into()));

        gate.begin_retry().expect("retry should start");
        let waiter = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.wait_until_ready().await })
        };
        gate.mark_ready();
        assert_eq!(waiter.await.expect("waiter panicked"), Ok(()));
    }
}
