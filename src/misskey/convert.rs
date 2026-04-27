//! Convert Misskey API types to the Mastodon-shaped intermediate types
//! the rest of the app already speaks.
//!
//! The mapping is intentionally lossy: Misskey concepts that Mastodon doesn't have
//! (reactions, channels, custom polls flags) are degraded to the closest Mastodon equivalent.

use std::collections::HashMap;

use chrono::Utc;

use crate::mastodon::types::account::{Account, AccountField, CustomEmoji};
use crate::mastodon::types::notification::{Notification, NotificationType};
use crate::mastodon::types::status::{
    Card, MediaAttachment, Mention, Poll, PollOption, Status, Tag,
};
use crate::misskey::types::note::{MisskeyDriveFile, MisskeyNote, MisskeyPoll};
use crate::misskey::types::notification::MisskeyNotification;
use crate::misskey::types::user::{MisskeyEmojis, MisskeyUser};

/// Convert MFM-ish text to a small subset of HTML so the existing renderer can show it.
///
/// The existing renderer relies on `<p>`, `<br>`, `<a>`, and a handful of inline tags. We don't
/// implement the full MFM grammar — that's a project of its own. Instead we:
/// - escape HTML metacharacters,
/// - preserve newlines as `<br>`,
/// - turn `@user` mentions and bare URLs into `<a>` tags,
/// - wrap the whole thing in `<p>`.
pub fn mfm_to_html(text: &str, local_host: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let escaped = html_escape(text);
    let with_links = linkify(&escaped, local_host);
    let with_mentions = mentionify(&with_links, local_host);
    let with_hashtags = hashtagify(&with_mentions);
    let with_breaks = with_hashtags.replace('\n', "<br>");
    format!("<p>{}</p>", with_breaks)
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn linkify(text: &str, _local_host: &str) -> String {
    // Very permissive URL detection: match http(s)://… until whitespace or HTML angle.
    // Good enough for Mastodon-compatible rendering; refine later if needed.
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = find_url_start(rest) {
        out.push_str(&rest[..idx]);
        let after = &rest[idx..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == '<' || c == '>')
            .unwrap_or(after.len());
        let url = &after[..end];
        out.push_str(&format!(
            "<a href=\"{0}\" rel=\"nofollow noopener noreferrer\" target=\"_blank\">{0}</a>",
            url
        ));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn find_url_start(text: &str) -> Option<usize> {
    let candidates = ["http://", "https://"];
    candidates
        .iter()
        .filter_map(|s| text.find(s))
        .min()
}

fn mentionify(text: &str, local_host: &str) -> String {
    // Match @user or @user@host
    let re = match regex::Regex::new(r"@([A-Za-z0-9_]+)(?:@([A-Za-z0-9.\-]+))?") {
        Ok(re) => re,
        Err(_) => return text.to_string(),
    };
    re.replace_all(text, |caps: &regex::Captures| {
        let username = &caps[1];
        let host = caps.get(2).map(|m| m.as_str()).unwrap_or(local_host);
        let acct = if host == local_host {
            format!("@{}", username)
        } else {
            format!("@{}@{}", username, host)
        };
        format!(
            "<span class=\"h-card\"><a href=\"https://{}/@{}\" class=\"u-url mention\">{}</a></span>",
            host, username, acct
        )
    })
    .to_string()
}

fn hashtagify(text: &str) -> String {
    let re = match regex::Regex::new(r"#([\w\u3000-\u30ff\u3400-\u9fff]+)") {
        Ok(re) => re,
        Err(_) => return text.to_string(),
    };
    re.replace_all(text, |caps: &regex::Captures| {
        let tag = &caps[1];
        format!("<a href=\"#\" class=\"mention hashtag\" rel=\"tag\">#{}</a>", tag)
    })
    .to_string()
}

/// Convert Misskey emojis (map or list form) into Mastodon CustomEmoji vector.
fn convert_emojis(emojis: Option<MisskeyEmojis>) -> Vec<CustomEmoji> {
    let mut out = Vec::new();
    if let Some(e) = emojis {
        for (name, url) in e.into_pairs() {
            out.push(CustomEmoji {
                shortcode: name,
                url: url.clone(),
                static_url: url,
                visible_in_picker: true,
                category: None,
            });
        }
    }
    out
}

/// Build the canonical `acct` (`user` for local, `user@host` for remote).
pub fn user_acct(user: &MisskeyUser, _local_host: &str) -> String {
    match &user.host {
        Some(host) if !host.is_empty() => format!("{}@{}", user.username, host),
        _ => user.username.clone(),
    }
}

pub fn user_to_account(user: &MisskeyUser, local_host: &str) -> Account {
    let acct = user_acct(user, local_host);
    let host = user.host.as_deref().unwrap_or(local_host);
    let url = format!("https://{}/@{}", host, user.username);

    let fields = user
        .fields
        .iter()
        .map(|f| AccountField {
            name: f.name.clone(),
            value: f.value.clone(),
            verified_at: None,
        })
        .collect();

    let avatar = user.avatar_url.clone().unwrap_or_default();
    let header = user.banner_url.clone().unwrap_or_default();
    let note = user
        .description
        .as_deref()
        .map(|d| mfm_to_html(d, local_host))
        .unwrap_or_default();

    Account {
        id: user.id.clone(),
        username: user.username.clone(),
        acct,
        display_name: user.name.clone().unwrap_or_else(|| user.username.clone()),
        note,
        url,
        uri: String::new(),
        avatar: avatar.clone(),
        avatar_static: avatar,
        header: header.clone(),
        header_static: header,
        locked: user.is_locked,
        bot: user.is_bot,
        created_at: user.created_at.unwrap_or_else(Utc::now),
        followers_count: user.followers_count.unwrap_or(0),
        following_count: user.following_count.unwrap_or(0),
        statuses_count: user.notes_count.unwrap_or(0),
        fields,
        emojis: convert_emojis(user.emojis.clone()),
        pleroma: None,
    }
}

fn convert_visibility(visibility: &str) -> String {
    match visibility {
        "public" => "public".to_string(),
        "home" => "unlisted".to_string(),
        "followers" => "private".to_string(),
        "specified" => "direct".to_string(),
        _ => "public".to_string(),
    }
}

fn file_to_attachment(file: &MisskeyDriveFile) -> MediaAttachment {
    let media_type = if file.r#type.starts_with("image/") {
        if file.r#type == "image/gif" {
            "gifv".to_string()
        } else {
            "image".to_string()
        }
    } else if file.r#type.starts_with("video/") {
        "video".to_string()
    } else if file.r#type.starts_with("audio/") {
        "audio".to_string()
    } else {
        "unknown".to_string()
    };

    MediaAttachment {
        id: file.id.clone(),
        media_type,
        url: Some(file.url.clone()),
        preview_url: file.thumbnail_url.clone().or(Some(file.url.clone())),
        remote_url: None,
        description: file.comment.clone(),
        blurhash: file.blurhash.clone(),
        meta: None,
    }
}

