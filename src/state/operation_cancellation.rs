use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct OperationCancellationManager {
    active: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

pub struct OperationCancellationLease {
    manager: OperationCancellationManager,
    operation_id: String,
    token: CancellationToken,
}

impl OperationCancellationManager {
    pub fn begin(&self, operation_id: &str) -> Option<OperationCancellationLease> {
        let token = CancellationToken::new();
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.contains_key(operation_id) {
            return None;
        }
        active.insert(operation_id.to_string(), token.clone());
        Some(OperationCancellationLease {
            manager: self.clone(),
            operation_id: operation_id.to_string(),
            token,
        })
    }

    pub fn cancel(&self, operation_id: &str) -> bool {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(token) = active.get(operation_id) else {
            return false;
        };
        token.cancel();
        true
    }

    pub fn cancel_all(&self) -> usize {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for token in active.values() {
            token.cancel();
        }
        active.len()
    }

    #[cfg(test)]
    fn active_count(&self) -> usize {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }
}

impl OperationCancellationLease {
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

impl Drop for OperationCancellationLease {
    fn drop(&mut self) {
        self.manager
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.operation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_is_unique_cancellable_and_removed_on_drop() {
        let manager = OperationCancellationManager::default();
        let lease = manager.begin("operation-1").expect("begin operation");
        assert!(manager.begin("operation-1").is_none());
        assert!(manager.cancel("operation-1"));
        assert!(lease.token().is_cancelled());
        assert_eq!(manager.active_count(), 1);
        drop(lease);
        assert_eq!(manager.active_count(), 0);
        assert!(!manager.cancel("operation-1"));
    }

    #[test]
    fn cancel_all_notifies_every_active_operation() {
        let manager = OperationCancellationManager::default();
        let first = manager.begin("operation-1").expect("first operation");
        let second = manager.begin("operation-2").expect("second operation");
        assert_eq!(manager.cancel_all(), 2);
        assert!(first.token().is_cancelled());
        assert!(second.token().is_cancelled());
    }
}
