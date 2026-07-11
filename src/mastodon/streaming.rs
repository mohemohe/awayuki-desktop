use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep_until, timeout, Instant, MissedTickBehavior};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::mastodon::types::streaming::{StreamEvent, StreamType};

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
    tx: mpsc::Sender<StreamEvent>,
) {
    let mut backoff_secs = 1u64;
    let mut reconnect_attempt = 0u64;
    let mut resync_on_connect = false;

    loop {
        tracing::info!("Connecting to streaming API: {}", streaming_url);

        match connect_once(
            streaming_url,
            access_token,
            stream_type,
            &tx,
            resync_on_connect,
        )
        .await
        {
            Ok(()) => {
                tracing::info!("Streaming connection closed normally");
                backoff_secs = 1;
            }
            Err(e) => {
                tracing::warn!("Streaming connection error: {}", e);
            }
        }
        resync_on_connect = true;

        // Check if the receiver has been dropped
        if tx.is_closed() {
            tracing::info!("Streaming channel closed, stopping reconnection");
            return;
        }

        let delay = reconnect_delay(backoff_secs, streaming_url, reconnect_attempt);
        reconnect_attempt = reconnect_attempt.saturating_add(1);
        tracing::info!("Reconnecting in {:?}...", delay);
        tokio::time::sleep(delay).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn connect_once(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    tx: &mpsc::Sender<StreamEvent>,
    resync_on_connect: bool,
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
                    Some(Ok(Message::Pong(_))) => {
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
                if let Err(e) = write.send(Message::Ping(vec![].into())).await {
                    tracing::warn!(
                        "Failed to send ping to streaming server {}: {}",
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
                    "Pong timeout for streaming server {} - connection appears dead, disconnecting",
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
mod tests {
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
