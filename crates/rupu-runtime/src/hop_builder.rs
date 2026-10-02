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
    build_for_provider_with_config, is_dispatchable_provider, provider_config_for,
};

pub struct RuntimeHopBuilder {
    pub resolver: Arc<dyn rupu_auth::CredentialResolver>,
    /// `[providers.<name>]`, the same table the primary provider was built from.
    pub providers: BTreeMap<String, rupu_config::ProviderConfig>,
    pub limits_ctx: LimitsContext,
    /// The run's netflow ledger: a hop's requests belong to the same run.
    pub sink: Arc<dyn rupu_netflow::FlowSink>,
    /// `[recovery].server_side_fallback`, applied to the hop's provider too.
    pub server_side_fallback: bool,
}

#[async_trait]
impl HopBuilder for RuntimeHopBuilder {
    async fn build(&self, provider: &str, model: &str) -> Result<Hop, String> {
        if !is_dispatchable_provider(provider, &self.providers) {
            return Err(format!("provider {provider} is not configured"));
        }
        let mut cfg = provider_config_for(provider, &self.providers);
        cfg.anthropic_server_side_fallback = Some(self.server_side_fallback);
        let (_mode, mut provider_box) = build_for_provider_with_config(
            provider,
            model,
            None,
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
/// primary provider was built with. Every launch site calls this, so none of
/// them assembles the builder by hand.
pub fn recovery_opts(
    recovery: &rupu_config::RecoveryConfig,
    agent_fallbacks: Option<&[rupu_config::FallbackEntry]>,
    resolver: Arc<dyn rupu_auth::CredentialResolver>,
    providers: BTreeMap<String, rupu_config::ProviderConfig>,
    limits_ctx: LimitsContext,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> RecoveryOpts {
    RecoveryOpts {
        chain: recovery.chain_for(agent_fallbacks),
        hop_builder: Some(Arc::new(RuntimeHopBuilder {
            resolver,
            providers,
            limits_ctx,
            sink,
            server_side_fallback: recovery.server_side_fallback,
        })),
    }
}
