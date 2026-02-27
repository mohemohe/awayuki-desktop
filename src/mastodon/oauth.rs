use crate::mastodon::client::UnauthenticatedClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::application::{AppRegistration, TokenResponse};
use crate::mastodon::types::instance::Instance;

pub struct OAuthFlow {
    client: UnauthenticatedClient,
    domain: String,
    redirect_uri: String,
    pub registration: Option<AppRegistration>,
    pub instance: Option<Instance>,
}

impl OAuthFlow {
    pub fn new(domain: &str, callback_port: u16) -> Result<Self, MastodonError> {
        Ok(Self {
            client: UnauthenticatedClient::new()?,
            domain: domain.to_string(),
            redirect_uri: format!("http://127.0.0.1:{}/callback", callback_port),
            registration: None,
            instance: None,
        })
    }

    pub fn domain(&self) -> &str {
        &self.domain
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
        Some(format!(
            "https://{}/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&scope={}",
            self.domain,
            &reg.client_id,
            urlencoding::encode(&self.redirect_uri),
            urlencoding::encode("read write follow push"),
        ))
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
                    ("scope", "read write follow push"),
                ],
            )
            .await
    }
}
