use sqlx::SqlitePool;

use crate::db::models::{DbAccount, DbStatus};
use crate::db::queries::{accounts, servers, statuses, tags, timeline};
use crate::api::client::ApiClient;
use crate::mastodon::endpoints::timelines::TimelineParams;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::status::Status;
use crate::mastodon::types::streaming::StreamType;

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
            "list" => column_param.map(|id| Self::List(id.to_string())),
            "hashtag" => column_param.map(|tag| Self::Hashtag(tag.to_string())),
            "custom" => column_param.map(|sql| Self::CustomSql(sql.to_string())),
            "yq" => column_param.map(|q| Self::YukariQuery(q.to_string())),
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
            (TimelineType::Bookmarks, _) => false,
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
            Self::YukariQuery(_) => "YQ".to_string(),
        }
    }
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
    let server_domain = client.domain();
    let tl_key = timeline_type.as_str();

    // Fetch from API
    let api_statuses = fetch_from_api(client, timeline_type, params).await?;

    // Save to DB
    for status in &api_statuses {
        save_status_to_db(writer, status, server_domain).await?;

        // Insert timeline entry
        timeline::insert_timeline_entry(
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
        "Synced {} statuses for timeline '{}' ({})",
        api_statuses.len(),
        tl_key,
        account_acct
    );

    Ok(api_statuses)
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
        TimelineType::CustomSql(_) | TimelineType::YukariQuery(_) => {
            // Custom SQL / YQ timelines query the local DB, not the API.
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

/// Load timeline entries from the database, returning status IDs
pub async fn load_timeline_from_db(
    reader: &SqlitePool,
    timeline_type: &TimelineType,
    account_acct: &str,
    before_position: Option<&str>,
    limit: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let tl_key = timeline_type.as_str();
    let entries = timeline::get_timeline_entries(
        reader,
        &tl_key,
        account_acct,
        before_position,
        limit,
    )
    .await?;

    Ok(entries.into_iter().map(|e| e.status_id).collect())
}
