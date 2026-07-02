use std::sync::Arc;
use std::time::Duration;

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
use crate::services::timeline_service;

/// Events that affect a timeline's displayed statuses.
///
/// Each variant carries `(source_acct, server_domain)`: the session's acct
/// (used by panels to remember "who fetched this") plus the source server's
/// hostname (the authoritative routing key for opening detail panels — it
/// uniquely determines `server_kind` via `login_accounts` and survives DB
/// hydrate where `source_acct` would otherwise drift to the panel's primary).
#[derive(Debug, Clone)]
pub enum TimelineEvent {
    NewStatus(Status, StreamType, String, String),
    StatusUpdate(Status, String, String),
    DeleteStatus(String, String, String),
    NewNotification(Notification, StreamType, String, String),
}

/// Maximum number of streaming events whose quote hydration (network I/O)
/// may run concurrently ahead of the sequential persist/broadcast stage.
const QUOTE_HYDRATION_CONCURRENCY: usize = 8;

/// Upper bound on the time spent hydrating quotes for a single streaming
/// event, so a slow quote lookup can't stall the pipeline indefinitely.
const QUOTE_HYDRATION_TIMEOUT: Duration = Duration::from_secs(10);

/// A stream event after parsing and quote hydration, ready for the
/// sequential persist/broadcast stage.
enum ProcessedEvent {
    Update(Status),
    StatusUpdate(Status),
    Delete(String),
    Notification(Notification),
}

/// Parse a raw stream event and hydrate quotes (network I/O). Returns `None`
/// for events that need no further processing (parse failures, unknown
/// events, filter changes).
async fn preprocess_stream_event(
    client: &ApiClient,
    event: StreamEvent,
) -> Option<ProcessedEvent> {
    match event {
        StreamEvent::Update(payload) => match serde_json::from_str::<Status>(&payload) {
            Ok(mut status) => {
                hydrate_quotes_with_timeout(client, &mut status).await;
                Some(ProcessedEvent::Update(status))
            }
            Err(e) => {
                tracing::warn!("Failed to parse streaming status: {}", e);
                None
            }
        },
        StreamEvent::StatusUpdate(payload) => match serde_json::from_str::<Status>(&payload) {
            Ok(mut status) => {
                hydrate_quotes_with_timeout(client, &mut status).await;
                Some(ProcessedEvent::StatusUpdate(status))
            }
            Err(e) => {
                tracing::warn!("Failed to parse streaming status update: {}", e);
                None
            }
        },
        StreamEvent::Delete(id) => Some(ProcessedEvent::Delete(id)),
        StreamEvent::Notification(payload) => {
            match serde_json::from_str::<Notification>(&payload) {
                Ok(mut notification) => {
                    if let Some(status) = notification.status.as_mut() {
                        hydrate_quotes_with_timeout(client, status).await;
                    }
                    Some(ProcessedEvent::Notification(notification))
                }
                Err(e) => {
                    tracing::warn!("Failed to parse streaming notification: {}", e);
                    None
                }
            }
        }
        StreamEvent::FiltersChanged => {
            // TODO: Handle filter changes
            None
        }
        StreamEvent::Unknown(event, _payload) => {
            tracing::debug!("Ignoring unknown stream event: {}", event);
            None
        }
    }
}

async fn hydrate_quotes_with_timeout(client: &ApiClient, status: &mut Status) {
    if tokio::time::timeout(
        QUOTE_HYDRATION_TIMEOUT,
        timeline_service::hydrate_and_resolve_quotes(client, std::slice::from_mut(status)),
    )
    .await
    .is_err()
    {
        tracing::warn!(
            "Timed out hydrating quotes for streaming status {} on {}",
            status.id,
            client.domain()
        );
    }
}

