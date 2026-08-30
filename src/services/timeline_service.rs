use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};

use chrono::Utc;
use regex::Regex;
use sqlx::SqlitePool;
use tokio::sync::{broadcast, Semaphore};
use url::Url;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;
use crate::db::models::{DbAccount, DbStatus};
use crate::db::queries::{accounts, servers, statuses, tags, timeline};
use crate::domain::adapter_error::AdapterError;
use crate::domain::capability::{CapabilityError, TimelineOperation};
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::types::status::Status;

const DB_WRITE_MAX_ATTEMPTS: usize = 5;
const DB_WRITE_RETRY_DELAYS_MS: [u64; DB_WRITE_MAX_ATTEMPTS - 1] = [200, 500, 1_000, 2_000];
const DB_BATCH_MAX_STATUSES: usize = 64;
const DB_BATCH_MAX_DURATION: Duration = Duration::from_millis(40);
const QUOTE_LINK_RETRY_RECENCY_SECS: i64 = 15 * 60;
const BACKGROUND_QUOTE_CAPACITY: usize = 128;
const BACKGROUND_QUOTE_CONCURRENCY: usize = 4;
const BACKGROUND_QUOTE_SERVER_CONCURRENCY: usize = 1;
const BACKGROUND_QUOTE_TIMEOUT: Duration = Duration::from_secs(5);
const BACKGROUND_QUOTE_NEGATIVE_TTL: Duration = Duration::from_secs(5 * 60);
const BACKGROUND_QUOTE_ATTEMPTS: usize = 3;
const QUOTE_UPDATE_BUS_CAPACITY: usize = 256;

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// Timeline types stored in the database
#[derive(Debug, Clone)]
pub enum TimelineType {
    Home,
    Public,
    Local,
    List(String),
    Feed(String),
    Hashtag(String),
    Notification,
    CustomSql(String),
    Bookmarks,
    Favourites,
    UserBookmarks {
        server_domain: String,
        account_id: String,
    },
    Search(String),
    YukariQuery(String),
    KrileQuery(String),
}

impl TimelineType {
    /// Global timeline resources aggregate every signed-in source. Active
    /// account is a mutation actor and must never narrow these reads.
    pub fn is_unified(&self) -> bool {
        matches!(self, Self::Home | Self::Public | Self::Notification)
    }

    pub fn as_str(&self) -> String {
        match self {
            Self::Home => "home".to_string(),
            Self::Public => "public".to_string(),
            Self::Local => "local".to_string(),
            Self::List(id) => format!("list:{}", id),
            Self::Feed(id) => format!("feed:{}", id),
            Self::Hashtag(tag) => format!("tag:{}", tag),
            Self::Notification => "notification".to_string(),
            Self::CustomSql(_) => "custom".to_string(),
            Self::Bookmarks => "bookmarks".to_string(),
            Self::Favourites => "favourites".to_string(),
            Self::UserBookmarks { .. } => "user_bookmarks".to_string(),
            Self::Search(_) => "search".to_string(),
            Self::YukariQuery(_) => "yq".to_string(),
            Self::KrileQuery(_) => "kq".to_string(),
        }
    }

    /// Convert from column_configs DB row to TimelineType
    pub fn from_column_config(column_type: &str, column_param: Option<&str>) -> Option<Self> {
        let param = plain_column_param(column_param);
        match column_type {
            "home" => Some(Self::Home),
            "public" => Some(Self::Public),
            "local" => Some(Self::Local),
            "notification" => Some(Self::Notification),
            "bookmarks" => Some(Self::Bookmarks),
            "favourites" => Some(Self::Favourites),
            "list" => param.map(Self::List),
            "feed" => param.map(Self::Feed),
            "hashtag" => param.map(Self::Hashtag),
            "custom" => param.map(Self::CustomSql),
            "search" => param.map(Self::Search),
            "yq" => param.map(Self::YukariQuery),
            "kq" => param.map(Self::KrileQuery),
            "user_bookmarks" => column_param.and_then(parse_user_bookmarks_column_param),
            _ => None,
        }
    }

    /// Display name for the timeline type
    pub fn display_name(&self) -> String {
        match self {
            Self::Home => "Home".to_string(),
            Self::Public => "Federated".to_string(),
            Self::Local => "Local".to_string(),
            Self::List(id) => format!("List: {}", id),
            Self::Feed(id) => format!("Feed: {}", id),
            Self::Hashtag(tag) => format!("#{}", tag),
            Self::Notification => "Notification".to_string(),
            Self::CustomSql(_) => "Custom".to_string(),
            Self::Bookmarks => "Bookmarks".to_string(),
            Self::Favourites => "Favorites".to_string(),
            Self::UserBookmarks { .. } => "Bookmarks".to_string(),
            Self::Search(_) => "Search".to_string(),
            Self::YukariQuery(_) => "YQ".to_string(),
            Self::KrileQuery(_) => "KQ".to_string(),
        }
    }
}

fn plain_column_param(column_param: Option<&str>) -> Option<String> {
    let raw = column_param?;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Some(raw.to_string());
    };
    let Some(object) = value.as_object() else {
        return Some(raw.to_string());
    };
    if !object.contains_key("filters") {
        return Some(raw.to_string());
    }
    object
        .get("value")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn parse_user_bookmarks_column_param(param: &str) -> Option<TimelineType> {
    let value = serde_json::from_str::<serde_json::Value>(param).ok()?;
    let server_domain = value.get("serverDomain")?.as_str()?.trim();
    let account_id = value.get("accountId")?.as_str()?.trim();
    if server_domain.is_empty() || account_id.is_empty() {
        return None;
    }
    Some(TimelineType::UserBookmarks {
        server_domain: server_domain.to_string(),
        account_id: account_id.to_string(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("API error: {0}")]
    Api(#[from] AdapterError),
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Capability(#[from] CapabilityError),
    #[error("timeline sync cancelled")]
    Cancelled,
}

/// Optional timeline metadata stored in the same transaction as a status page.
#[derive(Debug, Clone, Copy)]
pub struct BatchTimeline<'a> {
    pub timeline_type: &'a str,
    pub account_acct: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct StatusBatchItem<'a> {
    pub status: &'a Status,
    pub timeline: Option<BatchTimeline<'a>>,
    pub viewer_acct: Option<&'a str>,
}

pub struct TimelineSyncControl<'a> {
    pub quote_consumer_id: Option<&'a str>,
    pub cancellation: Option<&'a tokio_util::sync::CancellationToken>,
    pub on_commit: &'a mut (dyn FnMut() + Send),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineGap {
    pub timeline_type: String,
    pub source_acct: String,
    pub boundary_status_id: String,
    pub boundary_server_domain: String,
    pub boundary_position: String,
    pub next_max_status_id: String,
}

#[derive(Debug, Clone)]
pub struct TimelineSyncPage {
    pub statuses: Vec<Status>,
    pub gap: Option<TimelineGap>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchPersistMetrics {
    pub statuses: usize,
    pub transactions: usize,
    pub account_upserts: usize,
    pub status_upserts: usize,
    pub tag_upserts: usize,
    pub timeline_inserts: usize,
    pub max_transaction_statuses: usize,
    pub max_transaction_ms: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct QuoteResolutionUpdate {
    pub status: Status,
    pub server_domain: String,
    pub source_acct: String,
    pub state: QuoteResolutionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteResolutionState {
    Resolved,
    Unavailable,
}

impl QuoteResolutionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Default)]
struct BackgroundQuoteRegistry {
    negative_until: HashMap<String, Instant>,
    jobs: HashMap<String, BackgroundQuoteJob>,
    account_jobs: HashMap<(String, String), HashSet<String>>,
    source_jobs: HashMap<(String, String), HashSet<String>>,
    consumer_jobs: HashMap<String, HashSet<String>>,
    next_generation: u64,
}

#[derive(Debug)]
struct BackgroundQuoteJob {
    generation: u64,
    source_account: (String, String),
    owners: HashSet<BackgroundQuoteOwner>,
    abort_handle: Option<tokio::task::AbortHandle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum BackgroundQuoteOwner {
    Account(String, String),
    Consumer(String),
}

struct BackgroundQuoteGuard {
    key: String,
    generation: u64,
}

impl Drop for BackgroundQuoteGuard {
    fn drop(&mut self) {
        let mut registry = background_quote_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        remove_background_quote_job(&mut registry, &self.key, Some(self.generation));
    }
}

fn register_background_quote_owner(
    registry: &mut BackgroundQuoteRegistry,
    key: &str,
    owner: BackgroundQuoteOwner,
) {
    let Some(job) = registry.jobs.get_mut(key) else {
        return;
    };
    if !job.owners.insert(owner.clone()) {
        return;
    }
    match owner {
        BackgroundQuoteOwner::Account(server_domain, source_acct) => {
            registry
                .account_jobs
                .entry((server_domain, source_acct))
                .or_default()
                .insert(key.to_string());
        }
        BackgroundQuoteOwner::Consumer(consumer_id) => {
            registry
                .consumer_jobs
                .entry(consumer_id)
                .or_default()
                .insert(key.to_string());
        }
    }
}

fn remove_background_quote_job(
    registry: &mut BackgroundQuoteRegistry,
    key: &str,
    generation: Option<u64>,
) -> Option<tokio::task::AbortHandle> {
    if generation.is_some_and(|generation| {
        registry.jobs.get(key).map(|job| job.generation) != Some(generation)
    }) {
        return None;
    }
    let job = registry.jobs.remove(key)?;
    if let Some(jobs) = registry.source_jobs.get_mut(&job.source_account) {
        jobs.remove(key);
    }
    for owner in job.owners {
        let jobs = match owner {
            BackgroundQuoteOwner::Account(server_domain, source_acct) => {
                registry.account_jobs.get_mut(&(server_domain, source_acct))
            }
            BackgroundQuoteOwner::Consumer(consumer_id) => {
                registry.consumer_jobs.get_mut(&consumer_id)
            }
        };
        if let Some(jobs) = jobs {
            jobs.remove(key);
        }
    }
    registry.account_jobs.retain(|_, jobs| !jobs.is_empty());
    registry.source_jobs.retain(|_, jobs| !jobs.is_empty());
    registry.consumer_jobs.retain(|_, jobs| !jobs.is_empty());
    job.abort_handle
}

fn cancel_background_quote_owner(owner: BackgroundQuoteOwner) -> bool {
    let (removed, handles) = {
        let mut registry = background_quote_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = match &owner {
            BackgroundQuoteOwner::Account(server_domain, source_acct) => registry
                .account_jobs
                .remove(&(server_domain.clone(), source_acct.clone())),
            BackgroundQuoteOwner::Consumer(consumer_id) => {
                registry.consumer_jobs.remove(consumer_id)
            }
        }
        .unwrap_or_default();
        let removed = !keys.is_empty();
        let mut handles = Vec::new();
        for key in keys {
            let should_abort = registry.jobs.get_mut(&key).is_some_and(|job| {
                job.owners.remove(&owner);
                job.owners.is_empty()
            });
            if should_abort {
                if let Some(handle) = remove_background_quote_job(&mut registry, &key, None) {
                    handles.push(handle);
                }
            }
        }
        (removed, handles)
    };
    for handle in handles {
        handle.abort();
    }
    removed
}

fn cancel_background_quote_source(server_domain: &str, source_acct: &str) -> bool {
    let (removed, handles) = {
        let mut registry = background_quote_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let source_account = (server_domain.to_string(), source_acct.to_string());
        let keys = registry
            .source_jobs
            .remove(&source_account)
            .unwrap_or_default();
        let removed = !keys.is_empty();
        let handles = keys
            .into_iter()
            .filter_map(|key| remove_background_quote_job(&mut registry, &key, None))
            .collect::<Vec<_>>();
        (removed, handles)
    };
    for handle in handles {
        handle.abort();
    }
    removed
}

fn background_quote_registry() -> &'static Mutex<BackgroundQuoteRegistry> {
    static REGISTRY: OnceLock<Mutex<BackgroundQuoteRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BackgroundQuoteRegistry::default()))
}

fn background_quote_semaphore() -> Arc<Semaphore> {
    static SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(BACKGROUND_QUOTE_CONCURRENCY)))
        .clone()
}

