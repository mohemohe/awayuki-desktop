-- Legacy databases can contain close to a million cached statuses. Building a
-- trigram FTS index for every row inside the blocking schema migration keeps
-- the Tauri setup callback from returning for minutes. Keep the backfill
-- cursor in the portable SQLite database so small post-startup transactions
-- can resume safely after an interruption.
CREATE TABLE status_search_backfill_state (
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

INSERT INTO status_search_backfill_state(singleton)
VALUES (1);

-- SQLite fires an UPDATE OF trigger when a column appears in SET, even when
-- its value did not change. Startup synchronization upserts cached statuses,
-- so the old trigger needlessly deleted and rebuilt large trigram postings.
DROP TRIGGER IF EXISTS status_search_fts_status_update;
CREATE TRIGGER status_search_fts_status_update
AFTER UPDATE OF id, server_domain, account_id, content, spoiler_text, uri, url, tags_json ON statuses
WHEN OLD.id IS NOT NEW.id
  OR OLD.server_domain IS NOT NEW.server_domain
  OR OLD.account_id IS NOT NEW.account_id
  OR OLD.content IS NOT NEW.content
  OR OLD.spoiler_text IS NOT NEW.spoiler_text
  OR OLD.uri IS NOT NEW.uri
  OR OLD.url IS NOT NEW.url
  OR OLD.tags_json IS NOT NEW.tags_json
BEGIN
    DELETE FROM status_search_fts
     WHERE rowid = (
         SELECT docid
           FROM status_search_documents
          WHERE status_id = OLD.id
            AND server_domain = OLD.server_domain
     );

    UPDATE status_search_documents
       SET status_id = NEW.id,
           server_domain = NEW.server_domain
     WHERE status_id = OLD.id
       AND server_domain = OLD.server_domain;

    INSERT INTO status_search_fts (
        rowid,
        content,
        spoiler_text,
        uri,
        url,
        tags,
        account_acct,
        account_display_name
    )
    SELECT
        d.docid,
        NEW.content,
        NEW.spoiler_text,
        NEW.uri,
        COALESCE(NEW.url, ''),
        COALESCE(NEW.tags_json, ''),
        COALESCE(a.acct, ''),
        COALESCE(a.display_name, '')
    FROM status_search_documents d
    LEFT JOIN accounts a
      ON a.id = NEW.account_id
     AND a.server_domain = NEW.server_domain
    WHERE d.status_id = NEW.id
      AND d.server_domain = NEW.server_domain;
END;

DROP TRIGGER IF EXISTS status_search_fts_account_update;
CREATE TRIGGER status_search_fts_account_update
AFTER UPDATE OF acct, display_name ON accounts
WHEN OLD.acct IS NOT NEW.acct
  OR OLD.display_name IS NOT NEW.display_name
BEGIN
    UPDATE status_search_fts
       SET account_acct = NEW.acct,
           account_display_name = NEW.display_name
     WHERE rowid IN (
         SELECT d.docid
           FROM status_search_documents d
           JOIN statuses s
             ON s.id = d.status_id
            AND s.server_domain = d.server_domain
          WHERE s.account_id = NEW.id
            AND s.server_domain = NEW.server_domain
     );
END;
