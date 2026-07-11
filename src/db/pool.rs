use sqlx::migrate::{MigrateError, Migration, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::state::storage_security;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const MIGRATION_MODE_KEY: &str = "migration_mode";
const MANAGED_MODE: &str = "managed-v1";
const LEGACY_BOOTSTRAP_MODE: &str = "legacy-bootstrap";

pub struct Database {
    writer: SqlitePool,
    reader: SqlitePool,
    analytics_reader: SqlitePool,
    path: PathBuf,
}

#[derive(Debug, Default)]
#[must_use]
pub struct MigrationReport {
    pub applied_versions: Vec<i64>,
    pub repaired_legacy_schema: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrator(#[from] MigrateError),
    #[error("migration {version} checksum does not match the bundled migration")]
    ChecksumMismatch { version: i64 },
    #[cfg(test)]
    #[error("database foreign key check failed after migration: {details}")]
    ForeignKeyCheck { details: String },
    #[error("failed to protect database storage at {path}: {source}")]
    StorageIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationMode {
    New,
    Legacy,
    LegacyResume,
    Managed,
}

impl Database {
    pub async fn new(db_path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let path = db_path.as_ref().to_path_buf();
        storage_security::create_private_file_if_missing(&path).map_err(sqlx::Error::Io)?;

        let write_opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .pragma("foreign_keys", "ON")
            .pragma("auto_vacuum", "INCREMENTAL")
            .pragma("synchronous", "NORMAL");

        let read_opts = SqliteConnectOptions::new()
            .filename(&path)
            .read_only(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .pragma("foreign_keys", "ON");

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(write_opts)
            .await?;

        storage_security::harden_sqlite_files(&path).map_err(sqlx::Error::Io)?;

        let reader_count = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4)
            .clamp(2, 4);

        let reader = SqlitePoolOptions::new()
            .max_connections(reader_count)
            .connect_with(read_opts.clone())
            .await?;

        // User-authored SQL and YQ timelines can legitimately scan a large
        // portable cache. Keep those reads on their own WAL reader pool so a
        // slow analytical column cannot consume every connection needed by
        // the ordinary home/list/notification timelines.
        let analytics_reader = SqlitePoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(180))
            .connect_with(read_opts)
            .await?;

        Ok(Self {
            writer,
            reader,
            analytics_reader,
            path,
        })
    }

    pub fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    pub fn reader(&self) -> &SqlitePool {
        &self.reader
    }

    pub fn analytics_reader(&self) -> &SqlitePool {
        &self.analytics_reader
    }

    /// Upgrade the database with SQLx-compatible version/checksum history.
    ///
    /// Databases created before migration history existed are repaired with a
    /// schema-aware compatibility pass. Each compatibility migration and its
    /// history row commit in the same transaction, so an interrupted bootstrap
    /// safely resumes. Once baselined, the regular SQLx migrator owns all
    /// subsequent upgrades and checksum validation.
    pub async fn run_migrations(&self) -> Result<MigrationReport, MigrationError> {
        let mode = self.migration_mode().await?;
        let applied_before = self.applied_versions().await?;
        let mut report = MigrationReport::default();

        match mode {
            MigrationMode::Legacy | MigrationMode::LegacyResume => {
                self.bootstrap_legacy_schema(&mut report).await?;
                report.repaired_legacy_schema = true;
                // Validate the compatibility baseline against exactly the same
                // checksums future startups will use.
                MIGRATOR.run(self.writer()).await?;
            }
            MigrationMode::New | MigrationMode::Managed => {
                MIGRATOR.run(self.writer()).await?;
                ensure_metadata_table(self.writer()).await?;
                set_migration_mode(self.writer(), MANAGED_MODE).await?;

                let applied_after = self.applied_versions().await?;
                report.applied_versions =
                    applied_after.difference(&applied_before).copied().collect();
                report.applied_versions.sort_unstable();
            }
        }

        // A full `PRAGMA quick_check` or `foreign_key_check` scans the entire
        // cache. Real user databases can exceed several gigabytes, so doing
        // either on the startup path makes a healthy database look hung.
        // Foreign keys are enabled on every connection and migrations are
        // transactional. Exhaustive checks stay in migration tests and the
        // explicit diagnostic path instead of blocking the first window.
        self.verify_runtime_schema().await?;
        #[cfg(test)]
        self.verify_foreign_keys().await?;
        let schema_version = self
            .applied_versions()
            .await?
            .into_iter()
            .max()
            .unwrap_or_default();
        tracing::info!(schema_version, "SQLite schema version verified");

        storage_security::harden_sqlite_files(&self.path).map_err(|source| {
            MigrationError::StorageIo {
                path: self.path.clone(),
                source,
            }
        })?;

        Ok(report)
    }

    async fn verify_runtime_schema(&self) -> Result<(), MigrationError> {
        let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
            .fetch_one(self.writer())
            .await?;
        if foreign_keys != 1 {
            return Err(sqlx::Error::Protocol(
                "SQLite foreign key enforcement is disabled".to_string(),
            )
            .into());
        }
        Ok(())
    }

    #[cfg(test)]
    async fn verify_foreign_keys(&self) -> Result<(), MigrationError> {
        let rows = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(self.writer())
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let details = rows
            .into_iter()
            .take(5)
            .map(|row| {
                let table = row.try_get::<String, _>(0).unwrap_or_default();
                let rowid = row.try_get::<i64, _>(1).unwrap_or_default();
                let parent = row.try_get::<String, _>(2).unwrap_or_default();
                format!("{table} row {rowid} references {parent}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        Err(MigrationError::ForeignKeyCheck { details })
    }

    async fn migration_mode(&self) -> Result<MigrationMode, sqlx::Error> {
        let has_metadata = has_table(self.writer(), "_awayuki_schema_metadata").await?;
        if has_metadata {
            let value = sqlx::query_scalar::<_, String>(
                "SELECT value FROM _awayuki_schema_metadata WHERE key = ?",
            )
            .bind(MIGRATION_MODE_KEY)
            .fetch_optional(self.writer())
            .await?;

            if value.as_deref() == Some(LEGACY_BOOTSTRAP_MODE) {
                return Ok(MigrationMode::LegacyResume);
            }
            if value.as_deref() == Some(MANAGED_MODE) {
                return Ok(MigrationMode::Managed);
            }
        }

        if has_table(self.writer(), "_sqlx_migrations").await? {
            return Ok(MigrationMode::Managed);
        }
        if has_table(self.writer(), "servers").await? {
            return Ok(MigrationMode::Legacy);
        }
        Ok(MigrationMode::New)
    }

    async fn applied_versions(&self) -> Result<HashSet<i64>, sqlx::Error> {
        if !has_table(self.writer(), "_sqlx_migrations").await? {
            return Ok(HashSet::new());
        }

        let versions = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM _sqlx_migrations WHERE success = TRUE",
        )
        .fetch_all(self.writer())
        .await?;
        Ok(versions.into_iter().collect())
    }

    async fn bootstrap_legacy_schema(
        &self,
        report: &mut MigrationReport,
    ) -> Result<(), MigrationError> {
        ensure_metadata_table(self.writer()).await?;
        ensure_sqlx_migrations_table(self.writer()).await?;
        set_migration_mode(self.writer(), LEGACY_BOOTSTRAP_MODE).await?;

        for migration in MIGRATOR
            .iter()
            .filter(|migration| !migration.migration_type.is_down_migration())
        {
            let stored_checksum = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT checksum FROM _sqlx_migrations WHERE version = ?",
            )
            .bind(migration.version)
            .fetch_optional(self.writer())
            .await?;

            if let Some(stored_checksum) = stored_checksum {
                if stored_checksum.as_slice() != migration.checksum.as_ref() {
                    return Err(MigrationError::ChecksumMismatch {
                        version: migration.version,
                    });
                }
                continue;
            }

            let mut transaction = self.writer().begin().await?;
            apply_legacy_migration(&mut transaction, migration).await?;
            sqlx::query(
                "INSERT INTO _sqlx_migrations
                 (version, description, success, checksum, execution_time)
                 VALUES (?, ?, TRUE, ?, -1)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            report.applied_versions.push(migration.version);
        }

        set_migration_mode(self.writer(), MANAGED_MODE).await?;
        Ok(())
    }

    #[cfg(test)]
    pub async fn close(&self) {
        self.writer.close().await;
        self.reader.close().await;
        self.analytics_reader.close().await;
    }
}

async fn apply_legacy_migration(
    transaction: &mut Transaction<'_, Sqlite>,
    migration: &Migration,
) -> Result<(), sqlx::Error> {
    match migration.version {
        7 => {
            ensure_column(
                transaction,
                "column_configs",
                "name",
                "ALTER TABLE column_configs ADD COLUMN name TEXT",
            )
            .await?;
        }
        8 => {
            ensure_column(
                transaction,
                "login_accounts",
                "access_token",
                "ALTER TABLE login_accounts ADD COLUMN access_token TEXT NOT NULL DEFAULT ''",
            )
            .await?;
        }
        9 => {
            ensure_column(
                transaction,
                "column_configs",
                "max_statuses",
                "ALTER TABLE column_configs ADD COLUMN max_statuses INTEGER DEFAULT 100",
            )
            .await?;
        }
        10 => {
            ensure_column(
                transaction,
                "column_configs",
                "pane_index",
                "ALTER TABLE column_configs ADD COLUMN pane_index INTEGER",
            )
            .await?;
            sqlx::query("UPDATE column_configs SET pane_index = position WHERE pane_index IS NULL")
                .execute(&mut **transaction)
                .await?;
        }
        12 => {
            ensure_column(
                transaction,
                "statuses",
                "quote_id",
                "ALTER TABLE statuses ADD COLUMN quote_id TEXT",
            )
            .await?;
            ensure_column(
                transaction,
                "statuses",
                "quote_original_url",
                "ALTER TABLE statuses ADD COLUMN quote_original_url TEXT",
            )
            .await?;
        }
        13 => {
            ensure_column(
                transaction,
                "login_accounts",
                "server_kind",
                "ALTER TABLE login_accounts ADD COLUMN server_kind TEXT NOT NULL DEFAULT 'mastodon'",
            )
            .await?;
        }
        14 => {
            ensure_column(
                transaction,
                "servers",
                "server_kind",
                "ALTER TABLE servers ADD COLUMN server_kind TEXT NOT NULL DEFAULT 'mastodon'",
            )
            .await?;
        }
        15 => {
            ensure_column(
                transaction,
                "login_accounts",
                "app_password",
                "ALTER TABLE login_accounts ADD COLUMN app_password TEXT",
            )
            .await?;
        }
        17 => {
            ensure_column(
                transaction,
                "notifications",
                "account_acct",
                "ALTER TABLE notifications ADD COLUMN account_acct TEXT",
            )
            .await?;
        }
        19 => {
            ensure_column(
                transaction,
                "statuses",
                "application_json",
                "ALTER TABLE statuses ADD COLUMN application_json TEXT",
            )
            .await?;
        }
        20 => {
            // A legacy cache may contain close to a million statuses. Migration
            // 020's schema and triggers are required before the app starts, but
            // its all-at-once trigram backfill can block Tauri setup for minutes.
            // Keep the bundled migration (and therefore its checksum) unchanged,
            // while deferring only the two data statements to the resumable
            // post-startup backfill introduced by migration 023.
            let schema_sql = status_search_schema_only_sql(migration.sql.as_ref())?;
            sqlx::raw_sql(&schema_sql)
                .execute(&mut **transaction)
                .await?;
        }
        21 => {
            let already_applied: bool = sqlx::query_scalar(
                "SELECT
                   EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'status_identities')
                   AND EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'status_viewer_state')
                   AND EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'status_tags')",
            )
            .fetch_one(&mut **transaction)
            .await?;
            if !already_applied {
                sqlx::raw_sql(migration.sql.as_ref())
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        22 => {
            let already_applied: bool = sqlx::query_scalar(
                "SELECT
                   EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'startup_sync_state')
                   AND EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'startup_sync_reconciliation_members')
                   AND EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'status_retention_protections')
                   AND EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'cache_counters')",
            )
            .fetch_one(&mut **transaction)
            .await?;
            if !already_applied {
                sqlx::raw_sql(migration.sql.as_ref())
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        23 => {
            let already_applied: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                      WHERE type = 'table' AND name = 'status_search_backfill_state'
                 )",
            )
            .fetch_one(&mut **transaction)
            .await?;
            if !already_applied {
                sqlx::raw_sql(migration.sql.as_ref())
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        24 => {
            // Keep the immutable migration SQL/checksum intact. Some legacy
            // databases predate SQLx history but already contain the index,
            // so the compatibility bootstrap must recognize the schema
            // object instead of requiring CREATE INDEX to be replayable.
            let already_applied: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                      WHERE type = 'index'
                        AND name = 'idx_login_accounts_single_active'
                 )",
            )
            .fetch_one(&mut **transaction)
            .await?;
            if !already_applied {
                sqlx::raw_sql(migration.sql.as_ref())
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        _ => {
            sqlx::raw_sql(migration.sql.as_ref())
                .execute(&mut **transaction)
                .await?;
        }
    }
    Ok(())
}

fn status_search_schema_only_sql(migration_sql: &str) -> Result<String, sqlx::Error> {
    const BACKFILL_START: &str = "DELETE FROM status_search_fts;";
    const BACKFILL_END: &str = "CREATE TRIGGER IF NOT EXISTS status_search_fts_status_insert";

    let start = migration_sql.find(BACKFILL_START).ok_or_else(|| {
        sqlx::Error::Protocol("migration 020 backfill start marker is missing".to_string())
    })?;
    let relative_end = migration_sql[start..].find(BACKFILL_END).ok_or_else(|| {
        sqlx::Error::Protocol("migration 020 backfill end marker is missing".to_string())
    })?;
    let end = start + relative_end;

    let mut schema_sql = String::with_capacity(migration_sql.len() - (end - start));
    schema_sql.push_str(&migration_sql[..start]);
    schema_sql.push_str(&migration_sql[end..]);
    Ok(schema_sql)
}

async fn ensure_column(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &str,
    column: &str,
    alter_sql: &str,
) -> Result<(), sqlx::Error> {
    debug_assert!(table
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_'));
    let pragma = format!("PRAGMA table_info({table})");
    let rows = sqlx::query(&pragma).fetch_all(&mut **transaction).await?;
    let exists = rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column);
    if !exists {
        sqlx::query(alter_sql).execute(&mut **transaction).await?;
    }
    Ok(())
}

async fn has_table(pool: &SqlitePool, table: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?
        )",
    )
    .bind(table)
    .fetch_one(pool)
    .await
}

