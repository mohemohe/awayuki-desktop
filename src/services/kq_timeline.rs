//! Bounded KQ timeline scans over Awayuki's provider-neutral SQLite cache.
//!
//! The scanner deliberately treats SQL as a candidate prefilter only. KQ's
//! in-memory evaluator remains authoritative because several Fediverse values
//! (boost authors, per-login viewer state, and timeline membership) live
//! outside the canonical `statuses` row.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};
use tokio_util::sync::CancellationToken;

use crate::application::timeline_hydration::CachedStatusViewContext;
use crate::db::models::{DbAccount, DbStatus, DbStatusViewerState};
use crate::db::queries::read_models;
use crate::db::queries::statuses as status_queries;
use crate::services::kq_filter::{
    self, EvaluationContext, LoginAccountIdentity, SqlPrefilterValue, StatusKey, StatusView,
    TimelineMembership,
};

pub(crate) const FILTER_PAGE_SIZE: i64 = 250;
const MIN_SCANNED_ROWS: usize = 25_000;
const ABSOLUTE_MAX_SCANNED_ROWS: usize = 2_000_000;
const MIN_QUERY_DURATION: Duration = Duration::from_secs(10);
const MAX_QUERY_DURATION: Duration = Duration::from_secs(25);
const QUERY_DURATION_PER_100K_STATUSES: Duration = Duration::from_secs(1);
const SLOW_QUERY_DURATION: Duration = Duration::from_millis(500);
const SLOW_QUERY_SCANNED_ROWS: usize = 10_000;
const MAX_CONVERSATION_SOURCES: usize = 8;
const MAX_CONVERSATION_ROOTS: usize = 32;
const MAX_CONVERSATION_STATUSES_PER_SOURCE: usize = 500;
const MAX_CONVERSATION_STATUSES: usize = 4_000;
const COMPILED_QUERY_CACHE_CAPACITY: usize = 64;
const MAX_CACHED_QUERY_BYTES: usize = 16 * 1024;

type EntityKey = (String, String);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KqQueryMetrics {
    pub engine: &'static str,
    pub scanned_count: usize,
    pub matched_count: usize,
    pub duration_ms: u64,
    pub max_scanned_rows: usize,
    pub max_duration_ms: u64,
    pub slow: bool,
}

#[derive(Debug)]
pub struct KqQueryResult {
    pub statuses: Vec<DbStatus>,
    pub metrics: KqQueryMetrics,
}

#[derive(Debug, thiserror::Error)]
pub enum KqTimelineError {
    #[error("Invalid KQ query at line {line}, column {column}: {message}")]
    Compile {
        message: String,
        position: usize,
        line: usize,
        column: usize,
    },
    #[error(
        "KQ query exceeded its execution budget after scanning {scanned_count} statuses; add a selective condition"
    )]
    Timeout {
        scanned_count: usize,
        max_scanned_rows: usize,
        max_duration_ms: u64,
    },
    #[error("KQ query cancelled")]
    Cancelled,
    #[error("KQ database query failed: {0}")]
    Database(#[source] sqlx::Error),
}

impl From<sqlx::Error> for KqTimelineError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
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

#[derive(Debug, Clone, FromRow)]
struct TimelineMembershipRow {
    status_id: String,
    server_domain: String,
    timeline_type: String,
    account_acct: String,
}

/// The non-secret subset of `login_accounts` that KQ is allowed to inspect.
///
/// Keep the explicit projection in `load_login_account_identities`; using
/// `SELECT *` here would pull OAuth tokens and Bluesky app passwords into the
/// query path even though KQ never needs credentials.
#[derive(Debug, Clone, FromRow)]
struct LoginAccountIdentityRow {
    acct: String,
    server_domain: String,
    account_id: String,
    display_name: String,
    server_kind: String,
    is_active: bool,
}

#[derive(Default)]
struct CompiledQueryCache {
    entries: VecDeque<(String, Arc<kq_filter::CompiledQuery>)>,
}

impl CompiledQueryCache {
    fn get(&mut self, query: &str) -> Option<Arc<kq_filter::CompiledQuery>> {
        let index = self
            .entries
            .iter()
            .position(|(cached_query, _)| cached_query == query)?;
        let entry = self.entries.remove(index)?;
        let compiled = Arc::clone(&entry.1);
        self.entries.push_front(entry);
        Some(compiled)
    }

    fn insert(&mut self, query: String, compiled: Arc<kq_filter::CompiledQuery>) {
        self.entries
            .retain(|(cached_query, _)| cached_query != &query);
        self.entries.push_front((query, compiled));
        self.entries.truncate(COMPILED_QUERY_CACHE_CAPACITY);
    }
}

