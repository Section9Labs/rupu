//! Port: drafts agent/workflow definition content from a description via a
//! model. rupu-cp defines it; rupu-cli's `cp serve` provides the adapter
//! backed by `rupu_orchestrator::generate`. Read-only `rupu-cp` runs with
//! `None` → the generate endpoints return 501.

use async_trait::async_trait;

/// Which kind of definition to draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Agent,
    Workflow,
}

#[derive(Debug, Clone)]
pub struct GenerateDefRequest {
    pub kind: DefKind,
    pub description: String,
    /// Provider override; `None` → adapter picks the default.
    pub provider: Option<String>,
    /// Model override; `None` → adapter picks the default.
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GeneratedDef {
    pub raw: String,
    pub provider: String,
    pub model: String,
    pub attempts: u8,
}

/// Providers/models offered in the CP dropdown.
#[derive(Debug, Clone)]
pub struct ProviderModels {
    pub provider: String,
    pub models: Vec<String>,
    pub is_default: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum GenDefError {
    #[error("no authenticated provider; connect one to use AI generation")]
    NoCredentials,
    #[error("generation failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait DefinitionGenerator: Send + Sync {
    async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError>;
    async fn available_models(&self) -> Vec<ProviderModels>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Stub;
    #[async_trait]
    impl DefinitionGenerator for Stub {
        async fn generate(&self, req: GenerateDefRequest) -> Result<GeneratedDef, GenDefError> {
            Ok(GeneratedDef {
                raw: format!("kind={:?}", req.kind),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                attempts: 1,
            })
        }
        async fn available_models(&self) -> Vec<ProviderModels> {
            vec![ProviderModels {
                provider: "anthropic".into(),
                models: vec!["claude-sonnet-4-6".into()],
                is_default: true,
            }]
        }
    }

    #[tokio::test]
    async fn trait_object_dispatches() {
        let g: Arc<dyn DefinitionGenerator> = Arc::new(Stub);
        let out = g
            .generate(GenerateDefRequest {
                kind: DefKind::Agent,
                description: "x".into(),
                provider: None,
                model: None,
            })
            .await
            .unwrap();
        assert!(out.raw.contains("Agent"));
        assert_eq!(g.available_models().await.len(), 1);
    }
}
