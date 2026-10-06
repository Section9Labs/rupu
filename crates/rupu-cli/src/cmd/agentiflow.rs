//! `rupu agentiflow run <def>` — launch an agentiflow in the foreground.
//!
//! Wires together: paths → layered config → definition loader → engagement
//! profiles (fail-closed `validate`) → lead agent → provider config → the two
//! provider closures → `rupu_agentiflow::run_agentiflow`. The envelope, lead
//! driver and fleet live in `rupu-agentiflow`; this module only assembles
//! their inputs from the same machinery `rupu run` uses.
//!
//! # The two provider closures
//!
//! `run_agentiflow` takes providers as closures, not instances (a provider is
//! consumed by each run and is not `Clone`). The two call sites are in
//! different contexts, so the closures are built differently:
//!
//! - **`make_provider`** (the lead, one provider per round) is called on
//!   `run_agentiflow`'s plain worker thread, outside any runtime, so it owns a
//!   dedicated runtime and `block_on`s the EXISTING async
//!   `build_for_provider_with_config` each round. That gives the lead every
//!   auth mode, including Anthropic OAuth, and a per-round credential refresh.
//!   It is pre-validated once at launch so a config/auth error is a launch
//!   error rather than a panic in round one.
//! - **`generation.factory`** (a provider for each `generate_workflow` call) is
//!   invoked INSIDE the lead driver's current-thread runtime, where `block_on`
//!   and `block_in_place` both panic, so it must be synchronous. It builds from
//!   a credential resolved once at launch through the sync
//!   `build_provider_from_credential`. Anthropic OAuth cannot be built
//!   synchronously (its session bootstrap awaits the network), so under that
//!   credential generation is left OFF (`generation: None`, a warning), not
//!   faked.

use crate::output::formats::{self, OutputFormat};
use crate::output::report as output_report;
use crate::paths;
use anyhow::{anyhow, Context};
use clap::Subcommand;
use rupu_agentiflow::{
    agentiflow_dir, load_agentiflow_def, new_run_id, run_agentiflow, AgentiflowDef,
    AgentiflowRecord, EnvelopeOutcome, GenerationCapability, LeadInputs, ProviderFactory,
    RunAgentiflowOpts,
};
use rupu_auth::CredentialResolver;
use rupu_runtime::provider_factory::{self, FactoryError, ProviderConfig};
use serde::Serialize;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Run an agentiflow in the foreground until its envelope stops it.
    ///
    /// Loads `<project>/.rupu/agentiflows/<DEF>.yaml` (else
    /// `~/.rupu/agentiflows/<DEF>.yaml`), validates it against its engagement
    /// profiles, and runs it to a stop: goals met, coverage reached, a budget
    /// or ceiling, or an operator stop. Prints the run id up front (stderr) and
    /// the outcome at the end.
    Run {
        /// Agentiflow name (matches an `agentiflows/<name>.yaml` file).
        def: String,
    },
}

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    _absolute: bool,
    _all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::Run { def } => run_cmd(&def, global_format).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        // `{:#}` keeps the cause chain ("build the lead's provider: missing credential ...").
        Err(e) => crate::output::diag::fail(format!("{e:#}")),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::Run { .. } => ("agentiflow run", output_report::TABLE_JSON),
    };
    formats::ensure_supported(command_name, format, supported)
}

