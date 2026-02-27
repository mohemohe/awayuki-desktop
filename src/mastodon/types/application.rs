use serde::{Deserialize, Serialize};

/// Response from POST /api/v1/apps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRegistration {
    pub id: Option<String>,
    pub name: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub redirect_uri: Option<String>,
    #[serde(default)]
    pub vapid_key: Option<String>,
}

/// Response from POST /oauth/token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub created_at: i64,
}
