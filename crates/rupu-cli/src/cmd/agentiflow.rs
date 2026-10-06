//! `rupu agentiflow run <def>` — launch an agentiflow in the foreground — plus
//! `list` / `status <id>`, which read the runs it leaves under
//! `<global>/agentiflows/`.
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
use crate::output::report::{self as output_report, CollectionOutput, DetailOutput};
use crate::paths;
use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use clap::Subcommand;
use rupu_agentiflow::{
    agentiflow_dir, load_agentiflow_def, new_run_id, run_agentiflow, AgentiflowDef,
    AgentiflowRecord, Budget, CoverageTarget, EnvelopeOutcome, GenerationCapability, GoalTarget,
    LeadInputs, ProviderFactory, RunAgentiflowOpts,
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
    /// List agentiflow runs, newest first.
    ///
    /// Reads `<RUPU_HOME>/agentiflows/af_*/agentiflow.json`. The definition
    /// files that sit beside the run directories are not runs and are not
    /// listed (see `rupu agent list` / `rupu workflow list` for definitions).
    List,
    /// Show one agentiflow run: goals, budget, rounds and why it stopped.
    Status {
        /// Run id (`af_...`): the full id, the compact form `list` prints, or
        /// a unique prefix / suffix of it.
        id: String,
    },
}

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::Run { def } => run_cmd(&def, global_format).await,
        Action::List => list_cmd(global_format, absolute, all_columns),
        Action::Status { id } => status_cmd(&id, global_format),
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
        Action::List => ("agentiflow list", output_report::TABLE_JSON_CSV),
        Action::Status { .. } => ("agentiflow status", output_report::TABLE_JSON),
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
            // IMPORTANT: this MUST stay `new_multi_thread` with at least one
            // worker. Do NOT change it to `new_current_thread`. A current-thread
            // runtime only polls its tasks while a `block_on` is running on it;
            // once the `block_on` below returns, nothing drives the netflow HTTP
            // connection-pool dispatch task the Anthropic-OAuth bootstrap request
            // spawned onto this runtime. The lead's own runtime then reuses that
            // pooled connection for its first message, which is never polled, so
            // the first request hangs (verified in the Task-3 review). A worker
            // thread keeps those connections driven between rounds. There is no
            // timing test for this on purpose (it would be flaky); this comment
            // is the regression guard.
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
                 (run `rupu auth login --account {gen_provider} --mode <api-key|sso>`)"
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
                 which cannot be built where the tool runs (use an API key to enable it: \
                 `rupu auth login --account {provider} --mode api-key`)"
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

// ---- `list` / `status` ------------------------------------------------------

/// `<global>/agentiflows/` holds run directories (`af_<ULID>/`) AND the
/// definition files (`<name>.yaml`) the runs were started from. Only the former
/// are runs, so a candidate must be an `af_`-prefixed DIRECTORY.
const RUN_DIR_PREFIX: &str = "af_";

/// The names of every run directory under `<global>/agentiflows/`, whether or
/// not its record is readable (`status` wants to say why a record is unreadable
/// rather than pretend the run does not exist).
fn run_dir_names(global: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(agentiflow_dir(global)) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(RUN_DIR_PREFIX))
        .collect()
}

/// Every run with a readable `agentiflow.json`, newest first (by `started_at`,
/// the id breaking ties so the order is stable). A directory without a parseable
/// record (a run being laid out, a hand-made stray) is skipped, as the other
/// run listers do.
fn read_runs(global: &Path) -> Vec<AgentiflowRecord> {
    let root = agentiflow_dir(global);
    let mut runs: Vec<AgentiflowRecord> = run_dir_names(global)
        .into_iter()
        .filter_map(|name| {
            let mut record = AgentiflowRecord::read(&root.join(&name)).ok()?;
            // The directory is what `status` resolves, so it names the run.
            record.id = name;
            Some(record)
        })
        .collect();
    runs.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    runs
}

/// One row of `agentiflow list`.
#[derive(Debug, Clone, Serialize)]
struct ListRow {
    id: String,
    name: String,
    status: String,
    stop_reason: Option<String>,
    rounds: u32,
    goals_met: usize,
    goals_total: usize,
    started_at: DateTime<Utc>,
}

