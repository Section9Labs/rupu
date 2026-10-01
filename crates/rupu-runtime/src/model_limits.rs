//! Model-limit discovery + resolution (spec
//! `docs/superpowers/specs/2026-09-30-rupu-model-limits-discovery-design.md`).
//! The CLI (`rupu models`), the CP (`ModelCatalog` port) and every run
//! launch share this module.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rupu_providers::model_limits::{
    Limit, LimitSource, ModelLimits, ANTHROPIC_FALLBACK_MAX_TOKENS, DEFAULT_COMPACT_AT_PERCENT,
};
use rupu_providers::model_registry::match_model_id;
use rupu_providers::{LlmProvider, ModelRegistry, ModelSource, ProviderError, ProviderId};
use serde::Serialize;

use crate::provider_factory;

/// Model-list fetch timeout (spec §3).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// What `resolve` needs from config. Owned, so long-lived holders (the step
/// factory, the dispatcher) keep it without the whole `Config`.
#[derive(Debug, Clone)]
pub struct LimitsContext {
    pub cache_dir: PathBuf,
    /// `[providers.<name>].models`, keyed by provider/account name.
    pub custom: HashMap<String, Vec<rupu_config::CustomModel>>,
    /// Bound on a stale-cache refetch. [`FETCH_TIMEOUT`] everywhere but tests,
    /// which inject a short one instead of pausing the clock.
    pub fetch_timeout: Duration,
}

impl Default for LimitsContext {
    fn default() -> Self {
        Self::for_cache_dir(PathBuf::new())
    }
}

impl LimitsContext {
    pub fn from_config(cfg: &rupu_config::Config, global_dir: &Path) -> Self {
        Self {
            custom: cfg
                .providers
                .iter()
                .filter(|(_, p)| !p.models.is_empty())
                .map(|(n, p)| (n.clone(), p.models.clone()))
                .collect(),
            ..Self::for_cache_dir(cache_dir(global_dir))
        }
    }

    pub fn for_cache_dir(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir,
            custom: HashMap::new(),
            fetch_timeout: FETCH_TIMEOUT,
        }
    }
}

