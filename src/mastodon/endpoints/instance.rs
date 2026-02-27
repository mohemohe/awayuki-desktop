use crate::mastodon::client::UnauthenticatedClient;
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::instance::Instance;

impl UnauthenticatedClient {
    /// Fetch instance info, trying v2 first then falling back to v1.
    pub async fn get_instance(&self, domain: &str) -> Result<Instance, MastodonError> {
        let base = format!("https://{}", domain);

        // Try v2 first
        match self.get::<Instance>(&format!("{}/api/v2/instance", base)).await {
            Ok(instance) => return Ok(instance),
            Err(MastodonError::Api { status: 404, .. }) => {
                tracing::info!("v2 instance API not available, falling back to v1");
            }
            Err(e) => return Err(e),
        }

        // Fallback to v1
        self.get::<Instance>(&format!("{}/api/v1/instance", base)).await
    }
}
