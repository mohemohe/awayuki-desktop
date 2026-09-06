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
use crate::services::reconnect_budget::ReconnectBackoff;

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
    account: &str,
) {
    let status =
        crate::services::websocket_status::Connection::register(account, local_host, stream_type);
    let mut reconnect_backoff = ReconnectBackoff::default();
    let mut resync_on_connect = false;

    loop {
        crate::services::reconnect_budget::wait_for_server_slot(local_host).await;
        tracing::info!("Connecting to Misskey streaming: {}", streaming_url);

        status.connecting();
        let attempt = async {
            match connect_once(
                streaming_url,
                access_token,
                stream_type,
                local_host,
                &tx,
                resync_on_connect,
                &mut reconnect_backoff,
                &status,
            )
            .await
            {
                Ok(()) => {
                    tracing::info!("Misskey streaming connection closed normally");
                }
                Err(e) => {
                    tracing::warn!("Misskey streaming connection error: {}", e);
                }
            }
            status.disconnected();

            if tx.is_closed() {
                return;
            }

            let delay = reconnect_backoff.next_delay(streaming_url);
            tracing::info!("Reconnecting Misskey streaming in {:?}...", delay);
            tokio::time::sleep(delay).await;
        };
        tokio::select! {
            _ = attempt => {},
            _ = status.reconnect_requested() => {},
        }
        status.disconnected();
        resync_on_connect = true;
        if tx.is_closed() {
            return;
        }
    }
}

async fn connect_once(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    local_host: &str,
    tx: &mpsc::Sender<StreamEvent>,
    resync_on_connect: bool,
    reconnect_backoff: &mut ReconnectBackoff,
    status: &crate::services::websocket_status::Connection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = format!("{}/streaming?i={}", streaming_url, access_token);
    let heartbeat_log_url = format!("{}/streaming", streaming_url);
    let request = url.into_client_request()?;
    let (ws_stream, _resp) = timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "stream connect timeout")
        })??;

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

    // The WebSocket and all requested channels are established. A later
    // disconnect is a new outage, not another failure in the old retry chain.
    reconnect_backoff.reset();
    status.connected();

    if resync_on_connect && tx.send(StreamEvent::Resync).await.is_err() {
        return Ok(());
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
                    Some(Ok(Message::Pong(data))) => {
                        status.pong_received(&data);
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
                let payload = uuid::Uuid::new_v4().as_bytes().to_vec();
                if let Err(e) = write.send(Message::Ping(payload.clone().into())).await {
                    tracing::warn!(
                        "Failed to send ping to Misskey streaming server {}: {}",
                        heartbeat_log_url,
                        e
                    );
                    return Err(e.into());
                }
                status.ping_sent(payload);
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
        StreamType::Feed(_) | StreamType::Direct => vec![],
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
mod websocket_status_tests {
    use super::*;
    use crate::services::websocket_status;
    use tokio::net::TcpListener;

    async fn wait_for_status(
        account: &str,
        predicate: impl Fn(&websocket_status::WebSocketStatus) -> bool,
    ) -> websocket_status::WebSocketStatus {
        timeout(Duration::from_secs(5), async {
            loop {
                if let Some(status) = websocket_status::snapshot()
                    .into_iter()
                    .find(|status| status.account == account && predicate(status))
                {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("WebSocket status should reach the expected state")
    }

    #[tokio::test]
    async fn real_websocket_heartbeat_reconnect_and_cleanup() {
        let _guard = websocket_status::TEST_LOCK.read().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("ws://{address}");
        let account = format!("misskey-{}@test.invalid", uuid::Uuid::new_v4());
        let (accepted_tx, mut accepted_rx) = mpsc::channel(2);
        let (ping_tx, ping_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            accepted_tx.send(()).await.unwrap();
            while let Some(frame) = socket.next().await {
                if let Message::Ping(payload) = frame.unwrap() {
                    assert!(
                        !payload.is_empty(),
                        "latency uses a correlated ping payload"
                    );
                    socket.send(Message::Pong(payload)).await.unwrap();
                    ping_tx.send(()).unwrap();
                    break;
                }
            }
            // Keep the first connection alive until the client explicitly reconnects.
            let (replacement, _) = listener.accept().await.unwrap();
            let _replacement = tokio_tungstenite::accept_async(replacement).await.unwrap();
            accepted_tx.send(()).await.unwrap();
            std::future::pending::<()>().await;
        });
        let (tx, mut events) = mpsc::channel(8);
        let task_account = account.clone();
        let client = tokio::spawn(async move {
            run_streaming(
                &url,
                "test-token",
                &StreamType::User,
                &address.to_string(),
                tx,
                &task_account,
            )
            .await;
        });
        timeout(Duration::from_secs(5), accepted_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let connected = wait_for_status(&account, |status| status.state == "connected").await;
        assert_eq!(connected.server, address.to_string());
        assert_eq!(connected.stream_type, "Home");
        assert!(connected.last_ping_at.is_none());
        assert!(connected.last_pong_at.is_none());
        assert!(connected.latency_ms.is_none());
        // Exercise the actual production heartbeat interval and protocol control frames.
        timeout(PING_INTERVAL + Duration::from_secs(5), ping_rx)
            .await
            .unwrap()
            .unwrap();
        let heartbeat = wait_for_status(&account, |status| status.latency_ms.is_some()).await;
        let ping_at =
            chrono::DateTime::parse_from_rfc3339(heartbeat.last_ping_at.as_deref().unwrap())
                .unwrap();
        let pong_at =
            chrono::DateTime::parse_from_rfc3339(heartbeat.last_pong_at.as_deref().unwrap())
                .unwrap();
        assert!(pong_at >= ping_at);
        assert!(heartbeat.latency_ms.unwrap().is_finite());
        assert!(heartbeat.latency_ms.unwrap() >= 0.0);
        websocket_status::reconnect(Some(&connected.id)).unwrap();
        timeout(Duration::from_secs(5), accepted_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(5), events.recv())
                .await
                .unwrap(),
            Some(StreamEvent::Resync)
        ));
        let replacement = wait_for_status(&account, |status| status.state == "connected").await;
        assert_eq!(replacement.id, connected.id);
        assert!(replacement.last_ping_at.is_none());
        assert!(replacement.last_pong_at.is_none());
        assert!(replacement.latency_ms.is_none());
        client.abort();
        assert!(client.await.unwrap_err().is_cancelled());
        assert!(!websocket_status::snapshot()
            .iter()
            .any(|status| status.account == account));
        assert!(websocket_status::reconnect(Some(&connected.id)).is_err());
        server.abort();
        let _ = server.await;
    }
}
