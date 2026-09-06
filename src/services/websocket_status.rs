//! Live physical WebSocket diagnostics, scoped to the owning task lifetime.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::mastodon::types::streaming::StreamType;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketStatus {
    pub id: String,
    pub account: String,
    pub server: String,
    pub stream_type: String,
    pub state: String,
    pub last_ping_at: Option<String>,
    pub last_pong_at: Option<String>,
    pub latency_ms: Option<f64>,
}

struct Entry {
    status: WebSocketStatus,
    pending_ping: Option<(Vec<u8>, Instant)>,
    reconnect: Arc<tokio::sync::Notify>,
}

type Registry = BTreeMap<String, Entry>;
#[cfg(test)]
pub static TEST_LOCK: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}
fn lock() -> std::sync::MutexGuard<'static, Registry> {
    registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn snapshot() -> Vec<WebSocketStatus> {
    lock().values().map(|entry| entry.status.clone()).collect()
}

pub fn reconnect(id: Option<&str>) -> Result<(), String> {
    let entries = lock();
    if let Some(id) = id {
        let entry = entries.get(id).ok_or("WebSocket is no longer available")?;
        entry.reconnect.notify_one();
    } else {
        for entry in entries.values() {
            entry.reconnect.notify_one();
        }
    }
    Ok(())
}

pub struct Connection {
    id: String,
    reconnect: Arc<tokio::sync::Notify>,
}
impl Connection {
    pub fn register(account: &str, server: &str, stream: &StreamType) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let reconnect = Arc::new(tokio::sync::Notify::new());
        let stream_type = match stream {
            StreamType::User => "Home".into(),
            StreamType::UserNotification => "Notifications".into(),
            StreamType::Public => "Public".into(),
            StreamType::PublicLocal => "Local".into(),
            StreamType::PublicRemote => "Remote".into(),
            StreamType::Hashtag(tag) => format!("Hashtag: #{tag}"),
            StreamType::HashtagLocal(tag) => format!("Local hashtag: #{tag}"),
            StreamType::List(id) => format!("List: {id}"),
            StreamType::Feed(id) => format!("Feed: {id}"),
            StreamType::Direct => "Direct".into(),
        };
        lock().insert(
            id.clone(),
            Entry {
                status: WebSocketStatus {
                    id: id.clone(),
                    account: account.into(),
                    server: server.into(),
                    stream_type,
                    state: "connecting".into(),
                    last_ping_at: None,
                    last_pong_at: None,
                    latency_ms: None,
                },
                pending_ping: None,
                reconnect: reconnect.clone(),
            },
        );
        Self { id, reconnect }
    }
    fn update(&self, update: impl FnOnce(&mut Entry)) {
        if let Some(entry) = lock().get_mut(&self.id) {
            update(entry);
        }
    }
    pub fn connecting(&self) {
        self.update(|entry| entry.status.state = "connecting".into());
    }
    pub fn connected(&self) {
        self.update(|entry| {
            entry.status.state = "connected".into();
            entry.status.last_ping_at = None;
            entry.status.last_pong_at = None;
            entry.status.latency_ms = None;
            entry.pending_ping = None;
        });
    }
    pub fn disconnected(&self) {
        self.update(|entry| {
            entry.status.state = "reconnecting".into();
            entry.status.latency_ms = None;
            entry.pending_ping = None;
        });
    }
    pub fn ping_sent(&self, payload: Vec<u8>) {
        self.update(|entry| {
            entry.status.last_ping_at = Some(chrono::Utc::now().to_rfc3339());
            entry.pending_ping = Some((payload, Instant::now()));
        });
    }
    pub fn pong_received(&self, payload: &[u8]) {
        self.update(|entry| {
            entry.status.last_pong_at = Some(chrono::Utc::now().to_rfc3339());
            if entry
                .pending_ping
                .as_ref()
                .is_some_and(|(sent, _)| sent == payload)
            {
                let (_, sent) = entry.pending_ping.take().unwrap();
                entry.status.latency_ms = Some(sent.elapsed().as_secs_f64() * 1000.0);
            }
        });
    }
    pub async fn reconnect_requested(&self) {
        self.reconnect.notified().await;
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        lock().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lifecycle_heartbeat_and_targeted_reconnect() {
        let _guard = TEST_LOCK.write().await;
        let connection = Connection::register(
            "alice@example.test",
            "example.test",
            &StreamType::List("friends".into()),
        );
        let other = Connection::register("bob@example.test", "example.test", &StreamType::User);
        connection.connected();
        connection.ping_sent(vec![1]);
        connection.pong_received(&[2]);
        assert!(lock()[&connection.id].status.latency_ms.is_none());
        connection.pong_received(&[1]);
        let status = lock()[&connection.id].status.clone();
        assert!(status.last_ping_at.is_some());
        assert!(status.last_pong_at.is_some());
        assert!(status.latency_ms.is_some());
        assert_eq!(status.stream_type, "List: friends");
        reconnect(Some(&connection.id)).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            connection.reconnect_requested(),
        )
        .await
        .unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(20),
            other.reconnect_requested()
        )
        .await
        .is_err());
        reconnect(None).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            other.reconnect_requested(),
        )
        .await
        .unwrap();
        connection.disconnected();
        assert_eq!(lock()[&connection.id].status.state, "reconnecting");
        connection.connected();
        assert!(lock()[&connection.id].status.last_ping_at.is_none());
        let id = connection.id.clone();
        drop(connection);
        assert!(!lock().contains_key(&id));
        assert!(reconnect(Some(&id)).is_err());
    }
}
