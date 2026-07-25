-- The first ICU index implementation stored only word-like segments. The
-- current tokenizer also stores ICU-delimited punctuation and emoji so those
-- queries never fall back to a foreground cache scan. Keep existing postings
-- available while the low-priority worker refreshes them: resetting this
-- durable cursor is O(1), unlike deleting or synchronously re-enqueuing a
-- million-row portable cache during startup.
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

UPDATE account_search_icu_backfill_state
   SET cursor_account_id = NULL,
       cursor_server_domain = NULL,
       processed_count = 0,
       total_count = coalesce((
           SELECT value FROM cache_counters WHERE name = 'accounts'
       ), 0),
       completed = CASE
           WHEN coalesce((
                    SELECT value FROM cache_counters WHERE name = 'accounts'
                ), 0) = 0
           THEN 1
           ELSE 0
       END,
       updated_at = datetime('now')
 WHERE singleton = 1;