fn compiled_query_cache() -> &'static Mutex<CompiledQueryCache> {
    static CACHE: OnceLock<Mutex<CompiledQueryCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(CompiledQueryCache::default()))
}

fn compile_query_cached(query: &str) -> Result<Arc<kq_filter::CompiledQuery>, KqTimelineError> {
    if query.len() <= MAX_CACHED_QUERY_BYTES {
        let mut cache = compiled_query_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(compiled) = cache.get(query) {
            return Ok(compiled);
        }
    }

    let compiled =
        Arc::new(
            kq_filter::compile_query(query).map_err(|error| KqTimelineError::Compile {
                message: error.message().to_string(),
                position: error.offset(),
                line: error.line(),
                column: error.column(),
            })?,
        );
    if query.len() <= MAX_CACHED_QUERY_BYTES {
        compiled_query_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(query.to_string(), Arc::clone(&compiled));
    }
    Ok(compiled)
}

pub async fn query_statuses(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    stop_at: Option<(&str, &str)>,
    start_after: Option<(&str, &str)>,
    cancellation: &CancellationToken,
) -> Result<KqQueryResult, KqTimelineError> {
    let started_at = Instant::now();
    ensure_not_cancelled(cancellation)?;
    let compiled_query = compile_query_cached(query)?;
    ensure_not_cancelled(cancellation)?;

    let provisional_budget = QueryBudget::for_status_count(ABSOLUTE_MAX_SCANNED_ROWS);
    let absolute_deadline = tokio::time::Instant::now() + MAX_QUERY_DURATION;
    let budget = await_with_guards(
        query_budget(pool),
        cancellation,
        absolute_deadline,
        provisional_budget,
        0,
    )
    .await?;
    let deadline =
        tokio::time::Instant::now() + budget.max_duration.saturating_sub(started_at.elapsed());
    ensure_budget_remaining(started_at, budget, 0)?;

    let requested_limit = limit.max(0) as usize;
    let requested_offset = offset.max(0) as usize;
    if requested_limit == 0 {
        return Ok(KqQueryResult {
            statuses: Vec::new(),
            metrics: query_metrics(started_at, budget, 0, 0),
        });
    }

    let requirements = compiled_query.requirements();
    let viewer_state_source = query_uses_viewer_state_source(&compiled_query);
    let login_accounts =
        if requirements.login_accounts || requirements.viewer_states || viewer_state_source {
            await_with_guards(
                load_login_account_identities(pool),
                cancellation,
                deadline,
                budget,
                0,
            )
            .await?
        } else {
            Vec::new()
        };
    let conversation_keys = if requirements.conversations {
        let keys = await_with_guards(
            resolve_conversation_keys(pool, compiled_query.conversation_ids(), budget),
            cancellation,
            deadline,
            budget,
            0,
        )
        .await?;
        let mut keys = keys
            .into_iter()
            .map(|(id, server_domain)| StatusKey::new(server_domain, id))
            .collect::<Vec<_>>();
        keys.sort_by(|left, right| {
            (&left.server_domain, &left.id).cmp(&(&right.server_domain, &right.id))
        });
        keys
    } else {
        Vec::new()
    };

    let stop_cursor = await_with_guards(
        resolve_cursor(pool, stop_at),
        cancellation,
        deadline,
        budget,
        0,
    )
    .await?;
    let stop_key = if stop_cursor.is_some() { None } else { stop_at };
    let mut cursor = await_with_guards(
        resolve_cursor(pool, start_after),
        cancellation,
        deadline,
        budget,
        0,
    )
    .await?;
    let matches_to_skip = if cursor.is_some() {
        0
    } else {
        requested_offset
    };

    let mut matched_before_page = 0usize;
    let mut results = Vec::with_capacity(requested_limit);
    let mut scanned_count = 0usize;
    let mut stopped_at_since = false;
    let mut evaluator = kq_filter::Evaluator::new();

    while results.len() < requested_limit {
        ensure_not_cancelled(cancellation)?;
        ensure_budget_remaining(started_at, budget, scanned_count)?;
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
        // Only compiler-owned SQL fragments reach this statement. Every KQ
        // literal and every cursor component is bound below.
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
        let mut rows = await_with_guards(
            async {
                db_query
                    .bind(page_limit)
                    .fetch_all(pool)
                    .await
                    .map_err(KqTimelineError::from)
            },
            cancellation,
            deadline,
            budget,
            scanned_count,
        )
        .await?;

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
        if let Some((stop_id, stop_domain)) = stop_key {
            if let Some(stop_index) = rows
                .iter()
                .position(|status| status.id == stop_id && status.server_domain == stop_domain)
            {
                rows.truncate(stop_index);
                stopped_at_since = true;
            }
        }
        if rows.is_empty() {
            break;
        }

        let page_keys = rows
            .iter()
            .map(|status| entity_key(&status.id, &status.server_domain))
            .collect::<Vec<_>>();
        let needs_memberships =
            requirements.memberships || requirements.viewer_states || viewer_state_source;
        let (hydration, memberships) = await_with_guards(
            async {
                let hydration =
                    CachedStatusViewContext::load(pool, &rows)
                        .await
                        .map_err(|error| {
                            KqTimelineError::Database(sqlx::Error::Protocol(format!(
                                "KQ status hydration failed: {error}"
                            )))
                        })?;
                let memberships = if needs_memberships {
                    load_timeline_memberships(pool, &page_keys).await?
                } else {
                    HashMap::new()
                };
                Ok((hydration, memberships))
            },
            cancellation,
            deadline,
            budget,
            scanned_count,
        )
        .await?;

        let wrapper_to_effective = rows
            .iter()
            .filter_map(|status| {
                let wrapper_key = entity_key(&status.id, &status.server_domain);
                effective_status(status, &hydration).map(|effective| {
                    (
                        wrapper_key,
                        entity_key(&effective.id, &effective.server_domain),
                    )
                })
            })
            .collect::<HashMap<_, _>>();
        let viewer_states = if requirements.viewer_states || viewer_state_source {
            await_with_guards(
                load_viewer_states(
                    pool,
                    &memberships,
                    &wrapper_to_effective,
                    &login_accounts,
                    requirements.viewer_states || viewer_state_source,
                ),
                cancellation,
                deadline,
                budget,
                scanned_count,
            )
            .await?
        } else {
            HashMap::new()
        };

        for status in &rows {
            if scanned_count.is_multiple_of(64) {
                ensure_not_cancelled(cancellation)?;
                ensure_budget_remaining(started_at, budget, scanned_count)?;
            }
            scanned_count += 1;

            let wrapper_key = entity_key(&status.id, &status.server_domain);
            let row_memberships = memberships
                .get(&wrapper_key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let wrapper = StatusView::new(status, account_for_status(&hydration, status));
            let effective_status = effective_status(status, &hydration);
            let effective = effective_status.map(|effective| {
                StatusView::new(effective, account_for_status(&hydration, effective))
            });
            let quote = if requirements.quote_status {
                effective_status.and_then(|effective| {
                    quote_status(effective, &hydration)
                        .map(|quote| StatusView::new(quote, account_for_status(&hydration, quote)))
                })
            } else {
                None
            };
            let row_viewer_states = effective_status
                .and_then(|effective| {
                    viewer_states.get(&entity_key(&effective.id, &effective.server_domain))
                })
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut context = EvaluationContext::new(wrapper, effective);
            context.quote = quote;
            context.login_accounts = &login_accounts;
            context.memberships = row_memberships;
            context.viewer_states = row_viewer_states;
            context.conversation_keys = &conversation_keys;

            let matches = evaluator.matches(&compiled_query, &context);
            ensure_scan_within_guards(cancellation, started_at, budget, scanned_count)?;
            if !matches {
                continue;
            }
            if matched_before_page < matches_to_skip {
                matched_before_page += 1;
                continue;
            }
            results.push(status.clone());
            if results.len() >= requested_limit {
                break;
            }
        }

        if reached_end || stopped_at_since {
            break;
        }
    }

    // Do not return a partial success if cancellation or the deadline arrives
    // after the last evaluated row fills the requested page.
    ensure_scan_within_guards(cancellation, started_at, budget, scanned_count)?;

    tracing::info!(
        engine = kq_filter::ENGINE_ID,
        query_bytes = query.len(),
        source_count = compiled_query.sources().len(),
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
        "[awayuki][tauri-db] kq query scan complete"
    );
    let metrics = query_metrics(started_at, budget, scanned_count, results.len());
    Ok(KqQueryResult {
        statuses: results,
        metrics,
    })
}

#[cfg(test)]
fn compile_error(query: &str, message: impl Into<String>, position: usize) -> KqTimelineError {
    let (line, column) = line_and_column(query, position);
    KqTimelineError::Compile {
        message: message.into(),
        position: position.min(query.len()),
        line,
        column,
    }
}

#[cfg(test)]
fn line_and_column(query: &str, position: usize) -> (usize, usize) {
    let safe_position = floor_char_boundary(query, position.min(query.len()));
    let prefix = &query[..safe_position];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, current_line)| current_line)
        .chars()
        .count()
        + 1;
    (line, column)
}

