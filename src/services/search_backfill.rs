use std::time::Duration;

use serde::Serialize;
use sqlx::{Sqlite, SqlitePool, Transaction};
use tokio::sync::mpsc;

// Interactive timeline/stream writes always take priority over rebuilding a
// derived search index. A smaller chunk means a large portable database may
// finish indexing later, but the only SQLite writer is returned frequently and
// the app remains usable throughout the process.
pub const DEFAULT_CHUNK_SIZE: i64 = 64;
const MAX_CHUNK_SIZE: i64 = 500;
const CHUNK_YIELD_DELAY: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchBackfillProgress {
    pub processed_count: u64,
    pub total_count: u64,
    pub completed: bool,
}

/// Whether local-search queries may rely exclusively on the FTS candidate set.
///
/// Until this returns true callers must retain their exact LIKE path; otherwise
/// statuses not reached by the resumable cursor would silently disappear from
/// search results.
pub async fn is_complete(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let state_table_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
              WHERE type = 'table' AND name = 'status_search_backfill_state'
         )",
    )
    .fetch_one(pool)
    .await?;
    if !state_table_exists {
        // Before migration 023, migration 020 itself was atomic and did not
        // return until its full index existed. This also keeps focused search
        // fixtures that intentionally stop at schema version 020 valid.
        return Ok(true);
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT completed FROM status_search_backfill_state WHERE singleton = 1",
    )
    .fetch_one(pool)
    .await
}

/// Backfill the legacy status cache in bounded transactions. Cursor progress is
/// committed together with each FTS chunk and therefore resumes after a crash
/// without any sidecar file or system-owned state.
pub async fn run_to_completion(
    writer: &SqlitePool,
    reader: &SqlitePool,
    progress_tx: Option<&mpsc::Sender<SearchBackfillProgress>>,
) -> Result<SearchBackfillProgress, sqlx::Error> {
    loop {
        let progress = run_chunk(writer, reader, DEFAULT_CHUNK_SIZE).await?;
        if let Some(progress_tx) = progress_tx {
            let _ = progress_tx.send(progress.clone()).await;
        }
        if progress.completed {
            return Ok(progress);
        }

        // Releasing the single writer connection at every chunk is necessary
        // but not sufficient under a continuously-ready loop. Yield briefly so
        // startup synchronization and interactive writes get a fair turn.
        tokio::time::sleep(CHUNK_YIELD_DELAY).await;
    }
}

