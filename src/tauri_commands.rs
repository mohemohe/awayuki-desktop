use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use apple_ai::{AppleAiClient, GenerationOptions, Message};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{QueryBuilder, Sqlite};
use tauri::webview::PageLoadEvent;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewBuilder, WebviewUrl,
    WebviewWindow, WindowEvent,
};
use tokio::sync::{mpsc, RwLock};
use url::Url;

use crate::api::client::ApiClient;
use crate::api::detect::detect_server_kind;
use crate::api::kind::ServerKind;
use crate::auth::callback_server;
use crate::auth::credential_store::CredentialStore;
use crate::auth::session::{AccountSession, SessionManager};
use crate::bluesky::auth::login_with_app_password;
use crate::bluesky::client::{BlueskyClient, DEFAULT_BLUESKY_HOST};
use crate::constants::{APP_VERSION, DEFAULT_TIMELINE_LIMIT};
use crate::db::models::{DbAccount, DbColumnConfig, DbLoginAccount, DbNotification, DbStatus};
use crate::db::pool::Database;
use crate::db::queries::{accounts, notification_mutes, servers, settings, tags};
use crate::mastodon::client::MastodonClient;
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::statuses::{CreatePollParams, CreateStatusParams, VotePollParams};
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::oauth::OAuthFlow;
use crate::mastodon::types::account::Account;
use crate::mastodon::types::instance::Instance;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::{MediaAttachment, Poll, Status};
use crate::misskey::auth::MiAuthFlow;
use crate::misskey::client::MisskeyClient;
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::{self, TimelineType};
use crate::state::account_source_color::AccountSourceColor;
use crate::state::appearance::AppearanceSettings;
use crate::state::bluesky_fetch::BlueskyFetchSettings;
use crate::state::confirmation::{ConfirmationSettings, TranslationEngine};
use crate::state::debug_settings::DebugSettings;
use crate::state::logging;
use crate::state::notifications::NotificationSuppressionList;
use crate::state::paths;
use crate::state::performance::{PerformanceSettings, SuggestionSource};
use crate::state::preset_visibility::PresetVisibilitySettings;

pub struct RuntimeState {
    database: Arc<Database>,
    sessions: RwLock<SessionManager>,
    streaming_handles: RwLock<Vec<tokio::task::AbortHandle>>,
    emit_queue: QueuedEmitter,
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

