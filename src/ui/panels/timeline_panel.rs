use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, point, px, rgb, size, App, AsyncApp, AvailableSpace, Context, EventEmitter, FocusHandle,
    Focusable, IntoElement, Pixels, ScrollHandle, SharedString, Size, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{v_virtual_list, IconName, VirtualListScrollHandle};
use gpui_component::WindowExt;
use gpui_tokio_bridge::Tokio;

use sqlx;

use crate::db::models::DbStatus;
use crate::db::pool::Database;
use crate::mastodon::client::MastodonClient;
use crate::mastodon::endpoints::notifications::NotificationParams;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::types::status::Status;
use crate::services::streaming_service::{self, TimelineEvent};
use crate::services::timeline_service::{self, TimelineType};
use crate::state::appearance::{AppearanceSettings, DisplayMode};
use crate::state::performance::{PerformanceSettings, TimelineRenderer};
use crate::state::confirmation::ConfirmationSettings;
use crate::ui::workspace::ClosePanelRequest;
use crate::ui::components::status_item::{
    render_compact_status_item, render_status_item, EditTarget, ReplyTarget, StatusItemData,
};

const DEFAULT_MAX_STATUSES: usize = 100;

pub struct TimelinePanel {
    title: SharedString,
    timeline_type: TimelineType,
    max_statuses: usize,
    statuses: Vec<StatusItemData>,
    client: MastodonClient,
    account_acct: String,
    account_id: String,
    database: Arc<Database>,
    loading: bool,
    oldest_id: Option<String>,
    expanded_cw: HashSet<String>,
    revealed_nsfw: HashSet<String>,
    expanded_statuses: HashSet<String>,
    retry_media: HashMap<String, u64>,
    focus_handle: FocusHandle,
    scroll_handle: VirtualListScrollHandle,
    list_scroll_handle: ScrollHandle,
    height_cache: HashMap<String, Pixels>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    last_measured_width: Option<Pixels>,
}

impl TimelinePanel {
    pub fn new(
        title: impl Into<SharedString>,
        timeline_type: TimelineType,
        client: MastodonClient,
        account_acct: String,
        account_id: String,
        database: Arc<Database>,
        max_statuses: Option<u32>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            title: title.into(),
            timeline_type,
            max_statuses: max_statuses
                .map(|v| v as usize)
                .unwrap_or(DEFAULT_MAX_STATUSES),
            statuses: Vec::new(),
            client,
            account_acct,
            account_id,
            database,
            loading: false,
            oldest_id: None,
            expanded_cw: HashSet::new(),
            revealed_nsfw: HashSet::new(),
            expanded_statuses: HashSet::new(),
            retry_media: HashMap::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: VirtualListScrollHandle::new(),
            list_scroll_handle: ScrollHandle::new(),
            height_cache: HashMap::new(),
            item_sizes: Rc::new(Vec::new()),
            last_measured_width: None,
        };
        // Clear height cache when appearance settings change
        cx.observe_global::<AppearanceSettings>(|this: &mut TimelinePanel, cx| {
            this.height_cache.clear();
            this.last_measured_width = None;
            cx.notify();
        })
        .detach();

