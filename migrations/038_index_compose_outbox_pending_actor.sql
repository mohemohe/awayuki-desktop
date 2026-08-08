-- Keep per-account FIFO checks proportional to pending work, not to the
-- lifetime history of completed outbox items.
CREATE INDEX IF NOT EXISTS idx_compose_outbox_actor_pending
    ON compose_outbox(acting_account_acct, created_at)
    WHERE state IN ('queued', 'sending', 'retrying');
