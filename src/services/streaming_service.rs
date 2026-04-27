use std::sync::Arc;

use tokio::sync::mpsc;

use crate::api::kind::ServerKind;
use crate::db::pool::Database;
use crate::mastodon::streaming::run_streaming;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::{StreamEvent, StreamType};
use crate::misskey::streaming::run_streaming as run_misskey_streaming;
use crate::services::timeline_service;

/// Events that affect a timeline's displayed statuses
#[derive(Debug, Clone)]
pub enum TimelineEvent {
    NewStatus(Status, StreamType),
    StatusUpdate(Status),
    DeleteStatus(String),
    NewNotification(Notification, StreamType),
}

/// Start streaming connections for multiple stream types and forward timeline events to the given sender.
/// Must be called from within a tokio runtime context (e.g., inside Tokio::spawn).
/// Start streaming connections for multiple stream types and broadcast events to all senders.
/// Must be called from within a tokio runtime context (e.g., inside Tokio::spawn).
///
/// Returns abort handles for all spawned tasks so they can be cancelled externally.
pub fn start_streaming(
    streaming_url: String,
    access_token: String,
    stream_types: Vec<StreamType>,
    server_domain: String,
    server_kind: ServerKind,
    database: Arc<Database>,
    gpui_txs: Vec<futures::channel::mpsc::UnboundedSender<TimelineEvent>>,
) -> Vec<tokio::task::AbortHandle> {
    let mut abort_handles = Vec::new();

    for stream_type in stream_types {
        let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<StreamEvent>();

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
        };
        abort_handles.push(handle.abort_handle());

        // Spawn the event processor (tokio side: parse → DB save → broadcast to all GPUI panels)
        let db = database.clone();
        let domain = server_domain.clone();
        let txs = gpui_txs.clone();
        let handle = tokio::spawn(async move {
            while let Some(event) = ws_rx.recv().await {
                match event {
                    StreamEvent::Update(payload) => {
                        match serde_json::from_str::<Status>(&payload) {
                            Ok(status) => {
                                if let Err(e) = timeline_service::save_status_to_db(
                                    db.writer(),
                                    &status,
                                    &domain,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        "Failed to save streaming status to DB: {}",
                                        e
                                    );
                                }
                                let event =
                                    TimelineEvent::NewStatus(status, stream_type.clone());
                                if !broadcast_event(&txs, event) {
                                    return;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Failed to parse streaming status: {}", e);
                            }
                        }
                    }
                    StreamEvent::StatusUpdate(payload) => {
                        match serde_json::from_str::<Status>(&payload) {
                            Ok(status) => {
                                if let Err(e) = timeline_service::save_status_to_db(
                                    db.writer(),
                                    &status,
                                    &domain,
                                )
                                .await
                                {
                                    tracing::warn!(
                                        "Failed to save streaming status update to DB: {}",
                                        e
                                    );
                                }
                                if !broadcast_event(&txs, TimelineEvent::StatusUpdate(status))
                                {
                                    return;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse streaming status update: {}",
                                    e
                                );
                            }
                        }
                    }
                    StreamEvent::Delete(id) => {
                        if !broadcast_event(&txs, TimelineEvent::DeleteStatus(id)) {
                            return;
                        }
                    }
                    StreamEvent::Notification(payload) => {
                        match serde_json::from_str::<Notification>(&payload) {
                            Ok(notification) => {
                                let event = TimelineEvent::NewNotification(
                                    notification,
                                    stream_type.clone(),
                                );
                                if !broadcast_event(&txs, event) {
                                    return;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse streaming notification: {}",
                                    e
                                );
                            }
                        }
                    }
                    StreamEvent::FiltersChanged => {
                        // TODO: Handle filter changes
                    }
                    StreamEvent::Unknown(event, _payload) => {
                        tracing::debug!("Ignoring unknown stream event: {}", event);
                    }
                }
            }
        });
        abort_handles.push(handle.abort_handle());
    }

    abort_handles
}

/// Send a desktop notification for a Mastodon notification event.
pub(crate) fn send_desktop_notification(notification: &Notification) {
    let display_name = &notification.account.display_name;
    let acct = &notification.account.acct;

    let title = match notification.notification_type {
        NotificationType::Mention => format!("{} (@{}) mentioned you", display_name, acct),
        NotificationType::Reblog => format!("{} (@{}) boosted your post", display_name, acct),
        NotificationType::Favourite => {
            format!("{} (@{}) favourited your post", display_name, acct)
        }
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