#[cfg(test)]
fn floor_char_boundary(value: &str, mut position: usize) -> usize {
    while position > 0 && !value.is_char_boundary(position) {
        position -= 1;
    }
    position
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), KqTimelineError> {
    if cancellation.is_cancelled() {
        Err(KqTimelineError::Cancelled)
    } else {
        Ok(())
    }
}

fn ensure_scan_within_guards(
    cancellation: &CancellationToken,
    started_at: Instant,
    budget: QueryBudget,
    scanned_count: usize,
) -> Result<(), KqTimelineError> {
    ensure_not_cancelled(cancellation)?;
    ensure_budget_remaining(started_at, budget, scanned_count)
}

fn ensure_budget_remaining(
    started_at: Instant,
    budget: QueryBudget,
    scanned_count: usize,
) -> Result<(), KqTimelineError> {
    if scanned_count >= budget.max_scanned_rows || started_at.elapsed() >= budget.max_duration {
        Err(timeout_error(budget, scanned_count))
    } else {
        Ok(())
    }
}

fn timeout_error(budget: QueryBudget, scanned_count: usize) -> KqTimelineError {
    KqTimelineError::Timeout {
        scanned_count,
        max_scanned_rows: budget.max_scanned_rows,
        max_duration_ms: duration_ms(budget.max_duration),
    }
}

