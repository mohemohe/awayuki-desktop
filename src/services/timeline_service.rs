use std::time::{Duration, Instant};

use chrono::Utc;
use sqlx::SqlitePool;

use crate::api::client::ApiClient;
use crate::api::kind::ServerKind;
use crate::db::models::{DbAccount, DbStatus};
use crate::db::queries::{accounts, servers, settings, statuses, tags, timeline};
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::StreamType;

const DB_WRITE_MAX_ATTEMPTS: usize = 5;
const DB_WRITE_RETRY_DELAYS_MS: [u64; DB_WRITE_MAX_ATTEMPTS - 1] = [200, 500, 1_000, 2_000];
const QUOTE_RESOLUTION_RETRY_DELAYS_MS: [u64; 6] = [1_000, 2_000, 4_000, 8_000, 15_000, 30_000];
const QUOTE_LINK_RETRY_RECENCY_SECS: i64 = 15 * 60;

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u64::MAX as u128) as u64
}

/// Timeline types stored in the database
#[derive(Debug, Clone)]
pub enum TimelineType {
    Home,
    Public,
    Local,
    List(String),
    Hashtag(String),
    Notification,
    CustomSql(String),
    Bookmarks,
    UserBookmarks {
        server_domain: String,
        account_id: String,
    },
    Search(String),
    YukariQuery(String),
}

impl TimelineType {
    pub fn as_str(&self) -> String {
        match self {
            Self::Home => "home".to_string(),
            Self::Public => "public".to_string(),
            Self::Local => "local".to_string(),
            Self::List(id) => format!("list:{}", id),
            Self::Hashtag(tag) => format!("tag:{}", tag),
            Self::Notification => "notification".to_string(),
            Self::CustomSql(_) => "custom".to_string(),
            Self::Bookmarks => "bookmarks".to_string(),
            Self::UserBookmarks { .. } => "user_bookmarks".to_string(),
            Self::Search(_) => "search".to_string(),
            Self::YukariQuery(_) => "yq".to_string(),
        }
    }

    /// Convert from column_configs DB row to TimelineType
    pub fn from_column_config(column_type: &str, column_param: Option<&str>) -> Option<Self> {
        match column_type {
            "home" => Some(Self::Home),
            "public" => Some(Self::Public),
            "local" => Some(Self::Local),
            "notification" => Some(Self::Notification),
            "bookmarks" => Some(Self::Bookmarks),
            "list" => column_param.map(|id| Self::List(id.to_string())),
            "hashtag" => column_param.map(|tag| Self::Hashtag(tag.to_string())),
            "custom" => column_param.map(|sql| Self::CustomSql(sql.to_string())),
            "search" => column_param.map(|q| Self::Search(q.to_string())),
            "yq" => column_param.map(|q| Self::YukariQuery(q.to_string())),
            "user_bookmarks" => column_param.and_then(parse_user_bookmarks_column_param),
            _ => None,
        }
    }

    /// Convert to (column_type, column_param) for column_configs DB table
    pub fn to_column_config(&self) -> (&str, Option<String>) {
        match self {
            Self::Home => ("home", None),
            Self::Public => ("public", None),
            Self::Local => ("local", None),
            Self::Notification => ("notification", None),
            Self::List(id) => ("list", Some(id.clone())),
            Self::Hashtag(tag) => ("hashtag", Some(tag.clone())),
            Self::CustomSql(sql) => ("custom", Some(sql.clone())),
            Self::Bookmarks => ("bookmarks", None),
            Self::UserBookmarks {
                server_domain,
                account_id,
            } => {
                let column_param = serde_json::json!({
                    "serverDomain": server_domain,
                    "accountId": account_id,
                })
                .to_string();
                ("user_bookmarks", Some(column_param))
            }
            Self::Search(q) => ("search", Some(q.clone())),
            Self::YukariQuery(q) => ("yq", Some(q.clone())),
        }
    }

