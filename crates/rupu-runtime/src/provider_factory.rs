//! Build a `Box<dyn LlmProvider>` from a provider-name string +
//! credential lookup. v0 wires Anthropic, OpenAI/Codex, and Copilot;
//! Gemini is deferred (AI Studio API-key endpoint not yet wired;
//! SSO/Vertex path pending verification). Local returns a clear
//! "not wired in v0" error so the failure mode is informative rather
//! than a silent provider-discovery miss.
//!
//! Credentials come from a `&dyn CredentialResolver`. The resolver is
//! the single authoritative source for credentials; the factory does
//! not read env vars or `auth.json` directly.
//!
//! When the lifted `rupu-providers` API stabilizes, this file is the
//! one place to extend.

use rupu_providers::provider::LlmProvider;
use thiserror::Error;

/// Per-build configuration for the provider factory. Optional knobs
/// that flow from the agent file's frontmatter (or workflow step
/// config) into provider-specific behavior. `Default` keeps historical
/// behavior — every existing call site can pass `Default::default()`
/// and observe no change.
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// For Anthropic OAuth requests, whether to prepend the canonical
    /// "You are Claude Code, …" system-prompt prefix that signals
    /// first-party traffic to the OAuth-quota router. `None` defers to
    /// the client-side default (currently: enabled). `Some(false)` opts
    /// the agent out — useful when the prefix corrupts persona.
    pub anthropic_oauth_system_prefix: Option<bool>,
    /// Present when the provider name resolves to a config-declared
    /// OpenAI-compatible endpoint. Populated by callers that have a
    /// loaded `rupu_config::Config` (e.g. `rupu run`).
    pub openai_compatible: Option<OpenAiCompatibleParams>,
    /// Resolved `[providers.<name>]` runtime knobs — `timeout_ms`,
    /// `max_retries`, `max_concurrency`, `org_id`, `region` (ISSUES.md
    /// I-9/I-10/I-11/I-12). `None` means "no config for this provider": the
    /// factory then applies `ProviderTuning::for_provider(name)`, i.e. the
    /// documented defaults. Build it with [`provider_tuning`].
    pub tuning: Option<rupu_providers::ProviderTuning>,
    /// Resolved vendor kind for this account, from
    /// [`resolve_kind`]. `None` means "dispatch on the provider name",
    /// which is the pre-multi-account behavior and is what every
    /// existing `Default::default()` call site gets.
    pub kind: Option<String>,
}

/// Everything the factory needs to build an `OpenAiCompatibleClient`,
/// resolved from a `[providers.<name>]` config entry.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleParams {
    pub base_url: String,
    pub default_model: String,
    pub stream: bool,
    pub models: Vec<rupu_providers::OpenAiCompatibleModel>,
}

const DEFAULT_OAI_CONTEXT_WINDOW: u32 = 32_768;
const DEFAULT_OAI_MAX_OUTPUT: u32 = 8_192;

