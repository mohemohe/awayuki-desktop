use sqlx::SqlitePool;

use crate::db::models::{DbColumnConfig, DbLoginAccount};

// Login accounts

pub async fn upsert_login_account(
    pool: &SqlitePool,
    account: &DbLoginAccount,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO login_accounts (acct, server_domain, account_id, display_name, avatar, is_active, access_token)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(acct) DO UPDATE SET
           server_domain = excluded.server_domain,
           account_id = excluded.account_id,
           display_name = excluded.display_name,
           avatar = excluded.avatar,
           is_active = excluded.is_active,
           access_token = excluded.access_token"
    )
    .bind(&account.acct)
    .bind(&account.server_domain)
    .bind(&account.account_id)
    .bind(&account.display_name)
    .bind(&account.avatar)
    .bind(account.is_active)
    .bind(&account.access_token)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_login_accounts(pool: &SqlitePool) -> Result<Vec<DbLoginAccount>, sqlx::Error> {
    sqlx::query_as::<_, DbLoginAccount>("SELECT * FROM login_accounts ORDER BY acct")
        .fetch_all(pool)
        .await
}

pub async fn get_active_login_account(
    pool: &SqlitePool,
) -> Result<Option<DbLoginAccount>, sqlx::Error> {
    sqlx::query_as::<_, DbLoginAccount>("SELECT * FROM login_accounts WHERE is_active = 1 LIMIT 1")
        .fetch_optional(pool)
        .await
}

pub async fn delete_login_account(pool: &SqlitePool, acct: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM login_accounts WHERE acct = ?")
        .bind(acct)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_active_account(pool: &SqlitePool, acct: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE login_accounts SET is_active = 0")
        .execute(pool)
        .await?;
    sqlx::query("UPDATE login_accounts SET is_active = 1 WHERE acct = ?")
        .bind(acct)
        .execute(pool)
        .await?;
    Ok(())
}

// Column configs

pub async fn get_column_configs(
    pool: &SqlitePool,
    account_acct: &str,
) -> Result<Vec<DbColumnConfig>, sqlx::Error> {
    sqlx::query_as::<_, DbColumnConfig>(
        "SELECT * FROM column_configs WHERE account_acct = ? ORDER BY pane_index, position"
    )
    .bind(account_acct)
    .fetch_all(pool)
    .await
}

pub async fn upsert_column_config(
    pool: &SqlitePool,
    config: &DbColumnConfig,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO column_configs (id, account_acct, column_type, column_param, position, width, name, max_statuses, pane_index)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
           column_type = excluded.column_type,
           column_param = excluded.column_param,
           position = excluded.position,
           width = excluded.width,
           name = excluded.name,
           max_statuses = excluded.max_statuses,
           pane_index = excluded.pane_index"
    )
    .bind(&config.id)
    .bind(&config.account_acct)
    .bind(&config.column_type)
    .bind(&config.column_param)
    .bind(config.position)
    .bind(config.width)
    .bind(&config.name)
    .bind(config.max_statuses)
    .bind(config.pane_index)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_column_config(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM column_configs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all_column_configs(
    pool: &SqlitePool,
    account_acct: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM column_configs WHERE account_acct = ?")
        .bind(account_acct)
        .execute(pool)
        .await?;
    Ok(())
}

// Database maintenance

pub async fn get_status_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM statuses")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn get_recent_status_count(
    pool: &SqlitePool,
    since: &str,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM statuses WHERE created_at >= ?")
        .bind(since)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn get_account_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn get_db_size(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let page_count: (i64,) = sqlx::query_as("SELECT page_count FROM pragma_page_count")
        .fetch_one(pool)
        .await?;
    let page_size: (i64,) = sqlx::query_as("SELECT page_size FROM pragma_page_size")
        .fetch_one(pool)
        .await?;
    Ok(page_count.0 * page_size.0)
}

pub async fn vacuum(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("VACUUM").execute(pool).await?;
    Ok(())
}

pub async fn clear_status_cache(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM timeline_entries").execute(pool).await?;
    sqlx::query("DELETE FROM notifications").execute(pool).await?;
    sqlx::query("DELETE FROM statuses").execute(pool).await?;
    sqlx::query("DELETE FROM accounts").execute(pool).await?;
    Ok(())
}

// App settings (KV store)

pub async fn get_setting(
    pool: &SqlitePool,
    key: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT value FROM app_settings WHERE key = ?"
    )
    .bind(key)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.0))
}

pub async fn set_setting(
    pool: &SqlitePool,
    key: &str,
    value: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app_settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value"
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;

    Ok(())
}
