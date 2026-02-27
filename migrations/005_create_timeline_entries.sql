CREATE TABLE IF NOT EXISTS timeline_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timeline_type TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    status_id TEXT NOT NULL,
    account_acct TEXT NOT NULL,
    position_at TEXT NOT NULL,
    UNIQUE(timeline_type, server_domain, status_id, account_acct)
);

CREATE INDEX IF NOT EXISTS idx_timeline_entries_lookup
    ON timeline_entries(timeline_type, account_acct, position_at DESC);
