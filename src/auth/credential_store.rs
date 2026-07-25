//! SQLite-only account credential persistence.
//!
//! Awayuki's portability contract requires the database file to be the single
//! source of persistent application state. Credentials therefore stay in the
//! historical `login_accounts` columns and are never mirrored to an operating
//! system keychain, credential manager, secret service, file, or registry.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use sqlx::SqlitePool;

use crate::bluesky::client::BlueskyCredentialSink;
use crate::db::models::DbLoginAccount;
use crate::db::queries::settings;
use crate::mastodon::error::MastodonError;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("Database error while persisting credentials: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Credential write belongs to an obsolete login generation")]
    StaleLoginGeneration,
}

/// Secret account material. Deliberately does not implement `Debug` so it
/// cannot accidentally be included in diagnostic output.
#[derive(Clone)]
pub struct AccountCredentials {
    pub access_token: String,
    pub app_password: Option<String>,
}

impl AccountCredentials {
    pub fn new(access_token: String, app_password: Option<String>) -> Self {
        Self {
            access_token,
            app_password,
        }
    }

    pub fn from_login_account(account: &DbLoginAccount) -> Self {
        Self::new(account.access_token.clone(), account.app_password.clone())
    }
}

/// Serializes login, token rotation, and logout writes while keeping all
/// durable data inside SQLite.
#[derive(Clone, Default)]
pub struct CredentialStore {
    operations: Arc<tokio::sync::Mutex<()>>,
    account_generations: Arc<StdMutex<HashMap<String, Arc<AtomicU64>>>>,
}

struct AccountCredentialSink {
    store: CredentialStore,
    pool: SqlitePool,
    acct: String,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
}

impl BlueskyCredentialSink for AccountCredentialSink {
    fn persist(
        &self,
        access_token: String,
        app_password: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<(), MastodonError>> + Send + '_>> {
        Box::pin(async move {
            self.store
                .update_for_account_generation(
                    &self.pool,
                    &self.acct,
                    &AccountCredentials::new(access_token, app_password),
                    &self.generation,
                    self.expected_generation,
                )
                .await
                .map_err(|error| {
                    MastodonError::Other(format!(
                        "Failed to persist rotated Bluesky credentials: {error}"
                    ))
                })
        })
    }
}

impl CredentialStore {
    pub fn sqlite() -> Self {
        Self::default()
    }

    pub fn bluesky_sink(&self, pool: &SqlitePool, acct: String) -> Arc<dyn BlueskyCredentialSink> {
        let generation = self.generation_for_account(&acct);
        let expected_generation = generation.load(Ordering::Acquire);
        Arc::new(AccountCredentialSink {
            store: self.clone(),
            pool: pool.clone(),
            acct,
            generation,
            expected_generation,
        })
    }

    pub async fn persist_login_account(
        &self,
        pool: &SqlitePool,
        account: &mut DbLoginAccount,
        credentials: &AccountCredentials,
    ) -> Result<(), CredentialError> {
        let _operation_guard = self.operations.lock().await;
        account.access_token.clone_from(&credentials.access_token);
        account.app_password.clone_from(&credentials.app_password);
        settings::upsert_and_activate_login_account(pool, account).await?;
        self.advance_generation(&account.acct);
        Ok(())
    }

    pub async fn update_for_account(
        &self,
        pool: &SqlitePool,
        acct: &str,
        credentials: &AccountCredentials,
    ) -> Result<(), CredentialError> {
        let _operation_guard = self.operations.lock().await;
        settings::update_login_credentials(
            pool,
            acct,
            &credentials.access_token,
            credentials.app_password.as_deref(),
        )
        .await?;
        Ok(())
    }

    async fn update_for_account_generation(
        &self,
        pool: &SqlitePool,
        acct: &str,
        credentials: &AccountCredentials,
        generation: &AtomicU64,
        expected_generation: u64,
    ) -> Result<(), CredentialError> {
        let _operation_guard = self.operations.lock().await;
        if generation.load(Ordering::Acquire) != expected_generation {
            return Err(CredentialError::StaleLoginGeneration);
        }
        settings::update_login_credentials(
            pool,
            acct,
            &credentials.access_token,
            credentials.app_password.as_deref(),
        )
        .await?;
        Ok(())
    }

    pub async fn remove_account_and_reassign(
        &self,
        pool: &SqlitePool,
        acct: &str,
    ) -> Result<Option<String>, CredentialError> {
        let _operation_guard = self.operations.lock().await;
        let active = settings::remove_login_account_and_reassign(pool, acct).await?;
        self.advance_generation(acct);
        Ok(active)
    }

