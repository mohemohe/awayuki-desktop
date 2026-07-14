-- Trigram FTS cannot produce candidates for one- or two-character queries.
-- Index the distinct Unicode code points present in each searchable document
-- in a compact auxiliary FTS table. Query-time character intersection may
-- admit false candidates, but the existing exact substring predicate removes
-- them without scanning the entire statuses table.
CREATE TABLE status_search_char_positions (
    position INTEGER PRIMARY KEY CHECK (position BETWEEN 1 AND 8192)
);

WITH RECURSIVE positions(position) AS (
    VALUES (1)
    UNION ALL
    SELECT position + 1 FROM positions WHERE position < 8192
)
INSERT INTO status_search_char_positions(position)
SELECT position FROM positions;

CREATE VIRTUAL TABLE status_search_char_fts USING fts5(
    search_chars,
    tokenize = 'unicode61 remove_diacritics 0'
);

CREATE TABLE status_search_char_backfill_state (
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

INSERT INTO status_search_char_backfill_state(singleton) VALUES (1);

CREATE TRIGGER status_search_char_status_insert
AFTER INSERT ON statuses
BEGIN
    INSERT INTO status_search_documents(status_id, server_domain)
    VALUES (NEW.id, NEW.server_domain)
    ON CONFLICT(status_id, server_domain) DO NOTHING;

    INSERT INTO status_search_char_fts(rowid, search_chars)
    SELECT d.docid,
           COALESCE((
             SELECT group_concat(DISTINCT printf('u%06x', unicode(substr(lower(
               NEW.content || char(0) || NEW.spoiler_text || char(0) || NEW.uri || char(0) ||
               COALESCE(NEW.url, '') || char(0) || COALESCE(NEW.tags_json, '') || char(0) ||
               COALESCE(a.acct, '') || char(0) || COALESCE(a.display_name, '')
             ), p.position, 1))))
             FROM status_search_char_positions p
             WHERE p.position <= length(lower(
               NEW.content || char(0) || NEW.spoiler_text || char(0) || NEW.uri || char(0) ||
               COALESCE(NEW.url, '') || char(0) || COALESCE(NEW.tags_json, '') || char(0) ||
               COALESCE(a.acct, '') || char(0) || COALESCE(a.display_name, '')
             ))
               AND unicode(substr(lower(
                 NEW.content || char(0) || NEW.spoiler_text || char(0) || NEW.uri || char(0) ||
                 COALESCE(NEW.url, '') || char(0) || COALESCE(NEW.tags_json, '') || char(0) ||
                 COALESCE(a.acct, '') || char(0) || COALESCE(a.display_name, '')
               ), p.position, 1)) > 32
           ), '')
      FROM status_search_documents d
      LEFT JOIN accounts a
        ON a.id = NEW.account_id AND a.server_domain = NEW.server_domain
     WHERE d.status_id = NEW.id AND d.server_domain = NEW.server_domain;
END;

CREATE TRIGGER status_search_char_status_update
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
    DELETE FROM status_search_char_fts
     WHERE rowid = (SELECT docid FROM status_search_documents
                     WHERE status_id = OLD.id AND server_domain = OLD.server_domain);
    UPDATE status_search_documents
       SET status_id = NEW.id, server_domain = NEW.server_domain
     WHERE status_id = OLD.id AND server_domain = OLD.server_domain;
    INSERT INTO status_search_char_fts(rowid, search_chars)
    SELECT d.docid,
           COALESCE((
             SELECT group_concat(DISTINCT printf('u%06x', unicode(substr(lower(
               NEW.content || char(0) || NEW.spoiler_text || char(0) || NEW.uri || char(0) ||
               COALESCE(NEW.url, '') || char(0) || COALESCE(NEW.tags_json, '') || char(0) ||
               COALESCE(a.acct, '') || char(0) || COALESCE(a.display_name, '')
             ), p.position, 1))))
             FROM status_search_char_positions p
             WHERE p.position <= length(lower(
               NEW.content || char(0) || NEW.spoiler_text || char(0) || NEW.uri || char(0) ||
               COALESCE(NEW.url, '') || char(0) || COALESCE(NEW.tags_json, '') || char(0) ||
               COALESCE(a.acct, '') || char(0) || COALESCE(a.display_name, '')
             ))
               AND unicode(substr(lower(
                 NEW.content || char(0) || NEW.spoiler_text || char(0) || NEW.uri || char(0) ||
                 COALESCE(NEW.url, '') || char(0) || COALESCE(NEW.tags_json, '') || char(0) ||
                 COALESCE(a.acct, '') || char(0) || COALESCE(a.display_name, '')
               ), p.position, 1)) > 32
           ), '')
      FROM status_search_documents d
      LEFT JOIN accounts a
        ON a.id = NEW.account_id AND a.server_domain = NEW.server_domain
     WHERE d.status_id = NEW.id AND d.server_domain = NEW.server_domain;
END;

CREATE TRIGGER status_search_char_status_delete
AFTER DELETE ON statuses
BEGIN
    DELETE FROM status_search_char_fts
     WHERE rowid = (SELECT docid FROM status_search_documents
                     WHERE status_id = OLD.id AND server_domain = OLD.server_domain);
END;

CREATE TRIGGER status_search_char_account_update
AFTER UPDATE OF acct, display_name ON accounts
WHEN OLD.acct IS NOT NEW.acct OR OLD.display_name IS NOT NEW.display_name
BEGIN
    DELETE FROM status_search_char_fts
     WHERE rowid IN (
       SELECT d.docid FROM status_search_documents d
       JOIN statuses s ON s.id = d.status_id AND s.server_domain = d.server_domain
       WHERE s.account_id = NEW.id AND s.server_domain = NEW.server_domain
     );
    INSERT INTO status_search_char_fts(rowid, search_chars)
    SELECT d.docid,
           COALESCE((
             SELECT group_concat(DISTINCT printf('u%06x', unicode(substr(lower(
               s.content || char(0) || s.spoiler_text || char(0) || s.uri || char(0) ||
               COALESCE(s.url, '') || char(0) || COALESCE(s.tags_json, '') || char(0) ||
               NEW.acct || char(0) || NEW.display_name
             ), p.position, 1))))
             FROM status_search_char_positions p
             WHERE p.position <= length(lower(
               s.content || char(0) || s.spoiler_text || char(0) || s.uri || char(0) ||
               COALESCE(s.url, '') || char(0) || COALESCE(s.tags_json, '') || char(0) ||
               NEW.acct || char(0) || NEW.display_name
             ))
               AND unicode(substr(lower(
                 s.content || char(0) || s.spoiler_text || char(0) || s.uri || char(0) ||
                 COALESCE(s.url, '') || char(0) || COALESCE(s.tags_json, '') || char(0) ||
                 NEW.acct || char(0) || NEW.display_name
               ), p.position, 1)) > 32
           ), '')
      FROM status_search_documents d
      JOIN statuses s ON s.id = d.status_id AND s.server_domain = d.server_domain
     WHERE s.account_id = NEW.id AND s.server_domain = NEW.server_domain;
END;
