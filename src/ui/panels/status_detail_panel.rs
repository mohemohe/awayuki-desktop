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
use gpui_component::WindowExt;
use gpui_tokio_bridge::Tokio;

use crate::mastodon::client::MastodonClient;
use crate::state::confirmation::ConfirmationSettings;
use crate::ui::components::status_item::{render_status_item, EditTarget, QuoteTarget, ReplyTarget, StatusItemData};
use crate::ui::panels::account_panel::AccountDetailRequest;
use crate::ui::panels::timeline_panel::{EditState, LightboxState, QuoteState, ReplyState};
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
    account_id: String,
    loading: bool,
    expanded_cw: HashSet<String>,
    revealed_nsfw: HashSet<String>,
    pending_poll_votes: HashMap<String, HashSet<usize>>,
    retry_media: HashMap<String, u64>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
}

impl StatusDetailPanel {
    pub fn new(
        status_id: String,
        client: MastodonClient,
        account_id: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            status_id,
            target_status: None,
            ancestors: Vec::new(),
            client,
            account_id,
            loading: true,
            expanded_cw: HashSet::new(),
            revealed_nsfw: HashSet::new(),
            pending_poll_votes: HashMap::new(),
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

    fn refresh_poll(&mut self, poll_id: String, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let pid = poll_id.clone();

        let task = Tokio::spawn(cx, async move {
            client.get_poll(&pid).await.map_err(|e| e.to_string())
        });

        cx.spawn(
            async move |this: WeakEntity<StatusDetailPanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_poll)) => {
                    let _ = this.update(cx, |this, cx| {
                        let all_statuses = this
                            .target_status
                            .iter_mut()
                            .chain(this.ancestors.iter_mut());
                        for status in all_statuses {
                            if status.poll.as_ref().map(|p| p.id == poll_id).unwrap_or(false) {
                                status.poll = Some(updated_poll.clone());
                            }
                        }
                        cx.notify();
                    });
                }
                Ok(Err(e)) => tracing::error!("Poll refresh failed: {}", e),
                Err(e) => tracing::error!("Poll refresh task error: {}", e),
            },
        )
        .detach();
    }

    fn vote_poll(&mut self, poll_id: String, choices: Vec<usize>, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let pid = poll_id.clone();
        let params = crate::mastodon::endpoints::statuses::VotePollParams {
            choices: choices.iter().map(|&c| c as i64).collect(),
        };

        let task = Tokio::spawn(cx, async move {
            client.vote_poll(&pid, &params).await.map_err(|e| e.to_string())
        });

        cx.spawn(
            async move |this: WeakEntity<StatusDetailPanel>, cx: &mut AsyncApp| match task.await {
                Ok(Ok(updated_poll)) => {
                    let _ = this.update(cx, |this, cx| {
                        // Update poll in target_status or ancestors
                        let all_statuses = this
                            .target_status
                            .iter_mut()
                            .chain(this.ancestors.iter_mut());
                        for status in all_statuses {
                            if status.poll.as_ref().map(|p| p.id == poll_id).unwrap_or(false) {
                                status.poll = Some(updated_poll.clone());
                            }
                        }
                        this.pending_poll_votes.remove(&poll_id);
                        cx.notify();
                    });
                }
                Ok(Err(e)) => tracing::error!("Poll vote failed: {}", e),
                Err(e) => tracing::error!("Poll vote task error: {}", e),
            },
        )
        .detach();
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

        let on_quote: Arc<dyn Fn(QuoteTarget, &mut Window, &mut App)> =
            Arc::new(|target: QuoteTarget, _window: &mut Window, cx: &mut App| {
                cx.set_global(QuoteState {
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
                        let panel = e.read(cx);
                        panel
                            .target_status
                            .as_ref()
                            .filter(|s| s.id == id)
                            .map(|s| s.reblogged)
                            .or_else(|| {
                                panel
                                    .ancestors
                                    .iter()
                                    .find(|s| s.id == id)
                                    .map(|s| s.reblogged)
                            })
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
                        let panel = e.read(cx);
                        panel
                            .target_status
                            .as_ref()
                            .filter(|s| s.id == id)
                            .map(|s| s.favourited)
                            .or_else(|| {
                                panel
                                    .ancestors
                                    .iter()
                                    .find(|s| s.id == id)
                                    .map(|s| s.favourited)
                            })
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

        let entity_edit = cx.entity().downgrade();
        let on_edit: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |status_id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_edit.update(cx, |this, cx| {
                    let mut all_statuses = this
                        .target_status
                        .iter()
                        .chain(this.ancestors.iter());
                    let status_data = all_statuses
                        .find(|s| s.id == status_id)
                        .map(|s| {
                            (
                                s.display_name.to_string(),
                                s.acct.to_string(),
                                s.content.to_string(),
                                s.visibility.to_string(),
                                s.media_attachments.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                                s.quote_id.clone(),
                                s.poll.clone(),
                            )
                        });

                    if let Some((display_name, acct, content, visibility, media_ids, quote_id, poll)) = status_data {
                        let client = this.client.clone();
                        let status_id_clone = status_id.clone();
                        let task = Tokio::spawn(cx, async move {
                            client
                                .get_status_source(&status_id_clone)
                                .await
                                .map_err(|e| e.to_string())
                        });

                        cx.spawn(async move |_this: WeakEntity<StatusDetailPanel>, cx: &mut AsyncApp| {
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
                                                quote_id,
                                                poll,
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

        let entity_vote = cx.entity().downgrade();
        let on_vote: Arc<dyn Fn(String, Vec<usize>, &mut Window, &mut App)> =
            Arc::new(move |poll_id: String, choices: Vec<usize>, _window: &mut Window, cx: &mut App| {
                let _ = entity_vote.update(cx, |this, cx| {
                    this.vote_poll(poll_id, choices, cx);
                });
            });

        let entity_poll_sel = cx.entity().downgrade();
        let on_poll_select: Arc<dyn Fn(String, usize, &mut Window, &mut App)> =
            Arc::new(move |poll_id: String, index: usize, _window: &mut Window, cx: &mut App| {
                let _ = entity_poll_sel.update(cx, |this, cx| {
                    let set = this.pending_poll_votes.entry(poll_id).or_default();
                    if !set.remove(&index) {
                        set.insert(index);
                    }
                    cx.notify();
                });
            });

        let entity_poll_refresh = cx.entity().downgrade();
        let on_poll_refresh: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |poll_id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_poll_refresh.update(cx, |this, cx| {
                    this.refresh_poll(poll_id, cx);
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
                    None,
                    Some(&on_quote),
                    Some(&on_account_click),
                    Some(&on_timestamp_click),
                    Some(&on_media_reload),
                    Some(&on_edit),
                    Some(&on_vote),
                    Some(&on_poll_select),
                    Some(&on_poll_refresh),
                    status.poll.as_ref().and_then(|p| self.pending_poll_votes.get(&p.id)),
                    Some(&self.account_id),
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
                    None,
                    Some(&on_quote),
                    Some(&on_account_click),
                    Some(&on_timestamp_click),
                    Some(&on_media_reload),
                    Some(&on_edit),
                    Some(&on_vote),
                    Some(&on_poll_select),
                    Some(&on_poll_refresh),
                    status.poll.as_ref().and_then(|p| self.pending_poll_votes.get(&p.id)),
                    Some(&self.account_id),
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
