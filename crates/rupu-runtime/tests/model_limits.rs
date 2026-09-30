use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rupu_providers::model_limits::LimitSource;
use rupu_providers::types::{LlmRequest, LlmResponse, StreamEvent};
use rupu_providers::{LlmProvider, ModelCost, ModelInfo, ModelStatus, ProviderError, ProviderId};
use rupu_runtime::model_limits::{resolve, LimitOverrides, LimitsContext};

/// A provider whose `fetch_models` result is scripted and counted.
struct Fake {
    result: Result<Vec<(&'static str, u32, u32)>, &'static str>,
    calls: Arc<AtomicUsize>,
    shares: bool,
    id: ProviderId,
}

#[async_trait::async_trait]
impl LlmProvider for Fake {
    async fn send(&mut self, _: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        unreachable!()
    }
    async fn stream(
        &mut self,
        _: &LlmRequest,
        _: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        unreachable!()
    }
    fn default_model(&self) -> &str {
        "m"
    }
    fn provider_id(&self) -> ProviderId {
        self.id
    }
    async fn fetch_models(&mut self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.result {
            Ok(ms) => Ok(ms
                .iter()
                .map(|(id, cw, mo)| ModelInfo {
                    id: id.to_string(),
                    provider: self.id,
                    context_window: *cw,
                    max_output_tokens: *mo,
                    capabilities: vec![],
                    cost: ModelCost::default(),
                    status: ModelStatus::default(),
                })
                .collect()),
            Err("not-implemented") => Err(ProviderError::NotImplemented {
                provider: "fake".into(),
            }),
            Err(e) => Err(ProviderError::Http(e.to_string())),
        }
    }
    fn output_shares_context(&self) -> bool {
        self.shares
    }
}

fn fake(result: Result<Vec<(&'static str, u32, u32)>, &'static str>) -> (Fake, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Fake {
            result,
            calls: calls.clone(),
            shares: true,
            id: ProviderId::Anthropic,
        },
        calls,
    )
}

fn ctx(tmp: &tempfile::TempDir) -> LimitsContext {
    LimitsContext::for_cache_dir(tmp.path().join("cache/models"))
}

#[tokio::test]
async fn live_limits_are_used_and_cached() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, calls) = fake(Ok(vec![("claude-a", 1_000_000, 128_000)]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(l.input.tokens, Some(1_000_000));
    assert_eq!(l.output.tokens, Some(128_000));
    assert!(matches!(
        l.input.source,
        LimitSource::Live { stale: false, .. }
    ));
    // Second resolve reads the fresh cache: no refetch.
    let l2 = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(l2.input.tokens, Some(1_000_000));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn agent_beats_config_beats_live_per_field() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = ctx(&tmp);
    c.custom.insert(
        "anthropic".into(),
        vec![rupu_config::CustomModel {
            id: "claude-a".into(),
            context_window: Some(500_000),
            max_output: None,
        }],
    );
    let (mut p, _) = fake(Ok(vec![("claude-a", 1_000_000, 128_000)]));
    let o = LimitOverrides {
        context_window_tokens: None,
        max_tokens: Some(4096),
        compact_at_percent: Some(60),
    };
    let l = resolve(o, "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!(
        (l.input.tokens, &l.input.source),
        (Some(500_000), &LimitSource::Config)
    );
    assert_eq!(
        (l.output.tokens, &l.output.source),
        (Some(4096), &LimitSource::Agent)
    );
    assert_eq!(l.compact_at_percent, 60);
}

#[tokio::test]
async fn failed_refresh_falls_back_to_stale_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cache/models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("anthropic.json"),
        r#"{"schema":2,"fetched_at":"2020-01-01T00:00:00Z","models":[{"id":"claude-a","context_window":200000,"max_output_tokens":64000}]}"#,
    )
    .unwrap();
    let (mut p, calls) = fake(Err("connection refused"));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(l.input.tokens, Some(200_000));
    assert!(matches!(
        l.input.source,
        LimitSource::Live { stale: true, .. }
    ));
    assert!(l.note.as_deref().unwrap().contains("refresh failed"));
}

#[tokio::test]
async fn no_listing_is_unknown_with_a_note() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Err("not-implemented"));
    let l = resolve(
        LimitOverrides::default(),
        "gemini",
        "gemini-x",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(l.input.tokens, None);
    assert_eq!(l.compact_threshold(), None);
    assert!(l
        .note
        .as_deref()
        .unwrap()
        .contains("exposes no model limits"));
    assert_eq!(
        l.output_fallback,
        Some(8192),
        "fake reports ProviderId::Anthropic"
    );
}

#[tokio::test]
async fn dated_snapshot_and_1m_suffix_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![("claude-haiku-4-5-20251001", 200_000, 64_000)]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-haiku-4-5[1m]",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(l.input.tokens, Some(200_000));
    assert_eq!(l.compact_threshold(), Some(136_000));
}

#[tokio::test]
async fn model_missing_from_list_is_noted() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![("claude-a", 1, 1)]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-zzz",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(l.input.tokens, None);
    assert!(l
        .note
        .as_deref()
        .unwrap()
        .contains("not in anthropic's model list"));
}

