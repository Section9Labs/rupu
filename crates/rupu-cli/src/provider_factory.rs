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
    let client = match name {
        "anthropic" => build_anthropic(creds, model, config).await?,
        "openai" | "openai_codex" | "codex" => build_openai(creds, model).await?,
        "gemini" | "google_gemini" => build_gemini(creds, model).await?,
        "copilot" | "github_copilot" => build_copilot(creds, model).await?,
        "local" => return Err(FactoryError::NotWiredInV0("local".to_string())),
        _ => return Err(FactoryError::UnknownProvider(name.to_string())),
    };
    Ok((mode, client))
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
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let client = rupu_providers::openai_codex::OpenAiCodexClient::new(creds, None)
        .map_err(|e| FactoryError::Other(format!("openai client init: {e}")))?;
    Ok(Box::new(client))
}

async fn build_gemini(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
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
        .map_err(|e| FactoryError::Other(format!("gemini client init: {e}")))?;
    Ok(Box::new(client))
}

async fn build_copilot(
    creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    let client = rupu_providers::github_copilot::GithubCopilotClient::new(creds, None)
        .map_err(|e| FactoryError::Other(format!("copilot client init: {e}")))?;
    Ok(Box::new(client))
}

#[cfg(test)]
mod build_copilot_tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

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
    use crate::test_support::ENV_LOCK;
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

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
    use crate::test_support::ENV_LOCK;
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

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
