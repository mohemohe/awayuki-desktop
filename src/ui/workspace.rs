use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use std::path::PathBuf;

use gpui::{
    actions, deferred, div, hsla, img, px, rgb, rgba, App, AsyncApp, ClickEvent, ClipboardEntry,
    Context, Corner, Entity, EntityId, ExternalPaths, FocusHandle, Focusable, ImageFormat,
    KeyDownEvent, ObjectFit, PathPromptOptions, ScrollDelta, ScrollWheelEvent, SharedString,
    WeakEntity, Window,
};
use gpui::{prelude::*, rems};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{DockArea, DockItem, PanelView, TabPanel};
use gpui_component::input::{Input, InputEvent, InputState, Position};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_component::popover::Popover;
use gpui_component::select::{Select, SelectState};
use gpui_component::spinner::Spinner;
use gpui_component::Root;
use gpui_component::TitleBar;
use gpui_component::{Icon, IconName, Selectable, Sizable, Size};
use gpui_tokio_bridge::Tokio;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;
use crate::auth::session::{AccountSession, SessionManager};
use crate::constants::{APP_NAME, DB_FILENAME};
use crate::db::models::DbColumnConfig;
use crate::db::pool::Database;
use crate::mastodon::client::MastodonClient;
use crate::misskey::client::MisskeyClient;
use crate::mastodon::endpoints::statuses::{CreatePollParams, CreateStatusParams};
use crate::mastodon::types::account::CustomEmoji;
use crate::mastodon::types::streaming::StreamType;
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::TimelineType;
use crate::state::active_account::ActiveAccount;
use crate::state::app_state::AppState;
use crate::state::appearance::AppearanceSettings;
use crate::state::behavior::BehaviorSettings;
use crate::state::confirmation::ConfirmationSettings;
use crate::state::notifications::NotificationSuppressionList;
use crate::state::performance::PerformanceSettings;
use crate::state::preset_visibility::PresetVisibilitySettings;
use crate::ui::components::autocomplete_popup::AutocompletePopup;
use crate::ui::components::emoji_picker::{EmojiPicker, EmojiStore};
use crate::ui::components::status_item::{EditTarget, QuoteTarget, ReplyTarget};
use crate::ui::panels::account_panel::{AccountDetailRequest, AccountPanel};
use crate::ui::panels::status_detail_panel::{StatusDetailPanel, StatusDetailRequest};
use crate::ui::panels::timeline_panel::{
    BookmarkChanged, EditState, LightboxState, LightboxStatusContext, QuoteState, ReplyState,
    TimelinePanel,
};
use crate::ui::views::login_view::{LoginEvent, LoginView};
use crate::ui::views::settings_view::{AccountInfo, ColumnEntry, SettingsEvent, SettingsView};

actions!(workspace, [FocusCompose, SubmitPost]);

/// Global state for requesting a panel close (bypasses DockArea lock)
#[derive(Default, Clone)]
pub struct ClosePanelRequest {
    pub entity_id: Option<EntityId>,
}
impl gpui::Global for ClosePanelRequest {}

/// Global state for bookmark sync progress (shown in status bar)
#[derive(Default, Clone)]
pub struct BookmarkSyncState {
    pub syncing: bool,
    pub message: Option<String>,
}
impl gpui::Global for BookmarkSyncState {}

/// Global state for status bar statistics
#[derive(Default, Clone)]
pub struct StatusBarStats {
    pub status_count: i64,
    pub recent_count: i64,
}
impl gpui::Global for StatusBarStats {}

/// Global state for hamburger menu actions
#[derive(Default, Clone)]
struct MenuAction(Option<MenuActionKind>);

#[derive(Clone)]
enum MenuActionKind {
    OpenBookmarks,
    OpenSettings,
    SwitchAccount(String),
    AddAccount,
}
impl gpui::Global for MenuAction {}

/// Tracks dynamically added panels for force-close support
struct DynamicPanelEntry {
    tab_panel: Entity<TabPanel>,
    inner_panel: Arc<dyn PanelView>,
}

enum WorkspaceView {
    Loading(SharedString),
    Login(Entity<LoginView>),
    Main(Entity<DockArea>),
    Settings(Entity<SettingsView>),
}

const VISIBILITY_OPTIONS: &[&str] = &["Public", "Unlisted", "Private", "Direct"];

const POLL_DURATION_LABELS: &[&str] = &["5分", "30分", "1時間", "6時間", "12時間", "1日", "3日", "7日"];
const POLL_DURATION_SECONDS: &[i64] = &[300, 1800, 3600, 21600, 43200, 86400, 259200, 604800];

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "svg", "ico", "heic", "heif", "avif",
];

struct ComposeAttachment {
    media_id: String,
    filename: String,
    local_path: PathBuf,
    is_image: bool,
}

#[derive(Debug, Clone)]
struct DraggedAttachment {
    index: usize,
    name: SharedString,
}

impl Render for DraggedAttachment {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x313244))
            .rounded(px(4.0))
            .text_xs()
            .text_color(rgb(0xcdd6f4))
            .opacity(0.85)
            .child(self.name.clone())
    }
}

pub struct Workspace {
    view: WorkspaceView,
    session_manager: SessionManager,
    compose_input: Option<Entity<InputState>>,
    cw_enabled: bool,
    cw_input: Option<Entity<InputState>>,
    visibility_select: Option<Entity<SelectState<Vec<&'static str>>>>,
    posting: bool,
    attachments: Vec<ComposeAttachment>,
    uploading: bool,
    max_characters: usize,
    reply_target: Option<ReplyTarget>,
    edit_target: Option<EditTarget>,
    quote_target: Option<QuoteTarget>,
    pending_account_detail: Option<String>,
    pending_status_detail: Option<String>,
    drag_over: bool,
    focus_handle: FocusHandle,
    emoji_picker: Option<Entity<EmojiPicker>>,
    autocomplete_popup: Option<Entity<AutocompletePopup>>,
    dynamic_panels: HashMap<EntityId, DynamicPanelEntry>,
    pending_close_panel: Option<EntityId>,
    pending_bookmarks_panel: bool,
    pending_show_settings: bool,
    pending_switch_account: Option<String>,
    pending_add_account: bool,
    search_input: Option<Entity<InputState>>,
    poll_enabled: bool,
    poll_options: Vec<Entity<InputState>>,
    poll_multiple: bool,
    poll_duration_select: Option<Entity<SelectState<Vec<&'static str>>>>,
    started_at: std::time::Instant,
    stats_updater_started: bool,
    streaming_abort_handles: Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>,
    /// Ephemeral drag state for lightbox panning. `Some((start_mouse, start_pan_x, start_pan_y))`
    /// while the left mouse button is held after pressing on the lightbox overlay.
    lightbox_drag_start: Option<(gpui::Point<gpui::Pixels>, f32, f32)>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Initialize lightbox global state
        cx.set_global(LightboxState::default());
        cx.observe_global::<LightboxState>(|_this, cx| {
            cx.notify();
        })
        .detach();

        // Initialize reply global state
        cx.set_global(ReplyState::default());
        cx.observe_global_in::<ReplyState>(window, |this, window, cx| {
            let target = cx.global::<ReplyState>().target.clone();
            this.on_reply_target_changed(target, window, cx);
        })
        .detach();

        // Initialize edit global state
        cx.set_global(EditState::default());
        cx.observe_global_in::<EditState>(window, |this, window, cx| {
            let target = cx.global::<EditState>().target.clone();
            this.on_edit_target_changed(target, window, cx);
        })
        .detach();

        // Initialize quote global state
        cx.set_global(QuoteState::default());
        cx.observe_global_in::<QuoteState>(window, |this, window, cx| {
            let target = cx.global::<QuoteState>().target.clone();
            this.on_quote_target_changed(target, window, cx);
        })
        .detach();

        // Initialize account detail request global state
        cx.set_global(AccountDetailRequest::default());
        cx.observe_global::<AccountDetailRequest>(|this, cx| {
            if let Some(id) = cx.global::<AccountDetailRequest>().account_id.clone() {
                this.pending_account_detail = Some(id);
                cx.set_global(AccountDetailRequest::default());
                cx.notify();
            }
        })
        .detach();

        // Initialize status detail request global state
        cx.set_global(StatusDetailRequest::default());
        cx.observe_global::<StatusDetailRequest>(|this, cx| {
            if let Some(id) = cx.global::<StatusDetailRequest>().status_id.clone() {
                this.pending_status_detail = Some(id);
                cx.set_global(StatusDetailRequest::default());
                cx.notify();
            }
        })
        .detach();

        // Initialize close panel request global state
        cx.set_global(ClosePanelRequest::default());
        cx.observe_global::<ClosePanelRequest>(|this, cx| {
            if let Some(entity_id) = cx.global::<ClosePanelRequest>().entity_id {
                this.pending_close_panel = Some(entity_id);
                cx.set_global(ClosePanelRequest::default());
                cx.notify();
            }
        })
        .detach();

        // Initialize appearance settings global state
        cx.set_global(AppearanceSettings::default());
        cx.observe_global::<AppearanceSettings>(|_this, cx| {
            cx.notify();
        })
        .detach();

        // Initialize performance settings global state
        cx.set_global(PerformanceSettings::default());

        // Initialize notification suppression list (per-account desktop-notification mute)
        cx.set_global(NotificationSuppressionList::default());

        // Initialize bookmark changed notification state
        cx.set_global(BookmarkChanged::default());

        // Initialize bookmark sync state
        cx.set_global(BookmarkSyncState::default());
        cx.observe_global::<BookmarkSyncState>(|_this, cx| {
            cx.notify();
        })
        .detach();

        // Initialize status bar stats
        cx.set_global(StatusBarStats::default());
        cx.observe_global::<StatusBarStats>(|_this, cx| {
            cx.notify();
        })
        .detach();

        // Initialize menu action state
        cx.set_global(MenuAction::default());
        cx.observe_global::<MenuAction>(|this, cx| {
            if let Some(action) = cx.global::<MenuAction>().0.clone() {
                cx.set_global(MenuAction::default());
                match action {
                    MenuActionKind::OpenBookmarks => {
                        this.pending_bookmarks_panel = true;
                        cx.notify();
                    }
                    MenuActionKind::OpenSettings => {
                        this.pending_show_settings = true;
                        cx.notify();
                    }
                    MenuActionKind::SwitchAccount(acct) => {
                        this.pending_switch_account = Some(acct);
                        cx.notify();
                    }
                    MenuActionKind::AddAccount => {
                        this.pending_add_account = true;
                        cx.notify();
                    }
                }
            }
        })
        .detach();

