use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{div, img, px, rgb, rgba, AnyElement, App, ClipboardItem, Corner, ObjectFit, RenderImage, SharedString, Window};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::{Icon, IconName, Sizable, Size};
use gpui_component::spinner::Spinner;

use crate::ui::components::html_content::{render_html_content, render_plain_with_emojis, html_to_plain_text};

use crate::db::models::{DbAccount, DbStatus};
use crate::mastodon::types::account::CustomEmoji;
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::{MediaAttachment, Status};
use crate::state::appearance::{AppearanceSettings, AvatarShape, CwBehavior, NsfwBehavior};

/// Data for a reply target (used for reply preview in compose bar)
#[derive(Clone)]
pub struct ReplyTarget {
    pub status_id: String,
    pub display_name: String,
    pub acct: String,
    pub content: String,
    pub visibility: String,
}

/// Shortcode-URL pair for custom emoji rendering.
#[derive(Clone)]
pub struct EmojiMapping {
    pub shortcode: String,
    pub url: String,
}

/// A rendered status item for display in timelines
pub struct StatusItemData {
    pub id: String,
    /// The actual status ID for API calls (e.g. get_status_context).
    /// For reblogs, this is the original status ID, not the reblog wrapper ID.
    pub original_status_id: String,
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
    pub reblogged_by_account_id: Option<String>,
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
    /// Custom emojis from both the status content and account display_name.
    pub emojis: Vec<EmojiMapping>,
    /// The URL of this status (for "Copy URL" action).
    pub url: Option<String>,
}

impl StatusItemData {
    pub fn from_db(status: &DbStatus, account: Option<&DbAccount>) -> Self {
        let account_emojis: Vec<CustomEmoji> = account
            .and_then(|acc| acc.emojis_json.as_ref())
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();

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

        let status_emojis: Vec<CustomEmoji> = status
            .emojis_json
            .as_ref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();

        let mut all_emojis: Vec<EmojiMapping> = status_emojis
            .iter()
            .chain(account_emojis.iter())
            .map(|e| EmojiMapping {
                shortcode: e.shortcode.clone(),
                url: e.url.clone(),
            })
            .collect();
        all_emojis.dedup_by(|a, b| a.shortcode == b.shortcode);

        let media: Vec<MediaAttachment> = status
            .media_attachments_json
            .as_ref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();

        Self {
            id: status.id.clone(),
            original_status_id: status.id.clone(),
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
            reblogged_by_account_id: None,
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
            emojis: all_emojis,
            url: status.url.clone(),
        }
    }

    pub fn from_status(status: &Status) -> Self {
        // If this is a reblog, show the original status but note who reblogged it
        let (display_status, reblogged_by, reblogged_by_avatar, reblogged_by_account_id) = if let Some(ref reblog) = status.reblog {
            (
                reblog.as_ref(),
                Some(SharedString::from(status.account.display_name.clone())),
                Some(SharedString::from(status.account.avatar.clone())),
                Some(status.account.id.clone()),
            )
        } else {
            (status, None, None, None)
        };

        let mut all_emojis: Vec<EmojiMapping> = display_status
            .emojis
            .iter()
            .chain(display_status.account.emojis.iter())
            .map(|e| EmojiMapping {
                shortcode: e.shortcode.clone(),
                url: e.url.clone(),
            })
            .collect();
        // Also include reblog author emojis for reblogged_by label
        if let Some(ref reblog) = status.reblog {
            for e in &status.account.emojis {
                if !all_emojis.iter().any(|x| x.shortcode == e.shortcode) {
                    all_emojis.push(EmojiMapping {
                        shortcode: e.shortcode.clone(),
                        url: e.url.clone(),
                    });
                }
            }
            let _ = reblog; // suppress unused warning
        }
        all_emojis.dedup_by(|a, b| a.shortcode == b.shortcode);

        Self {
            id: status.id.clone(),
            original_status_id: display_status.id.clone(),
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
            reblogged_by_account_id,
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
            emojis: all_emojis,
            url: display_status.url.clone(),
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

        let notif_emojis: Vec<EmojiMapping> = notification
            .account
            .emojis
            .iter()
            .map(|e| EmojiMapping {
                shortcode: e.shortcode.clone(),
                url: e.url.clone(),
            })
            .collect();

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
            // Merge notification account emojis
            for e in &notif_emojis {
                if !item.emojis.iter().any(|x| x.shortcode == e.shortcode) {
                    item.emojis.push(e.clone());
                }
            }
            item
        } else {
            // Follow / follow_request: no status attached
            Self {
                id: notification.id.clone(),
                original_status_id: notification.id.clone(),
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
                reblogged_by_account_id: None,
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
                emojis: notif_emojis,
                url: None,
            }
        }
    }
}

