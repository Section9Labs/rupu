use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rupu_providers::model_limits::LimitSource;
use rupu_providers::types::{LlmRequest, LlmResponse, StreamEvent};
use rupu_providers::{LlmProvider, ModelCost, ModelInfo, ModelStatus, ProviderError, ProviderId};
use rupu_runtime::model_limits::{
    resolve, CatalogProvider, LimitOverrides, LimitsContext, RefreshOutcome, UnknownProvider,
};

/// A provider whose `fetch_models` result is scripted and counted.
struct Fake {
    result: Result<Vec<(&'static str, u32, u32)>, &'static str>,
    calls: Arc<AtomicUsize>,
    shares: bool,
    id: ProviderId,
    /// Sleep this long (real time) before answering. Timeout tests pair a
    /// short injected fetch timeout with a longer delay, so no test needs a
    /// paused clock (which would also have to cover real filesystem I/O).
    delay: Option<std::time::Duration>,
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
        if let Some(d) = self.delay {
            tokio::time::sleep(d).await;
        }
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
            delay: None,
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

fn declared_account(kind: &str) -> rupu_config::ProviderConfig {
    rupu_config::ProviderConfig {
        kind: Some(kind.to_string()),
        ..Default::default()
    }
}

#[test]
fn provider_names_appends_declared_accounts_after_the_builtins() {
    let mut cfg = rupu_config::Config::default();
    for (name, p) in [
        ("anthropic-work", declared_account("anthropic")),
        (
            "oracle",
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
        ),
        // Declared but not dispatchable by the LLM factory — excluded.
        ("gh-work", declared_account("github")),
        // A builtin that also has a config section must not be listed twice.
        ("anthropic", rupu_config::ProviderConfig::default()),
    ] {
        cfg.providers.insert(name.to_string(), p);
    }
    assert_eq!(
        rupu_runtime::model_limits::provider_names(&cfg),
        vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "gemini".to_string(),
            "copilot".to_string(),
            "anthropic-work".to_string(),
            "oracle".to_string(),
        ]
    );
}

/// The silent-success defect: an unresolvable name must be an Err (which the
/// CLI turns into a non-zero exit), not an empty target list that loops zero
/// times and reports nothing.
#[test]
fn an_unresolvable_name_is_an_error_naming_both_remedies() {
    let cfg = rupu_config::Config::default();
    let err = rupu_runtime::model_limits::resolve_targets(
        Some("anthropc"),
        &cfg,
        std::path::Path::new("/tmp/config.toml"),
    )
    .expect_err("a typo must not resolve");
    let msg = err.to_string();
    assert!(msg.contains("anthropc"), "names what it tried: {msg}");
    assert!(
        msg.contains("rupu auth login"),
        "remedy 1 — declare the account: {msg}"
    );
    assert!(
        msg.contains("openai-compatible"),
        "remedy 2 — declare an endpoint: {msg}"
    );
    assert!(
        msg.contains("config.toml"),
        "points at the file to edit: {msg}"
    );
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

/// `<tmp>/cache/models`: the explicit cache dir every refresh/catalog test
/// hands the library (it never reads `RUPU_CACHE_DIR_OVERRIDE` itself).
fn cache_of(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    tmp.path().join("cache/models")
}

async fn refresh_with(
    cfg: &rupu_config::Config,
    tmp: &tempfile::TempDir,
    resolver: Arc<dyn rupu_auth::CredentialResolver>,
    only: Option<&str>,
) -> Result<Vec<RefreshOutcome>, UnknownProvider> {
    refresh_with_timeout(
        cfg,
        tmp,
        resolver,
        only,
        rupu_runtime::model_limits::FETCH_TIMEOUT,
    )
    .await
}

async fn refresh_with_timeout(
    cfg: &rupu_config::Config,
    tmp: &tempfile::TempDir,
    resolver: Arc<dyn rupu_auth::CredentialResolver>,
    only: Option<&str>,
    fetch_timeout: std::time::Duration,
) -> Result<Vec<RefreshOutcome>, UnknownProvider> {
    rupu_runtime::model_limits::refresh(
        cfg,
        &cache_of(tmp),
        &tmp.path().join("config.toml"),
        resolver,
        only,
        fetch_timeout,
    )
    .await
}

async fn catalog_of(
    cfg: &rupu_config::Config,
    tmp: &tempfile::TempDir,
    only: Option<&str>,
) -> Result<Vec<CatalogProvider>, UnknownProvider> {
    rupu_runtime::model_limits::catalog(cfg, &cache_of(tmp), &tmp.path().join("config.toml"), only)
        .await
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

    let out = refresh_with(&cfg, &tmp, Arc::new(AnyKey), Some("oracle"))
        .await
        .unwrap();
    listing.assert();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{:?}", out[0]);
    assert_eq!((out[0].provider.as_str(), out[0].count), ("oracle", 1));
    assert!(out[0].error.is_none());

    let cat = catalog_of(&cfg, &tmp, Some("oracle")).await.unwrap();
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

    let out = refresh_with(&cfg, &tmp, Arc::new(AnyKey), Some("oracle"))
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
    let err = refresh_with(&cfg, &tmp, Arc::new(AnyKey), Some("nope"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown provider 'nope'"));
    let err = catalog_of(&cfg, &tmp, Some("nope")).await.unwrap_err();
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
    let cat = catalog_of(&cfg, &tmp, Some("anthropic")).await.unwrap();
    assert!(cat[0].fetched_at.is_none());
    assert!(!cat[0].stale, "never fetched is not stale");
    let by_id = |id: &str| cat[0].models.iter().find(|m| m.id == id).unwrap();
    assert_eq!(by_id("claude-pinned").input_tokens, Some(123_000));
    assert_eq!(by_id("claude-pinned").output_tokens, None);
    assert_eq!(by_id("claude-pinned").source, "custom");
    assert_eq!(by_id("claude-bare").input_tokens, None);
}

// ---- resolve precedence table (spec §10) ------------------------------------

fn custom_ctx(
    tmp: &tempfile::TempDir,
    context_window: Option<u32>,
    max_output: Option<u32>,
) -> LimitsContext {
    let mut c = ctx(tmp);
    c.custom.insert(
        "anthropic".into(),
        vec![rupu_config::CustomModel {
            id: "claude-a".into(),
            context_window,
            max_output,
        }],
    );
    c
}

#[tokio::test]
async fn config_zero_falls_through_to_live() {
    let tmp = tempfile::tempdir().unwrap();
    let c = custom_ctx(&tmp, Some(0), Some(0));
    let (mut p, _) = fake(Ok(vec![("claude-a", 1_000_000, 128_000)]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &c,
    )
    .await;
    assert_eq!(l.input.tokens, Some(1_000_000));
    assert!(matches!(l.input.source, LimitSource::Live { .. }));
    assert_eq!(l.output.tokens, Some(128_000));
    assert!(matches!(l.output.source, LimitSource::Live { .. }));
}

#[tokio::test]
async fn agent_beats_config_on_the_same_field() {
    let tmp = tempfile::tempdir().unwrap();
    let c = custom_ctx(&tmp, Some(500_000), Some(32_000));
    let (mut p, _) = fake(Ok(vec![("claude-a", 1_000_000, 128_000)]));
    let o = LimitOverrides {
        context_window_tokens: Some(300_000),
        max_tokens: Some(4096),
        compact_at_percent: None,
    };
    let l = resolve(o, "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!(
        (l.input.tokens, &l.input.source),
        (Some(300_000), &LimitSource::Agent)
    );
    assert_eq!(
        (l.output.tokens, &l.output.source),
        (Some(4096), &LimitSource::Agent)
    );
}

#[tokio::test]
async fn live_zero_is_unknown_not_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![("claude-a", 0, 0)]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(
        (l.input.tokens, &l.input.source),
        (None, &LimitSource::Unknown)
    );
    assert_eq!(
        (l.output.tokens, &l.output.source),
        (None, &LimitSource::Unknown)
    );
}

#[tokio::test]
async fn compact_percent_defaults_to_80_and_clamps_to_10_95() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![("claude-a", 1_000, 1_000)]));
    let pct = |o: Option<u8>| LimitOverrides {
        compact_at_percent: o,
        ..LimitOverrides::default()
    };
    let c = ctx(&tmp);
    let l = resolve(pct(None), "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!(l.compact_at_percent, 80);
    let l = resolve(pct(Some(99)), "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!(l.compact_at_percent, 95);
    let l = resolve(pct(Some(3)), "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!(l.compact_at_percent, 10);
    let l = resolve(pct(Some(60)), "anthropic", "claude-a", &mut p, &c).await;
    assert_eq!(l.compact_at_percent, 60);
}

#[tokio::test]
async fn fetch_error_with_no_cache_is_unknown_with_a_note() {
    let tmp = tempfile::tempdir().unwrap();
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
    assert_eq!(
        (l.input.tokens, &l.input.source),
        (None, &LimitSource::Unknown)
    );
    assert_eq!(
        (l.output.tokens, &l.output.source),
        (None, &LimitSource::Unknown)
    );
    let note = l.note.as_deref().unwrap();
    assert!(note.contains("refresh failed"), "{note}");
    assert!(note.contains("connection refused"), "{note}");
}

#[tokio::test]
async fn slow_fetch_times_out_to_unknown_with_a_note() {
    let tmp = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut p = Fake {
        result: Ok(vec![("claude-a", 1_000_000, 128_000)]),
        calls: calls.clone(),
        shares: true,
        id: ProviderId::Anthropic,
        delay: Some(std::time::Duration::from_secs(1)),
    };
    let mut c = ctx(&tmp);
    c.fetch_timeout = std::time::Duration::from_millis(50);
    let started = std::time::Instant::now();
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &c,
    )
    .await;
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the injected timeout, not the fetch, ends the wait"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        (l.input.tokens, &l.input.source),
        (None, &LimitSource::Unknown)
    );
    let note = l.note.as_deref().unwrap();
    assert!(note.contains("timed out after 50ms"), "{note}");
    // The late answer was dropped, never cached.
    assert!(!tmp.path().join("cache/models/anthropic.json").exists());
}

#[tokio::test]
async fn non_sharing_non_anthropic_provider_reports_its_own_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let mut p = Fake {
        result: Ok(vec![("m-x", 131_072, 8_192)]),
        calls: Arc::new(AtomicUsize::new(0)),
        shares: false,
        id: ProviderId::OpenaiCompatible,
        delay: None,
    };
    let l = resolve(
        LimitOverrides::default(),
        "oracle",
        "m-x",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert!(!l.output_shares_context);
    assert_eq!(l.output_fallback, None);
    assert_eq!(l.input.tokens, Some(131_072));
}

// ---- refresh bound + catalog merge -------------------------------------------

/// A credential lookup that outlasts `FETCH_TIMEOUT` by a wide margin, then
/// fails. The build step (not the fetch) is what hangs here.
struct SlowResolver;

#[async_trait::async_trait]
impl rupu_auth::CredentialResolver for SlowResolver {
    async fn get(
        &self,
        _provider: &str,
        _hint: Option<rupu_providers::AuthMode>,
    ) -> anyhow::Result<(
        rupu_providers::AuthMode,
        rupu_providers::auth::AuthCredentials,
    )> {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        anyhow::bail!("credential store answered too late")
    }
    async fn refresh(
        &self,
        _provider: &str,
        _mode: rupu_providers::AuthMode,
    ) -> anyhow::Result<rupu_providers::auth::AuthCredentials> {
        unreachable!()
    }
}

#[tokio::test]
async fn refresh_bounds_the_whole_provider_job_not_just_the_fetch() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = rupu_config::Config::default();
    let started = std::time::Instant::now();
    let out = refresh_with_timeout(
        &cfg,
        &tmp,
        Arc::new(SlowResolver),
        Some("anthropic"),
        std::time::Duration::from_millis(50),
    )
    .await
    .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(out[0].count, 0);
    assert_eq!(out[0].error.as_deref(), Some("timed out after 50ms"));
}

/// Production callers get the spec's 10s fetch timeout unless they inject
/// another.
#[test]
fn the_fetch_timeout_defaults_to_10s() {
    use rupu_runtime::model_limits::FETCH_TIMEOUT;
    assert_eq!(FETCH_TIMEOUT, std::time::Duration::from_secs(10));
    assert_eq!(LimitsContext::default().fetch_timeout, FETCH_TIMEOUT);
    assert_eq!(
        LimitsContext::for_cache_dir("/x".into()).fetch_timeout,
        FETCH_TIMEOUT
    );
    assert_eq!(
        LimitsContext::from_config(&rupu_config::Config::default(), std::path::Path::new("/x"))
            .fetch_timeout,
        FETCH_TIMEOUT
    );
}

/// Mock an openai-compatible `/v1/models` and refresh `oracle` from it.
async fn refreshed_oracle(
    tmp: &tempfile::TempDir,
    body: serde_json::Value,
    models: Vec<rupu_config::CustomModel>,
) -> rupu_config::Config {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(body);
    });
    let mut cfg = oracle_cfg(format!("{}/v1", server.url("")));
    cfg.providers.get_mut("oracle").unwrap().models = models;
    let out = refresh_with(&cfg, tmp, Arc::new(AnyKey), Some("oracle"))
        .await
        .unwrap();
    assert!(out[0].ok, "{:?}", out[0]);
    cfg
}

