//! Build a `Box<dyn LlmProvider>` from a provider-name string +
//! credential lookup. v0 wires Anthropic only; other providers (OpenAI
//! Codex, Copilot, Gemini, local) return a clear "not wired in v0"
//! error so the failure mode is informative rather than a silent
//! provider-discovery miss.
//!
//! When the lifted `rupu-providers` API stabilizes, this file is the
//! one place to extend.

use rupu_providers::provider::LlmProvider;
use std::path::Path;
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

/// Build a provider for `name`. Reads credentials from environment or
/// `auth_json_path` (the chmod-600 fallback file).
pub async fn build_for_provider(
    name: &str,
    model: &str,
    auth_json_path: &Path,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    match name {
        "anthropic" => build_anthropic(model, auth_json_path).await,
        "openai" | "openai_codex" | "codex" | "copilot" | "github_copilot" | "gemini"
        | "google_gemini" | "local" => Err(FactoryError::NotWiredInV0(name.to_string())),
        _ => Err(FactoryError::UnknownProvider(name.to_string())),
    }
}

async fn build_anthropic(
    model: &str,
    auth_json_path: &Path,
) -> Result<Box<dyn LlmProvider>, FactoryError> {
    // Prefer env var; fall back to auth.json.
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok().or_else(|| {
        if !auth_json_path.exists() {
            return None;
        }
        let text = std::fs::read_to_string(auth_json_path).ok()?;
        let val: serde_json::Value = serde_json::from_str(&text).ok()?;
        val.get("anthropic")
            .and_then(|v| v.get("key"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    let api_key = api_key.ok_or_else(|| FactoryError::MissingCredential {
        provider: "anthropic".to_string(),
    })?;

    // model is supplied per-request via LlmRequest.model, not at construction.
    let _ = model;
    let client = rupu_providers::anthropic::AnthropicClient::new(api_key);
    Ok(Box::new(client))
}
