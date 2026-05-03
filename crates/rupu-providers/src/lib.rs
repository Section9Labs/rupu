#![deny(clippy::all)]

//! LLM provider abstraction and credential-brokered API access.
//!
//! Wraps multiple LLM backends (Anthropic, OpenAI, local models via
//! Neocortex) behind a unified streaming interface. All calls route
//! through the Credential Broker for per-cell budget enforcement and
//! cost tracking. Supports model routing policies defined in each
//! cell's POLICIES.toml (cost tiers, fallback chains, capability
//! requirements).

pub mod anthropic;
pub mod auth;
pub mod auth_mode;
pub mod broker_client;
pub mod broker_types;
pub mod classify;
pub mod concurrency;
pub mod credential_source;
pub mod credential_store;
pub mod error;
pub mod github_copilot;
pub mod google_gemini;
pub mod local;
pub mod model_catalog;
pub mod model_pool;
pub mod model_registry;
pub mod model_scorer;
pub mod model_tier;
pub mod openai_codex;
pub mod provider;
pub mod provider_id;
pub mod registry;
pub mod router;
pub mod routing_history;
pub mod smart_router;
pub mod sse;
pub mod task_classifier;
pub mod types;

pub use anthropic::AnthropicClient;
pub use auth::{
    resolve_anthropic_auth, resolve_provider_auth, save_provider_auth, AuthCredentials, AuthMethod,
};
pub use auth_mode::AuthMode;
pub use broker_client::BrokerClient;
pub use broker_types::{BrokerError, BrokerRequest, BudgetStatus, CallCost, LlmRequestWire};
pub use credential_source::{CredentialSource, ProviderAuthStatus};
pub use credential_store::CredentialStore;
pub use error::ProviderError;
pub use github_copilot::GithubCopilotClient;
pub use google_gemini::GoogleGeminiClient;
pub use local::{LocalModelProvider, RoutingDecision, RoutingPolicy};
pub use model_pool::{
    ModelCapability, ModelCost, ModelInfo, ModelPool, ModelState, ModelStatus, ResponseHeaders,
};
pub use model_registry::{ModelRegistry, ModelSource, ResolvedModel};
pub use model_scorer::{BudgetMode, BudgetState, ScoreBreakdown, ScoredModel};
pub use model_tier::{ModelMap, ModelTier, ThinkingLevel};
pub use openai_codex::OpenAiCodexClient;
pub use provider::LlmProvider;
pub use provider_id::ProviderId;
pub use registry::ProviderRegistry;
pub use router::ProviderRouter;
pub use routing_history::RoutingHistory;
pub use smart_router::SmartRouter;
pub use task_classifier::TaskType;
pub use types::*;

/// Discover all providers with valid credentials.
/// Convenience wrapper: loads CredentialStore from auth.json, creates registry, discovers.
pub fn discover_providers(auth_json_path: &std::path::Path) -> Vec<Box<dyn LlmProvider>> {
    let status_path = auth_json_path.with_file_name("auth_status.json");
    let store = match CredentialStore::load(auth_json_path.to_path_buf(), status_path) {
        Ok(s) => std::sync::Arc::new(s),
        Err(e) => {
            tracing::warn!(error = %e, "failed to load credential store");
            return Vec::new();
        }
    };
    let registry = ProviderRegistry::new(store, Some(auth_json_path.to_path_buf()));
    registry.discover_all()
}
