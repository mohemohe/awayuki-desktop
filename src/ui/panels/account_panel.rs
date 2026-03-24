use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    div, img, point, px, rgb, AnyElement, App, AsyncApp, Context, EventEmitter, FocusHandle,
    Focusable, IntoElement, ScrollHandle, SharedString, WeakEntity, Window,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{Panel, PanelEvent};
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, IconName, Sizable};
use gpui_component::WindowExt;
use gpui_tokio_bridge::Tokio;

use crate::mastodon::client::MastodonClient;
use crate::mastodon::endpoints::accounts::AccountStatusesParams;
use crate::state::confirmation::ConfirmationSettings;
use crate::mastodon::types::account::{Account, Relationship};
use crate::state::appearance::AppearanceSettings;
use crate::ui::components::html_content::{render_html_content, render_plain_with_emojis};
use crate::ui::components::status_item::{render_status_item, EmojiMapping, QuoteTarget, ReplyTarget, StatusItemData};
use crate::ui::panels::timeline_panel::QuoteState;
use crate::ui::workspace::ClosePanelRequest;

/// Global state for requesting an account detail panel
#[derive(Default, Clone)]
pub struct AccountDetailRequest {
    pub account_id: Option<String>,
}

impl gpui::Global for AccountDetailRequest {}

pub struct AccountPanel {
    account_id: String,
    own_account_id: String,
    account: Option<Account>,
    relationship: Option<Relationship>,
    pinned_statuses: Vec<StatusItemData>,
    statuses: Vec<StatusItemData>,
    client: MastodonClient,
    loading: bool,
    follow_in_progress: bool,
    oldest_id: Option<String>,
    expanded_cw: HashSet<String>,
    revealed_nsfw: HashSet<String>,
    retry_media: HashMap<String, u64>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
}

impl AccountPanel {
    pub fn new(
        account_id: String,
        own_account_id: String,
        client: MastodonClient,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self {
            account_id,
            own_account_id,
            account: None,
            relationship: None,
            pinned_statuses: Vec::new(),
            statuses: Vec::new(),
            client,
            loading: true,
            follow_in_progress: false,
            oldest_id: None,
            expanded_cw: HashSet::new(),
            revealed_nsfw: HashSet::new(),
            retry_media: HashMap::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
        };
        panel.load_account(cx);
        panel
    }

    fn is_own_account(&self) -> bool {
        self.account_id == self.own_account_id
    }

