-- Unified and SQLite-global columns belong to the application layout, not to
-- whichever account happens to be active. SQLite cannot drop a NOT NULL
-- constraint in place, so rebuild the small configuration table atomically.
CREATE TABLE column_configs_v26 (
    id TEXT PRIMARY KEY,
    account_acct TEXT REFERENCES login_accounts(acct),
    column_type TEXT NOT NULL,
    column_param TEXT,
    position INTEGER NOT NULL,
    width INTEGER DEFAULT 350,
    name TEXT,
    max_statuses INTEGER DEFAULT 100,
    pane_index INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO column_configs_v26 (
    id,
    account_acct,
    column_type,
    column_param,
    position,
    width,
    name,
    max_statuses,
    pane_index,
    created_at
)
SELECT
    id,
    CASE
        WHEN column_type IN (
            'home',
            'public',
            'notification',
            'bookmarks',
            'favourites',
            'custom',
            'yq',
            'search',
            'user_bookmarks',
            'thread',
            'profile',
            'airContext'
        ) THEN NULL
        ELSE account_acct
    END,
    column_type,
    column_param,
    position,
    width,
    name,
    max_statuses,
    pane_index,
    created_at
FROM column_configs;

DROP TABLE column_configs;
ALTER TABLE column_configs_v26 RENAME TO column_configs;

CREATE INDEX idx_column_configs_order
    ON column_configs(account_acct, pane_index, position);
