//! `rupu agentiflow run <def>` — launch an agentiflow in the foreground (or,
//! with `--detach`, in a background process of its own) — plus `list` /
//! `status <id>`, which read the runs it leaves under `<global>/agentiflows/`,
//! and `send` / `stop`, the operator's controls over a run that is in flight.
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
    agentiflow_dir, hard_stop, load_agentiflow_def, new_run_id, run_agentiflow, AgentiflowDef,
    AgentiflowRecord, Budget, CoverageTarget, EnvelopeOutcome, GenerationCapability, GoalTarget,
    HardStopOutcome, LeadInputs, OperatorMessage, OperatorQueue, ProviderFactory,
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
    ///
    /// With `--detach` the run goes to a background process of its own and the
    /// command returns as soon as the run has started, printing the run id on
    /// stdout; follow it with `agentiflow status <id>`, steer it with `send`,
    /// end it with `stop`. A run that cannot start (no credential, an invalid
    /// definition) is reported here with the cause, not left to fail unseen.
    Run {
        /// Agentiflow name (matches an `agentiflows/<name>.yaml` file).
        def: String,
        /// Run in the background: start the run in its own process group and
        /// return once it has started. The background process's stderr is kept
        /// in `<run dir>/detach.log`; a run that fails before it starts has
        /// that error printed here and no run directory left behind.
        #[arg(long)]
        detach: bool,
        /// Internal: the run id a detached parent minted for this process, so
        /// the id it printed is the run this process records.
        #[arg(long, hide = true, value_name = "ID", conflicts_with = "detach")]
        run_id: Option<String>,
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
    /// Send a steering message to a running agentiflow's lead.
    ///
    /// The message is queued under the run's `steering/` directory and the
    /// envelope hands it to the lead at the next round boundary. `--now` marks
    /// it as an interrupt (for delivery mid-round); until the envelope acts on
    /// that mark, it is still delivered at the round boundary. A run that
    /// already finished takes no message.
    Send {
        /// Run id (`af_...`): the full id, the compact form `list` prints, or
        /// a unique prefix / suffix of it.
        id: String,
        /// What to tell the lead.
        message: String,
        /// Mark the message as an interrupt (mid-round delivery). Delivered at
        /// the round boundary until the envelope acts on the mark.
        #[arg(long)]
        now: bool,
    },
    /// Stop a running agentiflow.
    ///
    /// By default the stop is graceful: it is queued like a steering message,
    /// and the envelope winds the run down at the next round boundary (the
    /// units it launched are stopped with it). `--now` is the hard stop: it
    /// SIGTERMs the coordinator, SIGTERMs every unit still running (SIGKILL
    /// for one that outlives a short grace), then records the run as failed
    /// (`operator_stop:now`) without waiting for the round.
    Stop {
        /// Run id (`af_...`): the full id, the compact form `list` prints, or
        /// a unique prefix / suffix of it.
        id: String,
        /// Hard stop: signal the coordinator and its units now.
        #[arg(long)]
        now: bool,
    },
}

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::Run {
            def,
            detach,
            run_id,
        } => run_cmd(&def, detach, run_id, global_format).await,
        Action::List => list_cmd(global_format, absolute, all_columns),
        Action::Status { id } => status_cmd(&id, global_format),
        Action::Send { id, message, now } => send_cmd(&id, &message, now),
        Action::Stop { id, now } => stop_cmd(&id, now).await,
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
        Action::Send { .. } => ("agentiflow send", output_report::TABLE_ONLY),
        Action::Stop { .. } => ("agentiflow stop", output_report::TABLE_ONLY),
    };
    formats::ensure_supported(command_name, format, supported)
}

async fn run_cmd(
    def_name: &str,
    detach: bool,
    run_id: Option<String>,
    format: Option<OutputFormat>,
) -> anyhow::Result<()> {
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

    // The id is minted exactly once, here: a detached parent hands it to its
    // child (`--run-id`), which runs the foreground path below under it.
    let run_id = match run_id {
        Some(id) => {
            validate_run_id(&id)?;
            id
        }
        None => new_run_id(),
    };
    if detach {
        return spawn_detached(def_name, &def.name, &run_id, &global, format).await;
    }
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

/// A run id a caller supplied (`--run-id`) names a directory under
/// `<global>/agentiflows/`, so it must look like one `new_run_id` mints: `af_`
/// and a plain token, never a path.
fn validate_run_id(id: &str) -> anyhow::Result<()> {
    let token = id.strip_prefix("af_").unwrap_or_default();
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!("`{id}` is not a valid agentiflow run id (expected `af_` and a token)");
    }
    Ok(())
}