fn background_quote_server_semaphore(server_domain: &str) -> Arc<Semaphore> {
    static SEMAPHORES: OnceLock<Mutex<HashMap<String, Weak<Semaphore>>>> = OnceLock::new();
    let mut semaphores = SEMAPHORES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    semaphores.retain(|_, semaphore| semaphore.strong_count() > 0);
    if let Some(semaphore) = semaphores.get(server_domain).and_then(Weak::upgrade) {
        return semaphore;
    }
    let semaphore = Arc::new(Semaphore::new(BACKGROUND_QUOTE_SERVER_CONCURRENCY));
    semaphores.insert(server_domain.to_string(), Arc::downgrade(&semaphore));
    semaphore
}

fn quote_update_sender() -> &'static broadcast::Sender<QuoteResolutionUpdate> {
    static SENDER: OnceLock<broadcast::Sender<QuoteResolutionUpdate>> = OnceLock::new();
    SENDER.get_or_init(|| broadcast::channel(QUOTE_UPDATE_BUS_CAPACITY).0)
}

pub fn subscribe_quote_resolution_updates() -> broadcast::Receiver<QuoteResolutionUpdate> {
    quote_update_sender().subscribe()
}

impl BatchPersistMetrics {
    #[cfg(test)]
    pub fn statement_count(&self) -> usize {
        self.transactions
            + self.account_upserts
            + self.status_upserts
            + self.tag_upserts
            + self.timeline_inserts
    }
}

/// Fetch statuses from API and save them to the database.
/// Returns the fetched statuses.
pub async fn sync_timeline(
    client: &ApiClient,
    writer: &SqlitePool,
    timeline_type: &TimelineType,
    account_acct: &str,
    params: &TimelineParams,
    quote_consumer_id: Option<&str>,
) -> Result<Vec<Status>, SyncError> {
    let mut ignore_commit = || {};
    sync_timeline_with_control(
        client,
        writer,
        timeline_type,
        account_acct,
        params,
        TimelineSyncControl {
            quote_consumer_id,
            cancellation: None,
            on_commit: &mut ignore_commit,
        },
    )
    .await
    .map(|page| page.statuses)
}

pub async fn sync_timeline_with_control(
    client: &ApiClient,
    writer: &SqlitePool,
    timeline_type: &TimelineType,
    account_acct: &str,
    params: &TimelineParams,
    control: TimelineSyncControl<'_>,
) -> Result<TimelineSyncPage, SyncError> {
    let TimelineSyncControl {
        quote_consumer_id,
        cancellation,
        on_commit,
    } = control;
    let total_started_at = Instant::now();
    let server_domain = client.domain();
    let tl_key = timeline_type.as_str();
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        limit = ?params.limit,
        "[awayuki][timeline-sync] start"
    );

    // Fetch from API
    let fetch_started_at = Instant::now();
    let api_statuses = if let Some(cancellation) = cancellation {
        tokio::select! {
            _ = cancellation.cancelled() => return Err(SyncError::Cancelled),
            result = fetch_from_api(client, timeline_type, params) => result?,
        }
    } else {
        fetch_from_api(client, timeline_type, params).await?
    };
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(fetch_started_at),
        "[awayuki][timeline-sync] fetched from API"
    );
    // An algorithmic Bluesky feed is not a chronological continuity source,
    // and its pagination cursor is opaque. A missing-post interval cannot be
    // inferred from adjacent status timestamps/IDs for this timeline type.
    let gap = if !matches!(timeline_type, TimelineType::Feed(_))
        && params.since_id.is_none()
        && params.min_id.is_none()
    {
        timeline_gap_before_persisting(writer, &tl_key, account_acct, server_domain, &api_statuses)
            .await?
    } else {
        None
    };
    // Persist the fetched page before quote network lookups. Quote hydration is
    // deliberately a follow-up concern so a slow/deleted quote cannot add its
    // retry budget to initial timeline latency.
    let persist_started_at = Instant::now();
    save_status_batch_with_commit_observer(
        writer,
        &api_statuses,
        server_domain,
        Some(BatchTimeline {
            timeline_type: &tl_key,
            account_acct,
        }),
        on_commit,
    )
    .await?;
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(persist_started_at),
        "[awayuki][timeline-sync] persisted statuses"
    );

    if let Some(consumer_id) = quote_consumer_id {
        schedule_pending_quote_resolution_for_consumer(
            client,
            writer,
            &api_statuses,
            server_domain,
            account_acct,
            consumer_id,
        );
    } else {
        schedule_pending_quote_resolution(
            client,
            writer,
            &api_statuses,
            server_domain,
            account_acct,
        );
    }

    tracing::info!(
        "Synced {} statuses for timeline '{}' ({})",
        api_statuses.len(),
        tl_key,
        account_acct
    );
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(total_started_at),
        "[awayuki][timeline-sync] success"
    );

    Ok(TimelineSyncPage {
        statuses: api_statuses,
        gap,
    })
}

