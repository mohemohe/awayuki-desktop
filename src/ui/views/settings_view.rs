use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ObjectFit, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::WindowExt;
use gpui_tokio_bridge::Tokio;

use crate::db::pool::Database;
use crate::state::appearance::{
    AppearanceSettings, AvatarShape, CwBehavior, FontSize, NsfwBehavior,
};
use crate::state::performance::{PerformanceSettings, SuggestionSource};

const SCHEMA_TEXT: &str = "\
statuses: id, server_domain, uri, url, created_at, edited_at, account_id,
  content, visibility, sensitive, spoiler_text, reblogs_count,
  favourites_count, replies_count, in_reply_to_id, reblog_of_id,
  language, poll_json, card_json, media_attachments_json

accounts: id, server_domain, username, acct, display_name, note,
  avatar, locked, bot, followers_count, following_count, statuses_count

timeline_entries: id, timeline_type, server_domain, status_id,
  account_acct, position_at";

/// Events emitted by the settings view
pub enum SettingsEvent {
    /// Settings saved with updated column configurations
    ConfigSaved(Vec<ColumnEntry>),
    /// Appearance settings changed
    AppearanceSaved(AppearanceSettings),
    /// Performance settings changed
    PerformanceSaved(PerformanceSettings),
    /// Settings closed without changes
    Closed,
    /// User requested logout
    Logout,
}

/// Account info passed to the settings view for display
#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub avatar: String,
    pub display_name: String,
    pub acct: String,
}

#[derive(Debug, Clone, PartialEq)]
enum SelectedMenu {
    Account,
    Appearance,
    Performance,
    Timeline,
    Database,
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
    account_acct: String,
    account_info: AccountInfo,
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
    // Performance settings
    performance: PerformanceSettings,
    mention_source_select: Entity<SelectState<Vec<&'static str>>>,
    hashtag_source_select: Entity<SelectState<Vec<&'static str>>>,
    focus_handle: FocusHandle,
}

