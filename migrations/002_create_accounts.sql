CREATE TABLE IF NOT EXISTS accounts (
    id TEXT NOT NULL,
    server_domain TEXT NOT NULL REFERENCES servers(domain),
    username TEXT NOT NULL,
    acct TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    avatar TEXT NOT NULL DEFAULT '',
    avatar_static TEXT NOT NULL DEFAULT '',
    header TEXT NOT NULL DEFAULT '',
    locked INTEGER NOT NULL DEFAULT 0,
    bot INTEGER NOT NULL DEFAULT 0,
    followers_count INTEGER NOT NULL DEFAULT 0,
    following_count INTEGER NOT NULL DEFAULT 0,
    statuses_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    fields_json TEXT,
    emojis_json TEXT,
    PRIMARY KEY (id, server_domain)
);

CREATE INDEX IF NOT EXISTS idx_accounts_acct ON accounts(acct);
