//! Convert AT Protocol (Bluesky) types to the Mastodon-shaped intermediate types
//! the rest of the app already speaks.
//!
//! The mapping is intentionally lossy: AT Protocol concepts that Mastodon doesn't have
//! (CIDs, AT-URIs, feed generators) are degraded to the closest Mastodon equivalent.

use atrium_api::app::bsky::actor::defs::{
    ProfileViewBasic, ProfileViewBasicData, ProfileViewDetailed, ProfileViewDetailedData,
};
use atrium_api::app::bsky::feed::defs::{
    FeedViewPost, FeedViewPostReasonRefs, PostView, PostViewEmbedRefs, ReplyRefParentRefs,
};
use atrium_api::types::{TryFromUnknown, Union};
use chrono::{DateTime, Utc};

use crate::mastodon::types::account::{Account, AccountField, CustomEmoji};
use crate::mastodon::types::status::{Card, MediaAttachment, Mention, Poll, Status, Tag};

/// Public host for Bluesky web UI links.
pub const BSKY_APP_HOST: &str = "bsky.app";

/// Convert a Bluesky basic profile to a Mastodon-shaped Account.
pub fn profile_basic_to_account(profile: &ProfileViewBasic) -> Account {
    profile_basic_data_to_account(&profile.data)
}

