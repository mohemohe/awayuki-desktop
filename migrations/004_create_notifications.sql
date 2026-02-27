CREATE TABLE IF NOT EXISTS notifications (
    id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    notification_type TEXT NOT NULL,
    created_at TEXT NOT NULL,
    account_id TEXT NOT NULL,
    status_id TEXT,
    read_at TEXT,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (id, server_domain)
);

CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_notifications_unread ON notifications(read_at) WHERE read_at IS NULL;