/// Start streaming connections for multiple stream types and forward timeline events to the given sender.
/// Must be called from within a tokio runtime context (e.g., inside Tokio::spawn).
/// Start streaming connections for multiple stream types and broadcast events to all senders.
/// Must be called from within a tokio runtime context (e.g., inside Tokio::spawn).
///
/// Returns abort handles for all spawned tasks so they can be cancelled externally.
///
/// `client` is required because Bluesky has no WebSocket-style streaming yet
/// and we drive it via REST polling against the `BlueskyClient`'s authenticated
/// agent (see `bluesky::streaming::run_polling`). For Mastodon/Misskey the
/// client is unused — `streaming_url` and `access_token` cover the WebSocket
/// connect — so the parameter exists purely to give the Bluesky branch access
/// to the same authenticated session the rest of the app already uses.
pub fn start_streaming(
    client: ApiClient,
    streaming_url: String,
    access_token: String,
    stream_types: Vec<StreamType>,
    server_domain: String,
    server_kind: ServerKind,
    source_acct: String,
    database: Arc<Database>,
    event_txs: Vec<futures::channel::mpsc::UnboundedSender<TimelineEvent>>,
    bluesky_poll_interval: Duration,
) -> Vec<tokio::task::AbortHandle> {
    let mut abort_handles = Vec::new();

    for stream_type in stream_types {
        let (ws_tx, ws_rx) = mpsc::unbounded_channel::<StreamEvent>();

        // Spawn the WebSocket connection (runs forever with reconnection)
        let url = streaming_url.clone();
        let token = access_token.clone();
        let st = stream_type.clone();
        let host = server_domain.clone();
        let handle = match server_kind {
            ServerKind::Misskey => tokio::spawn(async move {
                run_misskey_streaming(&url, &token, &st, &host, ws_tx).await;
            }),
            ServerKind::Mastodon | ServerKind::Paon => tokio::spawn(async move {
                run_streaming(&url, &token, &st, ws_tx).await;
            }),
            ServerKind::Bluesky => match client.clone() {
                ApiClient::Bluesky(bsky) => {
                    let st_for_poll = st.clone();
                    tokio::spawn(async move {
                        run_bluesky_polling(bsky, st_for_poll, ws_tx, bluesky_poll_interval).await;
                    })
                }
                _ => {
                    // ServerKind says Bluesky but the client is something
                    // else — caller wired the wrong client. Bail out cleanly
                    // so the event-processor below doesn't hang on an idle
                    // channel.
                    tracing::warn!(
                        "Bluesky stream requested but client is not BlueskyClient; skipping"
                    );
                    drop(ws_tx);
                    tokio::spawn(async {})
                }
            },
        };
        abort_handles.push(handle.abort_handle());

        // Spawn the event processor (tokio side: parse, persist, broadcast).
        //
        // Parsing and quote hydration (network I/O) run concurrently for up to
        // `QUOTE_HYDRATION_CONCURRENCY` events via an order-preserving
        // `buffered` stage, so slow quote lookups don't stall draining the
        // unbounded channel. The sequential stage below only does DB writes
        // and broadcasting.
        let db = database.clone();
        let domain = server_domain.clone();
        let acct = source_acct.clone();
        let txs = event_txs.clone();
        let lookup_client = client.clone();
        let handle = tokio::spawn(async move {
            let events = futures::stream::unfold(ws_rx, |mut rx| async move {
                rx.recv().await.map(|event| (event, rx))
            });
            let processed_events = events
                .map(|event| {
                    let client = lookup_client.clone();
                    async move { preprocess_stream_event(&client, event).await }
                })
                .buffered(QUOTE_HYDRATION_CONCURRENCY);
            futures::pin_mut!(processed_events);
            while let Some(processed) = processed_events.next().await {
                match processed {
                    Some(ProcessedEvent::Update(status)) => {
                        if let Err(e) = timeline_service::save_status_to_db_with_retry(
                            db.writer(),
                            &status,
                            &domain,
                        )
                        .await
                        {
                            tracing::warn!("Failed to save streaming status to DB: {}", e);
                        }
                        if let Some(timeline_key) = timeline_key_for_stream_type(&stream_type) {
                            if let Err(e) = timeline_service::insert_timeline_entry_with_retry(
                                db.writer(),
                                &timeline_key,
                                &domain,
                                &status.id,
                                &acct,
                                &status.created_at.to_rfc3339(),
                            )
                            .await
                            {
                                tracing::warn!("Failed to save streaming timeline entry: {}", e);
                            }
                        }
                        let event = TimelineEvent::NewStatus(
                            status,
                            stream_type.clone(),
                            acct.clone(),
                            domain.clone(),
                        );
                        if !broadcast_event(&txs, event) {
                            return;
                        }
                    }
                    Some(ProcessedEvent::StatusUpdate(status)) => {
                        if let Err(e) = timeline_service::save_status_to_db_with_retry(
                            db.writer(),
                            &status,
                            &domain,
                        )
                        .await
                        {
                            tracing::warn!("Failed to save streaming status update to DB: {}", e);
                        }
                        if !broadcast_event(
                            &txs,
                            TimelineEvent::StatusUpdate(status, acct.clone(), domain.clone()),
                        ) {
                            return;
                        }
                    }
                    Some(ProcessedEvent::Delete(id)) => {
                        match timeline_service::delete_status_from_db_with_retry(
                            db.writer(),
                            &id,
                            &domain,
                        )
                        .await
                        {
                            Ok(rows) => tracing::info!(
                                status_id = id.as_str(),
                                server_domain = domain.as_str(),
                                rows,
                                "[awayuki][streaming] deleted status from DB"
                            ),
                            Err(e) => tracing::warn!(
                                "Failed to delete streaming status {} from DB on {}: {}",
                                id,
                                domain,
                                e
                            ),
                        }
                        if !broadcast_event(
                            &txs,
                            TimelineEvent::DeleteStatus(id, acct.clone(), domain.clone()),
                        ) {
                            return;
                        }
                    }
                    Some(ProcessedEvent::Notification(notification)) => {
                        let event = TimelineEvent::NewNotification(
                            notification,
                            stream_type.clone(),
                            acct.clone(),
                            domain.clone(),
                        );
                        if !broadcast_event(&txs, event) {
                            return;
                        }
                    }
                    None => {}
                }
            }
        });
        abort_handles.push(handle.abort_handle());
    }

    abort_handles
}

fn timeline_key_for_stream_type(stream_type: &StreamType) -> Option<String> {
    match stream_type {
        StreamType::User => Some("home".to_string()),
        StreamType::Public => Some("public".to_string()),
        StreamType::PublicLocal => Some("local".to_string()),
        StreamType::List(id) => Some(format!("list:{}", id)),
        StreamType::Hashtag(tag) => Some(format!("tag:{}", tag)),
        StreamType::HashtagLocal(tag) => Some(format!("tag:{}", tag)),
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
        .map(|s| strip_html_tags(&s.content))
        .unwrap_or_default();

    if let Err(e) = notify_rust::Notification::new()
        .summary(&title)
        .body(&body)
        .sound_name("Default")
        .show()
    {
        tracing::warn!("Failed to send desktop notification: {}", e);
    }
}

/// Strip HTML tags from a string to produce plain text.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result
}

/// Broadcast an event to all panel senders. Returns false if all receivers are dropped.
fn broadcast_event(
    txs: &[futures::channel::mpsc::UnboundedSender<TimelineEvent>],
    event: TimelineEvent,
) -> bool {
    let mut any_alive = false;
    for tx in txs {
        if tx.unbounded_send(event.clone()).is_ok() {
            any_alive = true;
        }
    }
    any_alive
}