/// Resolve `[providers.<name>]` into params iff it declares
/// `kind = "openai-compatible"` with a `base_url`. Returns `None` otherwise.
pub fn openai_compatible_params(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> Option<OpenAiCompatibleParams> {
    let p = providers.get(name)?;
    if p.kind.as_deref() != Some("openai-compatible") {
        return None;
    }
    let base_url = p.base_url.clone()?;
    let default_model = p.default_model.clone().unwrap_or_default();
    let models = p
        .models
        .iter()
        .map(|m| rupu_providers::OpenAiCompatibleModel {
            id: m.id.clone(),
            context_window: m.context_window.unwrap_or(DEFAULT_OAI_CONTEXT_WINDOW),
            max_output: m.max_output.unwrap_or(DEFAULT_OAI_MAX_OUTPUT),
        })
        .collect();
    Some(OpenAiCompatibleParams {
        base_url,
        default_model,
        stream: p.stream.unwrap_or(true),
        models,
    })
}

/// Resolve every `[providers.<name>] kind = "openai-compatible"` entry into a
/// map keyed by provider name. Used by the workflow runner to give each step
/// the same custom-provider params `rupu run` resolves for one-shot agents.
pub fn openai_compatible_map(
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> std::collections::HashMap<String, OpenAiCompatibleParams> {
    providers
        .keys()
        .filter_map(|name| openai_compatible_params(name, providers).map(|p| (name.clone(), p)))
        .collect()
}

/// Resolve `[providers.<name>]` into the runtime knobs the provider clients
/// and decorators consume. Absent keys collapse to the documented defaults
/// here, so no consumer re-implements defaulting.
///
/// This is the single adapter between `rupu-config` and `rupu-providers`;
/// `rupu-providers` does not depend on `rupu-config` (hexagonal rule 1).
pub fn provider_tuning(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> rupu_providers::ProviderTuning {
    use rupu_providers::tuning;
    let p = providers.get(name);
    // The DEFAULT permit count (i.e. when `max_concurrency` isn't set in
    // config) comes from the resolved vendor KIND, not the account name —
    // `default_permits` only recognizes vendor strings ("openai" => 8), so
    // a named account like `openai-work` must still get the `openai`
    // default rather than falling through to the generic `_ => 4`. This is
    // purely about which default to pick; the semaphore itself stays keyed
    // by account name elsewhere (`decorate` / `ThrottledProvider::wrap`),
    // because two accounts of one vendor kind must hold independent
    // concurrency budgets (rate limits are per-credential).
    let kind = resolve_kind(name, providers).unwrap_or_else(|| name.to_string());
    rupu_providers::ProviderTuning {
        timeout: tuning::client_timeout(p.and_then(|p| p.timeout_ms)),
        max_retries: tuning::retry_budget(p.and_then(|p| p.max_retries)),
        max_concurrency: tuning::concurrency_permits(&kind, p.and_then(|p| p.max_concurrency)),
        org_id: p.and_then(|p| p.org_id.clone()),
        region: p.and_then(|p| p.region.clone()),
    }
}

/// [`provider_tuning`] for every declared `[providers.<name>]`, keyed by name.
/// Used by the workflow runner and the sub-agent dispatcher, which resolve a
/// provider name per step rather than once up front.
pub fn provider_tuning_map(
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> std::collections::HashMap<String, rupu_providers::ProviderTuning> {
    providers
        .keys()
        .map(|name| (name.clone(), provider_tuning(name, providers)))
        .collect()
}

/// Provider used when neither the agent nor the config names one.
pub const FALLBACK_PROVIDER: &str = "anthropic";
/// Model used when neither the agent, the config, nor the provider names one.
pub const FALLBACK_MODEL: &str = "claude-sonnet-4-6";

/// Treat `Some("")` as unset. `openai_compatible_params` yields an empty
/// `default_model` when the config key is absent, and a blank frontmatter value
/// is a typo rather than an intentional pin — neither should win a fallback.
fn non_empty(v: Option<&str>) -> Option<&str> {
    v.filter(|s| !s.trim().is_empty())
}

/// Resolve the provider name for a run: agent frontmatter `provider:` wins,
/// then `default_provider` from `config.toml`, then [`FALLBACK_PROVIDER`].
///
/// The single resolution point for `rupu run`, `rupu session`, and workflow
/// steps. Before this existed each call site hardcoded the fallback and
/// `default_provider` was never read at all (ISSUES.md I-1).
pub fn resolve_provider_name(spec_provider: Option<&str>, cfg_default: Option<&str>) -> String {
    non_empty(spec_provider)
        .or(non_empty(cfg_default))
        .unwrap_or(FALLBACK_PROVIDER)
        .to_string()
}

/// Resolve the model for a run: agent frontmatter `model:` wins, then the
/// resolved provider's `[providers.<name>].default_model`, then the global
/// `default_model` from `config.toml`, then [`FALLBACK_MODEL`].
///
/// The single resolution point for `rupu run`, `rupu session`, and workflow
/// steps — the workflow path previously skipped `cfg.default_model`, so the
/// same agent could resolve to a different model depending on how it was
/// invoked (ISSUES.md I-2).
///
/// The provider-scoped default is consulted *before* the global one: it is
/// the more specific value, and an agent pinned to a custom openai-compatible
/// provider must get that provider's model rather than a global default the
/// custom endpoint doesn't recognize (ISSUES.md I-3).
pub fn resolve_model(
    spec_model: Option<&str>,
    cfg_default: Option<&str>,
    provider_default: Option<&str>,
) -> String {
    non_empty(spec_model)
        .or(non_empty(provider_default))
        .or(non_empty(cfg_default))
        .unwrap_or(FALLBACK_MODEL)
        .to_string()
}

/// Resolve an account name to its vendor kind string.
///
/// `[providers.<name>].kind` wins; otherwise the name itself, when it is
/// a built-in vendor name (design spec §3.1 — this is what keeps a
/// config-less `anthropic` working). `None` means the name is neither
/// declared nor a vendor.
pub fn resolve_kind(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> Option<String> {
    if let Some(k) = providers.get(name).and_then(|p| p.kind.clone()) {
        return Some(k);
    }
    if is_builtin_provider(name) {
        return Some(name.to_string());
    }
    None
}

/// [`resolve_kind`] for every declared `[providers.<name>]`, keyed by name.
/// Mirrors [`openai_compatible_map`] / [`provider_tuning_map`] — for callers
/// (`CliAgentDispatcher`, `DefaultStepFactory`) that resolve their whole
/// config once up front into pre-resolved maps rather than holding the raw
/// `rupu_config::Config` and calling [`resolve_kind`] per dispatch.
///
/// A bare built-in name with no `[providers.<name>]` section at all (e.g.
/// plain `anthropic`, never declared) has no entry here — that's fine: a
/// missing map entry becomes `kind: None` at the call site, and
/// `build_for_provider_with_config` already falls back from `None` to the
/// account name, which for an undeclared builtin IS its own kind.
pub fn resolve_kind_map(
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> std::collections::HashMap<String, String> {
    providers
        .keys()
        .filter_map(|name| resolve_kind(name, providers).map(|k| (name.clone(), k)))
        .collect()
}

/// True for provider names the factory builds directly (not openai-compatible).
///
/// Deliberately narrower than `rupu_config::config::BUILTIN_PROVIDER_KINDS`
/// (9 names here vs. 13 there): this list is LLM-dispatchable kinds only —
/// the ones this factory's `match kind { .. }` in
/// [`build_for_provider_with_config`] actually knows how to build a client
/// for. `BUILTIN_PROVIDER_KINDS` is broader because `rupu-config` validates
/// what's *declarable* in `[providers.<name>]`, which also covers the four
/// SCM kinds (`github`/`gitlab`/`linear`/`jira`) this factory never
/// dispatches on. The two lists diverging is correct, not a bug — do not
/// "fix" one to match the other.
pub fn is_builtin_provider(name: &str) -> bool {
    matches!(
        name,
        "anthropic"
            | "openai"
            | "openai_codex"
            | "codex"
            | "gemini"
            | "google_gemini"
            | "copilot"
            | "github_copilot"
            | "local"
    )
}

/// True when this factory can build a client for the provider `name` given
/// the declared `[providers.*]` map — i.e. when
/// [`build_for_provider_with_config`]'s `match kind { .. }` will land on a
/// real arm rather than fall through to [`FactoryError::UnknownProvider`].
///
/// Two ways to be dispatchable, and a name only needs one:
/// 1. its resolved *vendor kind* ([`resolve_kind`]) is a builtin the factory
///    builds directly — which covers both a bare builtin name (`anthropic`)
///    and a **named account** declaring that vendor
///    (`[providers.anthropic-work] kind = "anthropic"`, exactly what
///    `rupu auth login --account anthropic-work --kind anthropic` writes);
/// 2. it declares a complete `kind = "openai-compatible"` endpoint
///    ([`openai_compatible_params`]).
///
/// Order matters only in that both are checked: `openai-compatible` is NOT a
/// builtin kind, so branch 1 rejects it and branch 2 must carry it — that
/// path is unchanged from before named accounts were dispatchable. A
/// half-declared `kind = "openai-compatible"` with no `base_url` yields no
/// params and stays undispatchable, so its error message is preserved too.
///
/// Exists so callers that need to *reject early* (the `rupu run` pre-flight
/// check, which fails before any credential lookup) ask the factory rather
/// than re-deriving the rule — a second copy that can disagree with this one
/// is exactly how a declared account came to be rejected in the first place.
pub fn is_dispatchable_provider(
    name: &str,
    providers: &std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
) -> bool {
    if resolve_kind(name, providers).is_some_and(|k| is_builtin_provider(&k)) {
        return true;
    }
    openai_compatible_params(name, providers).is_some()
}

#[derive(Debug, Error)]
pub enum FactoryError {
    #[error(
        "missing credential for provider {provider} ({source}): configure with \
         `rupu auth login --provider {provider}` or set the env var the provider expects"
    )]
    MissingCredential {
        provider: String,
        source: anyhow::Error,
    },
    /// Names BOTH remedies deliberately: the caller cannot know which of the
    /// two the user meant, and a name reaches this arm for either reason —
    /// an account whose vendor `kind` was never declared, or an
    /// openai-compatible endpoint that was never declared (or declared
    /// without a `base_url`). Saying only one of them sends half the users
    /// down the wrong path.
    #[error(
        "unknown provider '{0}': it is not a built-in vendor name, no \
         [providers.{0}] declares its vendor kind (declare one with \
         `rupu auth login --account {0} --kind <vendor>`), and it is not \
         declared as an openai-compatible endpoint ([providers.{0}] with \
         kind = \"openai-compatible\" and a base_url) in config.toml"
    )]
    UnknownProvider(String),
    #[error("provider {0} is not wired in v0; only `anthropic` is currently supported")]
    NotWiredInV0(String),
    #[error("provider construction failed: {0}")]
    Other(String),
}

/// Build a provider for `name`. Reads credentials from `resolver`
/// (the single authoritative source — keychain, in-memory for tests,
/// or any other `CredentialResolver` impl).
///
/// `auth_hint` may force a specific auth mode; `None` lets the resolver
/// apply SSO > API-key precedence. Returns the resolved mode alongside
/// the provider so callers can display the actual mode in run headers.
///
/// Test-only seam: when `RUPU_MOCK_PROVIDER_SCRIPT` is set, the factory
/// builds a `MockProvider` from the JSON script in the env var and
/// ignores `name`/`resolver`. Production users never set this; tests
/// use it to drive the agent loop end-to-end without an API key.
pub async fn build_for_provider(
    name: &str,
    model: &str,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: &dyn rupu_auth::CredentialResolver,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<(rupu_providers::AuthMode, Box<dyn LlmProvider>), FactoryError> {
    build_for_provider_with_config(
        name,
        model,
        auth_hint,
        resolver,
        &ProviderConfig::default(),
        sink,
    )
    .await
}

/// Same as [`build_for_provider`] but accepts a [`ProviderConfig`] for
/// per-build knobs that flow from agent frontmatter / workflow step
/// config (currently: `anthropic_oauth_system_prefix`).
///
/// `sink` is taken explicitly rather than defaulted: there is no
/// process-global netflow sink (removed alongside the seven provider
/// constructors' own `Arc<dyn FlowSink>` params — see `rupu-netflow`'s
/// `http::client_with`), so every caller must decide which run's ledger
/// this provider's outbound HTTP belongs to. This function is the single
/// chokepoint through which every agent-driven provider is built; whatever
/// `sink` a caller passes here is the one every one of that provider's
/// requests lands in.
pub async fn build_for_provider_with_config(
    name: &str,
    model: &str,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: &dyn rupu_auth::CredentialResolver,
    config: &ProviderConfig,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<(rupu_providers::AuthMode, Box<dyn LlmProvider>), FactoryError> {
    if let Ok(json) = std::env::var("RUPU_MOCK_PROVIDER_SCRIPT") {
        return Ok((
            rupu_providers::AuthMode::ApiKey,
            build_mock_from_script(&json)?,
        ));
    }
    let (mode, creds) =
        resolver
            .get(name, auth_hint)
            .await
            .map_err(|source| FactoryError::MissingCredential {
                provider: name.to_string(),
                source,
            })?;
    // The account name identifies *who*; the kind identifies *what
    // vendor*. Falling back to the name preserves the single-account
    // behavior exactly.
    let kind = config.kind.as_deref().unwrap_or(name);
    let tuning = config
        .tuning
        .clone()
        .unwrap_or_else(|| rupu_providers::ProviderTuning::for_provider(kind));
    let client = match kind {
        "anthropic" => build_anthropic(creds, model, config, &tuning, sink.clone()).await?,
        "openai" | "openai_codex" | "codex" => {
            build_openai(creds, model, &tuning, sink.clone()).await?
        }
        "gemini" | "google_gemini" => build_gemini(creds, model, &tuning, sink.clone()).await?,
        "copilot" | "github_copilot" => build_copilot(creds, model, &tuning, sink.clone()).await?,
        "local" => return Err(FactoryError::NotWiredInV0("local".to_string())),
        _ => {
            if let Some(params) = &config.openai_compatible {
                let key = match &creds {
                    rupu_providers::auth::AuthCredentials::ApiKey { key } => key.clone(),
                    rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access.clone(),
                };
                Box::new(
                    rupu_providers::OpenAiCompatibleClient::new(
                        &params.base_url,
                        &key,
                        &params.default_model,
                        params.models.clone(),
                        params.stream,
                        sink.clone(),
                    )
                    .with_tuning(&tuning),
                ) as Box<dyn LlmProvider>
            } else {
                return Err(FactoryError::UnknownProvider(name.to_string()));
            }
        }
    };
    Ok((mode, decorate(client, name, kind, &tuning)))
}

/// Apply the tuning decorators that make `max_retries` and `max_concurrency`
/// observable on every call (ISSUES.md I-10 / I-11).
///
/// Takes BOTH the account `name` and the resolved `kind`, and they are NOT
/// interchangeable: `name` keys the concurrency semaphore (rate limits are
/// per-credential, so two accounts of the same vendor — e.g. `anthropic-work`
/// and `anthropic-personal` — must hold independent concurrency budgets),
/// while `kind` decides WHICH decorator shape applies (native-retry vendors
/// are exempted regardless of which account name they're parked under).
/// Passing `name` to [`provider_has_native_retry`] here was Task 4's original
/// bug: a named `anthropic`-kind account (`kind == "anthropic"`, `name ==
/// "anthropic-work"`) would fail the name-keyed check and get double-wrapped
/// — see the deadlock/squared-retry-budget hazard described below.
///
/// **Ordering matters.** Throttle wraps the raw client, retry wraps throttle —
/// `RetryingProvider(ThrottledProvider(client))`. Each retry attempt therefore
/// acquires its own permit and drops it the moment that attempt returns, so the
/// exponential backoff sleeps *outside* the semaphore. The inverse nesting
/// (throttle outermost) parks a permit in `tokio::time::sleep` for the whole
/// 2s/4s/8s ladder, which under rate limiting starves every other caller of the
/// same provider — `tuned::tests::a_backoff_sleep_does_not_hold_a_concurrency_permit`
/// and its `..._inverted_order_...` counterpart pin both halves of that claim.
///
/// Anthropic is the one *kind* given NEITHER decorator here — `client` is
/// returned as-is, for every account of that kind. It has its own in-client
/// 429 loop, already driven by `tuning.max_retries` via
/// `AnthropicClient::with_tuning` (stacking `RetryingProvider` on top would
/// silently square the retry budget), AND — since ISSUES.md I-75 — its own
/// per-attempt semaphore acquisition, also wired up by `with_tuning`. That
/// internal acquisition mirrors this same `RetryingProvider(ThrottledProvider(..))`
/// shape *inside* the client: a fresh permit per HTTP attempt, dropped before
/// that attempt's own backoff sleep, so the sleep no longer starves other
/// concurrent Anthropic calls the way it used to when `ThrottledProvider` held
/// one permit for the client's *entire* call (request + every internal retry).
/// Wrapping Anthropic in `ThrottledProvider` here too would have both layers
/// drawing from the same semaphore — at best double-counting a permit per
/// call, at worst deadlocking outright when `max_concurrency == 1` (the outer
/// wrapper holds the only permit for the whole call while the client tries to
/// acquire a second one from the same exhausted semaphore before its first
/// attempt).
///
/// KNOWN GAP (not fixed here, flagged for follow-up): `AnthropicClient::with_tuning`
/// resolves its *internal* semaphore via the string literal `"anthropic"`
/// (`crates/rupu-providers/src/anthropic.rs`), not the account name passed in
/// here. So while this function correctly skips the OUTER `ThrottledProvider`
/// for every `anthropic`-kind account (avoiding the double-wrap hazard above),
/// the INNER per-attempt semaphore `AnthropicClient` acquires for itself is
/// still shared process-wide across every named `anthropic`-kind account —
/// `anthropic-work` and `anthropic-personal` do NOT get independent
/// concurrency budgets the way two named `openai`-kind accounts do (those go
/// through this function's account-name-keyed `ThrottledProvider::wrap`
/// below). Fixing that requires threading the account name into
/// `AnthropicClient::with_tuning`'s own semaphore key, a change to a
/// different crate/file that is out of this function's scope.
fn decorate(
    client: Box<dyn LlmProvider>,
    name: &str,
    kind: &str,
    tuning: &rupu_providers::ProviderTuning,
) -> Box<dyn LlmProvider> {
    if provider_has_native_retry(kind) {
        return client;
    }
    // Account-name-keyed, deliberately NOT kind-keyed: two accounts of the
    // same vendor kind must not share a concurrency budget.
    let throttled = rupu_providers::ThrottledProvider::wrap(client, name, tuning);
    Box::new(rupu_providers::RetryingProvider::new(
        Box::new(throttled),
        tuning.max_retries,
    ))
}

/// True for VENDOR KINDS whose client already spends `tuning.max_retries`
/// (and, as of I-75, `tuning.max_concurrency`) itself — see `decorate` above.
/// Takes the resolved `kind`, never the account name: a named account
/// (`anthropic-work`) of a native-retry kind (`anthropic`) must still skip
/// the outer decorator, or it gets double-wrapped.
fn provider_has_native_retry(kind: &str) -> bool {
    kind == "anthropic"
}

fn build_mock_from_script(json: &str) -> Result<Box<dyn LlmProvider>, FactoryError> {
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    let turns: Vec<ScriptedTurn> =
        serde_json::from_str(json).map_err(|e| FactoryError::Other(format!("mock script: {e}")))?;
    Ok(Box::new(MockProvider::new(turns)))
}

async fn build_anthropic(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
    config: &ProviderConfig,
    tuning: &rupu_providers::ProviderTuning,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    // Convert the resolved credential into an Anthropic AuthMethod so OAuth
    // tokens travel via `Authorization: Bearer …` and API keys via
    // `x-api-key`. The earlier shape pulled `access` out of the OAuth variant
    // and shoved it into an ApiKey-mode client, which routed bearer tokens
    // through the api-key header and produced a confusing "invalid x-api-key"
    // 401 for every SSO request.
    //
    // For OAuth, also pull `account_uuid` out of the credential's `extra`
    // map (captured at SSO login time from the token-exchange response)
    // and thread it into the client so it lands in `metadata.user_id` and
    // binds the request to the user's Pro/Max quota.
    let account_uuid = match &creds {
        rupu_providers::auth::AuthCredentials::OAuth { extra, .. } => extra
            .get("account_uuid")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        _ => None,
    };
    let auth = creds.into_anthropic_auth_method();
    let mut client = match std::env::var("RUPU_ANTHROPIC_BASE_URL_OVERRIDE") {
        Ok(url) => rupu_providers::anthropic::AnthropicClient::from_auth_with_url(auth, url, sink),
        Err(_) => rupu_providers::anthropic::AnthropicClient::from_auth(auth, sink),
    }
    .with_tuning(tuning)
    .with_oauth_account_uuid(account_uuid);
    if let Some(enabled) = config.anthropic_oauth_system_prefix {
        client = client.with_oauth_system_prefix(enabled);
    }
    // Best-effort: register the OAuth session with Anthropic's bootstrap
    // endpoint before the first message lands. Mirrors what the reference
    // Claude Code client does on startup; appears to pre-warm the
    // OAuth-quota router. No-op on api-key clients.
    client.bootstrap_oauth_session().await;
    Ok(Box::new(client))
}

async fn build_openai(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
    tuning: &rupu_providers::ProviderTuning,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let client = rupu_providers::openai_codex::OpenAiCodexClient::new(creds, None, sink)
        .map_err(|e| FactoryError::Other(format!("openai client init: {e}")))?
        .with_tuning(tuning);
    Ok(Box::new(client))
}

async fn build_gemini(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
    tuning: &rupu_providers::ProviderTuning,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    // Branch on credential shape:
    // - `ApiKey` → AI Studio (`generativelanguage.googleapis.com`,
    //   `x-goog-api-key` header).
    // - `OAuth`  → Cloud Code Assist (Gemini-CLI / Antigravity
    //   variants). Picking between the two is currently driven by
    //   the `extra.variant` hint at SSO time, defaulting to the
    //   production GeminiCli endpoint.
    use rupu_providers::auth::AuthCredentials;
    use rupu_providers::google_gemini::{GeminiVariant, GoogleGeminiClient};
    let variant = match &creds {
        AuthCredentials::ApiKey { .. } => GeminiVariant::AiStudio,
        AuthCredentials::OAuth { extra, .. } => extra
            .get("variant")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "antigravity" => GeminiVariant::Antigravity,
                _ => GeminiVariant::GeminiCli,
            })
            .unwrap_or(GeminiVariant::GeminiCli),
    };
    let client = GoogleGeminiClient::new(creds, variant, None, sink)
        .map_err(|e| FactoryError::Other(format!("gemini client init: {e}")))?
        .with_tuning(tuning);
    Ok(Box::new(client))
}

async fn build_copilot(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
    tuning: &rupu_providers::ProviderTuning,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let client = rupu_providers::github_copilot::GithubCopilotClient::new(creds, None, sink)
        .map_err(|e| FactoryError::Other(format!("copilot client init: {e}")))?
        .with_tuning(tuning);
    Ok(Box::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_openai_compatible_params_from_config() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "oracle".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://192.29.35.246:8080".into()),
                default_model: Some("/raid/models/zai-org/GLM-5.2-FP8".into()),
                stream: Some(false),
                models: vec![rupu_config::CustomModel {
                    id: "/raid/models/zai-org/GLM-5.2-FP8".into(),
                    context_window: Some(131072),
                    max_output: Some(8192),
                }],
                ..Default::default()
            },
        );
        let p = openai_compatible_params("oracle", &providers).unwrap();
        assert_eq!(p.base_url, "http://192.29.35.246:8080");
        assert_eq!(p.default_model, "/raid/models/zai-org/GLM-5.2-FP8");
        assert!(!p.stream);
        assert_eq!(p.models.len(), 1);
        assert_eq!(p.models[0].context_window, 131072);
        // A name without kind=openai-compatible yields None.
        assert!(openai_compatible_params("anthropic", &providers).is_none());
    }

    #[test]
    fn provider_tuning_reads_every_configured_knob() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "openai".to_string(),
            rupu_config::ProviderConfig {
                timeout_ms: Some(9_000),
                max_retries: Some(4),
                max_concurrency: Some(3),
                org_id: Some("org-abc123".into()),
                region: Some("us-central1".into()),
                ..Default::default()
            },
        );
        let t = provider_tuning("openai", &providers);
        assert_eq!(t.timeout, std::time::Duration::from_millis(9_000));
        assert_eq!(t.max_retries, 4);
        assert_eq!(t.max_concurrency, 3);
        assert_eq!(t.org_id.as_deref(), Some("org-abc123"));
        assert_eq!(t.region.as_deref(), Some("us-central1"));
    }

    #[test]
    fn provider_tuning_falls_back_to_documented_defaults() {
        use std::collections::BTreeMap;
        let providers = BTreeMap::new();
        let t = provider_tuning("openai", &providers);
        assert_eq!(t.timeout, std::time::Duration::from_millis(120_000));
        assert_eq!(t.max_retries, 1);
        // Per-vendor permit table, not a flat default.
        assert_eq!(t.max_concurrency, 8);
        assert_eq!(provider_tuning("anthropic", &providers).max_concurrency, 4);
        assert!(t.org_id.is_none());
    }

    /// A named `openai`-kind account must get the OPENAI vendor default (8
    /// permits), not the generic 4-permit fallback `default_permits` uses
    /// for any name it doesn't recognize as a vendor. Before this fix,
    /// `provider_tuning` passed the ACCOUNT NAME (not the resolved kind)
    /// into `concurrency_permits`, so `openai-work` silently got 4 permits
    /// where bare `openai` got 8.
    ///
    /// The semaphore itself must stay keyed by account name — two accounts
    /// of the same vendor kind hold independent concurrency budgets, since
    /// rate limits are per-API-key.
    ///
    /// What THIS test discriminates is the permit COUNT coming from kind
    /// — that half is the fix's real RED. Its semaphore assertion is a
    /// sanity check only: it passes `semaphore()` the account names
    /// itself, so `semaphore_for`'s registry keeps them distinct however
    /// `decorate` keys things. The guard that would actually catch a
    /// kind-collapse regression is
    /// `two_accounts_of_the_same_kind_get_independent_semaphores`, which
    /// drives `decorate` rather than calling `semaphore()` directly.
    #[test]
    fn named_openai_kind_account_gets_the_vendor_default_permits() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "openai-work-t2".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "openai-personal-t2".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai".to_string()),
                ..Default::default()
            },
        );

        let work = provider_tuning("openai-work-t2", &providers);
        let personal = provider_tuning("openai-personal-t2", &providers);
        assert_eq!(
            work.max_concurrency, 8,
            "named openai account must default to the vendor's 8 permits, not the generic 4"
        );
        assert_eq!(personal.max_concurrency, 8);

        // Same default permit count, but each account's semaphore is its
        // own instance — draining one must not affect the other.
        let work_sem = work.semaphore("openai-work-t2");
        let personal_sem = personal.semaphore("openai-personal-t2");
        assert!(
            !std::sync::Arc::ptr_eq(&work_sem, &personal_sem),
            "two accounts of the same kind must not share a semaphore"
        );
        assert_eq!(work_sem.available_permits(), 8);
        assert_eq!(personal_sem.available_permits(), 8);
    }

    #[test]
    fn provider_tuning_map_covers_every_declared_provider() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "anthropic".to_string(),
            rupu_config::ProviderConfig {
                max_concurrency: Some(2),
                ..Default::default()
            },
        );
        providers.insert("openai".to_string(), Default::default());
        let map = provider_tuning_map(&providers);
        assert_eq!(map["anthropic"].max_concurrency, 2);
        assert_eq!(map["openai"].max_concurrency, 8);
        assert!(!map.contains_key("gemini"));
    }

    #[test]
    fn only_anthropic_skips_the_retry_decorator() {
        // Anthropic spends `max_retries` inside its own 429 loop (wrapping it
        // in RetryingProvider too would square the budget) and, as of I-75,
        // also spends `max_concurrency` inside its own per-attempt semaphore
        // acquisition (wrapping it in ThrottledProvider too would double-count
        // a permit per call, or deadlock at max_concurrency == 1) — see
        // `decorate`'s doc comment for the full reasoning.
        //
        // `provider_has_native_retry` takes a KIND string, not an account
        // name — these plain vendor strings double as both here, but
        // `decorate_kind_tests` below proves the distinction matters for a
        // named account whose name differs from its kind.
        assert!(provider_has_native_retry("anthropic"));
        assert!(!provider_has_native_retry("openai"));
        assert!(!provider_has_native_retry("gemini"));
        assert!(!provider_has_native_retry("copilot"));
        assert!(!provider_has_native_retry("oracle"));
    }

    #[test]
    fn is_builtin_recognizes_known_names() {
        assert!(is_builtin_provider("anthropic"));
        assert!(is_builtin_provider("copilot"));
        assert!(is_builtin_provider("local"));
        assert!(!is_builtin_provider("oracle"));
    }

    #[test]
    fn map_collects_only_openai_compatible_providers() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "oracle".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                default_model: Some("glm".into()),
                ..Default::default()
            },
        );
        // A plain (built-in) provider entry without kind=openai-compatible is excluded.
        providers.insert(
            "anthropic".to_string(),
            rupu_config::ProviderConfig {
                default_model: Some("claude-sonnet-4-6".into()),
                ..Default::default()
            },
        );
        let map = openai_compatible_map(&providers);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("oracle"));
        assert!(!map.contains_key("anthropic"));
        assert_eq!(map["oracle"].base_url, "http://host:8080");
    }

    #[test]
    fn resolve_kind_map_covers_every_declared_account() {
        use std::collections::BTreeMap;
        let mut providers = BTreeMap::new();
        providers.insert(
            "anthropic-work".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
        providers.insert(
            "oracle".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                ..Default::default()
            },
        );
        let map = resolve_kind_map(&providers);
        assert_eq!(map.len(), 2);
        assert_eq!(map["anthropic-work"], "anthropic");
        assert_eq!(map["oracle"], "openai-compatible");
        // A bare builtin with no declared section is absent — the call
        // site's fallback (`kind: None` → dispatch by name) covers it.
        assert!(!map.contains_key("anthropic"));
    }

    use std::collections::BTreeMap;

    fn providers_with(name: &str, kind: &str) -> BTreeMap<String, rupu_config::ProviderConfig> {
        let mut m = BTreeMap::new();
        m.insert(
            name.to_string(),
            rupu_config::ProviderConfig {
                kind: Some(kind.to_string()),
                ..Default::default()
            },
        );
        m
    }

    #[test]
    fn resolve_kind_prefers_declared_kind() {
        let p = providers_with("anthropic-work", "anthropic");
        assert_eq!(
            resolve_kind("anthropic-work", &p).as_deref(),
            Some("anthropic")
        );
    }

    /// Spec §3.1: with no config at all, a built-in name is its own kind.
    #[test]
    fn resolve_kind_falls_back_to_the_name_for_builtins() {
        let empty = BTreeMap::new();
        assert_eq!(
            resolve_kind("anthropic", &empty).as_deref(),
            Some("anthropic")
        );
        assert_eq!(resolve_kind("codex", &empty).as_deref(), Some("codex"));
    }

    #[test]
    fn resolve_kind_is_none_for_undeclared_non_builtin() {
        let empty = BTreeMap::new();
        assert_eq!(resolve_kind("anthropic-work", &empty), None);
    }

    #[test]
    fn resolve_kind_reports_openai_compatible() {
        let p = providers_with("oracle", "openai-compatible");
        assert_eq!(
            resolve_kind("oracle", &p).as_deref(),
            Some("openai-compatible")
        );
    }
}

