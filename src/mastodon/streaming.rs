use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::mastodon::types::streaming::{StreamEvent, StreamType};

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
    // Build WebSocket URL: wss://domain/api/v1/streaming?access_token=TOKEN&stream=TYPE
    let mut url = format!(
        "{}/api/v1/streaming?access_token={}&stream={}",
        streaming_url,
        access_token,
        stream_type.stream_param(),
    );

    // Add extra params (e.g., tag=foo, list=123)
    if let Some((key, value)) = stream_type.extra_param() {
        url.push_str(&format!("&{}={}", key, value));
    }

    let request = url.into_client_request()?;
    let (ws_stream, _response) = connect_async(request).await?;

    tracing::info!("Streaming connected: stream={}", stream_type.stream_param());

    let (mut write, mut read) = ws_stream.split();

    while let Some(msg_result) = read.next().await {
        // If receiver is dropped, stop
        if tx.is_closed() {
            let _ = write.close().await;
            return Ok(());
        }

        match msg_result {
            Ok(Message::Text(text)) => {
                if let Some(event) = parse_stream_message(&text) {
                    if tx.send(event).is_err() {
                        // Receiver dropped
                        return Ok(());
                    }
                }
            }
            Ok(Message::Ping(data)) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Ok(Message::Close(_)) => {
                tracing::info!("Server sent close frame");
                return Ok(());
            }
            Ok(_) => {
                // Binary, Pong, Frame - ignore
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Ok(())
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
