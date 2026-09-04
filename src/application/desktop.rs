use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;
use tauri::ipc::Request as IpcRequest;
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewBuilder, WebviewUrl,
    WebviewWindow,
};
use tokio::sync::{mpsc, Notify, RwLock};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;
use crate::api::ports::ServerMetadata;
#[cfg(test)]
use crate::application::account::{account_profile_to_view, preserve_cached_profile_media};
pub(crate) use crate::application::account::{
    AccountProfileSummary, AccountRateLimitSummary, AccountSummary,
};
use crate::application::maintenance::DbSummary;
use crate::application::settings as settings_application;
use crate::application::settings::SettingsSnapshot;
use crate::application::sidecar_policy::{self, SidecarPolicy};
use crate::application::startup_gate::StartupGate;
#[cfg(test)]
use crate::application::timeline_hydration::status_key;
pub(crate) use crate::application::timeline_hydration::{
    apply_viewer_states_to_views, db_status_refs_to_views, db_statuses_to_views,
    notification_db_to_view_with_context, with_source_acct, CachedStatusViewContext,
};
#[cfg(test)]
use crate::application::timeline_view::{db_status_to_view, notification_db_to_view};
pub(crate) use crate::application::timeline_view::{
    notification_to_view, poll_to_view, status_to_view, CustomEmojiView, PollView,
    StatusViewerStateSummary, TimelineGap, TimelinePageResponse, TimelineStatus,
};
use crate::auth::credential_store::{AccountCredentials, CredentialStore};
use crate::auth::session::{AccountSession, SessionManager};
use crate::bluesky::client::BlueskyClient;
use crate::constants::APP_VERSION;
#[cfg(test)]
use crate::db::models::DbNotification;
use crate::db::models::{DbAccount, DbColumnConfig, DbLoginAccount, DbStatus};
use crate::db::pool::Database;
use crate::db::queries::timeline_views::TimelineStatusRef;
use crate::db::queries::{
    accounts, notification_mutes, read_models, search, servers, settings, timeline_views,
};
use crate::domain::adapter_error::AdapterError;
use crate::domain::capability::TimelineOperation;
use crate::ipc::dto::{
    ColumnSummary, CreateSidecarWebviewRequest, TimelineDisplayFilter, VotePollRequest,
};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::mastodon::client::MastodonClient;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::types::account::Account;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::Status;
use crate::misskey::client::MisskeyClient;
use crate::observability::OperationContext;
use crate::plugins::PluginManager;
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::{self, TimelineType};
use crate::services::{compose_outbox, search_indexer, startup_sync};
use crate::state::bluesky_fetch::BlueskyFetchSettings;
use crate::state::media_upload::MediaUploadManager;
use crate::state::notifications::NotificationSuppressionList;
use crate::state::operation_cancellation::OperationCancellationManager;
use crate::state::paths;
use crate::state::storage_security;

mod release_security_smoke;
mod stream_bridge;
mod stream_notification;
mod stream_subscription;
mod window_state;

#[cfg(test)]
use self::release_security_smoke::validated_release_webview_smoke_url;
use self::release_security_smoke::{
    emit_release_security_attestation, inject_release_webview_smoke,
};
use self::stream_bridge::forward_stream_events;
#[cfg(test)]
use self::stream_bridge::forward_stream_events_to_queues;
use self::stream_notification::{desktop_notification_sound, save_notification_to_db};
use self::stream_subscription::stream_types_for_columns;
pub(crate) use self::window_state::{install_window_state_persistence, restore_window_state};

#[derive(Clone)]
pub struct RuntimeState {
    database: Arc<Database>,
    pub(crate) plugins: PluginManager,
    credentials: CredentialStore,
    sessions: Arc<RwLock<SessionManager>>,
    streaming_handles: Arc<RwLock<Vec<tokio::task::AbortHandle>>>,
    emit_queue: QueuedEmitter,
    login_flows: OperationCancellationManager,
    media_downloads: OperationCancellationManager,
    timeline_queries: OperationCancellationManager,
    mutation_operations: OperationCancellationManager,
    media_uploads: Arc<MediaUploadManager>,
    search_indexer_started: Arc<AtomicBool>,
    search_indexer_cancellation: CancellationToken,
    compose_outbox_started: Arc<AtomicBool>,
    compose_outbox_cancellation: CancellationToken,
    compose_outbox_notify: Arc<Notify>,
    startup: StartupGate,
    started_at: Instant,
}

impl RuntimeState {
    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    pub(crate) fn database_handle(&self) -> Arc<Database> {
        self.database.clone()
    }

    pub(crate) fn plugins(&self) -> &PluginManager {
        &self.plugins
    }

    pub(crate) fn credentials(&self) -> &CredentialStore {
        &self.credentials
    }

    pub(crate) fn sessions(&self) -> &Arc<RwLock<SessionManager>> {
        &self.sessions
    }

    pub(crate) fn media_uploads(&self) -> &Arc<MediaUploadManager> {
        &self.media_uploads
    }

    pub(crate) fn timeline_query_manager(&self) -> &OperationCancellationManager {
        &self.timeline_queries
    }

    pub(crate) fn mutation_operation_manager(&self) -> &OperationCancellationManager {
        &self.mutation_operations
    }

    pub(crate) fn startup_gate(&self) -> &StartupGate {
        &self.startup
    }

