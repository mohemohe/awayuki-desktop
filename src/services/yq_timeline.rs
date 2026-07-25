//! Bounded YQ timeline scan over the portable SQLite cache.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::db::models::{DbAccount, DbStatus};
use crate::db::queries::accounts;
use crate::services::yq_filter::{self, SqlPrefilterValue};

pub(crate) const FILTER_PAGE_SIZE: i64 = 250;
const MIN_SCANNED_ROWS: usize = 25_000;
const ABSOLUTE_MAX_SCANNED_ROWS: usize = 2_000_000;
const MIN_QUERY_DURATION: Duration = Duration::from_secs(15);
const MAX_QUERY_DURATION: Duration = Duration::from_secs(120);
const QUERY_DURATION_PER_100K_STATUSES: Duration = Duration::from_secs(10);
const SLOW_QUERY_DURATION: Duration = Duration::from_millis(500);
const SLOW_QUERY_SCANNED_ROWS: usize = 10_000;

pub const CANCELLED_ERROR: &str = "YQ query cancelled";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YqQueryMetrics {
    pub scanned_count: usize,
    pub matched_count: usize,
    pub duration_ms: u64,
    pub max_scanned_rows: usize,
    pub max_duration_ms: u64,
    pub slow: bool,
}

pub struct YqQueryResult {
    pub statuses: Vec<DbStatus>,
    pub metrics: YqQueryMetrics,
}

#[derive(Debug, Clone, Copy)]
struct QueryBudget {
    max_scanned_rows: usize,
    max_duration: Duration,
}

impl QueryBudget {
    fn for_status_count(status_count: usize) -> Self {
        let max_scanned_rows = status_count
            .saturating_add(FILTER_PAGE_SIZE as usize)
            .clamp(MIN_SCANNED_ROWS, ABSOLUTE_MAX_SCANNED_ROWS);
        let row_steps = status_count.div_ceil(100_000).max(1);
        let adaptive_duration = MIN_QUERY_DURATION.saturating_add(
            QUERY_DURATION_PER_100K_STATUSES
                .saturating_mul(row_steps.min(u32::MAX as usize) as u32),
        );
        Self {
            max_scanned_rows,
            max_duration: adaptive_duration.min(MAX_QUERY_DURATION),
        }
    }
}

#[derive(Debug, Clone)]
struct TimelineCursor {
    created_at: String,
    server_domain: String,
    id: String,
}

