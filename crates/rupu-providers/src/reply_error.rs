//! A provider's error reply, parsed (spec 2026-10-01 response-outcomes §4.4).
//!
//! Every HTTP error body and every mid-stream error event becomes an
//! [`ApiErrorBody`]: the provider's own kind/code, message, request id and
//! the raw body, plus a normalized [`ErrorClass`] that retry logic and the
//! agent runner read instead of guessing from strings.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Raw bodies are kept up to this many bytes (cut on a char boundary).
pub const RAW_CAP_BYTES: usize = 64 * 1024;

/// `message` (what `Display` prints) is cut to this many characters. The full
/// body stays in `raw`. The text reaches logs, transcripts and the dashboard's
/// provider-health cache, so a 64 KB HTML error page must not ride along.
pub const MESSAGE_PREVIEW_CHARS: usize = 500;

fn preview(text: &str) -> String {
    text.chars().take(MESSAGE_PREVIEW_CHARS).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ErrorOrigin {
    Http {
        status: u16,
    },
    /// An error event inside a 200 stream.
    Stream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    RateLimited,
    Overloaded,
    Server,
    Timeout,
    InvalidRequest,
    ContextOverflow,
    Auth,
    Permission,
    Quota,
    NotFound,
    TooLarge,
    Policy,
    Unrecognized,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub provider: String,
    pub origin: ErrorOrigin,
    /// Anthropic `error.type` · OpenAI `error.code` (else `error.type`) ·
    /// Gemini `error.status`.
    pub kind: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
    pub details: Option<serde_json::Value>,
    /// The body as received (JSON when it parsed, else a string), capped.
    pub raw: serde_json::Value,
    pub retry_after_secs: Option<u64>,
    pub class: ErrorClass,
}

impl ApiErrorBody {
    pub fn status(&self) -> Option<u16> {
        match self.origin {
            ErrorOrigin::Http { status } => Some(status),
            ErrorOrigin::Stream => None,
        }
    }

    /// Re-issuing the same request could plausibly succeed. A mid-stream
    /// error is retried when its class says so, or when the provider sent no
    /// kind/code at all; an unknown kind/code is not retried, since it may
    /// name a permanent condition (a new policy code, ...).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.class,
            ErrorClass::RateLimited
                | ErrorClass::Overloaded
                | ErrorClass::Server
                | ErrorClass::Timeout
        ) || (self.origin == ErrorOrigin::Stream
            && self.class == ErrorClass::Unrecognized
            && self.kind.is_none())
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after_secs.map(Duration::from_secs)
    }
}

impl std::fmt::Display for ApiErrorBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.origin, self.kind.as_deref()) {
            (ErrorOrigin::Http { status }, _) => write!(f, "API error {status}: {}", self.message),
            (ErrorOrigin::Stream, Some(kind)) => {
                write!(f, "stream error ({kind}): {}", self.message)
            }
            (ErrorOrigin::Stream, None) => write!(f, "stream error: {}", self.message),
        }
    }
}

fn cap_chars(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Parse an HTTP error body. `request_id_header` is the `request-id` /
/// `x-request-id` header; the body's own `request_id` wins when present.
pub fn parse_error_body(
    provider: &str,
    origin: ErrorOrigin,
    text: &str,
    request_id_header: Option<String>,
    retry_after: Option<Duration>,
) -> ApiErrorBody {
    let mut body = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => parse_error_value(provider, origin, &v),
        Err(_) => {
            let capped = cap_chars(text.trim(), RAW_CAP_BYTES).to_string();
            let message = preview(&capped);
            ApiErrorBody {
                provider: provider.to_string(),
                origin,
                kind: None,
                message,
                request_id: None,
                details: None,
                raw: serde_json::Value::String(capped.clone()),
                retry_after_secs: None,
                class: classify(origin, None, &capped),
            }
        }
    };
    if text.len() > RAW_CAP_BYTES {
        body.raw = serde_json::Value::String(cap_chars(text, RAW_CAP_BYTES).to_string());
    }
    if body.request_id.is_none() {
        body.request_id = request_id_header;
    }
    body.retry_after_secs = retry_after.map(|d| d.as_secs());
    body
}