    fn generation_for_account(&self, acct: &str) -> Arc<AtomicU64> {
        self.account_generations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(acct.to_string())
            .or_default()
            .clone()
    }

    fn advance_generation(&self, acct: &str) {
        self.generation_for_account(acct)
            .fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    async fn test_pool(path: Option<&PathBuf>) -> SqlitePool {
        let options = match path {
            Some(path) => SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
            None => SqliteConnectOptions::new().in_memory(true),
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS login_accounts (
                acct TEXT PRIMARY KEY,
                server_domain TEXT NOT NULL,
                account_id TEXT NOT NULL,
                display_name TEXT NOT NULL DEFAULT '',
                avatar TEXT NOT NULL DEFAULT '',
                is_active INTEGER NOT NULL DEFAULT 0,
                access_token TEXT NOT NULL DEFAULT '',
                server_kind TEXT NOT NULL DEFAULT 'mastodon',
                app_password TEXT
            );
            CREATE TABLE IF NOT EXISTS column_configs (
                id TEXT PRIMARY KEY,
                account_acct TEXT,
                column_type TEXT NOT NULL,
                position INTEGER NOT NULL
            );",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn login_account() -> DbLoginAccount {
        DbLoginAccount {
            acct: "alice@example.com".into(),
            server_domain: "example.com".into(),
            account_id: "1".into(),
            display_name: "Alice".into(),
            avatar: String::new(),
            is_active: true,
            access_token: String::new(),
            server_kind: "bluesky".into(),
            app_password: None,
        }
    }

    #[tokio::test]
    async fn login_and_rotation_write_credentials_to_sqlite_columns() {
        let pool = test_pool(None).await;
        let store = CredentialStore::sqlite();
        let mut account = login_account();
        store
            .persist_login_account(
                &pool,
                &mut account,
                &AccountCredentials::new("token-1".into(), Some("password".into())),
            )
            .await
            .unwrap();
        store
            .update_for_account(
                &pool,
                &account.acct,
                &AccountCredentials::new("token-2".into(), None),
            )
            .await
            .unwrap();

        let row: DbLoginAccount = sqlx::query_as("SELECT * FROM login_accounts WHERE acct = ?")
            .bind(&account.acct)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.access_token, "token-2");
        assert_eq!(row.app_password.as_deref(), Some("password"));
    }

    #[tokio::test]
    async fn moving_the_database_preserves_all_credentials() {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-portable-credentials-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let original = directory.join("awayuki.db");
        let moved = directory.join("moved-awayuki.db");
        let pool = test_pool(Some(&original)).await;
        let store = CredentialStore::sqlite();
        let mut account = login_account();
        store
            .persist_login_account(
                &pool,
                &mut account,
                &AccountCredentials::new("portable-token".into(), Some("portable-pass".into())),
            )
            .await
            .unwrap();
        pool.close().await;
        std::fs::rename(&original, &moved).unwrap();

        let reopened = test_pool(Some(&moved)).await;
        let row: DbLoginAccount =
            sqlx::query_as("SELECT * FROM login_accounts WHERE acct = 'alice@example.com'")
                .fetch_one(&reopened)
                .await
                .unwrap();
        assert_eq!(row.access_token, "portable-token");
        assert_eq!(row.app_password.as_deref(), Some("portable-pass"));
        reopened.close().await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn old_rotation_cannot_overwrite_credentials_after_logout_and_relogin() {
        let pool = test_pool(None).await;
        let store = CredentialStore::sqlite();
        let mut old_account = login_account();
        store
            .persist_login_account(
                &pool,
                &mut old_account,
                &AccountCredentials::new("old-session".into(), Some("old-pass".into())),
            )
            .await
            .unwrap();
        let stale_sink = store.bluesky_sink(&pool, old_account.acct.clone());

        store
            .remove_account_and_reassign(&pool, &old_account.acct)
            .await
            .unwrap();
        let mut new_account = login_account();
        store
            .persist_login_account(
                &pool,
                &mut new_account,
                &AccountCredentials::new("new-session".into(), Some("new-pass".into())),
            )
            .await
            .unwrap();

        let error = stale_sink
            .persist("rotated-old-session".into(), Some("old-pass".into()))
            .await
            .expect_err("obsolete client generation must be rejected");
        assert!(error.to_string().contains("obsolete login generation"));

        let row: DbLoginAccount = sqlx::query_as("SELECT * FROM login_accounts WHERE acct = ?")
            .bind(&new_account.acct)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.access_token, "new-session");
        assert_eq!(row.app_password.as_deref(), Some("new-pass"));
    }
}
