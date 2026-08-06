//! Bounded ICU4X/FTS search repository over the portable SQLite cache.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use sqlx::{SqliteConnection, SqlitePool};
use tokio_util::sync::CancellationToken;

use crate::db::icu_search;
use crate::db::models::DbStatus;
use crate::db::queries::timeline_views::{self, StatusDisplayFilter};

#[derive(Debug, Clone)]
struct SearchCursor {
    created_at: String,
    server_domain: String,
    id: String,
}

struct IndexedTerm {
    match_query: String,
    query_token_text: String,
    search_term: String,
}

struct StatusSearchState {
    completed: bool,
}

struct AccountSearchState {
    cursor_account_id: Option<String>,
    cursor_server_domain: Option<String>,
    completed: bool,
}

const PROGRESS_INTERVAL: i32 = 1_000;
const MAX_QUERY_DURATION: Duration = Duration::from_secs(10);
const MAX_QUERY_BYTES: usize = 4_096;
const MAX_QUERY_TERMS: usize = 8;
const CANDIDATE_COUNT_CAP: i64 = 10_000;
const PENDING_STATUS_SCAN_LIMIT: i64 = 256;
const PENDING_ACCOUNT_SCAN_LIMIT: i64 = 256;
const STATUS_GAP_SCAN_LIMIT: i64 = 256;
const ACCOUNT_GAP_SCAN_LIMIT: i64 = 256;
const RECENT_INDEX_SCAN_LIMIT: i64 = 10_000;

pub struct SearchQuery<'a> {
    pub query: &'a str,
    pub limit: i64,
    pub offset: i64,
    pub display_filter: StatusDisplayFilter,
    pub start_after: Option<(&'a str, &'a str)>,
}

#[cfg(test)]
async fn query_statuses(
    pool: &SqlitePool,
    request: SearchQuery<'_>,
) -> Result<Vec<DbStatus>, sqlx::Error> {
    query_statuses_with_cancellation(pool, request, &CancellationToken::new()).await
}

