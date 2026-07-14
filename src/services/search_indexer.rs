//! Low-priority asynchronous ICU search index maintenance.
//!
//! The portable SQLite queues are authoritative. Status and account
//! transactions only coalesce one key into their respective queue. This worker
//! probes the writer with `try_acquire()`, releases it during ICU4X
//! segmentation, and uses `try_acquire()` again before committing, so it never
//! joins the interactive writer's wait queue.

use std::time::{Duration, Instant};

use serde::Serialize;
use sqlx::{Acquire, FromRow, SqliteConnection, SqlitePool};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::db::icu_search;

// SQLite cannot preempt a transaction after this background worker acquires
// the sole writer. Keep those transactions deliberately smaller than normal
// foreground page batches so a newly arriving post/save waits only for a
// micro-chunk of FTS work.
const QUEUE_CHUNK_SIZE: i64 = 8;
const BACKFILL_CHUNK_SIZE: i64 = 32;
const BUSY_RETRY_DELAY: Duration = Duration::from_millis(250);
const IDLE_POLL_DELAY: Duration = Duration::from_secs(1);
const MIN_YIELD_DELAY: Duration = Duration::from_millis(5);
const MERGE_INTERVAL: Duration = Duration::from_secs(1);
const MERGE_PAGE_BUDGET: i64 = 8;
const MERGE_ATTEMPTS_PER_INDEX_COMMIT: i64 = 8;
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_BUSY_TIMEOUT_MS: i64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexProgress {
    pub processed_count: u64,
    pub total_count: u64,
    pub completed: bool,
    pub pending_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexStep {
    WriterBusy,
    Queue { processed: usize },
    Backfill(SearchIndexProgress),
    Idle(SearchIndexProgress),
}

#[derive(Debug, FromRow)]
struct StatusQueueRow {
    status_id: String,
    server_domain: String,
    action: String,
    generation: Vec<u8>,
    content: Option<String>,
    spoiler_text: Option<String>,
    uri: Option<String>,
    url: Option<String>,
    tags_json: Option<String>,
}

#[derive(Debug)]
struct PreparedStatusQueueRow {
    status_id: String,
    server_domain: String,
    action: String,
    generation: Vec<u8>,
    token_text: Option<String>,
}

#[derive(Debug, FromRow)]
struct StatusBackfillRow {
    status_id: String,
    server_domain: String,
    content: String,
    spoiler_text: String,
    uri: String,
    url: Option<String>,
    tags_json: Option<String>,
}

#[derive(Debug)]
struct PreparedStatusBackfillRow {
    status_id: String,
    server_domain: String,
    content: String,
    spoiler_text: String,
    uri: String,
    url: Option<String>,
    tags_json: Option<String>,
    token_text: String,
}

struct BackfillCommit {
    state_updated: bool,
    processed_count: i64,
    total_count: i64,
    completed: bool,
}

#[derive(Debug, FromRow)]
struct StatusBackfillState {
    cursor_status_id: Option<String>,
    cursor_server_domain: Option<String>,
    processed_count: i64,
    total_count: i64,
    completed: bool,
}

#[derive(Debug, FromRow)]
struct AccountQueueRow {
    account_id: String,
    server_domain: String,
    action: String,
    generation: Vec<u8>,
    acct: Option<String>,
    display_name: Option<String>,
}

#[derive(Debug)]
struct PreparedAccountQueueRow {
    account_id: String,
    server_domain: String,
    action: String,
    generation: Vec<u8>,
    token_text: Option<String>,
}

#[derive(Debug, FromRow)]
struct AccountBackfillRow {
    account_id: String,
    server_domain: String,
    acct: String,
    display_name: String,
}

#[derive(Debug)]
struct PreparedAccountBackfillRow {
    account_id: String,
    server_domain: String,
    acct: String,
    display_name: String,
    token_text: String,
}

#[derive(Debug, FromRow)]
struct AccountBackfillState {
    cursor_account_id: Option<String>,
    cursor_server_domain: Option<String>,
    processed_count: i64,
    total_count: i64,
    completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeTarget {
    Status,
    Account,
}

impl MergeTarget {
    fn other(self) -> Self {
        match self {
            Self::Status => Self::Account,
            Self::Account => Self::Status,
        }
    }
}

#[derive(Debug, FromRow)]
struct MergeDebt {
    merge_debt: i64,
    account_merge_debt: i64,
}

#[cfg(test)]
pub async fn is_complete(reader: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT status.completed AND account.completed
           FROM status_search_icu_backfill_state status
           JOIN account_search_icu_backfill_state account
             ON account.singleton = status.singleton
          WHERE status.singleton = 1",
    )
    .fetch_one(reader)
    .await
}

pub async fn pending_count(reader: &SqlitePool) -> Result<u64, sqlx::Error> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT (SELECT COUNT(*) FROM status_search_index_queue)
              + (SELECT COUNT(*) FROM account_search_index_queue)",
    )
    .fetch_one(reader)
    .await?;
    Ok(non_negative_u64(count))
}

