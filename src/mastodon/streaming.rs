use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep_until, timeout, Instant, MissedTickBehavior};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::mastodon::types::streaming::{StreamEvent, StreamType};
use crate::services::reconnect_budget::ReconnectBackoff;

/// Interval between client-initiated ping frames
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time to wait for a pong response before considering the connection dead
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Raw message from the Mastodon Streaming API
#[derive(Debug, serde::Deserialize)]
struct StreamMessage {
    event: String,
    payload: Option<String>,
}

/// Connect to the Mastodon Streaming API via WebSocket and forward events.
/// This function runs in a loop with automatic reconnection.
/// It should be spawned on a tokio runtime.
pub async fn run_streaming(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    server_domain: &str,
    tx: mpsc::Sender<StreamEvent>,
    account: &str,
) {
    let status = crate::services::websocket_status::Connection::register(
        account,
        server_domain,
        stream_type,
    );
    let mut reconnect_backoff = ReconnectBackoff::default();
    let mut resync_on_connect = false;

    loop {
        crate::services::reconnect_budget::wait_for_server_slot(server_domain).await;
        tracing::info!("Connecting to streaming API: {}", streaming_url);

        status.connecting();
        let attempt = async {
            match connect_once(
                streaming_url,
                access_token,
                stream_type,
                &tx,
                resync_on_connect,
                &mut reconnect_backoff,
                &status,
            )
            .await
            {
                Ok(()) => {
                    tracing::info!("Streaming connection closed normally");
                }
                Err(e) => {
                    tracing::warn!("Streaming connection error: {}", e);
                }
            }
            status.disconnected();

            // Check if the receiver has been dropped
            if tx.is_closed() {
                tracing::info!("Streaming channel closed, stopping reconnection");
                return;
            }

            let delay = reconnect_backoff.next_delay(streaming_url);
            tracing::info!("Reconnecting in {:?}...", delay);
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
    tx: &mpsc::Sender<StreamEvent>,
    resync_on_connect: bool,
    reconnect_backoff: &mut ReconnectBackoff,
    status: &crate::services::websocket_status::Connection,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stream_param = stream_type.stream_param();
    let heartbeat_log_url = streaming_log_url(streaming_url, stream_type);

    // Build WebSocket URL: wss://domain/api/v1/streaming?access_token=TOKEN&stream=TYPE
    let mut url = format!(
        "{}/api/v1/streaming?access_token={}&stream={}",
        streaming_url, access_token, stream_param,
    );

    // Add extra params (e.g., tag=foo, list=123)
    if let Some((key, value)) = stream_type.extra_param() {
        url.push_str(&format!("&{}={}", key, value));
    }

    let request = url.into_client_request()?;
    let (ws_stream, _response) = timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "stream connect timeout")
        })??;

    // Back off only consecutive failures to establish a connection. Once the
    // handshake succeeds, a later socket reset is a new outage and should
    // reconnect promptly instead of inheriting stale failures.
    reconnect_backoff.reset();
    status.connected();

    if resync_on_connect && tx.send(StreamEvent::Resync).await.is_err() {
        return Ok(());
    }

    tracing::info!(
        "Streaming connected: url={} stream={}",
        streaming_url,
        stream_param
    );

    let (mut write, mut read) = ws_stream.split();

    // Heartbeat: periodic client-initiated ping with pong timeout detection
    let mut ping_interval = interval(PING_INTERVAL);
    ping_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ping_interval.tick().await; // consume the immediate first tick

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
                        if let Some(event) = parse_stream_message(&text) {
                            if tx.send(event).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    Some(Ok(Message::Pong(data))) => {
                        status.pong_received(&data);
                        tracing::debug!(
                            "Received pong response from streaming server: {}",
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
                        tracing::info!("Server sent close frame");
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
                    "Sending ping to streaming server: {}",
                    heartbeat_log_url
                );
                let payload = uuid::Uuid::new_v4().as_bytes().to_vec();
                if let Err(e) = write.send(Message::Ping(payload.clone().into())).await {
                    tracing::warn!(
                        "Failed to send ping to streaming server {}: {}",
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
                    "Pong timeout for streaming server {} - connection appears dead, disconnecting",
                    heartbeat_log_url
                );
                let _ = write.close().await;
                return Err("Pong timeout".into());
            }
        }
    }
}

fn streaming_log_url(streaming_url: &str, stream_type: &StreamType) -> String {
    let mut url = format!(
        "{}/api/v1/streaming?stream={}",
        streaming_url,
        stream_type.stream_param()
    );

    if let Some((key, value)) = stream_type.extra_param() {
        url.push_str(&format!("&{}={}", key, value));
    }

    url
}

fn parse_stream_message(text: &str) -> Option<StreamEvent> {
    let msg: StreamMessage = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                payload_bytes = text.len(),
                "Failed to parse stream message: {}",
                e
            );
            return None;
        }
    };

    let payload = msg.payload.unwrap_or_default();

    match msg.event.as_str() {
        "update" => Some(StreamEvent::Update(payload)),
        "notification" => Some(StreamEvent::Notification(payload)),
        "delete" => Some(StreamEvent::Delete(payload)),
        "filters_changed" => Some(StreamEvent::FiltersChanged),
        "status.update" => Some(StreamEvent::StatusUpdate(payload)),
        other => {
            tracing::debug!("Unknown stream event: {}", other);
            Some(StreamEvent::Unknown(other.to_string(), payload))
        }
    }
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
        let account = format!("mastodon-{}@test.invalid", uuid::Uuid::new_v4());
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
