use crate::auth_mode::AuthMode;
use reqwest::header::HeaderMap;
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

/// The instrumented client's `.send()` returns `reqwest_middleware::Error`
/// (a superset of `reqwest::Error` that also covers middleware failures)
/// instead of a bare `reqwest::Error`. This keeps every existing `?` call
/// site on a provider's `.send()` compiling unchanged after the netflow
/// migration (rupu-netflow Plan 1 Task 10).
impl From<reqwest_middleware::Error> for ProviderError {
    fn from(e: reqwest_middleware::Error) -> Self {
        Self::Http(e.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e.to_string())
    }
}

/// Build the right `ProviderError` from a non-2xx HTTP response. 429s parse
/// the server's `Retry-After` header into `RateLimited { retry_after }` so
/// `tuned::RetryingProvider` can honor it (I-83); every other status keeps
/// the existing `Api { status, message }` shape.
///
/// This is the client-boundary call site: `headers` must come from the same
/// `reqwest::Response` the body was drained from (grab
/// `response.headers().clone()` *before* consuming the response with
/// `.text()`/`.json()` — headers are unavailable afterward).
pub fn api_error_from_response(status: u16, headers: &HeaderMap, message: String) -> ProviderError {
    if status == 429 {
        ProviderError::RateLimited {
            retry_after: parse_retry_after(headers),
        }
    } else {
        ProviderError::Api { status, message }
    }
}

/// Parse `Retry-After` as delta-seconds (RFC 9110 §10.2.3). The HTTP-date
/// form is not handled — mirrors `rupu-scm::error::parse_retry_after`, which
/// made the same call for the same header.
pub fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    v.parse::<u64>().ok().map(Duration::from_secs)
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
    fn parse_retry_after_reads_delta_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "3".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(3)));
    }

    #[test]
    fn parse_retry_after_absent_is_none() {
        assert_eq!(parse_retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn parse_retry_after_ignores_http_date_form() {
        let mut headers = HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn api_error_from_response_maps_429_to_rate_limited_with_header() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "5".parse().unwrap());
        let e = api_error_from_response(429, &headers, "rate limited".into());
        match e {
            ProviderError::RateLimited { retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(5)));
            }
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn api_error_from_response_maps_429_without_header_to_rate_limited_none() {
        let e = api_error_from_response(429, &HeaderMap::new(), "rate limited".into());
        assert!(matches!(
            e,
            ProviderError::RateLimited { retry_after: None }
        ));
    }

    #[test]
    fn api_error_from_response_keeps_other_statuses_as_api() {
        let e = api_error_from_response(500, &HeaderMap::new(), "boom".into());
        assert!(matches!(e, ProviderError::Api { status: 500, .. }));
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