pub async fn run(
    writer: &SqlitePool,
    reader: &SqlitePool,
    cancellation: CancellationToken,
    progress_tx: Option<&mpsc::Sender<SearchIndexProgress>>,
) -> Result<(), sqlx::Error> {
    let mut last_merge = Instant::now();
    let mut next_merge_target = MergeTarget::Account;
    let mut last_progress_emit = Instant::now()
        .checked_sub(PROGRESS_EMIT_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut last_emitted_progress: Option<SearchIndexProgress> = None;
    loop {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        let started_at = Instant::now();
        let step = run_index_step(writer, reader).await?;
        let mut merge_attempted = false;
        let mut merge_writer_busy = false;
        // Automatic merges are disabled so they can never surprise an
        // interactive status transaction. Pay down a bounded number of pages
        // between backfill chunks as well as after completion; otherwise a
        // large portable database would accumulate tens of thousands of tiny
        // level-zero segments before its first merge.
        if matches!(&step, IndexStep::Backfill(_) | IndexStep::Idle(_))
            && last_merge.elapsed() >= MERGE_INTERVAL
            && pending_count(reader).await? == 0
        {
            let debt = load_merge_debt(reader).await?;
            if let Some(target) = select_merge_target(&debt, next_merge_target) {
                // Alternate after every attempt, including a target-specific
                // error, so debt on one FTS index cannot starve the other.
                next_merge_target = target.other();
                match try_bounded_merge(writer, target).await {
                    Ok(true) => {
                        merge_attempted = true;
                        last_merge = Instant::now();
                    }
                    Ok(false) => merge_writer_busy = true,
                    Err(error) => {
                        tracing::warn!(?target, %error, "Deferred ICU search index merge failed");
                        // Avoid a hot retry loop if the FTS command itself fails.
                        last_merge = Instant::now();
                    }
                }
            }
        }
        // Include both indexing and bounded merge time in the duty-cycle
        // yield. The worker spends at most roughly one quarter of sustained
        // wall time doing background CPU/write work.
        let delay = match &step {
            IndexStep::WriterBusy => BUSY_RETRY_DELAY.max(started_at.elapsed().saturating_mul(3)),
            _ if merge_writer_busy => BUSY_RETRY_DELAY.max(started_at.elapsed().saturating_mul(3)),
            IndexStep::Queue { .. } | IndexStep::Backfill(_) => {
                started_at.elapsed().saturating_mul(3).max(MIN_YIELD_DELAY)
            }
            IndexStep::Idle(_) if merge_attempted => {
                started_at.elapsed().saturating_mul(3).max(MIN_YIELD_DELAY)
            }
            IndexStep::Idle(_) => IDLE_POLL_DELAY,
        };
        if let Some(progress_tx) = progress_tx {
            if let IndexStep::Backfill(progress) | IndexStep::Idle(progress) = &step {
                let changed = last_emitted_progress.as_ref() != Some(progress);
                let completion_transition = progress.completed
                    && last_emitted_progress
                        .as_ref()
                        .is_none_or(|previous| !previous.completed);
                if changed
                    && (completion_transition
                        || last_progress_emit.elapsed() >= PROGRESS_EMIT_INTERVAL)
                    && progress_tx.try_send(progress.clone()).is_ok()
                {
                    last_emitted_progress = Some(progress.clone());
                    last_progress_emit = Instant::now();
                }
            }
        }
        tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

pub async fn run_index_step(
    writer: &SqlitePool,
    reader: &SqlitePool,
) -> Result<IndexStep, sqlx::Error> {
    // Live status updates are always the highest-priority background work.
    let queue = load_status_queue_batch(reader).await?;
    if !queue.is_empty() {
        // Avoid spending CPU on ICU segmentation while an interactive write
        // already owns (or is queued for) the sole local writer connection.
        let Some(connection) = writer.try_acquire() else {
            return Ok(IndexStep::WriterBusy);
        };
        drop(connection);
        let prepared = tokio::task::spawn_blocking(move || {
            queue
                .into_iter()
                .map(prepare_status_queue_row)
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            sqlx::Error::Protocol(format!("ICU status queue worker failed: {error}"))
        })?;
        let Some(mut connection) = writer.try_acquire() else {
            return Ok(IndexStep::WriterBusy);
        };
        enter_low_priority_write(&mut connection).await?;
        let write_result = commit_prepared_status_queue(&mut connection, &prepared).await;
        if let Err(error) = finish_low_priority_write(&mut connection, write_result).await {
            if sqlite_write_is_busy(&error) {
                return Ok(IndexStep::WriterBusy);
            }
            return Err(error);
        }
        return Ok(IndexStep::Queue {
            processed: prepared.len(),
        });
    }

    // Account live updates are second so account discovery remains current
    // without delaying newly-arrived statuses.
    let account_queue = load_account_queue_batch(reader).await?;
    if !account_queue.is_empty() {
        let Some(connection) = writer.try_acquire() else {
            return Ok(IndexStep::WriterBusy);
        };
        drop(connection);
        let prepared = tokio::task::spawn_blocking(move || {
            account_queue
                .into_iter()
                .map(prepare_account_queue_row)
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            sqlx::Error::Protocol(format!("ICU account queue worker failed: {error}"))
        })?;
        let Some(mut connection) = writer.try_acquire() else {
            return Ok(IndexStep::WriterBusy);
        };
        enter_low_priority_write(&mut connection).await?;
        let write_result = commit_prepared_account_queue(&mut connection, &prepared).await;
        let committed = match finish_low_priority_write(&mut connection, write_result).await {
            Ok(committed) => committed,
            Err(error) if sqlite_write_is_busy(&error) => return Ok(IndexStep::WriterBusy),
            Err(error) => return Err(error),
        };
        return Ok(IndexStep::Queue {
            processed: if committed { prepared.len() } else { 0 },
        });
    }

    // Backfill accounts before statuses. Accounts are far fewer and make the
    // indexed author branch useful quickly on an upgraded portable database.
    let account_state = load_account_backfill_state(reader).await?;
    if !account_state.completed {
        let rows = load_account_backfill_batch(reader, &account_state).await?;
        let Some(connection) = writer.try_acquire() else {
            return Ok(IndexStep::WriterBusy);
        };
        drop(connection);
        let prepared = tokio::task::spawn_blocking(move || {
            rows.into_iter()
                .map(|row| {
                    let token_text =
                        icu_search::index_text([row.acct.as_str(), row.display_name.as_str()]);
                    PreparedAccountBackfillRow {
                        account_id: row.account_id,
                        server_domain: row.server_domain,
                        acct: row.acct,
                        display_name: row.display_name,
                        token_text,
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|error| {
            sqlx::Error::Protocol(format!("ICU account backfill worker failed: {error}"))
        })?;
        let Some(mut connection) = writer.try_acquire() else {
            return Ok(IndexStep::WriterBusy);
        };
        enter_low_priority_write(&mut connection).await?;
        let write_result =
            commit_prepared_account_backfill(&mut connection, &account_state, &prepared).await;
        let commit = match finish_low_priority_write(&mut connection, write_result).await {
            Ok(commit) => commit,
            Err(error) if sqlite_write_is_busy(&error) => return Ok(IndexStep::WriterBusy),
            Err(error) => return Err(error),
        };
        let account_state = if commit.state_updated {
            let last_key = prepared.last();
            AccountBackfillState {
                cursor_account_id: last_key
                    .map(|row| row.account_id.clone())
                    .or(account_state.cursor_account_id),
                cursor_server_domain: last_key
                    .map(|row| row.server_domain.clone())
                    .or(account_state.cursor_server_domain),
                processed_count: if commit.completed {
                    commit.total_count
                } else {
                    commit.processed_count
                },
                total_count: commit.total_count,
                completed: commit.completed,
            }
        } else {
            // Another process, a bulk reset, or newly queued live work made
            // this reader snapshot stale. Never regress the durable cursor.
            load_account_backfill_state(reader).await?
        };
        let status_state = load_status_backfill_state(reader).await?;
        return Ok(IndexStep::Backfill(
            progress(reader, &status_state, &account_state).await?,
        ));
    }

    let status_state = load_status_backfill_state(reader).await?;
    if status_state.completed {
        return Ok(IndexStep::Idle(
            progress(reader, &status_state, &account_state).await?,
        ));
    }
    let rows = load_status_backfill_batch(reader, &status_state).await?;
    let Some(connection) = writer.try_acquire() else {
        return Ok(IndexStep::WriterBusy);
    };
    drop(connection);
    let prepared = tokio::task::spawn_blocking(move || {
        rows.into_iter()
            .map(|row| {
                let token_text = icu_search::index_text([
                    row.content.as_str(),
                    row.spoiler_text.as_str(),
                    row.uri.as_str(),
                    row.url.as_deref().unwrap_or_default(),
                    row.tags_json.as_deref().unwrap_or_default(),
                ]);
                PreparedStatusBackfillRow {
                    status_id: row.status_id,
                    server_domain: row.server_domain,
                    content: row.content,
                    spoiler_text: row.spoiler_text,
                    uri: row.uri,
                    url: row.url,
                    tags_json: row.tags_json,
                    token_text,
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|error| {
        sqlx::Error::Protocol(format!("ICU status backfill worker failed: {error}"))
    })?;
    let Some(mut connection) = writer.try_acquire() else {
        return Ok(IndexStep::WriterBusy);
    };
    enter_low_priority_write(&mut connection).await?;
    let write_result =
        commit_prepared_status_backfill(&mut connection, &status_state, &prepared).await;
    let commit = match finish_low_priority_write(&mut connection, write_result).await {
        Ok(commit) => commit,
        Err(error) if sqlite_write_is_busy(&error) => return Ok(IndexStep::WriterBusy),
        Err(error) => return Err(error),
    };
    let status_state = if commit.state_updated {
        let last_key = prepared.last();
        StatusBackfillState {
            cursor_status_id: last_key
                .map(|row| row.status_id.clone())
                .or(status_state.cursor_status_id),
            cursor_server_domain: last_key
                .map(|row| row.server_domain.clone())
                .or(status_state.cursor_server_domain),
            processed_count: if commit.completed {
                commit.total_count
            } else {
                commit.processed_count
            },
            total_count: commit.total_count,
            completed: commit.completed,
        }
    } else {
        // Another app process (or a bulk cache reset) advanced the durable
        // cursor while this batch was being segmented. Its state is newer;
        // never regress it to this worker's stale reader snapshot.
        load_status_backfill_state(reader).await?
    };
    Ok(IndexStep::Backfill(
        progress(reader, &status_state, &account_state).await?,
    ))
}

async fn load_status_queue_batch(reader: &SqlitePool) -> Result<Vec<StatusQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, StatusQueueRow>(
        "SELECT q.status_id,
                q.server_domain,
                q.action,
                q.generation,
                s.content,
                s.spoiler_text,
                s.uri,
                s.url,
                s.tags_json
           FROM status_search_index_queue q
           LEFT JOIN statuses s
             ON s.id = q.status_id AND s.server_domain = q.server_domain
          ORDER BY q.queued_at, q.status_id, q.server_domain
          LIMIT ?",
    )
    .bind(QUEUE_CHUNK_SIZE)
    .fetch_all(reader)
    .await
}

fn prepare_status_queue_row(row: StatusQueueRow) -> PreparedStatusQueueRow {
    let token_text = (row.action == "upsert")
        .then(|| {
            Some(icu_search::index_text([
                row.content.as_deref()?,
                row.spoiler_text.as_deref().unwrap_or_default(),
                row.uri.as_deref()?,
                row.url.as_deref().unwrap_or_default(),
                row.tags_json.as_deref().unwrap_or_default(),
            ]))
        })
        .flatten();
    PreparedStatusQueueRow {
        status_id: row.status_id,
        server_domain: row.server_domain,
        action: row.action,
        generation: row.generation,
        token_text,
    }
}

async fn load_account_queue_batch(
    reader: &SqlitePool,
) -> Result<Vec<AccountQueueRow>, sqlx::Error> {
    sqlx::query_as::<_, AccountQueueRow>(
        "SELECT q.account_id,
                q.server_domain,
                q.action,
                q.generation,
                a.acct,
                a.display_name
           FROM account_search_index_queue q
           LEFT JOIN accounts a
             ON a.id = q.account_id AND a.server_domain = q.server_domain
          ORDER BY q.queued_at, q.account_id, q.server_domain
          LIMIT ?",
    )
    .bind(QUEUE_CHUNK_SIZE)
    .fetch_all(reader)
    .await
}

fn prepare_account_queue_row(row: AccountQueueRow) -> PreparedAccountQueueRow {
    let token_text = (row.action == "upsert")
        .then(|| {
            Some(icu_search::index_text([
                row.acct.as_deref()?,
                row.display_name.as_deref().unwrap_or_default(),
            ]))
        })
        .flatten();
    PreparedAccountQueueRow {
        account_id: row.account_id,
        server_domain: row.server_domain,
        action: row.action,
        generation: row.generation,
        token_text,
    }
}

async fn commit_prepared_status_queue(
    connection: &mut SqliteConnection,
    prepared: &[PreparedStatusQueueRow],
) -> Result<(), sqlx::Error> {
    let mut transaction = connection.begin().await?;
    let mut changed = false;
    for row in prepared {
        changed |= apply_prepared_status_row(&mut transaction, row).await?;
    }
    if changed {
        record_merge_debt(&mut transaction, MergeTarget::Status).await?;
    }
    transaction.commit().await
}

async fn commit_prepared_account_queue(
    connection: &mut SqliteConnection,
    prepared: &[PreparedAccountQueueRow],
) -> Result<bool, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    if status_queue_has_work(&mut transaction).await? {
        transaction.rollback().await?;
        return Ok(false);
    }
    let mut changed = false;
    for row in prepared {
        changed |= apply_prepared_account_row(&mut transaction, row).await?;
    }
    if changed {
        record_merge_debt(&mut transaction, MergeTarget::Account).await?;
    }
    transaction.commit().await?;
    Ok(true)
}

async fn commit_prepared_status_backfill(
    connection: &mut SqliteConnection,
    state: &StatusBackfillState,
    prepared: &[PreparedStatusBackfillRow],
) -> Result<BackfillCommit, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    // A live queue that appeared during CPU segmentation, or an account
    // backfill reset by another process, takes precedence over status history.
    let higher_priority_work = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM status_search_index_queue)
             OR EXISTS(SELECT 1 FROM account_search_index_queue)
             OR EXISTS(
                    SELECT 1
                      FROM account_search_icu_backfill_state
                     WHERE singleton = 1 AND completed = 0
                )",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if higher_priority_work {
        transaction.rollback().await?;
        return Ok(stale_backfill_commit(
            state.processed_count,
            state.total_count,
        ));
    }
    for row in prepared {
        upsert_status_backfill_content_if_current(&mut transaction, row).await?;
    }
    if !prepared.is_empty() {
        record_merge_debt(&mut transaction, MergeTarget::Status).await?;
    }
    let completed = prepared.len() < BACKFILL_CHUNK_SIZE as usize;
    let processed_count = state
        .processed_count
        .saturating_add(i64::try_from(prepared.len()).unwrap_or(i64::MAX));
    let observed_total =
        sqlx::query_scalar::<_, i64>("SELECT value FROM cache_counters WHERE name = 'statuses'")
            .fetch_one(&mut *transaction)
            .await?;
    let total_count = if completed {
        observed_total
    } else {
        state.total_count.max(observed_total).max(processed_count)
    };
    let last_key = prepared.last();
    let state_update = sqlx::query(
        "UPDATE status_search_icu_backfill_state
            SET cursor_status_id = coalesce(?, cursor_status_id),
                cursor_server_domain = coalesce(?, cursor_server_domain),
                processed_count = ?,
                total_count = ?,
                completed = ?,
                updated_at = datetime('now')
          WHERE singleton = 1
            AND completed = 0
            AND cursor_status_id IS ?
            AND cursor_server_domain IS ?
            AND processed_count = ?",
    )
    .bind(last_key.map(|row| row.status_id.as_str()))
    .bind(last_key.map(|row| row.server_domain.as_str()))
    .bind(
        if completed {
            total_count
        } else {
            processed_count
        }
        .max(0),
    )
    .bind(total_count.max(0))
    .bind(completed)
    .bind(state.cursor_status_id.as_deref())
    .bind(state.cursor_server_domain.as_deref())
    .bind(state.processed_count)
    .execute(&mut *transaction)
    .await?;
    if state_update.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok(BackfillCommit {
            state_updated: false,
            processed_count,
            total_count,
            completed,
        });
    }
    transaction.commit().await?;
    Ok(BackfillCommit {
        state_updated: true,
        processed_count,
        total_count,
        completed,
    })
}

async fn commit_prepared_account_backfill(
    connection: &mut SqliteConnection,
    state: &AccountBackfillState,
    prepared: &[PreparedAccountBackfillRow],
) -> Result<BackfillCommit, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    let live_work = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM status_search_index_queue)
             OR EXISTS(SELECT 1 FROM account_search_index_queue)",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if live_work {
        transaction.rollback().await?;
        return Ok(stale_backfill_commit(
            state.processed_count,
            state.total_count,
        ));
    }
    for row in prepared {
        upsert_account_backfill_content_if_current(&mut transaction, row).await?;
    }
    if !prepared.is_empty() {
        record_merge_debt(&mut transaction, MergeTarget::Account).await?;
    }
    let completed = prepared.len() < BACKFILL_CHUNK_SIZE as usize;
    let processed_count = state
        .processed_count
        .saturating_add(i64::try_from(prepared.len()).unwrap_or(i64::MAX));
    let observed_total =
        sqlx::query_scalar::<_, i64>("SELECT value FROM cache_counters WHERE name = 'accounts'")
            .fetch_one(&mut *transaction)
            .await?;
    let total_count = if completed {
        observed_total
    } else {
        state.total_count.max(observed_total).max(processed_count)
    };
    let last_key = prepared.last();
    let state_update = sqlx::query(
        "UPDATE account_search_icu_backfill_state
            SET cursor_account_id = coalesce(?, cursor_account_id),
                cursor_server_domain = coalesce(?, cursor_server_domain),
                processed_count = ?,
                total_count = ?,
                completed = ?,
                updated_at = datetime('now')
          WHERE singleton = 1
            AND completed = 0
            AND cursor_account_id IS ?
            AND cursor_server_domain IS ?
            AND processed_count = ?",
    )
    .bind(last_key.map(|row| row.account_id.as_str()))
    .bind(last_key.map(|row| row.server_domain.as_str()))
    .bind(
        if completed {
            total_count
        } else {
            processed_count
        }
        .max(0),
    )
    .bind(total_count.max(0))
    .bind(completed)
    .bind(state.cursor_account_id.as_deref())
    .bind(state.cursor_server_domain.as_deref())
    .bind(state.processed_count)
    .execute(&mut *transaction)
    .await?;
    if state_update.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok(BackfillCommit {
            state_updated: false,
            processed_count,
            total_count,
            completed,
        });
    }
    transaction.commit().await?;
    Ok(BackfillCommit {
        state_updated: true,
        processed_count,
        total_count,
        completed,
    })
}

fn stale_backfill_commit(processed_count: i64, total_count: i64) -> BackfillCommit {
    BackfillCommit {
        state_updated: false,
        processed_count,
        total_count,
        completed: false,
    }
}

async fn status_queue_has_work(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM status_search_index_queue)")
        .fetch_one(&mut **transaction)
        .await
}

