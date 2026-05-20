//! Stop-gap REST polling for Bluesky.
//!
//! AT Protocol's real-time transport (Jetstream) emits the entire firehose and
//! requires client-side filtering against the user's follow graph, which scales
//! poorly with Bluesky's overall traffic. Until a more selective pipeline is in
//! place, we approximate streaming for Bluesky accounts by periodically calling
//! the same REST endpoints panels already use, then re-emitting each fetched
//! status as a `StreamEvent::Update`. The existing event-processor in
//! `streaming_service` then handles DB persistence and panel broadcast — so
//! polled posts flow through exactly the same code path as a true WebSocket
//! event would.
//!
//! Duplicate suppression happens at two layers downstream: SQLite UPSERT in
//! `save_status_to_db`, and panel-side dedup by `status.uri`. That makes
//! overlap with the previous batch harmless, so we can simply re-fetch the
//! latest page each tick instead of tracking a watermark.

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};

use crate::bluesky::client::BlueskyClient;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::{StreamEvent, StreamType};

/// Page size per poll. Large enough to comfortably cover a busy 30-second
/// window even for users with active follow graphs.
const POLL_LIMIT: u32 = 40;

/// Poll one stream type forever, forwarding each fetched status as a
/// `StreamEvent::Update`. Returns when the receiver half of `tx` is dropped
/// (panel teardown / abort handle).
///
/// Unsupported stream types (`Public`, `PublicLocal`, `Direct`,
/// `UserNotification`, …) drop the sender and return immediately so the
/// consumer task in `streaming_service` exits cleanly instead of hanging on
/// an idle channel.
pub async fn run_polling(
    client: BlueskyClient,
    stream_type: StreamType,
    tx: mpsc::UnboundedSender<StreamEvent>,
    poll_interval: Duration,
) {
    if !is_supported(&stream_type) {
        tracing::debug!(
            "Bluesky polling skipped: stream type {:?} has no REST equivalent",
            stream_type
        );
        drop(tx);
        return;
    }

    let label = describe_stream(&stream_type);
    tracing::info!(
        "Bluesky polling started: stream={} interval={}s",
        label,
        poll_interval.as_secs()
    );

    let mut ticker = interval(poll_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut previous_real_status_ids = HashSet::new();

    loop {
        ticker.tick().await;

        if tx.is_closed() {
            tracing::info!(
                "Bluesky polling stopped: receiver dropped (stream={})",
                label
            );
            return;
        }

        match fetch_stream(&client, &stream_type).await {
            Ok(statuses) => {
                let count = statuses.len();
                let current_real_status_ids = statuses
                    .iter()
                    .filter_map(real_status_id)
                    .collect::<HashSet<_>>();
                for missing_id in previous_real_status_ids.difference(&current_real_status_ids) {
                    match client.get_status(missing_id).await {
                        Ok(_) => {}
                        Err(error) if is_not_found_error(&error) => {
                            if tx.send(StreamEvent::Delete(missing_id.clone())).is_err() {
                                tracing::info!(
                                    "Bluesky polling stopped: receiver dropped during delete emit (stream={})",
                                    label
                                );
                                return;
                            }
                        }
                        Err(error) => {
                            tracing::debug!(
                                "Bluesky poll: could not verify missing status {} (stream={}): {}",
                                missing_id,
                                label,
                                error
                            );
                        }
                    }
                }
                previous_real_status_ids = current_real_status_ids;

                // Emit oldest first so the panel's insert-at-front behaviour
                // leaves the newest post at the top of the timeline.
                for status in statuses.into_iter().rev() {
                    let payload = match serde_json::to_string(&status) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!("Bluesky polling: failed to encode status: {}", e);
                            continue;
                        }
                    };
                    if tx.send(StreamEvent::Update(payload)).is_err() {
                        tracing::info!(
                            "Bluesky polling stopped: receiver dropped mid-batch (stream={})",
                            label
                        );
                        return;
                    }
                }
                tracing::debug!("Bluesky poll: stream={} fetched={}", label, count);
            }
            Err(e) => {
                tracing::warn!("Bluesky poll failed (stream={}): {}", label, e);
            }
        }
    }
}

fn real_status_id(status: &Status) -> Option<String> {
    status.id.starts_with("at://").then(|| status.id.clone())
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

fn is_supported(stream_type: &StreamType) -> bool {
    matches!(
        stream_type,
        StreamType::User | StreamType::List(_) | StreamType::Hashtag(_)
    )
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

fn describe_stream(s: &StreamType) -> String {
    match s {
        StreamType::User => "user".to_string(),
        StreamType::List(id) => format!("list:{}", id),
        StreamType::Hashtag(tag) => format!("hashtag:#{}", tag),
        other => format!("{:?}", other),
    }
}
