use sqlx::SqlitePool;

use crate::db::models::DbAccount;

pub async fn upsert_account(pool: &SqlitePool, account: &DbAccount) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO accounts (id, server_domain, username, acct, display_name, note, avatar, avatar_static, header, locked, bot, followers_count, following_count, statuses_count, created_at, fetched_at, fields_json, emojis_json)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id, server_domain) DO UPDATE SET
           username = excluded.username,
           acct = excluded.acct,
           display_name = excluded.display_name,
           note = excluded.note,
           avatar = excluded.avatar,
           avatar_static = excluded.avatar_static,
           header = excluded.header,
           locked = excluded.locked,
           bot = excluded.bot,
           followers_count = excluded.followers_count,
           following_count = excluded.following_count,
           statuses_count = excluded.statuses_count,
           fetched_at = excluded.fetched_at,
           fields_json = excluded.fields_json,
           emojis_json = excluded.emojis_json"
    )
    .bind(&account.id)
    .bind(&account.server_domain)
    .bind(&account.username)
    .bind(&account.acct)
    .bind(&account.display_name)
    .bind(&account.note)
    .bind(&account.avatar)
    .bind(&account.avatar_static)
    .bind(&account.header)
    .bind(account.locked)
    .bind(account.bot)
    .bind(account.followers_count)
    .bind(account.following_count)
    .bind(account.statuses_count)
    .bind(&account.created_at)
    .bind(&account.fetched_at)
    .bind(&account.fields_json)
    .bind(&account.emojis_json)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_account(
    pool: &SqlitePool,
    id: &str,
    server_domain: &str,
) -> Result<Option<DbAccount>, sqlx::Error> {
    sqlx::query_as::<_, DbAccount>(
        "SELECT * FROM accounts WHERE id = ? AND server_domain = ?"
    )
    .bind(id)
    .bind(server_domain)
    .fetch_optional(pool)
    .await
}
