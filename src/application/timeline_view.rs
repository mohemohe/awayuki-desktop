//! Serializable timeline view models shared by application and IPC layers.

use serde::Serialize;

use crate::db::models::{DbAccount, DbNotification, DbStatus};
use crate::domain::identity::StatusIdentity;
use crate::mastodon::types::account::CustomEmoji;
use crate::mastodon::types::notification::Notification;
use crate::mastodon::types::status::{MediaAttachment, Poll, Status, StatusApplication};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineStatus {
    pub(crate) id: String,
    pub(crate) original_status_id: String,
    pub(crate) status_identity: StatusIdentity,
    pub(crate) source_acct: Option<String>,
    pub(crate) account_id: String,
    pub(crate) server_domain: String,
    pub(crate) uri: String,
    pub(crate) url: Option<String>,
    pub(crate) display_name: String,
    pub(crate) acct: String,
    pub(crate) avatar: String,
    pub(crate) created_at: String,
    pub(crate) original_created_at: Option<String>,
    pub(crate) in_reply_to_id: Option<String>,
    pub(crate) in_reply_to_account_id: Option<String>,
    pub(crate) content: String,
    pub(crate) spoiler_text: String,
    pub(crate) language: Option<String>,
    pub(crate) application_name: Option<String>,
    pub(crate) reblogs_count: i64,
    pub(crate) favourites_count: i64,
    pub(crate) replies_count: i64,
    pub(crate) visibility: String,
    pub(crate) sensitive: bool,
    pub(crate) favourited: bool,
    pub(crate) reblogged: bool,
    pub(crate) bookmarked: bool,
    pub(crate) media: Vec<MediaAttachment>,
    pub(crate) poll: Option<PollView>,
    pub(crate) emojis: Vec<CustomEmojiView>,
    pub(crate) account_emojis: Vec<CustomEmojiView>,
    pub(crate) quote_id: Option<String>,
    pub(crate) quote_original_url: Option<String>,
    pub(crate) quote: Option<Box<TimelineStatus>>,
    pub(crate) quote_state: Option<String>,
    pub(crate) notification_id: Option<String>,
    pub(crate) notification_kind: Option<String>,
    pub(crate) notification_label: Option<String>,
    pub(crate) notification_avatar: Option<String>,
    pub(crate) notification_account_id: Option<String>,
    pub(crate) notification_acct: Option<String>,
    pub(crate) notification_display_name: Option<String>,
    pub(crate) notification_account_emojis: Vec<CustomEmojiView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelinePageResponse {
    pub(crate) statuses: Vec<TimelineStatus>,
    pub(crate) has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusViewerStateSummary {
    pub(crate) identity: StatusIdentity,
    pub(crate) favourited: bool,
    pub(crate) reblogged: bool,
    pub(crate) bookmarked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomEmojiView {
    pub(crate) shortcode: String,
    pub(crate) url: String,
    pub(crate) static_url: String,
    pub(crate) category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PollOptionView {
    pub(crate) title: String,
    pub(crate) votes_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PollView {
    pub(crate) id: String,
    pub(crate) expires_at: Option<String>,
    pub(crate) expired: bool,
    pub(crate) multiple: bool,
    pub(crate) votes_count: i64,
    pub(crate) voters_count: Option<i64>,
    pub(crate) options: Vec<PollOptionView>,
    pub(crate) voted: Option<bool>,
    pub(crate) own_votes: Option<Vec<i64>>,
    pub(crate) emojis: Vec<CustomEmojiView>,
}

pub(crate) fn status_to_view(
    status: &Status,
    server_domain: &str,
    notification_label: Option<String>,
) -> TimelineStatus {
    if let Some(reblog) = status.reblog.as_ref() {
        let booster = if status.account.display_name.is_empty() {
            format!("@{}", status.account.acct)
        } else {
            status.account.display_name.clone()
        };
        let mut view = status_to_view_base(
            reblog,
            server_domain,
            Some(notification_label.unwrap_or_else(|| format!("{} boosted", booster))),
            Some(status.account.avatar.clone()),
        );
        view.original_created_at = Some(view.created_at.clone());
        view.id = status.id.clone();
        view.uri = status.uri.clone();
        view.created_at = status.created_at.to_rfc3339();
        view.notification_kind = Some("reblog".to_string());
        view.notification_account_id = Some(status.account.id.clone());
        view.notification_acct = Some(format!("@{}", status.account.acct));
        view.notification_display_name = Some(status.account.display_name.clone());
        view.notification_account_emojis = custom_emojis_to_views(&status.account.emojis);
        return view;
    }
    status_to_view_base(status, server_domain, notification_label, None)
}

fn status_to_view_base(
    status: &Status,
    server_domain: &str,
    notification_label: Option<String>,
    notification_avatar: Option<String>,
) -> TimelineStatus {
    status_to_view_base_with_quote_depth(
        status,
        server_domain,
        notification_label,
        notification_avatar,
        2,
    )
}

fn status_to_view_base_with_quote_depth(
    status: &Status,
    server_domain: &str,
    notification_label: Option<String>,
    notification_avatar: Option<String>,
    quote_depth: usize,
) -> TimelineStatus {
    let quote = if quote_depth == 0 {
        None
    } else {
        status.quote.as_deref().map(|quote| {
            let mut view = status_to_view_base_with_quote_depth(
                quote.reblog.as_deref().unwrap_or(quote),
                server_domain,
                None,
                None,
                quote_depth - 1,
            );
            if quote.reblog.is_some() {
                view.original_created_at = Some(view.created_at.clone());
                view.id = quote.id.clone();
                view.uri = quote.uri.clone();
                view.created_at = quote.created_at.to_rfc3339();
                view.notification_kind = Some("reblog".to_string());
            }
            Box::new(view)
        })
    };

    let quote_state = if quote.is_some() {
        Some("resolved".to_string())
    } else if status.quote_id.is_some() || status.quote_original_url.is_some() {
        Some("pending".to_string())
    } else {
        None
    };

    TimelineStatus {
        id: status.id.clone(),
        original_status_id: status.id.clone(),
        status_identity: StatusIdentity::inferred(server_domain, &status.uri, &status.id),
        source_acct: None,
        account_id: status.account.id.clone(),
        server_domain: server_domain.to_string(),
        uri: status.uri.clone(),
        url: status.url.clone(),
        display_name: status.account.display_name.clone(),
        acct: format!("@{}", status.account.acct),
        avatar: status.account.avatar.clone(),
        created_at: status.created_at.to_rfc3339(),
        original_created_at: None,
        in_reply_to_id: status.in_reply_to_id.clone(),
        in_reply_to_account_id: status.in_reply_to_account_id.clone(),
        content: status.content.clone(),
        spoiler_text: status.spoiler_text.clone(),
        language: status.language.clone(),
        application_name: status_application_name(status),
        reblogs_count: status.reblogs_count,
        favourites_count: status.favourites_count,
        replies_count: status.replies_count,
        visibility: status.visibility.clone(),
        sensitive: status.sensitive,
        favourited: status.favourited.unwrap_or(false),
        reblogged: status.reblogged.unwrap_or(false),
        bookmarked: status.bookmarked.unwrap_or(false),
        media: status.media_attachments.clone(),
        poll: status.poll.as_ref().map(poll_to_view),
        emojis: custom_emojis_to_views(&status.emojis),
        account_emojis: custom_emojis_to_views(&status.account.emojis),
        quote_id: status.quote_id.clone(),
        quote_original_url: status.quote_original_url.clone(),
        quote,
        quote_state,
        notification_id: None,
        notification_kind: None,
        notification_label,
        notification_avatar,
        notification_account_id: None,
        notification_acct: None,
        notification_display_name: None,
        notification_account_emojis: Vec::new(),
    }
}

pub(crate) fn notification_to_view(
    notification: &Notification,
    server_domain: &str,
    source_acct: Option<&str>,
) -> TimelineStatus {
    let notification_label = Some(format!(
        "{} {}",
        notification.account.display_name,
        notification_type_label(notification.notification_type.as_str())
    ));
    let notification_kind = Some(notification.notification_type.as_str().to_string());
    let notification_avatar = Some(notification.account.avatar.clone());
    let notification_account_id = Some(notification.account.id.clone());
    let notification_acct = Some(format!("@{}", notification.account.acct));
    let notification_display_name = Some(notification.account.display_name.clone());
    let notification_account_emojis = custom_emojis_to_views(&notification.account.emojis);

    let Some(status) = notification.status.as_ref() else {
        return TimelineStatus {
            id: notification.id.clone(),
            original_status_id: notification.id.clone(),
            status_identity: StatusIdentity::inferred(
                server_domain,
                format!("https://{server_domain}/notifications/{}", notification.id),
                &notification.id,
            ),
            source_acct: source_acct.map(str::to_string),
            account_id: notification.account.id.clone(),
            server_domain: server_domain.to_string(),
            uri: String::new(),
            url: None,
            display_name: notification.account.display_name.clone(),
            acct: format!("@{}", notification.account.acct),
            avatar: notification.account.avatar.clone(),
            created_at: notification.created_at.to_rfc3339(),
            original_created_at: None,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            content: String::new(),
            spoiler_text: String::new(),
            language: None,
            application_name: None,
            reblogs_count: 0,
            favourites_count: 0,
            replies_count: 0,
            visibility: "direct".to_string(),
            sensitive: false,
            favourited: false,
            reblogged: false,
            bookmarked: false,
            media: Vec::new(),
            poll: None,
            emojis: Vec::new(),
            account_emojis: custom_emojis_to_views(&notification.account.emojis),
            quote_id: None,
            quote_original_url: None,
            quote: None,
            quote_state: None,
            notification_id: Some(notification.id.clone()),
            notification_kind,
            notification_label,
            notification_avatar,
            notification_account_id,
            notification_acct,
            notification_display_name,
            notification_account_emojis,
        };
    };

    let mut view = status_to_view_base(
        status,
        server_domain,
        notification_label,
        notification_avatar,
    );
    view.id = notification.id.clone();
    view.created_at = notification.created_at.to_rfc3339();
    view.source_acct = source_acct.map(str::to_string);
    view.notification_id = Some(notification.id.clone());
    view.notification_kind = notification_kind;
    view.notification_account_id = notification_account_id;
    view.notification_acct = notification_acct;
    view.notification_display_name = notification_display_name;
    view.notification_account_emojis = notification_account_emojis;
    view
}

pub(crate) fn application_name_from_json(json: Option<&str>) -> Option<String> {
    json.and_then(|json| serde_json::from_str::<StatusApplication>(json).ok())
        .and_then(|application| normalized_application_name(&application.name))
}

fn status_application_name(status: &Status) -> Option<String> {
    status
        .application
        .as_ref()
        .and_then(|application| normalized_application_name(&application.name))
}

fn normalized_application_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

pub(crate) fn custom_emojis_to_views(emojis: &[CustomEmoji]) -> Vec<CustomEmojiView> {
    emojis
        .iter()
        .map(|emoji| CustomEmojiView {
            shortcode: emoji.shortcode.clone(),
            url: emoji.url.clone(),
            static_url: emoji.static_url.clone(),
            category: emoji.category.clone(),
        })
        .collect()
}

pub(crate) fn poll_to_view(poll: &Poll) -> PollView {
    PollView {
        id: poll.id.clone(),
        expires_at: poll.expires_at.map(|expires_at| expires_at.to_rfc3339()),
        expired: poll.expired,
        multiple: poll.multiple,
        votes_count: poll.votes_count,
        voters_count: poll.voters_count,
        options: poll
            .options
            .iter()
            .map(|option| PollOptionView {
                title: option.title.clone(),
                votes_count: option.votes_count,
            })
            .collect(),
        voted: poll.voted,
        own_votes: poll.own_votes.clone(),
        emojis: custom_emojis_to_views(&poll.emojis),
    }
}

pub(crate) fn parse_poll_view(json: &str) -> Option<PollView> {
    serde_json::from_str::<Poll>(json)
        .ok()
        .map(|poll| poll_to_view(&poll))
}

pub(crate) fn parse_custom_emoji_views(json: &str) -> Vec<CustomEmojiView> {
    serde_json::from_str::<Vec<CustomEmoji>>(json)
        .map(|emojis| custom_emojis_to_views(&emojis))
        .unwrap_or_default()
}

pub(crate) fn notification_type_label(notification_type: &str) -> &'static str {
    match notification_type {
        "mention" => "mentioned you",
        "reblog" => "boosted",
        "favourite" => "favourited",
        "follow" => "followed you",
        "follow_request" => "requested to follow",
        "status" => "posted",
        "update" => "edited",
        "poll" => "poll ended",
        "admin.sign_up" => "signed up",
        "admin.report" => "reported",
        _ => "notified you",
    }
}

pub(crate) fn db_status_to_view(status: DbStatus, account: Option<DbAccount>) -> TimelineStatus {
    let status_identity = StatusIdentity::inferred(&status.server_domain, &status.uri, &status.id);
    let media = status
        .media_attachments_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<Vec<MediaAttachment>>(json).ok())
        .unwrap_or_default();
    let poll = status.poll_json.as_deref().and_then(parse_poll_view);
    let emojis = status
        .emojis_json
        .as_deref()
        .map(parse_custom_emoji_views)
        .unwrap_or_default();
    let account_emojis = account
        .as_ref()
        .and_then(|account| account.emojis_json.as_deref())
        .map(parse_custom_emoji_views)
        .unwrap_or_default();
    let display_name = account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| status.account_id.clone());
    let acct = account
        .as_ref()
        .map(|account| format!("@{}", account.acct))
        .unwrap_or_else(|| format!("@{}", status.account_id));
    let avatar = account.map(|account| account.avatar).unwrap_or_default();
    let in_reply_to_id = status.in_reply_to_id.clone();
    let in_reply_to_account_id = status.in_reply_to_account_id.clone();

    let quote_state = if status.quote_id.is_some() || status.quote_original_url.is_some() {
        Some("pending".to_string())
    } else {
        None
    };
    TimelineStatus {
        id: status.id.clone(),
        original_status_id: status.id,
        status_identity,
        source_acct: None,
        account_id: status.account_id,
        server_domain: status.server_domain,
        uri: status.uri,
        url: status.url,
        display_name,
        acct,
        avatar,
        created_at: status.created_at,
        original_created_at: None,
        in_reply_to_id,
        in_reply_to_account_id,
        content: status.content,
        spoiler_text: status.spoiler_text,
        language: status.language,
        application_name: application_name_from_json(status.application_json.as_deref()),
        reblogs_count: status.reblogs_count,
        favourites_count: status.favourites_count,
        replies_count: status.replies_count,
        visibility: status.visibility,
        sensitive: status.sensitive,
        favourited: status.favourited.unwrap_or(false),
        reblogged: status.reblogged.unwrap_or(false),
        bookmarked: status.bookmarked.unwrap_or(false),
        media,
        poll,
        emojis,
        account_emojis,
        quote_id: status.quote_id,
        quote_original_url: status.quote_original_url,
        quote: None,
        quote_state,
        notification_id: None,
        notification_kind: None,
        notification_label: None,
        notification_avatar: None,
        notification_account_id: None,
        notification_acct: None,
        notification_display_name: None,
        notification_account_emojis: Vec::new(),
    }
}

pub(crate) fn notification_db_to_view(
    notification: DbNotification,
    actor_account: Option<DbAccount>,
    status: Option<DbStatus>,
    status_account: Option<DbAccount>,
) -> TimelineStatus {
    let source_acct = notification.account_acct.clone();
    let actor_account_id = notification.account_id.clone();
    let actor_display_name = actor_account
        .as_ref()
        .map(|account| account.display_name.clone())
        .unwrap_or_else(|| actor_account_id.clone());
    let actor_acct = actor_account
        .as_ref()
        .map(|account| format!("@{}", account.acct))
        .unwrap_or_else(|| format!("@{}", actor_account_id));
    let actor_avatar = actor_account
        .as_ref()
        .map(|account| account.avatar.clone())
        .unwrap_or_default();
    let notification_label = Some(format!(
        "{} {}",
        actor_display_name,
        notification_type_label(&notification.notification_type)
    ));
    let notification_kind = Some(notification.notification_type.clone());
    let notification_avatar = actor_account.as_ref().map(|account| account.avatar.clone());
    let notification_account_emojis = actor_account
        .as_ref()
        .and_then(|account| account.emojis_json.as_deref())
        .map(parse_custom_emoji_views)
        .unwrap_or_default();

    match status {
        Some(status) => {
            let notification_id = notification.id.clone();
            let mut view = db_status_to_view(status, status_account);
            view.id = notification.id;
            view.created_at = notification.created_at;
            view.source_acct = source_acct;
            view.notification_id = Some(notification_id);
            view.notification_kind = notification_kind;
            view.notification_label = notification_label;
            view.notification_avatar = notification_avatar;
            view.notification_account_id = Some(actor_account_id);
            view.notification_acct = Some(actor_acct);
            view.notification_display_name = Some(actor_display_name);
            view.notification_account_emojis = notification_account_emojis;
            view
        }
        None => {
            let notification_id = notification.id.clone();
            let status_identity = StatusIdentity::inferred(
                &notification.server_domain,
                format!(
                    "https://{}/notifications/{}",
                    notification.server_domain, notification.id
                ),
                &notification.id,
            );
            TimelineStatus {
                id: notification.id,
                original_status_id: notification.status_id.unwrap_or_default(),
                status_identity,
                source_acct,
                account_id: actor_account_id.clone(),
                server_domain: notification.server_domain,
                uri: String::new(),
                url: None,
                display_name: actor_display_name.clone(),
                acct: actor_acct.clone(),
                avatar: actor_avatar,
                created_at: notification.created_at,
                original_created_at: None,
                in_reply_to_id: None,
                in_reply_to_account_id: None,
                content: String::new(),
                spoiler_text: String::new(),
                language: None,
                application_name: None,
                reblogs_count: 0,
                favourites_count: 0,
                replies_count: 0,
                visibility: "direct".to_string(),
                sensitive: false,
                favourited: false,
                reblogged: false,
                bookmarked: false,
                media: Vec::new(),
                poll: None,
                emojis: Vec::new(),
                account_emojis: notification_account_emojis.clone(),
                quote_id: None,
                quote_original_url: None,
                quote: None,
                quote_state: None,
                notification_id: Some(notification_id),
                notification_kind,
                notification_label,
                notification_avatar,
                notification_account_id: Some(actor_account_id),
                notification_acct: Some(actor_acct),
                notification_display_name: Some(actor_display_name),
                notification_account_emojis,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_notification_labels_are_stable() {
        assert_eq!(
            notification_type_label(
                crate::mastodon::types::notification::NotificationType::Reblog.as_str()
            ),
            "boosted"
        );
        assert_eq!(
            notification_type_label(
                crate::mastodon::types::notification::NotificationType::Favourite.as_str()
            ),
            "favourited"
        );
    }
}
