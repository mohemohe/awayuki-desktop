//! Read-only status-reference queries for cached timeline views.

use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow, PartialEq, Eq)]
pub struct TimelineStatusRef {
    pub server_domain: String,
    pub status_id: String,
    pub source_acct: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatusDisplayFilter {
    pub exclude_boosts: bool,
    pub exclude_media: bool,
    pub include_media: bool,
}

pub async fn query_account_timeline_status_refs(
    pool: &SqlitePool,
    timeline_type: &str,
    account_acct: &str,
    limit: i64,
    offset: i64,
    filter: StatusDisplayFilter,
) -> Result<Vec<TimelineStatusRef>, sqlx::Error> {
    let filter_sql = display_filter_sql("s", filter);
    let sql = format!(
        "SELECT te.server_domain, te.status_id, te.account_acct AS source_acct FROM timeline_entries te
         JOIN statuses s ON s.id = te.status_id AND s.server_domain = te.server_domain
         WHERE te.timeline_type = ? AND te.account_acct = ?
         {filter_sql}
         ORDER BY te.position_at DESC, te.status_id DESC
         LIMIT ? OFFSET ?"
    );
    sqlx::query_as::<_, TimelineStatusRef>(&sql)
        .bind(timeline_type)
        .bind(account_acct)
        .bind(limit.max(0))
        .bind(offset.max(0))
        .fetch_all(pool)
        .await
}

pub async fn query_bookmarked_status_refs(
    pool: &SqlitePool,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, sqlx::Error> {
    query_viewer_state_refs(
        pool,
        "v.bookmarked = 1",
        account_acct,
        None,
        None,
        limit,
        offset,
    )
    .await
}

pub async fn query_favourited_status_refs(
    pool: &SqlitePool,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, sqlx::Error> {
    query_viewer_state_refs(
        pool,
        "v.favourited = 1 AND s.reblog_of_id IS NULL",
        account_acct,
        None,
        None,
        limit,
        offset,
    )
    .await
}

pub async fn query_user_bookmarked_status_refs(
    pool: &SqlitePool,
    server_domain: &str,
    account_id: &str,
    account_acct: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, sqlx::Error> {
    query_viewer_state_refs(
        pool,
        "v.bookmarked = 1",
        account_acct,
        Some(server_domain),
        Some(account_id),
        limit,
        offset,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn query_viewer_state_refs(
    pool: &SqlitePool,
    viewer_predicate: &str,
    account_acct: Option<&str>,
    server_domain: Option<&str>,
    account_id: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TimelineStatusRef>, sqlx::Error> {
    let sql = format!(
        "SELECT server_domain, status_id, source_acct FROM (
           SELECT
             v.server_domain,
             v.status_id,
             v.login_account_acct AS source_acct,
             v.updated_at,
             ROW_NUMBER() OVER (
               PARTITION BY COALESCE(NULLIF(s.uri, ''), v.server_domain || ':' || v.status_id)
               ORDER BY v.updated_at DESC, v.login_account_acct DESC
             ) AS identity_rank
           FROM status_viewer_state v
           JOIN statuses s ON s.id = v.status_id AND s.server_domain = v.server_domain
           WHERE {viewer_predicate}
             AND (? IS NULL OR v.login_account_acct = ?)
             AND (? IS NULL OR s.server_domain = ?)
             AND (? IS NULL OR s.account_id = ?)
         ) ranked
         WHERE identity_rank = 1
         ORDER BY updated_at DESC, server_domain DESC, status_id DESC
         LIMIT ? OFFSET ?"
    );
    sqlx::query_as::<_, TimelineStatusRef>(&sql)
        .bind(account_acct)
        .bind(account_acct)
        .bind(server_domain)
        .bind(server_domain)
        .bind(account_id)
        .bind(account_id)
        .bind(limit.max(0))
        .bind(offset.max(0))
        .fetch_all(pool)
        .await
}

pub(crate) fn display_filter_sql(alias: &str, filter: StatusDisplayFilter) -> String {
    let media_sql = format!(
        "(({alias}.media_attachments_json IS NOT NULL AND {alias}.media_attachments_json != '[]')
          OR EXISTS (
            SELECT 1 FROM statuses original
            WHERE original.id = {alias}.reblog_of_id
              AND original.server_domain = {alias}.server_domain
              AND original.media_attachments_json IS NOT NULL
              AND original.media_attachments_json != '[]'
          ))"
    );
    let mut sql = String::new();
    if filter.exclude_boosts {
        sql.push_str(&format!(" AND {alias}.reblog_of_id IS NULL"));
    }
    if filter.exclude_media {
        sql.push_str(" AND NOT ");
        sql.push_str(&media_sql);
    }
    if filter.include_media {
        sql.push_str(" AND ");
        sql.push_str(&media_sql);
    }
    sql
}
