-- Keep interactive status persistence independent from full-text indexing.
-- The previous trigram and character n-gram triggers could run an unbounded
-- FTS5 segment merge while holding Awayuki's only SQLite writer. Status
-- transactions now enqueue one coalesced key; a post-ready low-priority worker
-- performs ICU4X word segmentation and updates this index separately.
DROP TRIGGER IF EXISTS status_search_fts_status_insert;
DROP TRIGGER IF EXISTS status_search_fts_status_update;
DROP TRIGGER IF EXISTS status_search_fts_status_delete;
DROP TRIGGER IF EXISTS status_search_short_status_insert;
DROP TRIGGER IF EXISTS status_search_short_status_update;
DROP TRIGGER IF EXISTS status_search_short_status_delete;

-- The legacy indexes receive no new writes or reads after the triggers above
-- are dropped. Dropping multi-GB FTS tables here would block startup, so the
-- dormant payload is left until explicit cache/offline maintenance.

CREATE TABLE status_search_icu_content (
    docid INTEGER PRIMARY KEY,
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    token_text TEXT NOT NULL,
    UNIQUE(status_id, server_domain)
);

-- ICU4X produces normalized word tokens before the writer is acquired.
-- unicode61 only indexes that already segmented token stream. Position data is
-- unnecessary because search uses boolean word-prefix membership rather than
-- phrase ranking, so detail=none avoids the storage/write cost of offsets.
CREATE VIRTUAL TABLE status_search_icu_fts USING fts5(
    token_text,
    content = 'status_search_icu_content',
    content_rowid = 'docid',
    tokenize = 'unicode61 remove_diacritics 2',
    detail = 'none',
    columnsize = 0
);

CREATE TRIGGER status_search_icu_content_insert
AFTER INSERT ON status_search_icu_content
BEGIN
    INSERT INTO status_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER status_search_icu_content_update
AFTER UPDATE OF docid, token_text ON status_search_icu_content
WHEN OLD.docid IS NOT NEW.docid OR OLD.token_text IS NOT NEW.token_text
BEGIN
    INSERT INTO status_search_icu_fts(status_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
    INSERT INTO status_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER status_search_icu_content_delete
AFTER DELETE ON status_search_icu_content
BEGIN
    INSERT INTO status_search_icu_fts(status_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
END;

INSERT INTO status_search_icu_fts(status_search_icu_fts, rank)
VALUES ('automerge', 0);
INSERT INTO status_search_icu_fts(status_search_icu_fts, rank)
VALUES ('crisismerge', 2147483647);

CREATE TABLE status_search_index_queue (
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('upsert', 'delete')),
    -- A random generation avoids an ABA race if two app processes happen to
    -- open the same portable database. Integer revisions reset after dequeue.
    generation BLOB NOT NULL DEFAULT (randomblob(16)) CHECK (length(generation) = 16),
    queued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY(status_id, server_domain)
);

CREATE INDEX idx_status_search_index_queue_order
    ON status_search_index_queue(queued_at, status_id, server_domain);

CREATE TABLE status_search_icu_backfill_state (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    cursor_status_id TEXT,
    cursor_server_domain TEXT,
    processed_count INTEGER NOT NULL DEFAULT 0 CHECK (processed_count >= 0),
    total_count INTEGER NOT NULL DEFAULT 0 CHECK (total_count >= 0),
    completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (
        (cursor_status_id IS NULL AND cursor_server_domain IS NULL)
        OR (cursor_status_id IS NOT NULL AND cursor_server_domain IS NOT NULL)
    )
);

INSERT INTO status_search_icu_backfill_state(singleton) VALUES (1);

CREATE TRIGGER status_search_index_status_insert
AFTER INSERT ON statuses
BEGIN
    INSERT INTO status_search_index_queue(status_id, server_domain, action)
    VALUES (NEW.id, NEW.server_domain, 'upsert')
    ON CONFLICT(status_id, server_domain) DO UPDATE SET
        action = 'upsert',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;

CREATE TRIGGER status_search_index_status_update
AFTER UPDATE OF id, server_domain, content, spoiler_text, uri, url, tags_json ON statuses
WHEN OLD.id IS NOT NEW.id
  OR OLD.server_domain IS NOT NEW.server_domain
  OR OLD.content IS NOT NEW.content
  OR OLD.spoiler_text IS NOT NEW.spoiler_text
  OR OLD.uri IS NOT NEW.uri
  OR OLD.url IS NOT NEW.url
  OR OLD.tags_json IS NOT NEW.tags_json
BEGIN
    -- Status identity changes are rare, but the old key must not leave an
    -- orphaned ICU document behind.
    INSERT INTO status_search_index_queue(status_id, server_domain, action)
    SELECT OLD.id, OLD.server_domain, 'delete'
     WHERE OLD.id IS NOT NEW.id OR OLD.server_domain IS NOT NEW.server_domain
    ON CONFLICT(status_id, server_domain) DO UPDATE SET
        action = 'delete',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');

    INSERT INTO status_search_index_queue(status_id, server_domain, action)
    VALUES (NEW.id, NEW.server_domain, 'upsert')
    ON CONFLICT(status_id, server_domain) DO UPDATE SET
        action = 'upsert',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;

CREATE TRIGGER status_search_index_status_delete
AFTER DELETE ON statuses
BEGIN
    INSERT INTO status_search_index_queue(status_id, server_domain, action)
    VALUES (OLD.id, OLD.server_domain, 'delete')
    ON CONFLICT(status_id, server_domain) DO UPDATE SET
        action = 'delete',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;
