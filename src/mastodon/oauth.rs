use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::mastodon::client::UnauthenticatedClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::application::{AppRegistration, TokenResponse};
use crate::mastodon::types::instance::Instance;
use crate::mastodon::OAUTH_SCOPES;

pub struct OAuthFlow {
    client: UnauthenticatedClient,
    domain: String,
    redirect_uri: String,
    state: String,
    code_verifier: String,
    code_challenge: String,
    pub registration: Option<AppRegistration>,
    pub instance: Option<Instance>,
}

impl OAuthFlow {
    pub fn new(domain: &str, callback_port: u16) -> Result<Self, MastodonError> {
        let state = Uuid::new_v4().to_string();
        let code_verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        Ok(Self {
            client: UnauthenticatedClient::new()?,
            domain: domain.to_string(),
            redirect_uri: format!("http://127.0.0.1:{}/callback", callback_port),
            state,
            code_verifier,
            code_challenge,
            registration: None,
            instance: None,
        })
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    /// Step 1: Fetch instance info and register app.
    pub async fn prepare(&mut self) -> Result<(), MastodonError> {
        // Fetch instance info
        let instance = self.client.get_instance(&self.domain).await?;
        tracing::info!(
            "Instance: {} (version {})",
            instance.title,
            instance.version
        );
        self.instance = Some(instance);

        // Register app
        let registration = self
            .client
            .register_app(&self.domain, &self.redirect_uri)
            .await?;
        tracing::info!("App registered: client_id={}", registration.client_id);
        self.registration = Some(registration);

        Ok(())
    }

    /// Step 2: Generate authorization URL for the user's browser.
    pub fn authorize_url(&self) -> Option<String> {
        let reg = self.registration.as_ref()?;
        let mut url = Url::parse(&format!("https://{}/oauth/authorize", self.domain)).ok()?;
        url.query_pairs_mut()
            .append_pair("client_id", &reg.client_id)
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", OAUTH_SCOPES)
            .append_pair("state", &self.state)
            .append_pair("code_challenge", &self.code_challenge)
            .append_pair("code_challenge_method", "S256");
        Some(url.into())
    }

    /// Step 3: Exchange authorization code for access token.
    pub async fn exchange_code(&self, code: &str) -> Result<TokenResponse, MastodonError> {
        let reg = self
            .registration
            .as_ref()
            .ok_or_else(|| MastodonError::IncompatibleInstance("App not registered".into()))?;

        let url = format!("https://{}/oauth/token", self.domain);
        self.client
            .post_form(
                &url,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("client_id", &reg.client_id),
                    ("client_secret", &reg.client_secret),
                    ("redirect_uri", &self.redirect_uri),
                    ("scope", OAUTH_SCOPES),
                    ("code_verifier", &self.code_verifier),
                ],
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_contains_state_and_s256_pkce() {
        let mut flow = OAuthFlow::new("example.social", 12345).expect("create OAuth flow");
        flow.registration = Some(AppRegistration {
            id: Some("app-id".to_string()),
            name: "Awayuki".to_string(),
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            redirect_uri: Some("http://127.0.0.1:12345/callback".to_string()),
            vapid_key: None,
        });

        let url = Url::parse(&flow.authorize_url().expect("authorization URL"))
            .expect("parse authorization URL");
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("state"), Some(&flow.state));
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(query.get("scope").map(String::as_str), Some(OAUTH_SCOPES));
        assert_eq!(flow.code_verifier.len(), 64);
        assert_eq!(query.get("code_challenge"), Some(&flow.code_challenge));
        assert!(flow
            .code_verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')));
        assert_eq!(
            flow.code_challenge,
            URL_SAFE_NO_PAD.encode(Sha256::digest(flow.code_verifier.as_bytes()))
        );
        assert!(!flow.code_challenge.contains('='));
    }
}
