use sqlx::SqlitePool;

use crate::db::models::{DbColumnConfig, DbLoginAccount};

// Login accounts

pub async fn upsert_login_account(
    pool: &SqlitePool,
    account: &DbLoginAccount,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO login_accounts (acct, server_domain, account_id, display_name, avatar, is_active, access_token, server_kind, app_password)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(acct) DO UPDATE SET
           server_domain = excluded.server_domain,
           account_id = excluded.account_id,
           display_name = excluded.display_name,
           avatar = excluded.avatar,
           is_active = excluded.is_active,
           access_token = excluded.access_token,
           server_kind = excluded.server_kind,
           app_password = COALESCE(excluded.app_password, login_accounts.app_password)"
    )
    .bind(&account.acct)
    .bind(&account.server_domain)
    .bind(&account.account_id)
    .bind(&account.display_name)
    .bind(&account.avatar)
    .bind(account.is_active)
    .bind(&account.access_token)
    .bind(&account.server_kind)
    .bind(&account.app_password)
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

pub async fn update_login_credentials(
    pool: &SqlitePool,
    acct: &str,
    access_token: &str,
    app_password: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE login_accounts
         SET access_token = ?,
             app_password = COALESCE(?, app_password)
         WHERE acct = ?",
    )
    .bind(access_token)
    .bind(app_password)
    .bind(acct)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_login_account(pool: &SqlitePool, acct: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM login_accounts WHERE acct = ?")
        .bind(acct)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_fallback_login_account_acct(
    pool: &SqlitePool,
    excluded_acct: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT acct FROM login_accounts WHERE acct != ? ORDER BY is_active DESC, acct LIMIT 1",
    )
    .bind(excluded_acct)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row.0))
}

pub async fn update_column_config_account_acct(
    pool: &SqlitePool,
    from_acct: &str,
    to_acct: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE column_configs SET account_acct = ? WHERE account_acct = ?")
        .bind(to_acct)
        .bind(from_acct)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_column_configs_for_account(
    pool: &SqlitePool,
    acct: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM column_configs WHERE account_acct = ?")
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

/// Load every column config row regardless of `account_acct`.
///
/// Unified-timeline mode treats columns as application-level rather than
/// per-account, so loads must not filter by the currently active acct
/// (otherwise switching active accounts hides another account's saved layout).
pub async fn get_all_column_configs(pool: &SqlitePool) -> Result<Vec<DbColumnConfig>, sqlx::Error> {
    sqlx::query_as::<_, DbColumnConfig>(
        "SELECT * FROM column_configs ORDER BY pane_index, position",
    )
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

/// Wipe column configs across every login account.
///
/// Used as the pre-step of the unified save flow so that rows previously
/// saved under a now-inactive `account_acct` do not linger and resurface
/// at next launch.
pub async fn delete_all_column_configs_global(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM column_configs")
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

pub async fn get_recent_status_count(pool: &SqlitePool, since: &str) -> Result<i64, sqlx::Error> {
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
    sqlx::query("DELETE FROM timeline_entries")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM notifications")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM statuses").execute(pool).await?;
    sqlx::query("DELETE FROM accounts").execute(pool).await?;
    Ok(())
}

// App settings (KV store)

pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM app_settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| r.0))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO app_settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .pragma("foreign_keys", "ON");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"
            CREATE TABLE login_accounts (
                acct TEXT PRIMARY KEY,
                server_domain TEXT NOT NULL,
                account_id TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                avatar TEXT NOT NULL DEFAULT '',
                is_active INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE column_configs (
                id TEXT PRIMARY KEY,
                account_acct TEXT NOT NULL REFERENCES login_accounts(acct),
                column_type TEXT NOT NULL,
                column_param TEXT,
                position INTEGER NOT NULL,
                width INTEGER DEFAULT 350,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_login_account(pool: &SqlitePool, acct: &str, active: bool) {
        sqlx::query(
            "INSERT INTO login_accounts (acct, server_domain, account_id, is_active)
             VALUES (?, 'example.com', ?, ?)",
        )
        .bind(acct)
        .bind(format!("{acct}-id"))
        .bind(active)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_column_config(pool: &SqlitePool, id: &str, acct: &str) {
        sqlx::query(
            "INSERT INTO column_configs (id, account_acct, column_type, position)
             VALUES (?, ?, 'home', 0)",
        )
        .bind(id)
        .bind(acct)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn login_account_can_be_deleted_after_column_configs_are_reassigned() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_login_account(&pool, "bob@example.com", false).await;
        insert_column_config(&pool, "home", "alice@example.com").await;

        let fallback = get_fallback_login_account_acct(&pool, "alice@example.com")
            .await
            .unwrap()
            .unwrap();
        update_column_config_account_acct(&pool, "alice@example.com", &fallback)
            .await
            .unwrap();
        delete_login_account(&pool, "alice@example.com")
            .await
            .unwrap();

        let row: (String,) =
            sqlx::query_as("SELECT account_acct FROM column_configs WHERE id = 'home'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "bob@example.com");
    }

    #[tokio::test]
    async fn final_login_account_can_be_deleted_after_column_configs_are_removed() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_column_config(&pool, "home", "alice@example.com").await;

        assert!(get_fallback_login_account_acct(&pool, "alice@example.com")
            .await
            .unwrap()
            .is_none());
        delete_column_configs_for_account(&pool, "alice@example.com")
            .await
            .unwrap();
        delete_login_account(&pool, "alice@example.com")
            .await
            .unwrap();

        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM column_configs")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, 0);
    }
}
