//! Misskey WebSocket streaming.
//!
//! Misskey's streaming API is one persistent WebSocket per connection. After connect, the
//! client opens "channels" by sending a `connect` frame; events for that channel come back as
//! `channel` frames. We translate those frames into the Mastodon-shaped `StreamEvent` so the
//! rest of the streaming pipeline can stay agnostic.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep_until, timeout, Instant, MissedTickBehavior};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::mastodon::types::streaming::{StreamEvent, StreamType};
use crate::misskey::convert::{note_to_status, notification_to_mastodon};
use crate::misskey::types::note::MisskeyNote;
use crate::misskey::types::notification::MisskeyNotification;

const PING_INTERVAL: Duration = Duration::from_secs(30);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Spawned task body equivalent to the Mastodon `run_streaming`.
///
/// We connect once and open every Misskey channel that the requested `stream_types` map to.
/// `tx` receives `StreamEvent` items the rest of the app already understands.
pub async fn run_streaming(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    local_host: &str,
    tx: mpsc::Sender<StreamEvent>,
) {
    let mut backoff_secs = 1u64;
    let mut reconnect_attempt = 0u64;
    let mut resync_on_connect = false;

    loop {
        crate::services::reconnect_budget::wait_for_server_slot(local_host).await;
        tracing::info!("Connecting to Misskey streaming: {}", streaming_url);

        match connect_once(
            streaming_url,
            access_token,
            stream_type,
            local_host,
            &tx,
            resync_on_connect,
        )
        .await
        {
            Ok(()) => {
                tracing::info!("Misskey streaming connection closed normally");
                backoff_secs = 1;
            }
            Err(e) => {
                tracing::warn!("Misskey streaming connection error: {}", e);
            }
        }
        resync_on_connect = true;

        if tx.is_closed() {
            return;
        }

        let delay = reconnect_delay(backoff_secs, streaming_url, reconnect_attempt);
        reconnect_attempt = reconnect_attempt.saturating_add(1);
        tracing::info!("Reconnecting Misskey streaming in {:?}...", delay);
        tokio::time::sleep(delay).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn connect_once(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    local_host: &str,
    tx: &mpsc::Sender<StreamEvent>,
    resync_on_connect: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/streaming?i={}", streaming_url, access_token);
    let heartbeat_log_url = format!("{}/streaming", streaming_url);
    let request = url.into_client_request()?;
    let (ws_stream, _resp) = timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "stream connect timeout")
        })??;

    if resync_on_connect && tx.send(StreamEvent::Resync).await.is_err() {
        return Ok(());
    }
    let (mut write, mut read) = ws_stream.split();

    // Open the channels we care about for this stream_type.
    // `id` is the per-channel handle Misskey uses to tag incoming events.
    let mut id_to_kind: HashMap<String, ChannelKind> = HashMap::new();
    for (kind, frame) in build_subscribe_frames(stream_type) {
        let id = Uuid::new_v4().to_string();
        let body = serde_json::json!({
            "type": "connect",
            "body": {
                "channel": frame.channel,
                "id": &id,
                "params": frame.params,
            }
        });
        write.send(Message::Text(body.to_string().into())).await?;
        tracing::info!(
            "Misskey channel subscribed: channel={} id={}",
            frame.channel,
            id
        );
        id_to_kind.insert(id, kind);
    }

    let mut ping_interval = interval(PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping_interval.tick().await;

    let far_future = Instant::now() + Duration::from_secs(86400);
    let mut pong_deadline = far_future;
    let mut waiting_for_pong = false;

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if waiting_for_pong {
                            waiting_for_pong = false;
                            pong_deadline = far_future;
                        }
                        if let Some(events) = parse_message(&text, local_host, &id_to_kind) {
                            for event in events {
                                if tx.send(event).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        tracing::debug!(
                            "Received pong response from Misskey streaming server: {}",
                            heartbeat_log_url
                        );
                        waiting_for_pong = false;
                        pong_deadline = far_future;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                        if waiting_for_pong {
                            waiting_for_pong = false;
                            pong_deadline = far_future;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        return Ok(());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        return Err(e.into());
                    }
                    None => {
                        return Ok(());
                    }
                }

                if tx.is_closed() {
                    let _ = write.close().await;
                    return Ok(());
                }
            }

            _ = ping_interval.tick() => {
                tracing::debug!(
                    "Sending ping to Misskey streaming server: {}",
                    heartbeat_log_url
                );
                if let Err(e) = write.send(Message::Ping(vec![].into())).await {
                    tracing::warn!(
                        "Failed to send ping to Misskey streaming server {}: {}",
                        heartbeat_log_url,
                        e
                    );
                    return Err(e.into());
                }
                waiting_for_pong = true;
                pong_deadline = Instant::now() + PONG_TIMEOUT;
            }

            _ = sleep_until(pong_deadline), if waiting_for_pong => {
                tracing::warn!(
                    "Pong timeout for Misskey streaming server {} - connection appears dead, disconnecting",
                    heartbeat_log_url
                );
                let _ = write.close().await;
                return Err("Pong timeout".into());
            }
        }
    }
}