/// `--format json` for `agentiflow list`.
#[derive(Debug, Serialize)]
struct ListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ListRow>,
}

fn list_rows(global: &Path) -> Vec<ListRow> {
    read_runs(global)
        .into_iter()
        .map(|r| ListRow {
            goals_met: r.goals.iter().filter(|g| g.met).count(),
            goals_total: r.goals.len(),
            id: r.id,
            name: r.name,
            status: r.status,
            stop_reason: r.stop_reason,
            rounds: r.rounds,
            started_at: r.started_at,
        })
        .collect()
}

struct ListOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: ListReport,
}

impl CollectionOutput for ListOutput {
    type JsonReport = ListReport;
    type CsvRow = ListRow;

    fn command_name(&self) -> &'static str {
        "agentiflow list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "id",
            "name",
            "status",
            "stop_reason",
            "rounds",
            "goals_met",
            "goals_total",
            "started_at",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        if self.report.rows.is_empty() {
            println!(
                "(no agentiflow runs found)\n\nStart one with `rupu agentiflow run <def>`; \
                 definitions live under `.rupu/agentiflows/` (project) or `~/.rupu/agentiflows/` \
                 (global)."
            );
            return Ok(());
        }
        println!("{}", render_list_table(&self.report.rows, &self.prefs));
        Ok(())
    }
}

fn render_list_table(rows: &[ListRow], prefs: &crate::cmd::ui::UiPrefs) -> String {
    use crate::output::entity_table::{CellValue, EntityTable};

    let mut table = EntityTable::new(
        prefs,
        prefs.render_opts(),
        vec!["ID", "NAME", "STATUS", "STOP", "ROUNDS", "GOALS", "STARTED"],
    )
    .with_summary("agentiflow run");
    for row in rows {
        table = table.row(vec![
            CellValue::Id(row.id.clone()),
            CellValue::Name(row.name.clone()),
            CellValue::Status(row.status.clone()),
            row.stop_reason
                .clone()
                .map(CellValue::Text)
                .unwrap_or(CellValue::Missing),
            CellValue::Text(row.rounds.to_string()),
            if row.goals_total == 0 {
                CellValue::Missing
            } else {
                CellValue::Text(format!("{}/{}", row.goals_met, row.goals_total))
            },
            CellValue::Timestamp(row.started_at),
        ]);
    }
    table.render(Utc::now())
}

/// Table preferences for `list`: the layered config's `[ui]`, read tolerantly
/// (a malformed config must not stop a listing), plus the global table flags.
fn table_prefs(absolute: bool, all_columns: bool) -> anyhow::Result<crate::cmd::ui::UiPrefs> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = rupu_config::layer_files_locked(
        Some(&global.join("config.toml")),
        project_root
            .as_ref()
            .map(|p| p.join(".rupu/config.toml"))
            .as_deref(),
    )
    .unwrap_or_default();
    Ok(
        crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None, None)
            .with_table_flags(absolute, all_columns),
    )
}

fn list_cmd(format: Option<OutputFormat>, absolute: bool, all_columns: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let output = ListOutput {
        prefs: table_prefs(absolute, all_columns)?,
        report: ListReport {
            kind: "agentiflow_list",
            version: 1,
            rows: list_rows(&global),
        },
    };
    output_report::emit_collection(format, &output)
}

/// One goal in `agentiflow status`: the record's pass/fail and progress, joined
/// by id to what the run's definition snapshot says the goal is.
#[derive(Debug, Serialize)]
struct StatusGoal {
    id: String,
    met: bool,
    current: u64,
    target: u64,
    /// The goal's objective, from the run's `agentiflow.yaml` snapshot.
    objective: Option<String>,
    /// The goal's predicate in words (findings / asset / count / depth /
    /// verification), from the same snapshot.
    predicate: Option<String>,
    required: Option<bool>,
}

/// Budget as far as a run records it: the definition's caps, and the state the
/// last finished round saw (`ok` / `soft` / `hard:<dimension>`).
#[derive(Debug, Serialize)]
struct StatusBudget {
    state: Option<String>,
    caps: Option<Budget>,
}

