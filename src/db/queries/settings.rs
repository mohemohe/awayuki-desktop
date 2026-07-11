use sqlx::SqlitePool;

use crate::db::models::{DbColumnConfig, DbLoginAccount};
use crate::db::queries::read_models;

// Login accounts

pub async fn get_login_accounts(pool: &SqlitePool) -> Result<Vec<DbLoginAccount>, sqlx::Error> {
    sqlx::query_as::<_, DbLoginAccount>("SELECT * FROM login_accounts ORDER BY acct")
        .fetch_all(pool)
        .await
}

pub async fn update_login_credentials(
    pool: &SqlitePool,
    acct: &str,
    access_token: &str,
    app_password: Option<&str>,
) -> Result<(), sqlx::Error> {
    let result = sqlx::query(
        "UPDATE login_accounts
         SET access_token = ?, app_password = COALESCE(?, app_password)
         WHERE acct = ?",
    )
    .bind(access_token)
    .bind(app_password)
    .bind(acct)
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

pub async fn set_active_account(pool: &SqlitePool, acct: &str) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM login_accounts WHERE acct = ?")
        .bind(acct)
        .fetch_optional(&mut *transaction)
        .await?;
    if exists.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }
    sqlx::query("UPDATE login_accounts SET is_active = 0")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE login_accounts SET is_active = 1 WHERE acct = ?")
        .bind(acct)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn upsert_and_activate_login_account(
    pool: &SqlitePool,
    account: &DbLoginAccount,
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO login_accounts (acct, server_domain, account_id, display_name, avatar, is_active, access_token, server_kind, app_password)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?, ?)
         ON CONFLICT(acct) DO UPDATE SET
           server_domain = excluded.server_domain,
           account_id = excluded.account_id,
           display_name = excluded.display_name,
           avatar = excluded.avatar,
           access_token = excluded.access_token,
           server_kind = excluded.server_kind,
           app_password = COALESCE(excluded.app_password, login_accounts.app_password)",
    )
    .bind(&account.acct)
    .bind(&account.server_domain)
    .bind(&account.account_id)
    .bind(&account.display_name)
    .bind(&account.avatar)
    .bind(&account.access_token)
    .bind(&account.server_kind)
    .bind(&account.app_password)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE login_accounts SET is_active = (acct = ?)")
        .bind(&account.acct)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn remove_login_account_and_reassign(
    pool: &SqlitePool,
    acct: &str,
) -> Result<Option<String>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM login_accounts WHERE acct = ?")
        .bind(acct)
        .fetch_optional(&mut *transaction)
        .await?;
    if exists.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }
    let fallback: Option<(String,)> = sqlx::query_as(
        "SELECT acct FROM login_accounts WHERE acct != ? ORDER BY is_active DESC, acct LIMIT 1",
    )
    .bind(acct)
    .fetch_optional(&mut *transaction)
    .await?;

    if let Some((fallback_acct,)) = fallback.as_ref() {
        sqlx::query("UPDATE column_configs SET account_acct = ? WHERE account_acct = ?")
            .bind(fallback_acct)
            .bind(acct)
            .execute(&mut *transaction)
            .await?;
    } else {
        sqlx::query("DELETE FROM column_configs WHERE account_acct = ?")
            .bind(acct)
            .execute(&mut *transaction)
            .await?;
    }

    sqlx::query("DELETE FROM login_accounts WHERE acct = ?")
        .bind(acct)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE login_accounts SET is_active = 0")
        .execute(&mut *transaction)
        .await?;
    if let Some((fallback_acct,)) = fallback.as_ref() {
        sqlx::query("UPDATE login_accounts SET is_active = 1 WHERE acct = ?")
            .bind(fallback_acct)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(fallback.map(|(acct,)| acct))
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

pub async fn replace_all_column_configs(
    pool: &SqlitePool,
    configs: &[DbColumnConfig],
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM column_configs")
        .execute(&mut *transaction)
        .await?;
    for config in configs {
        sqlx::query(
            "INSERT INTO column_configs (id, account_acct, column_type, column_param, position, width, name, max_statuses, pane_index)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

// Database maintenance

pub async fn get_status_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    read_models::cache_counter(pool, "statuses").await
}

pub async fn get_recent_status_count(pool: &SqlitePool, since: &str) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM statuses WHERE created_at >= ?")
        .bind(since)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn get_account_count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    read_models::cache_counter(pool, "accounts").await
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
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM timeline_entries")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM notifications")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM statuses")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM accounts")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
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

pub async fn backup_and_reset_corrupt_setting(
    pool: &SqlitePool,
    key: &str,
    corrupt_value: &str,
    default_value: &str,
) -> Result<String, sqlx::Error> {
    let backup_key = format!("_corrupt_backup:{}:{}", key, uuid::Uuid::new_v4().simple());
    let mut transaction = pool.begin().await?;
    sqlx::query("INSERT INTO app_settings (key, value) VALUES (?, ?)")
        .bind(&backup_key)
        .bind(corrupt_value)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO app_settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(default_value)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(backup_key)
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
                account_acct TEXT REFERENCES login_accounts(acct),
                column_type TEXT NOT NULL,
                column_param TEXT,
                position INTEGER NOT NULL,
                width INTEGER DEFAULT 350,
                name TEXT,
                max_statuses INTEGER DEFAULT 100,
                pane_index INTEGER,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/024_enforce_single_active_account.sql"
        ))
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

    async fn insert_column_config(
        pool: &SqlitePool,
        id: &str,
        column_type: &str,
        acct: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO column_configs (id, account_acct, column_type, position)
             VALUES (?, ?, ?, 0)",
        )
        .bind(id)
        .bind(acct)
        .bind(column_type)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn logout_preserves_global_columns_and_reassigns_account_bound_columns() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_login_account(&pool, "bob@example.com", false).await;
        insert_column_config(&pool, "home", "home", None).await;
        insert_column_config(&pool, "local", "local", Some("alice@example.com")).await;

        let fallback = remove_login_account_and_reassign(&pool, "alice@example.com")
            .await
            .unwrap()
            .unwrap();

        let global_scope: (Option<String>,) =
            sqlx::query_as("SELECT account_acct FROM column_configs WHERE id = 'home'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(global_scope.0, None);
        let local_scope: (String,) =
            sqlx::query_as("SELECT account_acct FROM column_configs WHERE id = 'local'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(local_scope.0, "bob@example.com");
        assert_eq!(fallback, "bob@example.com");
        let active: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM login_accounts WHERE acct = 'bob@example.com' AND is_active = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active.0, 1);
    }

    #[tokio::test]
    async fn final_logout_removes_only_account_bound_columns() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_column_config(&pool, "home", "home", None).await;
        insert_column_config(&pool, "hashtag", "hashtag", Some("alice@example.com")).await;

        assert!(
            remove_login_account_and_reassign(&pool, "alice@example.com")
                .await
                .unwrap()
                .is_none()
        );

        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, account_acct FROM column_configs ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, vec![("home".to_string(), None)]);
    }

    #[tokio::test]
    async fn column_replacement_rolls_back_when_a_later_insert_fails() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_column_config(&pool, "original", "local", Some("alice@example.com")).await;
        sqlx::query(
            "CREATE TRIGGER reject_fault_column
             BEFORE INSERT ON column_configs
             WHEN NEW.id = 'fault'
             BEGIN
                 SELECT RAISE(ABORT, 'injected column failure');
             END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let configs = [
            test_column_config("replacement", 0),
            test_column_config("fault", 1),
        ];
        replace_all_column_configs(&pool, &configs)
            .await
            .expect_err("the injected second insert must abort the transaction");

        let ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM column_configs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(ids, vec![("original".to_string(),)]);
    }

    #[tokio::test]
    async fn missing_active_account_does_not_clear_existing_active_account() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_login_account(&pool, "bob@example.com", false).await;

        let error = set_active_account(&pool, "missing@example.com")
            .await
            .expect_err("missing account must fail");
        assert!(matches!(error, sqlx::Error::RowNotFound));
        let active: (String,) =
            sqlx::query_as("SELECT acct FROM login_accounts WHERE is_active = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(active.0, "alice@example.com");
    }

    #[tokio::test]
    async fn active_account_switch_rolls_back_if_activation_fails() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;
        insert_login_account(&pool, "bob@example.com", false).await;
        sqlx::query(
            "CREATE TRIGGER reject_bob_activation
             BEFORE UPDATE OF is_active ON login_accounts
             WHEN NEW.acct = 'bob@example.com' AND NEW.is_active = 1
             BEGIN
                 SELECT RAISE(ABORT, 'injected activation failure');
             END",
        )
        .execute(&pool)
        .await
        .unwrap();

        set_active_account(&pool, "bob@example.com")
            .await
            .expect_err("the injected activation failure must abort the transaction");

        let active: Vec<(String,)> =
            sqlx::query_as("SELECT acct FROM login_accounts WHERE is_active = 1 ORDER BY acct")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(active, vec![("alice@example.com".to_string(),)]);
    }

    #[tokio::test]
    async fn sqlite_rejects_more_than_one_active_account() {
        let pool = test_pool().await;
        insert_login_account(&pool, "alice@example.com", true).await;

        let error = sqlx::query(
            "INSERT INTO login_accounts (acct, server_domain, account_id, is_active)
             VALUES ('bob@example.com', 'example.com', 'bob-id', 1)",
        )
        .execute(&pool)
        .await
        .expect_err("the partial unique index must reject a second active account");

        assert!(error.to_string().contains("UNIQUE constraint failed"));
        let active_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM login_accounts WHERE is_active = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(active_count.0, 1);
    }

    #[tokio::test]
    async fn single_active_migration_repairs_historical_duplicates_deterministically() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE login_accounts (
                 acct TEXT PRIMARY KEY,
                 server_domain TEXT NOT NULL,
                 account_id TEXT NOT NULL,
                 is_active INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO login_accounts VALUES
                 ('zeta@example.com', 'example.com', 'zeta', 1),
                 ('alpha@example.com', 'example.com', 'alpha', 1);",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!(
            "../../../migrations/024_enforce_single_active_account.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let active: Vec<(String,)> =
            sqlx::query_as("SELECT acct FROM login_accounts WHERE is_active = 1 ORDER BY acct")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(active, vec![("alpha@example.com".to_string(),)]);
    }

    #[tokio::test]
    async fn corrupt_setting_is_backed_up_and_reset_atomically() {
        let pool = test_pool().await;
        set_setting(&pool, "appearance", "{broken")
            .await
            .expect("seed corrupt setting");

        let backup_key = backup_and_reset_corrupt_setting(
            &pool,
            "appearance",
            "{broken",
            r#"{"avatar_shape":"Circle"}"#,
        )
        .await
        .expect("repair setting");

        assert!(backup_key.starts_with("_corrupt_backup:appearance:"));
        assert_eq!(
            get_setting(&pool, &backup_key).await.unwrap().as_deref(),
            Some("{broken")
        );
        assert_eq!(
            get_setting(&pool, "appearance").await.unwrap().as_deref(),
            Some(r#"{"avatar_shape":"Circle"}"#)
        );
    }

    fn test_column_config(id: &str, position: i32) -> DbColumnConfig {
        DbColumnConfig {
            id: id.to_string(),
            account_acct: None,
            column_type: "home".to_string(),
            column_param: None,
            position,
            width: Some(350),
            name: None,
            max_statuses: Some(100),
            pane_index: Some(0),
        }
    }
}
