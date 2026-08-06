use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct ComposeOutboxRow {
    pub id: String,
    pub operation_kind: String,
    pub acting_account_acct: String,
    pub payload_json: String,
    pub state: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub next_attempt_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub result_status_id: Option<String>,
    pub result_server_domain: Option<String>,
}

pub async fn enqueue(
    pool: &SqlitePool,
    id: &str,
    operation_kind: &str,
    acting_account_acct: &str,
    payload_json: &str,
    now: &str,
) -> Result<ComposeOutboxRow, sqlx::Error> {
    sqlx::query(
        "INSERT INTO compose_outbox (
             id, payload_version, operation_kind, acting_account_acct, payload_json, state,
             attempts, next_attempt_at, created_at, updated_at
         ) VALUES (?, 1, ?, ?, ?, 'queued', 0, ?, ?, ?)",
    )
    .bind(id)
    .bind(operation_kind)
    .bind(acting_account_acct)
    .bind(payload_json)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get(pool, id).await?.ok_or_else(|| sqlx::Error::RowNotFound)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<ComposeOutboxRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, operation_kind, acting_account_acct, payload_json, state,
                attempts, last_error, next_attempt_at, created_at, updated_at,
                completed_at, result_status_id, result_server_domain
           FROM compose_outbox
          ORDER BY
                CASE state
                    WHEN 'sending' THEN 0
                    WHEN 'queued' THEN 1
                    WHEN 'retrying' THEN 2
                    WHEN 'failed' THEN 3
                    WHEN 'uncertain' THEN 3
                    ELSE 4
                END,
                created_at DESC
          LIMIT 100",
    )
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<ComposeOutboxRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, operation_kind, acting_account_acct, payload_json, state,
                attempts, last_error, next_attempt_at, created_at, updated_at,
                completed_at, result_status_id, result_server_domain
           FROM compose_outbox
          WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn recover_interrupted(pool: &SqlitePool, now: &str) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "UPDATE compose_outbox
            SET state = 'uncertain',
                last_error = 'errors.delivery_uncertain',
                updated_at = ?
          WHERE state = 'sending'",
    )
    .bind(now)
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn claim_next(
    pool: &SqlitePool,
    now: &str,
) -> Result<Option<ComposeOutboxRow>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let id = sqlx::query_scalar::<_, String>(
        "SELECT id
           FROM compose_outbox
          WHERE state IN ('queued', 'retrying')
            AND next_attempt_at <= ?
            AND NOT EXISTS (
                SELECT 1
                  FROM compose_outbox AS older
                 WHERE older.acting_account_acct = compose_outbox.acting_account_acct
                   AND older.created_at < compose_outbox.created_at
                   AND older.state IN ('queued', 'sending', 'retrying')
            )
          ORDER BY created_at ASC
          LIMIT 1",
    )
    .bind(now)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(id) = id else {
        transaction.commit().await?;
        return Ok(None);
    };
    let changed = sqlx::query(
        "UPDATE compose_outbox
            SET state = 'sending',
                attempts = attempts + 1,
                updated_at = ?
          WHERE id = ?
            AND state IN ('queued', 'retrying')",
    )
    .bind(now)
    .bind(&id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if changed == 0 {
        transaction.rollback().await?;
        return Ok(None);
    }
    let row = sqlx::query_as(
        "SELECT id, operation_kind, acting_account_acct, payload_json, state,
                attempts, last_error, next_attempt_at, created_at, updated_at,
                completed_at, result_status_id, result_server_domain
           FROM compose_outbox
          WHERE id = ?
            AND state = 'sending'",
    )
    .bind(&id)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(row))
}

pub async fn mark_succeeded(
    pool: &SqlitePool,
    id: &str,
    status_id: &str,
    server_domain: &str,
    now: &str,
) -> Result<ComposeOutboxRow, sqlx::Error> {
    sqlx::query(
        "UPDATE compose_outbox
            SET state = 'succeeded',
                last_error = NULL,
                completed_at = ?,
                result_status_id = ?,
                result_server_domain = ?,
                updated_at = ?
          WHERE id = ?
            AND state = 'sending'",
    )
    .bind(now)
    .bind(status_id)
    .bind(server_domain)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    get(pool, id).await?.ok_or_else(|| sqlx::Error::RowNotFound)
}

pub async fn mark_retrying(
    pool: &SqlitePool,
    id: &str,
    error: &str,
    next_attempt_at: &str,
    now: &str,
) -> Result<ComposeOutboxRow, sqlx::Error> {
    update_failure(pool, id, "retrying", error, next_attempt_at, now).await
}