async fn timeline_gap_before_persisting(
    pool: &SqlitePool,
    timeline_type: &str,
    account_acct: &str,
    server_domain: &str,
    statuses: &[Status],
) -> Result<Option<TimelineGap>, sqlx::Error> {
    let Some(oldest) = statuses.iter().min_by_key(|status| status.created_at) else {
        return Ok(None);
    };
    let boundary_position = oldest.created_at.to_rfc3339();
    let (boundary_exists, older_exists) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT EXISTS (
             SELECT 1
               FROM timeline_entries entry
               JOIN statuses status
                 ON status.id = entry.status_id
                AND status.server_domain = entry.server_domain
              WHERE entry.timeline_type = ?
                AND entry.account_acct = ?
                AND entry.server_domain = ?
                AND entry.status_id = ?
                AND status.uri = ?
         ), EXISTS (
             SELECT 1
               FROM timeline_entries entry
              WHERE entry.timeline_type = ?
                AND entry.account_acct = ?
                AND entry.position_at < ?
         )",
    )
    .bind(timeline_type)
    .bind(account_acct)
    .bind(server_domain)
    .bind(&oldest.id)
    .bind(&oldest.uri)
    .bind(timeline_type)
    .bind(account_acct)
    .bind(&boundary_position)
    .fetch_one(pool)
    .await?;

    if boundary_exists || !older_exists {
        return Ok(None);
    }
    Ok(Some(TimelineGap {
        timeline_type: timeline_type.to_string(),
        source_acct: account_acct.to_string(),
        boundary_status_id: oldest.id.clone(),
        boundary_server_domain: server_domain.to_string(),
        boundary_position,
        next_max_status_id: oldest.id.clone(),
    }))
}

pub async fn hydrate_and_resolve_quotes(client: &ApiClient, statuses: &mut [Status]) {
    hydrate_missing_quotes_once(client, statuses, true).await;
    resolve_linked_quotes_once(client, statuses, true).await;
}

/// Schedule quote hydration after the initial status page has already been
/// persisted/returned. Jobs are canonical-key deduplicated, globally bounded,
/// timeout-limited and broadcast as status updates when resolved.
pub fn schedule_pending_quote_resolution(
    client: &ApiClient,
    writer: &SqlitePool,
    statuses: &[Status],
    server_domain: &str,
    source_acct: &str,
) {
    schedule_pending_quote_resolution_owned(
        client,
        writer,
        statuses,
        server_domain,
        source_acct,
        BackgroundQuoteOwner::Account(server_domain.to_string(), source_acct.to_string()),
    );
}

pub fn schedule_pending_quote_resolution_for_consumer(
    client: &ApiClient,
    writer: &SqlitePool,
    statuses: &[Status],
    server_domain: &str,
    source_acct: &str,
    consumer_id: &str,
) {
    let consumer_id = consumer_id.trim();
    if consumer_id.is_empty() || consumer_id.len() > 256 {
        tracing::warn!("Ignoring invalid quote consumer ID");
        return;
    }
    schedule_pending_quote_resolution_owned(
        client,
        writer,
        statuses,
        server_domain,
        source_acct,
        BackgroundQuoteOwner::Consumer(consumer_id.to_string()),
    );
}

fn schedule_pending_quote_resolution_owned(
    client: &ApiClient,
    writer: &SqlitePool,
    statuses: &[Status],
    server_domain: &str,
    source_acct: &str,
    owner: BackgroundQuoteOwner,
) {
    for status in statuses {
        let Some(candidate_key) = background_quote_candidate_key(client.kind(), status) else {
            continue;
        };
        let key = format!("{server_domain}\0{source_acct}\0{candidate_key}");
        let now = Instant::now();
        let generation = {
            let mut registry = background_quote_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.negative_until.retain(|_, until| *until > now);
            if registry.negative_until.contains_key(&key) {
                None
            } else if registry.jobs.contains_key(&key) {
                register_background_quote_owner(&mut registry, &key, owner.clone());
                None
            } else if registry.jobs.len() >= BACKGROUND_QUOTE_CAPACITY {
                None
            } else {
                registry.next_generation = registry.next_generation.wrapping_add(1).max(1);
                let generation = registry.next_generation;
                registry.jobs.insert(
                    key.clone(),
                    BackgroundQuoteJob {
                        generation,
                        source_account: (server_domain.to_string(), source_acct.to_string()),
                        owners: HashSet::new(),
                        abort_handle: None,
                    },
                );
                registry
                    .source_jobs
                    .entry((server_domain.to_string(), source_acct.to_string()))
                    .or_default()
                    .insert(key.clone());
                register_background_quote_owner(&mut registry, &key, owner.clone());
                Some(generation)
            }
        };
        let Some(generation) = generation else {
            continue;
        };
        let guard = BackgroundQuoteGuard {
            key: key.clone(),
            generation,
        };
        let client = client.clone();
        let writer = writer.clone();
        let mut status = status.clone();
        let server_domain = server_domain.to_string();
        let source_acct = source_acct.to_string();
        let semaphore = background_quote_semaphore();
        let server_semaphore = background_quote_server_semaphore(&server_domain);
        let registry_key = key.clone();
        let task = tokio::spawn(async move {
            let _guard = guard;
            let Ok(_server_permit) = server_semaphore.acquire_owned().await else {
                return;
            };
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };
            let mut resolved = false;
            for attempt in 0..BACKGROUND_QUOTE_ATTEMPTS {
                let lookup = hydrate_and_resolve_quotes(&client, std::slice::from_mut(&mut status));
                if tokio::time::timeout(BACKGROUND_QUOTE_TIMEOUT, lookup)
                    .await
                    .is_ok()
                    && status.quote.is_some()
                {
                    resolved = true;
                    break;
                }
                if attempt + 1 < BACKGROUND_QUOTE_ATTEMPTS {
                    tokio::time::sleep(background_quote_retry_delay(&key, attempt)).await;
                }
            }

            if resolved {
                if let Err(error) = save_status_batch_with_retry(
                    &writer,
                    std::slice::from_ref(&status),
                    &server_domain,
                    None,
                )
                .await
                {
                    tracing::warn!("Failed to persist background quote {key}: {error}");
                } else {
                    let _ = quote_update_sender().send(QuoteResolutionUpdate {
                        status,
                        server_domain,
                        source_acct,
                        state: QuoteResolutionState::Resolved,
                    });
                }
            } else {
                let _ = quote_update_sender().send(QuoteResolutionUpdate {
                    status,
                    server_domain,
                    source_acct,
                    state: QuoteResolutionState::Unavailable,
                });
                let mut registry = background_quote_registry()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                registry
                    .negative_until
                    .insert(key, Instant::now() + BACKGROUND_QUOTE_NEGATIVE_TTL);
            }
        });
        let mut registry = background_quote_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let job_is_active = if let Some(job) = registry.jobs.get_mut(&registry_key) {
            if job.generation == generation && !job.owners.is_empty() {
                job.abort_handle = Some(task.abort_handle());
                true
            } else {
                false
            }
        } else {
            false
        };
        if !job_is_active {
            drop(registry);
            task.abort();
        }
    }
}

pub fn cancel_pending_quote_resolution(server_domain: &str, source_acct: &str) {
    cancel_background_quote_source(server_domain, source_acct);
}

pub fn cancel_pending_quote_consumer(consumer_id: &str) -> bool {
    let consumer_id = consumer_id.trim();
    if consumer_id.is_empty() || consumer_id.len() > 256 {
        return false;
    }
    cancel_background_quote_owner(BackgroundQuoteOwner::Consumer(consumer_id.to_string()))
}

pub fn cancel_all_pending_quote_resolution() -> usize {
    let (count, handles) = {
        let mut registry = background_quote_registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys = registry.jobs.keys().cloned().collect::<Vec<_>>();
        let count = keys.len();
        let handles = keys
            .into_iter()
            .filter_map(|key| remove_background_quote_job(&mut registry, &key, None))
            .collect::<Vec<_>>();
        (count, handles)
    };
    for handle in handles {
        handle.abort();
    }
    count
}

fn background_quote_candidate_key(kind: ServerKind, status: &Status) -> Option<String> {
    if !status_may_have_pending_quote(kind, status) {
        return None;
    }
    status
        .quote_id
        .as_ref()
        .map(|id| format!("id:{id}"))
        .or_else(|| {
            status
                .quote_original_url
                .as_ref()
                .map(|url| format!("url:{url}"))
        })
        .or_else(|| extract_status_link(&status.content).map(|url| format!("url:{url}")))
}

fn background_quote_retry_delay(key: &str, attempt: usize) -> Duration {
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    Duration::from_millis(250 * (attempt as u64 + 1) + hash % 250)
}