    /// Check if this timeline type should receive events from the given stream type
    pub fn matches_stream_type(&self, stream_type: &StreamType) -> bool {
        match (self, stream_type) {
            (TimelineType::Home, StreamType::User) => true,
            (TimelineType::Public, StreamType::Public) => true,
            (TimelineType::Local, StreamType::PublicLocal) => true,
            (TimelineType::List(a), StreamType::List(b)) => a == b,
            (TimelineType::Hashtag(a), StreamType::Hashtag(b)) => a == b,
            (TimelineType::Notification, StreamType::UserNotification) => true,
            (TimelineType::CustomSql(_), _) => true,
            (TimelineType::YukariQuery(_), _) => true,
            (TimelineType::Search(_), _) => true,
            (TimelineType::Bookmarks, _) => false,
            (TimelineType::UserBookmarks { .. }, _) => false,
            _ => false,
        }
    }

    /// Display name for the timeline type
    pub fn display_name(&self) -> String {
        match self {
            Self::Home => "Home".to_string(),
            Self::Public => "Federated".to_string(),
            Self::Local => "Local".to_string(),
            Self::List(id) => format!("List: {}", id),
            Self::Hashtag(tag) => format!("#{}", tag),
            Self::Notification => "Notification".to_string(),
            Self::CustomSql(_) => "Custom".to_string(),
            Self::Bookmarks => "Bookmarks".to_string(),
            Self::UserBookmarks { .. } => "Bookmarks".to_string(),
            Self::Search(_) => "Search".to_string(),
            Self::YukariQuery(_) => "YQ".to_string(),
        }
    }
}

fn parse_user_bookmarks_column_param(param: &str) -> Option<TimelineType> {
    let value = serde_json::from_str::<serde_json::Value>(param).ok()?;
    let server_domain = value.get("serverDomain")?.as_str()?.trim();
    let account_id = value.get("accountId")?.as_str()?.trim();
    if server_domain.is_empty() || account_id.is_empty() {
        return None;
    }
    Some(TimelineType::UserBookmarks {
        server_domain: server_domain.to_string(),
        account_id: account_id.to_string(),
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("API error: {0}")]
    Api(#[from] MastodonError),
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Fetch statuses from API and save them to the database.
/// Returns the fetched statuses.
pub async fn sync_timeline(
    client: &ApiClient,
    writer: &SqlitePool,
    _reader: &SqlitePool,
    timeline_type: &TimelineType,
    account_acct: &str,
    params: &TimelineParams,
) -> Result<Vec<Status>, SyncError> {
    let total_started_at = Instant::now();
    let server_domain = client.domain();
    let tl_key = timeline_type.as_str();
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        limit = ?params.limit,
        "[awayuki][timeline-sync] start"
    );

    // Fetch from API
    let fetch_started_at = Instant::now();
    let mut api_statuses = fetch_from_api(client, timeline_type, params).await?;
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(fetch_started_at),
        "[awayuki][timeline-sync] fetched from API"
    );
    let credentials_started_at = Instant::now();
    persist_rotated_bluesky_credentials(client, writer, account_acct).await?;
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        duration_ms = elapsed_ms(credentials_started_at),
        "[awayuki][timeline-sync] persisted rotated credentials"
    );
    let quote_started_at = Instant::now();
    resolve_pending_quotes_with_backoff(client, &mut api_statuses).await;
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(quote_started_at),
        "[awayuki][timeline-sync] resolved quote metadata"
    );

    // Save to DB
    let persist_started_at = Instant::now();
    for status in &api_statuses {
        save_status_to_db_with_retry(writer, status, server_domain).await?;

        // Insert timeline entry
        insert_timeline_entry_with_retry(
            writer,
            &tl_key,
            server_domain,
            &status.id,
            account_acct,
            &status.created_at.to_rfc3339(),
        )
        .await?;
    }
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(persist_started_at),
        "[awayuki][timeline-sync] persisted statuses"
    );

    tracing::info!(
        "Synced {} statuses for timeline '{}' ({})",
        api_statuses.len(),
        tl_key,
        account_acct
    );
    tracing::info!(
        server_domain,
        source_acct = account_acct,
        timeline = tl_key.as_str(),
        count = api_statuses.len(),
        duration_ms = elapsed_ms(total_started_at),
        "[awayuki][timeline-sync] success"
    );

    Ok(api_statuses)
}

