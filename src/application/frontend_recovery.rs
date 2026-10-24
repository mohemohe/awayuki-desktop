//! Process-local frontend checkpoints used when the main WebView is recovered.
//!
//! Draft content never reaches disk or logs. A newly created process, an
//! incomplete upload, or a rejected checkpoint keeps automatic recovery blocked.

use std::sync::Mutex;

use serde_json::Value;
#[cfg(target_os = "macos")]
use tauri::Manager;

pub(crate) const MAX_CHECKPOINT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointError {
    TooLarge,
    Unavailable,
    StaleDocument,
}

pub(crate) struct FrontendRecoveryState {
    stored: Mutex<StoredCheckpoint>,
}

struct StoredCheckpoint {
    checkpoint: Option<Value>,
    generation: Option<String>,
    recovery_blocked: bool,
    recreation_permitted: bool,
}

#[derive(Debug, PartialEq)]
pub(crate) struct RestoredCheckpoint {
    pub(crate) checkpoint: Option<Value>,
    pub(crate) generation: String,
}

impl Default for FrontendRecoveryState {
    fn default() -> Self {
        Self {
            stored: Mutex::new(StoredCheckpoint {
                checkpoint: None,
                generation: None,
                recovery_blocked: true,
                recreation_permitted: false,
            }),
        }
    }
}

impl FrontendRecoveryState {
    pub(crate) fn checkpoint(
        &self,
        checkpoint: Value,
        recovery_blocked: bool,
        generation: &str,
    ) -> Result<(), CheckpointError> {
        let mut stored = self
            .stored
            .lock()
            .map_err(|_| CheckpointError::Unavailable)?;
        // A reload is asynchronous: the previous document can still deliver
        // queued writes or ask to protect a submission after it was reserved.
        // Never acknowledge those requests, even if they would block recovery.
        if stored.generation.as_deref() != Some(generation) {
            return Err(CheckpointError::StaleDocument);
        }
        // A rejected update must never leave an older draft eligible for a
        // reload that would discard the user's more recent changes.
        stored.recovery_blocked = true;
        let serialized =
            serde_json::to_vec(&checkpoint).map_err(|_| CheckpointError::Unavailable)?;
        if serialized.len() > MAX_CHECKPOINT_BYTES {
            return Err(CheckpointError::TooLarge);
        }
        stored.checkpoint = (!checkpoint.is_null()).then_some(checkpoint);
        stored.recovery_blocked = recovery_blocked;
        Ok(())
    }

