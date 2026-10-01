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

    /// A failure detected before any provider request could be attempted —
    /// e.g. a workflow step whose agent file failed to load. Displays the
    /// message verbatim: the caller owns the whole text, so no provider
    /// attribution or auth hint gets prepended to a failure that never
    /// involved a provider. Never retryable.
    #[error("{0}")]
    Preflight(String),

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

    /// A request carrying the 1M-context beta (`context-1m-2025-08-07`) was
    /// refused with Anthropic's 429 "Extra usage is required for long context
    /// requests": this account has no extra-usage entitlement for 1M context.
    /// The client has stopped sending the beta, so a retry goes out at the
    /// standard window. Never retryable as-is — the agent runner clamps the
    /// input limit and retries once.
    #[error("long context unavailable (1M beta disabled): {message}")]
    LongContextUnavailable { message: String },
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

/// Anthropic's refusal of a long-context request on an account without
/// extra-usage billing: a 429 whose body says "Extra usage is required for
/// long context requests" (see the `anthropic-beta` comment in
/// `anthropic.rs`). It is triggered by the 1M-context beta header, not by
/// the request's size, so it is refused the same way on every attempt — not
/// a rate limit, and the retry layers must not spend their budget on it. The
/// Anthropic client turns it into [`ProviderError::LongContextUnavailable`]
/// and stops sending the beta.
pub fn is_long_context_refusal(message: &str) -> bool {
    message
        .to_ascii_lowercase()
        .contains("extra usage is required for long context")
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

/// First `max` characters of a response body, for log lines. Char-based so a
/// multi-byte boundary can never panic.
fn body_preview(body: &str, max: usize) -> String {
    body.chars().take(max).collect()
}

/// Parse a model-listing response body as JSON.
///
/// The server answered — so a body that is not the expected JSON is a decode
/// failure ([`ProviderError::Json`]), not a transport failure
/// ([`ProviderError::Http`]); callers (and the operator reading the error)
/// must be able to tell "unreachable" from "reachable but sent garbage". A
/// short preview of the body is logged so the garbage is visible.
pub(crate) fn parse_listing_json<T: serde::de::DeserializeOwned>(
    provider: &str,
    body: &str,
) -> Result<T, ProviderError> {
    serde_json::from_str(body).map_err(|e| {
        tracing::warn!(
            provider,
            error = %e,
            body_preview = %body_preview(body, 200),
            "model listing body is not the expected JSON"
        );
        ProviderError::Json(format!("{provider} model listing: {e}"))
    })
}

/// The error for a model listing that parsed as JSON but has none of the
/// shapes the provider documents (e.g. no `data` array). Never an empty
/// catalog: "the server told us it has no models" and "the server told us
/// something we don't understand" must not look the same. The top-level keys
/// are logged so a changed wire format is diagnosable.
pub(crate) fn listing_shape_error(
    provider: &str,
    expected: &str,
    body: &serde_json::Value,
) -> ProviderError {
    let keys: Vec<&str> = body
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    tracing::warn!(
        provider,
        expected,
        top_level_keys = ?keys,
        "model listing has an unexpected shape"
    );
    ProviderError::Json(format!(
        "{provider} model listing has an unexpected shape (expected {expected})"
    ))
}

#[cfg(test)]
mod listing_helper_tests {
    use super::*;

    #[test]
    fn parse_listing_json_maps_garbage_to_json_not_http() {
        let err = parse_listing_json::<serde_json::Value>("acme", "not json").unwrap_err();
        match err {
            ProviderError::Json(m) => assert!(m.contains("acme"), "{m}"),
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn parse_listing_json_accepts_valid_json() {
        let v = parse_listing_json::<serde_json::Value>("acme", r#"{"data":[]}"#).unwrap();
        assert!(v["data"].is_array());
    }

    #[test]
    fn listing_shape_error_is_json_and_names_the_provider_and_expectation() {
        let err = listing_shape_error("acme", "a `data` array", &serde_json::json!({"weird": 1}));
        match err {
            ProviderError::Json(m) => {
                assert!(m.contains("acme") && m.contains("`data` array"), "{m}")
            }
            other => panic!("expected Json, got {other:?}"),
        }
    }

    #[test]
    fn body_preview_is_char_safe() {
        let s = "é".repeat(300);
        assert_eq!(body_preview(&s, 200).chars().count(), 200);
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
