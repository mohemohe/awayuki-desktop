use sqlx::SqlitePool;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("Credential not found: {0}")]
    NotFound(String),
}

pub struct CredentialStore;

impl CredentialStore {
    pub async fn save_client_credentials(
        pool: &SqlitePool,
        domain: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), CredentialError> {
        sqlx::query(
            "INSERT INTO client_credentials (server_domain, client_id, client_secret)
             VALUES (?, ?, ?)
             ON CONFLICT(server_domain) DO UPDATE SET
               client_id = excluded.client_id,
               client_secret = excluded.client_secret",
        )
        .bind(domain)
        .bind(client_id)
        .bind(client_secret)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn get_client_credentials(
        pool: &SqlitePool,
        domain: &str,
    ) -> Result<(String, String), CredentialError> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT client_id, client_secret FROM client_credentials WHERE server_domain = ?",
        )
        .bind(domain)
        .fetch_optional(pool)
        .await?;

        row.ok_or_else(|| {
            CredentialError::NotFound(format!("No client credentials for {}", domain))
        })
    }
}
