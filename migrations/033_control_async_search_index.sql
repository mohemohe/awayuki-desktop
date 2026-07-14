-- Bulk cache maintenance must not turn one DELETE into one million durable
-- queue writes. The flag lives in the same portable database and is changed
-- inside the maintenance writer transaction, so another process can never
-- observe committed disabled indexing.
CREATE TABLE status_search_index_control (
    singleton INTEGER NOT NULL PRIMARY KEY CHECK (singleton = 1),
    index_updates_enabled INTEGER NOT NULL DEFAULT 1
        CHECK (index_updates_enabled IN (0, 1)),
    merge_debt INTEGER NOT NULL DEFAULT 0 CHECK (merge_debt >= 0)
);

INSERT INTO status_search_index_control(singleton, index_updates_enabled, merge_debt)
VALUES (1, 1, 0);

-- The same transaction-scoped flag also makes explicit bulk cache clears
-- O(1) for the cached row counters instead of updating one counter row once
-- for every deleted status/account.
DROP TRIGGER IF EXISTS cache_counter_status_insert;
DROP TRIGGER IF EXISTS cache_counter_status_delete;
DROP TRIGGER IF EXISTS cache_counter_account_insert;
DROP TRIGGER IF EXISTS cache_counter_account_delete;

CREATE TRIGGER cache_counter_status_insert
AFTER INSERT ON statuses
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    UPDATE cache_counters
       SET value = value + 1, updated_at = datetime('now')
     WHERE name = 'statuses';
END;

CREATE TRIGGER cache_counter_status_delete
AFTER DELETE ON statuses
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    UPDATE cache_counters
       SET value = MAX(0, value - 1), updated_at = datetime('now')
     WHERE name = 'statuses';
END;

CREATE TRIGGER cache_counter_account_insert
AFTER INSERT ON accounts
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    UPDATE cache_counters
       SET value = value + 1, updated_at = datetime('now')
     WHERE name = 'accounts';
END;

CREATE TRIGGER cache_counter_account_delete
AFTER DELETE ON accounts
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    UPDATE cache_counters
       SET value = MAX(0, value - 1), updated_at = datetime('now')
     WHERE name = 'accounts';
END;

DROP TRIGGER IF EXISTS status_search_icu_content_insert;
DROP TRIGGER IF EXISTS status_search_icu_content_update;
DROP TRIGGER IF EXISTS status_search_icu_content_delete;

CREATE TRIGGER status_search_icu_content_insert
AFTER INSERT ON status_search_icu_content
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER status_search_icu_content_update
AFTER UPDATE OF docid, token_text ON status_search_icu_content
WHEN (OLD.docid IS NOT NEW.docid OR OLD.token_text IS NOT NEW.token_text)
 AND (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_icu_fts(status_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
    INSERT INTO status_search_icu_fts(rowid, token_text)
    VALUES (NEW.docid, NEW.token_text);
END;

CREATE TRIGGER status_search_icu_content_delete
AFTER DELETE ON status_search_icu_content
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_icu_fts(status_search_icu_fts, rowid, token_text)
    VALUES ('delete', OLD.docid, OLD.token_text);
END;

DROP TRIGGER IF EXISTS status_search_index_status_insert;
DROP TRIGGER IF EXISTS status_search_index_status_update;
DROP TRIGGER IF EXISTS status_search_index_status_delete;

CREATE TRIGGER status_search_index_status_insert
AFTER INSERT ON statuses
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
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
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
 AND (
       OLD.id IS NOT NEW.id
    OR OLD.server_domain IS NOT NEW.server_domain
    OR OLD.content IS NOT NEW.content
    OR OLD.spoiler_text IS NOT NEW.spoiler_text
    OR OLD.uri IS NOT NEW.uri
    OR OLD.url IS NOT NEW.url
    OR OLD.tags_json IS NOT NEW.tags_json
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

CREATE TRIGGER status_search_index_status_delete
AFTER DELETE ON statuses
WHEN (SELECT index_updates_enabled
        FROM status_search_index_control
       WHERE singleton = 1) = 1
BEGIN
    INSERT INTO status_search_index_queue(status_id, server_domain, action)
    VALUES (OLD.id, OLD.server_domain, 'delete')
    ON CONFLICT(status_id, server_domain) DO UPDATE SET
        action = 'delete',
        generation = randomblob(16),
        queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
END;
