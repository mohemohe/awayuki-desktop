use sqlx::{SqliteConnection, SqlitePool};

use crate::db::models::DbServer;

/// Ensure a server record exists for the given domain.
/// Uses INSERT OR IGNORE so it won't overwrite existing data.
#[cfg(test)]
pub async fn upsert_server(
    pool: &SqlitePool,
    domain: &str,
    streaming_url: &str,
) -> Result<(), sqlx::Error> {
    let mut connection = pool.acquire().await?;
    upsert_server_on(&mut connection, domain, streaming_url).await
}

/// Transaction-friendly variant used by status page/event batches.
pub async fn upsert_server_on(
    connection: &mut SqliteConnection,
    domain: &str,
    streaming_url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO servers (domain, streaming_url)
         VALUES (?, ?)",
    )
    .bind(domain)
    .bind(streaming_url)
    .execute(connection)
    .await?;

    Ok(())
}

pub async fn upsert_server_details(
    pool: &SqlitePool,
    domain: &str,
    streaming_url: &str,
    version: Option<&str>,
    max_characters: i32,
    instance_json: Option<&str>,
    server_kind: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO servers (domain, streaming_url, version, max_characters, instance_json, server_kind)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(domain) DO UPDATE SET
           streaming_url = excluded.streaming_url,
           version = excluded.version,
           max_characters = excluded.max_characters,
           instance_json = excluded.instance_json,
           server_kind = excluded.server_kind",
    )
    .bind(domain)
    .bind(streaming_url)
    .bind(version)
    .bind(max_characters)
    .bind(instance_json)
    .bind(server_kind)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_server(pool: &SqlitePool, domain: &str) -> Result<Option<DbServer>, sqlx::Error> {
    sqlx::query_as::<_, DbServer>(
        "SELECT max_characters, instance_json FROM servers WHERE domain = ?",
    )
    .bind(domain)
    .fetch_optional(pool)
    .await
}
