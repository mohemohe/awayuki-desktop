use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{
    actions, deferred, div, hsla, img, px, rgb, rgba, App, AsyncApp, Context, Corner, Entity,
    EntityId, ExternalPaths, FocusHandle, Focusable, KeyDownEvent, ObjectFit, PathPromptOptions,
    SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{DockArea, DockItem, PanelView, TabPanel};
use gpui_component::input::{Input, InputEvent, InputState, Position};
use gpui_component::popover::Popover;
use gpui_component::select::{Select, SelectState};
use gpui_component::spinner::Spinner;
use gpui_component::TitleBar;
use gpui_component::Root;
use gpui_component::{Icon, IconName, Selectable, Sizable, Size};
use gpui_tokio_bridge::Tokio;

use crate::auth::session::{AccountSession, SessionManager};
use crate::constants::{APP_NAME, DB_FILENAME};
use crate::db::models::DbColumnConfig;
use crate::db::pool::Database;
use crate::mastodon::client::MastodonClient;
use crate::mastodon::endpoints::statuses::CreateStatusParams;
use crate::mastodon::types::streaming::StreamType;
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::TimelineType;
use crate::state::app_state::AppState;
use crate::state::appearance::AppearanceSettings;
use crate::state::confirmation::ConfirmationSettings;
use crate::state::performance::PerformanceSettings;
use crate::ui::components::autocomplete_popup::AutocompletePopup;
use crate::ui::components::emoji_picker::{EmojiPicker, EmojiStore};
use crate::ui::components::status_item::ReplyTarget;
use crate::ui::panels::account_panel::{AccountDetailRequest, AccountPanel};
use crate::ui::panels::status_detail_panel::{StatusDetailPanel, StatusDetailRequest};
use crate::ui::panels::timeline_panel::{LightboxState, ReplyState, TimelinePanel};
use crate::ui::views::login_view::{LoginEvent, LoginView};
use crate::ui::views::settings_view::{AccountInfo, ColumnEntry, SettingsEvent, SettingsView};

actions!(workspace, [FocusCompose, SubmitPost]);

/// Global state for requesting a panel close (bypasses DockArea lock)
#[derive(Default, Clone)]
pub struct ClosePanelRequest {
    pub entity_id: Option<EntityId>,
}
impl gpui::Global for ClosePanelRequest {}

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
    pending_account_detail: Option<String>,
    pending_status_detail: Option<String>,
    drag_over: bool,
    focus_handle: FocusHandle,
    emoji_picker: Option<Entity<EmojiPicker>>,
    autocomplete_popup: Option<Entity<AutocompletePopup>>,
    dynamic_panels: HashMap<EntityId, DynamicPanelEntry>,
    pending_close_panel: Option<EntityId>,
    search_input: Option<Entity<InputState>>,
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
            pending_account_detail: None,
            pending_status_detail: None,
            drag_over: false,
            focus_handle,
            emoji_picker: None,
            autocomplete_popup: None,
            dynamic_panels: HashMap::new(),
            pending_close_panel: None,
            search_input: None,
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

            let Some(account) = accounts.into_iter().find(|a| a.is_active) else {
                return Err("No active login account".to_string());
            };

            let acct = account.acct.clone();
            let domain = account.server_domain.clone();

            if account.access_token.is_empty() {
                return Err(format!("No token for @{}", acct));
            }

            tracing::info!("Restoring session for @{}", acct);

            let streaming_url = format!("wss://{}", domain);
            let client = MastodonClient::new(&domain, account.access_token, streaming_url)
                .map_err(|e| format!("Client error: {}", e))?;

            let account_info = client
                .verify_credentials()
                .await
                .map_err(|e| format!("Token expired for @{}: {}", acct, e))?;

            Ok(AccountSession {
                acct,
                domain,
                client,
                account_info,
            })
        });

        cx.spawn_in(
            window,
            async |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| match task.await
            {
                Ok(Ok(session)) => {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.on_login_success(&session, window, cx);
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

    fn show_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login_view = cx.new(|cx| LoginView::new(window, cx));

        cx.subscribe_in(
            &login_view,
            window,
            |this, _login, event: &LoginEvent, window, cx| match event {
                LoginEvent::LoggedIn(session) => {
                    this.on_login_success(session, window, cx);
                }
            },
        )
        .detach();

        self.view = WorkspaceView::Login(login_view);
        cx.notify();
    }

    fn on_login_success(
        &mut self,
        session: &AccountSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::info!("Login successful: @{}", session.acct);

        // Add to session manager
        self.session_manager.add_session(AccountSession {
            acct: session.acct.clone(),
            domain: session.domain.clone(),
            client: session.client.clone(),
            account_info: session.account_info.clone(),
        });

        // Save login account to DB for session restoration
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
            };
            Tokio::spawn(cx, async move {
                if let Err(e) =
                    crate::db::queries::settings::upsert_login_account(db.writer(), &login_account)
                        .await
                {
                    tracing::error!("Failed to save login account: {}", e);
                }
            })
            .detach();
        }

        // Load column configs from DB and build dock
        let database = cx
            .try_global::<AppState>()
            .map(|s| s.database.clone())
            .expect("AppState should be set before login");

        let acct = session.acct.clone();
        let client_for_emoji = session.client.clone();
        let db_for_query = database.clone();
        let domain_for_instance = session.domain.clone();

        let db_for_appearance = database.clone();
        let task = Tokio::spawn(cx, async move {
            let configs =
                crate::db::queries::settings::get_column_configs(db_for_query.reader(), &acct)
                    .await
                    .unwrap_or_default();

            // Fetch instance info for max_characters
            let unauth =
                crate::mastodon::client::UnauthenticatedClient::new().map_err(|e| e.to_string())?;
            let max_chars = match unauth.get_instance(&domain_for_instance).await {
                Ok(instance) => instance.max_characters() as usize,
                Err(e) => {
                    tracing::warn!("Failed to fetch instance info: {}", e);
                    500
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

            // Fetch custom emojis
            let custom_emojis = match client_for_emoji.get_custom_emojis().await {
                Ok(emojis) => emojis,
                Err(e) => {
                    tracing::warn!("Failed to fetch custom emojis: {}", e);
                    vec![]
                }
            };

            Ok::<(Vec<_>, usize, AppearanceSettings, PerformanceSettings, ConfirmationSettings, Vec<_>), String>((
                configs,
                max_chars,
                appearance,
                performance,
                confirmation,
                custom_emojis,
            ))
        });

        let _domain = session.domain.clone();
        cx.spawn_in(
            window,
            async move |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                let (configs, max_chars, appearance, performance, confirmation, custom_emojis) =
                    match task.await {
                        Ok(Ok((configs, max_chars, appearance, performance, confirmation, custom_emojis))) => {
                            (configs, max_chars, appearance, performance, confirmation, custom_emojis)
                        }
                        _ => (
                            vec![],
                            500,
                            AppearanceSettings::default(),
                            PerformanceSettings::default(),
                            ConfirmationSettings::default(),
                            vec![],
                        ),
                    };
                let _ = this.update_in(cx, |this, window, cx| {
                    this.max_characters = max_chars;
                    cx.set_global(appearance);
                    cx.set_global(performance);
                    cx.set_global(confirmation);

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
                        InputState::new(window, cx).placeholder("Search...")
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
        let Some(session) = self.session_manager.active_session().cloned() else {
            tracing::error!("No active session when building main view");
            return;
        };

        let client = session.client.clone();
        let acct = session.acct.clone();
        let database = cx
            .try_global::<AppState>()
            .map(|s| s.database.clone())
            .expect("AppState should be set before building main view");

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
        let streaming_url = client.streaming_url.clone();
        let streaming_token = client.access_token().to_string();
        let streaming_domain = session.domain.clone();
        let streaming_db = database.clone();

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

                        let panel = cx.new(|cx| {
                            TimelinePanel::new(
                                panel_name,
                                tl_type,
                                panel_client,
                                panel_acct,
                                panel_db,
                                panel_max_statuses,
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

            Tokio::spawn(cx, async move {
                streaming_service::start_streaming(
                    url,
                    token,
                    stream_types,
                    domain,
                    db,
                    streaming_txs,
                );
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

        self.view = WorkspaceView::Main(dock_area);
        cx.notify();
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
            let configs = crate::db::queries::settings::get_column_configs(database.reader(), &acct)
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
                    let session = this.session_manager.active_session();
                    let acct = session.map(|s| s.acct.clone()).unwrap_or_default();
                    let account_info = session
                        .map(|s| AccountInfo {
                            avatar: s.account_info.avatar.clone(),
                            display_name: s.account_info.display_name.clone(),
                            acct: s.account_info.acct.clone(),
                        })
                        .unwrap_or_else(|| AccountInfo {
                            avatar: String::new(),
                            display_name: String::new(),
                            acct: String::new(),
                        });

                    let database = cx
                        .try_global::<AppState>()
                        .map(|s| s.database.clone())
                        .expect("AppState should be set before settings");

                    let appearance = cx.global::<AppearanceSettings>().clone();
                    let performance = cx.global::<PerformanceSettings>().clone();
                    let confirmation = cx.global::<ConfirmationSettings>().clone();
                    let settings_view = cx.new(|cx| {
                        SettingsView::new(
                            acct,
                            account_info,
                            database,
                            entries,
                            lists,
                            appearance,
                            performance,
                            confirmation,
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
                                SettingsEvent::Closed => {
                                    // Go back to main view with current config
                                    this.on_settings_closed(window, cx);
                                }
                                SettingsEvent::Logout => {
                                    this.logout(window, cx);
                                }
                            }
                        },
                    )
                    .detach();

                    this.view = WorkspaceView::Settings(settings_view);
                    cx.notify();
                });
            },
        )
        .detach();
    }

    fn logout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session_manager.active_session().cloned() else {
            return;
        };
        let acct = session.acct.clone();

        // Remove session from manager
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

        // Reset compose state
        self.compose_input = None;
        self.visibility_select = None;

        // Show login screen
        self.show_login(window, cx);
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

    fn render_compose_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session_manager.active_session() else {
            return div().into_any_element();
        };
        let Some(compose_input) = &self.compose_input else {
            return div().into_any_element();
        };

        let avatar_url = session.account_info.avatar.clone();
        let char_count = compose_input.read(cx).value().chars().count();
        let max_chars = self.max_characters;
        let posting = self.posting;

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
                    // Avatar
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
                    // Flex spacer
                    .child(div().flex_1())
                    // Settings icon
                    .child(
                        Button::new("settings-btn")
                            .ghost()
                            .icon(IconName::Settings)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_settings(window, cx);
                            })),
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
                                |el, popup| {
                                    el.child(deferred(popup.clone()).with_priority(1))
                                },
                            ),
                    )
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
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.attach_file(window, cx);
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
                                    .label("Post")
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
        }
        self.reply_target = target;
        cx.notify();
    }

    fn cancel_reply(&mut self, cx: &mut Context<Self>) {
        self.reply_target = None;
        cx.set_global(ReplyState { target: None });
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

    fn handle_file_drop(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        if let Some(path) = paths.first() {
            self.uploading = true;
            cx.notify();
            self.upload_media(path.clone(), cx);
        }
    }

    fn attach_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading {
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
        if text.is_empty() && self.attachments.is_empty() {
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

        let in_reply_to_id = self.reply_target.as_ref().map(|r| r.status_id.clone());

        let params = CreateStatusParams {
            status: if text.is_empty() { None } else { Some(text) },
            in_reply_to_id,
            media_ids,
            sensitive,
            spoiler_text,
            visibility,
            language: None,
        };

        let task = Tokio::spawn(cx, async move {
            client
                .create_status(&params)
                .await
                .map_err(|e| e.to_string())
        });

        cx.spawn_in(
            window,
            async move |this: WeakEntity<Workspace>, cx: &mut gpui::AsyncWindowContext| {
                match task.await {
                    Ok(Ok(_status)) => {
                        tracing::info!("Status posted successfully");
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
                                let picker = cx.new(|cx| {
                                    EmojiPicker::new(compose_input.clone(), window, cx)
                                });
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
                            this.reply_target = None;
                            cx.set_global(ReplyState { target: None });
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
                        tracing::error!("Failed to post status: {}", e);
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
        let dock_area = dock_area.clone();

        let panel = cx.new(|cx| StatusDetailPanel::new(status_id, client, window, cx));
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
                |this, _state, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { secondary: true } = event {
                        this.post_status(window, cx);
                    }
                },
            )
            .detach();
        }
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

    fn open_search_panel(
        &mut self,
        query: String,
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
        let acct = session.acct.clone();
        let Some(app_state) = cx.try_global::<AppState>() else {
            return;
        };
        let database = app_state.database.clone();
        let dock_area = dock_area.clone();

        let escaped_query = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
            .replace('\'', "''");
        let sql = format!(
            "SELECT * FROM statuses WHERE content LIKE '%{}%' ESCAPE '\\' ORDER BY created_at DESC LIMIT 100",
            escaped_query
        );
        let title = format!("Search: {}", query);

        let panel = cx.new(|cx| {
            TimelinePanel::new(
                title,
                TimelineType::CustomSql(sql),
                client,
                acct,
                database,
                Some(100),
                window,
                cx,
            )
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

    fn render_title_bar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
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
                                    .w(px(250.)),
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

        div()
            .id("workspace-root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .relative()
            .track_focus(&self.focus_handle)
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
                WorkspaceView::Main(dock_area) => div()
                    .size_full()
                    .flex()
                    .flex_col()
                    // Compose bar
                    .child(self.render_compose_bar(cx))
                    // Dock area
                    .child(div().flex_1().child(dock_area.clone()))
                    .into_any_element(),
                WorkspaceView::Settings(settings_view) => div()
                    .size_full()
                    .child(settings_view.clone())
                    .into_any_element(),
            })
            // Lightbox overlay (window-level)
            .when_some(lightbox_source, |el, source| {
                el.child(
                    deferred(
                        div()
                            .id("lightbox-overlay")
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgba(0x000000CC))
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(|_, _, cx| {
                                cx.stop_propagation();
                                cx.set_global(LightboxState {
                                    url: None,
                                    local_path: None,
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
                                    .max_w_full()
                                    .max_h_full()
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
                                                    });
                                                });
                                            }
                                            el.into_any_element()
                                        }
                                    }),
                            ),
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