async fn run_cmd(def_name: &str, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    // Both the definition loader and the agent loader take the parent of
    // `agentiflows/` / `agents/`, which in a project is `<root>/.rupu`.
    let project_rupu = project_root.as_ref().map(|p| p.join(".rupu"));

    // Layered config, failing on a malformed file as `rupu run` does.
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(
        Some(&global.join("config.toml")),
        project_cfg_path.as_deref(),
    )?;

    // ---- the definition, validated before anything is built -----------------
    let def = load_agentiflow_def(&global, project_rupu.as_deref(), def_name)
        .ok_or_else(|| explain_missing_def(&global, project_rupu.as_deref(), def_name))?;
    // The same overlay-aware registry every other entry point resolves
    // engagement profiles through (`findings_opts::resolve_engagement`):
    // built-ins, then `<global>/profiles`, then `<workspace>/.rupu/profiles`.
    let registry = rupu_coverage::registry_with_overlay(
        &global.join("profiles"),
        &pwd.join(".rupu").join("profiles"),
    )
    .map_err(|e| anyhow!("engagement profiles: {e}"))?;
    let active = def.resolve_profiles(&registry)?;
    def.validate(&active)?;

    // ---- the lead agent -------------------------------------------------------
    let spec = rupu_agent::load_agent(&global, project_rupu.as_deref(), &def.lead)
        .with_context(|| format!("load lead agent `{}`", def.lead))?;

    // Provider + model for the lead, resolved exactly as `rupu run` does.
    let provider_name = provider_factory::resolve_provider_name(
        spec.provider.as_deref(),
        cfg.default_provider.as_deref(),
    );
    let oai_params = provider_factory::openai_compatible_params(&provider_name, &cfg.providers);
    // Pre-flight, before any credential lookup, so a typo'd provider name
    // fails with a config error rather than a confusing auth one.
    if !provider_factory::is_dispatchable_provider(&provider_name, &cfg.providers) {
        anyhow::bail!(
            "provider '{provider_name}' (the lead agent's) is not a built-in provider, is not a \
             declared account (no [providers.{provider_name}] with a vendor `kind` — declare one \
             with `rupu auth login --account {provider_name} --kind <vendor>`), and is not \
             declared as [providers.{provider_name}] with kind = \"openai-compatible\" and a \
             base_url in config.toml"
        );
    }
    let model = provider_factory::resolve_model(
        spec.model.as_deref(),
        cfg.default_model.as_deref(),
        oai_params.as_ref().map(|p| p.default_model.as_str()),
    );
    let lead_pc = ProviderConfig {
        anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
        anthropic_prompt_cache: spec.anthropic_prompt_cache,
        anthropic_server_side_fallback: Some(cfg.recovery.server_side_fallback),
        openai_compatible: oai_params,
        tuning: Some(provider_factory::provider_tuning(
            &provider_name,
            &cfg.providers,
        )),
        kind: provider_factory::resolve_kind(&provider_name, &cfg.providers),
    };

    let run_id = new_run_id();
    let run_dir = agentiflow_dir(&global).join(&run_id);

    // Netflow sink: built before any provider so the launch-time builds below
    // and every round's provider record into this run's ledger. Its transcript
    // half lands in `lead/netflow.jsonl`: the per-round `transcript.rN.jsonl`
    // files are truncated by the runner each round, so they cannot host it.
    let (netflow_sink, netflow_handle) = crate::netflow_sink::for_run(
        &global,
        project_root.as_deref(),
        &run_id,
        &run_dir.join("lead").join("netflow.jsonl"),
    );

    let body = launch(
        def,
        active,
        &spec,
        &cfg,
        &global,
        &pwd,
        run_id.clone(),
        run_dir.clone(),
        provider_name,
        model,
        lead_pc,
        netflow_sink,
    )
    .await;

    // Flush the ledger on the success AND failure paths (see `cmd/run.rs`).
    if let Some(handle) = netflow_handle {
        handle.shutdown().await;
    }
    let (name, outcome) = body?;

    let record_code = AgentiflowRecord::read(&run_dir)
        .ok()
        .and_then(|r| r.stop_reason);
    match format.unwrap_or(OutputFormat::Table) {
        OutputFormat::Json => formats::print_json(&RunReport::new(
            &run_id,
            &name,
            &run_dir,
            record_code,
            &outcome,
        ))?,
        _ => {
            println!("{}", outcome.summary);
            println!(
                "run: {run_id}  ({})",
                run_dir.join("agentiflow.json").display()
            );
        }
    }
    Ok(())
}