/// What the re-exec'd child of `run --detach` is invoked with: the plain
/// foreground `run`, under the id the parent minted. `--` keeps a definition
/// name from being read as a flag.
fn detached_argv(def_name: &str, run_id: &str) -> Vec<String> {
    ["agentiflow", "run", "--run-id", run_id, "--", def_name]
        .map(String::from)
        .to_vec()
}

/// `--format json` for a detached `agentiflow run`.
#[derive(Serialize)]
struct DetachedReport {
    kind: &'static str,
    version: u8,
    id: String,
    name: String,
    run_dir: String,
}

/// The file a detached run's stderr is kept in, under its run directory.
const DETACH_LOG: &str = "detach.log";

/// How long the parent of `run --detach` waits to learn the child started.
const DETACH_STARTUP_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// How the child of `run --detach` was doing when the parent stopped waiting.
#[derive(Debug)]
enum Startup {
    /// The run's record exists: the run started.
    Started,
    /// The child exited without ever writing a record: it failed to start.
    Exited(std::process::ExitStatus),
    /// Neither, within the wait (a very slow provider start, say).
    Pending,
}

/// Wait for `child` to either write the run's record (it started) or exit
/// without one (it did not). The record is written before the first round, so
/// a healthy start resolves in milliseconds and `--detach` stays quick.
async fn await_startup(
    child: &mut std::process::Child,
    run_dir: &Path,
    wait: std::time::Duration,
) -> std::io::Result<Startup> {
    let record = run_dir.join("agentiflow.json");
    let deadline = std::time::Instant::now() + wait;
    loop {
        if record.exists() {
            return Ok(Startup::Started);
        }
        if let Some(status) = child.try_wait()? {
            // It may have written the record in the instant before it exited.
            return Ok(if record.exists() {
                Startup::Started
            } else {
                Startup::Exited(status)
            });
        }
        if std::time::Instant::now() >= deadline {
            return Ok(Startup::Pending);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// The last few KiB of a detached run's log, for an error message.
fn log_tail(log: &Path) -> String {
    const TAIL: u64 = 4096;
    let tail = std::fs::File::open(log).and_then(|mut f| {
        use std::io::{Read as _, Seek as _, SeekFrom};
        let len = f.metadata()?.len();
        f.seek(SeekFrom::Start(len.saturating_sub(TAIL)))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    });
    match tail {
        Ok(buf) if !buf.iter().all(u8::is_ascii_whitespace) => {
            String::from_utf8_lossy(&buf).trim().to_string()
        }
        Ok(_) => "(it printed nothing)".to_string(),
        Err(e) => format!("(its log {} could not be read: {e})", log.display()),
    }
}

/// The parent half of `run --detach`: start this binary again as the run,
/// detached, and return once the run has started, without waiting for it to
/// finish.
///
/// Detached like the CP's launchers and the fleet's unit launcher
/// (`SubprocessUnitLauncher`): its own process group (`process_group(0)`, so a
/// Ctrl-C at this terminal or this process exiting does not take the run
/// down), stdin/stdout null, and `RUPU_HOME` pinned to the home this process
/// resolved so the child records the run where the id printed here is looked
/// for. The child is the process that runs `run_agentiflow`, so the
/// `runner_pid` it stamps is the detached process: what `stop` and the orphan
/// reaper signal.
///
/// A detached run that cannot start must not look like one that did, so this
/// is a handshake. The child's stderr goes to `<run dir>/detach.log` (the run
/// directory is made first; `run_agentiflow` accepts one that exists and
/// refuses only an existing record), and this process waits for the record to
/// appear (started: print the id) or for the child to exit without one
/// (failed: its log is the error, and the record-less directory is removed).
async fn spawn_detached(
    def_name: &str,
    name: &str,
    run_id: &str,
    global: &Path,
    format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("locate the rupu binary to detach")?;
    let run_dir = agentiflow_dir(global).join(run_id);
    std::fs::create_dir_all(&run_dir).with_context(|| format!("create {}", run_dir.display()))?;
    let log_path = run_dir.join(DETACH_LOG);
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("create {}", log_path.display()))?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.args(detached_argv(def_name, run_id))
        .env("RUPU_HOME", global)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&run_dir);
            return Err(anyhow::Error::new(e).context(format!("detach {}", exe.display())));
        }
    };

    // `child` is dropped unwaited on every path that returns Ok: this process
    // exits next, and the child is then reparented, so nothing is left to reap it.
    match await_startup(&mut child, &run_dir, DETACH_STARTUP_WAIT)
        .await
        .context("wait for the detached run to start")?
    {
        Startup::Started => {}
        Startup::Exited(status) => {
            // Read the log before the directory that holds it goes.
            let tail = log_tail(&log_path);
            let _ = std::fs::remove_dir_all(&run_dir);
            anyhow::bail!("the detached agentiflow exited ({status}) before it started:\n{tail}");
        }
        Startup::Pending => eprintln!(
            "agentiflow {name}: run {run_id} had not started after {}s; it is still going in \
             the background. Its output is in {}",
            DETACH_STARTUP_WAIT.as_secs(),
            log_path.display()
        ),
    }

    match format.unwrap_or(OutputFormat::Table) {
        OutputFormat::Json => formats::print_json(&DetachedReport {
            kind: "agentiflow_run_detached",
            version: 1,
            id: run_id.to_string(),
            name: name.to_string(),
            run_dir: run_dir.display().to_string(),
        })?,
        _ => println!("agentiflow {name}: run {run_id} (detached)"),
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
        // The layered `[pricing]` config (global, then project, with the
        // `[providers.*].kind` account map attached by the loader) over the
        // built-in vendor prices: what `budget.usd` is metered with. A lead
        // model neither prices makes `run_agentiflow` warn that the cap is not
        // enforced.
        pricing: cfg.pricing.clone(),
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
    record.runner_pid = None;
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

/// Budget as far as a run records it: the definition's caps, the state the
/// last finished round saw (`ok` / `soft` / `hard:<dimension>`), and what has
/// been spent so far.
#[derive(Debug, Serialize)]
struct StatusBudget {
    state: Option<String>,
    caps: Option<Budget>,
    /// USD spent, as of the last finished round while the run is going and
    /// final once it has stopped. `None` when the record carries no spend (it
    /// predates metering, or no round has finished) so only the tokens could
    /// be recovered from the ledgers.
    spent_usd: Option<f64>,
    /// Billable (input + output) tokens spent.
    spent_tokens: u64,
}

/// The tokens a run's ledgers hold, for a record that carries no spend: the
/// lead's `usage.jsonl` plus the `usage.jsonl` of every unit the run
/// launched (`units/<id>/` under the run dir names each one; its ledger is
/// `<global>/runs/<id>/usage.jsonl`). Best-effort, like the fold itself: a
/// ledger that is missing or unreadable adds nothing.
fn ledger_tokens(global: &Path, run_dir: &Path) -> u64 {
    let mut unit_ids: Vec<String> = std::fs::read_dir(run_dir.join("units"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    unit_ids.sort();
    let mut ledgers = vec![run_dir.join("usage.jsonl")];
    ledgers.extend(
        unit_ids
            .into_iter()
            .map(|id| global.join("runs").join(id).join("usage.jsonl")),
    );
    rupu_agentiflow::fold_tokens(&ledgers).total.billable()
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
            "no agentiflow run matches '{fragment}' (not found under {}; see `rupu agentiflow list`)",
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

    // Spend: the record's, kept current round by round. One without any (an
    // older build wrote it, or no round has finished yet) is recovered from the
    // ledgers; that gives tokens only, since pricing is not in the record.
    let (spent_usd, spent_tokens) = if record.spent_usd.is_some() || record.spent_tokens > 0 {
        (record.spent_usd, record.spent_tokens)
    } else {
        (None, ledger_tokens(global, &run_dir))
    };

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
            spent_usd,
            spent_tokens,
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
    let _ = writeln!(
        out,
        "  spent: {}, {} tokens",
        r.budget
            .spent_usd
            .map_or_else(|| "usd n/a".to_string(), |usd| format!("${usd:.4}")),
        r.budget.spent_tokens
    );

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

// ---- `send` / `stop` --------------------------------------------------------

/// Resolve `fragment` to a run and read its record. The run directory comes
/// back with it: the steering queue lives there.
fn load_run(
    global: &Path,
    fragment: &str,
) -> anyhow::Result<(String, std::path::PathBuf, AgentiflowRecord)> {
    let id = resolve_run_id(global, fragment)?;
    let run_dir = agentiflow_dir(global).join(&id);
    let record = AgentiflowRecord::read(&run_dir)
        .with_context(|| format!("read {}", run_dir.join("agentiflow.json").display()))?;
    Ok((id, run_dir, record))
}

fn enqueue_steering(run_dir: &Path, msg: &OperatorMessage) -> anyhow::Result<()> {
    OperatorQueue::new(run_dir).enqueue(msg).with_context(|| {
        format!(
            "queue a message under {}",
            run_dir.join("steering").display()
        )
    })
}

/// Queue a steering message for the run's lead. Returns the line to print.
fn send(global: &Path, fragment: &str, message: &str, now: bool) -> anyhow::Result<String> {
    let (id, run_dir, record) = load_run(global, fragment)?;
    if record.status != "running" {
        return Ok(format!("{id} already {}", record.status));
    }
    enqueue_steering(
        &run_dir,
        &OperatorMessage {
            ts: Utc::now().to_rfc3339(),
            body: message.to_string(),
            stop: false,
            interrupt: now,
        },
    )?;
    Ok(format!("queued steering for {id}"))
}

fn send_cmd(fragment: &str, message: &str, now: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    println!("{}", send(&global, fragment, message, now)?);
    Ok(())
}

/// Stop a run, gracefully (a queued stop the envelope honours at the next round
/// boundary) or, with `now`, the hard stop in `rupu_agentiflow::hard_stop`.
/// Returns the line to print. The hard stop blocks (it gives the units a grace
/// before SIGKILL): call this from `spawn_blocking` in async code.
fn stop(global: &Path, fragment: &str, now: bool) -> anyhow::Result<String> {
    if now {
        // `hard_stop` reads the record itself and judges it twice (before and
        // after the grace), so only the id is resolved here.
        let id = resolve_run_id(global, fragment)?;
        let run_dir = agentiflow_dir(global).join(&id);
        let outcome = hard_stop(&run_dir, Utc::now())
            .with_context(|| format!("hard-stop {}", run_dir.join("agentiflow.json").display()))?;
        return Ok(match outcome {
            HardStopOutcome::Stopped => format!("hard-stopped {id}"),
            HardStopOutcome::AlreadyTerminal(status) => format!("{id} already {status}"),
        });
    }

    let (id, run_dir, record) = load_run(global, fragment)?;
    if record.status != "running" {
        return Ok(format!("{id} already {}", record.status));
    }
    enqueue_steering(
        &run_dir,
        &OperatorMessage {
            ts: Utc::now().to_rfc3339(),
            body: "operator stop".into(),
            stop: true,
            interrupt: false,
        },
    )?;
    Ok(format!("requested graceful stop of {id}"))
}

async fn stop_cmd(fragment: &str, now: bool) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let fragment = fragment.to_string();
    // The hard stop sleeps through a SIGTERM grace; keep it off the runtime.
    let line = tokio::task::spawn_blocking(move || stop(&global, &fragment, now))
        .await
        .context("the stop task did not finish")??;
    println!("{line}");
    Ok(())
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
            spent_usd: None,
            spent_tokens: 0,
            runner_pid: None,
        };

        let mut running = record("running");
        running.runner_pid = Some(4321);
        running.write(tmp.path()).unwrap();
        mark_failed(tmp.path(), "boom");
        let after = AgentiflowRecord::read(tmp.path()).unwrap();
        assert_eq!(after.status, "failed");
        assert_eq!(after.runner_pid, None, "a closed-out run owns no pid");
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
            Action::Run {
                def,
                detach,
                run_id,
            } => {
                assert_eq!(def, "acme");
                assert!(!detach, "foreground unless asked");
                assert_eq!(run_id, &None);
            }
            other => panic!("expected `run`, got {other:?}"),
        }
        assert!(ensure_output_format(&action, OutputFormat::Json).is_ok());
        assert!(ensure_output_format(&action, OutputFormat::Csv).is_err());
    }

    #[test]
    fn run_detach_and_the_hidden_run_id_parse_but_never_together() {
        use clap::Parser;
        let detached = parse(["rupu", "agentiflow", "run", "acme", "--detach"]);
        assert!(matches!(
            detached,
            Action::Run {
                detach: true,
                run_id: None,
                ..
            }
        ));

        let child = parse(["rupu", "agentiflow", "run", "--run-id", "af_01X", "acme"]);
        match child {
            Action::Run { run_id, detach, .. } => {
                assert_eq!(run_id.as_deref(), Some("af_01X"));
                assert!(!detach);
            }
            other => panic!("expected `run`, got {other:?}"),
        }

        assert!(crate::Cli::try_parse_from([
            "rupu",
            "agentiflow",
            "run",
            "acme",
            "--detach",
            "--run-id",
            "af_01X"
        ])
        .is_err());
    }

    #[test]
    fn the_run_id_flag_is_hidden_from_help() {
        use clap::CommandFactory;
        let mut cmd = crate::Cli::command();
        let run = cmd
            .find_subcommand_mut("agentiflow")
            .and_then(|a| a.find_subcommand_mut("run"))
            .expect("agentiflow run");
        let help = run.render_long_help().to_string();
        assert!(help.contains("--detach"), "{help}");
        assert!(!help.contains("--run-id"), "{help}");
    }

    #[test]
    fn the_detached_child_runs_the_foreground_path_under_the_parents_id() {
        let argv = detached_argv("acme", "af_01X");
        assert_eq!(
            argv,
            ["agentiflow", "run", "--run-id", "af_01X", "--", "acme"]
        );
        // It parses as the plain foreground `run` carrying that id: no
        // `--detach`, so it does not detach again.
        let mut full = vec!["rupu".to_string()];
        full.extend(argv);
        match parse_vec(full) {
            Action::Run {
                def,
                detach,
                run_id,
            } => {
                assert_eq!(def, "acme");
                assert!(!detach);
                assert_eq!(run_id.as_deref(), Some("af_01X"));
            }
            other => panic!("expected `run`, got {other:?}"),
        }
        // A definition name that looks like a flag stays a name.
        let mut full = vec!["rupu".to_string()];
        full.extend(detached_argv("-x", "af_01X"));
        assert!(matches!(parse_vec(full), Action::Run { def, .. } if def == "-x"));
    }

    // ---- the startup handshake ------------------------------------------------

    fn spawn(program: &str, args: &[&str]) -> std::process::Child {
        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn a_child_that_exits_without_a_record_failed_to_start() {
        let run_dir = tempfile::TempDir::new().unwrap();
        let mut child = spawn("sh", &["-c", "exit 3"]);
        let startup = await_startup(
            &mut child,
            run_dir.path(),
            std::time::Duration::from_secs(20),
        )
        .await
        .unwrap();
        match startup {
            Startup::Exited(status) => assert_eq!(status.code(), Some(3)),
            other => panic!("expected `Exited`, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_child_that_writes_its_record_has_started_even_while_it_keeps_running() {
        let run_dir = tempfile::TempDir::new().unwrap();
        let mut child = spawn("sleep", &["30"]);
        let record = run_dir.path().join("agentiflow.json");
        let writer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            std::fs::write(record, "{}").unwrap();
        });
        let startup = await_startup(
            &mut child,
            run_dir.path(),
            std::time::Duration::from_secs(20),
        )
        .await
        .unwrap();
        writer.await.unwrap();
        assert!(matches!(startup, Startup::Started), "{startup:?}");
        assert!(
            child.try_wait().unwrap().is_none(),
            "the handshake must leave the child running"
        );
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn a_child_that_neither_writes_nor_exits_is_pending_not_failed() {
        let run_dir = tempfile::TempDir::new().unwrap();
        let mut child = spawn("sleep", &["30"]);
        let startup = await_startup(
            &mut child,
            run_dir.path(),
            std::time::Duration::from_millis(200),
        )
        .await
        .unwrap();
        assert!(matches!(startup, Startup::Pending), "{startup:?}");
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn the_log_tail_is_what_the_child_last_said() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log = tmp.path().join("detach.log");
        assert!(log_tail(&log).contains("could not be read"));
        std::fs::write(&log, "  \n").unwrap();
        assert_eq!(log_tail(&log), "(it printed nothing)");
        std::fs::write(&log, "error: no credential\n").unwrap();
        assert_eq!(log_tail(&log), "error: no credential");
        // A long log keeps its end, where the error is.
        std::fs::write(&log, format!("{}\nthe real error\n", "x".repeat(10_000))).unwrap();
        let tail = log_tail(&log);
        assert!(tail.ends_with("the real error"), "{tail}");
        assert!(tail.len() <= 4096, "{}", tail.len());
    }

    #[test]
    fn a_supplied_run_id_must_look_like_one_the_runner_mints() {
        assert!(validate_run_id(&new_run_id()).is_ok());
        assert!(validate_run_id("af_01HZZ_x9").is_ok());
        for bad in ["", "af_", "01HZZ", "af_../x", "af_a/b", "af_a b", "../af_x"] {
            assert!(validate_run_id(bad).is_err(), "{bad:?} accepted");
        }
    }

    fn parse<const N: usize>(argv: [&str; N]) -> Action {
        parse_vec(argv.iter().map(|s| s.to_string()).collect())
    }

    fn parse_vec(argv: Vec<String>) -> Action {
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

    #[test]
    fn send_and_stop_parse_and_take_only_the_table_format() {
        let send = parse(["rupu", "agentiflow", "send", "af_01ABC", "look at auth"]);
        match &send {
            Action::Send { id, message, now } => {
                assert_eq!(id, "af_01ABC");
                assert_eq!(message, "look at auth");
                assert!(!now);
            }
            other => panic!("expected `send`, got {other:?}"),
        }
        assert!(ensure_output_format(&send, OutputFormat::Table).is_ok());
        assert!(ensure_output_format(&send, OutputFormat::Json).is_err());

        let send_now = parse(["rupu", "agentiflow", "send", "01ABC", "hi", "--now"]);
        assert!(matches!(send_now, Action::Send { now: true, .. }));

        let stop = parse(["rupu", "agentiflow", "stop", "af_01ABC"]);
        assert!(matches!(&stop, Action::Stop { now: false, .. }));
        assert!(ensure_output_format(&stop, OutputFormat::Json).is_err());
        let stop_now = parse(["rupu", "agentiflow", "stop", "af_01ABC", "--now"]);
        assert!(matches!(stop_now, Action::Stop { now: true, .. }));
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
            spent_usd: None,
            spent_tokens: 0,
            runner_pid: None,
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

    // ---- status: spend ----------------------------------------------------

    /// Rewrite a seeded run's record with `edit` applied.
    fn edit_record(global: &Path, id: &str, edit: impl FnOnce(&mut AgentiflowRecord)) {
        let dir = agentiflow_dir(global).join(id);
        let mut record = AgentiflowRecord::read(&dir).unwrap();
        edit(&mut record);
        record.write(&dir).unwrap();
    }

    fn ledger_line(id: &str, input: u64, output: u64) -> String {
        let row = rupu_orchestrator::usage_ledger::LedgerRow {
            v: rupu_orchestrator::usage_ledger::LEDGER_VERSION,
            id: id.into(),
            at: chrono::Utc::now(),
            kind: rupu_orchestrator::usage_ledger::LedgerKind::Turn,
            step_id: None,
            unit_index: None,
            unit_key: None,
            agent_run_id: "ar_1".into(),
            parent_agent_run_id: None,
            transcript: std::path::PathBuf::from("/t/x.jsonl"),
            agent: "recon".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-5-5".into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
            cache_write_tokens: 0,
        };
        format!("{}\n", serde_json::to_string(&row).unwrap())
    }

    #[test]
    fn status_shows_the_spend_the_record_carries() {
        let tmp = seeded_global();
        let g = tmp.path();
        // A run in flight: the spend as of its last finished round.
        edit_record(g, "af_01NEWER", |r| {
            r.spent_usd = Some(1.5);
            r.spent_tokens = 123_456;
        });
        // A ledger that disagrees must not override a record that has spend.
        write(
            &agentiflow_dir(g).join("af_01NEWER/usage.jsonl"),
            &ledger_line("01A", 1, 1),
        );

        let status = load_status(g, "af_01NEWER").unwrap();
        assert_eq!(status.budget.spent_usd, Some(1.5));
        assert_eq!(status.budget.spent_tokens, 123_456);
        let human = render_status(&status);
        assert!(human.contains("spent: $1.5000, 123456 tokens"), "{human}");

        // The JSON report carries both under `budget`.
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["budget"]["spent_usd"], 1.5, "{json}");
        assert_eq!(json["budget"]["spent_tokens"], 123_456, "{json}");
    }

    #[test]
    fn a_metered_zero_is_a_record_with_spend_not_one_without() {
        let tmp = seeded_global();
        let g = tmp.path();
        edit_record(g, "af_01OLDER", |r| {
            r.spent_usd = Some(0.0);
            r.spent_tokens = 0;
        });
        write(
            &agentiflow_dir(g).join("af_01OLDER/usage.jsonl"),
            &ledger_line("01A", 500, 50),
        );

        let status = load_status(g, "af_01OLDER").unwrap();
        assert_eq!(status.budget.spent_usd, Some(0.0));
        assert_eq!(status.budget.spent_tokens, 0, "the record is authoritative");
        assert!(render_status(&status).contains("spent: $0.0000, 0 tokens"));
    }

    #[test]
    fn status_recovers_tokens_from_the_ledgers_when_the_record_has_no_spend() {
        let tmp = seeded_global();
        let g = tmp.path();
        let run_dir = agentiflow_dir(g).join("af_01NEWER");
        // The lead's ledger...
        write(&run_dir.join("usage.jsonl"), &ledger_line("01A", 100, 10));
        // ...and one launched unit's, found through `units/<id>/`. A second
        // unit that never wrote a ledger adds nothing.
        write(&run_dir.join("units/run_u1/unit.json"), "{}");
        write(&run_dir.join("units/run_u2/unit.json"), "{}");
        write(
            &g.join("runs/run_u1/usage.jsonl"),
            &format!(
                "{}{}",
                ledger_line("01B", 200, 20),
                // The same row twice (a mirror replay) counts once.
                ledger_line("01B", 200, 20)
            ),
        );

        let status = load_status(g, "af_01NEWER").unwrap();
        assert_eq!(
            status.budget.spent_usd, None,
            "pricing is not in the ledger"
        );
        assert_eq!(status.budget.spent_tokens, 100 + 10 + 200 + 20);
        let human = render_status(&status);
        assert!(human.contains("spent: usd n/a, 330 tokens"), "{human}");
    }

    #[test]
    fn status_with_no_spend_anywhere_reports_zero_tokens() {
        let tmp = seeded_global();
        let status = load_status(tmp.path(), "af_01NEWER").unwrap();
        assert_eq!(status.budget.spent_usd, None);
        assert_eq!(status.budget.spent_tokens, 0);
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

    // ---- send / stop ---------------------------------------------------------

    fn drained(global: &Path, id: &str) -> Vec<OperatorMessage> {
        OperatorQueue::new(agentiflow_dir(global).join(id))
            .drain()
            .unwrap()
    }

    #[test]
    fn send_queues_a_steering_message_for_a_running_run() {
        let tmp = seeded_global();
        let g = tmp.path();
        // The compact fragment `list` prints resolves, as it does for `status`.
        let line = send(g, "01NEWER", "focus on auth", false).unwrap();
        assert_eq!(line, "queued steering for af_01NEWER");
        let line = send(g, "af_01NEWER", "drop that, now", true).unwrap();
        assert_eq!(line, "queued steering for af_01NEWER");
        let msgs = drained(g, "af_01NEWER");
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].body, "focus on auth");
        assert!(!msgs[0].stop && !msgs[0].interrupt);
        assert_eq!(msgs[1].body, "drop that, now");
        assert!(
            !msgs[1].stop && msgs[1].interrupt,
            "--now marks an interrupt"
        );
        assert!(chrono::DateTime::parse_from_rfc3339(&msgs[0].ts).is_ok());
    }

    #[test]
    fn send_and_stop_to_a_finished_run_say_so_and_queue_nothing() {
        let tmp = seeded_global();
        let g = tmp.path();
        assert_eq!(
            send(g, "af_01OLDER", "hello", false).unwrap(),
            "af_01OLDER already completed"
        );
        for now in [false, true] {
            assert_eq!(
                stop(g, "af_01OLDER", now).unwrap(),
                "af_01OLDER already completed"
            );
        }
        assert!(drained(g, "af_01OLDER").is_empty());
        // The finished record is untouched, `--now` included.
        let rec = AgentiflowRecord::read(&agentiflow_dir(g).join("af_01OLDER")).unwrap();
        assert_eq!(rec.status, "completed");
        assert_eq!(rec.stop_reason.as_deref(), Some("goals_met"));
    }

    #[test]
    fn send_and_stop_refuse_an_unknown_or_ambiguous_run() {
        let tmp = seeded_global();
        let g = tmp.path();
        let msg = send(g, "af_01MISSING", "x", false).unwrap_err().to_string();
        assert!(
            msg.contains("no agentiflow run matches 'af_01MISSING'"),
            "{msg}"
        );
        let msg = stop(g, "af_01MISSING", true).unwrap_err().to_string();
        assert!(
            msg.contains("no agentiflow run matches 'af_01MISSING'"),
            "{msg}"
        );
        let msg = stop(g, "af_01", false).unwrap_err().to_string();
        assert!(msg.contains("more than one"), "{msg}");
        assert!(drained(g, "af_01NEWER").is_empty());
    }

    #[test]
    fn a_graceful_stop_queues_a_stop_message_and_leaves_the_record_running() {
        let tmp = seeded_global();
        let g = tmp.path();
        assert_eq!(
            stop(g, "01NEWER", false).unwrap(),
            "requested graceful stop of af_01NEWER"
        );
        let msgs = drained(g, "af_01NEWER");
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].stop && !msgs[0].interrupt);
        assert_eq!(msgs[0].body, "operator stop");
        // The envelope, not the CLI, ends a graceful stop.
        let rec = AgentiflowRecord::read(&agentiflow_dir(g).join("af_01NEWER")).unwrap();
        assert_eq!(rec.status, "running");
    }

    /// A `sleep` leading its own process group (as a launched unit does, so its
    /// pid is its pgid), plus a thread that reaps it the moment it dies:
    /// without the waiter a killed child stays a zombie of this process, and
    /// `kill(pid, 0)` reads a zombie as alive.
    #[cfg(unix)]
    fn spawn_sleeper() -> (u32, std::thread::JoinHandle<std::process::ExitStatus>) {
        use std::os::unix::process::CommandExt as _;
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        (
            child.id(),
            std::thread::spawn(move || child.wait().unwrap()),
        )
    }

    #[cfg(unix)]
    fn write_unit(run_dir: &Path, unit: &str, pgid: u32, state: &str) {
        let dir = run_dir.join("units").join(unit);
        std::fs::create_dir_all(&dir).unwrap();
        let status = if state == "done" {
            serde_json::json!({"state": "done", "success": true, "output": "ok"})
        } else {
            serde_json::json!({"state": state})
        };
        std::fs::write(
            dir.join("unit.json"),
            serde_json::json!({"kind": "agent", "pgid": pgid, "status": status}).to_string(),
        )
        .unwrap();
    }

    fn event_lines(run_dir: &Path) -> Vec<serde_json::Value> {
        match std::fs::read_to_string(run_dir.join("events.jsonl")) {
            Ok(raw) => raw
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_stop_signals_the_coordinator_and_the_live_units_and_closes_the_record() {
        use std::os::unix::process::ExitStatusExt as _;
        let tmp = seeded_global();
        let g = tmp.path();
        let run_dir = agentiflow_dir(g).join("af_01NEWER");

        let (coordinator, coordinator_exit) = spawn_sleeper();
        let (live_unit, live_unit_exit) = spawn_sleeper();
        let (finished_unit, finished_unit_exit) = spawn_sleeper();
        write_unit(&run_dir, "unit_live", live_unit, "running");
        // A unit that already finished keeps its process group: not signalled.
        write_unit(&run_dir, "unit_done", finished_unit, "done");
        edit_record(g, "af_01NEWER", |r| r.runner_pid = Some(coordinator));

        assert_eq!(
            stop(g, "af_01NEWER", true).unwrap(),
            "hard-stopped af_01NEWER"
        );

        assert_eq!(
            coordinator_exit.join().unwrap().signal(),
            Some(15),
            "coordinator got SIGTERM"
        );
        assert_eq!(
            live_unit_exit.join().unwrap().signal(),
            Some(15),
            "live unit's group got SIGTERM"
        );
        assert!(
            rupu_agentiflow::pid_is_running(finished_unit),
            "a finished unit is left alone"
        );
        rupu_agentiflow::kill_group(finished_unit);
        finished_unit_exit.join().unwrap();

        let rec = AgentiflowRecord::read(&run_dir).unwrap();
        assert_eq!(rec.status, "failed");
        assert_eq!(rec.stop_reason.as_deref(), Some("operator_stop:now"));
        assert_eq!(rec.runner_pid, None);
        assert!(rec.ended_at.is_some());
        // A hard stop is a signal, not a message.
        assert!(drained(g, "af_01NEWER").is_empty());
        // The coordinator dies on SIGTERM without a word, so the stop writes
        // the terminal event, or the log would end mid-run on a `failed` run.
        let last = event_lines(&run_dir).pop().expect("a terminal event");
        assert_eq!(last["kind"], "run_stopped");
        assert_eq!(last["stop_reason"], "operator_stop:now");
    }

    #[test]
    fn a_hard_stop_with_no_live_coordinator_still_closes_the_record_and_the_log() {
        let tmp = seeded_global();
        let g = tmp.path();
        let run_dir = agentiflow_dir(g).join("af_01NEWER");
        // `runner_pid` is `None` (an older record): nothing to signal.
        assert_eq!(
            stop(g, "af_01NEWER", true).unwrap(),
            "hard-stopped af_01NEWER"
        );
        let rec = AgentiflowRecord::read(&run_dir).unwrap();
        assert_eq!(rec.status, "failed");
        assert_eq!(rec.stop_reason.as_deref(), Some("operator_stop:now"));
        let events = event_lines(&run_dir);
        let last = events.last().expect("a terminal event");
        assert_eq!(last["kind"], "run_stopped");
        assert_eq!(last["stop_reason"], "operator_stop:now");
        assert_eq!(last["detail"], "operator hard stop (--now)");
        assert_eq!(events.len(), 1);
        // The record is final now: a second hard stop leaves it, and the log, be.
        assert_eq!(
            stop(g, "af_01NEWER", true).unwrap(),
            "af_01NEWER already failed"
        );
        assert_eq!(event_lines(&run_dir).len(), 1);
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