pub async fn query_statuses_with_cancellation(
    pool: &SqlitePool,
    request: SearchQuery<'_>,
    cancellation: &CancellationToken,
) -> Result<Vec<DbStatus>, sqlx::Error> {
    if cancellation.is_cancelled() {
        return Err(sqlx::Error::Protocol("search query cancelled".to_string()));
    }
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(sqlx::Error::Protocol(
            "search query exceeds input budget".to_string(),
        ));
    }
    if request.query.split_whitespace().next().is_none() {
        return Ok(Vec::new());
    }

    // One progress handler covers cursor resolution, selectivity probes, and
    // the final read. Candidate planning must not sit outside the same user-
    // visible execution budget as the result query.
    let started_at = Instant::now();
    let mut connection = tokio::select! {
        _ = cancellation.cancelled() => {
            return Err(sqlx::Error::Protocol("search query cancelled".to_string()));
        }
        result = tokio::time::timeout(MAX_QUERY_DURATION, pool.acquire()) => {
            result
                .map_err(|_| sqlx::Error::Protocol(
                    "search query exceeded execution budget".to_string()
                ))??
        }
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let callback_interrupted = Arc::clone(&interrupted);
    let callback_cancellation = cancellation.clone();
    let deadline = started_at + MAX_QUERY_DURATION;
    {
        let mut handle = connection.lock_handle().await?;
        handle.set_progress_handler(PROGRESS_INTERVAL, move || {
            let keep_running = !callback_cancellation.is_cancelled() && Instant::now() < deadline;
            if !keep_running {
                callback_interrupted.store(true, Ordering::Relaxed);
            }
            keep_running
        });
    }

    let result = query_statuses_on_connection(&mut connection, request, cancellation).await;
    {
        let mut handle = connection.lock_handle().await?;
        handle.remove_progress_handler();
    }
    match result {
        Ok(statuses) => Ok(statuses),
        Err(_error) if interrupted.load(Ordering::Relaxed) => {
            if cancellation.is_cancelled() {
                Err(sqlx::Error::Protocol("search query cancelled".to_string()))
            } else {
                Err(sqlx::Error::Protocol(
                    "search query exceeded execution budget".to_string(),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

async fn query_statuses_on_connection(
    connection: &mut SqliteConnection,
    request: SearchQuery<'_>,
    cancellation: &CancellationToken,
) -> Result<Vec<DbStatus>, sqlx::Error> {
    let SearchQuery {
        query,
        limit,
        offset,
        display_filter,
        start_after,
    } = request;
    let terms = normalize_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    if terms.len() > MAX_QUERY_TERMS {
        return Err(sqlx::Error::Protocol(
            "search query has too many terms".to_string(),
        ));
    }

    let (status_completed, cursor_account_id, account_cursor_server_domain, account_completed) =
        sqlx::query_as::<_, (bool, Option<String>, Option<String>, bool)>(
            "SELECT status.completed,
                account.cursor_account_id,
                account.cursor_server_domain,
                account.completed
           FROM status_search_icu_backfill_state status
           JOIN account_search_icu_backfill_state account
             ON account.singleton = status.singleton
          WHERE status.singleton = 1",
        )
        .fetch_one(&mut *connection)
        .await?;
    let status_state = StatusSearchState {
        completed: status_completed,
    };
    let account_state = AccountSearchState {
        cursor_account_id,
        cursor_server_domain: account_cursor_server_domain,
        completed: account_completed,
    };
    let cursor = resolve_cursor(connection, start_after).await?;
    let filter_sql = timeline_views::display_filter_sql("s", display_filter);
    let cursor_sql = if cursor.is_some() {
        " AND (s.created_at < ? OR (s.created_at = ? AND (s.server_domain < ? OR (s.server_domain = ? AND s.id < ?))))"
    } else {
        ""
    };
    let mut candidate_terms = Vec::with_capacity(terms.len());
    let mut seen_match_queries = std::collections::HashSet::new();
    for term in terms {
        let Some(match_query) = icu_search::match_expression(&term) else {
            return Ok(Vec::new());
        };
        if !seen_match_queries.insert(match_query.clone()) {
            continue;
        }
        candidate_terms.push(IndexedTerm {
            match_query,
            query_token_text: icu_search::index_text([term.as_str()]),
            search_term: term,
        });
    }
    let mut indexed_terms_with_counts = Vec::with_capacity(candidate_terms.len());
    for candidate in candidate_terms {
        if cancellation.is_cancelled() {
            return Err(sqlx::Error::Protocol("search query cancelled".to_string()));
        }
        let estimated_rows = capped_candidate_count(connection, &candidate).await?;
        // Keep ICU word-prefix semantics authoritative even for a frequent
        // term. The capped count only orders candidate joins; reverting a
        // frequent word to `%substring%` would both lose normalization
        // correctness and reintroduce a full status scan.
        indexed_terms_with_counts.push((estimated_rows, candidate));
    }
    indexed_terms_with_counts.sort_by_key(|(estimated_rows, _)| *estimated_rows);
    let indexed_terms = indexed_terms_with_counts
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect::<Vec<_>>();

    let mut cte_parts = vec![
        format!(
            "pending_status_rows AS MATERIALIZED (
                 SELECT pending_status.id,
                        pending_status.server_domain,
                        pending_status.content,
                        pending_status.spoiler_text
                   FROM status_search_index_queue pending_queue
                   JOIN statuses pending_status
                     ON pending_status.id = pending_queue.status_id
                    AND pending_status.server_domain = pending_queue.server_domain
                  WHERE pending_queue.action = 'upsert'
                  ORDER BY pending_queue.queued_at DESC,
                           pending_queue.status_id DESC,
                           pending_queue.server_domain DESC
                  LIMIT {PENDING_STATUS_SCAN_LIMIT}
             )"
        ),
        format!(
            "pending_account_rows AS MATERIALIZED (
                 SELECT pending_account.id,
                        pending_account.server_domain,
                        pending_account.acct,
                        pending_account.display_name
                   FROM account_search_index_queue pending_queue
                   JOIN accounts pending_account
                     ON pending_account.id = pending_queue.account_id
                    AND pending_account.server_domain = pending_queue.server_domain
                  WHERE pending_queue.action = 'upsert'
                  ORDER BY pending_queue.queued_at DESC,
                           pending_queue.account_id DESC,
                           pending_queue.server_domain DESC
                  LIMIT {PENDING_ACCOUNT_SCAN_LIMIT}
             )"
        ),
        format!(
            "recent_index_rows AS MATERIALIZED (
                 SELECT recent.id,
                        recent.server_domain,
                        recent.account_id,
                        coalesce(status_index.token_text, '') AS status_token_text,
                        coalesce(account_index.token_text, '') AS account_token_text
                   FROM statuses recent INDEXED BY idx_statuses_global_cursor
                   LEFT JOIN status_search_icu_content status_index
                     ON status_index.status_id = recent.id
                    AND status_index.server_domain = recent.server_domain
                    AND status_index.text_scope_version = {status_text_scope_version}
                   LEFT JOIN account_search_icu_content account_index
                     ON account_index.account_id = recent.account_id
                    AND account_index.server_domain = recent.server_domain
                  ORDER BY recent.created_at DESC,
                           recent.server_domain DESC,
                           recent.id DESC
                  LIMIT {RECENT_INDEX_SCAN_LIMIT}
             )",
            status_text_scope_version = icu_search::STATUS_TEXT_SCOPE_VERSION,
        ),
    ];
    if !status_state.completed {
        cte_parts.push(format!(
            "recent_status_window AS MATERIALIZED (
                 SELECT recent.id,
                        recent.server_domain,
                        recent.content,
                        recent.spoiler_text
                   FROM statuses recent INDEXED BY idx_statuses_global_cursor
                  ORDER BY recent.created_at DESC,
                           recent.server_domain DESC,
                           recent.id DESC
                  LIMIT {STATUS_GAP_SCAN_LIMIT}
             )"
        ));
        cte_parts.push(format!(
            "unindexed_status_rows AS MATERIALIZED (
                 SELECT recent.*
                   FROM recent_status_window recent
                  WHERE NOT EXISTS (
                            SELECT 1
                              FROM status_search_icu_content indexed_status
                             WHERE indexed_status.status_id = recent.id
                               AND indexed_status.server_domain = recent.server_domain
                               AND indexed_status.text_scope_version = {status_text_scope_version}
                        )
                    AND (recent.id, recent.server_domain) NOT IN (
                            SELECT pending.id, pending.server_domain
                              FROM pending_status_rows pending
                        )
             )",
            status_text_scope_version = icu_search::STATUS_TEXT_SCOPE_VERSION,
        ));
    }
    if !account_state.completed {
        let account_upper_bound = account_state
            .cursor_account_id
            .as_ref()
            .zip(account_state.cursor_server_domain.as_ref())
            .map(|_| "WHERE (candidate.id, candidate.server_domain) < (?, ?)")
            .unwrap_or_default();
        cte_parts.push(format!(
            "account_gap_window AS MATERIALIZED (
                 SELECT candidate.id,
                        candidate.server_domain,
                        candidate.acct,
                        candidate.display_name
                   FROM accounts candidate
                  {account_upper_bound}
                  ORDER BY candidate.id DESC, candidate.server_domain DESC
                  LIMIT {ACCOUNT_GAP_SCAN_LIMIT}
             )"
        ));
        cte_parts.push(
            "unindexed_account_rows AS MATERIALIZED (
                 SELECT candidate.*
                   FROM account_gap_window candidate
                  WHERE NOT EXISTS (
                            SELECT 1
                              FROM account_search_icu_content indexed_account
                             WHERE indexed_account.account_id = candidate.id
                               AND indexed_account.server_domain = candidate.server_domain
                        )
                    AND (candidate.id, candidate.server_domain) NOT IN (
                            SELECT pending.id, pending.server_domain
                              FROM pending_account_rows pending
                        )
             )"
            .to_string(),
        );
    }
    for (index, _candidate) in indexed_terms.iter().enumerate() {
        cte_parts.push(format!(
            "indexed_status_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                 SELECT indexed_content.status_id,
                        indexed_content.server_domain
                   FROM status_search_icu_fts
                   JOIN status_search_icu_content indexed_content
                     ON indexed_content.docid = status_search_icu_fts.rowid
                  WHERE status_search_icu_fts MATCH ?
                    AND (indexed_content.status_id, indexed_content.server_domain) NOT IN (
                            SELECT stale_index_{index}.id,
                                   stale_index_{index}.server_domain
                              FROM pending_status_rows stale_index_{index}
                        )
                  LIMIT {CANDIDATE_COUNT_CAP}
             )"
        ));
        cte_parts.push(format!(
            "indexed_account_keys_{index}(account_id, server_domain) AS MATERIALIZED (
                 SELECT indexed_account_{index}.account_id,
                        indexed_account_{index}.server_domain
                   FROM account_search_icu_fts
                   JOIN account_search_icu_content indexed_account_{index}
                     ON indexed_account_{index}.docid = account_search_icu_fts.rowid
                  WHERE account_search_icu_fts MATCH ?
                    AND (indexed_account_{index}.account_id,
                         indexed_account_{index}.server_domain) NOT IN (
                            SELECT stale_account_{index}.id,
                                   stale_account_{index}.server_domain
                              FROM pending_account_rows stale_account_{index}
                        )
                  LIMIT {CANDIDATE_COUNT_CAP}
             )"
        ));
        cte_parts.push(format!(
            "indexed_account_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                 SELECT indexed_account_status_{index}.id,
                        indexed_account_status_{index}.server_domain
                   FROM indexed_account_keys_{index} indexed_account_{index}
                   JOIN statuses indexed_account_status_{index}
                     INDEXED BY idx_statuses_account
                     ON indexed_account_status_{index}.account_id = indexed_account_{index}.account_id
                    AND indexed_account_status_{index}.server_domain = indexed_account_{index}.server_domain
                  LIMIT {CANDIDATE_COUNT_CAP}
             )"
        ));
        cte_parts.push(format!(
            "pending_status_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                 SELECT pending_status_{index}.id,
                        pending_status_{index}.server_domain
                   FROM pending_status_rows pending_status_{index}
                  WHERE {}
             )",
            status_icu_term_sql(&format!("pending_status_{index}")),
        ));
        cte_parts.push(format!(
            "pending_account_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                 SELECT pending_account_status_{index}.id,
                        pending_account_status_{index}.server_domain
                   FROM pending_account_rows pending_account_{index}
                   JOIN statuses pending_account_status_{index}
                     INDEXED BY idx_statuses_account
                    ON pending_account_status_{index}.account_id = pending_account_{index}.id
                    AND pending_account_status_{index}.server_domain = pending_account_{index}.server_domain
                  WHERE {}
                  LIMIT {CANDIDATE_COUNT_CAP}
             )",
            account_icu_term_sql(&format!("pending_account_{index}")),
        ));
        cte_parts.push(format!(
            "recent_index_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                 SELECT recent_index_{index}.id,
                        recent_index_{index}.server_domain
                   FROM recent_index_rows recent_index_{index}
                  WHERE awayuki_icu_index_match(
                            ?,
                            recent_index_{index}.status_token_text,
                            recent_index_{index}.account_token_text
                        ) = 1
                    AND (recent_index_{index}.id,
                         recent_index_{index}.server_domain) NOT IN (
                            SELECT pending_status_{index}.id,
                                   pending_status_{index}.server_domain
                              FROM pending_status_rows pending_status_{index}
                        )
                    AND (recent_index_{index}.account_id,
                         recent_index_{index}.server_domain) NOT IN (
                            SELECT pending_account_{index}.id,
                                   pending_account_{index}.server_domain
                              FROM pending_account_rows pending_account_{index}
                        )
             )"
        ));
        let mut candidate_sources = vec![
            format!("SELECT status_id, server_domain FROM indexed_status_candidate_{index}"),
            format!("SELECT status_id, server_domain FROM indexed_account_candidate_{index}"),
            format!("SELECT status_id, server_domain FROM pending_status_candidate_{index}"),
            format!("SELECT status_id, server_domain FROM pending_account_candidate_{index}"),
            format!("SELECT status_id, server_domain FROM recent_index_candidate_{index}"),
        ];
        if !status_state.completed {
            cte_parts.push(format!(
                "unindexed_status_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                     SELECT unindexed_status_{index}.id,
                            unindexed_status_{index}.server_domain
                       FROM unindexed_status_rows unindexed_status_{index}
                      WHERE {}
                 )",
                status_icu_term_sql(&format!("unindexed_status_{index}")),
            ));
            candidate_sources.push(format!(
                "SELECT status_id, server_domain FROM unindexed_status_candidate_{index}"
            ));
        }
        if !account_state.completed {
            cte_parts.push(format!(
                "unindexed_account_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                     SELECT unindexed_account_status_{index}.id,
                            unindexed_account_status_{index}.server_domain
                       FROM unindexed_account_rows unindexed_account_{index}
                       JOIN statuses unindexed_account_status_{index}
                         INDEXED BY idx_statuses_account
                         ON unindexed_account_status_{index}.account_id = unindexed_account_{index}.id
                        AND unindexed_account_status_{index}.server_domain = unindexed_account_{index}.server_domain
                      WHERE {}
                      LIMIT {CANDIDATE_COUNT_CAP}
                 )",
                account_icu_term_sql(&format!("unindexed_account_{index}")),
            ));
            candidate_sources.push(format!(
                "SELECT status_id, server_domain FROM unindexed_account_candidate_{index}"
            ));
        }
        cte_parts.push(format!(
            "search_candidate_{index}(status_id, server_domain) AS MATERIALIZED (
                 SELECT combined.status_id, combined.server_domain
                   FROM ({}) combined
                   JOIN statuses candidate_status_{index}
                     ON candidate_status_{index}.id = combined.status_id
                    AND candidate_status_{index}.server_domain = combined.server_domain
                  ORDER BY candidate_status_{index}.created_at DESC,
                           candidate_status_{index}.server_domain DESC,
                           candidate_status_{index}.id DESC
                  LIMIT {CANDIDATE_COUNT_CAP}
             )",
            candidate_sources.join(" UNION "),
        ));
    }
    let common_table_expression = format!("WITH {}", cte_parts.join(", "));
    let mut indexed_join = String::from("FROM statuses s");
    for index in 0..indexed_terms.len() {
        indexed_join.push_str(&format!(
            " JOIN search_candidate_{index}
                ON search_candidate_{index}.status_id = s.id
               AND search_candidate_{index}.server_domain = s.server_domain"
        ));
    }
    let sql = format!(
        "{common_table_expression}
         SELECT s.*
         {indexed_join}
         WHERE 1 = 1
         {filter_sql}
         {cursor_sql}
         ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
         LIMIT ? OFFSET ?"
    );
    // SQL structure and column names are generated internally; user search
    // terms and cursors are added only through bind parameters below.
    let mut db_query = sqlx::query_as::<_, DbStatus>(sqlx::AssertSqlSafe(sql));
    if !account_state.completed {
        if let Some((account_id, server_domain)) = account_state
            .cursor_account_id
            .as_ref()
            .zip(account_state.cursor_server_domain.as_ref())
        {
            db_query = db_query.bind(account_id).bind(server_domain);
        }
    }
    for candidate in &indexed_terms {
        db_query = db_query
            .bind(&candidate.match_query)
            .bind(&candidate.match_query)
            .bind(&candidate.search_term)
            .bind(&candidate.search_term)
            .bind(&candidate.query_token_text);
        if !status_state.completed {
            db_query = db_query.bind(&candidate.search_term);
        }
        if !account_state.completed {
            db_query = db_query.bind(&candidate.search_term);
        }
    }
    if let Some(cursor) = cursor.as_ref() {
        db_query = db_query
            .bind(&cursor.created_at)
            .bind(&cursor.created_at)
            .bind(&cursor.server_domain)
            .bind(&cursor.server_domain)
            .bind(&cursor.id);
    }
    db_query = db_query
        .bind(limit.max(0))
        .bind(if cursor.is_some() { 0 } else { offset.max(0) });

    db_query.fetch_all(connection).await
}

