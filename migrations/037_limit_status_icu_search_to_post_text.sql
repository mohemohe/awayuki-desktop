-- "Status full-text search" is intentionally limited to the text authored as
-- part of the post. URI, URL and structured tag metadata have dedicated SQL
-- columns/tables and must not create full-text matches.
--
-- Existing token streams may contain those metadata fields. Empty only the
-- FTS postings while trigger writes are disabled, then let the existing
-- low-priority backfill replace each legacy content row. Keeping those rows
-- avoids a synchronous million-row DELETE during startup. The scope version
-- prevents search fallbacks from consulting a legacy token stream while the
-- rebuild is incomplete. Account search is separate and remains untouched.
UPDATE status_search_index_control
   SET index_updates_enabled = 0
 WHERE singleton = 1;

INSERT INTO status_search_icu_fts(status_search_icu_fts)
VALUES ('delete-all');

ALTER TABLE status_search_icu_content
    ADD COLUMN text_scope_version INTEGER NOT NULL DEFAULT 1;

UPDATE status_search_icu_backfill_state
   SET cursor_status_id = NULL,
       cursor_server_domain = NULL,
       processed_count = 0,
       total_count = coalesce((
           SELECT value FROM cache_counters WHERE name = 'statuses'
       ), 0),
       completed = CASE
           WHEN coalesce((
                    SELECT value FROM cache_counters WHERE name = 'statuses'
                ), 0) = 0
           THEN 1
           ELSE 0
       END,
       updated_at = datetime('now')
 WHERE singleton = 1;

DROP TRIGGER IF EXISTS status_search_index_status_update;
DROP TRIGGER IF EXISTS status_search_icu_content_insert;
DROP TRIGGER IF EXISTS status_search_icu_content_update;
DROP TRIGGER IF EXISTS status_search_icu_content_delete;

CREATE TRIGGER status_search_icu_content_insert
AFTER INSERT ON status_search_icu_content
WHEN NEW.text_scope_version = 2
 AND (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER status_search_icu_content_update
AFTER UPDATE OF docid, token_text, text_scope_version ON status_search_icu_content
WHEN (
       OLD.docid IS NOT NEW.docid
    OR OLD.token_text IS NOT NEW.token_text
    OR OLD.text_scope_version IS NOT NEW.text_scope_version
 )
 AND (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_icu_fts(status_search_icu_fts, rowid, token_text)
    SELECT 'delete', OLD.docid, OLD.token_text
     WHERE OLD.text_scope_version = 2;
    INSERT INTO status_search_icu_fts(rowid, token_text)
    SELECT NEW.docid, NEW.token_text
     WHERE NEW.text_scope_version = 2;
END;

CREATE TRIGGER status_search_icu_content_delete
AFTER DELETE ON status_search_icu_content
WHEN OLD.text_scope_version = 2
 AND (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_icu_fts(status_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
END;

CREATE TRIGGER status_search_index_status_update
AFTER UPDATE OF id, server_domain, content, spoiler_text ON statuses
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
 AND (
       OLD.id IS NOT NEW.id
    OR OLD.server_domain IS NOT NEW.server_domain
    OR OLD.content IS NOT NEW.content
    OR OLD.spoiler_text IS NOT NEW.spoiler_text
 )
BEGIN
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

UPDATE status_search_index_control
   SET index_updates_enabled = 1,
       merge_debt = 0
 WHERE singleton = 1;
