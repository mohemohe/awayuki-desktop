use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ObjectFit, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::WindowExt;
use gpui_tokio_bridge::Tokio;

use crate::db::pool::Database;

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
}

#[derive(Debug, Clone, PartialEq)]
enum SelectedTab {
    Column(usize),
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

pub struct SettingsView {
    columns: Vec<ColumnEntry>,
    selected_tab: SelectedTab,
    selected_menu: SelectedMenu,
    // Inputs for editing custom column / adding new custom
    name_input: Entity<InputState>,
    sql_input: Entity<InputState>,
    schema_input: Entity<InputState>,
    account_acct: String,
    account_info: AccountInfo,
    // Database info
    database: Arc<Database>,
    db_size: Option<String>,
    status_count: Option<i64>,
    account_count: Option<i64>,
    db_loading: bool,
    focus_handle: FocusHandle,
}

impl SettingsView {
    pub fn new(
        account_acct: String,
        account_info: AccountInfo,
        database: Arc<Database>,
        existing_columns: Vec<ColumnEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_tab = if existing_columns.is_empty() {
            SelectedTab::AddNew
        } else {
            SelectedTab::Column(0)
        };

        let (name_input, sql_input) =
            Self::create_inputs_for_tab(&initial_tab, &existing_columns, window, cx);

        let schema_input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .soft_wrap(false)
                .default_value(SCHEMA_TEXT)
        });

        let mut view = Self {
            columns: existing_columns,
            selected_tab: initial_tab,
            selected_menu: SelectedMenu::Account,
            name_input,
            sql_input,
            schema_input,
            account_acct,
            account_info,
            database,
            db_size: None,
            status_count: None,
            account_count: None,
            db_loading: false,
            focus_handle: cx.focus_handle(),
        };

        view.load_db_info(window, cx);
        view
    }

    fn create_inputs_for_tab(
        tab: &SelectedTab,
        columns: &[ColumnEntry],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Entity<InputState>) {
        let (name_val, sql_val) = match tab {
            SelectedTab::Column(i) => {
                if let Some(col) = columns.get(*i) {
                    if col.column_type == "custom" {
                        (
                            col.name.clone(),
                            col.column_param.clone().unwrap_or_default(),
                        )
                    } else {
                        (String::new(), String::new())
                    }
                } else {
                    (String::new(), String::new())
                }
            }
            SelectedTab::AddNew => (String::new(), String::new()),
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

        (name_input, sql_input)
    }

    fn select_tab(&mut self, tab: SelectedTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_tab == tab {
            return;
        }
        self.selected_tab = tab;
        let (name_input, sql_input) =
            Self::create_inputs_for_tab(&self.selected_tab, &self.columns, window, cx);
        self.name_input = name_input;
        self.sql_input = sql_input;
        cx.notify();
    }

    fn add_preset(&mut self, column_type: &str, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let entry = ColumnEntry {
            id: uuid::Uuid::new_v4().to_string(),
            column_type: column_type.to_string(),
            column_param: None,
            name: name.to_string(),
        };
        self.columns.push(entry);
        let new_idx = self.columns.len() - 1;
        self.select_tab(SelectedTab::Column(new_idx), window, cx);
    }

    fn add_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().to_string().trim().to_string();
        let sql = self.sql_input.read(cx).value().to_string().trim().to_string();

        if name.is_empty() || sql.is_empty() {
            return;
        }

        let entry = ColumnEntry {
            id: uuid::Uuid::new_v4().to_string(),
            column_type: "custom".to_string(),
            column_param: Some(sql),
            name,
        };
        self.columns.push(entry);
        let new_idx = self.columns.len() - 1;
        self.select_tab(SelectedTab::Column(new_idx), window, cx);
    }

    fn save_current(&mut self, cx: &mut Context<Self>) {
        if let SelectedTab::Column(i) = self.selected_tab {
            if let Some(col) = self.columns.get_mut(i) {
                if col.column_type == "custom" {
                    col.name = self.name_input.read(cx).value().to_string().trim().to_string();
                    col.column_param =
                        Some(self.sql_input.read(cx).value().to_string().trim().to_string());
                    cx.notify();
                }
            }
        }
        // Emit save with all columns
        cx.emit(SettingsEvent::ConfigSaved(self.columns.clone()));
    }

    fn remove_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let SelectedTab::Column(i) = self.selected_tab {
            if i < self.columns.len() {
                self.columns.remove(i);
                // Select previous tab or AddNew
                if self.columns.is_empty() {
                    self.select_tab(SelectedTab::AddNew, window, cx);
                } else {
                    let new_idx = i.min(self.columns.len() - 1);
                    self.select_tab(SelectedTab::Column(new_idx), window, cx);
                }
            }
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(SettingsEvent::ConfigSaved(self.columns.clone()));
    }

    fn move_column(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if from == to || from >= self.columns.len() || to >= self.columns.len() {
            return;
        }
        let item = self.columns.remove(from);
        // After remove(from), inserting at `to` places the item at the
        // original drop target's position regardless of direction.
        self.columns.insert(to, item);

        // Keep the moved item selected
        self.selected_tab = SelectedTab::Column(to);
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

    fn render_tab_column(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns.clone();
        let selected_tab = self.selected_tab.clone();

        div()
            .w(px(180.0))
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
                    .children(columns.iter().enumerate().map(|(i, col)| {
                        let name = col.name.clone();
                        let is_selected = selected_tab == SelectedTab::Column(i);
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
                                this.select_tab(SelectedTab::Column(i), window, cx);
                            }))
                            // Drag and drop for reordering
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
                                this.move_column(drag.index, i, cx);
                            }))
                            // Drag handle
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x585b70))
                                    .cursor(gpui::CursorStyle::ClosedHand)
                                    .child("\u{283F}"),
                            )
                            .child(name)
                    }))
                    // "+ Add" button
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
                            .child("+ Add")
                    }),
            )
    }

    fn render_content_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.selected_tab {
            SelectedTab::AddNew => self.render_add_new_content(cx).into_any_element(),
            SelectedTab::Column(i) => {
                let i = *i;
                if let Some(col) = self.columns.get(i) {
                    if col.column_type == "custom" {
                        self.render_custom_edit_content(col.clone(), cx)
                            .into_any_element()
                    } else {
                        self.render_preset_content(col.clone(), cx)
                            .into_any_element()
                    }
                } else {
                    div().into_any_element()
                }
            }
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
                    .child("Add Timeline Column"),
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
            // Delete button
            .child(
                div().flex().justify_end().child(
                    Button::new("delete-preset")
                        .label("Delete")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.remove_current(window, cx);
                        })),
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
                        SelectedMenu::Timeline => self.render_content_area(cx).into_any_element(),
                        SelectedMenu::Database => self.render_database_content(cx).into_any_element(),
                        SelectedMenu::About => self.render_about_content(cx).into_any_element(),
                    }),
            )
    }
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
