use sqlx::SqlitePool;

#[derive(Debug, sqlx::FromRow)]
pub struct NotificationMutedAccountRow {
    pub account_id: String,
    pub server_domain: String,
    pub acct: String,
    pub display_name: String,
    pub avatar: String,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_muted_accounts(
    pool: &SqlitePool,
) -> Result<Vec<NotificationMutedAccountRow>, sqlx::Error> {
    sqlx::query_as::<_, NotificationMutedAccountRow>(
        "SELECT
           muted.account_id,
           muted.server_domain,
           COALESCE(NULLIF(accounts.acct, ''), NULLIF(muted.acct, ''), muted.account_id) AS acct,
           COALESCE(NULLIF(accounts.display_name, ''), NULLIF(muted.display_name, ''), NULLIF(accounts.acct, ''), NULLIF(muted.acct, ''), muted.account_id) AS display_name,
           COALESCE(accounts.avatar, '') AS avatar,
           muted.created_at,
           muted.updated_at
         FROM notification_muted_accounts muted
         LEFT JOIN accounts
           ON accounts.id = muted.account_id
          AND accounts.server_domain = muted.server_domain
         ORDER BY muted.updated_at DESC, muted.created_at DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn is_account_muted(
    pool: &SqlitePool,
    account_id: &str,
    server_domain: &str,
) -> Result<bool, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM notification_muted_accounts
         WHERE account_id = ? AND server_domain = ?",
    )
    .bind(account_id)
    .bind(server_domain)
    .fetch_one(pool)
    .await?;
    Ok(row.0 > 0)
}

pub async fn set_account_muted(
    pool: &SqlitePool,
    account_id: &str,
    server_domain: &str,
    acct: &str,
    display_name: &str,
    muted: bool,
) -> Result<(), sqlx::Error> {
    if muted {
        sqlx::query(
            "INSERT INTO notification_muted_accounts
             (account_id, server_domain, acct, display_name, updated_at)
             VALUES (?, ?, ?, ?, datetime('now'))
             ON CONFLICT(account_id, server_domain) DO UPDATE SET
               acct = excluded.acct,
               display_name = excluded.display_name,
               updated_at = excluded.updated_at",
        )
        .bind(account_id)
        .bind(server_domain)
        .bind(acct)
        .bind(display_name)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "DELETE FROM notification_muted_accounts
             WHERE account_id = ? AND server_domain = ?",
        )
        .bind(account_id)
        .bind(server_domain)
        .execute(pool)
        .await?;
    }
    Ok(())
}
