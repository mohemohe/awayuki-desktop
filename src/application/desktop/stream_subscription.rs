//! Provider subscription planning for configured timeline columns.
//!
//! Unified Home, Public and Notification timelines ignore historical column
//! account bindings: every capable signed-in session contributes. Account
//! bindings remain meaningful only for Local, List and Hashtag columns. The
//! Active account is not an input because it selects the actor for mutations,
//! never a timeline source.

use super::{provider_supports_aggregate_refresh, ColumnSummary, ServerKind, TimelineType};
use crate::mastodon::types::streaming::StreamType;

pub(super) fn stream_types_for_columns(
    columns: &[ColumnSummary],
    source_acct: Option<&str>,
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
            "hashtag" if column_stream_matches_source(column, source_acct) => {
                if let Some(tag) = column.column_param.as_ref().filter(|tag| !tag.is_empty()) {
                    push_stream_type(&mut stream_types, StreamType::Hashtag(tag.clone()));
                }
            }
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