async fn apply_prepared_status_row(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: &PreparedStatusQueueRow,
) -> Result<bool, sqlx::Error> {
    // The reader snapshot used to build this row can race with a newer status
    // update before the worker acquires the writer. Never apply stale tokens;
    // leave the newer queue generation for the next low-priority step.
    let still_current = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1
               FROM status_search_index_queue
              WHERE status_id = ?
                AND server_domain = ?
                AND action = ?
                AND generation = ?
         )",
    )
    .bind(&row.status_id)
    .bind(&row.server_domain)
    .bind(&row.action)
    .bind(&row.generation)
    .fetch_one(&mut **transaction)
    .await?;
    if !still_current {
        return Ok(false);
    }

    if let Some(token_text) = row.token_text.as_deref() {
        upsert_status_index_content(transaction, &row.status_id, &row.server_domain, token_text)
            .await?;
    } else {
        sqlx::query(
            "DELETE FROM status_search_icu_content
              WHERE status_id = ? AND server_domain = ?",
        )
        .bind(&row.status_id)
        .bind(&row.server_domain)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "DELETE FROM status_search_index_queue
          WHERE status_id = ? AND server_domain = ? AND generation = ?",
    )
    .bind(&row.status_id)
    .bind(&row.server_domain)
    .bind(&row.generation)
    .execute(&mut **transaction)
    .await?;
    Ok(true)
}

