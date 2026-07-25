use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;
use crate::bluesky::streaming::run_polling as run_bluesky_polling;
use crate::db::pool::Database;
use crate::mastodon::streaming::run_streaming;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::{StreamEvent, StreamType};
use crate::misskey::streaming::run_streaming as run_misskey_streaming;
use crate::services::timeline_service::{self, BatchTimeline, StatusBatchItem};

const RAW_STREAM_QUEUE_CAPACITY: usize = 256;
const EVENT_MICRO_BATCH_CAPACITY: usize = 64;
const EVENT_MICRO_BATCH_WINDOW: Duration = Duration::from_millis(25);
const PERSISTENCE_QUEUE_CAPACITY: usize = 64;
const QUOTE_JOB_QUEUE_CAPACITY: usize = 64;
const QUOTE_JOB_CONCURRENCY: usize = 4;
const QUOTE_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const QUOTE_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
const QUOTE_MAX_ATTEMPTS: usize = 3;

/// Events that affect a timeline's displayed statuses.
#[derive(Debug, Clone)]
pub enum TimelineEvent {
    NewStatus(Box<Status>, StreamType, String, String, StreamPosition),
    StatusUpdate(Box<Status>, String, String, StreamPosition),
    QuoteUpdate(
        Box<Status>,
        timeline_service::QuoteResolutionState,
        String,
        String,
        StreamPosition,
    ),
    DeleteStatus(String, String, String, StreamPosition),
    NewNotification(
        Box<Notification>,
        StreamType,
        String,
        String,
        StreamPosition,
    ),
    CacheCommitted(String, String),
    Resync(String, String, StreamPosition),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPosition {
    pub generation: u64,
    pub sequence: u64,
}

#[derive(Debug)]
struct StreamClock {
    generation: AtomicU64,
    sequence: AtomicU64,
}

impl StreamClock {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(1),
            sequence: AtomicU64::new(0),
        }
    }

    fn next(&self) -> StreamPosition {
        StreamPosition {
            generation: self.generation.load(Ordering::Acquire),
            sequence: self.sequence.fetch_add(1, Ordering::AcqRel) + 1,
        }
    }

    fn resync(&self) -> StreamPosition {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.sequence.store(0, Ordering::Release);
        StreamPosition {
            generation,
            sequence: 0,
        }
    }
}

/// All account-scoped inputs required to start a streaming session.
pub struct StreamingConfig {
    pub client: ApiClient,
    pub streaming_url: String,
    pub access_token: String,
    pub stream_types: Vec<StreamType>,
    pub server_domain: String,
    pub server_kind: ServerKind,
    pub source_acct: String,
    pub database: Arc<Database>,
    pub event_txs: Vec<mpsc::Sender<TimelineEvent>>,
    pub bluesky_poll_interval: Duration,
}

#[derive(Debug, Clone)]
enum ProcessedEvent {
    Update(Box<Status>),
    StatusUpdate(Box<Status>),
    Delete(String),
    Notification(Box<Notification>),
    Resync,
}

#[derive(Debug)]
struct PersistenceBatch {
    events: Vec<ProcessedEvent>,
    stream_type: StreamType,
}

fn parse_stream_event(event: StreamEvent) -> Option<ProcessedEvent> {
    match event {
        StreamEvent::Update(payload) => serde_json::from_str::<Status>(&payload)
            .map(|status| ProcessedEvent::Update(Box::new(status)))
            .map_err(|error| tracing::warn!("Failed to parse streaming status: {error}"))
            .ok(),
        StreamEvent::StatusUpdate(payload) => serde_json::from_str::<Status>(&payload)
            .map(|status| ProcessedEvent::StatusUpdate(Box::new(status)))
            .map_err(|error| tracing::warn!("Failed to parse streaming status update: {error}"))
            .ok(),
        StreamEvent::Delete(id) => Some(ProcessedEvent::Delete(id)),
        StreamEvent::Notification(payload) => serde_json::from_str::<Notification>(&payload)
            .map(|notification| ProcessedEvent::Notification(Box::new(notification)))
            .map_err(|error| tracing::warn!("Failed to parse streaming notification: {error}"))
            .ok(),
        StreamEvent::Resync => Some(ProcessedEvent::Resync),
        StreamEvent::FiltersChanged => None,
        StreamEvent::Unknown(event, payload) => {
            tracing::debug!(
                event,
                payload_bytes = payload.len(),
                "Ignoring unknown stream event"
            );
            None
        }
    }
}

