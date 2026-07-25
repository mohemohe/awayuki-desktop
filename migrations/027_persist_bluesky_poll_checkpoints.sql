-- Keep Bluesky revision baselines inside the portable database. One compact
-- checkpoint per signed-in account/stream avoids replaying old notifications
-- after restart without introducing a second state file.
CREATE TABLE bluesky_poll_checkpoints (
    account_acct TEXT NOT NULL,
    stream_key TEXT NOT NULL,
    checkpoint_json TEXT NOT NULL CHECK (json_valid(checkpoint_json)),
    updated_at TEXT NOT NULL,
    PRIMARY KEY (account_acct, stream_key),
    FOREIGN KEY (account_acct) REFERENCES login_accounts(acct) ON DELETE CASCADE
);
