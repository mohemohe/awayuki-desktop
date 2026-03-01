use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, point, px, rgb, size, App, AsyncApp, AvailableSpace, Context, EventEmitter, FocusHandle,
    Focusable, IntoElement, Pixels, SharedString, Size, WeakEntity, Window,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::scroll::ScrollableElement;
use gpui_component::{v_virtual_list, IconName, VirtualListScrollHandle};
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
use crate::state::appearance::AppearanceSettings;
use crate::ui::components::status_item::{render_status_item, ReplyTarget, StatusItemData};

const DEFAULT_MAX_STATUSES: usize = 100;

pub struct TimelinePanel {
    title: SharedString,
    timeline_type: TimelineType,
    max_statuses: usize,
    statuses: Vec<StatusItemData>,
    client: MastodonClient,
    account_acct: String,
    database: Arc<Database>,
    loading: bool,
    oldest_id: Option<String>,
    expanded_cw: HashSet<String>,
    revealed_nsfw: HashSet<String>,
    retry_media: HashMap<String, u64>,
    focus_handle: FocusHandle,
    scroll_handle: VirtualListScrollHandle,
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
        database: Arc<Database>,
        max_statuses: Option<u32>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            title: title.into(),
            timeline_type,
            max_statuses: max_statuses.map(|v| v as usize).unwrap_or(DEFAULT_MAX_STATUSES),
            statuses: Vec::new(),
            client,
            account_acct,
            database,
            loading: false,
            oldest_id: None,
            expanded_cw: HashSet::new(),
            revealed_nsfw: HashSet::new(),
            retry_media: HashMap::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: VirtualListScrollHandle::new(),
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
        self.fetch_statuses(Some(oldest_id), true, cx);
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
        for status in &self.statuses {
            let key = self.height_cache_key(&status.id);
            if self.height_cache.contains_key(&key) {
                continue;
            }
            tracing::debug!("measure_status_heights: {}", &status.id);
            let expanded = self.expanded_cw.contains(&status.id);
            let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
            let empty_retry = HashMap::new();
            let mut element = render_status_item(
                status, expanded, nsfw_revealed, None, None, None, None, None, None, None, None, None, &empty_retry, window, cx,
            );
            let measured = element.layout_as_root(
                size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
                window,
                cx,
            );
            self.height_cache.insert(key, measured.height);
        }
    }

    fn height_cache_key(&self, id: &str) -> String {
        if self.expanded_cw.contains(id) {
            format!("{}-expanded", id)
        } else {
            id.to_string()
        }
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
            let base_id = key.strip_suffix("-expanded").unwrap_or(key);
            valid_ids.contains(base_id)
        });
    }

    fn invalidate_height_cache(&mut self, status_id: &str) {
        self.height_cache.remove(status_id);
        self.height_cache.remove(&format!("{}-expanded", status_id));
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
        _cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let scroll_handle = self.scroll_handle.clone();
        Some(vec![Button::new("scroll-to-top")
            .icon(IconName::ArrowUp)
            .on_click(move |_event, _window, _cx| {
                scroll_handle.set_offset(point(px(0.), px(0.)));
            })])
    }
}

impl Render for TimelinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tracing::debug!("render: '{}' column", self.title);

        // --- Width change detection using viewport bounds from previous frame ---
        let viewport_bounds = self.scroll_handle.bounds();
        let current_width = viewport_bounds.size.width;
        if current_width > px(0.0) {
            let should_invalidate = match self.last_measured_width {
                // First time getting actual width: invalidate heights measured at fallback
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

        // --- Measure unmeasured items and rebuild item_sizes ---
        self.measure_status_heights(window, cx);
        self.rebuild_item_sizes();
        self.cleanup_height_cache();

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
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_reblog.update(cx, |this, cx| {
                    this.toggle_reblog(id, cx);
                });
            });

        let entity_fav = cx.entity().downgrade();
        let on_favourite: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_fav.update(cx, |this, cx| {
                    this.toggle_favourite(id, cx);
                });
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
        let on_media_reload: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |preview_url: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_reload.update(cx, |this, cx| {
                    let count = this.retry_media.entry(preview_url).or_insert(0);
                    *count += 1;
                    cx.notify();
                });
            });

        // --- Build VirtualList ---
        let has_statuses = !self.statuses.is_empty();
        let show_loading = self.loading && !has_statuses;
        let show_empty = !has_statuses && !self.loading;

        let item_sizes = self.item_sizes.clone();
        let entity_handle = cx.entity().clone();

        let mut container = div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .relative()
            .vertical_scrollbar(&self.scroll_handle);

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
                            let status = &this.statuses[ix];
                            let expanded = this.expanded_cw.contains(&status.id);
                            let nsfw_revealed = this.revealed_nsfw.contains(&status.id);
                            render_status_item(
                                status,
                                expanded,
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
                                &this.retry_media,
                                window,
                                cx,
                            )
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

        container
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