#[cfg(test)]
mod build_copilot_tests {
    use super::*;
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn build_copilot_returns_provider() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        resolver
            .put(
                ProviderId::Copilot,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test_copilot"),
            )
            .await;
        let (_mode, p) = build_for_provider(
            "copilot",
            "gpt-4o",
            None,
            &resolver,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await
        .expect("build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::GithubCopilot);
    }
}

#[cfg(test)]
mod build_openai_tests {
    use super::*;
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn build_openai_returns_provider() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        resolver
            .put(
                ProviderId::Openai,
                AuthMode::ApiKey,
                StoredCredential::api_key("sk-test-openai"),
            )
            .await;
        let (_mode, p) = build_for_provider(
            "openai",
            "gpt-5",
            None,
            &resolver,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await
        .expect("build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::OpenaiCodex);
    }

    #[tokio::test]
    async fn build_openai_missing_credential_errors() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        // No credentials inserted — resolver returns missing-credential error.
        let result = build_for_provider(
            "openai",
            "gpt-5",
            None,
            &resolver,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await;
        assert!(matches!(
            result,
            Err(FactoryError::MissingCredential { .. })
        ));
    }
}

#[cfg(test)]
mod build_gemini_tests {
    use super::*;
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn build_gemini_with_api_key_returns_provider() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        resolver
            .put(
                ProviderId::Gemini,
                AuthMode::ApiKey,
                StoredCredential::api_key("AIzaSy-test-key"),
            )
            .await;
        let result = build_for_provider(
            "gemini",
            "gemini-2.5-pro",
            None,
            &resolver,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await;
        assert!(result.is_ok(), "expected Ok(provider), got error");
    }