pub fn profile_basic_data_to_account(profile: &ProfileViewBasicData) -> Account {
    let did = profile.did.as_str().to_string();
    let handle = profile.handle.as_str().to_string();
    let avatar = profile.avatar.clone().unwrap_or_default();
    let display_name = profile
        .display_name
        .clone()
        .unwrap_or_else(|| handle.clone());
    let created_at = profile
        .created_at
        .as_ref()
        .and_then(|d| DateTime::parse_from_rfc3339(d.as_str()).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Account {
        id: did,
        username: handle.clone(),
        acct: handle.clone(),
        display_name,
        note: String::new(),
        url: format!("https://{}/profile/{}", BSKY_APP_HOST, handle),
        uri: String::new(),
        avatar: avatar.clone(),
        avatar_static: avatar,
        header: String::new(),
        header_static: String::new(),
        locked: false,
        bot: false,
        created_at,
        followers_count: 0,
        following_count: 0,
        statuses_count: 0,
        fields: Vec::new(),
        emojis: Vec::new(),
        pleroma: None,
    }
}

/// Convert the richer detailed profile (returned by getProfile) to a Mastodon Account.
pub fn profile_detailed_to_account(profile: &ProfileViewDetailed) -> Account {
    profile_detailed_data_to_account(&profile.data)
}

pub fn profile_detailed_data_to_account(profile: &ProfileViewDetailedData) -> Account {
    let did = profile.did.as_str().to_string();
    let handle = profile.handle.as_str().to_string();
    let avatar = profile.avatar.clone().unwrap_or_default();
    let header = profile.banner.clone().unwrap_or_default();
    let display_name = profile
        .display_name
        .clone()
        .unwrap_or_else(|| handle.clone());
    let note = profile
        .description
        .clone()
        .map(|d| description_to_html(&d))
        .unwrap_or_default();
    let created_at = profile
        .created_at
        .as_ref()
        .and_then(|d| DateTime::parse_from_rfc3339(d.as_str()).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Account {
        id: did,
        username: handle.clone(),
        acct: handle.clone(),
        display_name,
        note,
        url: format!("https://{}/profile/{}", BSKY_APP_HOST, handle),
        uri: String::new(),
        avatar: avatar.clone(),
        avatar_static: avatar,
        header: header.clone(),
        header_static: header,
        locked: false,
        bot: false,
        created_at,
        followers_count: profile.followers_count.unwrap_or(0),
        following_count: profile.follows_count.unwrap_or(0),
        statuses_count: profile.posts_count.unwrap_or(0),
        fields: Vec::new() as Vec<AccountField>,
        emojis: Vec::new() as Vec<CustomEmoji>,
        pleroma: None,
    }
}

/// Convert a Bluesky FeedViewPost (timeline entry) to a Mastodon Status.
///
/// The FeedViewPost wraps a PostView and may carry a `reason` (reasonRepost — meaning
/// "X reposted this") or a `reply` (parent reference). We surface reposts via Mastodon's
/// `reblog` field so existing UI treats them like boosts.
pub fn feed_view_post_to_status(feed: &FeedViewPost) -> Status {
    let post_status = post_view_to_status(&feed.post);

    if let Some(Union::Refs(FeedViewPostReasonRefs::ReasonRepost(reason))) = &feed.data.reason {
        let reposter = profile_basic_data_to_account(&reason.by.data);
        let indexed_at = parse_datetime(reason.indexed_at.as_str());
        let repost_id = format!("repost:{}:{}", reposter.id, post_status.id);
        return Status {
            id: repost_id.clone(),
            uri: repost_id,
            url: post_status.url.clone(),
            created_at: indexed_at,
            edited_at: None,
            account: reposter,
            content: String::new(),
            visibility: "public".to_string(),
            sensitive: false,
            spoiler_text: String::new(),
            media_attachments: Vec::new(),
            mentions: Vec::new(),
            tags: Vec::new(),
            emojis: Vec::new(),
            reblogs_count: 0,
            favourites_count: 0,
            replies_count: 0,
            in_reply_to_id: None,
            in_reply_to_account_id: None,
            reblog: Some(Box::new(post_status)),
            language: None,
            pinned: None,
            favourited: Some(false),
            reblogged: Some(true),
            muted: None,
            bookmarked: None,
            poll: None,
            card: None,
            application: None,
            quote_id: None,
            quote: None,
            quote_original_url: None,
            pleroma: None,
        };
    }

    post_status
}

/// Convert a Bluesky PostView to a Mastodon Status.
pub fn post_view_to_status(post: &PostView) -> Status {
    let data = &post.data;
    let author = profile_basic_data_to_account(&data.author.data);
    let url = post_uri_to_url(&data.uri, &author.username);
    let (text, langs, facets) = extract_post_text_langs_facets(&data.record);
    let content = text_with_facets_to_html(&text, &facets);
    let created_at = post_record_created_at(&data.record)
        .unwrap_or_else(|| parse_datetime(data.indexed_at.as_str()));

    let media_attachments = extract_media_attachments(data.embed.as_ref());
    let card = extract_external_card(data.embed.as_ref());
    let (quote_id, quote, quote_original_url) = extract_quote(data.embed.as_ref());

    let (favourited, reblogged, bookmarked) = match data.viewer.as_ref() {
        Some(viewer) => (
            Some(viewer.like.is_some()),
            Some(viewer.repost.is_some()),
            viewer.bookmarked,
        ),
        None => (Some(false), Some(false), None),
    };

    let (in_reply_to_id, in_reply_to_account_id) = post_record_reply(&data.record);

    Status {
        id: data.uri.clone(),
        uri: data.uri.clone(),
        url: Some(url),
        created_at,
        edited_at: None,
        account: author,
        content,
        visibility: "public".to_string(),
        sensitive: false,
        spoiler_text: String::new(),
        media_attachments,
        mentions: Vec::new() as Vec<Mention>,
        tags: Vec::new() as Vec<Tag>,
        emojis: Vec::new() as Vec<CustomEmoji>,
        reblogs_count: data.repost_count.unwrap_or(0),
        favourites_count: data.like_count.unwrap_or(0),
        replies_count: data.reply_count.unwrap_or(0),
        in_reply_to_id,
        in_reply_to_account_id,
        reblog: None,
        language: langs.into_iter().next(),
        pinned: None,
        favourited,
        reblogged,
        muted: None,
        bookmarked,
        poll: None as Option<Poll>,
        card,
        application: None,
        quote_id,
        quote,
        quote_original_url,
        pleroma: None,
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn post_uri_to_url(uri: &str, handle: &str) -> String {
    if let Some(rkey) = uri.rsplit('/').next() {
        format!("https://{}/profile/{}/post/{}", BSKY_APP_HOST, handle, rkey)
    } else {
        format!("https://{}/profile/{}", BSKY_APP_HOST, handle)
    }
}

fn extract_post_text_langs_facets(
    record: &atrium_api::types::Unknown,
) -> (
    String,
    Vec<String>,
    Vec<atrium_api::app::bsky::richtext::facet::Main>,
) {
    match atrium_api::app::bsky::feed::post::Record::try_from_unknown(record.clone()) {
        Ok(rec) => {
            let data = rec.data;
            let langs = data
                .langs
                .map(|ls| ls.iter().map(|l| l.as_ref().to_string()).collect())
                .unwrap_or_default();
            let facets = data.facets.unwrap_or_default();
            (data.text, langs, facets)
        }
        Err(_) => (String::new(), Vec::new(), Vec::new()),
    }
}

fn post_record_created_at(record: &atrium_api::types::Unknown) -> Option<DateTime<Utc>> {
    let rec = atrium_api::app::bsky::feed::post::Record::try_from_unknown(record.clone()).ok()?;
    Some(parse_datetime(rec.data.created_at.as_str()))
}

fn post_record_reply(record: &atrium_api::types::Unknown) -> (Option<String>, Option<String>) {
    let Ok(rec) = atrium_api::app::bsky::feed::post::Record::try_from_unknown(record.clone())
    else {
        return (None, None);
    };
    let Some(reply) = rec.data.reply else {
        return (None, None);
    };
    let parent_uri = reply.parent.uri.clone();
    let parent_did = at_uri_extract_did(&parent_uri);
    (Some(parent_uri), parent_did)
}

fn at_uri_extract_did(uri: &str) -> Option<String> {
    // "at://did:plc:xxx/app.bsky.feed.post/yyyy" → "did:plc:xxx"
    let rest = uri.strip_prefix("at://")?;
    let did = rest.split('/').next()?;
    if did.starts_with("did:") {
        Some(did.to_string())
    } else {
        None
    }
}

fn extract_media_attachments(embed: Option<&Union<PostViewEmbedRefs>>) -> Vec<MediaAttachment> {
    let Some(Union::Refs(refs)) = embed else {
        return Vec::new();
    };
    match refs {
        PostViewEmbedRefs::AppBskyEmbedImagesView(view) => view
            .data
            .images
            .iter()
            .enumerate()
            .map(|(idx, img)| MediaAttachment {
                id: format!("img-{}-{}", idx, img.data.thumb),
                media_type: "image".to_string(),
                url: Some(img.data.fullsize.clone()),
                preview_url: Some(img.data.thumb.clone()),
                remote_url: None,
                description: if img.data.alt.is_empty() {
                    None
                } else {
                    Some(img.data.alt.clone())
                },
                blurhash: None,
                meta: None,
            })
            .collect(),
        PostViewEmbedRefs::AppBskyEmbedVideoView(view) => vec![MediaAttachment {
            id: view.data.cid.as_ref().to_string(),
            media_type: "video".to_string(),
            url: Some(view.data.playlist.clone()),
            preview_url: view.data.thumbnail.clone(),
            remote_url: None,
            description: view.data.alt.clone(),
            blurhash: None,
            meta: None,
        }],
        PostViewEmbedRefs::AppBskyEmbedRecordWithMediaView(view) => {
            // Recurse into the media side (images/video) and ignore the record side here
            // (handled by extract_quote).
            // Avoid recursive Box<Union> by inlining the media match.
            match &view.data.media {
                Union::Refs(media_refs) => match media_refs {
                    atrium_api::app::bsky::embed::record_with_media::ViewMediaRefs::AppBskyEmbedImagesView(images) => images
                        .data
                        .images
                        .iter()
                        .enumerate()
                        .map(|(idx, img)| MediaAttachment {
                            id: format!("img-{}-{}", idx, img.data.thumb),
                            media_type: "image".to_string(),
                            url: Some(img.data.fullsize.clone()),
                            preview_url: Some(img.data.thumb.clone()),
                            remote_url: None,
                            description: if img.data.alt.is_empty() {
                                None
                            } else {
                                Some(img.data.alt.clone())
                            },
                            blurhash: None,
                            meta: None,
                        })
                        .collect(),
                    atrium_api::app::bsky::embed::record_with_media::ViewMediaRefs::AppBskyEmbedVideoView(video) => vec![MediaAttachment {
                        id: video.data.cid.as_ref().to_string(),
                        media_type: "video".to_string(),
                        url: Some(video.data.playlist.clone()),
                        preview_url: video.data.thumbnail.clone(),
                        remote_url: None,
                        description: video.data.alt.clone(),
                        blurhash: None,
                        meta: None,
                    }],
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

fn extract_external_card(embed: Option<&Union<PostViewEmbedRefs>>) -> Option<Card> {
    let Some(Union::Refs(refs)) = embed else {
        return None;
    };
    match refs {
        PostViewEmbedRefs::AppBskyEmbedExternalView(view) => {
            let ext = &view.data.external;
            Some(Card {
                url: ext.data.uri.clone(),
                title: ext.data.title.clone(),
                description: ext.data.description.clone(),
                card_type: "link".to_string(),
                image: ext.data.thumb.clone(),
                provider_name: None,
                provider_url: None,
                author_name: None,
                author_url: None,
                blurhash: None,
            })
        }
        _ => None,
    }
}

fn extract_quote(
    embed: Option<&Union<PostViewEmbedRefs>>,
) -> (Option<String>, Option<Box<Status>>, Option<String>) {
    let Some(Union::Refs(refs)) = embed else {
        return (None, None, None);
    };
    match refs {
        PostViewEmbedRefs::AppBskyEmbedRecordView(view) => {
            extract_quote_from_record_view(&view.data.record)
        }
        PostViewEmbedRefs::AppBskyEmbedRecordWithMediaView(view) => {
            extract_quote_from_record_view(&view.data.record.data.record)
        }
        _ => (None, None, None),
    }
}

fn extract_quote_from_record_view(
    record: &Union<atrium_api::app::bsky::embed::record::ViewRecordRefs>,
) -> (Option<String>, Option<Box<Status>>, Option<String>) {
    let Union::Refs(refs) = record else {
        return (None, None, None);
    };
    match refs {
        atrium_api::app::bsky::embed::record::ViewRecordRefs::ViewRecord(view) => {
            let data = &view.data;
            let author = profile_basic_data_to_account(&data.author.data);
            let url = post_uri_to_url(&data.uri, &author.username);
            let (text, _, facets) = extract_post_text_langs_facets(&data.value);
            let content = text_with_facets_to_html(&text, &facets);
            let created_at = post_record_created_at(&data.value)
                .unwrap_or_else(|| parse_datetime(data.indexed_at.as_str()));

            let quoted = Status {
                id: data.uri.clone(),
                uri: data.uri.clone(),
                url: Some(url.clone()),
                created_at,
                edited_at: None,
                account: author,
                content,
                visibility: "public".to_string(),
                sensitive: false,
                spoiler_text: String::new(),
                media_attachments: Vec::new(),
                mentions: Vec::new(),
                tags: Vec::new(),
                emojis: Vec::new(),
                reblogs_count: data.repost_count.unwrap_or(0),
                favourites_count: data.like_count.unwrap_or(0),
                replies_count: data.reply_count.unwrap_or(0),
                in_reply_to_id: None,
                in_reply_to_account_id: None,
                reblog: None,
                language: None,
                pinned: None,
                favourited: None,
                reblogged: None,
                muted: None,
                bookmarked: None,
                poll: None,
                card: None,
                application: None,
                quote_id: None,
                quote: None,
                quote_original_url: None,
                pleroma: None,
            };
            (Some(data.uri.clone()), Some(Box::new(quoted)), Some(url))
        }
        _ => (None, None, None),
    }
}

/// Convert a Bluesky post text (without facets) to a tiny subset of HTML.
/// Used for fields that don't carry rich-text annotations (e.g. profile descriptions).
pub fn text_to_html(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let escaped = html_escape(text);
    let with_links = linkify(&escaped);
    let with_breaks = with_links.replace('\n', "<br>");
    format!("<p>{}</p>", with_breaks)
}

/// Convert a Bluesky post text plus its rich-text facets into HTML.
///
/// Facets carry byte ranges (UTF-8 indices into `text`) and a feature describing
/// what the range represents — a link, a mention, or a hashtag. The display text
/// in a facet may be a simplified form of the actual URI (e.g. "sqex.to/VmsAO"
/// for `https://sqex.to/VmsAO`), which is exactly why we cannot rely on plain
/// URL detection alone.
pub fn text_with_facets_to_html(
    text: &str,
    facets: &[atrium_api::app::bsky::richtext::facet::Main],
) -> String {
    if text.is_empty() {
        return String::new();
    }
    if facets.is_empty() {
        return text_to_html(text);
    }

    // Sort by byte_start so we can walk the text once. Skip facets with invalid
    // ranges (start >= end, end past the buffer, or splits inside a multibyte char).
    let mut sorted: Vec<&atrium_api::app::bsky::richtext::facet::Main> = facets.iter().collect();
    sorted.sort_by_key(|f| f.data.index.byte_start);

    let mut out = String::with_capacity(text.len() + 64);
    let mut cursor: usize = 0;

    for facet in sorted {
        let start = facet.data.index.byte_start;
        let end = facet.data.index.byte_end;

        if start < cursor || start >= end || end > text.len() {
            continue;
        }
        let Some(segment) = text.get(start..end) else {
            continue;
        };

        if cursor < start {
            let chunk = text.get(cursor..start).unwrap_or("");
            out.push_str(&linkify(&html_escape(chunk)));
        }

        let escaped_segment = html_escape(segment);
        let mut handled = false;
        for feature in &facet.data.features {
            let Union::Refs(refs) = feature else {
                continue;
            };
            use atrium_api::app::bsky::richtext::facet::MainFeaturesItem;
            match refs {
                MainFeaturesItem::Link(link) => {
                    out.push_str(&format!(
                        "<a href=\"{}\" rel=\"nofollow noopener noreferrer\" target=\"_blank\">{}</a>",
                        html_escape(&link.data.uri),
                        escaped_segment
                    ));
                    handled = true;
                    break;
                }
                MainFeaturesItem::Mention(mention) => {
                    let url = format!(
                        "https://{}/profile/{}",
                        BSKY_APP_HOST,
                        mention.data.did.as_ref()
                    );
                    out.push_str(&format!(
                        "<a href=\"{}\" rel=\"nofollow noopener noreferrer\" target=\"_blank\">{}</a>",
                        html_escape(&url),
                        escaped_segment
                    ));
                    handled = true;
                    break;
                }
                MainFeaturesItem::Tag(tag) => {
                    let url = format!(
                        "https://{}/hashtag/{}",
                        BSKY_APP_HOST,
                        urlencoding::encode(&tag.data.tag)
                    );
                    out.push_str(&format!(
                        "<a href=\"{}\" rel=\"nofollow noopener noreferrer\" target=\"_blank\">{}</a>",
                        html_escape(&url),
                        escaped_segment
                    ));
                    handled = true;
                    break;
                }
            }
        }

        if !handled {
            out.push_str(&escaped_segment);
        }

        cursor = end;
    }

    if cursor < text.len() {
        let chunk = text.get(cursor..).unwrap_or("");
        out.push_str(&linkify(&html_escape(chunk)));
    }

    let with_breaks = out.replace('\n', "<br>");
    format!("<p>{}</p>", with_breaks)
}

fn description_to_html(text: &str) -> String {
    text_to_html(text)
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

fn linkify(text: &str) -> String {
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
    candidates.iter().filter_map(|s| text.find(s)).min()
}

/// Try to extract the parent post and root post from a getPostThread response chain.
#[allow(dead_code)]
pub fn parent_to_status(parent: &Union<ReplyRefParentRefs>) -> Option<Status> {
    let Union::Refs(refs) = parent else {
        return None;
    };
    match refs {
        ReplyRefParentRefs::PostView(post) => Some(post_view_to_status(post)),
        _ => None,
    }
}
