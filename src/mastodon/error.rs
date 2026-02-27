use thiserror::Error;

#[derive(Debug, Error)]
pub enum MastodonError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),

    #[error("Rate limited, retry after {retry_after:?}s")]
    RateLimited { retry_after: Option<u64> },

    #[error("Unauthorized - token may be expired")]
    Unauthorized,

    #[error("Instance not compatible: {0}")]
    IncompatibleInstance(String),

    #[error("{0}")]
    Other(String),
}
