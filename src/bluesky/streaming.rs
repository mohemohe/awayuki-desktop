//! Revision-aware REST polling for Bluesky.
//!
//! Bluesky does not expose a selective per-user WebSocket equivalent. The
//! polling adapter therefore keeps a bounded revision window, emits only new
//! or changed records, and treats a record leaving the latest page as a
//! reconciliation candidate rather than a deletion.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::time::{sleep, Instant};

use crate::bluesky::client::BlueskyClient;
use crate::db::pool::Database;
use crate::db::queries::bluesky_polling;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::notification::Notification;
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::{StreamEvent, StreamType};

const POLL_LIMIT: u32 = 40;
const MAX_TRACKED_REVISIONS: usize = 512;
const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RECONCILIATION_BUDGET: usize = 4;

#[derive(Debug)]
struct RevisionEntry {
    fingerprint: String,
    last_seen_poll: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedRevision {
    id: String,
    fingerprint: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCheckpoint {
    revisions: Vec<PersistedRevision>,
    #[serde(default)]
    reconciliation_queue: Vec<String>,
}

#[derive(Debug, Default)]
struct PollDiff {
    changed: Vec<Status>,
    reconciliation_candidates: usize,
}

#[derive(Debug, Default)]
struct NotificationRevisionState {
    revisions: HashMap<String, RevisionEntry>,
    poll_number: u64,
}

impl NotificationRevisionState {
    fn from_checkpoint(checkpoint: PersistedCheckpoint) -> Self {
        let revisions = checkpoint
            .revisions
            .into_iter()
            .take(MAX_TRACKED_REVISIONS)
            .filter(|revision| !revision.id.trim().is_empty())
            .map(|revision| {
                (
                    revision.id,
                    RevisionEntry {
                        fingerprint: revision.fingerprint,
                        last_seen_poll: 1,
                    },
                )
            })
            .collect();
        Self {
            revisions,
            poll_number: 1,
        }
    }

    fn checkpoint(&self) -> PersistedCheckpoint {
        let revisions = self
            .revisions
            .iter()
            .filter(|(_, revision)| revision.last_seen_poll == self.poll_number)
            .map(|(id, revision)| PersistedRevision {
                id: id.clone(),
                fingerprint: revision.fingerprint.clone(),
            })
            .collect();
        PersistedCheckpoint {
            revisions: sort_persisted_revisions(revisions),
            reconciliation_queue: Vec::new(),
        }
    }

    fn observe_page(&mut self, notifications: Vec<Notification>) -> Vec<Notification> {
        let is_initial_page = self.poll_number == 0;
        self.poll_number = self.poll_number.saturating_add(1);
        let mut changed = Vec::new();

        for notification in notifications {
            let id = notification.id.trim();
            if id.is_empty() {
                continue;
            }
            let fingerprint = notification_revision(&notification);
            let is_changed = self
                .revisions
                .get(id)
                .is_none_or(|entry| entry.fingerprint != fingerprint);
            self.revisions.insert(
                id.to_string(),
                RevisionEntry {
                    fingerprint,
                    last_seen_poll: self.poll_number,
                },
            );
            // startup_sync owns the initial notification snapshot. Treat the
            // first successful poll only as a revision baseline; replaying it
            // here would produce desktop alerts for up to 40 old events.
            if is_changed && !is_initial_page {
                changed.push(notification);
            }
        }

        if self.revisions.len() > MAX_TRACKED_REVISIONS {
            let mut candidates = self
                .revisions
                .iter()
                .map(|(id, revision)| (id.clone(), revision.last_seen_poll))
                .collect::<Vec<_>>();
            candidates.sort_unstable_by_key(|(_, last_seen)| *last_seen);
            let remove_count = candidates.len() - MAX_TRACKED_REVISIONS;
            for (id, _) in candidates.into_iter().take(remove_count) {
                self.revisions.remove(&id);
            }
        }

        changed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollingRoute {
    Status,
    Notification,
}

/// Bounded, protocol-revision state kept for one polling stream.
#[derive(Debug, Default)]
struct PollingRevisionState {
    revisions: HashMap<String, RevisionEntry>,
    reconciliation_queue: VecDeque<String>,
    reconciliation_set: HashSet<String>,
    poll_number: u64,
}

impl PollingRevisionState {
    fn from_checkpoint(checkpoint: PersistedCheckpoint) -> Self {
        let revisions = checkpoint
            .revisions
            .into_iter()
            .take(MAX_TRACKED_REVISIONS)
            .filter(|revision| !revision.id.trim().is_empty())
            .map(|revision| {
                (
                    revision.id,
                    RevisionEntry {
                        fingerprint: revision.fingerprint,
                        last_seen_poll: 1,
                    },
                )
            })
            .collect();
        let mut reconciliation_queue = VecDeque::new();
        let mut reconciliation_set = HashSet::new();
        for id in checkpoint
            .reconciliation_queue
            .into_iter()
            .take(MAX_TRACKED_REVISIONS)
        {
            if !id.trim().is_empty() && reconciliation_set.insert(id.clone()) {
                reconciliation_queue.push_back(id);
            }
        }
        Self {
            revisions,
            reconciliation_queue,
            reconciliation_set,
            poll_number: 1,
        }
    }

    fn checkpoint(&self) -> PersistedCheckpoint {
        let revisions = self
            .revisions
            .iter()
            .filter(|(id, revision)| {
                revision.last_seen_poll == self.poll_number || self.reconciliation_set.contains(*id)
            })
            .map(|(id, revision)| PersistedRevision {
                id: id.clone(),
                fingerprint: revision.fingerprint.clone(),
            })
            .collect();
        PersistedCheckpoint {
            revisions: sort_persisted_revisions(revisions),
            reconciliation_queue: self.reconciliation_queue.iter().cloned().collect(),
        }
    }

    fn observe_page(&mut self, statuses: Vec<Status>) -> PollDiff {
        self.poll_number = self.poll_number.saturating_add(1);
        let current_reconciliation_ids = statuses
            .iter()
            .filter_map(real_status_id)
            .collect::<HashSet<_>>();

        for id in &current_reconciliation_ids {
            self.reconciliation_set.remove(id);
        }
        self.reconciliation_queue
            .retain(|id| self.reconciliation_set.contains(id));

        let previous_poll = self.poll_number.saturating_sub(1);
        let missing = self
            .revisions
            .iter()
            .filter(|(id, revision)| {
                id.starts_with("at://")
                    && revision.last_seen_poll == previous_poll
                    && !current_reconciliation_ids.contains(*id)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in missing {
            self.queue_reconciliation(id);
        }

        let mut changed = Vec::new();
        for status in statuses {
            let Some(id) = revision_status_id(&status) else {
                continue;
            };
            let fingerprint = status_revision(&status);
            let is_changed = self
                .revisions
                .get(&id)
                .is_none_or(|entry| entry.fingerprint != fingerprint);
            self.revisions.insert(
                id,
                RevisionEntry {
                    fingerprint,
                    last_seen_poll: self.poll_number,
                },
            );
            if is_changed {
                changed.push(status);
            }
        }

        self.prune_revision_window();

        PollDiff {
            changed,
            reconciliation_candidates: self.reconciliation_queue.len(),
        }
    }

    fn observe_reconciled(&mut self, status: Status) -> Option<Status> {
        let id = real_status_id(&status)?;
        let fingerprint = status_revision(&status);
        let changed = self
            .revisions
            .get(&id)
            .is_none_or(|entry| entry.fingerprint != fingerprint);
        self.revisions.insert(
            id,
            RevisionEntry {
                fingerprint,
                last_seen_poll: self.poll_number,
            },
        );
        changed.then_some(status)
    }

    fn take_reconciliation_batch(&mut self) -> Vec<String> {
        let mut ids = Vec::with_capacity(RECONCILIATION_BUDGET);
        while ids.len() < RECONCILIATION_BUDGET {
            let Some(id) = self.reconciliation_queue.pop_front() else {
                break;
            };
            self.reconciliation_set.remove(&id);
            ids.push(id);
        }
        ids
    }

    fn requeue_reconciliation(&mut self, id: String) {
        self.queue_reconciliation(id);
    }

    fn mark_deleted(&mut self, id: &str) {
        self.revisions.remove(id);
        self.reconciliation_set.remove(id);
        self.reconciliation_queue.retain(|queued| queued != id);
    }

    fn queue_reconciliation(&mut self, id: String) {
        if self.reconciliation_set.insert(id.clone()) {
            self.reconciliation_queue.push_back(id);
        }
    }

    fn prune_revision_window(&mut self) {
        if self.revisions.len() <= MAX_TRACKED_REVISIONS {
            return;
        }
        let mut candidates = self
            .revisions
            .iter()
            .map(|(id, revision)| (id.clone(), revision.last_seen_poll))
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|(_, last_seen)| *last_seen);
        let remove_count = candidates.len() - MAX_TRACKED_REVISIONS;
        for (id, _) in candidates.into_iter().take(remove_count) {
            self.mark_deleted(&id);
        }
    }
}

/// Poll one stream until its bounded receiver is closed.
pub async fn run_polling(
    client: BlueskyClient,
    stream_type: StreamType,
    tx: mpsc::Sender<StreamEvent>,
    poll_interval: Duration,
    database: Arc<Database>,
    account_acct: String,
) {
    let Some(route) = polling_route(&stream_type) else {
        tracing::debug!(
            "Bluesky polling skipped: stream type {:?} has no REST equivalent",
            stream_type
        );
        return;
    };

    if route == PollingRoute::Notification {
        run_notification_polling(client, tx, poll_interval, database, account_acct).await;
        return;
    }

    let label = describe_stream(&stream_type);
    let mut state = load_checkpoint(&database, &account_acct, &label, true)
        .await
        .map(PollingRevisionState::from_checkpoint)
        .unwrap_or_default();
    let mut next_reconciliation = Instant::now() + RECONCILIATION_INTERVAL;
    let mut recovering_from_error = false;

    tracing::info!(
        "Bluesky revision polling started: stream={} base_interval={}s",
        label,
        poll_interval.as_secs()
    );

    loop {
        if tx.is_closed() {
            return;
        }

        match fetch_stream(&client, &stream_type).await {
            Ok(statuses) => {
                if recovering_from_error && tx.send(StreamEvent::Resync).await.is_err() {
                    return;
                }
                recovering_from_error = false;
                let fetched = statuses.len();
                let diff = state.observe_page(statuses);
                let changed_count = diff.changed.len();

                // Oldest first: panel insertion at the front leaves newest on top.
                for status in diff.changed.into_iter().rev() {
                    let payload = match serde_json::to_string(&status) {
                        Ok(payload) => payload,
                        Err(error) => {
                            tracing::warn!("Bluesky polling encode failed: {error}");
                            continue;
                        }
                    };
                    if tx.send(StreamEvent::Update(payload)).await.is_err() {
                        return;
                    }
                }
                save_checkpoint(&database, &account_acct, &label, state.checkpoint()).await;

                tracing::debug!(
                    stream = label,
                    fetched,
                    changed = changed_count,
                    reconciliation_candidates = diff.reconciliation_candidates,
                    next_poll_seconds = next_poll_delay(poll_interval).as_secs(),
                    "Bluesky revision poll complete"
                );
            }
            Err(error) => {
                recovering_from_error = true;
                tracing::warn!("Bluesky poll failed (stream={}): {}", label, error);
            }
        }

        if Instant::now() >= next_reconciliation {
            reconcile_missing(&client, &tx, &mut state, &label).await;
            save_checkpoint(&database, &account_acct, &label, state.checkpoint()).await;
            next_reconciliation = Instant::now() + RECONCILIATION_INTERVAL;
        }

        // The per-account setting is a user-visible freshness contract, not a
        // minimum. Revision filtering already removes unchanged DB writes and
        // UI events, so silently stretching 10 seconds to 160 seconds only
        // makes new Bluesky posts appear lost.
        sleep(next_poll_delay(poll_interval)).await;
    }
}

async fn run_notification_polling(
    client: BlueskyClient,
    tx: mpsc::Sender<StreamEvent>,
    poll_interval: Duration,
    database: Arc<Database>,
    account_acct: String,
) {
    const STREAM_KEY: &str = "notification";
    let mut state = load_checkpoint(&database, &account_acct, STREAM_KEY, false)
        .await
        .map(NotificationRevisionState::from_checkpoint)
        .unwrap_or_default();
    let mut recovering_from_error = false;

    tracing::info!(
        "Bluesky notification polling started: base_interval={}s",
        poll_interval.as_secs()
    );

    loop {
        if tx.is_closed() {
            return;
        }

        let params = NotificationParams {
            limit: Some(POLL_LIMIT),
            ..NotificationParams::default()
        };
        match client.get_notifications(&params).await {
            Ok(notifications) => {
                if recovering_from_error && tx.send(StreamEvent::Resync).await.is_err() {
                    return;
                }
                recovering_from_error = false;
                let fetched = notifications.len();
                let changed = state.observe_page(notifications);
                let changed_count = changed.len();
                if !emit_notification_changes(&tx, changed).await {
                    return;
                }
                save_checkpoint(&database, &account_acct, STREAM_KEY, state.checkpoint()).await;
                tracing::debug!(
                    fetched,
                    changed = changed_count,
                    next_poll_seconds = next_poll_delay(poll_interval).as_secs(),
                    "Bluesky notification revision poll complete"
                );
            }
            Err(error) => {
                recovering_from_error = true;
                tracing::warn!("Bluesky notification poll failed: {}", error);
            }
        }

        // Notification freshness follows the same exact per-account setting
        // as the home poller. Revision filtering suppresses unchanged events.
        sleep(next_poll_delay(poll_interval)).await;
    }
}

fn sort_persisted_revisions(mut revisions: Vec<PersistedRevision>) -> Vec<PersistedRevision> {
    revisions.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    revisions
}

async fn load_checkpoint(
    database: &Database,
    account_acct: &str,
    stream_key: &str,
    require_cached_status: bool,
) -> Option<PersistedCheckpoint> {
    match bluesky_polling::load_checkpoint(database.reader(), account_acct, stream_key).await {
        Ok(Some(json)) => match serde_json::from_str::<PersistedCheckpoint>(&json) {
            Ok(mut checkpoint) => {
                if require_cached_status {
                    let ids = checkpoint
                        .revisions
                        .iter()
                        .map(|revision| revision.id.clone())
                        .collect::<Vec<_>>();
                    match bluesky_polling::existing_status_ids(database.reader(), &ids).await {
                        Ok(existing) => {
                            checkpoint
                                .revisions
                                .retain(|revision| existing.contains(&revision.id));
                            checkpoint
                                .reconciliation_queue
                                .retain(|id| existing.contains(id));
                        }
                        Err(error) => {
                            tracing::warn!(account_acct, stream_key, %error, "Failed to validate Bluesky polling checkpoint against cached statuses");
                            return None;
                        }
                    }
                }
                Some(checkpoint)
            }
            Err(error) => {
                tracing::warn!(account_acct, stream_key, %error, "Ignoring invalid Bluesky polling checkpoint");
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(account_acct, stream_key, %error, "Failed to load Bluesky polling checkpoint");
            None
        }
    }
}

async fn save_checkpoint(
    database: &Database,
    account_acct: &str,
    stream_key: &str,
    checkpoint: PersistedCheckpoint,
) {
    let Ok(json) = serde_json::to_string(&checkpoint) else {
        tracing::warn!(
            account_acct,
            stream_key,
            "Failed to encode Bluesky polling checkpoint"
        );
        return;
    };
    match bluesky_polling::save_checkpoint(
        database.writer(),
        account_acct,
        stream_key,
        &json,
        &Utc::now().to_rfc3339(),
    )
    .await
    {
        Ok(writes) => tracing::debug!(
            account_acct,
            stream_key,
            writes,
            "Saved Bluesky polling checkpoint"
        ),
        Err(error) => {
            tracing::warn!(account_acct, stream_key, %error, "Failed to save Bluesky polling checkpoint")
        }
    }
}

async fn emit_notification_changes(
    tx: &mpsc::Sender<StreamEvent>,
    notifications: Vec<Notification>,
) -> bool {
    // Bluesky returns newest first. The UI inserts at the front, so emit the
    // page oldest first and leave the newest notification on top.
    for notification in notifications.into_iter().rev() {
        let payload = match serde_json::to_string(&notification) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!("Bluesky notification encode failed: {error}");
                continue;
            }
        };
        if tx.send(StreamEvent::Notification(payload)).await.is_err() {
            return false;
        }
    }
    true
}

fn next_poll_delay(configured: Duration) -> Duration {
    configured
}

async fn reconcile_missing(
    client: &BlueskyClient,
    tx: &mpsc::Sender<StreamEvent>,
    state: &mut PollingRevisionState,
    label: &str,
) {
    for id in state.take_reconciliation_batch() {
        match client.get_status(&id).await {
            Ok(status) => {
                if let Some(status) = state.observe_reconciled(status) {
                    if let Ok(payload) = serde_json::to_string(&status) {
                        if tx.send(StreamEvent::StatusUpdate(payload)).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(error) if is_not_found_error(&error) => {
                state.mark_deleted(&id);
                if tx.send(StreamEvent::Delete(id)).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::debug!(
                    "Bluesky reconciliation deferred for {} (stream={}): {}",
                    id,
                    label,
                    error
                );
                state.requeue_reconciliation(id);
            }
        }
    }
}

fn status_revision(status: &Status) -> String {
    // The converted status includes the AT URI plus all observable revision
    // inputs (record text, indexed timestamp, CID-derived embeds, counters and
    // viewer state). Keeping the canonical JSON avoids a process-random hash.
    let revision = serde_json::to_string(status).unwrap_or_else(|_| {
        format!(
            "{}:{}:{}:{}:{}",
            status.uri,
            status
                .edited_at
                .map(|date| date.timestamp_millis())
                .unwrap_or(0),
            status.replies_count,
            status.reblogs_count,
            status.favourites_count
        )
    });
    format!("{:x}", Sha256::digest(revision.as_bytes()))
}

fn notification_revision(notification: &Notification) -> String {
    // Notification identity is stable, while the selected fields capture the
    // observable AT notification revision without volatile subject counters
    // causing repeat desktop notifications on every home-feed interaction.
    format!(
        "{}:{}:{}:{}:{}",
        notification.notification_type.as_str(),
        notification.created_at.timestamp_millis(),
        notification.account.id,
        notification
            .status
            .as_ref()
            .map(|status| status.id.as_str())
            .unwrap_or_default(),
        notification
            .status
            .as_ref()
            .and_then(|status| status.edited_at)
            .map(|date| date.timestamp_millis())
            .unwrap_or_default()
    )
}

fn real_status_id(status: &Status) -> Option<String> {
    status.id.starts_with("at://").then(|| status.id.clone())
}

fn revision_status_id(status: &Status) -> Option<String> {
    let id = status.id.trim();
    if !id.is_empty() {
        return Some(id.to_string());
    }
    let uri = status.uri.trim();
    (!uri.is_empty()).then(|| uri.to_string())
}

fn is_not_found_error(error: &MastodonError) -> bool {
    match error {
        MastodonError::Api { status, .. } => matches!(*status, 404 | 410),
        MastodonError::Other(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("not found")
                || message.contains("record not found")
                || message.contains("could not locate record")
        }
        _ => false,
    }
}

fn polling_route(stream_type: &StreamType) -> Option<PollingRoute> {
    match stream_type {
        StreamType::User | StreamType::List(_) | StreamType::Hashtag(_) => {
            Some(PollingRoute::Status)
        }
        StreamType::UserNotification => Some(PollingRoute::Notification),
        // Bluesky has no ActivityPub public timeline. Unified Public must be
        // sourced only from the ActivityPub providers that implement it.
        StreamType::Public
        | StreamType::PublicLocal
        | StreamType::PublicRemote
        | StreamType::HashtagLocal(_)
        | StreamType::Direct => None,
    }
}

async fn fetch_stream(
    client: &BlueskyClient,
    stream_type: &StreamType,
) -> Result<Vec<Status>, MastodonError> {
    let params = TimelineParams {
        max_id: None,
        since_id: None,
        min_id: None,
        limit: Some(POLL_LIMIT),
    };

    match stream_type {
        StreamType::User => client.get_home_timeline(&params).await,
        StreamType::List(id) => client.get_list_timeline(id, &params).await,
        StreamType::Hashtag(tag) => client.get_hashtag_timeline(tag, false, &params).await,
        _ => Ok(Vec::new()),
    }
}

fn describe_stream(stream_type: &StreamType) -> String {
    match stream_type {
        StreamType::User => "user".to_string(),
        StreamType::List(id) => format!("list:{id}"),
        StreamType::Hashtag(tag) => format!("hashtag:#{tag}"),
        other => format!("{other:?}"),
    }
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
                "id": "did:plc:alice",
                "username": "alice.test",
                "acct": "alice.test",
                "url": "https://bsky.app/profile/alice.test",
                "created_at": "2025-01-01T00:00:00Z"
            },
            "content": content
        }))
        .expect("valid fixture")
    }

    fn notification(id: &str, created_at: &str) -> Notification {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "type": "favourite",
            "created_at": created_at,
            "account": {
                "id": "did:plc:bob",
                "username": "bob.test",
                "acct": "bob.test",
                "url": "https://bsky.app/profile/bob.test",
                "created_at": "2025-01-01T00:00:00Z"
            }
        }))
        .expect("valid notification fixture")
    }

    fn decoded_notification(event: StreamEvent) -> Notification {
        let StreamEvent::Notification(payload) = event else {
            panic!("expected notification event");
        };
        serde_json::from_str(&payload).expect("serialized notification")
    }

    #[tokio::test]
    async fn notification_initial_page_is_baseline_then_unchanged_is_quiet_and_new_emits_once() {
        let mut state = NotificationRevisionState::default();
        let (tx, mut rx) = mpsc::channel(8);
        let first_page = vec![
            notification(
                "at://did:plc:bob/app.bsky.feed.like/2",
                "2026-01-01T00:00:02Z",
            ),
            notification(
                "at://did:plc:bob/app.bsky.feed.like/1",
                "2026-01-01T00:00:01Z",
            ),
        ];

        assert!(emit_notification_changes(&tx, state.observe_page(first_page.clone())).await);
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        assert!(emit_notification_changes(&tx, state.observe_page(first_page.clone())).await);
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let mut next_page = vec![notification(
            "at://did:plc:carol/app.bsky.feed.like/3",
            "2026-01-01T00:00:03Z",
        )];
        next_page.extend(first_page);
        assert!(emit_notification_changes(&tx, state.observe_page(next_page)).await);
        assert_eq!(
            decoded_notification(rx.recv().await.expect("new notification event")).id,
            "at://did:plc:carol/app.bsky.feed.like/3"
        );
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn notification_checkpoint_prevents_restart_replay_but_keeps_new_events() {
        let initial = vec![notification(
            "at://did:plc:bob/app.bsky.feed.like/1",
            "2026-01-01T00:00:01Z",
        )];
        let mut before_restart = NotificationRevisionState::default();
        assert!(before_restart.observe_page(initial.clone()).is_empty());
        let encoded = serde_json::to_string(&before_restart.checkpoint()).unwrap();
        let checkpoint = serde_json::from_str(&encoded).unwrap();
        let mut restored = NotificationRevisionState::from_checkpoint(checkpoint);

        assert!(restored.observe_page(initial.clone()).is_empty());
        let mut next = vec![notification(
            "at://did:plc:carol/app.bsky.feed.like/2",
            "2026-01-01T00:00:02Z",
        )];
        next.extend(initial);
        let changed = restored.observe_page(next);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].id, "at://did:plc:carol/app.bsky.feed.like/2");
    }

    #[test]
    fn home_and_notification_use_distinct_polling_routes_and_public_uses_none() {
        assert_eq!(polling_route(&StreamType::User), Some(PollingRoute::Status));
        assert_eq!(
            polling_route(&StreamType::UserNotification),
            Some(PollingRoute::Notification)
        );
        assert_eq!(polling_route(&StreamType::Public), None);
        assert_eq!(
            next_poll_delay(Duration::from_secs(10)),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn unchanged_page_emits_no_revision_events() {
        let mut state = PollingRevisionState::default();
        assert_eq!(
            state
                .observe_page(vec![status(
                    "at://did:plc:alice/app.bsky.feed.post/1",
                    "one"
                )])
                .changed
                .len(),
            1
        );
        assert!(state
            .observe_page(vec![status(
                "at://did:plc:alice/app.bsky.feed.post/1",
                "one"
            )])
            .changed
            .is_empty());
    }

    #[test]
    fn changed_revision_is_emitted_once() {
        let id = "at://did:plc:alice/app.bsky.feed.post/1";
        let mut state = PollingRevisionState::default();
        state.observe_page(vec![status(id, "before")]);
        let diff = state.observe_page(vec![status(id, "after")]);
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].content, "after");
    }

    #[test]
    fn status_checkpoint_restores_revision_and_reconciliation_state() {
        let id = "at://did:plc:alice/app.bsky.feed.post/1";
        let missing = "at://did:plc:alice/app.bsky.feed.post/missing";
        let mut before_restart = PollingRevisionState::default();
        before_restart.observe_page(vec![status(id, "before"), status(missing, "missing")]);
        before_restart.observe_page(vec![status(id, "before")]);
        let encoded = serde_json::to_string(&before_restart.checkpoint()).unwrap();
        let checkpoint = serde_json::from_str(&encoded).unwrap();
        let mut restored = PollingRevisionState::from_checkpoint(checkpoint);

        assert!(restored
            .observe_page(vec![status(id, "before")])
            .changed
            .is_empty());
        assert_eq!(
            restored.take_reconciliation_batch(),
            vec![missing.to_string()]
        );
        let changed = restored.observe_page(vec![status(id, "after")]).changed;
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].content, "after");
    }

    #[test]
    fn window_dropout_is_reconciled_not_deleted() {
        let id = "at://did:plc:alice/app.bsky.feed.post/1";
        let mut state = PollingRevisionState::default();
        state.observe_page(vec![status(id, "one")]);
        let diff = state.observe_page(Vec::new());
        assert!(diff.changed.is_empty());
        assert_eq!(diff.reconciliation_candidates, 1);
        assert_eq!(state.take_reconciliation_batch(), vec![id.to_string()]);
    }

    #[test]
    fn unchanged_pages_emit_no_db_or_ui_work_without_stretching_the_user_interval() {
        let mut state = PollingRevisionState::default();
        let id = "at://did:plc:alice/app.bsky.feed.post/1";
        assert_eq!(state.observe_page(vec![status(id, "one")]).changed.len(), 1);
        for _ in 0..120 {
            assert!(state
                .observe_page(vec![status(id, "one")])
                .changed
                .is_empty());
        }
        assert_eq!(
            next_poll_delay(Duration::from_secs(10)),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn synthetic_reposts_participate_in_revision_diffs() {
        let id = "repost:did:plc:bob:at://did:plc:alice/app.bsky.feed.post/1";
        let mut state = PollingRevisionState::default();
        assert_eq!(
            state.observe_page(vec![status(id, "first")]).changed.len(),
            1
        );
        assert!(state
            .observe_page(vec![status(id, "first")])
            .changed
            .is_empty());
        let changed = state.observe_page(vec![status(id, "revised")]).changed;
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].content, "revised");
    }

    #[test]
    fn tracked_revision_memory_is_bounded() {
        let mut state = PollingRevisionState::default();
        for index in 0..(MAX_TRACKED_REVISIONS + 100) {
            state.observe_page(vec![status(
                &format!("at://did:plc:alice/app.bsky.feed.post/{index}"),
                "post",
            )]);
        }
        assert!(state.revisions.len() <= MAX_TRACKED_REVISIONS);
    }
}
