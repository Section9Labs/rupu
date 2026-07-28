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
    rupu_providers::ProviderTuning {
        timeout: tuning::client_timeout(p.and_then(|p| p.timeout_ms)),
        max_retries: tuning::retry_budget(p.and_then(|p| p.max_retries)),
        max_concurrency: tuning::concurrency_permits(name, p.and_then(|p| p.max_concurrency)),
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

/// Resolve the model for a run: agent frontmatter `model:` wins, then
/// `default_model` from `config.toml`, then the resolved provider's
/// `[providers.<name>].default_model`, then [`FALLBACK_MODEL`].
///
/// The single resolution point for `rupu run`, `rupu session`, and workflow
/// steps — the workflow path previously skipped `cfg.default_model`, so the
/// same agent could resolve to a different model depending on how it was
/// invoked (ISSUES.md I-2).
///
/// Note the global `default_model` is consulted *before* the provider-scoped
/// one. That is the pre-existing `rupu run` order, preserved here deliberately;
/// see ISSUES.md I-3 for why it is questionable.
pub fn resolve_model(
    spec_model: Option<&str>,
    cfg_default: Option<&str>,
    provider_default: Option<&str>,
) -> String {
    non_empty(spec_model)
        .or(non_empty(cfg_default))
        .or(non_empty(provider_default))
        .unwrap_or(FALLBACK_MODEL)
        .to_string()
}

/// True for provider names the factory builds directly (not openai-compatible).
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
    #[error("unknown provider: {0}")]
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
) -> Result<(rupu_providers::AuthMode, Box<dyn LlmProvider>), FactoryError> {
    build_for_provider_with_config(name, model, auth_hint, resolver, &ProviderConfig::default())
        .await
}

/// Same as [`build_for_provider`] but accepts a [`ProviderConfig`] for
/// per-build knobs that flow from agent frontmatter / workflow step
/// config (currently: `anthropic_oauth_system_prefix`).
pub async fn build_for_provider_with_config(
    name: &str,
    model: &str,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: &dyn rupu_auth::CredentialResolver,
    config: &ProviderConfig,
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
    let tuning = config
        .tuning
        .clone()
        .unwrap_or_else(|| rupu_providers::ProviderTuning::for_provider(name));
    let client = match name {
        "anthropic" => build_anthropic(creds, model, config, &tuning).await?,
        "openai" | "openai_codex" | "codex" => build_openai(creds, model, &tuning).await?,
        "gemini" | "google_gemini" => build_gemini(creds, model, &tuning).await?,
        "copilot" | "github_copilot" => build_copilot(creds, model, &tuning).await?,
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
                    )
                    .with_tuning(&tuning),
                ) as Box<dyn LlmProvider>
            } else {
                return Err(FactoryError::UnknownProvider(name.to_string()));
            }
        }
    };
    Ok((mode, decorate(client, name, &tuning)))
}

/// Apply the tuning decorators that make `max_retries` and `max_concurrency`
/// observable on every call (ISSUES.md I-10 / I-11).
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
/// Anthropic is the one provider given NEITHER decorator here — `client` is
/// returned as-is. It has its own in-client 429 loop, already driven by
/// `tuning.max_retries` via `AnthropicClient::with_tuning` (stacking
/// `RetryingProvider` on top would silently square the retry budget), AND —
/// since ISSUES.md I-75 — its own per-attempt semaphore acquisition, also
/// wired up by `with_tuning`. That internal acquisition mirrors this same
/// `RetryingProvider(ThrottledProvider(..))` shape *inside* the client: a
/// fresh permit per HTTP attempt, dropped before that attempt's own backoff
/// sleep, so the sleep no longer starves other concurrent Anthropic calls the
/// way it used to when `ThrottledProvider` held one permit for the client's
/// *entire* call (request + every internal retry). Wrapping Anthropic in
/// `ThrottledProvider` here too would have both layers drawing from the same
/// name-keyed semaphore (`ThrottledProvider::wrap` and
/// `AnthropicClient::with_tuning` both resolve `tuning.semaphore("anthropic")`
/// to the identical process-wide instance) — at best double-counting a permit
/// per call, at worst deadlocking outright when `max_concurrency == 1` (the
/// outer wrapper holds the only permit for the whole call while the client
/// tries to acquire a second one from the same exhausted semaphore before its
/// first attempt).
fn decorate(
    client: Box<dyn LlmProvider>,
    name: &str,
    tuning: &rupu_providers::ProviderTuning,
) -> Box<dyn LlmProvider> {
    if provider_has_native_retry(name) {
        return client;
    }
    let throttled = rupu_providers::ThrottledProvider::wrap(client, name, tuning);
    Box::new(rupu_providers::RetryingProvider::new(
        Box::new(throttled),
        tuning.max_retries,
    ))
}

/// True for providers whose client already spends `tuning.max_retries` (and,
/// as of I-75, `tuning.max_concurrency`) itself — see `decorate` above.
fn provider_has_native_retry(name: &str) -> bool {
    name == "anthropic"
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
        Ok(url) => rupu_providers::anthropic::AnthropicClient::from_auth_with_url(auth, url),
        Err(_) => rupu_providers::anthropic::AnthropicClient::from_auth(auth),
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
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let client = rupu_providers::openai_codex::OpenAiCodexClient::new(creds, None)
        .map_err(|e| FactoryError::Other(format!("openai client init: {e}")))?
        .with_tuning(tuning);
    Ok(Box::new(client))
}

async fn build_gemini(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
    tuning: &rupu_providers::ProviderTuning,
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
    let client = GoogleGeminiClient::new(creds, variant, None)
        .map_err(|e| FactoryError::Other(format!("gemini client init: {e}")))?
        .with_tuning(tuning);
    Ok(Box::new(client))
}

async fn build_copilot(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
    tuning: &rupu_providers::ProviderTuning,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let client = rupu_providers::github_copilot::GithubCopilotClient::new(creds, None)
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
        let (_mode, p) = build_for_provider("copilot", "gpt-4o", None, &resolver)
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
        let (_mode, p) = build_for_provider("openai", "gpt-5", None, &resolver)
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
        let result = build_for_provider("openai", "gpt-5", None, &resolver).await;
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
        let result = build_for_provider("gemini", "gemini-2.5-pro", None, &resolver).await;
        assert!(result.is_ok(), "expected Ok(provider), got error");
    }

    #[tokio::test]
    async fn build_gemini_missing_credential_errors() {
        let _guard = ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        let result = build_for_provider("gemini", "gemini-2.5-pro", None, &resolver).await;
        assert!(matches!(
            result,
            Err(FactoryError::MissingCredential { .. })
        ));
    }
}
