use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{interval, sleep_until, Instant, MissedTickBehavior};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::mastodon::types::streaming::{StreamEvent, StreamType};

/// Interval between client-initiated ping frames
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time to wait for a pong response before considering the connection dead
const PONG_TIMEOUT: Duration = Duration::from_secs(10);

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
    tx: mpsc::UnboundedSender<StreamEvent>,
) {
    let mut backoff_secs = 1u64;

    loop {
        tracing::info!("Connecting to streaming API: {}", streaming_url);

        match connect_once(streaming_url, access_token, stream_type, &tx).await {
            Ok(()) => {
                tracing::info!("Streaming connection closed normally");
                backoff_secs = 1;
            }
            Err(e) => {
                tracing::warn!("Streaming connection error: {}", e);
            }
        }

        // Check if the receiver has been dropped
        if tx.is_closed() {
            tracing::info!("Streaming channel closed, stopping reconnection");
            return;
        }

        tracing::info!("Reconnecting in {} seconds...", backoff_secs);
        tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn connect_once(
    streaming_url: &str,
    access_token: &str,
    stream_type: &StreamType,
    tx: &mpsc::UnboundedSender<StreamEvent>,
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
    let (ws_stream, _response) = connect_async(request).await?;

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
                            if tx.send(event).is_err() {
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
            tracing::warn!("Failed to parse stream message: {} - {}", e, text);
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