async fn apply_prepared_account_row(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: &PreparedAccountQueueRow,
) -> Result<bool, sqlx::Error> {
    // Generation validation prevents a stale reader snapshot from replacing a
    // newer account name/acct value or resurrecting a deleted account.
    let still_current = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1
               FROM account_search_index_queue
              WHERE account_id = ?
                AND server_domain = ?
                AND action = ?
                AND generation = ?
         )",
    )
    .bind(&row.account_id)
    .bind(&row.server_domain)
    .bind(&row.action)
    .bind(&row.generation)
    .fetch_one(&mut **transaction)
    .await?;
    if !still_current {
        return Ok(false);
    }

    if let Some(token_text) = row.token_text.as_deref() {
        sqlx::query(
            "INSERT INTO account_search_icu_content(account_id, server_domain, token_text)
             VALUES (?, ?, ?)
             ON CONFLICT(account_id, server_domain) DO UPDATE SET
                 token_text = excluded.token_text",
        )
        .bind(&row.account_id)
        .bind(&row.server_domain)
        .bind(token_text)
        .execute(&mut **transaction)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM account_search_icu_content
              WHERE account_id = ? AND server_domain = ?",
        )
        .bind(&row.account_id)
        .bind(&row.server_domain)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "DELETE FROM account_search_index_queue
          WHERE account_id = ? AND server_domain = ? AND generation = ?",
    )
    .bind(&row.account_id)
    .bind(&row.server_domain)
    .bind(&row.generation)
    .execute(&mut **transaction)
    .await?;
    Ok(true)
}

