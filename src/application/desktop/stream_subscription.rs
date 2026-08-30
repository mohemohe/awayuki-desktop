//! Provider subscription planning for configured timeline columns.
//!
//! Unified Home, Public and Notification timelines ignore historical column
//! account bindings: every capable signed-in session contributes. Account
//! bindings remain meaningful only for Local, List and Hashtag columns. The
//! Active account is not an input because it selects the actor for mutations,
//! never a timeline source.

use super::{provider_supports_aggregate_refresh, ColumnSummary, ServerKind, TimelineType};
use crate::api::client::ApiClient;
use crate::domain::capability::TimelineOperation;
use crate::mastodon::types::streaming::StreamType;
use crate::services::kq_filter::{compile_query as compile_kq_query, SourceKind, SourceSpec};

pub(super) fn stream_types_for_columns(
    columns: &[ColumnSummary],
    source_acct: Option<&str>,
    source_account_id: Option<&str>,
    server_kind: ServerKind,
) -> Vec<StreamType> {
    let mut stream_types = Vec::new();

    for column in columns {
        match column.column_type.as_str() {
            "home" => push_stream_type(&mut stream_types, StreamType::User),
            "public" if provider_supports_aggregate_refresh(server_kind, &TimelineType::Public) => {
                push_stream_type(&mut stream_types, StreamType::Public);
            }
            "notification" => push_stream_type(&mut stream_types, StreamType::UserNotification),
            "local" if column_stream_matches_source(column, source_acct) => {
                push_stream_type(&mut stream_types, StreamType::PublicLocal);
            }
            "list" if column_stream_matches_source(column, source_acct) => {
                if let Some(id) = column.column_param.as_ref().filter(|id| !id.is_empty()) {
                    push_stream_type(&mut stream_types, StreamType::List(id.clone()));
                }
            }
            "feed"
                if server_kind == ServerKind::Bluesky
                    && column_stream_matches_source(column, source_acct) =>
            {
                if let Some(id) = column.column_param.as_ref().filter(|id| !id.is_empty()) {
                    push_stream_type(&mut stream_types, StreamType::Feed(id.clone()));
                }
            }
            "hashtag" if column_stream_matches_source(column, source_acct) => {
                if let Some(tag) = column.column_param.as_ref().filter(|tag| !tag.is_empty()) {
                    push_stream_type(&mut stream_types, StreamType::Hashtag(tag.clone()));
                }
            }
            "kq" => add_kq_stream_types(
                &mut stream_types,
                column,
                source_acct,
                source_account_id,
                server_kind,
            ),
            _ => {}
        }
    }

    // Mastodon user streams and Misskey's combined user socket already carry
    // notifications, so do not open a duplicate notification connection.
    if !matches!(server_kind, ServerKind::Bluesky) && stream_types.contains(&StreamType::User) {
        stream_types.retain(|stream_type| !matches!(stream_type, StreamType::UserNotification));
    }

    stream_types
}