/// `--format json` for `agentiflow status`.
#[derive(Debug, Serialize)]
struct StatusReport {
    kind: &'static str,
    version: u8,
    id: String,
    name: String,
    codename: Option<String>,
    status: String,
    /// The stable code persisted in `agentiflow.json`.
    stop_reason: Option<String>,
    rounds: u32,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    engagement_profiles: Vec<String>,
    run_dir: String,
    goals: Vec<StatusGoal>,
    budget: StatusBudget,
    /// The definition's coverage target. A run does not persist the live
    /// fraction, only that it stopped on `coverage_reached`.
    coverage_target: Option<CoverageTarget>,
}

/// A goal predicate in words: what `target:` asks for, plus the verification bar.
fn describe_target(target: &GoalTarget, verify_with: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = &target.findings {
        parts.push(match &f.classification {
            Some(c) => format!("findings classified {c}"),
            None => "findings".to_string(),
        });
    }
    if let Some(a) = &target.asset {
        let locator = a
            .locator
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(if locator.is_empty() {
            format!("asset {}", a.kind)
        } else {
            format!("asset {} ({locator})", a.kind)
        });
    }
    if let Some(n) = target.count_gte {
        parts.push(format!("count >= {n}"));
    }
    if let Some(d) = &target.depth_at_least {
        parts.push(format!("depth >= {d}"));
    }
    if target.verified || verify_with.is_some() {
        let mut v = String::from("verified");
        if let Some(agent) = verify_with {
            v.push_str(&format!(" by {agent}"));
        }
        if matches!(
            target.verify_check,
            Some(rupu_agentiflow::VerifyCheck::WithPoc)
        ) {
            v.push_str(" with a PoC");
        }
        parts.push(v);
    }
    parts.join(", ")
}

/// The budget state the last finished round recorded in `events.jsonl`, if any.
/// Best-effort: the log is a local, minimal append and may be absent or partial.
fn last_round_budget(run_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(run_dir.join("events.jsonl")).ok()?;
    raw.lines().rev().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if v.get("kind")?.as_str()? != "round" {
            return None;
        }
        Some(v.get("budget")?.as_str()?.to_string())
    })
}

/// Resolve `fragment` to a run directory name the way every other entity list
/// does (full id, the compact form `list` prints, or a unique prefix/suffix).
fn resolve_run_id(global: &Path, fragment: &str) -> anyhow::Result<String> {
    use crate::output::ids::{self, Resolution};
    let names = run_dir_names(global);
    match ids::resolve(&names, fragment) {
        Resolution::Unique(id) => Ok(id),
        Resolution::NotFound => Err(anyhow!(
            "agentiflow run `{fragment}` not found under {} (see `rupu agentiflow list`)",
            agentiflow_dir(global).display()
        )),
        Resolution::Ambiguous(matches) => Err(anyhow!(
            "`{fragment}` matches more than one agentiflow run: {}",
            matches.join(", ")
        )),
    }
}

fn load_status(global: &Path, fragment: &str) -> anyhow::Result<StatusReport> {
    let id = resolve_run_id(global, fragment)?;
    let run_dir = agentiflow_dir(global).join(&id);
    let record = AgentiflowRecord::read(&run_dir)
        .with_context(|| format!("read {}", run_dir.join("agentiflow.json").display()))?;
    // The definition the run started from. Best-effort: it adds each goal's
    // objective and predicate and the budget caps, but the record alone is the
    // state of record.
    let def = std::fs::read_to_string(run_dir.join("agentiflow.yaml"))
        .ok()
        .and_then(|raw| AgentiflowDef::parse_str(&raw).ok());

    let goals = record
        .goals
        .iter()
        .map(|g| {
            let def_goal = def
                .as_ref()
                .and_then(|d| d.goals.iter().find(|dg| dg.id == g.id));
            StatusGoal {
                id: g.id.clone(),
                met: g.met,
                current: g.current,
                target: g.target,
                objective: def_goal.map(|dg| dg.objective.trim().to_string()),
                predicate: def_goal
                    .map(|dg| describe_target(&dg.target, dg.verify_with.as_deref())),
                required: def_goal.map(|dg| dg.required),
            }
        })
        .collect();

    Ok(StatusReport {
        kind: "agentiflow_status",
        version: 1,
        id,
        name: record.name,
        codename: record.codename,
        status: record.status,
        stop_reason: record.stop_reason,
        rounds: record.rounds,
        started_at: record.started_at,
        ended_at: record.ended_at,
        engagement_profiles: record.engagement_profiles,
        run_dir: run_dir.display().to_string(),
        goals,
        budget: StatusBudget {
            state: last_round_budget(&run_dir),
            caps: def.as_ref().and_then(|d| d.budget.clone()),
        },
        coverage_target: def.and_then(|d| d.coverage),
    })
}

