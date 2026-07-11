//! Resource-limited repository for user-authored timeline SQL.
//!
//! All custom SQL passes through this module. Callers cannot obtain the
//! underlying connection, disable the authorizer, or bypass the outer page and
//! payload limits.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use libsqlite3_sys::{
    sqlite3_set_authorizer, SQLITE_DENY, SQLITE_FUNCTION, SQLITE_OK, SQLITE_READ, SQLITE_RECURSIVE,
    SQLITE_SELECT,
};
use serde::Serialize;
use sqlx::{pool::PoolConnection, Sqlite, SqlitePool};
use tokio_util::sync::CancellationToken;

use crate::db::models::DbStatus;

pub const MAX_RESULT_ROWS: i64 = 120;
pub const MAX_SQL_BYTES: usize = 32 * 1024;
pub const MAX_RESULT_BYTES: usize = 2 * 1024 * 1024;
const PROGRESS_INTERVAL: i32 = 1_000;
const MIN_VM_OPERATIONS: u64 = 20_000_000;
const MAX_VM_OPERATIONS: u64 = 1_000_000_000;
const VM_OPERATIONS_PER_STATUS: u64 = 800;
const MIN_QUERY_DURATION: Duration = Duration::from_secs(10);
const MAX_QUERY_DURATION: Duration = Duration::from_secs(60);
const QUERY_DURATION_PER_100K_STATUSES: Duration = Duration::from_secs(5);

const READABLE_OBJECTS: &[&str] = &[
    "accounts",
    "notifications",
    "statuses",
    "status_tags",
    "status_viewer_state",
    "tags",
    "timeline_entries",
];
const FORBIDDEN_FUNCTIONS: &[&str] = &["load_extension", "readfile", "writefile"];