async fn resolve_cursor(
    connection: &mut SqliteConnection,
    status: Option<(&str, &str)>,
) -> Result<Option<SearchCursor>, sqlx::Error> {
    let Some((status_id, server_domain)) = status else {
        return Ok(None);
    };
    sqlx::query_as::<_, (String, String, String)>(
        "SELECT created_at, server_domain, id
         FROM statuses
         WHERE id = ? AND server_domain = ?",
    )
    .bind(status_id)
    .bind(server_domain)
    .fetch_optional(connection)
    .await
    .map(|cursor| {
        cursor.map(|(created_at, server_domain, id)| SearchCursor {
            created_at,
            server_domain,
            id,
        })
    })
}

async fn capped_candidate_count(
    connection: &mut SqliteConnection,
    candidate: &IndexedTerm,
) -> Result<i64, sqlx::Error> {
    let fts_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
           FROM (
             SELECT rowid FROM status_search_icu_fts
              WHERE status_search_icu_fts MATCH ?
              LIMIT ?
           )",
    )
    .bind(&candidate.match_query)
    .bind(CANDIDATE_COUNT_CAP)
    .fetch_one(&mut *connection)
    .await?;
    if fts_count >= CANDIDATE_COUNT_CAP {
        return Ok(CANDIDATE_COUNT_CAP);
    }

    let remaining = CANDIDATE_COUNT_CAP - fts_count;
    let account_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*)
           FROM (
             SELECT rowid
               FROM account_search_icu_fts
              WHERE account_search_icu_fts MATCH ?
              LIMIT ?
           )",
    )
    .bind(&candidate.match_query)
    .bind(remaining)
    .fetch_one(&mut *connection)
    .await?;
    Ok(fts_count.saturating_add(account_count))
}