pub fn poll_to_mastodon_public(poll: &MisskeyPoll, note_id: &str) -> Poll {
    poll_to_mastodon(poll, note_id)
}

fn poll_to_mastodon(poll: &MisskeyPoll, note_id: &str) -> Poll {
    let total: i64 = poll.choices.iter().map(|c| c.votes).sum();
    let voted = poll.choices.iter().any(|c| c.is_voted);
    let own_votes: Vec<i64> = poll
        .choices
        .iter()
        .enumerate()
        .filter_map(|(idx, c)| if c.is_voted { Some(idx as i64) } else { None })
        .collect();

    Poll {
        id: note_id.to_string(),
        expires_at: poll.expires_at,
        expired: poll
            .expires_at
            .map(|d| d < Utc::now())
            .unwrap_or(false),
        multiple: poll.multiple,
        votes_count: total,
        voters_count: None,
        options: poll
            .choices
            .iter()
            .map(|c| PollOption {
                title: c.text.clone(),
                votes_count: Some(c.votes),
            })
            .collect(),
        voted: Some(voted),
        own_votes: if own_votes.is_empty() {
            None
        } else {
            Some(own_votes)
        },
        emojis: vec![],
    }
}

/// Sum a `reactions` JSON object — Misskey returns either `{ "🌟": 3, ":foo:": 1 }`
/// or sometimes a list. We treat the total as `favourites_count`.
fn reactions_total(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::Object(map) => map
            .values()
            .map(|v| v.as_i64().unwrap_or(0))
            .sum(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(|item| {
                item.get("count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
            })
            .sum(),
        _ => 0,
    }
}

/// Convert a Misskey Note to a Mastodon-compatible Status.
///
/// `local_host` is the bare host (e.g. `misskey.example`) of the server we're talking to;
/// it's used to build absolute URLs and resolve mention `@user` (no host) to local users.
pub fn note_to_status(note: &MisskeyNote, local_host: &str) -> Status {
    note_to_status_inner(note, local_host, true)
}

fn note_to_status_inner(note: &MisskeyNote, local_host: &str, allow_renote: bool) -> Status {
    let visibility = convert_visibility(&note.visibility);
    let author_host = note.user.host.as_deref().unwrap_or(local_host);
    let url = note
        .url
        .clone()
        .or_else(|| Some(format!("https://{}/notes/{}", author_host, note.id)));
    let uri = note
        .uri
        .clone()
        .unwrap_or_else(|| format!("https://{}/notes/{}", author_host, note.id));

    let text = note.text.clone().unwrap_or_default();
    let content = mfm_to_html(&text, local_host);
    let spoiler_text = note.cw.clone().unwrap_or_default();
    let sensitive = !spoiler_text.is_empty()
        || note.files.iter().any(|f| f.is_sensitive);

    let media_attachments: Vec<MediaAttachment> =
        note.files.iter().map(file_to_attachment).collect();

    // Misskey "renote" with no text is a pure boost (Mastodon reblog).
    // A renote that *does* have text is a quote post.
    let is_pure_renote = note.renote.is_some() && note.text.is_none() && note.cw.is_none();

    let reblog: Option<Box<Status>> = if allow_renote && is_pure_renote {
        note.renote
            .as_ref()
            .map(|r| Box::new(note_to_status_inner(r, local_host, false)))
    } else {
        None
    };

    let (quote_id, quote, quote_original_url) = if !is_pure_renote {
        match note.renote.as_ref() {
            Some(rn) => (
                Some(rn.id.clone()),
                Some(Box::new(note_to_status_inner(rn, local_host, false))),
                rn.url.clone(),
            ),
            None => (None, None, None),
        }
    } else {
        (None, None, None)
    };

    let tags: Vec<Tag> = note
        .tags
        .iter()
        .map(|name| Tag {
            name: name.clone(),
            url: format!("https://{}/tags/{}", local_host, name),
        })
        .collect();

    // Mentions — Misskey returns user IDs only; we don't have full user info here.
    // The renderer mostly cares about acct text inside content, so mention list can stay empty
    // for now.
    let mentions: Vec<Mention> = note
        .mentions
        .iter()
        .map(|id| Mention {
            id: id.clone(),
            username: id.clone(),
            acct: id.clone(),
            url: String::new(),
        })
        .collect();

    let emojis = convert_emojis(note.emojis.clone());

    let poll = note.poll.as_ref().map(|p| poll_to_mastodon(p, &note.id));

    let favourites_count = reactions_total(&note.reactions);
    let favourited = Some(note.my_reaction.is_some());

    Status {
        id: note.id.clone(),
        uri,
        url,
        created_at: note.created_at,
        edited_at: None,
        account: user_to_account(&note.user, local_host),
        content,
        visibility,
        sensitive,
        spoiler_text,
        media_attachments,
        mentions,
        tags,
        emojis,
        reblogs_count: note.renote_count,
        favourites_count,
        replies_count: note.replies_count,
        in_reply_to_id: note.reply_id.clone(),
        in_reply_to_account_id: note.reply.as_ref().map(|r| r.user.id.clone()),
        reblog,
        language: None,
        pinned: None,
        favourited,
        reblogged: Some(false),
        muted: None,
        bookmarked: None,
        poll,
        card: None as Option<Card>,
        application: None,
        quote_id,
        quote,
        quote_original_url,
        pleroma: None,
    }
}

/// Convert a Misskey notification to the Mastodon-compatible variant. Some Misskey-specific
/// notification types (reaction, etc.) collapse into `Favourite` for compatibility.
pub fn notification_to_mastodon(
    notif: &MisskeyNotification,
    local_host: &str,
) -> Option<Notification> {
    let user = notif.user.as_ref()?;
    let account = user_to_account(user, local_host);

    let notification_type = match notif.r#type.as_str() {
        "mention" | "reply" => NotificationType::Mention,
        "renote" | "quote" => NotificationType::Reblog,
        "reaction" | "favourite" => NotificationType::Favourite,
        "follow" => NotificationType::Follow,
        "receiveFollowRequest" => NotificationType::FollowRequest,
        "pollEnded" | "pollVote" => NotificationType::Poll,
        "note" => NotificationType::Status,
        _ => return None,
    };

    let status = notif
        .note
        .as_ref()
        .map(|n| note_to_status(n, local_host));

    Some(Notification {
        id: notif.id.clone(),
        notification_type,
        created_at: notif.created_at,
        account,
        status,
    })
}

