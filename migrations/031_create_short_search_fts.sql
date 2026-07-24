-- Replace migration 028's Unicode-codepoint set with an order-preserving
-- unigram/bigram FTS5 index. `awayuki_short` is registered on every SQLx
-- connection before migrations run. Existing rows are intentionally left for
-- the bounded post-startup backfill; live inserts and edits stay indexed.
--
-- Account names are not duplicated into every status document. Short account
-- matches are resolved from the small accounts table at query time and joined
-- through idx_statuses_account. An account profile edit therefore remains one
-- bounded row update instead of rebuilding every status written by it.
DROP TRIGGER IF EXISTS status_search_char_status_insert;
DROP TRIGGER IF EXISTS status_search_char_status_update;
DROP TRIGGER IF EXISTS status_search_char_status_delete;
DROP TRIGGER IF EXISTS status_search_char_account_update;
DROP TABLE IF EXISTS status_search_char_fts;
DROP TABLE IF EXISTS status_search_char_positions;
DROP TABLE IF EXISTS status_search_char_backfill_state;

-- Account terms are resolved once from accounts and expanded through
-- idx_statuses_account at query time. Rebuilding every trigram FTS document
-- written by a prolific account on each profile refresh can monopolize the
-- single writer, and is unnecessary now that both short and long terms use
-- the account candidate branch.
DROP TRIGGER IF EXISTS status_search_fts_account_update;
DROP TRIGGER IF EXISTS status_search_fts_account_insert;
DROP TRIGGER IF EXISTS status_search_fts_account_delete;

CREATE TABLE status_search_short_content (
    docid INTEGER NOT NULL PRIMARY KEY,
    status_id TEXT NOT NULL,
    server_domain TEXT NOT NULL,
    search_text TEXT NOT NULL,
    UNIQUE(status_id, server_domain)
);

CREATE VIRTUAL TABLE status_search_short_fts USING fts5(
    search_text,
    content = 'status_search_short_content',
    content_rowid = 'docid',
    tokenize = 'awayuki_short',
    detail = 'none'
);

CREATE TRIGGER status_search_short_content_insert
AFTER INSERT ON status_search_short_content
BEGIN
    INSERT INTO status_search_short_fts(rowid, search_text)
    VALUES (NEW.docid, NEW.search_text);
END;

CREATE TRIGGER status_search_short_content_update
AFTER UPDATE OF docid, search_text ON status_search_short_content
WHEN OLD.docid IS NOT NEW.docid OR OLD.search_text IS NOT NEW.search_text
BEGIN
    INSERT INTO status_search_short_fts(status_search_short_fts, rowid, search_text)
    VALUES ('delete', OLD.docid, OLD.search_text);
    INSERT INTO status_search_short_fts(rowid, search_text)
    VALUES (NEW.docid, NEW.search_text);
END;

CREATE TRIGGER status_search_short_content_delete
AFTER DELETE ON status_search_short_content
BEGIN
    INSERT INTO status_search_short_fts(status_search_short_fts, rowid, search_text)
    VALUES ('delete', OLD.docid, OLD.search_text);
END;

CREATE TABLE status_search_short_backfill_state (
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

INSERT INTO status_search_short_backfill_state(singleton) VALUES (1);

CREATE TRIGGER status_search_short_status_insert
AFTER INSERT ON statuses
BEGIN
    INSERT INTO status_search_documents(status_id, server_domain)
    VALUES (NEW.id, NEW.server_domain)
    ON CONFLICT(status_id, server_domain) DO NOTHING;

    INSERT INTO status_search_short_content(docid, status_id, server_domain, search_text)
    SELECT d.docid,
           NEW.id,
           NEW.server_domain,
           NEW.content || char(0) ||
           NEW.spoiler_text || char(0) ||
           NEW.uri || char(0) ||
           COALESCE(NEW.url, '') || char(0) ||
           COALESCE(NEW.tags_json, '')
      FROM status_search_documents d
     WHERE d.status_id = NEW.id AND d.server_domain = NEW.server_domain
    ON CONFLICT(docid) DO UPDATE SET
        status_id = excluded.status_id,
        server_domain = excluded.server_domain,
        search_text = excluded.search_text;
END;

CREATE TRIGGER status_search_short_status_update
AFTER UPDATE OF id, server_domain, content, spoiler_text, uri, url, tags_json ON statuses
WHEN OLD.id IS NOT NEW.id
  OR OLD.server_domain IS NOT NEW.server_domain
  OR OLD.content IS NOT NEW.content
  OR OLD.spoiler_text IS NOT NEW.spoiler_text
  OR OLD.uri IS NOT NEW.uri
  OR OLD.url IS NOT NEW.url
  OR OLD.tags_json IS NOT NEW.tags_json
BEGIN
    -- A pre-023 legacy row may not have reached the shared docid backfill yet.
    -- Allocate its stable docid here so the live edit becomes searchable now.
    INSERT INTO status_search_documents(status_id, server_domain)
    SELECT NEW.id, NEW.server_domain
     WHERE NOT EXISTS (
         SELECT 1 FROM status_search_documents d
          WHERE (d.status_id = OLD.id AND d.server_domain = OLD.server_domain)
             OR (d.status_id = NEW.id AND d.server_domain = NEW.server_domain)
     )
    ON CONFLICT(status_id, server_domain) DO NOTHING;

    INSERT INTO status_search_short_content(docid, status_id, server_domain, search_text)
    SELECT d.docid,
           NEW.id,
           NEW.server_domain,
           NEW.content || char(0) ||
           NEW.spoiler_text || char(0) ||
           NEW.uri || char(0) ||
           COALESCE(NEW.url, '') || char(0) ||
           COALESCE(NEW.tags_json, '')
      FROM status_search_documents d
     WHERE (d.status_id = OLD.id AND d.server_domain = OLD.server_domain)
        OR (d.status_id = NEW.id AND d.server_domain = NEW.server_domain)
     LIMIT 1
    ON CONFLICT(docid) DO UPDATE SET
        status_id = excluded.status_id,
        server_domain = excluded.server_domain,
        search_text = excluded.search_text;
END;

CREATE TRIGGER status_search_short_status_delete
AFTER DELETE ON statuses
BEGIN
    DELETE FROM status_search_short_content
     WHERE status_id = OLD.id AND server_domain = OLD.server_domain;
END;

-- Bound automatic merge work on the interactive writer just as migration 025
-- does for the trigram index.
INSERT INTO status_search_short_fts(status_search_short_fts, rank)
VALUES ('automerge', 8);

INSERT INTO status_search_short_fts(status_search_short_fts, rank)
VALUES ('crisismerge', 128);