fn reconnect_delay(base_seconds: u64, server: &str, attempt: u64) -> Duration {
    let hash = server
        .bytes()
        .fold(0xcbf29ce484222325u64 ^ attempt, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    Duration::from_secs(base_seconds) + Duration::from_millis(hash % 1_000)
}

#[derive(Debug, Clone, Copy)]
enum ChannelKind {
    /// Channels that surface ordinary timeline notes.
    Timeline,
    /// `main` channel — surfaces notifications and follow events.
    Main,
}

struct ChannelFrame {
    channel: &'static str,
    params: serde_json::Value,
}

fn build_subscribe_frames(stream_type: &StreamType) -> Vec<(ChannelKind, ChannelFrame)> {
    match stream_type {
        StreamType::User => vec![
            (
                ChannelKind::Timeline,
                ChannelFrame {
                    channel: "homeTimeline",
                    params: serde_json::json!({}),
                },
            ),
            (
                ChannelKind::Main,
                ChannelFrame {
                    channel: "main",
                    params: serde_json::json!({}),
                },
            ),
        ],
        StreamType::UserNotification => vec![(
            ChannelKind::Main,
            ChannelFrame {
                channel: "main",
                params: serde_json::json!({}),
            },
        )],
        StreamType::Public => vec![(
            ChannelKind::Timeline,
            ChannelFrame {
                channel: "globalTimeline",
                params: serde_json::json!({}),
            },
        )],
        StreamType::PublicLocal => vec![(
            ChannelKind::Timeline,
            ChannelFrame {
                channel: "localTimeline",
                params: serde_json::json!({}),
            },
        )],
        StreamType::PublicRemote => vec![(
            ChannelKind::Timeline,
            ChannelFrame {
                channel: "globalTimeline",
                params: serde_json::json!({}),
            },
        )],
        StreamType::Hashtag(tag) | StreamType::HashtagLocal(tag) => vec![(
            ChannelKind::Timeline,
            ChannelFrame {
                channel: "hashtag",
                params: serde_json::json!({ "q": [[tag.clone()]] }),
            },
        )],
        StreamType::List(list_id) => vec![(
            ChannelKind::Timeline,
            ChannelFrame {
                channel: "userList",
                params: serde_json::json!({ "listId": list_id.clone() }),
            },
        )],
        StreamType::Direct => vec![],
    }
}

fn parse_message(
    text: &str,
    local_host: &str,
    id_to_kind: &HashMap<String, ChannelKind>,
) -> Option<Vec<StreamEvent>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let msg_type = value.get("type")?.as_str()?;
    if msg_type != "channel" {
        return None;
    }
    let body = value.get("body")?;
    let channel_id = body.get("id")?.as_str()?;
    let kind = id_to_kind.get(channel_id).copied()?;
    let inner_type = body.get("type")?.as_str()?;
    let inner_body = body.get("body")?;

    match (kind, inner_type) {
        (ChannelKind::Timeline, "note") => {
            let note: MisskeyNote = serde_json::from_value(inner_body.clone()).ok()?;
            let status = note_to_status(&note, local_host);
            let payload = serde_json::to_string(&status).ok()?;
            Some(vec![StreamEvent::Update(payload)])
        }
        (ChannelKind::Timeline | ChannelKind::Main, "deleted" | "delete" | "noteDeleted") => {
            extract_deleted_note_id(inner_body).map(|id| vec![StreamEvent::Delete(id)])
        }
        (ChannelKind::Main, "notification") => {
            let notif: MisskeyNotification = serde_json::from_value(inner_body.clone()).ok()?;
            let n = notification_to_mastodon(&notif, local_host)?;
            let payload = serde_json::to_string(&n).ok()?;
            Some(vec![StreamEvent::Notification(payload)])
        }
        (ChannelKind::Main, "mention" | "reply" | "renote" | "quote") => {
            // These also carry a Note payload — surface it so timeline panels see it too.
            let note: MisskeyNote = serde_json::from_value(inner_body.clone()).ok()?;
            let status = note_to_status(&note, local_host);
            let payload = serde_json::to_string(&status).ok()?;
            Some(vec![StreamEvent::Update(payload)])
        }
        _ => None,
    }
}

fn extract_deleted_note_id(value: &serde_json::Value) -> Option<String> {
    if let Some(id) = value.as_str() {
        return Some(id.to_string());
    }

    ["id", "noteId", "deletedNoteId"]
        .iter()
        .find_map(|key| value.get(key)?.as_str().map(ToString::to_string))
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;

    #[test]
    fn reconnect_delay_has_bounded_deterministic_jitter() {
        let first = reconnect_delay(8, "wss://example.test", 3);
        assert_eq!(first, reconnect_delay(8, "wss://example.test", 3));
        assert!(first >= Duration::from_secs(8));
        assert!(first < Duration::from_secs(9));
        assert_ne!(first, reconnect_delay(8, "wss://example.test", 4));
    }
}