pub async fn query_statuses(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    stop_at: Option<(&str, &str)>,
    start_after: Option<(&str, &str)>,
    cancellation: &CancellationToken,
) -> Result<YqQueryResult, String> {
    let started_at = Instant::now();
    ensure_not_cancelled(cancellation)?;
    let compiled_query = yq_filter::compile_query(query)?;
    let budget = query_budget(pool).await;
    let evaluation_cache = yq_filter::EvaluationCache::default();
    let requested_limit = limit.max(0) as usize;
    let requested_offset = offset.max(0) as usize;
    if requested_limit == 0 {
        return Ok(YqQueryResult {
            statuses: Vec::new(),
            metrics: query_metrics(started_at, budget, 0, 0),
        });
    }
    let stop_cursor = resolve_cursor(pool, stop_at).await?;
    let stop_key = if stop_cursor.is_some() { None } else { stop_at };
    let mut cursor = resolve_cursor(pool, start_after).await?;
    let matches_to_skip = if cursor.is_some() {
        0
    } else {
        requested_offset
    };
    let mut matched_before_page = 0usize;
    let mut results = Vec::with_capacity(requested_limit);
    let mut scanned_count = 0usize;
    let mut stopped_at_since = false;

    while results.len() < requested_limit {
        ensure_not_cancelled(cancellation)?;
        if scanned_count >= budget.max_scanned_rows {
            return Err(format!(
                "YQ query scanned more than {} statuses; add a selective condition",
                budget.max_scanned_rows
            ));
        }
        if started_at.elapsed() >= budget.max_duration {
            return Err(
                "YQ query exceeded its execution budget; add a selective condition".to_string(),
            );
        }
        tokio::task::yield_now().await;

        let page_limit =
            (budget.max_scanned_rows - scanned_count).min(FILTER_PAGE_SIZE as usize) as i64;
        let mut conditions = Vec::new();
        if !compiled_query.sql_prefilter().is_empty() {
            conditions.push(compiled_query.sql_prefilter().clause().to_string());
        }
        if stop_cursor.is_some() {
            conditions.push(
                "(s.created_at > ? OR (s.created_at = ? AND (s.server_domain > ? OR (s.server_domain = ? AND s.id > ?))))"
                    .to_string(),
            );
        }
        if cursor.is_some() {
            conditions.push(
                "(s.created_at < ? OR (s.created_at = ? AND (s.server_domain < ? OR (s.server_domain = ? AND s.id < ?))))"
                    .to_string(),
            );
        }
        let where_sql = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT s.* FROM statuses s
             {where_sql}
             ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
             LIMIT ?"
        );
        // The YQ compiler emits the SQL predicate; user values and cursors are
        // represented by the bindings applied below.
        let mut db_query = sqlx::query_as::<_, DbStatus>(sqlx::AssertSqlSafe(sql));
        for binding in compiled_query.sql_prefilter().bindings() {
            db_query = match binding {
                SqlPrefilterValue::Text(value) => db_query.bind(value),
                SqlPrefilterValue::Integer(value) => db_query.bind(*value),
            };
        }
        if let Some(stop_cursor) = stop_cursor.as_ref() {
            db_query = db_query
                .bind(&stop_cursor.created_at)
                .bind(&stop_cursor.created_at)
                .bind(&stop_cursor.server_domain)
                .bind(&stop_cursor.server_domain)
                .bind(&stop_cursor.id);
        }
        if let Some(cursor) = cursor.as_ref() {
            db_query = db_query
                .bind(&cursor.created_at)
                .bind(&cursor.created_at)
                .bind(&cursor.server_domain)
                .bind(&cursor.server_domain)
                .bind(&cursor.id);
        }
        let rows = tokio::select! {
            _ = cancellation.cancelled() => return Err(CANCELLED_ERROR.to_string()),
            rows = db_query.bind(page_limit).fetch_all(pool) => rows.map_err(|error| error.to_string())?,
        };

        if rows.is_empty() {
            break;
        }
        let reached_end = rows.len() < page_limit as usize;
        if let Some(last) = rows.last() {
            cursor = Some(TimelineCursor {
                created_at: last.created_at.clone(),
                server_domain: last.server_domain.clone(),
                id: last.id.clone(),
            });
        }
        let mut account_cache: HashMap<(String, String), Option<DbAccount>> = HashMap::new();
        let mut missing_account_keys = Vec::new();
        for status in &rows {
            if stop_key.is_some_and(|(id, server_domain)| {
                status.id == id && status.server_domain == server_domain
            }) {
                break;
            }
            let account_key = (status.account_id.clone(), status.server_domain.clone());
            if !account_cache.contains_key(&account_key)
                && !missing_account_keys.contains(&account_key)
            {
                missing_account_keys.push(account_key);
            }
        }
        let loaded_accounts = tokio::select! {
            _ = cancellation.cancelled() => return Err(CANCELLED_ERROR.to_string()),
            accounts = accounts::get_accounts_by_keys(pool, &missing_account_keys) => {
                accounts.map_err(|error| error.to_string())?
            }
        };
        for account in loaded_accounts {
            account_cache.insert(
                (account.id.clone(), account.server_domain.clone()),
                Some(account),
            );
        }
        for account_key in missing_account_keys {
            account_cache.entry(account_key).or_insert(None);
        }

        {
            let mut evaluator = yq_filter::Evaluator::with_cache(evaluation_cache.clone());
            for status in rows {
                if scanned_count.is_multiple_of(64) {
                    ensure_not_cancelled(cancellation)?;
                }
                if stop_key.is_some_and(|(id, server_domain)| {
                    status.id == id && status.server_domain == server_domain
                }) {
                    stopped_at_since = true;
                    break;
                }
                scanned_count += 1;
                if scanned_count.is_multiple_of(64) && started_at.elapsed() >= budget.max_duration {
                    return Err(
                        "YQ query exceeded its execution budget; add a selective condition"
                            .to_string(),
                    );
                }
                let account_key = (status.account_id.clone(), status.server_domain.clone());
                let account = account_cache
                    .get(&account_key)
                    .and_then(|account| account.as_ref());
                if !evaluator.matches(&compiled_query, &status, account) {
                    continue;
                }
                if matched_before_page < matches_to_skip {
                    matched_before_page += 1;
                    continue;
                }
                results.push(status);
                if results.len() >= requested_limit {
                    break;
                }
            }
        }

        if reached_end || stopped_at_since {
            break;
        }
    }

    tracing::info!(
        query,
        limit,
        offset,
        stop_at = ?stop_at,
        start_after = ?start_after,
        max_scanned_rows = budget.max_scanned_rows,
        max_duration_ms = budget.max_duration.as_millis(),
        scanned_count,
        matched_count = results.len(),
        stopped_at_since,
        duration_ms = elapsed_ms(started_at),
        "[awayuki][tauri-db] yq query scan complete"
    );
    let metrics = query_metrics(started_at, budget, scanned_count, results.len());
    Ok(YqQueryResult {
        statuses: results,
        metrics,
    })
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), String> {
    if cancellation.is_cancelled() {
        Err(CANCELLED_ERROR.to_string())
    } else {
        Ok(())
    }
}

