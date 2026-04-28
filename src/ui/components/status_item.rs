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
use crate::mastodon::types::status::{MediaAttachment, Poll, Status};
use crate::state::appearance::{AppearanceSettings, AvatarShape, CwBehavior, NsfwBehavior};
use crate::state::confirmation::{ConfirmationSettings, MediaSource};
use crate::ui::panels::timeline_panel::LightboxStatusContext;

/// Callback type for clicks on media thumbnails that should open the lightbox.
///
/// The first argument is the full-resolution URL to display. The second is an
/// optional status context; when `Some`, the lightbox can render action buttons
/// (reply/boost/favourite/show-detail) operating on the originating status.
pub type MediaClickHandler =
    Arc<dyn Fn(String, Option<LightboxStatusContext>, &mut Window, &mut App)>;

/// Data for a reply target (used for reply preview in compose bar)
#[derive(Clone)]
pub struct ReplyTarget {
    pub status_id: String,
    pub display_name: String,
    pub acct: String,
    pub content: String,
    pub visibility: String,
}

/// Data for an edit target (used for edit mode in compose bar)
#[derive(Clone)]
pub struct EditTarget {
    pub status_id: String,
    pub display_name: String,
    pub acct: String,
    pub content: String,
    pub source_text: String,
    pub spoiler_text: String,
    pub visibility: String,
    pub media_ids: Vec<String>,
    pub quote_id: Option<String>,
    pub poll: Option<Poll>,
}

/// Data for a quote target (used for quote preview in compose bar)
#[derive(Clone)]
pub struct QuoteTarget {
    pub status_id: String,
    pub display_name: String,
    pub acct: String,
    pub content: String,
    pub visibility: String,
    pub url: Option<String>,
}

/// Inline display data for a quoted status
#[derive(Clone)]
pub struct QuoteDisplay {
    pub status_id: String,
    pub display_name: SharedString,
    pub acct: SharedString,
    pub avatar_url: SharedString,
    pub content: SharedString,
    pub url: Option<String>,
    pub emojis: Vec<EmojiMapping>,
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
    /// ActivityPub URI of the underlying status. Used to deduplicate the same
    /// status received via different accounts/servers in unified-timeline mode.
    /// For reblogs, this is the original (reblogged) status's URI.
    pub uri: String,
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
    pub bookmarked: bool,
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
    /// Quote post ID (if this status quotes another)
    pub quote_id: Option<String>,
    /// Inline display data for quoted status
    pub quote_display: Option<QuoteDisplay>,
    /// Poll data (if this status has a poll)
    pub poll: Option<Poll>,
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

        let poll: Option<Poll> = status
            .poll_json
            .as_ref()
            .and_then(|j| serde_json::from_str(j).ok());

        Self {
            id: status.id.clone(),
            uri: status.uri.clone(),
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
            bookmarked: status.bookmarked.unwrap_or(false),
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
            quote_id: status.quote_id.clone(),
            quote_display: None, // Filled in by loading code after batch-fetching quoted statuses
            poll,
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
            uri: display_status.uri.clone(),
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
            bookmarked: display_status.bookmarked.unwrap_or(false),
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
            poll: display_status.poll.clone(),
            quote_id: display_status.quote_id.clone(),
            quote_display: display_status.quote.as_ref().map(|q| {
                let quote_emojis: Vec<EmojiMapping> = q
                    .emojis
                    .iter()
                    .chain(q.account.emojis.iter())
                    .map(|e| EmojiMapping {
                        shortcode: e.shortcode.clone(),
                        url: e.url.clone(),
                    })
                    .collect();
                QuoteDisplay {
                    status_id: q.id.clone(),
                    display_name: q.account.display_name.clone().into(),
                    acct: format!("@{}", q.account.acct).into(),
                    avatar_url: q.account.avatar.clone().into(),
                    content: q.content.clone().into(),
                    url: q.url.clone(),
                    emojis: quote_emojis,
                }
            }),
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
                uri: notification.id.clone(),
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
                bookmarked: false,
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
                quote_id: None,
                quote_display: None,
                poll: None,
            }
        }
    }
}