async fn await_with_guards<T, F>(
    future: F,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
    budget: QueryBudget,
    scanned_count: usize,
) -> Result<T, KqTimelineError>
where
    F: Future<Output = Result<T, KqTimelineError>>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(KqTimelineError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(timeout_error(budget, scanned_count)),
        result = &mut future => result,
    }
}

fn query_metrics(
    started_at: Instant,
    budget: QueryBudget,
    scanned_count: usize,
    matched_count: usize,
) -> KqQueryMetrics {
    let duration = started_at.elapsed();
    KqQueryMetrics {
        engine: kq_filter::ENGINE_ID,
        scanned_count,
        matched_count,
        duration_ms: duration_ms(duration),
        max_scanned_rows: budget.max_scanned_rows,
        max_duration_ms: duration_ms(budget.max_duration),
        slow: duration >= SLOW_QUERY_DURATION || scanned_count >= SLOW_QUERY_SCANNED_ROWS,
    }
}

async fn query_budget(pool: &SqlitePool) -> Result<QueryBudget, KqTimelineError> {
    let cached_count =
        sqlx::query_scalar::<_, i64>("SELECT value FROM cache_counters WHERE name = 'statuses'")
            .fetch_optional(pool)
            .await?;
    let status_count = match cached_count {
        Some(count) => count,
        None => {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM statuses")
                .fetch_one(pool)
                .await?
        }
    };
    Ok(QueryBudget::for_status_count(status_count.max(0) as usize))
}

async fn resolve_cursor(
    pool: &SqlitePool,
    status: Option<(&str, &str)>,
) -> Result<Option<TimelineCursor>, KqTimelineError> {
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
    .map_err(KqTimelineError::from)
}

async fn load_timeline_memberships(
    pool: &SqlitePool,
    keys: &[EntityKey],
) -> Result<HashMap<EntityKey, Vec<TimelineMembership>>, KqTimelineError> {
    let mut memberships = HashMap::<EntityKey, Vec<TimelineMembership>>::new();
    for chunk in keys.chunks(FILTER_PAGE_SIZE as usize) {
        if chunk.is_empty() {
            continue;
        }
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT status_id, server_domain, timeline_type, account_acct
             FROM timeline_entries WHERE ",
        );
        push_entity_key_predicates(&mut builder, chunk, "status_id", "server_domain");
        builder.push(" ORDER BY position_at DESC, account_acct DESC, timeline_type DESC");
        for row in builder
            .build_query_as::<TimelineMembershipRow>()
            .fetch_all(pool)
            .await?
        {
            let (timeline_type, parameter) = normalize_timeline_membership(&row.timeline_type);
            memberships
                .entry((row.status_id, row.server_domain))
                .or_default()
                .push(TimelineMembership::new(
                    timeline_type,
                    row.account_acct,
                    parameter,
                ));
        }
    }
    Ok(memberships)
}

fn normalize_timeline_membership(stored: &str) -> (String, Option<String>) {
    if let Some(parameter) = stored.strip_prefix("list:") {
        return ("list".to_string(), Some(parameter.to_string()));
    }
    if let Some(parameter) = stored.strip_prefix("tag:") {
        return ("hashtag".to_string(), Some(parameter.to_string()));
    }
    (stored.to_string(), None)
}

