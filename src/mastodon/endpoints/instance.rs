use serde::Deserialize;

use crate::mastodon::client::{MastodonClient, UnauthenticatedClient};
use crate::mastodon::error::MastodonError;
use crate::mastodon::types::instance::{Instance, InstanceConfiguration};

/// The part of the v2 / v1 instance document that describes upload limits.
/// Unlike `Instance` it does not require fields that differ between versions.
#[derive(Debug, Deserialize)]
struct InstanceUploadLimits {
    #[serde(default)]
    configuration: Option<InstanceConfiguration>,
}

impl InstanceUploadLimits {
    fn image_matrix_limit(self) -> Option<u64> {
        self.configuration?
            .media_attachments?
            .image_matrix_limit
            .and_then(|limit| u64::try_from(limit).ok())
            .filter(|limit| *limit > 0)
    }
}

impl MastodonClient {
    /// The server's current maximum image pixel count (`width * height`), or
    /// `None` when the server does not advertise one.
    pub async fn image_matrix_limit(&self) -> Result<Option<u64>, MastodonError> {
        let limits: InstanceUploadLimits = match self.get("/api/v2/instance").await {
            Ok(limits) => limits,
            Err(MastodonError::Api { status: 404, .. }) => self.get("/api/v1/instance").await?,
            Err(error) => return Err(error),
        };
        Ok(limits.image_matrix_limit())
    }
}

impl UnauthenticatedClient {
    /// Fetch instance info, trying v2 first then falling back to v1.
    pub async fn get_instance(&self, domain: &str) -> Result<Instance, MastodonError> {
        let base = format!("https://{}", domain);

        // Try v2 first
        match self
            .get::<Instance>(&format!("{}/api/v2/instance", base))
            .await
        {
            Ok(instance) => return Ok(instance),
            Err(MastodonError::Api { status: 404, .. }) => {
                tracing::info!("v2 instance API not available, falling back to v1");
            }
            Err(e) => return Err(e),
        }

        // Fallback to v1
        self.get::<Instance>(&format!("{}/api/v1/instance", base))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::InstanceUploadLimits;

    fn limit(json: &str) -> Option<u64> {
        serde_json::from_str::<InstanceUploadLimits>(json)
            .unwrap()
            .image_matrix_limit()
    }

    #[test]
    fn image_matrix_limit_is_read_from_v2_and_v1_instance_documents() {
        assert_eq!(
            limit(
                r#"{"domain":"example.test","configuration":{"urls":{"streaming":"wss://example.test"},"media_attachments":{"image_size_limit":41943040,"image_matrix_limit":16777216}}}"#
            ),
            Some(16_777_216)
        );
        assert_eq!(
            limit(
                r#"{"uri":"example.test","configuration":{"media_attachments":{"image_matrix_limit":33177600}}}"#
            ),
            Some(33_177_600)
        );
    }

    #[test]
    fn servers_without_a_usable_image_matrix_limit_report_none() {
        assert_eq!(limit(r#"{"uri":"example.test"}"#), None);
        assert_eq!(
            limit(r#"{"configuration":{"media_attachments":{"image_size_limit":1}}}"#),
            None
        );
        assert_eq!(
            limit(r#"{"configuration":{"media_attachments":{"image_matrix_limit":0}}}"#),
            None
        );
    }
}