#[tokio::test]
async fn catalog_fills_a_config_models_unset_limits_from_live() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = refreshed_oracle(
        &tmp,
        serde_json::json!({ "data": [{ "id": "base-model", "max_model_len": 4096 }] }),
        vec![rupu_config::CustomModel {
            id: "base-model".into(),
            context_window: None,
            max_output: None,
        }],
    )
    .await;
    let cat = catalog_of(&cfg, &tmp, Some("oracle")).await.unwrap();
    let m = cat[0].models.iter().find(|m| m.id == "base-model").unwrap();
    assert_eq!(
        m.input_tokens,
        Some(4096),
        "live fills the unset config field"
    );
    assert_eq!(m.source, "custom", "a config entry still owns the row");
    assert!(cat[0].fetched_at.is_some());
}

#[tokio::test]
async fn catalog_merges_config_and_live_per_field() {
    let tmp = tempfile::tempdir().unwrap();
    // A fresh v2 cache with both live limits set (the openai-compatible
    // listing never reports an output cap, so write the file directly).
    let dir = tmp.path().join("cache/models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("oracle.json"),
        format!(
            r#"{{"schema":2,"fetched_at":"{}","models":[{{"id":"base-model","context_window":4096,"max_output_tokens":1024}}]}}"#,
            chrono::Utc::now().to_rfc3339()
        ),
    )
    .unwrap();
    // Config pins only the window.
    let mut cfg = oracle_cfg("http://127.0.0.1:9/v1".into());
    cfg.providers.get_mut("oracle").unwrap().models = vec![rupu_config::CustomModel {
        id: "base-model".into(),
        context_window: Some(9000),
        max_output: None,
    }];
    let cat = catalog_of(&cfg, &tmp, Some("oracle")).await.unwrap();
    let m = cat[0].models.iter().find(|m| m.id == "base-model").unwrap();
    assert_eq!(m.input_tokens, Some(9000), "config wins where set");
    assert_eq!(
        m.output_tokens,
        Some(1024),
        "live fills what config left unset"
    );
    assert_eq!(m.source, "custom");
}

