use sqlx::SqlitePool;

use crate::db::models::DbStatus;

pub async fn upsert_status(pool: &SqlitePool, status: &DbStatus) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO statuses (id, server_domain, uri, url, created_at, edited_at, account_id, content, visibility, sensitive, spoiler_text, reblogs_count, favourites_count, replies_count, in_reply_to_id, in_reply_to_account_id, reblog_of_id, language, pinned, favourited, reblogged, muted, bookmarked, poll_json, card_json, mentions_json, tags_json, emojis_json, media_attachments_json, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
           fetched_at = excluded.fetched_at"
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
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_status(
    pool: &SqlitePool,
    id: &str,
    server_domain: &str,
) -> Result<Option<DbStatus>, sqlx::Error> {
    sqlx::query_as::<_, DbStatus>(
        "SELECT * FROM statuses WHERE id = ? AND server_domain = ?"
    )
    .bind(id)
    .bind(server_domain)
    .fetch_optional(pool)
    .await
}

pub async fn get_bookmarked_statuses(
    pool: &SqlitePool,
    server_domain: &str,
    limit: i64,
) -> Result<Vec<DbStatus>, sqlx::Error> {
    sqlx::query_as::<_, DbStatus>(
        "SELECT * FROM statuses WHERE server_domain = ? AND bookmarked = 1 ORDER BY created_at DESC LIMIT ?"
    )
    .bind(server_domain)
    .bind(limit)
    .fetch_all(pool)
    .await
}
