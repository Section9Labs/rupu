//! Live smoke tests. Skipped silently unless RUPU_LIVE_TESTS=1 AND
//! per-provider credentials are present in the env.
//!
//! Run via: `RUPU_LIVE_TESTS=1 RUPU_LIVE_ANTHROPIC_KEY=... cargo test -p rupu-providers --test live_smoke`
//!
//! These tests are NOT run in the regular `cargo test` flow. They live
//! behind an env gate so the per-PR CI workflow stays offline; the
//! nightly workflow at `.github/workflows/nightly-live-tests.yml` runs
//! them with secrets.

use rupu_providers::auth::AuthCredentials;
use rupu_providers::types::{LlmRequest, Message};

fn live_enabled() -> bool {
    std::env::var("RUPU_LIVE_TESTS").as_deref() == Ok("1")
}

fn minimal_request(model: &str) -> LlmRequest {
    LlmRequest {
        model: model.into(),
        system: None,
        messages: vec![Message::user("Say hi.")],
        max_tokens: 64,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        context_window: None,
        task_type: None,
    }
}

#[tokio::test]
async fn anthropic_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let key = match std::env::var("RUPU_LIVE_ANTHROPIC_KEY") {
        Ok(k) => k,
        Err(_) => return,
    };
    let mut client = rupu_providers::AnthropicClient::new(key);
    let resp = client
        .send(&minimal_request("claude-haiku-4-5"))
        .await
        .expect("anthropic round-trip");
    assert!(!resp.content.is_empty(), "anthropic returned empty content");
    assert!(resp.usage.input_tokens > 0, "anthropic input_tokens == 0");
}

#[tokio::test]
async fn openai_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let key = match std::env::var("RUPU_LIVE_OPENAI_KEY") {
        Ok(k) => k,
        Err(_) => return,
    };
    let creds = AuthCredentials::ApiKey { key };
    let mut client = rupu_providers::OpenAiCodexClient::new(creds, None).expect("init");
    let resp = client
        .send(&minimal_request("gpt-4o-mini"))
        .await
        .expect("openai round-trip");
    assert!(resp.usage.output_tokens > 0, "openai output_tokens == 0");
}

#[tokio::test]
async fn copilot_live_round_trip() {
    if !live_enabled() {
        return;
    }
    let token = match std::env::var("RUPU_LIVE_COPILOT_TOKEN") {
        Ok(t) => t,
        Err(_) => return,
    };
    let creds = AuthCredentials::ApiKey { key: token };
    let mut client = rupu_providers::GithubCopilotClient::new(creds, None).expect("init");
    let resp = client
        .send(&minimal_request("gpt-4o-mini"))
        .await
        .expect("copilot round-trip");
    assert!(resp.usage.output_tokens > 0, "copilot output_tokens == 0");
}

// Gemini live test deferred until AI Studio API-key path is wired
// (see TODO.md). The Vertex/CLI OAuth path requires a project_id +
// service-account-style credential which doesn't fit the simple env-
// var-keyed pattern used here.