async fn resolve_linked_quotes_once(client: &ApiClient, statuses: &mut [Status], warn: bool) {
    if !client.kind().is_mastodon_compatible() {
        return;
    }

    for status in statuses {
        if status.quote.is_some() {
            continue;
        }

        let quote_url = if let Some(url) = status.quote_original_url.clone() {
            Some(url)
        } else if content_contains_quote_reply_marker(&status.content) {
            extract_status_link(&status.content)
        } else {
            None
        };

        let Some(quote_url) = quote_url else {
            continue;
        };

        match client.lookup_status_by_uri(&quote_url).await {
            Ok(Some(quote)) => {
                if quote.id == status.id {
                    tracing::debug!(
                        "Resolved quote URL {} to the source status {} on {}; ignoring",
                        quote_url,
                        status.id,
                        client.domain()
                    );
                    continue;
                }
                status.quote_id = Some(quote.id.clone());
                status.quote_original_url = Some(quote_url);
                status.quote = Some(Box::new(quote));
            }
            Ok(None) => {
                tracing::debug!(
                    "Quote URL {} did not resolve to a status on {}",
                    quote_url,
                    client.domain()
                );
            }
            Err(error) => {
                if warn {
                    tracing::warn!(
                        "Failed to resolve quote URL {} on {}: {}",
                        quote_url,
                        client.domain(),
                        error
                    );
                } else {
                    tracing::debug!(
                        "Quote URL {} is not ready on {}: {}",
                        quote_url,
                        client.domain(),
                        error
                    );
                }
            }
        }
    }
}

async fn hydrate_missing_quotes_once(client: &ApiClient, statuses: &mut [Status], warn: bool) {
    for status in statuses {
        let Some(quote_id) = status.quote_id.clone() else {
            continue;
        };
        if status.quote.is_some() {
            continue;
        }

        match client.get_status(&quote_id).await {
            Ok(quote) => {
                status.quote = Some(Box::new(quote));
            }
            Err(error) => {
                if warn {
                    tracing::warn!(
                        "Failed to hydrate quoted status {} on {}: {}",
                        quote_id,
                        client.domain(),
                        error
                    );
                } else {
                    tracing::debug!(
                        "Quoted status {} is not ready on {}: {}",
                        quote_id,
                        client.domain(),
                        error
                    );
                }
            }
        }
    }
}

fn status_may_have_pending_quote(kind: ServerKind, status: &Status) -> bool {
    if status.quote.is_some() {
        return false;
    }
    if status.quote_id.is_some() || status.quote_original_url.is_some() {
        return true;
    }
    kind.is_mastodon_compatible()
        && is_recent_enough_for_quote_resolution(status)
        && content_contains_quote_reply_marker(&status.content)
        && content_contains_status_link(&status.content)
}

fn is_recent_enough_for_quote_resolution(status: &Status) -> bool {
    Utc::now()
        .signed_duration_since(status.created_at)
        .num_seconds()
        <= QUOTE_LINK_RETRY_RECENCY_SECS
}

fn content_contains_status_link(content: &str) -> bool {
    extract_status_link(content).is_some()
}

fn extract_status_link(content: &str) -> Option<String> {
    status_link_href_regex()
        .captures_iter(content)
        .filter_map(|capture| capture.get(1).map(|href| html_unescape_href(href.as_str())))
        .find(|href| is_probable_status_url(href))
}

