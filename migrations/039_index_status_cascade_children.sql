-- SQLite requires explicit child-key indexes for efficient foreign-key
-- cascades. These tables have account-scoped primary keys, so deleting one
-- status otherwise scans their entire history while holding the sole writer.
CREATE INDEX IF NOT EXISTS idx_status_viewer_state_status_fk
    ON status_viewer_state(status_id, server_domain);

CREATE INDEX IF NOT EXISTS idx_notifications_status_fk
    ON notifications(status_id, server_domain)
    WHERE status_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_startup_sync_reconciliation_status_fk
    ON startup_sync_reconciliation_members(status_id, server_domain);