impl SettingsView {
    pub fn new(
        account_acct: String,
        account_info: AccountInfo,
        database: Arc<Database>,
        existing_columns: Vec<ColumnEntry>,
        appearance: AppearanceSettings,
        performance: PerformanceSettings,
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

        let mut view = Self {
            panes,
            selected_pane: initial_pane,
            selected_tab: initial_tab,
            selected_menu: SelectedMenu::Account,
            name_input,
            sql_input,
            max_statuses_input,
            schema_input,
            account_acct,
            account_info,
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
            performance,
            mention_source_select,
            hashtag_source_select,
            focus_handle: cx.focus_handle(),
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
            if col.column_type == "custom" {
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

        let name_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Timeline name");
            if !name_val.is_empty() {
                state = state.default_value(name_val);
            }
            state
        });

        let sql_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .multi_line(true)
                .code_editor("sql")
                .rows(8)
                .placeholder("SELECT * FROM statuses WHERE visibility = 'public'");
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
        let (name_input, sql_input, max_statuses_input) =
            Self::create_inputs_for_current(&self.selected_pane, &self.selected_tab, &self.panes, window, cx);
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

    fn add_preset(&mut self, column_type: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) {
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

    fn add_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().to_string().trim().to_string();
        let sql = self.sql_input.read(cx).value().to_string().trim().to_string();

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

    fn save_current(&mut self, cx: &mut Context<Self>) {
        if let (SelectedPane::Pane(pi), SelectedTab::Tab(ti)) = (&self.selected_pane, &self.selected_tab) {
            if let Some(col) = self.panes.get_mut(*pi).and_then(|p| p.tabs.get_mut(*ti)) {
                if col.column_type == "custom" {
                    col.name = self.name_input.read(cx).value().to_string().trim().to_string();
                    col.column_param =
                        Some(self.sql_input.read(cx).value().to_string().trim().to_string());
                    cx.notify();
                } else {
                    let val = self.max_statuses_input.read(cx).value().to_string().trim().to_string();
                    col.max_statuses = val.parse::<u32>().ok().filter(|&v| v > 0);
                    cx.notify();
                }
            }
        }
        cx.emit(SettingsEvent::ConfigSaved(panes_to_entries(&self.panes)));
    }

    fn remove_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (SelectedPane::Pane(pi), SelectedTab::Tab(ti)) = (&self.selected_pane, &self.selected_tab) {
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

        self.appearance = AppearanceSettings {
            avatar_shape: AvatarShape::ALL[avatar_idx],
            font_size: FontSize::ALL[font_idx],
            cw_behavior: CwBehavior::ALL[cw_idx],
            nsfw_behavior: NsfwBehavior::ALL[nsfw_idx],
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

        self.performance = PerformanceSettings {
            mention_source: SuggestionSource::ALL[mention_idx],
            hashtag_source: SuggestionSource::ALL[hashtag_idx],
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
            let size = crate::db::queries::settings::get_db_size(db.reader()).await.unwrap_or(0);
            let status_count = crate::db::queries::settings::get_status_count(db.reader()).await.unwrap_or(0);
            let account_count = crate::db::queries::settings::get_account_count(db.reader()).await.unwrap_or(0);
            (size, status_count, account_count)
        });

        cx.spawn_in(window, async move |this: WeakEntity<SettingsView>, cx: &mut gpui::AsyncWindowContext| {
            if let Ok((size, status_count, account_count)) = task.await {
                let _ = this.update_in(cx, |this, _window, cx| {
                    this.db_size = Some(format_bytes(size));
                    this.status_count = Some(status_count);
                    this.account_count = Some(account_count);
                    this.db_loading = false;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn run_vacuum(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.db_loading = true;
        cx.notify();
        let db = self.database.clone();
        let task = Tokio::spawn(cx, async move {
            crate::db::queries::settings::vacuum(db.writer()).await
        });

        cx.spawn_in(window, async move |this: WeakEntity<SettingsView>, cx: &mut gpui::AsyncWindowContext| {
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
        })
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
                    .child(
                        Button::new("back")
                            .label("< Back")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.close(cx);
                            })),
                    ),
            )
            // Menu items
            .child(
                div()
                    .flex_1()
                    .py(px(4.0))
                    .child(self.render_menu_item("menu-account", "Account", *selected == SelectedMenu::Account, SelectedMenu::Account, cx))
                    .child(self.render_menu_item("menu-appearance", "Appearance", *selected == SelectedMenu::Appearance, SelectedMenu::Appearance, cx))
                    .child(self.render_menu_item("menu-performance", "Performance", *selected == SelectedMenu::Performance, SelectedMenu::Performance, cx))
                    .child(self.render_menu_item("menu-timeline", "Timeline", *selected == SelectedMenu::Timeline, SelectedMenu::Timeline, cx))
                    .child(self.render_menu_item("menu-database", "Database", *selected == SelectedMenu::Database, SelectedMenu::Database, cx))
                    .child(self.render_menu_item("menu-about", "About", *selected == SelectedMenu::About, SelectedMenu::About, cx)),
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
                                DraggedPane { index: i, name: drag_name },
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
            SelectedPane::Pane(i) => self.panes.get(*i).map(|p| p.tabs.clone()).unwrap_or_default(),
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
                                DraggedTab { index: i, name: drag_name },
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Presets"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                Button::new("add-home")
                                    .label("Home")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_preset("home", "Home", window, cx);
                                    })),
                            )
                            .child(
                                Button::new("add-federated")
                                    .label("Federated")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_preset("public", "Federated", window, cx);
                                    })),
                            )
                            .child(
                                Button::new("add-notification")
                                    .label("Notification")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_preset("notification", "Notification", window, cx);
                                    })),
                            ),
                    ),
            )
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
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x6c7086))
                                    .child("Name"),
                            )
                            .child(Input::new(&self.name_input)),
                    )
                    // SQL
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x6c7086))
                                    .child("SQL Query"),
                            )
                            .child(
                                Input::new(&self.sql_input).h(px(160.)),
                            ),
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
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6c7086))
                            .child("Name"),
                    )
                    .child(Input::new(&self.name_input)),
            )
            // SQL
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x6c7086))
                            .child("SQL Query"),
                    )
                    .child(
                        Input::new(&self.sql_input).h(px(160.)),
                    ),
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

    fn render_preset_content(
        &self,
        col: ColumnEntry,
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .child("Font Size"),
                    )
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
                            .child(
                                Select::new(&self.mention_source_select).menu_width(px(200.0)),
                            ),
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
                            .child(
                                Select::new(&self.hashtag_source_select).menu_width(px(200.0)),
                            ),
                    ),
            )
    }

    fn render_account_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info = &self.account_info;
        let avatar_url = info.avatar.clone();
        let display_name = info.display_name.clone();
        let acct = format!("@{}", info.acct);

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
                    .child("Account"),
            )
            // Account info card
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .rounded(px(8.0))
                    .bg(rgb(0x181825))
                    // Avatar
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
                    // Name and acct
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgb(0xcdd6f4))
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .child(acct),
                            ),
                    ),
            )
            // Logout button
            .child(
                div().flex().child(
                    Button::new("logout-btn")
                        .label("Logout")
                        .on_click(cx.listener(|_this, _, _window, cx| {
                            cx.emit(SettingsEvent::Logout);
                        })),
                ),
            )
    }

    fn render_database_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let db_size = self.db_size.clone().unwrap_or_else(|| "Loading...".to_string());
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

    fn render_about_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .p(px(24.0))
            .gap(px(16.0))
            .child(
                div()
                    .text_lg()
                    .text_color(rgb(0xcdd6f4))
                    .child("About"),
            )
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
                    .h(px(220.)),
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
                        SelectedMenu::Performance => {
                            self.render_performance_content(cx).into_any_element()
                        }
                        SelectedMenu::Timeline => self.render_content_area(cx).into_any_element(),
                        SelectedMenu::Database => self.render_database_content(cx).into_any_element(),
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
    pane_map.into_values().map(|tabs| PaneGroup { tabs }).collect()
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
