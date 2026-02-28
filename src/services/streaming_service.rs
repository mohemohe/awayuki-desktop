use std::sync::Arc;

use tokio::sync::mpsc;

use crate::db::pool::Database;
use crate::mastodon::streaming::run_streaming;
use crate::mastodon::types::notification::Notification;
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::{StreamEvent, StreamType};
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
pub fn start_streaming(
    streaming_url: String,
    access_token: String,
    stream_types: Vec<StreamType>,
    server_domain: String,
    database: Arc<Database>,
    gpui_txs: Vec<futures::channel::mpsc::UnboundedSender<TimelineEvent>>,
) {
    for stream_type in stream_types {
        let (ws_tx, mut ws_rx) = mpsc::unbounded_channel::<StreamEvent>();

        // Spawn the WebSocket connection (runs forever with reconnection)
        let url = streaming_url.clone();
        let token = access_token.clone();
        let st = stream_type.clone();
        tokio::spawn(async move {
            run_streaming(&url, &token, &st, ws_tx).await;
        });

        // Spawn the event processor (tokio side: parse → DB save → broadcast to all GPUI panels)
        let db = database.clone();
        let domain = server_domain.clone();
        let txs = gpui_txs.clone();
        tokio::spawn(async move {
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
    }
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
