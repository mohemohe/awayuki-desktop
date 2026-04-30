use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, AsyncApp, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ObjectFit, SharedString, Task, Timer, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{IconName, WindowExt};
use gpui_tokio_bridge::Tokio;

use crate::bluesky::rate_limit::{RateLimitSnapshot, RateLimitState};
use crate::db::pool::Database;
use crate::mastodon::types::list::List;
use crate::state::appearance::{
    AppearanceSettings, AvatarShape, CwBehavior, DisplayMode, FontSize, NsfwBehavior,
};
use crate::state::confirmation::{ConfirmationSettings, MediaSource};
use crate::state::debug_settings::{DebugSettings, LogLevel};
use crate::state::performance::{PerformanceSettings, SuggestionSource, TimelineRenderer};
use crate::state::preset_visibility::{
    PresetVisibilityEntry, PresetVisibilitySettings, VisibilityLevel,
};

const SCHEMA_TEXT: &str = "\
statuses: id, server_domain, uri, url, created_at, edited_at, account_id,
  content, visibility, sensitive, spoiler_text, reblogs_count,
  favourites_count, replies_count, in_reply_to_id, reblog_of_id,
  language, poll_json, card_json, media_attachments_json

accounts: id, server_domain, username, acct, display_name, note,
  avatar, locked, bot, followers_count, following_count, statuses_count

timeline_entries: id, timeline_type, server_domain, status_id,
  account_acct, position_at";

const YQ_REFERENCE_TEXT: &str = "\
Variables:
  text/content    - status text (plain)
  raw_content     - status text (HTML)
  visibility      - public/unlisted/private/direct
  language/lang   - language code (ja, en, ...)
  spoiler_text/cw - content warning text
  sensitive       - t/nil
  favourites_count/fav_count  - integer
  reblogs_count/boost_count   - integer
  replies_count   - integer
  bookmarked, favourited/faved   - t/nil
  reblogged/boosted, muted       - t/nil
  is_reply, is_reblog/is_boost   - t/nil
  has_media, has_poll, has_card   - t/nil
  has_cw          - t/nil
  user/username, acct             - string
  display_name    - string
  bot, locked     - t/nil
  server_domain/domain - string

Functions: and, or, not, =, !=, contains, regex
Example: (contains text \"Rust\")";

/// Events emitted by the settings view
pub enum SettingsEvent {
    /// Settings saved with updated column configurations
    ConfigSaved(Vec<ColumnEntry>),
    /// Appearance settings changed
    AppearanceSaved(AppearanceSettings),
    /// Performance settings changed
    PerformanceSaved(PerformanceSettings),
    /// Confirmation settings changed
    ConfirmationSaved(ConfirmationSettings),
    /// Preset visibility rules changed
    PresetVisibilitySaved(PresetVisibilitySettings),
    /// Debug settings changed
    DebugSaved(DebugSettings),
    /// Settings closed without changes
    Closed,
    /// User requested logout for the given login acct
    Logout(String),
    /// User requested adding a new account
    AddAccount,
    /// User requested switching the active posting account
    SwitchAccount(String),
}

/// Account info passed to the settings view for display
#[derive(Clone)]
pub struct AccountInfo {
    /// The login key (`username@domain`) used to address this account in storage
    pub acct_key: String,
    pub avatar: String,
    pub display_name: String,
    pub acct: String,
    pub is_active: bool,
    /// `Some` for Bluesky accounts: shared handle to the latest `RateLimit-*`
    /// snapshot from this account's agent. `None` for Mastodon / Misskey
    /// (those servers don't expose a uniform rate-limit header set we'd
    /// surface here). The slot's *inner* `Option` is `None` until the first
    /// rate-limited response lands.
    pub bluesky_rate_limit: Option<RateLimitState>,
}

#[derive(Debug, Clone, PartialEq)]
enum SelectedMenu {
    Account,
    Appearance,
    Behavior,
    Performance,
    Timeline,
    Database,
    Debug,
    About,
}

/// A single column configuration entry
#[derive(Debug, Clone)]
pub struct ColumnEntry {
    pub id: String,
    pub column_type: String,
    pub column_param: Option<String>,
    pub name: String,
    pub max_statuses: Option<u32>,
    pub pane_index: u32,
}

/// A group of tabs within a single pane
#[derive(Debug, Clone)]
struct PaneGroup {
    tabs: Vec<ColumnEntry>,
}

#[derive(Debug, Clone, PartialEq)]
enum SelectedPane {
    Pane(usize),
    AddNewPane,
}

#[derive(Debug, Clone, PartialEq)]
enum SelectedTab {
    Tab(usize),
    AddNew,
}

/// Drag data for tab reordering
#[derive(Debug, Clone)]
struct DraggedTab {
    index: usize,
    name: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(rgb(0x313244))
            .rounded(px(4.0))
            .text_sm()
            .text_color(rgb(0xcdd6f4))
            .opacity(0.85)
            .child(self.name.clone())
    }
}

/// Drag data for pane reordering
#[derive(Debug, Clone)]
struct DraggedPane {
    index: usize,
    name: SharedString,
}

impl Render for DraggedPane {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.0))
            .py(px(6.0))
            .bg(rgb(0x313244))
            .rounded(px(4.0))
            .text_sm()
            .text_color(rgb(0xcdd6f4))
            .opacity(0.85)
            .child(self.name.clone())
    }
}

/// UI state for a single preset visibility row (keyword + visibility pair).
struct PresetVisibilityRow {
    id: u64,
    keyword_input: Entity<InputState>,
    visibility_select: Entity<SelectState<Vec<&'static str>>>,
}

pub struct SettingsView {
    panes: Vec<PaneGroup>,
    selected_pane: SelectedPane,
    selected_tab: SelectedTab,
    selected_menu: SelectedMenu,
    // Inputs for editing custom column / adding new custom
    name_input: Entity<InputState>,
    sql_input: Entity<InputState>,
    max_statuses_input: Entity<InputState>,
    schema_input: Entity<InputState>,
    // Inputs for adding YQ timeline (separate from custom SQL inputs in AddNew tab)
    yq_name_input: Entity<InputState>,
    yq_query_input: Entity<InputState>,
    yq_reference_input: Entity<InputState>,
    accounts: Vec<AccountInfo>,
    // Database info
    database: Arc<Database>,
    db_size: Option<String>,
    status_count: Option<i64>,
    account_count: Option<i64>,
    db_loading: bool,
    // Appearance settings
    appearance: AppearanceSettings,
    avatar_shape_select: Entity<SelectState<Vec<&'static str>>>,
    font_size_select: Entity<SelectState<Vec<&'static str>>>,
    cw_behavior_select: Entity<SelectState<Vec<&'static str>>>,
    nsfw_behavior_select: Entity<SelectState<Vec<&'static str>>>,
    display_mode_select: Entity<SelectState<Vec<&'static str>>>,
    // Performance settings
    performance: PerformanceSettings,
    mention_source_select: Entity<SelectState<Vec<&'static str>>>,
    hashtag_source_select: Entity<SelectState<Vec<&'static str>>>,
    timeline_renderer_select: Entity<SelectState<Vec<&'static str>>>,
    // Confirmation settings
    confirmation: ConfirmationSettings,
    media_source_select: Entity<SelectState<Vec<&'static str>>>,
    // Preset visibility settings
    preset_visibility: PresetVisibilitySettings,
    preset_visibility_rows: Vec<PresetVisibilityRow>,
    preset_visibility_next_id: u64,
    // Debug settings
    debug: DebugSettings,
    log_level_select: Entity<SelectState<Vec<&'static str>>>,
    // List selection
    lists: Vec<List>,
    list_select: Entity<SelectState<Vec<String>>>,
    focus_handle: FocusHandle,
    /// Periodic `cx.notify()` driver that re-renders the Account pane so
    /// Bluesky rate-limit snapshots stay current. Held here so it's
    /// auto-cancelled when `SettingsView` drops. `None` when no Bluesky
    /// account is signed in — there'd be nothing to refresh.
    _rate_limit_refresh: Option<Task<()>>,
}

