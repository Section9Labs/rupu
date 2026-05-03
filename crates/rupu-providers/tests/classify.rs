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
