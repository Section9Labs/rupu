//! `RuntimeHopBuilder`: the production `HopBuilder` the launch sites hand the
//! agent runner. It builds a fallback hop's provider through the same factory
//! the primary provider uses, and resolves that hop's model limits.
//!
//! Every test here is `#[serial]`: the mock-script seam is a process env var.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rupu_agent::recovery::HopBuilder;
use rupu_auth::KeychainResolver;
use rupu_config::CustomModel;
use rupu_runtime::hop_builder::RuntimeHopBuilder;
use rupu_runtime::model_limits::LimitsContext;
use serial_test::serial;

/// Clears the env vars a test sets, even on panic.
struct EnvGuard(Vec<&'static str>);
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for k in &self.0 {
            std::env::remove_var(k);
        }
    }
}

fn builder(limits_ctx: LimitsContext) -> RuntimeHopBuilder {
    RuntimeHopBuilder {
        resolver: Arc::new(KeychainResolver::new()),
        providers: BTreeMap::new(),
        limits_ctx,
        sink: Arc::new(rupu_netflow::MemorySink::default()),
        server_side_fallback: true,
    }
}

#[tokio::test]
#[serial]
async fn an_unconfigured_provider_is_refused_before_any_build() {
    let _guard = EnvGuard(vec!["RUPU_MOCK_PROVIDER_SCRIPT"]);
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    let cache = tempfile::tempdir().unwrap();
    let b = builder(LimitsContext::for_cache_dir(cache.path().to_path_buf()));
    let err = match b.build("no-such-account", "some-model").await {
        Ok(_) => panic!("an undeclared provider must not build a hop"),
        Err(e) => e,
    };
    assert!(err.contains("not configured"), "{err}");
    assert!(err.contains("no-such-account"), "{err}");
}

#[tokio::test]
#[serial]
async fn a_configured_provider_builds_a_hop_with_resolved_limits() {
    let _guard = EnvGuard(vec!["RUPU_MOCK_PROVIDER_SCRIPT"]);
    std::env::set_var(
        "RUPU_MOCK_PROVIDER_SCRIPT",
        r#"[{"AssistantText":{"text":"ok","stop":"end_turn"}}]"#,
    );
    let cache = tempfile::tempdir().unwrap();
    let mut ctx = LimitsContext::for_cache_dir(cache.path().to_path_buf());
    ctx.custom = HashMap::from([(
        "anthropic".to_string(),
        vec![CustomModel {
            id: "fallback-model".into(),
            context_window: Some(123_000),
            max_output: Some(4_000),
        }],
    )]);
    let b = builder(ctx);
    let hop = match b.build("anthropic", "fallback-model").await {
        Ok(h) => h,
        Err(e) => panic!("hop build failed: {e}"),
    };
    assert_eq!(hop.provider_name, "anthropic");
    assert_eq!(hop.model, "fallback-model");
    assert_eq!(hop.limits.input.tokens, Some(123_000));
    assert_eq!(hop.limits.output.tokens, Some(4_000));
}

/// The mock seam's per-model form: a hop built for a listed model replays that
/// model's script, so a test can script the origin and the hop apart.
#[tokio::test]
#[serial]
async fn a_per_model_mock_script_serves_the_hops_model_its_own_turns() {
    let _guard = EnvGuard(vec!["RUPU_MOCK_PROVIDER_SCRIPT"]);
    std::env::set_var(
        "RUPU_MOCK_PROVIDER_SCRIPT",
        r#"{"turns":[{"AssistantText":{"text":"origin","stop":"end_turn"}}],
            "models":{"mock-2":[{"AssistantText":{"text":"hop","stop":"end_turn"}}]}}"#,
    );
    let cache = tempfile::tempdir().unwrap();
    let b = builder(LimitsContext::for_cache_dir(cache.path().to_path_buf()));
    let req = |model: &str| rupu_providers::types::LlmRequest {
        model: model.into(),
        system: None,
        messages: vec![rupu_providers::types::Message::user("hi")],
        max_tokens: Some(16),
        tools: vec![],
        ..Default::default()
    };
    for (model, want) in [("mock-2", "hop"), ("other", "origin")] {
        let mut hop = match b.build("anthropic", model).await {
            Ok(h) => h,
            Err(e) => panic!("hop build failed: {e}"),
        };
        let resp = hop.provider.send(&req(model)).await.unwrap();
        let text: String = resp
            .content
            .iter()
            .filter_map(|c| match c {
                rupu_providers::types::ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, want, "model {model}");
    }
}

/// `recovery_opts` is what every launch site calls: the agent's chain wins
/// over the config table, and the hop builder is always wired.
#[test]
fn recovery_opts_prefers_the_agents_chain_and_wires_a_hop_builder() {
    use rupu_config::{FallbackEntry, RecoveryConfig};
    let entry = |m: &str| FallbackEntry {
        provider: None,
        model: m.into(),
    };
    let table = RecoveryConfig {
        fallbacks: vec![entry("from-config")],
        server_side_fallback: false,
    };
    let opts = |agent: Option<&[FallbackEntry]>| {
        rupu_runtime::hop_builder::recovery_opts(
            &table,
            agent,
            Arc::new(KeychainResolver::new()),
            BTreeMap::new(),
            LimitsContext::default(),
            Arc::new(rupu_netflow::MemorySink::default()),
        )
    };
    let from_agent = opts(Some(&[entry("from-agent")]));
    assert_eq!(from_agent.chain, vec![entry("from-agent")]);
    assert!(from_agent.hop_builder.is_some());
    let from_table = opts(None);
    assert_eq!(from_table.chain, vec![entry("from-config")]);
    assert!(from_table.hop_builder.is_some());
}