impl SettingsView {
    pub fn new(
        _account_acct: String,
        accounts: Vec<AccountInfo>,
        database: Arc<Database>,
        existing_columns: Vec<ColumnEntry>,
        lists: Vec<List>,
        appearance: AppearanceSettings,
        performance: PerformanceSettings,
        confirmation: ConfirmationSettings,
        preset_visibility: PresetVisibilitySettings,
        debug: DebugSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let panes = entries_to_panes(existing_columns);
        let initial_pane = if panes.is_empty() {
            SelectedPane::AddNewPane
        } else {
            SelectedPane::Pane(0)
        };
        let initial_tab = if panes.first().map_or(true, |p| p.tabs.is_empty()) {
            SelectedTab::AddNew
        } else {
            SelectedTab::Tab(0)
        };

        let (name_input, sql_input, max_statuses_input) =
            Self::create_inputs_for_current(&initial_pane, &initial_tab, &panes, window, cx);

        let schema_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .soft_wrap(false)
                .default_value(SCHEMA_TEXT)
        });

        let yq_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("YQ timeline name"));
        let yq_query_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .code_editor("scheme")
                .rows(8)
                .placeholder("(contains text \"keyword\")")
        });
        let yq_reference_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .soft_wrap(false)
                .default_value(YQ_REFERENCE_TEXT)
        });

        // Initialize appearance selects
        let avatar_items: Vec<&'static str> = AvatarShape::ALL.iter().map(|s| s.label()).collect();
        let avatar_initial = AvatarShape::ALL
            .iter()
            .position(|s| *s == appearance.avatar_shape)
            .unwrap_or(1);
        let avatar_shape_select = cx.new(|cx| {
            SelectState::new(
                avatar_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: avatar_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        let font_items: Vec<&'static str> = FontSize::ALL.iter().map(|s| s.label()).collect();
        let font_initial = FontSize::ALL
            .iter()
            .position(|s| *s == appearance.font_size)
            .unwrap_or(1);
        let font_size_select = cx.new(|cx| {
            SelectState::new(
                font_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: font_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        let cw_items: Vec<&'static str> = CwBehavior::ALL.iter().map(|s| s.label()).collect();
        let cw_initial = CwBehavior::ALL
            .iter()
            .position(|s| *s == appearance.cw_behavior)
            .unwrap_or(0);
        let cw_behavior_select = cx.new(|cx| {
            SelectState::new(
                cw_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: cw_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        let nsfw_items: Vec<&'static str> = NsfwBehavior::ALL.iter().map(|s| s.label()).collect();
        let nsfw_initial = NsfwBehavior::ALL
            .iter()
            .position(|s| *s == appearance.nsfw_behavior)
            .unwrap_or(0);
        let nsfw_behavior_select = cx.new(|cx| {
            SelectState::new(
                nsfw_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: nsfw_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        // Initialize display mode select
        let display_mode_items: Vec<&'static str> =
            DisplayMode::ALL.iter().map(|s| s.label()).collect();
        let display_mode_initial = DisplayMode::ALL
            .iter()
            .position(|s| *s == appearance.display_mode)
            .unwrap_or(0);
        let display_mode_select = cx.new(|cx| {
            SelectState::new(
                display_mode_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: display_mode_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        // Initialize performance selects
        let mention_items: Vec<&'static str> =
            SuggestionSource::ALL.iter().map(|s| s.label()).collect();
        let mention_initial = SuggestionSource::ALL
            .iter()
            .position(|s| *s == performance.mention_source)
            .unwrap_or(0);
        let mention_source_select = cx.new(|cx| {
            SelectState::new(
                mention_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: mention_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        let hashtag_items: Vec<&'static str> =
            SuggestionSource::ALL.iter().map(|s| s.label()).collect();
        let hashtag_initial = SuggestionSource::ALL
            .iter()
            .position(|s| *s == performance.hashtag_source)
            .unwrap_or(0);
        let hashtag_source_select = cx.new(|cx| {
            SelectState::new(
                hashtag_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: hashtag_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        let renderer_items: Vec<&'static str> =
            TimelineRenderer::ALL.iter().map(|r| r.label()).collect();
        let renderer_initial = TimelineRenderer::ALL
            .iter()
            .position(|r| *r == performance.timeline_renderer)
            .unwrap_or(0);
        let timeline_renderer_select = cx.new(|cx| {
            SelectState::new(
                renderer_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: renderer_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        // Initialize media source select
        let media_source_items: Vec<&'static str> =
            MediaSource::ALL.iter().map(|s| s.label()).collect();
        let media_source_initial = MediaSource::ALL
            .iter()
            .position(|s| *s == confirmation.media_source)
            .unwrap_or(0);
        let media_source_select = cx.new(|cx| {
            SelectState::new(
                media_source_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: media_source_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        // Initialize log level select
        let log_level_items: Vec<&'static str> =
            LogLevel::ALL.iter().map(|l| l.label()).collect();
        let log_level_initial = LogLevel::ALL
            .iter()
            .position(|l| *l == debug.log_level)
            .unwrap_or(2);
        let log_level_select = cx.new(|cx| {
            SelectState::new(
                log_level_items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: log_level_initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        // Initialize preset visibility rows from existing settings
        let mut preset_visibility_next_id: u64 = 0;
        let mut preset_visibility_rows: Vec<PresetVisibilityRow> = Vec::new();
        for entry in &preset_visibility.entries {
            let row = Self::build_preset_visibility_row(
                preset_visibility_next_id,
                &entry.keyword,
                entry.visibility,
                window,
                cx,
            );
            preset_visibility_next_id += 1;
            preset_visibility_rows.push(row);
        }

        // Initialize list select
        let list_titles: Vec<String> = lists.iter().map(|l| l.title.clone()).collect();
        let list_select = cx.new(|cx| SelectState::new(list_titles, None, window, cx));

        // Subscribe to select confirm events for real-time preview
        cx.subscribe(
            &avatar_shape_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_appearance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &font_size_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_appearance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &cw_behavior_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_appearance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &nsfw_behavior_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_appearance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &display_mode_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_appearance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &mention_source_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_performance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &hashtag_source_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_performance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &timeline_renderer_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_performance(cx);
            },
        )
        .detach();
        cx.subscribe(
            &media_source_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_confirmation(cx);
            },
        )
        .detach();
        cx.subscribe(
            &log_level_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_debug(cx);
            },
        )
        .detach();

        // Subscribe to existing preset visibility rows
        for row in &preset_visibility_rows {
            Self::subscribe_preset_visibility_row(row, cx);
        }

        // Spin up a periodic notifier only when there's actually a Bluesky
        // session — otherwise nothing in the pane changes between renders
        // and we'd be burning frames for no reason. The task is auto-
        // cancelled on drop because we hold it in the struct (Tasks
        // returned by `cx.spawn` cancel when dropped).
        let needs_rate_limit_refresh =
            accounts.iter().any(|a| a.bluesky_rate_limit.is_some());
        let rate_limit_refresh = if needs_rate_limit_refresh {
            Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    Timer::after(Duration::from_secs(2)).await;
                    if this
                        .update(cx, |_, cx| cx.notify())
                        .is_err()
                    {
                        break;
                    }
                }
            }))
        } else {
            None
        };

        let mut view = Self {
            panes,
            selected_pane: initial_pane,
            selected_tab: initial_tab,
            selected_menu: SelectedMenu::Account,
            name_input,
            sql_input,
            max_statuses_input,
            schema_input,
            yq_name_input,
            yq_query_input,
            yq_reference_input,
            accounts,
            database,
            db_size: None,
            status_count: None,
            account_count: None,
            db_loading: false,
            appearance,
            avatar_shape_select,
            font_size_select,
            cw_behavior_select,
            nsfw_behavior_select,
            display_mode_select,
            performance,
            mention_source_select,
            hashtag_source_select,
            timeline_renderer_select,
            confirmation,
            media_source_select,
            preset_visibility,
            preset_visibility_rows,
            preset_visibility_next_id,
            debug,
            log_level_select,
            lists,
            list_select,
            focus_handle: cx.focus_handle(),
            _rate_limit_refresh: rate_limit_refresh,
        };

        view.load_db_info(window, cx);
        view
    }

    fn create_inputs_for_current(
        pane: &SelectedPane,
        tab: &SelectedTab,
        panes: &[PaneGroup],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>, Entity<InputState>) {
        let col = match (pane, tab) {
            (SelectedPane::Pane(pi), SelectedTab::Tab(ti)) => {
                panes.get(*pi).and_then(|p| p.tabs.get(*ti))
            }
            _ => None,
        };

        let (name_val, sql_val, max_statuses_val) = if let Some(col) = col {
            if col.column_type == "custom" || col.column_type == "yq" {
                (
                    col.name.clone(),
                    col.column_param.clone().unwrap_or_default(),
                    String::new(),
                )
            } else {
                let ms = col.max_statuses.unwrap_or(100).to_string();
                (String::new(), String::new(), ms)
            }
        } else {
            (String::new(), String::new(), "100".to_string())
        };

        let is_yq = col.map_or(false, |c| c.column_type == "yq");

        let name_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Timeline name");
            if !name_val.is_empty() {
                state = state.default_value(name_val);
            }
            state
        });

        let sql_input = cx.new(|cx| {
            let placeholder = if is_yq {
                "(contains text \"keyword\")"
            } else {
                "SELECT * FROM statuses WHERE visibility = 'public'"
            };
            let mut state = InputState::new(window, cx)
                .multi_line(true)
                .rows(8)
                .placeholder(placeholder);
            if is_yq {
                state = state.code_editor("scheme");
            } else {
                state = state.code_editor("sql");
            }
            if !sql_val.is_empty() {
                state = state.default_value(sql_val);
            }
            state
        });

        let max_statuses_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("100")
                .default_value(max_statuses_val)
        });

        (name_input, sql_input, max_statuses_input)
    }

    fn refresh_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (name_input, sql_input, max_statuses_input) = Self::create_inputs_for_current(
            &self.selected_pane,
            &self.selected_tab,
            &self.panes,
            window,
            cx,
        );
        self.name_input = name_input;
        self.sql_input = sql_input;
        self.max_statuses_input = max_statuses_input;
    }

    fn select_pane(&mut self, pane_idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_pane = SelectedPane::Pane(pane_idx);
        if let Some(pane) = self.panes.get(pane_idx) {
            if pane.tabs.is_empty() {
                self.selected_tab = SelectedTab::AddNew;
            } else {
                self.selected_tab = SelectedTab::Tab(0);
            }
        }
        self.refresh_inputs(window, cx);
        cx.notify();
    }

    fn select_tab(&mut self, tab: SelectedTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_tab == tab {
            return;
        }
        self.selected_tab = tab;
        self.refresh_inputs(window, cx);
        cx.notify();
    }

    fn add_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panes.push(PaneGroup { tabs: vec![] });
        let new_idx = self.panes.len() - 1;
        self.select_pane(new_idx, window, cx);
    }

    fn remove_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let SelectedPane::Pane(i) = self.selected_pane {
            if i < self.panes.len() {
                self.panes.remove(i);
                if self.panes.is_empty() {
                    self.selected_pane = SelectedPane::AddNewPane;
                    self.selected_tab = SelectedTab::AddNew;
                    self.refresh_inputs(window, cx);
                } else {
                    let new_idx = i.min(self.panes.len() - 1);
                    self.select_pane(new_idx, window, cx);
                }
                cx.notify();
            }
        }
    }

    fn current_pane_mut(&mut self) -> Option<&mut PaneGroup> {
        if let SelectedPane::Pane(i) = self.selected_pane {
            self.panes.get_mut(i)
        } else {
            None
        }
    }

    fn add_preset(
        &mut self,
        column_type: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Ensure we have a selected pane
        if let SelectedPane::AddNewPane = self.selected_pane {
            self.panes.push(PaneGroup { tabs: vec![] });
            self.selected_pane = SelectedPane::Pane(self.panes.len() - 1);
        }

        let pane_idx = match self.selected_pane {
            SelectedPane::Pane(i) => i,
            _ => return,
        };

        let entry = ColumnEntry {
            id: uuid::Uuid::new_v4().to_string(),
            column_type: column_type.to_string(),
            column_param: None,
            name: name.to_string(),
            max_statuses: Some(100),
            pane_index: pane_idx as u32,
        };
        self.panes[pane_idx].tabs.push(entry);
        let new_idx = self.panes[pane_idx].tabs.len() - 1;
        self.select_tab(SelectedTab::Tab(new_idx), window, cx);
    }

    fn add_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.list_select.read(cx).selected_index(cx) else {
            return;
        };
        let Some(list) = self.lists.get(index.row) else {
            return;
        };

        let list_id = list.id.clone();
        let list_title = list.title.clone();

        // Ensure we have a selected pane
        if let SelectedPane::AddNewPane = self.selected_pane {
            self.panes.push(PaneGroup { tabs: vec![] });
            self.selected_pane = SelectedPane::Pane(self.panes.len() - 1);
        }

        let pane_idx = match self.selected_pane {
            SelectedPane::Pane(i) => i,
            _ => return,
        };

        let entry = ColumnEntry {
            id: uuid::Uuid::new_v4().to_string(),
            column_type: "list".to_string(),
            column_param: Some(list_id),
            name: list_title,
            max_statuses: Some(100),
            pane_index: pane_idx as u32,
        };
        self.panes[pane_idx].tabs.push(entry);
        let new_idx = self.panes[pane_idx].tabs.len() - 1;
        self.select_tab(SelectedTab::Tab(new_idx), window, cx);
    }

    fn add_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .name_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        let sql = self
            .sql_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();

        if name.is_empty() || sql.is_empty() {
            return;
        }

        // Ensure we have a selected pane
        if let SelectedPane::AddNewPane = self.selected_pane {
            self.panes.push(PaneGroup { tabs: vec![] });
            self.selected_pane = SelectedPane::Pane(self.panes.len() - 1);
        }

        let pane_idx = match self.selected_pane {
            SelectedPane::Pane(i) => i,
            _ => return,
        };

        let entry = ColumnEntry {
            id: uuid::Uuid::new_v4().to_string(),
            column_type: "custom".to_string(),
            column_param: Some(sql),
            name,
            max_statuses: None,
            pane_index: pane_idx as u32,
        };
        self.panes[pane_idx].tabs.push(entry);
        let new_idx = self.panes[pane_idx].tabs.len() - 1;
        self.select_tab(SelectedTab::Tab(new_idx), window, cx);
    }

    fn add_yq(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .yq_name_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        let query = self
            .yq_query_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();

        if name.is_empty() || query.is_empty() {
            return;
        }

        // Validate YQ query syntax
        if let Err(e) = crate::services::yq_filter::parse_expression(&query) {
            tracing::error!("Invalid YQ query: {}", e);
            return;
        }

        // Ensure we have a selected pane
        if let SelectedPane::AddNewPane = self.selected_pane {
            self.panes.push(PaneGroup { tabs: vec![] });
            self.selected_pane = SelectedPane::Pane(self.panes.len() - 1);
        }

        let pane_idx = match self.selected_pane {
            SelectedPane::Pane(i) => i,
            _ => return,
        };

        let entry = ColumnEntry {
            id: uuid::Uuid::new_v4().to_string(),
            column_type: "yq".to_string(),
            column_param: Some(query),
            name,
            max_statuses: None,
            pane_index: pane_idx as u32,
        };
        self.panes[pane_idx].tabs.push(entry);
        let new_idx = self.panes[pane_idx].tabs.len() - 1;
        self.select_tab(SelectedTab::Tab(new_idx), window, cx);
    }

    fn save_current(&mut self, cx: &mut Context<Self>) {
        if let (SelectedPane::Pane(pi), SelectedTab::Tab(ti)) =
            (&self.selected_pane, &self.selected_tab)
        {
            if let Some(col) = self.panes.get_mut(*pi).and_then(|p| p.tabs.get_mut(*ti)) {
                if col.column_type == "custom" || col.column_type == "yq" {
                    col.name = self
                        .name_input
                        .read(cx)
                        .value()
                        .to_string()
                        .trim()
                        .to_string();
                    col.column_param = Some(
                        self.sql_input
                            .read(cx)
                            .value()
                            .to_string()
                            .trim()
                            .to_string(),
                    );
                    cx.notify();
                } else {
                    let val = self
                        .max_statuses_input
                        .read(cx)
                        .value()
                        .to_string()
                        .trim()
                        .to_string();
                    col.max_statuses = val.parse::<u32>().ok().filter(|&v| v > 0);
                    cx.notify();
                }
            }
        }
        cx.emit(SettingsEvent::ConfigSaved(panes_to_entries(&self.panes)));
    }

    fn remove_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (SelectedPane::Pane(pi), SelectedTab::Tab(ti)) =
            (&self.selected_pane, &self.selected_tab)
        {
            let pi = *pi;
            let ti = *ti;
            if let Some(pane) = self.panes.get_mut(pi) {
                if ti < pane.tabs.len() {
                    pane.tabs.remove(ti);
                    if pane.tabs.is_empty() {
                        self.selected_tab = SelectedTab::AddNew;
                        self.refresh_inputs(window, cx);
                    } else {
                        let new_idx = ti.min(pane.tabs.len() - 1);
                        self.select_tab(SelectedTab::Tab(new_idx), window, cx);
                    }
                }
            }
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::ConfigSaved(panes_to_entries(&self.panes)));
    }

    fn save_appearance(&mut self, cx: &mut Context<Self>) {
        let avatar_idx = self
            .avatar_shape_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(1);
        let font_idx = self
            .font_size_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(1);
        let cw_idx = self
            .cw_behavior_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);
        let nsfw_idx = self
            .nsfw_behavior_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);

        let display_mode_idx = self
            .display_mode_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);

        self.appearance = AppearanceSettings {
            avatar_shape: AvatarShape::ALL[avatar_idx],
            font_size: FontSize::ALL[font_idx],
            cw_behavior: CwBehavior::ALL[cw_idx],
            nsfw_behavior: NsfwBehavior::ALL[nsfw_idx],
            display_mode: DisplayMode::ALL[display_mode_idx],
        };

        cx.emit(SettingsEvent::AppearanceSaved(self.appearance.clone()));
    }

    fn save_performance(&mut self, cx: &mut Context<Self>) {
        let mention_idx = self
            .mention_source_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);
        let hashtag_idx = self
            .hashtag_source_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);
        let renderer_idx = self
            .timeline_renderer_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);

        self.performance = PerformanceSettings {
            mention_source: SuggestionSource::ALL[mention_idx],
            hashtag_source: SuggestionSource::ALL[hashtag_idx],
            timeline_renderer: TimelineRenderer::ALL[renderer_idx],
        };

        cx.emit(SettingsEvent::PerformanceSaved(self.performance.clone()));
    }

    fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let SelectedPane::Pane(pi) = self.selected_pane else {
            return;
        };
        let Some(pane) = self.panes.get_mut(pi) else {
            return;
        };
        if from == to || from >= pane.tabs.len() || to >= pane.tabs.len() {
            return;
        }
        let item = pane.tabs.remove(from);
        pane.tabs.insert(to, item);
        self.selected_tab = SelectedTab::Tab(to);
        cx.notify();
    }

    fn move_pane(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.panes.len() || to >= self.panes.len() {
            return;
        }
        let item = self.panes.remove(from);
        self.panes.insert(to, item);
        self.selected_pane = SelectedPane::Pane(to);
        cx.notify();
    }

    fn select_menu(&mut self, menu: SelectedMenu, cx: &mut Context<Self>) {
        if self.selected_menu == menu {
            return;
        }
        self.selected_menu = menu;
        cx.notify();
    }

    fn load_db_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.db_loading = true;
        let db = self.database.clone();
        let task = Tokio::spawn(cx, async move {
            let size = crate::db::queries::settings::get_db_size(db.reader())
                .await
                .unwrap_or(0);
            let status_count = crate::db::queries::settings::get_status_count(db.reader())
                .await
                .unwrap_or(0);
            let account_count = crate::db::queries::settings::get_account_count(db.reader())
                .await
                .unwrap_or(0);
            (size, status_count, account_count)
        });

        cx.spawn_in(
            window,
            async move |this: WeakEntity<SettingsView>, cx: &mut gpui::AsyncWindowContext| {
                if let Ok((size, status_count, account_count)) = task.await {
                    let _ = this.update_in(cx, |this, _window, cx| {
                        this.db_size = Some(format_bytes(size));
                        this.status_count = Some(status_count);
                        this.account_count = Some(account_count);
                        this.db_loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn run_vacuum(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.db_loading = true;
        cx.notify();
        let db = self.database.clone();
        let task = Tokio::spawn(cx, async move {
            crate::db::queries::settings::vacuum(db.writer()).await
        });

        cx.spawn_in(
            window,
            async move |this: WeakEntity<SettingsView>, cx: &mut gpui::AsyncWindowContext| {
                match task.await {
                    Ok(Ok(())) => {
                        tracing::info!("VACUUM completed");
                    }
                    Ok(Err(e)) => {
                        tracing::error!("VACUUM failed: {}", e);
                    }
                    Err(e) => {
                        tracing::error!("VACUUM task error: {}", e);
                    }
                }
                let _ = this.update_in(cx, |this, window, cx| {
                    this.load_db_info(window, cx);
                });
            },
        )
        .detach();
    }

    // ── Render helpers ──────────────────────────────────────────────

    fn render_menu_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = &self.selected_menu;

        div()
            .w(px(160.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x11111b))
            .border_r_1()
            .border_color(rgb(0x313244))
            // Back button
            .child(
                div()
                    .px(px(8.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(0x313244))
                    .child(Button::new("back").label("< Back").on_click(cx.listener(
                        |this, _, _window, cx| {
                            this.close(cx);
                        },
                    ))),
            )
            // Menu items
            .child(
                div()
                    .flex_1()
                    .py(px(4.0))
                    .child(self.render_menu_item(
                        "menu-account",
                        "Account",
                        *selected == SelectedMenu::Account,
                        SelectedMenu::Account,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-appearance",
                        "Appearance",
                        *selected == SelectedMenu::Appearance,
                        SelectedMenu::Appearance,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-behavior",
                        "Behavior",
                        *selected == SelectedMenu::Behavior,
                        SelectedMenu::Behavior,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-performance",
                        "Performance",
                        *selected == SelectedMenu::Performance,
                        SelectedMenu::Performance,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-timeline",
                        "Timeline",
                        *selected == SelectedMenu::Timeline,
                        SelectedMenu::Timeline,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-database",
                        "Database",
                        *selected == SelectedMenu::Database,
                        SelectedMenu::Database,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-debug",
                        "Debug",
                        *selected == SelectedMenu::Debug,
                        SelectedMenu::Debug,
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "menu-about",
                        "About",
                        *selected == SelectedMenu::About,
                        SelectedMenu::About,
                        cx,
                    )),
            )
    }

    fn render_menu_item(
        &self,
        id: &'static str,
        label: &'static str,
        selected: bool,
        menu: SelectedMenu,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .px(px(12.0))
            .py(px(8.0))
            .text_sm()
            .cursor_pointer()
            .when(selected, |el| {
                el.bg(rgb(0x1e1e2e)).text_color(rgb(0xcdd6f4))
            })
            .when(!selected, |el| el.text_color(rgb(0x6c7086)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.select_menu(menu.clone(), cx);
            }))
            .child(label)
    }

    fn render_pane_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_pane = self.selected_pane.clone();

        div()
            .w(px(140.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x181825))
            .border_r_1()
            .border_color(rgb(0x313244))
            .child(
                div()
                    .flex_1()
                    .py(px(4.0))
                    .flex()
                    .flex_col()
                    .children(self.panes.iter().enumerate().map(|(i, pane)| {
                        let is_selected = selected_pane == SelectedPane::Pane(i);
                        let tab_count = pane.tabs.len();
                        let label: SharedString = format!("Pane {} ({})", i + 1, tab_count).into();
                        let drag_name = label.clone();
                        div()
                            .id(SharedString::from(format!("pane-{}", i)))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(8.0))
                            .py(px(8.0))
                            .text_sm()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(rgb(0x313244))
                            .when(is_selected, |el| {
                                el.bg(rgb(0x1e1e2e)).text_color(rgb(0xcdd6f4))
                            })
                            .when(!is_selected, |el| el.text_color(rgb(0xa6adc8)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_pane(i, window, cx);
                            }))
                            .on_drag(
                                DraggedPane {
                                    index: i,
                                    name: drag_name,
                                },
                                |drag, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| drag.clone())
                                },
                            )
                            .drag_over::<DraggedPane>(|style, _, _, _| {
                                style
                                    .bg(rgb(0x313244))
                                    .border_color(rgb(0x89b4fa))
                                    .border_t_2()
                            })
                            .on_drop(cx.listener(move |this, drag: &DraggedPane, _window, cx| {
                                this.move_pane(drag.index, i, cx);
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x585b70))
                                    .cursor(gpui::CursorStyle::ClosedHand)
                                    .child("\u{283F}"),
                            )
                            .child(label)
                    }))
                    // "+ Add Pane" button
                    .child(
                        div()
                            .id("pane-add")
                            .px(px(12.0))
                            .py(px(8.0))
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(0xcdd6f4))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_pane(window, cx);
                            }))
                            .child("+ Add Pane"),
                    )
                    .when(matches!(self.selected_pane, SelectedPane::Pane(_)), |el| {
                        el.child(
                            div()
                                .id("pane-remove")
                                .px(px(12.0))
                                .py(px(8.0))
                                .text_sm()
                                .cursor_pointer()
                                .text_color(rgb(0xf38ba8))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.remove_pane(window, cx);
                                }))
                                .child("- Remove Pane"),
                        )
                    }),
            )
    }

    fn render_tab_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<ColumnEntry> = match &self.selected_pane {
            SelectedPane::Pane(i) => self
                .panes
                .get(*i)
                .map(|p| p.tabs.clone())
                .unwrap_or_default(),
            SelectedPane::AddNewPane => vec![],
        };
        let selected_tab = self.selected_tab.clone();

        div()
            .w(px(160.0))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x181825))
            .border_r_1()
            .border_color(rgb(0x313244))
            .child(
                div()
                    .flex_1()
                    .py(px(4.0))
                    .flex()
                    .flex_col()
                    .children(tabs.iter().enumerate().map(|(i, col)| {
                        let name = col.name.clone();
                        let is_selected = selected_tab == SelectedTab::Tab(i);
                        let drag_name: SharedString = col.name.clone().into();
                        div()
                            .id(SharedString::from(format!("tab-{}", i)))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .px(px(8.0))
                            .py(px(8.0))
                            .text_sm()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(rgb(0x313244))
                            .when(is_selected, |el| {
                                el.bg(rgb(0x1e1e2e)).text_color(rgb(0xcdd6f4))
                            })
                            .when(!is_selected, |el| el.text_color(rgb(0xa6adc8)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_tab(SelectedTab::Tab(i), window, cx);
                            }))
                            .on_drag(
                                DraggedTab {
                                    index: i,
                                    name: drag_name,
                                },
                                |drag, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| drag.clone())
                                },
                            )
                            .drag_over::<DraggedTab>(|style, _, _, _| {
                                style
                                    .bg(rgb(0x313244))
                                    .border_color(rgb(0x89b4fa))
                                    .border_t_2()
                            })
                            .on_drop(cx.listener(move |this, drag: &DraggedTab, _window, cx| {
                                this.move_tab(drag.index, i, cx);
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x585b70))
                                    .cursor(gpui::CursorStyle::ClosedHand)
                                    .child("\u{283F}"),
                            )
                            .child(name)
                    }))
                    // "+ Add Tab" button
                    .child({
                        let is_selected = selected_tab == SelectedTab::AddNew;
                        div()
                            .id("tab-add")
                            .px(px(12.0))
                            .py(px(8.0))
                            .text_sm()
                            .cursor_pointer()
                            .when(is_selected, |el| {
                                el.bg(rgb(0x1e1e2e)).text_color(rgb(0xcdd6f4))
                            })
                            .when(!is_selected, |el| el.text_color(rgb(0x6c7086)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.select_tab(SelectedTab::AddNew, window, cx);
                            }))
                            .child("+ Add Tab")
                    }),
            )
    }

    fn render_content_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // When AddNewPane is selected or no pane is active, show add content
        if matches!(self.selected_pane, SelectedPane::AddNewPane) {
            return self.render_add_new_content(cx).into_any_element();
        }

        let col = match (&self.selected_pane, &self.selected_tab) {
            (SelectedPane::Pane(pi), SelectedTab::Tab(ti)) => {
                self.panes.get(*pi).and_then(|p| p.tabs.get(*ti)).cloned()
            }
            _ => None,
        };

        match (&self.selected_tab, col) {
            (SelectedTab::AddNew, _) => self.render_add_new_content(cx).into_any_element(),
            (SelectedTab::Tab(_), Some(col)) => {
                if col.column_type == "custom" {
                    self.render_custom_edit_content(col, cx).into_any_element()
                } else if col.column_type == "yq" {
                    self.render_yq_edit_content(col, cx).into_any_element()
                } else {
                    self.render_preset_content(col, cx).into_any_element()
                }
            }
            _ => div().into_any_element(),
        }
    }

    fn render_add_new_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .mb(px(64.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("Add Timeline Tab"),
            )
            // Preset buttons
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(div().text_sm().text_color(rgb(0xa6adc8)).child("Presets"))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(Button::new("add-home").label("Home").on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.add_preset("home", "Home", window, cx);
                                },
                            )))
                            .child(Button::new("add-federated").label("Federated").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.add_preset("public", "Federated", window, cx);
                                }),
                            ))
                            .child(
                                Button::new("add-notification")
                                    .label("Notification")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_preset("notification", "Notification", window, cx);
                                    })),
                            ),
                    ),
            )
            // List timeline
            .when(!self.lists.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .mt(px(8.0))
                        .child(div().text_sm().text_color(rgb(0xa6adc8)).child("List"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(div().flex_1().child(Select::new(&self.list_select)))
                                .child(Button::new("add-list").label("Add List").on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.add_list(window, cx);
                                    }),
                                )),
                        ),
                )
            })
            // Custom timeline form
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .mt(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Custom Timeline"),
                    )
                    // Name
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_xs().text_color(rgb(0x6c7086)).child("Name"))
                            .child(Input::new(&self.name_input)),
                    )
                    // SQL
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_xs().text_color(rgb(0x6c7086)).child("SQL Query"))
                            .child(Input::new(&self.sql_input).h(px(160.))),
                    )
                    // Add button
                    .child(
                        div().flex().justify_end().child(
                            Button::new("add-custom")
                                .label("Add Custom")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.add_custom(window, cx);
                                })),
                        ),
                    ),
            )
            // Schema reference
            .child(self.render_schema_reference())
            // YQ Timeline form
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .mt(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("YQ Timeline (Yukari Query)"),
                    )
                    // Name
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_xs().text_color(rgb(0x6c7086)).child("Name"))
                            .child(Input::new(&self.yq_name_input)),
                    )
                    // Query
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().text_xs().text_color(rgb(0x6c7086)).child("YQ Query"))
                            .child(Input::new(&self.yq_query_input).h(px(160.))),
                    )
                    // Add button
                    .child(div().flex().justify_end().child(
                        Button::new("add-yq").label("Add YQ").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.add_yq(window, cx);
                            },
                        )),
                    )),
            )
            // YQ Reference
            .child(self.render_yq_reference())
    }

    fn render_custom_edit_content(
        &self,
        _col: ColumnEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("Custom Timeline"),
            )
            // Name
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_xs().text_color(rgb(0x6c7086)).child("Name"))
                    .child(Input::new(&self.name_input)),
            )
            // SQL
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_xs().text_color(rgb(0x6c7086)).child("SQL Query"))
                    .child(Input::new(&self.sql_input).h(px(160.))),
            )
            // Buttons
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .justify_end()
                    .child(
                        Button::new("delete-column")
                            .label("Delete")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_current(window, cx);
                            })),
                    )
                    .child(
                        Button::new("save-column")
                            .label("Save")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.save_current(cx);
                            })),
                    ),
            )
            // Schema reference
            .child(self.render_schema_reference())
    }

    fn render_yq_edit_content(
        &self,
        _col: ColumnEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("YQ Timeline"),
            )
            // Name
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_xs().text_color(rgb(0x6c7086)).child("Name"))
                    .child(Input::new(&self.name_input)),
            )
            // YQ Query
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_xs().text_color(rgb(0x6c7086)).child("YQ Query"))
                    .child(Input::new(&self.sql_input).h(px(160.))),
            )
            // Buttons
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .justify_end()
                    .child(
                        Button::new("delete-yq-column")
                            .label("Delete")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_current(window, cx);
                            })),
                    )
                    .child(
                        Button::new("save-yq-column")
                            .label("Save")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.save_current(cx);
                            })),
                    ),
            )
            // YQ Reference
            .child(self.render_yq_reference())
    }

    fn render_preset_content(&self, col: ColumnEntry, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child(col.name.clone()),
            )
            // Info
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x6c7086))
                    .child(format!("Type: {}", col.column_type)),
            )
            // Max Statuses
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6c7086))
                            .child("Max Statuses"),
                    )
                    .child(
                        div()
                            .w(px(120.0))
                            .child(Input::new(&self.max_statuses_input)),
                    ),
            )
            // Buttons
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .justify_end()
                    .child(
                        Button::new("delete-preset")
                            .label("Delete")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.remove_current(window, cx);
                            })),
                    )
                    .child(
                        Button::new("save-preset")
                            .label("Save")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.save_current(cx);
                            })),
                    ),
            )
    }

    fn render_appearance_content(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("Appearance"),
            )
            // Display Mode
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Display Mode"),
                    )
                    .child(
                        div()
                            .w(px(250.0))
                            .child(Select::new(&self.display_mode_select).menu_width(px(250.0))),
                    ),
            )
            // Avatar Shape
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Avatar Shape"),
                    )
                    .child(
                        div()
                            .w(px(200.0))
                            .child(Select::new(&self.avatar_shape_select).menu_width(px(200.0))),
                    ),
            )
            // Font Size
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(div().text_sm().text_color(rgb(0xa6adc8)).child("Font Size"))
                    .child(
                        div()
                            .w(px(200.0))
                            .child(Select::new(&self.font_size_select).menu_width(px(200.0))),
                    ),
            )
            // CW Behavior
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Content Warning"),
                    )
                    .child(
                        div()
                            .w(px(250.0))
                            .child(Select::new(&self.cw_behavior_select).menu_width(px(250.0))),
                    ),
            )
            // NSFW Behavior
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("NSFW Content"),
                    )
                    .child(
                        div()
                            .w(px(250.0))
                            .child(Select::new(&self.nsfw_behavior_select).menu_width(px(250.0))),
                    ),
            )
    }

    fn render_behavior_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            .child(div().text_lg().text_color(rgb(0xcdd6f4)).child("Behavior"))
            // Media source
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Media source"),
                    )
                    .child(
                        div()
                            .w(px(250.0))
                            .child(Select::new(&self.media_source_select).menu_width(px(250.0))),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa6adc8))
                    .child("Show confirmation dialog before:"),
            )
            .child(
                Switch::new("confirm-boost")
                    .label("Boost")
                    .checked(self.confirmation.confirm_boost)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.confirmation.confirm_boost = *checked;
                        this.save_confirmation(cx);
                    })),
            )
            .child(
                Switch::new("confirm-favourite")
                    .label("Favourite")
                    .checked(self.confirmation.confirm_favourite)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.confirmation.confirm_favourite = *checked;
                        this.save_confirmation(cx);
                    })),
            )
            .child(
                Switch::new("confirm-follow")
                    .label("Follow")
                    .checked(self.confirmation.confirm_follow)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.confirmation.confirm_follow = *checked;
                        this.save_confirmation(cx);
                    })),
            )
            .child(
                Switch::new("confirm-unfollow")
                    .label("Unfollow")
                    .checked(self.confirmation.confirm_unfollow)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.confirmation.confirm_unfollow = *checked;
                        this.save_confirmation(cx);
                    })),
            )
            // Preset visibility
            .child(self.render_preset_visibility_section(cx))
    }

    fn render_preset_visibility_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut container = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .mt(px(12.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa6adc8))
                    .child("Preset visibility"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x6c7086))
                    .child("Automatically switch visibility when the post text contains a keyword. If multiple keywords match, the strictest visibility is applied."),
            );

        for (idx, row) in self.preset_visibility_rows.iter().enumerate() {
            let row_id = row.id;
            container = container.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&row.keyword_input)),
                    )
                    .child(
                        div()
                            .w(px(140.0))
                            .flex_shrink_0()
                            .child(
                                Select::new(&row.visibility_select).menu_width(px(140.0)),
                            ),
                    )
                    .child(
                        Button::new(("remove-preset-visibility", row_id as usize))
                            .ghost()
                            .icon(IconName::Delete)
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.remove_preset_visibility_row(idx, cx);
                            })),
                    ),
            );
        }

        container.child(
            div().flex().justify_start().mt(px(4.0)).child(
                Button::new("add-preset-visibility")
                    .label("Add preset")
                    .icon(IconName::Plus)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_preset_visibility_row(window, cx);
                    })),
            ),
        )
    }

    fn build_preset_visibility_row(
        id: u64,
        keyword: &str,
        visibility: VisibilityLevel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PresetVisibilityRow {
        let keyword_owned = keyword.to_string();
        let keyword_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Keyword");
            if !keyword_owned.is_empty() {
                state = state.default_value(keyword_owned);
            }
            state
        });

        let items: Vec<&'static str> =
            VisibilityLevel::ALL.iter().map(|v| v.label()).collect();
        let initial = visibility.select_row();
        let visibility_select = cx.new(|cx| {
            SelectState::new(
                items,
                Some(gpui_component::IndexPath {
                    section: 0,
                    row: initial,
                    column: 0,
                }),
                window,
                cx,
            )
        });

        PresetVisibilityRow {
            id,
            keyword_input,
            visibility_select,
        }
    }

    fn subscribe_preset_visibility_row(row: &PresetVisibilityRow, cx: &mut Context<Self>) {
        cx.subscribe(
            &row.keyword_input,
            |this: &mut SettingsView, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.save_preset_visibility(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &row.visibility_select,
            |this: &mut SettingsView, _, _: &SelectEvent<Vec<&'static str>>, cx| {
                this.save_preset_visibility(cx);
            },
        )
        .detach();
    }

    fn add_preset_visibility_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.preset_visibility_next_id;
        self.preset_visibility_next_id += 1;
        let row = Self::build_preset_visibility_row(
            id,
            "",
            VisibilityLevel::Unlisted,
            window,
            cx,
        );
        Self::subscribe_preset_visibility_row(&row, cx);
        self.preset_visibility_rows.push(row);
        self.save_preset_visibility(cx);
        cx.notify();
    }

    fn remove_preset_visibility_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.preset_visibility_rows.len() {
            return;
        }
        self.preset_visibility_rows.remove(index);
        self.save_preset_visibility(cx);
        cx.notify();
    }

    fn save_preset_visibility(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<PresetVisibilityEntry> = self
            .preset_visibility_rows
            .iter()
            .map(|row| {
                let keyword = row.keyword_input.read(cx).value().to_string();
                let idx = row
                    .visibility_select
                    .read(cx)
                    .selected_index(cx)
                    .map(|ip| ip.row)
                    .unwrap_or(0);
                let visibility = VisibilityLevel::ALL
                    .get(idx)
                    .copied()
                    .unwrap_or(VisibilityLevel::Public);
                PresetVisibilityEntry {
                    keyword,
                    visibility,
                }
            })
            .collect();

        self.preset_visibility = PresetVisibilitySettings { entries };
        cx.emit(SettingsEvent::PresetVisibilitySaved(
            self.preset_visibility.clone(),
        ));
    }

    fn save_confirmation(&mut self, cx: &mut Context<Self>) {
        let media_source_idx = self
            .media_source_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(0);
        self.confirmation.media_source = MediaSource::ALL[media_source_idx];
        cx.emit(SettingsEvent::ConfirmationSaved(self.confirmation.clone()));
    }

    fn render_performance_content(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("Performance"),
            )
            // Timeline Renderer
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Timeline Renderer"),
                    )
                    .child(
                        div().w(px(280.0)).child(
                            Select::new(&self.timeline_renderer_select).menu_width(px(280.0)),
                        ),
                    ),
            )
            // Mention Suggestion Source
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Mention Suggestion Source"),
                    )
                    .child(
                        div()
                            .w(px(200.0))
                            .child(Select::new(&self.mention_source_select).menu_width(px(200.0))),
                    ),
            )
            // Hashtag Suggestion Source
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Hashtag Suggestion Source"),
                    )
                    .child(
                        div()
                            .w(px(200.0))
                            .child(Select::new(&self.hashtag_source_select).menu_width(px(200.0))),
                    ),
            )
    }

    /// Render the rate-limit subsection inside a Bluesky account card.
    /// Shows the latest snapshot from `bluesky::xrpc::RateLimitTrackingClient`
    /// or a placeholder until the first request lands. Counts come from the
    /// IETF-draft `RateLimit-*` headers, which Bluesky returns on most XRPC
    /// reads; the limits vary per-endpoint, so callers should read this as
    /// "the bucket touched by the most recent request."
    fn render_rate_limit_section(snapshot: Option<&RateLimitSnapshot>) -> impl IntoElement {
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(rgb(0xa6adc8))
                    .child("API Rate Limit"),
            );

        let body = match snapshot {
            None => div()
                .text_xs()
                .text_color(rgb(0x6c7086))
                .child("No requests observed yet."),
            Some(snap) => {
                let now = Utc::now();

                let reset_label = format_duration_until(snap.reset_at - now);
                let observed_label = format_duration_since(now - snap.observed_at);

                let used = snap.limit.saturating_sub(snap.remaining);
                let fraction = snap.used_fraction().clamp(0.0, 1.0);
                let bar_color = if fraction >= 0.9 {
                    rgb(0xf38ba8) // red
                } else if fraction >= 0.7 {
                    rgb(0xf9e2af) // yellow
                } else {
                    rgb(0xa6e3a1) // green
                };
                let bar_pct = (fraction * 100.0) as u32;

                let primary = format!(
                    "{} / {} remaining ({} used)",
                    snap.remaining, snap.limit, used,
                );
                let meta = format!(
                    "Resets in {} • Updated {} ago{}",
                    reset_label,
                    observed_label,
                    snap.policy
                        .as_ref()
                        .map(|p| format!(" • Policy: {}", p))
                        .unwrap_or_default(),
                );

                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(div().text_sm().text_color(rgb(0xcdd6f4)).child(primary))
                    // Progress bar — fixed-height track with a coloured fill.
                    // We render the fill as a child div sized by percentage
                    // because GPUI doesn't expose a dedicated progress widget.
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(rgb(0x313244))
                            .child(
                                div()
                                    .h(px(4.0))
                                    .w(gpui::relative(bar_pct as f32 / 100.0))
                                    .rounded(px(2.0))
                                    .bg(bar_color),
                            ),
                    )
                    .child(div().text_xs().text_color(rgb(0x6c7086)).child(meta))
            }
        };

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .pt(px(8.0))
            .border_t_1()
            .border_color(rgb(0x313244))
            .child(header)
            .child(body)
    }

    fn render_account_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut container = div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(div().text_lg().text_color(rgb(0xcdd6f4)).child("Account"));

        // One card per logged-in account
        for (i, info) in self.accounts.iter().enumerate() {
            let avatar_url = info.avatar.clone();
            let display_name = info.display_name.clone();
            let acct_label = format!("@{}", info.acct);
            let acct_key_for_switch = info.acct_key.clone();
            let acct_key_for_logout = info.acct_key.clone();
            let is_active = info.is_active;

            let rate_limit_snapshot = info
                .bluesky_rate_limit
                .as_ref()
                .and_then(|s| s.try_read().ok().and_then(|g| g.clone()));

            // Card body — outer is a column so we can append a rate-limit
            // row beneath the avatar/name/buttons row when applicable.
            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .p(px(16.0))
                .rounded(px(8.0))
                .bg(rgb(0x181825));

            if is_active {
                card = card.border_1().border_color(rgb(0x89b4fa));
            }

            let header_row = div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .w(px(48.0))
                        .h(px(48.0))
                        .rounded(px(8.0))
                        .overflow_hidden()
                        .flex_shrink_0()
                        .child(
                            img(avatar_url)
                                .w(px(48.0))
                                .h(px(48.0))
                                .object_fit(ObjectFit::Cover),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .gap(px(2.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(0xcdd6f4))
                                        .child(display_name),
                                )
                                .when(is_active, |el| {
                                    el.child(
                                        div()
                                            .px(px(6.0))
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(0x89b4fa))
                                            .text_xs()
                                            .text_color(rgb(0x1e1e2e))
                                            .child("Active"),
                                    )
                                }),
                        )
                        .child(div().text_sm().text_color(rgb(0x6c7086)).child(acct_label)),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .when(!is_active, |el| {
                            el.child(
                                Button::new(SharedString::from(format!("activate-{}", i)))
                                    .label("Activate")
                                    .on_click(cx.listener(move |_this, _, _window, cx| {
                                        cx.emit(SettingsEvent::SwitchAccount(
                                            acct_key_for_switch.clone(),
                                        ));
                                    })),
                            )
                        })
                        .child(
                            Button::new(SharedString::from(format!("logout-{}", i)))
                                .danger()
                                .label("Logout")
                                .on_click(cx.listener(move |_this, _, _window, cx| {
                                    cx.emit(SettingsEvent::Logout(acct_key_for_logout.clone()));
                                })),
                        ),
                );

            card = card.child(header_row);

            if info.bluesky_rate_limit.is_some() {
                card = card.child(Self::render_rate_limit_section(rate_limit_snapshot.as_ref()));
            }

            container = container.child(card);
        }

        // Add account button
        container = container.child(
            div().flex().child(
                Button::new("add-account-btn")
                    .label("Add Account")
                    .on_click(cx.listener(|_this, _, _window, cx| {
                        cx.emit(SettingsEvent::AddAccount);
                    })),
            ),
        );

        container
    }

    fn render_database_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let db_size = self
            .db_size
            .clone()
            .unwrap_or_else(|| "Loading...".to_string());
        let status_count = self
            .status_count
            .map(|c| c.to_string())
            .unwrap_or_else(|| "Loading...".to_string());
        let account_count = self
            .account_count
            .map(|c| c.to_string())
            .unwrap_or_else(|| "Loading...".to_string());
        let loading = self.db_loading;

        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            // Title
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("Database"),
            )
            // Info cards
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(rgb(0x181825))
                    // DB size
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .w(px(130.0))
                                    .child("Database Size"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xcdd6f4))
                                    .child(db_size),
                            ),
                    )
                    // Status count
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .w(px(130.0))
                                    .child("Cached Statuses"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xcdd6f4))
                                    .child(status_count),
                            ),
                    )
                    // Account count
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .w(px(130.0))
                                    .child("Cached Accounts"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xcdd6f4))
                                    .child(account_count),
                            ),
                    ),
            )
            // Action buttons
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        Button::new("vacuum-btn")
                            .label("Vacuum")
                            .loading(loading)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_vacuum(window, cx);
                            })),
                    )
                    .child(
                        Button::new("clear-cache-btn")
                            .danger()
                            .label("Clear Cache")
                            .loading(loading)
                            .on_click(cx.listener(|_this, _, window, cx| {
                                let weak = cx.entity().downgrade();
                                window.open_dialog(cx, move |dialog, _, _| {
                                    let weak = weak.clone();
                                    dialog
                                        .confirm()
                                        .child("Are you sure you want to clear all cached statuses and accounts? This action cannot be undone.")
                                        .on_ok(move |_, _window, cx| {
                                            if let Some(entity) = weak.upgrade() {
                                                entity.update(cx, |this, cx| {
                                                    this.db_loading = true;
                                                    cx.notify();
                                                    let db = this.database.clone();
                                                    let task = Tokio::spawn(cx, async move {
                                                        crate::db::queries::settings::clear_status_cache(db.writer()).await?;
                                                        let size = crate::db::queries::settings::get_db_size(db.reader()).await.unwrap_or(0);
                                                        let sc = crate::db::queries::settings::get_status_count(db.reader()).await.unwrap_or(0);
                                                        let ac = crate::db::queries::settings::get_account_count(db.reader()).await.unwrap_or(0);
                                                        Ok::<_, sqlx::Error>((size, sc, ac))
                                                    });
                                                    cx.spawn(async move |this: WeakEntity<SettingsView>, cx: &mut gpui::AsyncApp| {
                                                        match task.await {
                                                            Ok(Ok((size, sc, ac))) => {
                                                                tracing::info!("Cache cleared");
                                                                let _ = this.update(cx, |this, cx| {
                                                                    this.db_size = Some(format_bytes(size));
                                                                    this.status_count = Some(sc);
                                                                    this.account_count = Some(ac);
                                                                    this.db_loading = false;
                                                                    cx.notify();
                                                                });
                                                            }
                                                            Ok(Err(e)) => {
                                                                tracing::error!("Clear cache failed: {}", e);
                                                                let _ = this.update(cx, |this, cx| {
                                                                    this.db_loading = false;
                                                                    cx.notify();
                                                                });
                                                            }
                                                            Err(e) => {
                                                                tracing::error!("Clear cache task error: {}", e);
                                                                let _ = this.update(cx, |this, cx| {
                                                                    this.db_loading = false;
                                                                    cx.notify();
                                                                });
                                                            }
                                                        }
                                                    }).detach();
                                                });
                                            }
                                            true
                                        })
                                });
                            })),
                    ),
            )
    }

    fn save_debug(&mut self, cx: &mut Context<Self>) {
        let log_level_idx = self
            .log_level_select
            .read(cx)
            .selected_index(cx)
            .map(|ip| ip.row)
            .unwrap_or(2);
        self.debug.log_level = LogLevel::ALL
            .get(log_level_idx)
            .copied()
            .unwrap_or(LogLevel::Info);
        cx.emit(SettingsEvent::DebugSaved(self.debug.clone()));
    }

    fn render_debug_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let log_path = crate::state::logging::log_file_path()
            .to_string_lossy()
            .to_string();

        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            .child(div().text_lg().text_color(rgb(0xcdd6f4)).child("Debug"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(rgb(0x181825))
                    .child(
                        Switch::new("debug-logging")
                            .label("Enable file logging")
                            .checked(self.debug.logging_enabled)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.debug.logging_enabled = *checked;
                                this.save_debug(cx);
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xa6adc8))
                                    .child("Log level"),
                            )
                            .child(
                                div().w(px(250.0)).child(
                                    Select::new(&self.log_level_select)
                                        .menu_width(px(250.0)),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6c7086))
                            .child(format!("Log file: {}", log_path)),
                    )
                    .child(
                        div().flex().child(
                            Button::new("open-log-file")
                                .label("Open log file")
                                .on_click(cx.listener(|_this, _, _window, _cx| {
                                    if let Err(e) =
                                        crate::state::logging::open_in_default_app()
                                    {
                                        tracing::error!("Failed to open log file: {}", e);
                                    }
                                })),
                        ),
                    ),
            )
    }

    fn render_about_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            .child(div().text_lg().text_color(rgb(0xcdd6f4)).child("About"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(rgb(0x181825))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(0xcdd6f4))
                            .child(format!("Awayuki v{}", crate::constants::APP_VERSION)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("A lightweight Mastodon client"),
                    ),
            )
            .child(
                div().flex().child(
                    Button::new("check-updates")
                        .label("Check for Updates...")
                        .on_click(cx.listener(|_this, _, _window, _cx| {
                            crate::updater::check_for_updates();
                        })),
                ),
            )
    }

    fn render_schema_reference(&self) -> impl IntoElement {
        div()
            .mt(px(8.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa6adc8))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Schema Reference:"),
            )
            .child(
                Input::new(&self.schema_input)
                    .disabled(true)
                    .appearance(false)
                    .text_color(rgb(0xa6adc8))
                    .text_xs()
                    .h(px(220.)),
            )
    }

    fn render_yq_reference(&self) -> impl IntoElement {
        div()
            .mt(px(8.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa6adc8))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("YQ Reference:"),
            )
            .child(
                Input::new(&self.yq_reference_input)
                    .disabled(true)
                    .appearance(false)
                    .text_color(rgb(0xa6adc8))
                    .text_xs()
                    .h(px(460.)),
            )
            .child(
                div()
                    .mt(px(4.0))
                    .text_sm()
                    .flex()
                    .flex_row()
                    .gap(px(4.0))
                    .child(div().text_color(rgb(0xa6adc8)).child("Original Reference:"))
                    .child(
                        div()
                            .id("yq-original-reference-link")
                            .text_color(rgb(0x89b4fa))
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(|_, _, cx| {
                                let _ = open::that(
                                    "https://github.com/shibafu528/Yukari/wiki/Yukari-Query",
                                );
                                cx.stop_propagation();
                            })
                            .child("https://github.com/shibafu528/Yukari/wiki/Yukari-Query"),
                    ),
            )
    }
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl Focusable for SettingsView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = &self.selected_menu;
        let is_timeline = *menu == SelectedMenu::Timeline;

        div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(0x1e1e2e))
            // Menu column
            .child(self.render_menu_column(cx))
            // Pane column (only shown for Timeline menu)
            .when(is_timeline, |el| el.child(self.render_pane_column(cx)))
            // Tab column (only shown for Timeline menu)
            .when(is_timeline, |el| el.child(self.render_tab_column(cx)))
            // Content area
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .h_full()
                    .overflow_y_scrollbar()
                    .child(match menu {
                        SelectedMenu::Account => self.render_account_content(cx).into_any_element(),
                        SelectedMenu::Appearance => {
                            self.render_appearance_content(cx).into_any_element()
                        }
                        SelectedMenu::Behavior => {
                            self.render_behavior_content(cx).into_any_element()
                        }
                        SelectedMenu::Performance => {
                            self.render_performance_content(cx).into_any_element()
                        }
                        SelectedMenu::Timeline => self.render_content_area(cx).into_any_element(),
                        SelectedMenu::Database => {
                            self.render_database_content(cx).into_any_element()
                        }
                        SelectedMenu::Debug => self.render_debug_content(cx).into_any_element(),
                        SelectedMenu::About => self.render_about_content(cx).into_any_element(),
                    }),
            )
    }
}

