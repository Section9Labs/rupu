//! `rupu models list | refresh`.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use std::process::ExitCode;

use clap::Subcommand;
use rupu_providers::{ModelRegistry, ModelSource};
use serde::Serialize;

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

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    match action {
        Action::List { provider } => match list(provider, global_format).await {
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

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("models list", report::TABLE_JSON_CSV),
        Action::Refresh { .. } => ("models refresh", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

const PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

#[derive(Serialize)]
struct ModelListRow {
    provider: String,
    model: String,
    source: String,
    context: Option<u64>,
}

#[derive(Serialize)]
struct ModelListCsvRow {
    provider: String,
    model: String,
    source: String,
    context: String,
}

#[derive(Serialize)]
struct ModelListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ModelListRow>,
}

struct ModelListOutput {
    report: ModelListReport,
    csv_rows: Vec<ModelListCsvRow>,
}

impl CollectionOutput for ModelListOutput {
    type JsonReport = ModelListReport;
    type CsvRow = ModelListCsvRow;

    fn command_name(&self) -> &'static str {
        "models list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["provider", "model", "source", "context"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["PROVIDER", "MODEL", "SOURCE", "CONTEXT"]);
        for row in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(&row.provider),
                comfy_table::Cell::new(&row.model),
                comfy_table::Cell::new(&row.source),
                comfy_table::Cell::new(
                    row.context
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

async fn list(filter: Option<String>, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let registry = build_registry().await?;
    let mut rows = Vec::new();
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
            rows.push(ModelListRow {
                provider: (*p).to_string(),
                model: m.entry.id,
                source: src.to_string(),
                context: (m.entry.context_window > 0).then_some(m.entry.context_window.into()),
            });
        }
    }
    let csv_rows: Vec<ModelListCsvRow> = rows
        .iter()
        .map(|row| ModelListCsvRow {
            provider: row.provider.clone(),
            model: row.model.clone(),
            source: row.source.clone(),
            context: row
                .context
                .map(|value| value.to_string())
                .unwrap_or_default(),
        })
        .collect();
    let output = ModelListOutput {
        report: ModelListReport {
            kind: "model_list",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
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
            Ok(0) => {
                // Provider returned no models — almost always a
                // silently-swallowed HTTP error (401 / 404 / etc.).
                // Tell the operator to enable `RUST_LOG=warn` so the
                // provider client's tracing logs surface.
                eprintln!(
                    "rupu: refreshed {p} (0 models — re-run with `RUST_LOG=warn` to see why)"
                );
            }
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
            // Important: route OAuth tokens through `from_auth(OAuth)`
            // so the discovery endpoint receives `Authorization:
            // Bearer <token>` + the OAuth beta. Stuffing the OAuth
            // access token into `AnthropicClient::new(key)` builds an
            // api-key client that sends `x-api-key: <bearer>`,
            // which the server rejects with 401 "invalid x-api-key".
            let auth_method = creds.into_anthropic_auth_method();
            rupu_providers::AnthropicClient::from_auth(auth_method)
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
