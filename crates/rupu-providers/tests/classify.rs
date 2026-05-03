use rupu_providers::classify::{
    classify_anthropic, classify_copilot, classify_gemini, classify_openai,
};
use rupu_providers::error::ProviderError;

#[test]
fn anthropic_429_is_rate_limited() {
    let e = classify_anthropic(429, "{}", None);
    assert!(matches!(e, ProviderError::RateLimited { .. }));
}

#[test]
fn anthropic_529_is_rate_limited_overloaded() {
    let e = classify_anthropic(529, "{}", None);
    assert!(matches!(e, ProviderError::RateLimited { .. }));
}

#[test]
fn anthropic_401_is_unauthorized() {
    let e = classify_anthropic(401, "{}", None);
    assert!(matches!(e, ProviderError::Unauthorized { .. }));
}

#[test]
fn openai_403_with_billing_message_is_quota() {
    let body = r#"{"error":{"type":"billing_hard_limit_reached"}}"#;
    let e = classify_openai(403, body, Some("billing_hard_limit_reached"));
    assert!(matches!(e, ProviderError::QuotaExceeded { .. }));
}

#[test]
fn openai_404_model_not_found() {
    let body = r#"{"error":{"type":"model_not_found"}}"#;
    let e = classify_openai(404, body, Some("model_not_found"));
    assert!(matches!(e, ProviderError::ModelUnavailable { .. }));
}

#[test]
fn openai_400_is_bad_request() {
    let e = classify_openai(400, r#"{"error":{"message":"max_tokens too large"}}"#, None);
    match e {
        ProviderError::BadRequest { message } => assert!(message.contains("max_tokens")),
        other => panic!("expected BadRequest, got {other:?}"),
    }
}

#[test]
fn gemini_503_is_transient() {
    let e = classify_gemini(503, "{}", None);
    assert!(matches!(e, ProviderError::Transient(_)));
}

#[test]
fn copilot_500_is_transient() {
    let e = classify_copilot(500, "{}", None);
    assert!(matches!(e, ProviderError::Transient(_)));
}

#[test]
fn unknown_status_falls_to_other() {
    let e = classify_anthropic(418, "I'm a teapot", None);
    assert!(matches!(e, ProviderError::Other(_)));
}

#[test]
fn classify_handles_multibyte_utf8_body_without_panic() {
    // 200 crab emojis (4 bytes each = 800 bytes total), truncated to a
    // 500-byte limit with the helper inside classify_openai's BadRequest
    // arm. Pre-fix this would panic with "byte index 500 is not a char
    // boundary" for any multi-byte char whose size doesn't evenly divide
    // the limit.
    let body: String = "🦀".repeat(200);
    let e = rupu_providers::classify::classify_openai(400, &body, None);
    match e {
        rupu_providers::error::ProviderError::BadRequest { message } => {
            // Should produce a String that ends with the ellipsis.
            assert!(
                message.ends_with('…'),
                "expected ellipsis suffix, got: {message:?}"
            );
            // Should be <= 500 + the size of the ellipsis byte sequence.
            assert!(message.len() <= 500 + '…'.len_utf8());
        }
        other => panic!("expected BadRequest, got {other:?}"),
    }
}