        // Handle clipboard image paste
        cx.observe_keystrokes(|this, event, window, cx| {
            if let Some(action) = &event.action {
                tracing::debug!("observe_keystrokes action: {}", action.name());
                if action.name() == "input::Paste" {
                    this.handle_paste_image(window, cx);
                }
            }
        })
        .detach();

        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);

        let mut workspace = Self {
            view: WorkspaceView::Loading("Initializing...".into()),
            session_manager: SessionManager::new(),
            compose_input: None,
            cw_enabled: false,
            cw_input: None,
            visibility_select: None,
            posting: false,
            attachments: Vec::new(),
            uploading: false,
            max_characters: 500,
            reply_target: None,
            edit_target: None,
            quote_target: None,
            pending_account_detail: None,
            pending_status_detail: None,
            drag_over: false,
            focus_handle,
            emoji_picker: None,
            autocomplete_popup: None,
            dynamic_panels: HashMap::new(),
            pending_close_panel: None,
            pending_bookmarks_panel: false,
            pending_show_settings: false,
            pending_switch_account: None,
            pending_add_account: false,
            search_input: None,
            poll_enabled: false,
            poll_options: Vec::new(),
            poll_multiple: false,
            poll_duration_select: None,
            started_at: std::time::Instant::now(),
            stats_updater_started: false,
            streaming_abort_handles: Arc::new(std::sync::Mutex::new(Vec::new())),
            lightbox_drag_start: None,
        };
        workspace.init_database(window, cx);
        workspace
    }

    fn init_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let db_path = get_db_path();
        tracing::info!("Database path: {}", db_path);

        let task = Tokio::spawn(cx, async move {
            let database = Database::new(&db_path).await?;
            database.run_migrations().await?;
            Ok::<Database, sqlx::Error>(database)
        });

        cx.spawn_in(
            window,
            async |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| match task.await
            {
                Ok(Ok(database)) => {
                    tracing::info!("Database initialized successfully");
                    let _ = this.update_in(cx, |this, window, cx| {
                        cx.set_global(AppState {
                            database: Arc::new(database),
                        });
                        this.try_restore_session(window, cx);
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Database initialization failed: {}", e);
                    let _ = this.update_in(cx, |this, _window, cx| {
                        this.view = WorkspaceView::Loading(format!("DB error: {}", e).into());
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Tokio task failed: {}", e);
                    let _ = this.update_in(cx, |this, _window, cx| {
                        this.view = WorkspaceView::Loading(format!("Error: {}", e).into());
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn try_restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app_state = cx.try_global::<AppState>();
        let Some(db) = app_state.map(|s| s.database.clone()) else {
            self.show_login(window, cx);
            return;
        };

        self.view = WorkspaceView::Loading("Restoring session...".into());
        cx.notify();

        let task = Tokio::spawn(cx, async move {
            let accounts = crate::db::queries::settings::get_login_accounts(db.reader())
                .await
                .map_err(|e| e.to_string())?;

            if accounts.is_empty() {
                return Err("No login account".to_string());
            }

            let mut sessions: Vec<AccountSession> = Vec::new();
            let mut active_acct: Option<String> = None;

            for account in accounts {
                if account.access_token.is_empty() {
                    tracing::warn!("Skipping @{} — no access token", account.acct);
                    continue;
                }
                let acct = account.acct.clone();
                let domain = account.server_domain.clone();
                let streaming_url = format!("wss://{}", domain);
                let kind = ServerKind::from_db_str(&account.server_kind);
                let client_result = match kind {
                    ServerKind::Misskey => MisskeyClient::new(
                        &domain,
                        account.access_token.clone(),
                        streaming_url,
                    )
                    .map(ApiClient::Misskey),
                    ServerKind::Mastodon | ServerKind::Paon => MastodonClient::new(
                        &domain,
                        account.access_token.clone(),
                        streaming_url,
                    )
                    .map(ApiClient::Mastodon),
                };
                let client = match client_result {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Client error for @{}: {}", acct, e);
                        continue;
                    }
                };
                let account_info = match client.verify_credentials().await {
                    Ok(info) => info,
                    Err(e) => {
                        tracing::warn!("Token expired for @{}: {}", acct, e);
                        continue;
                    }
                };
                if account.is_active {
                    active_acct = Some(acct.clone());
                }
                sessions.push(AccountSession {
                    acct,
                    domain,
                    client,
                    account_info,
                });
            }

            if sessions.is_empty() {
                return Err("All sessions failed to restore".to_string());
            }

            // Fall back to the first session if no active account flagged
            if active_acct.is_none() {
                active_acct = sessions.first().map(|s| s.acct.clone());
            }

            Ok::<(Vec<AccountSession>, Option<String>), String>((sessions, active_acct))
        });

        cx.spawn_in(
            window,
            async |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| match task.await
            {
                Ok(Ok((sessions, active_acct))) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.on_sessions_restored(sessions, active_acct, window, cx);
                    });
                }
                Ok(Err(e)) => {
                    tracing::warn!("Session restore failed: {}", e);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.show_login(window, cx);
                    });
                }
                Err(e) => {
                    tracing::warn!("Task error: {}", e);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.show_login(window, cx);
                    });
                }
            },
        )
        .detach();
    }

    /// Add restored sessions to the manager, set the active one, and build the main view.
    fn on_sessions_restored(
        &mut self,
        sessions: Vec<AccountSession>,
        active_acct: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for session in sessions {
            self.session_manager.add_session(session);
        }
        if let Some(acct) = active_acct {
            self.session_manager.set_active(&acct);
        }
        let Some(active) = self.session_manager.active_session().cloned() else {
            self.show_login(window, cx);
            return;
        };
        // Persist the active flag to DB so the same account is restored next time
        self.persist_active_account(&active.acct, cx);
        self.activate_session(&active, window, cx);
    }

    fn show_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cancellable = !self.session_manager.is_empty();
        let login_view = cx.new(|cx| LoginView::new(window, cx).cancellable(cancellable));

        cx.subscribe_in(
            &login_view,
            window,
            |this, _login, event: &LoginEvent, window, cx| match event {
                LoginEvent::LoggedIn(session, kind) => {
                    this.on_login_success(session, *kind, window, cx);
                }
                LoginEvent::Cancelled => {
                    this.on_login_cancelled(window, cx);
                }
            },
        )
        .detach();

        self.view = WorkspaceView::Login(login_view);
        cx.notify();
    }

    fn on_login_cancelled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Cancel only makes sense when there's already an active session to fall back to
        if let Some(active) = self.session_manager.active_session().cloned() {
            self.activate_session(&active, window, cx);
        }
    }

    fn on_login_success(
        &mut self,
        session: &AccountSession,
        kind: ServerKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("Login successful: @{} ({:?})", session.acct, kind);

        // Add to session manager and mark as active
        self.session_manager.add_session(AccountSession {
            acct: session.acct.clone(),
            domain: session.domain.clone(),
            client: session.client.clone(),
            account_info: session.account_info.clone(),
        });
        self.session_manager.set_active(&session.acct);

        // Save login account to DB and clear is_active on others
        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let login_account = crate::db::models::DbLoginAccount {
                acct: session.acct.clone(),
                server_domain: session.domain.clone(),
                account_id: session.account_info.id.clone(),
                display_name: session.account_info.display_name.clone(),
                avatar: session.account_info.avatar.clone(),
                is_active: true,
                access_token: session.client.access_token().to_string(),
                server_kind: kind.as_db_str().to_string(),
            };
            let acct_for_active = session.acct.clone();
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::upsert_login_account(db.writer(), &login_account)
                        .await
                {
                    tracing::error!("Failed to save login account: {}", e);
                    return;
                }
                if let Err(e) = crate::db::queries::settings::set_active_account(
                    db.writer(),
                    &acct_for_active,
                )
                .await
                {
                    tracing::error!("Failed to set active account: {}", e);
                }
            })
            .detach();
        }

        self.activate_session(session, window, cx);
    }

    /// Build the main view (compose, columns, streaming) for the given session.
    fn activate_session(
        &mut self,
        session: &AccountSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let database = cx
            .try_global::<AppState>()
            .map(|s| s.database.clone())
            .expect("AppState should be set before login");

        // Track which session is the action-source for compose/boost/favourite.
        cx.set_global(ActiveAccount {
            client: session.client.clone(),
            acct: session.acct.clone(),
            account_id: session.account_info.id.clone(),
        });

        let acct = session.acct.clone();
        let client_for_emoji = session.client.clone();
        let db_for_query = database.clone();
        let domain_for_instance = session.domain.clone();
        let kind_for_instance = session.client.kind();

        let db_for_appearance = database.clone();
        let task = Tokio::spawn(cx, async move {
            let configs =
                crate::db::queries::settings::get_column_configs(db_for_query.reader(), &acct)
                    .await
                    .unwrap_or_default();

            // Fetch instance info for max_characters
            let max_chars = match kind_for_instance {
                crate::api::kind::ServerKind::Misskey => {
                    let unauth = crate::misskey::client::MisskeyUnauthenticatedClient::new()
                        .map_err(|e| e.to_string())?;
                    let url = format!("https://{}/api/meta", domain_for_instance);
                    match unauth
                        .post::<crate::misskey::types::meta::MisskeyMeta>(
                            &url,
                            serde_json::json!({ "detail": false }),
                        )
                        .await
                    {
                        Ok(meta) => meta.max_note_text_length.unwrap_or(3000) as usize,
                        Err(e) => {
                            tracing::warn!("Failed to fetch Misskey meta: {}", e);
                            3000
                        }
                    }
                }
                _ => {
                    let unauth = crate::mastodon::client::UnauthenticatedClient::new()
                        .map_err(|e| e.to_string())?;
                    match unauth.get_instance(&domain_for_instance).await {
                        Ok(instance) => instance.max_characters() as usize,
                        Err(e) => {
                            tracing::warn!("Failed to fetch instance info: {}", e);
                            500
                        }
                    }
                }
            };

            // Load appearance settings
            let appearance = match crate::db::queries::settings::get_setting(
                db_for_appearance.reader(),
                "appearance",
            )
            .await
            {
                Ok(Some(json)) => {
                    serde_json::from_str::<AppearanceSettings>(&json).unwrap_or_default()
                }
                _ => AppearanceSettings::default(),
            };

            // Load performance settings
            let performance = match crate::db::queries::settings::get_setting(
                db_for_appearance.reader(),
                "performance",
            )
            .await
            {
                Ok(Some(json)) => {
                    serde_json::from_str::<PerformanceSettings>(&json).unwrap_or_default()
                }
                _ => PerformanceSettings::default(),
            };

            // Load confirmation settings
            let confirmation = match crate::db::queries::settings::get_setting(
                db_for_appearance.reader(),
                "confirmation",
            )
            .await
            {
                Ok(Some(json)) => {
                    serde_json::from_str::<ConfirmationSettings>(&json).unwrap_or_default()
                }
                _ => ConfirmationSettings::default(),
            };

            // Load preset visibility settings
            let preset_visibility = match crate::db::queries::settings::get_setting(
                db_for_appearance.reader(),
                "preset_visibility",
            )
            .await
            {
                Ok(Some(json)) => {
                    serde_json::from_str::<PresetVisibilitySettings>(&json).unwrap_or_default()
                }
                _ => PresetVisibilitySettings::default(),
            };

            // Load notification suppression list
            let notification_suppression = match crate::db::queries::settings::get_setting(
                db_for_appearance.reader(),
                "notification_suppression",
            )
            .await
            {
                Ok(Some(json)) => serde_json::from_str::<NotificationSuppressionList>(&json)
                    .unwrap_or_default(),
                _ => NotificationSuppressionList::default(),
            };

            // Load behavior settings
            let behavior = match crate::db::queries::settings::get_setting(
                db_for_appearance.reader(),
                "behavior",
            )
            .await
            {
                Ok(Some(json)) => {
                    serde_json::from_str::<BehaviorSettings>(&json).unwrap_or_default()
                }
                _ => BehaviorSettings::default(),
            };

            // Fetch custom emojis
            let custom_emojis = match client_for_emoji.get_custom_emojis().await {
                Ok(emojis) => emojis,
                Err(e) => {
                    tracing::warn!("Failed to fetch custom emojis: {}", e);
                    vec![]
                }
            };

            Ok::<
                (
                    Vec<_>,
                    usize,
                    AppearanceSettings,
                    PerformanceSettings,
                    ConfirmationSettings,
                    PresetVisibilitySettings,
                    NotificationSuppressionList,
                    BehaviorSettings,
                    Vec<_>,
                ),
                String,
            >((
                configs,
                max_chars,
                appearance,
                performance,
                confirmation,
                preset_visibility,
                notification_suppression,
                behavior,
                custom_emojis,
            ))
        });

        let _domain = session.domain.clone();
        cx.spawn_in(
            window,
            async move |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                let (
                    configs,
                    max_chars,
                    appearance,
                    performance,
                    confirmation,
                    preset_visibility,
                    notification_suppression,
                    behavior,
                    custom_emojis,
                ) = match task.await {
                    Ok(Ok((
                        configs,
                        max_chars,
                        appearance,
                        performance,
                        confirmation,
                        preset_visibility,
                        notification_suppression,
                        behavior,
                        custom_emojis,
                    ))) => (
                        configs,
                        max_chars,
                        appearance,
                        performance,
                        confirmation,
                        preset_visibility,
                        notification_suppression,
                        behavior,
                        custom_emojis,
                    ),
                    _ => (
                        vec![],
                        500,
                        AppearanceSettings::default(),
                        PerformanceSettings::default(),
                        ConfirmationSettings::default(),
                        PresetVisibilitySettings::default(),
                        NotificationSuppressionList::default(),
                        BehaviorSettings::default(),
                        vec![],
                    ),
                };
                let _ = this.update_in(cx, |this, window, cx| {
                    this.max_characters = max_chars;
                    cx.set_global(appearance);
                    cx.set_global(performance);
                    cx.set_global(confirmation);
                    cx.set_global(preset_visibility);
                    cx.set_global(notification_suppression);
                    cx.set_global(behavior);

                    // Initialize emoji store
                    let mut emoji_store = EmojiStore::new();
                    emoji_store.set_custom_emojis(custom_emojis);
                    cx.set_global(emoji_store);

                    // Initialize compose input
                    this.compose_input = Some(cx.new(|cx| {
                        InputState::new(window, cx)
                            .multi_line(true)
                            .placeholder("What's on your mind?")
                    }));
                    this.subscribe_compose_enter(window, cx);

                    // Initialize search input
                    this.search_input = Some(cx.new(|cx| {
                        InputState::new(window, cx).placeholder("Search... (?query for YQ)")
                    }));
                    this.subscribe_search_enter(window, cx);

                    // Initialize emoji picker
                    if let Some(ref compose_input) = this.compose_input {
                        let picker =
                            cx.new(|cx| EmojiPicker::new(compose_input.clone(), window, cx));
                        // Observe EmojiPicker changes so the Workspace re-renders,
                        // which causes the Popover's deferred content to update.
                        cx.observe(&picker, |_this, _picker, cx| {
                            cx.notify();
                        })
                        .detach();
                        this.emoji_picker = Some(picker);
                    }

                    // Initialize autocomplete popup
                    if let Some(ref compose_input) = this.compose_input {
                        if let Some(session) = this.session_manager.active_session() {
                            let client = session.client.clone();
                            let db = cx
                                .try_global::<AppState>()
                                .map(|s| s.database.clone())
                                .expect("AppState should be set");
                            let popup = cx.new(|cx| {
                                AutocompletePopup::new(
                                    compose_input.clone(),
                                    client,
                                    db,
                                    window,
                                    cx,
                                )
                            });
                            cx.observe(&popup, |_this, _popup, cx| {
                                cx.notify();
                            })
                            .detach();
                            this.autocomplete_popup = Some(popup);
                        }
                    }

                    // Initialize visibility select (default: Public = index 0)
                    let items: Vec<&'static str> = VISIBILITY_OPTIONS.to_vec();
                    this.visibility_select = Some(cx.new(|cx| {
                        SelectState::new(
                            items,
                            Some(gpui_component::IndexPath {
                                section: 0,
                                row: 0,
                                column: 0,
                            }),
                            window,
                            cx,
                        )
                    }));

                    let entries = configs_to_entries(&configs);
                    this.build_main_view(entries, window, cx);
                });
            },
        )
        .detach();
    }

    fn build_main_view(
        &mut self,
        entries: Vec<ColumnEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Abort all previous streaming tasks
        {
            let handles = self.streaming_abort_handles.lock().unwrap();
            for handle in handles.iter() {
                handle.abort();
            }
        }
        self.streaming_abort_handles = Arc::new(std::sync::Mutex::new(Vec::new()));

        let Some(session) = self.session_manager.active_session().cloned() else {
            tracing::error!("No active session when building main view");
            return;
        };

        let client = session.client.clone();
        let acct = session.acct.clone();
        let account_id = session.account_info.id.clone();
        let database = cx
            .try_global::<AppState>()
            .map(|s| s.database.clone())
            .expect("AppState should be set before building main view");

        let unified_timeline = cx
            .try_global::<BehaviorSettings>()
            .map(|b| b.unified_timeline)
            .unwrap_or(false);

        // Collect non-active sessions for unified-timeline mode
        let extra_sessions: Vec<AccountSession> = if unified_timeline {
            self.session_manager
                .sessions()
                .values()
                .filter(|s| s.acct != acct)
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        // Use default (Home) if no entries configured
        let entries = if entries.is_empty() {
            let default_entry = ColumnEntry {
                id: uuid::Uuid::new_v4().to_string(),
                column_type: "home".to_string(),
                column_param: None,
                name: "Home".to_string(),
                max_statuses: Some(100),
                pane_index: 0,
            };
            // Save default config to DB
            let db = database.clone();
            let acct_for_save = acct.clone();
            let entry_for_save = default_entry.clone();
            Tokio::spawn(cx, async move {
                let config = DbColumnConfig {
                    id: entry_for_save.id.clone(),
                    account_acct: acct_for_save,
                    column_type: entry_for_save.column_type.clone(),
                    column_param: entry_for_save.column_param.clone(),
                    position: 0,
                    width: None,
                    created_at: String::new(),
                    name: Some(entry_for_save.name.clone()),
                    max_statuses: entry_for_save.max_statuses.map(|v| v as i32),
                    pane_index: Some(entry_for_save.pane_index as i32),
                };
                if let Err(e) =
                    crate::db::queries::settings::upsert_column_config(db.writer(), &config).await
                {
                    tracing::error!("Failed to save default column config: {}", e);
                }
            })
            .detach();
            vec![default_entry]
        } else {
            entries
        };

        // Build DockArea from column entries, creating per-panel streaming channels
        let streaming_url = client.streaming_url().to_string();
        let streaming_token = client.access_token().to_string();
        let streaming_domain = session.domain.clone();
        let streaming_kind = client.kind();
        let streaming_db = database.clone();
        let abort_handles = self.streaming_abort_handles.clone();

        let dock_area = cx.new(|cx| {
            let mut area = DockArea::new("main-dock", None, window, cx);
            area.set_locked(true, window, cx);
            let weak_area = cx.entity().downgrade();

            // Group entries by pane_index
            let mut pane_groups: BTreeMap<u32, Vec<&ColumnEntry>> = BTreeMap::new();
            for entry in &entries {
                pane_groups.entry(entry.pane_index).or_default().push(entry);
            }

            // Create panels grouped by pane, each with its own streaming channel
            let mut streaming_txs: Vec<futures::channel::mpsc::UnboundedSender<TimelineEvent>> =
                Vec::new();
            let mut pane_dock_items: Vec<DockItem> = Vec::new();

            for (_pane_idx, pane_entries) in &pane_groups {
                let mut pane_panels: Vec<Arc<dyn gpui_component::dock::PanelView>> = Vec::new();

                for entry in pane_entries {
                    let tl_type = TimelineType::from_column_config(
                        &entry.column_type,
                        entry.column_param.as_deref(),
                    );

                    if let Some(tl_type) = tl_type {
                        let panel_client = client.clone();
                        let panel_acct = acct.clone();
                        let panel_db = database.clone();
                        let panel_name = entry.name.clone();
                        let panel_max_statuses = entry.max_statuses;

                        let (panel_tx, panel_rx) =
                            futures::channel::mpsc::unbounded::<TimelineEvent>();

                        // For Home/Federated/Notification panels in unified mode,
                        // pass extra clients so initial load can aggregate across
                        // all signed-in accounts. Other panel types (List, Hashtag,
                        // Custom SQL, YQ, Bookmarks) are intentionally left
                        // single-account because they don't have a meaningful
                        // cross-account interpretation.
                        let panel_extra_clients: Vec<(ApiClient, String)> =
                            if unified_timeline
                                && matches!(
                                    tl_type,
                                    TimelineType::Home
                                        | TimelineType::Public
                                        | TimelineType::Notification
                                )
                            {
                                extra_sessions
                                    .iter()
                                    .map(|s| (s.client.clone(), s.acct.clone()))
                                    .collect()
                            } else {
                                Vec::new()
                            };

                        let panel_account_id = account_id.clone();
                        let panel = cx.new(|cx| {
                            TimelinePanel::new(
                                panel_name,
                                tl_type,
                                panel_client,
                                panel_acct,
                                panel_account_id,
                                panel_db,
                                panel_max_statuses,
                                panel_extra_clients,
                                window,
                                cx,
                            )
                        });

                        panel.update(cx, |panel, cx| {
                            panel.start_streaming(panel_rx, cx);
                        });

                        streaming_txs.push(panel_tx);
                        pane_panels.push(Arc::new(panel));
                    }
                }

                if !pane_panels.is_empty() {
                    pane_dock_items.push(DockItem::tabs(pane_panels, &weak_area, window, cx));
                }
            }

            // Start WebSocket streaming on tokio, broadcasting to all panel channels
            let url = streaming_url.clone();
            let token = streaming_token.clone();
            let domain = streaming_domain.clone();
            let db = streaming_db.clone();
            let mut stream_types = vec![
                StreamType::User,
                StreamType::Public,
                StreamType::PublicLocal,
                StreamType::Direct,
            ];
            for entry in &entries {
                if entry.column_type == "list" {
                    if let Some(ref list_id) = entry.column_param {
                        let st = StreamType::List(list_id.clone());
                        if !stream_types.contains(&st) {
                            stream_types.push(st);
                        }
                    }
                }
            }

            // In unified mode, also collect connection info for the extra
            // accounts so each one streams Home/Federated/Notification into
            // the same panel txs. Lists are primary-account-only (the list
            // belongs to that account's server).
            let extra_streaming: Vec<(String, String, String, ServerKind)> = extra_sessions
                .iter()
                .map(|s| {
                    (
                        s.client.streaming_url().to_string(),
                        s.client.access_token().to_string(),
                        s.domain.clone(),
                        s.client.kind(),
                    )
                })
                .collect();

            let abort_handles_ref = abort_handles.clone();
            Tokio::spawn(cx, async move {
                let mut all_handles = Vec::new();

                // Primary account: full set of stream types (incl. list streams).
                let primary_handles = streaming_service::start_streaming(
                    url,
                    token,
                    stream_types.clone(),
                    domain,
                    streaming_kind,
                    db.clone(),
                    streaming_txs.clone(),
                );
                all_handles.extend(primary_handles);

                // Extra accounts (unified mode): only the unified-relevant
                // stream types (User, Public, Direct). PublicLocal is included
                // so the Federated panel reflects any local-only posts the
                // extra account's server exposes; list streams are skipped
                // because list IDs are scoped to the primary account's server.
                let unified_stream_types = vec![
                    StreamType::User,
                    StreamType::Public,
                    StreamType::PublicLocal,
                    StreamType::Direct,
                ];
                for (extra_url, extra_token, extra_domain, extra_kind) in extra_streaming {
                    let handles = streaming_service::start_streaming(
                        extra_url,
                        extra_token,
                        unified_stream_types.clone(),
                        extra_domain,
                        extra_kind,
                        db.clone(),
                        streaming_txs.clone(),
                    );
                    all_handles.extend(handles);
                }

                *abort_handles_ref.lock().unwrap() = all_handles;
            })
            .detach();

            // Build DockItem layout
            if pane_dock_items.len() == 1 {
                area.set_center(pane_dock_items.into_iter().next().unwrap(), window, cx);
            } else if pane_dock_items.len() > 1 {
                area.set_center(
                    DockItem::h_split(pane_dock_items, &weak_area, window, cx),
                    window,
                    cx,
                );
            }

            area
        });

        // Clear old dynamic panel references to allow entity cleanup
        self.dynamic_panels.clear();

        self.view = WorkspaceView::Main(dock_area);
        self.start_bookmark_sync(cx);
        self.start_status_bar_stats(cx);
        cx.notify();
    }

    fn start_status_bar_stats(&mut self, cx: &mut Context<Self>) {
        if self.stats_updater_started {
            return;
        }
        self.stats_updater_started = true;

        let Some(db) = cx.try_global::<AppState>().map(|s| s.database.clone()) else {
            return;
        };

        let (tx, rx) = futures::channel::mpsc::unbounded::<(i64, i64)>();

        Tokio::spawn(cx, async move {
            loop {
                let count = crate::db::queries::settings::get_status_count(db.reader())
                    .await
                    .unwrap_or(0);
                let threshold = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
                let recent =
                    crate::db::queries::settings::get_recent_status_count(db.reader(), &threshold)
                        .await
                        .unwrap_or(0);
                if tx.unbounded_send((count, recent)).is_err() {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
            }
            Ok::<(), String>(())
        })
        .detach();

        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                let mut rx = rx;
                while let Some((count, recent)) = rx.next().await {
                    let _ = this.update(cx, |_this, cx| {
                        cx.set_global(StatusBarStats {
                            status_count: count,
                            recent_count: recent,
                        });
                    });
                }
            },
        )
        .detach();

        // 1-second tick for uptime display
        let (tick_tx, tick_rx) = futures::channel::mpsc::unbounded::<()>();
        Tokio::spawn(cx, async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                if tick_tx.unbounded_send(()).is_err() {
                    break;
                }
            }
            Ok::<(), String>(())
        })
        .detach();

        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                let mut tick_rx = tick_rx;
                while tick_rx.next().await.is_some() {
                    let _ = this.update(cx, |_this, cx| {
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn start_bookmark_sync(&self, cx: &mut Context<Self>) {
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let Some(app_state) = cx.try_global::<AppState>() else {
            return;
        };
        let database = app_state.database.clone();

        cx.set_global(BookmarkSyncState {
            syncing: true,
            message: Some("Syncing bookmarks...".into()),
        });

        let (progress_tx, progress_rx) = futures::channel::mpsc::unbounded::<u32>();

        let task = Tokio::spawn(cx, async move {
            use crate::mastodon::endpoints::timelines::TimelineParams;
            use crate::services::timeline_service;

            let mut page = 1u32;
            let mut max_id: Option<String> = None;
            let server_domain = client.domain().to_string();
            const MAX_PAGES: u32 = 50;

            loop {
                let params = TimelineParams {
                    max_id: max_id.clone(),
                    limit: Some(40),
                    ..TimelineParams::default()
                };
                let response = client
                    .get_bookmarks(&params)
                    .await
                    .map_err(|e| e.to_string())?;

                if response.data.is_empty() {
                    break;
                }

                for status in &response.data {
                    if let Err(e) = timeline_service::save_status_to_db(
                        database.writer(),
                        status,
                        &server_domain,
                    )
                    .await
                    {
                        tracing::warn!("Failed to save bookmarked status: {}", e);
                    }
                }

                // Use next page cursor from Link header
                max_id = response.next_max_id;
                if max_id.is_none() {
                    break; // No more pages
                }

                page += 1;
                let _ = progress_tx.unbounded_send(page);

                if page > MAX_PAGES {
                    break;
                }
            }

            Ok::<u32, String>(page)
        });

        // Receive progress updates
        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                let mut rx = progress_rx;
                while let Some(page) = rx.next().await {
                    let _ = this.update(cx, |_this, cx| {
                        cx.set_global(BookmarkSyncState {
                            syncing: true,
                            message: Some(format!("Syncing bookmarks... (page {})", page)),
                        });
                    });
                }
            },
        )
        .detach();

        // Handle completion
        let weak_for_clear = cx.entity().downgrade();
        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(pages)) => {
                    tracing::info!("Bookmark sync completed ({} pages)", pages);
                    let _ = this.update(cx, |_this, cx| {
                        cx.set_global(BookmarkSyncState {
                            syncing: false,
                            message: Some("Bookmarks synced".into()),
                        });
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Bookmark sync failed: {}", e);
                    let _ = this.update(cx, |_this, cx| {
                        cx.set_global(BookmarkSyncState {
                            syncing: false,
                            message: Some(format!("Bookmark sync failed: {}", e)),
                        });
                    });
                }
                Err(e) => {
                    tracing::error!("Bookmark sync task error: {}", e);
                }
            },
        )
        .detach();

        // Clear status bar message after 3 seconds (via tokio runtime)
        let delay_task = Tokio::spawn(cx, async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            Ok::<(), String>(())
        });
        cx.spawn(
            async move |_this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
                let _ = delay_task.await;
                let _ = weak_for_clear.update(cx, |_this, cx| {
                    cx.set_global(BookmarkSyncState {
                        syncing: false,
                        message: None,
                    });
                });
            },
        )
        .detach();
    }

    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let acct = session.acct.clone();

        // Load existing column configs
        let database = cx.try_global::<AppState>().map(|s| s.database.clone());

        let Some(database) = database else { return };

        let client_for_lists = session.client.clone();
        let task = Tokio::spawn(cx, async move {
            let configs =
                crate::db::queries::settings::get_column_configs(database.reader(), &acct)
                    .await
                    .unwrap_or_default();
            let lists = client_for_lists.get_lists().await.unwrap_or_default();
            (configs, lists)
        });

        cx.spawn_in(
            window,
            async move |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                let (configs, lists) = match task.await {
                    Ok(result) => result,
                    Err(_) => (vec![], vec![]),
                };
                let _ = this.update_in(cx, |this, window, cx| {
                    let entries = configs_to_entries(&configs);
                    let active_acct = this
                        .session_manager
                        .active_session()
                        .map(|s| s.acct.clone())
                        .unwrap_or_default();
                    let accounts = this
                        .session_manager
                        .sessions()
                        .values()
                        .map(|s| AccountInfo {
                            acct_key: s.acct.clone(),
                            avatar: s.account_info.avatar.clone(),
                            display_name: s.account_info.display_name.clone(),
                            acct: s.account_info.acct.clone(),
                            is_active: s.acct == active_acct,
                        })
                        .collect::<Vec<_>>();

                    let database = cx
                        .try_global::<AppState>()
                        .map(|s| s.database.clone())
                        .expect("AppState should be set before settings");

                    let appearance = cx.global::<AppearanceSettings>().clone();
                    let performance = cx.global::<PerformanceSettings>().clone();
                    let confirmation = cx.global::<ConfirmationSettings>().clone();
                    let preset_visibility =
                        cx.global::<PresetVisibilitySettings>().clone();
                    let behavior = cx
                        .try_global::<BehaviorSettings>()
                        .cloned()
                        .unwrap_or_default();
                    let settings_view = cx.new(|cx| {
                        SettingsView::new(
                            active_acct,
                            accounts,
                            database,
                            entries,
                            lists,
                            appearance,
                            performance,
                            confirmation,
                            preset_visibility,
                            behavior,
                            window,
                            cx,
                        )
                    });

                    cx.subscribe_in(
                        &settings_view,
                        window,
                        |this, _view, event: &SettingsEvent, window, cx| {
                            match event {
                                SettingsEvent::ConfigSaved(entries) => {
                                    this.on_config_saved(entries.clone(), window, cx);
                                }
                                SettingsEvent::AppearanceSaved(settings) => {
                                    this.on_appearance_saved(settings.clone(), cx);
                                }
                                SettingsEvent::PerformanceSaved(settings) => {
                                    this.on_performance_saved(settings.clone(), cx);
                                }
                                SettingsEvent::ConfirmationSaved(settings) => {
                                    this.on_confirmation_saved(settings.clone(), cx);
                                }
                                SettingsEvent::PresetVisibilitySaved(settings) => {
                                    this.on_preset_visibility_saved(settings.clone(), cx);
                                }
                                SettingsEvent::BehaviorSaved(settings) => {
                                    this.on_behavior_saved(settings.clone(), cx);
                                }
                                SettingsEvent::Closed => {
                                    // Go back to main view with current config
                                    this.on_settings_closed(window, cx);
                                }
                                SettingsEvent::Logout(acct) => {
                                    this.logout_account(acct.clone(), window, cx);
                                }
                                SettingsEvent::AddAccount => {
                                    this.add_account(window, cx);
                                }
                                SettingsEvent::SwitchAccount(acct) => {
                                    this.switch_active_account(acct.clone(), window, cx);
                                }
                            }
                        },
                    )
                    .detach();

                    // Abort streaming tasks and clear panel references before switching
                    {
                        let handles = this.streaming_abort_handles.lock().unwrap();
                        for handle in handles.iter() {
                            handle.abort();
                        }
                    }
                    this.dynamic_panels.clear();

                    this.view = WorkspaceView::Settings(settings_view);
                    cx.notify();
                });
            },
        )
        .detach();
    }

    /// Log out the specified account.
    /// If it was the active session, fall back to another account or show the login view.
    fn logout_account(
        &mut self,
        acct: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_active = self
            .session_manager
            .active_session()
            .map(|s| s.acct == acct)
            .unwrap_or(false);

        self.session_manager.remove_session(&acct);

        // Delete login account (and token) from DB
        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let acct_for_db = acct.clone();
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::delete_login_account(db.writer(), &acct_for_db)
                        .await
                {
                    tracing::error!("Failed to delete login account: {}", e);
                }
            })
            .detach();
        }

        if !was_active {
            // Stay on the current settings/main view; just refresh
            cx.notify();
            return;
        }

        // Abort streaming tasks before switching away from the now-removed session
        {
            let handles = self.streaming_abort_handles.lock().unwrap();
            for handle in handles.iter() {
                handle.abort();
            }
        }

        // Reset compose state
        self.compose_input = None;
        self.visibility_select = None;

        if let Some(next) = self.session_manager.active_session().cloned() {
            self.persist_active_account(&next.acct, cx);
            self.activate_session(&next, window, cx);
        } else {
            self.show_login(window, cx);
        }
    }

    /// Switch the active session to another already-logged-in account.
    fn switch_active_account(
        &mut self,
        acct: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .session_manager
            .active_session()
            .map(|s| s.acct == acct)
            .unwrap_or(false)
        {
            return;
        }
        if !self.session_manager.set_active(&acct) {
            tracing::warn!("Cannot switch to unknown account: {}", acct);
            return;
        }
        let Some(active) = self.session_manager.active_session().cloned() else {
            return;
        };

        let unified = cx
            .try_global::<BehaviorSettings>()
            .map(|b| b.unified_timeline)
            .unwrap_or(false);
        let in_main_view = matches!(self.view, WorkspaceView::Main(_));

        if unified && in_main_view {
            // Unified mode: keep the existing columns/streaming pinned to the
            // primary account; only swap the action-source session.
            cx.set_global(ActiveAccount {
                client: active.client.clone(),
                acct: active.acct.clone(),
                account_id: active.account_info.id.clone(),
            });
            self.persist_active_account(&active.acct, cx);
            self.refresh_compose_for_active_session(window, cx);
            cx.notify();
            return;
        }

        // Non-unified path: tear down streaming and rebuild for the new account.
        {
            let handles = self.streaming_abort_handles.lock().unwrap();
            for handle in handles.iter() {
                handle.abort();
            }
        }
        self.compose_input = None;
        self.visibility_select = None;

        self.persist_active_account(&active.acct, cx);
        self.activate_session(&active, window, cx);
    }

    /// In unified-timeline mode, swapping the active session must not rebuild
    /// the timeline columns. This recreates only the compose-related UI so
    /// posts/replies route through the newly-selected account.
    fn refresh_compose_for_active_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_manager.active_session().cloned() else {
            return;
        };
        let database = cx
            .try_global::<AppState>()
            .map(|s| s.database.clone())
            .expect("AppState should be set before refreshing compose");

        // Re-create compose input so any pending state (placeholder/text) is reset.
        self.compose_input = Some(cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .placeholder("What's on your mind?")
        }));
        self.subscribe_compose_enter(window, cx);

        // Re-create autocomplete popup against the new account's client.
        if let Some(ref compose_input) = self.compose_input {
            let popup = cx.new(|cx| {
                AutocompletePopup::new(
                    compose_input.clone(),
                    session.client.clone(),
                    database,
                    window,
                    cx,
                )
            });
            cx.observe(&popup, |_this, _popup, cx| {
                cx.notify();
            })
            .detach();
            self.autocomplete_popup = Some(popup);
        }

        // Re-create emoji picker bound to the new compose input.
        if let Some(ref compose_input) = self.compose_input {
            let picker = cx.new(|cx| EmojiPicker::new(compose_input.clone(), window, cx));
            cx.observe(&picker, |_this, _picker, cx| {
                cx.notify();
            })
            .detach();
            self.emoji_picker = Some(picker);
        }

        // Reset visibility to Public when switching accounts.
        let items: Vec<&'static str> = VISIBILITY_OPTIONS.to_vec();
        self.visibility_select = Some(cx.new(|cx| {
            SelectState::new(
                items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: 0,
                    column: 0,
                }),
                window,
                cx,
            )
        }));

        // Refresh per-instance compose limits (max characters, custom emojis).
        let domain = session.domain.clone();
        let kind = session.client.kind();
        let client_for_emoji = session.client.clone();
        let task = Tokio::spawn(cx, async move {
            let max_chars = match kind {
                crate::api::kind::ServerKind::Misskey => {
                    let unauth = crate::misskey::client::MisskeyUnauthenticatedClient::new()
                        .map_err(|e| e.to_string())?;
                    let url = format!("https://{}/api/meta", domain);
                    match unauth
                        .post::<crate::misskey::types::meta::MisskeyMeta>(
                            &url,
                            serde_json::json!({ "detail": false }),
                        )
                        .await
                    {
                        Ok(meta) => meta.max_note_text_length.unwrap_or(3000) as usize,
                        Err(_) => 3000,
                    }
                }
                _ => {
                    let unauth = crate::mastodon::client::UnauthenticatedClient::new()
                        .map_err(|e| e.to_string())?;
                    match unauth.get_instance(&domain).await {
                        Ok(instance) => instance.max_characters() as usize,
                        Err(_) => 500,
                    }
                }
            };
            let custom_emojis = client_for_emoji.get_custom_emojis().await.unwrap_or_default();
            Ok::<(usize, Vec<CustomEmoji>), String>((max_chars, custom_emojis))
        });

        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
                if let Ok(Ok((max_chars, custom_emojis))) = task.await {
                    let _ = this.update(cx, |this, cx| {
                        this.max_characters = max_chars;
                        let mut emoji_store = EmojiStore::new();
                        emoji_store.set_custom_emojis(custom_emojis);
                        cx.set_global(emoji_store);
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    /// Show the login flow to add another account on top of the existing sessions.
    fn add_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Streaming for the current session can stay running while the user types in the login flow.
        self.show_login(window, cx);
    }

    fn persist_active_account(&self, acct: &str, cx: &mut Context<Self>) {
        let Some(app_state) = cx.try_global::<AppState>() else {
            return;
        };
        let db = app_state.database.clone();
        let acct = acct.to_string();
        Tokio::spawn(cx, async move {
            if let Err(e) =
                crate::db::queries::settings::set_active_account(db.writer(), &acct).await
            {
                tracing::error!("Failed to persist active account: {}", e);
            }
        })
        .detach();
    }

    fn on_config_saved(
        &mut self,
        entries: Vec<ColumnEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let acct = session.acct.clone();

        // Save to DB
        let database = cx.try_global::<AppState>().map(|s| s.database.clone());

        if let Some(database) = database {
            let entries_for_save = entries.clone();
            let acct_for_save = acct.clone();
            Tokio::spawn(cx, async move {
                // Delete all existing configs
                if let Err(e) = crate::db::queries::settings::delete_all_column_configs(
                    database.writer(),
                    &acct_for_save,
                )
                .await
                {
                    tracing::error!("Failed to delete column configs: {}", e);
                    return;
                }

                // Insert new configs with per-pane position counters
                let mut position_counters: std::collections::HashMap<u32, i32> =
                    std::collections::HashMap::new();
                for entry in entries_for_save.iter() {
                    let position = position_counters.entry(entry.pane_index).or_insert(0);
                    let config = DbColumnConfig {
                        id: entry.id.clone(),
                        account_acct: acct_for_save.clone(),
                        column_type: entry.column_type.clone(),
                        column_param: entry.column_param.clone(),
                        position: *position,
                        width: None,
                        created_at: String::new(),
                        name: Some(entry.name.clone()),
                        max_statuses: entry.max_statuses.map(|v| v as i32),
                        pane_index: Some(entry.pane_index as i32),
                    };
                    *position += 1;
                    if let Err(e) = crate::db::queries::settings::upsert_column_config(
                        database.writer(),
                        &config,
                    )
                    .await
                    {
                        tracing::error!("Failed to save column config: {}", e);
                    }
                }
            })
            .detach();
        }

        // Rebuild main view
        self.build_main_view(entries, window, cx);
    }

    fn on_appearance_saved(&mut self, settings: AppearanceSettings, cx: &mut Context<Self>) {
        cx.set_global(settings.clone());

        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let json = serde_json::to_string(&settings).unwrap_or_default();
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::set_setting(db.writer(), "appearance", &json)
                        .await
                {
                    tracing::error!("Failed to save appearance settings: {}", e);
                }
            })
            .detach();
        }
    }

    fn on_performance_saved(&mut self, settings: PerformanceSettings, cx: &mut Context<Self>) {
        cx.set_global(settings.clone());

        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let json = serde_json::to_string(&settings).unwrap_or_default();
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::set_setting(db.writer(), "performance", &json)
                        .await
                {
                    tracing::error!("Failed to save performance settings: {}", e);
                }
            })
            .detach();
        }
    }

    fn on_confirmation_saved(&mut self, settings: ConfirmationSettings, cx: &mut Context<Self>) {
        cx.set_global(settings.clone());

        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let json = serde_json::to_string(&settings).unwrap_or_default();
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::set_setting(db.writer(), "confirmation", &json)
                        .await
                {
                    tracing::error!("Failed to save confirmation settings: {}", e);
                }
            })
            .detach();
        }
    }

    fn on_preset_visibility_saved(
        &mut self,
        settings: PresetVisibilitySettings,
        cx: &mut Context<Self>,
    ) {
        cx.set_global(settings.clone());

        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let json = serde_json::to_string(&settings).unwrap_or_default();
            Tokio::spawn(cx, async move {
                if let Err(e) = crate::db::queries::settings::set_setting(
                    db.writer(),
                    "preset_visibility",
                    &json,
                )
                .await
                {
                    tracing::error!("Failed to save preset visibility settings: {}", e);
                }
            })
            .detach();
        }
    }

    fn on_behavior_saved(&mut self, settings: BehaviorSettings, cx: &mut Context<Self>) {
        cx.set_global(settings.clone());

        if let Some(app_state) = cx.try_global::<AppState>() {
            let db = app_state.database.clone();
            let json = serde_json::to_string(&settings).unwrap_or_default();
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::set_setting(db.writer(), "behavior", &json)
                        .await
                {
                    tracing::error!("Failed to save behavior settings: {}", e);
                }
            })
            .detach();
        }
    }

    fn render_compose_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session_manager.active_session() else {
            return div().into_any_element();
        };
        let Some(compose_input) = &self.compose_input else {
            return div().into_any_element();
        };

        let avatar_url = session.account_info.avatar.clone();
        let active_acct = session.acct.clone();
        let char_count = compose_input.read(cx).value().chars().count();
        let max_chars = self.max_characters;
        let posting = self.posting;

        // Snapshot all logged-in accounts for the avatar switcher menu
        let switch_options: Vec<(String, String, String, bool)> = self
            .session_manager
            .sessions()
            .values()
            .map(|s| {
                (
                    s.acct.clone(),
                    s.account_info.display_name.clone(),
                    s.account_info.acct.clone(),
                    s.acct == active_acct,
                )
            })
            .collect();

        div()
            .flex()
            .bg(rgb(0x181825))
            .border_b_1()
            .border_color(rgb(0x313244))
            .p(px(8.0))
            .gap(px(8.0))
            // Left column: Avatar (top) + flex space + Settings icon (bottom)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(4.0))
                    // Avatar — clicking opens an account switcher dropdown
                    .child(
                        Button::new("compose-avatar-switch")
                            .text()
                            .child(
                                div()
                                    .w(px(36.0))
                                    .h(px(36.0))
                                    .rounded(px(4.0))
                                    .overflow_hidden()
                                    .child(
                                        img(avatar_url)
                                            .w(px(36.0))
                                            .h(px(36.0))
                                            .object_fit(ObjectFit::Cover),
                                    ),
                            )
                            .dropdown_menu_with_anchor(
                                Corner::TopLeft,
                                move |menu: PopupMenu,
                                      _window: &mut Window,
                                      _cx: &mut Context<PopupMenu>| {
                                    let mut menu = menu;
                                    for (acct_key, display_name, acct, is_active) in
                                        switch_options.iter()
                                    {
                                        let label = if *is_active {
                                            format!("✓ {} (@{})", display_name, acct)
                                        } else {
                                            format!("  {} (@{})", display_name, acct)
                                        };
                                        let acct_key = acct_key.clone();
                                        let active = *is_active;
                                        menu = menu.item(
                                            PopupMenuItem::new(label).on_click(
                                                move |_, _window, cx| {
                                                    if active {
                                                        return;
                                                    }
                                                    cx.set_global(MenuAction(Some(
                                                        MenuActionKind::SwitchAccount(
                                                            acct_key.clone(),
                                                        ),
                                                    )));
                                                },
                                            ),
                                        );
                                    }
                                    menu.separator().item(
                                        PopupMenuItem::new("Add Account").on_click(
                                            move |_, _window, cx| {
                                                cx.set_global(MenuAction(Some(
                                                    MenuActionKind::AddAccount,
                                                )));
                                            },
                                        ),
                                    )
                                },
                            ),
                    )
                    // Flex spacer
                    .child(div().flex_1())
                    // Hamburger menu
                    .child(
                        Button::new("menu-btn")
                            .ghost()
                            .icon(IconName::Menu)
                            .dropdown_menu_with_anchor(
                            Corner::TopLeft,
                            move |menu: PopupMenu,
                                  _window: &mut Window,
                                  _cx: &mut Context<PopupMenu>| {
                                menu.item(PopupMenuItem::new("Bookmarks").on_click(
                                    move |_, _window, cx| {
                                        cx.set_global(MenuAction(Some(
                                            MenuActionKind::OpenBookmarks,
                                        )));
                                    },
                                ))
                                .separator()
                                .item(
                                    PopupMenuItem::new("Settings").on_click(
                                        move |_, _window, cx| {
                                            cx.set_global(MenuAction(Some(
                                                MenuActionKind::OpenSettings,
                                            )));
                                        },
                                    ),
                                )
                            },
                        ),
                    ),
            )
            // Right area: Compose input (top) + bottom row
            .child(
                div()
                    .id("compose-drop-zone")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .rounded(px(4.0))
                    .when(self.drag_over, |el| {
                        el.border_2().border_color(rgb(0x89b4fa))
                    })
                    .on_drag_move::<ExternalPaths>(cx.listener(|this, _, _, cx| {
                        if !this.drag_over {
                            this.drag_over = true;
                            cx.notify();
                        }
                    }))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                        this.drag_over = false;
                        this.handle_file_drop(paths.paths(), cx);
                    }))
                    // Reply preview (shown when replying to a status)
                    .when_some(self.reply_target.as_ref(), |this, target| {
                        let content_preview = html_to_plain_text(&target.content);
                        let preview_text = if content_preview.chars().count() > 100 {
                            let truncated: String = content_preview.chars().take(100).collect();
                            format!("{}...", truncated)
                        } else {
                            content_preview
                        };
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x313244))
                                .child(
                                    Icon::default()
                                        .path("icons/message-circle.svg")
                                        .xsmall()
                                        .text_color(rgb(0x89b4fa)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_xs()
                                        .text_color(rgb(0xa6adc8))
                                        .child(format!(
                                            "{} {} — {}",
                                            target.display_name, target.acct, preview_text,
                                        )),
                                )
                                .child(
                                    div()
                                        .id("cancel-reply")
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(rgb(0x6c7086))
                                        .child("✕")
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.cancel_reply(cx);
                                        })),
                                ),
                        )
                    })
                    // Edit preview (shown when editing a status)
                    .when_some(self.edit_target.as_ref(), |this, target| {
                        let content_preview = html_to_plain_text(&target.content);
                        let preview_text = if content_preview.chars().count() > 100 {
                            let truncated: String = content_preview.chars().take(100).collect();
                            format!("{}...", truncated)
                        } else {
                            content_preview
                        };
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x313244))
                                .child(
                                    Icon::default()
                                        .path("icons/pencil.svg")
                                        .xsmall()
                                        .text_color(rgb(0xf9e2af)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_xs()
                                        .text_color(rgb(0xa6adc8))
                                        .child(format!("Editing — {}", preview_text,)),
                                )
                                .child(
                                    div()
                                        .id("cancel-edit")
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(rgb(0x6c7086))
                                        .child("✕")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel_edit(window, cx);
                                        })),
                                ),
                        )
                    })
                    // Quote preview (shown when quoting a status)
                    .when_some(self.quote_target.as_ref(), |this, target| {
                        let content_preview = crate::ui::components::html_content::html_to_plain_text(&target.content);
                        let preview_text = if content_preview.chars().count() > 100 {
                            let truncated: String = content_preview.chars().take(100).collect();
                            format!("{}...", truncated)
                        } else {
                            content_preview
                        };
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(8.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x313244))
                                .child(
                                    Icon::default()
                                        .path("icons/quote.svg")
                                        .xsmall()
                                        .text_color(rgb(0xa6e3a1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_xs()
                                        .text_color(rgb(0xa6adc8))
                                        .child(format!(
                                            "Quoting {} {} — {}",
                                            target.display_name, target.acct, preview_text,
                                        )),
                                )
                                .child(
                                    div()
                                        .id("cancel-quote")
                                        .cursor_pointer()
                                        .text_xs()
                                        .text_color(rgb(0x6c7086))
                                        .child("✕")
                                        .on_click(cx.listener(|this, _, _window, cx| {
                                            this.cancel_quote(cx);
                                        })),
                                ),
                        )
                    })
                    // CW input (shown when CW is enabled)
                    .when_some(
                        if self.cw_enabled {
                            self.cw_input.as_ref()
                        } else {
                            None
                        },
                        |this, cw_input| {
                            this.child(Input::new(cw_input).appearance(true).h(px(28.0)))
                        },
                    )
                    // Compose input (wrapped in div for key capture)
                    .child(
                        div()
                            .relative()
                            .capture_key_down(cx.listener(
                                |this, event: &KeyDownEvent, window, cx| {
                                    let Some(popup) = &this.autocomplete_popup else {
                                        return;
                                    };
                                    if !popup.read(cx).is_visible() {
                                        return;
                                    }
                                    let key = &event.keystroke.key;
                                    match key.as_ref() {
                                        "up" => {
                                            popup.update(cx, |p, cx| p.select_up(cx));
                                            cx.stop_propagation();
                                        }
                                        "down" => {
                                            popup.update(cx, |p, cx| p.select_down(cx));
                                            cx.stop_propagation();
                                        }
                                        "enter" => {
                                            popup
                                                .update(cx, |p, cx| p.accept_selection(window, cx));
                                            cx.stop_propagation();
                                        }
                                        "escape" => {
                                            popup.update(cx, |p, cx| p.dismiss(cx));
                                            cx.stop_propagation();
                                        }
                                        _ => {}
                                    }
                                },
                            ))
                            .child(Input::new(compose_input).appearance(true).h(px(60.0)))
                            // Autocomplete popup (floating below compose input)
                            .when_some(
                                self.autocomplete_popup
                                    .as_ref()
                                    .filter(|p| p.read(cx).is_visible()),
                                |el, popup| el.child(deferred(popup.clone()).with_priority(1)),
                            ),
                    )
                    // Poll options editor
                    .when(self.poll_enabled && !self.poll_options.is_empty(), |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .p(px(4.0))
                                .rounded(px(4.0))
                                .bg(rgb(0x313244))
                                // Poll option rows
                                .children(self.poll_options.iter().enumerate().map(
                                    |(i, input_state)| {
                                        let indicator = if self.poll_multiple { "☐" } else { "○" };
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(0x6c7086))
                                                    .child(indicator),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .child(
                                                        Input::new(input_state)
                                                            .appearance(true)
                                                            .h(px(28.0)),
                                                    ),
                                            )
                                            .when(self.poll_options.len() > 2, |el| {
                                                el.child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "remove-poll-{}",
                                                            i
                                                        )))
                                                        .cursor_pointer()
                                                        .text_xs()
                                                        .text_color(rgb(0x6c7086))
                                                        .hover(|s| s.text_color(rgb(0xf38ba8)))
                                                        .child("✕")
                                                        .on_click(cx.listener(
                                                            move |this, _, _window, cx| {
                                                                this.remove_poll_option(i, cx);
                                                            },
                                                        )),
                                                )
                                            })
                                            .into_any_element()
                                    },
                                ))
                                // Bottom controls: + Add | Single/Multiple | Duration
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        // Add option button
                                        .when(self.poll_options.len() < 4, |el| {
                                            el.child(
                                                Button::new("add-poll-option")
                                                    .ghost()
                                                    .xsmall()
                                                    .label("+ 追加")
                                                    .on_click(cx.listener(
                                                        |this, _, window, cx| {
                                                            this.add_poll_option(window, cx);
                                                        },
                                                    )),
                                            )
                                        })
                                        // Single/Multiple toggle
                                        .child(
                                            Button::new("poll-multiple-toggle")
                                                .ghost()
                                                .xsmall()
                                                .label(if self.poll_multiple {
                                                    "Multiple"
                                                } else {
                                                    "Single"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.poll_multiple = !this.poll_multiple;
                                                    cx.notify();
                                                })),
                                        )
                                        // Duration select
                                        .when_some(
                                            self.poll_duration_select.as_ref(),
                                            |el, dur_state| {
                                                el.child(
                                                    div()
                                                        .w(px(80.0))
                                                        .flex_shrink_0()
                                                        .child(
                                                            Select::new(dur_state)
                                                                .menu_width(px(100.0)),
                                                        ),
                                                )
                                            },
                                        ),
                                ),
                        )
                    })
                    // Attached files preview
                    .when(!self.attachments.is_empty(), |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .children(self.attachments.iter().enumerate().map(
                                    |(i, attachment)| {
                                        let drag_name: SharedString =
                                            attachment.filename.clone().into();
                                        if attachment.is_image {
                                            // Image preview thumbnail
                                            let local_path = attachment.local_path.clone();
                                            div()
                                                .id(SharedString::from(format!("attached-{}", i)))
                                                .relative()
                                                .w(px(80.0))
                                                .h(px(60.0))
                                                .rounded(px(4.0))
                                                .overflow_hidden()
                                                .cursor_pointer()
                                                .border_1()
                                                .border_color(rgb(0x45475a))
                                                .hover(|s| s.border_color(rgb(0x89b4fa)))
                                                .on_click(cx.listener(move |_this, _, _, cx| {
                                                    cx.set_global(LightboxState {
                                                        url: None,
                                                        local_path: Some(local_path.clone()),
                                                        status_ctx: None,
                                                        zoom: 1.0,
                                                        pan_x: 0.0,
                                                        pan_y: 0.0,
                                                    });
                                                }))
                                                .on_drag(
                                                    DraggedAttachment {
                                                        index: i,
                                                        name: drag_name,
                                                    },
                                                    |drag, _, _, cx| {
                                                        cx.stop_propagation();
                                                        cx.new(|_| drag.clone())
                                                    },
                                                )
                                                .drag_over::<DraggedAttachment>(
                                                    |style, _, _, _| {
                                                        style
                                                            .bg(rgb(0x313244))
                                                            .border_color(rgb(0x89b4fa))
                                                            .border_l_2()
                                                    },
                                                )
                                                .on_drop(cx.listener(
                                                    move |this,
                                                          drag: &DraggedAttachment,
                                                          _window,
                                                          cx| {
                                                        this.move_attachment(drag.index, i, cx);
                                                    },
                                                ))
                                                .child(
                                                    img(attachment.local_path.clone())
                                                        .size_full()
                                                        .object_fit(ObjectFit::Cover),
                                                )
                                                // Drag handle overlay
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .top(px(2.0))
                                                        .left(px(2.0))
                                                        .w(px(16.0))
                                                        .h(px(16.0))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded_full()
                                                        .bg(rgba(0x000000AA))
                                                        .text_color(rgb(0xffffff))
                                                        .cursor(gpui::CursorStyle::ClosedHand)
                                                        .text_xs()
                                                        .child("\u{283F}"),
                                                )
                                                // Remove button overlay
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "remove-attached-{}",
                                                            i
                                                        )))
                                                        .absolute()
                                                        .top(px(2.0))
                                                        .right(px(2.0))
                                                        .w(px(16.0))
                                                        .h(px(16.0))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .rounded_full()
                                                        .bg(rgba(0x000000AA))
                                                        .text_color(rgb(0xcdd6f4))
                                                        .hover(|s| {
                                                            s.bg(rgb(0xf38ba8))
                                                                .text_color(rgb(0x1e1e2e))
                                                        })
                                                        .cursor_pointer()
                                                        .text_xs()
                                                        .child("✕")
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                cx.stop_propagation();
                                                                this.remove_attachment(i, cx);
                                                            },
                                                        )),
                                                )
                                                .into_any_element()
                                        } else {
                                            // Non-image file: tag-style display
                                            div()
                                                .id(SharedString::from(format!("attached-{}", i)))
                                                .flex()
                                                .items_center()
                                                .gap(px(4.0))
                                                .text_xs()
                                                .text_color(rgb(0xa6adc8))
                                                .px(px(4.0))
                                                .py(px(1.0))
                                                .rounded(px(2.0))
                                                .bg(rgb(0x313244))
                                                .on_drag(
                                                    DraggedAttachment {
                                                        index: i,
                                                        name: drag_name,
                                                    },
                                                    |drag, _, _, cx| {
                                                        cx.stop_propagation();
                                                        cx.new(|_| drag.clone())
                                                    },
                                                )
                                                .drag_over::<DraggedAttachment>(
                                                    |style, _, _, _| {
                                                        style
                                                            .bg(rgb(0x313244))
                                                            .border_color(rgb(0x89b4fa))
                                                            .border_l_2()
                                                    },
                                                )
                                                .on_drop(cx.listener(
                                                    move |this,
                                                          drag: &DraggedAttachment,
                                                          _window,
                                                          cx| {
                                                        this.move_attachment(drag.index, i, cx);
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .text_color(rgb(0xffffff))
                                                        .cursor(gpui::CursorStyle::ClosedHand)
                                                        .child("\u{283F}"),
                                                )
                                                .child(attachment.filename.clone())
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!(
                                                            "remove-attached-{}",
                                                            i
                                                        )))
                                                        .cursor_pointer()
                                                        .text_color(rgb(0x6c7086))
                                                        .hover(|s| s.text_color(rgb(0xf38ba8)))
                                                        .child("✕")
                                                        .on_click(cx.listener(
                                                            move |this, _, _window, cx| {
                                                                this.remove_attachment(i, cx);
                                                            },
                                                        )),
                                                )
                                                .into_any_element()
                                        }
                                    },
                                )),
                        )
                    })
                    // Bottom row: Attach icon | flex space | chars | Post
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            // Attach icon
                            .child(
                                Button::new("attach-btn")
                                    .ghost()
                                    .icon(Icon::default().path("icons/paperclip.svg"))
                                    .loading(self.uploading)
                                    .when(self.poll_enabled, |btn| btn.ghost().loading(true))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.attach_file(window, cx);
                                    })),
                            )
                            // Poll toggle button
                            .child(
                                Button::new("poll-btn")
                                    .ghost()
                                    .when(self.poll_enabled, |btn| btn.selected(true))
                                    .icon(Icon::default().path("icons/bar-chart-2.svg"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.attachments.is_empty() || this.poll_enabled {
                                            this.toggle_poll(window, cx);
                                        }
                                    })),
                            )
                            // CW toggle button
                            .child(
                                Button::new("cw-btn")
                                    .ghost()
                                    .when(self.cw_enabled, |btn| btn.selected(true))
                                    .icon(IconName::TriangleAlert)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_cw(window, cx);
                                    })),
                            )
                            // Emoji picker
                            .when_some(self.emoji_picker.as_ref(), |this, picker| {
                                this.child(
                                    Popover::new("emoji-picker-popover")
                                        .anchor(Corner::TopLeft)
                                        .overlay_closable(false)
                                        .trigger(
                                            Button::new("emoji-btn")
                                                .ghost()
                                                .icon(Icon::default().path("icons/smile.svg")),
                                        )
                                        .content({
                                            let picker = picker.clone();
                                            move |_state, _window, _cx| picker.clone()
                                        }),
                                )
                            })
                            // Visibility select
                            .when_some(self.visibility_select.as_ref(), |this, vis_state| {
                                this.child(
                                    div()
                                        .w(px(100.0))
                                        .flex_shrink_0()
                                        .child(Select::new(vis_state).menu_width(px(120.0))),
                                )
                            })
                            // Spacer
                            .child(div().flex_1())
                            // Character count
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if char_count > max_chars {
                                        rgb(0xf38ba8)
                                    } else {
                                        rgb(0x6c7086)
                                    })
                                    .child(format!("{} / {}", char_count, max_chars)),
                            )
                            // Post button
                            .child(
                                Button::new("post-btn")
                                    .primary()
                                    .label(if self.edit_target.is_some() {
                                        "Edit"
                                    } else {
                                        "Post"
                                    })
                                    .loading(posting)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.post_status(window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.attachments.len() {
            self.attachments.remove(index);
            cx.notify();
        }
    }

    fn move_attachment(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.attachments.len() || to >= self.attachments.len() {
            return;
        }
        let item = self.attachments.remove(from);
        self.attachments.insert(to, item);
        cx.notify();
    }

    fn on_reply_target_changed(
        &mut self,
        target: Option<ReplyTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref target) = target {
            // Prepend @acct to compose input if replying to someone else
            let is_self_reply = self
                .session_manager
                .active_session()
                .map(|s| target.acct.strip_prefix('@').unwrap_or(&target.acct) == s.acct)
                .unwrap_or(false);

            if !is_self_reply {
                if let Some(compose_input) = &self.compose_input {
                    let mention = format!("{} ", target.acct);
                    let current_value = compose_input.read(cx).value().to_string();
                    if !current_value.starts_with(&mention) {
                        let new_value = format!("{}{}", mention, current_value);
                        let lines: Vec<&str> = new_value.split('\n').collect();
                        let last_line = lines.last().unwrap_or(&"");
                        let end_position = Position::new(
                            (lines.len() - 1) as u32,
                            last_line.chars().count() as u32,
                        );
                        compose_input.update(cx, |state, cx| {
                            state.set_value(&new_value, window, cx);
                            state.set_cursor_position(end_position, window, cx);
                        });
                    }
                }
            }

            // Set visibility to match the reply target
            if let Some(vis) = &self.visibility_select {
                let row = match target.visibility.as_str() {
                    "public" => 0,
                    "unlisted" => 1,
                    "private" => 2,
                    "direct" => 3,
                    _ => 0,
                };
                vis.update(cx, |state, cx| {
                    state.set_selected_index(
                        Some(gpui_component::IndexPath {
                            section: 0,
                            row,
                            column: 0,
                        }),
                        window,
                        cx,
                    );
                });
            }
        }
        // Clear edit mode when replying (mutual exclusion)
        if target.is_some() && self.edit_target.is_some() {
            self.edit_target = None;
            cx.set_global(EditState { target: None });
        }
        self.reply_target = target;
        cx.notify();
    }

    fn on_edit_target_changed(
        &mut self,
        target: Option<EditTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref target) = target {
            // Clear reply mode and quote mode (mutual exclusion)
            self.reply_target = None;
            cx.set_global(ReplyState { target: None });
            self.quote_target = None;
            cx.set_global(QuoteState { target: None });

            // Set source text into compose input
            if let Some(compose_input) = &self.compose_input {
                let lines: Vec<&str> = target.source_text.split('\n').collect();
                let last_line = lines.last().unwrap_or(&"");
                let end_position =
                    Position::new((lines.len() - 1) as u32, last_line.chars().count() as u32);
                compose_input.update(cx, |state, cx| {
                    state.set_value(&target.source_text, window, cx);
                    state.set_cursor_position(end_position, window, cx);
                });
            }

            // Enable CW if spoiler text exists
            if !target.spoiler_text.is_empty() {
                self.cw_enabled = true;
                if self.cw_input.is_none() {
                    self.cw_input = Some(
                        cx.new(|cx| InputState::new(window, cx).placeholder("Content warning")),
                    );
                }
                if let Some(cw_input) = &self.cw_input {
                    cw_input.update(cx, |state, cx| {
                        state.set_value(&target.spoiler_text, window, cx);
                    });
                }
            }

            // Set visibility to match original post
            if let Some(vis) = &self.visibility_select {
                let row = match target.visibility.as_str() {
                    "public" => 0,
                    "unlisted" => 1,
                    "private" => 2,
                    "direct" => 3,
                    _ => 0,
                };
                vis.update(cx, |state, cx| {
                    state.set_selected_index(
                        Some(gpui_component::IndexPath {
                            section: 0,
                            row,
                            column: 0,
                        }),
                        window,
                        cx,
                    );
                });
            }

            // Restore poll if the edited status has one
            if let Some(ref poll) = target.poll {
                self.poll_enabled = true;
                self.poll_multiple = poll.multiple;
                self.poll_options = poll
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, opt)| {
                        cx.new(|cx| {
                            let mut state = InputState::new(window, cx)
                                .placeholder(&format!("Option {}", i + 1));
                            state.set_value(&opt.title, window, cx);
                            state
                        })
                    })
                    .collect();
                if self.poll_duration_select.is_none() {
                    let items: Vec<&'static str> = POLL_DURATION_LABELS.to_vec();
                    self.poll_duration_select = Some(cx.new(|cx| {
                        SelectState::new(
                            items,
                            Some(gpui_component::IndexPath {
                                section: 0,
                                row: 5, // Default: 1日
                                column: 0,
                            }),
                            window,
                            cx,
                        )
                    }));
                }
            } else {
                self.poll_enabled = false;
                self.poll_options.clear();
                self.poll_multiple = false;
                self.poll_duration_select = None;
            }

            // Focus compose input
            if let Some(input) = &self.compose_input {
                input.update(cx, |state, cx| {
                    state.focus(window, cx);
                });
            }
        }
        self.edit_target = target;
        cx.notify();
    }

    fn cancel_reply(&mut self, cx: &mut Context<Self>) {
        self.reply_target = None;
        cx.set_global(ReplyState { target: None });
        cx.notify();
    }

    fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_target = None;
        cx.set_global(EditState { target: None });
        // Clear compose input
        if let Some(compose_input) = &self.compose_input {
            compose_input.update(cx, |state, cx| {
                state.set_value("", window, cx);
            });
        }
        // Reset CW
        self.cw_enabled = false;
        self.cw_input = None;
        // Reset poll
        self.poll_enabled = false;
        self.poll_options.clear();
        self.poll_multiple = false;
        self.poll_duration_select = None;
        // Reset visibility to Public
        if let Some(vis) = &self.visibility_select {
            vis.update(cx, |state, cx| {
                state.set_selected_index(
                    Some(gpui_component::IndexPath {
                        section: 0,
                        row: 0,
                        column: 0,
                    }),
                    window,
                    cx,
                );
            });
        }
        cx.notify();
    }

    fn on_quote_target_changed(
        &mut self,
        target: Option<QuoteTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Clear edit mode when quoting (mutual exclusion with edit)
        if target.is_some() && self.edit_target.is_some() {
            self.edit_target = None;
            cx.set_global(EditState { target: None });
        }
        self.quote_target = target;
        // Focus compose input
        if self.quote_target.is_some() {
            if let Some(input) = &self.compose_input {
                input.update(cx, |state, cx| {
                    state.focus(window, cx);
                });
            }
        }
        cx.notify();
    }

    fn cancel_quote(&mut self, cx: &mut Context<Self>) {
        self.quote_target = None;
        cx.set_global(QuoteState { target: None });
        cx.notify();
    }

    fn toggle_cw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cw_enabled = !self.cw_enabled;
        if self.cw_enabled && self.cw_input.is_none() {
            self.cw_input =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("Content warning")));
        }
        cx.notify();
    }

    fn toggle_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_enabled = !self.poll_enabled;
        if self.poll_enabled {
            // Mutual exclusion: clear attachments
            self.attachments.clear();
            // Initialize poll options (minimum 2)
            if self.poll_options.is_empty() {
                self.poll_options = vec![
                    cx.new(|cx| InputState::new(window, cx).placeholder("Option 1")),
                    cx.new(|cx| InputState::new(window, cx).placeholder("Option 2")),
                ];
            }
            if self.poll_duration_select.is_none() {
                let items: Vec<&'static str> = POLL_DURATION_LABELS.to_vec();
                self.poll_duration_select = Some(cx.new(|cx| {
                    SelectState::new(
                        items,
                        Some(gpui_component::IndexPath {
                            section: 0,
                            row: 5, // Default: 1日
                            column: 0,
                        }),
                        window,
                        cx,
                    )
                }));
            }
        } else {
            self.poll_options.clear();
            self.poll_duration_select = None;
            self.poll_multiple = false;
        }
        cx.notify();
    }

    fn add_poll_option(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.poll_options.len() < 4 {
            let index = self.poll_options.len() + 1;
            self.poll_options.push(
                cx.new(|cx| InputState::new(window, cx).placeholder(&format!("Option {}", index))),
            );
            cx.notify();
        }
    }

    fn remove_poll_option(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.poll_options.len() > 2 && index < self.poll_options.len() {
            self.poll_options.remove(index);
            cx.notify();
        }
    }

    fn handle_file_drop(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if self.uploading || self.poll_enabled {
            return;
        }
        if let Some(path) = paths.first() {
            self.uploading = true;
            cx.notify();
            self.upload_media(path.clone(), cx);
        }
    }

    fn handle_paste_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading || self.poll_enabled {
            return;
        }

        // Only process when compose input is focused
        let is_compose_focused = self
            .compose_input
            .as_ref()
            .map(|input| input.read(cx).focus_handle(cx).is_focused(window))
            .unwrap_or(false);
        if !is_compose_focused {
            return;
        }

        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        for entry in clipboard.entries() {
            if let ClipboardEntry::Image(image) = entry {
                let ext = match image.format {
                    ImageFormat::Png => "png",
                    ImageFormat::Jpeg => "jpg",
                    ImageFormat::Webp => "webp",
                    ImageFormat::Gif => "gif",
                    ImageFormat::Svg => "svg",
                    ImageFormat::Bmp => "bmp",
                    ImageFormat::Tiff => "tiff",
                };
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let filename = format!("awayuki_paste_{}.{}", timestamp, ext);
                let temp_path = std::env::temp_dir().join(&filename);

                match std::fs::write(&temp_path, &image.bytes) {
                    Ok(_) => {
                        self.uploading = true;
                        cx.notify();
                        self.upload_media(temp_path, cx);
                    }
                    Err(e) => {
                        tracing::error!("Failed to save clipboard image: {}", e);
                    }
                }
                break;
            }
        }
    }

    /// Render the bottom action bar inside the lightbox overlay.
    ///
    /// Five buttons from left to right: reply, boost, favourite, download, show post detail.
    /// Reply/boost/show-detail close the lightbox; favourite/download do not.
    fn render_lightbox_action_bar(
        &self,
        ctx_data: LightboxStatusContext,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon_color = rgb(0xcdd6f4);
        let boost_color = if ctx_data.reblogged {
            rgb(0xa6e3a1)
        } else {
            icon_color
        };
        let fav_color = if ctx_data.favourited {
            rgb(0xf9e2af)
        } else {
            icon_color
        };

        // Reply button: close lightbox and set ReplyState
        let reply_ctx = ctx_data.clone();
        let reply_btn = div()
            .id("lightbox-action-reply")
            .flex()
            .items_center()
            .justify_center()
            .w(px(40.0))
            .h(px(40.0))
            .rounded(px(20.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff33)))
            .child(
                Icon::default()
                    .path("icons/message-circle.svg")
                    .small()
                    .text_color(icon_color),
            )
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(move |_, _window, cx| {
                cx.stop_propagation();
                let target = ReplyTarget {
                    status_id: reply_ctx.api_status_id.clone(),
                    display_name: reply_ctx.display_name.clone(),
                    acct: reply_ctx.acct.clone(),
                    content: reply_ctx.content.clone(),
                    visibility: reply_ctx.visibility.clone(),
                };
                cx.set_global(LightboxState {
                    url: None,
                    local_path: None,
                    status_ctx: None,
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                });
                cx.set_global(ReplyState {
                    target: Some(target),
                });
            });

        // Boost button: close lightbox and toggle reblog
        let boost_ctx = ctx_data.clone();
        let boost_btn = div()
            .id("lightbox-action-boost")
            .flex()
            .items_center()
            .justify_center()
            .w(px(40.0))
            .h(px(40.0))
            .rounded(px(20.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff33)))
            .child(
                Icon::default()
                    .path("icons/repeat-2.svg")
                    .small()
                    .text_color(boost_color),
            )
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.lightbox_toggle_boost(boost_ctx.clone(), window, cx);
            }));

        // Favourite button: toggle without closing
        let fav_ctx = ctx_data.clone();
        let fav_btn = div()
            .id("lightbox-action-fav")
            .flex()
            .items_center()
            .justify_center()
            .w(px(40.0))
            .h(px(40.0))
            .rounded(px(20.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff33)))
            .child(Icon::new(IconName::Star).small().text_color(fav_color))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.lightbox_toggle_favourite(fav_ctx.clone(), window, cx);
            }));

        // Download button: prompt for path and save, without closing
        let download_btn = div()
            .id("lightbox-action-download")
            .flex()
            .items_center()
            .justify_center()
            .w(px(40.0))
            .h(px(40.0))
            .rounded(px(20.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff33)))
            .child(
                Icon::default()
                    .path("icons/download.svg")
                    .small()
                    .text_color(icon_color),
            )
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                let Some(url) = cx
                    .try_global::<LightboxState>()
                    .and_then(|s| s.url.clone())
                else {
                    return;
                };
                this.lightbox_download(url, window, cx);
            }));

        // Show detail button: close lightbox and open status detail panel
        let detail_status_id = ctx_data.api_status_id.clone();
        let detail_btn = div()
            .id("lightbox-action-detail")
            .flex()
            .items_center()
            .justify_center()
            .w(px(40.0))
            .h(px(40.0))
            .rounded(px(20.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgba(0xffffff33)))
            .child(
                Icon::default()
                    .path("icons/external-link.svg")
                    .small()
                    .text_color(icon_color),
            )
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(move |_, _window, cx| {
                cx.stop_propagation();
                cx.set_global(LightboxState {
                    url: None,
                    local_path: None,
                    status_ctx: None,
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                });
                cx.set_global(StatusDetailRequest {
                    status_id: Some(detail_status_id.clone()),
                });
            });

        div()
            .id("lightbox-action-bar")
            .absolute()
            .bottom(px(24.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(|_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(24.0))
                    .bg(rgba(0x00000099))
                    .child(reply_btn)
                    .child(boost_btn)
                    .child(fav_btn)
                    .child(download_btn)
                    .child(detail_btn),
            )
    }

    /// Toggle boost for a status referenced from the lightbox overlay.
    ///
    /// Closes the lightbox after dispatching. The active session's client is used
    /// to issue the reblog/unreblog call.
    fn lightbox_toggle_boost(
        &mut self,
        ctx_data: LightboxStatusContext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let api_id = ctx_data.api_status_id.clone();
        let currently_reblogged = ctx_data.reblogged;

        let task = Tokio::spawn(cx, async move {
            if currently_reblogged {
                client.unreblog(&api_id).await.map_err(|e| e.to_string())
            } else {
                client.reblog(&api_id).await.map_err(|e| e.to_string())
            }
        });

        cx.spawn(async move |_this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::error!("Lightbox reblog toggle failed: {}", e),
                Err(e) => tracing::error!("Lightbox reblog task error: {}", e),
            }
            let _ = cx.update(|cx| {
                cx.set_global(LightboxState {
                    url: None,
                    local_path: None,
                    status_ctx: None,
                    zoom: 1.0,
                    pan_x: 0.0,
                    pan_y: 0.0,
                });
            });
        })
        .detach();

        // Optimistically close the lightbox immediately
        cx.set_global(LightboxState {
            url: None,
            local_path: None,
            status_ctx: None,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        });
    }

    /// Toggle favourite for a status referenced from the lightbox overlay.
    ///
    /// Does NOT close the lightbox — updates the `LightboxState.status_ctx`
    /// so the star icon reflects the new state once the API call resolves.
    fn lightbox_toggle_favourite(
        &mut self,
        ctx_data: LightboxStatusContext,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let api_id = ctx_data.api_status_id.clone();
        let currently_favourited = ctx_data.favourited;

        let task = Tokio::spawn(cx, async move {
            if currently_favourited {
                client.unfavourite(&api_id).await.map_err(|e| e.to_string())
            } else {
                client.favourite(&api_id).await.map_err(|e| e.to_string())
            }
        });

        cx.spawn(async move |_this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(updated_status)) => {
                    let new_favourited =
                        updated_status.favourited.unwrap_or(!currently_favourited);
                    let _ = cx.update(|cx| {
                        let current = cx.try_global::<LightboxState>().cloned().unwrap_or_default();
                        // Only apply if the lightbox still points at this status
                        let still_matches = current
                            .status_ctx
                            .as_ref()
                            .map(|c| c.api_status_id == ctx_data.api_status_id)
                            .unwrap_or(false);
                        if still_matches {
                            let mut new_ctx = current.status_ctx.clone().unwrap();
                            new_ctx.favourited = new_favourited;
                            cx.set_global(LightboxState {
                                url: current.url,
                                local_path: current.local_path,
                                status_ctx: Some(new_ctx),
                                zoom: current.zoom,
                                pan_x: current.pan_x,
                                pan_y: current.pan_y,
                            });
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Lightbox favourite toggle failed: {}", e),
                Err(e) => tracing::error!("Lightbox favourite task error: {}", e),
            }
        })
        .detach();
    }

    /// Prompt for a save path and download the given image URL to that path.
    ///
    /// Does NOT close the lightbox. The download runs on the tokio runtime via
    /// `Tokio::spawn` using `reqwest` (which is already pulled in for HTTP calls).
    fn lightbox_download(&mut self, url: String, _window: &mut Window, cx: &mut Context<Self>) {
        let suggested_name = url
            .split('?')
            .next()
            .and_then(|u| u.rsplit('/').next())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "image".to_string());

        let download_dir = dirs::download_dir().unwrap_or_else(|| std::env::temp_dir());
        let receiver = cx.prompt_for_new_path(&download_dir, Some(&suggested_name));

        let fetch_task = Tokio::spawn(cx, async move {
            let bytes = reqwest::get(&url)
                .await
                .map_err(|e| e.to_string())?
                .bytes()
                .await
                .map_err(|e| e.to_string())?;
            Ok::<Vec<u8>, String>(bytes.to_vec())
        });

        cx.spawn(async move |_this: WeakEntity<Workspace>, _cx: &mut AsyncApp| {
            let chosen_path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                _ => {
                    drop(fetch_task);
                    return;
                }
            };

            match fetch_task.await {
                Ok(Ok(bytes)) => {
                    if let Err(e) = std::fs::write(&chosen_path, &bytes) {
                        tracing::error!("Failed to save downloaded image: {}", e);
                    } else {
                        tracing::info!("Saved image to {}", chosen_path.display());
                    }
                }
                Ok(Err(e)) => tracing::error!("Image download failed: {}", e),
                Err(e) => tracing::error!("Image download task error: {}", e),
            }
        })
        .detach();
    }

    fn attach_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading || self.poll_enabled {
            return;
        }

        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select a file to attach".into()),
        });

        self.uploading = true;
        cx.notify();

        // Wait for file dialog result, then trigger upload
        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| {
                let paths = match receiver.await {
                    Ok(Ok(Some(paths))) => paths,
                    _ => {
                        let _ = this.update(cx, |this, cx| {
                            this.uploading = false;
                            cx.notify();
                        });
                        return;
                    }
                };

                let Some(file_path) = paths.into_iter().next() else {
                    let _ = this.update(cx, |this, cx| {
                        this.uploading = false;
                        cx.notify();
                    });
                    return;
                };

                // Trigger upload from within workspace context (needed for Tokio::spawn)
                let _ = this.update(cx, |this, cx| {
                    this.upload_media(file_path, cx);
                });
            },
        )
        .detach();
    }

    fn upload_media(&mut self, file_path: PathBuf, cx: &mut Context<Self>) {
        let Some(session) = self.session_manager.active_session() else {
            self.uploading = false;
            cx.notify();
            return;
        };
        let client = session.client.clone();
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let is_image = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
            .unwrap_or(false);
        let local_path = file_path.clone();

        let task = Tokio::spawn(cx, async move {
            client
                .upload_media(&file_path)
                .await
                .map_err(|e| e.to_string())
        });

        cx.spawn(
            async move |this: WeakEntity<Workspace>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(media)) => {
                    tracing::info!("Media uploaded: {} ({})", media.id, filename);
                    let _ = this.update(cx, |this, cx| {
                        this.attachments.push(ComposeAttachment {
                            media_id: media.id,
                            filename,
                            local_path,
                            is_image,
                        });
                        this.uploading = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Media upload failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.uploading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Upload task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.uploading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn post_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose_input) = &self.compose_input else {
            return;
        };
        let text = compose_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        if text.is_empty() && self.attachments.is_empty() && !self.poll_enabled {
            return;
        }

        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();

        self.posting = true;
        cx.notify();

        let media_ids = if self.attachments.is_empty() {
            None
        } else {
            Some(
                self.attachments
                    .iter()
                    .map(|a| a.media_id.clone())
                    .collect(),
            )
        };

        // Get selected visibility
        let visibility = self.visibility_select.as_ref().and_then(|s| {
            s.read(cx).selected_value().map(|v| {
                match *v {
                    "Public" => "public",
                    "Unlisted" => "unlisted",
                    "Private" => "private",
                    "Direct" => "direct",
                    _ => "public",
                }
                .to_string()
            })
        });

        // Get CW text if enabled
        let spoiler_text = if self.cw_enabled {
            self.cw_input
                .as_ref()
                .map(|input| {
                    let val = input.read(cx).value().to_string().trim().to_string();
                    if val.is_empty() {
                        None
                    } else {
                        Some(val)
                    }
                })
                .flatten()
        } else {
            None
        };
        let sensitive = if spoiler_text.is_some() {
            Some(true)
        } else {
            None
        };

        let edit_status_id = self.edit_target.as_ref().map(|e| e.status_id.clone());

        // For edits, keep original media_ids if user hasn't changed attachments
        let media_ids = if let Some(ref edit_target) = self.edit_target {
            if self.attachments.is_empty() && !edit_target.media_ids.is_empty() {
                Some(edit_target.media_ids.clone())
            } else {
                media_ids
            }
        } else {
            media_ids
        };

        let in_reply_to_id = self.reply_target.as_ref().map(|r| r.status_id.clone());

        let quote_id = self.quote_target.as_ref().map(|q| q.status_id.clone())
            .or_else(|| self.edit_target.as_ref().and_then(|e| e.quote_id.clone()));

        // Build poll params if poll is enabled
        let poll = if self.poll_enabled && !self.poll_options.is_empty() {
            let options: Vec<String> = self
                .poll_options
                .iter()
                .map(|input| input.read(cx).value().to_string().trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if options.len() >= 2 {
                let duration_index = self
                    .poll_duration_select
                    .as_ref()
                    .and_then(|s| s.read(cx).selected_index(cx))
                    .map(|idx| idx.row)
                    .unwrap_or(5); // Default: 1日
                let expires_in = POLL_DURATION_SECONDS[duration_index];
                Some(CreatePollParams {
                    options,
                    expires_in,
                    multiple: if self.poll_multiple { Some(true) } else { None },
                    hide_totals: None,
                })
            } else {
                None
            }
        } else {
            None
        };

        let params = CreateStatusParams {
            status: if text.is_empty() { None } else { Some(text) },
            in_reply_to_id,
            media_ids,
            sensitive,
            spoiler_text,
            visibility,
            language: None,
            quote_id,
            poll,
        };

        let task = if let Some(status_id) = edit_status_id {
            Tokio::spawn(cx, async move {
                client
                    .edit_status(&status_id, &params)
                    .await
                    .map_err(|e| e.to_string())
            })
        } else {
            Tokio::spawn(cx, async move {
                client
                    .create_status(&params)
                    .await
                    .map_err(|e| e.to_string())
            })
        };

        cx.spawn_in(
            window,
            async move |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                match task.await {
                    Ok(Ok(_status)) => {
                        tracing::info!("Status posted/edited successfully");
                        let _ = this.update_in(cx, |this, window, cx| {
                            // Clear input by recreating it
                            this.compose_input = Some(cx.new(|cx| {
                                InputState::new(window, cx)
                                    .multi_line(true)
                                    .placeholder("What's on your mind?")
                            }));
                            this.subscribe_compose_enter(window, cx);
                            // Recreate autocomplete popup with new input
                            if let Some(ref compose_input) = this.compose_input {
                                if let Some(session) = this.session_manager.active_session() {
                                    let client = session.client.clone();
                                    let db = cx
                                        .try_global::<AppState>()
                                        .map(|s| s.database.clone())
                                        .expect("AppState should be set");
                                    let popup = cx.new(|cx| {
                                        AutocompletePopup::new(
                                            compose_input.clone(),
                                            client,
                                            db,
                                            window,
                                            cx,
                                        )
                                    });
                                    cx.observe(&popup, |_this, _popup, cx| {
                                        cx.notify();
                                    })
                                    .detach();
                                    this.autocomplete_popup = Some(popup);
                                }
                            }
                            // Recreate emoji picker with new input
                            if let Some(ref compose_input) = this.compose_input {
                                let picker = cx
                                    .new(|cx| EmojiPicker::new(compose_input.clone(), window, cx));
                                cx.observe(&picker, |_this, _picker, cx| {
                                    cx.notify();
                                })
                                .detach();
                                this.emoji_picker = Some(picker);
                            }
                            // Re-focus compose input after posting
                            if let Some(input) = &this.compose_input {
                                input.update(cx, |state, cx| {
                                    state.focus(window, cx);
                                });
                            }
                            this.attachments.clear();
                            this.cw_enabled = false;
                            this.cw_input = None;
                            this.poll_enabled = false;
                            this.poll_options.clear();
                            this.poll_multiple = false;
                            this.poll_duration_select = None;
                            this.reply_target = None;
                            this.edit_target = None;
                            this.quote_target = None;
                            cx.set_global(ReplyState { target: None });
                            cx.set_global(EditState { target: None });
                            cx.set_global(QuoteState { target: None });
                            // Reset visibility to Public
                            if let Some(vis) = &this.visibility_select {
                                vis.update(cx, |state, cx| {
                                    state.set_selected_index(
                                        Some(gpui_component::IndexPath {
                                            section: 0,
                                            row: 0,
                                            column: 0,
                                        }),
                                        window,
                                        cx,
                                    );
                                });
                            }
                            this.posting = false;
                            cx.notify();
                        });
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Failed to post/edit status: {}", e);
                        let _ = this.update_in(cx, |this, _window, cx| {
                            this.posting = false;
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!("Task error: {}", e);
                        let _ = this.update_in(cx, |this, _window, cx| {
                            this.posting = false;
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    fn on_settings_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Reload current configs and rebuild
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let acct = session.acct.clone();

        let database = cx.try_global::<AppState>().map(|s| s.database.clone());

        let Some(database) = database else { return };

        let task = Tokio::spawn(cx, async move {
            crate::db::queries::settings::get_column_configs(database.reader(), &acct)
                .await
                .unwrap_or_default()
        });

        cx.spawn_in(
            window,
            async move |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                let configs = match task.await {
                    Ok(configs) => configs,
                    Err(_) => vec![],
                };
                let _ = this.update_in(cx, |this, window, cx| {
                    let entries = configs_to_entries(&configs);
                    this.build_main_view(entries, window, cx);
                });
            },
        )
        .detach();
    }

    fn open_account_panel(
        &mut self,
        account_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let WorkspaceView::Main(dock_area) = &self.view else {
            return;
        };
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let own_id = session.account_info.id.clone();
        let dock_area = dock_area.clone();

        let panel = cx.new(|cx| AccountPanel::new(account_id, own_id, client, window, cx));
        let panel_entity_id = panel.entity_id();
        let panel_arc: Arc<dyn PanelView> = Arc::new(panel.clone());

        let tab_entity = dock_area.update(cx, |dock, cx| {
            let weak_dock = cx.entity().downgrade();
            let new_tab = DockItem::tab(panel, &weak_dock, window, cx);
            let tab_entity = match &new_tab {
                DockItem::Tabs { view, .. } => Some(view.clone()),
                _ => None,
            };

            match dock.items().clone() {
                DockItem::Split {
                    view: stack_entity, ..
                } => {
                    stack_entity.update(cx, |stack, cx| {
                        stack.add_panel(new_tab.view(), None, weak_dock, window, cx);
                    });
                }
                existing => {
                    let new_center =
                        DockItem::h_split(vec![existing, new_tab], &weak_dock, window, cx);
                    dock.set_center(new_center, window, cx);
                }
            }

            tab_entity
        });

        if let Some(tab) = tab_entity {
            self.dynamic_panels.insert(
                panel_entity_id,
                DynamicPanelEntry {
                    tab_panel: tab,
                    inner_panel: panel_arc,
                },
            );
        }
    }

    fn open_status_detail_panel(
        &mut self,
        status_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let WorkspaceView::Main(dock_area) = &self.view else {
            return;
        };
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let detail_account_id = session.account_info.id.clone();
        let dock_area = dock_area.clone();

        let panel =
            cx.new(|cx| StatusDetailPanel::new(status_id, client, detail_account_id, window, cx));
        let panel_entity_id = panel.entity_id();
        let panel_arc: Arc<dyn PanelView> = Arc::new(panel.clone());

        let tab_entity = dock_area.update(cx, |dock, cx| {
            let weak_dock = cx.entity().downgrade();
            let new_tab = DockItem::tab(panel, &weak_dock, window, cx);
            let tab_entity = match &new_tab {
                DockItem::Tabs { view, .. } => Some(view.clone()),
                _ => None,
            };

            match dock.items().clone() {
                DockItem::Split {
                    view: stack_entity, ..
                } => {
                    stack_entity.update(cx, |stack, cx| {
                        stack.add_panel(new_tab.view(), None, weak_dock, window, cx);
                    });
                }
                existing => {
                    let new_center =
                        DockItem::h_split(vec![existing, new_tab], &weak_dock, window, cx);
                    dock.set_center(new_center, window, cx);
                }
            }

            tab_entity
        });

        if let Some(tab) = tab_entity {
            self.dynamic_panels.insert(
                panel_entity_id,
                DynamicPanelEntry {
                    tab_panel: tab,
                    inner_panel: panel_arc,
                },
            );
        }
    }

    fn open_bookmarks_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let WorkspaceView::Main(dock_area) = &self.view else {
            return;
        };
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let acct = session.acct.clone();
        let bookmarks_account_id = session.account_info.id.clone();
        let Some(app_state) = cx.try_global::<AppState>() else {
            return;
        };
        let database = app_state.database.clone();
        let dock_area = dock_area.clone();

        let panel = cx.new(|cx| {
            let mut p = TimelinePanel::new(
                "Bookmarks",
                TimelineType::Bookmarks,
                client,
                acct,
                bookmarks_account_id,
                database,
                Some(100),
                Vec::new(),
                window,
                cx,
            );
            p.set_closable(true);
            p
        });
        let panel_entity_id = panel.entity_id();
        let panel_arc: Arc<dyn PanelView> = Arc::new(panel.clone());

        let tab_entity = dock_area.update(cx, |dock, cx| {
            let weak_dock = cx.entity().downgrade();
            let new_tab = DockItem::tab(panel, &weak_dock, window, cx);
            let tab_entity = match &new_tab {
                DockItem::Tabs { view, .. } => Some(view.clone()),
                _ => None,
            };

            match dock.items().clone() {
                DockItem::Split {
                    view: stack_entity, ..
                } => {
                    stack_entity.update(cx, |stack, cx| {
                        stack.add_panel(new_tab.view(), None, weak_dock, window, cx);
                    });
                }
                existing => {
                    let new_center =
                        DockItem::h_split(vec![existing, new_tab], &weak_dock, window, cx);
                    dock.set_center(new_center, window, cx);
                }
            }

            tab_entity
        });

        if let Some(tab) = tab_entity {
            self.dynamic_panels.insert(
                panel_entity_id,
                DynamicPanelEntry {
                    tab_panel: tab,
                    inner_panel: panel_arc,
                },
            );
        }
    }

    fn close_dynamic_panel(
        &mut self,
        entity_id: EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self.dynamic_panels.remove(&entity_id) {
            entry.tab_panel.update(cx, |tab, cx| {
                tab.remove_panel(entry.inner_panel, window, cx);
            });
        }
    }

    fn subscribe_compose_enter(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.compose_input {
            cx.subscribe_in(
                input,
                window,
                |this, _state, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { secondary: true } => {
                        this.post_status(window, cx);
                    }
                    InputEvent::Change => {
                        this.apply_preset_visibility(window, cx);
                    }
                    _ => {}
                },
            )
            .detach();
        }
    }

    fn apply_preset_visibility(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compose_input) = &self.compose_input else {
            return;
        };
        let Some(vis_state) = &self.visibility_select else {
            return;
        };
        let Some(settings) = cx.try_global::<PresetVisibilitySettings>() else {
            return;
        };

        let text = compose_input.read(cx).value().to_string();
        let Some(target) = settings.match_visibility(&text) else {
            return;
        };

        let current_row = vis_state.read(cx).selected_index(cx).map(|ip| ip.row);
        let current_strictness = current_row
            .and_then(|row| {
                crate::state::preset_visibility::VisibilityLevel::ALL
                    .get(row)
                    .copied()
            })
            .map(|v| v.strictness())
            .unwrap_or(0);

        if target.strictness() <= current_strictness {
            return;
        }

        let target_row = target.select_row();
        vis_state.update(cx, |state, cx| {
            state.set_selected_index(
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: target_row,
                    column: 0,
                }),
                window,
                cx,
            );
        });
    }

    fn subscribe_search_enter(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.search_input {
            cx.subscribe_in(
                input,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { secondary: false } = event {
                        let query = state.read(cx).value().to_string();
                        if !query.is_empty() {
                            this.open_search_panel(query, window, cx);
                            state.update(cx, |state, cx| {
                                state.set_value("", window, cx);
                            });
                        }
                    }
                },
            )
            .detach();
        }
    }

    fn open_search_panel(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let WorkspaceView::Main(dock_area) = &self.view else {
            return;
        };
        let Some(session) = self.session_manager.active_session() else {
            return;
        };
        let client = session.client.clone();
        let acct = session.acct.clone();
        let search_account_id = session.account_info.id.clone();
        let Some(app_state) = cx.try_global::<AppState>() else {
            return;
        };
        let database = app_state.database.clone();
        let dock_area = dock_area.clone();

        let (title, timeline_type) = if let Some(yq_query) = query.strip_prefix('?') {
            let yq_query = yq_query.trim().to_string();
            if yq_query.is_empty() {
                return;
            }
            if let Err(e) = crate::services::yq_filter::parse_expression(&yq_query) {
                tracing::error!("Invalid YQ query: {}", e);
                return;
            }
            (
                format!("YQ: {}", yq_query),
                TimelineType::YukariQuery(yq_query),
            )
        } else {
            let escaped_query = query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
                .replace('\'', "''");
            let sql = format!(
                "SELECT * FROM statuses WHERE content LIKE '%{}%' ESCAPE '\\' ORDER BY created_at DESC LIMIT 100",
                escaped_query
            );
            (format!("Search: {}", query), TimelineType::CustomSql(sql))
        };

        let panel = cx.new(|cx| {
            let mut p = TimelinePanel::new(
                title,
                timeline_type,
                client,
                acct,
                search_account_id,
                database,
                Some(100),
                Vec::new(),
                window,
                cx,
            );
            p.set_closable(true);
            p
        });
        let panel_entity_id = panel.entity_id();
        let panel_arc: Arc<dyn PanelView> = Arc::new(panel.clone());

        let tab_entity = dock_area.update(cx, |dock, cx| {
            let weak_dock = cx.entity().downgrade();
            let new_tab = DockItem::tab(panel, &weak_dock, window, cx);
            let tab_entity = match &new_tab {
                DockItem::Tabs { view, .. } => Some(view.clone()),
                _ => None,
            };

            match dock.items().clone() {
                DockItem::Split {
                    view: stack_entity, ..
                } => {
                    stack_entity.update(cx, |stack, cx| {
                        stack.add_panel(new_tab.view(), None, weak_dock, window, cx);
                    });
                }
                existing => {
                    let new_center =
                        DockItem::h_split(vec![existing, new_tab], &weak_dock, window, cx);
                    dock.set_center(new_center, window, cx);
                }
            }

            tab_entity
        });

        if let Some(tab) = tab_entity {
            self.dynamic_panels.insert(
                panel_entity_id,
                DynamicPanelEntry {
                    tab_panel: tab,
                    inner_panel: panel_arc,
                },
            );
        }
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_yq_mode = self
            .search_input
            .as_ref()
            .map(|input| input.read(cx).value().starts_with('?'))
            .unwrap_or(false);

        TitleBar::new().child(
            div()
                .flex()
                .items_center()
                .h_full()
                .flex_1()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(0xcdd6f4))
                        .child("Awayuki"),
                )
                .when_some(self.search_input.as_ref(), |el, input| {
                    el.child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .when(cfg!(target_os = "macos"), |el| el.justify_end())
                            .when(cfg!(not(target_os = "macos")), |el| el.justify_center())
                            .pr(px(8.))
                            .child(
                                Input::new(input)
                                    .appearance(true)
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .small()
                                            .text_color(rgb(0x6c7086)),
                                    )
                                    .small()
                                    .w(px(250.))
                                    .when(is_yq_mode, |el| el.border_color(rgb(0xc6a0f6))),
                            ),
                    )
                }),
        )
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Reset drag_over when no active drag
        if self.drag_over && !cx.has_active_drag() {
            self.drag_over = false;
        }

        // Process pending account detail request
        if let Some(account_id) = self.pending_account_detail.take() {
            self.open_account_panel(account_id, window, cx);
        }

        // Process pending status detail request
        if let Some(status_id) = self.pending_status_detail.take() {
            self.open_status_detail_panel(status_id, window, cx);
        }

        // Process pending close panel request
        if let Some(entity_id) = self.pending_close_panel.take() {
            self.close_dynamic_panel(entity_id, window, cx);
        }

        // Process pending bookmarks panel request
        if self.pending_bookmarks_panel {
            self.pending_bookmarks_panel = false;
            self.open_bookmarks_panel(window, cx);
        }

        // Process pending show settings request
        if self.pending_show_settings {
            self.pending_show_settings = false;
            self.show_settings(window, cx);
        }

        // Process pending account switch
        if let Some(acct) = self.pending_switch_account.take() {
            self.switch_active_account(acct, window, cx);
        }

        // Process pending add account
        if self.pending_add_account {
            self.pending_add_account = false;
            self.add_account(window, cx);
        }

        let lightbox_state = cx.try_global::<LightboxState>().cloned();
        let lightbox_source: Option<gpui::ImageSource> = lightbox_state.as_ref().and_then(|s| {
            if let Some(path) = &s.local_path {
                Some(gpui::ImageSource::from(path.clone()))
            } else {
                s.url
                    .as_ref()
                    .map(|url| gpui::ImageSource::from(url.clone()))
            }
        });

        // Query the asset cache for the image's natural size (in logical pixels).
        // Returns `None` until the image finishes loading; in that case we fall back
        // to viewport-sized rendering. Once available we can compute a proper initial
        // "fit or 100%" scale that the zoom multiplier is applied on top of.
        let lightbox_natural_size: Option<(f32, f32)> = lightbox_state.as_ref().and_then(|s| {
            let resource = if let Some(ref path) = s.local_path {
                gpui::Resource::Path(Arc::from(path.as_path()))
            } else if let Some(ref url) = s.url {
                gpui::Resource::Uri(gpui::SharedUri::from(url.clone()))
            } else {
                return None;
            };
            let data = window
                .get_asset::<gpui::ImgResourceLoader>(&resource, cx)?
                .ok()?;
            let sz = data.size(0);
            let sf = window.scale_factor();
            let w_dev: u32 = sz.width.into();
            let h_dev: u32 = sz.height.into();
            if w_dev == 0 || h_dev == 0 {
                return None;
            }
            Some((w_dev as f32 / sf, h_dev as f32 / sf))
        });

        div()
            .id("workspace-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .relative()
            .on_action(cx.listener(|this, _: &FocusCompose, window, cx| {
                if let Some(input) = &this.compose_input {
                    input.update(cx, |state, cx| {
                        state.focus(window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &SubmitPost, window, cx| {
                this.post_status(window, cx);
            }))
            .child(self.render_title_bar(cx))
            .child(div()
                .id("workspace-content")
                .flex_1()
                .flex()
                .flex_col()
                .track_focus(&self.focus_handle)
                .child(match &self.view {
                WorkspaceView::Loading(msg) => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0x6c7086))
                    .child(msg.clone())
                    .into_any_element(),
                WorkspaceView::Login(login_view) => div()
                    .size_full()
                    .child(login_view.clone())
                    .into_any_element(),
                WorkspaceView::Main(dock_area) => {
                    let sync_message = cx
                        .try_global::<BookmarkSyncState>()
                        .and_then(|s| s.message.clone());
                    let stats = cx
                        .try_global::<StatusBarStats>()
                        .cloned()
                        .unwrap_or_default();
                    let uptime = format_uptime(self.started_at.elapsed());
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        // Compose bar
                        .child(self.render_compose_bar(cx))
                        // Dock area
                        .child(div().flex_1().child(dock_area.clone()))
                        // Status bar
                        .child(
                            div()
                                .w_full()
                                .h(px(20.0))
                                .flex()
                                .items_center()
                                .px(px(8.0))
                                .bg(rgb(0x181825))
                                .border_t_1()
                                .border_color(rgb(0x313244))
                                .text_xs()
                                .text_color(rgb(0x6c7086))
                                .when_some(sync_message, |el, msg| el.child(msg))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(12.0))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(3.0))
                                                .mt(px(2.0))
                                                .child(
                                                    Icon::default()
                                                        .path("icons/database.svg")
                                                        .xsmall()
                                                        .mt(px(-3.0))
                                                        .text_color(rgb(0x6c7086)),
                                                )
                                                .font_family("monospace")
                                                .child(stats.status_count.to_string()),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(3.0))
                                                .mt(px(2.0))
                                                .child(
                                                    Icon::default()
                                                        .path("icons/activity.svg")
                                                        .xsmall()
                                                        .mt(px(-3.0))
                                                        .text_color(rgb(0x6c7086)),
                                                )
                                                .font_family("monospace")
                                                .child(stats.recent_count.to_string()),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(3.0))
                                                .mt(px(2.0))
                                                .child(
                                                    Icon::default()
                                                        .path("icons/clock.svg")
                                                        .xsmall()
                                                        .mt(px(-3.0))
                                                        .text_color(rgb(0x6c7086)),
                                                )
                                                .font_family("monospace")
                                                .child(uptime),
                                        ),
                                ),
                        )
                        .into_any_element()
                }
                WorkspaceView::Settings(settings_view) => div()
                    .size_full()
                    .child(settings_view.clone())
                    .into_any_element(),
            }))
            // Lightbox overlay (window-level)
            .when_some(lightbox_source, |el, source| {
                let action_bar_ctx = lightbox_state
                    .as_ref()
                    .and_then(|s| s.status_ctx.clone());
                let lightbox_zoom = lightbox_state.as_ref().map(|s| s.zoom).unwrap_or(1.0);
                let lightbox_pan_x =
                    lightbox_state.as_ref().map(|s| s.pan_x).unwrap_or(0.0);
                let lightbox_pan_y =
                    lightbox_state.as_ref().map(|s| s.pan_y).unwrap_or(0.0);
                let viewport = window.viewport_size();
                let vp_w = f32::from(viewport.width);
                let vp_h = f32::from(viewport.height);
                // Initial "fit or 100%" base size: if the image fits inside the viewport,
                // show at natural (100%); otherwise scale down to fit. User zoom multiplies on top.
                let (base_w, base_h) = match lightbox_natural_size {
                    Some((nat_w, nat_h)) => {
                        let fit_scale = (vp_w / nat_w).min(vp_h / nat_h).min(1.0);
                        (nat_w * fit_scale, nat_h * fit_scale)
                    }
                    None => (vp_w, vp_h),
                };
                let display_w = base_w * lightbox_zoom;
                let display_h = base_h * lightbox_zoom;
                let zoomed_box_w = px(display_w);
                let zoomed_box_h = px(display_h);
                // Absolute top-left of the image so it's centered plus user pan offset.
                let img_left = px((vp_w - display_w) / 2.0 + lightbox_pan_x);
                let img_top = px((vp_h - display_h) / 2.0 + lightbox_pan_y);
                el.child(
                    deferred(
                        div()
                            .id("lightbox-overlay")
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .overflow_hidden()
                            .bg(rgba(0x000000CC))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, event: &gpui::MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let state = cx
                                        .try_global::<LightboxState>()
                                        .cloned()
                                        .unwrap_or_default();
                                    if state.url.is_none() && state.local_path.is_none() {
                                        return;
                                    }
                                    this.lightbox_drag_start =
                                        Some((event.position, state.pan_x, state.pan_y));
                                }),
                            )
                            .on_mouse_move(cx.listener(
                                |this, event: &gpui::MouseMoveEvent, _window, cx| {
                                    let Some((start_pos, start_pan_x, start_pan_y)) =
                                        this.lightbox_drag_start
                                    else {
                                        return;
                                    };
                                    if !event.dragging() {
                                        this.lightbox_drag_start = None;
                                        return;
                                    }
                                    let dx = f32::from(event.position.x - start_pos.x);
                                    let dy = f32::from(event.position.y - start_pos.y);
                                    let current = cx
                                        .try_global::<LightboxState>()
                                        .cloned()
                                        .unwrap_or_default();
                                    cx.set_global(LightboxState {
                                        url: current.url,
                                        local_path: current.local_path,
                                        status_ctx: current.status_ctx,
                                        zoom: current.zoom,
                                        pan_x: start_pan_x + dx,
                                        pan_y: start_pan_y + dy,
                                    });
                                },
                            ))
                            .on_mouse_up(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _event: &gpui::MouseUpEvent, _window, _cx| {
                                    this.lightbox_drag_start = None;
                                }),
                            )
                            .on_click({
                                // Capture the image rect (in window coords) so we can
                                // distinguish clicks on the dimmed area from clicks on
                                // the image itself. Mouse-only clicks; ignore drag-clicks.
                                let img_left_v = f32::from(img_left);
                                let img_top_v = f32::from(img_top);
                                let img_right_v = img_left_v + display_w;
                                let img_bottom_v = img_top_v + display_h;
                                move |event: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let ClickEvent::Mouse(mouse) = event else {
                                        return;
                                    };
                                    let dx = f32::from(
                                        mouse.up.position.x - mouse.down.position.x,
                                    );
                                    let dy = f32::from(
                                        mouse.up.position.y - mouse.down.position.y,
                                    );
                                    if (dx * dx + dy * dy).sqrt() > 4.0 {
                                        return;
                                    }
                                    let cx_pt = f32::from(mouse.up.position.x);
                                    let cy_pt = f32::from(mouse.up.position.y);
                                    let inside_image = cx_pt >= img_left_v
                                        && cx_pt <= img_right_v
                                        && cy_pt >= img_top_v
                                        && cy_pt <= img_bottom_v;
                                    if inside_image {
                                        return;
                                    }
                                    cx.set_global(LightboxState {
                                        url: None,
                                        local_path: None,
                                        status_ctx: None,
                                        zoom: 1.0,
                                        pan_x: 0.0,
                                        pan_y: 0.0,
                                    });
                                }
                            })
                            .on_scroll_wheel(|event: &ScrollWheelEvent, _window, cx| {
                                cx.stop_propagation();
                                let delta_y = match event.delta {
                                    ScrollDelta::Pixels(p) => f32::from(p.y),
                                    ScrollDelta::Lines(l) => l.y * 20.0,
                                };
                                let current = cx
                                    .try_global::<LightboxState>()
                                    .cloned()
                                    .unwrap_or_default();
                                if current.url.is_none() && current.local_path.is_none() {
                                    return;
                                }
                                let new_zoom =
                                    (current.zoom * (1.0 + delta_y * 0.003)).clamp(0.1, 10.0);
                                cx.set_global(LightboxState {
                                    url: current.url,
                                    local_path: current.local_path,
                                    status_ctx: current.status_ctx,
                                    zoom: new_zoom,
                                    pan_x: current.pan_x,
                                    pan_y: current.pan_y,
                                });
                            })
                            // Loading spinner (visible until image loads and covers it)
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Spinner::new()
                                            .with_size(Size::Large)
                                            .color(hsla(0.0, 0.0, 1.0, 0.7)),
                                    ),
                            )
                            .child(
                                img(source)
                                    .absolute()
                                    .left(img_left)
                                    .top(img_top)
                                    .w(zoomed_box_w)
                                    .h(zoomed_box_h)
                                    .flex_shrink_0()
                                    .object_fit(ObjectFit::Contain)
                                    .with_loading(|| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Spinner::new()
                                                    .with_size(Size::Large)
                                                    .color(hsla(0.0, 0.0, 1.0, 0.7)),
                                            )
                                            .into_any_element()
                                    })
                                    .with_fallback({
                                        let lightbox_url =
                                            lightbox_state.as_ref().and_then(|s| s.url.clone());
                                        let prev_ctx = lightbox_state
                                            .as_ref()
                                            .and_then(|s| s.status_ctx.clone());
                                        move || {
                                            let mut el = div()
                                                .id("lightbox-reload")
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .justify_center()
                                                .gap(px(8.0))
                                                .cursor_pointer()
                                                .child(
                                                    Icon::default()
                                                        .path("icons/refresh-cw.svg")
                                                        .with_size(Size::Large)
                                                        .text_color(hsla(0.0, 0.0, 1.0, 0.7)),
                                                );
                                            if let Some(ref url) = lightbox_url {
                                                let url = url.clone();
                                                let prev_ctx = prev_ctx.clone();
                                                el = el.on_click(move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    let retry_url = format!(
                                                        "{}{}retry={}",
                                                        url,
                                                        if url.contains('?') { "&" } else { "?" },
                                                        std::time::SystemTime::now()
                                                            .duration_since(std::time::UNIX_EPOCH)
                                                            .unwrap_or_default()
                                                            .as_secs()
                                                    );
                                                    cx.set_global(LightboxState {
                                                        url: Some(retry_url),
                                                        local_path: None,
                                                        status_ctx: prev_ctx.clone(),
                                                        zoom: 1.0,
                                                        pan_x: 0.0,
                                                        pan_y: 0.0,
                                                    });
                                                });
                                            }
                                            el.into_any_element()
                                        }
                                    }),
                            )
                            // Top-left info badge: image dimensions and effective zoom percentage.
                            // Shown only once the image has loaded (natural size available).
                            .when_some(lightbox_natural_size, |el, (nat_w, nat_h)| {
                                let fit_scale = (vp_w / nat_w).min(vp_h / nat_h).min(1.0);
                                let effective_zoom_pct =
                                    (fit_scale * lightbox_zoom * 100.0).round() as i32;
                                el.child(
                                    div()
                                        .absolute()
                                        .top(px(16.0))
                                        .left(px(16.0))
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap(px(2.0))
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded(px(6.0))
                                        .bg(rgba(0x00000099))
                                        .text_color(rgb(0xcdd6f4))
                                        .font_family("monospace")
                                        .text_xs()
                                        .child(format!(
                                            "{} x {} px",
                                            nat_w.round() as i32,
                                            nat_h.round() as i32
                                        ))
                                        .child(format!("{}%", effective_zoom_pct)),
                                )
                            })
                            // Top-right buttons: [reset zoom] [close]
                            .child(
                                div()
                                    .absolute()
                                    .top(px(16.0))
                                    .right(px(16.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .id("lightbox-reset-zoom")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(40.0))
                                            .h(px(40.0))
                                            .rounded(px(20.0))
                                            .cursor_pointer()
                                            .bg(rgba(0x00000099))
                                            .hover(|s| s.bg(rgba(0xffffff33)))
                                            .child(
                                                Icon::default()
                                                    .path(if lightbox_zoom > 1.0 {
                                                        "icons/shrink.svg"
                                                    } else {
                                                        "icons/expand.svg"
                                                    })
                                                    .small()
                                                    .text_color(rgb(0xcdd6f4)),
                                            )
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                |_, _, cx| {
                                                    cx.stop_propagation();
                                                },
                                            )
                                            .on_click(|_, _, cx| {
                                                cx.stop_propagation();
                                                let current = cx
                                                    .try_global::<LightboxState>()
                                                    .cloned()
                                                    .unwrap_or_default();
                                                cx.set_global(LightboxState {
                                                    url: current.url,
                                                    local_path: current.local_path,
                                                    status_ctx: current.status_ctx,
                                                    zoom: 1.0,
                                                    pan_x: 0.0,
                                                    pan_y: 0.0,
                                                });
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("lightbox-close")
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(40.0))
                                            .h(px(40.0))
                                            .rounded(px(20.0))
                                            .cursor_pointer()
                                            .bg(rgba(0x00000099))
                                            .hover(|s| s.bg(rgba(0xffffff33)))
                                            .child(
                                                Icon::new(IconName::Close)
                                                    .small()
                                                    .text_color(rgb(0xcdd6f4)),
                                            )
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                |_, _, cx| {
                                                    cx.stop_propagation();
                                                },
                                            )
                                            .on_click(|_, _, cx| {
                                                cx.stop_propagation();
                                                cx.set_global(LightboxState {
                                                    url: None,
                                                    local_path: None,
                                                    status_ctx: None,
                                                    zoom: 1.0,
                                                    pan_x: 0.0,
                                                    pan_y: 0.0,
                                                });
                                            }),
                                    ),
                            )
                            .when_some(action_bar_ctx, |el, ctx| {
                                el.child(self.render_lightbox_action_bar(ctx, cx))
                            }),
                    )
                    .with_priority(100),
                )
            })
            // Dialog layer (for confirmation dialogs etc.)
            .children(Root::render_dialog_layer(window, cx))
    }
}