fn status_link_href_regex() -> &'static Regex {
    static HREF_RE: OnceLock<Regex> = OnceLock::new();
    HREF_RE.get_or_init(|| Regex::new(r#"href\s*=\s*["']([^"']+)["']"#).expect("valid href regex"))
}

fn html_unescape_href(href: &str) -> String {
    href.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

fn is_probable_status_url(href: &str) -> bool {
    let Ok(url) = Url::parse(href) else {
        return false;
    };
    let path = url.path();

    is_mastodon_status_path(path) || is_misskey_note_path(path)
}

fn is_mastodon_status_path(path: &str) -> bool {
    (path.starts_with("/@") && path.trim_matches('/').split('/').count() >= 2)
        || (path.starts_with("/users/") && path.contains("/statuses/"))
        || path.contains("/statuses/")
}

fn is_misskey_note_path(path: &str) -> bool {
    path.strip_prefix("/notes/")
        .is_some_and(|note_id| !note_id.trim_matches('/').is_empty())
}

fn content_contains_quote_reply_marker(content: &str) -> bool {
    content.contains("RE:")
}

/// Fetch statuses from the appropriate API endpoint
pub async fn fetch_from_api(
    client: &ApiClient,
    timeline_type: &TimelineType,
    params: &TimelineParams,
) -> Result<Vec<Status>, SyncError> {
    let capability = match timeline_type {
        TimelineType::Home => Some(TimelineOperation::Home),
        TimelineType::Public => Some(TimelineOperation::Public),
        TimelineType::Local => Some(TimelineOperation::Local),
        TimelineType::List(_) => Some(TimelineOperation::Lists),
        TimelineType::Feed(_) => Some(TimelineOperation::Feeds),
        TimelineType::Hashtag(_) => Some(TimelineOperation::Hashtags),
        TimelineType::Notification => Some(TimelineOperation::Notifications),
        TimelineType::Bookmarks => Some(TimelineOperation::Bookmarks),
        TimelineType::Favourites => Some(TimelineOperation::Favourites),
        TimelineType::CustomSql(_)
        | TimelineType::YukariQuery(_)
        | TimelineType::KrileQuery(_)
        | TimelineType::Search(_)
        | TimelineType::UserBookmarks { .. } => None,
    };
    if let Some(capability) = capability {
        client.capabilities(1).require_timeline(capability)?;
    }
    match timeline_type {
        TimelineType::Home => Ok(client.get_home_timeline(params).await?),
        TimelineType::Public => Ok(client.get_public_timeline(false, params).await?),
        TimelineType::Local => Ok(client.get_public_timeline(true, params).await?),
        TimelineType::List(id) => Ok(client.get_list_timeline(id, params).await?),
        TimelineType::Feed(id) => Ok(client.get_feed_timeline(id, params).await?),
        TimelineType::Hashtag(tag) => Ok(client.get_hashtag_timeline(tag, false, params).await?),
        TimelineType::Notification => {
            // Notifications use a different API and data structure.
            // Return empty for now; will be handled by a dedicated panel.
            Ok(vec![])
        }
        TimelineType::CustomSql(_)
        | TimelineType::YukariQuery(_)
        | TimelineType::KrileQuery(_)
        | TimelineType::Search(_)
        | TimelineType::UserBookmarks { .. } => {
            // SQLite-backed timelines query the local DB, not the API.
            Ok(vec![])
        }
        TimelineType::Bookmarks | TimelineType::Favourites => {
            // These are loaded from local DB after sync, not directly from API.
            Ok(vec![])
        }
    }
}

/// Save a status and its account to the database
pub async fn save_status_to_db(
    writer: &SqlitePool,
    status: &Status,
    server_domain: &str,
) -> Result<(), SyncError> {
    save_status_batch(writer, std::slice::from_ref(status), server_domain, None).await
}

/// Persist a status page/event micro-batch using short, bounded transactions.
/// Account and tag identities are deduplicated inside each transaction and the
/// timeline entry is committed atomically with its status.
pub async fn save_status_batch(
    writer: &SqlitePool,
    page: &[Status],
    server_domain: &str,
    timeline_context: Option<BatchTimeline<'_>>,
) -> Result<(), SyncError> {
    let mut ignore_commit = || {};
    save_status_batch_once_with_commit_observer(
        writer,
        page,
        server_domain,
        timeline_context,
        &mut ignore_commit,
    )
    .await
}

async fn save_status_batch_once_with_commit_observer(
    writer: &SqlitePool,
    page: &[Status],
    server_domain: &str,
    timeline_context: Option<BatchTimeline<'_>>,
    on_commit: &mut (dyn FnMut() + Send),
) -> Result<(), SyncError> {
    let items = page
        .iter()
        .map(|status| StatusBatchItem {
            status,
            timeline: timeline_context,
            viewer_acct: timeline_context.map(|context| context.account_acct),
        })
        .collect::<Vec<_>>();
    save_status_items_with_commit_observer(writer, &items, server_domain, on_commit).await
}

/// Mixed event-batch variant. Each item can independently opt in to a
/// timeline-entry insert while sharing the same short transaction.
pub async fn save_status_items(
    writer: &SqlitePool,
    items: &[StatusBatchItem<'_>],
    server_domain: &str,
) -> Result<(), SyncError> {
    let mut ignore_commit = || {};
    save_status_items_with_commit_observer(writer, items, server_domain, &mut ignore_commit).await
}

async fn save_status_items_with_commit_observer(
    writer: &SqlitePool,
    items: &[StatusBatchItem<'_>],
    server_domain: &str,
    on_commit: &mut (dyn FnMut() + Send),
) -> Result<(), SyncError> {
    save_status_items_measured_with_commit_observer(writer, items, server_domain, on_commit)
        .await
        .map(|_| ())
}

#[cfg(test)]
pub async fn save_status_items_measured(
    writer: &SqlitePool,
    items: &[StatusBatchItem<'_>],
    server_domain: &str,
) -> Result<BatchPersistMetrics, SyncError> {
    let mut ignore_commit = || {};
    save_status_items_measured_with_commit_observer(
        writer,
        items,
        server_domain,
        &mut ignore_commit,
    )
    .await
}

async fn save_status_items_measured_with_commit_observer(
    writer: &SqlitePool,
    items: &[StatusBatchItem<'_>],
    server_domain: &str,
    on_commit: &mut (dyn FnMut() + Send),
) -> Result<BatchPersistMetrics, SyncError> {
    let batch_started_at = Instant::now();
    let mut metrics = BatchPersistMetrics {
        statuses: items.len(),
        ..BatchPersistMetrics::default()
    };
    let mut offset = 0;
    let mut persisted_account_keys = HashSet::new();
    let mut persisted_tag_names = HashSet::new();
    while offset < items.len() {
        let transaction_started_at = Instant::now();
        let mut transaction = writer.begin().await?;
        let streaming_url = format!("wss://{server_domain}");
        servers::upsert_server_on(&mut transaction, server_domain, &streaming_url).await?;

        let mut account_keys = persisted_account_keys.clone();
        let mut tag_names = HashSet::new();
        let account_count_before = account_keys.len();
        let transaction_start_offset = offset;

        while offset < items.len()
            && offset - transaction_start_offset < DB_BATCH_MAX_STATUSES
            && (offset == transaction_start_offset
                || transaction_started_at.elapsed() < DB_BATCH_MAX_DURATION)
        {
            let item = items[offset];
            let status = item.status;
            metrics.status_upserts +=
                1 + usize::from(status.reblog.is_some()) + usize::from(status.quote.is_some());
            persist_status_graph(
                &mut transaction,
                status,
                server_domain,
                item.viewer_acct,
                &mut account_keys,
                &mut tag_names,
            )
            .await?;

            if let Some(context) = item.timeline {
                metrics.timeline_inserts += 1;
                timeline::insert_timeline_entry_on(
                    &mut transaction,
                    context.timeline_type,
                    server_domain,
                    &status.id,
                    context.account_acct,
                    &status.created_at.to_rfc3339(),
                )
                .await?;
            }
            offset += 1;
        }

        tag_names.retain(|tag_name| !persisted_tag_names.contains(tag_name));
        metrics.account_upserts += account_keys.len() - account_count_before;
        metrics.tag_upserts += tag_names.len();
        for tag_name in &tag_names {
            tags::upsert_tag_on(&mut transaction, tag_name, server_domain).await?;
        }
        transaction.commit().await?;
        on_commit();
        persisted_account_keys = account_keys;
        persisted_tag_names.extend(tag_names);
        metrics.transactions += 1;
        metrics.max_transaction_statuses = metrics
            .max_transaction_statuses
            .max(offset - transaction_start_offset);
        metrics.max_transaction_ms = metrics
            .max_transaction_ms
            .max(elapsed_ms(transaction_started_at));
    }
    metrics.elapsed_ms = elapsed_ms(batch_started_at);
    Ok(metrics)
}

async fn persist_status_graph(
    connection: &mut sqlx::SqliteConnection,
    status: &Status,
    server_domain: &str,
    viewer_acct: Option<&str>,
    account_keys: &mut HashSet<String>,
    tag_names: &mut HashSet<String>,
) -> Result<(), sqlx::Error> {
    let related = status
        .reblog
        .iter()
        .chain(status.quote.iter())
        .map(Box::as_ref);

    for nested in related {
        upsert_account_once(connection, nested, server_domain, account_keys).await?;
        let db_status = DbStatus::from_api(nested, server_domain);
        statuses::upsert_status_on(connection, &db_status).await?;
        if let Some(viewer_acct) = viewer_acct {
            statuses::upsert_status_viewer_state_on(connection, &db_status, viewer_acct).await?;
        }
        let nested_tags = nested
            .tags
            .iter()
            .map(|tag| tag.name.clone())
            .collect::<Vec<_>>();
        statuses::replace_status_tags_on(connection, &nested.id, server_domain, &nested_tags)
            .await?;
        tag_names.extend(nested_tags);
    }

    upsert_account_once(connection, status, server_domain, account_keys).await?;
    let db_status = DbStatus::from_api(status, server_domain);
    statuses::upsert_status_on(connection, &db_status).await?;
    if let Some(viewer_acct) = viewer_acct {
        statuses::upsert_status_viewer_state_on(connection, &db_status, viewer_acct).await?;
    }
    let status_tags = status
        .tags
        .iter()
        .map(|tag| tag.name.clone())
        .collect::<Vec<_>>();
    statuses::replace_status_tags_on(connection, &status.id, server_domain, &status_tags).await?;
    tag_names.extend(status_tags);
    Ok(())
}

async fn upsert_account_once(
    connection: &mut sqlx::SqliteConnection,
    status: &Status,
    server_domain: &str,
    account_keys: &mut HashSet<String>,
) -> Result<(), sqlx::Error> {
    let key = format!("{server_domain}\0{}", status.account.id);
    if account_keys.insert(key) {
        accounts::upsert_account_on(
            connection,
            &DbAccount::from_api(&status.account, server_domain),
        )
        .await?;
    }
    Ok(())
}

pub async fn save_status_batch_with_retry(
    writer: &SqlitePool,
    page: &[Status],
    server_domain: &str,
    timeline_context: Option<BatchTimeline<'_>>,
) -> Result<(), SyncError> {
    let mut ignore_commit = || {};
    save_status_batch_with_commit_observer(
        writer,
        page,
        server_domain,
        timeline_context,
        &mut ignore_commit,
    )
    .await
}

pub async fn save_status_batch_with_commit_observer(
    writer: &SqlitePool,
    page: &[Status],
    server_domain: &str,
    timeline_context: Option<BatchTimeline<'_>>,
    on_commit: &mut (dyn FnMut() + Send),
) -> Result<(), SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match save_status_batch_once_with_commit_observer(
            writer,
            page,
            server_domain,
            timeline_context,
            on_commit,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sync_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("save status batch", attempt, &error).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("DB write retry loop must return on success or final error")
}

pub async fn save_status_items_with_retry(
    writer: &SqlitePool,
    items: &[StatusBatchItem<'_>],
    server_domain: &str,
) -> Result<(), SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match save_status_items(writer, items, server_domain).await {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sync_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("save status event batch", attempt, &error).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("DB write retry loop must return on success or final error")
}

/// Save a status, retrying transient pool exhaustion so fetched stream/API data
/// is not dropped just because all SQLite connections were briefly busy.
pub async fn save_status_to_db_with_retry(
    writer: &SqlitePool,
    status: &Status,
    server_domain: &str,
) -> Result<(), SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match save_status_to_db(writer, status, server_domain).await {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sync_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("save status", attempt, &error).await;
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("DB write retry loop must return on success or final error")
}

/// Save canonical content together with viewer-scoped flags for one explicit
/// login account. Mutation handlers use this variant so active-account changes
/// cannot redirect or overwrite another viewer's state.
pub async fn save_status_for_viewer_to_db_with_retry(
    writer: &SqlitePool,
    status: &Status,
    server_domain: &str,
    viewer_acct: &str,
) -> Result<(), SyncError> {
    let items = [StatusBatchItem {
        status,
        timeline: None,
        viewer_acct: Some(viewer_acct),
    }];
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match save_status_items(writer, &items, server_domain).await {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sync_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("save viewer status", attempt, &error).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("DB write retry loop must return on success or final error")
}

/// Insert a timeline entry with the same transient-DB retry behavior as status
/// persistence. The underlying query is INSERT OR IGNORE, so retries are safe.
pub async fn insert_timeline_entry_with_retry(
    writer: &SqlitePool,
    timeline_type: &str,
    server_domain: &str,
    status_id: &str,
    account_acct: &str,
    position_at: &str,
) -> Result<(), SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match timeline::insert_timeline_entry(
            writer,
            timeline_type,
            server_domain,
            status_id,
            account_acct,
            position_at,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sqlx_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("insert timeline entry", attempt, &error).await;
            }
            Err(error) => return Err(SyncError::Db(error)),
        }
    }

    unreachable!("DB write retry loop must return on success or final error")
}

pub async fn delete_status_from_db_with_retry(
    writer: &SqlitePool,
    status_id: &str,
    server_domain: &str,
) -> Result<u64, SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match statuses::delete_status_and_references(writer, status_id, server_domain).await {
            Ok(rows) => return Ok(rows),
            Err(error) if should_retry_sqlx_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("delete status", attempt, &error).await;
            }
            Err(error) => return Err(SyncError::Db(error)),
        }
    }

    unreachable!("DB write retry loop must return on success or final error")
}

fn should_retry_sync_error(error: &SyncError) -> bool {
    match error {
        SyncError::Db(error) => should_retry_sqlx_error(error),
        SyncError::Api(_) | SyncError::Capability(_) | SyncError::Cancelled => false,
    }
}

fn should_retry_sqlx_error(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::Database(db_error) => {
            let message = db_error.message().to_ascii_lowercase();
            message.contains("database is locked") || message.contains("database is busy")
        }
        _ => false,
    }
}

async fn retry_db_write_after_delay(
    operation: &str,
    attempt: usize,
    error: &impl std::fmt::Display,
) {
    let delay = Duration::from_millis(DB_WRITE_RETRY_DELAYS_MS[attempt - 1]);
    tracing::warn!(
        "Transient DB error during {} (attempt {}/{}): {}; retrying in {:?}",
        operation,
        attempt,
        DB_WRITE_MAX_ATTEMPTS,
        error,
        delay
    );
    tokio::time::sleep(delay).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kq_column_config_round_trips_as_a_local_query_timeline() {
        let query = "from home where text contains \"Awayuki\"";
        let timeline = TimelineType::from_column_config("kq", Some(query))
            .expect("KQ column config should parse");

        assert!(matches!(&timeline, TimelineType::KrileQuery(value) if value == query));
        assert_eq!(timeline.as_str(), "kq");
        assert_eq!(timeline.display_name(), "KQ");
        assert!(!timeline.is_unified());
    }

    #[test]
    fn kq_column_requires_a_query_parameter() {
        assert!(TimelineType::from_column_config("kq", None).is_none());
    }

    #[test]
    fn feed_column_uses_the_generator_uri_as_its_scoped_timeline_key() {
        let feed_id = "at://did:plc:alice/app.bsky.feed.generator/news";
        let timeline = TimelineType::from_column_config("feed", Some(feed_id))
            .expect("feed column config should parse");

        assert!(matches!(&timeline, TimelineType::Feed(value) if value == feed_id));
        assert_eq!(timeline.as_str(), format!("feed:{feed_id}"));
        assert_eq!(timeline.display_name(), format!("Feed: {feed_id}"));
        assert!(!timeline.is_unified());
    }

    async fn apply_status_batch_fixture_schema(pool: &SqlitePool) {
        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/004_create_notifications.sql"),
            include_str!("../../migrations/005_create_timeline_entries.sql"),
            include_str!("../../migrations/006_create_app_settings.sql"),
            include_str!("../../migrations/011_create_tags.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
            include_str!("../../migrations/017_add_notification_account_acct.sql"),
            include_str!("../../migrations/018_add_timeline_query_indexes.sql"),
            include_str!("../../migrations/019_add_status_application.sql"),
            include_str!("../../migrations/021_normalize_status_identity_and_viewer_state.sql"),
        ] {
            sqlx::raw_sql(migration)
                .execute(pool)
                .await
                .expect("apply fixture migration");
        }

        sqlx::query(
            "INSERT INTO login_accounts
               (acct, server_domain, account_id, is_active)
             VALUES ('alice@example.test', 'example.test', 'account-1', 1)",
        )
        .execute(pool)
        .await
        .expect("insert viewer account");
    }

    async fn apply_async_search_write_fixture_schema(pool: &SqlitePool) {
        sqlx::raw_sql(
            "CREATE TABLE cache_counters (
                 name TEXT PRIMARY KEY,
                 value INTEGER NOT NULL CHECK (value >= 0),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO cache_counters(name, value)
             VALUES ('statuses', 0), ('accounts', 0);",
        )
        .execute(pool)
        .await
        .expect("create async-search fixture counters");
        for migration in [
            include_str!("../../migrations/020_create_status_search_fts.sql"),
            include_str!("../../migrations/023_resumable_status_search_backfill.sql"),
            include_str!("../../migrations/031_create_short_search_fts.sql"),
            include_str!("../../migrations/032_async_icu_status_search.sql"),
            include_str!("../../migrations/033_control_async_search_index.sql"),
            include_str!("../../migrations/034_async_icu_account_search.sql"),
            include_str!("../../migrations/035_reindex_icu_nonword_segments.sql"),
            include_str!("../../migrations/037_limit_status_icu_search_to_post_text.sql"),
        ] {
            sqlx::raw_sql(migration)
                .execute(pool)
                .await
                .expect("apply async-search fixture migration");
        }
    }

    fn fixture_status(index: usize) -> Status {
        serde_json::from_value(serde_json::json!({
            "id": format!("status-{index}"),
            "uri": format!("https://example.test/@alice/{index}"),
            "created_at": "2026-01-01T00:00:00Z",
            "account": {
                "id": "account-1",
                "username": "alice",
                "acct": "alice@example.test",
                "url": "https://example.test/@alice",
                "created_at": "2025-01-01T00:00:00Z"
            },
            "content": format!("<p>fixture {index}</p>"),
            "tags": [{
                "name": format!("tag{}", index % 10),
                "url": format!("https://example.test/tags/tag{}", index % 10)
            }]
        }))
        .expect("valid fixture")
    }

    #[test]
    fn detects_mastodon_status_links_as_pending_quote_candidates() {
        let content = r#"<p>RE:<br><a href="https://mastodon.example/@alice/123" rel="nofollow noopener noreferrer" target="_blank">mastodon.example/@alice/123</a></p>"#;

        assert!(content_contains_status_link(content));
        assert!(content_contains_quote_reply_marker(content));
    }

    #[test]
    fn detects_misskey_note_links_as_pending_quote_candidates() {
        let content = r#"<p>RE:<br><a href="https://azkey.azuki.blue/notes/ancxkus54yvf1jqf" rel="nofollow noopener noreferrer" target="_blank">azkey.azuki.blue/notes/ancxkus54yvf1jqf</a></p>"#;

        assert!(content_contains_status_link(content));
        assert_eq!(
            extract_status_link(content).as_deref(),
            Some("https://azkey.azuki.blue/notes/ancxkus54yvf1jqf")
        );
        assert!(content_contains_quote_reply_marker(content));
    }

    #[test]
    fn unescapes_status_link_hrefs() {
        let content = r#"<p>RE:<br><a href="https://mastodon.example/@alice/123?foo=1&amp;bar=2">mastodon.example/@alice/123</a></p>"#;

        assert_eq!(
            extract_status_link(content).as_deref(),
            Some("https://mastodon.example/@alice/123?foo=1&bar=2")
        );
    }

    #[test]
    fn ignores_ordinary_links_when_detecting_pending_quotes() {
        let content = r#"<p><a href="https://example.com/article" rel="nofollow noopener noreferrer" target="_blank">example.com/article</a></p>"#;

        assert!(!content_contains_status_link(content));
    }

    #[test]
    fn requires_re_marker_for_pending_quote_link_candidates() {
        let content = r#"<p><a href="https://mastodon.example/@alice/123" rel="nofollow noopener noreferrer" target="_blank">mastodon.example/@alice/123</a></p>"#;

        assert!(content_contains_status_link(content));
        assert!(!content_contains_quote_reply_marker(content));
    }

    #[tokio::test]
    async fn status_graph_and_timeline_entry_roll_back_as_one_unit() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        apply_status_batch_fixture_schema(&pool).await;
        sqlx::query(
            "CREATE TRIGGER reject_status_tag_fault
             BEFORE INSERT ON status_tags
             WHEN NEW.status_id = 'status-42'
             BEGIN
                 SELECT RAISE(ABORT, 'injected status graph failure');
             END",
        )
        .execute(&pool)
        .await
        .expect("install fault trigger");

        let status = fixture_status(42);
        let context = BatchTimeline {
            timeline_type: "home",
            account_acct: "alice@example.test",
        };
        save_status_batch(&pool, &[status], "example.test", Some(context))
            .await
            .expect_err("the injected tag relation failure must abort the transaction");

        for table in [
            "servers",
            "accounts",
            "statuses",
            "status_identities",
            "status_viewer_state",
            "status_tags",
            "tags",
            "timeline_entries",
        ] {
            let count: i64 =
                sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(count, 0, "{table} leaked a partial status graph");
        }
    }

    #[tokio::test]
    async fn timeline_gap_requires_an_uncached_boundary_and_older_same_source_entry() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        apply_status_batch_fixture_schema(&pool).await;
        sqlx::query(
            "INSERT INTO login_accounts
               (acct, server_domain, account_id, is_active)
             VALUES ('bob@example.test', 'example.test', 'account-2', 0)",
        )
        .execute(&pool)
        .await
        .expect("insert second viewer account");

        let mut older = fixture_status(1);
        older.created_at = "2026-01-01T00:00:00Z".parse().unwrap();
        save_status_batch(
            &pool,
            std::slice::from_ref(&older),
            "example.test",
            Some(BatchTimeline {
                timeline_type: "home",
                account_acct: "alice@example.test",
            }),
        )
        .await
        .expect("seed older cached delivery");

        let mut newest = fixture_status(3);
        newest.created_at = "2026-01-01T02:00:00Z".parse().unwrap();
        let mut boundary = fixture_status(2);
        boundary.created_at = "2026-01-01T01:00:00Z".parse().unwrap();
        let page = vec![newest, boundary];

        let gap = timeline_gap_before_persisting(
            &pool,
            "home",
            "alice@example.test",
            "example.test",
            &page,
        )
        .await
        .expect("detect gap")
        .expect("uncached boundary above older cache must create a gap");
        assert_eq!(gap.boundary_status_id, "status-2");
        assert_eq!(gap.next_max_status_id, "status-2");

        save_status_batch(
            &pool,
            &page,
            "example.test",
            Some(BatchTimeline {
                timeline_type: "home",
                account_acct: "bob@example.test",
            }),
        )
        .await
        .expect("cache page for another delivery source");
        assert!(timeline_gap_before_persisting(
            &pool,
            "home",
            "alice@example.test",
            "example.test",
            &page,
        )
        .await
        .expect("recheck other-source cache")
        .is_some());

        save_status_batch(
            &pool,
            &page,
            "example.test",
            Some(BatchTimeline {
                timeline_type: "home",
                account_acct: "alice@example.test",
            }),
        )
        .await
        .expect("cache exact boundary delivery");
        assert!(timeline_gap_before_persisting(
            &pool,
            "home",
            "alice@example.test",
            "example.test",
            &page,
        )
        .await
        .expect("recheck exact cache")
        .is_none());
    }

    #[tokio::test]
    async fn timeline_gap_is_not_created_without_older_cache() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        apply_status_batch_fixture_schema(&pool).await;
        let page = vec![fixture_status(1)];

        assert!(timeline_gap_before_persisting(
            &pool,
            "home",
            "alice@example.test",
            "example.test",
            &page,
        )
        .await
        .expect("check initial page")
        .is_none());

        save_status_batch(
            &pool,
            &page,
            "example.test",
            Some(BatchTimeline {
                timeline_type: "home",
                account_acct: "alice@example.test",
            }),
        )
        .await
        .expect("seed cached page");
        let mut newer = fixture_status(2);
        newer.created_at = "2026-01-01T01:00:00Z".parse().unwrap();
        assert!(timeline_gap_before_persisting(
            &pool,
            "home",
            "alice@example.test",
            "example.test",
            std::slice::from_ref(&newer),
        )
        .await
        .expect("check provider-capped page")
        .is_some());
    }

    #[tokio::test]
    async fn cache_clear_rolls_back_if_a_later_table_delete_fails() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _metadata| {
                Box::pin(crate::db::short_search_tokenizer::register(connection))
            })
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        apply_status_batch_fixture_schema(&pool).await;
        apply_async_search_write_fixture_schema(&pool).await;
        let status = fixture_status(7);
        let context = BatchTimeline {
            timeline_type: "home",
            account_acct: "alice@example.test",
        };
        save_status_batch(&pool, &[status], "example.test", Some(context))
            .await
            .expect("seed cached status");
        crate::services::search_indexer::run_index_step(&pool, &pool)
            .await
            .expect("index cached status before rollback fault");
        crate::services::search_indexer::run_index_step(&pool, &pool)
            .await
            .expect("index cached account before rollback fault");
        sqlx::query(
            "UPDATE statuses
                SET content = content || ' pending'
              WHERE id = 'status-7' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .expect("queue a newer status revision before rollback fault");
        let search_state_before =
            sqlx::query_as::<_, (bool, i64, i64, bool, bool, i64, i64, i64, i64, i64, i64)>(
                "SELECT control.index_updates_enabled,
                    control.merge_debt,
                    control.account_merge_debt,
                    status_backfill.completed,
                    account_backfill.completed,
                    (SELECT COUNT(*) FROM status_search_index_queue),
                    (SELECT COUNT(*) FROM status_search_icu_content),
                    (SELECT COUNT(*) FROM status_search_icu_fts),
                    (SELECT COUNT(*) FROM account_search_index_queue),
                    (SELECT COUNT(*) FROM account_search_icu_content),
                    (SELECT COUNT(*) FROM account_search_icu_fts)
               FROM status_search_index_control control
               JOIN status_search_icu_backfill_state status_backfill
                 ON status_backfill.singleton = control.singleton
               JOIN account_search_icu_backfill_state account_backfill
                 ON account_backfill.singleton = control.singleton
              WHERE control.singleton = 1",
            )
            .fetch_one(&pool)
            .await
            .expect("capture asynchronous search state before rollback fault");
        sqlx::query(
            "CREATE TRIGGER reject_cached_status_delete
             BEFORE DELETE ON statuses
             BEGIN
                 SELECT RAISE(ABORT, 'injected cache clear failure');
             END",
        )
        .execute(&pool)
        .await
        .expect("install fault trigger");

        crate::db::queries::settings::clear_status_cache(&pool)
            .await
            .expect_err("the injected status delete failure must abort cache clear");

        let status_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM statuses")
            .fetch_one(&pool)
            .await
            .unwrap();
        let timeline_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_entries")
            .fetch_one(&pool)
            .await
            .unwrap();
        let account_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts")
            .fetch_one(&pool)
            .await
            .unwrap();
        let search_state_after =
            sqlx::query_as::<_, (bool, i64, i64, bool, bool, i64, i64, i64, i64, i64, i64)>(
                "SELECT control.index_updates_enabled,
                    control.merge_debt,
                    control.account_merge_debt,
                    status_backfill.completed,
                    account_backfill.completed,
                    (SELECT COUNT(*) FROM status_search_index_queue),
                    (SELECT COUNT(*) FROM status_search_icu_content),
                    (SELECT COUNT(*) FROM status_search_icu_fts),
                    (SELECT COUNT(*) FROM account_search_index_queue),
                    (SELECT COUNT(*) FROM account_search_icu_content),
                    (SELECT COUNT(*) FROM account_search_icu_fts)
               FROM status_search_index_control control
               JOIN status_search_icu_backfill_state status_backfill
                 ON status_backfill.singleton = control.singleton
               JOIN account_search_icu_backfill_state account_backfill
                 ON account_backfill.singleton = control.singleton
              WHERE control.singleton = 1",
            )
            .fetch_one(&pool)
            .await
            .expect("capture asynchronous search state after rollback fault");
        assert_eq!(status_count, 1);
        assert_eq!(timeline_count, 1);
        assert_eq!(account_count, 1);
        assert_eq!(search_state_after, search_state_before);
    }

    #[tokio::test]
    async fn thousand_status_page_uses_bounded_batch_transactions() {
        use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

        let baseline_path = std::env::temp_dir().join(format!(
            "awayuki-status-baseline-{}.sqlite3",
            uuid::Uuid::new_v4().simple()
        ));
        let baseline_options = SqliteConnectOptions::new()
            .filename(&baseline_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("foreign_keys", "ON");
        let baseline_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _metadata| {
                Box::pin(crate::db::short_search_tokenizer::register(connection))
            })
            .connect_with(baseline_options)
            .await
            .expect("open baseline fixture database");
        apply_status_batch_fixture_schema(&baseline_pool).await;
        apply_async_search_write_fixture_schema(&baseline_pool).await;

        let path = std::env::temp_dir().join(format!(
            "awayuki-status-batch-{}.sqlite3",
            uuid::Uuid::new_v4().simple()
        ));
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("foreign_keys", "ON");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _metadata| {
                Box::pin(crate::db::short_search_tokenizer::register(connection))
            })
            .connect_with(options)
            .await
            .expect("open fixture database");
        apply_status_batch_fixture_schema(&pool).await;
        apply_async_search_write_fixture_schema(&pool).await;

        let statuses = (0..1_000).map(fixture_status).collect::<Vec<_>>();
        let context = BatchTimeline {
            timeline_type: "home",
            account_acct: "alice@example.test",
        };
        let items = statuses
            .iter()
            .map(|status| StatusBatchItem {
                status,
                timeline: Some(context),
                viewer_acct: Some(context.account_acct),
            })
            .collect::<Vec<_>>();

        let baseline_started_at = Instant::now();
        let mut baseline_metrics = BatchPersistMetrics::default();
        for item in &items {
            let metrics = save_status_items_measured(
                &baseline_pool,
                std::slice::from_ref(item),
                "example.test",
            )
            .await
            .expect("persist one baseline status");
            baseline_metrics.statuses += metrics.statuses;
            baseline_metrics.transactions += metrics.transactions;
            baseline_metrics.account_upserts += metrics.account_upserts;
            baseline_metrics.status_upserts += metrics.status_upserts;
            baseline_metrics.tag_upserts += metrics.tag_upserts;
            baseline_metrics.timeline_inserts += metrics.timeline_inserts;
        }
        baseline_metrics.elapsed_ms = elapsed_ms(baseline_started_at);
        let baseline_wal_path = baseline_path.with_extension("sqlite3-wal");
        let baseline_wal_bytes = std::fs::metadata(&baseline_wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        let mut commit_notifications = 0usize;
        let mut observe_commit = || commit_notifications += 1;
        let metrics = save_status_items_measured_with_commit_observer(
            &pool,
            &items,
            "example.test",
            &mut observe_commit,
        )
        .await
        .expect("persist fixture page");
        let status_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM statuses")
            .fetch_one(&pool)
            .await
            .expect("count statuses");
        let timeline_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM timeline_entries")
            .fetch_one(&pool)
            .await
            .expect("count entries");
        let queued_search_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM status_search_index_queue")
                .fetch_one(&pool)
                .await
                .expect("count asynchronous search queue");
        let queued_account_search_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM account_search_index_queue")
                .fetch_one(&pool)
                .await
                .expect("count asynchronous account search queue");
        let synchronous_icu_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM status_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count synchronous ICU writes");
        let synchronous_account_icu_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM account_search_icu_content")
                .fetch_one(&pool)
                .await
                .expect("count synchronous account ICU writes");
        let wal_path = path.with_extension("sqlite3-wal");
        let wal_bytes = std::fs::metadata(&wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        assert_eq!(status_count, 1_000);
        assert_eq!(timeline_count, 1_000);
        assert_eq!(queued_search_count, 1_000);
        assert_eq!(queued_account_search_count, 1);
        assert_eq!(synchronous_icu_count, 0);
        assert_eq!(synchronous_account_icu_count, 0);
        assert_eq!(metrics.statuses, 1_000);
        assert!(metrics.transactions < 1_000);
        assert_eq!(commit_notifications, metrics.transactions);
        assert!(metrics.max_transaction_statuses <= DB_BATCH_MAX_STATUSES);
        assert_eq!(metrics.account_upserts, 1);
        assert_eq!(metrics.tag_upserts, 10);
        assert!(metrics.statement_count() < 4_000);
        assert_eq!(baseline_metrics.transactions, 1_000);
        assert!(metrics.transactions < baseline_metrics.transactions);
        assert!(metrics.statement_count() < baseline_metrics.statement_count());
        eprintln!(
            "status_batch before={{commits:{},statements:{},wall_ms:{},wal_bytes:{}}} after={{commits:{},statements:{},wall_ms:{},wal_bytes:{wal_bytes}}}",
            baseline_metrics.transactions,
            baseline_metrics.statement_count(),
            baseline_metrics.elapsed_ms,
            baseline_wal_bytes,
            metrics.transactions,
            metrics.statement_count(),
            metrics.elapsed_ms,
        );

        baseline_pool.close().await;
        pool.close().await;
        let _ = std::fs::remove_file(&baseline_path);
        let _ = std::fs::remove_file(baseline_path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(baseline_wal_path);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
        let _ = std::fs::remove_file(wal_path);
    }

    #[tokio::test]
    async fn quote_timeout_is_not_part_of_initial_timeline_latency() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        let client = ApiClient::mastodon_with_kind(
            crate::mastodon::client::MastodonClient::new(
                "127.0.0.1:9",
                "token".to_string(),
                "wss://127.0.0.1:9".to_string(),
            )
            .expect("build client"),
            ServerKind::Mastodon,
        );
        let acct = format!("latency-test-{}", uuid::Uuid::new_v4().simple());
        let mut unresolved = fixture_status(1);
        unresolved.id = acct.clone();
        unresolved.quote_id = Some("quote-that-times-out".to_string());
        let permits = background_quote_semaphore()
            .acquire_many_owned(BACKGROUND_QUOTE_CONCURRENCY as u32)
            .await
            .expect("reserve quote workers for deterministic scheduling test");

        let started_at = Instant::now();
        schedule_pending_quote_resolution(
            &client,
            &pool,
            std::slice::from_ref(&unresolved),
            "127.0.0.1:9",
            &acct,
        );
        schedule_pending_quote_resolution(
            &client,
            &pool,
            std::slice::from_ref(&unresolved),
            "127.0.0.1:9",
            &acct,
        );
        assert!(
            started_at.elapsed() < Duration::from_millis(100),
            "scheduling quote work blocked the initial path"
        );
        {
            let registry = background_quote_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                registry
                    .account_jobs
                    .get(&("127.0.0.1:9".to_string(), acct.clone()))
                    .map(HashSet::len),
                Some(1),
                "canonical quote identity must deduplicate background jobs"
            );
        }
        cancel_pending_quote_resolution("127.0.0.1:9", &acct);
        {
            let registry = background_quote_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(!registry
                .account_jobs
                .contains_key(&("127.0.0.1:9".to_string(), acct)));
        }
        drop(permits);
    }

    #[tokio::test]
    async fn quote_consumer_cancel_is_reference_counted_and_preserves_account_jobs() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open fixture database");
        let client = ApiClient::mastodon_with_kind(
            crate::mastodon::client::MastodonClient::new(
                "127.0.0.1:9",
                "token".to_string(),
                "wss://127.0.0.1:9".to_string(),
            )
            .expect("build client"),
            ServerKind::Mastodon,
        );
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let account = format!("quote-owner-{suffix}");
        let first_consumer = format!("column-first-{suffix}");
        let second_consumer = format!("column-second-{suffix}");
        let third_consumer = format!("column-third-{suffix}");
        let quote_id = format!("quote-{suffix}");
        let key = format!("127.0.0.1:9\0{account}\0id:{quote_id}");
        let mut unresolved = fixture_status(1);
        unresolved.id = format!("status-{suffix}");
        unresolved.quote_id = Some(quote_id);
        let permits = background_quote_semaphore()
            .acquire_many_owned(BACKGROUND_QUOTE_CONCURRENCY as u32)
            .await
            .expect("reserve quote workers for ownership test");

        schedule_pending_quote_resolution_for_consumer(
            &client,
            &pool,
            std::slice::from_ref(&unresolved),
            "127.0.0.1:9",
            &account,
            &first_consumer,
        );
        schedule_pending_quote_resolution_for_consumer(
            &client,
            &pool,
            std::slice::from_ref(&unresolved),
            "127.0.0.1:9",
            &account,
            &second_consumer,
        );
        schedule_pending_quote_resolution(
            &client,
            &pool,
            std::slice::from_ref(&unresolved),
            "127.0.0.1:9",
            &account,
        );

        assert!(cancel_pending_quote_consumer(&first_consumer));
        assert!(cancel_pending_quote_consumer(&second_consumer));
        {
            let registry = background_quote_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let job = registry.jobs.get(&key).expect("account-owned job remains");
            assert_eq!(job.owners.len(), 1);
            assert!(job.owners.contains(&BackgroundQuoteOwner::Account(
                "127.0.0.1:9".to_string(),
                account.clone(),
            )));
            assert!(!registry.consumer_jobs.contains_key(&first_consumer));
            assert!(!registry.consumer_jobs.contains_key(&second_consumer));
        }

        let second_quote_id = format!("quote-consumer-only-{suffix}");
        let second_key = format!("127.0.0.1:9\0{account}\0id:{second_quote_id}");
        let mut consumer_only = fixture_status(2);
        consumer_only.id = format!("status-consumer-only-{suffix}");
        consumer_only.quote_id = Some(second_quote_id);
        schedule_pending_quote_resolution_for_consumer(
            &client,
            &pool,
            std::slice::from_ref(&consumer_only),
            "127.0.0.1:9",
            &account,
            &third_consumer,
        );

        cancel_pending_quote_resolution("127.0.0.1:9", &account);
        {
            let registry = background_quote_registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(!registry.jobs.contains_key(&key));
            assert!(!registry.jobs.contains_key(&second_key));
            assert!(!registry.consumer_jobs.contains_key(&third_consumer));
        }
        drop(permits);
    }

    #[test]
    fn quote_retry_budget_is_shared_per_server_not_active_account() {
        let first = background_quote_server_semaphore("shared.example");
        let second = background_quote_server_semaphore("shared.example");
        let other = background_quote_server_semaphore("other.example");

        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &other));
        assert_eq!(
            first.available_permits(),
            BACKGROUND_QUOTE_SERVER_CONCURRENCY
        );
    }
}
