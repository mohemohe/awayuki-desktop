use std::collections::HashSet;
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

use crate::auth::session::AccountSession;
use crate::db::models::DbAccount;
use crate::db::pool::Database;
use crate::db::queries::{accounts, servers};
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::types::notification::Notification;
use crate::services::timeline_service::{self, BatchTimeline, TimelineType};

const DEFAULT_HEAD_REFRESH_HOURS: i64 = 6;
const DEFAULT_FULL_RECONCILE_DAYS: i64 = 7;
const STARTUP_PAGE_LIMIT: u32 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartupSyncPhase {
    Home,
    Public,
    Notifications,
    Bookmarks,
    Favourites,
}

impl StartupSyncPhase {
    pub const ALL: [Self; 5] = [
        Self::Home,
        Self::Public,
        Self::Notifications,
        Self::Bookmarks,
        Self::Favourites,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Public => "public",
            Self::Notifications => "notifications",
            Self::Bookmarks => "bookmarks",
            Self::Favourites => "favourites",
        }
    }

    pub const fn supports_full_reconciliation(self) -> bool {
        matches!(self, Self::Bookmarks | Self::Favourites)
    }
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct StartupSyncState {
    pub account_acct: String,
    pub phase: String,
    pub high_water_id: Option<String>,
    pub resume_cursor: Option<String>,
    pub last_success_at: Option<String>,
    pub last_full_reconcile_at: Option<String>,
    pub reconciliation_generation: Option<String>,
    pub api_requests: i64,
    pub db_writes: i64,
    pub last_duration_ms: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupSyncPlan {
    Skip,
    Incremental {
        since_id: Option<String>,
        max_pages: usize,
    },
    FullReconcile {
        generation: String,
        cursor: Option<String>,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct StartupSyncPolicy {
    pub head_refresh_interval: Duration,
    pub full_reconcile_interval: Duration,
}

impl Default for StartupSyncPolicy {
    fn default() -> Self {
        Self {
            head_refresh_interval: Duration::hours(DEFAULT_HEAD_REFRESH_HOURS),
            full_reconcile_interval: Duration::days(DEFAULT_FULL_RECONCILE_DAYS),
        }
    }
}

impl StartupSyncPolicy {
    pub fn plan(
        self,
        phase: StartupSyncPhase,
        state: Option<&StartupSyncState>,
        now: DateTime<Utc>,
        next_generation: impl FnOnce() -> String,
    ) -> StartupSyncPlan {
        if let Some(generation) = state.and_then(|state| state.reconciliation_generation.clone()) {
            return StartupSyncPlan::FullReconcile {
                generation,
                cursor: state.and_then(|state| state.resume_cursor.clone()),
            };
        }

        if phase.supports_full_reconciliation() {
            let full_is_due = state
                .and_then(|state| parse_time(state.last_full_reconcile_at.as_deref()))
                .is_none_or(|last| now.signed_duration_since(last) >= self.full_reconcile_interval);
            if full_is_due {
                return StartupSyncPlan::FullReconcile {
                    generation: next_generation(),
                    cursor: None,
                };
            }

            let head_is_fresh = state
                .and_then(|state| parse_time(state.last_success_at.as_deref()))
                .is_some_and(|last| now.signed_duration_since(last) < self.head_refresh_interval);
            if head_is_fresh {
                return StartupSyncPlan::Skip;
            }
        }

        StartupSyncPlan::Incremental {
            since_id: state.and_then(|state| state.high_water_id.clone()),
            // A head check is intentionally bounded. Protocols without a
            // reliable bookmark mutation cursor are corrected by the weekly
            // full reconciliation instead of scanning all history at launch.
            max_pages: 1,
        }
    }
}

fn parse_time(value: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

pub async fn load_phase_state(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
) -> Result<Option<StartupSyncState>, sqlx::Error> {
    sqlx::query_as::<_, StartupSyncState>(
        "SELECT * FROM startup_sync_state WHERE account_acct = ? AND phase = ?",
    )
    .bind(account_acct)
    .bind(phase.as_str())
    .fetch_optional(pool)
    .await
}

pub async fn prepare_phase(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
    policy: StartupSyncPolicy,
) -> Result<StartupSyncPlan, sqlx::Error> {
    prepare_phase_at(
        pool,
        account_acct,
        phase,
        policy,
        Utc::now(),
        uuid::Uuid::new_v4().to_string(),
    )
    .await
}

pub async fn prepare_phase_at(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
    policy: StartupSyncPolicy,
    now: DateTime<Utc>,
    next_generation: String,
) -> Result<StartupSyncPlan, sqlx::Error> {
    let state = load_phase_state(pool, account_acct, phase).await?;
    let plan = policy.plan(phase, state.as_ref(), now, || next_generation);
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO startup_sync_state(account_acct, phase)
         VALUES (?, ?)
         ON CONFLICT(account_acct, phase) DO NOTHING",
    )
    .bind(account_acct)
    .bind(phase.as_str())
    .execute(&mut *transaction)
    .await?;

    if let StartupSyncPlan::FullReconcile { generation, cursor } = &plan {
        if cursor.is_none()
            && state
                .as_ref()
                .and_then(|state| state.reconciliation_generation.as_ref())
                .is_none()
        {
            sqlx::query(
                "DELETE FROM startup_sync_reconciliation_members
                 WHERE account_acct = ? AND phase = ?",
            )
            .bind(account_acct)
            .bind(phase.as_str())
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "UPDATE startup_sync_state
                    SET reconciliation_generation = ?, resume_cursor = NULL,
                        last_error = NULL
                  WHERE account_acct = ? AND phase = ?",
            )
            .bind(generation)
            .bind(account_acct)
            .bind(phase.as_str())
            .execute(&mut *transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(plan)
}

pub struct PageCheckpoint<'a> {
    /// Set only for the newest page of a run.
    pub high_water_id: Option<&'a str>,
    pub next_cursor: Option<&'a str>,
    pub reconciliation_generation: Option<&'a str>,
    pub members: &'a [(String, String)],
    pub api_requests: u64,
    pub db_writes: u64,
    pub duration_ms: u64,
}

pub async fn commit_page_checkpoint(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
    checkpoint: PageCheckpoint<'_>,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    if let Some(generation) = checkpoint.reconciliation_generation {
        if !checkpoint.members.is_empty() {
            let mut builder = QueryBuilder::<Sqlite>::new(
                "INSERT INTO startup_sync_reconciliation_members \
                 (account_acct, phase, generation, status_id, server_domain) ",
            );
            builder.push_values(checkpoint.members, |mut row, (status_id, server_domain)| {
                row.push_bind(account_acct)
                    .push_bind(phase.as_str())
                    .push_bind(generation)
                    .push_bind(status_id)
                    .push_bind(server_domain);
            });
            builder.push(" ON CONFLICT DO NOTHING");
            builder.build().execute(&mut *transaction).await?;
        }
    }

    let result = sqlx::query(
        "UPDATE startup_sync_state
            SET high_water_id = COALESCE(?, high_water_id),
                resume_cursor = ?,
                api_requests = api_requests + ?,
                db_writes = db_writes + ?,
                last_duration_ms = ?,
                last_error = NULL
          WHERE account_acct = ? AND phase = ?",
    )
    .bind(checkpoint.high_water_id)
    .bind(checkpoint.next_cursor)
    .bind(saturating_i64(checkpoint.api_requests))
    .bind(saturating_i64(checkpoint.db_writes))
    .bind(saturating_i64(checkpoint.duration_ms))
    .bind(account_acct)
    .bind(phase.as_str())
    .execute(&mut *transaction)
    .await?;
    if result.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    transaction.commit().await?;
    Ok(())
}

fn saturating_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconciliationResult {
    pub cleared_viewer_states: u64,
    pub removed_timeline_entries: u64,
}

pub async fn complete_phase(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
) -> Result<ReconciliationResult, sqlx::Error> {
    complete_phase_at(pool, account_acct, phase, Utc::now()).await
}

pub async fn complete_phase_at(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
    now: DateTime<Utc>,
) -> Result<ReconciliationResult, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let generation: Option<String> = sqlx::query_scalar(
        "SELECT reconciliation_generation FROM startup_sync_state
         WHERE account_acct = ? AND phase = ?",
    )
    .bind(account_acct)
    .bind(phase.as_str())
    .fetch_optional(&mut *transaction)
    .await?
    .flatten();

    let mut result = ReconciliationResult::default();
    if let Some(generation) = generation.as_deref() {
        let viewer_column = match phase {
            StartupSyncPhase::Bookmarks => Some("bookmarked"),
            StartupSyncPhase::Favourites => Some("favourited"),
            _ => None,
        };
        if let Some(viewer_column) = viewer_column {
            let statement = format!(
                "UPDATE status_viewer_state
                    SET {viewer_column} = 0, updated_at = ?
                  WHERE login_account_acct = ? AND {viewer_column} = 1
                    AND NOT EXISTS (
                        SELECT 1 FROM startup_sync_reconciliation_members seen
                         WHERE seen.account_acct = ? AND seen.phase = ?
                           AND seen.generation = ?
                           AND seen.status_id = status_viewer_state.status_id
                           AND seen.server_domain = status_viewer_state.server_domain
                    )"
            );
            result.cleared_viewer_states = sqlx::query(&statement)
                .bind(now.to_rfc3339())
                .bind(account_acct)
                .bind(account_acct)
                .bind(phase.as_str())
                .bind(generation)
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        }

        result.removed_timeline_entries = sqlx::query(
            "DELETE FROM timeline_entries
              WHERE account_acct = ? AND timeline_type = ?
                AND NOT EXISTS (
                    SELECT 1 FROM startup_sync_reconciliation_members seen
                     WHERE seen.account_acct = ? AND seen.phase = ?
                       AND seen.generation = ?
                       AND seen.status_id = timeline_entries.status_id
                       AND seen.server_domain = timeline_entries.server_domain
                )",
        )
        .bind(account_acct)
        .bind(phase.as_str())
        .bind(account_acct)
        .bind(phase.as_str())
        .bind(generation)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        sqlx::query(
            "DELETE FROM startup_sync_reconciliation_members
             WHERE account_acct = ? AND phase = ? AND generation = ?",
        )
        .bind(account_acct)
        .bind(phase.as_str())
        .bind(generation)
        .execute(&mut *transaction)
        .await?;
    }

    let now = now.to_rfc3339();
    let completed = sqlx::query(
        "UPDATE startup_sync_state
            SET resume_cursor = NULL,
                reconciliation_generation = NULL,
                last_success_at = ?,
                last_full_reconcile_at = CASE WHEN ? THEN ? ELSE last_full_reconcile_at END,
                last_error = NULL
          WHERE account_acct = ? AND phase = ?",
    )
    .bind(&now)
    .bind(generation.is_some())
    .bind(&now)
    .bind(account_acct)
    .bind(phase.as_str())
    .execute(&mut *transaction)
    .await?;
    if completed.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    transaction.commit().await?;
    Ok(result)
}

pub async fn fail_phase(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
    error: &str,
) -> Result<(), sqlx::Error> {
    let error = error.chars().take(1_000).collect::<String>();
    let result = sqlx::query(
        "UPDATE startup_sync_state SET last_error = ?
         WHERE account_acct = ? AND phase = ?",
    )
    .bind(error)
    .bind(account_acct)
    .bind(phase.as_str())
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseRunMetrics {
    pub phase: StartupSyncPhase,
    pub skipped: bool,
    pub api_requests: u64,
    pub db_writes: u64,
    pub fetched_items: u64,
    pub duration_ms: u64,
    pub error: Option<String>,
}

impl PhaseRunMetrics {
    fn skipped(phase: StartupSyncPhase) -> Self {
        Self {
            phase,
            skipped: true,
            api_requests: 0,
            db_writes: 0,
            fetched_items: 0,
            duration_ms: 0,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRunMetrics {
    pub account_acct: String,
    pub phases: Vec<PhaseRunMetrics>,
    pub api_requests: u64,
    pub db_writes: u64,
    pub fetched_items: u64,
    pub ready_ms: u64,
    pub failed_phases: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupSyncProgress {
    pub account_acct: String,
    pub phase: StartupSyncPhase,
    pub page: u32,
    pub total: usize,
}

/// Run every startup phase independently. An API or DB failure in one phase is
/// recorded in that phase's durable row and never prevents the remaining
/// phases from advancing.
pub async fn run_startup_account(
    database: &Database,
    session: &AccountSession,
    progress: Option<&tokio::sync::mpsc::Sender<StartupSyncProgress>>,
) -> AccountRunMetrics {
    let started = Instant::now();
    let mut phases = Vec::with_capacity(StartupSyncPhase::ALL.len());
    for phase in StartupSyncPhase::ALL {
        if !phase_supported(session, phase) {
            phases.push(PhaseRunMetrics::skipped(phase));
            continue;
        }
        let phase_started = Instant::now();
        match run_phase(database, session, phase, progress).await {
            Ok(mut metrics) => {
                metrics.duration_ms = elapsed_millis(phase_started);
                if record_phase_duration(
                    database.writer(),
                    &session.acct,
                    phase,
                    metrics.duration_ms,
                )
                .await
                .is_ok()
                {
                    metrics.db_writes += 1;
                }
                phases.push(metrics);
            }
            Err(error) => {
                let duration_ms = elapsed_millis(phase_started);
                if let Err(state_error) =
                    fail_phase(database.writer(), &session.acct, phase, &error).await
                {
                    tracing::warn!(
                        account_acct = session.acct,
                        phase = phase.as_str(),
                        %state_error,
                        "Failed to persist startup phase error"
                    );
                }
                let _ = record_phase_duration(database.writer(), &session.acct, phase, duration_ms)
                    .await;
                phases.push(PhaseRunMetrics {
                    phase,
                    skipped: false,
                    api_requests: 0,
                    db_writes: 0,
                    fetched_items: 0,
                    duration_ms,
                    error: Some(error),
                });
            }
        }
    }

    let metrics = AccountRunMetrics {
        account_acct: session.acct.clone(),
        api_requests: phases.iter().map(|phase| phase.api_requests).sum(),
        db_writes: phases.iter().map(|phase| phase.db_writes).sum(),
        fetched_items: phases.iter().map(|phase| phase.fetched_items).sum(),
        failed_phases: phases.iter().filter(|phase| phase.error.is_some()).count(),
        phases,
        ready_ms: elapsed_millis(started),
    };
    tracing::info!(
        account_acct = metrics.account_acct,
        api_requests = metrics.api_requests,
        db_writes = metrics.db_writes,
        fetched_items = metrics.fetched_items,
        ready_ms = metrics.ready_ms,
        failed_phases = metrics.failed_phases,
        "Startup account synchronization metrics"
    );
    metrics
}

async fn record_phase_duration(
    pool: &SqlitePool,
    account_acct: &str,
    phase: StartupSyncPhase,
    duration_ms: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE startup_sync_state
            SET last_duration_ms = ?, db_writes = db_writes + 1
          WHERE account_acct = ? AND phase = ?",
    )
    .bind(saturating_i64(duration_ms))
    .bind(account_acct)
    .bind(phase.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

fn phase_supported(session: &AccountSession, phase: StartupSyncPhase) -> bool {
    let capabilities = session.client.capabilities(1).timelines;
    match phase {
        StartupSyncPhase::Home => capabilities.home,
        StartupSyncPhase::Public => capabilities.public,
        StartupSyncPhase::Notifications => capabilities.notifications,
        StartupSyncPhase::Bookmarks => capabilities.bookmarks,
        StartupSyncPhase::Favourites => capabilities.favourites,
    }
}

async fn run_phase(
    database: &Database,
    session: &AccountSession,
    phase: StartupSyncPhase,
    progress: Option<&tokio::sync::mpsc::Sender<StartupSyncProgress>>,
) -> Result<PhaseRunMetrics, String> {
    let plan = prepare_phase(
        database.writer(),
        &session.acct,
        phase,
        StartupSyncPolicy::default(),
    )
    .await
    .map_err(|error| error.to_string())?;
    if matches!(plan, StartupSyncPlan::Skip) {
        return Ok(PhaseRunMetrics::skipped(phase));
    }
    match phase {
        StartupSyncPhase::Home | StartupSyncPhase::Public => {
            run_timeline_phase(database, session, phase, plan).await
        }
        StartupSyncPhase::Notifications => {
            run_notification_phase(database, session, phase, plan).await
        }
        StartupSyncPhase::Bookmarks | StartupSyncPhase::Favourites => {
            run_collection_phase(database, session, phase, plan, progress).await
        }
    }
}

async fn run_timeline_phase(
    database: &Database,
    session: &AccountSession,
    phase: StartupSyncPhase,
    plan: StartupSyncPlan,
) -> Result<PhaseRunMetrics, String> {
    let StartupSyncPlan::Incremental { since_id, .. } = plan else {
        return Err(format!(
            "unexpected reconciliation plan for {}",
            phase.as_str()
        ));
    };
    let timeline_type = match phase {
        StartupSyncPhase::Home => TimelineType::Home,
        StartupSyncPhase::Public => TimelineType::Public,
        _ => return Err(format!("invalid timeline phase: {}", phase.as_str())),
    };
    let statuses = timeline_service::sync_timeline(
        &session.client,
        database.writer(),
        database.reader(),
        &timeline_type,
        &session.acct,
        &TimelineParams {
            since_id,
            limit: Some(STARTUP_PAGE_LIMIT),
            ..Default::default()
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    let high_water_id = statuses.first().map(|status| status.id.as_str());
    commit_page_checkpoint(
        database.writer(),
        &session.acct,
        phase,
        PageCheckpoint {
            high_water_id,
            next_cursor: None,
            reconciliation_generation: None,
            members: &[],
            api_requests: 1,
            db_writes: 2,
            duration_ms: 0,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    complete_phase(database.writer(), &session.acct, phase)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PhaseRunMetrics {
        phase,
        skipped: false,
        api_requests: 1,
        db_writes: 3,
        fetched_items: statuses.len() as u64,
        duration_ms: 0,
        error: None,
    })
}

async fn run_notification_phase(
    database: &Database,
    session: &AccountSession,
    phase: StartupSyncPhase,
    plan: StartupSyncPlan,
) -> Result<PhaseRunMetrics, String> {
    let StartupSyncPlan::Incremental { since_id, .. } = plan else {
        return Err("unexpected notification reconciliation plan".to_string());
    };
    let notifications = session
        .client
        .get_notifications(&NotificationParams {
            since_id,
            limit: Some(STARTUP_PAGE_LIMIT),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;
    let high_water_id = notifications
        .first()
        .map(|notification| notification.id.clone());
    let persistence_writes = save_notification_batch(database, session, &notifications).await?;
    commit_page_checkpoint(
        database.writer(),
        &session.acct,
        phase,
        PageCheckpoint {
            high_water_id: high_water_id.as_deref(),
            next_cursor: None,
            reconciliation_generation: None,
            members: &[],
            api_requests: 1,
            db_writes: persistence_writes + 1,
            duration_ms: 0,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    complete_phase(database.writer(), &session.acct, phase)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PhaseRunMetrics {
        phase,
        skipped: false,
        api_requests: 1,
        db_writes: persistence_writes + 2,
        fetched_items: notifications.len() as u64,
        duration_ms: 0,
        error: None,
    })
}

async fn save_notification_batch(
    database: &Database,
    session: &AccountSession,
    notifications: &[Notification],
) -> Result<u64, String> {
    let status_items = notifications
        .iter()
        .filter_map(|notification| notification.status.as_ref())
        .map(|status| timeline_service::StatusBatchItem {
            status,
            timeline: None,
            viewer_acct: Some(session.acct.as_str()),
        })
        .collect::<Vec<_>>();
    if !status_items.is_empty() {
        timeline_service::save_status_items_with_retry(
            database.writer(),
            &status_items,
            session.client.domain(),
        )
        .await
        .map_err(|error| error.to_string())?;
    }

    let mut transaction = database
        .writer()
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    servers::upsert_server_on(
        &mut transaction,
        session.client.domain(),
        session.client.streaming_url(),
    )
    .await
    .map_err(|error| error.to_string())?;
    let mut actors = HashSet::new();
    for notification in notifications {
        if actors.insert(notification.account.id.as_str()) {
            accounts::upsert_account_on(
                &mut transaction,
                &DbAccount::from_api(&notification.account, session.client.domain()),
            )
            .await
            .map_err(|error| error.to_string())?;
        }
        sqlx::query(
            "INSERT INTO notifications
             (id, server_domain, account_acct, notification_type, created_at,
              account_id, status_id, read_at, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)
             ON CONFLICT(id, server_domain, account_acct) DO UPDATE SET
               notification_type = excluded.notification_type,
               created_at = excluded.created_at,
               account_id = excluded.account_id,
               status_id = excluded.status_id,
               fetched_at = excluded.fetched_at",
        )
        .bind(&notification.id)
        .bind(session.client.domain())
        .bind(&session.acct)
        .bind(notification.notification_type.as_str())
        .bind(notification.created_at.to_rfc3339())
        .bind(&notification.account.id)
        .bind(
            notification
                .status
                .as_ref()
                .map(|status| status.id.as_str()),
        )
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;

    for status in notifications
        .iter()
        .filter_map(|notification| notification.status.as_ref())
    {
        timeline_service::schedule_pending_quote_resolution(
            &session.client,
            database.writer(),
            std::slice::from_ref(status),
            session.client.domain(),
            &session.acct,
        );
    }
    Ok(u64::from(!status_items.is_empty()) + 1)
}

async fn run_collection_phase(
    database: &Database,
    session: &AccountSession,
    phase: StartupSyncPhase,
    plan: StartupSyncPlan,
    progress: Option<&tokio::sync::mpsc::Sender<StartupSyncProgress>>,
) -> Result<PhaseRunMetrics, String> {
    let (generation, mut cursor, since_id, max_pages) = match plan {
        StartupSyncPlan::FullReconcile { generation, cursor } => {
            (Some(generation), cursor, None, usize::MAX)
        }
        StartupSyncPlan::Incremental {
            since_id,
            max_pages,
        } => (None, None, since_id, max_pages.max(1)),
        StartupSyncPlan::Skip => return Ok(PhaseRunMetrics::skipped(phase)),
    };
    let mut seen_cursors = HashSet::new();
    if let Some(cursor) = cursor.as_ref() {
        seen_cursors.insert(cursor.clone());
    }
    let mut api_requests = 0_u64;
    let mut db_writes = 0_u64;
    let mut fetched_items = 0_u64;
    let mut page = 0_u32;

    loop {
        page += 1;
        let response = match phase {
            StartupSyncPhase::Bookmarks => {
                session
                    .client
                    .get_bookmarks(&TimelineParams {
                        max_id: cursor.clone(),
                        since_id: since_id.clone(),
                        limit: Some(STARTUP_PAGE_LIMIT),
                        ..Default::default()
                    })
                    .await
            }
            StartupSyncPhase::Favourites => {
                session
                    .client
                    .get_favourites(&TimelineParams {
                        max_id: cursor.clone(),
                        since_id: since_id.clone(),
                        limit: Some(STARTUP_PAGE_LIMIT),
                        ..Default::default()
                    })
                    .await
            }
            _ => return Err(format!("invalid collection phase: {}", phase.as_str())),
        }
        .map_err(|error| error.to_string())?;
        api_requests += 1;
        let high_water_id = (page == 1 && cursor.is_none())
            .then(|| response.data.first().map(|status| status.id.clone()))
            .flatten();
        let page_statuses = response
            .data
            .into_iter()
            .map(|status| match phase {
                StartupSyncPhase::Bookmarks => {
                    let mut status = status;
                    status.bookmarked = Some(true);
                    status
                }
                StartupSyncPhase::Favourites => {
                    let mut status = status.reblog.as_deref().cloned().unwrap_or(status);
                    status.favourited = Some(true);
                    status
                }
                _ => unreachable!(),
            })
            .collect::<Vec<_>>();
        let page_count = page_statuses.len();
        let timeline = BatchTimeline {
            timeline_type: phase.as_str(),
            account_acct: &session.acct,
        };
        timeline_service::save_status_batch_with_retry(
            database.writer(),
            &page_statuses,
            session.client.domain(),
            Some(timeline),
        )
        .await
        .map_err(|error| error.to_string())?;
        timeline_service::schedule_pending_quote_resolution(
            &session.client,
            database.writer(),
            &page_statuses,
            session.client.domain(),
            &session.acct,
        );

        let next_cursor = response.next_max_id.filter(|cursor| !cursor.is_empty());
        let members = page_statuses
            .iter()
            .map(|status| (status.id.clone(), session.client.domain().to_string()))
            .collect::<Vec<_>>();
        let page_db_writes = u64::from(!page_statuses.is_empty()) + 1;
        commit_page_checkpoint(
            database.writer(),
            &session.acct,
            phase,
            PageCheckpoint {
                high_water_id: high_water_id.as_deref(),
                next_cursor: next_cursor.as_deref(),
                reconciliation_generation: generation.as_deref(),
                members: &members,
                api_requests: 1,
                db_writes: page_db_writes,
                duration_ms: 0,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        db_writes += page_db_writes;
        fetched_items += page_count as u64;
        if let Some(progress) = progress {
            let _ = progress
                .send(StartupSyncProgress {
                    account_acct: session.acct.clone(),
                    phase,
                    page,
                    total: fetched_items as usize,
                })
                .await;
        }

        if generation.is_none() || (page as usize) >= max_pages || page_count == 0 {
            break;
        }
        match next_cursor {
            Some(next) if seen_cursors.insert(next.clone()) => cursor = Some(next),
            Some(next) => {
                return Err(format!(
                    "{} pagination repeated cursor {next}; reconciliation remains resumable",
                    phase.as_str()
                ));
            }
            None => break,
        }
    }

    complete_phase(database.writer(), &session.acct, phase)
        .await
        .map_err(|error| error.to_string())?;
    db_writes += 1;
    Ok(PhaseRunMetrics {
        phase,
        skipped: false,
        api_requests,
        db_writes,
        fetched_items,
        duration_ms: 0,
        error: None,
    })
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

pub async fn protect_thread_statuses(
    pool: &SqlitePool,
    statuses: &[(String, String)],
    accessed_at: DateTime<Utc>,
) -> Result<u64, sqlx::Error> {
    if statuses.is_empty() {
        return Ok(0);
    }
    let timestamp = accessed_at.to_rfc3339();
    let mut builder = QueryBuilder::<Sqlite>::new(
        "INSERT INTO status_retention_protections \
         (status_id, server_domain, reason, last_accessed_at) ",
    );
    builder.push_values(statuses, |mut row, (status_id, server_domain)| {
        row.push_bind(status_id)
            .push_bind(server_domain)
            .push_bind("thread")
            .push_bind(&timestamp);
    });
    builder.push(
        " ON CONFLICT(status_id, server_domain, reason) DO UPDATE
          SET last_accessed_at = excluded.last_accessed_at",
    );
    Ok(builder.build().execute(pool).await?.rows_affected())
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    pub max_status_age: Duration,
    pub max_thread_idle: Duration,
    pub max_entries_per_timeline: i64,
    pub incremental_vacuum_pages: u32,
}

#[cfg(test)]
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_status_age: Duration::days(90),
            max_thread_idle: Duration::days(30),
            max_entries_per_timeline: 10_000,
            incremental_vacuum_pages: 256,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceMetrics {
    pub removed_timeline_entries: u64,
    pub removed_statuses: u64,
    pub removed_accounts: u64,
    pub removed_tags: u64,
    pub removed_expired_protections: u64,
    pub elapsed_ms: u64,
}

#[cfg(test)]
pub async fn run_idle_maintenance_at(
    pool: &SqlitePool,
    policy: RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<MaintenanceMetrics, sqlx::Error> {
    let started = Instant::now();
    let status_cutoff = (now - policy.max_status_age).to_rfc3339();
    let protection_cutoff = (now - policy.max_thread_idle).to_rfc3339();
    let mut transaction = pool.begin().await?;
    let removed_expired_protections =
        sqlx::query("DELETE FROM status_retention_protections WHERE last_accessed_at < ?")
            .bind(protection_cutoff)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
    let mut metrics = MaintenanceMetrics {
        removed_expired_protections,
        ..MaintenanceMetrics::default()
    };

    metrics.removed_timeline_entries += sqlx::query(
        "DELETE FROM timeline_entries
          WHERE position_at < ?
            AND timeline_type NOT IN ('bookmarks', 'favourites')",
    )
    .bind(&status_cutoff)
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    metrics.removed_timeline_entries += sqlx::query(
        "DELETE FROM timeline_entries
          WHERE id IN (
            SELECT id FROM (
              SELECT id,
                     ROW_NUMBER() OVER (
                       PARTITION BY timeline_type, account_acct
                       ORDER BY position_at DESC, server_domain DESC, status_id DESC
                     ) AS retention_rank
                FROM timeline_entries
            ) ranked
            WHERE retention_rank > ?
          )",
    )
    .bind(policy.max_entries_per_timeline.max(1))
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    metrics.removed_statuses = sqlx::query(
        "DELETE FROM statuses
          WHERE fetched_at < ?
            AND COALESCE(pinned, 0) = 0
            AND NOT EXISTS (
                SELECT 1 FROM timeline_entries te
                 WHERE te.status_id = statuses.id
                   AND te.server_domain = statuses.server_domain
            )
            AND NOT EXISTS (
                SELECT 1 FROM notifications n
                 WHERE n.status_id = statuses.id
                   AND n.server_domain = statuses.server_domain
            )
            AND NOT EXISTS (
                SELECT 1 FROM status_viewer_state viewer
                 WHERE viewer.status_id = statuses.id
                   AND viewer.server_domain = statuses.server_domain
                   AND (COALESCE(viewer.bookmarked, 0) = 1
                     OR COALESCE(viewer.favourited, 0) = 1
                     OR COALESCE(viewer.pinned, 0) = 1)
            )
            AND NOT EXISTS (
                SELECT 1 FROM status_retention_protections protected
                 WHERE protected.status_id = statuses.id
                   AND protected.server_domain = statuses.server_domain
            )
            AND NOT EXISTS (
                SELECT 1 FROM startup_sync_reconciliation_members staged
                 WHERE staged.status_id = statuses.id
                   AND staged.server_domain = statuses.server_domain
            )
            AND NOT EXISTS (
                SELECT 1 FROM statuses dependent
                 WHERE dependent.server_domain = statuses.server_domain
                   AND (dependent.in_reply_to_id = statuses.id
                     OR dependent.reblog_of_id = statuses.id
                     OR dependent.quote_id = statuses.id)
            )",
    )
    .bind(status_cutoff)
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    metrics.removed_tags = sqlx::query(
        "DELETE FROM tags
          WHERE NOT EXISTS (
            SELECT 1 FROM status_tags st
             WHERE st.tag_name = tags.name
               AND st.server_domain = tags.server_domain
          )",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    metrics.removed_accounts = sqlx::query(
        "DELETE FROM accounts
          WHERE NOT EXISTS (
            SELECT 1 FROM statuses s
             WHERE s.account_id = accounts.id
               AND s.server_domain = accounts.server_domain
          )
            AND NOT EXISTS (
              SELECT 1 FROM notifications n
               WHERE n.account_id = accounts.id
                 AND n.server_domain = accounts.server_domain
            )
            AND NOT EXISTS (
              SELECT 1 FROM login_accounts login
               WHERE login.account_id = accounts.id
                 AND login.server_domain = accounts.server_domain
            )",
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    transaction.commit().await?;

    // PASSIVE never blocks active readers. incremental_vacuum is a no-op when
    // a legacy database has not yet opted into incremental auto-vacuum.
    let _ = sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
        .fetch_all(pool)
        .await?;
    let vacuum = format!(
        "PRAGMA incremental_vacuum({})",
        policy.incremental_vacuum_pages
    );
    sqlx::query(&vacuum).execute(pool).await?;
    metrics.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::Database;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);

    async fn database(label: &str) -> (Database, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-startup-sync-{label}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let database = Database::new(directory.join("awayuki.db")).await.unwrap();
        let _ = database.run_migrations().await.unwrap();
        sqlx::query(
            "INSERT INTO servers(domain, streaming_url, server_kind)
             VALUES ('example.test', 'wss://example.test', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO accounts
             (id, server_domain, username, acct, display_name, note, avatar,
              avatar_static, header, locked, bot, followers_count,
              following_count, statuses_count, created_at, fetched_at)
             VALUES ('viewer', 'example.test', 'viewer', 'viewer', 'Viewer', '', '',
                     '', '', 0, 0, 0, 0, 0,
                     '2025-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO login_accounts
             (acct, server_domain, account_id, display_name, avatar, is_active,
              access_token, server_kind)
             VALUES ('viewer@example.test', 'example.test', 'viewer', 'Viewer', '', 1,
                     'token', 'mastodon')",
        )
        .execute(database.writer())
        .await
        .unwrap();
        (database, directory)
    }

    async fn seed_status(pool: &SqlitePool, id: &str, fetched_at: &str) {
        sqlx::query(
            "INSERT INTO statuses
             (id, server_domain, uri, created_at, account_id, content, fetched_at)
             VALUES (?, 'example.test', ?, ?, 'viewer', ?, ?)",
        )
        .bind(id)
        .bind(format!("https://example.test/@viewer/{id}"))
        .bind(fetched_at)
        .bind(id)
        .bind(fetched_at)
        .execute(pool)
        .await
        .unwrap();
    }

    fn state(
        high_water_id: Option<&str>,
        success: Option<DateTime<Utc>>,
        full: Option<DateTime<Utc>>,
        generation: Option<&str>,
        cursor: Option<&str>,
    ) -> StartupSyncState {
        StartupSyncState {
            account_acct: "alice@example.test".to_string(),
            phase: "bookmarks".to_string(),
            high_water_id: high_water_id.map(str::to_string),
            resume_cursor: cursor.map(str::to_string),
            last_success_at: success.map(|value| value.to_rfc3339()),
            last_full_reconcile_at: full.map(|value| value.to_rfc3339()),
            reconciliation_generation: generation.map(str::to_string),
            api_requests: 0,
            db_writes: 0,
            last_duration_ms: 0,
            last_error: None,
        }
    }

    #[test]
    fn unchanged_second_startup_skips_bookmark_history() {
        let now = DateTime::parse_from_rfc3339("2026-07-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let existing = state(
            Some("high"),
            Some(now - Duration::minutes(1)),
            Some(now - Duration::days(1)),
            None,
            None,
        );
        let plan = StartupSyncPolicy::default().plan(
            StartupSyncPhase::Bookmarks,
            Some(&existing),
            now,
            || "unused".to_string(),
        );
        assert_eq!(plan, StartupSyncPlan::Skip);
    }

    #[test]
    fn interrupted_full_reconciliation_resumes_durable_cursor() {
        let now = Utc::now();
        let existing = state(
            Some("high"),
            None,
            None,
            Some("generation-1"),
            Some("cursor-4"),
        );
        let plan = StartupSyncPolicy::default().plan(
            StartupSyncPhase::Bookmarks,
            Some(&existing),
            now,
            || "must-not-replace".to_string(),
        );
        assert_eq!(
            plan,
            StartupSyncPlan::FullReconcile {
                generation: "generation-1".to_string(),
                cursor: Some("cursor-4".to_string())
            }
        );
    }

    #[test]
    fn ordinary_timeline_uses_high_water_and_one_page() {
        let now = Utc::now();
        let existing = state(Some("status-42"), Some(now), None, None, None);
        let plan =
            StartupSyncPolicy::default().plan(StartupSyncPhase::Home, Some(&existing), now, || {
                "unused".to_string()
            });
        assert_eq!(
            plan,
            StartupSyncPlan::Incremental {
                since_id: Some("status-42".to_string()),
                max_pages: 1
            }
        );
    }

    #[tokio::test]
    async fn full_reconciliation_resumes_and_clears_remote_removals() {
        let (database, directory) = database("reconcile").await;
        let now = DateTime::parse_from_rfc3339("2026-07-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for id in ["kept", "removed"] {
            seed_status(database.writer(), id, "2026-01-01T00:00:00Z").await;
            sqlx::query(
                "INSERT INTO status_viewer_state
                 (login_account_acct, status_id, server_domain, bookmarked, updated_at)
                 VALUES ('viewer@example.test', ?, 'example.test', 1, ?)",
            )
            .bind(id)
            .bind(now.to_rfc3339())
            .execute(database.writer())
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO timeline_entries
                 (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('bookmarks', 'example.test', ?, 'viewer@example.test', ?)",
            )
            .bind(id)
            .bind(now.to_rfc3339())
            .execute(database.writer())
            .await
            .unwrap();
        }

        let plan = prepare_phase_at(
            database.writer(),
            "viewer@example.test",
            StartupSyncPhase::Bookmarks,
            StartupSyncPolicy::default(),
            now,
            "generation-1".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            plan,
            StartupSyncPlan::FullReconcile {
                generation: "generation-1".to_string(),
                cursor: None
            }
        );
        commit_page_checkpoint(
            database.writer(),
            "viewer@example.test",
            StartupSyncPhase::Bookmarks,
            PageCheckpoint {
                high_water_id: Some("kept"),
                next_cursor: Some("page-2"),
                reconciliation_generation: Some("generation-1"),
                members: &[("kept".to_string(), "example.test".to_string())],
                api_requests: 1,
                db_writes: 1,
                duration_ms: 12,
            },
        )
        .await
        .unwrap();
        fail_phase(
            database.writer(),
            "viewer@example.test",
            StartupSyncPhase::Bookmarks,
            "injected interruption",
        )
        .await
        .unwrap();

        let resumed = prepare_phase_at(
            database.writer(),
            "viewer@example.test",
            StartupSyncPhase::Bookmarks,
            StartupSyncPolicy::default(),
            now + Duration::minutes(1),
            "must-not-replace".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            resumed,
            StartupSyncPlan::FullReconcile {
                generation: "generation-1".to_string(),
                cursor: Some("page-2".to_string())
            }
        );

        let result = complete_phase_at(
            database.writer(),
            "viewer@example.test",
            StartupSyncPhase::Bookmarks,
            now + Duration::minutes(2),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            ReconciliationResult {
                cleared_viewer_states: 1,
                removed_timeline_entries: 1
            }
        );
        let removed_bookmark: bool = sqlx::query_scalar(
            "SELECT bookmarked FROM status_viewer_state
             WHERE login_account_acct = 'viewer@example.test' AND status_id = 'removed'",
        )
        .fetch_one(database.reader())
        .await
        .unwrap();
        assert!(!removed_bookmark);
        let state = load_phase_state(
            database.reader(),
            "viewer@example.test",
            StartupSyncPhase::Bookmarks,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(state.reconciliation_generation.is_none());
        assert!(state.resume_cursor.is_none());
        assert_eq!(state.api_requests, 1);
        assert_eq!(state.db_writes, 1);

        database.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn retention_prunes_only_unreferenced_unprotected_statuses() {
        let (database, directory) = database("retention").await;
        let now = DateTime::parse_from_rfc3339("2026-07-11T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for id in ["orphan", "bookmark", "thread"] {
            seed_status(database.writer(), id, "2025-01-01T00:00:00Z").await;
        }
        sqlx::query(
            "INSERT INTO status_viewer_state
             (login_account_acct, status_id, server_domain, bookmarked, updated_at)
             VALUES ('viewer@example.test', 'bookmark', 'example.test', 1, ?)",
        )
        .bind(now.to_rfc3339())
        .execute(database.writer())
        .await
        .unwrap();
        protect_thread_statuses(
            database.writer(),
            &[("thread".to_string(), "example.test".to_string())],
            now,
        )
        .await
        .unwrap();

        let metrics = run_idle_maintenance_at(
            database.writer(),
            RetentionPolicy {
                max_status_age: Duration::days(90),
                max_thread_idle: Duration::days(30),
                max_entries_per_timeline: 100,
                incremental_vacuum_pages: 1,
            },
            now,
        )
        .await
        .unwrap();
        assert_eq!(metrics.removed_statuses, 1);
        let remaining: Vec<String> = sqlx::query_scalar("SELECT id FROM statuses ORDER BY id")
            .fetch_all(database.reader())
            .await
            .unwrap();
        assert_eq!(remaining, vec!["bookmark", "thread"]);

        database.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }
}
