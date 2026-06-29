//! `rupu cp serve` adapter for rupu-cp's `DefinitionGenerator` port. Calls
//! the orchestrator generation core with the real credential resolver and
//! gathers available agent names for workflow generation.

use std::path::PathBuf;

use rupu_auth::CredentialResolver as _;
use rupu_cp::definition_generator::{
    DefKind, DefinitionGenerator, GenDefError, GenerateDefRequest, GeneratedDef, ProviderModels,
};

pub struct RuntimeDefinitionGenerator {
    pub global_dir: PathBuf,
}

#[async_trait::async_trait]
impl DefinitionGenerator for RuntimeDefinitionGenerator {
    async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
        let resolver = rupu_auth::KeychainResolver::new();
        let (provider, model) = match (req.provider, req.model) {
            (Some(p), Some(m)) => (p, m),
            (Some(p), None) => {
                let m = rupu_orchestrator::generate::DEFAULT_GEN_MODELS
                    .iter()
                    .find(|(name, _)| *name == p)
                    .map(|(_, m)| m.to_string())
                    .ok_or_else(|| GenDefError::Failed(format!("unknown provider `{p}`")))?;
                (p, m)
            }
            (None, Some(m)) => {
                return Err(GenDefError::Failed(format!(
                    "model `{m}` requires a provider to be set"
                )))
            }
            (None, None) => rupu_orchestrator::pick_default_gen_model(&resolver)
                .await
                .ok_or(GenDefError::NoCredentials)?,
        };
        let (kind, available_agents) = match req.kind {
            DefKind::Agent => (rupu_orchestrator::GenKind::Agent, vec![]),
            DefKind::Workflow => {
                let agents = rupu_agent::load_agents(&self.global_dir, None)
                    .map(|specs| specs.into_iter().map(|s| s.name).collect())
                    .unwrap_or_default();
                (rupu_orchestrator::GenKind::Workflow, agents)
            }
        };
        let gen_req = rupu_orchestrator::GenerateRequest {
            kind,
            description: req.description,
            provider,
            model,
            available_agents,
        };
        let out = rupu_orchestrator::generate_definition(&gen_req, &resolver)
            .await
            .map_err(|e| match e {
                rupu_orchestrator::GenerateError::NoCredentials => GenDefError::NoCredentials,
                other => GenDefError::Failed(other.to_string()),
            })?;
        Ok(GeneratedDef {
            raw: out.content,
            provider: out.provider,
            model: out.model,
            attempts: out.attempts,
        })
    }

    async fn available_models(&self) -> Vec<ProviderModels> {
        let resolver = rupu_auth::KeychainResolver::new();
        let default = rupu_orchestrator::pick_default_gen_model(&resolver).await;
        let mut out = Vec::new();
        for &(provider, model) in rupu_orchestrator::generate::DEFAULT_GEN_MODELS {
            if resolver.get(provider, None).await.is_ok() {
                out.push(ProviderModels {
                    provider: provider.to_string(),
                    models: vec![model.to_string()],
                    is_default: default
                        .as_ref()
                        .map(|(p, _)| p == provider)
                        .unwrap_or(false),
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::const_new(());
    const VALID_AGENT_MD: &str = "---\nname: a\ndescription: d\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\n\nbody\n";

    #[tokio::test]
    async fn adapter_generates_agent_via_mock() {
        let _g = ENV_LOCK.lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let script = serde_json::json!([
            { "AssistantText": { "text": VALID_AGENT_MD, "stop": "end_turn" } }
        ])
        .to_string();
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", &script);

        let adapter = RuntimeDefinitionGenerator {
            global_dir: tmp.path().to_path_buf(),
        };
        let out = adapter
            .generate(GenerateDefRequest {
                kind: DefKind::Agent,
                description: "x".into(),
                provider: Some("anthropic".into()),
                model: Some("claude-sonnet-4-6".into()),
            })
            .await
            .expect("ok");
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        assert!(out.raw.contains("name: a"));
    }
}
