//! Ordered bridge from provider stream events to the main WebView.
//!
//! UI delivery is sequenced before notification persistence. Every payload and
//! side effect retains the source account supplied by the provider stream; the
//! Active account is deliberately outside this boundary.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::{
    desktop_notification_sound, notification_to_view, save_notification_to_db, status_to_view,
    streaming_service, with_source_acct, Database, Notification, QueuedEmitter,
    TimelineCacheCommittedPayload, TimelineEvent, TimelineStreamPayload,
    STREAM_SIDE_EFFECT_QUEUE_CAPACITY, TIMELINE_CACHE_COMMITTED_EVENT, TIMELINE_STREAM_EVENT,
};

pub(super) async fn forward_stream_events(
    emit_queue: QueuedEmitter,
    database: Arc<Database>,
    rx: mpsc::Receiver<TimelineEvent>,
) {
    let (side_effect_tx, side_effect_rx) = mpsc::channel(STREAM_SIDE_EFFECT_QUEUE_CAPACITY);
    // One worker per account bridge preserves notification side-effect order,
    // while the bounded handoff caps memory during a busy SQLite writer. The
    // UI event is emitted before this queue is awaited, and a full queue uses
    // backpressure instead of losing notification persistence.
    let _side_effect_worker = tokio::spawn(run_stream_side_effects(
        database,
        emit_queue.clone(),
        side_effect_rx,
    ));
    forward_stream_events_to_queues(emit_queue, rx, &side_effect_tx).await;
}

#[derive(Debug)]
pub(super) struct StreamSideEffect {
    pub(super) notification: Box<Notification>,
    pub(super) source_acct: String,
    pub(super) server_domain: String,
}

async fn run_stream_side_effects(
    database: Arc<Database>,
    emit_queue: QueuedEmitter,
    mut rx: mpsc::Receiver<StreamSideEffect>,
) {
    while let Some(side_effect) = rx.recv().await {
        let StreamSideEffect {
            notification,
            source_acct,
            server_domain,
        } = side_effect;
        match save_notification_to_db(
            &database,
            &notification,
            &server_domain,
            &source_acct,
            || {
                emit_queue.emit_detached(
                    TIMELINE_CACHE_COMMITTED_EVENT,
                    TimelineCacheCommittedPayload {
                        source_acct: source_acct.clone(),
                        server_domain: server_domain.clone(),
                    },
                    "stream notification cache committed",
                );
            },
        )
        .await
        {
            Ok(()) => {}
            Err(error) => {
                tracing::warn!("Failed to save streaming notification to DB: {}", error);
            }
        }
        if let Some(sound) =
            desktop_notification_sound(&database, &notification, &server_domain).await
        {
            streaming_service::send_desktop_notification(&notification, sound);
        }
    }
}

pub(super) async fn forward_stream_events_to_queues(
    emit_queue: QueuedEmitter,
    mut rx: mpsc::Receiver<TimelineEvent>,
    side_effect_tx: &mpsc::Sender<StreamSideEffect>,
) {
    // Sequence at the single consumer so metadata describes actual UI delivery
    // order even when socket and quote workers enqueue concurrently.
    let mut generation = 1u64;
    let mut sequence = 0u64;
    while let Some(event) = rx.recv().await {
        if let TimelineEvent::CacheCommitted(source_acct, server_domain) = &event {
            emit_queue
                .emit(
                    TIMELINE_CACHE_COMMITTED_EVENT,
                    TimelineCacheCommittedPayload {
                        source_acct: source_acct.clone(),
                        server_domain: server_domain.clone(),
                    },
                    "stream status cache committed",
                )
                .await;
            continue;
        }
        if matches!(&event, TimelineEvent::Resync(..)) {
            generation = generation.saturating_add(1);
            sequence = 0;
        } else {
            sequence = sequence.saturating_add(1);
        }
        let (payload, side_effect) = match event {
            TimelineEvent::NewStatus(
                status,
                stream_type,
                source_acct,
                server_domain,
                _position,
            ) => {
                let status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                (
                    TimelineStreamPayload {
                        kind: "newStatus".to_string(),
                        stream_type: stream_type_key(&stream_type),
                        source_acct,
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    None,
                )
            }
            TimelineEvent::StatusUpdate(status, source_acct, server_domain, _position) => {
                let status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                (
                    TimelineStreamPayload {
                        kind: "statusUpdate".to_string(),
                        stream_type: "status.update".to_string(),
                        source_acct,
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    None,
                )
            }
            TimelineEvent::QuoteUpdate(
                status,
                quote_state,
                source_acct,
                server_domain,
                _position,
            ) => {
                let mut status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                status.quote_state = Some(quote_state.as_str().to_string());
                (
                    TimelineStreamPayload {
                        kind: "statusUpdate".to_string(),
                        stream_type: "quote.update".to_string(),
                        source_acct,
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    None,
                )
            }
            TimelineEvent::DeleteStatus(status_id, source_acct, server_domain, _position) => (
                TimelineStreamPayload {
                    kind: "deleteStatus".to_string(),
                    stream_type: "delete".to_string(),
                    source_acct,
                    server_domain,
                    status: None,
                    status_id: Some(status_id),
                    generation,
                    sequence,
                },
                None,
            ),
            TimelineEvent::NewNotification(
                notification,
                stream_type,
                source_acct,
                server_domain,
                _position,
            ) => {
                let status =
                    notification_to_view(&notification, &server_domain, Some(&source_acct));
                (
                    TimelineStreamPayload {
                        kind: "newNotification".to_string(),
                        stream_type: stream_type_key(&stream_type),
                        source_acct: source_acct.clone(),
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    Some(StreamSideEffect {
                        notification,
                        source_acct,
                        server_domain,
                    }),
                )
            }
            TimelineEvent::Resync(source_acct, server_domain, _position) => (
                TimelineStreamPayload {
                    kind: "resync".to_string(),
                    stream_type: "resync".to_string(),
                    source_acct,
                    server_domain,
                    status: None,
                    status_id: None,
                    generation,
                    sequence,
                },
                None,
            ),
            TimelineEvent::CacheCommitted(..) => unreachable!("handled before sequencing"),
        };

        emit_queue
            .emit(TIMELINE_STREAM_EVENT, payload, "timeline stream event")
            .await;
        if let Some(side_effect) = side_effect {
            if side_effect_tx.send(side_effect).await.is_err() {
                tracing::warn!(
                    "Streaming side-effect worker stopped; live WebView delivery continues"
                );
            }
        }
    }
}

fn stream_type_key(stream_type: &crate::mastodon::types::streaming::StreamType) -> String {
    use crate::mastodon::types::streaming::StreamType;
    match stream_type {
        StreamType::User => "user".to_string(),
        StreamType::UserNotification => "notification".to_string(),
        StreamType::Public => "public".to_string(),
        StreamType::PublicLocal => "public:local".to_string(),
        StreamType::PublicRemote => "public:remote".to_string(),
        StreamType::Hashtag(tag) => format!("hashtag:{}", tag),
        StreamType::HashtagLocal(tag) => format!("hashtag:local:{}", tag),
        StreamType::List(id) => format!("list:{}", id),
        StreamType::Feed(id) => format!("feed:{}", id),
        StreamType::Direct => "direct".to_string(),
    }
}
