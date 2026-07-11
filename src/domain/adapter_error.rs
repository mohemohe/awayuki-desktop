use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterSource {
    ActivityPub,
    Misskey,
    AtProto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterErrorCode {
    Unauthorized,
    RateLimited,
    Timeout,
    Unsupported,
    InvalidResponse,
    Transport,
    Internal,
}

/// Protocol-neutral failure returned by domain ports.
///
/// The original provider error remains available as `source()` for redacted
/// diagnostics, while application decisions use stable code/source fields.
#[derive(Debug)]
pub struct AdapterError {
    pub code: AdapterErrorCode,
    pub adapter_source: AdapterSource,
    pub retry_after_seconds: Option<u64>,
    cause: Box<dyn Error + Send + Sync>,
}

impl AdapterError {
    pub fn new(
        code: AdapterErrorCode,
        adapter_source: AdapterSource,
        retry_after_seconds: Option<u64>,
        cause: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            code,
            adapter_source,
            retry_after_seconds,
            cause: Box::new(cause),
        }
    }
}

impl Display for AdapterError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:?} adapter error: {:?}",
            self.adapter_source, self.code
        )
    }
}

impl Error for AdapterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

pub type AdapterResult<T> = Result<T, AdapterError>;