/// Parse an already-decoded error value (an SSE error event's data, a
/// `response.failed` error object, a chat-completions error chunk, …).
pub fn parse_error_value(
    provider: &str,
    origin: ErrorOrigin,
    value: &serde_json::Value,
) -> ApiErrorBody {
    // Gemini sometimes wraps the error in a one-element array.
    let v = match value {
        serde_json::Value::Array(items) if !items.is_empty() => &items[0],
        other => other,
    };
    let wrapped = v.get("error");
    let err = wrapped.unwrap_or(v);
    let str_at =
        |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
    // Every kind candidate is tried against the kind table, in this order: an
    // unrecognized `code` must not hide a recognized `type`. `kind` reports
    // the first one present.
    let (kinds, full_message, details) = match err {
        serde_json::Value::String(s) => (Vec::new(), s.clone(), None),
        serde_json::Value::Object(_) => {
            // A flat stream error event (no `error` wrapper) is its own
            // error object; its `"type": "error"` names the event, not a
            // kind. A real `code` or `status` on it still counts.
            let flat_event_type =
                |key: &str, kind: &str| wrapped.is_none() && key == "type" && kind == "error";
            let kinds: Vec<String> = ["code", "status", "type"]
                .iter()
                .filter_map(|k| str_at(err, k).filter(|kind| !flat_event_type(k, kind)))
                .collect();
            let message = str_at(err, "message")
                .or_else(|| str_at(err, "detail"))
                .unwrap_or_else(|| err.to_string());
            let details = err.get("details").or_else(|| err.get("param")).cloned();
            (kinds, message, details)
        }
        other => (Vec::new(), other.to_string(), None),
    };
    let kind = kinds.first().cloned();
    let kind_refs: Vec<&str> = kinds.iter().map(String::as_str).collect();
    // Classify on the full text (an overflow format can sit anywhere in it),
    // then keep only a preview for display.
    let class = classify_kinds(origin, &kind_refs, &full_message);
    let message = preview(&full_message);
    // `raw` is capped like an HTTP body: a value whose JSON is over the cap
    // is kept as its capped JSON text.
    let raw = match serde_json::to_string(value) {
        Ok(text) if text.len() > RAW_CAP_BYTES => {
            serde_json::Value::String(cap_chars(&text, RAW_CAP_BYTES).to_string())
        }
        _ => value.clone(),
    };
    ApiErrorBody {
        provider: provider.to_string(),
        origin,
        kind,
        message,
        request_id: str_at(v, "request_id"),
        details,
        raw,
        retry_after_secs: None,
        class,
    }
}

/// Provider kind/code first, then HTTP status. An `invalid_request`-class
/// error whose message is a context-overflow format is `ContextOverflow`.
pub fn classify(origin: ErrorOrigin, kind: Option<&str>, message: &str) -> ErrorClass {
    let kinds: Vec<&str> = kind.into_iter().collect();
    classify_kinds(origin, &kinds, message)
}

