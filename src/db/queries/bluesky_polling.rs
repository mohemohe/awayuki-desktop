//! Portable Bluesky polling checkpoints.

use std::collections::HashSet;

use sqlx::{QueryBuilder, Sqlite, SqlitePool};

pub async fn load_checkpoint(
    pool: &SqlitePool,
    account_acct: &str,
    stream_key: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT checkpoint_json FROM bluesky_poll_checkpoints
          WHERE account_acct = ? AND stream_key = ?",
    )
    .bind(account_acct)
    .bind(stream_key)
    .fetch_optional(pool)
    .await
}

/// Returns one only when the durable checkpoint actually changed.
pub async fn save_checkpoint(
    pool: &SqlitePool,
    account_acct: &str,
    stream_key: &str,
    checkpoint_json: &str,
    updated_at: &str,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "INSERT INTO bluesky_poll_checkpoints
           (account_acct, stream_key, checkpoint_json, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(account_acct, stream_key) DO UPDATE SET
           checkpoint_json = excluded.checkpoint_json,
           updated_at = excluded.updated_at
         WHERE bluesky_poll_checkpoints.checkpoint_json != excluded.checkpoint_json",
    )
    .bind(account_acct)
    .bind(stream_key)
    .bind(checkpoint_json)
    .bind(updated_at)
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn existing_status_ids(
    pool: &SqlitePool,
    ids: &[String],
) -> Result<HashSet<String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashSet::new());
    }
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT DISTINCT id FROM statuses WHERE id IN (");
    let mut separated = builder.separated(", ");
    for id in ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(")");
    Ok(builder
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unchanged_checkpoint_does_not_write_again() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE login_accounts (acct TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE statuses (id TEXT NOT NULL, server_domain TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO login_accounts (acct) VALUES ('alice.test')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(include_str!(
            "../../../migrations/027_persist_bluesky_poll_checkpoints.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            save_checkpoint(&pool, "alice.test", "home", "{}", "2026-01-01T00:00:00Z")
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            save_checkpoint(&pool, "alice.test", "home", "{}", "2026-01-01T00:01:00Z")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            load_checkpoint(&pool, "alice.test", "home")
                .await
                .unwrap()
                .as_deref(),
            Some("{}")
        );
        sqlx::query("DELETE FROM login_accounts WHERE acct = 'alice.test'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(load_checkpoint(&pool, "alice.test", "home")
            .await
            .unwrap()
            .is_none());

        sqlx::query(
            "INSERT INTO statuses (id, server_domain) VALUES ('at://cached', 'bsky.social')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let existing = existing_status_ids(
            &pool,
            &["at://cached".to_string(), "at://missing".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(existing, HashSet::from(["at://cached".to_string()]));
    }
}
