//! Build a `Box<dyn LlmProvider>` from a provider-name string +
//! credential lookup. v0 wires Anthropic only; other providers (OpenAI
//! Codex, Copilot, Gemini, local) return a clear "not wired in v0"
//! error so the failure mode is informative rather than a silent
//! provider-discovery miss.
//!
//! Credentials come from a `&dyn AuthBackend` (keychain or chmod-600
//! JSON file — selected once at the call site by `rupu_auth::select_backend`).
//! The factory does not read `auth.json` directly, keeping the storage
//! abstraction in one place.
//!
//! When the lifted `rupu-providers` API stabilizes, this file is the
//! one place to extend.

use rupu_providers::provider::LlmProvider;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FactoryError {
    #[error(
        "missing credential for provider {provider}: configure with \
         `rupu auth login --provider {provider}` or set the env var the provider expects"
    )]
    MissingCredential { provider: String },
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("provider {0} is not wired in v0; only `anthropic` is currently supported")]
    NotWiredInV0(String),
    #[error("provider construction failed: {0}")]
    Other(String),
}

/// Build a provider for `name`. Reads credentials from `backend`
/// (keychain or JSON file) with an env-var fallback for unattended use.
///
/// Test-only seam: when `RUPU_MOCK_PROVIDER_SCRIPT` is set, the factory
/// builds a `MockProvider` from the JSON script in the env var and
/// ignores `name`/`backend`. Production users never set this; tests
/// use it to drive the agent loop end-to-end without an API key.
pub async fn build_for_provider(
    name: &str,
    model: &str,
    backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    if let Ok(json) = std::env::var("RUPU_MOCK_PROVIDER_SCRIPT") {
        return build_mock_from_script(&json);
    }
    match name {
        "anthropic" => build_anthropic(model, backend).await,
        "openai" | "openai_codex" | "codex" | "copilot" | "github_copilot" | "gemini"
        | "google_gemini" | "local" => Err(FactoryError::NotWiredInV0(name.to_string())),
        _ => Err(FactoryError::UnknownProvider(name.to_string())),
    }
}

fn build_mock_from_script(json: &str) -> Result<Box<dyn LlmProvider>, FactoryError> {
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    let turns: Vec<ScriptedTurn> =
        serde_json::from_str(json).map_err(|e| FactoryError::Other(format!("mock script: {e}")))?;
    Ok(Box::new(MockProvider::new(turns)))
}

async fn build_anthropic(
    model: &str,
    backend: &dyn rupu_auth::AuthBackend,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    // model is supplied per-request via LlmRequest.model, not at construction.
    let _ = model;
    let api_key = match backend.retrieve(rupu_auth::ProviderId::Anthropic) {
        Ok(k) => k,
        Err(_) => {
            // Fall back to env var for unattended use cases (CI etc).
            std::env::var("ANTHROPIC_API_KEY").map_err(|_| FactoryError::MissingCredential {
                provider: "anthropic".to_string(),
            })?
        }
    };
    let client = rupu_providers::anthropic::AnthropicClient::new(api_key);
    Ok(Box::new(client))
}