fn query_metrics(
    started_at: Instant,
    budget: QueryBudget,
    scanned_count: usize,
    matched_count: usize,
) -> YqQueryMetrics {
    let duration = started_at.elapsed();
    YqQueryMetrics {
        scanned_count,
        matched_count,
        duration_ms: duration.as_millis().min(u64::MAX as u128) as u64,
        max_scanned_rows: budget.max_scanned_rows,
        max_duration_ms: budget.max_duration.as_millis().min(u64::MAX as u128) as u64,
        slow: duration >= SLOW_QUERY_DURATION || scanned_count >= SLOW_QUERY_SCANNED_ROWS,
    }
}

async fn resolve_cursor(
    pool: &SqlitePool,
    status: Option<(&str, &str)>,
) -> Result<Option<TimelineCursor>, String> {
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
    .fetch_optional(pool)
    .await
    .map(|cursor| {
        cursor.map(|(created_at, server_domain, id)| TimelineCursor {
            created_at,
            server_domain,
            id,
        })
    })
    .map_err(|error| error.to_string())
}

async fn query_budget(pool: &SqlitePool) -> QueryBudget {
    let cached_count =
        sqlx::query_scalar::<_, i64>("SELECT value FROM cache_counters WHERE name = 'statuses'")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let status_count = match cached_count {
        Some(count) => count,
        None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM statuses")
            .fetch_one(pool)
            .await
            .unwrap_or_default(),
    };
    QueryBudget::for_status_count(status_count.max(0) as usize)
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_bounded_for_empty_and_large_databases() {
        let empty = QueryBudget::for_status_count(0);
        assert_eq!(empty.max_scanned_rows, MIN_SCANNED_ROWS);
        let large = QueryBudget::for_status_count(usize::MAX);
        assert_eq!(large.max_scanned_rows, ABSOLUTE_MAX_SCANNED_ROWS);
        assert_eq!(large.max_duration, MAX_QUERY_DURATION);
    }

    #[tokio::test]
    async fn cancelled_query_stops_before_sql_or_evaluation() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = query_statuses(&pool, "where t", 40, 0, None, None, &cancellation)
            .await
            .err()
            .expect("cancelled query");
        assert_eq!(error, CANCELLED_ERROR);
    }

    #[test]
    fn query_metrics_classify_slow_duration_or_scan_volume() {
        let budget = QueryBudget::for_status_count(20_000);
        assert!(!query_metrics(Instant::now(), budget, 100, 1).slow);
        assert!(query_metrics(Instant::now() - SLOW_QUERY_DURATION, budget, 100, 1).slow);
        assert!(query_metrics(Instant::now(), budget, SLOW_QUERY_SCANNED_ROWS, 1).slow);
    }
}
