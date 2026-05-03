use crate::auth_mode::AuthMode;
use std::time::Duration;
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

    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },

    #[error("unauthorized: {provider} ({auth_mode}). {hint}")]
    Unauthorized {
        provider: String,
        auth_mode: AuthMode,
        hint: String,
    },

    #[error("quota exceeded for {provider}")]
    QuotaExceeded { provider: String },

    #[error("model unavailable: {model}")]
    ModelUnavailable { model: String },

    #[error("bad request: {message}")]
    BadRequest { message: String },

    #[error("transient error: {0}")]
    Transient(#[source] anyhow::Error),

    #[error("provider error: {0}")]
    Other(#[source] anyhow::Error),
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

#[cfg(test)]
mod structured_variants_tests {
    use super::*;
    use std::time::Duration;

    use crate::auth_mode::AuthMode;

    #[test]
    fn rate_limited_carries_retry_after() {
        let e = ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(7)),
        };
        let s = e.to_string();
        assert!(s.contains("rate limited"), "got: {s}");
    }

    #[test]
    fn unauthorized_renders_provider_and_mode() {
        let e = ProviderError::Unauthorized {
            provider: "anthropic".into(),
            auth_mode: AuthMode::Sso,
            hint: "run rupu auth login --provider anthropic --mode sso".into(),
        };
        let s = e.to_string();
        assert!(s.contains("anthropic"));
        assert!(s.contains("sso"));
        assert!(s.contains("rupu auth login"));
    }

    #[test]
    fn quota_exceeded_names_provider() {
        let e = ProviderError::QuotaExceeded {
            provider: "openai".into(),
        };
        assert!(e.to_string().contains("openai"));
    }

    #[test]
    fn model_unavailable_names_model() {
        let e = ProviderError::ModelUnavailable {
            model: "gpt-5".into(),
        };
        assert!(e.to_string().contains("gpt-5"));
    }

    #[test]
    fn bad_request_includes_message() {
        let e = ProviderError::BadRequest {
            message: "max_tokens too large".into(),
        };
        assert!(e.to_string().contains("max_tokens too large"));
    }
}
