-- Migration 028 maintained the short-query candidate index from SQLite
-- triggers. Each normal status write decomposed up to 8192 Unicode characters
-- inside the writer transaction, which can monopolize the single SQLite
-- writer for minutes on a large client cache. Short searches are isolated on
-- the WAL analytics readers instead; the primary write path must stay bounded.
DROP TRIGGER IF EXISTS status_search_char_status_insert;
DROP TRIGGER IF EXISTS status_search_char_status_update;
DROP TRIGGER IF EXISTS status_search_char_status_delete;
DROP TRIGGER IF EXISTS status_search_char_account_update;