fn add_kq_stream_types(
    stream_types: &mut Vec<StreamType>,
    column: &ColumnSummary,
    source_acct: Option<&str>,
    source_account_id: Option<&str>,
    server_kind: ServerKind,
) {
    let Some(TimelineType::KrileQuery(query)) =
        TimelineType::from_column_config(&column.column_type, column.column_param.as_deref())
    else {
        return;
    };
    let compiled = match compile_kq_query(&query) {
        Ok(compiled) => compiled,
        Err(error) => {
            // Save validation prevents new invalid KQ columns. A portable DB
            // may still contain a legacy/corrupt value, so skip it without
            // logging query text or risking the whole streaming plan.
            tracing::warn!(
                column_id = %column.id,
                line = error.line(),
                column = error.column(),
                "Skipping invalid persisted KQ while planning streams"
            );
            return;
        }
    };

    for source in compiled.sources() {
        match source.kind {
            SourceKind::Local
            | SourceKind::Search
            | SourceKind::Track
            | SourceKind::Conversation
            | SourceKind::User => {
                push_kq_user_baseline(stream_types, server_kind);
            }
            SourceKind::Mentions
            | SourceKind::Direct
            | SourceKind::Bookmarks
            | SourceKind::Favourites
                if source_matches_session(source, source_acct, source_account_id, server_kind) =>
            {
                push_kq_user_baseline(stream_types, server_kind);
            }
            SourceKind::Mentions
            | SourceKind::Direct
            | SourceKind::Bookmarks
            | SourceKind::Favourites => {}
            SourceKind::Home
                if source_matches_session(source, source_acct, source_account_id, server_kind) =>
            {
                push_kq_user_baseline(stream_types, server_kind);
            }
            SourceKind::Home => {}
            SourceKind::Public
                if source_matches_session(source, source_acct, source_account_id, server_kind) =>
            {
                if provider_supports_kq_stream(server_kind, TimelineOperation::Public) {
                    push_stream_type(stream_types, StreamType::Public);
                }
            }
            SourceKind::Public => {}
            SourceKind::LocalPublic
                if source_matches_session(source, source_acct, source_account_id, server_kind) =>
            {
                if provider_supports_kq_stream(server_kind, TimelineOperation::Local) {
                    push_stream_type(stream_types, StreamType::PublicLocal);
                }
            }
            SourceKind::LocalPublic => {}
            SourceKind::Hashtag => {
                if provider_supports_kq_stream(server_kind, TimelineOperation::Hashtags) {
                    for argument in &source.arguments {
                        let tag = argument.trim().trim_start_matches('#');
                        if !tag.is_empty() {
                            push_stream_type(stream_types, StreamType::Hashtag(tag.to_string()));
                        }
                    }
                }
            }
            SourceKind::List => {
                if provider_supports_kq_stream(server_kind, TimelineOperation::Lists) {
                    for argument in &source.arguments {
                        if let Some(list_id) = list_id_for_session(
                            argument,
                            source_acct,
                            source_account_id,
                            server_kind,
                        ) {
                            push_stream_type(stream_types, StreamType::List(list_id));
                        }
                    }
                }
            }
        }
    }
}

fn push_kq_user_baseline(stream_types: &mut Vec<StreamType>, server_kind: ServerKind) {
    if provider_supports_kq_stream(server_kind, TimelineOperation::Home) {
        push_stream_type(stream_types, StreamType::User);
    }
}

fn provider_supports_kq_stream(server_kind: ServerKind, operation: TimelineOperation) -> bool {
    ApiClient::capabilities_for_kind(server_kind, 1)
        .timelines
        .supports(operation)
}

fn source_matches_session(
    source: &SourceSpec,
    source_acct: Option<&str>,
    source_account_id: Option<&str>,
    server_kind: ServerKind,
) -> bool {
    if source.arguments.is_empty()
        || source
            .arguments
            .iter()
            .any(|argument| argument.trim() == "*")
    {
        return true;
    }
    let Some(source_acct) = source_acct else {
        return false;
    };
    source.arguments.iter().any(|argument| {
        account_selector_matches(argument, source_acct, source_account_id, server_kind)
    })
}

fn account_selector_matches(
    selector: &str,
    source_acct: &str,
    source_account_id: Option<&str>,
    server_kind: ServerKind,
) -> bool {
    let selector = selector.trim();
    let normalized = selector
        .strip_prefix('@')
        .or_else(|| selector.strip_prefix('#'))
        .unwrap_or(selector);
    if source_account_id.is_some_and(|account_id| normalized == account_id) {
        return true;
    }

    let raw_acct = source_acct.trim().trim_start_matches('@');
    if normalized.eq_ignore_ascii_case(raw_acct) {
        return true;
    }
    matches!(server_kind, ServerKind::Bluesky)
        && raw_acct
            .rsplit_once('@')
            .filter(|(handle, domain)| handle.contains('.') && !domain.is_empty())
            .is_some_and(|(handle, _)| normalized.eq_ignore_ascii_case(handle))
}

fn list_id_for_session(
    argument: &str,
    source_acct: Option<&str>,
    source_account_id: Option<&str>,
    server_kind: ServerKind,
) -> Option<String> {
    let argument = argument.trim();
    if argument.is_empty() {
        return None;
    }

    // A bare provider list ID intentionally applies to every signed-in
    // session: list IDs are account-local, and membership rows retain the
    // account scope. `acct/list-id` optionally narrows the source without ever
    // forwarding the selector itself to the provider API. Treat a slash as a
    // selector separator only when its prefix is acct-shaped, preserving AT
    // Protocol list URIs such as `at://did/...` as opaque IDs.
    let Some((selector, list_id)) = argument.split_once('/') else {
        return Some(argument.to_string());
    };
    let selector = selector.trim();
    let account_shaped =
        selector.starts_with('@') || selector.contains('@') || selector.contains('.');
    if !account_shaped {
        return Some(argument.to_string());
    }
    let matches_current_session = source_acct.is_some_and(|source_acct| {
        account_selector_matches(selector, source_acct, source_account_id, server_kind)
    });
    if !matches_current_session {
        return None;
    }
    let list_id = list_id.trim();
    (!list_id.is_empty()).then(|| list_id.to_string())
}