    #[tokio::test]
    async fn build_gemini_missing_credential_errors() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        let result = build_for_provider(
            "gemini",
            "gemini-2.5-pro",
            None,
            &resolver,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await;
        assert!(matches!(
            result,
            Err(FactoryError::MissingCredential { .. })
        ));
    }
}

/// Directly exercises the Step-6 dispatch change: `build_for_provider_with_config`
/// must pick the client type from `config.kind`, not from the account name.
/// None of `resolve_kind`'s own unit tests (pure string resolution) or the
/// sibling `build_*_tests` modules (which all pass `ProviderConfig::default()`,
/// i.e. `kind: None`) exercise the `match kind { .. }` line itself.
#[cfg(test)]
mod dispatch_by_kind_tests {
    use super::*;
    use rupu_auth::backend::ProviderId as AuthProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn dispatch_follows_config_kind_not_the_account_name() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        // Credentials live under the account name "openai" ...
        resolver
            .put(
                AuthProviderId::Openai,
                AuthMode::ApiKey,
                StoredCredential::api_key("sk-test-openai"),
            )
            .await;
        let config = ProviderConfig {
            // ... but the declared kind says "anthropic". If dispatch still
            // matched on the name, this would build an OpenAI client.
            kind: Some("anthropic".to_string()),
            ..Default::default()
        };
        let (_mode, p) = build_for_provider_with_config(
            "openai",
            "claude-sonnet-4-6",
            None,
            &resolver,
            &config,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await
        .expect("build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::Anthropic);
    }
}

/// Covers `decorate`'s kind-vs-name split (the coordinator-ruled fix for
/// concern #1 in the Task 4 review): the native-retry SKIP must follow
/// `kind`, while the concurrency semaphore must stay keyed by the account
/// `name`. Uses hand-rolled `LlmProvider` probes (mirroring
/// `rupu_providers::tuned`'s own test fixtures) since `decorate` returns an
/// opaque `Box<dyn LlmProvider>` that can't be downcast to inspect which
/// decorators were applied — attempt counts and permit-contention timing are
/// the only externally observable signals.
#[cfg(test)]
mod decorate_kind_tests {
    use super::*;
    use async_trait::async_trait;
    use rupu_providers::{
        ContentBlock, LlmRequest, LlmResponse, Message, ProviderError, ProviderId as PId,
        StopReason, StreamEvent, Usage,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn req() -> LlmRequest {
        LlmRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message::user("hi")],
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
        }
    }

