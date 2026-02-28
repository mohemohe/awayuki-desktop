use std::sync::Arc;

use gpui::prelude::*;
use gpui::{div, img, px, rgb, AnyElement, App, ObjectFit, SharedString, Window};
use gpui_component::{Icon, IconName, Sizable};
use gpui_component::text::TextView;

use crate::db::models::{DbAccount, DbStatus};
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::{MediaAttachment, Status};

/// Data for a reply target (used for reply preview in compose bar)
#[derive(Clone)]
pub struct ReplyTarget {
    pub status_id: String,
    pub display_name: String,
    pub acct: String,
    pub content: String,
    pub visibility: String,
}

/// A rendered status item for display in timelines
pub struct StatusItemData {
    pub id: String,
    pub account_id: String,
    pub display_name: SharedString,
    pub acct: SharedString,
    pub avatar_url: SharedString,
    pub content: SharedString,
    pub timestamp: SharedString,
    pub reblogs_count: i64,
    pub favourites_count: i64,
    pub replies_count: i64,
    pub reblogged: bool,
    pub favourited: bool,
    pub reblogged_by: Option<SharedString>,
    pub reblogged_by_avatar: Option<SharedString>,
    pub notification_label: Option<SharedString>,
    pub notification_avatar: Option<SharedString>,
    /// Notification metadata for deduplication (detecting undo favourite/reblog)
    pub notification_type: Option<NotificationType>,
    pub notification_account_id: Option<String>,
    pub notification_status_id: Option<String>,
    pub visibility: SharedString,
    pub sensitive: bool,
    pub spoiler_text: SharedString,
    pub has_media: bool,
    pub media_attachments: Vec<MediaAttachment>,
}

impl StatusItemData {
    pub fn from_db(status: &DbStatus, account: Option<&DbAccount>) -> Self {
        let (display_name, acct, avatar_url) = if let Some(acc) = account {
            (
                acc.display_name.clone(),
                format!("@{}", acc.acct),
                acc.avatar.clone(),
            )
        } else {
            (
                status.account_id.clone(),
                format!("@{}", status.account_id),
                String::new(),
            )
        };

        let media: Vec<MediaAttachment> = status
            .media_attachments_json
            .as_ref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();

        Self {
            id: status.id.clone(),
            account_id: status.account_id.clone(),
            display_name: display_name.into(),
            acct: acct.into(),
            avatar_url: avatar_url.into(),
            content: status.content.clone().into(),
            timestamp: chrono::DateTime::parse_from_rfc3339(&status.created_at)
                .map(|dt| format_absolute_time(&dt.with_timezone(&chrono::Utc)))
                .unwrap_or_else(|_| status.created_at.clone())
                .into(),
            reblogs_count: status.reblogs_count,
            favourites_count: status.favourites_count,
            replies_count: status.replies_count,
            reblogged: status.reblogged.unwrap_or(false),
            favourited: status.favourited.unwrap_or(false),
            reblogged_by: None,
            reblogged_by_avatar: None,
            notification_label: None,
            notification_avatar: None,
            notification_type: None,
            notification_account_id: None,
            notification_status_id: None,
            visibility: status.visibility.clone().into(),
            sensitive: status.sensitive,
            spoiler_text: status.spoiler_text.clone().into(),
            has_media: !media.is_empty(),
            media_attachments: media,
        }
    }

    pub fn from_status(status: &Status) -> Self {
        // If this is a reblog, show the original status but note who reblogged it
        let (display_status, reblogged_by, reblogged_by_avatar) = if let Some(ref reblog) = status.reblog {
            (
                reblog.as_ref(),
                Some(SharedString::from(status.account.display_name.clone())),
                Some(SharedString::from(status.account.avatar.clone())),
            )
        } else {
            (status, None, None)
        };

        Self {
            id: status.id.clone(),
            account_id: display_status.account.id.clone(),
            display_name: display_status.account.display_name.clone().into(),
            acct: format!("@{}", display_status.account.acct).into(),
            avatar_url: display_status.account.avatar.clone().into(),
            content: display_status.content.clone().into(),
            timestamp: format_absolute_time(&display_status.created_at).into(),
            reblogs_count: display_status.reblogs_count,
            favourites_count: display_status.favourites_count,
            replies_count: display_status.replies_count,
            reblogged: display_status.reblogged.unwrap_or(false),
            favourited: display_status.favourited.unwrap_or(false),
            reblogged_by,
            reblogged_by_avatar,
            notification_label: None,
            notification_avatar: None,
            notification_type: None,
            notification_account_id: None,
            notification_status_id: None,
            visibility: display_status.visibility.clone().into(),
            sensitive: display_status.sensitive,
            spoiler_text: display_status.spoiler_text.clone().into(),
            has_media: !display_status.media_attachments.is_empty(),
            media_attachments: display_status.media_attachments.clone(),
        }
    }

