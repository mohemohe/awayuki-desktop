use sqlx::SqlitePool;

use crate::db::models::DbStatus;

pub async fn upsert_status(pool: &SqlitePool, status: &DbStatus) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO statuses (id, server_domain, uri, url, created_at, edited_at, account_id, content, visibility, sensitive, spoiler_text, reblogs_count, favourites_count, replies_count, in_reply_to_id, in_reply_to_account_id, reblog_of_id, language, pinned, favourited, reblogged, muted, bookmarked, poll_json, card_json, mentions_json, tags_json, emojis_json, media_attachments_json, fetched_at, quote_id, quote_original_url)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id, server_domain) DO UPDATE SET
           uri = excluded.uri,
           url = excluded.url,
           edited_at = excluded.edited_at,
           content = excluded.content,
           visibility = excluded.visibility,
           sensitive = excluded.sensitive,
           spoiler_text = excluded.spoiler_text,
           reblogs_count = excluded.reblogs_count,
           favourites_count = excluded.favourites_count,
           replies_count = excluded.replies_count,
           in_reply_to_id = excluded.in_reply_to_id,
           in_reply_to_account_id = excluded.in_reply_to_account_id,
           reblog_of_id = excluded.reblog_of_id,
           language = excluded.language,
           pinned = excluded.pinned,
           favourited = excluded.favourited,
           reblogged = excluded.reblogged,
           muted = excluded.muted,
           bookmarked = excluded.bookmarked,
           poll_json = excluded.poll_json,
           card_json = excluded.card_json,
           mentions_json = excluded.mentions_json,
           tags_json = excluded.tags_json,
           emojis_json = excluded.emojis_json,
           media_attachments_json = excluded.media_attachments_json,
           fetched_at = excluded.fetched_at,
           quote_id = excluded.quote_id,
           quote_original_url = excluded.quote_original_url"
    )
    .bind(&status.id)
    .bind(&status.server_domain)
    .bind(&status.uri)
    .bind(&status.url)
    .bind(&status.created_at)
    .bind(&status.edited_at)
    .bind(&status.account_id)
    .bind(&status.content)
    .bind(&status.visibility)
    .bind(status.sensitive)
    .bind(&status.spoiler_text)
    .bind(status.reblogs_count)
    .bind(status.favourites_count)
    .bind(status.replies_count)
    .bind(&status.in_reply_to_id)
    .bind(&status.in_reply_to_account_id)
    .bind(&status.reblog_of_id)
    .bind(&status.language)
    .bind(status.pinned)
    .bind(status.favourited)
    .bind(status.reblogged)
    .bind(status.muted)
    .bind(status.bookmarked)
    .bind(&status.poll_json)
    .bind(&status.card_json)
    .bind(&status.mentions_json)
    .bind(&status.tags_json)
    .bind(&status.emojis_json)
    .bind(&status.media_attachments_json)
    .bind(&status.fetched_at)
    .bind(&status.quote_id)
    .bind(&status.quote_original_url)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_status(
    pool: &SqlitePool,
    id: &str,
    server_domain: &str,
) -> Result<Option<DbStatus>, sqlx::Error> {
    sqlx::query_as::<_, DbStatus>("SELECT * FROM statuses WHERE id = ? AND server_domain = ?")
        .bind(id)
        .bind(server_domain)
        .fetch_optional(pool)
        .await
}

pub async fn delete_status_and_references(
    pool: &SqlitePool,
    id: &str,
    server_domain: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "DELETE FROM notifications
         WHERE server_domain = ?
           AND (
             status_id = ?
             OR status_id IN (
               SELECT id FROM statuses
               WHERE server_domain = ? AND reblog_of_id = ?
             )
           )",
    )
    .bind(server_domain)
    .bind(id)
    .bind(server_domain)
    .bind(id)
    .execute(pool)
    .await?;

    sqlx::query(
        "DELETE FROM timeline_entries
         WHERE server_domain = ?
           AND (
             status_id = ?
             OR status_id IN (
               SELECT id FROM statuses
               WHERE server_domain = ? AND reblog_of_id = ?
             )
           )",
    )
    .bind(server_domain)
    .bind(id)
    .bind(server_domain)
    .bind(id)
    .execute(pool)
    .await?;

    let result = sqlx::query(
        "DELETE FROM statuses
         WHERE server_domain = ?
           AND (id = ? OR reblog_of_id = ?)",
    )
    .bind(server_domain)
    .bind(id)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Bookmarked statuses across multiple server domains, used by the unified
/// Bookmarks panel. Empty `server_domains` returns no rows.
pub async fn get_bookmarked_statuses_by_domains(
    pool: &SqlitePool,
    server_domains: &[String],
    limit: i64,
    offset: i64,
) -> Result<Vec<DbStatus>, sqlx::Error> {
    if server_domains.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(server_domains.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT * FROM statuses WHERE bookmarked = 1 AND server_domain IN ({}) ORDER BY created_at DESC LIMIT ? OFFSET ?",
        placeholders
    );
    let mut q = sqlx::query_as::<_, DbStatus>(&sql);
    for d in server_domains {
        q = q.bind(d);
    }
    q.bind(limit).bind(offset).fetch_all(pool).await
}
