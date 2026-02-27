-- Add access_token to login_accounts for cross-platform credential storage
ALTER TABLE login_accounts ADD COLUMN access_token TEXT NOT NULL DEFAULT '';

-- Store OAuth app credentials per server domain
CREATE TABLE IF NOT EXISTS client_credentials (
    server_domain TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    client_secret TEXT NOT NULL
);