/// Render a status item as a GPUI element
pub fn render_status_item(
    data: &StatusItemData,
    cw_expanded: bool,
    nsfw_revealed: bool,
    on_cw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_nsfw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_account_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_timestamp_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    retry_media: &HashMap<String, u64>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let appearance = cx.global::<AppearanceSettings>();
    let avatar_radius_36 = px(appearance.avatar_shape.radius(36.0));
    let avatar_radius_16 = px(appearance.avatar_shape.radius(16.0));
    let content_size = px(appearance.font_size.content_px());
    let secondary_size = px(appearance.font_size.secondary_px());
    let effective_cw_expanded =
        cw_expanded || appearance.cw_behavior == CwBehavior::AlwaysExpand;
    let effective_nsfw_revealed =
        nsfw_revealed || appearance.nsfw_behavior == NsfwBehavior::AlwaysShow;

    let image_attachments: Vec<&MediaAttachment> = data
        .media_attachments
        .iter()
        .filter(|m| m.media_type == "image")
        .collect();

    let other_attachments: Vec<&MediaAttachment> = data
        .media_attachments
        .iter()
        .filter(|m| m.media_type != "image")
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
            let mut notif_row = div()
                .id(SharedString::from(format!("notif-{}", data.id)))
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(secondary_size)
                .text_color(rgb(0x89b4fa))
                .pl(px(40.0))
                .when(!avatar_url.is_empty(), |el| {
                    el.child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(avatar_radius_16)
                            .overflow_hidden()
                            .flex_shrink_0()
                            .child(
                                img(avatar_url)
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded(avatar_radius_16)
                                    .object_fit(ObjectFit::Cover)
                                    .with_fallback(|| {
                                        Icon::new(IconName::TriangleAlert)
                                            .with_size(Size::Size(px(10.0)))
                                            .text_color(rgb(0x6c7086))
                                            .into_any_element()
                                    }),
                            ),
                    )
                })
                .children(
                    render_plain_with_emojis(
                        &format!("notif-label-{}", data.id),
                        &label,
                        &data.emojis,
                        secondary_size,
                    )
                );
            if let (Some(cb), Some(ref acct_id)) = (on_account_click, &data.notification_account_id) {
                let cb = cb.clone();
                let acct_id = acct_id.clone();
                notif_row = notif_row.cursor_pointer().on_click(move |_, window, cx| {
                    cb(acct_id.clone(), window, cx);
                });
            }
            el.child(notif_row)
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
            let mut reblog_row = div()
                .id(SharedString::from(format!("reblog-row-{}", data.id)))
                .flex()
                .items_center()
                .gap(px(4.0))
                .text_size(secondary_size)
                .text_color(rgb(0x6c7086))
                .pl(px(40.0))
                .child(Icon::default().path("icons/repeat-2.svg").xsmall())
                .when(!avatar_url.is_empty(), |el| {
                    el.child(
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(avatar_radius_16)
                            .overflow_hidden()
                            .flex_shrink_0()
                            .child(
                                img(avatar_url)
                                    .w(px(16.0))
                                    .h(px(16.0))
                                    .rounded(avatar_radius_16)
                                    .object_fit(ObjectFit::Cover)
                                    .with_fallback(|| {
                                        Icon::new(IconName::TriangleAlert)
                                            .with_size(Size::Size(px(10.0)))
                                            .text_color(rgb(0x6c7086))
                                            .into_any_element()
                                    }),
                            ),
                    )
                })
                .children(
                    render_plain_with_emojis(
                        &format!("reblog-label-{}", data.id),
                        &format!("{} boosted", name),
                        &data.emojis,
                        secondary_size,
                    )
                );
            if let (Some(cb), Some(ref acct_id)) = (on_account_click, &data.reblogged_by_account_id) {
                let cb = cb.clone();
                let acct_id = acct_id.clone();
                reblog_row = reblog_row.cursor_pointer().on_click(move |_, window, cx| {
                    cb(acct_id.clone(), window, cx);
                });
            }
            el.child(reblog_row)
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
                        .rounded(avatar_radius_36)
                        .overflow_hidden()
                        .flex_shrink_0()
                        .child(
                            img(data.avatar_url.to_string())
                                .w(px(36.0))
                                .h(px(36.0))
                                .rounded(avatar_radius_36)
                                .object_fit(ObjectFit::Cover)
                                .with_loading(|| {
                                    Spinner::new().xsmall().into_any_element()
                                })
                                .with_fallback(|| {
                                    Icon::new(IconName::TriangleAlert)
                                        .xsmall()
                                        .text_color(rgb(0x6c7086))
                                        .into_any_element()
                                }),
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
                                // Display name (truncatable, rendered as HTML for custom emoji support)
                                .child({
                                    let name_inline_els = render_plain_with_emojis(
                                        &format!("display-name-{}", data.id),
                                        &data.display_name,
                                        &data.emojis,
                                        content_size,
                                    );
                                    let mut name_el = div()
                                        .id(SharedString::from(format!("name-{}", data.id)))
                                        .flex()
                                        .items_center()
                                        .flex_shrink()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(content_size)
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .text_color(rgb(0xcdd6f4))
                                        .children(name_inline_els);
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
                                        .text_size(secondary_size)
                                        .text_color(rgb(0x6c7086))
                                        .child(data.acct.clone()),
                                )
                                // Visibility icon + Timestamp (fixed, clickable)
                                .child({
                                    let mut timestamp_el = div()
                                        .id(SharedString::from(format!("ts-{}", data.id)))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(2.0))
                                        .child(visibility_icon(&data.visibility))
                                        .child(
                                            div()
                                                .whitespace_nowrap()
                                                .text_size(secondary_size)
                                                .text_color(rgb(0x585b70))
                                                .child(data.timestamp.clone()),
                                        );
                                    if let Some(cb) = on_timestamp_click {
                                        let cb = cb.clone();
                                        let original_id = data.original_status_id.clone();
                                        timestamp_el = timestamp_el
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| {
                                                cb(original_id.clone(), window, cx);
                                            });
                                    }
                                    timestamp_el
                                }),
                        )
                        // Content warning
                        .when(!data.spoiler_text.is_empty(), |el| {
                            let toggle_label = if effective_cw_expanded { "▼" } else { "▶" };
                            let mut cw_row = div()
                                .id(SharedString::from(format!("cw-{}", data.id)))
                                .flex()
                                .gap(px(4.0))
                                .items_center()
                                .cursor_pointer()
                                .text_size(content_size)
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
                        // Content (HTML rendered with inline custom emojis)
                        .when(data.spoiler_text.is_empty() || effective_cw_expanded, |el| {
                            let content_els = render_html_content(
                                &format!("status-{}", data.id),
                                &data.content,
                                &data.emojis,
                                content_size,
                            );
                            el.child(
                                div()
                                    .text_size(content_size)
                                    .text_color(rgb(0xbac2de))
                                    .children(content_els),
                            )
                        })
                        // Media thumbnails
                        .when(!image_attachments.is_empty(), |el| {
                            el.child(render_media_thumbnails(
                                &data.id,
                                &image_attachments,
                                data.sensitive,
                                effective_nsfw_revealed,
                                on_media_click,
                                on_nsfw_toggle,
                                on_media_reload,
                                retry_media,
                            ))
                        })
                        // Non-image media (video, audio, etc.)
                        .when(!other_attachments.is_empty(), |el| {
                            el.child(render_other_media(
                                &data.id,
                                &other_attachments,
                                data.sensitive,
                                effective_nsfw_revealed,
                                on_nsfw_toggle,
                                on_media_reload,
                                retry_media,
                            ))
                        })
                        // Action bar
                        .child(render_action_bar(data, on_reply, on_reblog, on_favourite)),
                ),
        )
        .into_any_element()
}

