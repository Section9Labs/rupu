use reqwest::header::HeaderMap;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP request failed: {0}")]
    Http(String),

    /// A provider's error reply — an HTTP error body or a mid-stream error
    /// event — parsed (spec 2026-10-01 §4.4).
    #[error("{0}")]
    Reply(Box<crate::reply_error::ApiErrorBody>),

    /// The stream ended before the provider finished the reply (e.g. an
    /// Anthropic stream with `message_start` but no `message_stop`).
    #[error("stream ended before the reply finished: {0}")]
    IncompleteStream(String),

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

    #[error("transient error: {0}")]
    Transient(#[source] anyhow::Error),

    /// SIGTERM has arrived (`credential_writes::terminating()`) and the
    /// request was not sent: nothing started now would finish.
    #[error("the process is terminating (SIGTERM); the request was not sent")]
    Terminating,

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

    /// A request carrying Anthropic's server-side fallback opt-in
    /// (`"fallbacks": "default"` + the `server-side-fallback-2026-07-01`
    /// beta) was refused with a 400 naming `fallbacks`: this account or
    /// gateway does not accept it. The client has stopped sending the opt-in,
    /// so a retry goes out without it. Never retryable as-is — the agent
    /// runner notes it and retries the turn once.
    #[error("server-side fallback unavailable: {message}")]
    FallbackUnavailable { message: String },
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

impl ProviderError {
    /// A synthetic HTTP error with a plain-text message — for tests and for
    /// errors rupu itself produces in the shape of a reply (router exhaustion).
    pub fn api(provider: &str, status: u16, message: &str) -> ProviderError {
        ProviderError::Reply(Box::new(crate::reply_error::parse_error_body(
            provider,
            crate::reply_error::ErrorOrigin::Http { status },
            message,
            None,
            None,
        )))
    }

    pub fn reply(&self) -> Option<&crate::reply_error::ApiErrorBody> {
        match self {
            ProviderError::Reply(b) => Some(b),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        self.reply().and_then(|b| b.status())
    }

    /// Normalized class for every variant (spec §4.4's fixed table for the
    /// errors that aren't a reply body).
    pub fn class(&self) -> crate::reply_error::ErrorClass {
        use crate::reply_error::ErrorClass as C;
        match self {
            ProviderError::Reply(b) => b.class,
            ProviderError::Http(_)
            | ProviderError::SseParse(_)
            | ProviderError::UnexpectedEndOfStream
            | ProviderError::IncompleteStream(_)
            | ProviderError::Transient(_) => C::Server,
            ProviderError::TokenRefreshFailed(_)
            | ProviderError::MissingAuth { .. }
            | ProviderError::AuthConfig(_) => C::Auth,
            ProviderError::LongContextUnavailable { .. } => C::ContextOverflow,
            ProviderError::FallbackUnavailable { .. } => C::InvalidRequest,
            ProviderError::Json(_)
            | ProviderError::Preflight(_)
            | ProviderError::NotImplemented { .. }
            | ProviderError::Other(_)
            | ProviderError::Terminating => C::Unrecognized,
        }
    }
}

impl ProviderError {
    /// Anthropic's long-context refusal in either shape: the extra-usage 429
    /// on a request without the 1M beta (a `Reply` with status 429), or
    /// [`ProviderError::LongContextUnavailable`] after one with it. The same
    /// request is refused the same way every time, so no retry layer — the
    /// providers' (`tuned::is_retryable`) or the agent runner's — retries
    /// it. The one predicate both use.
    pub fn is_long_context_refusal(&self) -> bool {
        match self {
            ProviderError::LongContextUnavailable { .. } => true,
            ProviderError::Reply(b) if b.status() == Some(429) => {
                is_long_context_refusal(&b.message)
            }
            _ => false,
        }
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

/// Build the error for a non-2xx HTTP response. `body` is the drained body
/// text; `headers` must come from the same response (grab them before
/// `.text()`). 429s carry the server's `Retry-After` (I-83).
pub fn api_error_from_response(
    provider: &str,
    status: u16,
    headers: &HeaderMap,
    body: &str,
) -> ProviderError {
    let request_id = ["request-id", "x-request-id"].iter().find_map(|h| {
        headers
            .get(*h)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    });
    let retry_after = if status == 429 {
        parse_retry_after(headers)
    } else {
        None
    };
    ProviderError::Reply(Box::new(crate::reply_error::parse_error_body(
        provider,
        crate::reply_error::ErrorOrigin::Http { status },
        body,
        request_id,
        retry_after,
    )))
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
    use crate::reply_error::ErrorClass;
    use std::time::Duration;

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
    fn api_error_from_response_carries_retry_after_on_a_429() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        let e = api_error_from_response("anthropic", 429, &headers, "slow down");
        assert_eq!(e.class(), ErrorClass::RateLimited);
        assert_eq!(
            e.reply().unwrap().retry_after(),
            Some(Duration::from_secs(7))
        );
    }

    #[test]
    fn api_error_from_response_without_header_has_no_retry_after() {
        let e = api_error_from_response("anthropic", 429, &HeaderMap::new(), "slow down");
        assert_eq!(e.reply().unwrap().retry_after(), None);
    }

    #[test]
    fn api_error_from_response_keeps_status_and_request_id_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", "req_h1".parse().unwrap());
        let e = api_error_from_response("openai-compatible", 500, &headers, "boom");
        assert_eq!(e.status(), Some(500));
        assert_eq!(e.class(), ErrorClass::Server);
        assert_eq!(e.reply().unwrap().request_id.as_deref(), Some("req_h1"));
    }

    #[test]
    fn long_context_429_body_is_a_long_context_refusal() {
        let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Extra usage is required for long context requests"}}"#;
        let e = api_error_from_response("anthropic", 429, &HeaderMap::new(), body);
        assert!(e.is_long_context_refusal());
        let plain = api_error_from_response("anthropic", 429, &HeaderMap::new(), "slow down");
        assert!(!plain.is_long_context_refusal());
    }

    #[test]
    fn non_reply_variants_have_a_fixed_class() {
        assert_eq!(ProviderError::Http("x".into()).class(), ErrorClass::Server);
        assert_eq!(
            ProviderError::IncompleteStream("x".into()).class(),
            ErrorClass::Server
        );
        assert_eq!(
            ProviderError::TokenRefreshFailed("x".into()).class(),
            ErrorClass::Auth
        );
        assert_eq!(
            ProviderError::Other(anyhow::anyhow!("x")).class(),
            ErrorClass::Unrecognized
        );
    }

    #[test]
    fn fallback_unavailable_is_an_invalid_request_and_never_retryable() {
        let e = ProviderError::FallbackUnavailable {
            message: "fallbacks: not enabled".into(),
        };
        assert_eq!(e.class(), ErrorClass::InvalidRequest);
        assert!(!crate::tuned::is_retryable(&e));
        assert_eq!(
            e.to_string(),
            "server-side fallback unavailable: fallbacks: not enabled"
        );
    }
}