    fn load_account(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let account_id = self.account_id.clone();
        let is_own = self.is_own_account();

        let task = Tokio::spawn(cx, async move {
            let account = client.get_account(&account_id).await
                .map_err(|e| e.to_string())?;

            let relationship = if !is_own {
                client.get_relationships(&[&account_id]).await
                    .ok()
                    .and_then(|rels| rels.into_iter().next())
            } else {
                None
            };

            let pinned = client.get_account_statuses(&account_id, &AccountStatusesParams {
                pinned: Some(true),
                limit: Some(20),
                ..Default::default()
            }).await.unwrap_or_default();

            let statuses = client.get_account_statuses(&account_id, &AccountStatusesParams {
                exclude_replies: Some(true),
                limit: Some(20),
                ..Default::default()
            }).await.unwrap_or_default();

            Ok::<_, String>((account, relationship, pinned, statuses))
        });

        cx.spawn(async move |this: WeakEntity<AccountPanel>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok((account, relationship, pinned, statuses))) => {
                    let _ = this.update(cx, |this, cx| {
                        this.account = Some(account);
                        this.relationship = relationship;
                        this.pinned_statuses = pinned.iter().map(StatusItemData::from_status).collect();
                        if let Some(last) = statuses.last() {
                            this.oldest_id = Some(last.id.clone());
                        }
                        this.statuses = statuses.iter().map(StatusItemData::from_status).collect();
                        this.loading = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Failed to load account: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Account load task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(max_id) = self.oldest_id.clone() else {
            return;
        };

        self.loading = true;
        cx.notify();

        let client = self.client.clone();
        let account_id = self.account_id.clone();

        let task = Tokio::spawn(cx, async move {
            client.get_account_statuses(&account_id, &AccountStatusesParams {
                max_id: Some(max_id),
                exclude_replies: Some(true),
                limit: Some(20),
                ..Default::default()
            }).await.map_err(|e| e.to_string())
        });

        cx.spawn(async move |this: WeakEntity<AccountPanel>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(statuses)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(last) = statuses.last() {
                            this.oldest_id = Some(last.id.clone());
                        }
                        let items: Vec<StatusItemData> =
                            statuses.iter().map(StatusItemData::from_status).collect();
                        this.statuses.extend(items);
                        this.loading = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Load more failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Load more task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn toggle_reblog(&mut self, status_id: String, cx: &mut Context<Self>) {
        let currently_reblogged = self.statuses.iter()
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

        cx.spawn(async move |this: WeakEntity<AccountPanel>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.reblogged = updated_status.reblogged.unwrap_or(!currently_reblogged);
                            item.reblogs_count = updated_status.reblogs_count;
                            cx.notify();
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Reblog toggle failed: {}", e),
                Err(e) => tracing::error!("Reblog task error: {}", e),
            }
        }).detach();
    }

    fn toggle_favourite(&mut self, status_id: String, cx: &mut Context<Self>) {
        let currently_favourited = self.statuses.iter()
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

        cx.spawn(async move |this: WeakEntity<AccountPanel>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(updated_status)) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(item) = this.statuses.iter_mut().find(|s| s.id == status_id) {
                            item.favourited = updated_status.favourited.unwrap_or(!currently_favourited);
                            item.favourites_count = updated_status.favourites_count;
                            cx.notify();
                        }
                    });
                }
                Ok(Err(e)) => tracing::error!("Favourite toggle failed: {}", e),
                Err(e) => tracing::error!("Favourite task error: {}", e),
            }
        }).detach();
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        if self.follow_in_progress || self.is_own_account() {
            return;
        }

        let currently_following = self.relationship
            .as_ref()
            .map(|r| r.following)
            .unwrap_or(false);

        self.follow_in_progress = true;
        cx.notify();

        let client = self.client.clone();
        let account_id = self.account_id.clone();

        let task = Tokio::spawn(cx, async move {
            if currently_following {
                client.unfollow_account(&account_id).await.map_err(|e| e.to_string())
            } else {
                client.follow_account(&account_id).await.map_err(|e| e.to_string())
            }
        });

        cx.spawn(async move |this: WeakEntity<AccountPanel>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(rel)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.relationship = Some(rel);
                        this.follow_in_progress = false;
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    tracing::error!("Follow toggle failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.follow_in_progress = false;
                        cx.notify();
                    });
                }
                Err(e) => {
                    tracing::error!("Follow task error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.follow_in_progress = false;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn render_profile(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(account) = &self.account else {
            return vec![];
        };

        let mut elements: Vec<AnyElement> = Vec::new();

        // Header image
        let has_header = !account.header.is_empty() && !account.header.ends_with("missing.png");
        if has_header {
            elements.push(
                div()
                    .w_full()
                    .h(px(150.0))
                    .overflow_hidden()
                    .child(
                        img(account.header.clone())
                            .w_full()
                            .h(px(150.0))
                            .object_fit(gpui::ObjectFit::Cover)
                            .with_loading(|| {
                                div()
                                    .w_full()
                                    .h(px(150.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(Spinner::new().small())
                                    .into_any_element()
                            })
                            .with_fallback(|| div().into_any_element()),
                    )
                    .into_any_element(),
            );
        }

        // Avatar + follow button row
        let is_own = self.is_own_account();
        let following = self.relationship.as_ref().map(|r| r.following).unwrap_or(false);
        let follow_in_progress = self.follow_in_progress;

        let mut avatar_row = div()
            .flex()
            .items_end()
            .justify_between()
            .px(px(12.0));

        if has_header {
            avatar_row = avatar_row.mt(px(-32.0));
        } else {
            avatar_row = avatar_row.pt(px(12.0));
        }

        // Avatar
        let avatar_radius = px(cx.global::<AppearanceSettings>().avatar_shape.radius(64.0));
        avatar_row = avatar_row.child(
            div()
                .w(px(64.0))
                .h(px(64.0))
                .rounded(avatar_radius)
                .overflow_hidden()
                .border_2()
                .border_color(rgb(0x1e1e2e))
                .child(
                    img(account.avatar.clone())
                        .w(px(64.0))
                        .h(px(64.0))
                        .rounded(avatar_radius)
                        .object_fit(gpui::ObjectFit::Cover)
                        .with_loading(|| Spinner::new().small().into_any_element())
                        .with_fallback(|| {
                            Icon::new(IconName::TriangleAlert)
                                .small()
                                .text_color(rgb(0x6c7086))
                                .into_any_element()
                        }),
                ),
        );

        // Follow button or "This is you"
        if is_own {
            avatar_row = avatar_row.child(
                div()
                    .text_xs()
                    .text_color(rgb(0x6c7086))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x313244))
                    .child("This is you"),
            );
        } else {
            let follow_label = if following { "Following" } else { "Follow" };
            let mut btn = Button::new("follow-btn")
                .small()
                .loading(follow_in_progress);
            if following {
                btn = btn.ghost().label(follow_label);
            } else {
                btn = btn.primary().label(follow_label);
            }
            btn = btn.on_click(cx.listener(|this, _, window, cx| {
                let confirmation = cx
                    .try_global::<ConfirmationSettings>()
                    .cloned()
                    .unwrap_or_default();
                let currently_following = this
                    .relationship
                    .as_ref()
                    .map(|r| r.following)
                    .unwrap_or(false);

                let needs_confirm = if currently_following {
                    confirmation.confirm_unfollow
                } else {
                    confirmation.confirm_follow
                };

                if needs_confirm {
                    let weak = cx.entity().downgrade();
                    let msg = if currently_following {
                        "Unfollow this account?"
                    } else {
                        "Follow this account?"
                    };
                    window.open_dialog(cx, move |dialog, _, _| {
                        let weak = weak.clone();
                        dialog.confirm().child(msg).on_ok(move |_, _window, cx| {
                            if let Some(entity) = weak.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.toggle_follow(cx);
                                });
                            }
                            true
                        })
                    });
                } else {
                    this.toggle_follow(cx);
                }
            }));
            avatar_row = avatar_row.child(btn);
        }

        elements.push(avatar_row.into_any_element());

        // Display name (with inline custom emojis)
        let account_emojis: Vec<EmojiMapping> = account
            .emojis
            .iter()
            .map(|e| EmojiMapping {
                shortcode: e.shortcode.clone(),
                url: e.url.clone(),
            })
            .collect();
        let name_size = px(20.0); // text_xl equivalent
        let name_inline_els = render_plain_with_emojis(
            &format!("account-name-{}", self.account_id),
            &account.display_name,
            &account_emojis,
            name_size,
        );
        elements.push(
            div()
                .px(px(12.0))
                .pt(px(8.0))
                .flex()
                .items_center()
                .flex_wrap()
                .text_xl()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(0xcdd6f4))
                .children(name_inline_els)
                .into_any_element(),
        );

        // Acct
        elements.push(
            div()
                .px(px(12.0))
                .text_sm()
                .text_color(rgb(0x6c7086))
                .child(format!("@{}", account.acct))
                .into_any_element(),
        );

        // Bio (with inline custom emojis)
        if !account.note.is_empty() {
            let bio_size = px(14.0); // text_sm equivalent
            let bio_els = render_html_content(
                &format!("bio-{}", self.account_id),
                &account.note,
                &account_emojis,
                bio_size,
            );
            elements.push(
                div()
                    .px(px(12.0))
                    .pt(px(8.0))
                    .text_sm()
                    .text_color(rgb(0xbac2de))
                    .children(bio_els)
                    .into_any_element(),
            );
        }

        // Profile fields
        if !account.fields.is_empty() {
            let mut fields_container = div()
                .mx(px(12.0))
                .mt(px(8.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(0x313244))
                .flex()
                .flex_col();

            for (i, field) in account.fields.iter().enumerate() {
                let is_verified = field.verified_at.is_some();
                let border_color = if is_verified { rgb(0xa6e3a1) } else { rgb(0x313244) };

                let mut field_row = div()
                    .flex()
                    .flex_col()
                    .px(px(8.0))
                    .py(px(6.0))
                    .border_l_2()
                    .border_color(border_color);

                if i > 0 {
                    field_row = field_row.border_t_1().border_color(rgb(0x313244));
                }

                // Field name
                field_row = field_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x6c7086))
                        .child(field.name.clone()),
                );

                // Field value (may contain HTML links)
                let value_color = if is_verified { rgb(0xa6e3a1) } else { rgb(0xcdd6f4) };
                let field_size = px(14.0); // text_sm
                let field_els = render_html_content(
                    &format!("field-{}-{}", self.account_id, i),
                    &field.value,
                    &account_emojis,
                    field_size,
                );
                field_row = field_row.child(
                    div()
                        .text_sm()
                        .text_color(value_color)
                        .children(field_els),
                );

                fields_container = fields_container.child(field_row);
            }

            elements.push(fields_container.into_any_element());
        }

        // Stats
        elements.push(
            div()
                .flex()
                .gap(px(16.0))
                .px(px(12.0))
                .pt(px(8.0))
                .pb(px(8.0))
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_sm()
                                .text_color(rgb(0xcdd6f4))
                                .child(format_count(account.statuses_count)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x6c7086))
                                .child("Posts"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_sm()
                                .text_color(rgb(0xcdd6f4))
                                .child(format_count(account.following_count)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x6c7086))
                                .child("Following"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_sm()
                                .text_color(rgb(0xcdd6f4))
                                .child(format_count(account.followers_count)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x6c7086))
                                .child("Followers"),
                        ),
                )
                .into_any_element(),
        );

        // Separator
        elements.push(
            div()
                .w_full()
                .h(px(1.0))
                .bg(rgb(0x313244))
                .into_any_element(),
        );

        elements
    }
}