#[derive(Debug)]
struct CoalescedBatch {
    events: Vec<Option<ProcessedEvent>>,
    identity_indices: HashMap<String, usize>,
    received: usize,
    coalesced: usize,
    started_at: Instant,
}

impl CoalescedBatch {
    fn new() -> Self {
        Self {
            events: Vec::with_capacity(EVENT_MICRO_BATCH_CAPACITY),
            identity_indices: HashMap::new(),
            received: 0,
            coalesced: 0,
            started_at: Instant::now(),
        }
    }

    fn push(&mut self, raw: StreamEvent) {
        self.received += 1;
        let Some(event) = parse_stream_event(raw) else {
            return;
        };
        let Some(identity) = event_identity(&event) else {
            self.events.push(Some(event));
            return;
        };

        if let Some(index) = self.identity_indices.get(&identity).copied() {
            self.coalesced += 1;
            // A delete is terminal within one micro-batch. This prevents a
            // stale update already queued behind it from resurrecting a row.
            if self.events[index]
                .as_ref()
                .is_some_and(|existing| matches!(existing, ProcessedEvent::Delete(_)))
                && !matches!(event, ProcessedEvent::Delete(_))
            {
                return;
            }
            self.events[index] = Some(event);
        } else {
            self.identity_indices.insert(identity, self.events.len());
            self.events.push(Some(event));
        }
    }

    fn into_events(self) -> Vec<ProcessedEvent> {
        self.events.into_iter().flatten().collect()
    }
}

fn event_identity(event: &ProcessedEvent) -> Option<String> {
    match event {
        ProcessedEvent::Update(status) | ProcessedEvent::StatusUpdate(status) => {
            Some(format!("status:{}", status.id))
        }
        ProcessedEvent::Delete(id) => Some(format!("status:{id}")),
        ProcessedEvent::Notification(notification) => {
            Some(format!("notification:{}", notification.id))
        }
        ProcessedEvent::Resync => None,
    }
}

