use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use apple_ai::{AppleAiClient, GenerationOptions, Message};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite};
use tauri::webview::{DownloadEvent, NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewBuilder, WebviewUrl,
    WebviewWindow, WindowEvent,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch, RwLock};
use url::Url;

use crate::api::client::ApiClient;
use crate::api::detect::detect_server_kind;
use crate::api::http::{download_client, MAX_DOWNLOAD_BYTES};
use crate::api::kind::ServerKind;
use crate::api::ports::ServerMetadata;
use crate::application::sidecar_policy::{self, SidecarPolicy};
use crate::application::startup_gate::{RetryStartError, StartupGate};
use crate::auth::callback_server;
use crate::auth::credential_store::{AccountCredentials, CredentialStore};
use crate::auth::session::{AccountSession, SessionManager};
use crate::bluesky::auth::login_with_app_password;
use crate::bluesky::client::{BlueskyClient, DEFAULT_BLUESKY_HOST};
use crate::constants::{APP_VERSION, DEFAULT_TIMELINE_LIMIT};
use crate::db::models::{DbAccount, DbColumnConfig, DbLoginAccount, DbNotification, DbStatus};
use crate::db::pool::Database;
use crate::db::queries::{
    accounts, notification_mutes, read_models, servers, settings, statuses as status_queries, tags,
};
use crate::domain::adapter_error::AdapterError;
use crate::domain::capability::{
    RelationshipOperation, SessionCapabilities, StatusOperation, TimelineOperation,
};
use crate::domain::identity::{FederationProtocol, StatusIdentity};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::mastodon::client::MastodonClient;
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::statuses::{CreatePollParams, CreateStatusParams, VotePollParams};
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::oauth::OAuthFlow;
use crate::mastodon::types::account::Account;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::{MediaAttachment, Poll, Status, StatusApplication};
use crate::misskey::auth::MiAuthFlow;
use crate::misskey::client::MisskeyClient;
use crate::observability::{
    DiagnosticsSnapshot, OperationContext, SupportBundle, SupportBundleRequest,
};
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::{self, TimelineType};
use crate::services::{search_backfill, startup_sync};
use crate::state::account_source_color::AccountSourceColor;
use crate::state::appearance::AppearanceSettings;
use crate::state::bluesky_fetch::BlueskyFetchSettings;
use crate::state::confirmation::{ConfirmationSettings, TranslationEngine};
use crate::state::debug_settings::DebugSettings;
use crate::state::logging;
use crate::state::media_upload::MediaUploadManager;
use crate::state::notifications::NotificationSuppressionList;
use crate::state::paths;
use crate::state::performance::{PerformanceSettings, SuggestionSource};
use crate::state::preset_visibility::PresetVisibilitySettings;
use crate::state::storage_security;

#[derive(Clone)]
pub struct RuntimeState {
    database: Arc<Database>,
    credentials: CredentialStore,
    sessions: Arc<RwLock<SessionManager>>,
    streaming_handles: Arc<RwLock<Vec<tokio::task::AbortHandle>>>,
    emit_queue: QueuedEmitter,
    media_uploads: Arc<MediaUploadManager>,
    startup: StartupGate,
    started_at: Instant,
}

#[derive(Clone)]
struct QueuedEmitter {
    sender: mpsc::Sender<QueuedEmit>,
}

struct QueuedEmit {
    event: &'static str,
    payload: String,
}