async fn persist_rotated_bluesky_credentials(
    client: &ApiClient,
    writer: &SqlitePool,
    account_acct: &str,
) -> Result<(), SyncError> {
    if !matches!(client.kind(), ServerKind::Bluesky) {
        return Ok(());
    }

    let access_token = client.current_access_token().await;
    let app_password = client.bluesky_app_password();
    settings::update_login_credentials(
        writer,
        account_acct,
        &access_token,
        app_password.as_deref(),
    )
    .await?;
    Ok(())
}

pub async fn hydrate_missing_quotes(client: &ApiClient, statuses: &mut [Status]) {
    hydrate_missing_quotes_once(client, statuses, true).await;
}

pub async fn resolve_pending_quotes_with_backoff(client: &ApiClient, statuses: &mut [Status]) {
    hydrate_missing_quotes_once(client, statuses, true).await;

    let mut unresolved = unresolved_quote_candidate_count(client.kind(), statuses);
    if unresolved == 0 {
        return;
    }

    for (attempt, delay_ms) in QUOTE_RESOLUTION_RETRY_DELAYS_MS.iter().enumerate() {
        tracing::debug!(
            "Retrying {} pending quote resolutions on {} in {}ms (attempt {}/{})",
            unresolved,
            client.domain(),
            delay_ms,
            attempt + 1,
            QUOTE_RESOLUTION_RETRY_DELAYS_MS.len()
        );
        tokio::time::sleep(Duration::from_millis(*delay_ms)).await;

        refetch_statuses_with_pending_quotes(client, statuses).await;
        hydrate_missing_quotes_once(
            client,
            statuses,
            attempt + 1 == QUOTE_RESOLUTION_RETRY_DELAYS_MS.len(),
        )
        .await;

        unresolved = unresolved_quote_candidate_count(client.kind(), statuses);
        if unresolved == 0 {
            tracing::info!(
                "Resolved pending quote metadata on {} after {} retry attempt(s)",
                client.domain(),
                attempt + 1
            );
            return;
        }
    }

    tracing::warn!(
        "Quote metadata is still pending for {} status(es) on {} after retrying for about 60 seconds",
        unresolved,
        client.domain()
    );
}

async fn hydrate_missing_quotes_once(client: &ApiClient, statuses: &mut [Status], warn: bool) {
    for status in statuses {
        let Some(quote_id) = status.quote_id.clone() else {
            continue;
        };
        if status.quote.is_some() {
            continue;
        }

        match client.get_status(&quote_id).await {
            Ok(quote) => {
                status.quote = Some(Box::new(quote));
            }
            Err(error) => {
                if warn {
                    tracing::warn!(
                        "Failed to hydrate quoted status {} on {}: {}",
                        quote_id,
                        client.domain(),
                        error
                    );
                } else {
                    tracing::debug!(
                        "Quoted status {} is not ready on {}: {}",
                        quote_id,
                        client.domain(),
                        error
                    );
                }
            }
        }
    }
}

async fn refetch_statuses_with_pending_quotes(client: &ApiClient, statuses: &mut [Status]) {
    for status in statuses {
        if !status_may_have_pending_quote(client.kind(), status) {
            continue;
        }

        match client.get_status(&status.id).await {
            Ok(refetched) => {
                *status = refetched;
            }
            Err(error) => {
                tracing::debug!(
                    "Failed to refetch status {} while waiting for quote metadata on {}: {}",
                    status.id,
                    client.domain(),
                    error
                );
            }
        }
    }
}

fn unresolved_quote_candidate_count(kind: ServerKind, statuses: &[Status]) -> usize {
    statuses
        .iter()
        .filter(|status| status_may_have_pending_quote(kind, status))
        .count()
}

fn status_may_have_pending_quote(kind: ServerKind, status: &Status) -> bool {
    if status.quote.is_some() {
        return false;
    }
    if status.quote_id.is_some() || status.quote_original_url.is_some() {
        return true;
    }
    kind.is_mastodon_compatible()
        && is_recent_enough_for_quote_resolution(status)
        && content_contains_quote_reply_marker(&status.content)
        && content_contains_status_link(&status.content)
}