fn normalize_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::to_string)
        .filter(|term| !term.is_empty())
        .collect()
}

fn status_icu_term_sql(alias: &str) -> String {
    format!(
        "awayuki_icu_match(
             ?, {alias}.content, {alias}.spoiler_text
         ) = 1"
    )
}

fn account_icu_term_sql(alias: &str) -> String {
    format!(
        "awayuki_icu_match(
             ?, {alias}.acct, {alias}.display_name
         ) = 1"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    async fn icu_search_schema(apply_status_text_scope: bool) -> SqlitePool {
        let options = "sqlite::memory:"
            .parse::<SqliteConnectOptions>()
            .expect("parse short-search fixture options")
            .shared_cache(true);
        let pool = SqlitePoolOptions::new()
            // `run_index_step` intentionally probes the low-priority writer
            // with `try_acquire()` after reading its work queue. Production
            // uses distinct reader and writer pools; keep a second shared
            // in-memory connection available so this fixture has the same
            // acquisition contract instead of reporting `WriterBusy` while
            // SQLx asynchronously returns the reader connection to the pool.
            .min_connections(2)
            .max_connections(2)
            .after_connect(|connection, _metadata| {
                Box::pin(crate::db::short_search_tokenizer::register(connection))
            })
            .connect_with(options)
            .await
            .expect("open short-search fixture");
        sqlx::raw_sql(
            "CREATE TABLE cache_counters (
                 name TEXT PRIMARY KEY,
                 value INTEGER NOT NULL CHECK (value >= 0),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO cache_counters(name, value)
             VALUES ('statuses', 0), ('accounts', 0);",
        )
        .execute(&pool)
        .await
        .expect("create cache counters required by migration 033");
        let mut migrations = vec![
            include_str!("../../../migrations/001_create_servers.sql"),
            include_str!("../../../migrations/002_create_accounts.sql"),
            include_str!("../../../migrations/003_create_statuses.sql"),
            include_str!("../../../migrations/012_add_status_quote_id.sql"),
            include_str!("../../../migrations/020_create_status_search_fts.sql"),
            include_str!("../../../migrations/023_resumable_status_search_backfill.sql"),
            include_str!("../../../migrations/030_index_global_status_cursor.sql"),
            include_str!("../../../migrations/031_create_short_search_fts.sql"),
            include_str!("../../../migrations/032_async_icu_status_search.sql"),
            include_str!("../../../migrations/033_control_async_search_index.sql"),
            include_str!("../../../migrations/034_async_icu_account_search.sql"),
            include_str!("../../../migrations/035_reindex_icu_nonword_segments.sql"),
        ];
        if apply_status_text_scope {
            migrations.push(include_str!(
                "../../../migrations/037_limit_status_icu_search_to_post_text.sql"
            ));
        }
        for migration in migrations {
            sqlx::raw_sql(migration)
                .execute(&pool)
                .await
                .expect("apply short-search fixture migration");
        }
        pool
    }

    async fn icu_search_fixture() -> SqlitePool {
        let pool = icu_search_schema(true).await;
        sqlx::query("INSERT INTO servers(domain, streaming_url) VALUES ('example.test', '')")
            .execute(&pool)
            .await
            .expect("insert fixture server");
        sqlx::query(
            "INSERT INTO accounts(
                 id, server_domain, username, acct, display_name, created_at
             ) VALUES ('author', 'example.test', 'author', 'writer', 'XY Author', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .expect("insert fixture account");
        for (id, second, content) in [
            (
                "one",
                1,
                "needle abacus 東京都でこんにちは世界 Awayuki Straße café can't example.com 👩‍💻",
            ),
            ("two", 2, "needle a-b and 100%"),
            ("three", 3, "unrelated"),
        ] {
            sqlx::query(
                "INSERT INTO statuses(
                     id, server_domain, uri, created_at, account_id, content
                 ) VALUES (?, 'example.test', ?, ?, 'author', ?)",
            )
            .bind(id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(format!("2026-01-01T00:00:0{second}Z"))
            .bind(content)
            .execute(&pool)
            .await
            .expect("insert fixture status");
        }
        drain_icu_search_queue(&pool).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count indexed fixture statuses"),
            3
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM account_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count indexed fixture accounts"),
            1
        );
        sqlx::query(
            "UPDATE status_search_icu_backfill_state
                SET processed_count = 3, total_count = 3, completed = 1
              WHERE singleton = 1",
        )
        .execute(&pool)
        .await
        .expect("mark fixture ICU index complete");
        sqlx::query(
            "UPDATE account_search_icu_backfill_state
                SET processed_count = 1, total_count = 1, completed = 1
              WHERE singleton = 1",
        )
        .execute(&pool)
        .await
        .expect("mark fixture account ICU index complete");
        pool
    }

    async fn drain_icu_search_queue(pool: &SqlitePool) {
        let mut pending = crate::services::search_indexer::pending_count(pool)
            .await
            .expect("count queued ICU search rows");
        for _ in 0..64 {
            if pending == 0 {
                break;
            }
            let step = crate::services::search_indexer::run_index_step(pool, pool)
                .await
                .expect("index queued fixture rows");
            if matches!(step, crate::services::search_indexer::IndexStep::WriterBusy) {
                tokio::task::yield_now().await;
            }
            pending = crate::services::search_indexer::pending_count(pool)
                .await
                .expect("recount queued ICU search rows");
        }
        assert_eq!(pending, 0, "fixture ICU search queue did not drain");
    }

    async fn icu_search(pool: &SqlitePool, query: &str) -> Vec<String> {
        icu_search_with_completion(pool, query, true).await
    }

    async fn icu_search_with_completion(
        pool: &SqlitePool,
        query: &str,
        icu_fts_complete: bool,
    ) -> Vec<String> {
        sqlx::query(
            "UPDATE status_search_icu_backfill_state
                SET completed = ?
              WHERE singleton = 1",
        )
        .bind(icu_fts_complete)
        .execute(pool)
        .await
        .expect("set fixture ICU completion state");
        query_statuses(
            pool,
            SearchQuery {
                query,
                limit: 40,
                offset: 0,
                display_filter: StatusDisplayFilter::default(),
                start_after: None,
            },
        )
        .await
        .expect("query short-search fixture")
        .into_iter()
        .map(|status| status.id)
        .collect()
    }

    async fn status_fts_count(pool: &SqlitePool, term: &str) -> i64 {
        let expression = icu_search::match_expression(term).expect("FTS expression");
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)
               FROM status_search_icu_fts
              WHERE status_search_icu_fts MATCH ?",
        )
        .bind(expression)
        .fetch_one(pool)
        .await
        .expect("count status FTS matches")
    }

    #[tokio::test]
    async fn status_icu_fts_indexes_only_content_and_spoiler_text() {
        let pool = icu_search_fixture().await;
        sqlx::query(
            "INSERT INTO statuses(
                 id, server_domain, uri, url, created_at, account_id,
                 content, spoiler_text, tags_json
             ) VALUES (
                 'post-text-only', 'example.test',
                 'https://urionlyneedle.example/statuses/1',
                 'https://urlonlyneedle.example/posts/1',
                 '2026-01-01T00:00:10Z', 'author',
                 'bodyonlyneedle', 'warningonlyneedle',
                 '[{\"name\":\"tagonlyneedle\"}]'
             )",
        )
        .execute(&pool)
        .await
        .expect("insert field-scope fixture status");

        assert_eq!(
            icu_search(&pool, "bodyonlyneedle").await,
            vec!["post-text-only"],
            "pending rows search post content"
        );
        assert_eq!(
            icu_search(&pool, "warningonlyneedle").await,
            vec!["post-text-only"],
            "pending rows search content warnings"
        );
        for metadata_term in ["urionlyneedle", "urlonlyneedle", "tagonlyneedle"] {
            assert!(
                icu_search(&pool, metadata_term).await.is_empty(),
                "pending rows must not match {metadata_term}"
            );
        }

        drain_icu_search_queue(&pool).await;
        assert_eq!(status_fts_count(&pool, "bodyonlyneedle").await, 1);
        assert_eq!(status_fts_count(&pool, "warningonlyneedle").await, 1);
        for metadata_term in ["urionlyneedle", "urlonlyneedle", "tagonlyneedle"] {
            assert_eq!(
                status_fts_count(&pool, metadata_term).await,
                0,
                "persisted FTS must not match {metadata_term}"
            );
        }

        sqlx::query(
            "UPDATE statuses
                SET uri = 'https://changedurionly.example/statuses/1',
                    url = 'https://changedurlonly.example/posts/1',
                    tags_json = '[{\"name\":\"changedtagonly\"}]'
              WHERE id = 'post-text-only' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .expect("update non-FTS status metadata");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_index_queue")
                .fetch_one(&pool)
                .await
                .expect("count status queue after metadata update"),
            0,
            "URI, URL and tag-only changes must not enqueue a status reindex"
        );

        sqlx::query(
            "UPDATE statuses
                SET spoiler_text = 'changedwarningonly'
              WHERE id = 'post-text-only' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .expect("update indexed spoiler text");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_index_queue")
                .fetch_one(&pool)
                .await
                .expect("count status queue after spoiler update"),
            1
        );
        drain_icu_search_queue(&pool).await;
        assert_eq!(status_fts_count(&pool, "warningonlyneedle").await, 0);
        assert_eq!(status_fts_count(&pool, "changedwarningonly").await, 1);
    }

    #[tokio::test]
    async fn status_text_scope_migration_removes_legacy_metadata_tokens() {
        let pool = icu_search_schema(false).await;
        sqlx::raw_sql(
            "INSERT INTO servers(domain, streaming_url) VALUES ('example.test', '');
             INSERT INTO accounts(
                 id, server_domain, username, acct, display_name, created_at
             ) VALUES (
                 'author', 'example.test', 'author', 'writer', 'Writer', '2026-01-01'
             );
             INSERT INTO statuses(
                 id, server_domain, uri, created_at, account_id, content, spoiler_text
             ) VALUES (
                 'legacy-status', 'example.test',
                 'https://legacyurionlyneedle.example/statuses/1',
                 '2026-01-01T00:00:01Z', 'author', 'needle', 'warning'
             );
             DELETE FROM status_search_index_queue;
             DELETE FROM account_search_index_queue;
             UPDATE status_search_icu_backfill_state
                SET processed_count = 1, total_count = 1, completed = 1
              WHERE singleton = 1;
             UPDATE account_search_icu_backfill_state
                SET processed_count = 1, total_count = 1, completed = 1
              WHERE singleton = 1;",
        )
        .execute(&pool)
        .await
        .expect("seed legacy status and account rows");
        let legacy_token_text = icu_search::index_text([
            "needle",
            "warning",
            "https://legacyurionlyneedle.example/statuses/1",
        ]);
        sqlx::query(
            "INSERT INTO status_search_icu_content(status_id, server_domain, token_text)
             VALUES ('legacy-status', 'example.test', ?)",
        )
        .bind(legacy_token_text)
        .execute(&pool)
        .await
        .expect("seed a legacy status token stream");
        sqlx::query(
            "INSERT INTO account_search_icu_content(account_id, server_domain, token_text)
             VALUES ('author', 'example.test', ?)",
        )
        .bind(icu_search::index_text(["writer", "Writer"]))
        .execute(&pool)
        .await
        .expect("seed the separate account token stream");
        assert_eq!(status_fts_count(&pool, "legacyurionlyneedle").await, 1);
        let account_rows_before =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM account_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count account index rows before migration");

        sqlx::raw_sql(include_str!(
            "../../../migrations/037_limit_status_icu_search_to_post_text.sql"
        ))
        .execute(&pool)
        .await
        .expect("reapply status text scope migration to legacy fixture");

        assert_eq!(status_fts_count(&pool, "legacyurionlyneedle").await, 0);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count retained legacy status content rows"),
            1,
            "the migration must avoid a synchronous bulk DELETE"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*)
                   FROM status_search_icu_content
                  WHERE text_scope_version = 1"
            )
            .fetch_one(&pool)
            .await
            .expect("count legacy status scope rows"),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM account_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count preserved account index rows"),
            account_rows_before,
            "account search is a separate index and must remain intact"
        );
        assert_eq!(
            sqlx::query_as::<_, (i64, i64, bool)>(
                "SELECT processed_count, total_count, completed
                   FROM status_search_icu_backfill_state
                  WHERE singleton = 1",
            )
            .fetch_one(&pool)
            .await
            .expect("read reset status backfill state"),
            (0, 1, false)
        );
        assert!(
            icu_search_with_completion(&pool, "legacyurionlyneedle", false)
                .await
                .is_empty(),
            "migration-gap fallback must not inspect a legacy metadata token stream"
        );

        for _ in 0..8 {
            if crate::services::search_indexer::is_complete(&pool)
                .await
                .expect("read text-scope rebuild progress")
            {
                break;
            }
            crate::services::search_indexer::run_index_step(&pool, &pool)
                .await
                .expect("advance content-only status index rebuild");
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*)
                   FROM status_search_icu_content
                  WHERE text_scope_version = ?"
            )
            .bind(icu_search::STATUS_TEXT_SCOPE_VERSION)
            .fetch_one(&pool)
            .await
            .expect("count current status scope rows"),
            1
        );
        assert_eq!(status_fts_count(&pool, "legacyurionlyneedle").await, 0);
        assert_eq!(status_fts_count(&pool, "needle").await, 1);
    }

    #[tokio::test]
    async fn icu_fts_uses_word_prefixes_accounts_and_pending_queue_without_ngrams() {
        let pool = icu_search_fixture().await;

        assert_eq!(icu_search(&pool, "ab").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "東京").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "can't").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "example.com").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "👩‍💻").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "STRASSE").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "Ａｗａｙｕｋｉ").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "cafe\u{301}").await, vec!["one"]);
        assert!(
            icu_search(&pool, "yuki").await.is_empty(),
            "arbitrary word-internal substrings are not restored as n-grams"
        );
        assert_eq!(
            icu_search(&pool, "%").await,
            vec!["two"],
            "punctuation-only terms use ICU segmented index matching"
        );
        assert_eq!(
            icu_search(&pool, "xy").await,
            vec!["three", "two", "one"],
            "account-only matches use the bounded account candidate branch"
        );
        assert_eq!(icu_search(&pool, "needle ab").await, vec!["one"]);

        sqlx::query(
            "UPDATE accounts
                SET display_name = 'Renamed Straße Ａｕｔｈｏｒ'
              WHERE id = 'author'",
        )
        .execute(&pool)
        .await
        .expect("rename fixture account without rebuilding every status FTS row");
        assert_eq!(
            icu_search(&pool, "renamed").await,
            vec!["three", "two", "one"],
            "account profile edits never rebuild every authored status"
        );
        assert_eq!(
            icu_search(&pool, "STRASSE").await,
            vec!["three", "two", "one"],
            "account-only matches use ICU case folding"
        );
        sqlx::query("UPDATE accounts SET display_name = 'Renamed Author' WHERE id = 'author'")
            .execute(&pool)
            .await
            .expect("remove account-only ICU match before pending-status assertions");
        drain_icu_search_queue(&pool).await;

        sqlx::query(
            "UPDATE statuses
                SET content = 'needle zz Straße Ａｗａｙｕｋｉ'
              WHERE id = 'one'",
        )
        .execute(&pool)
        .await
        .expect("edit indexed fixture status");
        assert!(icu_search(&pool, "ab").await.is_empty());
        assert_eq!(
            icu_search(&pool, "zz").await,
            vec!["one"],
            "pending updates remain searchable before the indexer runs"
        );
        assert_eq!(icu_search(&pool, "STRASSE").await, vec!["one"]);
        assert_eq!(icu_search(&pool, "awayuki").await, vec!["one"]);
        assert!(icu_search(&pool, "yuki").await.is_empty());
        drain_icu_search_queue(&pool).await;
        assert_eq!(
            crate::services::search_indexer::pending_count(&pool)
                .await
                .expect("count pending fixture rows"),
            0
        );

        sqlx::query("DELETE FROM statuses WHERE id = 'one'")
            .execute(&pool)
            .await
            .expect("delete indexed fixture status");
        assert!(icu_search(&pool, "zz").await.is_empty());
        drain_icu_search_queue(&pool).await;
        let indexed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM status_search_icu_content WHERE status_id = 'one'",
        )
        .fetch_one(&pool)
        .await
        .expect("count deleted ICU documents");
        assert_eq!(indexed, 0);
    }

    #[tokio::test]
    async fn unified_icu_search_spans_servers_before_and_after_async_indexing() {
        let pool = icu_search_fixture().await;
        sqlx::query("UPDATE statuses SET content = content || ' crossnetwork' WHERE id = 'one'")
            .execute(&pool)
            .await
            .expect("queue first-server status update");
        sqlx::query("INSERT INTO servers(domain, streaming_url) VALUES ('remote.test', '')")
            .execute(&pool)
            .await
            .expect("insert second fixture server");
        sqlx::query(
            "INSERT INTO accounts(
                 id, server_domain, username, acct, display_name, created_at
             ) VALUES ('remote-author', 'remote.test', 'remote', 'remote', 'Remote', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .expect("insert second-server author");
        sqlx::query(
            "INSERT INTO statuses(
                 id, server_domain, uri, created_at, account_id, content
             ) VALUES (
                 'remote-status', 'remote.test', 'https://remote.test/statuses/1',
                 '2026-01-01T00:00:04Z', 'remote-author', 'crossnetwork'
             )",
        )
        .execute(&pool)
        .await
        .expect("insert second-server status");

        assert_eq!(
            icu_search(&pool, "crossnetwork").await,
            vec!["remote-status", "one"]
        );
        drain_icu_search_queue(&pool).await;
        assert_eq!(
            icu_search(&pool, "crossnetwork").await,
            vec!["remote-status", "one"]
        );
    }

    #[tokio::test]
    async fn incomplete_backfill_combines_indexed_and_unindexed_statuses() {
        let pool = icu_search_fixture().await;
        sqlx::query(
            "DELETE FROM status_search_icu_content
              WHERE status_id = 'three' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .expect("make one fixture status unindexed");
        sqlx::query(
            "UPDATE status_search_icu_backfill_state
                SET completed = 0, cursor_status_id = NULL, cursor_server_domain = NULL
              WHERE singleton = 1",
        )
        .execute(&pool)
        .await
        .expect("mark fixture ICU rebuild incomplete");

        assert_eq!(
            icu_search_with_completion(&pool, "unrelated", false).await,
            vec!["three"],
            "queries remain correct while the low-priority rebuild is in progress"
        );
        assert_eq!(
            icu_search_with_completion(&pool, "needle", false).await,
            vec!["two", "one"],
            "already indexed rows use the ICU candidate during the same rebuild"
        );
    }

    #[tokio::test]
    async fn incomplete_icu_cursor_searches_the_bounded_recent_migration_window() {
        let pool = icu_search_fixture().await;
        sqlx::raw_sql(
            "DELETE FROM status_search_icu_content
              WHERE status_id IN ('one', 'three');
             UPDATE status_search_icu_backfill_state
                SET cursor_status_id = 'two',
                    cursor_server_domain = 'example.test',
                    processed_count = 1,
                    total_count = 3,
                    completed = 0
              WHERE singleton = 1;",
        )
        .execute(&pool)
        .await
        .expect("prepare a partial ICU backfill cursor");

        assert_eq!(
            icu_search_with_completion(&pool, "needle", false).await,
            vec!["two", "one"],
            "indexed and ICU-evaluated migration-gap rows are combined"
        );
        assert_eq!(
            icu_search_with_completion(&pool, "unrelated", false).await,
            vec!["three"],
            "the bounded recent migration window remains searchable"
        );
        assert_eq!(
            icu_search_with_completion(&pool, "STRASSE", false).await,
            vec!["one"],
            "migration-gap evaluation uses the same ICU case folding as FTS"
        );
    }

    #[tokio::test]
    async fn cancelled_icu_search_stops_before_sql_execution() {
        let pool = icu_search_fixture().await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = query_statuses_with_cancellation(
            &pool,
            SearchQuery {
                query: "a",
                limit: 40,
                offset: 0,
                display_filter: StatusDisplayFilter::default(),
                start_after: None,
            },
            &cancellation,
        )
        .await
        .expect_err("cancelled search must not keep scanning SQLite");
        assert!(error.to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn more_than_eight_terms_is_rejected_before_candidate_planning() {
        let pool = icu_search_fixture().await;
        let error = query_statuses(
            &pool,
            SearchQuery {
                query: "one two three four five six seven eight nine",
                limit: 40,
                offset: 0,
                display_filter: StatusDisplayFilter::default(),
                start_after: None,
            },
        )
        .await
        .expect_err("unbounded term expansion must be rejected");
        assert!(error.to_string().contains("too many terms"));
    }
}