    fn ok_response() -> LlmResponse {
        LlmResponse {
            id: "msg".into(),
            model: "m".into(),
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
        }
    }

    /// Always fails with a retryable error, counting every `send()` call —
    /// lets a test tell "wrapped in `RetryingProvider`" (multiple calls per
    /// one external `.send()`) from "not wrapped" (exactly one).
    struct RetryableProbe {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmProvider for RetryableProbe {
        async fn send(&mut self, _r: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::RateLimited { retry_after: None })
        }
        async fn stream(
            &mut self,
            _r: &LlmRequest,
            _e: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            unreachable!("test only exercises send()")
        }
        fn default_model(&self) -> &str {
            "mock"
        }
        fn provider_id(&self) -> PId {
            PId::Anthropic
        }
    }

    /// Always succeeds immediately.
    struct OkProbe;

    #[async_trait]
    impl LlmProvider for OkProbe {
        async fn send(&mut self, _r: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            Ok(ok_response())
        }
        async fn stream(
            &mut self,
            _r: &LlmRequest,
            _e: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            unreachable!("test only exercises send()")
        }
        fn default_model(&self) -> &str {
            "mock"
        }
        fn provider_id(&self) -> PId {
            PId::Anthropic
        }
    }

    /// Signals `ready` the instant `send()` is entered (i.e. the instant its
    /// `ThrottledProvider` permit is held), then blocks until `release` fires
    /// — lets a test hold one decorated provider's permit for a controlled
    /// window while probing whether a DIFFERENT decorated provider is
    /// blocked by it.
    struct BlockingProbe {
        ready: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl LlmProvider for BlockingProbe {
        async fn send(&mut self, _r: &LlmRequest) -> Result<LlmResponse, ProviderError> {
            self.ready.notify_one();
            self.release.notified().await;
            Ok(ok_response())
        }
        async fn stream(
            &mut self,
            _r: &LlmRequest,
            _e: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, ProviderError> {
            unreachable!("test only exercises send()")
        }
        fn default_model(&self) -> &str {
            "mock"
        }
        fn provider_id(&self) -> PId {
            PId::Anthropic
        }
    }

    /// The Task 4 review's concern #1: a NAMED anthropic-kind account
    /// (`name != kind`) must still skip the outer decorator. Before the fix,
    /// `decorate` checked `provider_has_native_retry(name)`, which is false
    /// for `"anthropic-work"` — silently double-wrapping the client on top
    /// of its own internal retry loop.
    #[tokio::test]
    async fn anthropic_kind_is_not_double_decorated_even_for_a_named_account() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = RetryableProbe {
            calls: calls.clone(),
        };
        let tuning = rupu_providers::ProviderTuning {
            max_retries: 1,
            ..rupu_providers::ProviderTuning::for_provider("anthropic")
        };
        // Account name deliberately differs from the kind.
        let mut decorated = decorate(Box::new(probe), "anthropic-work", "anthropic", &tuning);
        let _ = decorated.send(&req()).await;
        // Exactly one call: if `RetryingProvider` were wrapping this (the
        // pre-fix bug), a retryable error would trigger a second attempt
        // after the backoff sleep, inside this single external `.send()`.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an anthropic-kind account was double-decorated"
        );
    }