/// `10s`, or `50ms` for a sub-second (test-injected) timeout.
fn fmt_timeout(d: Duration) -> String {
    if d.subsec_nanos() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// `<global>/cache/models`, or `$RUPU_CACHE_DIR_OVERRIDE` (the existing
/// `rupu models` test seam). Read at the edge only — the CLI, the CP adapter
/// and [`LimitsContext::from_config`] call it; [`refresh`] and [`catalog`]
/// take the resulting directory explicitly, so the library's behavior never
/// depends on the environment.
pub fn cache_dir(global_dir: &Path) -> PathBuf {
    std::env::var("RUPU_CACHE_DIR_OVERRIDE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| global_dir.join("cache/models"))
}

/// Agent-frontmatter pins.
#[derive(Debug, Clone, Copy, Default)]
pub struct LimitOverrides {
    pub context_window_tokens: Option<u32>,
    pub max_tokens: Option<u32>,
    pub compact_at_percent: Option<u8>,
}

impl LimitOverrides {
    pub fn from_spec(spec: &rupu_agent::AgentSpec) -> Self {
        Self {
            context_window_tokens: spec.context_window_tokens,
            max_tokens: spec.max_tokens,
            compact_at_percent: spec.compact_at_percent,
        }
    }
}

/// Resolve the limits one run uses (spec §5). Precedence per field:
/// agent → config → live cache (refetched through `provider` when stale)
/// → unknown. Never errors: unknown is a valid, reported outcome.
pub async fn resolve(
    overrides: LimitOverrides,
    provider_name: &str,
    model: &str,
    provider: &mut dyn LlmProvider,
    ctx: &LimitsContext,
) -> ModelLimits {
    let registry = ModelRegistry::with_cache_dir(&ctx.cache_dir);
    if let Err(e) = registry.load_cache(provider_name).await {
        tracing::warn!(error = %e, provider = provider_name, "unreadable model cache; refetching");
    }
    let mut note: Option<String> = None;
    if registry.cache_is_stale(provider_name).await {
        match tokio::time::timeout(ctx.fetch_timeout, provider.fetch_models()).await {
            // A listing that parsed to zero models is a failed refresh, not an
            // answer: caching it would replace a good entry (and make every
            // model "missing from the list" for an hour).
            Ok(Ok(models)) if models.is_empty() => {
                note = Some("model list refresh returned no models".to_string());
            }
            Ok(Ok(models)) => {
                registry.set_live_cache(provider_name, models).await;
                if let Err(e) = registry.save_cache(provider_name).await {
                    tracing::warn!(error = %e, provider = provider_name, "failed to write model cache");
                }
            }
            Ok(Err(ProviderError::NotImplemented { .. })) => {
                note = Some(format!(
                    "{provider_name} exposes no model limits; set contextWindowTokens/maxTokens on the agent or [[providers.{provider_name}.models]]"
                ));
            }
            Ok(Err(e)) => note = Some(format!("model list refresh failed: {e}")),
            Err(_) => {
                note = Some(format!(
                    "model list refresh timed out after {}",
                    fmt_timeout(ctx.fetch_timeout)
                ))
            }
        }
    }
    let fetched_at = registry.fetched_at(provider_name).await;
    let stale = registry.cache_is_stale(provider_name).await;
    let live = registry.find_live(provider_name, model).await;
    let custom = ctx.custom.get(provider_name).and_then(|ms| {
        let id = match_model_id(ms.iter().map(|m| m.id.as_str()), model)?;
        ms.iter().find(|m| m.id == id)
    });
    if note.is_none() && live.is_none() && fetched_at.is_some() && custom.is_none() {
        note = Some(format!(
            "model '{model}' is not in {provider_name}'s model list"
        ));
    }
    let pick = |pin: Option<u32>, cfg: Option<u32>, live: Option<u32>| -> Limit {
        if let Some(n) = pin {
            return Limit::new(n, LimitSource::Agent);
        }
        if let Some(n) = cfg.filter(|n| *n > 0) {
            return Limit::new(n, LimitSource::Config);
        }
        match (live.filter(|n| *n > 0), fetched_at) {
            (Some(n), Some(fetched_at)) => Limit::new(n, LimitSource::Live { fetched_at, stale }),
            _ => Limit::unknown(),
        }
    };
    ModelLimits {
        input: pick(
            overrides.context_window_tokens,
            custom.and_then(|c| c.context_window),
            live.as_ref().map(|m| m.context_window),
        ),
        output: pick(
            overrides.max_tokens,
            custom.and_then(|c| c.max_output),
            live.as_ref().map(|m| m.max_output_tokens),
        ),
        compact_at_percent: overrides
            .compact_at_percent
            .unwrap_or(DEFAULT_COMPACT_AT_PERCENT)
            .clamp(10, 95),
        output_shares_context: provider.output_shares_context(),
        output_fallback: (provider.provider_id() == ProviderId::Anthropic)
            .then_some(ANTHROPIC_FALLBACK_MAX_TOKENS),
        note,
    }
}

/// The built-in vendor names, in the order they are listed/refreshed.
///
/// Deliberately NOT the set of names `rupu models` accepts — that is
/// [`provider_names`], which appends every declared `[providers.<name>]`
/// account. Treating this array as the accepted set is what made
/// `--provider <account>` a silent no-op: the filter was compared against
/// these four strings, so a declared account matched no iteration and the
/// loop body never ran.
pub const BUILTIN_PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

/// Every provider name the model commands operate on: the built-in vendors,
/// then every declared `[providers.<name>]` account that resolves to a
/// dispatchable kind (`provider_factory::is_dispatchable_provider` — the
/// same predicate `rupu run`'s pre-flight gate uses, so the two cannot
/// drift). Built-ins keep their historical position so table output is
/// stable; accounts follow in config order, duplicates skipped.
pub fn provider_names(cfg: &rupu_config::Config) -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_PROVIDERS.iter().map(|s| (*s).to_string()).collect();
    for name in cfg.providers.keys() {
        if !names.contains(name) && provider_factory::is_dispatchable_provider(name, &cfg.providers)
        {
            names.push(name.clone());
        }
    }
    names
}

/// `--provider <name>` named something that is neither a built-in vendor nor
/// a declared dispatchable account.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct UnknownProvider(pub String);

/// Resolve `--provider <name>` into the list of names to act on.
///
/// `None` means "all of them". A name that resolves to nothing is a hard
/// error: previously it matched no loop iteration and the command exited 0
/// having printed nothing, so a typo was indistinguishable from success.
pub fn resolve_targets(
    filter: Option<&str>,
    cfg: &rupu_config::Config,
    cfg_path: &Path,
) -> Result<Vec<String>, UnknownProvider> {
    let Some(only) = filter else {
        return Ok(provider_names(cfg));
    };
    if !provider_factory::is_dispatchable_provider(only, &cfg.providers) {
        return Err(UnknownProvider(format!(
            "unknown provider '{only}': it is not a built-in vendor name \
             (anthropic | openai | gemini | copilot), and no [providers.{only}] in {} \
             declares its vendor kind. Declare the account with `rupu auth login \
             --account {only} --kind <vendor>`, or declare an openai-compatible \
             endpoint as [providers.{only}] with kind = \"openai-compatible\" and a \
             base_url.",
            cfg_path.display()
        )));
    }
    Ok(vec![only.to_string()])
}

/// The resolved vendor kind for a provider name, falling back to the name
/// itself (which for an undeclared built-in IS its own kind).
fn kind_of(name: &str, cfg: &rupu_config::Config) -> String {
    provider_factory::resolve_kind(name, &cfg.providers).unwrap_or_else(|| name.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct RefreshOutcome {
    pub provider: String,
    pub ok: bool,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Refetch live model lists (spec §8). Providers run in parallel, each
/// bounded by `fetch_timeout` ([`FETCH_TIMEOUT`] outside tests) as a whole
/// (client build, credential resolution and any token refresh included, not
/// just the HTTP listing); one failing or hanging never fails the others.
///
/// Each provider's job is its own spawned task, and the timeout bounds only
/// the WAIT for it: a job that outlives it is reported as timed out but runs
/// to completion in the background. Cancelling it could abandon an OAuth/SSO
/// token refresh after the provider already rotated the refresh token,
/// before the new one was persisted. (A one-shot `rupu models refresh`
/// process still ends any such job when it exits; the long-lived `cp serve`
/// lets it finish.)
pub async fn refresh(
    cfg: &rupu_config::Config,
    cache_dir: &Path,
    cfg_path: &Path,
    resolver: Arc<dyn rupu_auth::CredentialResolver>,
    only: Option<&str>,
    fetch_timeout: Duration,
) -> Result<Vec<RefreshOutcome>, UnknownProvider> {
    let names = resolve_targets(only, cfg, cfg_path)?;
    let cfg = Arc::new(cfg.clone());
    let jobs = names.into_iter().map(|name| {
        let job = tokio::spawn({
            let (name, cfg, resolver) = (name.clone(), Arc::clone(&cfg), Arc::clone(&resolver));
            let registry = ModelRegistry::with_cache_dir(cache_dir);
            async move { refresh_one(&name, &cfg, resolver.as_ref(), &registry).await }
        });
        async move {
            let fail = |error: String| RefreshOutcome {
                provider: name.clone(),
                ok: false,
                count: 0,
                error: Some(error),
            };
            match tokio::time::timeout(fetch_timeout, job).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(e)) => fail(format!("refresh task failed: {e}")),
                // Dropping a `JoinHandle` detaches the task; it is not aborted.
                Err(_) => fail(format!("timed out after {}", fmt_timeout(fetch_timeout))),
            }
        }
    });
    Ok(futures_util::future::join_all(jobs).await)
}

/// One provider's refresh: build the client, fetch, write the cache. The
/// caller bounds its wait with the fetch timeout but never cancels it; the
/// cache write is an atomic rename either way.
async fn refresh_one(
    name: &str,
    cfg: &rupu_config::Config,
    resolver: &dyn rupu_auth::CredentialResolver,
    registry: &ModelRegistry,
) -> RefreshOutcome {
    let fail = |error: String| RefreshOutcome {
        provider: name.to_string(),
        ok: false,
        count: 0,
        error: Some(error),
    };
    // Kind first: a kind with no listing wired fails here, before any
    // credential lookup could misreport it as a missing credential.
    if kind_of(name, cfg) == "local" {
        return fail("provider kind \"local\" is not wired for listing".to_string());
    }
    let pcfg = provider_factory::provider_config_for(name, &cfg.providers);
    let model = provider_factory::resolve_model(
        None,
        cfg.default_model.as_deref(),
        pcfg.openai_compatible
            .as_ref()
            .map(|p| p.default_model.as_str()),
    );
    let mut provider = match provider_factory::build_for_provider_with_config(
        name,
        &model,
        None,
        resolver,
        &pcfg,
        Arc::new(rupu_netflow::NullSink),
    )
    .await
    {
        Ok((_, p)) => p,
        Err(provider_factory::FactoryError::NotWiredInV0(k)) => {
            return fail(format!("provider kind \"{k}\" is not wired for listing"))
        }
        Err(e) => return fail(e.to_string()),
    };
    match provider.fetch_models().await {
        // An empty listing is a failed refresh: never replace a good cache.
        Ok(models) if models.is_empty() => fail("provider returned no models".to_string()),
        Ok(models) => {
            let count = models.len();
            registry.set_live_cache(name, models).await;
            if let Err(e) = registry.save_cache(name).await {
                return fail(format!(
                    "fetched {count} models but could not write the cache: {e}"
                ));
            }
            RefreshOutcome {
                provider: name.to_string(),
                ok: true,
                count,
                error: None,
            }
        }
        Err(ProviderError::NotImplemented { .. }) => fail(format!(
            "no live model-list endpoint — declare limits in [[providers.{name}.models]]"
        )),
        Err(e) => fail(e.to_string()),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogModel {
    pub id: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    /// `live` | `custom` | `baked-in`
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogProvider {
    pub provider: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub stale: bool,
    pub models: Vec<CatalogModel>,
}

/// The cached catalog, with no network (spec §8).
pub async fn catalog(
    cfg: &rupu_config::Config,
    cache_dir: &Path,
    cfg_path: &Path,
    only: Option<&str>,
) -> Result<Vec<CatalogProvider>, UnknownProvider> {
    let names = resolve_targets(only, cfg, cfg_path)?;
    let registry = build_registry(cfg, cache_dir).await;
    let mut out = Vec::new();
    for p in names {
        let fetched_at = registry.fetched_at(&p).await;
        let stale = fetched_at.is_some() && registry.cache_is_stale(&p).await;
        let mut models = Vec::new();
        for m in registry.list(&p).await {
            // `list` lets a config entry replace the live one wholesale, so
            // its unset fields read 0. `resolve` fills those per field from
            // the live cache; so does the catalog, or the two would disagree
            // about the same model. The row stays `custom`: config owns it.
            let live = match m.source {
                ModelSource::Custom => registry.find_live(&p, &m.entry.id).await,
                _ => None,
            };
            let pick = |own: u32, live: Option<u32>| {
                Some(own).filter(|n| *n > 0).or(live.filter(|n| *n > 0))
            };
            models.push(CatalogModel {
                input_tokens: pick(
                    m.entry.context_window,
                    live.as_ref().map(|l| l.context_window),
                ),
                output_tokens: pick(
                    m.entry.max_output_tokens,
                    live.as_ref().map(|l| l.max_output_tokens),
                ),
                source: match m.source {
                    ModelSource::Custom => "custom",
                    ModelSource::Live => "live",
                    ModelSource::BakedIn => "baked-in",
                }
                .to_string(),
                id: m.entry.id,
            });
        }
        out.push(CatalogProvider {
            provider: p,
            fetched_at,
            stale,
            models,
        });
    }
    Ok(out)
}

async fn build_registry(cfg: &rupu_config::Config, cache_dir: &Path) -> ModelRegistry {
    let registry = ModelRegistry::with_cache_dir(cache_dir);

    // Baked-in id fallbacks: Copilot, and Gemini's Code Assist (OAuth) path,
    // which has no model listing (an AI Studio API key lists live).
    registry
        .set_baked_in(
            "copilot",
            ["gpt-4o", "gpt-4o-mini", "claude-sonnet-4", "o4-mini"]
                .iter()
                .map(|id| make_model_info(id, "copilot"))
                .collect(),
        )
        .await;
    registry
        .set_baked_in(
            "gemini",
            ["gemini-2.5-pro", "gemini-2.5-flash", "gemini-1.5-pro"]
                .iter()
                .map(|id| make_model_info(id, "gemini"))
                .collect(),
        )
        .await;

    // Load custom models from config.toml. Keyed by ACCOUNT name (that is
    // how `[providers.<name>].models` is written and how `list` reads it
    // back), but tagged with the account's resolved vendor KIND.
    for (name, pcfg) in &cfg.providers {
        if pcfg.models.is_empty() {
            continue;
        }
        let kind = kind_of(name, cfg);
        registry
            .set_custom(
                name,
                pcfg.models
                    .iter()
                    .map(|m| {
                        let mut mi = make_model_info(&m.id, &kind);
                        if let Some(cw) = m.context_window {
                            mi.context_window = cw;
                        }
                        if let Some(mo) = m.max_output {
                            mi.max_output_tokens = mo;
                        }
                        mi
                    })
                    .collect(),
            )
            .await;
    }

    // Load any persisted live caches — over the same name set `refresh`
    // writes, or a named account's freshly-refreshed cache would be written
    // and then never read back.
    for p in &provider_names(cfg) {
        registry.load_cache(p).await.ok();
    }
    registry
}

/// Build a `ModelInfo` tagged with the vendor for `kind`.
///
/// Takes the resolved KIND, not the account name — `make_model_info(id,
/// "oracle")` used to fall through to the `_` arm and label every custom
/// openai-compatible model as `Anthropic`.
fn make_model_info(id: &str, kind: &str) -> rupu_providers::ModelInfo {
    let pid = match kind {
        "anthropic" => ProviderId::Anthropic,
        "openai" | "openai_codex" | "codex" => ProviderId::OpenaiCodex,
        "gemini" | "google_gemini" => ProviderId::GoogleGeminiCli,
        "copilot" | "github_copilot" => ProviderId::GithubCopilot,
        "openai-compatible" => ProviderId::OpenaiCompatible,
        _ => ProviderId::Anthropic,
    };
    rupu_providers::ModelInfo {
        id: id.to_string(),
        provider: pid,
        context_window: 0,
        max_output_tokens: 0,
        capabilities: Vec::new(),
        cost: rupu_providers::ModelCost::default(),
        status: rupu_providers::ModelStatus::default(),
    }
}