pub async fn run_chunk(
    writer: &SqlitePool,
    reader: &SqlitePool,
    requested_chunk_size: i64,
) -> Result<SearchBackfillProgress, sqlx::Error> {
    let chunk_size = requested_chunk_size.clamp(1, MAX_CHUNK_SIZE);
    // The first-run equality probe can scan a very large FTS virtual table.
    // Complete every read on a WAL reader before acquiring Awayuki's only
    // writer connection; only the bounded mutation chunk belongs in the
    // writer transaction below.
    let preflight_state = load_state_from_pool(reader).await?;
    if preflight_state.completed {
        return Ok(preflight_state.progress());
    }
    let initial_counts =
        if preflight_state.cursor_status_id.is_none() && preflight_state.processed_count == 0 {
            Some(count_index_rows(reader).await?)
        } else {
            None
        };

    let mut transaction = writer.begin().await?;
    let state = load_state(&mut transaction).await?;
    if state.completed {
        transaction.commit().await?;
        return Ok(state.progress());
    }
    let mut known_total_count = state.total_count;

    // Migration 020 already performed a transactional full backfill for fresh
    // databases and for installations that completed it before migration 023.
    // Detect that once rather than walking every existing key in empty chunks.
    if state.cursor_status_id.is_none() && state.processed_count == 0 {
        let (status_count, document_count, fts_count) = initial_counts.ok_or_else(|| {
            sqlx::Error::Protocol(
                "search backfill state changed before the initial count probe".to_string(),
            )
        })?;
        if status_count == document_count && document_count == fts_count {
            mark_complete(&mut transaction, status_count).await?;
            transaction.commit().await?;
            return Ok(SearchBackfillProgress {
                processed_count: non_negative_u64(status_count),
                total_count: non_negative_u64(status_count),
                completed: true,
            });
        }
        known_total_count = status_count.max(0);
        sqlx::query(
            "UPDATE status_search_backfill_state
                SET total_count = ?, updated_at = datetime('now')
              WHERE singleton = 1",
        )
        .bind(status_count.max(0))
        .execute(&mut *transaction)
        .await?;
    }

    let keys = sqlx::query_as::<_, (String, String)>(
        "SELECT id, server_domain
           FROM statuses
          WHERE ? IS NULL
             OR id > ?
             OR (id = ? AND server_domain > ?)
          ORDER BY id, server_domain
          LIMIT ?",
    )
    .bind(state.cursor_status_id.as_deref())
    .bind(state.cursor_status_id.as_deref())
    .bind(state.cursor_status_id.as_deref())
    .bind(state.cursor_server_domain.as_deref())
    .bind(chunk_size)
    .fetch_all(&mut *transaction)
    .await?;

    if keys.is_empty() {
        // Inserts are indexed by migration 020's trigger, including keys that
        // sort before the persisted cursor. Reaching the end is therefore an
        // exact completion point rather than a snapshot that can miss later
        // writes.
        let total_count = sqlx::query_scalar::<_, i64>(
            "SELECT value FROM cache_counters WHERE name = 'statuses'",
        )
        .fetch_one(&mut *transaction)
        .await?;
        mark_complete(&mut transaction, total_count).await?;
        transaction.commit().await?;
        return Ok(SearchBackfillProgress {
            processed_count: non_negative_u64(total_count),
            total_count: non_negative_u64(total_count),
            completed: true,
        });
    }

    let (last_status_id, last_server_domain) = keys.last().cloned().ok_or_else(|| {
        sqlx::Error::Protocol("search backfill chunk unexpectedly had no last key".to_string())
    })?;

    insert_document_chunk(
        &mut transaction,
        state.cursor_status_id.as_deref(),
        state.cursor_server_domain.as_deref(),
        &last_status_id,
        &last_server_domain,
    )
    .await?;
    insert_fts_chunk(
        &mut transaction,
        state.cursor_status_id.as_deref(),
        state.cursor_server_domain.as_deref(),
        &last_status_id,
        &last_server_domain,
    )
    .await?;

    let processed_count = state
        .processed_count
        .saturating_add(i64::try_from(keys.len()).unwrap_or(i64::MAX));
    let completed = i64::try_from(keys.len()).unwrap_or(i64::MAX) < chunk_size;
    let total_count = if completed {
        sqlx::query_scalar::<_, i64>("SELECT value FROM cache_counters WHERE name = 'statuses'")
            .fetch_one(&mut *transaction)
            .await?
    } else {
        known_total_count.max(processed_count)
    };

    sqlx::query(
        "UPDATE status_search_backfill_state
            SET cursor_status_id = ?,
                cursor_server_domain = ?,
                processed_count = ?,
                total_count = ?,
                completed = ?,
                updated_at = datetime('now')
          WHERE singleton = 1",
    )
    .bind(&last_status_id)
    .bind(&last_server_domain)
    .bind(if completed {
        total_count.max(0)
    } else {
        processed_count.max(0)
    })
    .bind(total_count.max(0))
    .bind(completed)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(SearchBackfillProgress {
        processed_count: non_negative_u64(if completed {
            total_count
        } else {
            processed_count
        }),
        total_count: non_negative_u64(total_count),
        completed,
    })
}

async fn count_index_rows(reader: &SqlitePool) -> Result<(i64, i64, i64), sqlx::Error> {
    sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT
                (SELECT value FROM cache_counters WHERE name = 'statuses'),
                (SELECT COUNT(*) FROM status_search_documents),
                (SELECT COUNT(*) FROM status_search_fts_docsize)",
    )
    .fetch_one(reader)
    .await
}

#[derive(Debug)]
struct BackfillState {
    cursor_status_id: Option<String>,
    cursor_server_domain: Option<String>,
    processed_count: i64,
    total_count: i64,
    completed: bool,
}

