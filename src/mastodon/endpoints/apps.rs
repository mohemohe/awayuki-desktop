use crate::constants::APP_DISPLAY_NAME;
use crate::mastodon::client::UnauthenticatedClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::application::AppRegistration;

impl UnauthenticatedClient {
    /// Register a new OAuth application on the instance.
    pub async fn register_app(
        &self,
        domain: &str,
        redirect_uri: &str,
    ) -> Result<AppRegistration, MastodonError> {
        let url = format!("https://{}/api/v1/apps", domain);
        self.post_form(
            &url,
            &[
                ("client_name", APP_DISPLAY_NAME),
                ("redirect_uris", redirect_uri),
                ("scopes", "read write follow push"),
                ("website", "https://github.com/mohemohe/awayuki-macos"),
            ],
        )
        .await
    }
}