/// [`classify`] over several kind candidates: the first one the kind table
/// knows wins, and only when none is known does the HTTP status decide.
pub fn classify_kinds(origin: ErrorOrigin, kinds: &[&str], message: &str) -> ErrorClass {
    let by_kind = kinds
        .iter()
        .find_map(|k| match k.to_ascii_lowercase().as_str() {
            "rate_limit_error" | "rate_limit_exceeded" | "resource_exhausted" => {
                Some(ErrorClass::RateLimited)
            }
            "overloaded_error" | "unavailable" => Some(ErrorClass::Overloaded),
            "api_error" | "server_error" | "internal" => Some(ErrorClass::Server),
            "timeout_error" | "deadline_exceeded" => Some(ErrorClass::Timeout),
            "context_length_exceeded" | "model_max_prompt_tokens_exceeded" => {
                Some(ErrorClass::ContextOverflow)
            }
            "authentication_error" | "unauthenticated" | "invalid_api_key" => {
                Some(ErrorClass::Auth)
            }
            "permission_error" | "permission_denied" => Some(ErrorClass::Permission),
            "billing_error" | "insufficient_quota" => Some(ErrorClass::Quota),
            "not_found_error" | "not_found" | "model_not_found" => Some(ErrorClass::NotFound),
            "request_too_large" => Some(ErrorClass::TooLarge),
            "bio_policy"
            | "misalignment_policy_violation"
            | "content_policy_violation"
            | "image_content_policy_violation" => Some(ErrorClass::Policy),
            "invalid_request_error" | "invalid_argument" | "invalid_prompt" => {
                Some(ErrorClass::InvalidRequest)
            }
            // Policy codes keep growing (`cyber_policy`, ...); any kind that
            // names a policy is one.
            other if other.contains("policy") => Some(ErrorClass::Policy),
            _ => None,
        });
    let class = by_kind.unwrap_or(match origin {
        ErrorOrigin::Http { status } => match status {
            429 => ErrorClass::RateLimited,
            529 | 503 => ErrorClass::Overloaded,
            504 | 408 => ErrorClass::Timeout,
            401 => ErrorClass::Auth,
            403 => ErrorClass::Permission,
            402 => ErrorClass::Quota,
            404 => ErrorClass::NotFound,
            413 => ErrorClass::TooLarge,
            400 | 422 => ErrorClass::InvalidRequest,
            s if s >= 500 => ErrorClass::Server,
            _ => ErrorClass::Unrecognized,
        },
        ErrorOrigin::Stream => ErrorClass::Unrecognized,
    });
    if class == ErrorClass::InvalidRequest
        && crate::overflow::parse_context_overflow_specific(message).is_some()
    {
        return ErrorClass::ContextOverflow;
    }
    class
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(s: u16) -> ErrorOrigin {
        ErrorOrigin::Http { status: s }
    }

    #[test]
    fn anthropic_body_is_parsed() {
        let text = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_0001"}"#;
        let b = parse_error_body("anthropic", http(529), text, None, None);
        assert_eq!(b.kind.as_deref(), Some("overloaded_error"));
        assert_eq!(b.message, "Overloaded");
        assert_eq!(b.request_id.as_deref(), Some("req_0001"));
        assert_eq!(b.class, ErrorClass::Overloaded);
        assert!(b.is_retryable());
        assert_eq!(b.raw["error"]["type"], "overloaded_error");
    }

    #[test]
    fn openai_code_wins_over_generic_type() {
        let text = r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 130001 tokens.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
        let b = parse_error_body(
            "openai-codex",
            http(400),
            text,
            Some("req_hdr".into()),
            None,
        );
        assert_eq!(b.kind.as_deref(), Some("context_length_exceeded"));
        assert_eq!(b.class, ErrorClass::ContextOverflow);
        assert_eq!(b.request_id.as_deref(), Some("req_hdr"));
        assert!(!b.is_retryable());
    }

    #[test]
    fn gemini_status_is_the_kind_and_array_wrapping_is_unwrapped() {
        let text = r#"[{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"7s"}]}}]"#;
        let b = parse_error_body("google-gemini-cli", http(429), text, None, None);
        assert_eq!(b.kind.as_deref(), Some("RESOURCE_EXHAUSTED"));
        assert_eq!(b.class, ErrorClass::RateLimited);
        assert!(b.details.is_some());
    }

    #[test]
    fn invalid_request_that_is_an_overflow_classifies_as_overflow() {
        let text = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#;
        let b = parse_error_body("anthropic", http(400), text, None, None);
        assert_eq!(b.class, ErrorClass::ContextOverflow);
    }

    #[test]
    fn non_json_body_is_kept_raw_and_classified_by_status() {
        let b = parse_error_body(
            "openai-compatible",
            http(502),
            "<html>bad gateway</html>",
            None,
            None,
        );
        assert_eq!(b.kind, None);
        assert_eq!(b.message, "<html>bad gateway</html>");
        assert_eq!(b.raw, serde_json::json!("<html>bad gateway</html>"));
        assert_eq!(b.class, ErrorClass::Server);
    }

    #[test]
    fn huge_bodies_are_capped_char_safely() {
        let text = "é".repeat(RAW_CAP_BYTES); // 2 bytes each → over the cap
        let b = parse_error_body("anthropic", http(500), &text, None, None);
        let raw = b.raw.as_str().unwrap();
        assert!(raw.len() <= RAW_CAP_BYTES);
        assert!(raw.chars().all(|c| c == 'é'));
    }

    #[test]
    fn status_table_when_kind_is_unknown() {
        let c = |s| classify(http(s), None, "x");
        assert_eq!(c(429), ErrorClass::RateLimited);
        assert_eq!(c(529), ErrorClass::Overloaded);
        assert_eq!(c(503), ErrorClass::Overloaded);
        assert_eq!(c(500), ErrorClass::Server);
        assert_eq!(c(502), ErrorClass::Server);
        assert_eq!(c(504), ErrorClass::Timeout);
        assert_eq!(c(408), ErrorClass::Timeout);
        assert_eq!(c(401), ErrorClass::Auth);
        assert_eq!(c(403), ErrorClass::Permission);
        assert_eq!(c(402), ErrorClass::Quota);
        assert_eq!(c(404), ErrorClass::NotFound);
        assert_eq!(c(413), ErrorClass::TooLarge);
        assert_eq!(c(400), ErrorClass::InvalidRequest);
        assert_eq!(c(418), ErrorClass::Unrecognized);
    }

    #[test]
    fn kind_table() {
        let k = |kind: &str| classify(http(400), Some(kind), "x");
        assert_eq!(k("rate_limit_error"), ErrorClass::RateLimited);
        assert_eq!(k("rate_limit_exceeded"), ErrorClass::RateLimited);
        assert_eq!(k("RESOURCE_EXHAUSTED"), ErrorClass::RateLimited);
        assert_eq!(k("overloaded_error"), ErrorClass::Overloaded);
        assert_eq!(k("UNAVAILABLE"), ErrorClass::Overloaded);
        assert_eq!(k("api_error"), ErrorClass::Server);
        assert_eq!(k("server_error"), ErrorClass::Server);
        assert_eq!(k("INTERNAL"), ErrorClass::Server);
        assert_eq!(k("timeout_error"), ErrorClass::Timeout);
        assert_eq!(k("DEADLINE_EXCEEDED"), ErrorClass::Timeout);
        assert_eq!(k("context_length_exceeded"), ErrorClass::ContextOverflow);
        assert_eq!(
            k("model_max_prompt_tokens_exceeded"),
            ErrorClass::ContextOverflow
        );
        assert_eq!(k("authentication_error"), ErrorClass::Auth);
        assert_eq!(k("UNAUTHENTICATED"), ErrorClass::Auth);
        assert_eq!(k("invalid_api_key"), ErrorClass::Auth);
        assert_eq!(k("permission_error"), ErrorClass::Permission);
        assert_eq!(k("PERMISSION_DENIED"), ErrorClass::Permission);
        assert_eq!(k("billing_error"), ErrorClass::Quota);
        assert_eq!(k("insufficient_quota"), ErrorClass::Quota);
        assert_eq!(k("not_found_error"), ErrorClass::NotFound);
        assert_eq!(k("NOT_FOUND"), ErrorClass::NotFound);
        assert_eq!(k("model_not_found"), ErrorClass::NotFound);
        assert_eq!(k("request_too_large"), ErrorClass::TooLarge);
        assert_eq!(k("bio_policy"), ErrorClass::Policy);
        assert_eq!(k("misalignment_policy_violation"), ErrorClass::Policy);
        assert_eq!(k("invalid_request_error"), ErrorClass::InvalidRequest);
        assert_eq!(k("INVALID_ARGUMENT"), ErrorClass::InvalidRequest);
        assert_eq!(k("invalid_prompt"), ErrorClass::InvalidRequest);
        // Unknown kind falls back to the HTTP status (400 here).
        assert_eq!(k("brand_new_kind"), ErrorClass::InvalidRequest);
    }

    #[test]
    fn stream_errors_with_unknown_kind_are_unrecognized_and_not_retried() {
        let v =
            serde_json::json!({"type":"error","error":{"type":"brand_new_kind","message":"hmm"}});
        let b = parse_error_value("anthropic", ErrorOrigin::Stream, &v);
        assert_eq!(b.class, ErrorClass::Unrecognized);
        assert!(!b.is_retryable());
    }

    #[test]
    fn display_keeps_the_familiar_shape() {
        let b = parse_error_body(
            "anthropic",
            http(400),
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
            None,
            None,
        );
        assert_eq!(b.to_string(), "API error 400: bad");
        let s = parse_error_value(
            "anthropic",
            ErrorOrigin::Stream,
            &serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}),
        );
        assert_eq!(s.to_string(), "stream error (overloaded_error): Overloaded");
    }

    #[test]
    fn large_non_json_body_has_a_bounded_message_and_a_full_raw() {
        let text = "x".repeat(RAW_CAP_BYTES * 2);
        let b = parse_error_body("anthropic", http(502), &text, None, None);
        assert_eq!(b.message.chars().count(), MESSAGE_PREVIEW_CHARS);
        assert!(b.to_string().len() < MESSAGE_PREVIEW_CHARS + 64);
        assert_eq!(b.raw.as_str().unwrap().len(), RAW_CAP_BYTES);
    }

    #[test]
    fn a_pathologically_long_json_message_is_previewed_but_still_classified() {
        let long = format!(
            "{} prompt is too long: 215000 tokens > 200000 maximum",
            "y".repeat(2000)
        );
        let v = serde_json::json!({"error": {"type": "invalid_request_error", "message": long}});
        let b = parse_error_value("anthropic", http(400), &v);
        assert_eq!(b.message.chars().count(), MESSAGE_PREVIEW_CHARS);
        assert_eq!(b.class, ErrorClass::ContextOverflow);
        assert_eq!(
            b.raw["error"]["message"].as_str().unwrap().len(),
            long.len()
        );
    }

    #[test]
    fn an_unrecognized_code_does_not_hide_a_recognized_type() {
        let v = serde_json::json!({"error": {"code": "new_code", "type": "invalid_request_error", "message": "bad"}});
        let b = parse_error_value("openai-compatible", ErrorOrigin::Stream, &v);
        assert_eq!(b.kind.as_deref(), Some("new_code"));
        assert_eq!(b.class, ErrorClass::InvalidRequest);
    }

    #[test]
    fn any_kind_naming_a_policy_is_policy() {
        let k = |kind: &str| classify(ErrorOrigin::Stream, Some(kind), "x");
        assert_eq!(k("cyber_policy"), ErrorClass::Policy);
        assert_eq!(k("Brand_New_POLICY_violation"), ErrorClass::Policy);
        let v = serde_json::json!({"error": {"code": "weapons_policy", "message": "declined"}});
        let b = parse_error_value("openai-codex", ErrorOrigin::Stream, &v);
        assert_eq!(b.class, ErrorClass::Policy);
        assert!(!b.is_retryable());
    }

    #[test]
    fn a_stream_error_with_an_unknown_code_is_not_retried() {
        let v = serde_json::json!({"error": {"code": "permanent_new_thing", "message": "no"}});
        let b = parse_error_value("openai-codex", ErrorOrigin::Stream, &v);
        assert_eq!(b.class, ErrorClass::Unrecognized);
        assert!(!b.is_retryable());
    }

    #[test]
    fn a_stream_error_without_any_kind_is_retried() {
        let v = serde_json::json!({"error": {"message": "connection reset upstream"}});
        let b = parse_error_value("openai-codex", ErrorOrigin::Stream, &v);
        assert_eq!(b.kind, None);
        assert_eq!(b.class, ErrorClass::Unrecognized);
        assert!(b.is_retryable());
        let s = parse_error_value(
            "broker",
            ErrorOrigin::Stream,
            &serde_json::json!("something broke"),
        );
        assert!(s.is_retryable());
    }

    #[test]
    fn huge_stream_error_values_are_capped_char_safely() {
        let long = "é".repeat(RAW_CAP_BYTES);
        let v = serde_json::json!({"error": {"type": "api_error", "message": long}});
        let b = parse_error_value("anthropic", ErrorOrigin::Stream, &v);
        let raw = b
            .raw
            .as_str()
            .expect("an over-cap value is kept as a capped string");
        assert!(raw.len() <= RAW_CAP_BYTES);
        assert!(raw.starts_with(r#"{"error""#));
        assert_eq!(b.class, ErrorClass::Server);
        // A small value stays structured.
        let small = serde_json::json!({"error": {"type": "api_error", "message": "x"}});
        let b = parse_error_value("anthropic", ErrorOrigin::Stream, &small);
        assert_eq!(b.raw, small);
    }

    /// A flat stream error event (no `error` wrapper): its own
    /// `"type":"error"` is the event's type, not a kind.
    #[test]
    fn a_flat_error_event_s_type_is_not_a_kind() {
        let v = serde_json::json!({"type": "error", "code": null, "message": "upstream hiccup"});
        let b = parse_error_value("openai-codex", ErrorOrigin::Stream, &v);
        assert_eq!(b.kind, None);
        assert_eq!(b.class, ErrorClass::Unrecognized);
        assert!(b.is_retryable());
        assert_eq!(b.message, "upstream hiccup");

        let v =
            serde_json::json!({"type": "error", "code": "rate_limit_exceeded", "message": "slow"});
        let b = parse_error_value("openai-codex", ErrorOrigin::Stream, &v);
        assert_eq!(b.kind.as_deref(), Some("rate_limit_exceeded"));
        assert_eq!(b.class, ErrorClass::RateLimited);

        // A wrapped error's own `type` still counts.
        let v = serde_json::json!({"type": "error", "error": {"type": "overloaded_error", "message": "x"}});
        let b = parse_error_value("anthropic", ErrorOrigin::Stream, &v);
        assert_eq!(b.kind.as_deref(), Some("overloaded_error"));
    }
}
