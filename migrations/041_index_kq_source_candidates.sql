-- KQ sources narrow the status cache before the in-memory predicate runs.
-- `user:"acct"` compares acct/username case-insensitively, and `mentions`
-- collects replies to or mentions of a login account. Without these indexes
-- every such source walked the complete status cache.
CREATE INDEX IF NOT EXISTS idx_accounts_acct_nocase
    ON accounts(acct COLLATE NOCASE);

CREATE INDEX IF NOT EXISTS idx_accounts_username_nocase
    ON accounts(username COLLATE NOCASE);

CREATE INDEX IF NOT EXISTS idx_statuses_reply_account
    ON statuses(in_reply_to_account_id, server_domain)
    WHERE in_reply_to_account_id IS NOT NULL;

-- Status writers store an empty mention list as NULL.
CREATE INDEX IF NOT EXISTS idx_statuses_mentions_present
    ON statuses(server_domain, id)
    WHERE mentions_json IS NOT NULL;
