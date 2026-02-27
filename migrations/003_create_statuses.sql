CREATE TABLE IF NOT EXISTS statuses (
    id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    uri TEXT NOT NULL,
    url TEXT,
    created_at TEXT NOT NULL,
    edited_at TEXT,
    account_id TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    visibility TEXT NOT NULL DEFAULT 'public',
    sensitive INTEGER NOT NULL DEFAULT 0,
    spoiler_text TEXT NOT NULL DEFAULT '',
    reblogs_count INTEGER NOT NULL DEFAULT 0,
    favourites_count INTEGER NOT NULL DEFAULT 0,
    replies_count INTEGER NOT NULL DEFAULT 0,
    in_reply_to_id TEXT,
    in_reply_to_account_id TEXT,
    reblog_of_id TEXT,
    language TEXT,
    pinned INTEGER DEFAULT 0,
    favourited INTEGER DEFAULT 0,
    reblogged INTEGER DEFAULT 0,
    muted INTEGER DEFAULT 0,
    bookmarked INTEGER DEFAULT 0,
    poll_json TEXT,
    card_json TEXT,
    mentions_json TEXT,
    tags_json TEXT,
    emojis_json TEXT,
    media_attachments_json TEXT,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (id, server_domain),
    FOREIGN KEY (account_id, server_domain) REFERENCES accounts(id, server_domain)
);

CREATE INDEX IF NOT EXISTS idx_statuses_created_at ON statuses(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_statuses_account ON statuses(account_id, server_domain);
CREATE INDEX IF NOT EXISTS idx_statuses_reblog ON statuses(reblog_of_id) WHERE reblog_of_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_statuses_reply ON statuses(in_reply_to_id) WHERE in_reply_to_id IS NOT NULL;