    pub fn from_notification(notification: &Notification) -> Self {
        let label = match notification.notification_type {
            NotificationType::Mention => format!("💬 {} mentioned you", notification.account.display_name),
            NotificationType::Reblog => format!("🔁 {} boosted", notification.account.display_name),
            NotificationType::Favourite => format!("⭐ {} favourited", notification.account.display_name),
            NotificationType::Follow => format!("👤 {} followed you", notification.account.display_name),
            NotificationType::FollowRequest => format!("👤 {} requested to follow", notification.account.display_name),
            NotificationType::Poll => "📊 Poll ended".to_string(),
            NotificationType::Status => format!("📝 {} posted", notification.account.display_name),
            NotificationType::Update => format!("✏️ {} edited", notification.account.display_name),
            _ => format!("{} notification", notification.account.display_name),
        };

        let notification_avatar = Some(SharedString::from(notification.account.avatar.clone()));

        let notif_type = Some(notification.notification_type.clone());
        let notif_account_id = Some(notification.account.id.clone());
        let notif_status_id = notification.status.as_ref().map(|s| s.id.clone());

        if let Some(ref status) = notification.status {
            let mut item = Self::from_status(status);
            item.id = notification.id.clone();
            item.notification_label = Some(label.into());
            item.notification_avatar = notification_avatar;
            item.notification_type = notif_type;
            item.notification_account_id = notif_account_id;
            item.notification_status_id = notif_status_id;
            item.timestamp = format_absolute_time(&notification.created_at).into();
            item
        } else {
            // Follow / follow_request: no status attached
            Self {
                id: notification.id.clone(),
                account_id: notification.account.id.clone(),
                display_name: notification.account.display_name.clone().into(),
                acct: format!("@{}", notification.account.acct).into(),
                avatar_url: notification.account.avatar.clone().into(),
                content: SharedString::default(),
                timestamp: format_absolute_time(&notification.created_at).into(),
                reblogs_count: 0,
                favourites_count: 0,
                replies_count: 0,
                reblogged: false,
                favourited: false,
                reblogged_by: None,
                reblogged_by_avatar: None,
                notification_label: Some(label.into()),
                notification_avatar,
                notification_type: notif_type,
                notification_account_id: notif_account_id,
                notification_status_id: notif_status_id,
                visibility: "public".into(),
                sensitive: false,
                spoiler_text: SharedString::default(),
                has_media: false,
                media_attachments: Vec::new(),
            }
        }
    }
}

