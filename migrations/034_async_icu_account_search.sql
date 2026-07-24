-- Account names participate in status search, but updating a cached account
-- must never tokenize text or merge FTS segments while holding the interactive
-- SQLite writer. Keep the queue, index and progress state in awayuki.db so the
-- complete search state remains portable with that one file.
ALTER TABLE status_search_index_control
    ADD COLUMN account_merge_debt INTEGER NOT NULL DEFAULT 0
        CHECK (account_merge_debt >= 0);

CREATE TABLE account_search_icu_content (
    docid INTEGER PRIMARY KEY,
    account_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    token_text TEXT NOT NULL,
    UNIQUE(account_id, server_domain)
);

-- ICU4X performs normalization, case folding and word segmentation before the
-- low-priority worker acquires the writer. unicode61 only stores that encoded
-- token stream; search does not need positions or per-column token counts.
CREATE VIRTUAL TABLE account_search_icu_fts USING fts5(
    token_text,
    content = 'account_search_icu_content',
    content_rowid = 'docid',
    tokenize = 'unicode61 remove_diacritics 2',
    detail = 'none',
    columnsize = 0
);

CREATE TRIGGER account_search_icu_content_insert
AFTER INSERT ON account_search_icu_content
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO account_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER account_search_icu_content_update
AFTER UPDATE OF docid, token_text ON account_search_icu_content
WHEN (OLD.docid IS NOT NEW.docid OR OLD.token_text IS NOT NEW.token_text)
 AND (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO account_search_icu_fts(account_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
    INSERT INTO account_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER account_search_icu_content_delete
AFTER DELETE ON account_search_icu_content
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO account_search_icu_fts(account_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
END;

INSERT INTO account_search_icu_fts(account_search_icu_fts, rank)
VALUES ('automerge', 0);
INSERT INTO account_search_icu_fts(account_search_icu_fts, rank)
VALUES ('crisismerge', 2147483647);

CREATE TABLE account_search_index_queue (
    account_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('upsert', 'delete')),
    generation BLOB NOT NULL DEFAULT (randomblob(16))
        CHECK (length(generation) = 16),
    queued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY(account_id, server_domain)
);

CREATE INDEX idx_account_search_index_queue_order
    ON account_search_index_queue(queued_at, account_id, server_domain);

CREATE TABLE account_search_icu_backfill_state (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    cursor_account_id TEXT,
    cursor_server_domain TEXT,
    processed_count INTEGER NOT NULL DEFAULT 0 CHECK (processed_count >= 0),
    total_count INTEGER NOT NULL DEFAULT 0 CHECK (total_count >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (
        (cursor_account_id IS NULL AND cursor_server_domain IS NULL)
        OR (cursor_account_id IS NOT NULL AND cursor_server_domain IS NOT NULL)
    )
);

INSERT INTO account_search_icu_backfill_state(singleton) VALUES (1);

CREATE TRIGGER account_search_index_account_insert
AFTER INSERT ON accounts
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO account_search_index_queue(account_id, server_domain, action)
    VALUES (NEW.id, NEW.server_domain, 'upsert')
    ON CONFLICT(account_id, server_domain) DO UPDATE SET
        action = 'upsert',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;

CREATE TRIGGER account_search_index_account_update
AFTER UPDATE OF id, server_domain, acct, display_name ON accounts
WHEN (
       OLD.id IS NOT NEW.id
    OR OLD.server_domain IS NOT NEW.server_domain
    OR OLD.acct IS NOT NEW.acct
    OR OLD.display_name IS NOT NEW.display_name
 )
 AND (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO account_search_index_queue(account_id, server_domain, action)
    SELECT OLD.id, OLD.server_domain, 'delete'
     WHERE OLD.id IS NOT NEW.id OR OLD.server_domain IS NOT NEW.server_domain
    ON CONFLICT(account_id, server_domain) DO UPDATE SET
        action = 'delete',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');

    INSERT INTO account_search_index_queue(account_id, server_domain, action)
    VALUES (NEW.id, NEW.server_domain, 'upsert')
    ON CONFLICT(account_id, server_domain) DO UPDATE SET
        action = 'upsert',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;

CREATE TRIGGER account_search_index_account_delete
AFTER DELETE ON accounts
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO account_search_index_queue(account_id, server_domain, action)
    VALUES (OLD.id, OLD.server_domain, 'delete')
    ON CONFLICT(account_id, server_domain) DO UPDATE SET
        action = 'delete',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;