fn is_recent_enough_for_quote_resolution(status: &Status) -> bool {
    Utc::now()
        .signed_duration_since(status.created_at)
        .num_seconds()
        <= QUOTE_LINK_RETRY_RECENCY_SECS
}

fn content_contains_status_link(content: &str) -> bool {
    content.contains("href=\"")
        && (content.contains("/@") || content.contains("/users/"))
        && (content.contains("rel=\"nofollow")
            || content.contains("class=\"u-url")
            || content.contains("class=\"status-link")
            || content.contains("/statuses/"))
}

fn content_contains_quote_reply_marker(content: &str) -> bool {
    content.contains("RE:")
}

/// Fetch statuses from the appropriate API endpoint
pub async fn fetch_from_api(
    client: &ApiClient,
    timeline_type: &TimelineType,
    params: &TimelineParams,
) -> Result<Vec<Status>, MastodonError> {
    match timeline_type {
        TimelineType::Home => client.get_home_timeline(params).await,
        TimelineType::Public => client.get_public_timeline(false, params).await,
        TimelineType::Local => client.get_public_timeline(true, params).await,
        TimelineType::List(id) => client.get_list_timeline(id, params).await,
        TimelineType::Hashtag(tag) => client.get_hashtag_timeline(tag, false, params).await,
        TimelineType::Notification => {
            // Notifications use a different API and data structure.
            // Return empty for now; will be handled by a dedicated panel.
            Ok(vec![])
        }
        TimelineType::CustomSql(_)
        | TimelineType::YukariQuery(_)
        | TimelineType::Search(_)
        | TimelineType::UserBookmarks { .. } => {
            // SQLite-backed timelines query the local DB, not the API.
            Ok(vec![])
        }
        TimelineType::Bookmarks => {
            // Bookmarks are loaded from local DB after sync, not directly from API.
            Ok(vec![])
        }
    }
}

/// Save a status and its account to the database
pub async fn save_status_to_db(
    writer: &SqlitePool,
    status: &Status,
    server_domain: &str,
) -> Result<(), SyncError> {
    // Ensure server record exists (required by accounts FK)
    let streaming_url = format!("wss://{}", server_domain);
    servers::upsert_server(writer, server_domain, &streaming_url).await?;

    // Save the status author's account
    let db_account = DbAccount::from_api(&status.account, server_domain);
    accounts::upsert_account(writer, &db_account).await?;

    // If this is a reblog, save the original status and its author too
    if let Some(ref reblog) = status.reblog {
        let reblog_account = DbAccount::from_api(&reblog.account, server_domain);
        accounts::upsert_account(writer, &reblog_account).await?;

        let reblog_db = DbStatus::from_api(reblog, server_domain);
        statuses::upsert_status(writer, &reblog_db).await?;
    }

    // If this status quotes another, save the quoted status and its author too
    if let Some(ref quote) = status.quote {
        let quote_account = DbAccount::from_api(&quote.account, server_domain);
        accounts::upsert_account(writer, &quote_account).await?;

        let quote_db = DbStatus::from_api(quote, server_domain);
        statuses::upsert_status(writer, &quote_db).await?;
    }

    // Save the status itself
    let db_status = DbStatus::from_api(status, server_domain);
    statuses::upsert_status(writer, &db_status).await?;

    // Save tags to tags table for prefix search
    if !status.tags.is_empty() {
        let tag_names: Vec<String> = status.tags.iter().map(|t| t.name.clone()).collect();
        tags::upsert_tags(writer, &tag_names, server_domain).await?;
    }
    if let Some(ref reblog) = status.reblog {
        if !reblog.tags.is_empty() {
            let tag_names: Vec<String> = reblog.tags.iter().map(|t| t.name.clone()).collect();
            tags::upsert_tags(writer, &tag_names, server_domain).await?;
        }
    }

    Ok(())
}

/// Save a status, retrying transient pool exhaustion so fetched stream/API data
/// is not dropped just because all SQLite connections were briefly busy.
pub async fn save_status_to_db_with_retry(
    writer: &SqlitePool,
    status: &Status,
    server_domain: &str,
) -> Result<(), SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match save_status_to_db(writer, status, server_domain).await {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sync_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("save status", attempt, &error).await;
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("DB write retry loop must return on success or final error")
}