    /// The symmetric regression guard: a non-anthropic kind must still get
    /// the retry decorator, named account or not. Uses paused tokio time so
    /// the real 2s backoff doesn't slow the suite down (same idiom as
    /// `rupu_providers::tuned`'s own retry tests).
    #[tokio::test(start_paused = true)]
    async fn a_non_anthropic_kind_still_gets_the_retry_decorator() {
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = RetryableProbe {
            calls: calls.clone(),
        };
        let tuning = rupu_providers::ProviderTuning {
            max_retries: 1,
            ..rupu_providers::ProviderTuning::for_provider("openai")
        };
        let mut decorated = decorate(Box::new(probe), "openai-work", "openai", &tuning);
        let handle = tokio::spawn(async move {
            let _ = decorated.send(&req()).await;
        });
        // Let the spawned task run up to its backoff sleep before advancing
        // the clock, or `advance` below could find no timer registered yet.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        handle.await.unwrap();
        // Two calls: the RetryingProvider retried once after the backoff.
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a non-anthropic-kind account was not retry-decorated"
        );
    }

    /// The Task 4 review's concern #1's other half, and the explicit
    /// "do NOT change `ThrottledProvider::wrap`" constraint: two DIFFERENT
    /// account names of the SAME kind must not share a concurrency permit.
    ///
    /// Deliberately does NOT acquire `"acct-a"`'s semaphore directly (that
    /// would only prove the correct, name-keyed implementation is
    /// self-consistent — a kind-keyed regression would leave `"acct-a"`
    /// untouched and this test would pass vacuously either way). Instead it
    /// holds `acct_a`'s permit BY ACTUALLY CALLING THROUGH `decorate`, via
    /// `BlockingProbe`, so the held permit is whichever key `decorate`
    /// genuinely used — then checks whether that contends with `acct_b`.
    #[tokio::test]
    async fn two_accounts_of_the_same_kind_get_independent_semaphores() {
        let tuning = rupu_providers::ProviderTuning {
            max_concurrency: 1,
            ..rupu_providers::ProviderTuning::for_provider("openai")
        };

        let ready = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let mut decorated_a = decorate(
            Box::new(BlockingProbe {
                ready: ready.clone(),
                release: release.clone(),
            }),
            "acct-a",
            "openai",
            &tuning,
        );
        let handle_a = tokio::spawn(async move {
            let request = req();
            decorated_a.send(&request).await.unwrap();
        });
        // Wait until acct-a's send() has actually entered — i.e. it holds
        // whichever permit `decorate` acquired for it.
        ready.notified().await;

        let mut decorated_b = decorate(Box::new(OkProbe), "acct-b", "openai", &tuning);
        let request_b = req();
        let call_b = decorated_b.send(&request_b);
        tokio::pin!(call_b);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut call_b)
                .await
                .is_ok(),
            "acct-b was blocked while acct-a held its permit — semaphores \
             are not independent per account"
        );

