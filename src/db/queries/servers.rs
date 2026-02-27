use sqlx::SqlitePool;

/// Ensure a server record exists for the given domain.
/// Uses INSERT OR IGNORE so it won't overwrite existing data.
pub async fn upsert_server(
    pool: &SqlitePool,
    domain: &str,
    streaming_url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO servers (domain, streaming_url)
         VALUES (?, ?)",
    )
    .bind(domain)
    .bind(streaming_url)
    .execute(pool)
    .await?;

    Ok(())
}