        panel.load_initial(cx);
        panel
    }

    fn load_initial(&mut self, cx: &mut Context<Self>) {
        match self.timeline_type {
            TimelineType::CustomSql(ref sql) => self.fetch_custom_sql(sql.clone(), cx),
            TimelineType::Notification => self.fetch_notifications(None, false, cx),
            _ => self.fetch_statuses(None, false, cx),
        }
    }

    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(oldest_id) = self.oldest_id.clone() else {
            return;
        };
        match self.timeline_type {
            TimelineType::Notification => self.fetch_notifications(Some(oldest_id), true, cx),
            TimelineType::CustomSql(_) => {}
            _ => self.fetch_statuses(Some(oldest_id), true, cx),
        }
    }

    fn fetch_custom_sql(&mut self, sql: String, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let database = self.database.clone();

        let task = Tokio::spawn(cx, async move {
            let reader = database.reader();

            // Execute user SQL to get statuses
            let statuses: Vec<DbStatus> = sqlx::query_as(&sql)
                .fetch_all(reader)
                .await
                .map_err(|e| format!("SQL error: {}", e))?;

            // Collect unique (account_id, server_domain) and fetch accounts
            let account_keys: Vec<(String, String)> = statuses
                .iter()
                .map(|s| (s.account_id.clone(), s.server_domain.clone()))
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let mut accounts = std::collections::HashMap::new();
            for (account_id, server_domain) in &account_keys {
                if let Ok(Some(acc)) =
                    crate::db::queries::accounts::get_account(reader, account_id, server_domain)
                        .await
                {
                    accounts.insert(acc.id.clone(), acc);
                }
            }

            let items: Vec<StatusItemData> = statuses
                .iter()
                .map(|s| {
                    let acc = accounts.get(&s.account_id);
                    StatusItemData::from_db(s, acc)
                })
                .collect();

            Ok::<Vec<StatusItemData>, String>(items)
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(items)) => {
                    tracing::info!("Custom SQL returned {} statuses", items.len());
                    let _ = this.update(cx, |this, cx| {
                        this.statuses = items;
                        this.loading = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Custom SQL failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn fetch_statuses(&mut self, max_id: Option<String>, append: bool, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let database = self.database.clone();
        let account_acct = self.account_acct.clone();
        let tl_type = self.timeline_type.clone();
        let params = TimelineParams {
            max_id,
            ..TimelineParams::default()
        };

        let task = Tokio::spawn(cx, async move {
            // Fetch from API
            let statuses = timeline_service::fetch_from_api(&client, &tl_type, &params)
                .await
                .map_err(|e| e.to_string())?;

            // Save to DB (server + account + status + timeline_entry)
            let server_domain = client.domain().to_string();
            let tl_key = tl_type.as_str();
            for status in &statuses {
                if let Err(e) =
                    timeline_service::save_status_to_db(database.writer(), status, &server_domain)
                        .await
                {
                    tracing::warn!("Failed to save status {} to DB: {}", status.id, e);
                }
                if let Err(e) = crate::db::queries::timeline::insert_timeline_entry(
                    database.writer(),
                    &tl_key,
                    &server_domain,
                    &status.id,
                    &account_acct,
                    &status.created_at.to_rfc3339(),
                )
                .await
                {
                    tracing::warn!("Failed to insert timeline entry: {}", e);
                }
            }

            Ok::<Vec<Status>, String>(statuses)
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(statuses)) => {
                    tracing::info!("Fetched {} statuses from API", statuses.len());
                    let _ = this.update(cx, |this, cx| {
                        if let Some(last) = statuses.last() {
                            this.oldest_id = Some(last.id.clone());
                        }
                        let items: Vec<StatusItemData> =
                            statuses.iter().map(StatusItemData::from_status).collect();
                        if append {
                            this.statuses.extend(items);
                        } else {
                            this.statuses = items;
                        }
                        this.statuses.truncate(this.max_statuses);
                        this.loading = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Timeline fetch failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn fetch_notifications(
        &mut self,
        max_id: Option<String>,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let params = NotificationParams {
            max_id,
            ..NotificationParams::default()
        };

        let task = Tokio::spawn(cx, async move {
            let notifications = client
                .get_notifications(&params)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<Vec<crate::mastodon::types::notification::Notification>, String>(notifications)
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(notifications)) => {
                    tracing::info!("Fetched {} notifications from API", notifications.len());
                    let _ = this.update(cx, |this, cx| {
                        if let Some(last) = notifications.last() {
                            this.oldest_id = Some(last.id.clone());
                        }
                        let items: Vec<StatusItemData> = notifications
                            .iter()
                            .map(StatusItemData::from_notification)
                            .collect();
                        if append {
                            this.statuses.extend(items);
                        } else {
                            this.statuses = items;
                        }
                        this.statuses.truncate(this.max_statuses);
                        this.loading = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Notification fetch failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
    }

    fn toggle_reblog(&mut self, status_id: String, cx: &mut Context<Self>) {
        let currently_reblogged = self
            .statuses
            .iter()
            .find(|s| s.id == status_id)
            .map(|s| s.reblogged)
            .unwrap_or(false);

        let client = self.client.clone();
        let id = status_id.clone();

        let task = Tokio::spawn(cx, async move {
            if currently_reblogged {
                client.unreblog(&id).await.map_err(|e| e.to_string())
            } else {
                client.reblog(&id).await.map_err(|e| e.to_string())
            }
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.reblogged =
                                updated_status.reblogged.unwrap_or(!currently_reblogged);
                            item.reblogs_count = updated_status.reblogs_count;
                            cx.notify();
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Reblog toggle failed: {}", e),
                Err(e) => tracing::error!("Reblog task error: {}", e),
            },
        )
        .detach();
    }

    fn toggle_favourite(&mut self, status_id: String, cx: &mut Context<Self>) {
        let currently_favourited = self
            .statuses
            .iter()
            .find(|s| s.id == status_id)
            .map(|s| s.favourited)
            .unwrap_or(false);

        let client = self.client.clone();
        let id = status_id.clone();

        let task = Tokio::spawn(cx, async move {
            if currently_favourited {
                client.unfavourite(&id).await.map_err(|e| e.to_string())
            } else {
                client.favourite(&id).await.map_err(|e| e.to_string())
            }
        });

        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.favourited =
                                updated_status.favourited.unwrap_or(!currently_favourited);
                            item.favourites_count = updated_status.favourites_count;
                            cx.notify();
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Favourite toggle failed: {}", e),
                Err(e) => tracing::error!("Favourite task error: {}", e),
            },
        )
        .detach();
    }

    /// Measure heights of statuses that are not yet cached.
    /// Uses `layout_as_root()` for off-screen measurement (same pattern as VirtualList::measure_item).
    fn measure_status_heights(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.last_measured_width.unwrap_or(px(350.0));
        let display_mode = cx.global::<AppearanceSettings>().display_mode;
        for status in &self.statuses {
            let key = self.height_cache_key(&status.id);
            if self.height_cache.contains_key(&key) {
                continue;
            }
            let cw_expanded = self.expanded_cw.contains(&status.id);
            let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
            let empty_retry = HashMap::new();
            let mut element = match display_mode {
                DisplayMode::Mystique => {
                    let mystique_expanded = self.expanded_statuses.contains(&status.id);
                    render_compact_status_item(
                        status,
                        mystique_expanded,
                        None,
                        cw_expanded,
                        nsfw_revealed,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        &empty_retry,
                        window,
                        cx,
                    )
                }
                DisplayMode::StarryEyes => render_status_item(
                    status,
                    cw_expanded,
                    nsfw_revealed,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    &empty_retry,
                    window,
                    cx,
                ),
            };
            let measured = element.layout_as_root(
                size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                window,
                cx,
            );
            self.height_cache.insert(key, measured.height);
        }
    }

    fn height_cache_key(&self, id: &str) -> String {
        let mut key = id.to_string();
        if self.expanded_cw.contains(id) {
            key.push_str("-cw");
        }
        if self.expanded_statuses.contains(id) {
            key.push_str("-exp");
        }
        key
    }

    fn rebuild_item_sizes(&mut self) {
        let sizes: Vec<Size<Pixels>> = self
            .statuses
            .iter()
            .map(|status| {
                let key = self.height_cache_key(&status.id);
                let height = self.height_cache.get(&key).copied().unwrap_or(px(100.0));
                size(px(0.0), height)
            })
            .collect();
        self.item_sizes = Rc::new(sizes);
    }

    fn cleanup_height_cache(&mut self) {
        let valid_ids: HashSet<&str> = self.statuses.iter().map(|s| s.id.as_str()).collect();
        self.height_cache.retain(|key, _| {
            // Extract base ID by stripping known suffixes
            let base = key
                .strip_suffix("-cw-exp")
                .or_else(|| key.strip_suffix("-cw"))
                .or_else(|| key.strip_suffix("-exp"))
                .unwrap_or(key);
            valid_ids.contains(base)
        });
    }

    fn invalidate_height_cache(&mut self, status_id: &str) {
        let id = status_id.to_string();
        self.height_cache.remove(&id);
        self.height_cache.remove(&format!("{}-cw", id));
        self.height_cache.remove(&format!("{}-exp", id));
        self.height_cache.remove(&format!("{}-cw-exp", id));
    }

    /// Start receiving streaming events and prepend new statuses.
    /// Events are filtered based on whether the stream type matches this panel's timeline type.
    /// For CustomSql panels, the SQL query is re-executed and only re-rendered if results change.
    pub fn start_streaming(
        &mut self,
        receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        cx: &mut Context<Self>,
    ) {
        let timeline_type = self.timeline_type.clone();

        if let TimelineType::CustomSql(sql) = timeline_type {
            self.start_streaming_custom_sql(receiver, sql, cx);
        } else {
            self.start_streaming_standard(receiver, timeline_type, cx);
        }
    }

    fn start_streaming_standard(
        &mut self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        timeline_type: TimelineType,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                while let Some(event) = receiver.next().await {
                    let _ = this.update(cx, |this, cx| match event {
                        TimelineEvent::NewStatus(status, ref stream_type) => {
                            if timeline_type.matches_stream_type(stream_type) {
                                let item = StatusItemData::from_status(&status);
                                this.statuses.insert(0, item);
                                this.statuses.truncate(this.max_statuses);
                                cx.notify();
                            }
                        }
                        TimelineEvent::StatusUpdate(status) => {
                            let item = StatusItemData::from_status(&status);
                            if let Some(pos) = this.statuses.iter().position(|s| s.id == status.id)
                            {
                                this.invalidate_height_cache(&status.id);
                                this.statuses[pos] = item;
                                cx.notify();
                            }
                        }
                        TimelineEvent::DeleteStatus(id) => {
                            this.invalidate_height_cache(&id);
                            this.statuses.retain(|s| s.id != id);
                            cx.notify();
                        }
                        TimelineEvent::NewNotification(notification, _) => {
                            if matches!(timeline_type, TimelineType::Notification) {
                                let item = StatusItemData::from_notification(&notification);
                                this.statuses.insert(0, item);
                                this.statuses.truncate(this.max_statuses);
                                streaming_service::send_desktop_notification(&notification);
                                cx.notify();
                            }
                        }
                    });
                }
            },
        )
        .detach();
    }

    fn start_streaming_custom_sql(
        &mut self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<TimelineEvent>,
        sql: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(
            async move |this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                use futures::StreamExt;
                while let Some(_event) = receiver.next().await {
                    // Spawn SQL query on tokio runtime via entity context
                    let query = sql.clone();
                    let task = this.update(cx, |this, cx| {
                        let database = this.database.clone();
                        Tokio::spawn(cx, async move {
                            let reader = database.reader();
                            let statuses: Vec<DbStatus> = sqlx::query_as(&query)
                                .fetch_all(reader)
                                .await
                                .map_err(|e| format!("SQL error: {}", e))?;

                            let account_keys: Vec<(String, String)> = statuses
                                .iter()
                                .map(|s| (s.account_id.clone(), s.server_domain.clone()))
                                .collect::<std::collections::HashSet<_>>()
                                .into_iter()
                                .collect();

                            let mut accounts = std::collections::HashMap::new();
                            for (account_id, server_domain) in &account_keys {
                                if let Ok(Some(acc)) = crate::db::queries::accounts::get_account(
                                    reader,
                                    account_id,
                                    server_domain,
                                )
                                .await
                                {
                                    accounts.insert(acc.id.clone(), acc);
                                }
                            }

                            let items: Vec<StatusItemData> = statuses
                                .iter()
                                .map(|s| {
                                    let acc = accounts.get(&s.account_id);
                                    StatusItemData::from_db(s, acc)
                                })
                                .collect();

                            Ok::<Vec<StatusItemData>, String>(items)
                        })
                    });

                    let Ok(task) = task else { return };

                    match task.await {
                        Ok(Ok(new_items)) => {
                            let _ = this.update(cx, |this, cx| {
                                // Only re-render if the ID list has changed
                                let old_ids: Vec<&str> =
                                    this.statuses.iter().map(|s| s.id.as_str()).collect();
                                let new_ids: Vec<&str> =
                                    new_items.iter().map(|s| s.id.as_str()).collect();
                                if old_ids != new_ids {
                                    this.statuses = new_items;
                                    cx.notify();
                                }
                            });
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("Custom SQL re-query failed: {}", e);
                        }
                        Err(e) => {
                            tracing::warn!("Custom SQL task error: {}", e);
                        }
                    }
                }
            },
        )
        .detach();
    }
}

impl EventEmitter<PanelEvent> for TimelinePanel {}

impl Focusable for TimelinePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TimelinePanel {
    fn panel_name(&self) -> &'static str {
        "TimelinePanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }

    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let scroll_handle = self.scroll_handle.clone();
        let mut buttons = vec![Button::new("scroll-to-top")
            .icon(IconName::ArrowUp)
            .on_click(move |_event, _window, _cx| {
                scroll_handle.set_offset(point(px(0.), px(0.)));
            })];
        if matches!(self.timeline_type, TimelineType::CustomSql(_)) {
            let entity_id = cx.entity().entity_id();
            buttons.push(
                Button::new("close-panel")
                    .icon(IconName::Close)
                    .on_click(move |_event, _window, cx| {
                        cx.set_global(ClosePanelRequest {
                            entity_id: Some(entity_id),
                        });
                    }),
            );
        }
        Some(buttons)
    }
}

impl Render for TimelinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tracing::debug!("render: '{}' column", self.title);

        let timeline_renderer = cx.global::<PerformanceSettings>().timeline_renderer;

        // --- Width change detection & height measurement (VirtualList only) ---
        if timeline_renderer == TimelineRenderer::VirtualList {
            let viewport_bounds = self.scroll_handle.bounds();
            let current_width = viewport_bounds.size.width;
            if current_width > px(0.0) {
                let should_invalidate = match self.last_measured_width {
                    None => true,
                    Some(prev_width) => {
                        let diff = if prev_width > current_width {
                            prev_width - current_width
                        } else {
                            current_width - prev_width
                        };
                        diff > px(1.0)
                    }
                };
                if should_invalidate {
                    self.height_cache.clear();
                }
                self.last_measured_width = Some(current_width);
            }

            self.measure_status_heights(window, cx);
            self.rebuild_item_sizes();
            self.cleanup_height_cache();
        }

        // --- Build callbacks ---
        let on_media: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|url: String, _window: &mut Window, cx: &mut App| {
                cx.set_global(LightboxState {
                    url: Some(url),
                    local_path: None,
                });
            });

        let entity = cx.entity().downgrade();
        let on_cw_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity.update(cx, |this, cx| {
                    if !this.expanded_cw.remove(&id) {
                        this.expanded_cw.insert(id.clone());
                    }
                    this.invalidate_height_cache(&id);
                    cx.notify();
                });
            });

        let entity_nsfw = cx.entity().downgrade();
        let on_nsfw_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_nsfw.update(cx, |this, cx| {
                    if !this.revealed_nsfw.remove(&id) {
                        this.revealed_nsfw.insert(id.clone());
                    }
                    this.invalidate_height_cache(&id);
                    cx.notify();
                });
            });

        let on_reply: Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)> =
            Arc::new(|target: ReplyTarget, _window: &mut Window, cx: &mut App| {
                cx.set_global(ReplyState {
                    target: Some(target),
                });
            });

        let entity_reblog = cx.entity().downgrade();
        let on_reblog: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, window: &mut Window, cx: &mut App| {
                let confirm = cx
                    .try_global::<ConfirmationSettings>()
                    .map(|s| s.confirm_boost)
                    .unwrap_or(false);
                let currently_reblogged = entity_reblog
                    .upgrade()
                    .map(|e| {
                        e.read(cx)
                            .statuses
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.reblogged)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);

                if confirm && !currently_reblogged {
                    let entity = entity_reblog.clone();
                    let id = id.clone();
                    window.open_dialog(cx, move |dialog, _, _| {
                        let entity = entity.clone();
                        let id = id.clone();
                        dialog.confirm().child("Boost this post?").on_ok(
                            move |_, _window, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.toggle_reblog(id.clone(), cx);
                                });
                                true
                            },
                        )
                    });
                } else {
                    let _ = entity_reblog.update(cx, |this, cx| {
                        this.toggle_reblog(id, cx);
                    });
                }
            });

        let entity_fav = cx.entity().downgrade();
        let on_favourite: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, window: &mut Window, cx: &mut App| {
                let confirm = cx
                    .try_global::<ConfirmationSettings>()
                    .map(|s| s.confirm_favourite)
                    .unwrap_or(false);
                let currently_favourited = entity_fav
                    .upgrade()
                    .map(|e| {
                        e.read(cx)
                            .statuses
                            .iter()
                            .find(|s| s.id == id)
                            .map(|s| s.favourited)
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);

                if confirm && !currently_favourited {
                    let entity = entity_fav.clone();
                    let id = id.clone();
                    window.open_dialog(cx, move |dialog, _, _| {
                        let entity = entity.clone();
                        let id = id.clone();
                        dialog.confirm().child("Favourite this post?").on_ok(
                            move |_, _window, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.toggle_favourite(id.clone(), cx);
                                });
                                true
                            },
                        )
                    });
                } else {
                    let _ = entity_fav.update(cx, |this, cx| {
                        this.toggle_favourite(id, cx);
                    });
                }
            });

        let on_account_click: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|account_id: String, _window: &mut Window, cx: &mut App| {
                use crate::ui::panels::account_panel::AccountDetailRequest;
                cx.set_global(AccountDetailRequest {
                    account_id: Some(account_id),
                });
            });

        let on_timestamp_click: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|status_id: String, _window: &mut Window, cx: &mut App| {
                use crate::ui::panels::status_detail_panel::StatusDetailRequest;
                cx.set_global(StatusDetailRequest {
                    status_id: Some(status_id),
                });
            });

        let entity_reload = cx.entity().downgrade();
        let on_media_reload: Arc<dyn Fn(String, &mut Window, &mut App)> = Arc::new(
            move |preview_url: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_reload.update(cx, |this, cx| {
                    let count = this.retry_media.entry(preview_url).or_insert(0);
                    *count += 1;
                    cx.notify();
                });
            },
        );

        let entity_edit = cx.entity().downgrade();
        let on_edit: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |status_id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_edit.update(cx, |this, cx| {
                    let status_data = this.statuses.iter().find(|s| s.id == status_id).map(|s| {
                        (
                            s.display_name.to_string(),
                            s.acct.to_string(),
                            s.content.to_string(),
                            s.visibility.to_string(),
                            s.media_attachments.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                        )
                    });

                    if let Some((display_name, acct, content, visibility, media_ids)) = status_data {
                        let client = this.client.clone();
                        let status_id_clone = status_id.clone();
                        let task = Tokio::spawn(cx, async move {
                            client
                                .get_status_source(&status_id_clone)
                                .await
                                .map_err(|e| e.to_string())
                        });

                        cx.spawn(async move |_this: WeakEntity<TimelinePanel>, cx: &mut AsyncApp| {
                            match task.await {
                                Ok(Ok(source)) => {
                                    let _ = cx.update(|cx| {
                                        cx.set_global(EditState {
                                            target: Some(EditTarget {
                                                status_id,
                                                display_name,
                                                acct,
                                                content,
                                                source_text: source.text,
                                                spoiler_text: source.spoiler_text,
                                                visibility,
                                                media_ids,
                                            }),
                                        });
                                    });
                                }
                                Ok(Err(e)) => tracing::error!("Failed to get status source: {}", e),
                                Err(e) => tracing::error!("Task error: {}", e),
                            }
                        })
                        .detach();
                    }
                });
            });

        let entity_expand = cx.entity().downgrade();
        let on_expand_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_expand.update(cx, |this, cx| {
                    if !this.expanded_statuses.remove(&id) {
                        this.expanded_statuses.insert(id.clone());
                    }
                    this.invalidate_height_cache(&id);
                    cx.notify();
                });
            });

        // --- Load more state ---
        let show_load_more = !self.statuses.is_empty()
            && self.oldest_id.is_some()
            && !self.loading
            && !matches!(self.timeline_type, TimelineType::CustomSql(_));
        let loading_more = self.loading
            && !self.statuses.is_empty()
            && !matches!(self.timeline_type, TimelineType::CustomSql(_));
        let has_footer = show_load_more || loading_more;

        let entity_load = cx.entity().downgrade();
        let on_load_more: Arc<dyn Fn(&mut Window, &mut App)> =
            Arc::new(move |_window: &mut Window, cx: &mut App| {
                let _ = entity_load.update(cx, |this, cx| {
                    this.load_more(cx);
                });
            });

        // --- Build timeline list ---
        let has_statuses = !self.statuses.is_empty();
        let show_loading = self.loading && !has_statuses;
        let show_empty = !has_statuses && !self.loading;

        let display_mode = cx.global::<AppearanceSettings>().display_mode;

        let mut container = div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .relative();

        match timeline_renderer {
            TimelineRenderer::List => {
                if has_statuses {
                    let status_elements: Vec<_> = self
                        .statuses
                        .iter()
                        .map(|status| {
                            let cw_expanded = self.expanded_cw.contains(&status.id);
                            let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
                            match display_mode {
                                DisplayMode::Mystique => {
                                    let mystique_expanded =
                                        self.expanded_statuses.contains(&status.id);
                                    render_compact_status_item(
                                        status,
                                        mystique_expanded,
                                        Some(&on_expand_toggle),
                                        cw_expanded,
                                        nsfw_revealed,
                                        Some(&on_cw_toggle),
                                        Some(&on_nsfw_toggle),
                                        Some(&on_media),
                                        Some(&on_reply),
                                        Some(&on_reblog),
                                        Some(&on_favourite),
                                        Some(&on_account_click),
                                        Some(&on_timestamp_click),
                                        Some(&on_media_reload),
                                        Some(&on_edit),
                                        Some(&self.account_id),
                                        &self.retry_media,
                                        window,
                                        cx,
                                    )
                                }
                                DisplayMode::StarryEyes => render_status_item(
                                    status,
                                    cw_expanded,
                                    nsfw_revealed,
                                    Some(&on_cw_toggle),
                                    Some(&on_nsfw_toggle),
                                    Some(&on_media),
                                    Some(&on_reply),
                                    Some(&on_reblog),
                                    Some(&on_favourite),
                                    Some(&on_account_click),
                                    Some(&on_timestamp_click),
                                    Some(&on_media_reload),
                                    Some(&on_edit),
                                    Some(&self.account_id),
                                    &self.retry_media,
                                    window,
                                    cx,
                                ),
                            }
                        })
                        .collect();

                    let mut scroll_content = div()
                        .id("timeline-list-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .track_scroll(&self.list_scroll_handle)
                        .children(status_elements);

                    // Footer: Load More / Loading
                    if has_footer {
                        if self.loading {
                            scroll_content = scroll_content.child(
                                div()
                                    .id("load-more-loading")
                                    .w_full()
                                    .py(px(12.0))
                                    .flex()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(0x6c7086))
                                    .child("Loading..."),
                            );
                        } else {
                            let cb = on_load_more.clone();
                            scroll_content = scroll_content.child(
                                div()
                                    .id("load-more-timeline")
                                    .w_full()
                                    .py(px(12.0))
                                    .flex()
                                    .justify_center()
                                    .child(
                                        Button::new("load-more-btn")
                                            .ghost()
                                            .label("Load more")
                                            .on_click(move |_, window, cx| {
                                                cb(window, cx);
                                            }),
                                    ),
                            );
                        }
                    }

                    container = container.child(scroll_content);
                }

                if show_loading {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(16.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("Loading..."),
                    );
                }

                if show_empty {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(32.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("No statuses yet"),
                    );
                }

                container.vertical_scrollbar(&self.list_scroll_handle)
            }
            TimelineRenderer::VirtualList => {
                // Append a footer item for Load More button / loading indicator
                let item_sizes = if has_footer {
                    let mut sizes = (*self.item_sizes).clone();
                    sizes.push(size(px(0.0), px(48.0)));
                    Rc::new(sizes)
                } else {
                    self.item_sizes.clone()
                };
                let status_count = self.statuses.len();
                let entity_handle = cx.entity().clone();

                if has_statuses {
                    let virtual_list = v_virtual_list(
                        entity_handle,
                        "timeline-virtual-list",
                        item_sizes,
                        move |this: &mut TimelinePanel,
                              range: Range<usize>,
                              window: &mut Window,
                              cx: &mut Context<TimelinePanel>| {
                            range
                                .map(|ix| {
                                    // Footer item: Load More button or loading indicator
                                    if ix >= status_count {
                                        if this.loading {
                                            return div()
                                                .id("load-more-loading")
                                                .w_full()
                                                .py(px(12.0))
                                                .flex()
                                                .justify_center()
                                                .text_sm()
                                                .text_color(rgb(0x6c7086))
                                                .child("Loading...")
                                                .into_any_element();
                                        }
                                        let cb = on_load_more.clone();
                                        return div()
                                            .id("load-more-timeline")
                                            .w_full()
                                            .py(px(12.0))
                                            .flex()
                                            .justify_center()
                                            .child(
                                                Button::new("load-more-btn")
                                                    .ghost()
                                                    .label("Load more")
                                                    .on_click(move |_, window, cx| {
                                                        cb(window, cx);
                                                    }),
                                            )
                                            .into_any_element();
                                    }

                                    let status = &this.statuses[ix];
                                    let cw_expanded = this.expanded_cw.contains(&status.id);
                                    let nsfw_revealed = this.revealed_nsfw.contains(&status.id);
                                    match display_mode {
                                        DisplayMode::Mystique => {
                                            let mystique_expanded =
                                                this.expanded_statuses.contains(&status.id);
                                            render_compact_status_item(
                                                status,
                                                mystique_expanded,
                                                Some(&on_expand_toggle),
                                                cw_expanded,
                                                nsfw_revealed,
                                                Some(&on_cw_toggle),
                                                Some(&on_nsfw_toggle),
                                                Some(&on_media),
                                                Some(&on_reply),
                                                Some(&on_reblog),
                                                Some(&on_favourite),
                                                Some(&on_account_click),
                                                Some(&on_timestamp_click),
                                                Some(&on_media_reload),
                                                Some(&on_edit),
                                                Some(&this.account_id),
                                                &this.retry_media,
                                                window,
                                                cx,
                                            )
                                        }
                                        DisplayMode::StarryEyes => render_status_item(
                                            status,
                                            cw_expanded,
                                            nsfw_revealed,
                                            Some(&on_cw_toggle),
                                            Some(&on_nsfw_toggle),
                                            Some(&on_media),
                                            Some(&on_reply),
                                            Some(&on_reblog),
                                            Some(&on_favourite),
                                            Some(&on_account_click),
                                            Some(&on_timestamp_click),
                                            Some(&on_media_reload),
                                            Some(&on_edit),
                                            Some(&this.account_id),
                                            &this.retry_media,
                                            window,
                                            cx,
                                        ),
                                    }
                                })
                                .collect()
                        },
                    )
                    .track_scroll(&self.scroll_handle)
                    .flex_1();

                    container = container.child(virtual_list);
                }

                if show_loading {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(16.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("Loading..."),
                    );
                }

                if show_empty {
                    container = container.child(
                        div()
                            .w_full()
                            .py(px(32.0))
                            .flex()
                            .justify_center()
                            .text_sm()
                            .text_color(rgb(0x6c7086))
                            .child("No statuses yet"),
                    );
                }

                container.vertical_scrollbar(&self.scroll_handle)
            }
        }
    }
}

/// Global state for lightbox image display
#[derive(Default, Clone)]
pub struct LightboxState {
    pub url: Option<String>,
    pub local_path: Option<std::path::PathBuf>,
}

impl gpui::Global for LightboxState {}

/// Global state for reply target
#[derive(Default)]
pub struct ReplyState {
    pub target: Option<ReplyTarget>,
}

impl gpui::Global for ReplyState {}

/// Global state for edit target
#[derive(Default)]
pub struct EditState {
    pub target: Option<EditTarget>,
}

impl gpui::Global for EditState {}