/// Build a HashMap from Misskey emojis catalog response so the existing custom-emoji picker can
/// consume it after passing through our shared `CustomEmoji` type.
pub fn catalog_to_custom_emojis(
    entries: &[crate::misskey::types::meta::MisskeyEmojiCatalogEntry],
) -> Vec<CustomEmoji> {
    entries
        .iter()
        .map(|e| CustomEmoji {
            shortcode: e.name.clone(),
            url: e.url.clone(),
            static_url: e.url.clone(),
            visible_in_picker: true,
            category: e.category.clone(),
        })
        .collect()
}

/// Visibility (Mastodon → Misskey) for note creation.
pub fn visibility_to_misskey(mastodon_visibility: &str) -> &'static str {
    match mastodon_visibility {
        "public" => "public",
        "unlisted" => "home",
        "private" => "followers",
        "direct" => "specified",
        _ => "public",
    }
}

/// Build a quick lookup map from Misskey reactions JSON.
#[allow(dead_code)]
pub fn reactions_map(value: &serde_json::Value) -> HashMap<String, i64> {
    let mut map = HashMap::new();
    if let serde_json::Value::Object(obj) = value {
        for (k, v) in obj {
            if let Some(n) = v.as_i64() {
                map.insert(k.clone(), n);
            }
        }
    }
    map
}