fn column_stream_matches_source(column: &ColumnSummary, source_acct: Option<&str>) -> bool {
    let Some(requested) = column
        .account_acct
        .as_deref()
        .filter(|acct| !acct.is_empty())
    else {
        return false;
    };
    source_acct == Some(requested)
}

fn push_stream_type(stream_types: &mut Vec<StreamType>, stream_type: StreamType) {
    if !stream_types.contains(&stream_type) {
        stream_types.push(stream_type);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kq_column(query: &str) -> ColumnSummary {
        ColumnSummary {
            id: "kq-test".to_string(),
            column_type: "kq".to_string(),
            column_param: Some(query.to_string()),
            name: "KQ".to_string(),
            max_statuses: 100,
            pane_index: 0,
            position: 0,
            account_acct: None,
            display_filter: None,
            desktop_notifications: Some(true),
            notification_sound: None,
        }
    }

    fn feed_column(account_acct: &str, feed_id: &str) -> ColumnSummary {
        let mut column = kq_column("from home");
        column.column_type = "feed".to_string();
        column.column_param = Some(feed_id.to_string());
        column.account_acct = Some(account_acct.to_string());
        column
    }

    #[test]
    fn feed_stream_uses_only_the_selected_bluesky_account() {
        let feed_id = "at://did:plc:alice/app.bsky.feed.generator/news";
        let columns = [feed_column("alice.bsky.social", feed_id)];

        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice.bsky.social"),
                Some("did:plc:alice"),
                ServerKind::Bluesky,
            ),
            vec![StreamType::Feed(feed_id.to_string())]
        );
        assert!(stream_types_for_columns(
            &columns,
            Some("bob.bsky.social"),
            Some("did:plc:bob"),
            ServerKind::Bluesky,
        )
        .is_empty());
    }

    #[test]
    fn feed_stream_is_never_planned_for_non_bluesky_providers() {
        let columns = [feed_column(
            "alice@example.test",
            "at://did:plc:alice/app.bsky.feed.generator/news",
        )];

        for kind in [ServerKind::Mastodon, ServerKind::Paon, ServerKind::Misskey] {
            assert!(stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                Some("alice"),
                kind,
            )
            .is_empty());
        }
    }

    #[test]
    fn local_and_where_only_kq_use_each_sessions_user_baseline() {
        for query in ["from local", "from all", "where text contains \"snow\""] {
            for kind in [
                ServerKind::Mastodon,
                ServerKind::Paon,
                ServerKind::Misskey,
                ServerKind::Bluesky,
            ] {
                assert_eq!(
                    stream_types_for_columns(
                        &[kq_column(query)],
                        Some("alice@example.test"),
                        None,
                        kind,
                    ),
                    vec![StreamType::User],
                    "query={query}, provider={kind:?}"
                );
            }
        }
    }

    #[test]
    fn wildcard_optional_kq_sources_open_all_sessions_and_home_acct_selects_one() {
        let bare = [kq_column("from home:*, public:*, mentions:*")];
        for acct in ["alice@example.test", "bob@example.test"] {
            assert_eq!(
                stream_types_for_columns(&bare, Some(acct), None, ServerKind::Mastodon),
                vec![StreamType::User, StreamType::Public],
                "acct={acct}"
            );
        }

        let columns = [kq_column("from home:\"alice@example.test\"")];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::User]
        );
        assert!(stream_types_for_columns(
            &columns,
            Some("bob@example.test"),
            None,
            ServerKind::Mastodon,
        )
        .is_empty());
    }

    #[test]
    fn account_bound_kq_sources_match_opaque_account_ids_exactly() {
        let account_id = "did:plc:AliceCaseSensitive";
        for (source, expected) in [
            ("home", StreamType::User),
            ("mentions", StreamType::User),
            ("direct", StreamType::User),
            ("bookmarks", StreamType::User),
            ("favourites", StreamType::User),
            ("public", StreamType::Public),
            ("local_public", StreamType::PublicLocal),
        ] {
            let columns = [kq_column(&format!("from {source}:#{account_id}"))];
            assert_eq!(
                stream_types_for_columns(
                    &columns,
                    Some("alice@example.test"),
                    Some(account_id),
                    ServerKind::Mastodon,
                ),
                vec![expected],
                "source={source}"
            );
            assert!(
                stream_types_for_columns(
                    &columns,
                    Some("alice@example.test"),
                    Some("did:plc:alicecasesensitive"),
                    ServerKind::Mastodon,
                )
                .is_empty(),
                "account IDs must remain case-sensitive for source={source}"
            );
        }
    }

    #[test]
    fn bluesky_kq_account_selectors_match_new_and_legacy_session_keys() {
        let canonical = [kq_column("from home:\"alice.bsky.social\"")];
        assert_eq!(
            stream_types_for_columns(
                &canonical,
                Some("alice.bsky.social"),
                Some("did:plc:alice"),
                ServerKind::Bluesky,
            ),
            vec![StreamType::User]
        );
        assert_eq!(
            stream_types_for_columns(
                &canonical,
                Some("alice.bsky.social@bsky.social"),
                Some("did:plc:alice"),
                ServerKind::Bluesky,
            ),
            vec![StreamType::User]
        );

        let legacy_full = [kq_column("from home:\"alice.bsky.social@bsky.social\"")];
        assert_eq!(
            stream_types_for_columns(
                &legacy_full,
                Some("alice.bsky.social@bsky.social"),
                Some("did:plc:alice"),
                ServerKind::Bluesky,
            ),
            vec![StreamType::User]
        );
        assert!(stream_types_for_columns(
            &canonical,
            Some("bob.bsky.social@bsky.social"),
            Some("did:plc:bob"),
            ServerKind::Bluesky,
        )
        .is_empty());
    }

    #[test]
    fn public_kq_sources_open_all_sessions_when_bare_and_only_the_selected_session_when_explicit() {
        let bare = [kq_column("from public, local_public")];
        for acct in ["alice@example.test", "bob@example.test"] {
            assert_eq!(
                stream_types_for_columns(&bare, Some(acct), None, ServerKind::Mastodon),
                vec![StreamType::Public, StreamType::PublicLocal],
                "acct={acct}"
            );
        }

        let columns = [kq_column(
            "from public:\"alice@example.test\", local_public:\"alice@example.test\"",
        )];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::Public, StreamType::PublicLocal]
        );
        assert!(stream_types_for_columns(
            &columns,
            Some("bob@example.test"),
            None,
            ServerKind::Mastodon,
        )
        .is_empty());
    }

    #[test]
    fn viewer_scoped_kq_sources_open_all_sessions_when_bare_and_only_the_selected_session_when_explicit(
    ) {
        for source in ["mentions", "direct", "bookmarks", "favourites"] {
            let bare = [kq_column(&format!("from {source}"))];
            for acct in ["alice@example.test", "bob@example.test"] {
                assert_eq!(
                    stream_types_for_columns(&bare, Some(acct), None, ServerKind::Mastodon),
                    vec![StreamType::User],
                    "bare source={source}, acct={acct}"
                );
            }

            let columns = [kq_column(&format!("from {source}:\"alice@example.test\""))];
            assert_eq!(
                stream_types_for_columns(
                    &columns,
                    Some("alice@example.test"),
                    None,
                    ServerKind::Mastodon,
                ),
                vec![StreamType::User],
                "source={source}"
            );
            assert!(
                stream_types_for_columns(
                    &columns,
                    Some("bob@example.test"),
                    None,
                    ServerKind::Mastodon,
                )
                .is_empty(),
                "source={source}"
            );
        }
    }

    #[test]
    fn kq_provider_sources_expand_only_to_supported_streams_and_dedupe() {
        let columns = [kq_column(
            "from public, federated, local_public, hashtag:\"#rust\", list:\"friends\"",
        )];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![
                StreamType::Public,
                StreamType::PublicLocal,
                StreamType::Hashtag("rust".to_string()),
                StreamType::List("friends".to_string()),
            ]
        );
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice.bsky.social"),
                None,
                ServerKind::Bluesky,
            ),
            vec![
                StreamType::Hashtag("rust".to_string()),
                StreamType::List("friends".to_string()),
            ]
        );
    }

    #[test]
    fn kq_list_source_can_scope_an_account_without_forwarding_the_selector() {
        let scoped = [kq_column("from list:\"alice@example.test/friends\"")];
        assert_eq!(
            stream_types_for_columns(
                &scoped,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::List("friends".to_string())]
        );
        assert!(stream_types_for_columns(
            &scoped,
            Some("bob@example.test"),
            None,
            ServerKind::Mastodon,
        )
        .is_empty());

        let at_uri = "at://did:plc:alice/app.bsky.graph.list/friends";
        assert_eq!(
            list_id_for_session(at_uri, Some("alice.bsky.social"), None, ServerKind::Bluesky),
            Some(at_uri.to_string())
        );
        let scoped_at_uri = format!("alice.bsky.social/{at_uri}");
        assert_eq!(
            list_id_for_session(
                &scoped_at_uri,
                Some("alice.bsky.social"),
                None,
                ServerKind::Bluesky,
            ),
            Some(at_uri.to_string())
        );
        assert_eq!(
            list_id_for_session(
                &scoped_at_uri,
                Some("bob.bsky.social"),
                None,
                ServerKind::Bluesky,
            ),
            None
        );

        let legacy_scoped_at_uri = format!("alice.bsky.social/{at_uri}");
        assert_eq!(
            list_id_for_session(
                &legacy_scoped_at_uri,
                Some("alice.bsky.social@bsky.social"),
                Some("did:plc:alice"),
                ServerKind::Bluesky,
            ),
            Some(at_uri.to_string())
        );
        let legacy_scoped_column = [kq_column(&format!(
            "from list:\"alice.bsky.social/{at_uri}\""
        ))];
        assert_eq!(
            stream_types_for_columns(
                &legacy_scoped_column,
                Some("alice.bsky.social@bsky.social"),
                Some("did:plc:alice"),
                ServerKind::Bluesky,
            ),
            vec![StreamType::List(at_uri.to_string())]
        );
    }

    #[test]
    fn non_streamable_kq_sources_use_a_single_safe_user_baseline() {
        let columns = [kq_column(
            "from mentions, direct, search:\"snow\", track:\"ice\", \
             conversation:\"example.test/status-1\", user:\"alice@example.test\", \
             bookmarks, favourites",
        )];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::User]
        );
    }

    #[test]
    fn kq_streams_dedupe_against_equivalent_concrete_columns() {
        let mut home = kq_column("from home");
        home.column_type = "home".to_string();
        home.column_param = None;
        home.account_acct = Some("stale@example.test".to_string());

        let mut list = kq_column("from home");
        list.column_type = "list".to_string();
        list.column_param = Some("friends".to_string());
        list.account_acct = Some("alice@example.test".to_string());

        let mut hashtag = kq_column("from home");
        hashtag.column_type = "hashtag".to_string();
        hashtag.column_param = Some("rust".to_string());
        hashtag.account_acct = Some("alice@example.test".to_string());

        let kq = kq_column("from home, list:\"alice@example.test/friends\", hashtag:\"#rust\"");
        assert_eq!(
            stream_types_for_columns(
                &[home, list, hashtag, kq],
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![
                StreamType::User,
                StreamType::List("friends".to_string()),
                StreamType::Hashtag("rust".to_string()),
            ]
        );
    }

    #[test]
    fn invalid_persisted_kq_is_skipped_without_affecting_other_columns() {
        let invalid = kq_column("from home where text contains");
        assert!(stream_types_for_columns(
            std::slice::from_ref(&invalid),
            Some("alice@example.test"),
            None,
            ServerKind::Mastodon,
        )
        .is_empty());

        let mut home = kq_column("from home");
        home.column_type = "home".to_string();
        home.column_param = None;
        let columns = [invalid, home];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::User]
        );
    }

    #[test]
    fn kq_user_baseline_keeps_existing_notification_stream_integration() {
        let mut notification = kq_column("from local");
        notification.column_type = "notification".to_string();
        notification.column_param = None;
        let columns = [kq_column("from local"), notification];
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice@example.test"),
                None,
                ServerKind::Mastodon,
            ),
            vec![StreamType::User]
        );
        assert_eq!(
            stream_types_for_columns(
                &columns,
                Some("alice.bsky.social"),
                None,
                ServerKind::Bluesky,
            ),
            vec![StreamType::User, StreamType::UserNotification]
        );
    }
}
