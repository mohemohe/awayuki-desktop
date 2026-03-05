use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

pub struct Database {
    writer: SqlitePool,
    reader: SqlitePool,
}

impl Database {
    pub async fn new(db_path: &str) -> Result<Self, sqlx::Error> {
        let write_opts = SqliteConnectOptions::from_str(db_path)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .pragma("foreign_keys", "ON")
            .pragma("synchronous", "NORMAL");

        let read_opts = SqliteConnectOptions::from_str(db_path)?
            .read_only(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .pragma("foreign_keys", "ON");

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(write_opts)
            .await?;

        let reader_count = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(4);

        let reader = SqlitePoolOptions::new()
            .max_connections(reader_count)
            .connect_with(read_opts)
            .await?;

        Ok(Self { writer, reader })
    }

    pub fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    pub fn reader(&self) -> &SqlitePool {
        &self.reader
    }

    pub async fn run_migrations(&self) -> Result<(), sqlx::Error> {
        let migration_files = [
            include_str!("../../migrations/001_create_servers.sql"),
            include_str!("../../migrations/002_create_accounts.sql"),
            include_str!("../../migrations/003_create_statuses.sql"),
            include_str!("../../migrations/004_create_notifications.sql"),
            include_str!("../../migrations/005_create_timeline_entries.sql"),
            include_str!("../../migrations/006_create_app_settings.sql"),
        ];

        for sql in &migration_files {
            sqlx::raw_sql(sql).execute(self.writer()).await?;
        }

        // ALTER TABLE migrations: ignore "duplicate column name" errors
        let alter_migrations = [
            include_str!("../../migrations/007_add_column_config_name.sql"),
            include_str!("../../migrations/008_add_credentials.sql"),
            include_str!("../../migrations/009_add_column_config_max_statuses.sql"),
            include_str!("../../migrations/010_add_column_config_pane_index.sql"),
        ];

        for sql in &alter_migrations {
            match sqlx::raw_sql(sql).execute(self.writer()).await {
                Ok(_) => {}
                Err(e) if e.to_string().contains("duplicate column name") => {
                    // Column already exists, skip
                }
                Err(e) => return Err(e),
            }
        }

        // Migrate existing data: each existing column becomes its own pane
        sqlx::raw_sql("UPDATE column_configs SET pane_index = position WHERE pane_index IS NULL")
            .execute(self.writer())
            .await?;

        Ok(())
    }

    pub async fn close(&self) {
        self.writer.close().await;
        self.reader.close().await;
    }
}