async fn upsert_status_index_content(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    status_id: &str,
    server_domain: &str,
    token_text: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO status_search_icu_content(status_id, server_domain, token_text)
         VALUES (?, ?, ?)
         ON CONFLICT(status_id, server_domain) DO UPDATE SET
             token_text = excluded.token_text",
    )
    .bind(status_id)
    .bind(server_domain)
    .bind(token_text)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn upsert_status_backfill_content_if_current(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: &PreparedStatusBackfillRow,
) -> Result<(), sqlx::Error> {
    // Another app process may have updated and indexed this status after the
    // reader snapshot was prepared. Only write the backfill result when every
    // indexed source field still matches that snapshot. A later live update
    // will otherwise remain authoritative.
    sqlx::query(
        "INSERT INTO status_search_icu_content(status_id, server_domain, token_text)
         SELECT ?, ?, ?
          WHERE EXISTS (
              SELECT 1
                FROM statuses
               WHERE id = ?
                 AND server_domain = ?
                 AND content = ?
                 AND spoiler_text = ?
                 AND uri = ?
                 AND url IS ?
                 AND tags_json IS ?
          )
         ON CONFLICT(status_id, server_domain) DO UPDATE SET
             token_text = excluded.token_text",
    )
    .bind(&row.status_id)
    .bind(&row.server_domain)
    .bind(&row.token_text)
    .bind(&row.status_id)
    .bind(&row.server_domain)
    .bind(&row.content)
    .bind(&row.spoiler_text)
    .bind(&row.uri)
    .bind(row.url.as_deref())
    .bind(row.tags_json.as_deref())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn upsert_account_backfill_content_if_current(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    row: &PreparedAccountBackfillRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO account_search_icu_content(account_id, server_domain, token_text)
         SELECT ?, ?, ?
          WHERE EXISTS (
              SELECT 1
                FROM accounts
               WHERE id = ?
                 AND server_domain = ?
                 AND acct = ?
                 AND display_name = ?
          )
         ON CONFLICT(account_id, server_domain) DO UPDATE SET
             token_text = excluded.token_text",
    )
    .bind(&row.account_id)
    .bind(&row.server_domain)
    .bind(&row.token_text)
    .bind(&row.account_id)
    .bind(&row.server_domain)
    .bind(&row.acct)
    .bind(&row.display_name)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn load_status_backfill_state(
    reader: &SqlitePool,
) -> Result<StatusBackfillState, sqlx::Error> {
    sqlx::query_as::<_, StatusBackfillState>(
        "SELECT cursor_status_id,
                cursor_server_domain,
                processed_count,
                total_count,
                completed
           FROM status_search_icu_backfill_state
          WHERE singleton = 1",
    )
    .fetch_one(reader)
    .await
}

async fn load_status_backfill_batch(
    reader: &SqlitePool,
    state: &StatusBackfillState,
) -> Result<Vec<StatusBackfillRow>, sqlx::Error> {
    match (
        state.cursor_status_id.as_deref(),
        state.cursor_server_domain.as_deref(),
    ) {
        (Some(status_id), Some(server_domain)) => {
            sqlx::query_as::<_, StatusBackfillRow>(
                "SELECT id AS status_id, server_domain, content, spoiler_text,
                        uri, url, tags_json
                   FROM statuses
                  WHERE (id, server_domain) < (?, ?)
                  ORDER BY id DESC, server_domain DESC
                  LIMIT ?",
            )
            .bind(status_id)
            .bind(server_domain)
            .bind(BACKFILL_CHUNK_SIZE)
            .fetch_all(reader)
            .await
        }
        _ => {
            sqlx::query_as::<_, StatusBackfillRow>(
                "SELECT id AS status_id, server_domain, content, spoiler_text,
                        uri, url, tags_json
                   FROM statuses
                  ORDER BY id DESC, server_domain DESC
                  LIMIT ?",
            )
            .bind(BACKFILL_CHUNK_SIZE)
            .fetch_all(reader)
            .await
        }
    }
}

async fn load_account_backfill_state(
    reader: &SqlitePool,
) -> Result<AccountBackfillState, sqlx::Error> {
    sqlx::query_as::<_, AccountBackfillState>(
        "SELECT cursor_account_id,
                cursor_server_domain,
                processed_count,
                total_count,
                completed
           FROM account_search_icu_backfill_state
          WHERE singleton = 1",
    )
    .fetch_one(reader)
    .await
}

async fn load_account_backfill_batch(
    reader: &SqlitePool,
    state: &AccountBackfillState,
) -> Result<Vec<AccountBackfillRow>, sqlx::Error> {
    match (
        state.cursor_account_id.as_deref(),
        state.cursor_server_domain.as_deref(),
    ) {
        (Some(account_id), Some(server_domain)) => {
            sqlx::query_as::<_, AccountBackfillRow>(
                "SELECT id AS account_id, server_domain, acct, display_name
                   FROM accounts
                  WHERE (id, server_domain) < (?, ?)
                  ORDER BY id DESC, server_domain DESC
                  LIMIT ?",
            )
            .bind(account_id)
            .bind(server_domain)
            .bind(BACKFILL_CHUNK_SIZE)
            .fetch_all(reader)
            .await
        }
        _ => {
            sqlx::query_as::<_, AccountBackfillRow>(
                "SELECT id AS account_id, server_domain, acct, display_name
                   FROM accounts
                  ORDER BY id DESC, server_domain DESC
                  LIMIT ?",
            )
            .bind(BACKFILL_CHUNK_SIZE)
            .fetch_all(reader)
            .await
        }
    }
}

async fn progress(
    reader: &SqlitePool,
    status_state: &StatusBackfillState,
    account_state: &AccountBackfillState,
) -> Result<SearchIndexProgress, sqlx::Error> {
    Ok(SearchIndexProgress {
        processed_count: non_negative_u64(status_state.processed_count)
            .saturating_add(non_negative_u64(account_state.processed_count)),
        total_count: non_negative_u64(status_state.total_count)
            .saturating_add(non_negative_u64(account_state.total_count)),
        completed: status_state.completed && account_state.completed,
        pending_count: pending_count(reader).await?,
    })
}

async fn try_bounded_merge(writer: &SqlitePool, target: MergeTarget) -> Result<bool, sqlx::Error> {
    let Some(mut connection) = writer.try_acquire() else {
        return Ok(false);
    };
    enter_low_priority_write(&mut connection).await?;
    let write_result = commit_bounded_merge(&mut connection, target).await;
    match finish_low_priority_write(&mut connection, write_result).await {
        Ok(merged) => Ok(merged),
        Err(error) if sqlite_write_is_busy(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

async fn commit_bounded_merge(
    connection: &mut SqliteConnection,
    target: MergeTarget,
) -> Result<bool, sqlx::Error> {
    let mut transaction = connection.begin().await?;
    let live_work = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM status_search_index_queue)
             OR EXISTS(SELECT 1 FROM account_search_index_queue)",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if live_work {
        transaction.rollback().await?;
        return Ok(false);
    }
    let merge_result = match target {
        MergeTarget::Status => {
            sqlx::query(
                "INSERT INTO status_search_icu_fts(status_search_icu_fts, rank)
                 VALUES ('merge', ?)",
            )
            .bind(MERGE_PAGE_BUDGET)
            .execute(&mut *transaction)
            .await
        }
        MergeTarget::Account => {
            sqlx::query(
                "INSERT INTO account_search_icu_fts(account_search_icu_fts, rank)
                 VALUES ('merge', ?)",
            )
            .bind(MERGE_PAGE_BUDGET)
            .execute(&mut *transaction)
            .await
        }
    };
    if let Err(error) = merge_result {
        if sqlite_write_is_busy(&error) {
            transaction.rollback().await?;
            return Err(error);
        }
        // A permanently invalid/corrupt FTS command must not leave a durable
        // one-second retry loop. Consume this finite attempt, commit the debt
        // update, then surface the original error for observability.
        decrement_merge_debt(&mut transaction, target).await?;
        transaction.commit().await?;
        return Err(error);
    }
    decrement_merge_debt(&mut transaction, target).await?;
    transaction.commit().await?;
    Ok(true)
}

async fn decrement_merge_debt(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target: MergeTarget,
) -> Result<(), sqlx::Error> {
    let query = match target {
        MergeTarget::Status => {
            "UPDATE status_search_index_control
                SET merge_debt = MAX(0, merge_debt - 1)
              WHERE singleton = 1"
        }
        MergeTarget::Account => {
            "UPDATE status_search_index_control
                SET account_merge_debt = MAX(0, account_merge_debt - 1)
              WHERE singleton = 1"
        }
    };
    sqlx::query(query).execute(&mut **transaction).await?;
    Ok(())
}

async fn load_merge_debt(reader: &SqlitePool) -> Result<MergeDebt, sqlx::Error> {
    sqlx::query_as(
        "SELECT merge_debt, account_merge_debt
           FROM status_search_index_control
          WHERE singleton = 1",
    )
    .fetch_one(reader)
    .await
}

fn select_merge_target(debt: &MergeDebt, preferred: MergeTarget) -> Option<MergeTarget> {
    match (debt.merge_debt > 0, debt.account_merge_debt > 0) {
        (true, true) => Some(preferred),
        (true, false) => Some(MergeTarget::Status),
        (false, true) => Some(MergeTarget::Account),
        (false, false) => None,
    }
}

async fn record_merge_debt(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target: MergeTarget,
) -> Result<(), sqlx::Error> {
    // Positive FTS5 merges are bounded but may be no-ops until enough same-
    // level segments exist. Persist a finite number of idle-time attempts per
    // index commit: enough to continue a partial merge, but never a permanent
    // one-second writer loop when no merge is currently eligible.
    let query = match target {
        MergeTarget::Status => {
            "UPDATE status_search_index_control
                SET merge_debt = MIN(2147483647, merge_debt + ?)
              WHERE singleton = 1"
        }
        MergeTarget::Account => {
            "UPDATE status_search_index_control
                SET account_merge_debt = MIN(2147483647, account_merge_debt + ?)
              WHERE singleton = 1"
        }
    };
    sqlx::query(query)
        .bind(MERGE_ATTEMPTS_PER_INDEX_COMMIT)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn enter_low_priority_write(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    // SQLx's try_acquire only observes this process. A second Awayuki process
    // can still own SQLite's cross-process writer lock, so disable the normal
    // five-second busy wait while background work owns our only writer
    // connection. SQLITE_BUSY pauses the worker instead of queueing an
    // interactive local write behind it.
    sqlx::query("PRAGMA busy_timeout = 0")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn finish_low_priority_write<T>(
    connection: &mut SqliteConnection,
    result: Result<T, sqlx::Error>,
) -> Result<T, sqlx::Error> {
    let restore = sqlx::query(&format!("PRAGMA busy_timeout = {DEFAULT_BUSY_TIMEOUT_MS}"))
        .execute(&mut *connection)
        .await;
    match result {
        Ok(value) => {
            restore?;
            Ok(value)
        }
        Err(error) => {
            if let Err(restore_error) = restore {
                tracing::warn!(%restore_error, "Failed to restore SQLite writer busy timeout");
            }
            Err(error)
        }
    }
}

fn non_negative_u64(value: i64) -> u64 {
    u64::try_from(value.max(0)).unwrap_or(u64::MAX)
}

fn sqlite_write_is_busy(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database_error) = error else {
        return false;
    };
    let Some(code) = database_error.code() else {
        return false;
    };
    if matches!(code.as_ref(), "SQLITE_BUSY" | "SQLITE_LOCKED") {
        return true;
    }
    code.parse::<i32>()
        .is_ok_and(|numeric| matches!(numeric & 0xff, 5 | 6))
}