    pub(crate) fn restore(&self) -> Result<RestoredCheckpoint, CheckpointError> {
        let mut stored = self
            .stored
            .lock()
            .map_err(|_| CheckpointError::Unavailable)?;
        // This handshake runs once per frontend document. Keep recovery blocked
        // until that document has restored state and saved its first checkpoint.
        let generation = uuid::Uuid::new_v4().to_string();
        stored.generation = Some(generation.clone());
        stored.recovery_blocked = true;
        // Once a document begins restoring, it owns the preserved state. A
        // delayed fallback must not destroy it before its first checkpoint.
        stored.recreation_permitted = false;
        Ok(RestoredCheckpoint {
            checkpoint: stored.checkpoint.clone(),
            generation,
        })
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn recovery_allowed(&self) -> bool {
        self.stored
            .lock()
            .is_ok_and(|stored| !stored.recovery_blocked)
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn frontend_owns_checkpoint(&self) -> bool {
        self.stored
            .lock()
            .is_ok_and(|stored| stored.generation.is_some())
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn try_begin_recovery(&self) -> bool {
        let Ok(mut stored) = self.stored.lock() else {
            return false;
        };
        if stored.recovery_blocked {
            return false;
        }
        stored.recovery_blocked = true;
        stored.generation = None;
        stored.recreation_permitted = true;
        true
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn try_begin_recreation(&self) -> bool {
        let Ok(mut stored) = self.stored.lock() else {
            return false;
        };
        if !stored.recovery_blocked || stored.generation.is_some() || !stored.recreation_permitted {
            return false;
        }
        // Only the native owner of a reserved reload may reconstruct once if
        // WebKit cannot navigate away from the stalled document. Neither stale
        // writes nor subsequent watchdog ticks can renew this permission.
        stored.recreation_permitted = false;
        true
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn recovery_allowed(app: &tauri::AppHandle) -> bool {
    app.try_state::<FrontendRecoveryState>()
        .is_some_and(|state| state.recovery_allowed())
}

#[cfg(target_os = "macos")]
pub(crate) fn frontend_owns_checkpoint(app: &tauri::AppHandle) -> bool {
    app.try_state::<FrontendRecoveryState>()
        .is_some_and(|state| state.frontend_owns_checkpoint())
}

/// Reserve one reload. The restored frontend must checkpoint again before
/// another automatic reload is permitted.
#[cfg(target_os = "macos")]
pub(crate) fn try_begin_recovery(app: &tauri::AppHandle) -> bool {
    app.try_state::<FrontendRecoveryState>()
        .is_some_and(|state| state.try_begin_recovery())
}

/// Consume the single reconstruction fallback attached to a reserved reload.
/// The replacement document's restore handshake revokes this permission.
#[cfg(target_os = "macos")]
pub(crate) fn try_begin_recreation(app: &tauri::AppHandle) -> bool {
    app.try_state::<FrontendRecoveryState>()
        .is_some_and(|state| state.try_begin_recreation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_document_progress_cannot_claim_checkpoint_ownership_after_reload_reservation() {
        let state = FrontendRecoveryState::default();
        assert!(!state.frontend_owns_checkpoint());
        let restored = state.restore().unwrap();
        assert!(state.frontend_owns_checkpoint());
        state
            .checkpoint(Value::Null, false, &restored.generation)
            .unwrap();
        assert!(state.try_begin_recovery());
        assert!(!state.frontend_owns_checkpoint());
        assert_eq!(
            state.checkpoint(Value::Null, true, &restored.generation),
            Err(CheckpointError::StaleDocument)
        );
        assert!(!state.frontend_owns_checkpoint());
        state.restore().unwrap();
        assert!(state.frontend_owns_checkpoint());
    }

    #[test]
    fn recovery_stays_blocked_until_frontend_has_checkpointed() {
        let state = FrontendRecoveryState::default();
        assert!(!state.recovery_allowed());
        assert_eq!(
            state.checkpoint(Value::Null, false, ""),
            Err(CheckpointError::StaleDocument)
        );
        let restored = state.restore().unwrap();
        assert_eq!(restored.checkpoint, None);
        assert!(!state.recovery_allowed());

        state
            .checkpoint(Value::Null, false, &restored.generation)
            .unwrap();
        assert!(state.recovery_allowed());
    }

    #[test]
    fn latest_checkpoint_survives_multiple_restores_and_can_be_cleared() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        let first = json!({ "version": 1, "text": "draft one" });
        let latest = json!({ "version": 1, "text": "draft two", "pollEnabled": true });
        state.checkpoint(first, false, &generation).unwrap();
        state
            .checkpoint(latest.clone(), false, &generation)
            .unwrap();

        assert_eq!(state.restore().unwrap().checkpoint, Some(latest.clone()));
        let restored = state.restore().unwrap();
        assert_eq!(restored.checkpoint, Some(latest));
        state
            .checkpoint(Value::Null, false, &restored.generation)
            .unwrap();
        assert_eq!(state.restore().unwrap().checkpoint, None);
    }

    #[test]
    fn unsafe_frontend_work_blocks_recovery_without_losing_its_last_checkpoint() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        let draft = json!({ "version": 1, "text": "unsent" });
        state.checkpoint(draft.clone(), true, &generation).unwrap();

        assert!(!state.recovery_allowed());
        let restored = state.restore().unwrap();
        assert_eq!(restored.checkpoint, Some(draft.clone()));
        state
            .checkpoint(draft, false, &restored.generation)
            .unwrap();
        assert!(state.recovery_allowed());
    }

    #[test]
    fn a_reload_requires_a_new_checkpoint_before_another_recovery() {
        let state = FrontendRecoveryState::default();
        assert!(!state.try_begin_recovery());
        let generation = state.restore().unwrap().generation;
        let draft = json!({ "version": 1, "text": "unsent" });
        state.checkpoint(draft.clone(), false, &generation).unwrap();

        assert!(state.try_begin_recovery());
        assert!(!state.try_begin_recovery());
        assert!(!state.recovery_allowed());
        let restored = state.restore().unwrap();
        assert_ne!(restored.generation, generation);
        assert_eq!(restored.checkpoint, Some(draft.clone()));
        assert!(!state.try_begin_recovery());

        state
            .checkpoint(draft, false, &restored.generation)
            .unwrap();
        assert!(state.try_begin_recovery());
    }

    #[test]
    fn recreation_is_forbidden_without_a_reserved_reload() {
        let state = FrontendRecoveryState::default();
        assert!(!state.try_begin_recreation());
        let restored = state.restore().unwrap();
        assert!(!state.try_begin_recreation());
        state
            .checkpoint(Value::Null, false, &restored.generation)
            .unwrap();
        assert!(!state.try_begin_recreation());
    }

    #[test]
    fn reserved_reload_permits_only_one_recreation() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        let draft = json!({ "text": "preserved through reconstruction" });
        state.checkpoint(draft.clone(), false, &generation).unwrap();

        assert!(state.try_begin_recovery());
        assert!(state.try_begin_recreation());
        assert!(!state.try_begin_recreation());
        assert!(!state.try_begin_recovery());
        assert!(!state.recovery_allowed());
        assert_eq!(state.restore().unwrap().checkpoint, Some(draft));
    }

    #[test]
    fn restore_revokes_recreation_before_its_first_checkpoint() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        state.checkpoint(Value::Null, false, &generation).unwrap();
        assert!(state.try_begin_recovery());

        let restored = state.restore().unwrap();
        assert_ne!(restored.generation, generation);
        assert!(!state.try_begin_recreation());
        assert!(!state.try_begin_recovery());
        assert!(!state.recovery_allowed());
    }

    #[test]
    fn a_later_checkpoint_and_reload_grant_a_new_recreation() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        state.checkpoint(Value::Null, false, &generation).unwrap();
        assert!(state.try_begin_recovery());
        assert!(state.try_begin_recreation());

        let restored = state.restore().unwrap();
        state
            .checkpoint(Value::Null, false, &restored.generation)
            .unwrap();
        assert!(!state.try_begin_recreation());
        assert!(state.try_begin_recovery());
        assert!(state.try_begin_recreation());
        assert!(!state.try_begin_recreation());
    }

    #[test]
    fn old_document_writes_cannot_renew_recreation_permission() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        state.checkpoint(Value::Null, false, &generation).unwrap();
        assert!(state.try_begin_recovery());
        assert!(state.try_begin_recreation());

        for recovery_blocked in [true, false] {
            assert_eq!(
                state.checkpoint(Value::Null, recovery_blocked, &generation),
                Err(CheckpointError::StaleDocument)
            );
            assert!(!state.try_begin_recreation());
        }
        let restored = state.restore().unwrap();
        assert_eq!(
            state.checkpoint(Value::Null, false, &generation),
            Err(CheckpointError::StaleDocument)
        );
        assert!(!state.try_begin_recreation());
        state
            .checkpoint(Value::Null, false, &restored.generation)
            .unwrap();
        assert!(!state.try_begin_recreation());
    }

    #[test]
    fn serialized_byte_limit_is_inclusive_and_rejection_blocks_stale_recovery() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        let boundary = Value::String("x".repeat(MAX_CHECKPOINT_BYTES - 2));
        assert_eq!(
            state.checkpoint(boundary.clone(), false, &generation),
            Ok(())
        );
        assert!(state.recovery_allowed());

        let over_limit = Value::String("x".repeat(MAX_CHECKPOINT_BYTES - 1));
        assert_eq!(
            state.checkpoint(over_limit, false, &generation),
            Err(CheckpointError::TooLarge)
        );
        assert!(!state.recovery_allowed());
        assert_eq!(state.restore().unwrap().checkpoint, Some(boundary));
    }