impl QueuedEmitter {
    fn start(app_handle: AppHandle) -> Self {
        let (sender, mut receiver) = mpsc::channel::<QueuedEmit>(EMIT_QUEUE_CAPACITY);
        tauri::async_runtime::spawn(async move {
            while let Some(queued) = receiver.recv().await {
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
        Self { sender }
    }

    async fn emit<T>(&self, event: &'static str, payload: T, context: &str)
    where
        T: Serialize,
    {
        let Ok(payload) = serialize_emit_payload(event, payload, context) else {
            return;
        };
        if let Err(error) = self.sender.send(QueuedEmit { event, payload }).await {
            tracing::warn!(event, context, "Failed to queue Tauri event: {}", error);
        }
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
const STARTUP_SYNC_COMPLETE_EVENT: &str = "timeline-startup-sync-complete";
const APP_STARTUP_PROGRESS_EVENT: &str = "app-startup-progress";
const STATUS_SEARCH_BACKFILL_PROGRESS_EVENT: &str = "status-search-backfill-progress";
const EMIT_QUEUE_CAPACITY: usize = 1024;
const STREAM_BRIDGE_QUEUE_CAPACITY: usize = 256;
const MASTODON_DEFAULT_CHARACTER_LIMIT: i32 = 500;
const MISSKEY_DEFAULT_CHARACTER_LIMIT: i32 = 3000;
const SIDECAR_MIN_WIDTH: u32 = 160;
const SIDECAR_DEFAULT_WIDTH: u32 = 500;
const BLUESKY_CHARACTER_LIMIT: i32 = 300;
const YQ_FILTER_PAGE_SIZE: i64 = 250;
const YQ_MIN_SCANNED_ROWS: usize = 25_000;
const YQ_ABSOLUTE_MAX_SCANNED_ROWS: usize = 2_000_000;
const YQ_MIN_QUERY_DURATION: Duration = Duration::from_secs(15);
const YQ_MAX_QUERY_DURATION: Duration = Duration::from_secs(120);
const YQ_QUERY_DURATION_PER_100K_STATUSES: Duration = Duration::from_secs(10);
#[cfg(test)]
const CUSTOM_SQL_MAX_RESULT_ROWS: i64 = crate::db::queries::custom_timeline::MAX_RESULT_ROWS;

#[derive(Debug, Clone, Copy)]
struct YqQueryBudget {
    max_scanned_rows: usize,
    max_duration: Duration,
}

impl YqQueryBudget {
    fn for_status_count(status_count: usize) -> Self {
        let max_scanned_rows = status_count
            .saturating_add(YQ_FILTER_PAGE_SIZE as usize)
            .clamp(YQ_MIN_SCANNED_ROWS, YQ_ABSOLUTE_MAX_SCANNED_ROWS);
        let row_steps = status_count.div_ceil(100_000).max(1);
        let adaptive_duration = YQ_MIN_QUERY_DURATION.saturating_add(
            YQ_QUERY_DURATION_PER_100K_STATUSES
                .saturating_mul(row_steps.min(u32::MAX as usize) as u32),
        );
        Self {
            max_scanned_rows,
            max_duration: adaptive_duration.min(YQ_MAX_QUERY_DURATION),
        }
    }
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

pub(crate) fn command_error(command: &'static str, error: String) -> AppError {
    let request_id = uuid::Uuid::new_v4().to_string();
    tracing::warn!(
        command,
        request_id,
        cause = %crate::observability::redact_text(&error),
        "Application use case failed at the IPC boundary"
    );
    AppError::from_source(error, request_id)
}

struct TimelineCommandLogContext<'a> {
    command: &'a str,
    column_type: &'a str,
    column_param: &'a Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    since_status_id: &'a Option<String>,
    since_server_domain: &'a Option<String>,
    started_at: Instant,
}

fn log_timeline_command_result(
    context: &TimelineCommandLogContext<'_>,
    result: &Result<Vec<TimelineStatus>, String>,
) {
    match result {
        Ok(statuses) => tracing::info!(
            command = context.command,
            column_type = context.column_type,
            column_param = ?context.column_param,
            limit = ?context.limit,
            offset = ?context.offset,
            since_status_id = ?context.since_status_id,
            since_server_domain = ?context.since_server_domain,
            count = statuses.len(),
            duration_ms = elapsed_ms(context.started_at),
            "[awayuki][tauri-command] timeline command success"
        ),
        Err(error) => tracing::info!(
            command = context.command,
            column_type = context.column_type,
            column_param = ?context.column_param,
            limit = ?context.limit,
            offset = ?context.offset,
            since_status_id = ?context.since_status_id,
            since_server_domain = ?context.since_server_domain,
            duration_ms = elapsed_ms(context.started_at),
            "[awayuki][tauri-command] timeline command error: {}",
            error
        ),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedWindowState {
    /// "windowed", "maximized", or "fullscreen".
    state: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct RawSavedWindowState {
    state: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountSummary {
    acct: String,
    server_domain: String,
    account_id: String,
    display_name: String,
    avatar: String,
    is_active: bool,
    server_kind: String,
    character_limit: i32,
    rate_limit: Option<AccountRateLimitSummary>,
    capabilities: SessionCapabilities,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountListSummary {
    id: String,
    title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountRateLimitSummary {
    limit: u32,
    remaining: u32,
    used: u32,
    reset_in_seconds: i64,
    observed_ago_seconds: i64,
    policy: Option<String>,
    used_fraction: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountFieldSummary {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountRelationshipSummary {
    following: bool,
    followed_by: bool,
    requested: bool,
    blocking: bool,
    muting: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountProfileSummary {
    id: String,
    server_domain: String,
    username: String,
    acct: String,
    url: Option<String>,
    display_name: String,
    note: String,
    avatar: String,
    header: String,
    fields: Vec<AccountFieldSummary>,
    account_emojis: Vec<CustomEmojiView>,
    statuses_count: i64,
    following_count: i64,
    followers_count: i64,
    is_self: bool,
    relationship: Option<AccountRelationshipSummary>,
    notification_muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColumnSummary {
    id: String,
    column_type: String,
    column_param: Option<String>,
    name: String,
    max_statuses: u32,
    pane_index: u32,
    position: i32,
    account_acct: Option<String>,
    display_filter: Option<TimelineDisplayFilter>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct TimelineDisplayFilter {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    exclude_boosts: bool,
    #[serde(default)]
    exclude_media: bool,
    #[serde(default)]
    include_media: bool,
}

impl TimelineDisplayFilter {
    fn applies(self) -> bool {
        self.enabled && (self.exclude_boosts || self.exclude_media || self.include_media)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DbSummary {
    path: String,
    size: String,
    status_count: i64,
    recent_status_count: i64,
    account_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusBarSnapshot {
    status_count: i64,
    recent_status_count: i64,
    uptime_seconds: u64,
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
struct AppStartupProgressEvent {
    stage: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsSnapshot {
    appearance: AppearanceSettings,
    performance: PerformanceSettings,
    confirmation: ConfirmationSettings,
    bluesky_fetch: BlueskyFetchSettings,
    sidecars: SidecarSettings,
    account_source_colors: HashMap<String, AccountSourceColor>,
    preset_visibility: PresetVisibilitySettings,
    debug: DebugSettings,
    notification_suppression: NotificationSuppressionList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarEntry {
    id: String,
    name: String,
    url: String,
    #[serde(default)]
    user_style_enabled: bool,
    #[serde(default)]
    user_style: String,
    width: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct SidecarSettings {
    #[serde(default)]
    entries: Vec<SidecarEntry>,
    #[serde(default)]
    main_view_index: usize,
}

impl SidecarSettings {
    fn normalized(self) -> Result<Self, String> {
        let mut entries = Vec::new();
        for entry in self.entries {
            let id = entry.id.trim().to_string();
            let name = entry.name.trim().to_string();
            let url = entry.url.trim().to_string();
            if id.is_empty() {
                return Err("Sidecar id is empty".to_string());
            }
            SidecarPolicy::parse_initial_url(&url)?;
            entries.push(SidecarEntry {
                id,
                name: if name.is_empty() {
                    "Sidecar".to_string()
                } else {
                    name
                },
                url,
                user_style_enabled: entry.user_style_enabled,
                user_style: entry.user_style,
                width: if entry.width == 0 {
                    SIDECAR_DEFAULT_WIDTH
                } else {
                    entry.width.max(SIDECAR_MIN_WIDTH)
                },
            });
        }

        Ok(Self {
            entries,
            main_view_index: 0,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSidecarWebviewRequest {
    sidecar_id: String,
    url: String,
    user_style: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
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
                tracing::warn!(
                    target: "awayuki::sidecar",
                    sidecar = %label_for_navigation,
                    url = %url,
                    "Blocked sidecar navigation outside its initial origin"
                );
            }
            allowed
        })
        .on_new_window(move |url, _| {
            let allowed = popup_policy.allows_popup(&url);
            tracing::warn!(
                target: "awayuki::sidecar",
                sidecar = %label_for_new_window,
                url = %url,
                "Blocked sidecar popup; new-window requests are denied by policy"
            );
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

    if let Err(error) = window.add_child(
        builder,
        LogicalPosition::new(x, y),
        LogicalSize::new(width, height),
    ) {
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
pub(crate) struct NotificationMutedAccountSummary {
    account_id: String,
    server_domain: String,
    acct: String,
    display_name: String,
    avatar: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineStatus {
    id: String,
    original_status_id: String,
    status_identity: StatusIdentity,
    source_acct: Option<String>,
    account_id: String,
    server_domain: String,
    uri: String,
    url: Option<String>,
    display_name: String,
    acct: String,
    avatar: String,
    created_at: String,
    original_created_at: Option<String>,
    in_reply_to_id: Option<String>,
    in_reply_to_account_id: Option<String>,
    content: String,
    spoiler_text: String,
    language: Option<String>,
    application_name: Option<String>,
    reblogs_count: i64,
    favourites_count: i64,
    replies_count: i64,
    visibility: String,
    sensitive: bool,
    favourited: bool,
    reblogged: bool,
    bookmarked: bool,
    media: Vec<MediaAttachment>,
    poll: Option<PollView>,
    emojis: Vec<CustomEmojiView>,
    account_emojis: Vec<CustomEmojiView>,
    quote_id: Option<String>,
    quote_original_url: Option<String>,
    quote: Option<Box<TimelineStatus>>,
    notification_id: Option<String>,
    notification_kind: Option<String>,
    notification_label: Option<String>,
    notification_avatar: Option<String>,
    notification_account_id: Option<String>,
    notification_acct: Option<String>,
    notification_display_name: Option<String>,
    notification_account_emojis: Vec<CustomEmojiView>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct TimelineStatusRef {
    server_domain: String,
    status_id: String,
    source_acct: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StatusSourceAcctRef {
    server_domain: String,
    status_id: String,
    source_acct: Option<String>,
}

type StatusCacheKey = (String, String);

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineRequest {
    #[serde(default)]
    operation_id: Option<String>,
    column_type: String,
    column_param: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    max_status_id: Option<String>,
    max_server_domain: Option<String>,
    since_status_id: Option<String>,
    since_server_domain: Option<String>,
    account_acct: Option<String>,
    display_filter: Option<TimelineDisplayFilter>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelinePageResponse {
    statuses: Vec<TimelineStatus>,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountListsRequest {
    acct: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountProfileRequest {
    account_id: String,
    server_domain: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountTimelineRequest {
    account_id: String,
    server_domain: String,
    only_media: Option<bool>,
    pinned: Option<bool>,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusThreadRequest {
    status_id: String,
    server_domain: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AirContextRequest {
    status_id: String,
    server_domain: String,
    account_id: String,
    account_acct: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountFollowRequest {
    account_id: String,
    server_domain: String,
    target_acct: String,
    acting_account_acct: String,
    action: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountNotificationMuteRequest {
    account_id: String,
    server_domain: String,
    muted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginInstanceRequest {
    domain: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginBlueskyRequest {
    identifier: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostRequest {
    #[serde(default)]
    operation_id: Option<String>,
    acting_account_acct: String,
    status: String,
    visibility: Option<String>,
    spoiler_text: Option<String>,
    sensitive: Option<bool>,
    media_ids: Option<Vec<String>>,
    in_reply_to_id: Option<String>,
    quote_id: Option<String>,
    poll: Option<PostPollRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostPollRequest {
    options: Vec<String>,
    multiple: bool,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BeginMediaUploadRequest {
    acting_account_acct: String,
    filename: String,
    mime_type: String,
    size: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BeginMediaUploadResponse {
    upload_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppendMediaUploadRequest {
    upload_id: String,
    data: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaUploadProgressResponse {
    written: u64,
    total: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaUploadIdRequest {
    upload_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimDroppedMediaPathRequest {
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaimDroppedMediaPathResponse {
    capability: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UploadMediaPathRequest {
    acting_account_acct: String,
    path: String,
    capability: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComposeSuggestionRequest {
    query: String,
    limit: Option<u32>,
    account_acct: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MentionSuggestionView {
    acct: String,
    display_name: String,
    avatar: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HashtagSuggestionView {
    name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomEmojiView {
    shortcode: String,
    url: String,
    static_url: String,
    category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PollOptionView {
    title: String,
    votes_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PollView {
    id: String,
    expires_at: Option<String>,
    expired: bool,
    multiple: bool,
    votes_count: i64,
    voters_count: Option<i64>,
    options: Vec<PollOptionView>,
    voted: Option<bool>,
    own_votes: Option<Vec<i64>>,
    emojis: Vec<CustomEmojiView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSettingsRequest {
    key: String,
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslateStatusRequest {
    text: String,
    source_language: Option<String>,
    target_language: String,
    #[serde(default)]
    translation_engine: Option<TranslationEngine>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranslateStatusResponse {
    text: String,
    source_language: Option<String>,
    target_language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveColumnsRequest {
    columns: Vec<ColumnSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExplainCustomTimelineRequest {
    sql: String,
    #[serde(default)]
    operation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusActionRequest {
    identity: StatusIdentity,
    acting_account_acct: String,
    action: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VotePollRequest {
    identity: StatusIdentity,
    acting_account_acct: String,
    poll_id: String,
    choices: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditStatusRequest {
    identity: StatusIdentity,
    acting_account_acct: String,
    account_id: String,
    status: String,
    visibility: Option<String>,
    spoiler_text: Option<String>,
    sensitive: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteStatusRequest {
    identity: StatusIdentity,
    acting_account_acct: String,
    account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadMediaRequest {
    url: String,
    suggested_filename: Option<String>,
}

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().skip_logger().build());

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

    builder
        .setup(|app| {
            // Opening the SQLite pools is intentionally the only synchronous
            // setup work. Schema migration, session restoration and service
            // startup run after the WebView exists so a large portable
            // database can show progress instead of presenting a frozen app.
            let state = tauri::async_runtime::block_on(open_runtime_state(app.handle().clone()))?;
            if let Some(window) = app.get_webview_window("main") {
                install_drop_path_registration(window, Arc::clone(&state.media_uploads));
            }
            app.manage(state.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            crate::ipc::runtime::app_snapshot,
            crate::ipc::runtime::start_runtime_initialization,
            crate::ipc::runtime::retry_runtime_initialization,
            crate::ipc::account::account_summaries,
            crate::ipc::account::account_lists,
            crate::ipc::auth::login_with_instance_domain,
            crate::ipc::auth::login_with_bluesky_app_password,
            crate::ipc::timeline::load_timeline,
            crate::ipc::timeline::load_more_timeline,
            crate::ipc::timeline::refresh_timeline,
            crate::ipc::timeline::status_thread,
            crate::ipc::timeline::air_context,
            crate::ipc::account::account_profile,
            crate::ipc::account::account_timeline,
            crate::ipc::account::account_follow_action,
            crate::ipc::account::notification_muted_accounts,
            crate::ipc::account::set_account_notification_mute,
            crate::ipc::compose::post_status,
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
            crate::ipc::maintenance::explain_custom_timeline,
            crate::ipc::maintenance::vacuum_database,
            crate::ipc::maintenance::clear_status_cache,
            crate::ipc::maintenance::status_bar_snapshot,
            crate::ipc::maintenance::diagnostics_snapshot,
            crate::ipc::maintenance::support_bundle,
            crate::ipc::compose::status_action,
            crate::ipc::media::download_media,
            crate::ipc::media::open_status_url,
            crate::ipc::sidecar::create_sidecar_webview,
            crate::ipc::sidecar::navigate_sidecar_webview,
            crate::ipc::sidecar::reload_sidecar_webview,
            crate::ipc::sidecar::close_sidecar_webview,
            crate::ipc::sidecar::scroll_sidecar_webview_to_top,
            crate::ipc::sidecar::inject_sidecar_user_style,
            crate::ipc::media::open_log_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
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

    let database_open_started = Instant::now();
    let database = Arc::new(Database::new(&db_path).await?);
    tracing::info!(
        duration_ms = elapsed_ms(database_open_started),
        "SQLite connection pools opened"
    );

    Ok(RuntimeState {
        database,
        credentials: CredentialStore::sqlite(),
        sessions: Arc::new(RwLock::new(SessionManager::new())),
        streaming_handles: Arc::new(RwLock::new(Vec::new())),
        emit_queue: QueuedEmitter::start(app_handle),
        media_uploads: Arc::new(MediaUploadManager::default()),
        startup: StartupGate::new(),
        started_at: Instant::now(),
    })
}

fn schedule_runtime_initialization(state: RuntimeState, app_handle: AppHandle) {
    if !state.startup.begin_initialization() {
        tracing::warn!("Skipped duplicate initial runtime worker");
        return;
    }
    spawn_runtime_initialization_worker(state, app_handle);
}

pub(crate) async fn start_runtime_initialization_impl(
    state: State<'_, RuntimeState>,
    app: AppHandle,
) -> Result<(), AppError> {
    // This command is invoked only after React registered its progress
    // listener. Duplicate handshakes are coalesced by StartupGate.
    schedule_runtime_initialization(state.inner().clone(), app);
    Ok(())
}

fn spawn_runtime_initialization_worker(state: RuntimeState, app_handle: AppHandle) {
    // Some SQLx migration futures hold a transaction-local executor and are
    // intentionally !Send. Drive that future on one background worker instead
    // of forcing it onto Tauri's multi-thread task scheduler. The WebView/main
    // thread remains free while the worker awaits SQLite and provider setup.
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(async move {
            let mut operation = OperationContext::start("startup", None, None);
            match initialize_runtime_state(&state, &app_handle, &operation).await {
                Ok(()) => operation.finish_ok(),
                Err(error) => {
                    tracing::error!(error = %error, "Awayuki background initialization failed");
                    let public_message =
                        "Awayuki could not initialize its database and account sessions";
                    state.startup.mark_failed(public_message);
                    emit_app_startup_progress(
                        &state,
                        "error",
                        "error",
                        Some(public_message.to_string()),
                    )
                    .await;
                    let _ = operation.finish_error(error);
                }
            }
        });
    });
}

async fn initialize_runtime_state(
    state: &RuntimeState,
    app_handle: &AppHandle,
    startup_operation: &OperationContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    startup_operation.phase("db");
    emit_app_startup_progress(
        state,
        "database",
        "running",
        Some("Preparing the portable database".to_string()),
    )
    .await;

    let migration_started = Instant::now();
    let migration_report = state.database.run_migrations().await?;
    tracing::info!(
        repaired_legacy_schema = migration_report.repaired_legacy_schema,
        applied_versions = ?migration_report.applied_versions,
        duration_ms = elapsed_ms(migration_started),
        "Database migration check completed"
    );
    apply_debug_logging_settings(&state.database).await;
    emit_app_startup_progress(
        state,
        "database",
        "complete",
        Some("Portable database ready".to_string()),
    )
    .await;

    startup_operation.phase("api");
    emit_app_startup_progress(
        state,
        "sessions",
        "running",
        Some("Restoring account sessions".to_string()),
    )
    .await;
    let mut sessions = SessionManager::new();
    let accounts = settings::get_login_accounts(state.database.reader()).await?;
    let active_acct = accounts
        .iter()
        .find(|account| account.is_active)
        .map(|account| account.acct.clone());

    for account in accounts {
        let account_credentials = AccountCredentials::from_login_account(&account);
        match restore_session(&account, &account_credentials).await {
            Ok(session) => {
                if matches!(session.client.kind(), ServerKind::Bluesky) {
                    let access_token = session.client.current_access_token().await?;
                    let app_password = session.client.bluesky_app_password();
                    state
                        .credentials
                        .update_for_account(
                            state.database.writer(),
                            &session.acct,
                            &AccountCredentials::new(access_token, app_password),
                        )
                        .await?;
                    session.client.set_bluesky_credential_sink(
                        state
                            .credentials
                            .bluesky_sink(state.database.writer(), session.acct.clone()),
                    );
                }
                sessions.add_session(session)
            }
            Err(error) => tracing::warn!("Failed to restore session {}: {}", account.acct, error),
        }
    }

    if let Some(acct) = active_acct {
        sessions.set_active(&acct);
    }
    *state.sessions.write().await = sessions;
    emit_app_startup_progress(
        state,
        "sessions",
        "complete",
        Some("Account sessions restored".to_string()),
    )
    .await;

    emit_app_startup_progress(
        state,
        "services",
        "running",
        Some("Starting local services".to_string()),
    )
    .await;
    if let Some(window) = app_handle.get_webview_window("main") {
        restore_window_state(&window, &state.database).await;
        install_window_state_persistence(window, state.database.clone());
    }
    restart_streaming(state).await;

    // The initial snapshot may now proceed. Potentially expensive remote
    // reconciliation deliberately starts only after readiness is observable.
    state.startup.mark_ready();
    emit_app_startup_progress(
        state,
        "ready",
        "complete",
        Some("Awayuki is ready".to_string()),
    )
    .await;
    schedule_post_ready_work(state);
    Ok(())
}

async fn emit_app_startup_progress(
    state: &RuntimeState,
    stage: &'static str,
    status: &'static str,
    message: Option<String>,
) {
    state
        .emit_queue
        .emit(
            APP_STARTUP_PROGRESS_EVENT,
            AppStartupProgressEvent {
                stage,
                status,
                message,
            },
            "application startup status",
        )
        .await;
}

async fn apply_debug_logging_settings(database: &Database) {
    let debug = match load_database_setting::<DebugSettings>(database, "debug").await {
        Ok(debug) => debug,
        Err(error) => {
            tracing::warn!("Failed to load debug settings for logging: {}", error);
            DebugSettings::default()
        }
    };
    if debug.logging_enabled {
        if let Err(error) = logging::enable() {
            tracing::warn!("Failed to enable file logging: {}", error);
        }
    }
    logging::set_log_level(debug.log_level);
}

async fn restore_window_state(window: &WebviewWindow, database: &Database) {
    let Some(state) = load_saved_window_state(database).await else {
        return;
    };

    if !is_window_state_usable(window, &state) {
        tracing::warn!("Ignoring unusable saved window state: {:?}", state);
        return;
    }

    if let Err(error) = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
        x: state.x,
        y: state.y,
    })) {
        tracing::warn!("Failed to restore window position: {}", error);
    }

    if let Err(error) = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
        width: state.width,
        height: state.height,
    })) {
        tracing::warn!("Failed to restore window size: {}", error);
    }

    match state.state.as_str() {
        "maximized" => {
            if let Err(error) = window.maximize() {
                tracing::warn!("Failed to restore maximized window state: {}", error);
            }
        }
        "fullscreen" => {
            if let Err(error) = window.set_fullscreen(true) {
                tracing::warn!("Failed to restore fullscreen window state: {}", error);
            }
        }
        _ => {}
    }
}

async fn load_saved_window_state(database: &Database) -> Option<SavedWindowState> {
    match settings::get_setting(database.reader(), WINDOW_STATE_SETTING_KEY).await {
        Ok(Some(json)) => parse_saved_window_state(&json, "app_settings.window_state"),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!("Failed to load window state: {}", error);
            None
        }
    }
}

fn parse_saved_window_state(json: &str, source: &str) -> Option<SavedWindowState> {
    let raw = match serde_json::from_str::<RawSavedWindowState>(json) {
        Ok(raw) => raw,
        Err(error) => {
            tracing::warn!("Failed to parse window state from {}: {}", source, error);
            return None;
        }
    };

    if !raw.x.is_finite() || !raw.y.is_finite() || !raw.width.is_finite() || !raw.height.is_finite()
    {
        tracing::warn!("Ignoring non-finite window state from {}", source);
        return None;
    }

    Some(SavedWindowState {
        state: raw.state,
        x: raw.x.round() as i32,
        y: raw.y.round() as i32,
        width: raw.width.round().max(0.0) as u32,
        height: raw.height.round().max(0.0) as u32,
    })
}

fn install_window_state_persistence(window: WebviewWindow, database: Arc<Database>) {
    #[derive(Clone, Copy)]
    enum PersistSignal {
        Idle,
        Changed,
        Flush,
    }

    let (save_signal, mut save_events) = watch::channel(PersistSignal::Idle);
    let worker_window = window.clone();
    let worker_database = database.clone();

    tauri::async_runtime::spawn(async move {
        while save_events.changed().await.is_ok() {
            let signal = *save_events.borrow_and_update();
            match signal {
                PersistSignal::Idle => {}
                PersistSignal::Flush => {
                    if let Err(error) = persist_window_state(&worker_window, &worker_database).await
                    {
                        tracing::warn!(error = %error, "Failed to flush window state");
                    }
                }
                PersistSignal::Changed => loop {
                    let debounce =
                        tokio::time::sleep(Duration::from_millis(WINDOW_STATE_SAVE_DEBOUNCE_MS));
                    tokio::pin!(debounce);
                    tokio::select! {
                        () = &mut debounce => {
                            if let Err(error) = persist_window_state(
                                &worker_window,
                                &worker_database,
                            ).await {
                                tracing::warn!(error = %error, "Failed to save window state");
                            }
                            break;
                        }
                        changed = save_events.changed() => {
                            if changed.is_err() {
                                return;
                            }
                            let next_signal = { *save_events.borrow_and_update() };
                            match next_signal {
                                PersistSignal::Changed | PersistSignal::Idle => continue,
                                PersistSignal::Flush => {
                                    if let Err(error) = persist_window_state(
                                        &worker_window,
                                        &worker_database,
                                    ).await {
                                        tracing::warn!(error = %error, "Failed to flush window state");
                                    }
                                    break;
                                }
                            }
                        }
                    }
                },
            }
        }
    });

    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            save_signal.send_replace(PersistSignal::Changed);
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            save_signal.send_replace(PersistSignal::Flush);
        }
        _ => {}
    });
}

fn install_drop_path_registration(window: WebviewWindow, media_uploads: Arc<MediaUploadManager>) {
    window.on_window_event(move |event| {
        let paths = match event {
            WindowEvent::DragDrop(tauri::DragDropEvent::Enter { paths, .. })
            | WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => paths,
            _ => return,
        };
        media_uploads.register_dropped_paths(paths);
    });
}

async fn persist_window_state(
    window: &WebviewWindow,
    database: &Database,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let is_fullscreen = window.is_fullscreen()?;
    let is_maximized = window.is_maximized()?;
    let position = window.outer_position()?;
    let size = window.outer_size()?;
    let state = SavedWindowState {
        state: if is_fullscreen {
            "fullscreen"
        } else if is_maximized {
            "maximized"
        } else {
            "windowed"
        }
        .to_string(),
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let json = serde_json::to_string(&state)?;
    settings::set_setting(database.writer(), WINDOW_STATE_SETTING_KEY, &json).await?;
    Ok(())
}

fn is_window_state_usable(window: &WebviewWindow, state: &SavedWindowState) -> bool {
    if state.width < 320 || state.height < 240 {
        return false;
    }

    let Ok(monitors) = window.available_monitors() else {
        return true;
    };
    if monitors.is_empty() {
        return true;
    }

    let window_left = state.x;
    let window_top = state.y;
    let window_right = state.x.saturating_add(state.width as i32);
    let window_bottom = state.y.saturating_add(state.height as i32);

    monitors.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        let monitor_left = position.x;
        let monitor_top = position.y;
        let monitor_right = position.x.saturating_add(size.width as i32);
        let monitor_bottom = position.y.saturating_add(size.height as i32);

        window_left < monitor_right
            && window_right > monitor_left
            && window_top < monitor_bottom
            && window_bottom > monitor_top
    })
}

async fn restore_session(
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

pub(crate) async fn app_snapshot_impl(
    state: State<'_, RuntimeState>,
) -> Result<AppSnapshot, String> {
    state.startup.wait_until_ready().await?;
    app_snapshot_for_state(&state).await
}

/// Re-run a failed background initializer without restarting the process.
///
/// This is a mutation: migrations/session restoration can write SQLite, so the
/// generated client must not automatically replay it after a response loss.
pub(crate) async fn retry_runtime_initialization_impl(
    state: State<'_, RuntimeState>,
    app: AppHandle,
) -> Result<(), AppError> {
    let mut operation = OperationContext::start("retry_runtime_initialization", None, None);
    match state.startup.begin_retry() {
        Ok(()) => {
            spawn_runtime_initialization_worker(state.inner().clone(), app);
            operation.finish_ok();
            Ok(())
        }
        // Coalesce a double-click with the already-running retry. The caller
        // can proceed to app_snapshot, which waits on the same gate.
        Err(RetryStartError::AlreadyRunning) => {
            operation.finish_ok();
            Ok(())
        }
        Err(error @ RetryStartError::NotFailed) => {
            let app_error = AppError::validation(operation.id());
            tracing::warn!(error = %error, "Rejected runtime retry in non-failed state");
            Err(operation.finish_app_error(app_error))
        }
    }
}

pub(crate) async fn account_summaries_impl(
    state: State<'_, RuntimeState>,
) -> Result<Vec<AccountSummary>, String> {
    login_accounts(&state).await
}

pub(crate) async fn account_lists_impl(
    state: State<'_, RuntimeState>,
    request: AccountListsRequest,
) -> Result<Vec<AccountListSummary>, String> {
    let acct = request.acct.trim();
    if acct.is_empty() {
        return Err("Account is required".to_string());
    }
    let session = session_for_acct(&state, acct)
        .await
        .ok_or_else(|| format!("Account is not signed in: {}", acct))?;
    let mut lists = session
        .client
        .get_lists()
        .await
        .map_err(|error| error.to_string())?;
    lists.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(lists
        .into_iter()
        .map(|list| AccountListSummary {
            id: list.id,
            title: list.title,
        })
        .collect())
}

pub(crate) async fn login_with_instance_domain_impl(
    state: State<'_, RuntimeState>,
    request: LoginInstanceRequest,
) -> Result<AppSnapshot, String> {
    let domain = normalize_login_domain(&request.domain)?;
    let (session, kind) = run_login_flow(&domain).await?;
    persist_login_session(&state, session, kind).await
}

pub(crate) async fn login_with_bluesky_app_password_impl(
    state: State<'_, RuntimeState>,
    request: LoginBlueskyRequest,
) -> Result<AppSnapshot, String> {
    let identifier = request.identifier.trim().to_string();
    if identifier.is_empty() || request.password.is_empty() {
        return Err("Bluesky identifier and app password are required".to_string());
    }
    let (session, kind) = run_bluesky_login(&identifier, &request.password).await?;
    persist_login_session(&state, session, kind).await
}

fn normalize_login_domain(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Please enter an instance domain".to_string());
    }
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let domain = without_scheme
        .split('/')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.');
    if domain.is_empty() {
        return Err("Please enter an instance domain".to_string());
    }
    Ok(domain.to_lowercase())
}

async fn run_login_flow(domain: &str) -> Result<(AccountSession, ServerKind), String> {
    let kind = detect_server_kind(domain)
        .await
        .map_err(|error| error.to_string())?;
    tracing::info!("Detected server kind for {}: {:?}", domain, kind);
    match kind {
        ServerKind::Mastodon | ServerKind::Paon => run_mastodon_oauth(domain, kind).await,
        ServerKind::Misskey => run_misskey_miauth(domain, kind).await,
        ServerKind::Bluesky => Err(
            "Bluesky cannot be configured via instance domain; use the Bluesky login form below."
                .to_string(),
        ),
    }
}

async fn run_mastodon_oauth(
    domain: &str,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), String> {
    let callback_listener = callback_server::CallbackListener::bind()
        .await
        .map_err(|error| error.to_string())?;
    let port = callback_listener.port();
    let mut flow = OAuthFlow::new(domain, port).map_err(|error| error.to_string())?;
    flow.prepare().await.map_err(|error| error.to_string())?;
    let auth_url = flow
        .authorize_url()
        .ok_or_else(|| "Failed to generate authorization URL".to_string())?;
    let expected_state = flow.state().to_string();

    tracing::info!("Opening browser for Mastodon authorization");
    open::that(&auth_url).map_err(|error| error.to_string())?;

    let (_, code) = callback_listener
        .wait_for_callback(&[("state", expected_state.as_str())], &["code"])
        .await
        .map_err(|error| error.to_string())?;
    let token_response = flow
        .exchange_code(&code)
        .await
        .map_err(|error| error.to_string())?;

    let instance = flow
        .instance
        .as_ref()
        .ok_or_else(|| "No instance info".to_string())?;
    let streaming_url = instance
        .streaming_url()
        .unwrap_or(&format!("wss://{}", domain))
        .to_string();

    let client = ApiClient::mastodon_with_kind(
        MastodonClient::new(domain, token_response.access_token, streaming_url)
            .map_err(|error| error.to_string())?,
        kind,
    );
    let account = client
        .verify_credentials()
        .await
        .map_err(|error| error.to_string())?;
    let acct = normalized_account_key(&account, domain);
    tracing::info!("Mastodon login successful: @{}", acct);

    Ok((
        AccountSession {
            acct,
            domain: domain.to_string(),
            client,
            account_info: account,
        },
        kind,
    ))
}

async fn run_misskey_miauth(
    domain: &str,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), String> {
    let callback_listener = callback_server::CallbackListener::bind()
        .await
        .map_err(|error| error.to_string())?;
    let port = callback_listener.port();
    let flow = MiAuthFlow::new(domain, port).map_err(|error| error.to_string())?;
    let auth_url = flow.authorize_url();
    let expected_session = flow.session_id().to_string();

    tracing::info!("Opening browser for Misskey authorization");
    open::that(&auth_url).map_err(|error| error.to_string())?;
    callback_listener
        .wait_for_callback(&[("session", expected_session.as_str())], &["session"])
        .await
        .map_err(|error| error.to_string())?;

    let result = flow.check().await.map_err(|error| error.to_string())?;
    let streaming_url = format!("wss://{}", domain);
    let client = ApiClient::misskey(
        MisskeyClient::new(domain, result.token, streaming_url)
            .map_err(|error| error.to_string())?,
    );
    let account = client
        .verify_credentials()
        .await
        .map_err(|error| error.to_string())?;
    let acct = normalized_account_key(&account, domain);
    tracing::info!("Misskey login successful: @{}", acct);

    Ok((
        AccountSession {
            acct,
            domain: domain.to_string(),
            client,
            account_info: account,
        },
        kind,
    ))
}

async fn run_bluesky_login(
    identifier: &str,
    password: &str,
) -> Result<(AccountSession, ServerKind), String> {
    let domain = DEFAULT_BLUESKY_HOST;
    let streaming_url = format!("wss://{}", domain);
    let client = ApiClient::bluesky(
        login_with_app_password(domain, identifier, password, streaming_url)
            .await
            .map_err(|error| error.to_string())?,
    );
    let account = client
        .verify_credentials()
        .await
        .map_err(|error| error.to_string())?;
    let acct = normalized_account_key(&account, domain);
    tracing::info!("Bluesky login successful: @{}", acct);

    Ok((
        AccountSession {
            acct,
            domain: domain.to_string(),
            client,
            account_info: account,
        },
        ServerKind::Bluesky,
    ))
}

fn normalized_account_key(account: &Account, domain: &str) -> String {
    if account.acct.contains('@') {
        account.acct.clone()
    } else {
        format!("{}@{}", account.acct, domain)
    }
}

async fn persist_login_session(
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

    {
        let mut sessions = state.sessions.write().await;
        sessions.add_session(session.clone());
        sessions.set_active(&session.acct);
    }
    restart_streaming(state).await;
    app_snapshot_for_state(state).await
}

pub(crate) async fn load_timeline_impl(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let started_at = Instant::now();
    let column_type = request.column_type.clone();
    let column_param = request.column_param.clone();
    let limit = request.limit;
    let offset = request.offset;
    let since_status_id = request.since_status_id.clone();
    let since_server_domain = request.since_server_domain.clone();
    tracing::info!(
        column_type = column_type.as_str(),
        column_param = ?column_param,
        limit = ?limit,
        offset = ?offset,
        since_status_id = ?since_status_id,
        since_server_domain = ?since_server_domain,
        "[awayuki][tauri-command] load_timeline start"
    );
    let result = load_local_timeline(&state, request).await;
    match &result {
        Ok(statuses) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit = ?limit,
            offset = ?offset,
            since_status_id = ?since_status_id,
            since_server_domain = ?since_server_domain,
            count = statuses.len(),
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_timeline success"
        ),
        Err(error) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit = ?limit,
            offset = ?offset,
            since_status_id = ?since_status_id,
            since_server_domain = ?since_server_domain,
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_timeline error: {}",
            error
        ),
    }
    result
}

pub(crate) async fn load_more_timeline_impl(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<TimelinePageResponse, String> {
    let started_at = Instant::now();
    let column_type = request.column_type.clone();
    let column_param = request.column_param.clone();
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(120);
    let offset = request.offset.unwrap_or(0);
    let max_status_id = request.max_status_id.clone();
    tracing::info!(
        column_type = column_type.as_str(),
        column_param = ?column_param,
        limit,
        offset,
        max_status_id = ?max_status_id,
        "[awayuki][tauri-command] load_more_timeline start"
    );

    let tl_type =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
            .ok_or_else(|| "Unsupported timeline type".to_string())?;
    let result = if timeline_type_can_load_more_from_api(&tl_type) {
        load_more_api_timeline(&state, request, &tl_type, limit).await
    } else {
        let statuses = load_local_timeline(&state, request).await?;
        Ok(TimelinePageResponse {
            has_more: statuses.len() >= limit as usize,
            statuses,
        })
    };

    match &result {
        Ok(response) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit,
            offset,
            max_status_id = ?max_status_id,
            count = response.statuses.len(),
            has_more = response.has_more,
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_more_timeline success"
        ),
        Err(error) => tracing::info!(
            column_type = column_type.as_str(),
            column_param = ?column_param,
            limit,
            offset,
            max_status_id = ?max_status_id,
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] load_more_timeline error: {}",
            error
        ),
    }
    result
}

pub(crate) async fn refresh_timeline_impl(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, AppError> {
    let mut operation = OperationContext::start(
        "refresh_timeline",
        request.operation_id.as_deref(),
        request.account_acct.as_deref(),
    );
    operation.phase("api");
    let result = refresh_timeline_inner(state, request).await;
    match result {
        Ok(statuses) => {
            operation.phase("commit");
            operation.finish_ok();
            Ok(statuses)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

async fn refresh_timeline_inner(
    state: State<'_, RuntimeState>,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let total_started_at = Instant::now();
    let request_column_type = request.column_type.clone();
    let request_column_param = request.column_param.clone();
    let request_limit = request.limit;
    let request_offset = request.offset;
    let request_since_status_id = request.since_status_id.clone();
    let request_since_server_domain = request.since_server_domain.clone();
    let request_account_acct = request.account_acct.clone();
    let log_context = TimelineCommandLogContext {
        command: "refresh_timeline",
        column_type: &request_column_type,
        column_param: &request_column_param,
        limit: request_limit,
        offset: request_offset,
        since_status_id: &request_since_status_id,
        since_server_domain: &request_since_server_domain,
        started_at: total_started_at,
    };
    tracing::info!(
        column_type = request_column_type.as_str(),
        column_param = ?request_column_param,
        limit = ?request_limit,
        offset = ?request_offset,
        since_status_id = ?request_since_status_id,
        since_server_domain = ?request_since_server_domain,
        account_acct = ?request_account_acct,
        "[awayuki][tauri-command] refresh_timeline start"
    );
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(80);
    let tl_type =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
            .ok_or_else(|| "Unsupported timeline type".to_string())?;

    if matches!(tl_type, TimelineType::Notification) {
        let result = refresh_aggregate_notifications(&state, limit).await;
        log_timeline_command_result(&log_context, &result);
        return result;
    }

    if is_aggregate_timeline(&tl_type, request.account_acct.as_deref()) {
        let result =
            refresh_aggregate_timeline(&state, &tl_type, limit, request.display_filter).await;
        log_timeline_command_result(&log_context, &result);
        return result;
    }

    if matches!(
        tl_type,
        TimelineType::CustomSql(_)
            | TimelineType::YukariQuery(_)
            | TimelineType::Search(_)
            | TimelineType::Bookmarks
            | TimelineType::Favourites
            | TimelineType::UserBookmarks { .. }
    ) {
        let result = load_local_timeline(&state, request).await;
        log_timeline_command_result(&log_context, &result);
        return result;
    }

    let session = session_for_timeline_source(&state, request.account_acct.as_deref()).await?;
    let client = session.client;
    let source_acct = session.acct;
    let statuses = timeline_service::sync_timeline(
        &client,
        state.database.writer(),
        state.database.reader(),
        &tl_type,
        &source_acct,
        &TimelineParams {
            limit: Some(limit),
            ..Default::default()
        },
    )
    .await
    .map_err(|error| error.to_string())?;

    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let result: Result<Vec<TimelineStatus>, String> = Ok(statuses
        .into_iter()
        .map(|status| {
            with_source_acct(
                status_to_view(&status, client.domain(), None),
                Some(source_acct.clone()),
            )
        })
        .filter(|status| timeline_status_matches_display_filter(status, display_filter))
        .collect());
    log_timeline_command_result(&log_context, &result);
    result
}

pub(crate) async fn account_profile_impl(
    state: State<'_, RuntimeState>,
    request: AccountProfileRequest,
) -> Result<AccountProfileSummary, String> {
    let total_started_at = Instant::now();
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        "[awayuki][tauri-command] account_profile start"
    );
    let session_started_at = Instant::now();
    let session = session_for_domain(&state, &request.server_domain).await;
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        session_found = session.is_some(),
        duration_ms = elapsed_ms(session_started_at),
        "[awayuki][tauri-command] account_profile session lookup"
    );
    let cached_started_at = Instant::now();
    let cached_account = accounts::get_account(
        state.database.reader(),
        &request.account_id,
        &request.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        cache_hit = cached_account.is_some(),
        duration_ms = elapsed_ms(cached_started_at),
        "[awayuki][tauri-db] account_profile cached account lookup"
    );
    let mut account_url = None;
    let mut account = match &session {
        Some(session) => {
            let api_started_at = Instant::now();
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                source_acct = session.acct.as_str(),
                "[awayuki][tauri-command] account_profile get_account start"
            );
            let account = match session.client.get_account(&request.account_id).await {
                Ok(account) => {
                    tracing::info!(
                        account_id = request.account_id.as_str(),
                        server_domain = session.client.domain(),
                        source_acct = session.acct.as_str(),
                        duration_ms = elapsed_ms(api_started_at),
                        "[awayuki][tauri-command] account_profile get_account success"
                    );
                    account
                }
                Err(error) => {
                    tracing::info!(
                        account_id = request.account_id.as_str(),
                        server_domain = session.client.domain(),
                        source_acct = session.acct.as_str(),
                        duration_ms = elapsed_ms(api_started_at),
                        "[awayuki][tauri-command] account_profile get_account error: {}",
                        error
                    );
                    return Err(error.to_string());
                }
            };
            account_url = Some(account.url.clone());
            let mut fresh_account = DbAccount::from_api(&account, session.client.domain());
            if let Some(cached_account) = cached_account.as_ref() {
                preserve_cached_profile_media(&mut fresh_account, cached_account);
            }
            let upsert_started_at = Instant::now();
            accounts::upsert_account(state.database.writer(), &fresh_account)
                .await
                .map_err(|error| error.to_string())?;
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                duration_ms = elapsed_ms(upsert_started_at),
                "[awayuki][tauri-db] account_profile upsert account"
            );
            fresh_account
        }
        None => match cached_account {
            Some(account) => account,
            None => {
                tracing::info!(
                    account_id = request.account_id.as_str(),
                    server_domain = request.server_domain.as_str(),
                    duration_ms = elapsed_ms(total_started_at),
                    "[awayuki][tauri-command] account_profile error: account is not cached"
                );
                return Err("Account is not cached".to_string());
            }
        },
    };

    if account.server_domain.is_empty() {
        account.server_domain = request.server_domain.clone();
    }

    let relationship = match &session {
        Some(session) if session.account_info.id != request.account_id => {
            let relationship_started_at = Instant::now();
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                source_acct = session.acct.as_str(),
                "[awayuki][tauri-command] account_profile get_relationships start"
            );
            match session
                .client
                .get_relationships(&[&request.account_id])
                .await
            {
                Ok(relationships) => {
                    tracing::info!(
                        account_id = request.account_id.as_str(),
                        server_domain = session.client.domain(),
                        source_acct = session.acct.as_str(),
                        count = relationships.len(),
                        duration_ms = elapsed_ms(relationship_started_at),
                        "[awayuki][tauri-command] account_profile get_relationships success"
                    );
                    relationships.into_iter().next().map(|relationship| {
                        AccountRelationshipSummary {
                            following: relationship.following,
                            followed_by: relationship.followed_by,
                            requested: relationship.requested,
                            blocking: relationship.blocking,
                            muting: relationship.muting,
                        }
                    })
                }
                Err(error) => {
                    tracing::info!(
                        account_id = request.account_id.as_str(),
                        server_domain = session.client.domain(),
                        source_acct = session.acct.as_str(),
                        duration_ms = elapsed_ms(relationship_started_at),
                        "[awayuki][tauri-command] account_profile get_relationships error: {}",
                        error
                    );
                    None
                }
            }
        }
        _ => None,
    };
    let is_self = session
        .as_ref()
        .map(|session| session.account_info.id == request.account_id)
        .unwrap_or(false);

    let mute_started_at = Instant::now();
    let notification_muted = notification_mutes::is_account_muted(
        state.database.reader(),
        &request.account_id,
        &request.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        notification_muted,
        duration_ms = elapsed_ms(mute_started_at),
        "[awayuki][tauri-db] account_profile notification mute lookup"
    );

    let view = account_profile_to_view(
        account,
        is_self,
        relationship,
        notification_muted,
        account_url,
    );
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        duration_ms = elapsed_ms(total_started_at),
        "[awayuki][tauri-command] account_profile success"
    );
    Ok(view)
}

pub(crate) async fn account_timeline_impl(
    state: State<'_, RuntimeState>,
    request: AccountTimelineRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let total_started_at = Instant::now();
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(80) as i64;
    let offset = request.offset.unwrap_or(0) as i64;
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        pinned = request.pinned.unwrap_or(false),
        only_media = request.only_media.unwrap_or(false),
        limit,
        offset,
        "[awayuki][tauri-command] account_timeline start"
    );
    if request.pinned == Some(true) && offset == 0 {
        let session_started_at = Instant::now();
        let session = session_for_domain(&state, &request.server_domain).await;
        tracing::info!(
            account_id = request.account_id.as_str(),
            server_domain = request.server_domain.as_str(),
            session_found = session.is_some(),
            duration_ms = elapsed_ms(session_started_at),
            "[awayuki][tauri-command] account_timeline session lookup"
        );
        if let Some(session) = session {
            let api_started_at = Instant::now();
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                source_acct = session.acct.as_str(),
                limit,
                "[awayuki][tauri-command] account_timeline get_account_statuses start"
            );
            let statuses = match session
                .client
                .get_account_statuses(
                    &request.account_id,
                    &AccountStatusesParams {
                        pinned: Some(true),
                        limit: Some(limit as u32),
                        exclude_replies: Some(false),
                        exclude_reblogs: Some(false),
                        only_media: request.only_media,
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(statuses) => {
                    tracing::info!(
                        account_id = request.account_id.as_str(),
                        server_domain = session.client.domain(),
                        source_acct = session.acct.as_str(),
                        count = statuses.len(),
                        duration_ms = elapsed_ms(api_started_at),
                        "[awayuki][tauri-command] account_timeline get_account_statuses success"
                    );
                    statuses
                }
                Err(error) => {
                    tracing::info!(
                        account_id = request.account_id.as_str(),
                        server_domain = session.client.domain(),
                        source_acct = session.acct.as_str(),
                        duration_ms = elapsed_ms(api_started_at),
                        "[awayuki][tauri-command] account_timeline get_account_statuses error: {}",
                        error
                    );
                    return Err(error.to_string());
                }
            };

            let mut statuses = statuses;
            let quote_started_at = Instant::now();
            timeline_service::resolve_pending_quotes_with_backoff(&session.client, &mut statuses)
                .await;
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                count = statuses.len(),
                duration_ms = elapsed_ms(quote_started_at),
                "[awayuki][tauri-command] account_timeline quote resolution"
            );

            let save_started_at = Instant::now();
            for status in &statuses {
                timeline_service::save_status_to_db_with_retry(
                    state.database.writer(),
                    status,
                    session.client.domain(),
                )
                .await
                .map_err(|error| error.to_string())?;
            }
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                count = statuses.len(),
                duration_ms = elapsed_ms(save_started_at),
                "[awayuki][tauri-db] account_timeline save pinned statuses"
            );

            let view_started_at = Instant::now();
            let views = statuses
                .iter()
                .map(|status| {
                    with_source_acct(
                        status_to_view(status, session.client.domain(), None),
                        Some(session.acct.clone()),
                    )
                })
                .collect::<Vec<_>>();
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                count = views.len(),
                duration_ms = elapsed_ms(view_started_at),
                "[awayuki][tauri-command] account_timeline convert pinned views"
            );
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = session.client.domain(),
                count = views.len(),
                duration_ms = elapsed_ms(total_started_at),
                "[awayuki][tauri-command] account_timeline success source=api"
            );
            return Ok(views);
        }
    }

    let query_started_at = Instant::now();
    let statuses = match query_account_statuses(
        state.database.reader(),
        &request.account_id,
        &request.server_domain,
        request.only_media.unwrap_or(false),
        request.pinned,
        limit,
        offset,
    )
    .await
    {
        Ok(statuses) => {
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = request.server_domain.as_str(),
                pinned = request.pinned.unwrap_or(false),
                only_media = request.only_media.unwrap_or(false),
                limit,
                offset,
                count = statuses.len(),
                duration_ms = elapsed_ms(query_started_at),
                "[awayuki][tauri-db] account_timeline query cached statuses"
            );
            statuses
        }
        Err(error) => {
            tracing::info!(
                account_id = request.account_id.as_str(),
                server_domain = request.server_domain.as_str(),
                pinned = request.pinned.unwrap_or(false),
                only_media = request.only_media.unwrap_or(false),
                limit,
                offset,
                duration_ms = elapsed_ms(query_started_at),
                "[awayuki][tauri-db] account_timeline query cached statuses error: {}",
                error
            );
            return Err(error);
        }
    };
    let view_started_at = Instant::now();
    let views = db_statuses_to_views(state.database.reader(), statuses).await?;
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        count = views.len(),
        duration_ms = elapsed_ms(view_started_at),
        "[awayuki][tauri-command] account_timeline convert cached views"
    );
    tracing::info!(
        account_id = request.account_id.as_str(),
        server_domain = request.server_domain.as_str(),
        count = views.len(),
        duration_ms = elapsed_ms(total_started_at),
        "[awayuki][tauri-command] account_timeline success source=db"
    );
    Ok(views)
}

pub(crate) async fn air_context_impl(
    state: State<'_, RuntimeState>,
    request: AirContextRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let limit = request.limit.unwrap_or(2).clamp(1, 2) as usize;
    let session = session_for_domain(&state, &request.server_domain)
        .await
        .ok_or_else(|| "No signed-in account for this server".to_string())?;

    tracing::info!(
        status_id = request.status_id.as_str(),
        server_domain = request.server_domain.as_str(),
        account_id = request.account_id.as_str(),
        account_acct = ?request.account_acct,
        "[awayuki][tauri-command] air_context start"
    );

    let target_status = match session.client.get_status(&request.status_id).await {
        Ok(status) => {
            timeline_service::save_status_to_db_with_retry(
                state.database.writer(),
                &status,
                session.client.domain(),
            )
            .await
            .map_err(|error| error.to_string())?;
            Some(status)
        }
        Err(error) => {
            tracing::info!(
                status_id = request.status_id.as_str(),
                server_domain = session.client.domain(),
                "[awayuki][tauri-command] air_context target fetch fallback: {}",
                error
            );
            None
        }
    };

    let target_created_at = match target_status.as_ref() {
        Some(status) => status.created_at,
        None => {
            let cached = query_cached_status(
                state.database.reader(),
                &request.status_id,
                &request.server_domain,
            )
            .await?
            .ok_or_else(|| "AIR context target status is not cached".to_string())?;
            parse_cached_status_created_at(&cached)?
        }
    };

    let mut views = match target_status.as_ref() {
        Some(status) => vec![with_source_acct(
            status_to_view(status, session.client.domain(), None),
            Some(session.acct.clone()),
        )],
        None => {
            let cached = query_cached_status(
                state.database.reader(),
                &request.status_id,
                &request.server_domain,
            )
            .await?
            .ok_or_else(|| "AIR context target status is not cached".to_string())?;
            db_statuses_to_views(state.database.reader(), vec![cached]).await?
        }
    };

    let found = find_air_context_post(
        &session.client,
        &request.account_id,
        &request.status_id,
        target_created_at,
    )
    .await?;
    let mut found_statuses = vec![found];
    timeline_service::resolve_pending_quotes_with_backoff(&session.client, &mut found_statuses)
        .await;
    for status in &found_statuses {
        timeline_service::save_status_to_db_with_retry(
            state.database.writer(),
            status,
            session.client.domain(),
        )
        .await
        .map_err(|error| error.to_string())?;
    }

    if let Some(status) = found_statuses.first() {
        views.push(with_source_acct(
            status_to_view(status, session.client.domain(), None),
            Some(session.acct.clone()),
        ));
    }
    views.truncate(limit);

    tracing::info!(
        status_id = request.status_id.as_str(),
        server_domain = session.client.domain(),
        account_id = request.account_id.as_str(),
        count = views.len(),
        "[awayuki][tauri-command] air_context success"
    );
    Ok(views)
}

async fn find_air_context_post(
    client: &ApiClient,
    account_id: &str,
    target_status_id: &str,
    target_created_at: DateTime<Utc>,
) -> Result<Status, String> {
    const PAGE_LIMIT: u32 = 40;
    const MAX_PAGES: usize = 8;

    let mut max_id = None;
    let mut candidate: Option<Status> = None;

    for _ in 0..MAX_PAGES {
        let statuses = client
            .get_account_statuses(
                account_id,
                &AccountStatusesParams {
                    max_id: max_id.clone(),
                    limit: Some(PAGE_LIMIT),
                    pinned: None,
                    exclude_replies: Some(false),
                    exclude_reblogs: Some(true),
                    only_media: Some(false),
                },
            )
            .await
            .map_err(|error| error.to_string())?;

        if statuses.is_empty() {
            break;
        }

        let mut reached_target_time = false;
        for status in &statuses {
            if status.id == target_status_id || status.account.id != account_id {
                continue;
            }
            if status.created_at > target_created_at {
                let closer = candidate
                    .as_ref()
                    .map(|current| status.created_at < current.created_at)
                    .unwrap_or(true);
                if closer {
                    candidate = Some(status.clone());
                }
            } else {
                reached_target_time = true;
            }
        }

        if reached_target_time || matches!(client.kind(), ServerKind::Bluesky) {
            break;
        }

        let Some(last_id) = statuses.last().map(|status| status.id.clone()) else {
            break;
        };
        if max_id.as_deref() == Some(last_id.as_str()) {
            break;
        }
        max_id = Some(last_id);
    }

    candidate.ok_or_else(|| "No AIR context post found after the notification target".to_string())
}

fn parse_cached_status_created_at(status: &DbStatus) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(&status.created_at)
        .map(|created_at| created_at.with_timezone(&Utc))
        .map_err(|error| format!("AIR context target timestamp is invalid: {}", error))
}

pub(crate) async fn status_thread_impl(
    state: State<'_, RuntimeState>,
    request: StatusThreadRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let limit = request.limit.unwrap_or(240).clamp(1, 300) as usize;
    let mut remote_error = None;

    if let Some(session) = session_for_domain(&state, &request.server_domain).await {
        let mut remote_statuses = Vec::new();
        match session.client.get_status_context(&request.status_id).await {
            Ok(context) => {
                remote_statuses.extend(context.ancestors);
                remote_statuses.extend(context.descendants);
            }
            Err(error) => {
                remote_error = Some(error.to_string());
            }
        }

        match session.client.get_status(&request.status_id).await {
            Ok(status) => remote_statuses.push(status),
            Err(error) if remote_error.is_none() => {
                remote_error = Some(error.to_string());
            }
            Err(_) => {}
        }

        dedupe_statuses_by_uri(&mut remote_statuses);
        timeline_service::resolve_pending_quotes_with_backoff(
            &session.client,
            &mut remote_statuses,
        )
        .await;
        timeline_service::save_status_batch_with_retry(
            state.database.writer(),
            &remote_statuses,
            session.client.domain(),
            None,
        )
        .await
        .map_err(|error| error.to_string())?;
    }

    let statuses = query_status_thread_statuses(
        state.database.reader(),
        &request.status_id,
        &request.server_domain,
        limit,
    )
    .await?;
    if statuses.is_empty() {
        return Err(remote_error.unwrap_or_else(|| "Thread status is not cached".to_string()));
    }
    let retention_keys = statuses
        .iter()
        .map(|status| (status.id.clone(), status.server_domain.clone()))
        .collect::<Vec<_>>();
    startup_sync::protect_thread_statuses(state.database.writer(), &retention_keys, Utc::now())
        .await
        .map_err(|error| error.to_string())?;

    db_statuses_to_views(state.database.reader(), statuses).await
}

pub(crate) async fn account_follow_action_impl(
    state: State<'_, RuntimeState>,
    request: AccountFollowRequest,
) -> Result<AccountRelationshipSummary, String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    let operation = relationship_operation(&request.action)?;
    session
        .client
        .capabilities(1)
        .require_relationship(operation)
        .map_err(|error| error.to_string())?;
    let target_account_id = if session
        .client
        .domain()
        .eq_ignore_ascii_case(&request.server_domain)
    {
        request.account_id.clone()
    } else {
        let target_acct = request.target_acct.trim().trim_start_matches('@');
        if target_acct.is_empty() {
            return Err("targetAcct is required for a remote relationship action".to_string());
        }
        session
            .client
            .search_accounts(target_acct, 20)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|account| {
                account
                    .acct
                    .trim_start_matches('@')
                    .eq_ignore_ascii_case(target_acct)
                    || account.id == request.account_id
            })
            .map(|account| account.id)
            .ok_or_else(|| {
                format!(
                    "Remote account is not available to acting account {}: {}",
                    session.acct, request.target_acct
                )
            })?
    };
    let relationship = match request.action.as_str() {
        "follow" => session
            .client
            .follow_account(&target_account_id)
            .await
            .map_err(|error| error.to_string())?,
        "unfollow" => session
            .client
            .unfollow_account(&target_account_id)
            .await
            .map_err(|error| error.to_string())?,
        "mute" => session
            .client
            .mute_account(&target_account_id)
            .await
            .map_err(|error| error.to_string())?,
        "unmute" => session
            .client
            .unmute_account(&target_account_id)
            .await
            .map_err(|error| error.to_string())?,
        "block" => session
            .client
            .block_account(&target_account_id)
            .await
            .map_err(|error| error.to_string())?,
        "unblock" => session
            .client
            .unblock_account(&target_account_id)
            .await
            .map_err(|error| error.to_string())?,
        _ => unreachable!("validated by relationship_operation"),
    };
    Ok(AccountRelationshipSummary {
        following: relationship.following,
        followed_by: relationship.followed_by,
        requested: relationship.requested,
        blocking: relationship.blocking,
        muting: relationship.muting,
    })
}

pub(crate) async fn set_account_notification_mute_impl(
    state: State<'_, RuntimeState>,
    request: AccountNotificationMuteRequest,
) -> Result<bool, String> {
    let cached_account = accounts::get_account(
        state.database.reader(),
        &request.account_id,
        &request.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;
    let cached_acct = cached_account
        .as_ref()
        .map(|account| account.acct.as_str())
        .unwrap_or("");
    let cached_display_name = cached_account
        .as_ref()
        .map(|account| account.display_name.as_str())
        .unwrap_or("");
    notification_mutes::set_account_muted(
        state.database.writer(),
        &request.account_id,
        &request.server_domain,
        cached_acct,
        cached_display_name,
        request.muted,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(request.muted)
}

pub(crate) async fn notification_muted_accounts_impl(
    state: State<'_, RuntimeState>,
) -> Result<Vec<NotificationMutedAccountSummary>, String> {
    let rows = notification_mutes::list_muted_accounts(state.database.reader())
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| NotificationMutedAccountSummary {
            account_id: row.account_id,
            server_domain: row.server_domain,
            acct: row.acct,
            display_name: row.display_name,
            avatar: row.avatar,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

pub(crate) async fn post_status_impl(
    state: State<'_, RuntimeState>,
    request: PostRequest,
) -> Result<TimelineStatus, AppError> {
    let mut operation = OperationContext::start(
        "post_status",
        request.operation_id.as_deref(),
        Some(&request.acting_account_acct),
    );
    let result = post_status_inner(state, request, &operation).await;
    match result {
        Ok(status) => {
            operation.finish_ok();
            Ok(status)
        }
        Err(error) => Err(operation.finish_app_error(error)),
    }
}

async fn post_status_inner(
    state: State<'_, RuntimeState>,
    request: PostRequest,
    operation: &OperationContext,
) -> Result<TimelineStatus, AppError> {
    let status_text = request.status.trim().to_string();
    let media_ids = request.media_ids.filter(|ids| !ids.is_empty());
    let poll = request.poll.and_then(|poll| {
        let options = poll
            .options
            .into_iter()
            .map(|option| option.trim().to_string())
            .filter(|option| !option.is_empty())
            .collect::<Vec<_>>();
        if options.len() < 2 {
            None
        } else {
            Some(CreatePollParams {
                options,
                expires_in: poll.expires_in,
                multiple: Some(poll.multiple),
                hide_totals: None,
            })
        }
    });
    if status_text.is_empty() && media_ids.is_none() && poll.is_none() {
        return Err(AppError::validation(operation.id()));
    }
    let preset_visibility = load_setting::<PresetVisibilitySettings>(&state, "preset_visibility")
        .await
        .map_err(|error| AppError::from_source(error, operation.id()))?
        .match_visibility(&status_text)
        .map(|visibility| visibility.as_request_visibility().to_string());
    let session = acting_session(&state, &request.acting_account_acct)
        .await
        .map_err(|error| AppError::from_source(error, operation.id()))?;
    let capabilities = session.client.capabilities(1);
    if media_ids.as_ref().is_some_and(|ids| !ids.is_empty()) && !capabilities.compose.media_upload {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    if media_ids
        .as_ref()
        .is_some_and(|ids| ids.len() > capabilities.compose.max_media_attachments as usize)
    {
        return Err(AppError::validation(operation.id()));
    }
    if poll.is_some() && !capabilities.compose.poll {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    if request.quote_id.is_some() && !capabilities.compose.quote {
        return Err(AppError::new(
            AppErrorCode::CapabilityUnsupported,
            operation.id(),
        ));
    }
    let client = session.client;
    operation.phase("api");
    let mut status = client
        .create_status(&CreateStatusParams {
            status: if status_text.is_empty() {
                None
            } else {
                Some(request.status)
            },
            in_reply_to_id: request.in_reply_to_id,
            media_ids,
            sensitive: request.sensitive,
            spoiler_text: request.spoiler_text,
            visibility: preset_visibility.or(request.visibility),
            language: None,
            quote_id: request.quote_id,
            poll,
        })
        .await
        .map_err(|error| AppError::from_adapter(error, operation.id()))?;
    timeline_service::hydrate_missing_quotes(&client, std::slice::from_mut(&mut status)).await;

    operation.phase("db");
    let db_started_at = Instant::now();
    let save_result = timeline_service::save_status_for_viewer_to_db_with_retry(
        state.database.writer(),
        &status,
        client.domain(),
        &session.acct,
    )
    .await;
    crate::observability::observe_db_query(1, elapsed_ms(db_started_at));
    save_result.map_err(|error| AppError::from_source(error, operation.id()))?;

    Ok(status_to_view(&status, client.domain(), None))
}

pub(crate) async fn begin_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: BeginMediaUploadRequest,
) -> Result<BeginMediaUploadResponse, String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if !session.client.capabilities(1).compose.media_upload {
        return Err("Media upload is not supported by this account".to_string());
    }
    let upload_id = state
        .media_uploads
        .begin(
            session.acct,
            &request.filename,
            &request.mime_type,
            request.size,
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(BeginMediaUploadResponse { upload_id })
}

pub(crate) async fn append_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: AppendMediaUploadRequest,
) -> Result<MediaUploadProgressResponse, String> {
    let progress = state
        .media_uploads
        .append(&request.upload_id, &request.data)
        .await
        .map_err(|error| error.to_string())?;
    Ok(MediaUploadProgressResponse {
        written: progress.written,
        total: progress.total,
    })
}

pub(crate) async fn finish_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
) -> Result<MediaAttachment, String> {
    let completed = state
        .media_uploads
        .finish(&request.upload_id)
        .await
        .map_err(|error| error.to_string())?;
    let session = acting_session(&state, &completed.acting_account_acct).await?;
    if !session.client.capabilities(1).compose.media_upload {
        return Err("Media upload is not supported by this account".to_string());
    }
    session
        .client
        .upload_media(&completed.path)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn cancel_compose_media_upload_impl(
    state: State<'_, RuntimeState>,
    request: MediaUploadIdRequest,
) -> Result<(), String> {
    state.media_uploads.cancel(&request.upload_id).await;
    Ok(())
}

pub(crate) fn claim_dropped_media_path_impl(
    state: State<'_, RuntimeState>,
    request: ClaimDroppedMediaPathRequest,
) -> Result<ClaimDroppedMediaPathResponse, String> {
    let capability = state
        .media_uploads
        .claim_dropped_path(PathBuf::from(request.path).as_path())
        .map_err(|error| error.to_string())?;
    Ok(ClaimDroppedMediaPathResponse { capability })
}

pub(crate) async fn upload_compose_media_path_impl(
    state: State<'_, RuntimeState>,
    request: UploadMediaPathRequest,
) -> Result<MediaAttachment, String> {
    let path = PathBuf::from(request.path);
    let path = state
        .media_uploads
        .consume_dropped_path(&request.capability, &path)
        .await
        .map_err(|error| error.to_string())?;
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if !session.client.capabilities(1).compose.media_upload {
        return Err("Media upload is not supported by this account".to_string());
    }
    let client = session.client;
    client
        .upload_media(&path)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn autocomplete_mentions_impl(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<MentionSuggestionView>, String> {
    let query = normalize_suggestion_query(&request.query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = normalize_suggestion_limit(request.limit);
    let performance = load_setting::<PerformanceSettings>(&state, "performance").await?;

    match performance.mention_source {
        SuggestionSource::Server => {
            let session =
                session_for_timeline_request(&state, request.account_acct.as_deref()).await?;
            let accounts = session
                .client
                .search_accounts(&query, limit)
                .await
                .map_err(|error| error.to_string())?;
            Ok(unique_mention_suggestions(
                accounts
                    .into_iter()
                    .map(|account| MentionSuggestionView {
                        acct: account.acct,
                        display_name: account.display_name,
                        avatar: account.avatar,
                    })
                    .collect(),
            ))
        }
        SuggestionSource::SQLite => {
            let accounts = accounts::search_accounts_prefix(state.database.reader(), &query, limit)
                .await
                .map_err(|error| error.to_string())?;
            Ok(unique_mention_suggestions(
                accounts
                    .into_iter()
                    .map(|account| MentionSuggestionView {
                        acct: account.acct,
                        display_name: account.display_name,
                        avatar: account.avatar,
                    })
                    .collect(),
            ))
        }
    }
}

pub(crate) async fn autocomplete_hashtags_impl(
    state: State<'_, RuntimeState>,
    request: ComposeSuggestionRequest,
) -> Result<Vec<HashtagSuggestionView>, String> {
    let query = normalize_suggestion_query(&request.query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = normalize_suggestion_limit(request.limit);
    let performance = load_setting::<PerformanceSettings>(&state, "performance").await?;

    let names = match performance.hashtag_source {
        SuggestionSource::Server => {
            let session =
                session_for_timeline_request(&state, request.account_acct.as_deref()).await?;
            session
                .client
                .search_hashtags(&query, limit)
                .await
                .map_err(|error| error.to_string())?
                .hashtags
                .into_iter()
                .map(|tag| tag.name)
                .collect()
        }
        SuggestionSource::SQLite => {
            tags::search_tags_prefix(state.database.reader(), &query, limit)
                .await
                .map_err(|error| error.to_string())?
        }
    };

    Ok(unique_hashtag_names(names)
        .into_iter()
        .map(|name| HashtagSuggestionView { name })
        .collect())
}

fn normalize_suggestion_query(query: &str) -> String {
    query
        .trim()
        .trim_start_matches(['@', '#'])
        .chars()
        .take(80)
        .collect()
}

fn normalize_suggestion_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(8).clamp(1, 20)
}

fn unique_mention_suggestions(
    suggestions: Vec<MentionSuggestionView>,
) -> Vec<MentionSuggestionView> {
    let mut seen = HashSet::new();
    suggestions
        .into_iter()
        .filter(|suggestion| {
            let key = normalize_suggestion_identity(&suggestion.acct, '@');
            !key.is_empty() && seen.insert(key)
        })
        .collect()
}

fn unique_hashtag_names(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter(|name| {
            let key = normalize_suggestion_identity(name, '#');
            !key.is_empty() && seen.insert(key)
        })
        .collect()
}

fn normalize_suggestion_identity(value: &str, marker: char) -> String {
    value.trim().trim_start_matches(marker).to_lowercase()
}

pub(crate) async fn custom_emojis_impl(
    state: State<'_, RuntimeState>,
    account_acct: String,
) -> Result<Vec<CustomEmojiView>, String> {
    let session = acting_session(&state, &account_acct).await?;
    let client = session.client;
    let emojis = client
        .get_custom_emojis()
        .await
        .map_err(|error| error.to_string())?;
    Ok(emojis
        .into_iter()
        .filter(|emoji| emoji.visible_in_picker)
        .map(|emoji| CustomEmojiView {
            shortcode: emoji.shortcode,
            url: emoji.url,
            static_url: emoji.static_url,
            category: emoji.category,
        })
        .collect())
}

pub(crate) async fn switch_active_account_impl(
    state: State<'_, RuntimeState>,
    acct: String,
) -> Result<AppSnapshot, String> {
    let mut sessions = state.sessions.write().await;
    if !sessions.sessions().contains_key(&acct) {
        return Err(format!("Account session not found: {acct}"));
    }
    let previous_acct = sessions
        .active_session()
        .map(|session| session.acct.clone());
    settings::set_active_account(state.database.writer(), &acct)
        .await
        .map_err(|error| error.to_string())?;
    if !sessions.set_active(&acct) {
        return Err(format!("Failed to activate account session: {acct}"));
    }
    drop(sessions);
    if let Some(previous_acct) = previous_acct.filter(|previous| previous != &acct) {
        state.media_uploads.cancel_account(&previous_acct).await;
    }
    app_snapshot_impl(state).await
}

pub(crate) async fn logout_account_impl(
    state: State<'_, RuntimeState>,
    acct: String,
) -> Result<AppSnapshot, String> {
    state.media_uploads.cancel_account(&acct).await;
    let fallback_acct = state
        .credentials
        .remove_account_and_reassign(state.database.writer(), &acct)
        .await
        .map_err(|error| error.to_string())?;
    let mut sessions = state.sessions.write().await;
    sessions.remove_session(&acct);
    if let Some(fallback_acct) = fallback_acct.as_deref() {
        sessions.set_active(fallback_acct);
    }
    drop(sessions);
    restart_streaming(state.inner()).await;
    app_snapshot_impl(state).await
}

pub(crate) async fn save_settings_impl(
    state: State<'_, RuntimeState>,
    request: SaveSettingsRequest,
) -> Result<SettingsSnapshot, String> {
    let json = validated_settings_json(&request.key, request.value)?;
    settings::set_setting(state.database.writer(), &request.key, &json)
        .await
        .map_err(|error| error.to_string())?;

    if request.key == "debug" {
        let debug = serde_json::from_str::<DebugSettings>(&json)
            .map_err(|error| format!("Validated debug settings could not be read: {error}"))?;
        if debug.logging_enabled {
            logging::enable().map_err(|error| error.to_string())?;
        } else {
            logging::disable();
        }
        logging::set_log_level(debug.log_level);
    }

    if request.key == "bluesky_fetch" {
        restart_streaming(state.inner()).await;
    }

    settings_snapshot(&state).await
}

fn validated_settings_json(key: &str, value: serde_json::Value) -> Result<String, String> {
    fn encode<T>(value: serde_json::Value) -> Result<String, String>
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let typed = serde_json::from_value::<T>(value).map_err(|error| error.to_string())?;
        serde_json::to_string(&typed).map_err(|error| error.to_string())
    }

    match key {
        "appearance" => encode::<AppearanceSettings>(value),
        "performance" => encode::<PerformanceSettings>(value),
        "confirmation" => encode::<ConfirmationSettings>(value),
        "bluesky_fetch" => {
            let typed = serde_json::from_value::<BlueskyFetchSettings>(value)
                .map_err(|error| error.to_string())?
                .normalized();
            serde_json::to_string(&typed).map_err(|error| error.to_string())
        }
        "sidecars" => {
            let typed = serde_json::from_value::<SidecarSettings>(value)
                .map_err(|error| error.to_string())?
                .normalized()?;
            serde_json::to_string(&typed).map_err(|error| error.to_string())
        }
        "account_source_colors" => encode::<HashMap<String, AccountSourceColor>>(value),
        "preset_visibility" => encode::<PresetVisibilitySettings>(value),
        "debug" => encode::<DebugSettings>(value),
        "notification_suppression" => encode::<NotificationSuppressionList>(value),
        _ => Err(format!("Unsupported settings key: {key}")),
    }
}

pub(crate) async fn translate_status_text_command_impl(
    request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, String> {
    translate_status_text_impl(request).await
}

#[cfg(target_os = "macos")]
async fn translate_status_text_impl(
    request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, String> {
    let request = prepare_translation_request(request)?;
    match request.translation_engine {
        TranslationEngine::FoundationModel => translate_with_foundation_model(request).await,
        TranslationEngine::TranslationFramework => {
            translate_with_translation_framework(request).await
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct PreparedTranslationRequest {
    text: String,
    source_language: Option<String>,
    target_language: String,
    translation_engine: TranslationEngine,
}

#[cfg(target_os = "macos")]
fn prepare_translation_request(
    request: TranslateStatusRequest,
) -> Result<PreparedTranslationRequest, String> {
    let text = request.text.trim().to_string();
    if text.is_empty() {
        return Err("Text to translate is empty".to_string());
    }

    let target_language = normalized_language_identifier(&request.target_language);
    if target_language.is_empty() {
        return Err("Target language is empty".to_string());
    }

    let source_language = request
        .source_language
        .as_deref()
        .map(str::trim)
        .map(normalized_language_identifier)
        .filter(|value| !value.is_empty());

    Ok(PreparedTranslationRequest {
        text,
        source_language,
        target_language,
        translation_engine: request.translation_engine.unwrap_or_default(),
    })
}

#[cfg(target_os = "macos")]
async fn translate_with_foundation_model(
    request: PreparedTranslationRequest,
) -> Result<TranslateStatusResponse, String> {
    let source_hint = request.source_language.as_deref().unwrap_or("auto-detect");
    let target_label = translation_language_label(&request.target_language);
    let prompt = format!(
        "Source language: {source_hint}\nTarget language: {target_label}\n\nText:\n{}",
        request.text
    );

    let client =
        AppleAiClient::new().map_err(|error| format!("Translation unavailable: {error}"))?;
    let response = client
        .generate(
            vec![
                Message::system(
                    "You are a translation engine. Translate the user's social-media post text faithfully. Preserve line breaks, mentions, hashtags, URLs, emoji, and punctuation. Return only the translated text without explanations, quotes, language labels, or markdown.",
                ),
                Message::user(prompt),
            ],
            GenerationOptions::default().temperature(0.0),
        )
        .await
        .map_err(|error| format!("Translation failed: {error}"))?;
    let translated = response.text.trim().to_string();
    if translated.is_empty() {
        return Err("Translation returned empty text".to_string());
    }

    Ok(TranslateStatusResponse {
        text: translated,
        source_language: request.source_language,
        target_language: request.target_language,
    })
}

#[cfg(target_os = "macos")]
async fn translate_with_translation_framework(
    request: PreparedTranslationRequest,
) -> Result<TranslateStatusResponse, String> {
    tokio::task::spawn_blocking(move || translate_with_translation_framework_blocking(request))
        .await
        .map_err(|error| format!("Translation failed: {error}"))?
}

#[cfg(target_os = "macos")]
fn translate_with_translation_framework_blocking(
    request: PreparedTranslationRequest,
) -> Result<TranslateStatusResponse, String> {
    let source_language = request.source_language.clone().map(Ok).unwrap_or_else(|| {
        translation::detect_language(&request.text)
            .map_err(|error| format!("Language detection failed: {error}"))?
            .ok_or_else(|| "Source language could not be detected".to_string())
    })?;
    let source = translation::Language::new(source_language)
        .canonicalized()
        .map_err(|error| format!("Invalid source language: {error}"))?;
    let target = translation::Language::new(request.target_language.clone())
        .canonicalized()
        .map_err(|error| format!("Invalid target language: {error}"))?;
    let config =
        translation::TranslationSessionConfiguration::new(source.identifier(), target.identifier());
    let session = translation::TranslationSession::new(config)
        .map_err(|error| format!("Translation unavailable: {error}"))?;

    if !session
        .is_ready()
        .map_err(|error| format!("Translation readiness check failed: {error}"))?
    {
        session
            .prepare_translation()
            .map_err(|error| format!("Translation preparation failed: {error}"))?;
    }

    let response = session
        .translate(&request.text)
        .map_err(|error| format!("Translation failed: {error}"))?;
    let translated = response.target_text().trim().to_string();
    if translated.is_empty() {
        return Err("Translation returned empty text".to_string());
    }

    Ok(TranslateStatusResponse {
        text: translated,
        source_language: Some(response.source_language().to_string()),
        target_language: response.target_language().to_string(),
    })
}

#[cfg(target_os = "macos")]
fn normalized_language_identifier(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "english" => "en".to_string(),
        "japanese" => "ja".to_string(),
        _ => value.trim().to_string(),
    }
}

#[cfg(target_os = "macos")]
fn translation_language_label(identifier: &str) -> &str {
    match identifier.trim().to_lowercase().as_str() {
        "en" | "en-us" | "en-gb" => "English",
        "ja" | "ja-jp" => "Japanese",
        _ => identifier,
    }
}

#[cfg(not(target_os = "macos"))]
async fn translate_status_text_impl(
    _request: TranslateStatusRequest,
) -> Result<TranslateStatusResponse, String> {
    Err("Translation is only supported on macOS.".to_string())
}

pub(crate) async fn save_columns_impl(
    state: State<'_, RuntimeState>,
    request: SaveColumnsRequest,
) -> Result<AppSnapshot, String> {
    let columns = normalized_column_request(request.columns);
    for column in &columns {
        if TimelineType::from_column_config(&column.column_type, column.column_param.as_deref())
            .is_none()
        {
            return Err(format!("Unsupported timeline type: {}", column.column_type));
        }
    }

    let mut configs = Vec::with_capacity(columns.len());
    for (index, column) in columns.into_iter().enumerate() {
        let account_acct = normalized_column_account_acct(&column)?;
        configs.push(DbColumnConfig {
            id: column.id.clone(),
            account_acct,
            column_type: column.column_type.clone(),
            column_param: encode_column_param_with_display_filter(&column),
            position: column.position,
            width: None,
            name: Some(column.name.clone()),
            max_statuses: Some(column.max_statuses.max(1) as i32),
            pane_index: Some(column.pane_index as i32),
        });
        if configs[..index].iter().any(|config| config.id == column.id) {
            return Err(format!("Duplicate column id: {}", column.id));
        }
    }
    settings::replace_all_column_configs(state.database.writer(), &configs)
        .await
        .map_err(|error| format!("Failed to save columns atomically: {error}"))?;

    restart_streaming(state.inner()).await;
    app_snapshot_impl(state).await
}

fn encode_column_param_with_display_filter(column: &ColumnSummary) -> Option<String> {
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

pub(crate) async fn vacuum_database_impl(
    state: State<'_, RuntimeState>,
) -> Result<DbSummary, String> {
    settings::vacuum(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    database_summary(&state).await
}

pub(crate) async fn explain_custom_timeline_impl(
    state: State<'_, RuntimeState>,
    request: ExplainCustomTimelineRequest,
) -> Result<Vec<crate::db::queries::custom_timeline::QueryPlanStep>, AppError> {
    let mut operation = OperationContext::start(
        "explain_custom_timeline",
        request.operation_id.as_deref(),
        None,
    );
    operation.phase("db");
    if let Err(error) = state.startup.wait_until_ready().await {
        return Err(operation.finish_error(error));
    }
    match crate::db::queries::custom_timeline::explain(
        state.database.analytics_reader(),
        &request.sql,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    {
        Ok(plan) => {
            operation.finish_ok();
            Ok(plan)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn clear_status_cache_impl(
    state: State<'_, RuntimeState>,
) -> Result<DbSummary, String> {
    settings::clear_status_cache(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    database_summary(&state).await
}

pub(crate) async fn status_bar_snapshot_impl(
    state: State<'_, RuntimeState>,
) -> Result<StatusBarSnapshot, String> {
    status_bar_summary(&state).await
}

pub(crate) async fn diagnostics_snapshot_impl() -> DiagnosticsSnapshot {
    crate::observability::snapshot()
}

/// Build an explicitly requested, in-memory support payload. It is returned to
/// the caller and is never persisted outside the portable SQLite database.
pub(crate) async fn support_bundle_impl(
    state: State<'_, RuntimeState>,
    request: SupportBundleRequest,
) -> Result<SupportBundle, AppError> {
    let mut operation =
        OperationContext::start("support_bundle", request.operation_id.as_deref(), None);
    operation.phase("db");
    let query_started_at = Instant::now();
    let schema_version =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(state.database.reader())
            .await;
    crate::observability::observe_db_query(1, elapsed_ms(query_started_at));
    match schema_version {
        Ok(schema_version) => {
            let bundle = SupportBundle::in_memory(APP_VERSION, schema_version, request.frontend);
            operation.finish_ok();
            Ok(bundle)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn status_action_impl(
    state: State<'_, RuntimeState>,
    request: StatusActionRequest,
) -> Result<TimelineStatus, String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    let operation = status_operation(&request.action)?;
    session
        .client
        .capabilities(1)
        .require_status(operation)
        .map_err(|error| error.to_string())?;
    let remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let client = session.client;
    let acting_acct = session.acct;
    let mut status = match request.action.as_str() {
        "favourite" => client.favourite(&remote_id).await,
        "unfavourite" => client.unfavourite(&remote_id).await,
        "reblog" => client.reblog(&remote_id).await,
        "unreblog" => client.unreblog(&remote_id).await,
        "bookmark" => client.bookmark(&remote_id).await,
        "unbookmark" => client.unbookmark(&remote_id).await,
        _ => unreachable!("validated by status_operation"),
    }
    .map_err(|error| error.to_string())?;
    timeline_service::hydrate_missing_quotes(&client, std::slice::from_mut(&mut status)).await;

    timeline_service::save_status_for_viewer_to_db_with_retry(
        state.database.writer(),
        &status,
        client.domain(),
        &acting_acct,
    )
    .await
    .map_err(|error| error.to_string())?;

    if request.action == "favourite" {
        timeline_service::insert_timeline_entry_with_retry(
            state.database.writer(),
            "favourites",
            client.domain(),
            &status.id,
            &acting_acct,
            &status.created_at.to_rfc3339(),
        )
        .await
        .map_err(|error| error.to_string())?;
    } else if request.action == "unfavourite" {
        sqlx::query(
            "DELETE FROM timeline_entries
             WHERE timeline_type = 'favourites'
               AND status_id = ?
               AND server_domain = ?
               AND account_acct = ?",
        )
        .bind(&status.id)
        .bind(client.domain())
        .bind(&acting_acct)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    } else if request.action == "bookmark" {
        timeline_service::insert_timeline_entry_with_retry(
            state.database.writer(),
            "bookmarks",
            client.domain(),
            &status.id,
            &acting_acct,
            &status.created_at.to_rfc3339(),
        )
        .await
        .map_err(|error| error.to_string())?;
    } else if request.action == "unbookmark" {
        sqlx::query(
            "DELETE FROM timeline_entries
             WHERE timeline_type = 'bookmarks'
               AND status_id = ?
               AND server_domain = ?
               AND account_acct = ?",
        )
        .bind(&status.id)
        .bind(client.domain())
        .bind(&acting_acct)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    }

    Ok(with_source_acct(
        status_to_view(&status, client.domain(), None),
        Some(acting_acct),
    ))
}

pub(crate) async fn vote_poll_impl(
    state: State<'_, RuntimeState>,
    request: VotePollRequest,
) -> Result<PollView, String> {
    if request.choices.is_empty() {
        return Err("Select at least one poll option".to_string());
    }

    let session = acting_session(&state, &request.acting_account_acct).await?;
    session
        .client
        .capabilities(1)
        .require_status(StatusOperation::Vote)
        .map_err(|error| error.to_string())?;
    let remote_status_id =
        resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let remote_poll_id = if session
        .client
        .domain()
        .eq_ignore_ascii_case(&request.identity.server_domain)
    {
        request.poll_id.clone()
    } else {
        session
            .client
            .get_status(&remote_status_id)
            .await
            .map_err(|error| error.to_string())?
            .poll
            .map(|poll| poll.id)
            .ok_or_else(|| "Resolved remote status has no poll".to_string())?
    };
    let poll = session
        .client
        .vote_poll(
            &remote_poll_id,
            &VotePollParams {
                choices: request.choices.clone(),
            },
        )
        .await
        .map_err(|error| error.to_string())?;

    if let Ok(poll_json) = serde_json::to_string(&poll) {
        if let Err(error) =
            update_cached_status_poll(state.database.writer(), &request, &poll_json).await
        {
            tracing::warn!(
                "Failed to update cached poll {}: {}",
                request.poll_id,
                error
            );
        }
    }

    Ok(poll_to_view(&poll))
}

pub(crate) async fn edit_own_status_impl(
    state: State<'_, RuntimeState>,
    request: EditStatusRequest,
) -> Result<TimelineStatus, String> {
    let status_text = request.status.trim().to_string();
    if status_text.is_empty() {
        return Err("Post text is empty".to_string());
    }
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if session.account_info.id != request.account_id {
        return Err("Acting account does not own this post".to_string());
    }
    session
        .client
        .capabilities(1)
        .require_status(StatusOperation::Edit)
        .map_err(|error| error.to_string())?;
    let remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    let mut status = session
        .client
        .edit_status(
            &remote_id,
            &CreateStatusParams {
                status: Some(status_text),
                in_reply_to_id: None,
                media_ids: None,
                sensitive: request.sensitive,
                spoiler_text: request.spoiler_text,
                visibility: request.visibility,
                language: None,
                quote_id: None,
                poll: None,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    timeline_service::hydrate_missing_quotes(&session.client, std::slice::from_mut(&mut status))
        .await;

    timeline_service::save_status_for_viewer_to_db_with_retry(
        state.database.writer(),
        &status,
        session.client.domain(),
        &session.acct,
    )
    .await
    .map_err(|error| error.to_string())?;

    Ok(status_to_view(&status, session.client.domain(), None))
}

pub(crate) async fn delete_own_status_impl(
    state: State<'_, RuntimeState>,
    request: DeleteStatusRequest,
) -> Result<(), String> {
    let session = acting_session(&state, &request.acting_account_acct).await?;
    if session.account_info.id != request.account_id {
        return Err("Acting account does not own this post".to_string());
    }
    session
        .client
        .capabilities(1)
        .require_status(StatusOperation::Delete)
        .map_err(|error| error.to_string())?;
    let remote_id = resolve_status_id_for_acting_account(&session, &request.identity).await?;
    session
        .client
        .delete_status(&remote_id)
        .await
        .map_err(|error| error.to_string())?;

    crate::db::queries::statuses::delete_status_and_references(
        state.database.writer(),
        &request.identity.remote_id,
        &request.identity.server_domain,
    )
    .await
    .map_err(|error| error.to_string())?;

    Ok(())
}

pub(crate) async fn open_status_url_impl(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Unsupported URL scheme".to_string());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|error| error.to_string())
}

pub(crate) async fn download_media_impl(request: DownloadMediaRequest) -> Result<(), String> {
    let parsed = url::Url::parse(&request.url).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err("Unsupported URL scheme".to_string());
    }

    let suggested = request
        .suggested_filename
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| suggested_filename_from_url(&parsed));
    let filename = sanitize_download_filename(&suggested);
    let Some(path) = choose_download_path(&filename)? else {
        return Ok(());
    };
    let path = unique_download_path(path);
    let parent = path
        .parent()
        .ok_or_else(|| "Download path has no parent directory".to_string())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| error.to_string())?;

    let response = download_client()
        .map_err(|error| error.to_string())?
        .get(parsed)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    if !matches!(response.url().scheme(), "https" | "http") {
        return Err("Download redirected to an unsupported URL scheme".to_string());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DOWNLOAD_BYTES as u64)
    {
        return Err(format!(
            "Download exceeds the {} MiB size limit",
            MAX_DOWNLOAD_BYTES / (1024 * 1024)
        ));
    }

    let temp_path = parent.join(format!(
        ".{}.awayuki-{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("media"),
        uuid::Uuid::new_v4().simple()
    ));
    let mut temp_guard = TempDownloadGuard::new(temp_path.clone());
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut downloaded = 0usize;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        downloaded = downloaded
            .checked_add(chunk.len())
            .ok_or_else(|| "Download size overflow".to_string())?;
        if downloaded > MAX_DOWNLOAD_BYTES {
            return Err(format!(
                "Download exceeds the {} MiB size limit",
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    file.flush().await.map_err(|error| error.to_string())?;
    file.sync_all().await.map_err(|error| error.to_string())?;
    drop(file);
    tokio::fs::hard_link(&temp_path, &path)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::remove_file(&temp_path)
        .await
        .map_err(|error| error.to_string())?;
    temp_guard.disarm();
    Ok(())
}

struct TempDownloadGuard {
    path: PathBuf,
    armed: bool,
}

impl TempDownloadGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempDownloadGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn unique_download_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("media");
    let extension = path.extension().and_then(|value| value.to_str());
    for suffix in 1..=10_000 {
        let filename = match extension {
            Some(extension) if !extension.is_empty() => {
                format!("{stem} ({suffix}).{extension}")
            }
            _ => format!("{stem} ({suffix})"),
        };
        let candidate = parent.join(filename);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}", uuid::Uuid::new_v4().simple()))
}

pub(crate) async fn open_log_file_impl() -> Result<(), String> {
    logging::open_in_default_app().map_err(|error| error.to_string())
}

fn suggested_filename_from_url(url: &url::Url) -> String {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.trim().is_empty())
        .map(urlencoding::decode)
        .and_then(Result::ok)
        .map(|name| name.into_owned())
        .unwrap_or_else(|| "media".to_string())
}

fn sanitize_download_filename(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();

    if sanitized.is_empty() {
        "media".to_string()
    } else {
        sanitized
    }
}

#[cfg(target_os = "macos")]
fn choose_download_path(default_name: &str) -> Result<Option<PathBuf>, String> {
    let script = format!(
        "POSIX path of (choose file name with prompt \"Save media as\" default name {})",
        applescript_string(default_name)
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|error| error.to_string())?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            return Ok(None);
        }
        return Ok(Some(PathBuf::from(path)));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || output.status.code() == Some(1) {
        return Ok(None);
    }
    Err(stderr.trim().to_string())
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(not(target_os = "macos"))]
fn choose_download_path(default_name: &str) -> Result<Option<PathBuf>, String> {
    let directory = dirs::download_dir()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(Some(directory.join(default_name)))
}

async fn login_accounts(state: &RuntimeState) -> Result<Vec<AccountSummary>, String> {
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

async fn app_snapshot_for_state(state: &RuntimeState) -> Result<AppSnapshot, String> {
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
        settings: settings_snapshot(state).await?,
        database: database_summary(state).await?,
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
    })
}

fn normalized_column_account_acct(column: &ColumnSummary) -> Result<Option<String>, String> {
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

fn normalized_column_request(columns: Vec<ColumnSummary>) -> Vec<ColumnSummary> {
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

async fn settings_snapshot(state: &RuntimeState) -> Result<SettingsSnapshot, String> {
    Ok(SettingsSnapshot {
        appearance: load_setting(state, "appearance").await?,
        performance: load_setting(state, "performance").await?,
        confirmation: load_setting(state, "confirmation").await?,
        bluesky_fetch: load_setting::<BlueskyFetchSettings>(state, "bluesky_fetch")
            .await?
            .normalized(),
        sidecars: load_setting::<SidecarSettings>(state, "sidecars")
            .await?
            .normalized()?,
        account_source_colors: load_setting(state, "account_source_colors").await?,
        preset_visibility: load_setting(state, "preset_visibility").await?,
        debug: load_setting(state, "debug").await?,
        notification_suppression: load_setting(state, "notification_suppression").await?,
    })
}

async fn load_setting<T>(state: &RuntimeState, key: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Serialize + Default,
{
    load_database_setting(&state.database, key).await
}

async fn load_database_setting<T>(database: &Database, key: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Serialize + Default,
{
    match settings::get_setting(database.reader(), key)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(json) => match serde_json::from_str(&json) {
            Ok(value) => Ok(value),
            Err(error) => {
                let default_value = T::default();
                let default_json = serde_json::to_string(&default_value)
                    .map_err(|serialize_error| serialize_error.to_string())?;
                let backup_key = settings::backup_and_reset_corrupt_setting(
                    database.writer(),
                    key,
                    &json,
                    &default_json,
                )
                .await
                .map_err(|backup_error| backup_error.to_string())?;
                tracing::error!(
                    setting = key,
                    backup_key,
                    %error,
                    "Invalid setting was backed up and reset"
                );
                Err(format!(
                    "Stored setting '{key}' was invalid and has been reset; retry the operation"
                ))
            }
        },
        None => Ok(T::default()),
    }
}

async fn database_summary(state: &RuntimeState) -> Result<DbSummary, String> {
    let status_count = settings::get_status_count(state.database.reader())
        .await
        .unwrap_or_default();
    let recent_since = (Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let recent_status_count =
        settings::get_recent_status_count(state.database.reader(), &recent_since)
            .await
            .unwrap_or_default();
    let account_count = settings::get_account_count(state.database.reader())
        .await
        .unwrap_or_default();
    let size = settings::get_db_size(state.database.reader())
        .await
        .unwrap_or_default();

    Ok(DbSummary {
        path: paths::db_path().display().to_string(),
        size: format_size(size),
        status_count,
        recent_status_count,
        account_count,
    })
}

async fn status_bar_summary(state: &RuntimeState) -> Result<StatusBarSnapshot, String> {
    let status_count = settings::get_status_count(state.database.reader())
        .await
        .unwrap_or_default();
    let recent_since = (Utc::now() - chrono::Duration::minutes(15)).to_rfc3339();
    let recent_status_count =
        settings::get_recent_status_count(state.database.reader(), &recent_since)
            .await
            .unwrap_or_default();

    Ok(StatusBarSnapshot {
        status_count,
        recent_status_count,
        uptime_seconds: state.started_at.elapsed().as_secs(),
    })
}

#[derive(Debug, Clone)]
struct ResolvedStatusId {
    remote_id: String,
    expires_at: Instant,
}

type ResolvedStatusCacheKey = (String, FederationProtocol, String, String);

fn resolved_status_cache() -> &'static Mutex<HashMap<ResolvedStatusCacheKey, ResolvedStatusId>> {
    static CACHE: OnceLock<Mutex<HashMap<ResolvedStatusCacheKey, ResolvedStatusId>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn acting_session(state: &RuntimeState, acct: &str) -> Result<AccountSession, String> {
    let acct = acct.trim();
    if acct.is_empty() {
        return Err("actingAccountAcct is required".to_string());
    }
    session_for_acct(state, acct)
        .await
        .ok_or_else(|| format!("Acting account is not signed in: {acct}"))
}

fn status_operation(action: &str) -> Result<StatusOperation, String> {
    match action {
        "favourite" => Ok(StatusOperation::Favourite),
        "unfavourite" => Ok(StatusOperation::Unfavourite),
        "reblog" => Ok(StatusOperation::Reblog),
        "unreblog" => Ok(StatusOperation::Unreblog),
        "bookmark" => Ok(StatusOperation::Bookmark),
        "unbookmark" => Ok(StatusOperation::Unbookmark),
        other => Err(format!("Unsupported status action: {other}")),
    }
}

fn relationship_operation(action: &str) -> Result<RelationshipOperation, String> {
    match action {
        "follow" => Ok(RelationshipOperation::Follow),
        "unfollow" => Ok(RelationshipOperation::Unfollow),
        "mute" => Ok(RelationshipOperation::Mute),
        "unmute" => Ok(RelationshipOperation::Unmute),
        "block" => Ok(RelationshipOperation::Block),
        "unblock" => Ok(RelationshipOperation::Unblock),
        other => Err(format!("Unsupported account action: {other}")),
    }
}

async fn resolve_status_id_for_acting_account(
    session: &AccountSession,
    identity: &StatusIdentity,
) -> Result<String, String> {
    identity.validate().map_err(|error| error.to_string())?;
    let capabilities = session.client.capabilities(1);
    if capabilities.protocol != identity.protocol {
        return Err("Acting account protocol cannot address this status identity".to_string());
    }

    if session
        .client
        .domain()
        .eq_ignore_ascii_case(&identity.server_domain)
    {
        return Ok(identity.remote_id.clone());
    }

    let key = (
        session.acct.clone(),
        identity.protocol,
        identity.server_domain.clone(),
        identity.canonical_uri.clone(),
    );
    let now = Instant::now();
    if let Some(cached) = resolved_status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .filter(|entry| entry.expires_at > now)
        .cloned()
    {
        return Ok(cached.remote_id);
    }

    let resolved = session
        .client
        .lookup_status_by_uri(&identity.canonical_uri)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "Status is not available to acting account {}: {}",
                session.acct, identity.canonical_uri
            )
        })?;
    let remote_id = resolved.id;
    let mut cache = resolved_status_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.insert(
        key,
        ResolvedStatusId {
            remote_id: remote_id.clone(),
            expires_at: now + Duration::from_secs(5 * 60),
        },
    );
    crate::observability::set_cache_entries(cache.len());
    Ok(remote_id)
}

async fn session_for_acct(state: &RuntimeState, acct: &str) -> Option<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions.sessions().get(acct).cloned()
}

async fn session_for_timeline_request(
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

async fn session_for_timeline_source(
    state: &RuntimeState,
    account_acct: Option<&str>,
) -> Result<AccountSession, String> {
    let acct = required_timeline_source_acct(account_acct)?;
    session_for_acct(state, acct)
        .await
        .ok_or_else(|| format!("Account is not signed in: {acct}"))
}

async fn session_for_domain(state: &RuntimeState, server_domain: &str) -> Option<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions
        .sessions()
        .values()
        .find(|session| session.client.domain() == server_domain || session.domain == server_domain)
        .cloned()
}

async fn signed_in_sessions(state: &RuntimeState) -> Vec<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions.sessions().values().cloned().collect()
}

fn schedule_status_search_backfill(state: &RuntimeState) {
    let database = state.database.clone();
    let emit_queue = state.emit_queue.clone();

    // Like legacy migration bootstrap, a chunk owns a transaction-local SQLx
    // executor. Keep it on one background worker while yielding the writer
    // connection between chunks; UI rendering and snapshot reads stay live.
    tauri::async_runtime::spawn_blocking(move || {
        tauri::async_runtime::block_on(async move {
            let mut operation = OperationContext::start("status_search_backfill", None, None);
            operation.phase("db");
            let (progress_tx, mut progress_rx) =
                mpsc::channel::<search_backfill::SearchBackfillProgress>(16);
            let progress_emitter = emit_queue.clone();
            let progress_handle = tauri::async_runtime::spawn(async move {
                while let Some(progress) = progress_rx.recv().await {
                    progress_emitter
                        .emit(
                            STATUS_SEARCH_BACKFILL_PROGRESS_EVENT,
                            progress,
                            "status search backfill progress",
                        )
                        .await;
                }
            });

            let result = search_backfill::run_to_completion(
                database.writer(),
                database.reader(),
                Some(&progress_tx),
            )
            .await;
            drop(progress_tx);
            let _ = progress_handle.await;
            match result {
                Ok(progress) => {
                    tracing::info!(
                        processed_count = progress.processed_count,
                        total_count = progress.total_count,
                        "Status search backfill completed"
                    );
                    operation.finish_ok();
                }
                Err(error) => {
                    tracing::warn!(%error, "Status search backfill paused after an error");
                    let _ = operation.finish_error(error);
                }
            }
        });
    });
}

fn schedule_post_ready_work(state: &RuntimeState) {
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        run_startup_sync(&state).await;
        // The FTS backfill owns the same single writer pool as startup sync.
        // Start it only after reconciliation has released that writer; with no
        // sessions, run_startup_sync returns immediately.
        schedule_status_search_backfill(&state);
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
    // Retention maintenance is intentionally not automatic. Its current
    // whole-cache transaction can monopolize the only SQLite writer on large
    // portable databases. Keep it available for explicit maintenance/tests,
    // but startup writer availability takes priority over cache pruning.
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

async fn save_notification_to_db(
    database: &Database,
    notification: &crate::mastodon::types::notification::Notification,
    server_domain: &str,
    account_acct: &str,
) -> Result<(), String> {
    let account = DbAccount::from_api(&notification.account, server_domain);
    accounts::upsert_account(database.writer(), &account)
        .await
        .map_err(|error| error.to_string())?;

    if let Some(status) = notification.status.as_ref() {
        timeline_service::save_status_for_viewer_to_db_with_retry(
            database.writer(),
            status,
            server_domain,
            account_acct,
        )
        .await
        .map_err(|error| error.to_string())?;
    }

    sqlx::query(
        "INSERT INTO notifications (id, server_domain, account_acct, notification_type, created_at, account_id, status_id, read_at, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)
         ON CONFLICT(id, server_domain, account_acct) DO UPDATE SET
           notification_type = excluded.notification_type,
           created_at = excluded.created_at,
           account_id = excluded.account_id,
           status_id = excluded.status_id,
           fetched_at = excluded.fetched_at",
    )
    .bind(&notification.id)
    .bind(server_domain)
    .bind(account_acct)
    .bind(notification.notification_type.as_str())
    .bind(notification.created_at.to_rfc3339())
    .bind(&notification.account.id)
    .bind(notification.status.as_ref().map(|status| status.id.as_str()))
    .bind(Utc::now().to_rfc3339())
    .execute(database.writer())
    .await
    .map_err(|error| error.to_string())?;

    Ok(())
}

async fn restart_streaming(state: &RuntimeState) {
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

    let bluesky_fetch = load_setting::<BlueskyFetchSettings>(state, "bluesky_fetch")
        .await
        .unwrap_or_default()
        .normalized();

    for session in sessions {
        let server_kind = session.client.kind();
        let stream_types = stream_types_for_columns(&columns, Some(&session.acct), server_kind);
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

fn stream_types_for_columns(
    columns: &[ColumnSummary],
    account_acct: Option<&str>,
    server_kind: ServerKind,
) -> Vec<crate::mastodon::types::streaming::StreamType> {
    use crate::mastodon::types::streaming::StreamType;

    let mut stream_types = Vec::new();

    for column in columns {
        match column.column_type.as_str() {
            // Unified timelines subscribe every capable signed-in session;
            // stale historical column bindings are intentionally ignored.
            "home" => push_stream_type(&mut stream_types, StreamType::User),
            "public" if provider_supports_aggregate_refresh(server_kind, &TimelineType::Public) => {
                push_stream_type(&mut stream_types, StreamType::Public)
            }
            "notification" => push_stream_type(&mut stream_types, StreamType::UserNotification),
            "local" if column_stream_matches_account(column, account_acct) => {
                push_stream_type(&mut stream_types, StreamType::PublicLocal)
            }
            "list" => {
                if column_stream_matches_account(column, account_acct) {
                    if let Some(id) = column.column_param.as_ref().filter(|id| !id.is_empty()) {
                        push_stream_type(&mut stream_types, StreamType::List(id.clone()));
                    }
                }
            }
            "hashtag" => {
                if column_stream_matches_account(column, account_acct) {
                    if let Some(tag) = column.column_param.as_ref().filter(|tag| !tag.is_empty()) {
                        push_stream_type(&mut stream_types, StreamType::Hashtag(tag.clone()));
                    }
                }
            }
            _ => {}
        }
    }

    // Both Mastodon user streams and Misskey's combined user socket carry
    // notifications. Reuse that subscription instead of opening a duplicate
    // notification connection when a home stream is already required.
    if !matches!(server_kind, ServerKind::Bluesky) && stream_types.contains(&StreamType::User) {
        stream_types.retain(|stream_type| !matches!(stream_type, StreamType::UserNotification));
    }

    stream_types
}

fn column_stream_matches_account(column: &ColumnSummary, account_acct: Option<&str>) -> bool {
    let Some(requested) = column
        .account_acct
        .as_deref()
        .filter(|acct| !acct.is_empty())
    else {
        return false;
    };
    account_acct == Some(requested)
}

fn push_stream_type(
    stream_types: &mut Vec<crate::mastodon::types::streaming::StreamType>,
    stream_type: crate::mastodon::types::streaming::StreamType,
) {
    if !stream_types.contains(&stream_type) {
        stream_types.push(stream_type);
    }
}

async fn forward_stream_events(
    emit_queue: QueuedEmitter,
    database: Arc<Database>,
    rx: mpsc::Receiver<TimelineEvent>,
) {
    let (side_effect_tx, side_effect_rx) = mpsc::unbounded_channel();
    // One worker per account bridge preserves notification side-effect order,
    // while the unbounded handoff keeps a busy SQLite writer from applying
    // backpressure to WebView delivery. Streaming notifications are low-rate;
    // provider status persistence remains protected by its own bounded queue.
    let _side_effect_worker = tokio::spawn(run_stream_side_effects(database, side_effect_rx));
    forward_stream_events_to_queues(emit_queue, rx, &side_effect_tx).await;
}

#[derive(Debug)]
struct StreamSideEffect {
    notification: Box<Notification>,
    source_acct: String,
    server_domain: String,
}

async fn run_stream_side_effects(
    database: Arc<Database>,
    mut rx: mpsc::UnboundedReceiver<StreamSideEffect>,
) {
    while let Some(side_effect) = rx.recv().await {
        let StreamSideEffect {
            notification,
            source_acct,
            server_domain,
        } = side_effect;
        if let Err(error) =
            save_notification_to_db(&database, &notification, &server_domain, &source_acct).await
        {
            tracing::warn!("Failed to save streaming notification to DB: {}", error);
        }
        if should_send_desktop_notification(&database, &notification, &server_domain).await {
            streaming_service::send_desktop_notification(&notification);
        }
    }
}

async fn forward_stream_events_to_queues(
    emit_queue: QueuedEmitter,
    mut rx: mpsc::Receiver<TimelineEvent>,
    side_effect_tx: &mpsc::UnboundedSender<StreamSideEffect>,
) {
    // Sequence at the single consumer so metadata describes actual UI delivery
    // order even when socket and quote workers enqueue concurrently.
    let mut generation = 1u64;
    let mut sequence = 0u64;
    while let Some(event) = rx.recv().await {
        if matches!(&event, TimelineEvent::Resync(..)) {
            generation = generation.saturating_add(1);
            sequence = 0;
        } else {
            sequence = sequence.saturating_add(1);
        }
        let (payload, side_effect) = match event {
            TimelineEvent::NewStatus(
                status,
                stream_type,
                source_acct,
                server_domain,
                _position,
            ) => {
                let status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                (
                    TimelineStreamPayload {
                        kind: "newStatus".to_string(),
                        stream_type: stream_type_key(&stream_type),
                        source_acct,
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    None,
                )
            }
            TimelineEvent::StatusUpdate(status, source_acct, server_domain, _position) => {
                let status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                (
                    TimelineStreamPayload {
                        kind: "statusUpdate".to_string(),
                        stream_type: "status.update".to_string(),
                        source_acct,
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    None,
                )
            }
            TimelineEvent::DeleteStatus(status_id, source_acct, server_domain, _position) => (
                TimelineStreamPayload {
                    kind: "deleteStatus".to_string(),
                    stream_type: "delete".to_string(),
                    source_acct,
                    server_domain,
                    status: None,
                    status_id: Some(status_id),
                    generation,
                    sequence,
                },
                None,
            ),
            TimelineEvent::NewNotification(
                notification,
                stream_type,
                source_acct,
                server_domain,
                _position,
            ) => {
                let status =
                    notification_to_view(&notification, &server_domain, Some(&source_acct));
                (
                    TimelineStreamPayload {
                        kind: "newNotification".to_string(),
                        stream_type: stream_type_key(&stream_type),
                        source_acct: source_acct.clone(),
                        server_domain: server_domain.clone(),
                        status: Some(status),
                        status_id: None,
                        generation,
                        sequence,
                    },
                    Some(StreamSideEffect {
                        notification,
                        source_acct,
                        server_domain,
                    }),
                )
            }
            TimelineEvent::Resync(source_acct, server_domain, _position) => (
                TimelineStreamPayload {
                    kind: "resync".to_string(),
                    stream_type: "resync".to_string(),
                    source_acct,
                    server_domain,
                    status: None,
                    status_id: None,
                    generation,
                    sequence,
                },
                None,
            ),
        };

        emit_queue
            .emit(TIMELINE_STREAM_EVENT, payload, "timeline stream event")
            .await;
        if let Some(side_effect) = side_effect {
            if side_effect_tx.send(side_effect).is_err() {
                tracing::warn!(
                    "Streaming side-effect worker stopped; live WebView delivery continues"
                );
            }
        }
    }
}

async fn should_send_desktop_notification(
    database: &Database,
    notification: &Notification,
    server_domain: &str,
) -> bool {
    if !matches!(
        &notification.notification_type,
        NotificationType::Reblog | NotificationType::Favourite | NotificationType::Follow
    ) {
        return false;
    }

    match notification_mutes::is_account_muted(
        database.reader(),
        &notification.account.id,
        server_domain,
    )
    .await
    {
        Ok(true) => return false,
        Ok(false) => {}
        Err(error) => {
            tracing::warn!("Failed to read notification mute state: {}", error);
        }
    }

    match load_database_setting::<NotificationSuppressionList>(database, "notification_suppression")
        .await
    {
        Ok(suppression) => {
            let acct = notification.account.acct.trim_start_matches('@');
            let display_acct = format!("@{}", acct);
            let qualified_acct = if acct.contains('@') {
                acct.to_string()
            } else {
                format!("{}@{}", acct, server_domain)
            };
            !suppression.is_suppressed(acct)
                && !suppression.is_suppressed(&display_acct)
                && !suppression.is_suppressed(&qualified_acct)
        }
        Err(error) => {
            tracing::warn!("Failed to read notification suppression: {}", error);
            true
        }
    }
}

fn stream_type_key(stream_type: &crate::mastodon::types::streaming::StreamType) -> String {
    use crate::mastodon::types::streaming::StreamType;
    match stream_type {
        StreamType::User => "user".to_string(),
        StreamType::UserNotification => "notification".to_string(),
        StreamType::Public => "public".to_string(),
        StreamType::PublicLocal => "public:local".to_string(),
        StreamType::PublicRemote => "public:remote".to_string(),
        StreamType::Hashtag(tag) => format!("hashtag:{}", tag),
        StreamType::HashtagLocal(tag) => format!("hashtag:local:{}", tag),
        StreamType::List(id) => format!("list:{}", id),
        StreamType::Direct => "direct".to_string(),
    }
}

fn is_aggregate_timeline(
    timeline_type: &TimelineType,
    _legacy_column_account_acct: Option<&str>,
) -> bool {
    // Home and Public are unified timelines. Historical column rows may still
    // carry an account binding, but that value must never narrow provider sync
    // or the local aggregate query.
    matches!(timeline_type, TimelineType::Home | TimelineType::Public)
}

fn timeline_type_can_load_more_from_api(timeline_type: &TimelineType) -> bool {
    matches!(
        timeline_type,
        TimelineType::Local | TimelineType::List(_) | TimelineType::Hashtag(_)
    )
}

async fn load_more_api_timeline(
    state: &RuntimeState,
    request: TimelineRequest,
    timeline_type: &TimelineType,
    limit: u32,
) -> Result<TimelinePageResponse, String> {
    const MAX_API_PAGES_PER_LOAD_MORE: usize = 10;

    let session = session_for_timeline_source(state, request.account_acct.as_deref()).await?;
    let client = session.client;
    let source_acct = session.acct;
    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let page_limit = limit.clamp(1, 80);
    let mut max_id = request.max_status_id;
    let mut statuses = Vec::new();
    let mut has_more = true;
    let mut scanned_pages = 0usize;

    while statuses.len() < limit as usize && scanned_pages < MAX_API_PAGES_PER_LOAD_MORE {
        let raw_statuses = timeline_service::sync_timeline(
            &client,
            state.database.writer(),
            state.database.reader(),
            timeline_type,
            &source_acct,
            &TimelineParams {
                max_id: max_id.clone(),
                limit: Some(page_limit),
                ..Default::default()
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        scanned_pages += 1;

        if raw_statuses.is_empty() {
            has_more = false;
            break;
        }

        let raw_count = raw_statuses.len();
        max_id = raw_statuses.last().map(|status| status.id.clone());
        let matched_before = statuses.len();
        statuses.extend(raw_statuses.into_iter().filter_map(|status| {
            let view = with_source_acct(
                status_to_view(&status, client.domain(), None),
                Some(source_acct.clone()),
            );
            timeline_status_matches_display_filter(&view, display_filter).then_some(view)
        }));
        tracing::info!(
            timeline = timeline_type.as_str(),
            source_acct = source_acct.as_str(),
            raw_count,
            matched_count = statuses.len().saturating_sub(matched_before),
            scanned_pages,
            next_max_id = ?max_id,
            "[awayuki][tauri-command] load_more_timeline api page"
        );
    }

    statuses.truncate(limit as usize);
    Ok(TimelinePageResponse { statuses, has_more })
}

async fn refresh_aggregate_timeline(
    state: &RuntimeState,
    timeline_type: &TimelineType,
    limit: u32,
    display_filter: Option<TimelineDisplayFilter>,
) -> Result<Vec<TimelineStatus>, String> {
    let sessions = signed_in_sessions(state).await;
    let mut attempted_sources = 0usize;
    let mut refreshed_sources = 0usize;
    let mut refresh_failures = Vec::new();

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
        match timeline_service::sync_timeline(
            &session.client,
            state.database.writer(),
            state.database.reader(),
            timeline_type,
            &session.acct,
            &TimelineParams {
                limit: Some(limit),
                ..Default::default()
            },
        )
        .await
        {
            Ok(_) => refreshed_sources += 1,
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
    db_status_refs_to_views(state.database.reader(), statuses).await
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

async fn refresh_aggregate_notifications(
    state: &RuntimeState,
    limit: u32,
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
        let mut notifications = match session
            .client
            .get_notifications(&NotificationParams {
                limit: Some(limit),
                ..Default::default()
            })
            .await
        {
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
            save_notification_to_db(
                &state.database,
                notification,
                session.client.domain(),
                &session.acct,
            )
            .await?;
            if let Some(status) = notification.status.as_ref() {
                timeline_service::schedule_pending_quote_resolution(
                    &session.client,
                    state.database.writer(),
                    std::slice::from_ref(status),
                    session.client.domain(),
                    &session.acct,
                );
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
        let cached = query_notification_statuses(state.database.reader(), limit as i64, 0).await?;
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

fn timeline_status_matches_display_filter(
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

fn timeline_display_filter_sql(
    alias: &str,
    display_filter: Option<TimelineDisplayFilter>,
) -> String {
    let Some(filter) = display_filter.filter(|filter| filter.applies()) else {
        return String::new();
    };
    let media_sql = format!(
        "(({alias}.media_attachments_json IS NOT NULL AND {alias}.media_attachments_json != '[]')
          OR EXISTS (
            SELECT 1 FROM statuses original
            WHERE original.id = {alias}.reblog_of_id
              AND original.server_domain = {alias}.server_domain
              AND original.media_attachments_json IS NOT NULL
              AND original.media_attachments_json != '[]'
          ))"
    );
    let mut sql = String::new();
    if filter.exclude_boosts {
        sql.push_str(&format!(" AND {alias}.reblog_of_id IS NULL"));
    }
    if filter.exclude_media {
        sql.push_str(" AND NOT ");
        sql.push_str(&media_sql);
    }
    if filter.include_media {
        sql.push_str(" AND ");
        sql.push_str(&media_sql);
    }
    sql
}

async fn load_local_timeline(
    state: &RuntimeState,
    request: TimelineRequest,
) -> Result<Vec<TimelineStatus>, String> {
    let limit = request.limit.unwrap_or(DEFAULT_TIMELINE_LIMIT).min(120) as i64;
    let offset = request.offset.unwrap_or(0) as i64;
    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let tl_type =
        TimelineType::from_column_config(&request.column_type, request.column_param.as_deref())
            .ok_or_else(|| "Unsupported timeline type".to_string())?;
    let aggregate_timeline = is_aggregate_timeline(&tl_type, request.account_acct.as_deref());

    let statuses = match tl_type {
        TimelineType::CustomSql(sql) => {
            query_custom_statuses(state.database.analytics_reader(), &sql, limit, offset).await?
        }
        TimelineType::YukariQuery(query) => {
            query_yq_statuses(
                state.database.analytics_reader(),
                &query,
                limit,
                offset,
                request
                    .since_status_id
                    .as_deref()
                    .zip(request.since_server_domain.as_deref()),
                request
                    .max_status_id
                    .as_deref()
                    .zip(request.max_server_domain.as_deref()),
            )
            .await?
        }
        TimelineType::Search(query) => {
            query_search_statuses(
                state.database.reader(),
                &query,
                limit,
                offset,
                display_filter,
                request
                    .max_status_id
                    .as_deref()
                    .zip(request.max_server_domain.as_deref()),
            )
            .await?
        }
        TimelineType::Bookmarks => {
            let statuses = query_bookmarked_statuses(
                state.database.reader(),
                request.account_acct.as_deref(),
                limit,
                offset,
            )
            .await?;
            return db_status_refs_to_views(state.database.reader(), statuses).await;
        }
        TimelineType::Favourites => {
            let statuses = query_favourited_statuses(
                state.database.reader(),
                request.account_acct.as_deref(),
                limit,
                offset,
            )
            .await?;
            return db_status_refs_to_views(state.database.reader(), statuses).await;
        }
        TimelineType::UserBookmarks {
            server_domain,
            account_id,
        } => {
            let statuses = query_user_bookmarked_statuses(
                state.database.reader(),
                &server_domain,
                &account_id,
                request.account_acct.as_deref(),
                limit,
                offset,
            )
            .await?;
            return db_status_refs_to_views(state.database.reader(), statuses).await;
        }
        TimelineType::Notification => {
            return query_notification_statuses(state.database.reader(), limit, offset).await;
        }
        TimelineType::Home | TimelineType::Public => {
            debug_assert!(aggregate_timeline);
            let statuses = query_aggregate_timeline_statuses(
                state.database.reader(),
                &tl_type.as_str(),
                limit,
                offset,
                display_filter,
            )
            .await?;
            return db_status_refs_to_views(state.database.reader(), statuses).await;
        }
        _ => {
            let source_acct = session_for_timeline_source(state, request.account_acct.as_deref())
                .await?
                .acct;
            let statuses = query_timeline_statuses(
                state.database.reader(),
                &tl_type.as_str(),
                &source_acct,
                limit,
                offset,
                display_filter,
            )
            .await?;
            return db_status_refs_to_views(state.database.reader(), statuses).await;
        }
    };

    db_statuses_to_views(state.database.reader(), statuses).await
}

async fn query_aggregate_timeline_statuses(
    pool: &sqlx::SqlitePool,
    timeline_type: &str,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
) -> Result<Vec<TimelineStatusRef>, String> {
    let filter = display_filter.filter(|filter| filter.applies());
    read_models::query_aggregate_status_refs(
        pool,
        timeline_type,
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

async fn query_timeline_statuses(
    pool: &sqlx::SqlitePool,
    timeline_type: &str,
    account_acct: &str,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
) -> Result<Vec<TimelineStatusRef>, String> {
    let filter_sql = timeline_display_filter_sql("s", display_filter);
    let sql = format!(
        "SELECT te.server_domain, te.status_id, te.account_acct AS source_acct FROM timeline_entries te
         JOIN statuses s ON s.id = te.status_id AND s.server_domain = te.server_domain
         WHERE te.timeline_type = ? AND te.account_acct = ?
         {}
         ORDER BY te.position_at DESC, te.status_id DESC
         LIMIT ? OFFSET ?",
        filter_sql
    );
    sqlx::query_as::<_, TimelineStatusRef>(&sql)
        .bind(timeline_type)
        .bind(account_acct)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

async fn query_custom_statuses(
    pool: &sqlx::SqlitePool,
    sql: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<DbStatus>, String> {
    crate::db::queries::custom_timeline::query_statuses(
        pool,
        sql,
        limit,
        offset,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|error| {
        tracing::warn!(error = ?error, "Custom timeline repository rejected a query");
        error.to_string()
    })
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

#[derive(Debug, Clone)]
struct LocalTimelineCursor {
    created_at: String,
    server_domain: String,
    id: String,
}

async fn resolve_local_timeline_cursor(
    pool: &sqlx::SqlitePool,
    status: Option<(&str, &str)>,
) -> Result<Option<LocalTimelineCursor>, String> {
    let Some((status_id, server_domain)) = status else {
        return Ok(None);
    };
    sqlx::query_as::<_, (String, String, String)>(
        "SELECT created_at, server_domain, id
         FROM statuses
         WHERE id = ? AND server_domain = ?",
    )
    .bind(status_id)
    .bind(server_domain)
    .fetch_optional(pool)
    .await
    .map(|cursor| {
        cursor.map(|(created_at, server_domain, id)| LocalTimelineCursor {
            created_at,
            server_domain,
            id,
        })
    })
    .map_err(|error| error.to_string())
}

async fn yq_query_budget(pool: &sqlx::SqlitePool) -> YqQueryBudget {
    let cached_count =
        sqlx::query_scalar::<_, i64>("SELECT value FROM cache_counters WHERE name = 'statuses'")
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let status_count = match cached_count {
        Some(count) => count,
        None => sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM statuses")
            .fetch_one(pool)
            .await
            .unwrap_or_default(),
    };
    YqQueryBudget::for_status_count(status_count.max(0) as usize)
}

async fn query_yq_statuses(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    stop_at: Option<(&str, &str)>,
    start_after: Option<(&str, &str)>,
) -> Result<Vec<DbStatus>, String> {
    let started_at = Instant::now();
    let compiled_query = crate::services::yq_filter::compile_query(query)?;
    let budget = yq_query_budget(pool).await;
    let evaluation_cache = crate::services::yq_filter::EvaluationCache::default();
    let requested_limit = limit.max(0) as usize;
    let requested_offset = offset.max(0) as usize;
    if requested_limit == 0 {
        return Ok(Vec::new());
    }
    // A selective SQL prefilter may omit the status that marks the `since`
    // boundary. Resolve its ordered key separately so matching rows older than
    // that status can never leak into the result. If the key no longer exists,
    // preserve the previous best-effort scan behavior.
    let stop_cursor = resolve_local_timeline_cursor(pool, stop_at).await?;
    let stop_key = if stop_cursor.is_some() { None } else { stop_at };
    let mut cursor = resolve_local_timeline_cursor(pool, start_after).await?;
    let matches_to_skip = if cursor.is_some() {
        0
    } else {
        requested_offset
    };
    let mut matched_before_page = 0usize;
    let mut results = Vec::with_capacity(requested_limit);
    let mut scanned_count = 0usize;
    let mut stopped_at_since = false;

    while results.len() < requested_limit {
        if scanned_count >= budget.max_scanned_rows {
            return Err(format!(
                "YQ query scanned more than {} statuses; add a selective condition",
                budget.max_scanned_rows
            ));
        }
        if started_at.elapsed() >= budget.max_duration {
            return Err(
                "YQ query exceeded its execution budget; add a selective condition".to_string(),
            );
        }

        // Yield between bounded pages so dropping the IPC future cancels a
        // long filter promptly instead of monopolizing the async executor.
        tokio::task::yield_now().await;

        let page_limit =
            (budget.max_scanned_rows - scanned_count).min(YQ_FILTER_PAGE_SIZE as usize) as i64;
        let mut conditions = Vec::new();
        if !compiled_query.sql_prefilter().is_empty() {
            conditions.push(compiled_query.sql_prefilter().clause().to_string());
        }
        if stop_cursor.is_some() {
            conditions.push(
                "(s.created_at > ? OR (s.created_at = ? AND (s.server_domain > ? OR (s.server_domain = ? AND s.id > ?))))"
                    .to_string(),
            );
        }
        if cursor.is_some() {
            conditions.push(
                "(s.created_at < ? OR (s.created_at = ? AND (s.server_domain < ? OR (s.server_domain = ? AND s.id < ?))))"
                    .to_string(),
            );
        }
        let where_sql = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT s.* FROM statuses s
             {where_sql}
             ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
             LIMIT ?"
        );
        let mut db_query = sqlx::query_as::<_, DbStatus>(&sql);
        for binding in compiled_query.sql_prefilter().bindings() {
            db_query = match binding {
                crate::services::yq_filter::SqlPrefilterValue::Text(value) => db_query.bind(value),
                crate::services::yq_filter::SqlPrefilterValue::Integer(value) => {
                    db_query.bind(*value)
                }
            };
        }
        if let Some(stop_cursor) = stop_cursor.as_ref() {
            db_query = db_query
                .bind(&stop_cursor.created_at)
                .bind(&stop_cursor.created_at)
                .bind(&stop_cursor.server_domain)
                .bind(&stop_cursor.server_domain)
                .bind(&stop_cursor.id);
        }
        if let Some(cursor) = cursor.as_ref() {
            db_query = db_query
                .bind(&cursor.created_at)
                .bind(&cursor.created_at)
                .bind(&cursor.server_domain)
                .bind(&cursor.server_domain)
                .bind(&cursor.id);
        }
        let rows = db_query
            .bind(page_limit)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;

        if rows.is_empty() {
            break;
        }
        let reached_end = rows.len() < page_limit as usize;
        if let Some(last) = rows.last() {
            cursor = Some(LocalTimelineCursor {
                created_at: last.created_at.clone(),
                server_domain: last.server_domain.clone(),
                id: last.id.clone(),
            });
        }
        // Keep account hydration bounded to this raw page. A single request may
        // traverse thousands of statuses but should not retain every account.
        let mut account_cache: HashMap<(String, String), Option<DbAccount>> = HashMap::new();
        let mut missing_account_keys = Vec::new();
        for status in &rows {
            if stop_key.is_some_and(|(id, server_domain)| {
                status.id == id && status.server_domain == server_domain
            }) {
                break;
            }
            let account_key = (status.account_id.clone(), status.server_domain.clone());
            if !account_cache.contains_key(&account_key)
                && !missing_account_keys.contains(&account_key)
            {
                missing_account_keys.push(account_key);
            }
        }
        for account in accounts::get_accounts_by_keys(pool, &missing_account_keys)
            .await
            .map_err(|error| error.to_string())?
        {
            account_cache.insert(
                (account.id.clone(), account.server_domain.clone()),
                Some(account),
            );
        }
        for account_key in missing_account_keys {
            account_cache.entry(account_key).or_insert(None);
        }

        // yqrs::Context is intentionally local to this synchronous block (it
        // is not Send). The regex cache is Send and survives across pages.
        {
            let mut evaluator =
                crate::services::yq_filter::Evaluator::with_cache(evaluation_cache.clone());
            for status in rows {
                if stop_key.is_some_and(|(id, server_domain)| {
                    status.id == id && status.server_domain == server_domain
                }) {
                    stopped_at_since = true;
                    break;
                }
                scanned_count += 1;
                if scanned_count.is_multiple_of(64) && started_at.elapsed() >= budget.max_duration {
                    return Err(
                        "YQ query exceeded its execution budget; add a selective condition"
                            .to_string(),
                    );
                }
                let account_key = (status.account_id.clone(), status.server_domain.clone());
                let account = account_cache
                    .get(&account_key)
                    .and_then(|account| account.as_ref());

                if !evaluator.matches(&compiled_query, &status, account) {
                    continue;
                }

                if matched_before_page < matches_to_skip {
                    matched_before_page += 1;
                    continue;
                }

                results.push(status);
                if results.len() >= requested_limit {
                    break;
                }
            }
        }

        if reached_end || stopped_at_since {
            break;
        }
    }

    tracing::info!(
        query,
        limit,
        offset,
        stop_at = ?stop_at,
        start_after = ?start_after,
        max_scanned_rows = budget.max_scanned_rows,
        max_duration_ms = budget.max_duration.as_millis(),
        scanned_count,
        matched_count = results.len(),
        stopped_at_since,
        duration_ms = elapsed_ms(started_at),
        "[awayuki][tauri-db] yq query scan complete"
    );

    Ok(results)
}

async fn query_search_statuses(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    display_filter: Option<TimelineDisplayFilter>,
    start_after: Option<(&str, &str)>,
) -> Result<Vec<DbStatus>, String> {
    let terms = normalize_search_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let cursor = resolve_local_timeline_cursor(pool, start_after).await?;
    let filter_sql = timeline_display_filter_sql("s", display_filter);
    let cursor_sql = if cursor.is_some() {
        " AND (s.created_at < ? OR (s.created_at = ? AND (s.server_domain < ? OR (s.server_domain = ? AND s.id < ?))))"
    } else {
        ""
    };
    let term_sql = terms
        .iter()
        .map(|_| search_term_sql())
        .collect::<Vec<_>>()
        .join(" AND ");
    // Legacy FTS rows are populated after startup in resumable chunks. Until
    // the SQLite-backed cursor reaches completion, use the exact LIKE path so
    // an unindexed status can never become a false negative.
    let fts_query = if crate::services::search_backfill::is_complete(pool)
        .await
        .map_err(|error| error.to_string())?
    {
        search_fts_query(&terms)
    } else {
        None
    };
    let indexed_join = if fts_query.is_some() {
        "FROM status_search_fts
         JOIN status_search_documents search_document
           ON search_document.docid = status_search_fts.rowid
         JOIN statuses s
           ON s.id = search_document.status_id
          AND s.server_domain = search_document.server_domain"
    } else {
        "FROM statuses s"
    };
    let indexed_filter = if fts_query.is_some() {
        "status_search_fts MATCH ? AND"
    } else {
        ""
    };
    let sql = format!(
        "SELECT s.*
         {indexed_join}
         LEFT JOIN accounts a ON a.id = s.account_id AND a.server_domain = s.server_domain
         WHERE {indexed_filter} {}
         {}
         {cursor_sql}
         ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
         LIMIT ? OFFSET ?",
        term_sql, filter_sql
    );
    let mut db_query = sqlx::query_as::<_, DbStatus>(&sql);
    if let Some(fts_query) = fts_query {
        db_query = db_query.bind(fts_query);
    }
    for term in &terms {
        let pattern = search_like_pattern(term);
        for _ in 0..7 {
            db_query = db_query.bind(pattern.clone());
        }
    }
    if let Some(cursor) = cursor.as_ref() {
        db_query = db_query
            .bind(&cursor.created_at)
            .bind(&cursor.created_at)
            .bind(&cursor.server_domain)
            .bind(&cursor.server_domain)
            .bind(&cursor.id);
    }
    db_query
        .bind(limit)
        .bind(if cursor.is_some() { 0 } else { offset })
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

fn search_fts_query(terms: &[String]) -> Option<String> {
    let terms = terms
        .iter()
        // FTS5's trigram tokenizer cannot produce a token for shorter terms.
        // Such queries retain the legacy LIKE path instead of returning false
        // negatives for short Latin or Japanese searches.
        .filter(|term| term.chars().count() >= 3)
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

fn normalize_search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn search_like_pattern(term: &str) -> String {
    let mut pattern = String::with_capacity(term.len() + 2);
    pattern.push('%');
    for ch in term.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn search_term_sql() -> &'static str {
    "(lower(s.content) LIKE ? ESCAPE '\\'
      OR lower(s.spoiler_text) LIKE ? ESCAPE '\\'
      OR lower(s.uri) LIKE ? ESCAPE '\\'
      OR lower(coalesce(s.url, '')) LIKE ? ESCAPE '\\'
      OR lower(coalesce(s.tags_json, '')) LIKE ? ESCAPE '\\'
      OR lower(a.acct) LIKE ? ESCAPE '\\'
      OR lower(a.display_name) LIKE ? ESCAPE '\\')"
}

async fn query_bookmarked_statuses(
    pool: &sqlx::SqlitePool,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, String> {
    sqlx::query_as::<_, TimelineStatusRef>(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT
             v.server_domain,
             v.status_id,
             v.login_account_acct AS source_acct,
             ROW_NUMBER() OVER (
               PARTITION BY COALESCE(NULLIF(s.uri, ''), v.server_domain || ':' || v.status_id)
               ORDER BY v.updated_at DESC, v.login_account_acct DESC
             ) AS identity_rank,
             v.updated_at
           FROM status_viewer_state v
           JOIN statuses s ON s.id = v.status_id AND s.server_domain = v.server_domain
           WHERE v.bookmarked = 1
             AND (? IS NULL OR v.login_account_acct = ?)
         ) ranked
         WHERE identity_rank = 1
         ORDER BY updated_at DESC, server_domain DESC, status_id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(account_acct)
    .bind(account_acct)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())
}

async fn query_favourited_statuses(
    pool: &sqlx::SqlitePool,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, String> {
    sqlx::query_as::<_, TimelineStatusRef>(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT
             v.status_id,
             v.server_domain,
             v.login_account_acct AS source_acct,
             v.updated_at,
             ROW_NUMBER() OVER (
               PARTITION BY COALESCE(NULLIF(s.uri, ''), v.server_domain || ':' || v.status_id)
               ORDER BY v.updated_at DESC, v.login_account_acct DESC
             ) AS identity_rank
           FROM status_viewer_state v
           JOIN statuses s ON s.id = v.status_id AND s.server_domain = v.server_domain
           WHERE v.favourited = 1
             AND s.reblog_of_id IS NULL
             AND (? IS NULL OR v.login_account_acct = ?)
         ) ranked
         WHERE identity_rank = 1
         ORDER BY updated_at DESC, server_domain DESC, status_id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(account_acct)
    .bind(account_acct)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())
}

async fn query_user_bookmarked_statuses(
    pool: &sqlx::SqlitePool,
    server_domain: &str,
    account_id: &str,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, String> {
    sqlx::query_as::<_, TimelineStatusRef>(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT
             v.server_domain,
             v.status_id,
             v.login_account_acct AS source_acct,
             v.updated_at,
             ROW_NUMBER() OVER (
               PARTITION BY COALESCE(NULLIF(s.uri, ''), v.server_domain || ':' || v.status_id)
               ORDER BY v.updated_at DESC, v.login_account_acct DESC
             ) AS identity_rank
           FROM status_viewer_state v
           JOIN statuses s ON s.id = v.status_id AND s.server_domain = v.server_domain
           WHERE v.bookmarked = 1
             AND s.server_domain = ?
             AND s.account_id = ?
             AND (? IS NULL OR v.login_account_acct = ?)
         ) ranked
         WHERE identity_rank = 1
         ORDER BY updated_at DESC, server_domain DESC, status_id DESC
         LIMIT ? OFFSET ?",
    )
    .bind(server_domain)
    .bind(account_id)
    .bind(account_acct)
    .bind(account_acct)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())
}

async fn update_cached_status_poll(
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

async fn query_notification_statuses(
    pool: &sqlx::SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatus>, String> {
    let context = read_models::load_notification_page_context(pool, limit, offset)
        .await
        .map_err(|error| error.to_string())?;
    tracing::debug!(
        statement_count = context.statement_count,
        notification_count = context.notifications.len(),
        "Loaded notification page with bounded SQL statements"
    );
    let primary_statuses = context.statuses.values().cloned().collect::<Vec<_>>();
    let status_context = CachedStatusViewContext::load(pool, &primary_statuses).await?;
    let mut views = Vec::with_capacity(context.notifications.len());
    for notification in context.notifications {
        let actor_account = context
            .accounts
            .get(&(
                notification.account_id.clone(),
                notification.server_domain.clone(),
            ))
            .cloned();
        let status = notification.status_id.as_ref().and_then(|status_id| {
            context
                .statuses
                .get(&(status_id.clone(), notification.server_domain.clone()))
                .cloned()
        });
        views.push(notification_db_to_view_with_context(
            notification,
            actor_account,
            status,
            &status_context,
        ));
    }

    apply_viewer_states_to_views(pool, &mut views).await?;

    Ok(views)
}

fn notification_db_to_view_with_context(
    notification: DbNotification,
    actor_account: Option<DbAccount>,
    status: Option<DbStatus>,
    status_context: &CachedStatusViewContext,
) -> TimelineStatus {
    let Some(status) = status else {
        return notification_db_to_view(notification, actor_account, None, None);
    };

    let notification_id = notification.id.clone();
    let source_acct = notification.account_acct.clone();
    let actor_account_id = notification.account_id.clone();
    let actor_label = actor_account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| actor_account_id.clone());
    let actor_acct = actor_account
        .as_ref()
        .map(|account| format!("@{}", account.acct))
        .unwrap_or_else(|| format!("@{}", actor_account_id));
    let notification_label = Some(format!(
        "{} {}",
        actor_label,
        notification_type_label(&notification.notification_type)
    ));
    let notification_avatar = actor_account.as_ref().map(|account| account.avatar.clone());
    let notification_account_emojis = actor_account
        .as_ref()
        .and_then(|account| account.emojis_json.as_deref())
        .map(parse_custom_emoji_views)
        .unwrap_or_default();
    let notification_kind = Some(notification.notification_type.clone());
    let mut view = status_context.status_to_view_resolving_reblog(status);
    view.id = notification.id;
    view.created_at = notification.created_at;
    view.source_acct = source_acct;
    view.notification_id = Some(notification_id);
    view.notification_kind = notification_kind;
    view.notification_label = notification_label;
    view.notification_avatar = notification_avatar;
    view.notification_account_id = Some(actor_account_id);
    view.notification_acct = Some(actor_acct);
    view.notification_display_name = Some(actor_label);
    view.notification_account_emojis = notification_account_emojis;
    view
}

async fn query_account_statuses(
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

async fn query_status_thread_statuses(
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

async fn query_cached_status(
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

fn dedupe_statuses_by_uri(statuses: &mut Vec<Status>) {
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

async fn db_statuses_to_views(
    pool: &sqlx::SqlitePool,
    statuses: Vec<DbStatus>,
) -> Result<Vec<TimelineStatus>, String> {
    let primary_keys = status_keys_for_statuses(&statuses);
    let source_accts = query_latest_source_accts_by_status_keys(pool, &primary_keys).await?;
    let cache = CachedStatusViewContext::load(pool, &statuses).await?;
    let mut views = Vec::with_capacity(statuses.len());
    for status in statuses {
        let source_acct = source_accts
            .get(&status_key(&status.id, &status.server_domain))
            .cloned()
            .flatten();
        let view = cache.status_to_view_resolving_reblog(status);
        views.push(with_source_acct(view, source_acct));
    }
    apply_viewer_states_to_views(pool, &mut views).await?;
    Ok(views)
}

async fn db_status_refs_to_views(
    pool: &sqlx::SqlitePool,
    statuses: Vec<TimelineStatusRef>,
) -> Result<Vec<TimelineStatus>, String> {
    let primary_keys = status_keys_for_refs(&statuses);
    let status_cache = query_statuses_by_keys(pool, &primary_keys).await?;
    let primary_statuses = statuses
        .iter()
        .map(|status_ref| {
            status_cache
                .get(&status_key(
                    &status_ref.status_id,
                    &status_ref.server_domain,
                ))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "Status {} on {} is not cached",
                        status_ref.status_id, status_ref.server_domain
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cache = CachedStatusViewContext::load(pool, &primary_statuses).await?;

    let mut views = Vec::with_capacity(statuses.len());
    for (status_ref, status) in statuses.into_iter().zip(primary_statuses) {
        let view = cache.status_to_view_resolving_reblog(status);
        views.push(with_source_acct(view, status_ref.source_acct));
    }
    apply_viewer_states_to_views(pool, &mut views).await?;
    Ok(views)
}

fn with_source_acct(mut status: TimelineStatus, source_acct: Option<String>) -> TimelineStatus {
    status.source_acct = source_acct;
    status
}

async fn apply_viewer_states_to_views(
    pool: &sqlx::SqlitePool,
    views: &mut [TimelineStatus],
) -> Result<(), String> {
    let mut seen = HashSet::new();
    let keys = views
        .iter()
        .filter_map(|view| {
            let acct = view.source_acct.as_ref()?.clone();
            let key = (
                acct,
                view.status_identity.remote_id.clone(),
                view.status_identity.server_domain.clone(),
            );
            seen.insert(key.clone()).then_some(key)
        })
        .collect::<Vec<_>>();
    let viewer_states = status_queries::get_viewer_states_by_keys(pool, &keys)
        .await
        .map_err(|error| error.to_string())?;
    for view in views {
        let Some(acct) = view.source_acct.as_ref() else {
            continue;
        };
        let key = (
            acct.clone(),
            view.status_identity.remote_id.clone(),
            view.status_identity.server_domain.clone(),
        );
        if let Some(viewer) = viewer_states.get(&key) {
            view.favourited = viewer.favourited.unwrap_or(false);
            view.reblogged = viewer.reblogged.unwrap_or(false);
            view.bookmarked = viewer.bookmarked.unwrap_or(false);
        }
    }
    Ok(())
}

struct CachedStatusViewContext {
    statuses: HashMap<StatusCacheKey, DbStatus>,
    accounts: HashMap<StatusCacheKey, DbAccount>,
}

impl CachedStatusViewContext {
    async fn load(pool: &sqlx::SqlitePool, statuses: &[DbStatus]) -> Result<Self, String> {
        let mut statuses_by_key = HashMap::new();
        for status in statuses {
            statuses_by_key.insert(status_key_for_status(status), status.clone());
        }

        let mut related_status_keys = Vec::new();
        let mut seen_status_keys = statuses_by_key.keys().cloned().collect::<HashSet<_>>();
        for status in statuses {
            if let Some(reblog_of_id) = status.reblog_of_id.as_deref() {
                push_unique_status_key(
                    &mut related_status_keys,
                    &mut seen_status_keys,
                    reblog_of_id,
                    &status.server_domain,
                );
            }
            if let Some(quote_id) = status.quote_id.as_deref() {
                push_unique_status_key(
                    &mut related_status_keys,
                    &mut seen_status_keys,
                    quote_id,
                    &status.server_domain,
                );
            }
        }

        let related_statuses = query_statuses_by_keys(pool, &related_status_keys).await?;
        for (key, status) in related_statuses {
            statuses_by_key.entry(key).or_insert(status);
        }

        let mut original_quote_keys = Vec::new();
        let mut seen_status_keys = statuses_by_key.keys().cloned().collect::<HashSet<_>>();
        for status in statuses {
            let Some(reblog_of_id) = status.reblog_of_id.as_deref() else {
                continue;
            };
            let Some(original) =
                statuses_by_key.get(&status_key(reblog_of_id, &status.server_domain))
            else {
                continue;
            };
            let Some(quote_id) = original.quote_id.as_deref() else {
                continue;
            };
            push_unique_status_key(
                &mut original_quote_keys,
                &mut seen_status_keys,
                quote_id,
                &original.server_domain,
            );
        }

        let original_quotes = query_statuses_by_keys(pool, &original_quote_keys).await?;
        for (key, status) in original_quotes {
            statuses_by_key.entry(key).or_insert(status);
        }

        let mut account_keys = Vec::new();
        let mut seen_account_keys = HashSet::new();
        for status in statuses_by_key.values() {
            push_unique_status_key(
                &mut account_keys,
                &mut seen_account_keys,
                &status.account_id,
                &status.server_domain,
            );
        }
        let accounts = accounts::get_accounts_by_keys(pool, &account_keys)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|account| ((account.id.clone(), account.server_domain.clone()), account))
            .collect();

        Ok(Self {
            statuses: statuses_by_key,
            accounts,
        })
    }

    fn status_to_view_resolving_reblog(&self, status: DbStatus) -> TimelineStatus {
        let account = self.account_for_status(&status);
        let Some(reblog_of_id) = status.reblog_of_id.clone() else {
            return self.status_to_view_with_cached_quote(status, account);
        };

        let Some(original) = self
            .statuses
            .get(&status_key(&reblog_of_id, &status.server_domain))
            .cloned()
        else {
            return self.status_to_view_with_cached_quote(status, account);
        };
        let original_account = self.account_for_status(&original);
        let booster = account
            .as_ref()
            .map(|account| {
                if account.display_name.is_empty() {
                    format!("@{}", account.acct)
                } else {
                    account.display_name.clone()
                }
            })
            .unwrap_or_else(|| status.account_id.clone());
        let booster_avatar = account.as_ref().map(|account| account.avatar.clone());
        let top_level_uri = status.uri.clone();
        let mut view = self.status_to_view_with_cached_quote(original, original_account);
        view.id = status.id;
        view.uri = top_level_uri;
        view.original_created_at = Some(view.created_at.clone());
        view.created_at = status.created_at;
        view.notification_label = Some(format!("{} boosted", booster));
        view.notification_kind = Some("reblog".to_string());
        view.notification_avatar = booster_avatar;
        view.notification_account_id = Some(status.account_id.clone());
        view.notification_acct = Some(
            account
                .as_ref()
                .map(|account| format!("@{}", account.acct))
                .unwrap_or_else(|| format!("@{}", status.account_id)),
        );
        view.notification_display_name = Some(booster);
        view.notification_account_emojis = account
            .as_ref()
            .and_then(|account| account.emojis_json.as_deref())
            .map(parse_custom_emoji_views)
            .unwrap_or_default();
        view
    }

    fn status_to_view_with_cached_quote(
        &self,
        status: DbStatus,
        account: Option<DbAccount>,
    ) -> TimelineStatus {
        let quote_id = status.quote_id.clone();
        let server_domain = status.server_domain.clone();
        let mut view = db_status_to_view(status, account);
        let Some(quote_id) = quote_id else {
            return view;
        };
        let Some(quote) = self
            .statuses
            .get(&status_key(&quote_id, &server_domain))
            .cloned()
        else {
            return view;
        };
        let quote_account = self.account_for_status(&quote);
        view.quote = Some(Box::new(db_status_to_view(quote, quote_account)));
        view
    }

    fn account_for_status(&self, status: &DbStatus) -> Option<DbAccount> {
        self.accounts
            .get(&status_key(&status.account_id, &status.server_domain))
            .cloned()
    }
}

fn status_key(id: &str, server_domain: &str) -> StatusCacheKey {
    (id.to_string(), server_domain.to_string())
}

fn status_key_for_status(status: &DbStatus) -> StatusCacheKey {
    status_key(&status.id, &status.server_domain)
}

fn status_keys_for_statuses(statuses: &[DbStatus]) -> Vec<StatusCacheKey> {
    let mut keys = Vec::with_capacity(statuses.len());
    let mut seen = HashSet::new();
    for status in statuses {
        push_unique_status_key(&mut keys, &mut seen, &status.id, &status.server_domain);
    }
    keys
}

fn status_keys_for_refs(statuses: &[TimelineStatusRef]) -> Vec<StatusCacheKey> {
    let mut keys = Vec::with_capacity(statuses.len());
    let mut seen = HashSet::new();
    for status in statuses {
        push_unique_status_key(
            &mut keys,
            &mut seen,
            &status.status_id,
            &status.server_domain,
        );
    }
    keys
}

fn push_unique_status_key(
    keys: &mut Vec<StatusCacheKey>,
    seen: &mut HashSet<StatusCacheKey>,
    id: &str,
    server_domain: &str,
) {
    let key = status_key(id, server_domain);
    if seen.insert(key.clone()) {
        keys.push(key);
    }
}

async fn query_statuses_by_keys(
    pool: &sqlx::SqlitePool,
    keys: &[StatusCacheKey],
) -> Result<HashMap<StatusCacheKey, DbStatus>, String> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder = QueryBuilder::<Sqlite>::new("SELECT * FROM statuses WHERE ");
    push_status_key_predicates(&mut builder, keys, "id", "server_domain");

    builder
        .build_query_as::<DbStatus>()
        .fetch_all(pool)
        .await
        .map(|statuses| {
            statuses
                .into_iter()
                .map(|status| (status_key_for_status(&status), status))
                .collect()
        })
        .map_err(|error| error.to_string())
}

async fn query_latest_source_accts_by_status_keys(
    pool: &sqlx::SqlitePool,
    keys: &[StatusCacheKey],
) -> Result<HashMap<StatusCacheKey, Option<String>>, String> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT server_domain, status_id, account_acct AS source_acct,
                  ROW_NUMBER() OVER (
                    PARTITION BY server_domain, status_id
                    ORDER BY position_at DESC
                  ) AS source_rank
           FROM timeline_entries
           WHERE ",
    );
    push_status_key_predicates(&mut builder, keys, "status_id", "server_domain");
    builder.push(") ranked WHERE source_rank = 1");

    builder
        .build_query_as::<StatusSourceAcctRef>()
        .fetch_all(pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    (
                        status_key(&row.status_id, &row.server_domain),
                        row.source_acct,
                    )
                })
                .collect()
        })
        .map_err(|error| error.to_string())
}

fn push_status_key_predicates<'args>(
    builder: &mut QueryBuilder<'args, Sqlite>,
    keys: &'args [StatusCacheKey],
    id_column: &str,
    server_domain_column: &str,
) {
    for (index, (id, server_domain)) in keys.iter().enumerate() {
        if index > 0 {
            builder.push(" OR ");
        }
        builder
            .push("(")
            .push(id_column)
            .push(" = ")
            .push_bind(id)
            .push(" AND ")
            .push(server_domain_column)
            .push(" = ")
            .push_bind(server_domain)
            .push(")");
    }
}

fn db_status_to_view(status: DbStatus, account: Option<DbAccount>) -> TimelineStatus {
    let status_identity = StatusIdentity::inferred(&status.server_domain, &status.uri, &status.id);
    let media = status
        .media_attachments_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<MediaAttachment>>(json).ok())
        .unwrap_or_default();
    let poll = status.poll_json.as_deref().and_then(parse_poll_view);
    let emojis = status
        .emojis_json
        .as_deref()
        .map(parse_custom_emoji_views)
        .unwrap_or_default();
    let account_emojis = account
        .as_ref()
        .and_then(|account| account.emojis_json.as_deref())
        .map(parse_custom_emoji_views)
        .unwrap_or_default();
    let display_name = account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| status.account_id.clone());
    let acct = account
        .as_ref()
        .map(|account| format!("@{}", account.acct))
        .unwrap_or_else(|| format!("@{}", status.account_id));
    let avatar = account.map(|account| account.avatar).unwrap_or_default();
    let in_reply_to_id = status.in_reply_to_id.clone();
    let in_reply_to_account_id = status.in_reply_to_account_id.clone();

    TimelineStatus {
        id: status.id.clone(),
        original_status_id: status.id,
        status_identity,
        source_acct: None,
        account_id: status.account_id,
        server_domain: status.server_domain,
        uri: status.uri,
        url: status.url,
        display_name,
        acct,
        avatar,
        created_at: status.created_at,
        original_created_at: None,
        in_reply_to_id,
        in_reply_to_account_id,
        content: status.content,
        spoiler_text: status.spoiler_text,
        language: status.language,
        application_name: application_name_from_json(status.application_json.as_deref()),
        reblogs_count: status.reblogs_count,
        favourites_count: status.favourites_count,
        replies_count: status.replies_count,
        visibility: status.visibility,
        sensitive: status.sensitive,
        favourited: status.favourited.unwrap_or(false),
        reblogged: status.reblogged.unwrap_or(false),
        bookmarked: status.bookmarked.unwrap_or(false),
        media,
        poll,
        emojis,
        account_emojis,
        quote_id: status.quote_id,
        quote_original_url: status.quote_original_url,
        quote: None,
        notification_id: None,
        notification_kind: None,
        notification_label: None,
        notification_avatar: None,
        notification_account_id: None,
        notification_acct: None,
        notification_display_name: None,
        notification_account_emojis: Vec::new(),
    }
}

fn application_name_from_json(json: Option<&str>) -> Option<String> {
    json.and_then(|json| serde_json::from_str::<StatusApplication>(json).ok())
        .and_then(|application| normalized_application_name(&application.name))
}

fn status_application_name(status: &Status) -> Option<String> {
    status
        .application
        .as_ref()
        .and_then(|application| normalized_application_name(&application.name))
}

fn normalized_application_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn account_profile_to_view(
    account: DbAccount,
    is_self: bool,
    relationship: Option<AccountRelationshipSummary>,
    notification_muted: bool,
    url: Option<String>,
) -> AccountProfileSummary {
    let fields = account
        .fields_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<AccountFieldSummary>>(json).ok())
        .unwrap_or_default();

    AccountProfileSummary {
        id: account.id,
        server_domain: account.server_domain,
        username: account.username,
        acct: account.acct,
        url,
        display_name: account.display_name,
        note: account.note,
        avatar: account.avatar,
        header: account.header,
        fields,
        account_emojis: account
            .emojis_json
            .as_deref()
            .map(parse_custom_emoji_views)
            .unwrap_or_default(),
        statuses_count: account.statuses_count,
        following_count: account.following_count,
        followers_count: account.followers_count,
        is_self,
        relationship,
        notification_muted,
    }
}

fn preserve_cached_profile_media(account: &mut DbAccount, cached: &DbAccount) {
    if account.avatar.trim().is_empty() {
        account.avatar.clone_from(&cached.avatar);
    }
    if account.avatar_static.trim().is_empty() {
        account.avatar_static.clone_from(&cached.avatar_static);
    }
    if account.header.trim().is_empty() {
        account.header.clone_from(&cached.header);
    }
    if account.emojis_json.is_none() {
        account.emojis_json.clone_from(&cached.emojis_json);
    }
}

fn status_to_view(
    status: &Status,
    server_domain: &str,
    notification_label: Option<String>,
) -> TimelineStatus {
    if let Some(reblog) = status.reblog.as_ref() {
        let booster = if status.account.display_name.is_empty() {
            format!("@{}", status.account.acct)
        } else {
            status.account.display_name.clone()
        };
        let mut view = status_to_view_base(
            reblog,
            server_domain,
            Some(notification_label.unwrap_or_else(|| format!("{} boosted", booster))),
            Some(status.account.avatar.clone()),
        );
        view.original_created_at = Some(view.created_at.clone());
        view.id = status.id.clone();
        view.uri = status.uri.clone();
        view.created_at = status.created_at.to_rfc3339();
        view.notification_kind = Some("reblog".to_string());
        view.notification_account_id = Some(status.account.id.clone());
        view.notification_acct = Some(format!("@{}", status.account.acct));
        view.notification_display_name = Some(status.account.display_name.clone());
        view.notification_account_emojis = custom_emojis_to_views(&status.account.emojis);
        return view;
    }

    status_to_view_base(status, server_domain, notification_label, None)
}

fn status_to_view_base(
    status: &Status,
    server_domain: &str,
    notification_label: Option<String>,
    notification_avatar: Option<String>,
) -> TimelineStatus {
    status_to_view_base_with_quote_depth(
        status,
        server_domain,
        notification_label,
        notification_avatar,
        2,
    )
}

fn status_to_view_base_with_quote_depth(
    status: &Status,
    server_domain: &str,
    notification_label: Option<String>,
    notification_avatar: Option<String>,
    quote_depth: usize,
) -> TimelineStatus {
    let quote = if quote_depth == 0 {
        None
    } else {
        status.quote.as_deref().map(|quote| {
            let mut view = status_to_view_base_with_quote_depth(
                quote.reblog.as_deref().unwrap_or(quote),
                server_domain,
                None,
                None,
                quote_depth - 1,
            );
            if quote.reblog.is_some() {
                view.original_created_at = Some(view.created_at.clone());
                view.id = quote.id.clone();
                view.uri = quote.uri.clone();
                view.created_at = quote.created_at.to_rfc3339();
                view.notification_kind = Some("reblog".to_string());
            }
            Box::new(view)
        })
    };

    TimelineStatus {
        id: status.id.clone(),
        original_status_id: status.id.clone(),
        status_identity: StatusIdentity::inferred(server_domain, &status.uri, &status.id),
        source_acct: None,
        account_id: status.account.id.clone(),
        server_domain: server_domain.to_string(),
        uri: status.uri.clone(),
        url: status.url.clone(),
        display_name: status.account.display_name.clone(),
        acct: format!("@{}", status.account.acct),
        avatar: status.account.avatar.clone(),
        created_at: status.created_at.to_rfc3339(),
        original_created_at: None,
        in_reply_to_id: status.in_reply_to_id.clone(),
        in_reply_to_account_id: status.in_reply_to_account_id.clone(),
        content: status.content.clone(),
        spoiler_text: status.spoiler_text.clone(),
        language: status.language.clone(),
        application_name: status_application_name(status),
        reblogs_count: status.reblogs_count,
        favourites_count: status.favourites_count,
        replies_count: status.replies_count,
        visibility: status.visibility.clone(),
        sensitive: status.sensitive,
        favourited: status.favourited.unwrap_or(false),
        reblogged: status.reblogged.unwrap_or(false),
        bookmarked: status.bookmarked.unwrap_or(false),
        media: status.media_attachments.clone(),
        poll: status.poll.as_ref().map(poll_to_view),
        emojis: custom_emojis_to_views(&status.emojis),
        account_emojis: custom_emojis_to_views(&status.account.emojis),
        quote_id: status.quote_id.clone(),
        quote_original_url: status.quote_original_url.clone(),
        quote,
        notification_id: None,
        notification_kind: None,
        notification_label,
        notification_avatar,
        notification_account_id: None,
        notification_acct: None,
        notification_display_name: None,
        notification_account_emojis: Vec::new(),
    }
}

fn notification_db_to_view(
    notification: DbNotification,
    actor_account: Option<DbAccount>,
    status: Option<DbStatus>,
    status_account: Option<DbAccount>,
) -> TimelineStatus {
    let source_acct = notification.account_acct.clone();
    let actor_account_id = notification.account_id.clone();
    let actor_display_name = actor_account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| actor_account_id.clone());
    let actor_acct = actor_account
        .as_ref()
        .map(|account| format!("@{}", account.acct))
        .unwrap_or_else(|| format!("@{}", actor_account_id));
    let actor_avatar = actor_account
        .as_ref()
        .map(|account| account.avatar.clone())
        .unwrap_or_default();
    let label_account = actor_account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| notification.account_id.clone());
    let notification_label = Some(format!(
        "{} {}",
        label_account,
        notification_type_label(&notification.notification_type)
    ));
    let notification_kind = Some(notification.notification_type.clone());
    let notification_avatar = actor_account.as_ref().map(|account| account.avatar.clone());
    let notification_account_emojis = actor_account
        .as_ref()
        .and_then(|account| account.emojis_json.as_deref())
        .map(parse_custom_emoji_views)
        .unwrap_or_default();

    match status {
        Some(status) => {
            let notification_id = notification.id.clone();
            let mut view = db_status_to_view(status, status_account);
            view.id = notification.id;
            view.created_at = notification.created_at;
            view.source_acct = source_acct;
            view.notification_id = Some(notification_id);
            view.notification_kind = notification_kind;
            view.notification_label = notification_label;
            view.notification_avatar = notification_avatar;
            view.notification_account_id = Some(actor_account_id);
            view.notification_acct = Some(actor_acct);
            view.notification_display_name = Some(actor_display_name);
            view.notification_account_emojis = notification_account_emojis;
            view
        }
        None => {
            let notification_id = notification.id.clone();
            let status_identity = StatusIdentity::inferred(
                &notification.server_domain,
                format!(
                    "https://{}/notifications/{}",
                    notification.server_domain, notification.id
                ),
                &notification.id,
            );
            TimelineStatus {
                id: notification.id,
                original_status_id: notification.status_id.unwrap_or_default(),
                status_identity,
                source_acct,
                account_id: actor_account_id.clone(),
                server_domain: notification.server_domain,
                uri: String::new(),
                url: None,
                display_name: actor_display_name.clone(),
                acct: actor_acct.clone(),
                avatar: actor_avatar,
                created_at: notification.created_at,
                original_created_at: None,
                in_reply_to_id: None,
                in_reply_to_account_id: None,
                content: String::new(),
                spoiler_text: String::new(),
                language: None,
                application_name: None,
                reblogs_count: 0,
                favourites_count: 0,
                replies_count: 0,
                visibility: "direct".to_string(),
                sensitive: false,
                favourited: false,
                reblogged: false,
                bookmarked: false,
                media: Vec::new(),
                poll: None,
                emojis: Vec::new(),
                account_emojis: notification_account_emojis.clone(),
                quote_id: None,
                quote_original_url: None,
                quote: None,
                notification_id: Some(notification_id),
                notification_kind,
                notification_label,
                notification_avatar,
                notification_account_id: Some(actor_account_id),
                notification_acct: Some(actor_acct),
                notification_display_name: Some(actor_display_name),
                notification_account_emojis,
            }
        }
    }
}

fn notification_to_view(
    notification: &crate::mastodon::types::notification::Notification,
    server_domain: &str,
    source_acct: Option<&str>,
) -> TimelineStatus {
    let notification_label = Some(format!(
        "{} {}",
        notification.account.display_name,
        notification.notification_type.label()
    ));
    let notification_kind = Some(notification.notification_type.as_str().to_string());
    let notification_avatar = Some(notification.account.avatar.clone());
    let notification_account_id = Some(notification.account.id.clone());
    let notification_acct = Some(format!("@{}", notification.account.acct));
    let notification_display_name = Some(notification.account.display_name.clone());
    let notification_account_emojis = custom_emojis_to_views(&notification.account.emojis);

    let Some(status) = notification.status.as_ref() else {
        return TimelineStatus {
            id: notification.id.clone(),
            original_status_id: notification.id.clone(),
            status_identity: StatusIdentity::inferred(
                server_domain,
                format!("https://{server_domain}/notifications/{}", notification.id),
                &notification.id,
            ),
            source_acct: source_acct.map(str::to_string),
            account_id: notification.account.id.clone(),
            server_domain: server_domain.to_string(),
            uri: String::new(),
            url: None,
            display_name: notification.account.display_name.clone(),
            acct: format!("@{}", notification.account.acct),
            avatar: notification.account.avatar.clone(),
            created_at: notification.created_at.to_rfc3339(),
            original_created_at: None,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            content: String::new(),
            spoiler_text: String::new(),
            language: None,
            application_name: None,
            reblogs_count: 0,
            favourites_count: 0,
            replies_count: 0,
            visibility: "direct".to_string(),
            sensitive: false,
            favourited: false,
            reblogged: false,
            bookmarked: false,
            media: Vec::new(),
            poll: None,
            emojis: Vec::new(),
            account_emojis: custom_emojis_to_views(&notification.account.emojis),
            quote_id: None,
            quote_original_url: None,
            quote: None,
            notification_id: Some(notification.id.clone()),
            notification_kind,
            notification_label,
            notification_avatar,
            notification_account_id,
            notification_acct,
            notification_display_name,
            notification_account_emojis,
        };
    };

    let mut view = status_to_view_base(
        status,
        server_domain,
        notification_label,
        notification_avatar,
    );
    view.id = notification.id.clone();
    view.created_at = notification.created_at.to_rfc3339();
    view.source_acct = source_acct.map(str::to_string);
    view.notification_id = Some(notification.id.clone());
    view.notification_kind = notification_kind;
    view.notification_account_id = notification_account_id;
    view.notification_acct = notification_acct;
    view.notification_display_name = notification_display_name;
    view.notification_account_emojis = notification_account_emojis;
    view
}

fn custom_emojis_to_views(
    emojis: &[crate::mastodon::types::account::CustomEmoji],
) -> Vec<CustomEmojiView> {
    emojis
        .iter()
        .map(|emoji| CustomEmojiView {
            shortcode: emoji.shortcode.clone(),
            url: emoji.url.clone(),
            static_url: emoji.static_url.clone(),
            category: emoji.category.clone(),
        })
        .collect()
}

fn poll_to_view(poll: &Poll) -> PollView {
    PollView {
        id: poll.id.clone(),
        expires_at: poll.expires_at.map(|expires_at| expires_at.to_rfc3339()),
        expired: poll.expired,
        multiple: poll.multiple,
        votes_count: poll.votes_count,
        voters_count: poll.voters_count,
        options: poll
            .options
            .iter()
            .map(|option| PollOptionView {
                title: option.title.clone(),
                votes_count: option.votes_count,
            })
            .collect(),
        voted: poll.voted,
        own_votes: poll.own_votes.clone(),
        emojis: custom_emojis_to_views(&poll.emojis),
    }
}

fn parse_poll_view(json: &str) -> Option<PollView> {
    serde_json::from_str::<Poll>(json)
        .ok()
        .map(|poll| poll_to_view(&poll))
}

fn parse_custom_emoji_views(json: &str) -> Vec<CustomEmojiView> {
    serde_json::from_str::<Vec<crate::mastodon::types::account::CustomEmoji>>(json)
        .map(|emojis| custom_emojis_to_views(&emojis))
        .unwrap_or_default()
}

fn notification_type_label(notification_type: &str) -> &'static str {
    match notification_type {
        "mention" => "mentioned you",
        "reblog" => "boosted",
        "favourite" => "favourited",
        "follow" => "followed you",
        "follow_request" => "requested to follow",
        "status" => "posted",
        "update" => "edited",
        "poll" => "poll ended",
        "admin.sign_up" => "signed up",
        "admin.report" => "reported",
        _ => "notified you",
    }
}

fn format_size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    if bytes >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", bytes / 1024.0 / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / 1024.0 / 1024.0)
    } else if bytes >= 1024.0 {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{} B", bytes as i64)
    }
}

trait NotificationTypeLabel {
    fn label(&self) -> &'static str;
}

impl NotificationTypeLabel for crate::mastodon::types::notification::NotificationType {
    fn label(&self) -> &'static str {
        use crate::mastodon::types::notification::NotificationType;
        match self {
            NotificationType::Mention => "mentioned you",
            NotificationType::Reblog => "boosted",
            NotificationType::Favourite => "favourited",
            NotificationType::Follow => "followed you",
            NotificationType::FollowRequest => "requested to follow",
            NotificationType::Status => "posted",
            NotificationType::Update => "edited",
            NotificationType::Poll => "poll ended",
            NotificationType::AdminSignUp => "signed up",
            NotificationType::AdminReport => "reported",
            _ => "notified you",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for column_type in ["local", "list", "hashtag"] {
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

    #[tokio::test]
    async fn pending_notification_side_effect_does_not_block_following_ui_events() {
        let (emit_sender, mut emit_receiver) = mpsc::channel(4);
        let emit_queue = QueuedEmitter {
            sender: emit_sender,
        };
        let (side_effect_sender, mut side_effect_receiver) = mpsc::unbounded_channel();
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
                Box::new(status),
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
        for _ in 0..3 {
            let queued = tokio::time::timeout(Duration::from_secs(1), emit_receiver.recv())
                .await
                .expect("UI event must not wait for side effects")
                .expect("UI event queue remains open");
            assert_eq!(queued.event, TIMELINE_STREAM_EVENT);
            let payload: serde_json::Value = serde_json::from_str(&queued.payload).unwrap();
            kinds.push(payload["kind"].as_str().unwrap().to_string());
        }
        assert_eq!(kinds, ["newNotification", "statusUpdate", "deleteStatus"]);

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
            })
            .await
            .unwrap();
        let emit_queue = QueuedEmitter {
            sender: emit_sender,
        };
        let (side_effect_sender, mut side_effect_receiver) = mpsc::unbounded_channel();
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
    fn main_capability_does_not_match_remote_sidecar_webviews() {
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
        assert!(windows_config["app"].get("withGlobalTauri").is_none());
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
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/012_add_status_quote_id.sql"),
            include_str!("../../migrations/020_create_status_search_fts.sql"),
        ] {
            sqlx::raw_sql(migration).execute(&pool).await.unwrap();
        }

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

        let statuses = query_aggregate_timeline_statuses(&pool, "home", 10, 0, None)
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
    async fn aggregate_timeline_query_dedupes_remote_copies_by_uri() {
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
        for (id, server_domain, position_at) in [
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
                "INSERT INTO timeline_entries
                   (timeline_type, server_domain, status_id, account_acct, position_at)
                 VALUES ('home', ?, ?, 'viewer@example.test', ?)",
            )
            .bind(server_domain)
            .bind(id)
            .bind(position_at)
            .execute(&pool)
            .await
            .unwrap();
        }

        let statuses = query_aggregate_timeline_statuses(&pool, "home", 10, 0, None)
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

        let statuses = query_aggregate_timeline_statuses(&pool, "home", 10, 0, None)
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
        )
        .await
        .unwrap();
        let second_page = query_custom_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY created_at DESC LIMIT 2",
            1,
            1,
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
        )
        .await
        .unwrap_err();
        assert!(error.contains("execution budget"), "{error}");

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
    async fn search_uses_exact_fallback_while_legacy_fts_backfill_is_incomplete() {
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
        sqlx::raw_sql(include_str!(
            "../../migrations/023_resumable_status_search_backfill.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "DELETE FROM status_search_fts
              WHERE rowid = (
                  SELECT docid FROM status_search_documents
                   WHERE status_id = 'not-indexed-yet'
                     AND server_domain = 'example.test'
              )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "DELETE FROM status_search_documents
              WHERE status_id = 'not-indexed-yet' AND server_domain = 'example.test'",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert!(!crate::services::search_backfill::is_complete(&pool)
            .await
            .unwrap());
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
             VALUES ('status-1', 'example.test', 'https://example.test/statuses/status-1', '0001', 'author-1', '<p>embeddedneedlevalue 100%_safe</p>')",
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            query_search_statuses(&pool, "needle", 10, 0, None, None)
                .await
                .unwrap()
                .len(),
            1
        );
        // Trigram FTS cannot index terms shorter than three characters. Those
        // terms must keep the escaped substring fallback without false matches.
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
            1
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
    async fn search_fts_migration_backfills_existing_statuses() {
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

        sqlx::raw_sql(include_str!(
            "../../migrations/020_create_status_search_fts.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let by_content = query_search_statuses(&pool, "preexisting", 10, 0, None, None)
            .await
            .unwrap();
        let by_account = query_search_statuses(&pool, "backfilled-author", 10, 0, None, None)
            .await
            .unwrap();
        assert_eq!(by_content.len(), 1);
        assert_eq!(by_account.len(), 1);

        // statuses has no INTEGER PRIMARY KEY alias, so its implicit rowid is
        // not stable across maintenance. Search joins through the persistent
        // document mapping and must not depend on that physical row number.
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

        let plan = sqlx::query_as::<_, (i64, i64, i64, String)>(
            "EXPLAIN QUERY PLAN
             SELECT s.id
             FROM status_search_fts
             JOIN status_search_documents search_document
               ON search_document.docid = status_search_fts.rowid
             JOIN statuses s
               ON s.id = search_document.status_id
              AND s.server_domain = search_document.server_domain
             WHERE status_search_fts MATCH ?",
        )
        .bind("\"unique-search-needle\"")
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
        sqlx::query(&format!(
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
        ))
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

    #[test]
    fn download_path_never_reuses_an_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-download-path-test-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&directory).expect("create download test directory");
        let existing = directory.join("photo.png");
        std::fs::write(&existing, b"existing").expect("write existing download");

        let candidate = unique_download_path(existing.clone());

        assert_eq!(candidate, directory.join("photo (1).png"));
        assert!(!candidate.exists());
        assert_eq!(std::fs::read(existing).expect("read existing"), b"existing");
        std::fs::remove_dir_all(directory).expect("remove download test directory");
    }

    #[test]
    fn settings_registry_rejects_unknown_keys_and_invalid_values() {
        assert!(validated_settings_json("unknown", serde_json::json!({})).is_err());
        assert!(validated_settings_json(
            "appearance",
            serde_json::json!({
                "avatar_shape": "Triangle",
                "font_size": "Medium",
                "cw_behavior": "Hide",
                "nsfw_behavior": "Hide",
                "display_mode": "StarryEyes"
            })
        )
        .is_err());
    }

    #[test]
    fn settings_registry_round_trips_every_supported_type() {
        let fixtures = [
            (
                "appearance",
                serde_json::to_value(AppearanceSettings::default()).unwrap(),
            ),
            (
                "performance",
                serde_json::to_value(PerformanceSettings::default()).unwrap(),
            ),
            (
                "confirmation",
                serde_json::to_value(ConfirmationSettings::default()).unwrap(),
            ),
            (
                "bluesky_fetch",
                serde_json::to_value(BlueskyFetchSettings::default()).unwrap(),
            ),
            (
                "sidecars",
                serde_json::to_value(SidecarSettings::default()).unwrap(),
            ),
            ("account_source_colors", serde_json::json!({})),
            (
                "preset_visibility",
                serde_json::to_value(PresetVisibilitySettings::default()).unwrap(),
            ),
            (
                "debug",
                serde_json::to_value(DebugSettings::default()).unwrap(),
            ),
            (
                "notification_suppression",
                serde_json::to_value(NotificationSuppressionList::default()).unwrap(),
            ),
        ];

        for (key, value) in fixtures {
            let json = validated_settings_json(key, value).expect("validate setting");
            assert!(
                serde_json::from_str::<serde_json::Value>(&json).is_ok(),
                "{key}"
            );
        }
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
            stream_types_for_columns(&columns, Some("alice@example.test"), ServerKind::Mastodon),
            vec![StreamType::User]
        );
        assert_eq!(
            stream_types_for_columns(&columns, Some("bob@example.test"), ServerKind::Misskey),
            vec![StreamType::User]
        );
        assert_eq!(
            stream_types_for_columns(&columns, Some("bluesky.bsky.social"), ServerKind::Bluesky),
            vec![StreamType::User, StreamType::UserNotification]
        );
        assert_eq!(
            stream_types_for_columns(
                &[stream_column("notification", "stale-bluesky.bsky.social")],
                Some("alice@example.test"),
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
                stream_types_for_columns(&columns, Some("any@example.test"), kind),
                vec![StreamType::Public]
            );
        }
        assert!(stream_types_for_columns(
            &columns,
            Some("stale-bluesky.bsky.social"),
            ServerKind::Bluesky
        )
        .is_empty());
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
                ServerKind::Mastodon
            ),
            vec![StreamType::List("friends".to_string())]
        );
        assert!(
            stream_types_for_columns(&[list], Some("bob@example.test"), ServerKind::Mastodon)
                .is_empty()
        );
        let mut unbound = stream_column("list", "discarded@example.test");
        unbound.account_acct = None;
        unbound.column_param = Some("friends".to_string());
        assert!(stream_types_for_columns(
            &[unbound],
            Some("active-must-not-be-used@example.test"),
            ServerKind::Mastodon
        )
        .is_empty());
    }
}