// ---- an empty listing is a failed refresh ----------------------------------

const GOOD_CACHE: &str = r#"{"schema":2,"fetched_at":"2020-01-01T00:00:00Z","models":[{"id":"claude-a","context_window":200000,"max_output_tokens":64000}]}"#;

/// A listing that parses to zero models must not replace a good cache: the
/// resolve keeps the prior entry (marked stale) and says why the refresh
/// didn't take.
#[tokio::test]
async fn resolve_treats_an_empty_listing_as_a_failed_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cache/models");
    std::fs::create_dir_all(&dir).unwrap();
    let cache_file = dir.join("anthropic.json");
    std::fs::write(&cache_file, GOOD_CACHE).unwrap();
    let (mut p, calls) = fake(Ok(vec![]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the stale cache refetched");
    assert_eq!(
        std::fs::read_to_string(&cache_file).unwrap(),
        GOOD_CACHE,
        "an empty listing must not overwrite the cache file"
    );
    assert_eq!(l.input.tokens, Some(200_000));
    assert_eq!(l.output.tokens, Some(64_000));
    assert!(
        matches!(l.input.source, LimitSource::Live { stale: true, .. }),
        "{:?}",
        l.input.source
    );
    assert!(
        l.note
            .as_deref()
            .unwrap()
            .contains("model list refresh returned no models"),
        "{:?}",
        l.note
    );
}

/// With no prior cache an empty listing writes nothing either — there is no
/// "empty but fresh" cache to read back and trust for an hour.
#[tokio::test]
async fn resolve_writes_no_cache_for_an_empty_listing() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut p, _) = fake(Ok(vec![]));
    let l = resolve(
        LimitOverrides::default(),
        "anthropic",
        "claude-a",
        &mut p,
        &ctx(&tmp),
    )
    .await;
    assert!(!tmp.path().join("cache/models/anthropic.json").exists());
    assert_eq!(l.input.tokens, None);
    assert!(l
        .note
        .as_deref()
        .unwrap()
        .contains("model list refresh returned no models"));
}