async fn load_login_account_identities(
    pool: &SqlitePool,
) -> Result<Vec<LoginAccountIdentity>, KqTimelineError> {
    sqlx::query_as::<_, LoginAccountIdentityRow>(
        "SELECT acct, server_domain, account_id, display_name, server_kind, is_active
         FROM login_accounts
         ORDER BY is_active DESC, acct",
    )
    .fetch_all(pool)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| LoginAccountIdentity {
                acct: row.acct,
                server_domain: row.server_domain,
                account_id: row.account_id,
                display_name: row.display_name,
                server_kind: row.server_kind,
                is_active: row.is_active,
            })
            .collect()
    })
    .map_err(KqTimelineError::from)
}

async fn load_viewer_states(
    pool: &SqlitePool,
    memberships: &HashMap<EntityKey, Vec<TimelineMembership>>,
    wrapper_to_effective: &HashMap<EntityKey, EntityKey>,
    login_accounts: &[LoginAccountIdentity],
    include_all_login_accounts: bool,
) -> Result<HashMap<EntityKey, Vec<DbStatusViewerState>>, KqTimelineError> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for (wrapper_key, effective_key) in wrapper_to_effective {
        let mut scoped_accounts = memberships
            .get(wrapper_key)
            .into_iter()
            .flatten()
            .map(|membership| membership.account_acct.clone())
            .collect::<HashSet<_>>();
        if include_all_login_accounts {
            // Bookmarks/favourites have no timeline_entries, while mentions
            // and direct sources can derive their account scope from status
            // data alone. These are candidate states only: the evaluator still
            // derives a unique scope per matching source branch and never
            // aggregates them for an unscoped/ambiguous branch.
            scoped_accounts.extend(login_accounts.iter().map(|account| account.acct.clone()));
        }
        for account_acct in scoped_accounts {
            let key = (
                account_acct,
                effective_key.0.clone(),
                effective_key.1.clone(),
            );
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }
    }
    status_queries::get_viewer_states_by_keys(pool, &keys)
        .await
        .map(|states| {
            let mut by_status = HashMap::<EntityKey, Vec<DbStatusViewerState>>::new();
            for state in states.into_values() {
                by_status
                    .entry((state.status_id.clone(), state.server_domain.clone()))
                    .or_default()
                    .push(state);
            }
            by_status
        })
        .map_err(KqTimelineError::from)
}

fn query_uses_viewer_state_source(query: &kq_filter::CompiledQuery) -> bool {
    query.sources().iter().any(|source| {
        matches!(
            source.kind,
            kq_filter::SourceKind::Bookmarks | kq_filter::SourceKind::Favourites
        )
    })
}

/// Resolve already-cached conversation trees without unbounded recursion.
///
/// The read model uses a cycle-safe recursive CTE and caps each tree at 500
/// statuses. The source compiler is expected to reject more than eight roots;
/// this defensive check prevents a future caller from multiplying work.
async fn resolve_conversation_keys(
    pool: &SqlitePool,
    source_values: &[String],
    budget: QueryBudget,
) -> Result<HashSet<EntityKey>, KqTimelineError> {
    if source_values.len() > MAX_CONVERSATION_SOURCES {
        return Err(timeout_error(budget, 0));
    }
    let mut roots = Vec::new();
    let mut seen_roots = HashSet::new();
    for source_value in source_values {
        let remaining = MAX_CONVERSATION_ROOTS
            .saturating_sub(roots.len())
            .saturating_add(1) as i64;
        let candidates = if source_value.contains("://") {
            sqlx::query_as::<_, (String, String)>(
                "SELECT id, server_domain FROM statuses
                 WHERE uri = ? OR url = ?
                 ORDER BY server_domain, id LIMIT ?",
            )
            .bind(source_value)
            .bind(source_value)
            .bind(remaining)
            .fetch_all(pool)
            .await?
        } else if let Some((server_domain, status_id)) = source_value.split_once('/') {
            sqlx::query_as::<_, (String, String)>(
                "SELECT id, server_domain FROM statuses
                 WHERE id = ? AND server_domain = ? LIMIT ?",
            )
            .bind(status_id)
            .bind(server_domain)
            .bind(remaining)
            .fetch_all(pool)
            .await?
        } else {
            sqlx::query_as::<_, (String, String)>(
                "SELECT id, server_domain FROM statuses
                 WHERE id = ? ORDER BY server_domain LIMIT ?",
            )
            .bind(source_value)
            .bind(remaining)
            .fetch_all(pool)
            .await?
        };
        for root in candidates {
            if seen_roots.insert(root.clone()) {
                roots.push(root);
            }
        }
        if roots.len() > MAX_CONVERSATION_ROOTS {
            return Err(timeout_error(budget, 0));
        }
    }

    let mut keys = HashSet::new();
    for (status_id, server_domain) in &roots {
        if let Some(page) = read_models::query_thread_status_page(
            pool,
            status_id,
            server_domain,
            MAX_CONVERSATION_STATUSES_PER_SOURCE,
        )
        .await?
        {
            if page.statuses.len() >= MAX_CONVERSATION_STATUSES_PER_SOURCE {
                // The read model intentionally returns a bounded page. Never
                // evaluate against a silently truncated conversation set.
                return Err(timeout_error(budget, page.statuses.len()));
            }
            keys.extend(
                page.statuses
                    .into_iter()
                    .map(|status| (status.id, status.server_domain)),
            );
            if keys.len() > MAX_CONVERSATION_STATUSES {
                return Err(timeout_error(budget, keys.len()));
            }
        }
    }
    Ok(keys)
}

