use sqlx::{SqliteConnection, SqlitePool};

/// Transaction-friendly single-tag upsert. Callers deduplicate a batch before
/// invoking this function, avoiding repeated lookups/statements in one page.
pub async fn upsert_tag_on(
    connection: &mut SqliteConnection,
    tag_name: &str,
    server_domain: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tags (name, server_domain) VALUES (?, ?)
         ON CONFLICT(name, server_domain) DO NOTHING",
    )
    .bind(tag_name)
    .bind(server_domain)
    .execute(connection)
    .await?;
    Ok(())
}

pub async fn search_tags_prefix(
    pool: &SqlitePool,
    query: &str,
    limit: u32,
) -> Result<Vec<String>, sqlx::Error> {
    let pattern = format!("{}%", query);
    let limit_i64 = limit as i64;
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT DISTINCT name FROM tags WHERE name LIKE ? ORDER BY name LIMIT ?")
            .bind(&pattern)
            .bind(limit_i64)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}
