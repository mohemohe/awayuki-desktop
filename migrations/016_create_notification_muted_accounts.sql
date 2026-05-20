CREATE TABLE IF NOT EXISTS notification_muted_accounts (
    account_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    acct TEXT NOT NULL DEFAULT '',
    display_name TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (account_id, server_domain)
);

CREATE INDEX IF NOT EXISTS idx_notification_muted_accounts_acct
    ON notification_muted_accounts(server_domain, acct);
