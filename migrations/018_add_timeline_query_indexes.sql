CREATE INDEX IF NOT EXISTS idx_statuses_account_created
    ON statuses(account_id, server_domain, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_statuses_account_pinned_created
    ON statuses(account_id, server_domain, pinned, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_statuses_created_id
    ON statuses(created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_timeline_entries_status_latest
    ON timeline_entries(server_domain, status_id, position_at DESC);
