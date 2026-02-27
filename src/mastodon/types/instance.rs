use serde::{Deserialize, Serialize};

/// Instance information from GET /api/v2/instance or GET /api/v1/instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub domain: String,
    pub title: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub configuration: Option<InstanceConfiguration>,
    // v1 compatibility: streaming_api field
    #[serde(default)]
    pub urls: Option<InstanceUrls>,
}

impl Instance {
    pub fn streaming_url(&self) -> Option<&str> {
        // v2: configuration.urls.streaming
        if let Some(config) = &self.configuration {
            if let Some(urls) = &config.urls {
                return Some(&urls.streaming);
            }
        }
        // v1: urls.streaming_api
        if let Some(urls) = &self.urls {
            if let Some(streaming) = &urls.streaming_api {
                return Some(streaming);
            }
        }
        None
    }

    pub fn max_characters(&self) -> i64 {
        self.configuration
            .as_ref()
            .and_then(|c| c.statuses.as_ref())
            .and_then(|s| s.max_characters)
            .unwrap_or(500)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceConfiguration {
    #[serde(default)]
    pub urls: Option<ConfigurationUrls>,
    #[serde(default)]
    pub statuses: Option<StatusesConfiguration>,
    #[serde(default)]
    pub media_attachments: Option<MediaConfiguration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigurationUrls {
    pub streaming: String,
}

/// v1 instance urls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceUrls {
    #[serde(default)]
    pub streaming_api: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusesConfiguration {
    pub max_characters: Option<i64>,
    pub max_media_attachments: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfiguration {
    pub supported_mime_types: Option<Vec<String>>,
    pub image_size_limit: Option<i64>,
    pub video_size_limit: Option<i64>,
}
