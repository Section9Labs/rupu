//! The production [`HopBuilder`]: builds a fallback hop's provider through the
//! same factory, credential resolver, provider table and netflow sink the
//! launch site used for the primary provider, then resolves the hop model's
//! limits. The agent runner only knows the trait; every launch site (`rupu
//! run`, session turns, sub-agent dispatch, workflow steps) hands it one of
//! these.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rupu_agent::recovery::{Hop, HopBuilder, RecoveryOpts};

use crate::model_limits::{self, LimitOverrides, LimitsContext};
use crate::provider_factory::{
    build_for_provider_with_config, is_dispatchable_provider, provider_config_for, ProviderConfig,
};

/// The agent's own provider settings, which a hop keeps. Each launch site
/// passes exactly what it put into its primary provider's `ProviderConfig`
/// and auth hint.
#[derive(Debug, Clone, Default)]
pub struct AgentOverrides {
    /// `anthropicOauthPrefix`. Applied to every hop; non-Anthropic clients
    /// ignore it.
    pub oauth_prefix: Option<bool>,
    /// `anthropicPromptCache`. Applied to every hop; non-Anthropic clients
    /// ignore it.
    pub prompt_cache: Option<bool>,
    /// The agent's `auth:` mode. Applied only to a hop on `origin_provider`:
    /// it names how the agent reaches its own provider, and says nothing
    /// about another provider's credentials.
    pub auth: Option<rupu_providers::AuthMode>,
    /// The provider the run started on.
    pub origin_provider: String,
}

/// Builds a fallback hop's provider from config. It uses the same
/// credential resolver, `[providers]` table and netflow sink as the run's
/// primary provider, plus the agent's own provider settings
/// ([`AgentOverrides`]), so a hop to the same provider differs only in
/// model. It then resolves the hop model's limits through config and
/// discovery.
pub struct RuntimeHopBuilder {
    pub resolver: Arc<dyn rupu_auth::CredentialResolver>,
    /// `[providers.<name>]`, the same table the primary provider was built from.
    pub providers: BTreeMap<String, rupu_config::ProviderConfig>,
    pub limits_ctx: LimitsContext,
    /// The run's netflow ledger: a hop's requests belong to the same run.
    pub sink: Arc<dyn rupu_netflow::FlowSink>,
    /// `[recovery].server_side_fallback`, applied to the hop's provider too.
    pub server_side_fallback: bool,
    pub agent_overrides: AgentOverrides,
}

impl RuntimeHopBuilder {
    /// The `ProviderConfig` and auth hint a hop on `provider` is built with.
    pub fn hop_config(&self, provider: &str) -> (ProviderConfig, Option<rupu_providers::AuthMode>) {
        let mut cfg = provider_config_for(provider, &self.providers);
        cfg.anthropic_server_side_fallback = Some(self.server_side_fallback);
        cfg.anthropic_oauth_system_prefix = self.agent_overrides.oauth_prefix;
        cfg.anthropic_prompt_cache = self.agent_overrides.prompt_cache;
        let auth = (provider == self.agent_overrides.origin_provider)
            .then_some(self.agent_overrides.auth)
            .flatten();
        (cfg, auth)
    }
}

#[async_trait]
impl HopBuilder for RuntimeHopBuilder {
    async fn build(&self, provider: &str, model: &str) -> Result<Hop, String> {
        if !is_dispatchable_provider(provider, &self.providers) {
            return Err(format!("provider {provider} is not configured"));
        }
        let (cfg, auth_hint) = self.hop_config(provider);
        let (_mode, mut provider_box) = build_for_provider_with_config(
            provider,
            model,
            auth_hint,
            self.resolver.as_ref(),
            &cfg,
            self.sink.clone(),
        )
        .await
        .map_err(|e| e.to_string())?;
        // A hop carries no agent pins of its own: the origin's pins were for
        // the origin model, so the hop's limits come from config and discovery.
        let limits = model_limits::resolve(
            LimitOverrides::default(),
            provider,
            model,
            provider_box.as_mut(),
            &self.limits_ctx,
        )
        .await;
        Ok(Hop {
            provider: provider_box,
            provider_name: provider.into(),
            model: model.into(),
            limits,
        })
    }
}

/// One run's [`RecoveryOpts`]: the agent's `fallbacks:` (else the
/// `[recovery].fallbacks` table) and a [`RuntimeHopBuilder`] over the
/// resolver, provider table, limits context and netflow sink the run's
/// primary provider was built with, keeping the agent's own provider settings.
/// Every launch site calls this, so none of them assembles the builder by
/// hand.
pub fn recovery_opts(
    recovery: &rupu_config::RecoveryConfig,
    agent_fallbacks: Option<&[rupu_config::FallbackEntry]>,
    resolver: Arc<dyn rupu_auth::CredentialResolver>,
    providers: BTreeMap<String, rupu_config::ProviderConfig>,
    limits_ctx: LimitsContext,
    sink: Arc<dyn rupu_netflow::FlowSink>,
    agent_overrides: AgentOverrides,
) -> RecoveryOpts {
    RecoveryOpts {
        chain: recovery.chain_for(agent_fallbacks),
        hop_builder: Some(Arc::new(RuntimeHopBuilder {
            resolver,
            providers,
            limits_ctx,
            sink,
            server_side_fallback: recovery.server_side_fallback,
            agent_overrides,
        })),
    }
}