impl BackfillState {
    fn progress(&self) -> SearchBackfillProgress {
        SearchBackfillProgress {
            processed_count: non_negative_u64(self.processed_count),
            total_count: non_negative_u64(self.total_count),
            completed: self.completed,
        }
    }
}

async fn load_state(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<BackfillState, sqlx::Error> {
    let (cursor_status_id, cursor_server_domain, processed_count, total_count, completed) =
        sqlx::query_as::<_, (Option<String>, Option<String>, i64, i64, bool)>(
            "SELECT cursor_status_id, cursor_server_domain, processed_count, total_count, completed
               FROM status_search_backfill_state
              WHERE singleton = 1",
        )
        .fetch_one(&mut **transaction)
        .await?;
    Ok(BackfillState {
        cursor_status_id,
        cursor_server_domain,
        processed_count,
        total_count,
        completed,
    })
}

async fn load_state_from_pool(pool: &SqlitePool) -> Result<BackfillState, sqlx::Error> {
    let (cursor_status_id, cursor_server_domain, processed_count, total_count, completed) =
        sqlx::query_as::<_, (Option<String>, Option<String>, i64, i64, bool)>(
            "SELECT cursor_status_id, cursor_server_domain, processed_count, total_count, completed
               FROM status_search_backfill_state
              WHERE singleton = 1",
        )
        .fetch_one(pool)
        .await?;
    Ok(BackfillState {
        cursor_status_id,
        cursor_server_domain,
        processed_count,
        total_count,
        completed,
    })
}

async fn insert_document_chunk(
    transaction: &mut Transaction<'_, Sqlite>,
    cursor_status_id: Option<&str>,
    cursor_server_domain: Option<&str>,
    last_status_id: &str,
    last_server_domain: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO status_search_documents(status_id, server_domain)
         SELECT id, server_domain
           FROM statuses
          WHERE (? IS NULL OR id > ? OR (id = ? AND server_domain > ?))
            AND (id < ? OR (id = ? AND server_domain <= ?))",
    )
    .bind(cursor_status_id)
    .bind(cursor_status_id)
    .bind(cursor_status_id)
    .bind(cursor_server_domain)
    .bind(last_status_id)
    .bind(last_status_id)
    .bind(last_server_domain)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_fts_chunk(
    transaction: &mut Transaction<'_, Sqlite>,
    cursor_status_id: Option<&str>,
    cursor_server_domain: Option<&str>,
    last_status_id: &str,
    last_server_domain: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO status_search_fts (
             rowid, content, spoiler_text, uri, url, tags,
             account_acct, account_display_name
         )
         SELECT
             d.docid,
             s.content,
             s.spoiler_text,
             s.uri,
             COALESCE(s.url, ''),
             COALESCE(s.tags_json, ''),
             COALESCE(a.acct, ''),
             COALESCE(a.display_name, '')
           FROM statuses s
           JOIN status_search_documents d
             ON d.status_id = s.id
            AND d.server_domain = s.server_domain
           LEFT JOIN accounts a
             ON a.id = s.account_id
            AND a.server_domain = s.server_domain
          WHERE (? IS NULL OR s.id > ? OR (s.id = ? AND s.server_domain > ?))
            AND (s.id < ? OR (s.id = ? AND s.server_domain <= ?))
            AND NOT EXISTS (
                SELECT 1 FROM status_search_fts existing
                 WHERE existing.rowid = d.docid
            )",
    )
    .bind(cursor_status_id)
    .bind(cursor_status_id)
    .bind(cursor_status_id)
    .bind(cursor_server_domain)
    .bind(last_status_id)
    .bind(last_status_id)
    .bind(last_server_domain)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn mark_complete(
    transaction: &mut Transaction<'_, Sqlite>,
    total_count: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE status_search_backfill_state
            SET processed_count = ?,
                total_count = ?,
                completed = 1,
                updated_at = datetime('now')
          WHERE singleton = 1",
    )
    .bind(total_count.max(0))
    .bind(total_count.max(0))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn non_negative_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    async fn fixture_pool(status_count: usize) -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::raw_sql(migration)
                .execute(&pool)
                .await
                .expect("apply fixture migration");
        }
        sqlx::query("INSERT INTO servers(domain, streaming_url) VALUES ('example.test', '')")
            .execute(&pool)
            .await
            .expect("insert server");
        sqlx::query(
            "INSERT INTO accounts(
                 id, server_domain, username, acct, display_name, created_at
             ) VALUES ('author', 'example.test', 'author', 'author', 'Author', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .expect("insert author");
        for index in 0..status_count {
            sqlx::query(
                "INSERT INTO statuses(
                     id, server_domain, uri, created_at, account_id, content
                 ) VALUES (?, 'example.test', ?, ?, 'author', ?)",
            )
            .bind(format!("status-{index:04}"))
            .bind(format!("https://example.test/statuses/{index}"))
            .bind(format!("2026-01-01T00:{index:02}:00Z"))
            .bind(format!("searchable content {index}"))
            .execute(&pool)
            .await
            .expect("insert status");
        }
        sqlx::raw_sql(
            "CREATE TABLE cache_counters (
                 name TEXT PRIMARY KEY,
                 value INTEGER NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO cache_counters(name, value)
             SELECT 'statuses', COUNT(*) FROM statuses;",
        )
        .execute(&pool)
        .await
        .expect("create status counter");

        sqlx::raw_sql(include_str!(
            "../../migrations/020_create_status_search_fts.sql"
        ))
        .execute(&pool)
        .await
        .expect("create search schema");
        // Model the legacy bootstrap path, which creates schema and triggers
        // but intentionally leaves pre-existing rows for the background job.
        sqlx::query("DELETE FROM status_search_fts")
            .execute(&pool)
            .await
            .expect("clear full fixture index");
        sqlx::query("DELETE FROM status_search_documents")
            .execute(&pool)
            .await
            .expect("clear fixture documents");
        sqlx::raw_sql(include_str!(
            "../../migrations/023_resumable_status_search_backfill.sql"
        ))
        .execute(&pool)
        .await
        .expect("create backfill state");
        pool
    }

    #[tokio::test]
    async fn resumes_in_bounded_chunks_and_indexes_every_status() {
        let pool = fixture_pool(5).await;

        let first = run_chunk(&pool, &pool, 2).await.expect("first chunk");
        assert_eq!(first.processed_count, 2);
        assert_eq!(first.total_count, 5);
        assert!(!first.completed);
        assert!(!is_complete(&pool).await.expect("read incomplete state"));

        let second = run_chunk(&pool, &pool, 2).await.expect("resumed chunk");
        assert_eq!(second.processed_count, 4);
        assert!(!second.completed);

        let final_progress = run_chunk(&pool, &pool, 2).await.expect("final chunk");
        assert_eq!(final_progress.processed_count, 5);
        assert!(final_progress.completed);
        assert!(is_complete(&pool).await.expect("read complete state"));

        let document_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_documents")
                .fetch_one(&pool)
                .await
                .expect("count documents");
        let fts_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_fts_docsize")
                .fetch_one(&pool)
                .await
                .expect("count FTS rows");
        assert_eq!(document_count, 5);
        assert_eq!(fts_count, 5);

        let matches = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM status_search_fts
              WHERE status_search_fts MATCH '\"content 4\"'",
        )
        .fetch_one(&pool)
        .await
        .expect("search indexed content");
        assert_eq!(matches, 1);
    }

    #[tokio::test]
    async fn migrations_023_and_025_complete_index_exits_without_rebuilding() {
        let pool = fixture_pool(3).await;
        sqlx::query(
            "INSERT INTO status_search_documents(status_id, server_domain)
             SELECT id, server_domain FROM statuses",
        )
        .execute(&pool)
        .await
        .expect("restore documents");
        sqlx::query(
            "INSERT INTO status_search_fts(rowid, content, spoiler_text, uri, url, tags, account_acct, account_display_name)
             SELECT d.docid, s.content, s.spoiler_text, s.uri, COALESCE(s.url, ''),
                    COALESCE(s.tags_json, ''), a.acct, a.display_name
               FROM status_search_documents d
               JOIN statuses s ON s.id = d.status_id AND s.server_domain = d.server_domain
               JOIN accounts a ON a.id = s.account_id AND a.server_domain = s.server_domain",
        )
        .execute(&pool)
        .await
        .expect("restore FTS rows");
        sqlx::raw_sql(include_str!(
            "../../migrations/025_bound_fts_merge_work.sql"
        ))
        .execute(&pool)
        .await
        .expect("apply bounded FTS merge settings");

        let changes_before = sqlx::query_scalar::<_, i64>("SELECT total_changes()")
            .fetch_one(&pool)
            .await
            .expect("read changes before completion check");
        let progress = run_to_completion(&pool, &pool, None)
            .await
            .expect("detect complete index");
        let changes_after = sqlx::query_scalar::<_, i64>("SELECT total_changes()")
            .fetch_one(&pool)
            .await
            .expect("read changes after completion check");
        assert_eq!(progress.processed_count, 3);
        assert!(progress.completed);
        // Only the portable progress row is marked complete. The document and
        // FTS tables are not walked or rebuilt when their counts already agree.
        assert_eq!(changes_after - changes_before, 1);
    }

    #[tokio::test]
    async fn initial_count_probe_waits_on_reader_without_reserving_writer() {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-search-reader-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).expect("create probe directory");
        let path = directory.join("probe.db");
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await
            .expect("open probe writer");
        sqlx::raw_sql(
            "CREATE TABLE statuses(id TEXT);
             CREATE TABLE cache_counters(name TEXT PRIMARY KEY, value INTEGER NOT NULL);
             INSERT INTO cache_counters(name, value) VALUES ('statuses', 0);
             CREATE TABLE status_search_documents(docid INTEGER);
             CREATE VIRTUAL TABLE status_search_fts USING fts5(content);
             CREATE TABLE status_search_backfill_state (
                 singleton INTEGER PRIMARY KEY,
                 cursor_status_id TEXT,
                 cursor_server_domain TEXT,
                 processed_count INTEGER NOT NULL,
                 total_count INTEGER NOT NULL,
                 completed INTEGER NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO status_search_backfill_state
                 (singleton, processed_count, total_count, completed)
             VALUES (1, 0, 0, 0);",
        )
        .execute(&writer)
        .await
        .expect("create probe schema");
        let reader = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path).read_only(true))
            .await
            .expect("open probe reader");

        let held_reader = reader.acquire().await.expect("reserve probe reader");
        let task_writer = writer.clone();
        let task_reader = reader.clone();
        let backfill = tokio::spawn(async move { run_chunk(&task_writer, &task_reader, 1).await });
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert!(
            !backfill.is_finished(),
            "probe must be waiting for its reader"
        );
        let writer_connection = tokio::time::timeout(Duration::from_millis(250), writer.acquire())
            .await
            .expect("reader-side count must not reserve the writer")
            .expect("acquire writer during reader probe");
        drop(writer_connection);

        drop(held_reader);
        let progress = backfill
            .await
            .expect("join reader probe")
            .expect("complete reader probe");
        assert!(progress.completed);
        writer.close().await;
        reader.close().await;
        std::fs::remove_dir_all(directory).expect("remove probe directory");
    }

    #[tokio::test]
    async fn unchanged_status_upsert_does_not_rebuild_fts_postings() {
        let pool = fixture_pool(1).await;
        run_to_completion(&pool, &pool, None)
            .await
            .expect("complete fixture index");
        let before = sqlx::query_scalar::<_, i64>("SELECT total_changes()")
            .fetch_one(&pool)
            .await
            .expect("read changes before update");

        sqlx::query("UPDATE statuses SET content = content WHERE id = 'status-0000'")
            .execute(&pool)
            .await
            .expect("perform unchanged update");
        let after = sqlx::query_scalar::<_, i64>("SELECT total_changes()")
            .fetch_one(&pool)
            .await
            .expect("read changes after update");

        // Only the statuses row changes; the WHEN predicate prevents all FTS
        // shadow-table writes for an identical indexed value.
        assert_eq!(after - before, 1);
    }
}
