use sqlx::SqlitePool;

use crate::db::models::DbTimelineEntry;

pub async fn insert_timeline_entry(
    pool: &SqlitePool,
    timeline_type: &str,
    server_domain: &str,
    status_id: &str,
    account_acct: &str,
    position_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO timeline_entries (timeline_type, server_domain, status_id, account_acct, position_at)
         VALUES (?, ?, ?, ?, ?)"
    )
    .bind(timeline_type)
    .bind(server_domain)
    .bind(status_id)
    .bind(account_acct)
    .bind(position_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Fetch timeline entries with pagination for the custom timeline query
pub async fn get_timeline_entries(
    pool: &SqlitePool,
    timeline_type: &str,
    account_acct: &str,
    before_position: Option<&str>,
    limit: i64,
) -> Result<Vec<DbTimelineEntry>, sqlx::Error> {
    if let Some(before) = before_position {
        sqlx::query_as::<_, DbTimelineEntry>(
            "SELECT * FROM timeline_entries
             WHERE timeline_type = ? AND account_acct = ? AND position_at < ?
             ORDER BY position_at DESC
             LIMIT ?",
        )
        .bind(timeline_type)
        .bind(account_acct)
        .bind(before)
        .bind(limit)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, DbTimelineEntry>(
            "SELECT * FROM timeline_entries
             WHERE timeline_type = ? AND account_acct = ?
             ORDER BY position_at DESC
             LIMIT ?",
        )
        .bind(timeline_type)
        .bind(account_acct)
        .bind(limit)
        .fetch_all(pool)
        .await
    }
}

/// Get the latest position for a timeline (used to determine since_id for sync)
pub async fn get_latest_position(
    pool: &SqlitePool,
    timeline_type: &str,
    account_acct: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT position_at FROM timeline_entries
         WHERE timeline_type = ? AND account_acct = ?
         ORDER BY position_at DESC LIMIT 1",
    )
    .bind(timeline_type)
    .bind(account_acct)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.0))
}
