use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("SSE parse error: {0}")]
    SseParse(String),

    #[error("JSON deserialization error: {0}")]
    Json(String),

    #[error("missing auth for {provider}: set {env_hint} or provide auth.json")]
    MissingAuth { provider: String, env_hint: String },

    #[error("stream ended unexpectedly")]
    UnexpectedEndOfStream,

    #[error("token refresh failed: {0}")]
    TokenRefreshFailed(String),

    #[error("auth config error: {0}")]
    AuthConfig(String),

    #[error("provider {provider} is not yet implemented")]
    NotImplemented { provider: String },
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e.to_string())
    }
}
