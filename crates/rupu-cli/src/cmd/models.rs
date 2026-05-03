//! `rupu models list | refresh`.

use std::process::ExitCode;

use clap::Subcommand;
use rupu_providers::{ModelRegistry, ModelSource};

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List available models (custom + cached + baked-in).
    List {
        /// Filter output to a single provider.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Re-fetch live model lists from each provider.
    Refresh {
        /// Limit refresh to a single provider.
        #[arg(long)]
        provider: Option<String>,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List { provider } => match list(provider).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rupu models list: {e}");
                ExitCode::FAILURE
            }
        },
        Action::Refresh { provider } => match refresh(provider).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rupu models refresh: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

const PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

async fn list(filter: Option<String>) -> anyhow::Result<()> {
    let registry = build_registry().await?;
    println!(
        "{:<10} {:<32} {:<10} CONTEXT",
        "PROVIDER", "MODEL", "SOURCE"
    );
    for p in &PROVIDERS {
        if let Some(only) = &filter {
            if only != p {
                continue;
            }
        }
        let models = registry.list(p).await;
        for m in models {
            let src = match m.source {
                ModelSource::Custom => "custom",
                ModelSource::Live => "live",
                ModelSource::BakedIn => "baked-in",
            };
            let ctx = if m.entry.context_window > 0 {
                m.entry.context_window.to_string()
            } else {
                "-".to_string()
            };
            println!("{:<10} {:<32} {:<10} {}", p, m.entry.id, src, ctx);
        }
    }
    Ok(())
}

async fn refresh(filter: Option<String>) -> anyhow::Result<()> {
    let registry = build_registry().await?;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    for p in &PROVIDERS {
        if let Some(only) = &filter {
            if only != p {
                continue;
            }
        }
        match populate_live(&registry, &resolver, p).await {
            Ok(n) => println!("rupu: refreshed {p} ({n} models)"),
            Err(e) => eprintln!("rupu: skip {p}: {e}"),
        }
        registry.save_cache(p).await.ok();
    }
    Ok(())
}

async fn populate_live(
    registry: &ModelRegistry,
    resolver: &rupu_auth::resolver::KeychainResolver,
    provider: &str,
) -> anyhow::Result<usize> {
    use rupu_auth::CredentialResolver;
    use rupu_providers::provider::LlmProvider;
    let (_, creds) = resolver.get(provider, None).await?;
    // Call list_models on each concrete type so we avoid the Box<dyn LlmProvider>
    // Sync constraint that async_trait imposes on &self methods.
    let models = match provider {
        "anthropic" => {
            let key = match creds {
                rupu_providers::AuthCredentials::ApiKey { key } => key,
                rupu_providers::AuthCredentials::OAuth { access, .. } => access,
            };
            rupu_providers::AnthropicClient::new(key)
                .list_models()
                .await
        }
        "openai" => {
            rupu_providers::OpenAiCodexClient::new(creds, None)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .list_models()
                .await
        }
        "gemini" => {
            rupu_providers::GoogleGeminiClient::new(
                creds,
                rupu_providers::google_gemini::GeminiVariant::GeminiCli,
                None,
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .list_models()
            .await
        }
        "copilot" => {
            rupu_providers::GithubCopilotClient::new(creds, None)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .list_models()
                .await
        }
        _ => anyhow::bail!("unknown provider: {provider}"),
    };
    let n = models.len();
    registry.set_live_cache(provider, models).await;
    Ok(n)
}

async fn build_registry() -> anyhow::Result<ModelRegistry> {
    let cache_dir = if let Ok(o) = std::env::var("RUPU_CACHE_DIR_OVERRIDE") {
        std::path::PathBuf::from(o)
    } else {
        crate::paths::global_dir()?.join("cache/models")
    };
    let registry = ModelRegistry::with_cache_dir(&cache_dir);

    // Baked-in fallback for Copilot (and Gemini until AI Studio is wired).
    registry
        .set_baked_in(
            "copilot",
            ["gpt-4o", "gpt-4o-mini", "claude-sonnet-4", "o4-mini"]
                .iter()
                .map(|id| make_model_info(id, "copilot"))
                .collect(),
        )
        .await;
    registry
        .set_baked_in(
            "gemini",
            ["gemini-2.5-pro", "gemini-2.5-flash", "gemini-1.5-pro"]
                .iter()
                .map(|id| make_model_info(id, "gemini"))
                .collect(),
        )
        .await;

    // Load custom models from config.toml.
    let cfg_path = crate::paths::global_dir()?.join("config.toml");
    if cfg_path.exists() {
        let text = std::fs::read_to_string(&cfg_path)?;
        let cfg: rupu_config::Config = toml::from_str(&text)?;
        for (name, pcfg) in &cfg.providers {
            if pcfg.models.is_empty() {
                continue;
            }
            registry
                .set_custom(
                    name,
                    pcfg.models
                        .iter()
                        .map(|m| {
                            let mut mi = make_model_info(&m.id, name);
                            if let Some(cw) = m.context_window {
                                mi.context_window = cw;
                            }
                            if let Some(mo) = m.max_output {
                                mi.max_output_tokens = mo;
                            }
                            mi
                        })
                        .collect(),
                )
                .await;
        }
    }

    // Load any persisted live caches.
    for p in &PROVIDERS {
        registry.load_cache(p).await.ok();
    }
    Ok(registry)
}

fn make_model_info(id: &str, provider_name: &str) -> rupu_providers::ModelInfo {
    let pid = match provider_name {
        "anthropic" => rupu_providers::ProviderId::Anthropic,
        "openai" => rupu_providers::ProviderId::OpenaiCodex,
        "gemini" => rupu_providers::ProviderId::GoogleGeminiCli,
        "copilot" => rupu_providers::ProviderId::GithubCopilot,
        _ => rupu_providers::ProviderId::Anthropic,
    };
    rupu_providers::ModelInfo {
        id: id.to_string(),
        provider: pid,
        context_window: 0,
        max_output_tokens: 0,
        capabilities: Vec::new(),
        cost: rupu_providers::ModelCost::default(),
        status: rupu_providers::ModelStatus::default(),
    }
}