#[derive(Debug, thiserror::Error)]
pub enum CustomTimelineError {
    #[error("custom timeline SQL exceeds the {MAX_SQL_BYTES}-byte input limit")]
    SqlTooLarge,
    #[error("{0}")]
    Invalid(String),
    #[error("custom timeline SQL was rejected by the read sandbox")]
    Rejected(#[source] sqlx::Error),
    #[error("custom timeline SQL exceeded its execution budget; narrow the query")]
    ExecutionBudget,
    #[error("custom timeline SQL was cancelled")]
    Cancelled,
    #[error("custom timeline result exceeds the {MAX_RESULT_BYTES}-byte payload limit")]
    ResultTooLarge,
    #[error("custom timeline result could not be encoded")]
    Encoding(#[source] serde_json::Error),
    #[error("custom timeline connection could not be prepared")]
    Connection(#[source] sqlx::Error),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct QueryPlanStep {
    pub id: i64,
    pub parent: i64,
    pub detail: String,
}

#[derive(Debug, Clone, Copy)]
struct QueryBudget {
    max_vm_operations: u64,
    max_duration: Duration,
}

impl QueryBudget {
    fn for_status_count(status_count: u64) -> Self {
        let row_steps = status_count.div_ceil(100_000).max(1);
        let adaptive_duration = MIN_QUERY_DURATION.saturating_add(
            QUERY_DURATION_PER_100K_STATUSES.saturating_mul(row_steps.min(u32::MAX as u64) as u32),
        );
        Self {
            max_vm_operations: MIN_VM_OPERATIONS
                .saturating_add(status_count.saturating_mul(VM_OPERATIONS_PER_STATUS))
                .min(MAX_VM_OPERATIONS),
            max_duration: adaptive_duration.min(MAX_QUERY_DURATION),
        }
    }
}

struct SandboxedConnection {
    connection: PoolConnection<Sqlite>,
    release_to_pool: bool,
}

impl SandboxedConnection {
    fn new(connection: PoolConnection<Sqlite>) -> Self {
        Self {
            connection,
            release_to_pool: false,
        }
    }

    async fn install(
        &mut self,
        cancellation: CancellationToken,
        interrupted: Arc<AtomicBool>,
        budget: QueryBudget,
    ) -> Result<(), CustomTimelineError> {
        let executed_operations = Arc::new(AtomicU64::new(0));
        let deadline = Instant::now() + budget.max_duration;
        let mut handle = self
            .connection
            .lock_handle()
            .await
            .map_err(CustomTimelineError::Connection)?;
        // SAFETY: SQLx guarantees exclusive access while LockedSqliteHandle is
        // alive. The callback is a function pointer with no borrowed context.
        let result = unsafe {
            sqlite3_set_authorizer(
                handle.as_raw_handle().as_ptr(),
                Some(read_only_authorizer),
                std::ptr::null_mut(),
            )
        };
        if result != SQLITE_OK {
            return Err(CustomTimelineError::Invalid(
                "SQLite could not enable the custom timeline authorizer".to_string(),
            ));
        }
        handle.set_progress_handler(PROGRESS_INTERVAL, move || {
            let operations = executed_operations
                .fetch_add(PROGRESS_INTERVAL as u64, Ordering::Relaxed)
                + PROGRESS_INTERVAL as u64;
            let keep_running = operations <= budget.max_vm_operations
                && Instant::now() < deadline
                && !cancellation.is_cancelled();
            if !keep_running {
                interrupted.store(true, Ordering::Relaxed);
            }
            keep_running
        });
        Ok(())
    }

    async fn uninstall(&mut self) -> Result<(), CustomTimelineError> {
        let mut handle = self
            .connection
            .lock_handle()
            .await
            .map_err(CustomTimelineError::Connection)?;
        handle.remove_progress_handler();
        // SAFETY: the handle is exclusively locked by SQLx. Passing None
        // removes the callback before this connection can return to the pool.
        let result = unsafe {
            sqlite3_set_authorizer(handle.as_raw_handle().as_ptr(), None, std::ptr::null_mut())
        };
        if result != SQLITE_OK {
            return Err(CustomTimelineError::Invalid(
                "SQLite could not remove the custom timeline authorizer".to_string(),
            ));
        }
        self.release_to_pool = true;
        Ok(())
    }
}

impl Drop for SandboxedConnection {
    fn drop(&mut self) {
        if !self.release_to_pool {
            // Cancellation can drop an IPC future before async cleanup. Never
            // return an authorizer/progress callback to the shared reader pool.
            self.connection.close_on_drop();
        }
    }
}

unsafe extern "C" fn read_only_authorizer(
    _context: *mut c_void,
    action: c_int,
    parameter_one: *const c_char,
    parameter_two: *const c_char,
    _database: *const c_char,
    _trigger: *const c_char,
) -> c_int {
    match action {
        SQLITE_SELECT | SQLITE_RECURSIVE => SQLITE_OK,
        SQLITE_READ => {
            if c_string(parameter_one).is_some_and(|table| READABLE_OBJECTS.contains(&table)) {
                SQLITE_OK
            } else {
                SQLITE_DENY
            }
        }
        SQLITE_FUNCTION => c_string(parameter_two)
            .filter(|function| !FORBIDDEN_FUNCTIONS.contains(function))
            .map(|_| SQLITE_OK)
            .unwrap_or(SQLITE_DENY),
        _ => SQLITE_DENY,
    }
}

fn c_string(pointer: *const c_char) -> Option<&'static str> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: SQLite owns authorizer strings for the duration of the callback.
    // The returned value is consumed before the callback returns; the static
    // annotation cannot escape this private function's callers.
    unsafe { CStr::from_ptr(pointer) }.to_str().ok()
}

pub async fn query_statuses(
    pool: &SqlitePool,
    sql: &str,
    limit: i64,
    offset: i64,
    cancellation: &CancellationToken,
) -> Result<Vec<DbStatus>, CustomTimelineError> {
    let custom_sql = validate(sql)?;
    let has_user_limit = has_top_level_limit(&custom_sql);
    // Before the sandbox was introduced, a top-level LIMIT made a custom
    // timeline a single fixed page: the column's limit/offset did not rewrite
    // that query and later pages were empty. Keep that persisted-query
    // contract while applying an outer safety cap to the first page.
    if has_user_limit && offset > 0 {
        return Ok(Vec::new());
    }
    let page_limit = if has_user_limit {
        MAX_RESULT_ROWS
    } else {
        limit.clamp(0, MAX_RESULT_ROWS)
    };
    if page_limit == 0 {
        return Ok(Vec::new());
    }
    let query = if has_user_limit {
        format!("SELECT * FROM ({custom_sql}) custom_timeline_page LIMIT ?")
    } else {
        format!("SELECT * FROM ({custom_sql}) custom_timeline_page LIMIT ? OFFSET ?")
    };
    let budget = query_budget(pool).await;
    let interrupted = Arc::new(AtomicBool::new(false));
    let connection = pool
        .acquire()
        .await
        .map_err(CustomTimelineError::Connection)?;
    let mut connection = SandboxedConnection::new(connection);
    connection
        .install(cancellation.clone(), Arc::clone(&interrupted), budget)
        .await?;

    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(CustomTimelineError::Cancelled),
        result = async {
            let query = sqlx::query_as::<_, DbStatus>(&query).bind(page_limit);
            if has_user_limit {
                query.fetch_all(&mut *connection.connection).await
            } else {
                query
                    .bind(offset.max(0))
                    .fetch_all(&mut *connection.connection)
                    .await
            }
        } => {
                match result {
                    Ok(statuses) => Ok(statuses),
                    Err(_error) if interrupted.load(Ordering::Relaxed) => {
                        if cancellation.is_cancelled() {
                            Err(CustomTimelineError::Cancelled)
                        } else {
                            Err(CustomTimelineError::ExecutionBudget)
                        }
                    }
                    Err(error) => Err(CustomTimelineError::Rejected(error)),
                }
            }
    };

    connection.uninstall().await?;
    let statuses = result?;
    enforce_payload_limit(&statuses)?;
    Ok(statuses)
}

pub async fn explain(
    pool: &SqlitePool,
    sql: &str,
    cancellation: &CancellationToken,
) -> Result<Vec<QueryPlanStep>, CustomTimelineError> {
    let custom_sql = validate(sql)?;
    let query = format!("EXPLAIN QUERY PLAN SELECT * FROM ({custom_sql}) custom_timeline_page LIMIT {MAX_RESULT_ROWS}");
    let budget = query_budget(pool).await;
    let interrupted = Arc::new(AtomicBool::new(false));
    let connection = pool
        .acquire()
        .await
        .map_err(CustomTimelineError::Connection)?;
    let mut connection = SandboxedConnection::new(connection);
    connection
        .install(cancellation.clone(), Arc::clone(&interrupted), budget)
        .await?;
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(CustomTimelineError::Cancelled),
        result = sqlx::query_as::<_, QueryPlanStep>(&query).fetch_all(&mut *connection.connection) => {
            match result {
                Ok(plan) => Ok(plan),
                Err(_error) if interrupted.load(Ordering::Relaxed) => Err(CustomTimelineError::ExecutionBudget),
                Err(error) => Err(CustomTimelineError::Rejected(error)),
            }
        }
    };
    connection.uninstall().await?;
    let plan = result?;
    enforce_payload_limit(&plan)?;
    Ok(plan)
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
    QueryBudget::for_status_count(status_count.max(0) as u64)
}