    pub(crate) fn uptime_seconds(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub(crate) fn compose_outbox_notify(&self) -> &Arc<Notify> {
        &self.compose_outbox_notify
    }

    pub(crate) fn login_flow_manager(&self) -> &OperationCancellationManager {
        &self.login_flows
    }

    pub(crate) fn media_download_manager(&self) -> &OperationCancellationManager {
        &self.media_downloads
    }

    fn cancel_in_flight_operations(&self) -> usize {
        let search_indexer = usize::from(!self.search_indexer_cancellation.is_cancelled());
        let compose_outbox = usize::from(!self.compose_outbox_cancellation.is_cancelled());
        self.search_indexer_cancellation.cancel();
        self.compose_outbox_cancellation.cancel();
        self.login_flows.cancel_all()
            + self.media_downloads.cancel_all()
            + self.timeline_queries.cancel_all()
            + self.mutation_operations.cancel_all()
            + timeline_service::cancel_all_pending_quote_resolution()
            + search_indexer
            + compose_outbox
    }

    fn abort_streaming_tasks(&self) -> usize {
        let Ok(handles) = self.streaming_handles.try_read() else {
            return 0;
        };
        for handle in handles.iter() {
            handle.abort();
        }
        handles.len()
    }

    pub(crate) async fn emit_application_event<T>(
        &self,
        event: &'static str,
        payload: T,
        context: &str,
    ) where
        T: Serialize,
    {
        self.emit_queue.emit(event, payload, context).await;
    }

    pub(crate) fn try_emit_application_event<T>(
        &self,
        event: &'static str,
        payload: T,
        context: &str,
    ) where
        T: Serialize,
    {
        self.emit_queue.try_emit(event, payload, context);
    }

    pub(crate) fn emit_timeline_cache_committed(&self, source_acct: &str, server_domain: &str) {
        self.emit_queue.emit_detached(
            TIMELINE_CACHE_COMMITTED_EVENT,
            TimelineCacheCommittedPayload {
                source_acct: source_acct.to_string(),
                server_domain: server_domain.to_string(),
            },
            "application cache committed",
        );
    }
}

#[derive(Clone)]
struct QueuedEmitter {
    sender: mpsc::Sender<QueuedEmit>,
    detached_pending: Arc<AtomicBool>,
}

struct QueuedEmit {
    event: &'static str,
    payload: String,
    detached_pending: Option<Arc<AtomicBool>>,
}

impl QueuedEmitter {
    fn start(app_handle: AppHandle) -> Self {
        let (sender, mut receiver) = mpsc::channel::<QueuedEmit>(EMIT_QUEUE_CAPACITY);
        tauri::async_runtime::spawn(async move {
            while let Some(queued) = receiver.recv().await {
                if let Some(pending) = queued.detached_pending {
                    // Clear before delivery. A commit racing after this point
                    // schedules a second event; commits before it are covered
                    // by the event that is about to be delivered.
                    pending.store(false, Ordering::Release);
                }
                if let Err(error) = app_handle.emit_str(queued.event, queued.payload) {
                    tracing::warn!(
                        event = queued.event,
                        "Failed to emit queued Tauri event: {}",
                        error
                    );
                }
                tokio::task::yield_now().await;
            }
        });
        Self {
            sender,
            detached_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn emit<T>(&self, event: &'static str, payload: T, context: &str)
    where
        T: Serialize,
    {
        let Ok(payload) = serialize_emit_payload(event, payload, context) else {
            return;
        };
        if let Err(error) = self
            .sender
            .send(QueuedEmit {
                event,
                payload,
                detached_pending: None,
            })
            .await
        {
            tracing::warn!(event, context, "Failed to queue Tauri event: {}", error);
        }
    }

    fn try_emit<T>(&self, event: &'static str, payload: T, context: &str)
    where
        T: Serialize,
    {
        let Ok(payload) = serialize_emit_payload(event, payload, context) else {
            return;
        };
        if let Err(error) = self.sender.try_send(QueuedEmit {
            event,
            payload,
            detached_pending: None,
        }) {
            tracing::warn!(
                event,
                context,
                "Skipped non-blocking Tauri event because its queue is unavailable: {}",
                error
            );
        }
    }

    fn emit_detached<T>(&self, event: &'static str, payload: T, context: &'static str)
    where
        T: Serialize,
    {
        if self.detached_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(payload) = serialize_emit_payload(event, payload, context) else {
            self.detached_pending.store(false, Ordering::Release);
            return;
        };
        let sender = self.sender.clone();
        let detached_pending = Arc::clone(&self.detached_pending);
        tauri::async_runtime::spawn(async move {
            if let Err(error) = sender
                .send(QueuedEmit {
                    event,
                    payload,
                    detached_pending: Some(Arc::clone(&detached_pending)),
                })
                .await
            {
                detached_pending.store(false, Ordering::Release);
                tracing::warn!(event, context, "Failed to queue Tauri event: {}", error);
            }
        });
    }
}

fn serialize_emit_payload<T>(event: &'static str, payload: T, context: &str) -> Result<String, ()>
where
    T: Serialize,
{
    serde_json::to_string(&payload).map_err(|error| {
        tracing::warn!(
            event,
            context,
            "Failed to serialize Tauri event payload: {}",
            error
        );
    })
}

const WINDOW_STATE_SETTING_KEY: &str = "window_state";
const WINDOW_STATE_SAVE_DEBOUNCE_MS: u64 = 400;
const TIMELINE_STREAM_EVENT: &str = "timeline-stream-event";
const TIMELINE_CACHE_COMMITTED_EVENT: &str = "timeline-cache-committed";
const STARTUP_SYNC_COMPLETE_EVENT: &str = "timeline-startup-sync-complete";
pub(crate) const APP_STARTUP_PROGRESS_EVENT: &str = "app-startup-progress";
const STATUS_SEARCH_BACKFILL_PROGRESS_EVENT: &str = "status-search-backfill-progress";
const EMIT_QUEUE_CAPACITY: usize = 1024;
const STREAM_BRIDGE_QUEUE_CAPACITY: usize = 256;
const STREAM_SIDE_EFFECT_QUEUE_CAPACITY: usize = 64;
const MASTODON_DEFAULT_CHARACTER_LIMIT: i32 = 500;
const MISSKEY_DEFAULT_CHARACTER_LIMIT: i32 = 3000;
const BLUESKY_CHARACTER_LIMIT: i32 = 300;
#[cfg(test)]
const CUSTOM_SQL_MAX_RESULT_ROWS: i64 = crate::db::queries::custom_timeline::MAX_RESULT_ROWS;
#[cfg(test)]
const YQ_FILTER_PAGE_SIZE: i64 = crate::services::yq_timeline::FILTER_PAGE_SIZE;

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

pub(crate) fn ipc_operation_id(request: &IpcRequest<'_>) -> Option<String> {
    operation_id_from_headers(request.headers())
}

fn operation_id_from_headers(headers: &tauri::http::HeaderMap) -> Option<String> {
    headers
        .get("x-awayuki-operation-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .map(|value| value.to_string())
}

pub(crate) async fn observe_string_command<T, F>(
    command: &'static str,
    request: &IpcRequest<'_>,
    future: F,
) -> Result<T, AppError>
where
    F: Future<Output = Result<T, String>>,
{
    let requested_id = ipc_operation_id(request);
    let mut operation = OperationContext::start(command, requested_id.as_deref(), None);
    match future.await {
        Ok(value) => {
            operation.finish_ok();
            Ok(value)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) fn observe_string_command_sync<T>(
    command: &'static str,
    request: &IpcRequest<'_>,
    result: Result<T, String>,
) -> Result<T, AppError> {
    let requested_id = ipc_operation_id(request);
    let mut operation = OperationContext::start(command, requested_id.as_deref(), None);
    match result {
        Ok(value) => {
            operation.finish_ok();
            Ok(value)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn observe_infallible_command<T, F>(
    command: &'static str,
    requested_id: Option<String>,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    let mut operation = OperationContext::start(command, requested_id.as_deref(), None);
    let value = future.await;
    operation.finish_ok();
    value
}

pub(crate) async fn run_cancellable_read<T, F>(
    manager: OperationCancellationManager,
    command: &'static str,
    requested_operation_id: Option<&str>,
    future: F,
) -> Result<T, AppError>
where
    F: Future<Output = Result<T, String>>,
{
    let mut operation = OperationContext::start(command, requested_operation_id, None);
    let Some(lease) = manager.begin(operation.id()) else {
        return Err(
            operation.finish_error_code(AppErrorCode::Validation, "operation ID is already active")
        );
    };
    let cancellation = lease.token().clone();
    tokio::pin!(future);
    let result = tokio::select! {
        () = cancellation.cancelled() => {
            return Err(operation.finish_error_code(AppErrorCode::Cancelled, "operation cancelled"));
        }
        result = &mut future => result,
    };
    match result {
        Ok(value) => {
            operation.finish_ok();
            Ok(value)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn run_cancellable_ipc_mutation<T, F>(
    manager: OperationCancellationManager,
    command: &'static str,
    request: &IpcRequest<'_>,
    future: F,
) -> Result<T, AppError>
where
    F: Future<Output = Result<T, String>>,
{
    let operation_id = ipc_operation_id(request);
    run_cancellable_read(manager, command, operation_id.as_deref(), future).await
}

pub(crate) async fn run_cancellable_app_mutation<T, F>(
    manager: OperationCancellationManager,
    requested_operation_id: Option<&str>,
    future: F,
) -> Result<T, AppError>
where
    F: Future<Output = Result<T, AppError>>,
{
    let operation_id = requested_operation_id
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let Some(lease) = manager.begin(&operation_id) else {
        return Err(AppError::from_code(
            AppErrorCode::Validation,
            "operation ID is already active",
            operation_id,
        ));
    };
    let cancellation = lease.token().clone();
    tokio::pin!(future);
    tokio::select! {
        () = cancellation.cancelled() => Err(AppError::from_code(
            AppErrorCode::Cancelled,
            "operation cancelled; external mutation result may be uncertain",
            operation_id,
        )),
        result = &mut future => result,
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupSyncEvent {
    kind: String,
    message: String,
    acct: Option<String>,
    page: Option<u32>,
    total: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppStartupProgressEvent {
    pub(crate) stage: &'static str,
    pub(crate) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

fn open_sidecar_external_url(sidecar_label: &str, url: &Url) {
    if let Err(error) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
        tracing::error!(
            target: "awayuki::sidecar",
            sidecar = %sidecar_label,
            url = %url,
            "Failed to open external sidecar URL in the default browser: {}",
            error
        );
    }
}

pub(crate) async fn create_sidecar_webview_impl(
    app: AppHandle,
    request: CreateSidecarWebviewRequest,
) -> Result<(), String> {
    let CreateSidecarWebviewRequest {
        sidecar_id,
        url,
        user_style,
        x,
        y,
        width,
        height,
    } = request;
    let (url, policy) = SidecarPolicy::parse_initial_url(&url)?;
    let label = sidecar_webview_label(&sidecar_id);
    sidecar_policy::register(&label, policy.clone(), user_style)?;
    if let Some(webview) = app.get_webview(&label) {
        webview.hide().map_err(|error| error.to_string())?;
        eval_sidecar_user_style(&webview, &sidecar_policy::user_style(&label))?;
        return Ok(());
    }

    let Some(window) = app.get_window("main") else {
        sidecar_policy::remove(&label);
        return Err("Main window not found".to_string());
    };
    let label_for_page_load = label.clone();
    let label_for_navigation = label.clone();
    let label_for_new_window = label.clone();
    let label_for_download = label.clone();
    let navigation_policy = policy.clone();
    let popup_policy = policy.clone();
    let download_policy = policy;
    let builder = WebviewBuilder::new(label.clone(), WebviewUrl::External(url))
        .on_navigation(move |url| {
            let allowed = navigation_policy.allows_navigation(url);
            if !allowed {
                if navigation_policy.should_open_external(url) {
                    open_sidecar_external_url(&label_for_navigation, url);
                } else {
                    tracing::warn!(
                        target: "awayuki::sidecar",
                        sidecar = %label_for_navigation,
                        url = %url,
                        "Blocked unsupported sidecar navigation"
                    );
                }
            }
            allowed
        })
        .on_new_window(move |url, _| {
            let allowed = popup_policy.allows_popup(&url);
            if popup_policy.should_open_external(&url) {
                open_sidecar_external_url(&label_for_new_window, &url);
            } else {
                tracing::warn!(
                    target: "awayuki::sidecar",
                    sidecar = %label_for_new_window,
                    url = %url,
                    "Blocked sidecar popup; new-window requests are denied by policy"
                );
            }
            if allowed {
                NewWindowResponse::Allow
            } else {
                NewWindowResponse::Deny
            }
        })
        .on_download(move |_, event| {
            if let DownloadEvent::Requested { url, .. } = event {
                let allowed = download_policy.allows_download(&url);
                tracing::warn!(
                    target: "awayuki::sidecar",
                    sidecar = %label_for_download,
                    url = %url,
                    "Blocked sidecar download; downloads must use an explicit app action"
                );
                return allowed;
            }
            false
        })
        .on_page_load(move |webview, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                if let Err(error) = eval_sidecar_user_style(
                    &webview,
                    &sidecar_policy::user_style(&label_for_page_load),
                ) {
                    tracing::warn!(
                        target: "awayuki::sidecar",
                        sidecar = %label_for_page_load,
                        "Failed to inject sidecar UserStyle on page load: {}",
                        error
                    );
                }
            }
        });

    let webview = match window.add_child(
        builder,
        LogicalPosition::new(x, y),
        LogicalSize::new(width, height),
    ) {
        Ok(webview) => webview,
        Err(error) => {
            sidecar_policy::remove(&label);
            return Err(error.to_string());
        }
    };
    if let Err(error) = webview.hide() {
        let _ = webview.close();
        sidecar_policy::remove(&label);
        return Err(error.to_string());
    }
    Ok(())
}

pub(crate) fn navigate_sidecar_webview_impl(
    app: AppHandle,
    sidecar_id: String,
    url: String,
) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let url = Url::parse(url.trim()).map_err(|_| "Sidecar URL is invalid".to_string())?;
    let policy = sidecar_policy::policy(&label)
        .ok_or_else(|| format!("Sidecar lifecycle not found: {label}"))?;
    if !policy.allows_navigation(&url) {
        return Err("Sidecar navigation must stay on its initial origin".to_string());
    }
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    webview.navigate(url).map_err(|error| error.to_string())
}

pub(crate) fn reload_sidecar_webview_impl(
    app: AppHandle,
    sidecar_id: String,
) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    webview.reload().map_err(|error| error.to_string())
}

pub(crate) fn close_sidecar_webview_impl(app: AppHandle, sidecar_id: String) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let close_result = app
        .get_webview(&label)
        .map(|webview| webview.close().map_err(|error| error.to_string()))
        .unwrap_or(Ok(()));
    sidecar_policy::remove(&label);
    close_result
}

pub(crate) fn scroll_sidecar_webview_to_top_impl(
    app: AppHandle,
    sidecar_id: String,
) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    webview
        .eval("window.scrollTo({ top: 0, left: 0, behavior: 'smooth' });")
        .map_err(|error| error.to_string())
}

pub(crate) fn inject_sidecar_user_style_impl(
    app: AppHandle,
    sidecar_id: String,
    user_style: String,
) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    sidecar_policy::set_user_style(&label, user_style)?;
    eval_sidecar_user_style(&webview, &sidecar_policy::user_style(&label))
}

fn eval_sidecar_user_style<R: tauri::Runtime>(
    webview: &tauri::Webview<R>,
    user_style: &str,
) -> Result<(), String> {
    webview
        .eval(sidecar_user_style_script(user_style)?)
        .map_err(|error| error.to_string())
}

fn sidecar_user_style_script(user_style: &str) -> Result<String, String> {
    let css = serde_json::to_string(user_style).map_err(|error| error.to_string())?;
    Ok(format!(
        r#"
(() => {{
  const STYLE_ID = "awayuki-sidecar-user-style";
  const STATE_KEY = "__awayukiSidecarUserStyle";
  const css = {css};
  const win = window;
  const removeStyle = () => {{
    document.getElementById(STYLE_ID)?.remove();
  }};
  let state = win[STATE_KEY];
  if (!css.trim()) {{
    state?.cleanup?.();
    delete win[STATE_KEY];
    removeStyle();
    return;
  }}
  if (!state) {{
    state = {{
      css: "",
      observer: null,
      scheduledId: null,
      scheduledWithAnimationFrame: false,
      historyOriginals: {{}},
      historyWrappers: {{}},
      locationListener: null,
    }};
    const install = () => {{
      const root = document.head || document.documentElement;
      if (!root) return;
      let style = document.getElementById(STYLE_ID);
      if (!style) {{
        style = document.createElement("style");
        style.id = STYLE_ID;
        style.setAttribute("data-awayuki-sidecar-user-style", "");
        root.appendChild(style);
      }}
      if (style.textContent !== state.css) {{
        style.textContent = state.css;
      }}
    }};
    const cancelScheduled = () => {{
      if (state.scheduledId === null) return;
      if (state.scheduledWithAnimationFrame) {{
        cancelAnimationFrame(state.scheduledId);
      }} else {{
        clearTimeout(state.scheduledId);
      }}
      state.scheduledId = null;
    }};
    const schedule = () => {{
      if (state.scheduledId !== null) return;
      const run = () => {{
        state.scheduledId = null;
        install();
      }};
      if (typeof requestAnimationFrame === "function") {{
        state.scheduledWithAnimationFrame = true;
        state.scheduledId = requestAnimationFrame(run);
      }} else {{
        state.scheduledWithAnimationFrame = false;
        state.scheduledId = setTimeout(run, 0);
      }}
    }};
    const locationListener = () => schedule();
    state.install = install;
    state.schedule = schedule;
    state.locationListener = locationListener;
    state.cleanup = () => {{
      cancelScheduled();
      state.observer?.disconnect();
      state.observer = null;
      removeEventListener("popstate", locationListener);
      removeEventListener("hashchange", locationListener);
      for (const method of ["pushState", "replaceState"]) {{
        if (history[method] === state.historyWrappers[method]) {{
          history[method] = state.historyOriginals[method];
        }}
      }}
      removeStyle();
    }};
    for (const method of ["pushState", "replaceState"]) {{
      const original = history[method];
      const wrapper = function (...args) {{
        const result = original.apply(this, args);
        schedule();
        return result;
      }};
      state.historyOriginals[method] = original;
      state.historyWrappers[method] = wrapper;
      history[method] = wrapper;
    }}
    addEventListener("popstate", locationListener);
    addEventListener("hashchange", locationListener);
    if (document.documentElement) {{
      state.observer = new MutationObserver(schedule);
      state.observer.observe(document.documentElement, {{
        childList: true,
        subtree: true,
      }});
    }}
    win[STATE_KEY] = state;
  }}
  state.css = css;
  state.install();
}})();
"#
    ))
}

fn sidecar_webview_label(id: &str) -> String {
    let suffix: String = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '/' | ':' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    format!("sidecar-{}", suffix)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSnapshot {
    version: String,
    accounts: Vec<AccountSummary>,
    active_acct: Option<String>,
    columns: Vec<ColumnSummary>,
    settings: SettingsSnapshot,
    database: DbSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineStreamPayload {
    kind: String,
    stream_type: String,
    source_acct: String,
    server_domain: String,
    status: Option<TimelineStatus>,
    status_id: Option<String>,
    generation: u64,
    sequence: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelineCacheCommittedPayload {
    source_acct: String,
    server_domain: String,
}

pub fn run() {
    let builder = tauri::Builder::default()
        // The plugin's default click script is injected into every WebView and
        // would make remote sidecars invoke the opener command without an ACL.
        // Sidecar external links are handled by their Rust navigation hooks.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_log::Builder::new().skip_logger().build())
        .on_page_load(|webview, payload| {
            if webview.label() == "main" && matches!(payload.event(), PageLoadEvent::Finished) {
                inject_release_webview_smoke(webview);
            }
        });

    #[cfg(target_os = "windows")]
    let builder = builder.plugin(
        tauri_plugin_frame::FramePluginBuilder::new()
            .titlebar_height(32)
            .button_width(44)
            .auto_titlebar(true)
            .snap_overlay(true)
            .close_hover_bg("rgba(196,43,28,1)")
            .button_hover_bg("rgba(49,50,68,1)")
            .build(),
    );

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let builder = builder.on_web_content_process_terminate(|webview| {
        let label = webview.label().to_string();
        tracing::error!(
            target: "awayuki::webview",
            webview = %label,
            "Web content process terminated; reloading webview"
        );
        if let Err(error) = webview.reload() {
            tracing::error!(
                target: "awayuki::webview",
                webview = %label,
                "Failed to reload webview after WebContent termination: {}",
                error
            );
        }
    });

    let app = builder
        .setup(|app| {
            // Opening the SQLite pools is intentionally the only synchronous
            // setup work. Schema migration, session restoration and service
            // startup run after the WebView exists so a large portable
            // database can show progress instead of presenting a frozen app.
            let state = tauri::async_runtime::block_on(open_runtime_state(app.handle().clone()))?;
            emit_release_security_attestation(app);
            if let Some(window) = app.get_webview_window("main") {
                #[cfg(target_os = "macos")]
                activate_performance_smoke_window(&window);
                install_drop_path_registration(window, Arc::clone(&state.media_uploads));
            }
            app.manage(state.clone());
            crate::updater::init_updater();
            crate::updater::schedule_periodic_update_checks(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::ipc::runtime::app_snapshot,
            crate::ipc::runtime::start_runtime_initialization,
            crate::ipc::runtime::retry_runtime_initialization,
            crate::ipc::runtime::cancel_mutation_operation,
            crate::ipc::runtime::report_release_webview_smoke,
            crate::ipc::account::account_summaries,
            crate::ipc::account::account_lists,
            crate::ipc::account::account_feeds,
            crate::ipc::auth::login_with_instance_domain,
            crate::ipc::auth::login_with_bluesky_app_password,
            crate::ipc::auth::cancel_login_flow,
            crate::ipc::timeline::load_timeline,
            crate::ipc::timeline::load_more_timeline,
            crate::ipc::timeline::load_timeline_gap,
            crate::ipc::timeline::cancel_timeline_query,
            crate::ipc::timeline::cancel_quote_consumer,
            crate::ipc::timeline::refresh_timeline,
            crate::ipc::timeline::status_viewer_states,
            crate::ipc::timeline::status_thread,
            crate::ipc::timeline::air_context,
            crate::ipc::account::account_profile,
            crate::ipc::account::account_timeline,
            crate::ipc::account::account_follow_action,
            crate::ipc::account::notification_muted_accounts,
            crate::ipc::account::set_account_notification_mute,
            crate::ipc::compose::post_status,
            crate::ipc::compose::enqueue_post_status,
            crate::ipc::compose::enqueue_edit_status,
            crate::ipc::compose::compose_outbox_items,
            crate::ipc::compose::retry_compose_outbox_item,
            crate::ipc::compose::cancel_compose_outbox_item,
            crate::ipc::compose::begin_compose_media_upload,
            crate::ipc::compose::append_compose_media_upload,
            crate::ipc::compose::finish_compose_media_upload,
            crate::ipc::compose::cancel_compose_media_upload,
            crate::ipc::compose::claim_dropped_media_path,
            crate::ipc::compose::upload_compose_media_path,
            crate::ipc::compose::autocomplete_mentions,
            crate::ipc::compose::autocomplete_hashtags,
            crate::ipc::compose::custom_emojis,
            crate::ipc::compose::edit_own_status,
            crate::ipc::compose::delete_own_status,
            crate::ipc::compose::vote_poll,
            crate::ipc::account::switch_active_account,
            crate::ipc::account::logout_account,
            crate::ipc::settings::save_settings,
            crate::ipc::settings::translate_status_text,
            crate::ipc::settings::save_columns,
            crate::ipc::plugins::plugin_snapshot,
            crate::ipc::plugins::open_plugin_directory,
            crate::ipc::plugins::reload_plugins,
            crate::ipc::plugins::reload_plugin,
            crate::ipc::plugins::unload_plugin,
            crate::ipc::plugins::invoke_plugin_compose_button,
            crate::ipc::maintenance::explain_custom_timeline,
            crate::ipc::maintenance::icu_match_expression,
            crate::ipc::maintenance::vacuum_database,
            crate::ipc::maintenance::clear_status_cache,
            crate::ipc::maintenance::status_bar_snapshot,
            crate::ipc::maintenance::diagnostics_snapshot,
            crate::ipc::maintenance::support_bundle,
            crate::ipc::compose::status_action,
            crate::ipc::media::download_media,
            crate::ipc::media::cancel_media_download,
            crate::ipc::media::open_status_url,
            crate::ipc::sidecar::create_sidecar_webview,
            crate::ipc::sidecar::navigate_sidecar_webview,
            crate::ipc::sidecar::reload_sidecar_webview,
            crate::ipc::sidecar::close_sidecar_webview,
            crate::ipc::sidecar::scroll_sidecar_webview_to_top,
            crate::ipc::sidecar::inject_sidecar_user_style,
            crate::ipc::media::open_log_file
        ])
        .build(tauri::generate_context!())
        .expect("error while building Tauri application");
    app.run(|app_handle, event| {
        if !matches!(event, tauri::RunEvent::ExitRequested { .. }) {
            return;
        }
        let Some(state) = app_handle.try_state::<RuntimeState>() else {
            return;
        };
        let operations = state.cancel_in_flight_operations();
        let streams = state.abort_streaming_tasks();
        // A plugin may be inside a blocking fetch or timer-backed Promise.
        // Never make Tauri's run-event callback wait for that actor queue.
        // Process exit may race this best-effort cleanup; explicit unload and
        // reload commands remain synchronous and wait for lifecycle teardown.
        let plugins = state.plugins.clone();
        drop(tauri::async_runtime::spawn_blocking(move || {
            plugins.shutdown();
        }));
        tracing::info!(
            operations,
            streams,
            "cancelled in-flight work for app shutdown"
        );
    });
}

#[cfg(target_os = "macos")]
fn activate_performance_smoke_window(window: &WebviewWindow) {
    if std::env::var_os("AWAYUKI_PERFORMANCE_SMOKE").is_none() {
        return;
    }
    if let Err(error) = window.show() {
        tracing::warn!(%error, "failed to show performance fixture window");
    }
    if let Err(error) = window.set_focus() {
        tracing::warn!(%error, "failed to focus performance fixture window");
    }
    if let Err(error) = window.set_always_on_top(true) {
        tracing::warn!(%error, "failed to keep performance fixture window visible");
    }
    // LaunchServices may start a CLI-driven .app without making it the active
    // application. WKWebView reports that window as hidden and suppresses rAF,
    // which makes paint measurements invalid.
    unsafe {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};

        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![application, activateIgnoringOtherApps: true];
    }
}

async fn open_runtime_state(
    app_handle: AppHandle,
) -> Result<RuntimeState, Box<dyn std::error::Error>> {
    let storage = paths::storage_location();
    if let Some(warning) = storage_security::prepare_storage(&storage)? {
        tracing::warn!(
            storage_kind = ?storage.kind,
            %warning,
            "Awayuki is using a storage location with inherited parent permissions"
        );
    }
    let db_path = storage.directory.join(crate::constants::DB_FILENAME);
    let plugins_directory = storage.plugins_directory()?;

    let database_open_started = Instant::now();
    let database = Arc::new(Database::new(&db_path).await?);
    tracing::info!(
        duration_ms = elapsed_ms(database_open_started),
        "SQLite connection pools opened"
    );
    let plugins = PluginManager::start(plugins_directory).map_err(std::io::Error::other)?;

    Ok(RuntimeState {
        database,
        plugins,
        credentials: CredentialStore::sqlite(),
        sessions: Arc::new(RwLock::new(SessionManager::new())),
        streaming_handles: Arc::new(RwLock::new(Vec::new())),
        emit_queue: QueuedEmitter::start(app_handle),
        login_flows: OperationCancellationManager::default(),
        media_downloads: OperationCancellationManager::default(),
        timeline_queries: OperationCancellationManager::default(),
        mutation_operations: OperationCancellationManager::default(),
        media_uploads: Arc::new(MediaUploadManager::default()),
        search_indexer_started: Arc::new(AtomicBool::new(false)),
        search_indexer_cancellation: CancellationToken::new(),
        compose_outbox_started: Arc::new(AtomicBool::new(false)),
        compose_outbox_cancellation: CancellationToken::new(),
        compose_outbox_notify: Arc::new(Notify::new()),
        startup: StartupGate::new(),
        started_at: Instant::now(),
    })
}

fn install_drop_path_registration(window: WebviewWindow, media_uploads: Arc<MediaUploadManager>) {
    window.on_webview_event(move |event| {
        let Some(paths) = dropped_paths_from_webview_event(event) else {
            return;
        };
        tracing::debug!(path_count = paths.len(), "Observed native media drop paths");
        media_uploads.register_dropped_paths(paths);
    });
}

fn dropped_paths_from_webview_event(event: &tauri::WebviewEvent) -> Option<&[PathBuf]> {
    match event {
        tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
            Some(paths.as_slice())
        }
        _ => None,
    }
}

pub(crate) async fn restore_session(
    row: &DbLoginAccount,
    credentials: &AccountCredentials,
) -> Result<AccountSession, String> {
    let kind = ServerKind::from_db_str(&row.server_kind);
    let streaming_url = format!("wss://{}", row.server_domain);
    let client = match kind {
        ServerKind::Misskey => ApiClient::misskey(
            MisskeyClient::new(
                &row.server_domain,
                credentials.access_token.clone(),
                streaming_url,
            )
            .map_err(|error| error.to_string())?,
        ),
        ServerKind::Bluesky => ApiClient::bluesky(
            BlueskyClient::from_stored(
                &row.server_domain,
                credentials.access_token.clone(),
                streaming_url,
                credentials.app_password.clone(),
            )
            .await
            .map_err(|error| error.to_string())?,
        ),
        ServerKind::Mastodon | ServerKind::Paon => ApiClient::mastodon_with_kind(
            MastodonClient::new(
                &row.server_domain,
                credentials.access_token.clone(),
                streaming_url,
            )
            .map_err(|error| error.to_string())?,
            kind,
        ),
    };

    Ok(AccountSession {
        acct: row.acct.clone(),
        domain: row.server_domain.clone(),
        client,
        account_info: Account {
            id: row.account_id.clone(),
            username: row.acct.split('@').next().unwrap_or(&row.acct).to_string(),
            acct: row.acct.clone(),
            display_name: row.display_name.clone(),
            note: String::new(),
            url: format!("https://{}/@{}", row.server_domain, row.acct),
            uri: String::new(),
            avatar: row.avatar.clone(),
            avatar_static: row.avatar.clone(),
            header: String::new(),
            header_static: String::new(),
            locked: false,
            bot: false,
            created_at: Utc::now(),
            followers_count: 0,
            following_count: 0,
            statuses_count: 0,
            fields: Vec::new(),
            emojis: Vec::new(),
            pleroma: None,
        },
    })
}

pub(crate) async fn persist_login_session(
    state: &RuntimeState,
    session: AccountSession,
    kind: ServerKind,
) -> Result<AppSnapshot, String> {
    let access_token = session
        .client
        .current_access_token()
        .await
        .map_err(|error| error.to_string())?;
    let app_password = session.client.bluesky_app_password();
    let account_credentials = AccountCredentials::new(access_token, app_password);
    let mut login_account = DbLoginAccount {
        acct: session.acct.clone(),
        server_domain: session.domain.clone(),
        account_id: session.account_info.id.clone(),
        display_name: session.account_info.display_name.clone(),
        avatar: session.account_info.avatar.clone(),
        is_active: true,
        access_token: String::new(),
        server_kind: kind.as_db_str().to_string(),
        app_password: None,
    };

    state
        .credentials
        .persist_login_account(
            state.database.writer(),
            &mut login_account,
            &account_credentials,
        )
        .await
        .map_err(|error| error.to_string())?;
    session.client.set_bluesky_credential_sink(
        state
            .credentials
            .bluesky_sink(state.database.writer(), session.acct.clone()),
    );
    if let Err(error) =
        refresh_server_metadata_for_client(state.database.writer(), &session.client, kind).await
    {
        tracing::warn!(
            "Failed to refresh server metadata for {}: {}",
            session.domain,
            error
        );
    }
    accounts::upsert_account(
        state.database.writer(),
        &DbAccount::from_api(&session.account_info, &session.domain),
    )
    .await
    .map_err(|error| error.to_string())?;
    state.emit_timeline_cache_committed(&session.acct, session.client.domain());

    {
        let mut sessions = state.sessions.write().await;
        sessions.add_session(session.clone());
        sessions.set_active(&session.acct);
    }
    restart_streaming(state).await;
    app_snapshot_for_state(state).await
}

pub(crate) fn encode_column_param_with_display_filter(column: &ColumnSummary) -> Option<String> {
    if !timeline_type_supports_display_filter(&column.column_type) {
        return column.column_param.clone();
    }
    let filter = column.display_filter.unwrap_or_default();
    if filter == TimelineDisplayFilter::default() {
        return column.column_param.clone();
    }
    Some(
        serde_json::json!({
            "value": column.column_param.as_deref(),
            "filters": filter,
        })
        .to_string(),
    )
}

pub(crate) async fn login_accounts(state: &RuntimeState) -> Result<Vec<AccountSummary>, String> {
    let rows = settings::get_login_accounts(state.database.reader())
        .await
        .map_err(|error| error.to_string())?;
    let (rate_limits, session_by_acct) = {
        let sessions = state.sessions.read().await;
        let rate_limits = rows
            .iter()
            .map(|row| {
                (
                    row.acct.clone(),
                    account_rate_limit_summary(&sessions, &row.acct),
                )
            })
            .collect::<HashMap<_, _>>();
        let session_by_acct = rows
            .iter()
            .filter_map(|row| {
                sessions
                    .sessions()
                    .get(&row.acct)
                    .cloned()
                    .map(|session| (row.acct.clone(), session))
            })
            .collect::<HashMap<_, _>>();
        (rate_limits, session_by_acct)
    };

    let mut summaries = Vec::with_capacity(rows.len());
    for row in rows {
        let character_limit =
            account_character_limit(state, &row, session_by_acct.get(&row.acct)).await;
        let kind = ServerKind::from_db_str(&row.server_kind);
        let capabilities = session_by_acct
            .get(&row.acct)
            .map(|session| session.client.capabilities(character_limit.max(1) as u32))
            .unwrap_or_else(|| {
                ApiClient::capabilities_for_kind(kind, character_limit.max(1) as u32)
            });
        summaries.push(AccountSummary {
            rate_limit: rate_limits.get(&row.acct).cloned().flatten(),
            character_limit,
            capabilities,
            acct: row.acct,
            server_domain: row.server_domain,
            account_id: row.account_id,
            display_name: row.display_name,
            avatar: row.avatar,
            is_active: row.is_active,
            server_kind: row.server_kind,
        });
    }

    Ok(summaries)
}

pub(crate) async fn app_snapshot_for_state(state: &RuntimeState) -> Result<AppSnapshot, String> {
    let accounts = login_accounts(state).await?;
    let active_acct = {
        let sessions = state.sessions.read().await;
        sessions
            .active_session()
            .map(|session| session.acct.clone())
    }
    .or_else(|| {
        accounts
            .iter()
            .find(|account| account.is_active)
            .map(|account| account.acct.clone())
    });

    Ok(AppSnapshot {
        version: APP_VERSION.to_string(),
        active_acct,
        accounts,
        columns: columns(state).await?,
        settings: settings_application::settings_snapshot(&state.database).await?,
        database: crate::application::maintenance::database_summary(state).await?,
    })
}

fn account_rate_limit_summary(
    sessions: &SessionManager,
    acct: &str,
) -> Option<AccountRateLimitSummary> {
    let state = sessions
        .sessions()
        .get(acct)?
        .client
        .bluesky_rate_limit_state()?;
    let snapshot = state.read().ok()?.clone()?;
    let now = Utc::now();
    Some(AccountRateLimitSummary {
        limit: snapshot.limit,
        remaining: snapshot.remaining,
        used: snapshot.limit.saturating_sub(snapshot.remaining),
        reset_in_seconds: (snapshot.reset_at - now).num_seconds().max(0),
        observed_ago_seconds: (now - snapshot.observed_at).num_seconds().max(0),
        used_fraction: snapshot.used_fraction(),
        policy: snapshot.policy,
    })
}

async fn account_character_limit(
    state: &RuntimeState,
    row: &DbLoginAccount,
    session: Option<&AccountSession>,
) -> i32 {
    let kind = ServerKind::from_db_str(&row.server_kind);
    if matches!(kind, ServerKind::Bluesky) {
        return BLUESKY_CHARACTER_LIMIT;
    }

    match servers::get_server(state.database.reader(), &row.server_domain).await {
        Ok(Some(server)) if server.instance_json.is_some() => {
            if let Some(max_characters) = valid_character_limit(server.max_characters) {
                return max_characters;
            }
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(
            "Failed to load cached server metadata for {}: {}",
            row.server_domain,
            error
        ),
    }

    if let Some(session) = session {
        match refresh_server_metadata_for_client(state.database.writer(), &session.client, kind)
            .await
        {
            Ok(metadata) => return metadata.max_characters,
            Err(error) => tracing::warn!(
                "Failed to refresh server metadata for {}: {}",
                row.server_domain,
                error
            ),
        }
    }

    default_character_limit(kind)
}

fn valid_character_limit(value: Option<i32>) -> Option<i32> {
    value.filter(|limit| *limit > 0)
}

fn default_character_limit(kind: ServerKind) -> i32 {
    match kind {
        ServerKind::Misskey => MISSKEY_DEFAULT_CHARACTER_LIMIT,
        ServerKind::Bluesky => BLUESKY_CHARACTER_LIMIT,
        ServerKind::Mastodon | ServerKind::Paon => MASTODON_DEFAULT_CHARACTER_LIMIT,
    }
}

async fn refresh_server_metadata_for_client(
    pool: &sqlx::SqlitePool,
    client: &ApiClient,
    stored_kind: ServerKind,
) -> Result<ServerMetadata, String> {
    let metadata = fetch_server_metadata(client, stored_kind)
        .await
        .map_err(|error| error.to_string())?;
    servers::upsert_server_details(
        pool,
        client.domain(),
        &metadata.streaming_url,
        metadata.version.as_deref(),
        metadata.max_characters,
        metadata.instance_json.as_deref(),
        metadata.server_kind.as_db_str(),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(metadata)
}

async fn fetch_server_metadata(
    client: &ApiClient,
    stored_kind: ServerKind,
) -> Result<ServerMetadata, AdapterError> {
    client.server_metadata(stored_kind).await
}

async fn columns(state: &RuntimeState) -> Result<Vec<ColumnSummary>, String> {
    let rows = settings::get_all_column_configs(state.database.reader())
        .await
        .map_err(|error| error.to_string())?;

    if rows.is_empty() {
        return Ok(vec![ColumnSummary {
            id: "default-home".to_string(),
            column_type: "home".to_string(),
            column_param: None,
            name: "Home".to_string(),
            max_statuses: 100,
            pane_index: 0,
            position: 0,
            account_acct: None,
            display_filter: None,
            desktop_notifications: Some(true),
            notification_sound: None,
        }]);
    }

    Ok(rows.into_iter().filter_map(column_to_summary).collect())
}

fn column_to_summary(config: DbColumnConfig) -> Option<ColumnSummary> {
    let timeline_type =
        TimelineType::from_column_config(&config.column_type, config.column_param.as_deref())?;
    let (column_param, display_filter) =
        decode_column_param_with_display_filter(&config.column_type, config.column_param);
    Some(ColumnSummary {
        id: config.id,
        column_type: config.column_type,
        column_param,
        name: config.name.unwrap_or_else(|| timeline_type.display_name()),
        max_statuses: config.max_statuses.unwrap_or(100).max(1) as u32,
        pane_index: config.pane_index.unwrap_or(0).max(0) as u32,
        position: config.position,
        account_acct: config.account_acct,
        display_filter,
        desktop_notifications: Some(config.desktop_notifications),
        notification_sound: config.notification_sound,
    })
}

pub(crate) fn normalized_column_account_acct(
    column: &ColumnSummary,
) -> Result<Option<String>, String> {
    if column_type_is_global(&column.column_type) {
        return Ok(None);
    }

    column
        .account_acct
        .as_deref()
        .map(str::trim)
        .filter(|acct| !acct.is_empty())
        .map(|acct| Some(acct.to_string()))
        .ok_or_else(|| {
            format!(
                "Timeline type '{}' requires an explicit source account",
                column.column_type
            )
        })
}

fn column_type_is_global(column_type: &str) -> bool {
    matches!(
        column_type,
        "home"
            | "public"
            | "notification"
            | "bookmarks"
            | "favourites"
            | "custom"
            | "yq"
            | "kq"
            | "search"
            | "user_bookmarks"
            | "thread"
            | "profile"
            | "airContext"
    )
}

fn timeline_type_supports_display_filter(column_type: &str) -> bool {
    !matches!(
        column_type,
        "custom"
            | "yq"
            | "kq"
            | "notification"
            | "bookmarks"
            | "favourites"
            | "user_bookmarks"
            | "thread"
            | "profile"
            | "airContext"
    )
}

fn decode_column_param_with_display_filter(
    column_type: &str,
    column_param: Option<String>,
) -> (Option<String>, Option<TimelineDisplayFilter>) {
    if !timeline_type_supports_display_filter(column_type) {
        return (column_param, None);
    }
    let Some(raw) = column_param else {
        return (None, None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return (Some(raw), None);
    };
    let Some(object) = value.as_object() else {
        return (Some(raw), None);
    };
    if !object.contains_key("filters") {
        return (Some(raw), None);
    }
    let display_filter = object
        .get("filters")
        .and_then(|filters| serde_json::from_value::<TimelineDisplayFilter>(filters.clone()).ok());
    let column_param = object
        .get("value")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    (column_param, display_filter)
}

pub(crate) fn normalized_column_request(columns: Vec<ColumnSummary>) -> Vec<ColumnSummary> {
    let mut columns = columns;
    columns.sort_by(|a, b| {
        a.pane_index
            .cmp(&b.pane_index)
            .then(a.position.cmp(&b.position))
            .then(a.name.cmp(&b.name))
    });

    let mut pane_map = std::collections::BTreeMap::<u32, u32>::new();
    let mut positions = std::collections::BTreeMap::<u32, i32>::new();

    columns
        .into_iter()
        .map(|mut column| {
            let next_pane = pane_map.len() as u32;
            let pane_index = *pane_map.entry(column.pane_index).or_insert(next_pane);
            let position = positions.entry(pane_index).or_insert(0);
            column.pane_index = pane_index;
            column.position = *position;
            column.max_statuses = column.max_statuses.max(1);
            *position += 1;
            column
        })
        .collect()
}

pub(crate) async fn acting_session(
    state: &RuntimeState,
    acct: &str,
) -> Result<AccountSession, String> {
    let acct = acct.trim();
    if acct.is_empty() {
        return Err("actingAccountAcct is required".to_string());
    }
    session_for_acct(state, acct)
        .await
        .ok_or_else(|| format!("Acting account is not signed in: {acct}"))
}

pub(crate) async fn session_for_acct(state: &RuntimeState, acct: &str) -> Option<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions.sessions().get(acct).cloned()
}

pub(crate) async fn session_for_timeline_request(
    state: &RuntimeState,
    account_acct: Option<&str>,
) -> Result<AccountSession, String> {
    let sessions = state.sessions.read().await;
    if let Some(acct) = account_acct.map(str::trim).filter(|acct| !acct.is_empty()) {
        return sessions
            .sessions()
            .get(acct)
            .cloned()
            .ok_or_else(|| format!("Account is not signed in: {}", acct));
    }
    sessions
        .active_session()
        .cloned()
        .ok_or_else(|| "No active account is signed in".to_string())
}

fn required_timeline_source_acct(account_acct: Option<&str>) -> Result<&str, String> {
    account_acct
        .map(str::trim)
        .filter(|acct| !acct.is_empty())
        .ok_or_else(|| "Timeline column is not bound to an account".to_string())
}

pub(crate) async fn session_for_timeline_source(
    state: &RuntimeState,
    account_acct: Option<&str>,
) -> Result<AccountSession, String> {
    let acct = required_timeline_source_acct(account_acct)?;
    session_for_acct(state, acct)
        .await
        .ok_or_else(|| format!("Account is not signed in: {acct}"))
}

pub(crate) async fn session_for_read_source(
    state: &RuntimeState,
    server_domain: &str,
    source_acct: Option<&str>,
) -> Result<Option<AccountSession>, String> {
    let sessions = state.sessions.read().await;
    select_read_source_session(&sessions, server_domain, source_acct)
}

fn select_read_source_session(
    sessions: &SessionManager,
    server_domain: &str,
    source_acct: Option<&str>,
) -> Result<Option<AccountSession>, String> {
    if let Some(source_acct) = source_acct.map(str::trim).filter(|acct| !acct.is_empty()) {
        let session = sessions
            .sessions()
            .get(source_acct)
            .ok_or_else(|| format!("Read source account is not signed in: {source_acct}"))?;
        if !session_matches_domain(session, server_domain) {
            return Err(format!(
                "Read source account {source_acct} does not belong to {server_domain}"
            ));
        }
        return Ok(Some(session.clone()));
    }

    let mut matches = sessions
        .sessions()
        .values()
        .filter(|session| session_matches_domain(session, server_domain));
    let first = matches.next().cloned();
    if first.is_some() && matches.next().is_some() {
        return Err(format!(
            "Read source account is required because multiple sessions belong to {server_domain}"
        ));
    }
    Ok(first)
}

fn session_matches_domain(session: &AccountSession, server_domain: &str) -> bool {
    session.client.domain().eq_ignore_ascii_case(server_domain)
        || session.domain.eq_ignore_ascii_case(server_domain)
}

async fn signed_in_sessions(state: &RuntimeState) -> Vec<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions.sessions().values().cloned().collect()
}

fn schedule_status_search_indexer(state: &RuntimeState) {
    if state.search_indexer_started.swap(true, Ordering::AcqRel) {
        return;
    }
    let database = state.database.clone();
    let emit_queue = state.emit_queue.clone();
    let cancellation = state.search_indexer_cancellation.clone();

    tauri::async_runtime::spawn(async move {
        let (progress_tx, mut progress_rx) =
            mpsc::channel::<search_indexer::SearchIndexProgress>(16);
        let progress_emitter = emit_queue.clone();
        let progress_handle = tauri::async_runtime::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                progress_emitter
                    .emit(
                        STATUS_SEARCH_BACKFILL_PROGRESS_EVENT,
                        progress,
                        "status search index progress",
                    )
                    .await;
            }
        });

        tracing::info!("Low-priority ICU status search indexer started");
        loop {
            let result = search_indexer::run(
                database.writer(),
                database.reader(),
                cancellation.clone(),
                Some(&progress_tx),
            )
            .await;
            if cancellation.is_cancelled() {
                break;
            }
            if let Err(error) = result {
                tracing::warn!(%error, "Status search indexer paused after an error");
            } else {
                break;
            }
            tokio::select! {
                _ = cancellation.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            }
        }
        drop(progress_tx);
        let _ = progress_handle.await;
        tracing::info!("Low-priority ICU status search indexer stopped");
    });
}

pub(crate) fn schedule_post_ready_work(state: &RuntimeState) {
    compose_outbox::schedule(
        state.clone(),
        &state.compose_outbox_started,
        state.compose_outbox_cancellation.clone(),
        state.compose_outbox_notify.clone(),
    );
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        // Start after the first window is ready, but do not wait for network
        // startup reconciliation to finish. The indexer probes the writer with
        // try_acquire and duty-cycle yields, so sync writes remain higher
        // priority while the searchable migration gap starts shrinking now.
        schedule_status_search_indexer(&state);
        run_startup_sync(&state).await;
    });
}

async fn run_startup_sync(state: &RuntimeState) {
    let sessions = {
        let sessions = state.sessions.read().await;
        sessions.sessions().values().cloned().collect()
    };
    sync_startup_accounts(state.emit_queue.clone(), state.database.clone(), sessions).await;
}

async fn sync_startup_accounts(
    emit_queue: QueuedEmitter,
    database: Arc<Database>,
    sessions: Vec<AccountSession>,
) {
    if sessions.is_empty() {
        return;
    }
    let mut operation = OperationContext::start("startup_sync", None, None);
    operation.phase("sync");
    tracing::info!(
        "Startup timeline sync started for {} accounts",
        sessions.len()
    );

    let (progress_tx, mut progress_rx) = mpsc::channel::<startup_sync::StartupSyncProgress>(32);
    let progress_emitter = emit_queue.clone();
    let progress_handle = tokio::spawn(async move {
        while let Some(progress) = progress_rx.recv().await {
            let kind = match progress.phase {
                startup_sync::StartupSyncPhase::Bookmarks => "bookmarkProgress",
                startup_sync::StartupSyncPhase::Favourites => "favouriteProgress",
                _ => "phaseProgress",
            };
            emit_startup_sync_event(
                &progress_emitter,
                StartupSyncEvent {
                    kind: kind.to_string(),
                    message: format!(
                        "Syncing {}: {} page {} ({})",
                        progress.phase.as_str(),
                        progress.account_acct,
                        progress.page,
                        progress.total
                    ),
                    acct: Some(progress.account_acct),
                    page: Some(progress.page),
                    total: Some(progress.total),
                },
            )
            .await;
        }
    });

    let mut failed_accounts = 0usize;
    let mut api_requests = 0_u64;
    let mut db_writes = 0_u64;
    let mut ready_ms = 0_u64;
    for session in sessions {
        let metrics =
            startup_sync::run_startup_account(&database, &session, Some(&progress_tx)).await;
        api_requests += metrics.api_requests;
        db_writes += metrics.db_writes;
        ready_ms = ready_ms.max(metrics.ready_ms);
        if metrics.failed_phases > 0 {
            failed_accounts += 1;
            tracing::warn!(
                account_acct = session.acct,
                failed_phases = metrics.failed_phases,
                "Startup synchronization completed with failed phases"
            );
        }
    }
    drop(progress_tx);
    let _ = progress_handle.await;
    crate::observability::observe_startup_sync(api_requests, db_writes, ready_ms);
    emit_startup_sync_event(
        &emit_queue,
        StartupSyncEvent {
            kind: "complete".to_string(),
            message: "Startup synchronization complete".to_string(),
            acct: None,
            page: None,
            total: None,
        },
    )
    .await;
    if failed_accounts == 0 {
        operation.finish_ok();
    } else {
        let _ = operation.finish_error(format!(
            "startup sync failed for {failed_accounts} account(s)"
        ));
    }
    tracing::info!(
        api_requests,
        db_writes,
        ready_ms,
        "Startup timeline sync finished"
    );
}

async fn emit_startup_sync_event(emit_queue: &QueuedEmitter, event: StartupSyncEvent) {
    emit_queue
        .emit(STARTUP_SYNC_COMPLETE_EVENT, event, "startup sync status")
        .await;
}

pub(crate) async fn restart_streaming(state: &RuntimeState) {
    let sessions = signed_in_sessions(state).await;
    let columns = match columns(state).await {
        Ok(columns) => columns,
        Err(error) => {
            tracing::warn!("Failed to load columns for streaming setup: {}", error);
            Vec::new()
        }
    };
    let mut handles = state.streaming_handles.write().await;
    for handle in handles.drain(..) {
        handle.abort();
    }

    if sessions.is_empty() {
        return;
    }

    let bluesky_fetch = settings_application::load_setting::<BlueskyFetchSettings>(
        &state.database,
        "bluesky_fetch",
    )
    .await
    .unwrap_or_default()
    .normalized();

    for session in sessions {
        let server_kind = session.client.kind();
        let stream_types = stream_types_for_columns(
            &columns,
            Some(&session.acct),
            Some(&session.account_info.id),
            server_kind,
        );
        if stream_types.is_empty() {
            continue;
        }
        let bluesky_poll_interval =
            Duration::from_secs(bluesky_fetch.interval_for_acct(&session.acct));

        let (tx, rx) = mpsc::channel::<TimelineEvent>(STREAM_BRIDGE_QUEUE_CAPACITY);
        let bridge_handle = tokio::spawn(forward_stream_events(
            state.emit_queue.clone(),
            state.database.clone(),
            rx,
        ));
        handles.push(bridge_handle.abort_handle());
        handles.extend(streaming_service::start_streaming(
            streaming_service::StreamingConfig {
                client: session.client.clone(),
                streaming_url: session.client.streaming_url().to_string(),
                access_token: session.client.access_token(),
                stream_types: stream_types.clone(),
                server_domain: session.domain.clone(),
                server_kind,
                source_acct: session.acct.clone(),
                database: state.database.clone(),
                event_txs: vec![tx],
                bluesky_poll_interval,
            },
        ));
    }
}

#[cfg(test)]
fn is_aggregate_timeline(
    timeline_type: &TimelineType,
    _legacy_column_account_acct: Option<&str>,
) -> bool {
    // Home and Public are unified timelines. Historical column rows may still
    // carry an account binding, but that value must never narrow provider sync
    // or the local aggregate query.
    matches!(timeline_type, TimelineType::Home | TimelineType::Public)
}

pub(crate) fn timeline_type_can_load_more_from_api(timeline_type: &TimelineType) -> bool {
    matches!(
        timeline_type,
        TimelineType::Local | TimelineType::List(_) | TimelineType::Hashtag(_)
    )
}

pub(crate) async fn refresh_aggregate_timeline(
    state: &RuntimeState,
    timeline_type: &TimelineType,
    preferred_account_acct: Option<&str>,
    limit: u32,
    display_filter: Option<TimelineDisplayFilter>,
    quote_consumer_id: Option<&str>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<(Vec<TimelineStatus>, Vec<TimelineGap>), String> {
    let sessions = signed_in_sessions(state).await;
    let mut attempted_sources = 0usize;
    let mut refreshed_sources = 0usize;
    let mut refresh_failures = Vec::new();
    let mut gaps = Vec::new();

    for session in sessions {
        let server_kind = session.client.kind();
        if !provider_supports_aggregate_refresh(server_kind, timeline_type) {
            tracing::debug!(
                source_acct = session.acct.as_str(),
                provider = ?server_kind,
                timeline = timeline_type.as_str(),
                "Skipping provider that does not expose this unified timeline"
            );
            continue;
        }
        attempted_sources += 1;
        let mut on_commit = || {
            state.emit_timeline_cache_committed(&session.acct, session.client.domain());
        };
        match timeline_service::sync_timeline_with_control(
            &session.client,
            state.database.writer(),
            timeline_type,
            &session.acct,
            &TimelineParams {
                limit: Some(limit),
                ..Default::default()
            },
            timeline_service::TimelineSyncControl {
                quote_consumer_id,
                cancellation: Some(cancellation),
                on_commit: &mut on_commit,
            },
        )
        .await
        {
            Ok(page) => {
                refreshed_sources += 1;
                gaps.extend(page.gap.map(TimelineGap::from));
            }
            Err(timeline_service::SyncError::Cancelled) => {
                return Err("timeline query cancelled".to_string());
            }
            Err(error) => {
                tracing::warn!(
                    source_acct = session.acct.as_str(),
                    provider = ?server_kind,
                    timeline = timeline_type.as_str(),
                    error = %error,
                    "Unified timeline source refresh failed; continuing with other sources and cache"
                );
                refresh_failures.push(format!("{}: {error}", session.acct));
            }
        }
    }

    let statuses = query_aggregate_timeline_statuses(
        state.database.reader(),
        &timeline_type.as_str(),
        preferred_account_acct,
        limit as i64,
        0,
        display_filter.filter(|filter| filter.applies()),
    )
    .await?;
    if attempted_sources > 0
        && refreshed_sources == 0
        && statuses.is_empty()
        && !refresh_failures.is_empty()
    {
        return Err(format!(
            "Unified {} refresh failed for every signed-in source: {}",
            timeline_type.as_str(),
            refresh_failures.join("; ")
        ));
    }
    db_status_refs_to_views(state.database.reader(), statuses)
        .await
        .map(|statuses| (statuses, gaps))
}

fn provider_supports_aggregate_refresh(
    server_kind: ServerKind,
    timeline_type: &TimelineType,
) -> bool {
    let operation = match timeline_type {
        TimelineType::Home => TimelineOperation::Home,
        TimelineType::Public => TimelineOperation::Public,
        _ => return false,
    };
    ApiClient::capabilities_for_kind(server_kind, 1)
        .timelines
        .supports(operation)
}

pub(crate) async fn refresh_aggregate_notifications(
    state: &RuntimeState,
    limit: u32,
    quote_consumer_id: Option<&str>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Vec<TimelineStatus>, String> {
    let sessions = signed_in_sessions(state).await;
    if sessions.is_empty() {
        return Err("No account is signed in".to_string());
    }

    let mut views = Vec::new();
    let mut seen = HashSet::new();
    let mut refreshed_sources = 0usize;
    let mut refresh_failures = Vec::new();
    for session in sessions {
        let notification_params = NotificationParams {
            limit: Some(limit),
            ..Default::default()
        };
        let fetch = session.client.get_notifications(&notification_params);
        let fetched = tokio::select! {
            _ = cancellation.cancelled() => return Err("timeline query cancelled".to_string()),
            result = fetch => result,
        };
        let mut notifications = match fetched {
            Ok(notifications) => {
                refreshed_sources += 1;
                notifications
            }
            Err(error) => {
                tracing::warn!(
                    source_acct = session.acct.as_str(),
                    provider = ?session.client.kind(),
                    error = %error,
                    "Unified notification source refresh failed; continuing with other sources and cache"
                );
                refresh_failures.push(format!("{}: {error}", session.acct));
                continue;
            }
        };

        for notification in &mut notifications {
            if cancellation.is_cancelled() {
                return Err("timeline query cancelled".to_string());
            }
            save_notification_to_db(
                &state.database,
                notification,
                session.client.domain(),
                &session.acct,
                || {
                    state.emit_timeline_cache_committed(&session.acct, session.client.domain());
                },
            )
            .await?;
            if let Some(status) = notification.status.as_ref() {
                if let Some(consumer_id) = quote_consumer_id {
                    timeline_service::schedule_pending_quote_resolution_for_consumer(
                        &session.client,
                        state.database.writer(),
                        std::slice::from_ref(status),
                        session.client.domain(),
                        &session.acct,
                        consumer_id,
                    );
                } else {
                    timeline_service::schedule_pending_quote_resolution(
                        &session.client,
                        state.database.writer(),
                        std::slice::from_ref(status),
                        session.client.domain(),
                        &session.acct,
                    );
                }
            }
            if seen.insert((session.client.domain().to_string(), notification.id.clone())) {
                views.push(notification_to_view(
                    notification,
                    session.client.domain(),
                    Some(&session.acct),
                ));
            }
        }
    }

    views.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    views.truncate(limit as usize);
    if refreshed_sources == 0 && views.is_empty() && !refresh_failures.is_empty() {
        let cached = crate::application::notification::query_cached_statuses(
            state.database.reader(),
            limit as i64,
            0,
        )
        .await?;
        if !cached.is_empty() {
            return Ok(cached);
        }
        return Err(format!(
            "Unified notification refresh failed for every signed-in source: {}",
            refresh_failures.join("; ")
        ));
    }
    Ok(views)
}

pub(crate) fn timeline_status_matches_display_filter(
    status: &TimelineStatus,
    display_filter: Option<TimelineDisplayFilter>,
) -> bool {
    let Some(filter) = display_filter.filter(|filter| filter.applies()) else {
        return true;
    };
    let is_boost = status.id != status.original_status_id
        || status
            .notification_kind
            .as_deref()
            .is_some_and(|kind| matches!(kind, "reblog" | "renote" | "repost"));
    let has_media = !status.media.is_empty();
    if filter.exclude_boosts && is_boost {
        return false;
    }
    if filter.exclude_media && has_media {
        return false;
    }
    if filter.include_media && !has_media {
        return false;
    }
    true
}

pub(crate) async fn query_aggregate_timeline_statuses(
    pool: &sqlx::SqlitePool,
    timeline_type: &str,
    preferred_account_acct: Option<&str>,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
) -> Result<Vec<TimelineStatusRef>, String> {
    let filter = display_filter.filter(|filter| filter.applies());
    read_models::query_aggregate_status_refs(
        pool,
        timeline_type,
        preferred_account_acct,
        limit,
        offset,
        read_models::AggregateFilter {
            exclude_boosts: filter.is_some_and(|filter| filter.exclude_boosts),
            exclude_media: filter.is_some_and(|filter| filter.exclude_media),
            include_media: filter.is_some_and(|filter| filter.include_media),
        },
    )
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|row| TimelineStatusRef {
                server_domain: row.server_domain,
                status_id: row.status_id,
                source_acct: row.source_acct,
            })
            .collect()
    })
    .map_err(|error| error.to_string())
}

pub(crate) async fn query_timeline_statuses(
    pool: &sqlx::SqlitePool,
    timeline_type: &str,
    account_acct: &str,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
) -> Result<Vec<TimelineStatusRef>, String> {
    let filter = display_filter.filter(|filter| filter.applies());
    timeline_views::query_account_timeline_status_refs(
        pool,
        timeline_type,
        account_acct,
        limit,
        offset,
        timeline_views::StatusDisplayFilter {
            exclude_boosts: filter.is_some_and(|filter| filter.exclude_boosts),
            exclude_media: filter.is_some_and(|filter| filter.exclude_media),
            include_media: filter.is_some_and(|filter| filter.include_media),
        },
    )
    .await
    .map_err(|error| error.to_string())
}

pub(crate) async fn query_custom_statuses(
    pool: &sqlx::SqlitePool,
    sql: &str,
    limit: i64,
    offset: i64,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Vec<DbStatus>, crate::db::queries::custom_timeline::CustomTimelineError> {
    crate::db::queries::custom_timeline::query_statuses(pool, sql, limit, offset, cancellation)
        .await
}

#[cfg(test)]
fn validate_custom_timeline_sql(sql: &str) -> Result<String, String> {
    crate::db::queries::custom_timeline::validate(sql).map_err(|error| error.to_string())
}

#[cfg(test)]
fn custom_sql_has_top_level_limit(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();

        if byte == b'-' && next == Some(b'-') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }

        if byte == b'/' && next == Some(b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }

        if matches!(byte, b'\'' | b'"' | b'`') {
            let quote = byte;
            index += 1;
            while index < bytes.len() {
                if bytes[index] == quote {
                    if bytes.get(index + 1) == Some(&quote) {
                        index += 2;
                        continue;
                    }
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }

        if byte == b'[' {
            index += 1;
            while index < bytes.len() && bytes[index] != b']' {
                index += 1;
            }
            index = (index + 1).min(bytes.len());
            continue;
        }

        match byte {
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            _ if depth == 0 && sql_keyword_at(bytes, index, b"limit") => return true,
            _ => index += 1,
        }
    }

    false
}

#[cfg(test)]
fn sql_keyword_at(bytes: &[u8], index: usize, keyword: &[u8]) -> bool {
    let end = index + keyword.len();
    if end > bytes.len() {
        return false;
    }
    if bytes[index..end]
        .iter()
        .zip(keyword.iter())
        .any(|(actual, expected)| actual.to_ascii_lowercase() != *expected)
    {
        return false;
    }
    !is_sql_identifier_byte(
        index
            .checked_sub(1)
            .and_then(|previous| bytes.get(previous))
            .copied(),
    ) && !is_sql_identifier_byte(bytes.get(end).copied())
}

#[cfg(test)]
fn is_sql_identifier_byte(byte: Option<u8>) -> bool {
    matches!(
        byte,
        Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
    )
}

#[cfg(test)]
pub(crate) async fn query_yq_statuses(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    stop_at: Option<(&str, &str)>,
    start_after: Option<(&str, &str)>,
) -> Result<Vec<DbStatus>, String> {
    crate::services::yq_timeline::query_statuses(
        pool,
        query,
        limit,
        offset,
        stop_at,
        start_after,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map(|result| result.statuses)
}

pub(crate) async fn query_yq_statuses_with_metrics(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    stop_at: Option<(&str, &str)>,
    start_after: Option<(&str, &str)>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<crate::services::yq_timeline::YqQueryResult, String> {
    crate::services::yq_timeline::query_statuses(
        pool,
        query,
        limit,
        offset,
        stop_at,
        start_after,
        cancellation,
    )
    .await
}
#[cfg(test)]
pub(crate) async fn query_search_statuses(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
    start_after: Option<(&str, &str)>,
) -> Result<Vec<DbStatus>, String> {
    query_search_statuses_with_cancellation(
        pool,
        query,
        limit,
        offset,
        display_filter,
        start_after,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

pub(crate) async fn query_search_statuses_with_cancellation(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
    start_after: Option<(&str, &str)>,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<Vec<DbStatus>, String> {
    let filter = display_filter.filter(|filter| filter.applies());
    search::query_statuses_with_cancellation(
        pool,
        search::SearchQuery {
            query,
            limit,
            offset,
            display_filter: timeline_views::StatusDisplayFilter {
                exclude_boosts: filter.is_some_and(|filter| filter.exclude_boosts),
                exclude_media: filter.is_some_and(|filter| filter.exclude_media),
                include_media: filter.is_some_and(|filter| filter.include_media),
            },
            start_after,
        },
        cancellation,
    )
    .await
    .map_err(|error| error.to_string())
}

pub(crate) async fn query_bookmarked_statuses(
    pool: &sqlx::SqlitePool,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, String> {
    timeline_views::query_bookmarked_status_refs(pool, account_acct, limit, offset)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn query_favourited_statuses(
    pool: &sqlx::SqlitePool,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, String> {
    timeline_views::query_favourited_status_refs(pool, account_acct, limit, offset)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn query_user_bookmarked_statuses(
    pool: &sqlx::SqlitePool,
    server_domain: &str,
    account_id: &str,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, String> {
    timeline_views::query_user_bookmarked_status_refs(
        pool,
        server_domain,
        account_id,
        account_acct,
        limit,
        offset,
    )
    .await
    .map_err(|error| error.to_string())
}

pub(crate) async fn update_cached_status_poll(
    pool: &sqlx::SqlitePool,
    request: &VotePollRequest,
    poll_json: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE statuses SET poll_json = ? WHERE id = ? AND server_domain = ?")
        .bind(poll_json)
        .bind(&request.identity.remote_id)
        .bind(&request.identity.server_domain)
        .execute(pool)
        .await?;
    Ok(())
}

pub(crate) async fn query_account_statuses(
    pool: &sqlx::SqlitePool,
    account_id: &str,
    server_domain: &str,
    only_media: bool,
    pinned: Option<bool>,
    limit: i64,
    offset: i64,
) -> Result<Vec<DbStatus>, String> {
    let sql = if only_media {
        "SELECT * FROM statuses
         WHERE account_id = ? AND server_domain = ?
           AND (
             (media_attachments_json IS NOT NULL AND media_attachments_json != '[]')
             OR EXISTS (
               SELECT 1 FROM statuses original
               WHERE original.id = statuses.reblog_of_id
                 AND original.server_domain = statuses.server_domain
                 AND original.media_attachments_json IS NOT NULL
                 AND original.media_attachments_json != '[]'
             )
           )
         ORDER BY created_at DESC, id DESC
         LIMIT ? OFFSET ?"
    } else {
        match pinned {
            Some(true) => {
                "SELECT * FROM statuses
                 WHERE account_id = ? AND server_domain = ? AND pinned = 1
                 ORDER BY created_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            }
            Some(false) => {
                "SELECT * FROM statuses
                 WHERE account_id = ? AND server_domain = ? AND (pinned IS NULL OR pinned = 0)
                 ORDER BY created_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            }
            None => {
                "SELECT * FROM statuses
                 WHERE account_id = ? AND server_domain = ?
                 ORDER BY created_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            }
        }
    };
    sqlx::query_as::<_, DbStatus>(sql)
        .bind(account_id)
        .bind(server_domain)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn query_status_thread_statuses(
    pool: &sqlx::SqlitePool,
    status_id: &str,
    server_domain: &str,
    limit: usize,
) -> Result<Vec<DbStatus>, String> {
    let Some(page) = read_models::query_thread_status_page(pool, status_id, server_domain, limit)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(Vec::new());
    };
    tracing::debug!(
        statement_count = page.statement_count,
        status_count = page.statuses.len(),
        "Loaded thread with recursive CTE"
    );
    let statuses = page
        .statuses
        .into_iter()
        .map(|status| (status.id.clone(), status))
        .collect();
    Ok(order_thread_statuses(statuses, &page.root_id))
}

pub(crate) async fn query_cached_status(
    pool: &sqlx::SqlitePool,
    status_id: &str,
    server_domain: &str,
) -> Result<Option<DbStatus>, String> {
    sqlx::query_as::<_, DbStatus>("SELECT * FROM statuses WHERE id = ? AND server_domain = ?")
        .bind(status_id)
        .bind(server_domain)
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())
}

fn order_thread_statuses(statuses: HashMap<String, DbStatus>, root_id: &str) -> Vec<DbStatus> {
    let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
    for status in statuses.values() {
        let Some(parent_id) = status.in_reply_to_id.as_ref() else {
            continue;
        };
        if statuses.contains_key(parent_id) {
            children_by_parent
                .entry(parent_id.clone())
                .or_default()
                .push(status.id.clone());
        }
    }
    for children in children_by_parent.values_mut() {
        children.sort_by(|left, right| {
            let left_status = statuses.get(left);
            let right_status = statuses.get(right);
            match (left_status, right_status) {
                (Some(left_status), Some(right_status)) => left_status
                    .created_at
                    .cmp(&right_status.created_at)
                    .then(left_status.id.cmp(&right_status.id)),
                _ => left.cmp(right),
            }
        });
    }

    let mut ordered = Vec::with_capacity(statuses.len());
    let mut visited = HashSet::new();
    append_thread_status(
        root_id,
        &statuses,
        &children_by_parent,
        &mut visited,
        &mut ordered,
    );

    let mut remaining = statuses
        .keys()
        .filter(|id| !visited.contains(*id))
        .cloned()
        .collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        let left_status = statuses.get(left);
        let right_status = statuses.get(right);
        match (left_status, right_status) {
            (Some(left_status), Some(right_status)) => left_status
                .created_at
                .cmp(&right_status.created_at)
                .then(left_status.id.cmp(&right_status.id)),
            _ => left.cmp(right),
        }
    });
    for id in remaining {
        append_thread_status(
            &id,
            &statuses,
            &children_by_parent,
            &mut visited,
            &mut ordered,
        );
    }

    ordered
}

fn append_thread_status(
    id: &str,
    statuses: &HashMap<String, DbStatus>,
    children_by_parent: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    ordered: &mut Vec<DbStatus>,
) {
    if !visited.insert(id.to_string()) {
        return;
    }
    let Some(status) = statuses.get(id) else {
        return;
    };
    ordered.push(status.clone());
    if let Some(children) = children_by_parent.get(id) {
        for child_id in children {
            append_thread_status(child_id, statuses, children_by_parent, visited, ordered);
        }
    }
}

pub(crate) fn dedupe_statuses_by_uri(statuses: &mut Vec<Status>) {
    let mut seen = HashSet::new();
    statuses.retain(|status| seen.insert(status_dedupe_uri(status)));
}

fn status_dedupe_uri(status: &Status) -> String {
    let uri = status.uri.trim();
    if uri.is_empty() {
        status.id.clone()
    } else {
        uri.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_webview_drop_exposes_image_and_video_paths() {
        let paths = vec![
            PathBuf::from("/tmp/dropped-image.png"),
            PathBuf::from("/tmp/dropped-video.mp4"),
        ];
        let event = tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop {
            paths: paths.clone(),
            position: tauri::PhysicalPosition::new(10.0, 20.0),
        });

        assert_eq!(
            dropped_paths_from_webview_event(&event),
            Some(paths.as_slice())
        );
    }

    #[tokio::test]
    async fn startup_gate_releases_snapshot_waiters_only_after_ready() {
        let gate = StartupGate::new();
        let waiter_gate = gate.clone();
        let waiter = tokio::spawn(async move { waiter_gate.wait_until_ready().await });

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        gate.mark_ready();
        assert_eq!(waiter.await.expect("waiter task"), Ok(()));
    }

    #[tokio::test]
    async fn startup_gate_propagates_a_safe_initialization_error() {
        let gate = StartupGate::new();
        gate.mark_failed("initialization failed");

        assert_eq!(
            gate.wait_until_ready().await,
            Err("initialization failed".to_string())
        );
    }

    #[test]
    fn startup_progress_event_uses_the_frontend_contract() {
        let payload = serde_json::to_value(AppStartupProgressEvent {
            stage: "database",
            status: "running",
            message: Some("Preparing the portable database".to_string()),
        })
        .expect("serialize startup progress");

        assert_eq!(payload["stage"], "database");
        assert_eq!(payload["status"], "running");
        assert_eq!(payload["message"], "Preparing the portable database");
    }

    #[test]
    fn unified_home_and_public_load_ignores_legacy_account_scope() {
        for timeline_type in [TimelineType::Home, TimelineType::Public] {
            assert!(is_aggregate_timeline(&timeline_type, None));
            assert!(is_aggregate_timeline(
                &timeline_type,
                Some("mastodon@example.test")
            ));
            assert!(is_aggregate_timeline(
                &timeline_type,
                Some("bluesky.bsky.social")
            ));
        }
        assert!(!is_aggregate_timeline(&TimelineType::Local, None));
    }

    #[test]
    fn persisted_global_columns_never_inherit_an_account_scope() {
        for column_type in [
            "home",
            "public",
            "notification",
            "bookmarks",
            "favourites",
            "custom",
            "yq",
            "kq",
            "search",
            "user_bookmarks",
            "thread",
            "profile",
            "airContext",
        ] {
            let column = stream_column(column_type, "stale@example.test");
            assert_eq!(
                normalized_column_account_acct(&column).unwrap(),
                None,
                "{column_type} must be application-scoped"
            );
        }
    }

    #[test]
    fn persisted_account_bound_columns_require_their_own_source_account() {
        for column_type in ["local", "list", "feed", "hashtag"] {
            let mut column = stream_column(column_type, " source@example.test ");
            assert_eq!(
                normalized_column_account_acct(&column).unwrap(),
                Some("source@example.test".to_string())
            );

            column.account_acct = None;
            assert!(normalized_column_account_acct(&column).is_err());
        }
    }

    #[test]
    fn kq_owns_filtering_and_never_wraps_its_query_in_a_display_filter() {
        let query = "from local where text contains \"snow\"";
        let mut column = stream_column("kq", "stale@example.test");
        column.column_param = Some(query.to_string());
        column.display_filter = Some(TimelineDisplayFilter {
            enabled: true,
            exclude_boosts: true,
            ..TimelineDisplayFilter::default()
        });

        assert_eq!(
            encode_column_param_with_display_filter(&column),
            Some(query.to_string())
        );
        let (decoded_query, decoded_filter) =
            decode_column_param_with_display_filter("kq", Some(query.to_string()));
        assert_eq!(decoded_query.as_deref(), Some(query));
        assert_eq!(decoded_filter, None);
    }

    #[test]
    fn unified_refresh_ignores_legacy_account_scope_and_skips_unsupported_public_provider() {
        assert!(is_aggregate_timeline(
            &TimelineType::Public,
            Some("stale-bluesky.bsky.social")
        ));
        for kind in [ServerKind::Mastodon, ServerKind::Paon, ServerKind::Misskey] {
            assert!(provider_supports_aggregate_refresh(
                kind,
                &TimelineType::Public
            ));
        }
        assert!(!provider_supports_aggregate_refresh(
            ServerKind::Bluesky,
            &TimelineType::Public
        ));
        for kind in [
            ServerKind::Mastodon,
            ServerKind::Paon,
            ServerKind::Misskey,
            ServerKind::Bluesky,
        ] {
            assert!(provider_supports_aggregate_refresh(
                kind,
                &TimelineType::Home
            ));
        }
    }

    #[test]
    fn account_bound_timeline_source_never_falls_back_to_active_account() {
        assert_eq!(
            required_timeline_source_acct(Some(" list-owner@example.test ")).unwrap(),
            "list-owner@example.test"
        );
        assert!(required_timeline_source_acct(None).is_err());
        assert!(required_timeline_source_acct(Some("   ")).is_err());
    }

    #[test]
    fn same_domain_read_source_is_explicit_and_never_uses_active_account() {
        fn session(acct: &str, domain: &str, id: &str) -> AccountSession {
            AccountSession {
                acct: acct.to_string(),
                domain: domain.to_string(),
                client: ApiClient::mastodon_with_kind(
                    MastodonClient::new(domain, format!("token-{id}"), format!("wss://{domain}"))
                        .unwrap(),
                    ServerKind::Mastodon,
                ),
                account_info: api_account(id, acct, acct),
            }
        }

        let mut sessions = SessionManager::new();
        sessions.add_session(session("alice@example.test", "example.test", "alice"));
        sessions.add_session(session("bob@example.test", "example.test", "bob"));
        assert!(sessions.set_active("alice@example.test"));

        let selected =
            select_read_source_session(&sessions, "example.test", Some("bob@example.test"))
                .unwrap()
                .unwrap();
        assert_eq!(selected.acct, "bob@example.test");
        assert_eq!(
            sessions
                .active_session()
                .map(|session| session.acct.as_str()),
            Some("alice@example.test")
        );
        let ambiguous = match select_read_source_session(&sessions, "example.test", None) {
            Err(error) => error,
            Ok(_) => panic!("same-domain sessions must require an explicit read source"),
        };
        assert!(ambiguous.contains("multiple sessions"));
    }

    #[test]
    fn explicit_read_source_must_match_the_requested_domain() {
        let mut sessions = SessionManager::new();
        sessions.add_session(AccountSession {
            acct: "alice@example.test".to_string(),
            domain: "example.test".to_string(),
            client: ApiClient::mastodon_with_kind(
                MastodonClient::new(
                    "example.test",
                    "token".to_string(),
                    "wss://example.test".to_string(),
                )
                .unwrap(),
                ServerKind::Mastodon,
            ),
            account_info: api_account("alice", "alice@example.test", "Alice"),
        });

        let mismatch = match select_read_source_session(
            &sessions,
            "other.example",
            Some("alice@example.test"),
        ) {
            Err(error) => error,
            Ok(_) => panic!("a read source from another domain must be rejected"),
        };
        assert!(mismatch.contains("does not belong"));
    }

    #[tokio::test]
    async fn cache_commit_signal_does_not_consume_the_ui_stream_sequence() {
        let (emit_sender, mut emit_receiver) = mpsc::channel(4);
        let emit_queue = QueuedEmitter {
            sender: emit_sender,
            detached_pending: Arc::new(AtomicBool::new(false)),
        };
        let (side_effect_sender, _side_effect_receiver) =
            mpsc::channel(STREAM_SIDE_EFFECT_QUEUE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel(4);
        let bridge = tokio::spawn(async move {
            forward_stream_events_to_queues(emit_queue, event_receiver, &side_effect_sender).await;
        });

        event_sender
            .send(TimelineEvent::CacheCommitted(
                "alice@example.test".to_string(),
                "example.test".to_string(),
            ))
            .await
            .unwrap();
        event_sender
            .send(TimelineEvent::NewStatus(
                Box::new(api_status(
                    "status-after-commit",
                    api_account("alice", "alice@example.test", "Alice"),
                    "2026-05-20T00:00:00Z",
                    "<p>committed</p>",
                )),
                crate::mastodon::types::streaming::StreamType::User,
                "alice@example.test".to_string(),
                "example.test".to_string(),
                crate::services::streaming_service::StreamPosition {
                    generation: 1,
                    sequence: 1,
                },
            ))
            .await
            .unwrap();
        drop(event_sender);

        let committed = tokio::time::timeout(Duration::from_secs(1), emit_receiver.recv())
            .await
            .expect("cache commit event reaches the WebView queue")
            .expect("emit queue remains open");
        assert_eq!(committed.event, TIMELINE_CACHE_COMMITTED_EVENT);
        let committed_payload: serde_json::Value =
            serde_json::from_str(&committed.payload).unwrap();
        assert_eq!(committed_payload["sourceAcct"], "alice@example.test");
        assert_eq!(committed_payload["serverDomain"], "example.test");

        let streamed = tokio::time::timeout(Duration::from_secs(1), emit_receiver.recv())
            .await
            .expect("stream event reaches the WebView queue")
            .expect("emit queue remains open");
        assert_eq!(streamed.event, TIMELINE_STREAM_EVENT);
        let streamed_payload: serde_json::Value = serde_json::from_str(&streamed.payload).unwrap();
        assert_eq!(streamed_payload["generation"], 1);
        assert_eq!(streamed_payload["sequence"], 1);

        bridge.await.unwrap();
    }

    #[tokio::test]
    async fn detached_cache_commit_signals_coalesce_behind_a_full_emit_queue() {
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(QueuedEmit {
                event: "prefill",
                payload: "{}".to_string(),
                detached_pending: None,
            })
            .await
            .expect("prefill emit queue");
        let pending = Arc::new(AtomicBool::new(false));
        let emitter = QueuedEmitter {
            sender,
            detached_pending: Arc::clone(&pending),
        };

        for index in 0..240 {
            emitter.emit_detached(
                TIMELINE_CACHE_COMMITTED_EVENT,
                TimelineCacheCommittedPayload {
                    source_acct: format!("source-{index}"),
                    server_domain: "example.test".to_string(),
                },
                "coalesced commit fixture",
            );
        }
        assert!(pending.load(Ordering::Acquire));

        receiver.recv().await.expect("drain prefilled event");
        let committed = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("receive coalesced cache commit")
            .expect("cache commit queue remains open");
        assert_eq!(committed.event, TIMELINE_CACHE_COMMITTED_EVENT);
        if let Some(flag) = committed.detached_pending {
            flag.store(false, Ordering::Release);
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), receiver.recv())
                .await
                .is_err(),
            "a full queue must retain one detached waiter, not one per commit"
        );
    }

    #[tokio::test]
    async fn best_effort_emit_never_waits_behind_a_full_webview_queue() {
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(QueuedEmit {
                event: "prefill",
                payload: "{}".to_string(),
                detached_pending: None,
            })
            .await
            .expect("prefill emit queue");
        let emitter = QueuedEmitter {
            sender,
            detached_pending: Arc::new(AtomicBool::new(false)),
        };

        emitter.try_emit(
            "best-effort",
            serde_json::json!({ "state": "queued" }),
            "non-blocking fixture",
        );

        let queued = receiver
            .recv()
            .await
            .expect("prefilled event remains queued");
        assert_eq!(queued.event, "prefill");
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn pending_notification_side_effect_does_not_block_following_ui_events() {
        let (emit_sender, mut emit_receiver) = mpsc::channel(4);
        let emit_queue = QueuedEmitter {
            sender: emit_sender,
            detached_pending: Arc::new(AtomicBool::new(false)),
        };
        let (side_effect_sender, mut side_effect_receiver) =
            mpsc::channel(STREAM_SIDE_EFFECT_QUEUE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel(4);
        let bridge = tokio::spawn(async move {
            forward_stream_events_to_queues(emit_queue, event_receiver, &side_effect_sender).await;
        });

        let status = api_status(
            "status-1",
            api_account("self-1", "me@example.test", "Me"),
            "2026-05-20T00:00:00Z",
            "<p>my post</p>",
        );
        let notification = Notification {
            id: "notification-1".to_string(),
            notification_type: NotificationType::Favourite,
            created_at: "2026-05-20T00:00:01Z".parse().unwrap(),
            account: api_account("actor-1", "alice@example.test", "Alice"),
            status: Some(status.clone()),
        };
        let position = crate::services::streaming_service::StreamPosition {
            generation: 1,
            sequence: 1,
        };
        event_sender
            .send(TimelineEvent::NewNotification(
                Box::new(notification),
                crate::mastodon::types::streaming::StreamType::User,
                "me@example.test".to_string(),
                "example.test".to_string(),
                position,
            ))
            .await
            .unwrap();
        event_sender
            .send(TimelineEvent::StatusUpdate(
                Box::new(status.clone()),
                "me@example.test".to_string(),
                "example.test".to_string(),
                position,
            ))
            .await
            .unwrap();
        event_sender
            .send(TimelineEvent::QuoteUpdate(
                Box::new(status),
                timeline_service::QuoteResolutionState::Unavailable,
                "me@example.test".to_string(),
                "example.test".to_string(),
                position,
            ))
            .await
            .unwrap();
        event_sender
            .send(TimelineEvent::DeleteStatus(
                "status-1".to_string(),
                "me@example.test".to_string(),
                "example.test".to_string(),
                position,
            ))
            .await
            .unwrap();

        let mut kinds = Vec::new();
        let mut quote_state = None;
        for _ in 0..4 {
            let queued = tokio::time::timeout(Duration::from_secs(1), emit_receiver.recv())
                .await
                .expect("UI event must not wait for side effects")
                .expect("UI event queue remains open");
            assert_eq!(queued.event, TIMELINE_STREAM_EVENT);
            let payload: serde_json::Value = serde_json::from_str(&queued.payload).unwrap();
            if payload["streamType"] == "quote.update" {
                quote_state = payload["status"]["quoteState"].as_str().map(str::to_string);
            }
            kinds.push(payload["kind"].as_str().unwrap().to_string());
        }
        assert_eq!(
            kinds,
            [
                "newNotification",
                "statusUpdate",
                "statusUpdate",
                "deleteStatus"
            ]
        );
        assert_eq!(quote_state.as_deref(), Some("unavailable"));

        // Deliberately leave the side-effect receiver unread until every UI
        // event arrives. The notification job remains ordered and available.
        let side_effect = side_effect_receiver.recv().await.unwrap();
        assert_eq!(side_effect.notification.id, "notification-1");
        assert!(side_effect_receiver.try_recv().is_err());

        drop(event_sender);
        bridge.await.unwrap();
    }

    #[tokio::test]
    async fn notification_is_queued_for_ui_before_its_side_effect() {
        let (emit_sender, mut emit_receiver) = mpsc::channel(1);
        emit_sender
            .send(QueuedEmit {
                event: "test-prefill",
                payload: "{}".to_string(),
                detached_pending: None,
            })
            .await
            .unwrap();
        let emit_queue = QueuedEmitter {
            sender: emit_sender,
            detached_pending: Arc::new(AtomicBool::new(false)),
        };
        let (side_effect_sender, mut side_effect_receiver) =
            mpsc::channel(STREAM_SIDE_EFFECT_QUEUE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel(1);
        let bridge = tokio::spawn(async move {
            forward_stream_events_to_queues(emit_queue, event_receiver, &side_effect_sender).await;
        });

        event_sender
            .send(TimelineEvent::NewNotification(
                Box::new(Notification {
                    id: "notification-ordered".to_string(),
                    notification_type: NotificationType::Favourite,
                    created_at: "2026-05-20T00:00:01Z".parse().unwrap(),
                    account: api_account("actor-1", "alice@example.test", "Alice"),
                    status: Some(api_status(
                        "status-ordered",
                        api_account("self-1", "me@example.test", "Me"),
                        "2026-05-20T00:00:00Z",
                        "<p>my post</p>",
                    )),
                }),
                crate::mastodon::types::streaming::StreamType::User,
                "me@example.test".to_string(),
                "example.test".to_string(),
                crate::services::streaming_service::StreamPosition {
                    generation: 1,
                    sequence: 1,
                },
            ))
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            while event_sender.capacity() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("bridge consumes the provider event");
        assert!(matches!(
            side_effect_receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let prefill = emit_receiver.recv().await.unwrap();
        assert_eq!(prefill.event, "test-prefill");
        let queued = tokio::time::timeout(Duration::from_secs(1), emit_receiver.recv())
            .await
            .expect("notification reaches UI queue")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&queued.payload).unwrap();
        assert_eq!(payload["kind"], "newNotification");
        let side_effect = tokio::time::timeout(Duration::from_secs(1), side_effect_receiver.recv())
            .await
            .expect("side effect follows UI queueing")
            .unwrap();
        assert_eq!(side_effect.notification.id, "notification-ordered");

        drop(event_sender);
        bridge.await.unwrap();
    }

    #[test]
    fn sidecar_user_style_script_has_an_empty_style_cleanup_path() {
        let script = sidecar_user_style_script("").unwrap();
        assert!(script.contains("state?.cleanup?.()"));
        assert!(script.contains("delete win[STATE_KEY]"));
        assert!(script.contains("observer?.disconnect()"));
    }

    #[test]
    fn windows_global_tauri_keeps_remote_sidecars_outside_capabilities() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../../capabilities/default.json")).unwrap();
        assert_eq!(capability["webviews"], serde_json::json!(["main"]));
        assert!(capability.get("windows").is_none());
        let permissions = capability["permissions"].as_array().unwrap();
        assert!(!permissions.iter().any(|permission| {
            permission == "core:webview:allow-create-webview"
                || permission == "core:webview:allow-webview-close"
        }));

        let windows_config: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.windows.conf.json")).unwrap();
        // tauri-plugin-frame 1.1.8 reads window.__TAURI__ to render the native
        // controls. The global wrapper does not grant sidecars an ACL match.
        assert_eq!(windows_config["app"]["withGlobalTauri"], true);
    }

    fn db_account(id: &str, acct: &str, display_name: &str) -> DbAccount {
        DbAccount {
            id: id.to_string(),
            server_domain: "example.test".to_string(),
            username: acct.to_string(),
            acct: acct.to_string(),
            display_name: display_name.to_string(),
            note: String::new(),
            avatar: format!("https://example.test/{id}.png"),
            avatar_static: String::new(),
            header: String::new(),
            locked: false,
            bot: false,
            followers_count: 0,
            following_count: 0,
            statuses_count: 0,
            created_at: "2026-05-20T00:00:00Z".to_string(),
            fetched_at: "2026-05-20T00:00:00Z".to_string(),
            fields_json: None,
            emojis_json: None,
        }
    }

    fn db_status(id: &str, account_id: &str) -> DbStatus {
        DbStatus {
            id: id.to_string(),
            server_domain: "example.test".to_string(),
            uri: format!("https://example.test/statuses/{id}"),
            url: Some(format!("https://example.test/@me/{id}")),
            created_at: "2026-05-20T00:00:00Z".to_string(),
            edited_at: None,
            account_id: account_id.to_string(),
            content: "<p>post</p>".to_string(),
            visibility: "public".to_string(),
            sensitive: false,
            spoiler_text: String::new(),
            reblogs_count: 0,
            favourites_count: 0,
            replies_count: 0,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            reblog_of_id: None,
            language: None,
            pinned: None,
            favourited: None,
            reblogged: None,
            muted: None,
            bookmarked: None,
            poll_json: None,
            card_json: None,
            application_json: None,
            mentions_json: None,
            tags_json: None,
            emojis_json: None,
            media_attachments_json: None,
            fetched_at: "2026-05-20T00:00:00Z".to_string(),
            quote_id: None,
            quote_original_url: None,
        }
    }

    fn db_notification(id: &str, actor_id: &str, status_id: Option<&str>) -> DbNotification {
        DbNotification {
            id: id.to_string(),
            server_domain: "example.test".to_string(),
            account_acct: Some("viewer@example.test".to_string()),
            notification_type: "favourite".to_string(),
            created_at: "2026-05-20T00:00:01Z".to_string(),
            account_id: actor_id.to_string(),
            status_id: status_id.map(str::to_string),
        }
    }

    async fn setup_search_test_pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _metadata| {
                Box::pin(crate::db::short_search_tokenizer::register(connection))
            })
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::raw_sql(
            "CREATE TABLE cache_counters (
                 name TEXT PRIMARY KEY,
                 value INTEGER NOT NULL CHECK (value >= 0),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO cache_counters(name, value)
             VALUES ('statuses', 0), ('accounts', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
            include_str!("../../migrations/020_create_status_search_fts.sql"),
            include_str!("../../migrations/023_resumable_status_search_backfill.sql"),
            include_str!("../../migrations/028_index_short_search_terms.sql"),
            include_str!("../../migrations/029_remove_synchronous_short_search_triggers.sql"),
            include_str!("../../migrations/030_index_global_status_cursor.sql"),
            include_str!("../../migrations/031_create_short_search_fts.sql"),
            include_str!("../../migrations/032_async_icu_status_search.sql"),
            include_str!("../../migrations/033_control_async_search_index.sql"),
            include_str!("../../migrations/034_async_icu_account_search.sql"),
            include_str!("../../migrations/035_reindex_icu_nonword_segments.sql"),
            include_str!("../../migrations/037_limit_status_icu_search_to_post_text.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        sqlx::raw_sql(
            "UPDATE status_search_icu_backfill_state
                SET completed = 1 WHERE singleton = 1;
             UPDATE account_search_icu_backfill_state
                SET completed = 1 WHERE singleton = 1;",
        )
        .execute(&pool)
        .await
        .unwrap();
        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        pool
    }

    async fn add_status_identity_test_table(pool: &sqlx::SqlitePool) {
        sqlx::query(
            "CREATE TABLE status_identities (
               status_id TEXT NOT NULL,
               server_domain TEXT NOT NULL,
               canonical_uri TEXT NOT NULL,
               PRIMARY KEY (status_id, server_domain)
             )",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    fn custom_emoji(shortcode: &str) -> crate::mastodon::types::account::CustomEmoji {
        crate::mastodon::types::account::CustomEmoji {
            shortcode: shortcode.to_string(),
            url: format!("https://example.test/emoji/{shortcode}.png"),
            static_url: format!("https://example.test/emoji/{shortcode}-static.png"),
            visible_in_picker: true,
            category: Some("Custom".to_string()),
        }
    }

    fn api_account(id: &str, acct: &str, display_name: &str) -> Account {
        Account {
            id: id.to_string(),
            username: acct.split('@').next().unwrap_or(acct).to_string(),
            acct: acct.to_string(),
            display_name: display_name.to_string(),
            note: String::new(),
            url: format!("https://example.test/@{acct}"),
            uri: format!("https://example.test/users/{id}"),
            avatar: format!("https://example.test/{id}.png"),
            avatar_static: String::new(),
            header: String::new(),
            header_static: String::new(),
            locked: false,
            bot: false,
            created_at: "2026-05-20T00:00:00Z".parse().unwrap(),
            followers_count: 0,
            following_count: 0,
            statuses_count: 0,
            fields: Vec::new(),
            emojis: Vec::new(),
            pleroma: None,
        }
    }

    fn api_status(id: &str, account: Account, created_at: &str, content: &str) -> Status {
        Status {
            id: id.to_string(),
            uri: format!("https://example.test/statuses/{id}"),
            url: Some(format!("https://example.test/@{}/{}", account.acct, id)),
            created_at: created_at.parse().unwrap(),
            edited_at: None,
            account,
            content: content.to_string(),
            visibility: "public".to_string(),
            sensitive: false,
            spoiler_text: String::new(),
            media_attachments: Vec::new(),
            mentions: Vec::new(),
            tags: Vec::new(),
            emojis: Vec::new(),
            reblogs_count: 3,
            favourites_count: 5,
            replies_count: 7,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            reblog: None,
            language: None,
            pinned: None,
            favourited: Some(false),
            reblogged: Some(false),
            muted: None,
            bookmarked: Some(false),
            poll: None,
            card: None,
            application: None,
            quote_id: None,
            quote: None,
            quote_original_url: None,
            pleroma: None,
        }
    }

    #[tokio::test]
    async fn aggregate_timeline_query_keeps_rapid_consecutive_statuses() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/005_create_timeline_entries.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        add_status_identity_test_table(&pool).await;
        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();

        for (id, created_at, content) in [
            (
                "status-older",
                "2026-05-22T06:02:25.327+00:00",
                "<p>first</p>",
            ),
            (
                "status-newer",
                "2026-05-22T06:02:25.546+00:00",
                "<p>second</p>",
            ),
        ] {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', ?)",
            )
            .bind(id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(created_at)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', 'example.test', ?, 'viewer@example.test', ?)",
            )
            .bind(id)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_aggregate_timeline_statuses(&pool, "home", None, 10, 0, None)
            .await
            .unwrap();

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].status_id, "status-newer");
        assert_eq!(
            statuses[0].source_acct.as_deref(),
            Some("viewer@example.test")
        );
        assert_eq!(statuses[1].status_id, "status-older");
    }

    #[tokio::test]
    async fn aggregate_timeline_query_prefers_acting_account_copy_for_same_uri() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/005_create_timeline_entries.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        add_status_identity_test_table(&pool).await;

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        servers::upsert_server(&pool, "remote.example", "wss://remote.example")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        let mut remote_author = db_account("author-1", "author", "Author");
        remote_author.server_domain = "remote.example".to_string();
        accounts::upsert_account(&pool, &remote_author)
            .await
            .unwrap();

        let canonical_uri = "https://origin.example/users/alice/statuses/1";
        for (id, server_domain, source_acct, position_at) in [
            (
                "local-copy",
                "example.test",
                "alice@example.test",
                "2026-05-20T00:00:00Z",
            ),
            (
                "remote-copy",
                "remote.example",
                "bob@remote.example",
                "2026-05-22T00:00:00Z",
            ),
        ] {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, ?, ?, ?, 'author-1', '<p>same post</p>')",
            )
            .bind(id)
            .bind(server_domain)
            .bind(canonical_uri)
            .bind(position_at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO status_identities (status_id, server_domain, canonical_uri)
                 VALUES (?, ?, ?)",
            )
            .bind(id)
            .bind(server_domain)
            .bind(canonical_uri)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', ?, ?, ?, ?)",
            )
            .bind(server_domain)
            .bind(id)
            .bind(source_acct)
            .bind(position_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Keep the selected account's copy outside the bounded 128-row recent
        // candidate page. It must still be selected without moving the logical
        // post away from the remote copy's newest timeline position.
        for index in 0..127 {
            let id = format!("filler-{index:03}");
            let position_at = format!("2026-05-21T{:02}:{:02}:00Z", index / 60, index % 60);
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', '<p>filler</p>')",
            )
            .bind(&id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(&position_at)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', 'example.test', ?, 'bob@remote.example', ?)",
            )
            .bind(&id)
            .bind(&position_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_aggregate_timeline_statuses(
            &pool,
            "home",
            Some("alice@example.test"),
            1,
            0,
            None,
        )
        .await
        .unwrap();

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].status_id, "local-copy");
        assert_eq!(statuses[0].server_domain, "example.test");
        assert_eq!(
            statuses[0].source_acct.as_deref(),
            Some("alice@example.test")
        );

        let remote_statuses = query_aggregate_timeline_statuses(
            &pool,
            "home",
            Some("bob@remote.example"),
            1,
            0,
            None,
        )
        .await
        .unwrap();
        assert_eq!(remote_statuses.len(), 1);
        assert_eq!(remote_statuses[0].status_id, "remote-copy");
        assert_eq!(remote_statuses[0].server_domain, "remote.example");
        assert_eq!(
            remote_statuses[0].source_acct.as_deref(),
            Some("bob@remote.example")
        );
    }

    #[tokio::test]
    async fn favourited_statuses_query_dedupes_remote_copies_by_uri() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "CREATE TABLE status_viewer_state (
               login_account_acct TEXT NOT NULL,
               status_id TEXT NOT NULL,
               server_domain TEXT NOT NULL,
               favourited INTEGER,
               reblogged INTEGER,
               muted INTEGER,
               bookmarked INTEGER,
               pinned INTEGER,
               updated_at TEXT NOT NULL,
               PRIMARY KEY (login_account_acct, status_id, server_domain)
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        servers::upsert_server(&pool, "remote.example", "wss://remote.example")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        let mut remote_author = db_account("author-1", "author", "Author");
        remote_author.server_domain = "remote.example".to_string();
        accounts::upsert_account(&pool, &remote_author)
            .await
            .unwrap();

        let canonical_uri = "https://origin.example/users/alice/statuses/1";
        for (id, server_domain, fetched_at) in [
            (
                "local-copy",
                "example.test",
                "2026-05-22T06:02:25.327+00:00",
            ),
            (
                "remote-copy",
                "remote.example",
                "2026-05-22T06:02:26.327+00:00",
            ),
        ] {
            sqlx::query(
                "INSERT INTO statuses
                   (id, server_domain, uri, created_at, fetched_at, account_id, content, favourited)
                 VALUES (?, ?, ?, '2026-05-22T06:00:00.000+00:00', ?, 'author-1', '<p>same post</p>', 1)",
            )
            .bind(id)
            .bind(server_domain)
            .bind(canonical_uri)
            .bind(fetched_at)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO status_viewer_state
                   (login_account_acct, status_id, server_domain, favourited, updated_at)
                 VALUES ('viewer@example.test', ?, ?, 1, ?)",
            )
            .bind(id)
            .bind(server_domain)
            .bind(fetched_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_favourited_statuses(&pool, Some("viewer@example.test"), 10, 0)
            .await
            .unwrap();

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].status_id, "remote-copy");
        assert_eq!(statuses[0].server_domain, "remote.example");
        assert_eq!(
            statuses[0].source_acct.as_deref(),
            Some("viewer@example.test")
        );
    }

    #[tokio::test]
    async fn favourited_statuses_query_excludes_boost_wrappers() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        sqlx::query(
            "CREATE TABLE status_viewer_state (
               login_account_acct TEXT NOT NULL,
               status_id TEXT NOT NULL,
               server_domain TEXT NOT NULL,
               favourited INTEGER,
               reblogged INTEGER,
               muted INTEGER,
               bookmarked INTEGER,
               pinned INTEGER,
               updated_at TEXT NOT NULL,
               PRIMARY KEY (login_account_acct, status_id, server_domain)
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("booster-1", "booster", "Booster"))
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO statuses
               (id, server_domain, uri, created_at, account_id, content, favourited)
             VALUES ('original', 'example.test', 'https://example.test/statuses/original',
                     '2026-05-22T06:00:00.000+00:00', 'author-1', '<p>original</p>', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO statuses
               (id, server_domain, uri, created_at, account_id, content, reblog_of_id, favourited)
             VALUES ('boost-wrapper', 'example.test', 'https://example.test/statuses/boost-wrapper',
                     '2026-05-22T06:01:00.000+00:00', 'booster-1', '', 'original', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for status_id in ["original", "boost-wrapper"] {
            sqlx::query(
                "INSERT INTO status_viewer_state
                   (login_account_acct, status_id, server_domain, favourited, updated_at)
                 VALUES ('viewer@example.test', ?, 'example.test', 1,
                         '2026-05-22T06:02:26.327+00:00')",
            )
            .bind(status_id)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_favourited_statuses(&pool, Some("viewer@example.test"), 10, 0)
            .await
            .unwrap();

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].status_id, "original");
    }

    #[tokio::test]
    async fn aggregate_timeline_query_keeps_boost_and_original_as_separate_posts() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/005_create_timeline_entries.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        add_status_identity_test_table(&pool).await;

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("booster-1", "booster", "Booster"))
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES (
               'status-1',
               'example.test',
               'https://origin.example/users/author/statuses/status-1',
               '2026-05-22T06:02:25.327+00:00',
               'author-1',
               '<p>original</p>'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO statuses
               (id, server_domain, uri, created_at, account_id, content, reblog_of_id)
             VALUES (
               'boost-1',
               'example.test',
               'https://example.test/users/booster/statuses/boost-1',
               '2026-05-22T06:02:26.327+00:00',
               'booster-1',
               '',
               'status-1'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();

        for (id, position_at) in [
            ("status-1", "2026-05-22T06:02:25.327+00:00"),
            ("boost-1", "2026-05-22T06:02:26.327+00:00"),
        ] {
            sqlx::query(
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', 'example.test', ?, 'viewer@example.test', ?)",
            )
            .bind(id)
            .bind(position_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_aggregate_timeline_statuses(&pool, "home", None, 10, 0, None)
            .await
            .unwrap();

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].status_id, "boost-1");
        assert_eq!(statuses[1].status_id, "status-1");
    }

    #[tokio::test]
    async fn aggregate_timeline_query_applies_display_filters() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/005_create_timeline_entries.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        add_status_identity_test_table(&pool).await;

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("booster-1", "booster", "Booster"))
            .await
            .unwrap();

        for (id, account_id, created_at, reblog_of_id, media_json) in [
            (
                "plain-1",
                "author-1",
                "2026-05-22T06:02:25.327+00:00",
                None,
                None,
            ),
            (
                "media-1",
                "author-1",
                "2026-05-22T06:02:26.327+00:00",
                None,
                Some(
                    "[{\"id\":\"m1\",\"type\":\"image\",\"url\":\"https://example.test/m1.png\"}]",
                ),
            ),
            (
                "boost-1",
                "booster-1",
                "2026-05-22T06:02:27.327+00:00",
                Some("plain-1"),
                None,
            ),
            (
                "boost-media-1",
                "booster-1",
                "2026-05-22T06:02:28.327+00:00",
                Some("media-1"),
                None,
            ),
        ] {
            sqlx::query(
                "INSERT INTO statuses
                   (id, server_domain, uri, created_at, account_id, content, reblog_of_id, media_attachments_json)
                 VALUES (?, 'example.test', ?, ?, ?, '<p>post</p>', ?, ?)",
            )
            .bind(id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(created_at)
            .bind(account_id)
            .bind(reblog_of_id)
            .bind(media_json)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', 'example.test', ?, 'viewer@example.test', ?)",
            )
            .bind(id)
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let exclude_boosts = query_aggregate_timeline_statuses(
            &pool,
            "home",
            None,
            10,
            0,
            Some(TimelineDisplayFilter {
                enabled: true,
                exclude_boosts: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            exclude_boosts
                .iter()
                .map(|status| status.status_id.as_str())
                .collect::<Vec<_>>(),
            vec!["media-1", "plain-1"]
        );

        let include_media = query_aggregate_timeline_statuses(
            &pool,
            "home",
            None,
            10,
            0,
            Some(TimelineDisplayFilter {
                enabled: true,
                include_media: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            include_media
                .iter()
                .map(|status| status.status_id.as_str())
                .collect::<Vec<_>>(),
            vec!["boost-media-1", "media-1"]
        );

        let exclude_media = query_aggregate_timeline_statuses(
            &pool,
            "home",
            None,
            10,
            0,
            Some(TimelineDisplayFilter {
                enabled: true,
                exclude_media: true,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            exclude_media
                .iter()
                .map(|status| status.status_id.as_str())
                .collect::<Vec<_>>(),
            vec!["boost-1", "plain-1"]
        );
    }

    #[test]
    fn custom_sql_limit_detection_uses_top_level_keyword_only() {
        assert!(custom_sql_has_top_level_limit(
            "SELECT * FROM statuses ORDER BY created_at DESC LIMIT 10"
        ));
        assert!(custom_sql_has_top_level_limit(
            "SELECT * FROM statuses\n-- paging\nLIMIT 10"
        ));
        assert!(!custom_sql_has_top_level_limit(
            "SELECT * FROM statuses WHERE content LIKE '%limit 10%'"
        ));
        assert!(!custom_sql_has_top_level_limit(
            "SELECT * FROM statuses WHERE id IN (SELECT status_id FROM timeline_entries LIMIT 10)"
        ));
        assert!(!custom_sql_has_top_level_limit(
            "SELECT limit_value FROM statuses"
        ));
    }

    #[test]
    fn custom_sql_validator_allows_one_select_and_rejects_bypasses() {
        assert_eq!(
            validate_custom_timeline_sql(
                "-- a harmless comment\nSELECT s.* FROM statuses s WHERE content = 'delete; pragma' ; /* done */"
            )
            .unwrap(),
            "-- a harmless comment\nSELECT s.* FROM statuses s WHERE content = 'delete; pragma'"
        );

        for sql in [
            "PRAGMA table_info(statuses)",
            "ATTACH DATABASE '/tmp/other.db' AS other",
            "SELECT * FROM pragma_table_info('statuses')",
            "SELECT load_extension('untrusted')",
            "SELECT * FROM statuses; DELETE FROM statuses",
            "SELECT * FROM statuses;;",
            "WITH picked AS (DELETE FROM statuses RETURNING *) SELECT * FROM picked",
            "UPDATE statuses SET content = '' RETURNING *",
        ] {
            assert!(
                validate_custom_timeline_sql(sql).is_err(),
                "validator accepted {sql}"
            );
        }
    }

    #[tokio::test]
    async fn custom_sql_inner_limit_keeps_legacy_single_page_semantics() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();

        for (index, created_at) in ["0003", "0002", "0001"].iter().enumerate() {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', '<p>post</p>')",
            )
            .bind(format!("status-{index}"))
            .bind(format!("https://example.test/statuses/status-{index}"))
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_custom_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY created_at DESC LIMIT 2",
            1,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        let second_page = query_custom_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY created_at DESC LIMIT 2",
            1,
            1,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].id, "status-0");
        assert_eq!(statuses[1].id, "status-1");
        assert!(second_page.is_empty());
    }

    #[tokio::test]
    async fn custom_sql_without_limit_uses_column_limit_and_offset() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();

        for (index, created_at) in ["0003", "0002", "0001"].iter().enumerate() {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', '<p>post</p>')",
            )
            .bind(format!("status-{index}"))
            .bind(format!("https://example.test/statuses/status-{index}"))
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let page = query_custom_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY created_at DESC",
            1,
            1,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "status-1");
    }

    #[tokio::test]
    async fn custom_sql_enforces_result_and_execution_budgets() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        sqlx::query(
            "WITH RECURSIVE seq(value) AS (
                 SELECT 1
                 UNION ALL
                 SELECT value + 1 FROM seq WHERE value < 180
             )
             INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             SELECT
                 printf('status-%03d', value),
                 'example.test',
                 printf('https://example.test/statuses/%03d', value),
                 printf('%04d', value),
                 'author-1',
                 '<p>post</p>'
             FROM seq",
        )
        .execute(&pool)
        .await
        .unwrap();

        let capped = query_custom_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY created_at DESC",
            1_000,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(capped.len(), CUSTOM_SQL_MAX_RESULT_ROWS as usize);

        let error = query_custom_statuses(
            &pool,
            "SELECT s.* FROM statuses s
             CROSS JOIN statuses a
             CROSS JOIN statuses b
             CROSS JOIN statuses c
             ORDER BY random()",
            10,
            0,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            crate::db::queries::custom_timeline::CustomTimelineError::ExecutionBudget
        ));

        // The progress callback and defense-in-depth query_only flag must not
        // leak into the pooled connection after either success or interruption.
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES ('after-budget', 'example.test', 'https://example.test/after-budget', '9999', 'author-1', '')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn search_query_matches_all_terms_across_status_and_account_fields() {
        let pool = setup_search_test_pool().await;
        accounts::upsert_account(&pool, &db_account("author-1", "alice", "Needle Author"))
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-2", "bob", "Other Author"))
            .await
            .unwrap();

        for (id, account_id, content) in [
            ("match", "author-1", "<p>alpha post</p>"),
            ("content-only", "author-2", "<p>alpha post</p>"),
            ("account-only", "author-1", "<p>ordinary post</p>"),
        ] {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, '2026-05-22T06:00:00.000+00:00', ?, ?)",
            )
            .bind(id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(account_id)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_search_statuses(&pool, "alpha needle", 10, 0, None, None)
            .await
            .unwrap();

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "match");
    }

    #[tokio::test]
    async fn search_uses_exact_fallback_while_icu_backfill_is_incomplete() {
        let pool = setup_search_test_pool().await;
        accounts::upsert_account(&pool, &db_account("author-1", "alice", "Alice"))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES (
                 'not-indexed-yet', 'example.test',
                 'https://example.test/statuses/not-indexed-yet',
                 '2026-05-22T06:00:00.000+00:00', 'author-1',
                 '<p>resumable exact needle</p>'
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE status_search_icu_backfill_state
                SET cursor_status_id = NULL, cursor_server_domain = NULL,
                    processed_count = 0, total_count = 1, completed = 0
              WHERE singleton = 1",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "DELETE FROM status_search_index_queue
              WHERE status_id = 'not-indexed-yet'
                AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "DELETE FROM account_search_index_queue
              WHERE account_id = 'author-1'
                AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(!crate::services::search_indexer::is_complete(&pool)
            .await
            .unwrap());
        assert_eq!(
            crate::services::search_indexer::pending_count(&pool)
                .await
                .unwrap(),
            0,
            "fixture must exercise the migration-gap fallback, not the live queue"
        );
        let statuses = query_search_statuses(&pool, "exact needle", 10, 0, None, None)
            .await
            .unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "not-indexed-yet");
    }

    #[tokio::test]
    async fn search_query_paginates_stably_when_created_at_ties() {
        let pool = setup_search_test_pool().await;
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();

        for index in 0..5 {
            let id = format!("status-{index}");
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, '2026-05-22T06:00:00.000+00:00', 'author-1', '<p>needle post</p>')",
            )
            .bind(&id)
            .bind(format!("https://example.test/statuses/{id}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let first_page = query_search_statuses(&pool, "needle", 2, 0, None, None)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES ('new-between-pages', 'example.test', 'https://example.test/statuses/new-between-pages', '2026-05-22T06:00:01.000+00:00', 'author-1', '<p>needle post</p>')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let page_cursor = first_page.last().unwrap();
        let second_page = query_search_statuses(
            &pool,
            "needle",
            2,
            2,
            None,
            Some((page_cursor.id.as_str(), page_cursor.server_domain.as_str())),
        )
        .await
        .unwrap();

        assert_eq!(
            first_page
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            vec!["status-4", "status-3"]
        );
        assert_eq!(
            second_page
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            vec!["status-2", "status-1"]
        );
    }

    #[tokio::test]
    async fn search_fts_tracks_status_and_account_changes() {
        let pool = setup_search_test_pool().await;
        accounts::upsert_account(&pool, &db_account("author-1", "alice", "Initial Author"))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES ('status-1', 'example.test', 'https://example.test/statuses/status-1', '0001', 'author-1', '<p>needlevalue 100%_safe 東京</p>')",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::services::search_indexer::run_index_step(&pool, &pool)
            .await
            .unwrap();
        crate::services::search_indexer::run_index_step(&pool, &pool)
            .await
            .unwrap();

        assert_eq!(
            query_search_statuses(&pool, "needle", 10, 0, None, None)
                .await
                .unwrap()
                .len(),
            1
        );
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES ('false-candidate', 'example.test', 'https://example.test/statuses/false-candidate', '0000', 'author-1', '<p>東 separated 京</p>')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // ICU word-prefix candidates do not join separated Japanese text.
        assert_eq!(
            query_search_statuses(&pool, "東京", 10, 0, None, None)
                .await
                .unwrap()
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            vec!["status-1"]
        );
        // Punctuation-only terms remain on the ICU segmented FTS path.
        assert_eq!(
            query_search_statuses(&pool, "%_", 10, 0, None, None)
                .await
                .unwrap()
                .len(),
            1
        );

        sqlx::query(
            "UPDATE statuses SET content = '<p>replacementtoken</p>'
             WHERE id = 'status-1' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(query_search_statuses(&pool, "needle", 10, 0, None, None)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            query_search_statuses(&pool, "replacement", 10, 0, None, None)
                .await
                .unwrap()
                .len(),
            1
        );

        sqlx::query(
            "UPDATE accounts SET display_name = 'Renamed Searchable Author'
             WHERE id = 'author-1' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            query_search_statuses(&pool, "searchable", 10, 0, None, None)
                .await
                .unwrap()
                .len(),
            2
        );

        sqlx::query(
            "DELETE FROM statuses WHERE id = 'status-1' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            query_search_statuses(&pool, "replacement", 10, 0, None, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn async_icu_indexer_backfills_existing_statuses() {
        let options = "sqlite::memory:"
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .unwrap()
            .shared_cache(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            // The production indexer has independent reader and writer pools.
            // Its low-priority writer probe is non-blocking, so a one-slot
            // fixture can observe `WriterBusy` while SQLx asynchronously
            // returns the preceding read connection.
            .min_connections(2)
            .max_connections(2)
            .after_connect(|connection, _metadata| {
                Box::pin(crate::db::short_search_tokenizer::register(connection))
            })
            .connect_with(options)
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(
            &pool,
            &db_account("author-1", "backfilled-author", "Backfilled Author"),
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             VALUES ('status-1', 'example.test', 'https://example.test/statuses/status-1', '0001', 'author-1', '<p>preexisting searchable post</p>')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(
            "CREATE TABLE cache_counters (
                 name TEXT PRIMARY KEY,
                 value INTEGER NOT NULL CHECK (value >= 0),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO cache_counters(name, value)
             VALUES ('statuses', (SELECT COUNT(*) FROM statuses)),
                    ('accounts', (SELECT COUNT(*) FROM accounts));",
        )
        .execute(&pool)
        .await
        .unwrap();
        for migration in [
            include_str!("../../migrations/020_create_status_search_fts.sql"),
            include_str!("../../migrations/023_resumable_status_search_backfill.sql"),
            include_str!("../../migrations/028_index_short_search_terms.sql"),
            include_str!("../../migrations/029_remove_synchronous_short_search_triggers.sql"),
            include_str!("../../migrations/030_index_global_status_cursor.sql"),
            include_str!("../../migrations/031_create_short_search_fts.sql"),
            include_str!("../../migrations/032_async_icu_status_search.sql"),
            include_str!("../../migrations/033_control_async_search_index.sql"),
            include_str!("../../migrations/034_async_icu_account_search.sql"),
            include_str!("../../migrations/035_reindex_icu_nonword_segments.sql"),
            include_str!("../../migrations/037_limit_status_icu_search_to_post_text.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        for _ in 0..8 {
            if crate::services::search_indexer::is_complete(&pool)
                .await
                .unwrap()
            {
                break;
            }
            crate::services::search_indexer::run_index_step(&pool, &pool)
                .await
                .unwrap();
        }
        assert!(crate::services::search_indexer::is_complete(&pool)
            .await
            .unwrap());

        let by_content = query_search_statuses(&pool, "preexisting", 10, 0, None, None)
            .await
            .unwrap();
        let by_account = query_search_statuses(&pool, "backfilled-author", 10, 0, None, None)
            .await
            .unwrap();
        assert_eq!(by_content.len(), 1);
        assert_eq!(by_account.len(), 1);

        // statuses has no INTEGER PRIMARY KEY alias, so its implicit rowid is
        // not stable across maintenance. ICU search joins through its own
        // content mapping and must not depend on that physical row number.
        sqlx::query(
            "UPDATE statuses SET rowid = rowid + 1000
             WHERE id = 'status-1' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();
        settings::vacuum(&pool).await.unwrap();
        assert_eq!(
            query_search_statuses(&pool, "preexisting", 10, 0, None, None)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn search_uses_fts_virtual_index_on_a_realistic_cache() {
        let pool = setup_search_test_pool().await;
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        sqlx::query(
            "WITH RECURSIVE seq(value) AS (
                 SELECT 1
                 UNION ALL
                 SELECT value + 1 FROM seq WHERE value < 20000
             )
             INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             SELECT
                 printf('status-%04d', value),
                 'example.test',
                 printf('https://example.test/statuses/%04d', value),
                 printf('%04d', value),
                 'author-1',
                 CASE WHEN value = 19999 THEN '<p>unique-search-needle</p>' ELSE '<p>ordinary cached post</p>' END
             FROM seq",
        )
        .execute(&pool)
        .await
        .unwrap();
        let token_text = crate::db::icu_search::index_text([
            "<p>unique-search-needle</p>",
            "",
            "https://example.test/statuses/19999",
            "",
            "",
        ]);
        sqlx::query(
            "INSERT INTO status_search_icu_content(
                 status_id, server_domain, token_text, text_scope_version
             ) VALUES ('status-19999', 'example.test', ?, ?)",
        )
        .bind(token_text)
        .bind(crate::db::icu_search::STATUS_TEXT_SCOPE_VERSION)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM status_search_index_queue")
            .execute(&pool)
            .await
            .unwrap();

        let plan = sqlx::query_as::<_, (i64, i64, i64, String)>(
            "EXPLAIN QUERY PLAN
             SELECT s.id
             FROM status_search_icu_fts
             JOIN status_search_icu_content search_document
               ON search_document.docid = status_search_icu_fts.rowid
             JOIN statuses s
               ON s.id = search_document.status_id
              AND s.server_domain = search_document.server_domain
             WHERE status_search_icu_fts MATCH ?",
        )
        .bind(crate::db::icu_search::match_expression("unique-search-needle").unwrap())
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(
            plan.iter()
                .any(|(_, _, _, detail)| detail.contains("VIRTUAL TABLE INDEX")),
            "{plan:?}"
        );

        let statuses = query_search_statuses(&pool, "unique-search-needle", 10, 0, None, None)
            .await
            .unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "status-19999");
    }

    #[test]
    fn cached_notification_status_displays_status_author() {
        let actor = db_account("actor-1", "alice", "Alice");
        let status_author = db_account("self-1", "me", "Me");
        let own_status = db_status("status-1", "self-1");
        let notification = db_notification("notification-1", "actor-1", Some("status-1"));

        let view = notification_db_to_view(
            notification,
            Some(actor),
            Some(own_status),
            Some(status_author),
        );

        assert_eq!(view.id, "notification-1");
        assert_eq!(view.notification_id.as_deref(), Some("notification-1"));
        assert_eq!(view.original_status_id, "status-1");
        assert_eq!(view.account_id, "self-1");
        assert_eq!(view.display_name, "Me");
        assert_eq!(view.acct, "@me");
        assert_eq!(view.created_at, "2026-05-20T00:00:01Z");
        assert_eq!(
            view.original_created_at.as_deref(),
            Some("2026-05-20T00:00:00Z")
        );
        assert_eq!(view.notification_label.as_deref(), Some("Alice favourited"));
        assert_eq!(view.notification_account_id.as_deref(), Some("actor-1"));
        assert_eq!(view.notification_acct.as_deref(), Some("@alice"));
        assert_eq!(view.notification_display_name.as_deref(), Some("Alice"));
        assert_eq!(
            view.notification_avatar.as_deref(),
            Some("https://example.test/actor-1.png")
        );
    }

    #[test]
    fn api_notification_status_displays_status_author() {
        let actor = api_account("actor-1", "alice@example.test", "Alice");
        let status_author = api_account("self-1", "me@example.test", "Me");
        let status = api_status(
            "status-1",
            status_author,
            "2026-05-20T00:00:00Z",
            "<p>my post</p>",
        );
        let notification = crate::mastodon::types::notification::Notification {
            id: "notification-1".to_string(),
            notification_type: crate::mastodon::types::notification::NotificationType::Favourite,
            created_at: "2026-05-20T00:00:01Z".parse().unwrap(),
            account: actor,
            status: Some(status),
        };

        let view = notification_to_view(&notification, "example.test", Some("viewer@example.test"));

        assert_eq!(view.id, "notification-1");
        assert_eq!(view.source_acct.as_deref(), Some("viewer@example.test"));
        assert_eq!(view.notification_id.as_deref(), Some("notification-1"));
        assert_eq!(view.original_status_id, "status-1");
        assert_eq!(view.account_id, "self-1");
        assert_eq!(view.display_name, "Me");
        assert_eq!(view.acct, "@me@example.test");
        assert_eq!(view.created_at, "2026-05-20T00:00:01+00:00");
        assert_eq!(
            view.original_created_at.as_deref(),
            Some("2026-05-20T00:00:00+00:00")
        );
        assert_eq!(view.notification_label.as_deref(), Some("Alice favourited"));
        assert_eq!(view.notification_account_id.as_deref(), Some("actor-1"));
        assert_eq!(
            view.notification_acct.as_deref(),
            Some("@alice@example.test")
        );
        assert_eq!(view.notification_display_name.as_deref(), Some("Alice"));
        assert_eq!(
            view.notification_avatar.as_deref(),
            Some("https://example.test/actor-1.png")
        );
    }

    #[test]
    fn api_notifications_for_same_status_keep_distinct_notification_identity() {
        let status = api_status(
            "status-1",
            api_account("self-1", "me@example.test", "Me"),
            "2026-05-20T00:00:00Z",
            "<p>my post</p>",
        );
        let first = crate::mastodon::types::notification::Notification {
            id: "notification-1".to_string(),
            notification_type: crate::mastodon::types::notification::NotificationType::Reblog,
            created_at: "2026-05-20T00:00:01Z".parse().unwrap(),
            account: api_account("actor-1", "alice@example.test", "Alice"),
            status: Some(status.clone()),
        };
        let second = crate::mastodon::types::notification::Notification {
            id: "notification-2".to_string(),
            notification_type: crate::mastodon::types::notification::NotificationType::Reblog,
            created_at: "2026-05-20T00:00:02Z".parse().unwrap(),
            account: api_account("actor-2", "bob@example.test", "Bob"),
            status: Some(status),
        };

        let first_view = notification_to_view(&first, "example.test", Some("viewer@example.test"));
        let second_view =
            notification_to_view(&second, "example.test", Some("viewer@example.test"));

        assert_eq!(first_view.original_status_id, "status-1");
        assert_eq!(second_view.original_status_id, "status-1");
        assert_eq!(first_view.id, "notification-1");
        assert_eq!(second_view.id, "notification-2");
        assert_eq!(
            first_view.notification_id.as_deref(),
            Some("notification-1")
        );
        assert_eq!(
            second_view.notification_id.as_deref(),
            Some("notification-2")
        );
        assert_eq!(
            first_view.notification_label.as_deref(),
            Some("Alice boosted")
        );
        assert_eq!(
            second_view.notification_label.as_deref(),
            Some("Bob boosted")
        );
    }

    #[test]
    fn api_reblog_status_displays_original_post_with_boost_context() {
        let original = api_status(
            "status-1",
            api_account("author-1", "author@example.test", "Author"),
            "2026-05-20T00:00:00Z",
            "<p>original post</p>",
        );
        let mut boost = api_status(
            "boost-1",
            api_account("booster-1", "booster@example.test", "Booster"),
            "2026-05-20T00:00:05Z",
            "",
        );
        boost.reblog = Some(Box::new(original));

        let view = status_to_view(&boost, "example.test", None);

        assert_eq!(view.id, "boost-1");
        assert_eq!(view.uri, "https://example.test/statuses/boost-1");
        assert_eq!(view.original_status_id, "status-1");
        assert_eq!(view.account_id, "author-1");
        assert_eq!(view.display_name, "Author");
        assert_eq!(view.acct, "@author@example.test");
        assert_eq!(view.created_at, "2026-05-20T00:00:05+00:00");
        assert_eq!(
            view.original_created_at.as_deref(),
            Some("2026-05-20T00:00:00+00:00")
        );
        assert_eq!(view.content, "<p>original post</p>");
        assert_eq!(view.notification_label.as_deref(), Some("Booster boosted"));
        assert_eq!(
            view.notification_avatar.as_deref(),
            Some("https://example.test/booster-1.png")
        );
        assert_eq!(view.notification_account_id.as_deref(), Some("booster-1"));
        assert_eq!(
            view.notification_acct.as_deref(),
            Some("@booster@example.test")
        );
        assert_eq!(view.notification_display_name.as_deref(), Some("Booster"));
    }

    #[test]
    fn status_dedupe_by_uri_keeps_boost_and_original_distinct() {
        let original = api_status(
            "status-1",
            api_account("author-1", "author@example.test", "Author"),
            "2026-05-20T00:00:00Z",
            "<p>original post</p>",
        );
        let mut boost = api_status(
            "boost-1",
            api_account("booster-1", "booster@example.test", "Booster"),
            "2026-05-20T00:00:05Z",
            "",
        );
        boost.reblog = Some(Box::new(original.clone()));

        let mut statuses = vec![original, boost];
        dedupe_statuses_by_uri(&mut statuses);

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].id, "status-1");
        assert_eq!(statuses[1].id, "boost-1");
    }

    #[tokio::test]
    async fn yq_query_scans_beyond_first_raw_page_until_display_limit() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

        sqlx::query(
            "INSERT INTO servers (domain, streaming_url) VALUES ('example.test', 'wss://example.test')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let account = db_account("author-1", "author", "Author");
        accounts::upsert_account(&pool, &account).await.unwrap();

        for index in 0..(YQ_FILTER_PAGE_SIZE + 10) {
            let content = if index >= YQ_FILTER_PAGE_SIZE {
                "<p>needle post</p>"
            } else {
                "<p>ordinary post</p>"
            };
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', ?)",
            )
            .bind(format!("status-{index}"))
            .bind(format!("https://example.test/statuses/status-{index}"))
            .bind(format!("{:05}", 10_000 - index))
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let first_page =
            query_yq_statuses(&pool, "where (regex text \"needle\")", 3, 0, None, None)
                .await
                .unwrap();
        let second_page =
            query_yq_statuses(&pool, "where (regex text \"needle\")", 3, 3, None, None)
                .await
                .unwrap();

        assert_eq!(first_page.len(), 3);
        assert_eq!(first_page[0].id, format!("status-{YQ_FILTER_PAGE_SIZE}"));
        assert_eq!(
            first_page[2].id,
            format!("status-{}", YQ_FILTER_PAGE_SIZE + 2)
        );
        assert_eq!(second_page.len(), 3);
        assert_eq!(
            second_page[0].id,
            format!("status-{}", YQ_FILTER_PAGE_SIZE + 3)
        );
    }

    #[tokio::test]
    async fn yq_query_stops_at_since_status() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

        sqlx::query(
            "INSERT INTO servers (domain, streaming_url) VALUES ('example.test', 'wss://example.test')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let account = db_account("author-1", "author", "Author");
        accounts::upsert_account(&pool, &account).await.unwrap();

        for (id, created_at, content) in [
            ("status-new-match", "0004", "<p>needle after current</p>"),
            ("status-new-other", "0003", "<p>ordinary after current</p>"),
            // The boundary itself intentionally does not match. A SQL
            // prefilter must still stop before the older matching row.
            ("status-current", "0002", "<p>ordinary current</p>"),
            ("status-old-match", "0001", "<p>needle before current</p>"),
        ] {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', ?)",
            )
            .bind(id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(created_at)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let page = query_yq_statuses(
            &pool,
            "where (contains text \"needle\")",
            10,
            0,
            Some(("status-current", "example.test")),
            None,
        )
        .await
        .unwrap();

        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "status-new-match");
    }

    #[tokio::test]
    async fn yq_contains_prefilter_has_no_false_negatives_for_rendered_html() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }
        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();

        for (id, created_at, content) in [
            // The visible match crosses an HTML tag boundary and is not a raw
            // contiguous substring.
            ("tag-spanning", "0004", "<p>#えあい<em></em>さん</p>"),
            // The ampersand exists only after entity decoding.
            ("entity-decoded", "0003", "<p>fish&amp;chips</p>"),
            // This satisfies the conservative subsequence predicate but not
            // the authoritative YQ substring expression.
            ("sql-false-positive", "0002", "<p>filler i chips</p>"),
            ("unrelated", "0001", "<p>ordinary post</p>"),
        ] {
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES (?, 'example.test', ?, ?, 'author-1', ?)",
            )
            .bind(id)
            .bind(format!("https://example.test/statuses/{id}"))
            .bind(created_at)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_yq_statuses(
            &pool,
            "where (or (contains text \"#えあいさん\") (contains text \"fish&chips\"))",
            10,
            0,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            statuses
                .iter()
                .map(|status| status.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tag-spanning", "entity-decoded"]
        );
    }

    #[tokio::test]
    async fn yq_query_cursor_is_stable_across_servers_with_duplicate_ids() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }

        for domain in ["a.example", "z.example"] {
            servers::upsert_server(&pool, domain, &format!("wss://{domain}"))
                .await
                .unwrap();
            let mut account = db_account("author-1", "author", "Author");
            account.server_domain = domain.to_string();
            accounts::upsert_account(&pool, &account).await.unwrap();
            sqlx::query(
                "INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
                 VALUES ('same-id', ?, ?, '0001', 'author-1', '<p>post</p>')",
            )
            .bind(domain)
            .bind(format!("https://{domain}/statuses/same-id"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let first = query_yq_statuses(&pool, "where t", 1, 0, None, None)
            .await
            .unwrap();
        let second = query_yq_statuses(
            &pool,
            "where t",
            1,
            1,
            None,
            Some((first[0].id.as_str(), first[0].server_domain.as_str())),
        )
        .await
        .unwrap();
        assert_eq!(first[0].server_domain, "z.example");
        assert_eq!(second[0].server_domain, "a.example");
    }

    #[tokio::test]
    async fn yq_query_can_find_a_rare_match_beyond_the_old_fixed_scan_budget() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }
        servers::upsert_server(&pool, "example.test", "wss://example.test")
            .await
            .unwrap();
        accounts::upsert_account(&pool, &db_account("author-1", "author", "Author"))
            .await
            .unwrap();
        // Mirrors the number of rows the reported production query had to
        // traverse before finding 100 matches, while remaining inexpensive
        // enough for the regular test suite.
        const STATUS_COUNT: usize = 200_000;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "WITH RECURSIVE seq(value) AS (
                 SELECT 1
                 UNION ALL
                 SELECT value + 1 FROM seq WHERE value < {STATUS_COUNT}
             )
             INSERT INTO statuses (id, server_domain, uri, created_at, account_id, content)
             SELECT
                 printf('status-%05d', value),
                 'example.test',
                 printf('https://example.test/statuses/%05d', value),
                 printf('%05d', {STATUS_COUNT} - value),
                 'author-1',
                 CASE WHEN value = {STATUS_COUNT}
                      THEN '<p>#しばふさんちの今日のごはん</p>'
                      ELSE '<p>ordinary cached post</p>'
                 END
             FROM seq"
        )))
        .execute(&pool)
        .await
        .unwrap();

        let statuses = query_yq_statuses(
            &pool,
            "(or
              (contains text \"#えあいさんちの今日のごはん\")
              (contains text \"#しばふさんちの今日のごはん\")
            )",
            1,
            0,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "status-200000");
    }

    #[test]
    fn api_status_view_includes_status_and_account_custom_emojis() {
        let mut account = api_account("author-1", "author@example.test", "Author :blob:");
        account.emojis = vec![custom_emoji("blob")];
        let mut status = api_status(
            "status-1",
            account,
            "2026-05-20T00:00:00Z",
            "<p>hello :wave:</p>",
        );
        status.emojis = vec![custom_emoji("wave")];

        let view = status_to_view(&status, "example.test", None);

        assert_eq!(view.emojis.len(), 1);
        assert_eq!(view.emojis[0].shortcode, "wave");
        assert_eq!(view.account_emojis.len(), 1);
        assert_eq!(view.account_emojis[0].shortcode, "blob");
    }

    #[test]
    fn api_status_view_includes_quoted_status() {
        let quote = api_status(
            "quote-1",
            api_account("quote-author-1", "quote@example.test", "Quote Author"),
            "2026-05-20T00:00:00Z",
            "<p>quoted post</p>",
        );
        let mut status = api_status(
            "status-1",
            api_account("author-1", "author@example.test", "Author"),
            "2026-05-20T00:00:10Z",
            "<p>quote reply</p>",
        );
        status.quote_id = Some("quote-1".to_string());
        status.quote = Some(Box::new(quote));
        status.quote_original_url = Some("https://example.test/@quote/quote-1".to_string());

        let view = status_to_view(&status, "example.test", None);

        assert_eq!(view.quote_id.as_deref(), Some("quote-1"));
        assert_eq!(
            view.quote_original_url.as_deref(),
            Some("https://example.test/@quote/quote-1")
        );
        let quote = view.quote.as_ref().expect("quoted status");
        assert_eq!(quote.original_status_id, "quote-1");
        assert_eq!(quote.display_name, "Quote Author");
        assert_eq!(quote.content, "<p>quoted post</p>");
    }

    #[test]
    fn db_status_view_includes_cached_custom_emojis() {
        let mut account = db_account("author-1", "author", "Author :blob:");
        account.emojis_json = serde_json::to_string(&vec![custom_emoji("blob")]).ok();
        let mut status = db_status("status-1", "author-1");
        status.content = "<p>hello :wave:</p>".to_string();
        status.emojis_json = serde_json::to_string(&vec![custom_emoji("wave")]).ok();

        let view = db_status_to_view(status, Some(account));

        assert_eq!(view.emojis.len(), 1);
        assert_eq!(view.emojis[0].shortcode, "wave");
        assert_eq!(view.account_emojis.len(), 1);
        assert_eq!(view.account_emojis[0].shortcode, "blob");
    }

    #[test]
    fn db_status_view_exposes_quote_metadata() {
        let account = db_account("author-1", "author", "Author");
        let mut status = db_status("status-1", "author-1");
        status.quote_id = Some("quote-1".to_string());
        status.quote_original_url = Some("https://example.test/@quote/quote-1".to_string());

        let view = db_status_to_view(status, Some(account));

        assert_eq!(view.quote_id.as_deref(), Some("quote-1"));
        assert_eq!(
            view.quote_original_url.as_deref(),
            Some("https://example.test/@quote/quote-1")
        );
        assert!(view.quote.is_none());
    }

    #[test]
    fn cached_reblog_view_exposes_booster_account_metadata() {
        let original = db_status("status-1", "author-1");
        let mut boost = db_status("boost-1", "booster-1");
        boost.created_at = "2026-05-20T00:00:05Z".to_string();
        boost.content = String::new();
        boost.reblog_of_id = Some("status-1".to_string());
        let original_account = db_account("author-1", "author", "Author");
        let mut booster_account = db_account("booster-1", "booster", "Booster :boost:");
        booster_account.emojis_json = serde_json::to_string(&vec![custom_emoji("boost")]).ok();

        let context = CachedStatusViewContext {
            statuses: HashMap::from([(status_key("status-1", "example.test"), original.clone())]),
            accounts: HashMap::from([
                (status_key("author-1", "example.test"), original_account),
                (status_key("booster-1", "example.test"), booster_account),
            ]),
        };

        let view = context.status_to_view_resolving_reblog(boost);

        assert_eq!(view.id, "boost-1");
        assert_eq!(view.original_status_id, "status-1");
        assert_eq!(view.created_at, "2026-05-20T00:00:05Z");
        assert_eq!(
            view.original_created_at.as_deref(),
            Some("2026-05-20T00:00:00Z")
        );
        assert_eq!(
            view.notification_label.as_deref(),
            Some("Booster :boost: boosted")
        );
        assert_eq!(
            view.notification_avatar.as_deref(),
            Some("https://example.test/booster-1.png")
        );
        assert_eq!(view.notification_account_id.as_deref(), Some("booster-1"));
        assert_eq!(view.notification_acct.as_deref(), Some("@booster"));
        assert_eq!(
            view.notification_display_name.as_deref(),
            Some("Booster :boost:")
        );
        assert_eq!(view.notification_account_emojis.len(), 1);
        assert_eq!(view.notification_account_emojis[0].shortcode, "boost");
    }

    #[test]
    fn account_profile_view_includes_cached_custom_emojis() {
        let mut account = db_account("author-1", "author", "Author :blob:");
        account.emojis_json = serde_json::to_string(&vec![custom_emoji("blob")]).ok();

        let view = account_profile_to_view(account, false, None, false, None);

        assert_eq!(view.account_emojis.len(), 1);
        assert_eq!(view.account_emojis[0].shortcode, "blob");
    }

    #[test]
    fn profile_refresh_preserves_cached_media_when_api_media_is_empty() {
        let mut fresh = db_account("author-1", "author", "Author :blob:");
        fresh.avatar.clear();
        fresh.avatar_static.clear();
        fresh.header.clear();
        fresh.emojis_json = None;
        let mut cached = db_account("author-1", "author", "Author :blob:");
        cached.avatar = "https://example.test/cached-avatar.png".to_string();
        cached.avatar_static = "https://example.test/cached-avatar-static.png".to_string();
        cached.header = "https://example.test/cached-header.png".to_string();
        cached.emojis_json = serde_json::to_string(&vec![custom_emoji("blob")]).ok();

        preserve_cached_profile_media(&mut fresh, &cached);

        assert_eq!(fresh.avatar, "https://example.test/cached-avatar.png");
        assert_eq!(
            fresh.avatar_static,
            "https://example.test/cached-avatar-static.png"
        );
        assert_eq!(fresh.header, "https://example.test/cached-header.png");
        assert_eq!(fresh.emojis_json, cached.emojis_json);
    }

    fn stream_column(column_type: &str, account_acct: &str) -> ColumnSummary {
        ColumnSummary {
            id: format!("{column_type}-{account_acct}"),
            column_type: column_type.to_string(),
            column_param: None,
            name: column_type.to_string(),
            max_statuses: 100,
            pane_index: 0,
            position: 0,
            account_acct: Some(account_acct.to_string()),
            display_filter: None,
            desktop_notifications: Some(true),
            notification_sound: None,
        }
    }

    #[test]
    fn unified_home_and_notification_streams_ignore_stale_column_account() {
        use crate::mastodon::types::streaming::StreamType;

        let columns = vec![
            stream_column("home", "stale-bluesky.bsky.social"),
            stream_column("notification", "stale-bluesky.bsky.social"),
        ];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::User]
        );
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("bob@example.test"),
                None,
                ServerKind::Misskey,
            ),
            vec![StreamType::User]
        );
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("bluesky.bsky.social"),
                None,
                ServerKind::Bluesky,
            ),
            vec![StreamType::User, StreamType::UserNotification]
        );
        assert_eq!(
            stream_types_for_columns(
                &[stream_column("notification", "stale-bluesky.bsky.social")],
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon
            ),
            vec![StreamType::UserNotification]
        );
    }