async fn ensure_metadata_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _awayuki_schema_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn ensure_sqlx_migrations_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version BIGINT PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success BOOLEAN NOT NULL,
            checksum BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn set_migration_mode(pool: &SqlitePool, mode: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO _awayuki_schema_metadata (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(MIGRATION_MODE_KEY)
    .bind(mode)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::migrate::{Migrate, MigrationType};
    use sqlx::{Connection, SqliteConnection};
    use std::borrow::Cow;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn fixture_path(label: &str) -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "awayuki-migration-{label}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).expect("create fixture directory");
        let database = directory.join("fixture.db");
        (directory, database)
    }

    #[tokio::test]
    async fn analytical_reads_cannot_exhaust_the_interactive_reader_pool() {
        let (directory, path) = fixture_path("analytics-isolation");
        let database = Database::new(&path).await.expect("open database");

        let mut analytical_connections = Vec::new();
        for _ in 0..4 {
            analytical_connections.push(
                database
                    .analytics_reader()
                    .acquire()
                    .await
                    .expect("reserve analytical reader"),
            );
        }

        let interactive_read = tokio::time::timeout(
            Duration::from_secs(1),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(database.reader()),
        )
        .await
        .expect("interactive reader must not wait for analytical readers")
        .expect("interactive query");
        assert_eq!(interactive_read, 1);

        drop(analytical_connections);
        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    async fn legacy_database_at(path: &Path, version: i64) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true),
            )
            .await
            .expect("open legacy fixture");
        for migration in MIGRATOR.iter().filter(|migration| {
            !migration.migration_type.is_down_migration() && migration.version <= version
        }) {
            let mut transaction = pool.begin().await.expect("begin legacy migration");
            apply_legacy_migration(&mut transaction, migration)
                .await
                .expect("apply legacy migration");
            transaction.commit().await.expect("commit legacy migration");
        }
        pool.close().await;
    }

    async fn record_legacy_bootstrap_progress(pool: &SqlitePool, through_version: i64) {
        ensure_metadata_table(pool)
            .await
            .expect("create metadata table");
        ensure_sqlx_migrations_table(pool)
            .await
            .expect("create migration table");
        set_migration_mode(pool, LEGACY_BOOTSTRAP_MODE)
            .await
            .expect("mark legacy bootstrap");

        for migration in MIGRATOR.iter().filter(|migration| {
            !migration.migration_type.is_down_migration() && migration.version <= through_version
        }) {
            sqlx::query(
                "INSERT INTO _sqlx_migrations
                 (version, description, success, checksum, execution_time)
                 VALUES (?, ?, TRUE, ?, -1)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(pool)
            .await
            .expect("record bootstrap progress");
        }
    }

    async fn assert_current_schema(database: &Database) {
        let expected = MIGRATOR
            .iter()
            .filter(|migration| !migration.migration_type.is_down_migration())
            .count() as i64;
        let recorded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(database.reader())
            .await
            .expect("count migration history");
        assert_eq!(recorded, expected);
        assert!(column_exists(database.reader(), "login_accounts", "access_token").await);
        assert!(column_exists(database.reader(), "statuses", "quote_id").await);
        assert!(column_exists(database.reader(), "statuses", "quote_original_url").await);
        assert!(column_exists(database.reader(), "statuses", "application_json").await);
        assert!(!has_table(database.reader(), "client_credentials")
            .await
            .expect("inspect credentials table"));
    }

    async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
        let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(pool)
            .await
            .expect("inspect table");
        rows.iter()
            .any(|row| row.get::<String, _>("name") == column)
    }

    #[tokio::test]
    async fn fresh_database_uses_versioned_checksum_history() {
        let (directory, path) = fixture_path("fresh");
        let database = Database::new(&path).await.expect("open database");
        let report = database.run_migrations().await.expect("run migrations");

        assert!(!report.repaired_legacy_schema);
        assert_eq!(report.applied_versions.len(), MIGRATOR.iter().count());
        assert_current_schema(&database).await;

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn every_historical_schema_version_upgrades_to_current() {
        let latest_version = MIGRATOR
            .iter()
            .filter(|migration| !migration.migration_type.is_down_migration())
            .map(|migration| migration.version)
            .max()
            .expect("at least one migration");
        for version in 1..=latest_version {
            let (directory, path) = fixture_path(&format!("historical-{version}"));
            legacy_database_at(&path, version).await;
            let database = Database::new(&path).await.expect("open database");
            let report = database.run_migrations().await.expect("upgrade database");

            assert!(report.repaired_legacy_schema, "version {version}");
            assert_current_schema(&database).await;

            database.close().await;
            std::fs::remove_dir_all(directory).expect("remove fixture");
        }
    }

    #[tokio::test]
    async fn legacy_search_migration_defers_existing_rows_without_changing_checksum_history() {
        let (directory, path) = fixture_path("legacy-search-backfill");
        legacy_database_at(&path, 19).await;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path))
            .await
            .expect("open legacy search fixture");
        sqlx::query("INSERT INTO servers(domain, streaming_url) VALUES ('example.test', '')")
            .execute(&pool)
            .await
            .expect("insert server");
        sqlx::query(
            "INSERT INTO accounts(
                 id, server_domain, username, acct, display_name, created_at
             ) VALUES ('author', 'example.test', 'author', 'author', 'Author', '2026-01-01')",
        )
        .execute(&pool)
        .await
        .expect("insert account");
        sqlx::query(
            "INSERT INTO statuses(
                 id, server_domain, uri, created_at, account_id, content
             ) VALUES (
                 'status-1', 'example.test', 'https://example.test/statuses/1',
                 '2026-01-01', 'author', 'deferred searchable content'
             )",
        )
        .execute(&pool)
        .await
        .expect("insert cached status");
        pool.close().await;

        let database = Database::new(&path).await.expect("open database");
        let report = database.run_migrations().await.expect("upgrade database");
        assert!(report.repaired_legacy_schema);
        assert!(report.applied_versions.contains(&20));
        assert!(report.applied_versions.contains(&23));

        let document_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM status_search_documents")
                .fetch_one(database.reader())
                .await
                .expect("count deferred search documents");
        let backfill_complete = sqlx::query_scalar::<_, bool>(
            "SELECT completed FROM status_search_backfill_state WHERE singleton = 1",
        )
        .fetch_one(database.reader())
        .await
        .expect("read search backfill state");
        assert_eq!(document_count, 0);
        assert!(!backfill_complete);

        let migration_checksum = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT checksum FROM _sqlx_migrations WHERE version = 20",
        )
        .fetch_one(database.reader())
        .await
        .expect("read migration checksum");
        let bundled_checksum = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20)
            .expect("bundled migration 020")
            .checksum
            .as_ref();
        assert_eq!(migration_checksum.as_slice(), bundled_checksum);

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn fts_merge_policy_cannot_run_default_crisis_merge_on_interactive_writes() {
        let (directory, path) = fixture_path("fts-merge-policy");
        let database = Database::new(&path).await.expect("open database");
        let report = database.run_migrations().await.expect("run migrations");
        assert!(report.applied_versions.contains(&25));

        let config = sqlx::query_as::<_, (String, i64)>(
            "SELECT k, v FROM status_search_fts_config
             WHERE k IN ('automerge', 'crisismerge') ORDER BY k",
        )
        .fetch_all(database.reader())
        .await
        .expect("read FTS merge policy");
        assert_eq!(
            config,
            vec![
                ("automerge".to_string(), 8),
                ("crisismerge".to_string(), 128),
            ]
        );

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn migration_026_makes_global_column_scope_nullable_without_unbinding_local_columns() {
        let (directory, path) = fixture_path("global-column-scope");
        legacy_database_at(&path, 25).await;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path))
            .await
            .expect("open pre-026 fixture");
        sqlx::query(
            "INSERT INTO login_accounts(acct, server_domain, account_id)
             VALUES ('alice@example.test', 'example.test', 'alice')",
        )
        .execute(&pool)
        .await
        .expect("insert login account");
        sqlx::query(
            "INSERT INTO column_configs(id, account_acct, column_type, position)
             VALUES
               ('home', 'alice@example.test', 'home', 0),
               ('local', 'alice@example.test', 'local', 1)",
        )
        .execute(&pool)
        .await
        .expect("insert legacy columns");
        pool.close().await;

        let database = Database::new(&path).await.expect("open database");
        let report = database.run_migrations().await.expect("run migration 026");
        assert!(report.repaired_legacy_schema);
        assert!(report.applied_versions.contains(&26));

        let account_column = sqlx::query("PRAGMA table_info(column_configs)")
            .fetch_all(database.reader())
            .await
            .expect("inspect column config schema")
            .into_iter()
            .find(|row| row.get::<String, _>("name") == "account_acct")
            .expect("account_acct column");
        assert_eq!(account_column.get::<i64, _>("notnull"), 0);

        let scopes: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT id, account_acct FROM column_configs ORDER BY position")
                .fetch_all(database.reader())
                .await
                .expect("load migrated columns");
        assert_eq!(
            scopes,
            vec![
                ("home".to_string(), None),
                ("local".to_string(), Some("alice@example.test".to_string())),
            ]
        );

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[test]
    fn status_search_schema_only_keeps_schema_and_trigger_statements() {
        let migration = MIGRATOR
            .iter()
            .find(|migration| migration.version == 20)
            .expect("bundled migration 020");
        let schema =
            status_search_schema_only_sql(migration.sql.as_ref()).expect("extract search schema");

        assert!(schema.contains("CREATE VIRTUAL TABLE IF NOT EXISTS status_search_fts"));
        assert!(schema.contains("CREATE TRIGGER IF NOT EXISTS status_search_fts_status_insert"));
        assert!(!schema
            .contains("DELETE FROM status_search_fts;\nDELETE FROM status_search_documents;"));
        assert!(!schema.contains("SELECT id, server_domain\nFROM statuses\nORDER BY"));
    }

    #[tokio::test]
    async fn repairs_partially_applied_008_and_removes_obsolete_credentials_table() {
        let (directory, path) = fixture_path("partial-008");
        legacy_database_at(&path, 7).await;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path))
            .await
            .expect("open partial fixture");
        sqlx::query("ALTER TABLE login_accounts ADD COLUMN access_token TEXT NOT NULL DEFAULT ''")
            .execute(&pool)
            .await
            .expect("apply first half of 008");
        pool.close().await;

        let database = Database::new(&path).await.expect("open database");
        let _report = database.run_migrations().await.expect("repair database");
        assert!(!has_table(database.reader(), "client_credentials")
            .await
            .expect("inspect credentials table"));
        assert_current_schema(&database).await;

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn repairs_partially_applied_012_without_skipping_second_column() {
        let (directory, path) = fixture_path("partial-012");
        legacy_database_at(&path, 11).await;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path))
            .await
            .expect("open partial fixture");
        sqlx::query("ALTER TABLE statuses ADD COLUMN quote_id TEXT")
            .execute(&pool)
            .await
            .expect("apply first half of 012");
        pool.close().await;

        let database = Database::new(&path).await.expect("open database");
        let _report = database.run_migrations().await.expect("repair database");
        assert!(column_exists(database.reader(), "statuses", "quote_original_url").await);
        assert_current_schema(&database).await;

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn interrupted_legacy_bootstrap_resumes_transactionally() {
        let (directory, path) = fixture_path("legacy-resume");
        legacy_database_at(&path, 7).await;
        let database = Database::new(&path).await.expect("open database");
        record_legacy_bootstrap_progress(database.writer(), 7).await;

        let report = database
            .run_migrations()
            .await
            .expect("resume legacy bootstrap");

        assert!(report.repaired_legacy_schema);
        assert_eq!(report.applied_versions.first(), Some(&8));
        assert_current_schema(&database).await;

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn compatibility_migration_and_history_are_one_atomic_transaction() {
        let (directory, path) = fixture_path("legacy-atomic");
        legacy_database_at(&path, 11).await;
        let database = Database::new(&path).await.expect("open database");
        record_legacy_bootstrap_progress(database.writer(), 11).await;
        sqlx::query(
            "CREATE TRIGGER fail_migration_history
             BEFORE INSERT ON _sqlx_migrations
             WHEN NEW.version = 12
             BEGIN
                 SELECT RAISE(ABORT, 'injected migration history failure');
             END",
        )
        .execute(database.writer())
        .await
        .expect("install fault injection");

        database
            .run_migrations()
            .await
            .expect_err("fault injection must abort migration");
        assert!(!column_exists(database.reader(), "statuses", "quote_id").await);
        assert!(!column_exists(database.reader(), "statuses", "quote_original_url").await);
        let recorded: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM _sqlx_migrations WHERE version = 12)")
                .fetch_one(database.reader())
                .await
                .expect("inspect history");
        assert!(!recorded);

        sqlx::query("DROP TRIGGER fail_migration_history")
            .execute(database.writer())
            .await
            .expect("remove fault injection");
        let _report = database.run_migrations().await.expect("retry migration");
        assert_current_schema(&database).await;

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn checksum_drift_is_rejected_without_changing_schema() {
        let (directory, path) = fixture_path("checksum");
        let database = Database::new(&path).await.expect("open database");
        let _report = database.run_migrations().await.expect("run migrations");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = 1")
            .execute(database.writer())
            .await
            .expect("tamper checksum");

        let error = database
            .run_migrations()
            .await
            .expect_err("checksum mismatch must fail");
        assert!(matches!(
            error,
            MigrationError::Migrator(MigrateError::VersionMismatch(1))
        ));

        database.close().await;
        std::fs::remove_dir_all(directory).expect("remove fixture");
    }

    #[tokio::test]
    async fn failed_tracked_migration_rolls_back_its_schema_changes() {
        let mut connection = SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("open memory database");
        connection
            .ensure_migrations_table()
            .await
            .expect("create history table");
        let migration = Migration::new(
            900,
            Cow::Borrowed("atomic failure fixture"),
            MigrationType::Simple,
            Cow::Borrowed("CREATE TABLE should_rollback (id INTEGER);\nTHIS IS INVALID SQL;"),
            false,
        );

        connection
            .apply(&migration)
            .await
            .expect_err("migration must fail");
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'should_rollback')",
        )
        .fetch_one(&mut connection)
        .await
        .expect("inspect rollback");
        assert!(!exists);
    }
}