fn push_entity_key_predicates(
    builder: &mut QueryBuilder<Sqlite>,
    keys: &[EntityKey],
    id_column: &str,
    server_domain_column: &str,
) {
    for (index, (id, server_domain)) in keys.iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder
            .push("(")
            .push(id_column)
            .push(" = ")
            .push_bind(id)
            .push(" AND ")
            .push(server_domain_column)
            .push(" = ")
            .push_bind(server_domain)
            .push(")");
    }
}

fn entity_key(id: &str, server_domain: &str) -> EntityKey {
    (id.to_string(), server_domain.to_string())
}

fn account_for_status<'a>(
    hydration: &'a CachedStatusViewContext,
    status: &DbStatus,
) -> Option<&'a DbAccount> {
    hydration
        .accounts
        .get(&entity_key(&status.account_id, &status.server_domain))
}

fn effective_status<'a>(
    wrapper: &'a DbStatus,
    hydration: &'a CachedStatusViewContext,
) -> Option<&'a DbStatus> {
    match wrapper.reblog_of_id.as_deref() {
        Some(original_id) => hydration
            .statuses
            .get(&entity_key(original_id, &wrapper.server_domain)),
        None => Some(wrapper),
    }
}

fn quote_status<'a>(
    effective: &DbStatus,
    hydration: &'a CachedStatusViewContext,
) -> Option<&'a DbStatus> {
    let quote_id = effective.quote_id.as_deref()?;
    hydration
        .statuses
        .get(&entity_key(quote_id, &effective.server_domain))
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn elapsed_ms(started_at: Instant) -> u64 {
    duration_ms(started_at.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::Database;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    async fn migrated_database(label: &str) -> (Database, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-kq-timeline-{label}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = Database::new(directory.join("awayuki.db")).await.unwrap();
        let _ = database.run_migrations().await.unwrap();
        (database, directory)
    }

    #[test]
    fn budget_is_bounded_and_stays_below_frontend_timeout() {
        let empty = QueryBudget::for_status_count(0);
        assert_eq!(empty.max_scanned_rows, MIN_SCANNED_ROWS);
        assert!(empty.max_duration >= MIN_QUERY_DURATION);

        let large = QueryBudget::for_status_count(usize::MAX);
        assert_eq!(large.max_scanned_rows, ABSOLUTE_MAX_SCANNED_ROWS);
        assert_eq!(large.max_duration, MAX_QUERY_DURATION);
        assert!(large.max_duration < Duration::from_secs(30));
    }

    #[test]
    fn cancelled_error_is_typed() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = ensure_not_cancelled(&cancellation).expect_err("cancelled query");
        assert!(matches!(error, KqTimelineError::Cancelled));
    }

    #[test]
    fn final_scan_guard_rejects_cancelled_and_expired_results() {
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let budget = QueryBudget::for_status_count(1);
        assert!(matches!(
            ensure_scan_within_guards(&cancelled, Instant::now(), budget, 0),
            Err(KqTimelineError::Cancelled)
        ));

        let active = CancellationToken::new();
        let expired = QueryBudget {
            max_scanned_rows: 1,
            max_duration: Duration::ZERO,
        };
        assert!(matches!(
            ensure_scan_within_guards(&active, Instant::now(), expired, 0),
            Err(KqTimelineError::Timeout { .. })
        ));
    }

    #[tokio::test]
    async fn cancelled_query_stops_before_compile_or_sql() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = query_statuses(
            &pool,
            "from local where ()",
            40,
            0,
            None,
            None,
            &cancellation,
        )
        .await
        .expect_err("cancelled query");
        assert!(matches!(error, KqTimelineError::Cancelled));
    }

    #[test]
    fn compile_error_maps_utf8_position_to_one_based_line_and_column() {
        let query = "from local\nwhere text contains \"雪\"";
        let position = query.find('雪').unwrap();
        let error = compile_error(query, "invalid value", position);
        assert!(matches!(
            error,
            KqTimelineError::Compile {
                position: error_position,
                line: 2,
                column: 22,
                ..
            } if error_position == position
        ));
    }

    #[test]
    fn database_error_mapping_stays_distinct() {
        let error = KqTimelineError::from(sqlx::Error::RowNotFound);
        assert!(matches!(
            error,
            KqTimelineError::Database(sqlx::Error::RowNotFound)
        ));
    }

    #[test]
    fn timeout_error_carries_the_effective_budget() {
        let budget = QueryBudget::for_status_count(150_000);
        let error = timeout_error(budget, 10_000);
        assert!(matches!(
            error,
            KqTimelineError::Timeout {
                scanned_count: 10_000,
                max_scanned_rows,
                max_duration_ms,
            } if max_scanned_rows == budget.max_scanned_rows
                && max_duration_ms == duration_ms(budget.max_duration)
        ));
    }

    #[test]
    fn stored_parameterized_memberships_are_normalized_for_kq_sources() {
        assert_eq!(
            normalize_timeline_membership("list:friends"),
            ("list".to_string(), Some("friends".to_string()))
        );
        assert_eq!(
            normalize_timeline_membership("tag:rust"),
            ("hashtag".to_string(), Some("rust".to_string()))
        );
        assert_eq!(
            normalize_timeline_membership("public"),
            ("public".to_string(), None)
        );
    }

    #[tokio::test]
    async fn login_identity_loader_never_requires_secret_columns_in_its_shape() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE login_accounts (
                acct TEXT PRIMARY KEY,
                server_domain TEXT NOT NULL,
                account_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                server_kind TEXT NOT NULL,
                is_active INTEGER NOT NULL,
                access_token TEXT NOT NULL,
                app_password TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO login_accounts VALUES
             ('alice@example.test', 'example.test', 'alice-id', 'Alice', 'paon', 1,
              'must-not-be-loaded', 'also-must-not-be-loaded')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let identities = load_login_account_identities(&pool).await.unwrap();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].acct, "alice@example.test");
        assert_eq!(identities[0].server_kind, "paon");
    }

    #[tokio::test]
    async fn membership_loader_batches_rows_by_composite_status_key() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE timeline_entries (
                status_id TEXT NOT NULL,
                server_domain TEXT NOT NULL,
                timeline_type TEXT NOT NULL,
                account_acct TEXT NOT NULL,
                position_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO timeline_entries VALUES
             ('same-id', 'one.test', 'home', 'alice@one.test', '2026-08-09T02:00:00Z'),
             ('same-id', 'one.test', 'list:friends', 'alice@one.test', '2026-08-09T01:00:00Z'),
             ('same-id', 'two.test', 'home', 'bob@two.test', '2026-08-09T03:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let memberships =
            load_timeline_memberships(&pool, &[("same-id".to_string(), "one.test".to_string())])
                .await
                .unwrap();
        let rows = &memberships[&("same-id".to_string(), "one.test".to_string())];
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].timeline_type, "home");
        assert!(!memberships.contains_key(&("same-id".to_string(), "two.test".to_string())));
    }

    #[tokio::test]
    async fn viewer_state_source_loads_effective_status_without_timeline_membership() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE status_viewer_state (
                login_account_acct TEXT NOT NULL,
                status_id TEXT NOT NULL,
                server_domain TEXT NOT NULL,
                favourited INTEGER,
                reblogged INTEGER,
                muted INTEGER,
                bookmarked INTEGER,
                pinned INTEGER,
                updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO status_viewer_state VALUES
             ('alice@example.test', 'original', 'example.test', 1, 0, 0, 1, 0,
              '2026-08-09T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let login_accounts = vec![LoginAccountIdentity {
            acct: "alice@example.test".to_string(),
            server_domain: "example.test".to_string(),
            account_id: "alice-id".to_string(),
            display_name: "Alice".to_string(),
            server_kind: "mastodon".to_string(),
            is_active: true,
        }];
        let wrapper_to_effective = HashMap::from([(
            ("boost".to_string(), "example.test".to_string()),
            ("original".to_string(), "example.test".to_string()),
        )]);

        let states = load_viewer_states(
            &pool,
            &HashMap::new(),
            &wrapper_to_effective,
            &login_accounts,
            true,
        )
        .await
        .unwrap();
        let effective_states = &states[&("original".to_string(), "example.test".to_string())];
        assert_eq!(effective_states.len(), 1);
        assert_eq!(effective_states[0].login_account_acct, "alice@example.test");
        assert_eq!(effective_states[0].bookmarked, Some(true));
    }

    #[tokio::test]
    async fn mention_source_viewer_state_uses_derived_account_without_membership() {
        let (database, directory) = migrated_database("mention-viewer-scope").await;
        sqlx::query(
            "INSERT INTO servers(domain, streaming_url, server_kind)
             VALUES ('example.test', 'wss://example.test', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO accounts
               (id, server_domain, username, acct, display_name, created_at)
             VALUES
               ('author', 'example.test', 'author', 'author@example.test', 'Author',
                '2026-08-09T00:00:00Z')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO login_accounts
               (acct, server_domain, account_id, display_name, server_kind, is_active)
             VALUES
               ('alice@example.test', 'example.test', 'alice-id', 'Alice', 'mastodon', 1),
               ('bob@example.test', 'example.test', 'bob-id', 'Bob', 'mastodon', 0)",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO statuses
               (id, server_domain, uri, created_at, account_id, content, mentions_json, fetched_at)
             VALUES
               ('mentioned-favourited', 'example.test',
                'https://example.test/@author/mentioned-favourited', '2026-08-09T02:00:00Z',
                'author', '<p>favourited mention</p>', ?, '2026-08-09T02:00:00Z'),
               ('mentioned-by-other-viewer', 'example.test',
                'https://example.test/@author/mentioned-by-other-viewer', '2026-08-09T01:00:00Z',
                'author', '<p>other viewer mention</p>', ?, '2026-08-09T01:00:00Z')",
        )
        .bind(
            r#"[{"id":"alice-id","username":"alice","acct":"alice","url":"https://example.test/@alice"}]"#,
        )
        .bind(
            r#"[{"id":"alice-id","username":"alice","acct":"alice","url":"https://example.test/@alice"}]"#,
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO status_viewer_state
               (login_account_acct, status_id, server_domain, favourited)
             VALUES
               ('alice@example.test', 'mentioned-favourited', 'example.test', 1),
               ('bob@example.test', 'mentioned-favourited', 'example.test', 0),
               ('alice@example.test', 'mentioned-by-other-viewer', 'example.test', 0),
               ('bob@example.test', 'mentioned-by-other-viewer', 'example.test', 1)",
        )
        .execute(database.writer())
        .await
        .unwrap();

        let timeline_entry_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM timeline_entries")
                .fetch_one(database.analytics_reader())
                .await
                .unwrap();
        assert_eq!(timeline_entry_count, 0);

        let result = query_statuses(
            database.analytics_reader(),
            "from mentions where viewer.favourited",
            10,
            0,
            None,
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            result
                .statuses
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            vec!["mentioned-favourited"]
        );

        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn scan_crosses_page_boundary_and_cursor_ignores_offset() {
        let (database, directory) = migrated_database("page-boundary").await;
        sqlx::query(
            "INSERT INTO servers(domain, streaming_url, server_kind)
             VALUES ('example.test', 'wss://example.test', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO accounts
               (id, server_domain, username, acct, display_name, created_at)
             VALUES
               ('author', 'example.test', 'author', 'author@example.test', 'Author',
                '2026-08-09T00:00:00Z'),
               ('target', 'example.test', 'target', 'target@example.test', 'Target',
                '2026-08-09T00:00:00Z')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "WITH RECURSIVE sequence(value) AS (
                 SELECT 0
                 UNION ALL
                 SELECT value + 1 FROM sequence WHERE value < 259
             )
             INSERT INTO statuses
               (id, server_domain, uri, created_at, account_id, content, fetched_at)
             SELECT printf('s%03d', value),
                    'example.test',
                    printf('https://example.test/@author/s%03d', value),
                    printf('%06d', 260 - value),
                    CASE WHEN value IN (250, 255) THEN 'target' ELSE 'author' END,
                    '<p>status body</p>',
                    '2026-08-09T00:00:00Z'
             FROM sequence",
        )
        .execute(database.writer())
        .await
        .unwrap();

        let cancellation = CancellationToken::new();
        let first = query_statuses(
            database.analytics_reader(),
            "from home, local where author.username == \"target\"",
            1,
            0,
            None,
            None,
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(first.statuses[0].id, "s250");
        assert!(first.metrics.scanned_count > FILTER_PAGE_SIZE as usize);

        let conversation = query_statuses(
            database.analytics_reader(),
            "from conversation:\"example.test/s250\" where ()",
            1,
            0,
            None,
            None,
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(conversation.statuses[0].id, "s250");

        let second = query_statuses(
            database.analytics_reader(),
            "from home, local where author.username == \"target\"",
            1,
            999,
            None,
            Some(("s250", "example.test")),
            &cancellation,
        )
        .await
        .unwrap();
        assert_eq!(second.statuses[0].id, "s255");

        drop(database);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
