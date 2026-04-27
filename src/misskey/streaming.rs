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
use tokio::time::{interval, sleep_until, Instant, MissedTickBehavior};
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

/// Spawned task body equivalent to the Mastodon `run_streaming`.
///
/// We connect once and open every Misskey channel that the requested `stream_types` map to.
/// `tx` receives `StreamEvent` items the rest of the app already understands.
pub async fn run_streaming(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    local_host: &str,
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let mut backoff_secs = 1u64;

    loop {
        tracing::info!("Connecting to Misskey streaming: {}", streaming_url);

        match connect_once(streaming_url, access_token, stream_type, local_host, &tx).await {
            Ok(()) => {
                tracing::info!("Misskey streaming connection closed normally");
                backoff_secs = 1;
            }
            Err(e) => {
                tracing::warn!("Misskey streaming connection error: {}", e);
            }
        }

        if tx.is_closed() {
            return;
        }

        tracing::info!("Reconnecting Misskey streaming in {} seconds...", backoff_secs);
        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn connect_once(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    local_host: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/streaming?i={}", streaming_url, access_token);
    let request = url.into_client_request()?;
    let (ws_stream, _resp) = connect_async(request).await?;
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
        write
            .send(Message::Text(body.to_string().into()))
            .await?;
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
                                if tx.send(event).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
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
                if let Err(e) = write.send(Message::Ping(vec![].into())).await {
                    return Err(e.into());
                }
                waiting_for_pong = true;
                pong_deadline = Instant::now() + PONG_TIMEOUT;
            }

            _ = sleep_until(pong_deadline), if waiting_for_pong => {
                let _ = write.close().await;
                return Err("Pong timeout".into());
            }
        }
    }
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