    #[test]
    fn unified_public_stream_uses_activitypub_sessions_but_not_bluesky() {
        use crate::mastodon::types::streaming::StreamType;

        let columns = vec![stream_column("public", "stale-bluesky.bsky.social")];
        for kind in [ServerKind::Mastodon, ServerKind::Paon, ServerKind::Misskey] {
            assert_eq!(
                stream_types_for_columns(&columns, Some("any@example.test"), None, kind),
                vec![StreamType::Public]
            );
        }
        assert!(stream_types_for_columns(
            &columns,
            Some("stale-bluesky.bsky.social"),
            None,
            ServerKind::Bluesky
        )
        .is_empty());
    }

    #[test]
    fn public_stream_is_not_opened_without_a_public_column() {
        use crate::mastodon::types::streaming::StreamType;

        let columns = vec![
            stream_column("home", "stale@example.test"),
            stream_column("notification", "stale@example.test"),
        ];
        for kind in [ServerKind::Mastodon, ServerKind::Paon, ServerKind::Misskey] {
            let streams =
                stream_types_for_columns(&columns, Some("actor@example.test"), None, kind);
            assert!(!streams.contains(&StreamType::Public));
        }
    }

    #[test]
    fn account_scoped_list_stream_uses_only_its_column_account() {
        use crate::mastodon::types::streaming::StreamType;

        let mut list = stream_column("list", "alice@example.test");
        list.column_param = Some("friends".to_string());
        assert_eq!(
            stream_types_for_columns(
                std::slice::from_ref(&list),
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon
            ),
            vec![StreamType::List("friends".to_string())]
        );
        assert!(stream_types_for_columns(
            &[list],
            Some("bob@example.test"),
            None,
            ServerKind::Mastodon,
        )
        .is_empty());
        let mut unbound = stream_column("list", "discarded@example.test");
        unbound.account_acct = None;
        unbound.column_param = Some("friends".to_string());
        assert!(stream_types_for_columns(
            &[unbound],
            Some("active-must-not-be-used@example.test"),
            None,
            ServerKind::Mastodon
        )
        .is_empty());
    }

