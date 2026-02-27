CREATE TABLE IF NOT EXISTS login_accounts (
    acct TEXT PRIMARY KEY,
    server_domain TEXT NOT NULL,
    account_id TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    avatar TEXT NOT NULL DEFAULT '',
    is_active INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS column_configs (
    id TEXT PRIMARY KEY,
    account_acct TEXT NOT NULL REFERENCES login_accounts(acct),
    column_type TEXT NOT NULL,
    column_param TEXT,
    position INTEGER NOT NULL,
    width INTEGER DEFAULT 350,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_column_configs_order ON column_configs(account_acct, position);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
