//! `rupu models list | refresh`.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput};
use std::process::ExitCode;
use std::sync::Arc;

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

/// The built-in vendor names, in the order they are listed/refreshed.
///
/// Deliberately NOT the set of names these subcommands accept — that is
/// [`provider_names`], which appends every declared `[providers.<name>]`
/// account. Treating this array as the accepted set is what made
/// `--provider <account>` a silent no-op: the filter was compared against
/// these four strings, so a declared account matched no iteration and the
/// loop body never ran.
const PROVIDERS: [&str; 4] = ["anthropic", "openai", "gemini", "copilot"];

/// Every provider name `rupu models` operates on: the built-in vendors,
/// then every declared `[providers.<name>]` account that resolves to a
/// dispatchable kind (`provider_factory::is_dispatchable_provider` — the
/// same predicate `rupu run`'s pre-flight gate uses, so the two cannot
/// drift). Built-ins keep their historical position so table output is
/// stable; accounts follow in config order, duplicates skipped.
fn provider_names(cfg: &rupu_config::Config) -> Vec<String> {
    let mut names: Vec<String> = PROVIDERS.iter().map(|s| (*s).to_string()).collect();
    for name in cfg.providers.keys() {
        if !names.contains(name)
            && rupu_runtime::provider_factory::is_dispatchable_provider(name, &cfg.providers)
        {
            names.push(name.clone());
        }
    }
    names
}

/// What refreshing one provider name actually means, decided by its resolved
/// vendor *kind* rather than by the account name.
///
/// The four fetchable variants are the vendors with a real model-list
/// endpoint; the other two exist so a name that legitimately has no such
/// endpoint reports that fact instead of being silently skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveList {
    Anthropic,
    OpenAi,
    Gemini,
    Copilot,
    /// `openai-compatible`: the models are whatever `[providers.<name>].models`
    /// declares. There is no endpoint to re-fetch — `OpenAiCompatibleClient::
    /// list_models` just replays the configured list.
    ConfigDeclared,
    /// `local` — no provider client is wired at all (`FactoryError::NotWiredInV0`).
    NotWired,
}

/// Map a resolved vendor kind onto its live-listing behavior. Takes the
/// KIND, never the account name, so `anthropic-work` and `anthropic` reach
/// the same arm.
fn live_list_target(kind: &str) -> LiveList {
    match kind {
        "anthropic" => LiveList::Anthropic,
        "openai" | "openai_codex" | "codex" => LiveList::OpenAi,
        "gemini" | "google_gemini" => LiveList::Gemini,
        "copilot" | "github_copilot" => LiveList::Copilot,
        "local" => LiveList::NotWired,
        // Everything else that got this far is dispatchable but not
        // fetchable — today that is only `openai-compatible`.
        _ => LiveList::ConfigDeclared,
    }
}

/// Resolve `--provider <name>` into the list of names to act on.
///
/// `None` means "all of them". A name that resolves to nothing is a hard
/// error with a non-zero exit: previously it matched no loop iteration and
/// the command exited 0 having printed nothing, so a typo was
/// indistinguishable from success.
fn resolve_targets(
    filter: Option<&str>,
    cfg: &rupu_config::Config,
    cfg_path: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    let Some(only) = filter else {
        return Ok(provider_names(cfg));
    };
    if !rupu_runtime::provider_factory::is_dispatchable_provider(only, &cfg.providers) {
        anyhow::bail!(
            "unknown provider '{only}': it is not a built-in vendor name \
             (anthropic | openai | gemini | copilot), and no [providers.{only}] in {} \
             declares its vendor kind. Declare the account with `rupu auth login \
             --account {only} --kind <vendor>`, or declare an openai-compatible \
             endpoint as [providers.{only}] with kind = \"openai-compatible\" and a \
             base_url.",
            cfg_path.display()
        );
    }
    Ok(vec![only.to_string()])
}

/// Path of the global `config.toml` these subcommands read.
fn global_config_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::paths::global_dir()?.join("config.toml"))
}

/// Load the global `config.toml`, or an empty config if there isn't one.
///
/// Deliberately global-only and raw `toml::from_str`, matching what
/// `build_registry` has always done — `rupu models` has never layered a
/// project-level config on top and this change does not start.
fn load_global_config() -> anyhow::Result<rupu_config::Config> {
    let cfg_path = global_config_path()?;
    if !cfg_path.exists() {
        return Ok(rupu_config::Config::default());
    }
    let text = std::fs::read_to_string(&cfg_path)?;
    Ok(toml::from_str(&text)?)
}

