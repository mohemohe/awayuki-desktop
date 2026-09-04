//! Shared timing contract for the single-threaded plugin actor and its IPC.

/// Maximum time a single Promise-returning plugin callback may run.
pub const PLUGIN_CALLBACK_DEADLINE_MS: u32 = 30_000;

/// Scheduling and serialization allowance after callback execution.
pub const PLUGIN_ACTOR_MARGIN_MS: u32 = 5_000;

/// A quick actor command can legitimately wait behind one callback.
pub const PLUGIN_QUICK_REPLY_TIMEOUT_MS: u32 = PLUGIN_CALLBACK_DEADLINE_MS + PLUGIN_ACTOR_MARGIN_MS;

/// A callback-bearing actor command can wait behind one callback and then use
/// its own full callback budget.
pub const PLUGIN_CALLBACK_REPLY_TIMEOUT_MS: u32 =
    (PLUGIN_CALLBACK_DEADLINE_MS * 2) + PLUGIN_ACTOR_MARGIN_MS;

/// The IPC contract must outlive the manager reply deadline so a bounded actor
/// timeout can be returned as a typed application error before transport gives
/// up on the invocation.
pub const PLUGIN_TRANSPORT_MARGIN_MS: u32 = 5_000;
pub const PLUGIN_QUICK_IPC_TIMEOUT_MS: u32 =
    PLUGIN_QUICK_REPLY_TIMEOUT_MS + PLUGIN_TRANSPORT_MARGIN_MS;
pub const PLUGIN_CALLBACK_IPC_TIMEOUT_MS: u32 =
    PLUGIN_CALLBACK_REPLY_TIMEOUT_MS + PLUGIN_TRANSPORT_MARGIN_MS;

/// Adds plugin actor round trips to an existing command transport allowance
/// while retaining a final transport margin.
///
/// The quick and callback counts come from each mutation's actual call graph;
/// they prevent a legitimate bounded actor wait from outliving outer IPC.
pub const fn plugin_mutation_ipc_timeout_ms(
    existing_command_allowance_ms: u32,
    quick_round_trips: u32,
    callback_round_trips: u32,
) -> u32 {
    existing_command_allowance_ms
        + (PLUGIN_QUICK_REPLY_TIMEOUT_MS * quick_round_trips)
        + (PLUGIN_CALLBACK_REPLY_TIMEOUT_MS * callback_round_trips)
        + PLUGIN_TRANSPORT_MARGIN_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn reply_budgets_cover_one_queued_callback_and_transport_has_margin() {
        assert!(
            PLUGIN_QUICK_REPLY_TIMEOUT_MS >= PLUGIN_CALLBACK_DEADLINE_MS + PLUGIN_ACTOR_MARGIN_MS
        );
        assert!(
            PLUGIN_CALLBACK_REPLY_TIMEOUT_MS
                >= (PLUGIN_CALLBACK_DEADLINE_MS * 2) + PLUGIN_ACTOR_MARGIN_MS
        );
        assert_eq!(
            PLUGIN_QUICK_IPC_TIMEOUT_MS - PLUGIN_QUICK_REPLY_TIMEOUT_MS,
            PLUGIN_TRANSPORT_MARGIN_MS
        );
        assert_eq!(
            PLUGIN_CALLBACK_IPC_TIMEOUT_MS - PLUGIN_CALLBACK_REPLY_TIMEOUT_MS,
            PLUGIN_TRANSPORT_MARGIN_MS
        );
        assert_eq!(
            plugin_mutation_ipc_timeout_ms(30_000, 2, 2)
                - 30_000
                - PLUGIN_TRANSPORT_MARGIN_MS,
            (PLUGIN_QUICK_REPLY_TIMEOUT_MS * 2) + (PLUGIN_CALLBACK_REPLY_TIMEOUT_MS * 2),
            "outer mutation budget must contain availability and callback waits for both hook phases"
        );
        assert_eq!(plugin_mutation_ipc_timeout_ms(60_000, 0, 2), 195_000);
        assert_eq!(plugin_mutation_ipc_timeout_ms(30_000, 1, 2), 200_000);
        assert_eq!(plugin_mutation_ipc_timeout_ms(30_000, 2, 2), 235_000);
    }
}