fn enforce_payload_limit<T: Serialize>(value: &T) -> Result<(), CustomTimelineError> {
    let encoded = serde_json::to_vec(value).map_err(CustomTimelineError::Encoding)?;
    if encoded.len() > MAX_RESULT_BYTES {
        return Err(CustomTimelineError::ResultTooLarge);
    }
    Ok(())
}

pub fn validate(sql: &str) -> Result<String, CustomTimelineError> {
    if sql.len() > MAX_SQL_BYTES {
        return Err(CustomTimelineError::SqlTooLarge);
    }
    const FORBIDDEN_KEYWORDS: &[&str] = &[
        "alter",
        "analyze",
        "attach",
        "begin",
        "commit",
        "create",
        "delete",
        "detach",
        "drop",
        "insert",
        "pragma",
        "reindex",
        "release",
        "replace",
        "rollback",
        "savepoint",
        "transaction",
        "update",
        "vacuum",
    ];

    let bytes = sql.as_bytes();
    let mut index = 0usize;
    let mut first_keyword: Option<String> = None;
    let mut terminator: Option<usize> = None;

    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'-') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let comment_start = index;
            index += 2;
            let mut closed = false;
            while index + 1 < bytes.len() {
                if bytes[index] == b'*' && bytes[index + 1] == b'/' {
                    index += 2;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return Err(CustomTimelineError::Invalid(format!(
                    "Custom timeline SQL contains an unterminated comment at byte {comment_start}"
                )));
            }
            continue;
        }
        if terminator.is_some() {
            return Err(CustomTimelineError::Invalid(format!(
                "Custom timeline SQL must contain exactly one statement (extra input at byte {index})"
            )));
        }
        if matches!(bytes[index], b'\'' | b'"' | b'`') {
            let quote_start = index;
            let quote = bytes[index];
            index += 1;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return Err(CustomTimelineError::Invalid(format!(
                    "Custom timeline SQL contains an unterminated quoted value at byte {quote_start}"
                )));
            }
            continue;
        }
        if bytes[index] == b'[' {
            let quote_start = index;
            index += 1;
            while index < bytes.len() && bytes[index] != b']' {
                index += 1;
            }
            if index == bytes.len() {
                return Err(CustomTimelineError::Invalid(format!(
                    "Custom timeline SQL contains an unterminated identifier at byte {quote_start}"
                )));
            }
            index += 1;
            continue;
        }
        if bytes[index] == b';' {
            terminator = Some(index);
            index += 1;
            continue;
        }
        if is_identifier_byte(Some(bytes[index])) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_identifier_byte(Some(bytes[index])) {
                index += 1;
            }
            let token = sql[start..index].to_ascii_lowercase();
            first_keyword.get_or_insert_with(|| token.clone());
            if FORBIDDEN_KEYWORDS.contains(&token.as_str())
                || FORBIDDEN_FUNCTIONS.contains(&token.as_str())
                || token.starts_with("pragma_")
            {
                return Err(CustomTimelineError::Invalid(format!(
                    "Custom timeline SQL cannot use `{token}` at byte {start}"
                )));
            }
            continue;
        }
        index += 1;
    }

    if first_keyword.as_deref() != Some("select") {
        return Err(CustomTimelineError::Invalid(
            "Custom timeline SQL must start with SELECT".to_string(),
        ));
    }
    let statement = terminator
        .map(|position| &sql[..position])
        .unwrap_or(sql)
        .trim();
    if statement.is_empty() {
        return Err(CustomTimelineError::Invalid(
            "Custom timeline SQL must not be empty".to_string(),
        ));
    }
    Ok(statement.to_string())
}