/// The resolved vendor kind for a provider name, falling back to the name
/// itself (which for an undeclared built-in IS its own kind).
fn kind_of(name: &str, cfg: &rupu_config::Config) -> String {
    rupu_runtime::provider_factory::resolve_kind(name, &cfg.providers)
        .unwrap_or_else(|| name.to_string())
}

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
    let cfg = load_global_config()?;
    let names = resolve_targets(filter.as_deref(), &cfg, &global_config_path()?)?;
    let registry = build_registry(&cfg).await?;
    let mut rows = Vec::new();
    for p in &names {
        let models = registry.list(p).await;
        for m in models {
            let src = match m.source {
                ModelSource::Custom => "custom",
                ModelSource::Live => "live",
                ModelSource::BakedIn => "baked-in",
            };
            rows.push(ModelListRow {
                provider: p.clone(),
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
    let cfg = load_global_config()?;
    let names = resolve_targets(filter.as_deref(), &cfg, &global_config_path()?)?;
    let registry = build_registry(&cfg).await?;
    // `resolver_for` rather than `KeychainResolver::new()`: a bare resolver
    // knows no declared accounts, so a named account's SSO credential would
    // be read but never refreshed on near-expiry (the defect
    // `crate::accounts::account_specs` exists to prevent).
    let resolver = crate::accounts::resolver_for(&cfg);
    for p in &names {
        let kind = kind_of(p, &cfg);
        let target = live_list_target(&kind);
        let fetch = match target {
            // Not an error and not a silent skip: the account is real, it
            // just has nowhere to fetch from. Say which and where its models
            // do come from.
            LiveList::ConfigDeclared => {
                eprintln!(
                    "rupu: {p}: kind \"{kind}\" has no live model-list endpoint — its models \
                     come from [providers.{p}].models in config.toml (`rupu models list \
                     --provider {p}` shows them)"
                );
                continue;
            }
            LiveList::NotWired => {
                eprintln!("rupu: skip {p}: provider kind \"{kind}\" is not wired for listing");
                continue;
            }
            other => other,
        };
        match populate_live(&registry, &resolver, p, fetch).await {
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

/// Fetch one account's live model list.
///
/// `account` is the credential identity (`anthropic-work`) and is what the
/// resolver is asked for; `target` is the resolved *vendor*, and is what
/// decides which client gets built. They are not interchangeable — passing
/// the account name to the client dispatch is precisely the bug this
/// function used to have (`match provider { "anthropic" => .. }` fell to
/// `_ => bail!("unknown provider")` for every named account, and before that
/// the caller's filter meant it was never even reached).
async fn populate_live(
    registry: &ModelRegistry,
    resolver: &rupu_auth::resolver::KeychainResolver,
    account: &str,
    target: LiveList,
) -> anyhow::Result<usize> {
    use rupu_auth::CredentialResolver;
    use rupu_providers::provider::LlmProvider;
    let (_, creds) = resolver.get(account, None).await?;
    // Call list_models on each concrete type so we avoid the Box<dyn LlmProvider>
    // Sync constraint that async_trait imposes on &self methods.
    let models = match target {
        LiveList::Anthropic => {
            // Important: route OAuth tokens through `from_auth(OAuth)`
            // so the discovery endpoint receives `Authorization:
            // Bearer <token>` + the OAuth beta. Stuffing the OAuth
            // access token into `AnthropicClient::new(key)` builds an
            // api-key client that sends `x-api-key: <bearer>`,
            // which the server rejects with 401 "invalid x-api-key".
            // No run exists yet at this point — `rupu models` is a bare
            // discovery command, not an agent run — so this traffic is
            // deliberately not attributed to a ledger.
            let auth_method = creds.into_anthropic_auth_method();
            // Honour the same `RUPU_ANTHROPIC_BASE_URL_OVERRIDE` test seam
            // `provider_factory::build_anthropic` already reads. Without it
            // this discovery path is the one Anthropic client in the tree
            // that cannot be pointed at a mock server, which is why nothing
            // covered it.
            match std::env::var("RUPU_ANTHROPIC_BASE_URL_OVERRIDE") {
                Ok(url) => rupu_providers::AnthropicClient::from_auth_with_url(
                    auth_method,
                    url,
                    Arc::new(rupu_netflow::NullSink),
                ),
                Err(_) => rupu_providers::AnthropicClient::from_auth(
                    auth_method,
                    Arc::new(rupu_netflow::NullSink),
                ),
            }
            .list_models()
            .await
        }
        LiveList::OpenAi => {
            rupu_providers::OpenAiCodexClient::new(creds, None, Arc::new(rupu_netflow::NullSink))
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .list_models()
                .await
        }
        LiveList::Gemini => {
            rupu_providers::GoogleGeminiClient::new(
                creds,
                rupu_providers::google_gemini::GeminiVariant::GeminiCli,
                None,
                Arc::new(rupu_netflow::NullSink),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .list_models()
            .await
        }
        LiveList::Copilot => {
            rupu_providers::GithubCopilotClient::new(creds, None, Arc::new(rupu_netflow::NullSink))
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .list_models()
                .await
        }
        // Filtered out by `refresh` before this call — the caller reports
        // them rather than reaching a fetch that cannot happen.
        LiveList::ConfigDeclared | LiveList::NotWired => {
            anyhow::bail!("{account}: no live model-list endpoint")
        }
    };
    let n = models.len();
    // Cached under the ACCOUNT name, not the vendor: two accounts of one
    // vendor can be entitled to different models (that is the whole reason
    // someone refreshes a named account), so they must not share a cache
    // file. `models list` reads them back by the same key.
    registry.set_live_cache(account, models).await;
    Ok(n)
}

async fn build_registry(cfg: &rupu_config::Config) -> anyhow::Result<ModelRegistry> {
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

    // Load custom models from config.toml. Keyed by ACCOUNT name (that is
    // how `[providers.<name>].models` is written and how `list` reads it
    // back), but tagged with the account's resolved vendor KIND.
    for (name, pcfg) in &cfg.providers {
        if pcfg.models.is_empty() {
            continue;
        }
        let kind = kind_of(name, cfg);
        registry
            .set_custom(
                name,
                pcfg.models
                    .iter()
                    .map(|m| {
                        let mut mi = make_model_info(&m.id, &kind);
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

    // Load any persisted live caches — over the same name set `refresh`
    // writes, or a named account's freshly-refreshed cache would be written
    // and then never read back.
    for p in &provider_names(cfg) {
        registry.load_cache(p).await.ok();
    }
    Ok(registry)
}

/// Build a `ModelInfo` tagged with the vendor for `kind`.
///
/// Takes the resolved KIND, not the account name — `make_model_info(id,
/// "oracle")` used to fall through to the `_` arm and label every custom
/// openai-compatible model as `Anthropic`.
fn make_model_info(id: &str, kind: &str) -> rupu_providers::ModelInfo {
    let pid = match kind {
        "anthropic" => rupu_providers::ProviderId::Anthropic,
        "openai" | "openai_codex" | "codex" => rupu_providers::ProviderId::OpenaiCodex,
        "gemini" | "google_gemini" => rupu_providers::ProviderId::GoogleGeminiCli,
        "copilot" | "github_copilot" => rupu_providers::ProviderId::GithubCopilot,
        "openai-compatible" => rupu_providers::ProviderId::OpenaiCompatible,
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

/// The name/kind resolution these subcommands do before any I/O. Pure, so
/// the rules are pinned without credentials, a network, or a config file.
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with(entries: &[(&str, rupu_config::ProviderConfig)]) -> rupu_config::Config {
        let mut cfg = rupu_config::Config::default();
        for (name, p) in entries {
            cfg.providers.insert((*name).to_string(), p.clone());
        }
        cfg
    }

    fn declared(kind: &str) -> rupu_config::ProviderConfig {
        rupu_config::ProviderConfig {
            kind: Some(kind.to_string()),
            ..Default::default()
        }
    }

    fn oai_compatible() -> rupu_config::ProviderConfig {
        rupu_config::ProviderConfig {
            kind: Some("openai-compatible".into()),
            base_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        }
    }

    fn path() -> std::path::PathBuf {
        std::path::PathBuf::from("/tmp/config.toml")
    }

    /// The dispatch table `populate_live` matches on. Takes the KIND, so a
    /// named account reaches the same arm as the bare vendor.
    #[test]
    fn live_list_target_follows_the_kind_including_aliases() {
        assert_eq!(live_list_target("anthropic"), LiveList::Anthropic);
        assert_eq!(live_list_target("openai"), LiveList::OpenAi);
        assert_eq!(live_list_target("openai_codex"), LiveList::OpenAi);
        assert_eq!(live_list_target("codex"), LiveList::OpenAi);
        assert_eq!(live_list_target("gemini"), LiveList::Gemini);
        assert_eq!(live_list_target("google_gemini"), LiveList::Gemini);
        assert_eq!(live_list_target("copilot"), LiveList::Copilot);
        assert_eq!(live_list_target("github_copilot"), LiveList::Copilot);
        assert_eq!(live_list_target("local"), LiveList::NotWired);
        assert_eq!(
            live_list_target("openai-compatible"),
            LiveList::ConfigDeclared
        );
    }

    /// End to end through the resolution the CLI does: an account NAME
    /// resolves to its declared vendor kind, and that kind picks the client.
    /// `anthropic-work` must reach the Anthropic arm, `openai-work` the
    /// OpenAI one — the account name itself matches neither.
    #[test]
    fn a_named_account_resolves_to_its_vendors_client() {
        let cfg = cfg_with(&[
            ("anthropic-work", declared("anthropic")),
            ("openai-work", declared("openai")),
        ]);
        assert_eq!(
            live_list_target(&kind_of("anthropic-work", &cfg)),
            LiveList::Anthropic
        );
        assert_eq!(
            live_list_target(&kind_of("openai-work", &cfg)),
            LiveList::OpenAi
        );
    }

    #[test]
    fn kind_of_falls_back_to_the_name_for_undeclared_builtins() {
        let cfg = rupu_config::Config::default();
        assert_eq!(kind_of("anthropic", &cfg), "anthropic");
        assert_eq!(kind_of("copilot", &cfg), "copilot");
    }

    #[test]
    fn provider_names_appends_declared_accounts_after_the_builtins() {
        let cfg = cfg_with(&[
            ("anthropic-work", declared("anthropic")),
            ("oracle", oai_compatible()),
            // Declared but not dispatchable by the LLM factory — excluded.
            ("gh-work", declared("github")),
            // A builtin that also has a config section must not be listed twice.
            ("anthropic", rupu_config::ProviderConfig::default()),
        ]);
        let names = provider_names(&cfg);
        assert_eq!(
            names,
            vec![
                "anthropic".to_string(),
                "openai".to_string(),
                "gemini".to_string(),
                "copilot".to_string(),
                "anthropic-work".to_string(),
                "oracle".to_string(),
            ]
        );
    }

    #[test]
    fn no_filter_targets_every_known_name() {
        let cfg = cfg_with(&[("anthropic-work", declared("anthropic"))]);
        let targets = resolve_targets(None, &cfg, &path()).expect("no filter always resolves");
        assert!(targets.contains(&"anthropic".to_string()));
        assert!(targets.contains(&"anthropic-work".to_string()));
    }

    #[test]
    fn a_declared_account_resolves_as_a_target() {
        let cfg = cfg_with(&[("anthropic-work", declared("anthropic"))]);
        assert_eq!(
            resolve_targets(Some("anthropic-work"), &cfg, &path()).unwrap(),
            vec!["anthropic-work".to_string()]
        );
    }

    #[test]
    fn a_builtin_name_resolves_as_a_target_with_no_config_at_all() {
        let cfg = rupu_config::Config::default();
        assert_eq!(
            resolve_targets(Some("anthropic"), &cfg, &path()).unwrap(),
            vec!["anthropic".to_string()]
        );
    }

    /// The silent-success defect: an unresolvable name must be an Err (which
    /// `handle` turns into a non-zero exit), not an empty target list that
    /// loops zero times and reports nothing.
    #[test]
    fn an_unresolvable_name_is_an_error_naming_both_remedies() {
        let cfg = rupu_config::Config::default();
        let err =
            resolve_targets(Some("anthropc"), &cfg, &path()).expect_err("a typo must not resolve");
        let msg = err.to_string();
        assert!(msg.contains("anthropc"), "names what it tried: {msg}");
        assert!(
            msg.contains("rupu auth login"),
            "remedy 1 — declare the account: {msg}"
        );
        assert!(
            msg.contains("openai-compatible"),
            "remedy 2 — declare an endpoint: {msg}"
        );
        assert!(
            msg.contains("config.toml"),
            "points at the file to edit: {msg}"
        );
    }

    /// A `kind = "openai-compatible"` with no `base_url` builds nothing, so
    /// it is not a usable target either — same rule `rupu run` applies.
    #[test]
    fn a_half_declared_openai_compatible_account_does_not_resolve() {
        let cfg = cfg_with(&[("half", declared("openai-compatible"))]);
        assert!(resolve_targets(Some("half"), &cfg, &path()).is_err());
    }

    #[test]
    fn custom_models_are_tagged_with_the_accounts_vendor_not_anthropic() {
        // Pre-fix, `make_model_info(id, "oracle")` hit the `_` arm and
        // labelled every custom openai-compatible model as Anthropic.
        assert_eq!(
            make_model_info("glm", "openai-compatible").provider,
            rupu_providers::ProviderId::OpenaiCompatible
        );
        assert_eq!(
            make_model_info("gpt-5", "openai").provider,
            rupu_providers::ProviderId::OpenaiCodex
        );
    }
}