/// Render a status item as a GPUI element
pub fn render_status_item(
    data: &StatusItemData,
    cw_expanded: bool,
    on_cw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_account_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let image_attachments: Vec<&MediaAttachment> = data
        .media_attachments
        .iter()
        .filter(|m| m.media_type == "image")
        .collect();

    div()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .border_b_1()
        .border_color(rgb(0x313244))
        .flex()
        .flex_col()
        .gap(px(4.0))
        // Notification indicator
        .when(data.notification_label.is_some(), |el| {
            let label = data
                .notification_label
                .as_ref()
                .map(|n| n.to_string())
                .unwrap_or_default();
            let avatar_url = data
                .notification_avatar
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default();
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .text_color(rgb(0x89b4fa))
                    .pl(px(40.0))
                    .when(!avatar_url.is_empty(), |el| {
                        el.child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .rounded(px(8.0))
                                .overflow_hidden()
                                .flex_shrink_0()
                                .child(
                                    img(avatar_url)
                                        .w(px(16.0))
                                        .h(px(16.0))
                                        .object_fit(ObjectFit::Cover),
                                ),
                        )
                    })
                    .child(label),
            )
        })
        // Reblog indicator
        .when(data.reblogged_by.is_some() && data.notification_label.is_none(), |el| {
            let name = data
                .reblogged_by
                .as_ref()
                .map(|n| n.to_string())
                .unwrap_or_default();
            let avatar_url = data
                .reblogged_by_avatar
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_default();
            el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .text_color(rgb(0x6c7086))
                    .pl(px(40.0))
                    .child(Icon::default().path("icons/repeat-2.svg").xsmall())
                    .when(!avatar_url.is_empty(), |el| {
                        el.child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .rounded(px(8.0))
                                .overflow_hidden()
                                .flex_shrink_0()
                                .child(
                                    img(avatar_url)
                                        .w(px(16.0))
                                        .h(px(16.0))
                                        .object_fit(ObjectFit::Cover),
                                ),
                        )
                    })
                    .child(format!("{} boosted", name)),
            )
        })
        // Header: avatar placeholder + name + acct + time
        .child(
            div()
                .flex()
                .gap(px(8.0))
                .items_start()
                // Avatar
                .child({
                    let mut avatar = div()
                        .id(SharedString::from(format!("avatar-{}", data.id)))
                        .w(px(36.0))
                        .h(px(36.0))
                        .rounded(px(18.0))
                        .overflow_hidden()
                        .flex_shrink_0()
                        .child(
                            img(data.avatar_url.to_string())
                                .w(px(36.0))
                                .h(px(36.0))
                                .object_fit(ObjectFit::Cover),
                        );
                    if let Some(cb) = on_account_click {
                        let cb = cb.clone();
                        let account_id = data.account_id.clone();
                        avatar = avatar.cursor_pointer().on_click(move |_, window, cx| {
                            cb(account_id.clone(), window, cx);
                        });
                    }
                    avatar
                })
                // Name + content column
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        // Name row
                        .child(
                            div()
                                .flex()
                                .gap(px(4.0))
                                .items_baseline()
                                .w_full()
                                .overflow_hidden()
                                // Display name (truncatable)
                                .child({
                                    let mut name_el = div()
                                        .id(SharedString::from(format!("name-{}", data.id)))
                                        .flex_shrink()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_sm()
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(0xcdd6f4))
                                        .child(data.display_name.clone());
                                    if let Some(cb) = on_account_click {
                                        let cb = cb.clone();
                                        let account_id = data.account_id.clone();
                                        name_el = name_el.cursor_pointer().on_click(move |_, window, cx| {
                                            cb(account_id.clone(), window, cx);
                                        });
                                    }
                                    name_el
                                })
                                // Acct (truncatable, shrinks first)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_xs()
                                        .text_color(rgb(0x6c7086))
                                        .child(data.acct.clone()),
                                )
                                // Visibility icon + Timestamp (fixed)
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        .child(visibility_icon(&data.visibility))
                                        .child(
                                            div()
                                                .whitespace_nowrap()
                                                .text_xs()
                                                .text_color(rgb(0x585b70))
                                                .child(data.timestamp.clone()),
                                        ),
                                ),
                        )
                        // Content warning
                        .when(!data.spoiler_text.is_empty(), |el| {
                            let toggle_label = if cw_expanded { "▼" } else { "▶" };
                            let mut cw_row = div()
                                .id(SharedString::from(format!("cw-{}", data.id)))
                                .flex()
                                .gap(px(4.0))
                                .items_center()
                                .cursor_pointer()
                                .text_sm()
                                .text_color(rgb(0xf9e2af))
                                .child(format!("{} CW: {}", toggle_label, data.spoiler_text));

                            if let Some(cb) = on_cw_toggle {
                                let cb = cb.clone();
                                let id = data.id.clone();
                                cw_row = cw_row.on_click(move |_, window, cx| {
                                    cb(id.clone(), window, cx);
                                });
                            }

                            el.child(cw_row)
                        })
                        // Content (HTML rendered) - show when no CW or when CW is expanded
                        .when(data.spoiler_text.is_empty() || cw_expanded, |el| {
                            // Split on <br> tags and render each part as a separate
                            // TextView to work around gpui-component's HTML parser
                            // not handling <br> tags properly.
                            // Each TextView gets .h_auto() to override the internal
                            // size_full() that would otherwise break multi-element layout.
                            let normalized = data.content
                                .replace("<br />", "<br>")
                                .replace("<br/>", "<br>");
                            let parts: Vec<&str> = normalized.split("<br>").collect();
                            let text_views: Vec<gpui::AnyElement> = parts
                                .iter()
                                .enumerate()
                                .map(|(i, part)| {
                                    TextView::html(
                                        SharedString::from(format!("status-{}-{}", data.id, i)),
                                        SharedString::from(part.to_string()),
                                        window,
                                        cx,
                                    )
                                    .h_auto()
                                    .into_any_element()
                                })
                                .collect();
                            el.child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xbac2de))
                                    .children(text_views),
                            )
                        })
                        // Media thumbnails
                        .when(!image_attachments.is_empty(), |el| {
                            el.child(render_media_thumbnails(
                                &data.id,
                                &image_attachments,
                                on_media_click,
                            ))
                        })
                        // Action bar
                        .child(render_action_bar(data, on_reply, on_reblog, on_favourite)),
                ),
        )
        .into_any_element()
}

