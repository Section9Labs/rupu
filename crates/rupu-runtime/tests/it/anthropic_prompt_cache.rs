//! End-to-end: the prompt-cache opt-out resolved in `build_anthropic` actually
//! reaches the wire (spec 2026-09-29 §9.4). Agent frontmatter
//! (`ProviderConfig::anthropic_prompt_cache`) wins over `[providers.<name>]
//! prompt_cache` (`ProviderTuning::prompt_cache`); both absent ⇒ on.
//!
//! `#[serial]`, because it points the process-global
//! `RUPU_ANTHROPIC_BASE_URL_OVERRIDE` seam at an httpmock server — any
//! other test in this binary touching provider env could race on it.

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::in_memory::InMemoryResolver;
use rupu_auth::stored::StoredCredential;
use rupu_providers::types::{LlmRequest, Message};
use rupu_providers::AuthMode;
use rupu_runtime::provider_factory::{build_for_provider_with_config, ProviderConfig};
use serial_test::serial;
use std::sync::Arc;

fn body_has_cache_control(req: &HttpMockRequest) -> bool {
    req.body
        .as_ref()
        .is_some_and(|b| String::from_utf8_lossy(b).contains("\"cache_control\""))
}

fn body_lacks_cache_control(req: &HttpMockRequest) -> bool {
    !body_has_cache_control(req)
}

#[tokio::test]
#[serial]
async fn prompt_cache_resolution_reaches_the_request_body() {
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let server = MockServer::start();
    let reply = serde_json::json!({
        "id": "msg_test",
        "type": "message",
        "role": "assistant",
        "model": "claude-sonnet-4-6",
        "content": [{ "type": "text", "text": "hi" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 1, "output_tokens": 1 }
    });
    let cached = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_has_cache_control);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(reply.clone());
    });
    let uncached = server.mock(|when, then| {
        when.method(POST)
            .path("/v1/messages")
            .matches(body_lacks_cache_control);
        then.status(200)
            .header("content-type", "application/json")
            .json_body(reply.clone());
    });
    std::env::set_var(
        "RUPU_ANTHROPIC_BASE_URL_OVERRIDE",
        format!("{}/v1/messages", server.url("")),
    );

    let resolver = InMemoryResolver::new();
    resolver
        .put(
            ProviderId::Anthropic,
            AuthMode::ApiKey,
            StoredCredential::api_key("sk-ant-test"),
        )
        .await;
    let request = LlmRequest {
        model: "claude-sonnet-4-6".into(),
        system: Some("You are a reviewer.".into()),
        messages: vec![Message::user("hi")],
        max_tokens: Some(16),
        ..Default::default()
    };

    // (agent frontmatter, [providers.anthropic] prompt_cache) → cached?
    let cases = [
        (None, None, true),
        (None, Some(false), false),
        (Some(false), None, false),
        (Some(true), Some(false), true),
        (Some(false), Some(true), false),
    ];
    let mut want_cached = 0;
    let mut want_uncached = 0;
    for (agent, provider, want) in cases {
        let config = ProviderConfig {
            anthropic_prompt_cache: agent,
            tuning: Some(rupu_providers::ProviderTuning {
                prompt_cache: provider,
                ..rupu_providers::ProviderTuning::for_provider("anthropic")
            }),
            ..Default::default()
        };
        let (_mode, mut p) = build_for_provider_with_config(
            "anthropic",
            "claude-sonnet-4-6",
            Some(AuthMode::ApiKey),
            &resolver,
            &config,
            Arc::new(rupu_netflow::NullSink),
        )
        .await
        .expect("anthropic client builds");
        let result = p.send(&request).await;
        assert!(
            result.is_ok(),
            "agent={agent:?} provider={provider:?}: {:?}",
            result.err()
        );
        if want {
            want_cached += 1;
        } else {
            want_uncached += 1;
        }
        assert_eq!(
            (cached.hits(), uncached.hits()),
            (want_cached, want_uncached),
            "agent={agent:?} provider={provider:?} should be cached={want}"
        );
    }

    std::env::remove_var("RUPU_ANTHROPIC_BASE_URL_OVERRIDE");
}