impl StatusItemData {
    /// Build a `LightboxStatusContext` for this item, used when opening the lightbox
    /// to enable reply/boost/favourite/show-detail actions against the underlying status.
    pub fn to_lightbox_context(&self) -> LightboxStatusContext {
        LightboxStatusContext {
            api_status_id: self.original_status_id.clone(),
            display_name: self.display_name.to_string(),
            acct: self.acct.to_string(),
            content: self.content.to_string(),
            visibility: self.visibility.to_string(),
            url: self.url.clone(),
            reblogged: self.reblogged,
            favourited: self.favourited,
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
    on_media_click: Option<&MediaClickHandler>,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_bookmark: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_quote: Option<&Arc<dyn Fn(QuoteTarget, &mut Window, &mut App)>>,
    on_account_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_timestamp_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_edit: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_vote: Option<&Arc<dyn Fn(String, Vec<usize>, &mut Window, &mut App)>>,
    on_poll_select: Option<&Arc<dyn Fn(String, usize, &mut Window, &mut App)>>,
    on_poll_refresh: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    pending_poll_votes: Option<&std::collections::HashSet<usize>>,
    current_user_id: Option<&str>,
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
    let media_source = cx
        .try_global::<ConfirmationSettings>()
        .map(|c| c.media_source)
        .unwrap_or_default();

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
                            let lightbox_ctx = data.to_lightbox_context();
                            el.child(render_media_thumbnails(
                                &data.id,
                                &image_attachments,
                                data.sensitive,
                                effective_nsfw_revealed,
                                on_media_click,
                                on_nsfw_toggle,
                                on_media_reload,
                                retry_media,
                                media_source,
                                &lightbox_ctx,
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
                                media_source,
                            ))
                        })
                        // Quoted post card
                        .when_some(data.quote_display.as_ref(), |el, quote| {
                            el.child(render_quote_card(
                                quote,
                                content_size,
                                secondary_size,
                                avatar_radius_16,
                            ))
                        })
                        // Poll
                        .when_some(data.poll.as_ref(), |el, poll| {
                            el.child(render_poll(poll, on_vote, on_poll_select, on_poll_refresh, pending_poll_votes, _window, cx))
                        })
                        // Action bar
                        .child(render_action_bar(data, on_reply, on_reblog, on_favourite, on_bookmark, on_quote, on_edit, current_user_id)),
                ),
        )
        .into_any_element()
}