pub async fn mark_failed(
    pool: &SqlitePool,
    id: &str,
    error: &str,
    now: &str,
) -> Result<ComposeOutboxRow, sqlx::Error> {
    update_failure(pool, id, "failed", error, now, now).await
}

pub async fn mark_uncertain(
    pool: &SqlitePool,
    id: &str,
    error: &str,
    now: &str,
) -> Result<ComposeOutboxRow, sqlx::Error> {
    update_failure(pool, id, "uncertain", error, now, now).await
}

async fn update_failure(
    pool: &SqlitePool,
    id: &str,
    state: &str,
    error: &str,
    next_attempt_at: &str,
    now: &str,
) -> Result<ComposeOutboxRow, sqlx::Error> {
    sqlx::query(
        "UPDATE compose_outbox
            SET state = ?,
                last_error = ?,
                next_attempt_at = ?,
                updated_at = ?
          WHERE id = ?
            AND state = 'sending'",
    )
    .bind(state)
    .bind(error)
    .bind(next_attempt_at)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    get(pool, id).await?.ok_or_else(|| sqlx::Error::RowNotFound)
}

pub async fn retry(
    pool: &SqlitePool,
    id: &str,
    now: &str,
) -> Result<Option<ComposeOutboxRow>, sqlx::Error> {
    let changed = sqlx::query(
        "UPDATE compose_outbox
            SET state = 'queued',
                last_error = NULL,
                next_attempt_at = ?,
                completed_at = NULL,
                updated_at = ?
          WHERE id = ?
            AND state IN ('failed', 'uncertain', 'cancelled')",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

pub async fn cancel(
    pool: &SqlitePool,
    id: &str,
    now: &str,
) -> Result<Option<ComposeOutboxRow>, sqlx::Error> {
    let changed = sqlx::query(
        "UPDATE compose_outbox
            SET state = 'cancelled',
                completed_at = ?,
                updated_at = ?
          WHERE id = ?
            AND state IN ('queued', 'retrying', 'failed', 'uncertain')",
    )
    .bind(now)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
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
        sqlx::query("INSERT INTO login_accounts (acct) VALUES ('alice'), ('bob')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(include_str!(
            "../../../migrations/036_create_compose_outbox.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn preserves_fifo_per_actor_without_blocking_other_actors() {
        let pool = pool().await;
        enqueue(
            &pool,
            "alice-1",
            "post",
            "alice",
            "{}",
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();
        enqueue(
            &pool,
            "alice-2",
            "edit",
            "alice",
            "{}",
            "2026-01-01T00:00:01Z",
        )
        .await
        .unwrap();
        enqueue(&pool, "bob-1", "post", "bob", "{}", "2026-01-01T00:00:02Z")
            .await
            .unwrap();

        let first = claim_next(&pool, "2026-01-01T00:01:00Z")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.id, "alice-1");
        mark_retrying(
            &pool,
            "alice-1",
            "temporary",
            "2026-01-01T01:00:00Z",
            "2026-01-01T00:01:01Z",
        )
        .await
        .unwrap();

        let second = claim_next(&pool, "2026-01-01T00:02:00Z")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.id, "bob-1");
        assert_eq!(
            get(&pool, "alice-2").await.unwrap().unwrap().state,
            "queued"
        );
    }

    #[tokio::test]
    async fn list_is_limited_to_the_100_most_recent_items() {
        let pool = pool().await;
        for index in 0..101 {
            enqueue(
                &pool,
                &format!("post-{index:03}"),
                "post",
                "alice",
                "{}",
                &format!("2026-01-01T00:{:02}:{:02}Z", index / 60, index % 60),
            )
            .await
            .unwrap();
        }

        let items = list(&pool).await.unwrap();

        assert_eq!(items.len(), 100);
        assert_eq!(items.first().unwrap().id, "post-100");
        assert_eq!(items.last().unwrap().id, "post-001");
        assert!(!items.iter().any(|item| item.id == "post-000"));
    }

    #[tokio::test]
    async fn interrupted_delivery_becomes_uncertain_instead_of_auto_retrying() {
        let pool = pool().await;
        enqueue(
            &pool,
            "post-1",
            "post",
            "alice",
            "{}",
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();
        claim_next(&pool, "2026-01-01T00:01:00Z")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            recover_interrupted(&pool, "2026-01-01T00:02:00Z")
                .await
                .unwrap(),
            1
        );
        let recovered = get(&pool, "post-1").await.unwrap().unwrap();
        assert_eq!(recovered.state, "uncertain");
        assert_eq!(
            recovered.last_error.as_deref(),
            Some("errors.delivery_uncertain")
        );
    }
}
