//! How `workflows.generate` authors: a provider/model and a factory that
//! mints a provider for each generation call ([`Generation`]).

use std::sync::Arc;

use rupu_auth::CredentialResolver;
use rupu_providers::LlmProvider;
use rupu_runtime::provider_factory::{self, FactoryError, ProviderConfig};

/// Mints a provider for each generation call. Shared (`Arc`, `Fn`): one
/// generator serves every call of a run.
pub type GenerationProviderFactory = Arc<dyn Fn() -> Box<dyn LlmProvider> + Send + Sync>;

/// The provider/model `workflows.generate` authors with, and a factory that
/// mints a provider for each call.
#[derive(Clone)]
pub struct Generation {
    pub provider: String,
    pub model: String,
    pub factory: GenerationProviderFactory,
}

impl Generation {
    /// The generation capability, or `None` when it cannot be offered.
    ///
    /// The generating provider is the first authenticated one in
    /// `DEFAULT_GEN_MODELS` order (falling back to `lead_provider` /
    /// `lead_model`). Its credential is resolved once here and the factory
    /// builds from it synchronously; a provider that can only be built
    /// asynchronously, or has no credential, leaves generation unavailable
    /// rather than half-working.
    pub async fn resolve(
        resolver: Arc<dyn CredentialResolver>,
        cfg: &rupu_config::Config,
        lead_provider: &str,
        lead_model: &str,
        sink: Arc<dyn rupu_netflow::FlowSink>,
    ) -> Result<Option<Generation>, FactoryError> {
        let (gen_provider, gen_model) =
            rupu_orchestrator::pick_default_gen_model(resolver.as_ref())
                .await
                .unwrap_or_else(|| (lead_provider.to_string(), lead_model.to_string()));
        // The operator's `[providers.<name>]` settings and no agent-level
        // overrides (as the `agent generate` / `workflow generate` call sites
        // build it).
        let gen_pc = ProviderConfig::for_agent(
            &gen_provider,
            &cfg.providers,
            Default::default(),
            Some(cfg.recovery.server_side_fallback),
        );

        let (gen_mode, gen_cred) = match resolver.get(&gen_provider, None).await {
            Ok(resolved) => resolved,
            Err(e) => {
                tracing::warn!(
                    provider = %gen_provider,
                    error = %e,
                    "workflows.generate is unavailable: no credential for the generating provider \
                     (run `rupu auth login --account {gen_provider} --mode <api-key|sso>`)"
                );
                return Ok(None);
            }
        };
        let kind = gen_pc.kind.as_deref().unwrap_or(&gen_provider).to_string();
        let refresher = resolver.oauth_refresher(&gen_provider, &kind);

        let build = {
            let (provider, model, pc, sink) =
                (gen_provider.clone(), gen_model.clone(), gen_pc, sink);
            move || {
                provider_factory::build_provider_from_credential(
                    &provider,
                    &model,
                    gen_mode,
                    &gen_cred,
                    refresher.clone(),
                    &pc,
                    sink.clone(),
                )
            }
        };
        match build() {
            Ok(_) => Ok(Some(Generation {
                provider: gen_provider.clone(),
                model: gen_model,
                factory: Arc::new(move || {
                    build()
                        .unwrap_or_else(|e| {
                            panic!("generation provider `{gen_provider}` built at launch but not now: {e}")
                        })
                        .1
                }),
            })),
            Err(FactoryError::RequiresAsyncBootstrap { provider }) => {
                tracing::warn!(
                    provider = %provider,
                    "workflows.generate is unavailable: `{provider}` authenticates with Anthropic OAuth, \
                     which cannot be built where the tool runs (use an API key to enable it: \
                     `rupu auth login --account {provider} --mode api-key`)"
                );
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}
