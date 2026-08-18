//! Proves `provider_factory::build_for_provider_with_config` binds the
//! *caller-supplied* sink to the provider it builds, rather than dropping
//! flows on the floor or reaching for some other (non-existent, post-Task-4)
//! process-global sink.
//!
//! Placement note (deviates from the task-5 brief, which suggested
//! `crates/rupu-cli/tests/` alongside a `netflow_capture.rs` there): no such
//! file exists in `rupu-cli/tests` today, and — more importantly — `rupu-cli`
//! does not currently compile at all (`rupu-update` still calls the
//! `rupu_netflow::http::client`/`client_from` free functions Task 4 deleted;
//! that's explicitly Task 7's job, not this task's). Placing the test there
//! would mean it could never run. `rupu-runtime`'s own factory tests
//! (`crates/rupu-runtime/src/provider_factory.rs`'s `#[cfg(test)] mod tests`
//! and its `build_copilot_tests`/`build_openai_tests`/`build_gemini_tests`
//! siblings, plus this crate's `tests/provider_resolution.rs`) never drive a
//! real HTTP call either — every one of them either exercises pure resolution
//! logic or hits the `RUPU_MOCK_PROVIDER_SCRIPT` seam, which short-circuits
//! `build_for_provider_with_config` *before* any client/sink is touched. So
//! this file lives in `crates/rupu-runtime/tests/`, named `netflow_capture.rs`
//! to match the convention the sibling crates (`rupu-providers`, `rupu-auth`,
//! `rupu-cp`, `rupu-update`, `rupu-scm`) already use for this exact kind of
//! test.
//!
//! The `openai-compatible` branch is used (rather than `anthropic`) because
//! it needs nothing beyond an `ApiKey` credential and a `base_url` — no
//! OAuth bootstrap call, no bearer-vs-x-api-key branching — keeping the test
//! focused purely on "did the sink the caller passed in receive the flow."

use rupu_auth::KeychainResolver;
use rupu_runtime::provider_factory::{
    build_for_provider_with_config, OpenAiCompatibleParams, ProviderConfig,
};

/// Guarantees the env vars this test sets are cleared even on panic, so a
/// failing assertion can't leak state into a sibling test in this binary.
struct EnvGuard(Vec<&'static str>);
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for k in &self.0 {
            std::env::remove_var(k);
        }
    }
}

#[tokio::test]
async fn the_factory_binds_the_supplied_sink_to_the_built_provider() {
    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "id": "cmpl_1",
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "hi"},
                        "finish_reason": "stop"
                    }]
                }));
        })
        .await;

    // The mock-provider seam short-circuits before a real client exists.
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

    let sink = std::sync::Arc::new(rupu_netflow::MemorySink::default());

    // "netflowmock" is not a name `KeychainResolver::parse_provider`
    // recognizes, so `get` falls through to `get_named`, which resolves an
    // API key from `RUPU_NETFLOWMOCK_API_KEY` — mirrors
    // `crates/rupu-cli/tests/netflow_run.rs`'s hermetic-credential setup for
    // the same "custom openai-compatible provider" shape. A nonexistent
    // `RUPU_AUTH_FILE` path is a valid empty file-backend store, so this
    // never touches a real keychain.
    let _guard = EnvGuard(vec![
        "RUPU_AUTH_FILE",
        "RUPU_AUTH_BACKEND",
        "RUPU_NETFLOWMOCK_API_KEY",
    ]);
    let auth_dir = tempfile::tempdir().unwrap();
    std::env::set_var("RUPU_AUTH_FILE", auth_dir.path().join("auth.json"));
    std::env::set_var("RUPU_AUTH_BACKEND", "file");
    std::env::set_var("RUPU_NETFLOWMOCK_API_KEY", "sk-netflow-test");
    let resolver = KeychainResolver::new();

    let config = ProviderConfig {
        openai_compatible: Some(OpenAiCompatibleParams {
            base_url: server.base_url(),
            default_model: "mock-model".into(),
            stream: false,
            models: vec![],
        }),
        ..Default::default()
    };

    // "netflowmock" is not one of the built-in provider names, so the
    // factory falls into the `config.openai_compatible` branch — the one
    // that builds `OpenAiCompatibleClient`.
    let (_mode, mut provider) = build_for_provider_with_config(
        "netflowmock",
        "mock-model",
        None,
        &resolver,
        &config,
        sink.clone(),
    )
    .await
    .expect("build_for_provider_with_config should succeed against the mocked endpoint");

    let request = rupu_providers::types::LlmRequest {
        model: "mock-model".into(),
        system: None,
        messages: vec![rupu_providers::types::Message::user("hi")],
        max_tokens: 16,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        thinking: None,
        context_window: None,
        task_type: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
    };

    provider
        .send(&request)
        .await
        .expect("send should succeed against the mocked endpoint");

    // The decisive assertion: the flow landed in THIS sink — the one this
    // call passed in — not some other sink and not nowhere.
    let records = sink.records();
    assert_eq!(
        records.len(),
        1,
        "expected exactly one flow recorded in the caller-supplied sink"
    );
    assert_eq!(records[0].path, "/v1/chat/completions");
}