/// Convert DbColumnConfig rows to ColumnEntry for settings UI
fn configs_to_entries(configs: &[DbColumnConfig]) -> Vec<ColumnEntry> {
    configs
        .iter()
        .filter_map(|config| {
            let tl_type = TimelineType::from_column_config(
                &config.column_type,
                config.column_param.as_deref(),
            )?;
            let name = config
                .name
                .clone()
                .unwrap_or_else(|| tl_type.display_name());
            Some(ColumnEntry {
                id: config.id.clone(),
                column_type: config.column_type.clone(),
                column_param: config.column_param.clone(),
                name,
                max_statuses: config.max_statuses.map(|v| v as u32),
                pane_index: config.pane_index.unwrap_or(0) as u32,
            })
        })
        .collect()
}

/// Strip HTML tags for plain text preview
fn html_to_plain_text(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }
    result.trim().to_string()
}

fn format_uptime(elapsed: std::time::Duration) -> String {
    let total_secs = elapsed.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{}:{:02}", mins, secs)
    }
}

fn get_db_path() -> String {
    if cfg!(debug_assertions) {
        // Debug build: use current directory
        return DB_FILENAME.to_string();
    }

    // Release build: use macOS-standard location with fallbacks
    let candidates = [
        dirs::data_dir().map(|d| d.join(APP_NAME)),
        dirs::home_dir().map(|d| d.join(format!(".{}", APP_NAME))),
    ];

    for candidate in &candidates {
        if let Some(dir) = candidate {
            if std::fs::create_dir_all(dir).is_ok() {
                return dir.join(DB_FILENAME).to_string_lossy().to_string();
            }
        }
    }

    // Final fallback: current directory
    DB_FILENAME.to_string()
}