impl EventEmitter<PanelEvent> for AccountPanel {}

impl Focusable for AccountPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AccountPanel {
    fn panel_name(&self) -> &'static str {
        "AccountPanel"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(ref account) = self.account {
            SharedString::from(format!("@{}", account.acct))
        } else {
            SharedString::from("Loading...")
        }
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

impl Render for AccountPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Build account click callback (for statuses within this panel)
        let on_account_click: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|account_id: String, _window: &mut Window, cx: &mut App| {
                cx.set_global(AccountDetailRequest {
                    account_id: Some(account_id),
                });
            });

        // Build CW toggle callback
        let entity = cx.entity().downgrade();
        let on_cw_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity.update(cx, |this, cx| {
                    if !this.expanded_cw.remove(&id) {
                        this.expanded_cw.insert(id);
                    }
                    cx.notify();
                });
            });

        // Build NSFW toggle callback
        let entity_nsfw = cx.entity().downgrade();
        let on_nsfw_toggle: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(move |id: String, _window: &mut Window, cx: &mut App| {
                let _ = entity_nsfw.update(cx, |this, cx| {
                    if !this.revealed_nsfw.remove(&id) {
                        this.revealed_nsfw.insert(id);
                    }
                    cx.notify();
                });
            });

        // Build media click callback
        let on_media: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|url: String, _window: &mut Window, cx: &mut App| {
                use crate::ui::panels::timeline_panel::LightboxState;
                cx.set_global(LightboxState { url: Some(url), local_path: None });
            });

        // Build reply callback — sets global ReplyState
        let on_reply: Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)> =
            Arc::new(|target: ReplyTarget, _window: &mut Window, cx: &mut App| {
                use crate::ui::panels::timeline_panel::ReplyState;
                cx.set_global(ReplyState { target: Some(target) });
            });

        // Build reblog callback
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

        // Build favourite callback
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

        let on_timestamp_click: Arc<dyn Fn(String, &mut Window, &mut App)> =
            Arc::new(|status_id: String, _window: &mut Window, cx: &mut App| {
                use crate::ui::panels::status_detail_panel::StatusDetailRequest;
                cx.set_global(StatusDetailRequest {
                    status_id: Some(status_id),
                });
            });

        let on_quote: Arc<dyn Fn(QuoteTarget, &mut Window, &mut App)> =
            Arc::new(|target: QuoteTarget, _window: &mut Window, cx: &mut App| {
                cx.set_global(QuoteState {
                    target: Some(target),
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

        // Render profile section
        let profile_elements = self.render_profile(window, cx);

        // Render pinned statuses
        let mut pinned_elements: Vec<AnyElement> = Vec::new();
        if !self.pinned_statuses.is_empty() {
            pinned_elements.push(
                div()
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .text_color(rgb(0x6c7086))
                    .child("Pinned posts")
                    .into_any_element(),
            );
            for status in &self.pinned_statuses {
                let expanded = self.expanded_cw.contains(&status.id);
                let nsfw_revealed = self.revealed_nsfw.contains(&status.id);
                pinned_elements.push(render_status_item(
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
                    None,
                    None,
                    None,
                    None,
                    None,
                    &self.retry_media,
                    window,
                    cx,
                ));
            }
        }

        // Render statuses
        let status_elements: Vec<AnyElement> = self
            .statuses
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
                    None,
                    None,
                    None,
                    None,
                    None,
                    &self.retry_media,
                    window,
                    cx,
                )
            })
            .collect();

        // Load more button
        let entity_load = cx.entity().downgrade();
        let on_load_more: Arc<dyn Fn(&mut Window, &mut App)> =
            Arc::new(move |_window: &mut Window, cx: &mut App| {
                let _ = entity_load.update(cx, |this, cx| {
                    this.load_more(cx);
                });
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
                    .id("account-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(profile_elements)
                    .children(pinned_elements)
                    .children(status_elements)
                    // Load more button
                    .when(!self.statuses.is_empty() && self.oldest_id.is_some() && !self.loading, |el| {
                        let cb = on_load_more.clone();
                        el.child(
                            div()
                                .id("load-more-account")
                                .w_full()
                                .py(px(12.0))
                                .flex()
                                .justify_center()
                                .cursor_pointer()
                                .child(
                                    Button::new("load-more-btn")
                                        .ghost()
                                        .label("Load more")
                                        .on_click(move |_, window, cx| {
                                            cb(window, cx);
                                        }),
                                ),
                        )
                    })
                    // Loading indicator
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

fn format_count(count: i64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}