fn entries_to_panes(entries: Vec<ColumnEntry>) -> Vec<PaneGroup> {
    use std::collections::BTreeMap;
    let mut pane_map: BTreeMap<u32, Vec<ColumnEntry>> = BTreeMap::new();
    for entry in entries {
        pane_map.entry(entry.pane_index).or_default().push(entry);
    }
    pane_map
        .into_values()
        .map(|tabs| PaneGroup { tabs })
        .collect()
}

fn panes_to_entries(panes: &[PaneGroup]) -> Vec<ColumnEntry> {
    let mut entries = Vec::new();
    for (pane_idx, pane) in panes.iter().enumerate() {
        for tab in &pane.tabs {
            let mut e = tab.clone();
            e.pane_index = pane_idx as u32;
            entries.push(e);
        }
    }
    entries
}

fn format_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

/// Render a "time until X" duration, e.g. `1m 23s`. Negative inputs (i.e.
/// the deadline already passed) render as `0s` rather than going negative,
/// because the rate-limit window has already rolled over and the headers
/// will catch up on the next request.
fn format_duration_until(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    format_secs(secs as u64)
}

/// Render a "time since X" duration, e.g. `12s`. Same clamp-at-zero as
/// `format_duration_until` since the snapshot can't have been observed in
/// the future.
fn format_duration_since(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    format_secs(secs as u64)
}

fn format_secs(total: u64) -> String {
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}
