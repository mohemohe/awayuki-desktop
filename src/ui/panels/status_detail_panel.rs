use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, point, px, rgb, AnyElement, App, AsyncApp, Context, EventEmitter, FocusHandle, Focusable,
    IntoElement, ScrollHandle, SharedString, WeakEntity, Window,
};
use gpui_component::button::Button;
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::scroll::ScrollableElement;
use gpui_component::IconName;
use gpui_tokio_bridge::Tokio;

use crate::mastodon::client::MastodonClient;
use crate::ui::components::status_item::{render_status_item, ReplyTarget, StatusItemData};
use crate::ui::panels::account_panel::AccountDetailRequest;
use crate::ui::panels::timeline_panel::{LightboxState, ReplyState};
use crate::ui::workspace::ClosePanelRequest;

/// Global state for requesting a status detail panel
#[derive(Default, Clone)]
pub struct StatusDetailRequest {
    pub status_id: Option<String>,
}

impl gpui::Global for StatusDetailRequest {}

pub struct StatusDetailPanel {
    status_id: String,
    target_status: Option<StatusItemData>,
    ancestors: Vec<StatusItemData>,
    client: MastodonClient,
    loading: bool,
    expanded_cw: HashSet<String>,
    revealed_nsfw: HashSet<String>,
    retry_media: HashMap<String, u64>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
}

impl StatusDetailPanel {
    pub fn new(
        status_id: String,
        client: MastodonClient,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            status_id,
            target_status: None,
            ancestors: Vec::new(),
            client,
            loading: true,
            expanded_cw: HashSet::new(),
            revealed_nsfw: HashSet::new(),
            retry_media: HashMap::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
        };
        panel.load_status_detail(cx);
        panel
    }

    fn load_status_detail(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let status_id = self.status_id.clone();

        let task = Tokio::spawn(cx, async move {
            let status = client
                .get_status(&status_id)
                .await
                .map_err(|e| e.to_string())?;
            let context = client
                .get_status_context(&status_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((status, context))
        });

        cx.spawn(
            async move |this: WeakEntity<StatusDetailPanel>, cx: &mut AsyncApp| {
                match task.await {
                    Ok(Ok((status, context))) => {
                        let _ = this.update(cx, |this, cx| {
                            this.target_status = Some(StatusItemData::from_status(&status));
                            this.ancestors = context
                                .ancestors
                                .iter()
                                .map(StatusItemData::from_status)
                                .collect();
                            this.loading = false;
                            cx.notify();
                        });
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Failed to load status detail: {}", e);
                        let _ = this.update(cx, |this, cx| {
                            this.loading = false;
                            cx.notify();
                        });
                    }
                    Err(e) => {
                        tracing::error!("Status detail task error: {}", e);
                        let _ = this.update(cx, |this, cx| {
                            this.loading = false;
                            cx.notify();
                        });
                    }
                }
            },
        )
        .detach();
    }

    fn find_status_mut(&mut self, id: &str) -> Option<&mut StatusItemData> {
        if let Some(ref mut target) = self.target_status {
            if target.id == id {
                return Some(target);
            }
        }
        self.ancestors.iter_mut().find(|s| s.id == id)
    }

    fn toggle_reblog(&mut self, status_id: String, cx: &mut Context<Self>) {
        let currently_reblogged = self
            .find_status_mut(&status_id)
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
            async move |this: WeakEntity<StatusDetailPanel>, cx: &mut AsyncApp| {
                match task.await {
                    Ok(Ok(updated_status)) => {
                        let _ = this.update(cx, |this, cx| {
                            if let Some(item) = this.find_status_mut(&status_id) {
                                item.reblogged =
                                    updated_status.reblogged.unwrap_or(!currently_reblogged);
                                item.reblogs_count = updated_status.reblogs_count;
                                cx.notify();
                            }
                        });
                    }
                    Ok(Err(e)) => tracing::error!("Reblog toggle failed: {}", e),
                    Err(e) => tracing::error!("Reblog task error: {}", e),
                }
            },
        )
        .detach();
    }

    fn toggle_favourite(&mut self, status_id: String, cx: &mut Context<Self>) {
        let currently_favourited = self
            .find_status_mut(&status_id)
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
            async move |this: WeakEntity<StatusDetailPanel>, cx: &mut AsyncApp| {
                match task.await {
                    Ok(Ok(updated_status)) => {
                        let _ = this.update(cx, |this, cx| {
                            if let Some(item) = this.find_status_mut(&status_id) {
                                item.favourited =
                                    updated_status.favourited.unwrap_or(!currently_favourited);
                                item.favourites_count = updated_status.favourites_count;
                                cx.notify();
                            }
                        });
                    }
                    Ok(Err(e)) => tracing::error!("Favourite toggle failed: {}", e),
                    Err(e) => tracing::error!("Favourite task error: {}", e),
                }
            },
        )
        .detach();
    }
}

impl EventEmitter<PanelEvent> for StatusDetailPanel {}

impl Focusable for StatusDetailPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for StatusDetailPanel {
    fn panel_name(&self) -> &'static str {
        "StatusDetailPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from("Thread")
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
        let entity_id = cx.entity().entity_id();
        Some(vec![
            Button::new("scroll-to-top")
                .icon(IconName::ArrowUp)
                .on_click(move |_event, _window, _cx| {
                    scroll_handle.set_offset(point(px(0.), px(0.)));
                }),
            Button::new("close-panel")
                .icon(IconName::Close)
                .on_click(move |_event, _window, cx| {
                    cx.set_global(ClosePanelRequest {
                        entity_id: Some(entity_id),
                    });
                }),
        ])
    }
}

impl Render for StatusDetailPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Build callbacks
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
                cx.set_global(AccountDetailRequest {
                    account_id: Some(account_id),
                });
            });

        let on_timestamp_click: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|status_id: String, _window: &mut Window, cx: &mut App| {
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

        // Render ancestor statuses
        let ancestor_elements: Vec<AnyElement> = self
            .ancestors
            .iter()
            .map(|status| {
                let expanded = self.expanded_cw.contains(&status.id);
                let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
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
                    &self.retry_media,
                    window,
                    cx,
                )
            })
            .collect();

        // Render target status with highlight
        let target_element: Option<AnyElement> = self.target_status.as_ref().map(|status| {
            let expanded = self.expanded_cw.contains(&status.id);
            let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
            div()
                .bg(rgb(0x262637))
                .border_l_2()
                .border_color(rgb(0x89b4fa))
                .child(render_status_item(
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
                    &self.retry_media,
                    window,
                    cx,
                ))
                .into_any_element()
        });

        div()
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e2e))
            .relative()
            .vertical_scrollbar(&self.scroll_handle)
            .child(
                div()
                    .id("status-detail-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(ancestor_elements)
                    .children(target_element)
                    .when(self.loading, |el| {
                        el.child(
                            div()
                                .w_full()
                                .py(px(16.0))
                                .flex()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(0x6c7086))
                                .child("Loading..."),
                        )
                    }),
            )
    }
}
