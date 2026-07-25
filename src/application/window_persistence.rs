//! Coalesced persistence scheduling for native window state.
//!
//! Native move/resize callbacks can arrive thousands of times during one drag.
//! They only signal this queue; exactly one owner task holds the resettable
//! debounce timer and performs the final close-time flush.

use std::future::Future;
use std::time::Duration;

use tokio::sync::watch;

#[derive(Clone, Copy)]
pub(crate) enum PersistSignal {
    Idle,
    Changed,
    Flush,
}

#[derive(Clone)]
pub(crate) struct WindowPersistenceSignals {
    sender: watch::Sender<PersistSignal>,
}

pub(crate) fn window_persistence_channel(
) -> (WindowPersistenceSignals, watch::Receiver<PersistSignal>) {
    let (sender, receiver) = watch::channel(PersistSignal::Idle);
    (WindowPersistenceSignals { sender }, receiver)
}

impl WindowPersistenceSignals {
    pub(crate) fn changed(&self) {
        self.sender.send_replace(PersistSignal::Changed);
    }

    pub(crate) fn flush(&self) {
        self.sender.send_replace(PersistSignal::Flush);
    }
}

pub(crate) async fn run_window_persistence_worker<F, Fut>(
    mut events: watch::Receiver<PersistSignal>,
    debounce: Duration,
    mut persist: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    while events.changed().await.is_ok() {
        let signal = { *events.borrow_and_update() };
        match signal {
            PersistSignal::Idle => {}
            PersistSignal::Flush => persist().await,
            PersistSignal::Changed => loop {
                let delay = tokio::time::sleep(debounce);
                tokio::pin!(delay);
                tokio::select! {
                    () = &mut delay => {
                        persist().await;
                        break;
                    }
                    changed = events.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        let next_signal = { *events.borrow_and_update() };
                        match next_signal {
                            PersistSignal::Changed | PersistSignal::Idle => continue,
                            PersistSignal::Flush => {
                                persist().await;
                                break;
                            }
                        }
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn ten_thousand_events_use_one_worker_and_one_debounced_write() {
        const EVENTS: usize = 10_000;
        let (signals, receiver) = window_persistence_channel();
        let writes = Arc::new(AtomicUsize::new(0));
        let worker_writes = writes.clone();
        // One spawned handle is the task-count contract. Signal volume never
        // creates another task or timer owner.
        let worker = tokio::spawn(run_window_persistence_worker(
            receiver,
            Duration::from_millis(5),
            move || {
                let writes = worker_writes.clone();
                async move {
                    writes.fetch_add(1, Ordering::Relaxed);
                }
            },
        ));

        for _ in 0..EVENTS {
            signals.changed();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(writes.load(Ordering::Relaxed), 1);

        signals.flush();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(writes.load(Ordering::Relaxed), 2);

        drop(signals);
        worker.await.expect("window persistence worker");
    }
}