    #[test]
    fn size_limit_counts_utf8_and_json_escaping() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        for text in [
            "あ".repeat(MAX_CHECKPOINT_BYTES / 3),
            "\n".repeat(MAX_CHECKPOINT_BYTES / 2),
        ] {
            assert_eq!(
                state.checkpoint(Value::String(text), false, &generation),
                Err(CheckpointError::TooLarge)
            );
        }
        assert!(!state.recovery_allowed());
        assert_eq!(state.restore().unwrap().checkpoint, None);
    }

    #[test]
    fn reserved_reload_rejects_all_writes_from_the_previous_document() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        let draft = json!({ "text": "preserved draft" });
        state.checkpoint(draft.clone(), false, &generation).unwrap();
        assert!(state.try_begin_recovery());

        // Blocking writes protect posting/upload operations. A stale document
        // must never receive success and start one while reload is pending.
        for recovery_blocked in [true, false] {
            assert_eq!(
                state.checkpoint(Value::Null, recovery_blocked, &generation),
                Err(CheckpointError::StaleDocument)
            );
            assert!(!state.recovery_allowed());
        }
        assert_eq!(state.restore().unwrap().checkpoint, Some(draft));
    }

    #[test]
    fn old_writes_cannot_modify_the_replacement_documents_checkpoint_or_guard() {
        let state = FrontendRecoveryState::default();
        let old_generation = state.restore().unwrap().generation;
        state
            .checkpoint(json!({ "text": "old" }), false, &old_generation)
            .unwrap();
        assert!(state.try_begin_recovery());
        let restored = state.restore().unwrap();
        assert_ne!(restored.generation, old_generation);
        let latest = json!({ "text": "restored and edited" });
        state
            .checkpoint(latest.clone(), false, &restored.generation)
            .unwrap();

        for recovery_blocked in [true, false] {
            assert_eq!(
                state.checkpoint(Value::Null, recovery_blocked, &old_generation),
                Err(CheckpointError::StaleDocument)
            );
            assert!(state.recovery_allowed());
        }
        assert_eq!(state.restore().unwrap().checkpoint, Some(latest));
    }

    #[test]
    fn ordinary_navigation_also_invalidates_previous_document_writes() {
        let state = FrontendRecoveryState::default();
        let old_generation = state.restore().unwrap().generation;
        state
            .checkpoint(Value::Null, false, &old_generation)
            .unwrap();
        let restored = state.restore().unwrap();
        assert_ne!(restored.generation, old_generation);
        assert!(!state.recovery_allowed());
        assert_eq!(
            state.checkpoint(Value::Null, false, &old_generation),
            Err(CheckpointError::StaleDocument)
        );
    }

    #[test]
    fn reservation_and_submission_protection_have_one_atomic_winner() {
        use std::sync::{Arc, Barrier};

        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        let draft = json!({ "text": "unsent" });
        state.checkpoint(draft.clone(), false, &generation).unwrap();
        let state = Arc::new(state);
        let barrier = Arc::new(Barrier::new(2));
        let recovery = {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                state.try_begin_recovery()
            })
        };
        barrier.wait();
        let protection = state.checkpoint(draft.clone(), true, &generation);
        if recovery.join().unwrap() {
            assert_eq!(protection, Err(CheckpointError::StaleDocument));
        } else {
            assert_eq!(protection, Ok(()));
        }
        assert!(!state.recovery_allowed());
        assert_eq!(state.restore().unwrap().checkpoint, Some(draft));
    }

    #[test]
    fn poisoned_state_fails_closed() {
        let state = FrontendRecoveryState::default();
        let generation = state.restore().unwrap().generation;
        state.checkpoint(Value::Null, false, &generation).unwrap();
        let _ = std::panic::catch_unwind(|| {
            let _guard = state.stored.lock().unwrap();
            panic!("poison recovery state");
        });
        assert!(!state.recovery_allowed());
        assert!(!state.try_begin_recovery());
        assert!(!state.try_begin_recreation());
        assert_eq!(state.restore(), Err(CheckpointError::Unavailable));
        assert_eq!(
            state.checkpoint(Value::Null, false, &generation),
            Err(CheckpointError::Unavailable)
        );
    }
}