struct StatusOutput {
    report: StatusReport,
}

impl DetailOutput for StatusOutput {
    type JsonReport = StatusReport;

    fn command_name(&self) -> &'static str {
        "agentiflow status"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        print!("{}", render_status(&self.report));
        Ok(())
    }
}

fn render_status(r: &StatusReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = write!(out, "agentiflow {}  ({})", r.name, r.id);
    if let Some(codename) = &r.codename {
        let _ = write!(out, "  {codename}");
    }
    out.push('\n');
    let _ = writeln!(
        out,
        "  status:    {}{}",
        r.status,
        r.stop_reason
            .as_deref()
            .map(|s| format!("  (stopped: {s})"))
            .unwrap_or_default()
    );
    let _ = writeln!(out, "  rounds:    {}", r.rounds);
    let _ = writeln!(
        out,
        "  started:   {}",
        r.started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    if let Some(ended) = r.ended_at {
        let _ = writeln!(
            out,
            "  ended:     {}",
            ended.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    let _ = writeln!(out, "  profiles:  {}", r.engagement_profiles.join(", "));

    let met = r.goals.iter().filter(|g| g.met).count();
    let _ = writeln!(out, "\ngoals: {met}/{} met", r.goals.len());
    for g in &r.goals {
        let _ = writeln!(
            out,
            "  {} {}  {}/{}{}",
            if g.met { "✓" } else { "○" },
            g.id,
            g.current,
            g.target,
            match g.required {
                Some(false) => "  (optional)",
                _ => "",
            }
        );
        if let Some(objective) = &g.objective {
            let _ = writeln!(out, "      {objective}");
        }
        if let Some(predicate) = g.predicate.as_deref().filter(|p| !p.is_empty()) {
            let _ = writeln!(out, "      predicate: {predicate}");
        }
    }

    let _ = writeln!(
        out,
        "\nbudget: {}",
        r.budget.state.as_deref().unwrap_or("(no round recorded)")
    );
    if let Some(b) = &r.budget.caps {
        let mut caps: Vec<String> = Vec::new();
        if let Some(v) = b.usd {
            caps.push(format!("usd {v}"));
        }
        if let Some(v) = b.tokens {
            caps.push(format!("tokens {v}"));
        }
        if let Some(v) = &b.wall_clock {
            caps.push(format!("wall clock {v}"));
        }
        if let Some(v) = b.rounds {
            caps.push(format!("rounds {v}"));
        }
        if !caps.is_empty() {
            let _ = writeln!(out, "  caps: {}", caps.join(", "));
        }
    }

    if let Some(c) = &r.coverage_target {
        let _ = writeln!(
            out,
            "\ncoverage target: {:.0}%{}{}",
            c.reach * 100.0,
            c.depth
                .as_deref()
                .map(|d| format!(" at depth {d}"))
                .unwrap_or_default(),
            if r.stop_reason.as_deref() == Some("coverage_reached") {
                "  (reached)"
            } else {
                ""
            }
        );
    }
    out
}

fn status_cmd(id: &str, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let report = load_status(&global, id)?;
    output_report::emit_detail(format, &StatusOutput { report })
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
        let action = parse(["rupu", "agentiflow", "run", "acme"]);
        match &action {
            Action::Run { def } => assert_eq!(def, "acme"),
            other => panic!("expected `run`, got {other:?}"),
        }
        assert!(ensure_output_format(&action, OutputFormat::Json).is_ok());
        assert!(ensure_output_format(&action, OutputFormat::Csv).is_err());
    }

    fn parse<const N: usize>(argv: [&str; N]) -> Action {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(argv).unwrap();
        let crate::Cmd::Agentiflow { action } = cli.command else {
            panic!("expected Cmd::Agentiflow");
        };
        action
    }

    #[test]
    fn list_and_status_parse_and_gate_their_formats() {
        let list = parse(["rupu", "agentiflow", "list"]);
        assert!(matches!(list, Action::List));
        for ok in [OutputFormat::Table, OutputFormat::Json, OutputFormat::Csv] {
            assert!(ensure_output_format(&list, ok).is_ok(), "{ok:?}");
        }
        assert!(ensure_output_format(&list, OutputFormat::Jsonl).is_err());

        let status = parse(["rupu", "agentiflow", "status", "af_01ABC"]);
        match &status {
            Action::Status { id } => assert_eq!(id, "af_01ABC"),
            other => panic!("expected `status`, got {other:?}"),
        }
        assert!(ensure_output_format(&status, OutputFormat::Json).is_ok());
        assert!(ensure_output_format(&status, OutputFormat::Csv).is_err());
    }

    // ---- list / status over a seeded `<global>/agentiflows/` ----------------

    const DEF_YAML: &str = r#"
name: acme
description: Assess the in-scope services.
lead: lead
engagement_profiles: [code]
goals:
  - id: rce
    objective: "Find 3 verified RCE issues."
    target: { findings: { classification: "CWE-94" }, count_gte: 3, verified: true }
    required: true
  - id: recon
    objective: "Map the hosts."
    target: { asset: { kind: "network:host" }, count_gte: 5 }
    required: false
coverage:
  reach: 0.9
  depth: tested
budget:
  usd: 50
  wall_clock: "6h"
  rounds: 40
scope:
  authorized: true
  roots: []
pool:
  agents: [lead]
"#;

    fn goal(id: &str, met: bool, current: u64, target: u64) -> rupu_agentiflow::GoalStatus {
        rupu_agentiflow::GoalStatus {
            id: id.into(),
            met,
            current,
            target,
        }
    }

    fn seed(
        global: &Path,
        id: &str,
        name: &str,
        started_secs: i64,
        status: &str,
        stop: Option<&str>,
        goals: Vec<rupu_agentiflow::GoalStatus>,
    ) {
        use chrono::TimeZone as _;
        AgentiflowRecord {
            id: id.into(),
            name: name.into(),
            engagement_profiles: vec!["code".into()],
            trigger: rupu_runtime::RunTriggerSource::Agentiflow,
            status: status.into(),
            stop_reason: stop.map(str::to_string),
            rounds: 3,
            goals,
            started_at: chrono::Utc
                .timestamp_opt(1_800_000_000 + started_secs, 0)
                .unwrap(),
            ended_at: None,
            codename: None,
        }
        .write(&agentiflow_dir(global).join(id))
        .unwrap();
    }

    /// Two runs, the older one written FIRST; plus everything `list` must not
    /// mistake for a run.
    fn seeded_global() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let g = tmp.path();
        seed(
            g,
            "af_01OLDER",
            "acme",
            0,
            "completed",
            Some("goals_met"),
            vec![goal("rce", true, 3, 3), goal("recon", true, 5, 5)],
        );
        seed(
            g,
            "af_01NEWER",
            "acme",
            600,
            "running",
            None,
            vec![goal("rce", false, 1, 3), goal("recon", true, 5, 5)],
        );
        // A definition file beside the run directories: NOT a run.
        write(&agentiflow_dir(g).join("acme.yaml"), DEF_YAML);
        // An `af_` directory with no record (a run being laid out): skipped.
        std::fs::create_dir_all(agentiflow_dir(g).join("af_01NORECORD")).unwrap();
        // A directory that is not `af_`-prefixed, even with a valid record.
        seed(g, "scratch", "ignored", 900, "completed", None, vec![]);
        // An `af_`-named FILE: not a directory.
        write(&agentiflow_dir(g).join("af_01FILE"), "x");
        tmp
    }

    #[test]
    fn list_is_newest_first_and_ignores_definition_files_and_strays() {
        let tmp = seeded_global();
        let rows = list_rows(tmp.path());
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["af_01NEWER", "af_01OLDER"]);

        let newer = &rows[0];
        assert_eq!(newer.name, "acme");
        assert_eq!(newer.status, "running");
        assert_eq!(newer.stop_reason, None);
        assert_eq!(newer.rounds, 3);
        assert_eq!((newer.goals_met, newer.goals_total), (1, 2));
        let older = &rows[1];
        assert_eq!(older.stop_reason.as_deref(), Some("goals_met"));
        assert_eq!((older.goals_met, older.goals_total), (2, 2));
    }

    #[test]
    fn list_over_a_missing_or_empty_directory_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(list_rows(tmp.path()).is_empty());
        std::fs::create_dir_all(agentiflow_dir(tmp.path())).unwrap();
        assert!(list_rows(tmp.path()).is_empty());
    }

    #[test]
    fn list_table_names_each_run_and_json_carries_the_full_id() {
        let tmp = seeded_global();
        let rows = list_rows(tmp.path());
        let cfg = rupu_config::Config::default();
        let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, true, None, None, None);
        let table = render_list_table(&rows, &prefs);
        assert!(table.contains("acme"), "{table}");
        assert!(table.contains("goals_met"), "{table}");
        assert!(table.contains("1/2"), "{table}");
        assert!(table.contains("2 agentiflow runs"), "{table}");
        // The newest run is the first data row.
        let (newer, older) = (table.find("1/2").unwrap(), table.find("2/2").unwrap());
        assert!(newer < older, "{table}");
    }

    #[test]
    fn status_returns_the_one_runs_goals_budget_and_stop() {
        let tmp = seeded_global();
        let g = tmp.path();
        let run_dir = agentiflow_dir(g).join("af_01OLDER");
        write(&run_dir.join("agentiflow.yaml"), DEF_YAML);
        write(
            &run_dir.join("events.jsonl"),
            concat!(
                "{\"kind\":\"run_started\"}\n",
                "{\"kind\":\"round\",\"round\":0,\"budget\":\"ok\"}\n",
                "not json at all\n",
                "{\"kind\":\"round\",\"round\":1,\"budget\":\"soft\"}\n",
                "{\"kind\":\"run_stopped\",\"stop_reason\":\"goals_met\"}\n",
            ),
        );

        let status = load_status(g, "af_01OLDER").unwrap();
        assert_eq!(status.id, "af_01OLDER");
        assert_eq!(status.status, "completed");
        assert_eq!(status.stop_reason.as_deref(), Some("goals_met"));
        assert_eq!(status.rounds, 3);
        assert_eq!(status.goals.len(), 2);

        let rce = &status.goals[0];
        assert_eq!(
            (rce.id.as_str(), rce.met, rce.current, rce.target),
            ("rce", true, 3, 3)
        );
        assert_eq!(
            rce.objective.as_deref(),
            Some("Find 3 verified RCE issues.")
        );
        assert_eq!(rce.required, Some(true));
        let predicate = rce.predicate.as_deref().unwrap();
        assert!(predicate.contains("CWE-94"), "{predicate}");
        assert!(predicate.contains("count >= 3"), "{predicate}");
        assert!(predicate.contains("verified"), "{predicate}");
        assert_eq!(status.goals[1].required, Some(false));

        // The LAST round's budget state, skipping the unparseable line.
        assert_eq!(status.budget.state.as_deref(), Some("soft"));
        let caps = status.budget.caps.as_ref().unwrap();
        assert_eq!(caps.rounds, Some(40));
        assert_eq!(caps.wall_clock.as_deref(), Some("6h"));
        assert_eq!(status.coverage_target.as_ref().unwrap().reach, 0.9);

        let human = render_status(&status);
        assert!(human.contains("goals: 2/2 met"), "{human}");
        assert!(human.contains("stopped: goals_met"), "{human}");
        assert!(human.contains("budget: soft"), "{human}");
        assert!(human.contains("rounds 40"), "{human}");

        // The other run is not mixed in.
        let other = load_status(g, "af_01NEWER").unwrap();
        assert_eq!(other.status, "running");
        assert_eq!(other.stop_reason, None);
        assert!(!other.goals[0].met);
    }

    #[test]
    fn status_without_a_snapshot_or_event_log_still_reports_the_record() {
        let tmp = seeded_global();
        let status = load_status(tmp.path(), "af_01NEWER").unwrap();
        assert_eq!(status.goals.len(), 2);
        assert_eq!(status.goals[0].objective, None);
        assert_eq!(status.goals[0].predicate, None);
        assert_eq!(status.budget.state, None);
        assert!(status.budget.caps.is_none());
        assert!(status.coverage_target.is_none());
        let human = render_status(&status);
        assert!(human.contains("(no round recorded)"), "{human}");
    }

    #[test]
    fn status_resolves_the_fragments_list_prints_and_refuses_the_rest() {
        let tmp = seeded_global();
        let g = tmp.path();
        // A unique suffix resolves.
        assert_eq!(load_status(g, "01OLDER").unwrap().id, "af_01OLDER");
        // Both runs share the `af_01` prefix: ambiguous, never a silent pick.
        let msg = load_status(g, "af_01").unwrap_err().to_string();
        assert!(msg.contains("more than one"), "{msg}");
        assert!(
            msg.contains("af_01NEWER") && msg.contains("af_01OLDER"),
            "{msg}"
        );
        // Unknown, and a path that could leave the directory, are not found.
        for bad in ["af_01MISSING", "../x", "acme.yaml", ""] {
            let msg = load_status(g, bad).unwrap_err().to_string();
            assert!(msg.contains("not found"), "{bad}: {msg}");
        }
    }

    #[test]
    fn status_of_a_run_with_an_unreadable_record_says_so() {
        let tmp = seeded_global();
        let msg = format!(
            "{:#}",
            load_status(tmp.path(), "af_01NORECORD").unwrap_err()
        );
        assert!(msg.contains("agentiflow.json"), "{msg}");
    }

    // ---- generation capability gating ---------------------------------------
    //
    // The mock end-to-end run cannot reach the Anthropic-OAuth branch (the mock
    // seam short-circuits the provider factory), so these are the only coverage
    // of what `generation_capability` offers under each credential.

    async fn classify_generation(
        resolver: rupu_auth::in_memory::InMemoryResolver,
    ) -> Option<GenerationCapability> {
        let cfg = rupu_config::Config::default();
        generation_capability(
            Arc::new(resolver),
            &cfg,
            "anthropic",
            "claude-sonnet-4-6",
            Arc::new(rupu_netflow::NullSink),
        )
        .await
        .expect("classifying generation is never a launch error for these credentials")
    }

    #[tokio::test]
    async fn generation_is_off_under_an_anthropic_oauth_credential() {
        let _g = crate::test_support::ENV_LOCK.lock().await;
        // The mock seam would build a mock for ANY credential and hide the branch.
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = rupu_auth::in_memory::InMemoryResolver::new();
        resolver
            .put(
                rupu_auth::ProviderId::Anthropic,
                rupu_providers::AuthMode::Sso,
                rupu_auth::StoredCredential {
                    credentials: rupu_providers::auth::AuthCredentials::OAuth {
                        access: "sk-ant-oat01-access".into(),
                        refresh: "refresh".into(),
                        expires: u64::MAX,
                        extra: Default::default(),
                    },
                    refresh_token: Some("refresh".into()),
                    expires_at: None,
                },
            )
            .await;
        assert!(classify_generation(resolver).await.is_none());
    }

    #[tokio::test]
    async fn generation_is_on_under_an_anthropic_api_key() {
        let _g = crate::test_support::ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let resolver = rupu_auth::in_memory::InMemoryResolver::new();
        resolver
            .put(
                rupu_auth::ProviderId::Anthropic,
                rupu_providers::AuthMode::ApiKey,
                rupu_auth::StoredCredential::api_key("sk-ant-api03-test"),
            )
            .await;
        let generation = classify_generation(resolver)
            .await
            .expect("an API key can be built synchronously");
        assert_eq!(generation.provider, "anthropic");
        // The factory the tool calls builds without a runtime or the network.
        let _provider = (generation.factory)();
    }

    #[tokio::test]
    async fn generation_is_off_when_the_generating_provider_has_no_credential() {
        let _g = crate::test_support::ENV_LOCK.lock().await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        assert!(
            classify_generation(rupu_auth::in_memory::InMemoryResolver::new())
                .await
                .is_none()
        );
    }
}