/// Append a cache-bust query parameter to a URL if a retry count exists
fn cache_bust_url(url: &str, retry_media: &HashMap<String, u64>) -> String {
    match retry_media.get(url) {
        Some(&count) if count > 0 => {
            let sep = if url.contains('?') { "&" } else { "?" };
            format!("{}{}_retry={}", url, sep, count)
        }
        _ => url.to_string(),
    }
}

/// Decode a blurhash string into a RenderImage for display
fn decode_blurhash_image(hash: &str) -> Option<Arc<RenderImage>> {
    let pixels = blurhash::decode(hash, 32, 24, 1.0).ok()?;
    let rgba = image::RgbaImage::from_raw(32, 24, pixels)?;
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

/// Render the NSFW toggle button overlay
fn render_nsfw_toggle(
    status_id: &str,
    index: usize,
    revealed: bool,
    on_nsfw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
) -> gpui::Stateful<gpui::Div> {
    let icon_name = if revealed { IconName::Eye } else { IconName::EyeOff };
    let mut toggle = div()
        .id(SharedString::from(format!("nsfw-toggle-{}-{}", status_id, index)))
        .absolute()
        .top(px(4.0))
        .left(px(4.0))
        .p(px(4.0))
        .rounded(px(4.0))
        .bg(rgba(0x000000AA))
        .cursor_pointer()
        .child(Icon::new(icon_name).xsmall().text_color(rgb(0xffffff)));

    if let Some(cb) = on_nsfw_toggle {
        let cb = cb.clone();
        let id = status_id.to_string();
        toggle = toggle.on_click(move |_, window, cx| {
            cx.stop_propagation();
            cb(id.clone(), window, cx);
        });
    }

    toggle
}

/// Render media thumbnails grid
fn render_media_thumbnails(
    status_id: &str,
    attachments: &[&MediaAttachment],
    sensitive: bool,
    nsfw_revealed: bool,
    on_media_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_nsfw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    retry_media: &HashMap<String, u64>,
) -> gpui::Div {
    let mut container = div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .pt(px(4.0));

    let show_blur = sensitive && !nsfw_revealed;

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

        if show_blur {
            // Show blurhash placeholder instead of actual image
            let mut thumb = div()
                .id(SharedString::from(format!("thumb-{}-{}", status_id, i)))
                .w(px(120.0))
                .h(px(90.0))
                .rounded(px(4.0))
                .overflow_hidden()
                .bg(rgb(0x313244))
                .relative();

            if let Some(blur_img) = media.blurhash.as_deref().and_then(decode_blurhash_image) {
                thumb = thumb.child(
                    img(blur_img)
                        .w(px(120.0))
                        .h(px(90.0))
                        .object_fit(ObjectFit::Cover),
                );
            }

            thumb = thumb.child(render_nsfw_toggle(status_id, i, false, on_nsfw_toggle));
            container = container.child(thumb);
        } else {
            // Apply cache-bust for retry
            let retry_count = retry_media.get(&preview_url).copied().unwrap_or(0);
            let img_url = cache_bust_url(&preview_url, retry_media);

            let mut thumb = div()
                .id(SharedString::from(format!("thumb-{}-{}-{}", status_id, i, retry_count)))
                .w(px(120.0))
                .h(px(90.0))
                .rounded(px(4.0))
                .overflow_hidden()
                .cursor_pointer()
                .bg(rgb(0x313244))
                .relative()
                .child(
                    img(img_url)
                        .w(px(120.0))
                        .h(px(90.0))
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| {
                            div()
                                .w(px(120.0))
                                .h(px(90.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Spinner::new().small())
                                .into_any_element()
                        })
                        .with_fallback({
                            let has_remote = media.remote_url.as_ref().map_or(false, |u| !u.is_empty());
                            let remote_url = media.remote_url.clone().unwrap_or_default();
                            let full_url = full_url.clone();
                            let preview_url_for_retry = preview_url.clone();
                            let on_media_click = on_media_click.cloned();
                            let on_media_reload = on_media_reload.cloned();
                            move || {
                                if has_remote {
                                    // Fallback to remote_url (original source) when cached URL fails
                                    let on_media_click = on_media_click.clone();
                                    let on_media_reload = on_media_reload.clone();
                                    let full_url = full_url.clone();
                                    let preview_url = preview_url_for_retry.clone();
                                    img(remote_url.clone())
                                        .w(px(120.0))
                                        .h(px(90.0))
                                        .object_fit(ObjectFit::Cover)
                                        .with_fallback(move || {
                                            let on_media_click = on_media_click.clone();
                                            let on_media_reload = on_media_reload.clone();
                                            let full_url = full_url.clone();
                                            let preview_url = preview_url.clone();
                                            div()
                                                .id("thumb-error-reload")
                                                .w(px(120.0))
                                                .h(px(90.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .child(
                                                    Icon::default()
                                                        .path("icons/refresh-cw.svg")
                                                        .small()
                                                        .text_color(rgb(0x6c7086)),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    if let Some(cb) = on_media_reload.as_ref() {
                                                        cb(preview_url.clone(), window, cx);
                                                    }
                                                    if let Some(cb) = on_media_click.as_ref() {
                                                        cb(full_url.clone(), window, cx);
                                                    }
                                                })
                                                .into_any_element()
                                        })
                                        .into_any_element()
                                } else {
                                    let on_media_click = on_media_click.clone();
                                    let on_media_reload = on_media_reload.clone();
                                    let full_url = full_url.clone();
                                    let preview_url = preview_url_for_retry.clone();
                                    div()
                                        .id("thumb-error-reload")
                                        .w(px(120.0))
                                        .h(px(90.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .child(
                                            Icon::default()
                                                .path("icons/refresh-cw.svg")
                                                .small()
                                                .text_color(rgb(0x6c7086)),
                                        )
                                        .on_click(move |_, window, cx| {
                                            if let Some(cb) = on_media_reload.as_ref() {
                                                cb(preview_url.clone(), window, cx);
                                            }
                                            if let Some(cb) = on_media_click.as_ref() {
                                                cb(full_url.clone(), window, cx);
                                            }
                                        })
                                        .into_any_element()
                                }
                            }
                        }),
                );

            if sensitive {
                thumb = thumb.child(render_nsfw_toggle(status_id, i, true, on_nsfw_toggle));
            }

            if let Some(callback) = on_media_click {
                let cb = callback.clone();
                thumb = thumb.on_click(move |_, window, cx| {
                    cb(full_url.clone(), window, cx);
                });
            }

            container = container.child(thumb);
        }
    }

    container
}

/// Render non-image media (video, audio, etc.) with thumbnails or URL links
fn render_other_media(
    status_id: &str,
    attachments: &[&MediaAttachment],
    sensitive: bool,
    nsfw_revealed: bool,
    on_nsfw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    retry_media: &HashMap<String, u64>,
) -> gpui::Div {
    let mut container = div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .pt(px(4.0));

    let show_blur = sensitive && !nsfw_revealed;

    for (i, media) in attachments.iter().enumerate() {
        let url = media
            .url
            .clone()
            .or_else(|| media.remote_url.clone())
            .unwrap_or_default();

        if show_blur {
            // Show blurhash placeholder for NSFW non-image media
            let has_thumbnail = media.preview_url.is_some() || media.blurhash.is_some();
            if has_thumbnail {
                let mut thumb = div()
                    .id(SharedString::from(format!("media-{}-{}", status_id, i)))
                    .w(px(120.0))
                    .h(px(90.0))
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .bg(rgb(0x313244))
                    .relative();

                if let Some(blur_img) = media.blurhash.as_deref().and_then(decode_blurhash_image) {
                    thumb = thumb.child(
                        img(blur_img)
                            .w(px(120.0))
                            .h(px(90.0))
                            .object_fit(ObjectFit::Cover),
                    );
                }

                thumb = thumb.child(render_nsfw_toggle(status_id, i, false, on_nsfw_toggle));
                container = container.child(thumb);
            } else {
                // No thumbnail or blurhash — show hidden text
                let type_label = match media.media_type.as_str() {
                    "video" | "gifv" => "Video",
                    "audio" => "Audio",
                    _ => "Media",
                };
                let mut link = div()
                    .id(SharedString::from(format!("media-{}-{}", status_id, i)))
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_xs()
                    .text_color(rgb(0x6c7086))
                    .child(format!("[{} hidden]", type_label));

                if let Some(cb) = on_nsfw_toggle {
                    let cb = cb.clone();
                    let id = status_id.to_string();
                    link = link.cursor_pointer().on_click(move |_, window, cx| {
                        cb(id.clone(), window, cx);
                    });
                }

                container = container.child(link);
            }
        } else if let Some(preview) = &media.preview_url {
            let open_url = url.clone();
            let type_label = match media.media_type.as_str() {
                "video" | "gifv" => "\u{25B6}",
                "audio" => "\u{266A}",
                _ => "\u{2197}",
            };
            let retry_count = retry_media.get(preview.as_str()).copied().unwrap_or(0);
            let img_url = cache_bust_url(preview, retry_media);
            let mut thumb = div()
                .id(SharedString::from(format!("media-{}-{}-{}", status_id, i, retry_count)))
                .w(px(120.0))
                .h(px(90.0))
                .rounded(px(4.0))
                .overflow_hidden()
                .cursor_pointer()
                .bg(rgb(0x313244))
                .relative()
                .child(
                    img(img_url)
                        .w(px(120.0))
                        .h(px(90.0))
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| {
                            div()
                                .w(px(120.0))
                                .h(px(90.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Spinner::new().small())
                                .into_any_element()
                        })
                        .with_fallback({
                            let has_remote = media.remote_url.as_ref().map_or(false, |u| !u.is_empty());
                            let remote_url = media.remote_url.clone().unwrap_or_default();
                            let preview_url_for_retry = preview.clone();
                            let open_url = open_url.clone();
                            let on_media_reload = on_media_reload.cloned();
                            move || {
                                if has_remote {
                                    // Fallback to remote_url (original source) when cached URL fails
                                    let on_media_reload = on_media_reload.clone();
                                    let preview_url = preview_url_for_retry.clone();
                                    let open_url = open_url.clone();
                                    img(remote_url.clone())
                                        .w(px(120.0))
                                        .h(px(90.0))
                                        .object_fit(ObjectFit::Cover)
                                        .with_fallback(move || {
                                            let on_media_reload = on_media_reload.clone();
                                            let preview_url = preview_url.clone();
                                            let open_url = open_url.clone();
                                            div()
                                                .id("other-media-error-reload")
                                                .w(px(120.0))
                                                .h(px(90.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .child(
                                                    Icon::default()
                                                        .path("icons/refresh-cw.svg")
                                                        .small()
                                                        .text_color(rgb(0x6c7086)),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    if let Some(cb) = on_media_reload.as_ref() {
                                                        cb(preview_url.clone(), window, cx);
                                                    }
                                                    let _ = open::that(&open_url);
                                                })
                                                .into_any_element()
                                        })
                                        .into_any_element()
                                } else {
                                    let on_media_reload = on_media_reload.clone();
                                    let preview_url = preview_url_for_retry.clone();
                                    let open_url = open_url.clone();
                                    div()
                                        .id("other-media-error-reload")
                                        .w(px(120.0))
                                        .h(px(90.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .child(
                                            Icon::default()
                                                .path("icons/refresh-cw.svg")
                                                .small()
                                                .text_color(rgb(0x6c7086)),
                                        )
                                        .on_click(move |_, window, cx| {
                                            if let Some(cb) = on_media_reload.as_ref() {
                                                cb(preview_url.clone(), window, cx);
                                            }
                                            let _ = open::that(&open_url);
                                        })
                                        .into_any_element()
                                }
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .bottom(px(4.0))
                        .right(px(4.0))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(rgba(0x000000AA))
                        .text_xs()
                        .text_color(rgb(0xffffff))
                        .child(type_label),
                );

            if sensitive {
                thumb = thumb.child(render_nsfw_toggle(status_id, i, true, on_nsfw_toggle));
            }

            thumb = thumb.on_click(move |_, _, _| {
                let _ = open::that(&open_url);
            });
            container = container.child(thumb);
        } else {
            let open_url = url.clone();
            let type_label = match media.media_type.as_str() {
                "video" | "gifv" => "Video",
                "audio" => "Audio",
                _ => "Media",
            };
            let display = format!("[{}] {}", type_label, &url);
            let mut link = div()
                .id(SharedString::from(format!("media-{}-{}", status_id, i)))
                .cursor_pointer()
                .text_xs()
                .text_color(rgb(0x89b4fa))
                .child(display)
                .on_click(move |_, _, _| {
                    let _ = open::that(&open_url);
                });

            if sensitive {
                link = link.child(
                    div()
                        .pl(px(4.0))
                        .child(render_nsfw_toggle(status_id, i, true, on_nsfw_toggle)),
                );
            }

            container = container.child(link);
        }
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

    // "..." more menu (Copy text / Copy URL)
    let content_for_copy = data.content.clone();
    let url_for_copy = data.url.clone();
    let url_disabled = data.url.is_none();

    let more_menu = Button::new(SharedString::from(format!("more-{}", data.id)))
        .ghost()
        .xsmall()
        .icon(
            Icon::default()
                .path("icons/ellipsis.svg")
                .xsmall()
                .text_color(rgb(0x6c7086)),
        )
        .dropdown_menu_with_anchor(
            Corner::TopRight,
            move |menu: gpui_component::menu::PopupMenu,
                  _window: &mut Window,
                  _cx: &mut gpui::Context<gpui_component::menu::PopupMenu>| {
                let content = content_for_copy.clone();
                let url = url_for_copy.clone();

                menu.item(
                    PopupMenuItem::new("Copy text").on_click(
                        move |_: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
                            let plain = html_to_plain_text(&content);
                            cx.write_to_clipboard(ClipboardItem::new_string(plain));
                        },
                    ),
                )
                .item(
                    PopupMenuItem::new("Copy URL")
                        .disabled(url_disabled)
                        .on_click(
                            move |_: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
                                if let Some(ref u) = url {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        u.clone(),
                                    ));
                                }
                            },
                        ),
                )
            },
        );

    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .pt(px(4.0))
        .child(reply_btn)
        .child(reblog_btn)
        .child(fav_btn)
        .child(div().flex_grow())
        .child(more_menu)
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

/// Render a compact (one-line) status item for Mystique display mode.
///
/// When `expanded` is false, shows a single row:
///   [avatar 20x20] [display_name (bold)] [body text (truncated…)] [timestamp]
///
/// When `expanded` is true, shows the compact row as a header followed by
/// the full `render_status_item()` output underneath.
pub fn render_compact_status_item(
    data: &StatusItemData,
    expanded: bool,
    on_expand_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    cw_expanded: bool,
    nsfw_revealed: bool,
    on_cw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_nsfw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_account_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_timestamp_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    retry_media: &HashMap<String, u64>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let appearance = cx.global::<AppearanceSettings>();
    let avatar_radius = px(AvatarShape::Square.radius(40.0));
    let content_size = px(appearance.font_size.content_px());
    let secondary_size = px(appearance.font_size.secondary_px());

    // Build plain text body (strip HTML, collapse whitespace to single line)
    let plain_body: String = html_to_plain_text(&data.content)
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ");

    // --- Compact row ---
    let bg_color = if expanded { rgb(0x262637) } else { rgb(0x1e1e2e) };

    let name_els = render_plain_with_emojis(
        &format!("compact-name-{}", data.id),
        &data.display_name,
        &data.emojis,
        content_size,
    );

    // Row height is determined by content_size + padding; avatar overflows and is clipped
    let row_height = content_size + px(8.0); // text height + py padding

    let mut compact_row = div()
        .id(SharedString::from(format!("compact-row-{}", data.id)))
        .w_full()
        .h(row_height)
        .px(px(8.0))
        .bg(bg_color)
        .border_b_1()
        .border_color(rgb(0x313244))
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        // Avatar (40x40, vertically centered, overflows row height and is clipped)
        .child(
            div()
                .w(px(40.0))
                .h(px(40.0))
                .rounded(avatar_radius)
                .overflow_hidden()
                .flex_shrink_0()
                .child(
                    img(data.avatar_url.to_string())
                        .w(px(40.0))
                        .h(px(40.0))
                        .rounded(avatar_radius)
                        .object_fit(ObjectFit::Cover)
                        .with_fallback(|| {
                            Icon::new(IconName::TriangleAlert)
                                .with_size(Size::Size(px(14.0)))
                                .text_color(rgb(0x6c7086))
                                .into_any_element()
                        }),
                ),
        )
        // Display name
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(120.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(content_size)
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(rgb(0xcdd6f4))
                .children(name_els),
        )
        // Body text (truncated)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(secondary_size)
                .text_color(rgb(0xbac2de))
                .child(SharedString::from(plain_body)),
        )
        // Timestamp
        .child(
            div()
                .flex_shrink_0()
                .whitespace_nowrap()
                .text_size(secondary_size)
                .text_color(rgb(0x585b70))
                .child(data.timestamp.clone()),
        );

    if let Some(cb) = on_expand_toggle {
        let cb = cb.clone();
        let id = data.id.clone();
        compact_row = compact_row.on_click(move |_, window, cx| {
            cb(id.clone(), window, cx);
        });
    }

    if !expanded {
        return compact_row.into_any_element();
    }

    // --- Expanded: compact row + full status ---
    let full_item = render_status_item(
        data,
        cw_expanded,
        nsfw_revealed,
        on_cw_toggle,
        on_nsfw_toggle,
        on_media_click,
        on_reply,
        on_reblog,
        on_favourite,
        on_account_click,
        on_timestamp_click,
        on_media_reload,
        retry_media,
        window,
        cx,
    );

    div()
        .w_full()
        .flex()
        .flex_col()
        .child(compact_row)
        .child(full_item)
        .into_any_element()
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

