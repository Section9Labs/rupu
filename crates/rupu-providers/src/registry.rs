use std::path::PathBuf;
use std::sync::Arc;

use tracing::info;

use crate::credential_source::CredentialSource;
use crate::error::ProviderError;
use crate::provider::LlmProvider;
use crate::provider_id::ProviderId;

/// Factory for creating LLM provider clients.
/// Uses a CredentialSource for auth instead of resolving credentials itself.
pub struct ProviderRegistry {
    store: Arc<dyn CredentialSource>,
    auth_json_path: Option<PathBuf>,
}

impl ProviderRegistry {
    pub fn new(store: Arc<dyn CredentialSource>, auth_json_path: Option<PathBuf>) -> Self {
        Self {
            store,
            auth_json_path,
        }
    }

    /// Create a provider client for the given ProviderId.
    pub fn create_provider(&self, id: ProviderId) -> Result<Box<dyn LlmProvider>, ProviderError> {
        let creds = self
            .store
            .get(id)
            .ok_or_else(|| ProviderError::MissingAuth {
                provider: id.auth_key().to_string(),
                env_hint: id.env_var_name().to_string(),
            })?;

        match id {
            ProviderId::Anthropic => {
                let auth_method = creds.into_anthropic_auth_method();
                let client = crate::anthropic::AnthropicClient::from_auth_with_store(
                    auth_method,
                    self.store.clone(),
                );
                info!(provider = "anthropic", "provider created");
                Ok(Box::new(client))
            }
            ProviderId::OpenaiCodex => {
                let mut client = crate::openai_codex::OpenAiCodexClient::new(
                    creds,
                    self.auth_json_path.clone(),
                )?;
                client.set_credential_store(self.store.clone());
                info!(provider = "openai-codex", "provider created");
                Ok(Box::new(client))
            }
            ProviderId::GoogleGeminiCli => {
                let client = crate::google_gemini::GoogleGeminiClient::new(
                    creds,
                    crate::google_gemini::GeminiVariant::GeminiCli,
                    self.auth_json_path.clone(),
                )?;
                info!(provider = "google-gemini-cli", "provider created");
                Ok(Box::new(client))
            }
            ProviderId::GoogleAntigravity => {
                let client = crate::google_gemini::GoogleGeminiClient::new(
                    creds,
                    crate::google_gemini::GeminiVariant::Antigravity,
                    self.auth_json_path.clone(),
                )?;
                info!(provider = "google-antigravity", "provider created");
                Ok(Box::new(client))
            }
            ProviderId::GithubCopilot => {
                let client = crate::github_copilot::GithubCopilotClient::new(
                    creds,
                    self.auth_json_path.clone(),
                )?;
                info!(provider = "github-copilot", "provider created");
                Ok(Box::new(client))
            }
        }
    }

    /// Discover all providers with valid credentials.
    pub fn discover_all(&self) -> Vec<Box<dyn LlmProvider>> {
        let mut providers = Vec::new();
        for id in self.store.available() {
            match self.create_provider(id) {
                Ok(provider) => {
                    info!(provider = %id, "discovered provider");
                    providers.push(provider);
                }
                Err(e) => {
                    tracing::debug!(provider = %id, error = %e, "provider creation failed");
                }
            }
        }
        providers
    }

    /// List providers with valid credentials.
    pub fn available_providers(&self) -> Vec<ProviderId> {
        self.store.available()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::CredentialStore;

    fn make_registry(dir: &std::path::Path, json: &str) -> ProviderRegistry {
        let path = dir.join("auth.json");
        std::fs::write(&path, json).unwrap();
        let store =
            Arc::new(CredentialStore::load(path.clone(), dir.join("auth_status.json")).unwrap());
        ProviderRegistry::new(store, Some(path))
    }

    #[test]
    fn test_create_anthropic_provider() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-ant-test-key"}}"#,
        );
        assert!(registry.create_provider(ProviderId::Anthropic).is_ok());
    }

    #[test]
    fn test_create_openai_provider() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(
            dir.path(),
            r#"{"openai-codex":{"type":"oauth","access":"tok","refresh":"ref","expires":9999999999999}}"#,
        );
        assert!(registry.create_provider(ProviderId::OpenaiCodex).is_ok());
    }

    #[test]
    fn test_create_google_gemini_provider() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(
            dir.path(),
            r#"{"google-gemini-cli":{"type":"oauth","access":"gtoken","refresh":"gref","expires":9999999999999,"project_id":"my-project"}}"#,
        );
        assert!(registry
            .create_provider(ProviderId::GoogleGeminiCli)
            .is_ok());
    }

    #[test]
    fn test_create_github_copilot_provider() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(
            dir.path(),
            r#"{"github-copilot":{"type":"oauth","access":"ghu_test","refresh":"","expires":0}}"#,
        );
        assert!(registry.create_provider(ProviderId::GithubCopilot).is_ok());
    }

    #[test]
    fn test_create_provider_missing_auth() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(dir.path(), r#"{}"#);
        assert!(registry.create_provider(ProviderId::Anthropic).is_err());
    }

    #[test]
    fn test_available_providers() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(
            dir.path(),
            r#"{
                "anthropic": {"type":"api_key","key":"sk-test"},
                "openai-codex": {"type":"oauth","access":"tok","refresh":"ref","expires":999}
            }"#,
        );
        let available = registry.available_providers();
        assert!(available.contains(&ProviderId::Anthropic));
        assert!(available.contains(&ProviderId::OpenaiCodex));
    }

    #[test]
    fn test_available_providers_empty_auth() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(dir.path(), r#"{}"#);
        assert!(registry.available_providers().is_empty());
    }

    #[test]
    fn discover_all_from_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        let registry = make_registry(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test-discover"}}"#,
        );
        let providers = registry.discover_all();
        assert!(
            providers
                .iter()
                .any(|p| p.provider_id() == ProviderId::Anthropic),
            "should discover Anthropic from auth.json"
        );
    }
}