    fn try_emit<T>(&self, event: &'static str, payload: T, context: &str) -> bool
    where
        T: Serialize,
    {
        let Ok(payload) = serialize_emit_payload(event, payload, context) else {
            return false;
        };
        match self.sender.try_send(QueuedEmit { event, payload }) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => false,
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::warn!(
                    event,
                    context,
                    "Failed to queue Tauri event: emit queue closed"
                );
                false
            }
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
const LEGACY_WINDOW_STATE_FILENAME: &str = "window_state.json";
const WINDOW_STATE_SAVE_DEBOUNCE_MS: u64 = 400;
const TIMELINE_STREAM_EVENT: &str = "timeline-stream-event";
const STARTUP_SYNC_COMPLETE_EVENT: &str = "timeline-startup-sync-complete";
const STARTUP_SYNC_LIMIT: u32 = 80;
const EMIT_QUEUE_CAPACITY: usize = 1024;
const MASTODON_DEFAULT_CHARACTER_LIMIT: i32 = 500;
const MISSKEY_DEFAULT_CHARACTER_LIMIT: i32 = 3000;
const SIDECAR_MIN_WIDTH: u32 = 160;
const SIDECAR_DEFAULT_WIDTH: u32 = 500;
const SIDECAR_USER_STYLE_RETRY_DELAYS_MS: [u64; 5] = [0, 150, 400, 900, 1800];
const BLUESKY_CHARACTER_LIMIT: i32 = 300;
static SIDECAR_USER_STYLES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
const YQ_FILTER_PAGE_SIZE: i64 = 250;
static DROPPED_STREAM_EMITS: AtomicU64 = AtomicU64::new(0);

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn log_timeline_command_result(
    command: &str,
    column_type: &str,
    column_param: &Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    since_status_id: &Option<String>,
    since_server_domain: &Option<String>,
    started_at: Instant,
    result: &Result<Vec<TimelineStatus>, String>,
) {
    match result {
        Ok(statuses) => tracing::info!(
            command,
            column_type,
            column_param = ?column_param,
            limit = ?limit,
            offset = ?offset,
            since_status_id = ?since_status_id,
            since_server_domain = ?since_server_domain,
            count = statuses.len(),
            duration_ms = elapsed_ms(started_at),
            "[awayuki][tauri-command] timeline command success"
        ),
        Err(error) => tracing::info!(
            command,
            column_type,
            column_param = ?column_param,
            limit = ?limit,
            offset = ?offset,
            since_status_id = ?since_status_id,
            since_server_domain = ?since_server_domain,
            duration_ms = elapsed_ms(started_at),
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
struct AccountSummary {
    acct: String,
    server_domain: String,
    account_id: String,
    display_name: String,
    avatar: String,
    is_active: bool,
    server_kind: String,
    character_limit: i32,
    rate_limit: Option<AccountRateLimitSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountListSummary {
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

#[derive(Debug, Clone)]
struct ServerMetadataSnapshot {
    streaming_url: String,
    version: Option<String>,
    max_characters: i32,
    instance_json: Option<String>,
    server_kind: ServerKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountFieldSummary {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountRelationshipSummary {
    following: bool,
    followed_by: bool,
    requested: bool,
    blocking: bool,
    muting: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountProfileSummary {
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
struct DbSummary {
    path: String,
    size: String,
    status_count: i64,
    recent_status_count: i64,
    account_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusBarSnapshot {
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
struct SettingsSnapshot {
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
            if !is_supported_sidecar_url(&url) {
                return Err("Sidecar URL must start with http:// or https://".to_string());
            }
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

fn is_supported_sidecar_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

#[tauri::command]
async fn create_sidecar_webview(
    app: AppHandle,
    sidecar_id: String,
    url: String,
    user_style: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let url = parse_sidecar_url(&url)?;
    let label = sidecar_webview_label(&sidecar_id);
    set_sidecar_user_style(&label, user_style);
    if app.get_webview(&label).is_some() {
        schedule_sidecar_user_style_injection(app, label);
        return Ok(());
    }

    let window = app
        .get_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;
    let app_for_navigation = app.clone();
    let label_for_navigation = label.clone();
    let label_for_page_load = label.clone();
    let builder = WebviewBuilder::new(label, WebviewUrl::External(url))
        .on_navigation(move |_| {
            schedule_sidecar_user_style_injection(
                app_for_navigation.clone(),
                label_for_navigation.clone(),
            );
            true
        })
        .on_page_load(move |webview, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                if let Err(error) =
                    eval_sidecar_user_style(&webview, &get_sidecar_user_style(&label_for_page_load))
                {
                    tracing::warn!(
                        target: "awayuki::sidecar",
                        sidecar = %label_for_page_load,
                        "Failed to inject sidecar UserStyle on page load: {}",
                        error
                    );
                }
            }
        });

    window
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width, height),
        )
        .map_err(|error| error.to_string())?;
    schedule_sidecar_user_style_injection(app, sidecar_webview_label(&sidecar_id));
    Ok(())
}

#[tauri::command]
fn navigate_sidecar_webview(app: AppHandle, sidecar_id: String, url: String) -> Result<(), String> {
    let url = parse_sidecar_url(&url)?;
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    webview.navigate(url).map_err(|error| error.to_string())?;
    schedule_sidecar_user_style_injection(app, label);
    Ok(())
}

#[tauri::command]
fn reload_sidecar_webview(app: AppHandle, sidecar_id: String) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    webview.reload().map_err(|error| error.to_string())?;
    schedule_sidecar_user_style_injection(app, label);
    Ok(())
}

#[tauri::command]
fn scroll_sidecar_webview_to_top(app: AppHandle, sidecar_id: String) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    webview
        .eval("window.scrollTo({ top: 0, left: 0, behavior: 'smooth' });")
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn inject_sidecar_user_style(
    app: AppHandle,
    sidecar_id: String,
    user_style: String,
) -> Result<(), String> {
    let label = sidecar_webview_label(&sidecar_id);
    let webview = app
        .get_webview(&label)
        .ok_or_else(|| format!("Sidecar WebView not found: {}", label))?;
    set_sidecar_user_style(&label, user_style);
    eval_sidecar_user_style(&webview, &get_sidecar_user_style(&label))
}

fn sidecar_user_style_store() -> &'static Mutex<HashMap<String, String>> {
    SIDECAR_USER_STYLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_sidecar_user_style(label: &str, user_style: String) {
    if let Ok(mut user_styles) = sidecar_user_style_store().lock() {
        user_styles.insert(label.to_string(), user_style);
    }
}

fn get_sidecar_user_style(label: &str) -> String {
    sidecar_user_style_store()
        .lock()
        .ok()
        .and_then(|user_styles| user_styles.get(label).cloned())
        .unwrap_or_default()
}

fn schedule_sidecar_user_style_injection(app: AppHandle, label: String) {
    std::thread::spawn(move || {
        for delay_ms in SIDECAR_USER_STYLE_RETRY_DELAYS_MS {
            if delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            let Some(webview) = app.get_webview(&label) else {
                return;
            };
            if let Err(error) = eval_sidecar_user_style(&webview, &get_sidecar_user_style(&label)) {
                tracing::debug!(
                    target: "awayuki::sidecar",
                    sidecar = %label,
                    "Sidecar UserStyle injection retry failed: {}",
                    error
                );
            }
        }
    });
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
  const install = () => {{
    if (!state.css.trim()) {{
      removeStyle();
      return;
    }}
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
  const schedule = () => {{
    if (state.scheduled) return;
    state.scheduled = true;
    const run = () => {{
      state.scheduled = false;
      install();
    }};
    if (typeof requestAnimationFrame === "function") {{
      requestAnimationFrame(run);
    }} else {{
      setTimeout(run, 0);
    }}
  }};
  if (!state) {{
    state = {{
      css: "",
      scheduled: false,
      observer: null,
      historyPatched: false,
    }};
    win[STATE_KEY] = state;
  }}
  state.css = css;
  state.install = install;
  state.schedule = schedule;
  if (!state.historyPatched) {{
    state.historyPatched = true;
    for (const method of ["pushState", "replaceState"]) {{
      const original = history[method];
      history[method] = function (...args) {{
        const result = original.apply(this, args);
        schedule();
        return result;
      }};
    }}
    addEventListener("popstate", schedule);
    addEventListener("hashchange", schedule);
  }}
  if (!state.observer && document.documentElement) {{
    state.observer = new MutationObserver(schedule);
    state.observer.observe(document.documentElement, {{
      childList: true,
      subtree: true,
    }});
  }}
  install();
}})();
"#
    ))
}

fn parse_sidecar_url(url: &str) -> Result<Url, String> {
    let trimmed = url.trim();
    if !is_supported_sidecar_url(trimmed) {
        return Err("Sidecar URL must start with http:// or https://".to_string());
    }
    Url::parse(trimmed).map_err(|error| error.to_string())
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
struct AppSnapshot {
    version: String,
    accounts: Vec<AccountSummary>,
    active_acct: Option<String>,
    columns: Vec<ColumnSummary>,
    settings: SettingsSnapshot,
    database: DbSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationMutedAccountSummary {
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
struct TimelineStatus {
    id: String,
    original_status_id: String,
    source_acct: Option<String>,
    account_id: String,
    server_domain: String,
    uri: String,
    url: Option<String>,
    display_name: String,
    acct: String,
    avatar: String,
    created_at: String,
    in_reply_to_id: Option<String>,
    in_reply_to_account_id: Option<String>,
    content: String,
    spoiler_text: String,
    language: Option<String>,
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineRequest {
    column_type: String,
    column_param: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    max_status_id: Option<String>,
    since_status_id: Option<String>,
    since_server_domain: Option<String>,
    account_acct: Option<String>,
    display_filter: Option<TimelineDisplayFilter>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TimelinePageResponse {
    statuses: Vec<TimelineStatus>,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountListsRequest {
    acct: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountProfileRequest {
    account_id: String,
    server_domain: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountTimelineRequest {
    account_id: String,
    server_domain: String,
    only_media: Option<bool>,
    pinned: Option<bool>,
    limit: Option<u32>,
    offset: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusThreadRequest {
    status_id: String,
    server_domain: String,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AirContextRequest {
    status_id: String,
    server_domain: String,
    account_id: String,
    account_acct: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountFollowRequest {
    account_id: String,
    server_domain: String,
    action: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountNotificationMuteRequest {
    account_id: String,
    server_domain: String,
    muted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginInstanceRequest {
    domain: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginBlueskyRequest {
    identifier: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostRequest {
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
struct UploadMediaRequest {
    filename: String,
    mime_type: Option<String>,
    data: Vec<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadMediaPathRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComposeSuggestionRequest {
    query: String,
    limit: Option<u32>,
    account_acct: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionSuggestionView {
    acct: String,
    display_name: String,
    avatar: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HashtagSuggestionView {
    name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CustomEmojiView {
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
struct PollView {
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
struct SaveSettingsRequest {
    key: String,
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslateStatusRequest {
    text: String,
    source_language: Option<String>,
    target_language: String,
    #[serde(default)]
    translation_engine: Option<TranslationEngine>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslateStatusResponse {
    text: String,
    source_language: Option<String>,
    target_language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveColumnsRequest {
    columns: Vec<ColumnSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusActionRequest {
    status_id: String,
    action: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VotePollRequest {
    status_id: String,
    server_domain: String,
    poll_id: String,
    choices: Vec<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EditStatusRequest {
    status_id: String,
    server_domain: String,
    account_id: String,
    status: String,
    visibility: Option<String>,
    spoiler_text: Option<String>,
    sensitive: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteStatusRequest {
    status_id: String,
    server_domain: String,
    account_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadMediaRequest {
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
            let state = tauri::async_runtime::block_on(init_runtime_state(app.handle().clone()))?;
            if let Some(window) = app.get_webview_window("main") {
                let database = state.database.clone();
                tauri::async_runtime::block_on(restore_window_state(&window, &database));
                install_window_state_persistence(window, database);
            }
            tauri::async_runtime::block_on(restart_streaming(&state));
            schedule_startup_sync(&state);
            app.manage(state);
            crate::updater::init_updater();
            crate::updater::schedule_periodic_update_checks(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_snapshot,
            account_summaries,
            account_lists,
            login_with_instance_domain,
            login_with_bluesky_app_password,
            load_timeline,
            load_more_timeline,
            refresh_timeline,
            status_thread,
            air_context,
            account_profile,
            account_timeline,
            account_follow_action,
            notification_muted_accounts,
            set_account_notification_mute,
            post_status,
            upload_compose_media,
            upload_compose_media_path,
            autocomplete_mentions,
            autocomplete_hashtags,
            custom_emojis,
            edit_own_status,
            delete_own_status,
            vote_poll,
            switch_active_account,
            logout_account,
            save_settings,
            translate_status_text,
            save_columns,
            vacuum_database,
            clear_status_cache,
            status_bar_snapshot,
            status_action,
            download_media,
            open_status_url,
            create_sidecar_webview,
            navigate_sidecar_webview,
            reload_sidecar_webview,
            scroll_sidecar_webview_to_top,
            inject_sidecar_user_style,
            open_log_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}

async fn init_runtime_state(
    app_handle: AppHandle,
) -> Result<RuntimeState, Box<dyn std::error::Error>> {
    let db_path = paths::db_path();
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let database = Arc::new(Database::new(&db_path.to_string_lossy()).await?);
    database.run_migrations().await?;
    apply_debug_logging_settings(&database).await;

    let mut sessions = SessionManager::new();
    let accounts = settings::get_login_accounts(database.reader())
        .await
        .unwrap_or_default();
    let active_acct = accounts
        .iter()
        .find(|account| account.is_active)
        .map(|account| account.acct.clone());

    for account in accounts {
        match restore_session(&account).await {
            Ok(session) => {
                if matches!(session.client.kind(), ServerKind::Bluesky) {
                    let access_token = session.client.current_access_token().await;
                    let app_password = session.client.bluesky_app_password();
                    if let Err(error) = settings::update_login_credentials(
                        database.writer(),
                        &session.acct,
                        &access_token,
                        app_password.as_deref(),
                    )
                    .await
                    {
                        tracing::warn!(
                            "Failed to persist restored Bluesky session {}: {}",
                            session.acct,
                            error
                        );
                    }
                }
                sessions.add_session(session)
            }
            Err(error) => tracing::warn!("Failed to restore session {}: {}", account.acct, error),
        }
    }

    if let Some(acct) = active_acct {
        sessions.set_active(&acct);
    }

    Ok(RuntimeState {
        database,
        sessions: RwLock::new(sessions),
        streaming_handles: RwLock::new(Vec::new()),
        emit_queue: QueuedEmitter::start(app_handle),
        started_at: Instant::now(),
    })
}

async fn apply_debug_logging_settings(database: &Database) {
    let debug = match settings::get_setting(database.reader(), "debug").await {
        Ok(Some(json)) => serde_json::from_str::<DebugSettings>(&json).unwrap_or_default(),
        Ok(None) => DebugSettings::default(),
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
        Ok(Some(json)) => return parse_saved_window_state(&json, "app_settings.window_state"),
        Ok(None) => {}
        Err(error) => tracing::warn!("Failed to load window state: {}", error),
    }

    let legacy_path = paths::data_dir().join(LEGACY_WINDOW_STATE_FILENAME);
    let json = std::fs::read_to_string(&legacy_path).ok()?;
    parse_saved_window_state(&json, &legacy_path.display().to_string())
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
    let save_generation = Arc::new(AtomicU64::new(0));
    let event_window = window.clone();

    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. } => {
            let generation = save_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let save_generation = save_generation.clone();
            let window = event_window.clone();
            let database = database.clone();

            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(WINDOW_STATE_SAVE_DEBOUNCE_MS)).await;
                if save_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Err(error) = persist_window_state(&window, &database).await {
                    tracing::warn!("Failed to save window state: {}", error);
                }
            });
        }
        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => {
            if let Err(error) =
                tauri::async_runtime::block_on(persist_window_state(&event_window, &database))
            {
                tracing::warn!("Failed to save window state: {}", error);
            }
        }
        _ => {}
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

async fn restore_session(row: &DbLoginAccount) -> Result<AccountSession, String> {
    let kind = ServerKind::from_db_str(&row.server_kind);
    let streaming_url = format!("wss://{}", row.server_domain);
    let client = match kind {
        ServerKind::Misskey => ApiClient::Misskey(
            MisskeyClient::new(&row.server_domain, row.access_token.clone(), streaming_url)
                .map_err(|error| error.to_string())?,
        ),
        ServerKind::Bluesky => ApiClient::Bluesky(
            BlueskyClient::from_stored(
                &row.server_domain,
                row.access_token.clone(),
                streaming_url,
                row.app_password.clone(),
            )
            .await
            .map_err(|error| error.to_string())?,
        ),
        ServerKind::Mastodon | ServerKind::Paon => ApiClient::Mastodon(
            MastodonClient::new(&row.server_domain, row.access_token.clone(), streaming_url)
                .map_err(|error| error.to_string())?,
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

#[tauri::command]
async fn app_snapshot(state: State<'_, RuntimeState>) -> Result<AppSnapshot, String> {
    app_snapshot_for_state(&state).await
}

#[tauri::command]
async fn account_summaries(state: State<'_, RuntimeState>) -> Result<Vec<AccountSummary>, String> {
    login_accounts(&state).await
}

#[tauri::command]
async fn account_lists(
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

#[tauri::command]
async fn login_with_instance_domain(
    state: State<'_, RuntimeState>,
    request: LoginInstanceRequest,
) -> Result<AppSnapshot, String> {
    let domain = normalize_login_domain(&request.domain)?;
    let (session, kind) = run_login_flow(&domain, state.database.clone()).await?;
    persist_login_session(&state, session, kind).await
}

#[tauri::command]
async fn login_with_bluesky_app_password(
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

async fn run_login_flow(
    domain: &str,
    database: Arc<Database>,
) -> Result<(AccountSession, ServerKind), String> {
    let kind = detect_server_kind(domain)
        .await
        .map_err(|error| error.to_string())?;
    tracing::info!("Detected server kind for {}: {:?}", domain, kind);
    match kind {
        ServerKind::Mastodon | ServerKind::Paon => run_mastodon_oauth(domain, database, kind).await,
        ServerKind::Misskey => run_misskey_miauth(domain, kind).await,
        ServerKind::Bluesky => Err(
            "Bluesky cannot be configured via instance domain; use the Bluesky login form below."
                .to_string(),
        ),
    }
}

async fn run_mastodon_oauth(
    domain: &str,
    database: Arc<Database>,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), String> {
    let port = callback_server::find_available_port()
        .await
        .map_err(|error| error.to_string())?;
    let mut flow = OAuthFlow::new(domain, port).map_err(|error| error.to_string())?;
    flow.prepare().await.map_err(|error| error.to_string())?;
    let auth_url = flow
        .authorize_url()
        .ok_or_else(|| "Failed to generate authorization URL".to_string())?;

    let callback_handle = tokio::spawn(callback_server::wait_for_callback(port));
    tracing::info!("Opening browser for Mastodon authorization");
    open::that(&auth_url).map_err(|error| error.to_string())?;

    let code = callback_handle
        .await
        .map_err(|error| error.to_string())?
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

    let registration = flow
        .registration
        .as_ref()
        .ok_or_else(|| "No app registration".to_string())?;
    CredentialStore::save_client_credentials(
        database.writer(),
        domain,
        &registration.client_id,
        &registration.client_secret,
    )
    .await
    .map_err(|error| error.to_string())?;

    let client = ApiClient::Mastodon(
        MastodonClient::new(domain, token_response.access_token, streaming_url)
            .map_err(|error| error.to_string())?,
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
    let port = callback_server::find_available_port()
        .await
        .map_err(|error| error.to_string())?;
    let flow = MiAuthFlow::new(domain, port).map_err(|error| error.to_string())?;
    let auth_url = flow.authorize_url();

    let callback_handle = tokio::spawn(async move {
        callback_server::wait_for_callback_any(port, &["session", "code"]).await
    });
    tracing::info!("Opening browser for Misskey authorization");
    open::that(&auth_url).map_err(|error| error.to_string())?;
    callback_handle
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let result = flow.check().await.map_err(|error| error.to_string())?;
    let streaming_url = format!("wss://{}", domain);
    let client = ApiClient::Misskey(
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
    let client = ApiClient::Bluesky(
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
    let access_token = session.client.current_access_token().await;
    let app_password = session.client.bluesky_app_password();
    let login_account = DbLoginAccount {
        acct: session.acct.clone(),
        server_domain: session.domain.clone(),
        account_id: session.account_info.id.clone(),
        display_name: session.account_info.display_name.clone(),
        avatar: session.account_info.avatar.clone(),
        is_active: true,
        access_token,
        server_kind: kind.as_db_str().to_string(),
        app_password,
    };

    settings::upsert_login_account(state.database.writer(), &login_account)
        .await
        .map_err(|error| error.to_string())?;
    settings::set_active_account(state.database.writer(), &session.acct)
        .await
        .map_err(|error| error.to_string())?;
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

#[tauri::command]
async fn load_timeline(
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

#[tauri::command]
async fn load_more_timeline(
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

#[tauri::command]
async fn refresh_timeline(
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
        log_timeline_command_result(
            "refresh_timeline",
            &request_column_type,
            &request_column_param,
            request_limit,
            request_offset,
            &request_since_status_id,
            &request_since_server_domain,
            total_started_at,
            &result,
        );
        return result;
    }

    if is_aggregate_timeline(&tl_type) {
        let result =
            refresh_aggregate_timeline(&state, &tl_type, limit, request.display_filter).await;
        log_timeline_command_result(
            "refresh_timeline",
            &request_column_type,
            &request_column_param,
            request_limit,
            request_offset,
            &request_since_status_id,
            &request_since_server_domain,
            total_started_at,
            &result,
        );
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
        log_timeline_command_result(
            "refresh_timeline",
            &request_column_type,
            &request_column_param,
            request_limit,
            request_offset,
            &request_since_status_id,
            &request_since_server_domain,
            total_started_at,
            &result,
        );
        return result;
    }

    let session = session_for_timeline_request(&state, request.account_acct.as_deref()).await?;
    let client = session.client;
    let active_acct = session.acct;
    let statuses = timeline_service::sync_timeline(
        &client,
        state.database.writer(),
        state.database.reader(),
        &tl_type,
        &active_acct,
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
                Some(active_acct.clone()),
            )
        })
        .filter(|status| timeline_status_matches_display_filter(status, display_filter))
        .collect());
    log_timeline_command_result(
        "refresh_timeline",
        &request_column_type,
        &request_column_param,
        request_limit,
        request_offset,
        &request_since_status_id,
        &request_since_server_domain,
        total_started_at,
        &result,
    );
    result
}

#[tauri::command]
async fn account_profile(
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

#[tauri::command]
async fn account_timeline(
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

#[tauri::command]
async fn air_context(
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

#[tauri::command]
async fn status_thread(
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
        for status in &remote_statuses {
            timeline_service::save_status_to_db_with_retry(
                state.database.writer(),
                status,
                session.client.domain(),
            )
            .await
            .map_err(|error| error.to_string())?;
        }
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

    db_statuses_to_views(state.database.reader(), statuses).await
}

#[tauri::command]
async fn account_follow_action(
    state: State<'_, RuntimeState>,
    request: AccountFollowRequest,
) -> Result<AccountRelationshipSummary, String> {
    let session = session_for_domain(&state, &request.server_domain)
        .await
        .ok_or_else(|| "No signed-in account for this server".to_string())?;
    let relationship = match request.action.as_str() {
        "follow" => session
            .client
            .follow_account(&request.account_id)
            .await
            .map_err(|error| error.to_string())?,
        "unfollow" => session
            .client
            .unfollow_account(&request.account_id)
            .await
            .map_err(|error| error.to_string())?,
        "mute" => session
            .client
            .mute_account(&request.account_id)
            .await
            .map_err(|error| error.to_string())?,
        "unmute" => session
            .client
            .unmute_account(&request.account_id)
            .await
            .map_err(|error| error.to_string())?,
        "block" => session
            .client
            .block_account(&request.account_id)
            .await
            .map_err(|error| error.to_string())?,
        "unblock" => session
            .client
            .unblock_account(&request.account_id)
            .await
            .map_err(|error| error.to_string())?,
        other => return Err(format!("Unsupported account action: {}", other)),
    };
    Ok(AccountRelationshipSummary {
        following: relationship.following,
        followed_by: relationship.followed_by,
        requested: relationship.requested,
        blocking: relationship.blocking,
        muting: relationship.muting,
    })
}

#[tauri::command]
async fn set_account_notification_mute(
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

#[tauri::command]
async fn notification_muted_accounts(
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

#[tauri::command]
async fn post_status(
    state: State<'_, RuntimeState>,
    request: PostRequest,
) -> Result<TimelineStatus, String> {
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
        return Err("Post text is empty".to_string());
    }
    let preset_visibility = load_setting::<PresetVisibilitySettings>(&state, "preset_visibility")
        .await?
        .match_visibility(&status_text)
        .map(|visibility| visibility.as_request_visibility().to_string());
    let (client, _) = active_client(&state).await?;
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
        .map_err(|error| error.to_string())?;
    timeline_service::hydrate_missing_quotes(&client, std::slice::from_mut(&mut status)).await;

    timeline_service::save_status_to_db_with_retry(
        state.database.writer(),
        &status,
        client.domain(),
    )
    .await
    .map_err(|error| error.to_string())?;

    Ok(status_to_view(&status, client.domain(), None))
}

#[tauri::command]
async fn upload_compose_media(
    state: State<'_, RuntimeState>,
    request: UploadMediaRequest,
) -> Result<MediaAttachment, String> {
    if request.data.is_empty() {
        return Err("Media file is empty".to_string());
    }
    let (client, _) = active_client(&state).await?;
    let filename = sanitize_upload_filename(&request.filename, request.mime_type.as_deref());
    let path = std::env::temp_dir().join(format!(
        "awayuki-upload-{}-{}",
        Utc::now().timestamp_millis(),
        filename
    ));
    tokio::fs::write(&path, request.data)
        .await
        .map_err(|error| error.to_string())?;
    let result = client.upload_media(&path).await;
    let _ = tokio::fs::remove_file(&path).await;
    result.map_err(|error| error.to_string())
}

#[tauri::command]
async fn upload_compose_media_path(
    state: State<'_, RuntimeState>,
    request: UploadMediaPathRequest,
) -> Result<MediaAttachment, String> {
    let path = PathBuf::from(request.path);
    if !path.is_file() {
        return Err("Dropped item is not a file".to_string());
    }
    let (client, _) = active_client(&state).await?;
    client
        .upload_media(&path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn autocomplete_mentions(
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

#[tauri::command]
async fn autocomplete_hashtags(
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

#[tauri::command]
async fn custom_emojis(state: State<'_, RuntimeState>) -> Result<Vec<CustomEmojiView>, String> {
    let (client, _) = active_client(&state).await?;
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

#[tauri::command]
async fn switch_active_account(
    state: State<'_, RuntimeState>,
    acct: String,
) -> Result<AppSnapshot, String> {
    settings::set_active_account(state.database.writer(), &acct)
        .await
        .map_err(|error| error.to_string())?;
    state.sessions.write().await.set_active(&acct);
    app_snapshot(state).await
}

#[tauri::command]
async fn logout_account(
    state: State<'_, RuntimeState>,
    acct: String,
) -> Result<AppSnapshot, String> {
    settings::delete_login_account(state.database.writer(), &acct)
        .await
        .map_err(|error| error.to_string())?;
    state.sessions.write().await.remove_session(&acct);
    restart_streaming(state.inner()).await;
    app_snapshot(state).await
}

#[tauri::command]
async fn save_settings(
    state: State<'_, RuntimeState>,
    request: SaveSettingsRequest,
) -> Result<SettingsSnapshot, String> {
    let allowed = [
        "appearance",
        "performance",
        "confirmation",
        "bluesky_fetch",
        "sidecars",
        "account_source_colors",
        "preset_visibility",
        "debug",
        "notification_suppression",
    ];
    if !allowed.contains(&request.key.as_str()) {
        return Err(format!("Unsupported settings key: {}", request.key));
    }
    let json = if request.key == "bluesky_fetch" {
        let settings = serde_json::from_value::<BlueskyFetchSettings>(request.value.clone())
            .map_err(|error| error.to_string())?
            .normalized();
        serde_json::to_string(&settings).map_err(|error| error.to_string())?
    } else if request.key == "sidecars" {
        let settings = serde_json::from_value::<SidecarSettings>(request.value.clone())
            .map_err(|error| error.to_string())?
            .normalized()?;
        serde_json::to_string(&settings).map_err(|error| error.to_string())?
    } else {
        serde_json::to_string(&request.value).map_err(|error| error.to_string())?
    };
    settings::set_setting(state.database.writer(), &request.key, &json)
        .await
        .map_err(|error| error.to_string())?;

    if request.key == "debug" {
        if let Ok(debug) = serde_json::from_value::<DebugSettings>(request.value) {
            if debug.logging_enabled {
                logging::enable().map_err(|error| error.to_string())?;
            } else {
                logging::disable();
            }
            logging::set_log_level(debug.log_level);
        }
    }

    if request.key == "bluesky_fetch" {
        restart_streaming(state.inner()).await;
    }

    settings_snapshot(&state).await
}

#[tauri::command]
async fn translate_status_text(
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

#[tauri::command]
async fn save_columns(
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

    let session_acct = {
        let sessions = state.sessions.read().await;
        sessions
            .active_session()
            .map(|session| session.acct.clone())
    };
    let active_acct = match session_acct {
        Some(acct) => acct,
        None => settings::get_active_login_account(state.database.reader())
            .await
            .ok()
            .flatten()
            .map(|account| account.acct)
            .unwrap_or_else(|| "global".to_string()),
    };

    settings::delete_all_column_configs_global(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;

    for (index, column) in columns.into_iter().enumerate() {
        let account_acct = column
            .account_acct
            .clone()
            .filter(|acct| !acct.trim().is_empty())
            .unwrap_or_else(|| active_acct.clone());
        let config = DbColumnConfig {
            id: column.id.clone(),
            account_acct,
            column_type: column.column_type.clone(),
            column_param: encode_column_param_with_display_filter(&column),
            position: column.position,
            width: None,
            created_at: String::new(),
            name: Some(column.name.clone()),
            max_statuses: Some(column.max_statuses.max(1) as i32),
            pane_index: Some(column.pane_index as i32),
        };

        settings::upsert_column_config(state.database.writer(), &config)
            .await
            .map_err(|error| format!("Failed to save column {}: {}", index + 1, error))?;
    }

    restart_streaming(state.inner()).await;
    app_snapshot(state).await
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

#[tauri::command]
async fn vacuum_database(state: State<'_, RuntimeState>) -> Result<DbSummary, String> {
    settings::vacuum(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    database_summary(&state).await
}

#[tauri::command]
async fn clear_status_cache(state: State<'_, RuntimeState>) -> Result<DbSummary, String> {
    settings::clear_status_cache(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    database_summary(&state).await
}

#[tauri::command]
async fn status_bar_snapshot(state: State<'_, RuntimeState>) -> Result<StatusBarSnapshot, String> {
    status_bar_summary(&state).await
}

#[tauri::command]
async fn status_action(
    state: State<'_, RuntimeState>,
    request: StatusActionRequest,
) -> Result<TimelineStatus, String> {
    let (client, active_acct) = active_client(&state).await?;
    let mut status = match request.action.as_str() {
        "favourite" => client.favourite(&request.status_id).await,
        "unfavourite" => client.unfavourite(&request.status_id).await,
        "reblog" => client.reblog(&request.status_id).await,
        "unreblog" => client.unreblog(&request.status_id).await,
        "bookmark" => client.bookmark(&request.status_id).await,
        "unbookmark" => client.unbookmark(&request.status_id).await,
        other => return Err(format!("Unsupported status action: {}", other)),
    }
    .map_err(|error| error.to_string())?;
    timeline_service::hydrate_missing_quotes(&client, std::slice::from_mut(&mut status)).await;

    timeline_service::save_status_to_db_with_retry(
        state.database.writer(),
        &status,
        client.domain(),
    )
    .await
    .map_err(|error| error.to_string())?;

    if request.action == "favourite" {
        timeline_service::insert_timeline_entry_with_retry(
            state.database.writer(),
            "favourites",
            client.domain(),
            &status.id,
            &active_acct,
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
        .bind(&active_acct)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    } else if request.action == "bookmark" {
        timeline_service::insert_timeline_entry_with_retry(
            state.database.writer(),
            "bookmarks",
            client.domain(),
            &status.id,
            &active_acct,
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
        .bind(&active_acct)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    }

    Ok(with_source_acct(
        status_to_view(&status, client.domain(), None),
        Some(active_acct),
    ))
}

#[tauri::command]
async fn vote_poll(
    state: State<'_, RuntimeState>,
    request: VotePollRequest,
) -> Result<PollView, String> {
    if request.choices.is_empty() {
        return Err("Select at least one poll option".to_string());
    }

    let session = session_for_domain(&state, &request.server_domain)
        .await
        .ok_or_else(|| "No signed-in account can vote on this poll".to_string())?;
    let poll = session
        .client
        .vote_poll(
            &request.poll_id,
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

#[tauri::command]
async fn edit_own_status(
    state: State<'_, RuntimeState>,
    request: EditStatusRequest,
) -> Result<TimelineStatus, String> {
    let status_text = request.status.trim().to_string();
    if status_text.is_empty() {
        return Err("Post text is empty".to_string());
    }
    let session = session_for_status_owner(&state, &request.server_domain, &request.account_id)
        .await
        .ok_or_else(|| "No signed-in account owns this post".to_string())?;
    let mut status = session
        .client
        .edit_status(
            &request.status_id,
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

    timeline_service::save_status_to_db_with_retry(
        state.database.writer(),
        &status,
        session.client.domain(),
    )
    .await
    .map_err(|error| error.to_string())?;

    Ok(status_to_view(&status, session.client.domain(), None))
}

#[tauri::command]
async fn delete_own_status(
    state: State<'_, RuntimeState>,
    request: DeleteStatusRequest,
) -> Result<(), String> {
    let session = session_for_status_owner(&state, &request.server_domain, &request.account_id)
        .await
        .ok_or_else(|| "No signed-in account owns this post".to_string())?;
    session
        .client
        .delete_status(&request.status_id)
        .await
        .map_err(|error| error.to_string())?;

    sqlx::query("DELETE FROM notifications WHERE status_id = ? AND server_domain = ?")
        .bind(&request.status_id)
        .bind(&request.server_domain)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM timeline_entries WHERE status_id = ? AND server_domain = ?")
        .bind(&request.status_id)
        .bind(&request.server_domain)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;
    sqlx::query("DELETE FROM statuses WHERE id = ? AND server_domain = ?")
        .bind(&request.status_id)
        .bind(&request.server_domain)
        .execute(state.database.writer())
        .await
        .map_err(|error| error.to_string())?;

    Ok(())
}

#[tauri::command]
async fn open_status_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Unsupported URL scheme".to_string());
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_media(request: DownloadMediaRequest) -> Result<(), String> {
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

    let response = reqwest::Client::new()
        .get(parsed)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn open_log_file() -> Result<(), String> {
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
        summaries.push(AccountSummary {
            rate_limit: rate_limits.get(&row.acct).cloned().flatten(),
            character_limit,
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
) -> Result<ServerMetadataSnapshot, String> {
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
) -> Result<ServerMetadataSnapshot, MastodonError> {
    match client {
        ApiClient::Mastodon(client) => {
            let instance: Instance = match client.get("/api/v2/instance").await {
                Ok(instance) => instance,
                Err(MastodonError::Api { status: 404, .. }) => {
                    client.get("/api/v1/instance").await?
                }
                Err(error) => return Err(error),
            };
            let streaming_url = instance
                .streaming_url()
                .unwrap_or(client.streaming_url.as_str())
                .to_string();
            let max_characters = normalize_character_limit(
                instance.max_characters(),
                MASTODON_DEFAULT_CHARACTER_LIMIT,
            );
            let version = Some(instance.version.clone());
            let instance_json = serde_json::to_string(&instance).ok();
            Ok(ServerMetadataSnapshot {
                streaming_url,
                version,
                max_characters,
                instance_json,
                server_kind: stored_kind,
            })
        }
        ApiClient::Misskey(client) => {
            let meta = client.get_meta().await?;
            let max_characters = normalize_character_limit(
                meta.max_note_text_length
                    .unwrap_or(MISSKEY_DEFAULT_CHARACTER_LIMIT as i64),
                MISSKEY_DEFAULT_CHARACTER_LIMIT,
            );
            let version = Some(meta.version.clone());
            let instance_json = serde_json::to_string(&meta).ok();
            Ok(ServerMetadataSnapshot {
                streaming_url: client.streaming_url.clone(),
                version,
                max_characters,
                instance_json,
                server_kind: ServerKind::Misskey,
            })
        }
        ApiClient::Bluesky(client) => {
            let instance_json = serde_json::to_string(&serde_json::json!({
                "post": {
                    "maxGraphemes": BLUESKY_CHARACTER_LIMIT,
                    "maxBytes": 3000
                }
            }))
            .ok();
            Ok(ServerMetadataSnapshot {
                streaming_url: client.streaming_url.clone(),
                version: None,
                max_characters: BLUESKY_CHARACTER_LIMIT,
                instance_json,
                server_kind: ServerKind::Bluesky,
            })
        }
    }
}

fn normalize_character_limit(value: i64, fallback: i32) -> i32 {
    if value <= 0 {
        return fallback;
    }
    i32::try_from(value).unwrap_or(fallback)
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
        account_acct: Some(config.account_acct),
        display_filter,
    })
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
    T: serde::de::DeserializeOwned + Default,
{
    match settings::get_setting(state.database.reader(), key)
        .await
        .map_err(|error| error.to_string())?
    {
        Some(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
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

async fn active_client(state: &RuntimeState) -> Result<(ApiClient, String), String> {
    let sessions = state.sessions.read().await;
    let session = sessions
        .active_session()
        .ok_or_else(|| "No active account is signed in".to_string())?;
    Ok((session.client.clone(), session.acct.clone()))
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

async fn session_for_domain(state: &RuntimeState, server_domain: &str) -> Option<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions
        .sessions()
        .values()
        .find(|session| session.client.domain() == server_domain || session.domain == server_domain)
        .cloned()
}

async fn session_for_status_owner(
    state: &RuntimeState,
    server_domain: &str,
    account_id: &str,
) -> Option<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions
        .sessions()
        .values()
        .find(|session| {
            (session.client.domain() == server_domain || session.domain == server_domain)
                && session.account_info.id == account_id
        })
        .cloned()
}

async fn signed_in_sessions(state: &RuntimeState) -> Vec<AccountSession> {
    let sessions = state.sessions.read().await;
    sessions.sessions().values().cloned().collect()
}

fn schedule_startup_sync(state: &RuntimeState) {
    let database = state.database.clone();
    let emit_queue = state.emit_queue.clone();
    let sessions = tauri::async_runtime::block_on(signed_in_sessions(state));

    tauri::async_runtime::spawn(async move {
        sync_startup_accounts(emit_queue, database, sessions).await;
    });
}

async fn sync_startup_accounts(
    emit_queue: QueuedEmitter,
    database: Arc<Database>,
    sessions: Vec<AccountSession>,
) {
    if sessions.is_empty() {
        return;
    }

    tracing::info!(
        "Startup timeline sync started for {} accounts",
        sessions.len()
    );
    for session in sessions {
        if let Err(error) = sync_startup_account(&emit_queue, &database, &session).await {
            tracing::warn!("Startup sync failed for {}: {}", session.acct, error);
        }
    }
    emit_startup_sync_event(
        &emit_queue,
        StartupSyncEvent {
            kind: "complete".to_string(),
            message: "Bookmarks synced".to_string(),
            acct: None,
            page: None,
            total: None,
        },
    )
    .await;
    tracing::info!("Startup timeline sync finished");
}

async fn sync_startup_account(
    emit_queue: &QueuedEmitter,
    database: &Database,
    session: &AccountSession,
) -> Result<(), String> {
    sync_startup_timeline(database, session, TimelineType::Home).await?;
    sync_startup_timeline(database, session, TimelineType::Public).await?;
    sync_startup_notifications(database, session).await?;
    sync_all_bookmarks(emit_queue, database, session).await?;
    sync_all_favourites(emit_queue, database, session).await?;
    Ok(())
}

async fn sync_startup_timeline(
    database: &Database,
    session: &AccountSession,
    timeline_type: TimelineType,
) -> Result<(), String> {
    let synced = timeline_service::sync_timeline(
        &session.client,
        database.writer(),
        database.reader(),
        &timeline_type,
        &session.acct,
        &TimelineParams {
            limit: Some(STARTUP_SYNC_LIMIT),
            ..Default::default()
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    tracing::info!(
        "Startup synced {} {} statuses for {}",
        synced.len(),
        timeline_type.as_str(),
        session.acct
    );
    Ok(())
}

async fn sync_startup_notifications(
    database: &Database,
    session: &AccountSession,
) -> Result<(), String> {
    let mut notifications = session
        .client
        .get_notifications(&NotificationParams {
            limit: Some(STARTUP_SYNC_LIMIT),
            ..Default::default()
        })
        .await
        .map_err(|error| error.to_string())?;

    for notification in &mut notifications {
        if let Some(status) = notification.status.as_mut() {
            timeline_service::hydrate_missing_quotes(&session.client, std::slice::from_mut(status))
                .await;
        }
        save_notification_to_db(
            database,
            notification,
            session.client.domain(),
            &session.acct,
        )
        .await?;
    }
    tracing::info!(
        "Startup synced {} notifications for {}",
        notifications.len(),
        session.acct
    );
    Ok(())
}

async fn sync_all_bookmarks(
    emit_queue: &QueuedEmitter,
    database: &Database,
    session: &AccountSession,
) -> Result<(), String> {
    let mut max_id = None;
    let mut seen_pages = HashSet::new();
    let mut page = 0u32;
    let mut total = 0usize;

    loop {
        page += 1;
        let mut response = session
            .client
            .get_bookmarks(&TimelineParams {
                max_id: max_id.clone(),
                limit: Some(STARTUP_SYNC_LIMIT),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        timeline_service::hydrate_missing_quotes(&session.client, &mut response.data).await;

        let count = response.data.len();
        for mut status in response.data {
            status.bookmarked = Some(true);
            let position_at = status.created_at.to_rfc3339();
            timeline_service::save_status_to_db_with_retry(
                database.writer(),
                &status,
                session.client.domain(),
            )
            .await
            .map_err(|error| error.to_string())?;
            timeline_service::insert_timeline_entry_with_retry(
                database.writer(),
                "bookmarks",
                session.client.domain(),
                &status.id,
                &session.acct,
                &position_at,
            )
            .await
            .map_err(|error| error.to_string())?;
            total += 1;
        }

        emit_startup_sync_event(
            emit_queue,
            StartupSyncEvent {
                kind: "bookmarkProgress".to_string(),
                message: format!(
                    "Syncing bookmarks: {} page {} ({})",
                    session.acct, page, total
                ),
                acct: Some(session.acct.clone()),
                page: Some(page),
                total: Some(total),
            },
        )
        .await;

        let next_max_id = response.next_max_id.filter(|id| !id.is_empty());
        match next_max_id {
            Some(next) if count > 0 && seen_pages.insert(next.clone()) => {
                max_id = Some(next);
            }
            _ => break,
        }
    }

    tracing::info!("Startup synced {} bookmarks for {}", total, session.acct);
    Ok(())
}

async fn sync_all_favourites(
    emit_queue: &QueuedEmitter,
    database: &Database,
    session: &AccountSession,
) -> Result<(), String> {
    if matches!(session.client.kind(), ServerKind::Bluesky) {
        tracing::info!(
            "Skipping favorites startup sync for unsupported Bluesky account {}",
            session.acct
        );
        return Ok(());
    }

    let mut max_id = None;
    let mut seen_pages = HashSet::new();
    let mut page = 0u32;
    let mut total = 0usize;

    loop {
        page += 1;
        let mut response = session
            .client
            .get_favourites(&TimelineParams {
                max_id: max_id.clone(),
                limit: Some(STARTUP_SYNC_LIMIT),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        timeline_service::hydrate_missing_quotes(&session.client, &mut response.data).await;

        let count = response.data.len();
        for status in response.data {
            let mut favourite_status = status
                .reblog
                .as_deref()
                .cloned()
                .unwrap_or(status);
            favourite_status.favourited = Some(true);
            let position_at = favourite_status.created_at.to_rfc3339();
            timeline_service::save_status_to_db_with_retry(
                database.writer(),
                &favourite_status,
                session.client.domain(),
            )
            .await
            .map_err(|error| error.to_string())?;
            timeline_service::insert_timeline_entry_with_retry(
                database.writer(),
                "favourites",
                session.client.domain(),
                &favourite_status.id,
                &session.acct,
                &position_at,
            )
            .await
            .map_err(|error| error.to_string())?;
            total += 1;
        }

        emit_startup_sync_event(
            emit_queue,
            StartupSyncEvent {
                kind: "favouriteProgress".to_string(),
                message: format!(
                    "Syncing favorites: {} page {} ({})",
                    session.acct, page, total
                ),
                acct: Some(session.acct.clone()),
                page: Some(page),
                total: Some(total),
            },
        )
        .await;

        let next_max_id = response.next_max_id.filter(|id| !id.is_empty());
        match next_max_id {
            Some(next) if count > 0 && seen_pages.insert(next.clone()) => {
                max_id = Some(next);
            }
            _ => break,
        }
    }

    tracing::info!("Startup synced {} favorites for {}", total, session.acct);
    Ok(())
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
        timeline_service::save_status_to_db_with_retry(database.writer(), status, server_domain)
            .await
            .map_err(|error| error.to_string())?;
    }

    sqlx::query(
        "INSERT INTO notifications (id, server_domain, account_acct, notification_type, created_at, account_id, status_id, read_at, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, ?)
         ON CONFLICT(id, server_domain) DO UPDATE SET
           account_acct = excluded.account_acct,
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
        let stream_types = stream_types_for_columns(&columns, Some(&session.acct));
        if stream_types.is_empty() {
            continue;
        }
        let server_kind = session.client.kind();
        let bluesky_poll_interval =
            Duration::from_secs(bluesky_fetch.interval_for_acct(&session.acct));

        let (tx, rx) = futures::channel::mpsc::unbounded::<TimelineEvent>();
        let bridge_handle = tokio::spawn(forward_stream_events(
            state.emit_queue.clone(),
            state.database.clone(),
            rx,
        ));
        handles.push(bridge_handle.abort_handle());
        handles.extend(streaming_service::start_streaming(
            session.client.clone(),
            session.client.streaming_url().to_string(),
            session.client.access_token(),
            stream_types.clone(),
            session.domain.clone(),
            server_kind,
            session.acct.clone(),
            state.database.clone(),
            vec![tx],
            bluesky_poll_interval,
        ));
    }
}

fn stream_types_for_columns(
    columns: &[ColumnSummary],
    account_acct: Option<&str>,
) -> Vec<crate::mastodon::types::streaming::StreamType> {
    use crate::mastodon::types::streaming::StreamType;

    let mut stream_types = Vec::new();
    push_stream_type(&mut stream_types, StreamType::User);
    push_stream_type(&mut stream_types, StreamType::Public);
    push_stream_type(&mut stream_types, StreamType::UserNotification);

    for column in columns {
        if !column_stream_matches_account(column, account_acct) {
            continue;
        }
        match column.column_type.as_str() {
            "local" => push_stream_type(&mut stream_types, StreamType::PublicLocal),
            "list" => {
                if let Some(id) = column.column_param.as_ref().filter(|id| !id.is_empty()) {
                    push_stream_type(&mut stream_types, StreamType::List(id.clone()));
                }
            }
            "hashtag" => {
                if let Some(tag) = column.column_param.as_ref().filter(|tag| !tag.is_empty()) {
                    push_stream_type(&mut stream_types, StreamType::Hashtag(tag.clone()));
                }
            }
            _ => {}
        }
    }

    stream_types
}

fn column_stream_matches_account(column: &ColumnSummary, account_acct: Option<&str>) -> bool {
    if !matches!(column.column_type.as_str(), "local" | "list" | "hashtag") {
        return true;
    }
    let Some(requested) = column
        .account_acct
        .as_deref()
        .filter(|acct| !acct.is_empty())
    else {
        return true;
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
    mut rx: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
) {
    while let Some(event) = rx.next().await {
        let payload = match event {
            TimelineEvent::NewStatus(status, stream_type, source_acct, server_domain) => {
                let status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                TimelineStreamPayload {
                    kind: "newStatus".to_string(),
                    stream_type: stream_type_key(&stream_type),
                    source_acct,
                    server_domain: server_domain.clone(),
                    status: Some(status),
                    status_id: None,
                }
            }
            TimelineEvent::StatusUpdate(status, source_acct, server_domain) => {
                let status = with_source_acct(
                    status_to_view(&status, &server_domain, None),
                    Some(source_acct.clone()),
                );
                TimelineStreamPayload {
                    kind: "statusUpdate".to_string(),
                    stream_type: "status.update".to_string(),
                    source_acct,
                    server_domain: server_domain.clone(),
                    status: Some(status),
                    status_id: None,
                }
            }
            TimelineEvent::DeleteStatus(status_id, source_acct, server_domain) => {
                TimelineStreamPayload {
                    kind: "deleteStatus".to_string(),
                    stream_type: "delete".to_string(),
                    source_acct,
                    server_domain,
                    status: None,
                    status_id: Some(status_id),
                }
            }
            TimelineEvent::NewNotification(
                notification,
                stream_type,
                source_acct,
                server_domain,
            ) => {
                if !matches!(
                    stream_type,
                    crate::mastodon::types::streaming::StreamType::UserNotification
                ) {
                    continue;
                }
                if let Err(error) =
                    save_notification_to_db(&database, &notification, &server_domain, &source_acct)
                        .await
                {
                    tracing::warn!("Failed to save streaming notification to DB: {}", error);
                }
                if should_send_desktop_notification(&database, &notification, &server_domain).await
                {
                    streaming_service::send_desktop_notification(&notification);
                }
                let status =
                    notification_to_view(&notification, &server_domain, Some(&source_acct));
                TimelineStreamPayload {
                    kind: "newNotification".to_string(),
                    stream_type: stream_type_key(&stream_type),
                    source_acct,
                    server_domain: server_domain.clone(),
                    status: Some(status),
                    status_id: None,
                }
            }
        };

        if !emit_queue.try_emit(TIMELINE_STREAM_EVENT, payload, "timeline stream event") {
            log_dropped_stream_emit();
        }
    }
}

fn log_dropped_stream_emit() {
    let dropped = DROPPED_STREAM_EMITS.fetch_add(1, Ordering::Relaxed) + 1;
    if dropped == 1 || dropped % 100 == 0 {
        tracing::warn!(
            dropped,
            "Dropped timeline stream event because the Tauri emit queue is full"
        );
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

    match settings::get_setting(database.reader(), "notification_suppression").await {
        Ok(Some(json)) => {
            let suppression =
                serde_json::from_str::<NotificationSuppressionList>(&json).unwrap_or_default();
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
        Ok(None) => true,
        Err(error) => {
            tracing::warn!("Failed to read legacy notification suppression: {}", error);
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

fn is_aggregate_timeline(timeline_type: &TimelineType) -> bool {
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

    let session = session_for_timeline_request(state, request.account_acct.as_deref()).await?;
    let client = session.client;
    let active_acct = session.acct;
    let display_filter = request.display_filter.filter(|filter| filter.applies());
    let page_limit = limit.min(80).max(1);
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
            &active_acct,
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
                Some(active_acct.clone()),
            );
            timeline_status_matches_display_filter(&view, display_filter).then_some(view)
        }));
        tracing::info!(
            timeline = timeline_type.as_str(),
            source_acct = active_acct.as_str(),
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
    if sessions.is_empty() {
        return Err("No account is signed in".to_string());
    }

    for session in sessions {
        timeline_service::sync_timeline(
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
        .map_err(|error| error.to_string())?;
    }

    let statuses = query_aggregate_timeline_statuses(
        state.database.reader(),
        &timeline_type.as_str(),
        limit as i64,
        0,
        display_filter.filter(|filter| filter.applies()),
    )
    .await?;
    db_status_refs_to_views(state.database.reader(), statuses).await
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
    for session in sessions {
        let mut notifications = session
            .client
            .get_notifications(&NotificationParams {
                limit: Some(limit),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())?;

        for notification in &mut notifications {
            if let Some(status) = notification.status.as_mut() {
                timeline_service::resolve_pending_quotes_with_backoff(
                    &session.client,
                    std::slice::from_mut(status),
                )
                .await;
            }
            save_notification_to_db(
                &state.database,
                notification,
                session.client.domain(),
                &session.acct,
            )
            .await?;
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
            .notification_label
            .as_deref()
            .is_some_and(|label| label.contains("boosted"));
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

    let statuses = match tl_type {
        TimelineType::CustomSql(sql) => {
            query_custom_statuses(state.database.reader(), &sql, limit, offset).await?
        }
        TimelineType::YukariQuery(query) => {
            query_yq_statuses(
                state.database.reader(),
                &query,
                limit,
                offset,
                request.since_status_id.as_deref(),
                request.since_server_domain.as_deref(),
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
            )
            .await?
        }
        TimelineType::Bookmarks => {
            query_bookmarked_statuses(state.database.reader(), limit, offset).await?
        }
        TimelineType::Favourites => {
            query_favourited_statuses(state.database.reader(), limit, offset).await?
        }
        TimelineType::UserBookmarks {
            server_domain,
            account_id,
        } => {
            query_user_bookmarked_statuses(
                state.database.reader(),
                &server_domain,
                &account_id,
                limit,
                offset,
            )
            .await?
        }
        TimelineType::Notification => {
            return query_notification_statuses(state.database.reader(), limit, offset).await;
        }
        TimelineType::Home | TimelineType::Public => {
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
            let active_acct =
                match session_for_timeline_request(state, request.account_acct.as_deref()).await {
                    Ok(session) => session.acct,
                    Err(_) => String::new(),
                };
            let statuses = query_timeline_statuses(
                state.database.reader(),
                &tl_type.as_str(),
                &active_acct,
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
    let filter_sql = timeline_display_filter_sql("s", display_filter);
    let sql = format!(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT server_domain, status_id, source_acct, latest_position FROM (
             SELECT
               te.server_domain,
               te.status_id,
               te.account_acct AS source_acct,
               te.position_at AS latest_position,
               ROW_NUMBER() OVER (
                 PARTITION BY COALESCE(NULLIF(s.uri, ''), te.server_domain || ':' || te.status_id)
                 ORDER BY te.position_at DESC, te.server_domain DESC, te.status_id DESC, te.account_acct DESC
               ) AS uri_rank
             FROM timeline_entries te
             JOIN statuses s ON s.id = te.status_id AND s.server_domain = te.server_domain
             WHERE te.timeline_type = ?
             {}
           ) ranked_by_uri
           WHERE uri_rank = 1
           ORDER BY latest_position DESC, server_domain DESC, status_id DESC
           LIMIT ? OFFSET ?
         ) ranked
         ORDER BY ranked.latest_position DESC, ranked.server_domain DESC, ranked.status_id DESC",
        filter_sql
    );
    sqlx::query_as::<_, TimelineStatusRef>(&sql)
        .bind(timeline_type)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
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
    let trimmed = sql.trim();
    if !trimmed.to_uppercase().starts_with("SELECT") {
        return Err("Custom timeline SQL must start with SELECT".to_string());
    }
    let custom_sql = trimmed.trim_end_matches(';').trim();
    if custom_sql_has_top_level_limit(custom_sql) && offset > 0 {
        return Ok(Vec::new());
    }
    let query = if custom_sql_has_top_level_limit(custom_sql) {
        custom_sql.to_string()
    } else {
        format!(
            "SELECT * FROM ({}) custom_timeline_page LIMIT {} OFFSET {}",
            custom_sql, limit, offset
        )
    };
    sqlx::query_as::<_, DbStatus>(&query)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

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

fn is_sql_identifier_byte(byte: Option<u8>) -> bool {
    matches!(
        byte,
        Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
    )
}

async fn query_yq_statuses(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    offset: i64,
    since_status_id: Option<&str>,
    since_server_domain: Option<&str>,
) -> Result<Vec<DbStatus>, String> {
    let started_at = Instant::now();
    let expression = crate::services::yq_filter::parse_expression(query)?;
    let stop_key = since_status_id.zip(since_server_domain);
    let mut matched_before_page = 0;
    let mut results = Vec::with_capacity(limit as usize);
    let mut account_cache: HashMap<(String, String), Option<DbAccount>> = HashMap::new();
    let mut cursor: Option<(String, String)> = None;
    let mut scanned_count = 0usize;
    let mut stopped_at_since = false;

    while results.len() < limit as usize {
        let rows = if let Some((created_at, id)) = cursor.as_ref() {
            sqlx::query_as::<_, DbStatus>(
                "SELECT * FROM statuses
                 WHERE created_at < ? OR (created_at = ? AND id < ?)
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?",
            )
            .bind(created_at)
            .bind(created_at)
            .bind(id)
            .bind(YQ_FILTER_PAGE_SIZE)
            .fetch_all(pool)
            .await
        } else {
            sqlx::query_as::<_, DbStatus>(
                "SELECT * FROM statuses
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?",
            )
            .bind(YQ_FILTER_PAGE_SIZE)
            .fetch_all(pool)
            .await
        }
        .map_err(|error| error.to_string())?;

        if rows.is_empty() {
            break;
        }
        let reached_end = rows.len() < YQ_FILTER_PAGE_SIZE as usize;
        if let Some(last) = rows.last() {
            cursor = Some((last.created_at.clone(), last.id.clone()));
        }
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

        for status in rows {
            if stop_key.is_some_and(|(id, server_domain)| {
                status.id == id && status.server_domain == server_domain
            }) {
                stopped_at_since = true;
                break;
            }
            scanned_count += 1;
            let account_key = (status.account_id.clone(), status.server_domain.clone());
            let account = account_cache
                .get(&account_key)
                .and_then(|account| account.as_ref());

            if !crate::services::yq_filter::matches_expression(&expression, &status, account) {
                continue;
            }

            if matched_before_page < offset {
                matched_before_page += 1;
                continue;
            }

            results.push(status);
            if results.len() >= limit as usize {
                break;
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
        since_status_id = ?since_status_id,
        since_server_domain = ?since_server_domain,
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
) -> Result<Vec<DbStatus>, String> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = format!("%{}%", normalized_query);

    let filter_sql = timeline_display_filter_sql("s", display_filter);
    let sql = format!(
        "SELECT DISTINCT s.*
         FROM statuses s
         LEFT JOIN accounts a ON a.id = s.account_id AND a.server_domain = s.server_domain
         WHERE (
            lower(s.content) LIKE ?
            OR lower(s.spoiler_text) LIKE ?
            OR lower(s.uri) LIKE ?
            OR lower(coalesce(s.url, '')) LIKE ?
            OR lower(coalesce(s.tags_json, '')) LIKE ?
            OR lower(a.acct) LIKE ?
            OR lower(a.display_name) LIKE ?
         )
         {}
         ORDER BY s.created_at DESC
         LIMIT ? OFFSET ?",
        filter_sql
    );
    sqlx::query_as::<_, DbStatus>(&sql)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(&pattern)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())
}

async fn query_bookmarked_statuses(
    pool: &sqlx::SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<DbStatus>, String> {
    sqlx::query_as::<_, DbStatus>(
        "SELECT * FROM statuses WHERE bookmarked = 1 ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())
}

async fn query_favourited_statuses(
    pool: &sqlx::SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<DbStatus>, String> {
    sqlx::query_as::<_, DbStatus>(
        "SELECT s.* FROM (
           SELECT
             id,
             server_domain,
             created_at,
             ROW_NUMBER() OVER (
               PARTITION BY COALESCE(NULLIF(uri, ''), server_domain || ':' || id)
               ORDER BY fetched_at DESC, created_at DESC, server_domain DESC, id DESC
             ) AS uri_rank
           FROM statuses
           WHERE favourited = 1 AND reblog_of_id IS NULL
         ) ranked
         JOIN statuses s ON s.id = ranked.id AND s.server_domain = ranked.server_domain
         WHERE ranked.uri_rank = 1
         ORDER BY ranked.created_at DESC, s.server_domain DESC, s.id DESC
         LIMIT ? OFFSET ?",
    )
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
    limit: i64,
    offset: i64,
) -> Result<Vec<DbStatus>, String> {
    sqlx::query_as::<_, DbStatus>(
        "SELECT * FROM statuses
         WHERE bookmarked = 1 AND server_domain = ? AND account_id = ?
         ORDER BY created_at DESC
         LIMIT ? OFFSET ?",
    )
    .bind(server_domain)
    .bind(account_id)
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
        .bind(&request.status_id)
        .bind(&request.server_domain)
        .execute(pool)
        .await?;
    Ok(())
}

async fn query_notification_statuses(
    pool: &sqlx::SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatus>, String> {
    let rows = sqlx::query_as::<_, DbNotification>(
        "SELECT * FROM notifications ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(|error| error.to_string())?;

    let mut views = Vec::with_capacity(rows.len());
    for notification in rows {
        let account =
            accounts::get_account(pool, &notification.account_id, &notification.server_domain)
                .await
                .map_err(|error| error.to_string())?;
        let status = match notification.status_id.as_deref() {
            Some(status_id) => sqlx::query_as::<_, DbStatus>(
                "SELECT * FROM statuses WHERE id = ? AND server_domain = ?",
            )
            .bind(status_id)
            .bind(&notification.server_domain)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?,
            None => None,
        };

        let status_account = match status.as_ref() {
            Some(status) => accounts::get_account(pool, &status.account_id, &status.server_domain)
                .await
                .map_err(|error| error.to_string())?,
            None => None,
        };

        views.push(
            notification_db_to_view_resolving_quote(
                pool,
                notification,
                account,
                status,
                status_account,
            )
            .await?,
        );
    }

    Ok(views)
}

async fn notification_db_to_view_resolving_quote(
    pool: &sqlx::SqlitePool,
    notification: DbNotification,
    actor_account: Option<DbAccount>,
    status: Option<DbStatus>,
    status_account: Option<DbAccount>,
) -> Result<TimelineStatus, String> {
    let Some(status) = status else {
        return Ok(notification_db_to_view(
            notification,
            actor_account,
            None,
            status_account,
        ));
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
    let mut view = db_status_to_view_with_cached_quote(pool, status, status_account, 2).await?;
    view.id = notification.id;
    view.created_at = notification.created_at;
    view.source_acct = source_acct;
    view.notification_id = Some(notification_id);
    view.notification_label = notification_label;
    view.notification_avatar = notification_avatar;
    view.notification_account_id = Some(actor_account_id);
    view.notification_acct = Some(actor_acct);
    view.notification_display_name = Some(actor_label);
    view.notification_account_emojis = notification_account_emojis;
    Ok(view)
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
    let Some(seed) = query_cached_status(pool, status_id, server_domain).await? else {
        return Ok(Vec::new());
    };

    let mut statuses = HashMap::new();
    let mut current = seed.clone();
    let mut root_id = seed.id.clone();
    statuses.insert(seed.id.clone(), seed);

    while statuses.len() < limit {
        let Some(parent_id) = current.in_reply_to_id.clone() else {
            break;
        };
        let Some(parent) = query_cached_status(pool, &parent_id, server_domain).await? else {
            break;
        };
        if statuses.contains_key(&parent.id) {
            root_id = parent.id;
            break;
        }
        root_id = parent.id.clone();
        current = parent.clone();
        statuses.insert(parent.id.clone(), parent);
    }

    let mut queue = VecDeque::from([root_id.clone()]);
    while let Some(parent_id) = queue.pop_front() {
        if statuses.len() >= limit {
            break;
        }

        let mut children = query_cached_reply_children(pool, &parent_id, server_domain).await?;
        children.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        for child in children {
            if statuses.len() >= limit {
                break;
            }
            if statuses.contains_key(&child.id) {
                continue;
            }
            queue.push_back(child.id.clone());
            statuses.insert(child.id.clone(), child);
        }
    }

    Ok(order_thread_statuses(statuses, &root_id))
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

async fn query_cached_reply_children(
    pool: &sqlx::SqlitePool,
    parent_id: &str,
    server_domain: &str,
) -> Result<Vec<DbStatus>, String> {
    sqlx::query_as::<_, DbStatus>(
        "SELECT * FROM statuses
         WHERE in_reply_to_id = ? AND server_domain = ?
         ORDER BY created_at ASC, id ASC",
    )
    .bind(parent_id)
    .bind(server_domain)
    .fetch_all(pool)
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
    Ok(views)
}

fn with_source_acct(mut status: TimelineStatus, source_acct: Option<String>) -> TimelineStatus {
    status.source_acct = source_acct;
    status
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
        view.created_at = status.created_at;
        view.notification_label = Some(format!("{} boosted", booster));
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

async fn db_status_to_view_with_cached_quote(
    pool: &sqlx::SqlitePool,
    status: DbStatus,
    account: Option<DbAccount>,
    quote_depth: usize,
) -> Result<TimelineStatus, String> {
    let quote_id = status.quote_id.clone();
    let server_domain = status.server_domain.clone();
    let mut view = db_status_to_view(status, account);
    if quote_depth == 0 {
        return Ok(view);
    }

    let Some(quote_id) = quote_id else {
        return Ok(view);
    };

    let quote =
        sqlx::query_as::<_, DbStatus>("SELECT * FROM statuses WHERE id = ? AND server_domain = ?")
            .bind(&quote_id)
            .bind(&server_domain)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
    let Some(quote) = quote else {
        return Ok(view);
    };

    let quote_account = accounts::get_account(pool, &quote.account_id, &quote.server_domain)
        .await
        .map_err(|error| error.to_string())?;
    view.quote = Some(Box::new(db_status_to_view(quote, quote_account)));
    Ok(view)
}

fn db_status_to_view(status: DbStatus, account: Option<DbAccount>) -> TimelineStatus {
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
        source_acct: None,
        account_id: status.account_id,
        server_domain: status.server_domain,
        uri: status.uri,
        url: status.url,
        display_name,
        acct,
        avatar,
        created_at: status.created_at,
        in_reply_to_id,
        in_reply_to_account_id,
        content: status.content,
        spoiler_text: status.spoiler_text,
        language: status.language,
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
        notification_label: None,
        notification_avatar: None,
        notification_account_id: None,
        notification_acct: None,
        notification_display_name: None,
        notification_account_emojis: Vec::new(),
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
        view.id = status.id.clone();
        view.uri = status.uri.clone();
        view.created_at = status.created_at.to_rfc3339();
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
            Box::new(status_to_view_base_with_quote_depth(
                quote,
                server_domain,
                None,
                None,
                quote_depth - 1,
            ))
        })
    };

    TimelineStatus {
        id: status.id.clone(),
        original_status_id: status.id.clone(),
        source_acct: None,
        account_id: status.account.id.clone(),
        server_domain: server_domain.to_string(),
        uri: status.uri.clone(),
        url: status.url.clone(),
        display_name: status.account.display_name.clone(),
        acct: format!("@{}", status.account.acct),
        avatar: status.account.avatar.clone(),
        created_at: status.created_at.to_rfc3339(),
        in_reply_to_id: status.in_reply_to_id.clone(),
        in_reply_to_account_id: status.in_reply_to_account_id.clone(),
        content: status.content.clone(),
        spoiler_text: status.spoiler_text.clone(),
        language: status.language.clone(),
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
            TimelineStatus {
                id: notification.id,
                original_status_id: notification.status_id.unwrap_or_default(),
                source_acct,
                account_id: actor_account_id.clone(),
                server_domain: notification.server_domain,
                uri: String::new(),
                url: None,
                display_name: actor_display_name.clone(),
                acct: actor_acct.clone(),
                avatar: actor_avatar,
                created_at: notification.created_at,
                in_reply_to_id: None,
                in_reply_to_account_id: None,
                content: String::new(),
                spoiler_text: String::new(),
                language: None,
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
    let notification_avatar = Some(notification.account.avatar.clone());
    let notification_account_id = Some(notification.account.id.clone());
    let notification_acct = Some(format!("@{}", notification.account.acct));
    let notification_display_name = Some(notification.account.display_name.clone());
    let notification_account_emojis = custom_emojis_to_views(&notification.account.emojis);

    let Some(status) = notification.status.as_ref() else {
        return TimelineStatus {
            id: notification.id.clone(),
            original_status_id: notification.id.clone(),
            source_acct: source_acct.map(str::to_string),
            account_id: notification.account.id.clone(),
            server_domain: server_domain.to_string(),
            uri: String::new(),
            url: None,
            display_name: notification.account.display_name.clone(),
            acct: format!("@{}", notification.account.acct),
            avatar: notification.account.avatar.clone(),
            created_at: notification.created_at.to_rfc3339(),
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            content: String::new(),
            spoiler_text: String::new(),
            language: None,
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

fn sanitize_upload_filename(filename: &str, mime_type: Option<&str>) -> String {
    let sanitized = filename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let fallback = match mime_type {
        Some("image/jpeg") => "upload.jpg",
        Some("image/png") => "upload.png",
        Some("image/gif") => "upload.gif",
        Some("image/webp") => "upload.webp",
        Some("video/mp4") => "upload.mp4",
        Some("video/webm") => "upload.webm",
        Some("audio/mpeg") => "upload.mp3",
        Some("audio/ogg") => "upload.ogg",
        _ => "upload.bin",
    };
    let name = sanitized.trim_matches('_');
    if name.is_empty() {
        fallback.to_string()
    } else {
        name.to_string()
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
            read_at: None,
            fetched_at: "2026-05-20T00:00:01Z".to_string(),
        }
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
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/005_create_timeline_entries.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
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
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/005_create_timeline_entries.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

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
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

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
        }

        let statuses = query_favourited_statuses(&pool, 10, 0).await.unwrap();

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "remote-copy");
        assert_eq!(statuses[0].server_domain, "remote.example");
    }

    #[tokio::test]
    async fn favourited_statuses_query_excludes_boost_wrappers() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

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

        let statuses = query_favourited_statuses(&pool, 10, 0).await.unwrap();

        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].id, "original");
        assert!(statuses[0].reblog_of_id.is_none());
    }

    #[tokio::test]
    async fn aggregate_timeline_query_keeps_boost_and_original_as_separate_posts() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/005_create_timeline_entries.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

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
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/005_create_timeline_entries.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
        ] {
            sqlx::query(migration).execute(&pool).await.unwrap();
        }

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

    #[tokio::test]
    async fn custom_sql_limit_takes_precedence_over_column_limit() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
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
        let next_page = query_custom_statuses(
            &pool,
            "SELECT * FROM statuses ORDER BY created_at DESC LIMIT 2",
            1,
            2,
        )
        .await
        .unwrap();

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].id, "status-0");
        assert_eq!(statuses[1].id, "status-1");
        assert!(next_page.is_empty());
    }

    #[tokio::test]
    async fn custom_sql_without_limit_uses_column_limit_and_offset() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        for migration in [
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
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
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
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
            include_str!("../migrations/001_create_servers.sql"),
            include_str!("../migrations/002_create_accounts.sql"),
            include_str!("../migrations/003_create_statuses.sql"),
            include_str!("../migrations/012_add_status_quote_id.sql"),
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
            ("status-current", "0002", "<p>needle current</p>"),
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
            "where (regex text \"needle\")",
            10,
            0,
            Some("status-current"),
            Some("example.test"),
        )
        .await
        .unwrap();

        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "status-new-match");
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
}
