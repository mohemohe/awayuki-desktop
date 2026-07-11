use serde::{Deserialize, Serialize};

/// The federation protocol that owns a canonical object identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FederationProtocol {
    ActivityPub,
    AtProto,
}

impl FederationProtocol {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::ActivityPub => "activitypub",
            Self::AtProto => "atproto",
        }
    }
}

/// Stable subject identity, kept separate from the account performing an
/// operation. `remote_id` is only meaningful on `server_domain`; cross-server
/// mutations must resolve `canonical_uri` on the acting account's server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusIdentity {
    pub protocol: FederationProtocol,
    pub server_domain: String,
    pub canonical_uri: String,
    pub remote_id: String,
}

impl StatusIdentity {
    pub fn new(
        protocol: FederationProtocol,
        server_domain: impl Into<String>,
        canonical_uri: impl Into<String>,
        remote_id: impl Into<String>,
    ) -> Self {
        Self {
            protocol,
            server_domain: server_domain.into().trim().to_ascii_lowercase(),
            canonical_uri: canonical_uri.into().trim().to_string(),
            remote_id: remote_id.into().trim().to_string(),
        }
    }

    pub fn inferred(
        server_domain: impl Into<String>,
        canonical_uri: impl Into<String>,
        remote_id: impl Into<String>,
    ) -> Self {
        let canonical_uri = canonical_uri.into();
        let protocol = if canonical_uri.trim().starts_with("at://") {
            FederationProtocol::AtProto
        } else {
            FederationProtocol::ActivityPub
        };
        Self::new(protocol, server_domain, canonical_uri, remote_id)
    }

    pub fn validate(&self) -> Result<(), IdentityError> {
        if self.server_domain.is_empty() {
            return Err(IdentityError::MissingServerDomain);
        }
        if self.remote_id.is_empty() {
            return Err(IdentityError::MissingRemoteId);
        }
        if self.canonical_uri.is_empty() {
            return Err(IdentityError::MissingCanonicalUri);
        }
        match self.protocol {
            FederationProtocol::AtProto if !self.canonical_uri.starts_with("at://") => {
                Err(IdentityError::ProtocolUriMismatch)
            }
            FederationProtocol::ActivityPub => {
                let parsed = url::Url::parse(&self.canonical_uri)
                    .map_err(|_| IdentityError::ProtocolUriMismatch)?;
                if matches!(parsed.scheme(), "http" | "https") {
                    Ok(())
                } else {
                    Err(IdentityError::ProtocolUriMismatch)
                }
            }
            FederationProtocol::AtProto => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("status identity is missing server_domain")]
    MissingServerDomain,
    #[error("status identity is missing remote_id")]
    MissingRemoteId,
    #[error("status identity is missing canonical_uri")]
    MissingCanonicalUri,
    #[error("status identity protocol does not match canonical_uri")]
    ProtocolUriMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_include_server_for_equal_remote_ids() {
        let alpha = StatusIdentity::inferred("alpha.example", "https://alpha.example/@a/1", "1");
        let beta = StatusIdentity::inferred("beta.example", "https://beta.example/@b/1", "1");
        assert_ne!(alpha, beta);
    }

    #[test]
    fn protocol_and_uri_must_agree() {
        let identity = StatusIdentity::new(
            FederationProtocol::AtProto,
            "bsky.social",
            "https://example.test/status/1",
            "1",
        );
        assert_eq!(identity.validate(), Err(IdentityError::ProtocolUriMismatch));
    }
}