fn has_top_level_limit(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();

        if byte == b'-' && next == Some(b'-') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if byte == b'/' && next == Some(b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if matches!(byte, b'\'' | b'"' | b'`') {
            let quote = byte;
            index += 1;
            while index < bytes.len() {
                if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if byte == b'[' {
            index += 1;
            while index < bytes.len() && bytes[index] != b']' {
                index += 1;
            }
            index = (index + 1).min(bytes.len());
            continue;
        }

        match byte {
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ if depth == 0 && keyword_at(bytes, index, b"limit") => return true,
            _ => index += 1,
        }
    }

    false
}

fn keyword_at(bytes: &[u8], index: usize, keyword: &[u8]) -> bool {
    let end = index + keyword.len();
    if end > bytes.len()
        || bytes[index..end]
            .iter()
            .zip(keyword.iter())
            .any(|(actual, expected)| actual.to_ascii_lowercase() != *expected)
    {
        return false;
    }
    !is_identifier_byte(
        index
            .checked_sub(1)
            .and_then(|previous| bytes.get(previous))
            .copied(),
    ) && !is_identifier_byte(bytes.get(end).copied())
}

fn is_identifier_byte(byte: Option<u8>) -> bool {
    matches!(
        byte,
        Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_one_bounded_select_without_echoing_the_query() {
        assert!(validate("SELECT * FROM statuses").is_ok());
        assert!(matches!(
            validate(&"x".repeat(MAX_SQL_BYTES + 1)),
            Err(CustomTimelineError::SqlTooLarge)
        ));
        for sql in [
            "PRAGMA table_info(statuses)",
            "SELECT load_extension('untrusted')",
            "SELECT * FROM statuses; DELETE FROM statuses",
        ] {
            assert!(validate(sql).is_err(), "validator accepted {sql}");
        }
    }

    #[tokio::test]
    async fn authorizer_caps_pages_and_returns_a_typed_plan() {
        let pool = setup_pool().await;
        for index in 0..150 {
            insert_status(&pool, &format!("status-{index:03}"), "post").await;
        }

        let cancellation = CancellationToken::new();
        let rows = query_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY id",
            1_000,
            0,
            &cancellation,
        )
        .await
        .expect("bounded query");
        assert_eq!(rows.len(), MAX_RESULT_ROWS as usize);

        let second = query_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY id",
            1,
            1,
            &cancellation,
        )
        .await
        .expect("outer pagination");
        assert_eq!(second[0].id, "status-001");

        let plan = explain(
            &pool,
            "SELECT * FROM statuses WHERE id = 'status-001'",
            &cancellation,
        )
        .await
        .expect("typed plan");
        assert!(!plan.is_empty());
        assert!(plan.iter().all(|step| !step.detail.is_empty()));

        sqlx::query(
            "CREATE TABLE status_viewer_state (
                status_id TEXT NOT NULL,
                server_domain TEXT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .expect("viewer state fixture");
        sqlx::query(
            "INSERT INTO status_viewer_state(status_id, server_domain)
             VALUES ('status-001', 'example.test')",
        )
        .execute(&pool)
        .await
        .expect("viewer state row");
        let viewer_state_rows = query_statuses(
            &pool,
            "SELECT s.* FROM statuses s
             WHERE EXISTS (
                 SELECT 1 FROM status_viewer_state v
                 WHERE v.status_id = s.id AND v.server_domain = s.server_domain
             )",
            10,
            0,
            &cancellation,
        )
        .await
        .expect("real singular viewer-state table is readable");
        assert_eq!(viewer_state_rows.len(), 1);
        assert_eq!(viewer_state_rows[0].id, "status-001");
    }

    #[tokio::test]
    async fn log_sql_shapes_complete_on_a_large_cache_fixture() {
        let pool = setup_pool().await;
        const STATUS_COUNT: i64 = 30_000;
        sqlx::query(&format!(
            "WITH RECURSIVE seq(value) AS (
                 SELECT 1
                 UNION ALL
                 SELECT value + 1 FROM seq WHERE value < {STATUS_COUNT}
             )
             INSERT INTO statuses (
                 id, server_domain, uri, created_at, account_id, content,
                 media_attachments_json
             )
             SELECT
                 printf('bulk-%05d', value),
                 'example.test',
                 printf('https://example.test/statuses/bulk-%05d', value),
                 printf('%05d', value),
                 'author',
                 CASE
                     WHEN value = {STATUS_COUNT} THEN '<p>FF14 ルレ</p>'
                     WHEN value = {STATUS_COUNT} - 1 THEN '<p>エンドフィールド</p>'
                     ELSE '<p>ordinary cached post</p>'
                 END,
                 CASE WHEN value % 300 = 0 THEN '[]' ELSE NULL END
             FROM seq"
        ))
        .execute(&pool)
        .await
        .expect("large status fixture");

        let cancellation = CancellationToken::new();
        let sql_cases = [
            "SELECT * FROM statuses WHERE media_attachments_json IS NOT NULL GROUP BY uri ORDER BY created_at DESC LIMIT 100;",
            "SELECT * FROM statuses WHERE (
               content LIKE \"%FF14%\" OR content LIKE \"%FFXIV%\" OR content LIKE \"%かみげー%\" OR content LIKE \"%ルレ%\"
             ) AND (
               content NOT LIKE \"%アズールレーン%\" AND
               content NOT LIKE \"%アルレッキーノ%\" AND
               content NOT LIKE \"%エルレイド%\" AND
               content NOT LIKE \"%スキルレベル%\" AND
               content NOT LIKE \"%ムルゴルレジル%\"
             ) GROUP BY uri ORDER BY created_at DESC LIMIT 150;",
            "SELECT * FROM statuses WHERE content LIKE \"%エンドフィールド%\" OR content LIKE \"%ンィー%\" GROUP BY uri ORDER BY created_at DESC LIMIT 150;",
        ];

        for sql in sql_cases {
            let rows = query_statuses(&pool, sql, 100, 0, &cancellation)
                .await
                .unwrap_or_else(|error| panic!("log query must remain usable: {error}"));
            assert!(!rows.is_empty(), "expected a match for {sql}");
            assert!(rows.len() <= MAX_RESULT_ROWS as usize);
        }

        // A persisted top-level LIMIT is a fixed first page, matching the
        // pre-sandbox behavior instead of being reinterpreted as an offset.
        let next_page = query_statuses(&pool, sql_cases[0], 100, 1, &cancellation)
            .await
            .expect("fixed query next page");
        assert!(next_page.is_empty());
    }

    #[tokio::test]
    async fn rejects_unlisted_objects_and_does_not_leak_the_policy_to_the_pool() {
        let pool = setup_pool().await;
        let error = query_statuses(
            &pool,
            "SELECT * FROM sqlite_master",
            10,
            0,
            &CancellationToken::new(),
        )
        .await
        .expect_err("sqlite_master must be denied");
        assert!(matches!(error, CustomTimelineError::Rejected(_)));

        // The rejected connection was cleaned or discarded. Ordinary writer
        // work on a pooled connection is not affected by the sandbox policy.
        insert_status(&pool, "after-rejection", "post").await;
    }

    #[tokio::test]
    async fn enforces_payload_and_cancellation_budgets() {
        let pool = setup_pool().await;
        insert_status(&pool, "large", &"x".repeat(MAX_RESULT_BYTES + 1)).await;
        let error = query_statuses(
            &pool,
            "SELECT * FROM statuses",
            10,
            0,
            &CancellationToken::new(),
        )
        .await
        .expect_err("oversized IPC payload must fail");
        assert!(matches!(error, CustomTimelineError::ResultTooLarge));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = query_statuses(&pool, "SELECT * FROM statuses", 10, 0, &cancellation)
            .await
            .expect_err("cancelled query must fail");
        assert!(matches!(error, CustomTimelineError::Cancelled));
    }

    async fn setup_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory SQLite");
        for migration in [
            include_str!("../../../migrations/001_create_servers.sql"),
            include_str!("../../../migrations/002_create_accounts.sql"),
            include_str!("../../../migrations/003_create_statuses.sql"),
            include_str!("../../../migrations/012_add_status_quote_id.sql"),
            include_str!("../../../migrations/019_add_status_application.sql"),
        ] {
            sqlx::raw_sql(migration)
                .execute(&pool)
                .await
                .expect("migration");
        }
        sqlx::query(
            "INSERT INTO servers (domain, streaming_url) VALUES ('example.test', 'wss://example.test')",
        )
        .execute(&pool)
        .await
        .expect("server");
        sqlx::query(
            "INSERT INTO accounts (id, server_domain, username, acct, display_name, created_at)
             VALUES ('author', 'example.test', 'author', 'author@example.test', 'Author', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .expect("account");
        pool
    }

    async fn insert_status(pool: &SqlitePool, id: &str, content: &str) {
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES (?, 'example.test', ?, '2026-01-01', 'author', ?)",
        )
        .bind(id)
        .bind(format!("https://example.test/statuses/{id}"))
        .bind(content)
        .execute(pool)
        .await
        .expect("status");
    }
}