        release.notify_one();
        handle_a.await.unwrap();
    }
}

/// The multi-account gate: a provider name that is not a builtin vendor name
/// must still be dispatchable when its account declares a vendor `kind`.
///
/// `InMemoryResolver` deliberately only knows builtin vendor names, so these
/// tests use a permissive local resolver — the point under test is which
/// client the `match kind { .. }` arm builds for a genuinely *named* account,
/// not credential lookup (which `KeychainResolver::get_named` already covers).
#[cfg(test)]
mod named_account_dispatch_tests {
    use super::*;
    use async_trait::async_trait;
    use rupu_providers::auth::AuthCredentials;
    use rupu_providers::AuthMode;
    use std::collections::BTreeMap;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    /// Hands back the same API key for any account name — including names
    /// `rupu_auth::in_memory::InMemoryResolver` refuses to parse.
    struct AnyAccountResolver;

    #[async_trait]
    impl rupu_auth::CredentialResolver for AnyAccountResolver {
        async fn get(
            &self,
            _provider: &str,
            _hint: Option<AuthMode>,
        ) -> anyhow::Result<(AuthMode, AuthCredentials)> {
            Ok((
                AuthMode::ApiKey,
                AuthCredentials::ApiKey {
                    key: "sk-test-any".to_string(),
                },
            ))
        }