/// Insert a timeline entry with the same transient-DB retry behavior as status
/// persistence. The underlying query is INSERT OR IGNORE, so retries are safe.
pub async fn insert_timeline_entry_with_retry(
    writer: &SqlitePool,
    timeline_type: &str,
    server_domain: &str,
    status_id: &str,
    account_acct: &str,
    position_at: &str,
) -> Result<(), SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match timeline::insert_timeline_entry(
            writer,
            timeline_type,
            server_domain,
            status_id,
            account_acct,
            position_at,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) if should_retry_sqlx_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("insert timeline entry", attempt, &error).await;
            }
            Err(error) => return Err(SyncError::Db(error)),
        }
    }

    unreachable!("DB write retry loop must return on success or final error")
}

pub async fn delete_status_from_db_with_retry(
    writer: &SqlitePool,
    status_id: &str,
    server_domain: &str,
) -> Result<u64, SyncError> {
    for attempt in 1..=DB_WRITE_MAX_ATTEMPTS {
        match statuses::delete_status_and_references(writer, status_id, server_domain).await {
            Ok(rows) => return Ok(rows),
            Err(error) if should_retry_sqlx_error(&error) && attempt < DB_WRITE_MAX_ATTEMPTS => {
                retry_db_write_after_delay("delete status", attempt, &error).await;
            }
            Err(error) => return Err(SyncError::Db(error)),
        }
    }

    unreachable!("DB write retry loop must return on success or final error")
}

fn should_retry_sync_error(error: &SyncError) -> bool {
    match error {
        SyncError::Db(error) => should_retry_sqlx_error(error),
        SyncError::Api(_) => false,
    }
}

fn should_retry_sqlx_error(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::PoolTimedOut => true,
        sqlx::Error::Database(db_error) => {
            let message = db_error.message().to_ascii_lowercase();
            message.contains("database is locked") || message.contains("database is busy")
        }
        _ => false,
    }
}

async fn retry_db_write_after_delay(
    operation: &str,
    attempt: usize,
    error: &impl std::fmt::Display,
) {
    let delay = Duration::from_millis(DB_WRITE_RETRY_DELAYS_MS[attempt - 1]);
    tracing::warn!(
        "Transient DB error during {} (attempt {}/{}): {}; retrying in {:?}",
        operation,
        attempt,
        DB_WRITE_MAX_ATTEMPTS,
        error,
        delay
    );
    tokio::time::sleep(delay).await;
}

/// Load timeline entries from the database, returning status IDs
pub async fn load_timeline_from_db(
    reader: &SqlitePool,
    timeline_type: &TimelineType,
    account_acct: &str,
    before_position: Option<&str>,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let tl_key = timeline_type.as_str();
    let entries =
        timeline::get_timeline_entries(reader, &tl_key, account_acct, before_position, limit)
            .await?;

    Ok(entries.into_iter().map(|e| e.status_id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mastodon_status_links_as_pending_quote_candidates() {
        let content = r#"<p>RE:<br><a href="https://mastodon.example/@alice/123" rel="nofollow noopener noreferrer" target="_blank">mastodon.example/@alice/123</a></p>"#;

        assert!(content_contains_status_link(content));
        assert!(content_contains_quote_reply_marker(content));
    }

    #[test]
    fn ignores_ordinary_links_when_detecting_pending_quotes() {
        let content = r#"<p><a href="https://example.com/article" rel="nofollow noopener noreferrer" target="_blank">example.com/article</a></p>"#;

        assert!(!content_contains_status_link(content));
    }

    #[test]
    fn requires_re_marker_for_pending_quote_link_candidates() {
        let content = r#"<p><a href="https://mastodon.example/@alice/123" rel="nofollow noopener noreferrer" target="_blank">mastodon.example/@alice/123</a></p>"#;

        assert!(content_contains_status_link(content));
        assert!(!content_contains_quote_reply_marker(content));
    }
}