/// Pick preview/full URLs for a media attachment, honoring the Media source setting.
///
/// Returns `(preview_url, full_url)`. Falls back to whatever URL is available so
/// that empty fields in the API response do not produce blank images.
fn pick_media_urls(media: &MediaAttachment, source: MediaSource) -> (String, String) {
    let preview = media.preview_url.clone().unwrap_or_default();
    let url = media.url.clone().unwrap_or_default();
    let remote = media.remote_url.clone().unwrap_or_default();

    let first_non_empty = |candidates: &[&str]| -> String {
        candidates
            .iter()
            .find(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_default()
    };

    match source {
        MediaSource::Local => (
            first_non_empty(&[&preview, &url, &remote]),
            first_non_empty(&[&url, &remote, &preview]),
        ),
        MediaSource::Remote => (
            first_non_empty(&[&remote, &preview, &url]),
            first_non_empty(&[&remote, &url, &preview]),
        ),
    }
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
    on_media_click: Option<&MediaClickHandler>,
    on_nsfw_toggle: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    retry_media: &HashMap<String, u64>,
    media_source: MediaSource,
    lightbox_ctx: &LightboxStatusContext,
) -> gpui::Div {
    let mut container = div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .pt(px(4.0));

    let show_blur = sensitive && !nsfw_revealed;

    for (i, media) in attachments.iter().enumerate() {
        let (preview_url, full_url) = pick_media_urls(media, media_source);

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
                            let lightbox_ctx = lightbox_ctx.clone();
                            move || {
                                if has_remote {
                                    // Fallback to remote_url (original source) when cached URL fails
                                    let on_media_click = on_media_click.clone();
                                    let on_media_reload = on_media_reload.clone();
                                    let full_url = full_url.clone();
                                    let preview_url = preview_url_for_retry.clone();
                                    let lightbox_ctx = lightbox_ctx.clone();
                                    img(remote_url.clone())
                                        .w(px(120.0))
                                        .h(px(90.0))
                                        .object_fit(ObjectFit::Cover)
                                        .with_fallback(move || {
                                            let on_media_click = on_media_click.clone();
                                            let on_media_reload = on_media_reload.clone();
                                            let full_url = full_url.clone();
                                            let preview_url = preview_url.clone();
                                            let lightbox_ctx = lightbox_ctx.clone();
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
                                                        cb(full_url.clone(), Some(lightbox_ctx.clone()), window, cx);
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
                                    let lightbox_ctx = lightbox_ctx.clone();
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
                                                cb(full_url.clone(), Some(lightbox_ctx.clone()), window, cx);
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
                let lightbox_ctx = lightbox_ctx.clone();
                thumb = thumb.on_click(move |_, window, cx| {
                    cb(full_url.clone(), Some(lightbox_ctx.clone()), window, cx);
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
    media_source: MediaSource,
) -> gpui::Div {
    let mut container = div()
        .flex()
        .flex_wrap()
        .gap(px(4.0))
        .pt(px(4.0));

    let show_blur = sensitive && !nsfw_revealed;

    for (i, media) in attachments.iter().enumerate() {
        let (preview_url, url) = pick_media_urls(media, media_source);

        if show_blur {
            // Show blurhash placeholder for NSFW non-image media
            let has_thumbnail = !preview_url.is_empty() || media.blurhash.is_some();
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
        } else if !preview_url.is_empty() {
            let open_url = url.clone();
            let type_label = match media.media_type.as_str() {
                "video" | "gifv" => "\u{25B6}",
                "audio" => "\u{266A}",
                _ => "\u{2197}",
            };
            let retry_count = retry_media.get(preview_url.as_str()).copied().unwrap_or(0);
            let img_url = cache_bust_url(&preview_url, retry_media);
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
                            let preview_url_for_retry = preview_url.clone();
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

/// Render the action bar (reply, boost, favourite, quote buttons)
fn render_action_bar(
    data: &StatusItemData,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_bookmark: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_quote: Option<&Arc<dyn Fn(QuoteTarget, &mut Window, &mut App)>>,
    on_edit: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    current_user_id: Option<&str>,
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
            status_id: data.original_status_id.clone(),
            display_name: data.display_name.to_string(),
            acct: data.acct.to_string(),
            content: data.content.to_string(),
            visibility: data.visibility.to_string(),
        };
        reply_btn = reply_btn.on_click(move |_, window, cx| {
            cb(target.clone(), window, cx);
        });
    }

    let mut quote_btn = div()
        .id(SharedString::from(format!("quote-{}", data.id)))
        .flex()
        .gap(px(4.0))
        .items_center()
        .text_xs()
        .text_color(rgb(0x6c7086))
        .cursor_pointer()
        .child(Icon::default().path("icons/quote.svg").xsmall());

    if let Some(cb) = on_quote {
        let cb = cb.clone();
        let target = QuoteTarget {
            status_id: data.original_status_id.clone(),
            display_name: data.display_name.to_string(),
            acct: data.acct.to_string(),
            content: data.content.to_string(),
            visibility: data.visibility.to_string(),
            url: data.url.clone(),
        };
        quote_btn = quote_btn.on_click(move |_, window, cx| {
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

    let bookmark_color = if data.bookmarked { rgb(0x89b4fa) } else { rgb(0x6c7086) };
    let mut bookmark_btn = div()
        .id(SharedString::from(format!("bookmark-{}", data.id)))
        .flex()
        .gap(px(4.0))
        .items_center()
        .text_xs()
        .text_color(bookmark_color)
        .cursor_pointer()
        .child(Icon::default().path("icons/bookmark.svg").xsmall().text_color(bookmark_color));

    if let Some(cb) = on_bookmark {
        let cb = cb.clone();
        let id = data.id.clone();
        bookmark_btn = bookmark_btn.on_click(move |_, window, cx| {
            cb(id.clone(), window, cx);
        });
    }

    // "..." more menu (Copy text / Copy URL / Open in browser / Edit post)
    let content_for_copy = data.content.clone();
    let url_for_copy = data.url.clone();
    let url_disabled = data.url.is_none();
    let is_own_post = current_user_id
        .map(|uid| uid == data.account_id)
        .unwrap_or(false);
    let edit_cb = on_edit.cloned();
    let edit_status_id = data.id.clone();

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
                let url_for_open = url_for_copy.clone();
                let edit_cb = edit_cb.clone();
                let edit_status_id = edit_status_id.clone();

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
                .item(
                    PopupMenuItem::new("Open in browser")
                        .disabled(url_disabled)
                        .on_click(
                            move |_: &gpui::ClickEvent, _window: &mut Window, _cx: &mut App| {
                                if let Some(ref u) = url_for_open {
                                    let _ = open::that(u);
                                }
                            },
                        ),
                )
                .when(is_own_post, |menu| {
                    menu.separator().item(
                        PopupMenuItem::new("Edit post").on_click(
                            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                                if let Some(ref cb) = edit_cb {
                                    cb(edit_status_id.clone(), window, cx);
                                }
                            },
                        ),
                    )
                })
            },
        );

    div()
        .flex()
        .items_center()
        .gap(px(16.0))
        .pt(px(4.0))
        .child(reply_btn)
        .child(quote_btn)
        .child(reblog_btn)
        .child(fav_btn)
        .child(bookmark_btn)
        .child(div().flex_grow())
        .child(more_menu)
}

/// Render a compact card for a quoted status
fn render_quote_card(
    quote: &QuoteDisplay,
    content_size: gpui::Pixels,
    secondary_size: gpui::Pixels,
    avatar_radius: gpui::Pixels,
) -> gpui::Div {
    let name_els = render_plain_with_emojis(
        &format!("quote-name-{}", quote.status_id),
        &quote.display_name,
        &quote.emojis,
        secondary_size,
    );
    let content_els = render_html_content(
        &format!("quote-content-{}", quote.status_id),
        &quote.content,
        &quote.emojis,
        content_size,
    );

    div()
        .mt(px(4.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(0x45475a))
        .bg(rgb(0x181825))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .w_full()
                .overflow_hidden()
                .child(
                    div()
                        .w(px(20.0))
                        .h(px(20.0))
                        .rounded(avatar_radius)
                        .overflow_hidden()
                        .flex_shrink_0()
                        .child(
                            img(quote.avatar_url.to_string())
                                .w(px(20.0))
                                .h(px(20.0))
                                .rounded(avatar_radius)
                                .object_fit(ObjectFit::Cover)
                                .with_fallback(|| {
                                    Icon::new(IconName::TriangleAlert)
                                        .with_size(Size::Size(px(10.0)))
                                        .text_color(rgb(0x6c7086))
                                        .into_any_element()
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_shrink()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(secondary_size)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(rgb(0xcdd6f4))
                        .children(name_els),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(secondary_size)
                        .text_color(rgb(0x6c7086))
                        .child(quote.acct.to_string()),
                ),
        )
        .child(
            div()
                .text_size(content_size)
                .text_color(rgb(0xbac2de))
                .children(content_els),
        )
}

/// Render a poll attached to a status.
///
/// Two modes:
/// - **Results mode** (voted or expired): show percentage bars and vote counts
/// - **Voting mode** (not voted, not expired): clickable options to cast a vote
fn render_poll(
    poll: &Poll,
    on_vote: Option<&Arc<dyn Fn(String, Vec<usize>, &mut Window, &mut App)>>,
    on_poll_select: Option<&Arc<dyn Fn(String, usize, &mut Window, &mut App)>>,
    on_poll_refresh: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    pending_poll_votes: Option<&std::collections::HashSet<usize>>,
    _window: &mut Window,
    _cx: &mut App,
) -> gpui::Div {
    let show_results = poll.voted.unwrap_or(false) || poll.expired;
    let total_votes = poll.votes_count.max(1) as f32;
    let poll_id = poll.id.clone();

    let mut container = div()
        .mt(px(4.0))
        .flex()
        .flex_col()
        .gap(px(4.0));

    for (i, option) in poll.options.iter().enumerate() {
        let votes = option.votes_count.unwrap_or(0);
        let pct = votes as f32 / total_votes;
        let pct_display = (pct * 100.0).round() as i64;
        let is_own_vote = poll
            .own_votes
            .as_ref()
            .map(|v| v.contains(&(i as i64)))
            .unwrap_or(false);

        if show_results {
            // Results mode: percentage bar + title + count
            container = container.child(
                div()
                    .id(SharedString::from(format!("poll-opt-{}-{}", poll_id, i)))
                    .relative()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .overflow_hidden()
                    // Background bar
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(gpui::relative(pct))
                            .rounded(px(4.0))
                            .bg(if is_own_vote {
                                rgba(0xa6e3a140)
                            } else {
                                rgba(0x89b4fa30)
                            }),
                    )
                    // Content row
                    .child(
                        div()
                            .relative()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_xs()
                            .when(is_own_vote, |el| {
                                el.child(
                                    div()
                                        .text_color(rgb(0xa6e3a1))
                                        .child("✓"),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(rgb(0xcdd6f4))
                                    .child(option.title.clone()),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0x6c7086))
                                    .child(format!("{}%", pct_display)),
                            ),
                    ),
            );
        } else {
            // Voting mode: clickable option rows
            let is_selected = pending_poll_votes
                .map(|s| s.contains(&i))
                .unwrap_or(false);
            let indicator = if poll.multiple {
                if is_selected { "☑" } else { "☐" }
            } else {
                if is_selected { "●" } else { "○" }
            };

            let pid = poll_id.clone();
            let mut row = div()
                .id(SharedString::from(format!("poll-vote-{}-{}", poll_id, i)))
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .border_1()
                .border_color(if is_selected {
                    rgb(0x89b4fa)
                } else {
                    rgb(0x45475a)
                })
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0x31324480)))
                .text_xs()
                .child(
                    div()
                        .text_color(if is_selected {
                            rgb(0x89b4fa)
                        } else {
                            rgb(0x6c7086)
                        })
                        .child(indicator),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(0xcdd6f4))
                        .child(option.title.clone()),
                );

            if poll.multiple {
                // Multiple choice: toggle selection
                if let Some(cb) = on_poll_select {
                    let cb = cb.clone();
                    row = row.on_click(move |_, window, cx| {
                        cb(pid.clone(), i, window, cx);
                    });
                }
            } else {
                // Single choice: vote immediately
                if let Some(cb) = on_vote {
                    let cb = cb.clone();
                    row = row.on_click(move |_, window, cx| {
                        cb(pid.clone(), vec![i], window, cx);
                    });
                }
            }

            container = container.child(row);
        }
    }

    // Footer: vote count + remaining time + vote button (multiple)
    let remaining = format_poll_remaining(poll);
    let vote_count_text = if let Some(voters) = poll.voters_count {
        format!("{} voters", voters)
    } else {
        format!("{} votes", poll.votes_count)
    };

    let mut footer = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .text_xs()
        .text_color(rgb(0x6c7086));

    // Vote button for multiple choice (only in voting mode)
    if !show_results && poll.multiple {
        let pid = poll_id.clone();
        let selections: Vec<usize> = pending_poll_votes
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        let has_selections = !selections.is_empty();

        if let Some(cb) = on_vote {
            let cb = cb.clone();
            footer = footer.child(
                div()
                    .id("poll-vote-submit")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(if has_selections {
                        rgb(0x89b4fa)
                    } else {
                        rgb(0x45475a)
                    })
                    .text_color(if has_selections {
                        rgb(0x89b4fa)
                    } else {
                        rgb(0x6c7086)
                    })
                    .when(has_selections, |el| {
                        el.hover(|s| s.bg(rgba(0x89b4fa20)))
                    })
                    .child("Vote")
                    .on_click(move |_, window, cx| {
                        if has_selections {
                            cb(pid.clone(), selections.clone(), window, cx);
                        }
                    }),
            );
        }
    }

    // Refresh button (always shown for polls)
    if let Some(cb) = on_poll_refresh {
        let cb = cb.clone();
        let pid = poll_id.clone();
        footer = footer.child(
            div()
                .id(SharedString::from(format!("poll-refresh-{}", poll_id)))
                .cursor_pointer()
                .text_color(rgb(0x89b4fa))
                .hover(|s| s.text_color(rgb(0xb4d0fb)))
                .child("更新")
                .on_click(move |_, window, cx| {
                    cb(pid.clone(), window, cx);
                }),
        );
        footer = footer.child(div().child("·"));
    }

    footer = footer
        .child(div().child(vote_count_text))
        .child(div().child("·"))
        .child(div().child(remaining));

    container = container.child(footer);
    container
}

/// Format the remaining time for a poll
fn format_poll_remaining(poll: &Poll) -> String {
    if poll.expired {
        return "Closed".to_string();
    }
    let Some(ref expires_at) = poll.expires_at else {
        return String::new();
    };
    let now = chrono::Utc::now();
    if *expires_at <= now {
        return "Closed".to_string();
    }
    let diff = *expires_at - now;
    let days = diff.num_days();
    let hours = diff.num_hours() % 24;
    let minutes = diff.num_minutes() % 60;
    if days > 0 {
        format!("{}d {}h remaining", days, hours)
    } else if hours > 0 {
        format!("{}h {}m remaining", hours, minutes)
    } else {
        format!("{}m remaining", minutes)
    }
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
    on_media_click: Option<&MediaClickHandler>,
    on_reply: Option<&Arc<dyn Fn(ReplyTarget, &mut Window, &mut App)>>,
    on_reblog: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_favourite: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_bookmark: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_quote: Option<&Arc<dyn Fn(QuoteTarget, &mut Window, &mut App)>>,
    on_account_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_timestamp_click: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_media_reload: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_edit: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    on_vote: Option<&Arc<dyn Fn(String, Vec<usize>, &mut Window, &mut App)>>,
    on_poll_select: Option<&Arc<dyn Fn(String, usize, &mut Window, &mut App)>>,
    on_poll_refresh: Option<&Arc<dyn Fn(String, &mut Window, &mut App)>>,
    pending_poll_votes: Option<&std::collections::HashSet<usize>>,
    current_user_id: Option<&str>,
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
        on_bookmark,
        on_quote,
        on_account_click,
        on_timestamp_click,
        on_media_reload,
        on_edit,
        on_vote,
        on_poll_select,
        on_poll_refresh,
        pending_poll_votes,
        current_user_id,
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