        async fn refresh(
            &self,
            _provider: &str,
            _mode: AuthMode,
        ) -> anyhow::Result<AuthCredentials> {
            unreachable!("tests never refresh")
        }
    }

    async fn build(
        name: &str,
        config: ProviderConfig,
    ) -> Result<Box<dyn LlmProvider>, FactoryError> {
        build_for_provider_with_config(
            name,
            "some-model",
            None,
            &AnyAccountResolver,
            &config,
            std::sync::Arc::new(rupu_netflow::NullSink),
        )
        .await
        .map(|(_mode, p)| p)
    }

    fn declared(name: &str, kind: &str) -> BTreeMap<String, rupu_config::ProviderConfig> {
        let mut m = BTreeMap::new();
        m.insert(
            name.to_string(),
            rupu_config::ProviderConfig {
                kind: Some(kind.to_string()),
                ..Default::default()
            },
        );
        m
    }

    #[tokio::test]
    async fn a_named_anthropic_account_builds_an_anthropic_client() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let providers = declared("anthropic-work", "anthropic");
        let p = build(
            "anthropic-work",
            ProviderConfig {
                kind: resolve_kind("anthropic-work", &providers),
                ..Default::default()
            },
        )
        .await
        .expect("named anthropic account should build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::Anthropic);
    }

    #[tokio::test]
    async fn a_named_openai_account_builds_an_openai_client() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let providers = declared("openai-work", "openai");
        let p = build(
            "openai-work",
            ProviderConfig {
                kind: resolve_kind("openai-work", &providers),
                ..Default::default()
            },
        )
        .await
        .expect("named openai account should build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::OpenaiCodex);
    }

    /// The openai-compatible path must keep winning where it applies today:
    /// its kind is not a dispatchable builtin, so it lands on the `_` arm and
    /// is satisfied by `config.openai_compatible`.
    #[tokio::test]
    async fn an_openai_compatible_account_still_builds_that_client() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let mut providers = BTreeMap::new();
        providers.insert(
            "oracle".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://127.0.0.1:9".into()),
                default_model: Some("glm".into()),
                ..Default::default()
            },
        );
        let p = build(
            "oracle",
            ProviderConfig {
                kind: resolve_kind("oracle", &providers),
                openai_compatible: openai_compatible_params("oracle", &providers),
                ..Default::default()
            },
        )
        .await
        .expect("openai-compatible account should build");
        assert_eq!(
            p.provider_id(),
            rupu_providers::ProviderId::OpenaiCompatible
        );
    }

    /// A builtin name with no config at all is untouched by any of this.
    #[tokio::test]
    async fn a_builtin_name_is_unaffected() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let empty = BTreeMap::new();
        let p = build(
            "anthropic",
            ProviderConfig {
                kind: resolve_kind("anthropic", &empty),
                ..Default::default()
            },
        )
        .await
        .expect("builtin should build");
        assert_eq!(p.provider_id(), rupu_providers::ProviderId::Anthropic);
    }

    /// Neither a builtin, nor a declared account, nor an openai-compatible
    /// endpoint: still an error, and the message must name BOTH remedies —
    /// the user does not know which of the two they forgot.
    #[tokio::test]
    async fn an_unknown_name_errors_naming_both_remedies() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let empty = BTreeMap::new();
        let result = build(
            "typo-provider",
            ProviderConfig {
                kind: resolve_kind("typo-provider", &empty),
                ..Default::default()
            },
        )
        .await;
        // Not `expect_err`: `Box<dyn LlmProvider>` isn't `Debug`.
        let Err(err) = result else {
            panic!("undeclared provider must error");
        };
        let msg = err.to_string();
        assert!(msg.contains("typo-provider"), "names the provider: {msg}");
        assert!(
            msg.contains("rupu auth login"),
            "remedy 1 — declare the account's vendor kind: {msg}"
        );
        assert!(
            msg.contains("openai-compatible"),
            "remedy 2 — declare an openai-compatible endpoint: {msg}"
        );
    }

    #[test]
    fn is_dispatchable_covers_builtins_named_accounts_and_openai_compatible() {
        let empty = BTreeMap::new();
        // Builtin vendor names, with no config at all.
        assert!(is_dispatchable_provider("anthropic", &empty));
        assert!(is_dispatchable_provider("copilot", &empty));
        // A named account declaring a dispatchable vendor kind.
        assert!(is_dispatchable_provider(
            "anthropic-work",
            &declared("anthropic-work", "anthropic")
        ));
        assert!(is_dispatchable_provider(
            "openai-work",
            &declared("openai-work", "openai")
        ));
        // A complete openai-compatible declaration.
        let mut oai = BTreeMap::new();
        oai.insert(
            "oracle".to_string(),
            rupu_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
        );
        assert!(is_dispatchable_provider("oracle", &oai));
        // Undeclared, and a `kind = "openai-compatible"` with no `base_url`
        // (which yields no params, so nothing can be built from it).
        assert!(!is_dispatchable_provider("typo-provider", &empty));
        assert!(!is_dispatchable_provider(
            "half-declared",
            &declared("half-declared", "openai-compatible")
        ));
        // A kind rupu-config accepts but this factory never dispatches on.
        assert!(!is_dispatchable_provider(
            "gh-work",
            &declared("gh-work", "github")
        ));
    }
}