/// Render media thumbnails grid
fn render_media_thumbnails(
    status_id: &str,
    attachments: &[&MediaAttachment],
    on_media_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
) -> gpui::Div {
    let mut container = div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .pt(px(4.0));

    for (i, media) in attachments.iter().enumerate() {
        let preview_url = media
            .preview_url
            .clone()
            .or_else(|| media.url.clone())
            .unwrap_or_default();
        let full_url = media
            .url
            .clone()
            .or_else(|| media.remote_url.clone())
            .unwrap_or_default();

        let mut thumb = div()
            .id(SharedString::from(format!("thumb-{}-{}", status_id, i)))
            .w(px(120.0))
            .h(px(90.0))
            .rounded(px(4.0))
            .overflow_hidden()
            .cursor_pointer()
            .bg(rgb(0x313244))
            .child(
                img(preview_url)
                    .w(px(120.0))
                    .h(px(90.0))
                    .object_fit(ObjectFit::Cover),
            );

        if let Some(callback) = on_media_click {
            let cb = callback.clone();
            thumb = thumb.on_click(move |_, window, cx| {
                cb(full_url.clone(), window, cx);
            });
        }

        container = container.child(thumb);
    }

    container
}

/// Render the action bar (reply, boost, favourite buttons)
fn render_action_bar(
    data: &StatusItemData,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
) -> gpui::Div {
    let mut reply_btn = div()
        .id(SharedString::from(format!("reply-{}", data.id)))
        .flex()
        .gap(px(4.0))
        .items_center()
        .text_xs()
        .text_color(rgb(0x6c7086))
        .cursor_pointer()
        .child(Icon::default().path("icons/message-circle.svg").xsmall())
        .when(data.replies_count > 0, |el| el.child(data.replies_count.to_string()));

    if let Some(cb) = on_reply {
        let cb = cb.clone();
        let target = ReplyTarget {
            status_id: data.id.clone(),
            display_name: data.display_name.to_string(),
            acct: data.acct.to_string(),
            content: data.content.to_string(),
            visibility: data.visibility.to_string(),
        };
        reply_btn = reply_btn.on_click(move |_, window, cx| {
            cb(target.clone(), window, cx);
        });
    }

    let reblog_color = if data.reblogged { rgb(0xa6e3a1) } else { rgb(0x6c7086) };
    let mut reblog_btn = div()
        .id(SharedString::from(format!("reblog-{}", data.id)))
        .flex()
        .gap(px(4.0))
        .items_center()
        .text_xs()
        .text_color(reblog_color)
        .cursor_pointer()
        .child(Icon::default().path("icons/repeat-2.svg").xsmall().text_color(reblog_color))
        .when(data.reblogs_count > 0, |el| el.child(data.reblogs_count.to_string()));

    if let Some(cb) = on_reblog {
        let cb = cb.clone();
        let id = data.id.clone();
        reblog_btn = reblog_btn.on_click(move |_, window, cx| {
            cb(id.clone(), window, cx);
        });
    }

    let fav_color = if data.favourited { rgb(0xf9e2af) } else { rgb(0x6c7086) };
    let mut fav_btn = div()
        .id(SharedString::from(format!("fav-{}", data.id)))
        .flex()
        .gap(px(4.0))
        .items_center()
        .text_xs()
        .text_color(fav_color)
        .cursor_pointer()
        .child(Icon::new(IconName::Star).xsmall().text_color(fav_color))
        .when(data.favourites_count > 0, |el| el.child(data.favourites_count.to_string()));

    if let Some(cb) = on_favourite {
        let cb = cb.clone();
        let id = data.id.clone();
        fav_btn = fav_btn.on_click(move |_, window, cx| {
            cb(id.clone(), window, cx);
        });
    }

    div()
        .flex()
        .gap(px(16.0))
        .pt(px(4.0))
        .child(reply_btn)
        .child(reblog_btn)
        .child(fav_btn)
}

/// Return an SVG icon element for the given visibility string
fn visibility_icon(visibility: &str) -> Icon {
    let path = match visibility {
        "unlisted" => "icons/lock-open.svg",
        "private" => "icons/lock.svg",
        "direct" => "icons/mail.svg",
        _ => "icons/globe.svg", // public or unknown
    };
    Icon::default().path(path).xsmall().text_color(rgb(0x585b70))
}

/// Format a timestamp as absolute local time.
/// Same day: "HH:mm:ss", earlier days: "YYYY/MM/DD HH:mm:ss"
fn format_absolute_time(dt: &chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Local;

    let local = dt.with_timezone(&Local);
    let today = Local::now().date_naive();

    if local.date_naive() == today {
        local.format("%H:%M:%S").to_string()
    } else {
        local.format("%Y/%m/%d %H:%M:%S").to_string()
    }
}