/// `refresh` reports an empty listing as a failure and leaves the cache file
/// byte-identical.
#[tokio::test]
async fn refresh_reports_an_empty_listing_and_keeps_the_cache() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200)
            .json_body(serde_json::json!({ "data": [] }));
    });
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cache/models");
    std::fs::create_dir_all(&dir).unwrap();
    let cache_file = dir.join("oracle.json");
    let good = r#"{"schema":2,"fetched_at":"2020-01-01T00:00:00Z","models":[{"id":"base-model","context_window":4096,"max_output_tokens":0}]}"#;
    std::fs::write(&cache_file, good).unwrap();
    let cfg = oracle_cfg(format!("{}/v1", server.url("")));

    let out = refresh_with(&cfg, &tmp, Arc::new(AnyKey), Some("oracle"))
        .await
        .unwrap();
    assert!(!out[0].ok, "{:?}", out[0]);
    assert_eq!(out[0].count, 0);
    assert_eq!(out[0].error.as_deref(), Some("provider returned no models"));
    assert_eq!(std::fs::read_to_string(&cache_file).unwrap(), good);
}

/// The library never reads `RUPU_CACHE_DIR_OVERRIDE`: `refresh` and `catalog`
/// use exactly the cache dir they are handed, so a test's result can't depend
/// on the environment (the CLI and the CP resolve that seam at the edge).
#[tokio::test]
async fn refresh_and_catalog_use_the_cache_dir_they_are_given() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{ "id": "base-model", "max_model_len": 4096 }]
        }));
    });
    let tmp = tempfile::tempdir().unwrap();
    let elsewhere = tmp.path().join("elsewhere");
    let cfg = oracle_cfg(format!("{}/v1", server.url("")));
    std::env::set_var("RUPU_CACHE_DIR_OVERRIDE", &elsewhere);
    let out = refresh_with(&cfg, &tmp, Arc::new(AnyKey), Some("oracle")).await;
    let cat = catalog_of(&cfg, &tmp, Some("oracle")).await;
    std::env::remove_var("RUPU_CACHE_DIR_OVERRIDE");
    assert!(out.unwrap()[0].ok);
    assert!(cache_of(&tmp).join("oracle.json").exists());
    assert!(!elsewhere.exists(), "the env seam is the caller's business");
    assert_eq!(cat.unwrap()[0].models[0].input_tokens, Some(4096));
}

/// A provider job that outlives the refresh timeout is reported as timed out
/// but NOT cancelled: it may be mid-way through an OAuth token refresh whose
/// rotated token must still be persisted. It finishes in the background —
/// observable here as the cache file it writes after the call returned.
#[tokio::test]
async fn a_timed_out_refresh_job_still_finishes_in_the_background() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/v1/models");
        then.status(200)
            .delay(std::time::Duration::from_millis(400))
            .json_body(serde_json::json!({
                "data": [{ "id": "base-model", "max_model_len": 4096 }]
            }));
    });
    let tmp = tempfile::tempdir().unwrap();
    let cfg = oracle_cfg(format!("{}/v1", server.url("")));
    let out = refresh_with_timeout(
        &cfg,
        &tmp,
        Arc::new(AnyKey),
        Some("oracle"),
        std::time::Duration::from_millis(100),
    )
    .await
    .unwrap();
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some("timed out after 100ms"));
    let cache_file = cache_of(&tmp).join("oracle.json");
    assert!(
        !cache_file.exists(),
        "not written yet when the call returns"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !cache_file.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let body = std::fs::read_to_string(&cache_file).expect("the job finished and wrote its cache");
    assert!(body.contains("base-model"), "{body}");
}
