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
        "anthropic" => build_anthropic(creds, model).await?,
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
    let client = match std::env::var("RUPU_ANTHROPIC_BASE_URL_OVERRIDE") {
        Ok(url) => rupu_providers::anthropic::AnthropicClient::from_auth_with_url(auth, url),
        Err(_) => rupu_providers::anthropic::AnthropicClient::from_auth(auth),
    }
    .with_oauth_account_uuid(account_uuid);
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
    _creds: rupu_providers::auth::AuthCredentials,
    _model: &str,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    // Gemini API-key path requires AI Studio endpoint; not yet implemented.
    // OAuth/Vertex path can be wired here when SSO support is verified end-to-end.
    Err(FactoryError::NotWiredInV0(
        "gemini (AI Studio API-key endpoint not yet wired; SSO/Vertex pending verification)"
            .to_string(),
    ))
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
    use rupu_auth::backend::ProviderId;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::stored::StoredCredential;
    use rupu_providers::AuthMode;

    #[tokio::test]
    async fn build_copilot_returns_provider() {
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

    #[tokio::test]
    async fn build_openai_returns_provider() {
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

    #[tokio::test]
    async fn build_gemini_returns_not_wired_until_sso() {
        // Plan 2 reality: Gemini's lifted client needs the AI Studio
        // API-key endpoint (not yet implemented) or the Vertex/SSO path
        // (pending verification). The factory must surface this constraint
        // clearly rather than panic or silently succeed.
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = InMemoryResolver::new();
        // Insert a dummy credential so we exercise the NotWiredInV0 path,
        // not the resolver-side missing-credential path.
        resolver
            .put(
                ProviderId::Gemini,
                AuthMode::ApiKey,
                StoredCredential::api_key("dummy"),
            )
            .await;
        let result = build_for_provider("gemini", "gemini-2.5-pro", None, &resolver).await;
        match result {
            Err(FactoryError::NotWiredInV0(_)) => {}
            Err(e) => panic!("expected NotWiredInV0, got Err({e})"),
            Ok(_) => panic!("expected NotWiredInV0, got Ok(provider)"),
        }
    }
}