async fn receive_micro_batch(receiver: &mut mpsc::Receiver<StreamEvent>) -> Option<CoalescedBatch> {
    let first = receiver.recv().await?;
    let mut batch = CoalescedBatch::new();
    batch.push(first);
    let deadline = tokio::time::Instant::now() + EVENT_MICRO_BATCH_WINDOW;

    while batch.received < EVENT_MICRO_BATCH_CAPACITY {
        tokio::select! {
            biased;
            event = receiver.recv() => match event {
                Some(event) => batch.push(event),
                None => break,
            },
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
    Some(batch)
}

#[derive(Debug)]
struct QuoteJob {
    key: String,
    status: Status,
}

#[derive(Debug, Default)]
struct QuoteRegistry {
    in_flight: HashSet<String>,
    negative_until: HashMap<String, Instant>,
}

#[derive(Clone)]
struct QuoteQueue {
    sender: mpsc::Sender<QuoteJob>,
    registry: Arc<Mutex<QuoteRegistry>>,
}

impl QuoteQueue {
    fn enqueue(&self, status: &Status) -> QuoteEnqueueResult {
        let Some(key) = pending_quote_key(status) else {
            return QuoteEnqueueResult::NotNeeded;
        };
        let now = Instant::now();
        {
            let mut registry = lock_registry(&self.registry);
            registry.negative_until.retain(|_, until| *until > now);
            if registry.negative_until.contains_key(&key) {
                return QuoteEnqueueResult::NegativeCached;
            }
            if !registry.in_flight.insert(key.clone()) {
                return QuoteEnqueueResult::Deduplicated;
            }
        }

        match self.sender.try_send(QuoteJob {
            key: key.clone(),
            status: status.clone(),
        }) {
            Ok(()) => QuoteEnqueueResult::Queued,
            Err(_) => {
                lock_registry(&self.registry).in_flight.remove(&key);
                QuoteEnqueueResult::QueueFull
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuoteEnqueueResult {
    Queued,
    Deduplicated,
    NegativeCached,
    QueueFull,
    NotNeeded,
}

#[derive(Clone)]
struct QuoteWorkerContext {
    registry: Arc<Mutex<QuoteRegistry>>,
    client: ApiClient,
    database: Arc<Database>,
    server_domain: String,
    source_acct: String,
    event_txs: Vec<mpsc::Sender<TimelineEvent>>,
    clock: Arc<StreamClock>,
}

fn lock_registry(registry: &Mutex<QuoteRegistry>) -> std::sync::MutexGuard<'_, QuoteRegistry> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn pending_quote_key(status: &Status) -> Option<String> {
    if status.quote.is_some() {
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
        .or_else(|| {
            (status.content.contains("RE:") && status.content.contains("href="))
                .then(|| format!("source:{}", status.id))
        })
}

fn start_quote_worker(
    receiver: mpsc::Receiver<QuoteJob>,
    context: QuoteWorkerContext,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        futures::stream::unfold(receiver, |mut receiver| async move {
            receiver.recv().await.map(|job| (job, receiver))
        })
        .for_each_concurrent(QUOTE_JOB_CONCURRENCY, |job| {
            let context = context.clone();
            async move {
                resolve_quote_job(job, &context).await;
            }
        })
        .await;
    })
}

async fn resolve_quote_job(job: QuoteJob, context: &QuoteWorkerContext) {
    let mut status = job.status;
    let mut resolved = false;
    for attempt in 0..QUOTE_MAX_ATTEMPTS {
        let lookup = timeline_service::hydrate_and_resolve_quotes(
            &context.client,
            std::slice::from_mut(&mut status),
        );
        if tokio::time::timeout(QUOTE_LOOKUP_TIMEOUT, lookup)
            .await
            .is_ok()
            && status.quote.is_some()
        {
            resolved = true;
            break;
        }
        if attempt + 1 < QUOTE_MAX_ATTEMPTS {
            tokio::time::sleep(quote_retry_delay(&job.key, attempt)).await;
        }
    }

    if resolved {
        if let Err(error) = timeline_service::save_status_batch_with_retry(
            context.database.writer(),
            std::slice::from_ref(&status),
            &context.server_domain,
            None,
        )
        .await
        {
            tracing::warn!("Failed to persist resolved quote {}: {error}", job.key);
        } else {
            if !broadcast_event(
                &context.event_txs,
                TimelineEvent::CacheCommitted(
                    context.source_acct.clone(),
                    context.server_domain.clone(),
                ),
            )
            .await
            {
                return;
            }
            broadcast_event(
                &context.event_txs,
                TimelineEvent::StatusUpdate(
                    Box::new(status),
                    context.source_acct.clone(),
                    context.server_domain.clone(),
                    context.clock.next(),
                ),
            )
            .await;
        }
    }

    let mut registry = lock_registry(&context.registry);
    registry.in_flight.remove(&job.key);
    if !resolved {
        registry
            .negative_until
            .insert(job.key, Instant::now() + QUOTE_NEGATIVE_CACHE_TTL);
    }
}

fn quote_retry_delay(key: &str, attempt: usize) -> Duration {
    let hash = key.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    Duration::from_millis(250 * (attempt as u64 + 1) + hash % 250)
}

fn start_persistence_worker(
    database: Arc<Database>,
    mut receiver: mpsc::Receiver<PersistenceBatch>,
    event_txs: Vec<mpsc::Sender<TimelineEvent>>,
    server_domain: String,
    source_acct: String,
    clock: Arc<StreamClock>,
    lagged: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(batch) = receiver.recv().await {
            let changed_cache = batch.events.iter().any(|event| match event {
                ProcessedEvent::Update(_)
                | ProcessedEvent::StatusUpdate(_)
                | ProcessedEvent::Delete(_) => true,
                ProcessedEvent::Notification(notification) => notification.status.is_some(),
                ProcessedEvent::Resync => false,
            });
            loop {
                if persist_event_batch(
                    database.writer(),
                    &batch.events,
                    &batch.stream_type,
                    &server_domain,
                    &source_acct,
                )
                .await
                {
                    break;
                }
                lagged.store(true, Ordering::Release);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }

            if changed_cache
                && !broadcast_event(
                    &event_txs,
                    TimelineEvent::CacheCommitted(source_acct.clone(), server_domain.clone()),
                )
                .await
            {
                return;
            }

            // A full persistence queue means at least one live event was not
            // written. Once the writer has recovered and the retained work is
            // drained, ask the UI to refresh a bounded snapshot from the
            // provider/DB path. Live delivery remains independent throughout.
            if receiver.is_empty() && lagged.swap(false, Ordering::AcqRel) {
                crate::observability::observe_stream_resync();
                let position = clock.resync();
                tracing::warn!(
                    source_acct,
                    server_domain,
                    generation = position.generation,
                    "stream persistence recovered after lag; requesting snapshot resync"
                );
                if !broadcast_event(
                    &event_txs,
                    TimelineEvent::Resync(source_acct.clone(), server_domain.clone(), position),
                )
                .await
                {
                    return;
                }
            }
        }
    })
}

fn enqueue_persistence_batch(
    sender: &mpsc::Sender<PersistenceBatch>,
    lagged: &AtomicBool,
    batch: PersistenceBatch,
) {
    match sender.try_send(batch) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            let first_overflow = !lagged.swap(true, Ordering::AcqRel);
            if first_overflow {
                tracing::warn!(
                    capacity = PERSISTENCE_QUEUE_CAPACITY,
                    "stream persistence queue is full; live delivery continues and snapshot resync will follow recovery"
                );
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            lagged.store(true, Ordering::Release);
            tracing::warn!("stream persistence worker stopped; live delivery continues");
        }
    }
}

/// Start bounded streaming connections and processors for the requested set.
pub fn start_streaming(config: StreamingConfig) -> Vec<tokio::task::AbortHandle> {
    let StreamingConfig {
        client,
        streaming_url,
        access_token,
        stream_types,
        server_domain,
        server_kind,
        source_acct,
        database,
        event_txs,
        bluesky_poll_interval,
    } = config;
    let mut abort_handles = Vec::new();
    let clock = Arc::new(StreamClock::new());

    let mut resolved_quotes = timeline_service::subscribe_quote_resolution_updates();
    let quote_domain = server_domain.clone();
    let quote_acct = source_acct.clone();
    let quote_txs = event_txs.clone();
    let quote_clock = clock.clone();
    let quote_update_task = tokio::spawn(async move {
        loop {
            match resolved_quotes.recv().await {
                Ok(update)
                    if update.server_domain == quote_domain && update.source_acct == quote_acct =>
                {
                    if update.state == timeline_service::QuoteResolutionState::Resolved
                        && !broadcast_event(
                            &quote_txs,
                            TimelineEvent::CacheCommitted(quote_acct.clone(), quote_domain.clone()),
                        )
                        .await
                    {
                        return;
                    }
                    if !broadcast_event(
                        &quote_txs,
                        TimelineEvent::QuoteUpdate(
                            Box::new(update.status),
                            update.state,
                            quote_acct.clone(),
                            quote_domain.clone(),
                            quote_clock.next(),
                        ),
                    )
                    .await
                    {
                        return;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    crate::observability::observe_stream_resync();
                    tracing::warn!(
                        skipped,
                        source_acct = quote_acct,
                        server_domain = quote_domain,
                        "quote update listener lagged; requesting snapshot resync"
                    );
                    if !broadcast_event(
                        &quote_txs,
                        TimelineEvent::Resync(
                            quote_acct.clone(),
                            quote_domain.clone(),
                            quote_clock.resync(),
                        ),
                    )
                    .await
                    {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    abort_handles.push(quote_update_task.abort_handle());

    let (quote_sender, quote_receiver) = mpsc::channel(QUOTE_JOB_QUEUE_CAPACITY);
    let quote_registry = Arc::new(Mutex::new(QuoteRegistry::default()));
    let quote_queue = QuoteQueue {
        sender: quote_sender,
        registry: quote_registry.clone(),
    };
    let quote_worker = start_quote_worker(
        quote_receiver,
        QuoteWorkerContext {
            registry: quote_registry,
            client: client.clone(),
            database: database.clone(),
            server_domain: server_domain.clone(),
            source_acct: source_acct.clone(),
            event_txs: event_txs.clone(),
            clock: clock.clone(),
        },
    );
    abort_handles.push(quote_worker.abort_handle());

    // Persistence is deliberately downstream of live delivery. A busy SQLite
    // writer must never make a healthy provider connection look frozen to the
    // user. One bounded worker per account also avoids competing retry loops
    // for each subscribed column stream.
    let (persistence_sender, persistence_receiver) =
        mpsc::channel::<PersistenceBatch>(PERSISTENCE_QUEUE_CAPACITY);
    let persistence_lagged = Arc::new(AtomicBool::new(false));
    let persistence_worker = start_persistence_worker(
        database.clone(),
        persistence_receiver,
        event_txs.clone(),
        server_domain.clone(),
        source_acct.clone(),
        clock.clone(),
        persistence_lagged.clone(),
    );
    abort_handles.push(persistence_worker.abort_handle());

    for stream_type in stream_types {
        let (ws_tx, mut ws_rx) = mpsc::channel::<StreamEvent>(RAW_STREAM_QUEUE_CAPACITY);
        let url = streaming_url.clone();
        let token = access_token.clone();
        let stream_for_connection = stream_type.clone();
        let host = server_domain.clone();
        let polling_database = database.clone();
        let polling_acct = source_acct.clone();
        let handle = match server_kind {
            ServerKind::Misskey => tokio::spawn(async move {
                run_misskey_streaming(&url, &token, &stream_for_connection, &host, ws_tx).await;
            }),
            ServerKind::Mastodon | ServerKind::Paon => tokio::spawn(async move {
                run_streaming(&url, &token, &stream_for_connection, &host, ws_tx).await;
            }),
            ServerKind::Bluesky => match client.bluesky_polling_client() {
                Some(bluesky) => tokio::spawn(async move {
                    run_bluesky_polling(
                        bluesky,
                        stream_for_connection,
                        ws_tx,
                        bluesky_poll_interval,
                        polling_database,
                        polling_acct,
                    )
                    .await;
                }),
                None => {
                    tracing::warn!(
                        "Bluesky stream requested but client is not BlueskyClient; skipping"
                    );
                    drop(ws_tx);
                    tokio::spawn(async {})
                }
            },
        };
        abort_handles.push(handle.abort_handle());

        let domain = server_domain.clone();
        let acct = source_acct.clone();
        let txs = event_txs.clone();
        let quote_queue = quote_queue.clone();
        let clock = clock.clone();
        let persistence_sender = persistence_sender.clone();
        let persistence_lagged = persistence_lagged.clone();
        let handle = tokio::spawn(async move {
            while let Some(batch) = receive_micro_batch(&mut ws_rx).await {
                let queue_depth = ws_rx.len();
                let received = batch.received;
                let coalesced = batch.coalesced;
                let oldest_age_ms = batch.started_at.elapsed().as_millis();
                let events = batch.into_events();
                let persistence_events = events.clone();
                enqueue_persistence_batch(
                    &persistence_sender,
                    &persistence_lagged,
                    PersistenceBatch {
                        events: persistence_events,
                        stream_type: stream_type.clone(),
                    },
                );

                let mut quote_queue_overflows = 0usize;
                for event in events {
                    let quote_candidate = match &event {
                        ProcessedEvent::Update(status) | ProcessedEvent::StatusUpdate(status) => {
                            Some(status.as_ref())
                        }
                        ProcessedEvent::Notification(notification) => notification.status.as_ref(),
                        ProcessedEvent::Delete(_) | ProcessedEvent::Resync => None,
                    };
                    if quote_candidate.is_some_and(|status| {
                        quote_queue.enqueue(status) == QuoteEnqueueResult::QueueFull
                    }) {
                        quote_queue_overflows += 1;
                    }

                    let keep_running = match event {
                        ProcessedEvent::Update(status) => {
                            broadcast_event(
                                &txs,
                                TimelineEvent::NewStatus(
                                    status,
                                    stream_type.clone(),
                                    acct.clone(),
                                    domain.clone(),
                                    clock.next(),
                                ),
                            )
                            .await
                        }
                        ProcessedEvent::StatusUpdate(status) => {
                            broadcast_event(
                                &txs,
                                TimelineEvent::StatusUpdate(
                                    status,
                                    acct.clone(),
                                    domain.clone(),
                                    clock.next(),
                                ),
                            )
                            .await
                        }
                        ProcessedEvent::Delete(id) => {
                            broadcast_event(
                                &txs,
                                TimelineEvent::DeleteStatus(
                                    id,
                                    acct.clone(),
                                    domain.clone(),
                                    clock.next(),
                                ),
                            )
                            .await
                        }
                        ProcessedEvent::Notification(notification) => {
                            broadcast_event(
                                &txs,
                                TimelineEvent::NewNotification(
                                    notification,
                                    stream_type.clone(),
                                    acct.clone(),
                                    domain.clone(),
                                    clock.next(),
                                ),
                            )
                            .await
                        }
                        ProcessedEvent::Resync => {
                            crate::observability::observe_stream_resync();
                            let position = clock.resync();
                            tracing::info!(
                                source_acct = acct,
                                server_domain = domain,
                                generation = position.generation,
                                "stream generation changed; downstream snapshot resync required"
                            );
                            broadcast_event(
                                &txs,
                                TimelineEvent::Resync(acct.clone(), domain.clone(), position),
                            )
                            .await
                        }
                    };
                    if !keep_running {
                        return;
                    }
                }

                tracing::debug!(
                    received,
                    coalesced,
                    queue_depth,
                    oldest_age_ms,
                    quote_queue_overflows,
                    "processed bounded streaming micro-batch"
                );
                crate::observability::observe_stream_batch(
                    queue_depth,
                    coalesced,
                    quote_queue_overflows,
                );
            }
        });
        abort_handles.push(handle.abort_handle());
    }

    abort_handles
}

async fn persist_event_batch(
    writer: &sqlx::SqlitePool,
    events: &[ProcessedEvent],
    stream_type: &StreamType,
    server_domain: &str,
    source_acct: &str,
) -> bool {
    let timeline_key = timeline_key_for_stream_type(stream_type);
    let timeline = timeline_key.as_deref().map(|timeline_type| BatchTimeline {
        timeline_type,
        account_acct: source_acct,
    });
    let items = events
        .iter()
        .filter_map(|event| match event {
            ProcessedEvent::Update(status) => Some(StatusBatchItem {
                status,
                timeline,
                viewer_acct: Some(source_acct),
            }),
            ProcessedEvent::StatusUpdate(status) => Some(StatusBatchItem {
                status,
                timeline: None,
                viewer_acct: Some(source_acct),
            }),
            ProcessedEvent::Notification(notification) => {
                notification.status.as_ref().map(|status| StatusBatchItem {
                    status,
                    timeline: None,
                    viewer_acct: Some(source_acct),
                })
            }
            ProcessedEvent::Delete(_) | ProcessedEvent::Resync => None,
        })
        .collect::<Vec<_>>();
    if !items.is_empty() {
        if let Err(error) =
            timeline_service::save_status_items_with_retry(writer, &items, server_domain).await
        {
            tracing::warn!("Failed to save streaming status batch: {error}");
            return false;
        }
    }

    for id in events.iter().filter_map(|event| match event {
        ProcessedEvent::Delete(id) => Some(id),
        _ => None,
    }) {
        if let Err(error) =
            timeline_service::delete_status_from_db_with_retry(writer, id, server_domain).await
        {
            tracing::warn!("Failed to delete streaming status {id} on {server_domain}: {error}");
            return false;
        }
    }

    true
}

fn timeline_key_for_stream_type(stream_type: &StreamType) -> Option<String> {
    match stream_type {
        StreamType::User => Some("home".to_string()),
        StreamType::Public => Some("public".to_string()),
        StreamType::PublicLocal => Some("local".to_string()),
        StreamType::List(id) => Some(format!("list:{id}")),
        StreamType::Hashtag(tag) | StreamType::HashtagLocal(tag) => Some(format!("tag:{tag}")),
        StreamType::UserNotification | StreamType::PublicRemote | StreamType::Direct => None,
    }
}

/// Send a desktop notification for a Mastodon notification event.
pub(crate) fn send_desktop_notification(notification: &Notification) {
    let display_name = &notification.account.display_name;
    let acct = &notification.account.acct;

    let title = match &notification.notification_type {
        NotificationType::Mention => format!("{} (@{}) mentioned you", display_name, acct),
        NotificationType::Reblog => format!("{} (@{}) boosted your post", display_name, acct),
        NotificationType::Favourite => format!("{} (@{}) favorited your post", display_name, acct),
        NotificationType::Follow => format!("{} (@{}) followed you", display_name, acct),
        NotificationType::FollowRequest => {
            format!("{} (@{}) requested to follow you", display_name, acct)
        }
        NotificationType::Poll => "A poll has ended".to_string(),
        NotificationType::Status => format!("{} (@{}) posted", display_name, acct),
        NotificationType::Update => format!("{} (@{}) edited a post", display_name, acct),
        _ => format!("Notification from {} (@{})", display_name, acct),
    };

    let body = notification
        .status
        .as_ref()
        .map(|status| strip_html_tags(&status.content))
        .unwrap_or_default();

    if let Err(error) = notify_rust::Notification::new()
        .summary(&title)
        .body(&body)
        .sound_name("Default")
        .show()
    {
        tracing::warn!("Failed to send desktop notification: {error}");
    }
}

fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for character in html.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(character),
            _ => {}
        }
    }
    result
}

async fn broadcast_event(senders: &[mpsc::Sender<TimelineEvent>], event: TimelineEvent) -> bool {
    let mut alive = 0;
    for sender in senders {
        if sender.send(event.clone()).await.is_ok() {
            alive += 1;
        }
    }
    alive > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: &str, content: &str) -> Status {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "uri": id,
            "created_at": "2026-01-01T00:00:00Z",
            "account": {
                "id": "account-1",
                "username": "alice",
                "acct": "alice@example.test",
                "url": "https://example.test/@alice",
                "created_at": "2025-01-01T00:00:00Z"
            },
            "content": content
        }))
        .expect("valid status fixture")
    }

    fn raw_update(status: &Status) -> StreamEvent {
        StreamEvent::Update(serde_json::to_string(status).expect("serializable status"))
    }

    #[test]
    fn coalesces_synthetic_burst_by_status_identity() {
        let mut batch = CoalescedBatch::new();
        for revision in 0..1_000 {
            batch.push(raw_update(&status(
                "status-1",
                &format!("revision-{revision}"),
            )));
        }
        assert_eq!(batch.received, 1_000);
        assert_eq!(batch.coalesced, 999);
        let events = batch.into_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProcessedEvent::Update(status) if status.content == "revision-999"
        ));
    }

    #[test]
    fn delete_wins_over_stale_update_in_same_batch() {
        let mut batch = CoalescedBatch::new();
        batch.push(raw_update(&status("status-1", "before")));
        batch.push(StreamEvent::Delete("status-1".to_string()));
        batch.push(raw_update(&status("status-1", "stale")));
        assert!(matches!(
            batch.into_events().as_slice(),
            [ProcessedEvent::Delete(id)] if id == "status-1"
        ));
    }

    #[tokio::test]
    async fn raw_queue_has_hard_capacity() {
        let (sender, _receiver) = mpsc::channel(RAW_STREAM_QUEUE_CAPACITY);
        for index in 0..RAW_STREAM_QUEUE_CAPACITY {
            sender
                .try_send(StreamEvent::Delete(index.to_string()))
                .expect("within capacity");
        }
        assert!(matches!(
            sender.try_send(StreamEvent::Delete("overflow".to_string())),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    #[tokio::test]
    async fn quote_jobs_are_deduplicated_and_bounded() {
        let (sender, _receiver) = mpsc::channel(1);
        let queue = QuoteQueue {
            sender,
            registry: Arc::new(Mutex::new(QuoteRegistry::default())),
        };
        let mut quoted = status("source-1", "quote");
        quoted.quote_id = Some("quote-1".to_string());
        assert_eq!(queue.enqueue(&quoted), QuoteEnqueueResult::Queued);
        assert_eq!(queue.enqueue(&quoted), QuoteEnqueueResult::Deduplicated);

        let mut second = status("source-2", "quote");
        second.quote_id = Some("quote-2".to_string());
        assert_eq!(queue.enqueue(&second), QuoteEnqueueResult::QueueFull);
    }

    #[tokio::test]
    async fn full_persistence_queue_does_not_block_live_delivery() {
        let (persistence_sender, _persistence_receiver) = mpsc::channel(1);
        persistence_sender
            .try_send(PersistenceBatch {
                events: Vec::new(),
                stream_type: StreamType::User,
            })
            .expect("fill persistence queue");
        let lagged = AtomicBool::new(false);
        enqueue_persistence_batch(
            &persistence_sender,
            &lagged,
            PersistenceBatch {
                events: vec![ProcessedEvent::Update(Box::new(status(
                    "status-queued",
                    "queued",
                )))],
                stream_type: StreamType::User,
            },
        );
        assert!(lagged.load(Ordering::Acquire));

        let (live_sender, mut live_receiver) = mpsc::channel(1);
        let delivered = broadcast_event(
            &[live_sender],
            TimelineEvent::NewStatus(
                Box::new(status("status-live", "live")),
                StreamType::User,
                "alice@example.test".to_string(),
                "example.test".to_string(),
                StreamPosition {
                    generation: 1,
                    sequence: 1,
                },
            ),
        )
        .await;
        assert!(delivered);
        let event = tokio::time::timeout(Duration::from_millis(50), live_receiver.recv())
            .await
            .expect("live delivery must not wait for SQLite")
            .expect("live event");
        assert!(matches!(
            event,
            TimelineEvent::NewStatus(status, StreamType::User, _, _, _)
                if status.id == "status-live"
        ));
    }

    #[tokio::test]
    async fn persistence_recovery_emits_a_new_generation_resync() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open persistence fixture");
        let (event_sender, mut event_receiver) = mpsc::channel(2);
        let (persistence_sender, persistence_receiver) = mpsc::channel(1);
        let lagged = Arc::new(AtomicBool::new(true));
        let clock = Arc::new(StreamClock::new());
        let worker = start_persistence_worker(
            Arc::new(Database::from_test_pools(
                pool.clone(),
                pool.clone(),
                pool.clone(),
            )),
            persistence_receiver,
            vec![event_sender],
            "example.test".to_string(),
            "alice@example.test".to_string(),
            clock,
            lagged.clone(),
        );
        persistence_sender
            .send(PersistenceBatch {
                events: Vec::new(),
                stream_type: StreamType::User,
            })
            .await
            .expect("send recovery marker");

        let event = tokio::time::timeout(Duration::from_millis(100), event_receiver.recv())
            .await
            .expect("resync emitted after recovery")
            .expect("resync event");
        assert!(matches!(
            event,
            TimelineEvent::Resync(
                _,
                _,
                StreamPosition {
                    generation: 2,
                    sequence: 0
                }
            )
        ));
        assert!(!lagged.load(Ordering::Acquire));
        worker.abort();
    }

    #[test]
    fn stream_clock_is_monotonic_and_resets_on_generation_change() {
        let clock = StreamClock::new();
        assert_eq!(
            clock.next(),
            StreamPosition {
                generation: 1,
                sequence: 1,
            }
        );
        assert_eq!(clock.next().sequence, 2);
        assert_eq!(
            clock.resync(),
            StreamPosition {
                generation: 2,
                sequence: 0,
            }
        );
        assert_eq!(
            clock.next(),
            StreamPosition {
                generation: 2,
                sequence: 1,
            }
        );
    }
}