#[test]
fn provider_names_and_targets() {
    let cfg = rupu_config::Config::default();
    assert_eq!(
        rupu_runtime::model_limits::provider_names(&cfg),
        ["anthropic", "openai", "gemini", "copilot"]
    );
    let err = rupu_runtime::model_limits::resolve_targets(
        Some("nope"),
        &cfg,
        std::path::Path::new("/x/config.toml"),
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown provider 'nope'"));
}

// ---- refresh + catalog ----------------------------------------------------

/// Hands every account the same API key: `refresh` only needs a credential to
/// build the client, the mock server never checks it.
struct AnyKey;

#[async_trait::async_trait]
impl rupu_auth::CredentialResolver for AnyKey {
    async fn get(
        &self,
        _provider: &str,
        _hint: Option<rupu_providers::AuthMode>,
    ) -> anyhow::Result<(
        rupu_providers::AuthMode,
        rupu_providers::auth::AuthCredentials,
    )> {
        Ok((
            rupu_providers::AuthMode::ApiKey,
            rupu_providers::auth::AuthCredentials::ApiKey { key: "k".into() },
        ))
    }
    async fn refresh(
        &self,
        _provider: &str,
        _mode: rupu_providers::AuthMode,
    ) -> anyhow::Result<rupu_providers::auth::AuthCredentials> {
        unreachable!()
    }
}

fn oracle_cfg(base_url: String) -> rupu_config::Config {
    let mut cfg = rupu_config::Config::default();
    cfg.providers.insert(
        "oracle".into(),
        rupu_config::ProviderConfig {
            kind: Some("openai-compatible".into()),
            base_url: Some(base_url),
            default_model: Some("base-model".into()),
            ..Default::default()
        },
    );
    cfg
}

#[tokio::test]
async fn refresh_writes_the_cache_that_catalog_reads() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let listing = server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{ "id": "base-model", "max_model_len": 4096 }]
        }));
    });
    let tmp = tempfile::tempdir().unwrap();
    let cfg = oracle_cfg(format!("{}/v1", server.url("")));
    let cfg_path = tmp.path().join("config.toml");

    let out =
        rupu_runtime::model_limits::refresh(&cfg, tmp.path(), &cfg_path, &AnyKey, Some("oracle"))
            .await
            .unwrap();
    listing.assert();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{:?}", out[0]);
    assert_eq!((out[0].provider.as_str(), out[0].count), ("oracle", 1));
    assert!(out[0].error.is_none());

    let cat = rupu_runtime::model_limits::catalog(&cfg, tmp.path(), &cfg_path, Some("oracle"))
        .await
        .unwrap();
    assert_eq!(cat.len(), 1);
    assert!(cat[0].fetched_at.is_some());
    assert!(!cat[0].stale);
    assert_eq!(cat[0].models.len(), 1);
    let m = &cat[0].models[0];
    assert_eq!(m.id, "base-model");
    assert_eq!(m.input_tokens, Some(4096));
    assert_eq!(m.source, "live");
}

#[tokio::test]
async fn refresh_reports_a_failing_provider_without_failing_the_call() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(401);
    });
    let tmp = tempfile::tempdir().unwrap();
    let cfg = oracle_cfg(format!("{}/v1", server.url("")));
    let cfg_path = tmp.path().join("config.toml");

    let out =
        rupu_runtime::model_limits::refresh(&cfg, tmp.path(), &cfg_path, &AnyKey, Some("oracle"))
            .await
            .unwrap();
    assert!(!out[0].ok);
    assert_eq!(out[0].count, 0);
    assert!(out[0].error.as_deref().unwrap().contains("401"));
    // A failed refresh leaves no cache behind.
    assert!(!tmp.path().join("cache/models/oracle.json").exists());
}

#[tokio::test]
async fn refresh_and_catalog_reject_an_unknown_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = rupu_config::Config::default();
    let cfg_path = tmp.path().join("config.toml");
    let err =
        rupu_runtime::model_limits::refresh(&cfg, tmp.path(), &cfg_path, &AnyKey, Some("nope"))
            .await
            .unwrap_err();
    assert!(err.to_string().contains("unknown provider 'nope'"));
    let err = rupu_runtime::model_limits::catalog(&cfg, tmp.path(), &cfg_path, Some("nope"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown provider 'nope'"));
}

#[tokio::test]
async fn catalog_lists_custom_models_with_unknown_limits_as_none() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = rupu_config::Config::default();
    cfg.providers.insert(
        "anthropic".into(),
        rupu_config::ProviderConfig {
            models: vec![
                rupu_config::CustomModel {
                    id: "claude-pinned".into(),
                    context_window: Some(123_000),
                    max_output: None,
                },
                rupu_config::CustomModel {
                    id: "claude-bare".into(),
                    context_window: None,
                    max_output: None,
                },
            ],
            ..Default::default()
        },
    );
    let cfg_path = tmp.path().join("config.toml");
    let cat = rupu_runtime::model_limits::catalog(&cfg, tmp.path(), &cfg_path, Some("anthropic"))
        .await
        .unwrap();
    assert!(cat[0].fetched_at.is_none());
    assert!(!cat[0].stale, "never fetched is not stale");
    let by_id = |id: &str| cat[0].models.iter().find(|m| m.id == id).unwrap();
    assert_eq!(by_id("claude-pinned").input_tokens, Some(123_000));
    assert_eq!(by_id("claude-pinned").output_tokens, None);
    assert_eq!(by_id("claude-pinned").source, "custom");
    assert_eq!(by_id("claude-bare").input_tokens, None);
}