/// Everything after the netflow sink exists: pre-validate the lead's provider,
/// classify generation, assemble the options and run to a stop. Split from
/// [`run_cmd`] so the ledger is flushed on every path out of it.
#[allow(clippy::too_many_arguments)]
async fn launch(
    def: AgentiflowDef,
    active: rupu_coverage::ActiveSet,
    spec: &rupu_agent::AgentSpec,
    cfg: &rupu_config::Config,
    global: &Path,
    workspace: &Path,
    run_id: String,
    run_dir: std::path::PathBuf,
    provider_name: String,
    model: String,
    lead_pc: ProviderConfig,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> anyhow::Result<(String, EnvelopeOutcome)> {
    let resolver: Arc<dyn CredentialResolver> = Arc::new(crate::accounts::resolver_for(cfg));
    let auth_hint = spec.auth;

    // The lead's provider, built once now so a missing credential or bad
    // config fails the launch; each round then builds its own (below).
    provider_factory::build_for_provider_with_config(
        &provider_name,
        &model,
        auth_hint,
        resolver.as_ref(),
        &lead_pc,
        sink.clone(),
    )
    .await
    .with_context(|| format!("build the lead's provider `{provider_name}`"))?;

    let generation =
        generation_capability(resolver.clone(), cfg, &provider_name, &model, sink.clone()).await?;

    let make_provider = lead_provider_factory(
        provider_name.clone(),
        model.clone(),
        auth_hint,
        resolver,
        lead_pc,
        sink,
    );

    let name = def.name.clone();
    let opts = RunAgentiflowOpts {
        def,
        workspace: workspace.to_path_buf(),
        global: global.to_path_buf(),
        active,
        started: chrono::Utc::now(),
        now: Box::new(chrono::Utc::now),
        make_provider,
        lead: LeadInputs {
            agent_name: spec.name.clone(),
            system_prompt: spec.system_prompt.clone(),
            provider_name,
            model,
            // What the lead file declares, exactly. `rupu run` treats an absent
            // `tools:` as "all six builtins"; an autonomous lead (it runs under
            // `BypassDecider`) gets none of them unless it asks. `run_agentiflow`
            // adds `report_finding` and the fleet tools on top either way.
            agent_tools: spec.tools.clone().unwrap_or_default(),
        },
        run_id: run_id.clone(),
        // Production: a `SubprocessUnitLauncher` over this binary.
        unit_launcher: None,
        generation,
    };

    eprintln!("agentiflow {name}: run {run_id}");
    // `run_agentiflow` creates and drops the lead driver's runtime, so it must
    // run from a blocking context, never on this async worker.
    let joined = tokio::task::spawn_blocking(move || run_agentiflow(opts)).await;
    match joined {
        Ok(Ok(outcome)) => Ok((name, outcome)),
        Ok(Err(e)) => Err(anyhow::Error::new(e).context(format!("agentiflow run {run_id}"))),
        Err(join_err) => {
            let why = panic_message(join_err);
            // A panic skips the run's own bookkeeping; don't leave the record
            // claiming `running`.
            mark_failed(&run_dir, &why);
            Err(anyhow!("agentiflow run {run_id} aborted: {why}"))
        }
    }
}

/// The lead's per-round provider factory: a dedicated runtime that `block_on`s
/// the async factory each round, so every auth mode builds (Anthropic OAuth
/// included) and each round re-resolves its credential.
///
/// The runtime is created on first use, on `run_agentiflow`'s worker thread
/// (never in an async context, where creating-then-dropping one on an early
/// exit would panic), and lives as long as the closure. It is multi-threaded
/// (one worker) rather than current-thread on purpose: a client built here
/// shares the netflow connection pool keyed by THIS runtime, and an Anthropic
/// OAuth build's bootstrap request leaves a pooled connection whose dispatch
/// task lives here; the lead's own runtime would then send its first message
/// on it, and a current-thread runtime that is not inside `block_on` never
/// polls it. A worker thread keeps those connections driven.
fn lead_provider_factory(
    provider: String,
    model: String,
    auth_hint: Option<rupu_providers::AuthMode>,
    resolver: Arc<dyn CredentialResolver>,
    pc: ProviderConfig,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> ProviderFactory {
    let mut rt: Option<tokio::runtime::Runtime> = None;
    Box::new(move || {
        let rt = rt.get_or_insert_with(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("agentiflow-lead-provider")
                .enable_all()
                .build()
                .expect("build the lead provider runtime")
        });
        // The signature is infallible; the same build succeeded at launch, so
        // a failure here is a credential that went away mid-run.
        rt.block_on(provider_factory::build_for_provider_with_config(
            &provider,
            &model,
            auth_hint,
            resolver.as_ref(),
            &pc,
            sink.clone(),
        ))
        .unwrap_or_else(|e| panic!("lead provider `{provider}` could not be built: {e}"))
        .1
    })
}

/// The lead's `generate_workflow` capability, or `None` when it cannot be
/// offered.
///
/// The generating provider is the first authenticated one in
/// `DEFAULT_GEN_MODELS` order (falling back to the lead's own). Its credential
/// is resolved once here and the factory builds from it synchronously (see the
/// module docs); a provider that can only be built asynchronously, or has no
/// credential, leaves the tool unoffered rather than half-working.
async fn generation_capability(
    resolver: Arc<dyn CredentialResolver>,
    cfg: &rupu_config::Config,
    lead_provider: &str,
    lead_model: &str,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> anyhow::Result<Option<GenerationCapability>> {
    let (gen_provider, gen_model) = rupu_orchestrator::pick_default_gen_model(resolver.as_ref())
        .await
        .unwrap_or_else(|| (lead_provider.to_string(), lead_model.to_string()));
    let gen_pc = generation_provider_config(&gen_provider, cfg);

    let (gen_mode, gen_cred) = match resolver.get(&gen_provider, None).await {
        Ok(resolved) => resolved,
        Err(e) => {
            tracing::warn!(
                provider = %gen_provider,
                error = %e,
                "generate_workflow is unavailable: no credential for the generating provider \
                 (run `rupu auth login --provider {gen_provider}`)"
            );
            return Ok(None);
        }
    };
    let kind = gen_pc.kind.as_deref().unwrap_or(&gen_provider).to_string();
    let refresher = resolver.oauth_refresher(&gen_provider, &kind);

    let build = {
        let (provider, model, pc, sink) = (gen_provider.clone(), gen_model.clone(), gen_pc, sink);
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
        Ok(_) => {
            Ok(Some(GenerationCapability {
                provider: gen_provider.clone(),
                model: gen_model,
                factory: Arc::new(move || {
                    build()
                    .unwrap_or_else(|e| {
                        panic!("generation provider `{gen_provider}` built at launch but not now: {e}")
                    })
                    .1
                }),
            }))
        }
        Err(FactoryError::RequiresAsyncBootstrap { provider }) => {
            tracing::warn!(
                provider = %provider,
                "generate_workflow is unavailable: `{provider}` authenticates with Anthropic OAuth, \
                 which cannot be built where the tool runs (use an API key to enable it)"
            );
            Ok(None)
        }
        Err(e) => Err(anyhow::Error::new(e)
            .context(format!("build the generation provider `{gen_provider}`"))),
    }
}

/// `ProviderConfig` for the generating provider: the operator's
/// `[providers.<name>]` settings and no agent-level overrides (as the
/// `agent generate` / `workflow generate` call sites build it).
fn generation_provider_config(provider: &str, cfg: &rupu_config::Config) -> ProviderConfig {
    ProviderConfig {
        anthropic_oauth_system_prefix: None,
        anthropic_prompt_cache: None,
        anthropic_server_side_fallback: Some(cfg.recovery.server_side_fallback),
        openai_compatible: provider_factory::openai_compatible_params(provider, &cfg.providers),
        tuning: Some(provider_factory::provider_tuning(provider, &cfg.providers)),
        kind: provider_factory::resolve_kind(provider, &cfg.providers),
    }
}

/// Why `load_agentiflow_def` returned `None`: it folds "no such file" and
/// "the file does not parse" together, and an operator needs to tell them
/// apart. Follows the loader's own shadowing (project, then global).
fn explain_missing_def(global: &Path, project: Option<&Path>, name: &str) -> anyhow::Error {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
        return anyhow!("`{name}` is not a valid agentiflow name");
    }
    let file = format!("{name}.yaml");
    let candidates: Vec<std::path::PathBuf> = project
        .map(|p| p.join("agentiflows").join(&file))
        .into_iter()
        .chain(std::iter::once(global.join("agentiflows").join(&file)))
        .collect();
    for path in &candidates {
        if path.is_file() {
            let parsed = std::fs::read_to_string(path)
                .map_err(anyhow::Error::from)
                .and_then(|raw| AgentiflowDef::parse_str(&raw).map_err(anyhow::Error::from));
            return match parsed {
                Err(e) => anyhow!(
                    "agentiflow `{name}` ({}) is not loadable: {e}",
                    path.display()
                ),
                Ok(_) => anyhow!(
                    "agentiflow `{name}` ({}) could not be loaded",
                    path.display()
                ),
            };
        }
    }
    let searched = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    anyhow!("agentiflow `{name}` not found (looked for {searched})")
}

fn panic_message(e: tokio::task::JoinError) -> String {
    if !e.is_panic() {
        return e.to_string();
    }
    let payload = e.into_panic();
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the run panicked".to_string())
}

/// Best-effort: a run whose worker panicked never wrote its final record, so
/// close it out as failed. Leaves a record that already finished alone.
fn mark_failed(run_dir: &Path, why: &str) {
    let Ok(mut record) = AgentiflowRecord::read(run_dir) else {
        return;
    };
    if record.status != "running" {
        return;
    }
    record.status = "failed".into();
    record.stop_reason = Some(format!("error: {why}"));
    record.ended_at = Some(chrono::Utc::now());
    if let Err(e) = record.write(run_dir) {
        tracing::warn!(error = %e, "could not record the aborted agentiflow run");
    }
}

#[derive(Serialize)]
struct GoalRow {
    id: String,
    met: bool,
    current: u64,
    target: u64,
    detail: String,
}

#[derive(Serialize)]
struct CoverageRow {
    met: bool,
    fraction: f64,
}

/// `--format json` for `agentiflow run`.
#[derive(Serialize)]
struct RunReport {
    kind: &'static str,
    version: u8,
    id: String,
    name: String,
    run_dir: String,
    /// The stable code persisted in `agentiflow.json` (`goals_met`,
    /// `budget_exhausted:<dimension>`, ...); `null` if the record could not be
    /// read back.
    stop_reason: Option<String>,
    /// The same stop, as prose.
    stop: String,
    rounds: u32,
    goals: Vec<GoalRow>,
    coverage: Option<CoverageRow>,
    summary: String,
}

impl RunReport {
    fn new(
        id: &str,
        name: &str,
        run_dir: &Path,
        stop_reason: Option<String>,
        outcome: &EnvelopeOutcome,
    ) -> Self {
        Self {
            kind: "agentiflow_run",
            version: 1,
            id: id.to_string(),
            name: name.to_string(),
            run_dir: run_dir.display().to_string(),
            stop_reason,
            stop: outcome.stop.to_string(),
            rounds: outcome.rounds,
            goals: outcome
                .goals
                .iter()
                .map(|g| GoalRow {
                    id: g.id.clone(),
                    met: g.met,
                    current: g.current,
                    target: g.target,
                    detail: g.detail.clone(),
                })
                .collect(),
            coverage: outcome.coverage.as_ref().map(|c| CoverageRow {
                met: c.met,
                fraction: c.fraction,
            }),
            summary: outcome.summary.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_missing_definition_names_both_places_it_looked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (global, project) = (tmp.path().join("g"), tmp.path().join("p"));
        let msg = explain_missing_def(&global, Some(&project), "acme").to_string();
        assert!(msg.contains("not found"), "{msg}");
        assert!(msg.contains(&project.join("agentiflows/acme.yaml").display().to_string()));
        assert!(msg.contains(&global.join("agentiflows/acme.yaml").display().to_string()));
    }

    #[test]
    fn a_definition_that_exists_but_does_not_parse_says_so() {
        let tmp = tempfile::TempDir::new().unwrap();
        write(&tmp.path().join("agentiflows/acme.yaml"), "name: [\n");
        let msg = explain_missing_def(tmp.path(), None, "acme").to_string();
        assert!(msg.contains("is not loadable"), "{msg}");
    }

    #[test]
    fn a_name_that_could_leave_the_directory_is_refused() {
        let tmp = tempfile::TempDir::new().unwrap();
        for bad in ["", "../x", "a/b", "a\\b"] {
            let msg = explain_missing_def(tmp.path(), None, bad).to_string();
            assert!(msg.contains("not a valid agentiflow name"), "{bad}: {msg}");
        }
    }

    #[test]
    fn a_panicked_run_is_closed_out_as_failed_but_a_finished_one_is_left_alone() {
        let tmp = tempfile::TempDir::new().unwrap();
        let record = |status: &str| AgentiflowRecord {
            id: "af_x".into(),
            name: "acme".into(),
            engagement_profiles: vec!["code".into()],
            trigger: rupu_runtime::RunTriggerSource::Agentiflow,
            status: status.into(),
            stop_reason: None,
            rounds: 0,
            goals: Vec::new(),
            started_at: chrono::Utc::now(),
            ended_at: None,
            codename: None,
        };

        record("running").write(tmp.path()).unwrap();
        mark_failed(tmp.path(), "boom");
        let after = AgentiflowRecord::read(tmp.path()).unwrap();
        assert_eq!(after.status, "failed");
        assert_eq!(after.stop_reason.as_deref(), Some("error: boom"));
        assert!(after.ended_at.is_some());

        let mut done = record("completed");
        done.stop_reason = Some("goals_met".into());
        done.write(tmp.path()).unwrap();
        mark_failed(tmp.path(), "boom");
        let after = AgentiflowRecord::read(tmp.path()).unwrap();
        assert_eq!(after.status, "completed");
        assert_eq!(after.stop_reason.as_deref(), Some("goals_met"));
    }

    #[test]
    fn run_takes_a_definition_name_and_only_table_or_json() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["rupu", "agentiflow", "run", "acme"]).unwrap();
        let crate::Cmd::Agentiflow { action } = cli.command else {
            panic!("expected Cmd::Agentiflow");
        };
        let Action::Run { def } = &action;
        assert_eq!(def, "acme");
        assert!(ensure_output_format(&action, OutputFormat::Json).is_ok());
        assert!(ensure_output_format(&action, OutputFormat::Csv).is_err());
    }
}
