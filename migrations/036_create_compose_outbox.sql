-- Durable, portable compose outbox.
--
-- The payload stays in the same SQLite database as the rest of Awayuki so
-- moving the database file keeps pending posts and edits with the application.
CREATE TABLE IF NOT EXISTS compose_outbox (
    id TEXT PRIMARY KEY NOT NULL,
    payload_version INTEGER NOT NULL DEFAULT 1 CHECK (payload_version = 1),
    operation_kind TEXT NOT NULL CHECK (operation_kind IN ('post', 'edit')),
    acting_account_acct TEXT NOT NULL
        REFERENCES login_accounts(acct) ON DELETE CASCADE,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN (
            'queued', 'sending', 'retrying', 'failed', 'uncertain',
            'succeeded', 'cancelled'
        )),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    last_error TEXT,
    next_attempt_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    result_status_id TEXT,
    result_server_domain TEXT
);

CREATE INDEX IF NOT EXISTS idx_compose_outbox_due
    ON compose_outbox(state, next_attempt_at, created_at);

CREATE INDEX IF NOT EXISTS idx_compose_outbox_created
    ON compose_outbox(created_at DESC);