    #[test]
    fn release_webview_smoke_accepts_only_loopback_http_with_explicit_port() {
        assert_eq!(
            validated_release_webview_smoke_url("http://127.0.0.1:43123/"),
            Some("http://127.0.0.1:43123".to_string())
        );
        assert_eq!(
            validated_release_webview_smoke_url("http://localhost:43123"),
            Some("http://localhost:43123".to_string())
        );
        assert!(validated_release_webview_smoke_url("https://127.0.0.1:43123").is_none());
        assert!(validated_release_webview_smoke_url("http://example.test:43123").is_none());
        assert!(validated_release_webview_smoke_url("http://127.0.0.1").is_none());
    }

    #[test]
    fn ipc_operation_header_accepts_only_uuid_values() {
        let mut headers = tauri::http::HeaderMap::new();
        headers.insert(
            "x-awayuki-operation-id",
            "22222222-2222-4222-8222-222222222222"
                .parse()
                .expect("valid header"),
        );
        assert_eq!(
            operation_id_from_headers(&headers).as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        headers.insert(
            "x-awayuki-operation-id",
            "secret-not-a-uuid".parse().expect("valid header text"),
        );
        assert_eq!(operation_id_from_headers(&headers), None);
    }

    #[tokio::test]
    async fn cancellable_read_keeps_the_client_operation_id() {
        let manager = OperationCancellationManager::default();
        let operation_id = "11111111-1111-4111-8111-111111111111";
        let task = tokio::spawn(run_cancellable_read(
            manager.clone(),
            "account_profile",
            Some(operation_id),
            std::future::pending::<Result<(), String>>(),
        ));
        tokio::task::yield_now().await;

        assert!(manager.cancel(operation_id));
        let error = task
            .await
            .expect("join cancellable read")
            .expect_err("cancelled read must fail");
        assert_eq!(error.code, AppErrorCode::Cancelled);
        assert_eq!(error.request_id, operation_id);
    }

    #[tokio::test]
    async fn cancellable_mutation_drops_the_provider_future_without_retry() {
        let manager = OperationCancellationManager::default();
        let operation_id = "33333333-3333-4333-8333-333333333333";
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let future_dropped = dropped.clone();
        let task = tokio::spawn(run_cancellable_app_mutation(
            manager.clone(),
            Some(operation_id),
            async move {
                struct DropMarker(Arc<std::sync::atomic::AtomicBool>);
                impl Drop for DropMarker {
                    fn drop(&mut self) {
                        self.0.store(true, std::sync::atomic::Ordering::Release);
                    }
                }
                let _marker = DropMarker(future_dropped);
                std::future::pending::<Result<(), AppError>>().await
            },
        ));
        tokio::task::yield_now().await;

        assert!(manager.cancel(operation_id));
        let error = task
            .await
            .expect("join cancellable mutation")
            .expect_err("cancelled mutation must fail");
        assert_eq!(error.code, AppErrorCode::Cancelled);
        assert_eq!(error.request_id, operation_id);
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    }
}
