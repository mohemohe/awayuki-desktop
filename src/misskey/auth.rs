//! MiAuth (Misskey OAuth-like) flow.
//!
//! Reference: https://misskey-hub.net/en/docs/for-developers/api/authentication/
//!
//! Flow:
//! 1. Generate a session UUID.
//! 2. Open `https://<host>/miauth/<session>?name=<app>&callback=<url>&permission=<csv>` in a browser.
//! 3. User confirms; Misskey redirects to `<callback>?session=<session>`.
//! 4. Client POSTs `/api/miauth/<session>/check`, getting `{ ok, token, user }` back.

use uuid::Uuid;

use crate::mastodon::error::MastodonError;
use crate::misskey::client::MisskeyUnauthenticatedClient;
use crate::misskey::types::user::MisskeyUser;

/// Permissions we ask Misskey for. Mirrors the Mastodon scopes we use elsewhere
/// (`read write follow push`) but mapped to Misskey's enum.
pub const MIAUTH_PERMISSIONS: &[&str] = &[
    "read:account",
    "write:account",
    "read:blocks",
    "write:blocks",
    "read:drive",
    "write:drive",
    "read:favorites",
    "write:favorites",
    "read:following",
    "write:following",
    "read:messaging",
    "write:messaging",
    "read:mutes",
    "write:mutes",
    "write:notes",
    "read:notifications",
    "write:notifications",
    "read:reactions",
    "write:reactions",
    "write:votes",
];

pub struct MiAuthFlow {
    client: MisskeyUnauthenticatedClient,
    domain: String,
    session_id: String,
    callback_url: String,
}

impl MiAuthFlow {
    pub fn new(domain: &str, callback_port: u16) -> Result<Self, MastodonError> {
        Ok(Self {
            client: MisskeyUnauthenticatedClient::new()?,
            domain: domain.to_string(),
            session_id: Uuid::new_v4().to_string(),
            callback_url: format!("http://127.0.0.1:{}/callback", callback_port),
        })
    }

    pub fn authorize_url(&self) -> String {
        let perms = MIAUTH_PERMISSIONS.join(",");
        format!(
            "https://{host}/miauth/{session}?name={name}&callback={callback}&permission={perms}",
            host = self.domain,
            session = self.session_id,
            name = urlencoding::encode("awayuki"),
            callback = urlencoding::encode(&self.callback_url),
            perms = urlencoding::encode(&perms),
        )
    }

    pub async fn check(&self) -> Result<MiAuthCheckResult, MastodonError> {
        let url = format!(
            "https://{}/api/miauth/{}/check",
            self.domain, self.session_id
        );
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct CheckResponse {
            ok: bool,
            token: Option<String>,
            user: Option<MisskeyUser>,
        }
        let resp: CheckResponse = self.client.post(&url, serde_json::json!({})).await?;
        if !resp.ok {
            return Err(MastodonError::Other(
                "MiAuth check returned ok=false".into(),
            ));
        }
        let token = resp
            .token
            .ok_or_else(|| MastodonError::Other("MiAuth check missing token".into()))?;
        let user = resp
            .user
            .ok_or_else(|| MastodonError::Other("MiAuth check missing user".into()))?;
        Ok(MiAuthCheckResult { token, user })
    }
}

pub struct MiAuthCheckResult {
    pub token: String,
    #[allow(dead_code)]
    pub user: MisskeyUser,
}
