use sqlx::{SqliteConnection, SqlitePool};

pub async fn insert_timeline_entry(
    pool: &SqlitePool,
    timeline_type: &str,
    server_domain: &str,
    status_id: &str,
    account_acct: &str,
    position_at: &str,
) -> Result<(), sqlx::Error> {
    let mut connection = pool.acquire().await?;
    insert_timeline_entry_on(
        &mut connection,
        timeline_type,
        server_domain,
        status_id,
        account_acct,
        position_at,
    )
    .await
}

/// Transaction-friendly variant used by status page/event batches.
pub async fn insert_timeline_entry_on(
    connection: &mut SqliteConnection,
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
    .execute(connection)
    .await?;

    Ok(())
}
