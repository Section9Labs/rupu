//! `rupu workflow list | show | run`.
//!
//! Lists workflows from `<global>/workflows/*.yaml` and (if any)
//! `<project>/.rupu/workflows/*.yaml`; project entries shadow global by
//! filename. `show` renders a retained definition snapshot for human
//! output (or structured JSON for automation). `run` parses the workflow,
//! builds a [`StepFactory`] that wires real providers via
//! [`rupu_runtime::provider_factory::build_for_provider`], and dispatches
//! [`rupu_orchestrator::run_workflow`].
//!
//! The factory carries a clone of the parsed [`Workflow`] so each
//! step's `agent:` field is honored (no hardcoded agent name).

use crate::cmd::completers::workflow_names;
use crate::cmd::ui::LiveViewMode;
use crate::fleet_unit_dispatcher::build_dispatcher_if_needed;
use crate::output::formats::OutputFormat;
use crate::output::palette::{self, Status as UiStatus, BRAND, DIM};
use crate::output::printer::{visible_len, wrap_with_ansi};
use crate::output::report::{self, CollectionOutput, DetailOutput, EventOutput};
use crate::paths;
use anyhow::Context;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_agent::load_agents as load_agent_specs;
use rupu_app_canvas::{render_rows as render_graph_rows, GraphCell, NodeStatus};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts};
use rupu_orchestrator::runs::{CancelError, CancelOutcome, PauseError};
use rupu_orchestrator::{DefaultStepFactory, RunWorkflowError};
use rupu_runtime::{
    ArtifactKind, ArtifactManifest, ArtifactRef, AutoflowEnvelope, ExecutionBackend,
    ExecutionRequest, PreparedRun, RepoBinding, RunContext, RunCorrelation, RunEnvelope, RunKind,
    RunResult, RunResultStatus, RunTrigger, RunTriggerSource, WorkerRequest, WorkflowBinding,
};
use rupu_workspace::{WorkerKind, WorkerRecord, WorkerStore};
use sha2::{Digest, Sha256};

/// Convert a typed `RunWorkflowError` to `anyhow::Error`. Input
/// validation variants get a Cargo-style YAML snippet pointing at
/// the offending declaration; everything else falls through to the
/// typed error's `Display`. Path + body must point at the workflow
/// the runner just rejected — that's what the snippet renders.
fn to_anyhow_with_input_snippet(
    e: RunWorkflowError,
    path: &std::path::Path,
    body: &str,
) -> anyhow::Error {
    let formatted = crate::output::yaml_snippet::render_input_error(&e, path, body);
    if formatted == e.to_string() {
        // Non-input variant — fall through to the standard conversion
        // so anyhow's source-chain integration remains intact.
        anyhow::Error::from(e)
    } else {
        anyhow::anyhow!("{formatted}")
    }
}
use rupu_orchestrator::runner::{OrchestratorRunResult, RunWorkflowError as RunWfErr};
use rupu_orchestrator::Workflow;
use serde::Serialize;

/// Is the live three-zone view (dashboard + git-graph spine + focus
/// feed) enabled? It is the DEFAULT for interactive runs: on whenever
/// stdout is a tty, UNLESS the caller opts out via `--plain` or the
/// `RUPU_LIVE_VIEW=0` / `false` escape hatch. Non-tty always falls back
/// to the line printer.
fn live_view_enabled(stdout_is_tty: bool, plain: bool) -> bool {
    if !stdout_is_tty || plain {
        return false;
    }
    // `RUPU_LIVE_VIEW` is now an OFF switch: any value other than
    // `0` / `false` leaves the (default-on) live view enabled.
    let env_off = std::env::var("RUPU_LIVE_VIEW")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false);
    !env_off
}

/// Poll interval for the cooperative pause marker
/// ([`rupu_orchestrator::RunStore::pause_marker_path`]).
const PAUSE_MARKER_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Spawn a lightweight task that watches a run's pause marker and trips
/// `token` when it appears.
///
/// This is the delivery mechanism that lets a *detached* `rupu workflow run
/// <id>` (or `resume`) subprocess honor a pause requested by another process
/// — `cp serve`'s `pause_run` writes the marker
/// ([`RunStore::set_pause_marker`](rupu_orchestrator::RunStore::set_pause_marker)),
/// and this poller flips the subprocess's
/// [`OrchestratorRunOpts::pause`](rupu_orchestrator::runner::OrchestratorRunOpts::pause)
/// token so the T2/T3 cooperative-pause machinery stops at the next safe
/// boundary. It checks once immediately (covering a pause requested before
/// the run started polling) and then every
/// [`PAUSE_MARKER_POLL_INTERVAL`] until the marker is seen; the caller
/// aborts the returned handle once the run finishes so the poller never
/// outlives its run.
fn spawn_pause_marker_poller(
    store: Arc<rupu_orchestrator::RunStore>,
    run_id: String,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if token.is_cancelled() {
                // Already paused/cancelled by another path; nothing to do.
                return;
            }
            if store.pause_marker_exists(&run_id) {
                token.cancel();
                return;
            }
            tokio::time::sleep(PAUSE_MARKER_POLL_INTERVAL).await;
        }
    })
}

/// Duplicate-execution guard for the resume path: returns the still-live
/// `runner_pid` (a reason to REFUSE resume) when a run's original process is
/// still running.
///
/// A cooperatively-`Paused` run whose process is still alive has NOT honored
/// the pause yet — the process clears `runner_pid` only once it stops at a
/// safe boundary. Re-launching now would spawn a SECOND process racing the
/// first over the same run dir (duplicate side effects). `own_pid` is excluded
/// so a run resumed in-process (whose `runner_pid` is us) is never mistaken
/// for a foreign live process. Returns `None` (not blocked) for a cleared
/// pid, our own pid, or a dead pid.
fn resume_blocked_by_live_runner(runner_pid: Option<u32>, own_pid: u32) -> Option<u32> {
    match runner_pid {
        Some(pid) if pid != own_pid && rupu_orchestrator::runs::pid_is_running(pid) => Some(pid),
        _ => None,
    }
}

/// Drive `run_workflow(opts)` while painting the live three-zone view in
/// a sibling task, returning the workflow result. The view tails
/// `events.jsonl` + the active (unit or step) transcript and stops when
/// the run reaches a terminal state. Shared by `run` and `resume_run`.
async fn run_workflow_with_live_view(
    opts: OrchestratorRunOpts,
    view_workflow: Workflow,
    runs_dir: PathBuf,
    run_id: String,
    pricing: rupu_config::PricingConfig,
) -> Result<OrchestratorRunResult, RunWfErr> {
    let runner_task = tokio::spawn(run_workflow(opts));
    let view_task = tokio::spawn(async move {
        let _ =
            crate::output::live_run::run_live_view(view_workflow, runs_dir, run_id, pricing).await;
    });
    let result = match runner_task.await {
        Ok(r) => r,
        Err(e) => {
            // A panicked runner task is a hard failure; surface it as an
            // io error (the closest typed variant) so the caller's
            // existing error mapping applies.
            view_task.abort();
            return Err(RunWfErr::Io(std::io::Error::other(format!(
                "workflow task panicked: {e}"
            ))));
        }
    };
    // Give the view a brief moment to paint the final frame, then stop it.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(300), view_task).await;
    result
}
use std::collections::BTreeMap;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tracing::warn;
use ulid::Ulid;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all workflows (global + project).
    List,
    /// Inspect a workflow definition.
    Show {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Human snapshot density (`focused` | `compact` | `full`).
        #[arg(long, value_enum, default_value_t = LiveViewMode::Full)]
        view: LiveViewMode,
        /// Disable colored output (also honored: `NO_COLOR` env var).
        #[arg(long)]
        no_color: bool,
        /// syntect theme name. Default: `base16-ocean.dark`.
        #[arg(long)]
        theme: Option<String>,
        /// Force pager. Default: page when stdout is a tty.
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        /// Disable pager.
        #[arg(long)]
        no_pager: bool,
    },
    /// Open a workflow file in `$VISUAL` / `$EDITOR`. Validates the
    /// YAML on save (warn-only).
    Edit {
        /// Workflow name (filename stem under `workflows/`).
        name: String,
        /// Force the project shadow (`.rupu/workflows/<name>.yaml`) or
        /// the global file (`<global>/workflows/<name>.yaml`). Default:
        /// prefer project if it exists, else global.
        #[arg(long, value_parser = ["global", "project"])]
        scope: Option<String>,
        /// Override the editor (e.g. `--editor "code --wait"`).
        /// Default: `$VISUAL` then `$EDITOR` then `vi`.
        #[arg(long)]
        editor: Option<String>,
    },
    /// Scaffold a new workflow YAML from a template, then open it for
    /// editing. Prompts interactively for scope and name when omitted.
    /// With `--describe`, a model drafts the definition before you review it.
    Create {
        /// Workflow name (no `.yaml` extension).
        name: Option<String>,
        /// Target scope (`global` or `project`). Prompts when omitted.
        #[arg(long, value_parser = ["global", "project"])]
        scope: Option<String>,
        /// Override the editor (e.g. `--editor "code --wait"`).
        #[arg(long)]
        editor: Option<String>,
        /// Natural-language description — a model drafts the workflow.
        #[arg(long)]
        describe: Option<String>,
        /// Provider for generation (default: first authenticated).
        #[arg(long)]
        gen_provider: Option<String>,
        /// Model for generation (default: provider's default).
        #[arg(long)]
        gen_model: Option<String>,
        /// Host to create on (only `local` available today).
        #[arg(long, default_value = "local")]
        host: String,
    },
    /// Run a workflow.
    Run {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Optional run-target: a repo, PR, or issue reference.
        ///
        /// Accepts repo (`github:owner/repo`, `gitlab:group/proj`), PR
        /// (`github:owner/repo#42`), or issue
        /// (`github:owner/repo/issues/42`). Repo / PR targets clone to
        /// a tmpdir for the run; issue targets pre-fetch the issue
        /// payload and bind it as `{{ issue.* }}` in step prompts.
        target: Option<String>,
        /// `KEY=VALUE` template inputs (repeatable).
        #[arg(long, value_parser = parse_kv)]
        input: Vec<(String, String)>,
        /// Override permission mode (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
        /// Control live output density (`focused` | `full`).
        #[arg(long, value_enum)]
        view: Option<LiveViewMode>,
        /// Use the plain line printer instead of the live graph view.
        #[arg(long)]
        plain: bool,
        /// Pre-assign the run id (e.g. so a caller can reference the run before it starts).
        #[arg(long)]
        run_id: Option<String>,
    },
    /// List recent workflow runs from the persistent run-store
    /// (`<global>/runs/`). Newest first.
    Runs {
        /// Show only the N most recent runs.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Filter by status: `pending` / `running` / `completed` /
        /// `failed` / `awaiting_approval` / `rejected`.
        #[arg(long)]
        status: Option<String>,
        /// Filter by issue ref (full or shorthand). Matches the
        /// textual `RunRecord.issue_ref` persisted at run start.
        /// Accepts `<platform>:<owner>/<repo>/issues/<N>` (full),
        /// `<owner>/<repo>#<N>` (GitHub shorthand), or bare `<N>`
        /// (autodetects from cwd's git remote).
        #[arg(long)]
        issue: Option<String>,
        /// Disable colored output (also honored: `NO_COLOR` env,
        /// `[ui].color = "never"` in config).
        #[arg(long)]
        no_color: bool,
    },
    /// Inspect one persisted run: status, inputs, per-step
    /// transcript pointers.
    ShowRun {
        /// Full run id (`run_<ULID>`) as printed by
        /// `rupu workflow run`.
        run_id: String,
        /// Live view density for the retained snapshot.
        #[arg(long)]
        view: Option<LiveViewMode>,
        /// Disable colored output (also honored: `NO_COLOR` env,
        /// `[ui].color = \"never\"` in config).
        #[arg(long)]
        no_color: bool,
        /// Force pager. Default: page when stdout is a tty.
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        /// Disable pager.
        #[arg(long, conflicts_with = "pager")]
        no_pager: bool,
    },
    /// Approve a paused run and resume execution from the awaited
    /// step. The run must be in `awaiting_approval` status.
    Approve {
        run_id: String,
        /// Override permission mode for the resumed run
        /// (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
        /// Target a specific parked approval gate by step id (Task 5b-2a,
        /// spec §7) — required once a run has more than one gate parked
        /// at once (a concurrent DAG run with several gates in the same
        /// batch-park wave). Omit for the legacy/single-gate case: the
        /// sole parked gate is approved exactly as before. Approving one
        /// gate of several leaves the others parked; the run stays
        /// `awaiting_approval` until every gate is resolved.
        #[arg(long)]
        gate: Option<String>,
        /// Override the recorded approver identity (ISSUES.md I-82).
        /// Internal: this is how `cp serve`'s resume worker carries a
        /// web-initiated approve's true actor (e.g. `"web"`) across the
        /// process boundary — it reads the run's `resume_approver` marker
        /// and re-derives this flag when spawning `rupu workflow approve`.
        /// A direct operator invocation should omit it; the approver then
        /// defaults to the OS user running this process, exactly as
        /// before this flag existed.
        #[arg(long, hide = true)]
        approver: Option<String>,
    },
    /// Reject a paused run. Marks it `rejected`; no further steps
    /// dispatch.
    Reject {
        run_id: String,
        /// Optional human-readable reason recorded in the run's
        /// `error_message`.
        #[arg(long)]
        reason: Option<String>,
        /// Target a specific parked approval gate by step id (Task 5b-2a,
        /// spec §7) — same targeting rule as `approve --gate`. Rejecting
        /// one gate of several runs THAT gate's `on_reject` cleanup chain
        /// and leaves the others parked; the run flips to `rejected` only
        /// once every gate is resolved.
        #[arg(long)]
        gate: Option<String>,
    },
    /// Cancel a running workflow run.
    Cancel {
        /// Full run id (`run_<ULID>`) as printed by
        /// `rupu workflow run`.
        run_id: String,
    },
    /// Cooperatively pause a running workflow run at its next safe boundary,
    /// leaving it non-terminal and resumable via `rupu workflow resume`.
    // This is the primitive a remote transport (SSH) reaches over `ssh` the
    // same way it reaches `cancel`/`approve`/`reject` — see
    // `docs/superpowers/plans/2026-07-01-rupu-pause-resume-plan.md` Task 5.
    Pause {
        /// Full run id (`run_<ULID>`) as printed by
        /// `rupu workflow run`.
        run_id: String,
    },
    /// Resume a failed/cancelled run, re-running only the agent runs
    /// that didn't succeed.
    Resume {
        /// Full run id (`run_<ULID>`) of the terminal run to resume.
        run_id: String,
        /// Override permission mode for the resumed run
        /// (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
        /// Use the plain line printer instead of the live graph view.
        #[arg(long)]
        plain: bool,
    },
    /// Archive a terminal run (move it out of the active list; reversible).
    ArchiveRun {
        /// Full run id (`run_<ULID>`).
        run_id: String,
    },
    /// Restore an archived run back to the active list.
    RestoreRun {
        /// Full run id (`run_<ULID>`).
        run_id: String,
    },
    /// Permanently delete a run and its transcripts. Requires `--force`.
    DeleteRun {
        /// Full run id (`run_<ULID>`).
        run_id: String,
        #[arg(long)]
        force: bool,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE: {s}"))?;
    Ok((k.to_string(), v.to_string()))
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List => list(global_format).await,
        Action::Show {
            name,
            view,
            no_color,
            theme,
            pager,
            no_pager,
        } => {
            let pager_flag = if pager {
                Some(true)
            } else if no_pager {
                Some(false)
            } else {
                None
            };
            show(
                &name,
                Some(view),
                no_color,
                theme.as_deref(),
                pager_flag,
                global_format,
            )
            .await
        }
        Action::Edit {
            name,
            scope,
            editor,
        } => edit(&name, scope.as_deref(), editor.as_deref()).await,
        Action::Create {
            name,
            scope,
            editor,
            describe,
            gen_provider,
            gen_model,
            host,
        } => {
            create(
                name,
                scope,
                editor.as_deref(),
                describe,
                gen_provider,
                gen_model,
                &host,
            )
            .await
        }
        Action::Run {
            name,
            target,
            input,
            mode,
            view,
            plain,
            run_id,
        } => {
            run(
                &name,
                target.as_deref(),
                input,
                mode.as_deref(),
                None,
                view,
                plain,
                run_id,
            )
            .await
        }
        Action::Runs {
            limit,
            status,
            issue,
            no_color,
        } => {
            runs(
                limit,
                status.as_deref(),
                issue.as_deref(),
                no_color,
                global_format,
            )
            .await
        }
        Action::ShowRun {
            run_id,
            view,
            no_color,
            pager,
            no_pager,
        } => {
            let pager_flag = if pager {
                Some(true)
            } else if no_pager {
                Some(false)
            } else {
                None
            };
            show_run(&run_id, view, no_color, pager_flag, global_format).await
        }
        Action::Approve {
            run_id,
            mode,
            gate,
            approver,
        } => {
            approve(
                &run_id,
                mode.as_deref(),
                gate.as_deref(),
                approver.as_deref(),
            )
            .await
        }
        Action::Reject {
            run_id,
            reason,
            gate,
        } => reject(&run_id, reason.as_deref(), gate.as_deref()).await,
        Action::Cancel { run_id } => cancel(&run_id).await,
        Action::Pause { run_id } => pause(&run_id).await,
        Action::Resume {
            run_id,
            mode,
            plain,
        } => resume_run(&run_id, mode.as_deref(), plain).await,
        Action::ArchiveRun { run_id } => archive_run(&run_id).await,
        Action::RestoreRun { run_id } => restore_run(&run_id).await,
        Action::DeleteRun { run_id, force } => delete_run(&run_id, force).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List => ("workflow list", report::TABLE_JSON_CSV),
        Action::Show { .. } => ("workflow show", report::TABLE_JSON),
        Action::Runs { .. } => ("workflow runs", report::TABLE_JSON_CSV),
        Action::ShowRun { .. } => ("workflow show-run", report::PRETTY_TABLE_JSON),
        Action::Edit { .. } => ("workflow edit", report::TABLE_ONLY),
        Action::Create { .. } => ("workflow create", report::TABLE_ONLY),
        Action::Run { .. } => ("workflow run", report::TABLE_ONLY),
        Action::Approve { .. } => ("workflow approve", report::TABLE_ONLY),
        Action::Reject { .. } => ("workflow reject", report::TABLE_ONLY),
        Action::Cancel { .. } => ("workflow cancel", report::TABLE_ONLY),
        Action::Pause { .. } => ("workflow pause", report::TABLE_ONLY),
        Action::Resume { .. } => ("workflow resume", report::TABLE_ONLY),
        Action::ArchiveRun { .. } => ("workflow archive-run", report::TABLE_ONLY),
        Action::RestoreRun { .. } => ("workflow restore-run", report::TABLE_ONLY),
        Action::DeleteRun { .. } => ("workflow delete-run", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

#[derive(Serialize)]
struct WorkflowListRow {
    name: String,
    scope: String,
}

#[derive(Serialize)]
struct WorkflowRunsRow {
    run_id: String,
    status: String,
    started_at: String,
    duration_seconds: Option<i64>,
    expires_in_seconds: Option<i64>,
    total_tokens: u64,
    cost_usd: Option<f64>,
    workflow: String,
}

#[derive(Serialize)]
struct WorkflowListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<WorkflowListRow>,
}

#[derive(Serialize)]
struct WorkflowRunsSummary {
    count: usize,
    limit: usize,
    status_filter: Option<String>,
    issue_filter: Option<String>,
}

#[derive(Serialize)]
struct WorkflowRunsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<WorkflowRunsRow>,
    summary: WorkflowRunsSummary,
}

struct WorkflowListOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: WorkflowListReport,
}

impl CollectionOutput for WorkflowListOutput {
    type JsonReport = WorkflowListReport;
    type CsvRow = WorkflowListRow;

    fn command_name(&self) -> &'static str {
        "workflow list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["name", "scope"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["NAME", "SCOPE"]);
        for row in &self.report.rows {
            table.add_row(vec![
                comfy_table::Cell::new(&row.name),
                crate::output::tables::status_cell(&row.scope, &self.prefs),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

struct WorkflowRunsOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: WorkflowRunsReport,
}

#[derive(Serialize)]
struct WorkflowShowItem {
    name: String,
    path: String,
    body: String,
}

#[derive(Serialize)]
struct WorkflowShowReport {
    kind: &'static str,
    version: u8,
    item: WorkflowShowItem,
}

struct WorkflowShowOutput {
    prefs: crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
    report: WorkflowShowReport,
}

#[derive(Serialize)]
struct WorkflowShowRunStepItem {
    label: String,
    status: String,
    transcript_path: String,
}

#[derive(Serialize)]
struct WorkflowShowRunStep {
    step_id: String,
    status: String,
    transcript_path: String,
    items: Vec<WorkflowShowRunStepItem>,
}

#[derive(Serialize)]
struct WorkflowShowRunUsageRow {
    provider: String,
    model: String,
    agent: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cost_usd: Option<f64>,
}

#[derive(Serialize)]
struct WorkflowShowRunUsageTotals {
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cost_usd: Option<f64>,
}

#[derive(Serialize)]
struct WorkflowShowRunItem {
    run_id: String,
    workflow: String,
    status: String,
    workspace_id: String,
    workspace_path: String,
    started_at: String,
    finished_at: Option<String>,
    inputs: BTreeMap<String, String>,
    error: Option<String>,
    awaiting_step: Option<String>,
    awaiting_since: Option<String>,
    expires_at: Option<String>,
    steps: Vec<WorkflowShowRunStep>,
    usage_rows: Vec<WorkflowShowRunUsageRow>,
    usage_totals: Option<WorkflowShowRunUsageTotals>,
}

#[derive(Serialize)]
struct WorkflowShowRunReport {
    kind: &'static str,
    version: u8,
    item: WorkflowShowRunItem,
}

struct WorkflowShowRunOutput {
    prefs: crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
    record: rupu_orchestrator::RunRecord,
    step_results_log: PathBuf,
    report: WorkflowShowRunReport,
}

impl CollectionOutput for WorkflowRunsOutput {
    type JsonReport = WorkflowRunsReport;
    type CsvRow = WorkflowRunsRow;

    fn command_name(&self) -> &'static str {
        "workflow runs"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "run_id",
            "status",
            "started_at",
            "duration_seconds",
            "expires_in_seconds",
            "total_tokens",
            "cost_usd",
            "workflow",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "RUN ID",
            "STATUS",
            "STARTED (UTC)",
            "DURATION",
            "EXPIRES",
            "TOKENS",
            "COST",
            "WORKFLOW",
        ]);
        for row in &self.report.rows {
            let expires_cell = match row.expires_in_seconds {
                Some(delta) => crate::output::tables::relative_time_cell(delta, &self.prefs),
                None => comfy_table::Cell::new(""),
            };
            let duration = row
                .duration_seconds
                .map(|seconds| format!("{seconds}s"))
                .unwrap_or_else(|| "(in flight)".to_string());
            let cost = row
                .cost_usd
                .map(|value| format!("${value:.4}"))
                .unwrap_or_else(|| "—".to_string());
            table.add_row(vec![
                comfy_table::Cell::new(&row.run_id),
                crate::output::tables::status_cell(&row.status, &self.prefs),
                comfy_table::Cell::new(&row.started_at),
                comfy_table::Cell::new(duration),
                expires_cell,
                comfy_table::Cell::new(format_tokens_total(row.total_tokens)),
                comfy_table::Cell::new(cost),
                comfy_table::Cell::new(&row.workflow),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

impl DetailOutput for WorkflowShowOutput {
    type JsonReport = WorkflowShowReport;

    fn command_name(&self) -> &'static str {
        "workflow show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let width = crossterm::terminal::size()
            .map(|(value, _)| value.max(40) as usize)
            .unwrap_or(100);
        let rendered =
            render_workflow_show_snapshot(&self.report.item, self.view_mode, &self.prefs, width);
        crate::cmd::ui::paginate(&rendered, &self.prefs)
    }
}

impl EventOutput for WorkflowShowRunOutput {
    type JsonReport = WorkflowShowRunReport;
    type JsonlRow = serde_json::Value;

    fn command_name(&self) -> &'static str {
        "workflow show-run"
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        report::PRETTY_TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_pretty(&self) -> anyhow::Result<()> {
        render_pretty_workflow_run(
            &self.record,
            &self.step_results_log,
            &self.report.item.usage_rows,
            self.report.item.usage_totals.as_ref(),
            &self.prefs,
            self.view_mode,
        )
    }
}

fn render_pretty_workflow_run(
    record: &rupu_orchestrator::RunRecord,
    step_results_log: &Path,
    usage_rows: &[WorkflowShowRunUsageRow],
    usage_totals: Option<&WorkflowShowRunUsageTotals>,
    prefs: &crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
) -> anyhow::Result<()> {
    let width = crossterm::terminal::size()
        .map(|(value, _)| value.max(40) as usize)
        .unwrap_or(100);
    let mut body = crate::output::workflow_printer::render_workflow_snapshot_body(
        &record.workflow_name,
        record,
        step_results_log,
        view_mode,
        prefs,
        width,
    );
    let usage_block = render_workflow_usage_block(usage_rows, usage_totals);
    if !usage_block.is_empty() {
        body.push_str("\n\n");
        body.push_str(&usage_block);
    }
    body.push('\n');
    crate::cmd::ui::paginate(&body, prefs)
}

fn render_workflow_usage_block(
    usage_rows: &[WorkflowShowRunUsageRow],
    usage_totals: Option<&WorkflowShowRunUsageTotals>,
) -> String {
    if usage_rows.is_empty() {
        return String::new();
    }

    let mut lines = vec![styled_usage_line(
        UiStatus::Active,
        "usage",
        "provider/model/agent usage",
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "PROVIDER", "MODEL", "AGENT", "INPUT", "OUTPUT", "CACHED", "COST",
    ]);
    for row in usage_rows {
        table.add_row(vec![
            comfy_table::Cell::new(&row.provider),
            comfy_table::Cell::new(&row.model),
            comfy_table::Cell::new(&row.agent),
            comfy_table::Cell::new(row.input_tokens),
            comfy_table::Cell::new(row.output_tokens),
            comfy_table::Cell::new(row.cached_tokens),
            comfy_table::Cell::new(
                row.cost_usd
                    .map(|value| format!("${value:.4}"))
                    .unwrap_or_else(|| "—".into()),
            ),
        ]);
    }
    if let Some(totals) = usage_totals {
        table.add_row(vec![
            comfy_table::Cell::new("total"),
            comfy_table::Cell::new("—"),
            comfy_table::Cell::new("—"),
            comfy_table::Cell::new(totals.input_tokens),
            comfy_table::Cell::new(totals.output_tokens),
            comfy_table::Cell::new(totals.cached_tokens),
            comfy_table::Cell::new(
                totals
                    .cost_usd
                    .map(|value| format!("${value:.4}"))
                    .unwrap_or_else(|| "—".into()),
            ),
        ]);
    }
    lines.extend(table.to_string().lines().map(|line| line.to_string()));
    lines.join("\n")
}

fn styled_usage_line(status: UiStatus, label: &str, detail: &str) -> String {
    let mut buf = String::new();
    let _ = crate::output::palette::write_bold_colored(&mut buf, label, status.color());
    let _ = crate::output::palette::write_colored(&mut buf, "  ", crate::output::palette::DIM);
    let _ = crate::output::palette::write_colored(&mut buf, detail, crate::output::palette::DIM);
    buf
}

async fn list(global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // (name, scope) — project shadows global by name. We collect into
    // a BTreeMap to dedupe before printing.
    let mut by_name: BTreeMap<String, String> = BTreeMap::new();
    push_yaml_names(&global.join("workflows"), "global", &mut by_name);
    if let Some(p) = &project_root {
        // Project entries inserted second deliberately overwrite the
        // global scope chip for the same name.
        push_yaml_names(&p.join(".rupu/workflows"), "project", &mut by_name);
    }
    let cfg = layered_config_workflow(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None, None);
    let output = WorkflowListOutput {
        prefs,
        report: WorkflowListReport {
            kind: "workflow_list",
            version: 1,
            rows: by_name
                .into_iter()
                .map(|(name, scope)| WorkflowListRow { name, scope })
                .collect(),
        },
    };
    report::emit_collection(global_format, &output)
}

fn push_yaml_names(dir: &Path, scope: &str, into: &mut BTreeMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            into.insert(stem.to_string(), scope.to_string());
        }
    }
}

async fn show(
    name: &str,
    view: Option<LiveViewMode>,
    no_color: bool,
    theme: Option<&str>,
    pager_flag: Option<bool>,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    // UI prefs only — lock does not apply (I-7)
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, theme, pager_flag, view);
    let view_mode = prefs.live_view;
    let output = WorkflowShowOutput {
        prefs,
        view_mode,
        report: WorkflowShowReport {
            kind: "workflow_show",
            version: 1,
            item: WorkflowShowItem {
                name: name.to_string(),
                path: path.display().to_string(),
                body,
            },
        },
    };
    report::emit_detail(global_format, &output)
}

fn render_workflow_show_snapshot(
    item: &WorkflowShowItem,
    view_mode: LiveViewMode,
    prefs: &crate::cmd::ui::UiPrefs,
    width: usize,
) -> String {
    let mut rows = vec![
        render_workflow_show_header_line(item, view_mode, width),
        String::new(),
    ];

    match Workflow::parse(&item.body) {
        Ok(workflow) => {
            rows.extend(render_workflow_show_summary_rows(&workflow, item, width));
            rows.push(String::new());
            rows.push(render_workflow_show_section_header(
                "graph",
                "workflow structure",
                width,
            ));
            rows.extend(render_workflow_show_graph(&workflow, width));

            if matches!(view_mode, LiveViewMode::Compact | LiveViewMode::Full) {
                let input_rows = render_workflow_show_inputs(&workflow, width);
                if !input_rows.is_empty() {
                    rows.push(String::new());
                    rows.extend(input_rows);
                }
                let output_rows = render_workflow_show_outputs(&workflow, width);
                if !output_rows.is_empty() {
                    rows.push(String::new());
                    rows.extend(output_rows);
                }
                let detail_rows = render_workflow_show_step_details(&workflow, width);
                if !detail_rows.is_empty() {
                    rows.push(String::new());
                    rows.extend(detail_rows);
                }
            }

            if view_mode == LiveViewMode::Full {
                rows.push(String::new());
                rows.push(render_workflow_show_section_header(
                    "yaml",
                    "raw definition",
                    width,
                ));
                rows.extend(
                    crate::cmd::ui::highlight_yaml(&item.body, prefs)
                        .lines()
                        .map(|line| line.to_string()),
                );
            }
        }
        Err(err) => {
            rows.push(render_workflow_show_kv_row(
                "path",
                &item.path,
                width,
                UiStatus::Active,
            ));
            rows.push(render_workflow_show_kv_row(
                "parse",
                &err.to_string(),
                width,
                UiStatus::Failed,
            ));
            rows.push(String::new());
            rows.push(render_workflow_show_section_header(
                "yaml",
                "raw definition",
                width,
            ));
            rows.extend(
                crate::cmd::ui::highlight_yaml(&item.body, prefs)
                    .lines()
                    .map(|line| line.to_string()),
            );
        }
    }

    rows.join("\n") + "\n"
}

fn render_workflow_show_header_line(
    item: &WorkflowShowItem,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "workflow show", BRAND);
    let _ = palette::write_colored(&mut buf, "  ", DIM);
    let _ = palette::write_bold_colored(
        &mut buf,
        &crate::cmd::transcript::truncate_single_line(&item.name, 28),
        BRAND,
    );
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, view_mode.as_str(), DIM);
    truncate_workflow_show_ansi_line(&buf, width)
}

fn render_workflow_show_summary_rows(
    workflow: &Workflow,
    item: &WorkflowShowItem,
    width: usize,
) -> Vec<String> {
    let mut rows = vec![
        render_workflow_show_kv_row("path", &item.path, width, UiStatus::Active),
        render_workflow_show_kv_row(
            "trigger",
            &workflow_trigger_summary(&workflow.trigger),
            width,
            UiStatus::Active,
        ),
        render_workflow_show_kv_row(
            "steps",
            &format!(
                "{}  ·  agents {}  ·  inputs {}  ·  outputs {}",
                workflow.steps.len(),
                collect_workflow_agents(workflow).len(),
                workflow.inputs.len(),
                workflow.contracts.outputs.len()
            ),
            width,
            UiStatus::Active,
        ),
    ];

    if let Some(description) = workflow
        .description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        rows.push(render_workflow_show_kv_row(
            "description",
            description.trim(),
            width,
            UiStatus::Active,
        ));
    }

    rows.push(render_workflow_show_kv_row(
        "agents",
        &collect_workflow_agents(workflow)
            .into_iter()
            .collect::<Vec<_>>()
            .join(", "),
        width,
        UiStatus::Active,
    ));

    if let Some(autoflow) = workflow.autoflow.as_ref().filter(|value| value.enabled) {
        rows.push(render_workflow_show_kv_row(
            "autoflow",
            &workflow_autoflow_summary(autoflow),
            width,
            UiStatus::Active,
        ));
    }

    if workflow.notify_issue {
        rows.push(render_workflow_show_kv_row(
            "notify",
            "issue comments enabled",
            width,
            UiStatus::Awaiting,
        ));
    }

    rows
}

fn render_workflow_show_inputs(workflow: &Workflow, width: usize) -> Vec<String> {
    if workflow.inputs.is_empty() {
        return Vec::new();
    }

    let mut rows = vec![render_workflow_show_section_header(
        "inputs",
        "declared inputs",
        width,
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "NAME",
        "TYPE",
        "REQUIRED",
        "DEFAULT",
        "ENUM",
        "DESCRIPTION",
    ]);
    for (name, input) in &workflow.inputs {
        table.add_row(vec![
            comfy_table::Cell::new(name),
            comfy_table::Cell::new(workflow_input_type_name(input.ty)),
            comfy_table::Cell::new(if input.required { "yes" } else { "no" }),
            comfy_table::Cell::new(
                input
                    .default
                    .as_ref()
                    .map(yaml_scalar_inline)
                    .unwrap_or_else(|| "—".into()),
            ),
            comfy_table::Cell::new(if input.allowed.is_empty() {
                "—".to_string()
            } else {
                crate::cmd::transcript::truncate_single_line(&input.allowed.join(", "), 40)
            }),
            comfy_table::Cell::new(
                input
                    .description
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| crate::cmd::transcript::truncate_single_line(value.trim(), 48))
                    .unwrap_or_else(|| "—".into()),
            ),
        ]);
    }
    rows.extend(
        table
            .to_string()
            .lines()
            .map(|line| truncate_workflow_show_ansi_line(line, width)),
    );
    rows
}

fn render_workflow_show_outputs(workflow: &Workflow, width: usize) -> Vec<String> {
    if workflow.contracts.outputs.is_empty() {
        return Vec::new();
    }

    let mut rows = vec![render_workflow_show_section_header(
        "outputs",
        "declared outputs",
        width,
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["NAME", "FROM STEP", "FORMAT", "SCHEMA"]);
    for (name, output) in &workflow.contracts.outputs {
        table.add_row(vec![
            comfy_table::Cell::new(name),
            comfy_table::Cell::new(&output.from_step),
            comfy_table::Cell::new(workflow_contract_format_name(output.format)),
            comfy_table::Cell::new(&output.schema),
        ]);
    }
    rows.extend(
        table
            .to_string()
            .lines()
            .map(|line| truncate_workflow_show_ansi_line(line, width)),
    );
    rows
}

fn render_workflow_show_step_details(workflow: &Workflow, width: usize) -> Vec<String> {
    let mut rows = vec![render_workflow_show_section_header(
        "steps",
        "declared steps",
        width,
    )];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["ID", "KIND", "PRIMARY", "DETAIL"]);
    for step in &workflow.steps {
        let (kind, primary, detail) = workflow_step_table_summary(step);
        table.add_row(vec![
            comfy_table::Cell::new(&step.id),
            comfy_table::Cell::new(kind),
            comfy_table::Cell::new(primary),
            comfy_table::Cell::new(detail),
        ]);

        if let Some(sub_steps) = &step.parallel {
            for sub in sub_steps {
                table.add_row(vec![
                    comfy_table::Cell::new(format!("  ├─ {}", sub.id)),
                    comfy_table::Cell::new("substep"),
                    comfy_table::Cell::new(&sub.agent),
                    comfy_table::Cell::new(crate::cmd::transcript::truncate_single_line(
                        &sub.prompt,
                        48,
                    )),
                ]);
            }
        }
    }
    rows.extend(
        table
            .to_string()
            .lines()
            .map(|line| truncate_workflow_show_ansi_line(line, width)),
    );
    rows
}

fn render_workflow_show_graph(workflow: &Workflow, width: usize) -> Vec<String> {
    render_graph_rows(workflow, |_| NodeStatus::Waiting)
        .into_iter()
        .map(|row| render_workflow_show_graph_row(&row, width))
        .collect()
}

fn render_workflow_show_graph_row(row: &rupu_app_canvas::GraphRow, width: usize) -> String {
    let mut buf = String::new();
    for cell in &row.cells {
        match cell {
            GraphCell::Pipe(status) => {
                let _ = palette::write_colored(&mut buf, "│", node_status_color(*status));
            }
            GraphCell::Branch(glyph, status) => {
                let _ =
                    palette::write_colored(&mut buf, glyph.as_str(), node_status_color(*status));
            }
            GraphCell::Bullet(status) => {
                let _ = palette::write_bold_colored(
                    &mut buf,
                    &status.glyph().to_string(),
                    node_status_color(*status),
                );
            }
            GraphCell::Space(count) => {
                buf.push_str(&" ".repeat((*count).into()));
            }
            GraphCell::Label(label) => {
                let _ = palette::write_bold_colored(&mut buf, label, BRAND);
            }
            GraphCell::Meta(meta) => {
                let _ = palette::write_colored(&mut buf, meta, DIM);
            }
        }
    }
    truncate_workflow_show_ansi_line(&buf, width)
}

fn render_workflow_show_section_header(label: &str, detail: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, BRAND);
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
        let _ = palette::write_colored(&mut buf, detail, DIM);
    }
    truncate_workflow_show_ansi_line(&buf, width)
}

fn render_workflow_show_kv_row(label: &str, value: &str, width: usize, status: UiStatus) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), status.color());
    let _ = palette::write_colored(
        &mut buf,
        &crate::cmd::transcript::truncate_single_line(value, width.saturating_sub(11)),
        DIM,
    );
    truncate_workflow_show_ansi_line(&buf, width)
}

fn workflow_trigger_summary(trigger: &rupu_orchestrator::Trigger) -> String {
    match trigger.on {
        rupu_orchestrator::TriggerKind::Manual => "manual".into(),
        rupu_orchestrator::TriggerKind::Cron => match trigger.cron.as_deref() {
            Some(cron) => format!("cron  ·  {cron}"),
            None => "cron".into(),
        },
        rupu_orchestrator::TriggerKind::Event => {
            let mut parts = vec!["event".to_string()];
            if let Some(event) = trigger.event.as_deref() {
                parts.push(event.to_string());
            }
            if let Some(filter) = trigger
                .filter
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                parts.push(format!(
                    "filter {}",
                    crate::cmd::transcript::truncate_single_line(filter, 40)
                ));
            }
            parts.join("  ·  ")
        }
    }
}

fn workflow_autoflow_summary(autoflow: &rupu_orchestrator::Autoflow) -> String {
    let mut parts = vec![
        format!("{:?}", autoflow.entity).to_ascii_lowercase(),
        format!("priority {}", autoflow.priority),
    ];
    if let Some(source) = autoflow.source.as_deref() {
        parts.push(source.to_string());
    }
    if let Some(ttl) = autoflow
        .claim
        .as_ref()
        .and_then(|claim| claim.ttl.as_deref())
    {
        parts.push(format!("ttl {ttl}"));
    }
    parts.join("  ·  ")
}

fn collect_workflow_agents(workflow: &Workflow) -> std::collections::BTreeSet<String> {
    let mut agents = std::collections::BTreeSet::new();
    for step in &workflow.steps {
        if let Some(agent) = step.agent.as_deref().filter(|value| !value.is_empty()) {
            agents.insert(agent.to_string());
        }
        if let Some(subs) = &step.parallel {
            for sub in subs {
                if !sub.agent.is_empty() {
                    agents.insert(sub.agent.clone());
                }
            }
        }
        if let Some(panel) = &step.panel {
            for panelist in &panel.panelists {
                if !panelist.is_empty() {
                    agents.insert(panelist.clone());
                }
            }
            if let Some(gate) = &panel.gate {
                if !gate.fix_with.is_empty() {
                    agents.insert(gate.fix_with.clone());
                }
            }
        }
    }
    agents
}

fn workflow_step_table_summary(step: &rupu_orchestrator::Step) -> (&'static str, String, String) {
    if let Some(sub_steps) = &step.parallel {
        let primary = format!("{} sub-steps", sub_steps.len());
        let mut detail = String::new();
        if let Some(max_parallel) = step.max_parallel {
            detail.push_str(&format!("max_parallel {max_parallel}"));
        }
        return (
            "parallel",
            primary,
            if detail.is_empty() {
                "—".into()
            } else {
                detail
            },
        );
    }
    if let Some(panel) = &step.panel {
        let primary = format!("{} panelists", panel.panelists.len());
        let mut parts = vec![crate::cmd::transcript::truncate_single_line(
            &panel.panelists.join(", "),
            40,
        )];
        if let Some(max_parallel) = panel.max_parallel {
            parts.push(format!("max_parallel {max_parallel}"));
        }
        if let Some(gate) = &panel.gate {
            parts.push(format!(
                "gate {} → {} ({} iters)",
                gate.until_no_findings_at_severity_or_above.as_str(),
                gate.fix_with,
                gate.max_iterations
            ));
        }
        return ("panel", primary, parts.join("  ·  "));
    }
    if let Some(for_each) = step.for_each.as_deref() {
        let primary = step.agent.clone().unwrap_or_default();
        let mut parts = vec![crate::cmd::transcript::truncate_single_line(for_each, 32)];
        if let Some(max_parallel) = step.max_parallel {
            parts.push(format!("max_parallel {max_parallel}"));
        }
        if step
            .approval
            .as_ref()
            .is_some_and(|approval| approval.required)
        {
            parts.push("approval".into());
        }
        return ("for_each", primary, parts.join("  ·  "));
    }

    // ISSUES.md I-41: gate and action nodes are first-class step kinds and must
    // not fall through to the `linear` arm below, which would print KIND=linear
    // with a BLANK primary column — gates have no `agent:`, and an action step's
    // whole identity is the tool it calls. The graph renderer
    // (`rupu-app-canvas`'s `git_graph.rs`) already renders both correctly; this
    // table was the only view that didn't.
    if rupu_orchestrator::is_approval_gate(step) {
        let mut parts = Vec::new();
        if let Some(approval) = &step.approval {
            if let Some(expr) = approval
                .auto_approve
                .as_deref()
                .filter(|v| !v.trim().is_empty())
            {
                parts.push(format!(
                    "auto_approve {}",
                    crate::cmd::transcript::truncate_single_line(expr, 20)
                ));
            }
            if let Some(secs) = approval.timeout_seconds {
                parts.push(format!("timeout {secs}s"));
            }
            if let Some(on_timeout) = approval.on_timeout {
                parts.push(format!(
                    "on_timeout {}",
                    match on_timeout {
                        rupu_orchestrator::TimeoutAction::Approve => "approve",
                        rupu_orchestrator::TimeoutAction::Reject => "reject",
                        rupu_orchestrator::TimeoutAction::Fail => "fail",
                    }
                ));
            }
            if !approval.on_reject.is_empty() {
                parts.push(format!("on_reject {} step(s)", approval.on_reject.len()));
            }
            if !approval.notify.is_empty() {
                parts.push(format!("notify {}", approval.notify.len()));
            }
        }
        let primary = step
            .approval
            .as_ref()
            .and_then(|a| a.prompt.as_deref())
            .map(|p| crate::cmd::transcript::truncate_single_line(p, 32))
            .unwrap_or_else(|| "—".into());
        return (
            "gate",
            primary,
            if parts.is_empty() {
                "—".into()
            } else {
                parts.join("  ·  ")
            },
        );
    }

    if let Some(action) = &step.action {
        let mut parts = Vec::new();
        if let Some(with) = &step.with {
            if let Some(map) = with.as_object() {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                if !keys.is_empty() {
                    parts.push(format!("with {}", keys.join(", ")));
                }
            }
        }
        if let Some(when) = step
            .when
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            parts.push(format!(
                "when {}",
                crate::cmd::transcript::truncate_single_line(when, 28)
            ));
        }
        return (
            "action",
            action.clone(),
            if parts.is_empty() {
                "—".into()
            } else {
                parts.join("  ·  ")
            },
        );
    }

    let primary = step.agent.clone().unwrap_or_default();
    let mut parts = Vec::new();
    if !step.actions.is_empty() {
        parts.push(format!("actions {}", step.actions.join(", ")));
    }
    if let Some(when) = step
        .when
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!(
            "when {}",
            crate::cmd::transcript::truncate_single_line(when, 28)
        ));
    }
    if step
        .approval
        .as_ref()
        .is_some_and(|approval| approval.required)
    {
        parts.push("approval".into());
    }
    if let Some(contract) = &step.contract {
        parts.push(format!(
            "emits {} ({})",
            contract.emits,
            workflow_contract_format_name(contract.format)
        ));
    }
    (
        "linear",
        primary,
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join("  ·  ")
        },
    )
}

fn yaml_scalar_inline(value: &serde_yaml::Value) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| format!("{value:?}"))
        .replace('\n', " ")
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .to_string()
}

fn workflow_contract_format_name(format: rupu_orchestrator::ContractFormat) -> &'static str {
    match format {
        rupu_orchestrator::ContractFormat::Json => "json",
        rupu_orchestrator::ContractFormat::Yaml => "yaml",
    }
}

fn workflow_input_type_name(ty: rupu_orchestrator::InputType) -> &'static str {
    match ty {
        rupu_orchestrator::InputType::String => "string",
        rupu_orchestrator::InputType::Int => "int",
        rupu_orchestrator::InputType::Bool => "bool",
    }
}

fn node_status_color(status: NodeStatus) -> owo_colors::Rgb {
    match status {
        NodeStatus::Waiting => DIM,
        NodeStatus::Active | NodeStatus::Working => crate::output::palette::RUNNING,
        NodeStatus::Complete => crate::output::palette::COMPLETE,
        NodeStatus::Failed => crate::output::palette::FAILED,
        NodeStatus::SoftFailed => crate::output::palette::SOFT_FAILED,
        NodeStatus::Awaiting => crate::output::palette::AWAITING,
        NodeStatus::Retrying => crate::output::palette::RETRYING,
        NodeStatus::Skipped => crate::output::palette::SKIPPED,
    }
}

fn truncate_workflow_show_ansi_line(value: &str, width: usize) -> String {
    if visible_len(value) <= width {
        value.to_string()
    } else {
        wrap_with_ansi(value, width)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

/// Minimal valid workflow that parses through the orchestrator: one
/// linear step delegating to an agent. Comments mark common extension
/// points without locking the user into them.
const WORKFLOW_TEMPLATE: &str = r#"name: {{name}}
description: # one-line summary of what this workflow does

# Optional inputs become `{{ inputs.<key> }}` in step prompts.
# inputs:
#   topic:
#     type: string
#     required: true
#     description: What the workflow operates on.

steps:
  - id: main
    agent: # agent name from `rupu agent list`
    actions: []
    prompt: |
      Replace this with the prompt the agent should receive. You can
      reference inputs as {{ inputs.topic }} and prior step outputs as
      {{ steps.<id>.output }}.
"#;

async fn create(
    name: Option<String>,
    scope: Option<String>,
    editor_override: Option<&str>,
    describe: Option<String>,
    gen_provider: Option<String>,
    gen_model: Option<String>,
    host: &str,
) -> anyhow::Result<()> {
    if host != "local" {
        anyhow::bail!("host `{host}` is not available (only `local` today)");
    }
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let scope = match scope {
        Some(s) => s,
        None => crate::cmd::create_common::prompt_scope("workflow", project_root.as_deref())?,
    };
    let name = match name {
        Some(n) => {
            crate::cmd::create_common::validate_name(n.trim())?;
            n.trim().to_string()
        }
        None => crate::cmd::create_common::prompt_name("workflow")?,
    };

    let dir = crate::cmd::create_common::target_dir(
        &scope,
        &global,
        project_root.as_deref(),
        "workflows",
    )?;
    let target = dir.join(format!("{name}.yaml"));
    let yml_sibling = dir.join(format!("{name}.yml"));
    if target.exists() || yml_sibling.exists() {
        let existing = if target.exists() {
            &target
        } else {
            &yml_sibling
        };
        anyhow::bail!(
            "workflow `{name}` already exists at {} — use `rupu workflow edit {name}` to modify",
            existing.display()
        );
    }
    std::fs::create_dir_all(&dir)?;

    let contents = match describe {
        Some(desc) => {
            let resolver = rupu_auth::KeychainResolver::new();
            let (provider, model) = match (gen_provider, gen_model) {
                (Some(p), Some(m)) => (p, m),
                (Some(p), None) => {
                    let m = rupu_orchestrator::generate::DEFAULT_GEN_MODELS
                        .iter()
                        .find(|(n, _)| *n == p.as_str())
                        .map(|(_, m)| m.to_string())
                        .ok_or_else(|| anyhow::anyhow!("unknown --gen-provider `{p}`"))?;
                    (p, m)
                }
                (None, Some(m)) => {
                    anyhow::bail!("--gen-model `{m}` requires --gen-provider to be set")
                }
                (None, None) => rupu_orchestrator::pick_default_gen_model(&resolver)
                    .await
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no authenticated provider; run `rupu auth login` or pass \
                             --gen-provider/--gen-model"
                        )
                    })?,
            };
            // Real agent names so generated steps reference agents that exist.
            let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
            let available_agents = load_agent_specs(&global, project_agents_parent.as_deref())
                .map(|specs| specs.into_iter().map(|s| s.name).collect::<Vec<_>>())
                .unwrap_or_default();
            println!("generating workflow `{name}` via {provider}/{model}\u{2026}");
            let req = rupu_orchestrator::GenerateRequest {
                kind: rupu_orchestrator::GenKind::Workflow,
                description: desc,
                provider,
                model,
                available_agents,
            };
            // ISSUES.md I-74: pass the operator's `[providers.<name>]`
            // settings through instead of silently generating with none.
            let gen_cfg = layered_config_workflow(&global, project_root.as_deref());
            let gen_provider_config = rupu_runtime::provider_factory::ProviderConfig {
                anthropic_oauth_system_prefix: None,
                openai_compatible: rupu_runtime::provider_factory::openai_compatible_params(
                    &req.provider,
                    &gen_cfg.providers,
                ),
                tuning: Some(rupu_runtime::provider_factory::provider_tuning(
                    &req.provider,
                    &gen_cfg.providers,
                )),
            };
            let outcome =
                rupu_orchestrator::generate_definition(&req, &resolver, &gen_provider_config)
                    .await?;
            outcome.content
        }
        None => WORKFLOW_TEMPLATE.replace("{{name}}", &name),
    };

    std::fs::write(&target, &contents)?;
    println!("created {} ({scope})", target.display());

    crate::cmd::editor::open_for_edit(editor_override, &target)?;

    match Workflow::parse_file(&target) {
        Ok(_) => {
            println!("\u{2713} {name}: workflow YAML parses cleanly");
            Ok(())
        }
        Err(e) => {
            eprintln!("\u{26a0} {name}: failed to re-parse after save:\n  {e}");
            Ok(())
        }
    }
}

async fn edit(
    name: &str,
    scope: Option<&str>,
    editor_override: Option<&str>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let target = resolve_workflow_path(name, scope, &global, project_root.as_deref())?;
    let scope_label = if target.starts_with(&global) {
        "global"
    } else {
        "project"
    };
    println!("editing {} ({scope_label})", target.display());

    crate::cmd::editor::open_for_edit(editor_override, &target)?;

    match Workflow::parse_file(&target) {
        Ok(_) => {
            println!("✓ {name}: workflow YAML parses cleanly");
            Ok(())
        }
        Err(e) => {
            eprintln!("⚠ {name}: failed to re-parse after save:\n  {e}");
            Ok(())
        }
    }
}

/// Pick the on-disk file to edit. With `--scope` set we honor it
/// strictly; without it we prefer the project shadow if present and
/// fall back to global. Tries `.yaml` first, then `.yml`.
fn resolve_workflow_path(
    name: &str,
    scope: Option<&str>,
    global: &Path,
    project_root: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let candidates_for = |dir: PathBuf| -> Vec<PathBuf> {
        vec![
            dir.join(format!("{name}.yaml")),
            dir.join(format!("{name}.yml")),
        ]
    };

    let project_dir = project_root.map(|p| p.join(".rupu").join("workflows"));
    let global_dir = global.join("workflows");

    let pick =
        |dir: PathBuf| -> Option<PathBuf> { candidates_for(dir).into_iter().find(|p| p.exists()) };

    match scope {
        Some("project") => match project_dir {
            Some(d) => pick(d.clone()).ok_or_else(|| {
                anyhow::anyhow!(
                    "workflow `{name}` not found at project scope ({}/{name}.{{yaml,yml}})",
                    d.display()
                )
            }),
            None => Err(anyhow::anyhow!(
                "no project root detected; cannot use --scope project"
            )),
        },
        Some("global") => pick(global_dir.clone()).ok_or_else(|| {
            anyhow::anyhow!(
                "workflow `{name}` not found at global scope ({}/{name}.{{yaml,yml}})",
                global_dir.display()
            )
        }),
        Some(other) => Err(anyhow::anyhow!(
            "invalid --scope `{other}` (expected `global` or `project`)"
        )),
        None => {
            if let Some(d) = project_dir {
                if let Some(p) = pick(d) {
                    return Ok(p);
                }
            }
            pick(global_dir).ok_or_else(|| {
                anyhow::anyhow!("workflow `{name}` not found in project or global workflows dir")
            })
        }
    }
}

/// Read + parse a run's persisted workflow snapshot — no config/resolver/
/// MCP-registry/dispatcher rebuild, just the YAML on disk. Shared by
/// [`gate_on_timeout_for`] and [`gate_on_timeout_for_step`] so callers on
/// the same run don't each pay for their own read + parse.
fn read_and_parse_workflow_snapshot(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
) -> anyhow::Result<rupu_orchestrator::Workflow> {
    let body = store
        .read_workflow_snapshot(run_id)
        .map_err(|e| anyhow::anyhow!("read workflow snapshot: {e}"))?;
    rupu_orchestrator::Workflow::parse(&body)
        .map_err(|e| anyhow::anyhow!("parse workflow snapshot: {e}"))
}

/// Resolve `record`'s gate `on_timeout` policy for the lazy-expiry
/// checks the CLI does before an `approve` / the `runs` listing's
/// sweep, by loading the run's persisted workflow snapshot.
/// Best-effort — any failure (unreadable/unparseable snapshot, no
/// awaiting step, step isn't a gate NODE, no `on_timeout` set)
/// collapses to `None`, which `expire_if_overdue` treats as the
/// default `Fail`.
///
/// **Sole-gate-shaped** — resolves the policy for `record.awaiting_step_id`
/// (the compat mirror of the FIRST parked gate). Task 5b-2a's per-gate
/// callers (an explicit `--gate <id>`) want
/// [`gate_on_timeout_for_step`] instead.
fn gate_on_timeout_for(
    store: &rupu_orchestrator::RunStore,
    record: &rupu_orchestrator::RunRecord,
) -> Option<rupu_orchestrator::TimeoutAction> {
    let step_id = record.awaiting_step_id.as_deref()?;
    gate_on_timeout_for_step(store, record, step_id)
}

/// Per-gate counterpart to [`gate_on_timeout_for`] (Task 5b-2a): resolve
/// the `on_timeout` routing for the gate NODE named `step_id`, independent
/// of which gate the compat fields happen to mirror. Used by `approve`/
/// `reject`'s `--gate`-targeted overdue pre-check.
fn gate_on_timeout_for_step(
    store: &rupu_orchestrator::RunStore,
    record: &rupu_orchestrator::RunRecord,
    step_id: &str,
) -> Option<rupu_orchestrator::TimeoutAction> {
    let workflow = match read_and_parse_workflow_snapshot(store, &record.id) {
        Ok(wf) => wf,
        Err(e) => {
            eprintln!(
                "warning: could not read workflow snapshot for run {} to resolve gate \
                 on_timeout; defaulting to `fail`: {e}",
                record.id
            );
            return None;
        }
    };
    rupu_orchestrator::gate_timeout_action(&workflow, step_id)
}

// `cheap_on_reject_chain_len` (a pre-check that skipped
// `build_reject_cleanup_opts` + `run_reject_cleanup` entirely for an empty
// `on_reject:` chain) was removed as part of I-36: `run_reject_cleanup` is
// the ONLY caller of `emit_gate_result`, so skipping it for an empty chain
// meant no gate decision row was ever written for that reject. Every
// caller now runs the (heavier, but still correct) full rebuild
// unconditionally; `build_reject_cleanup_opts` still returns the chain
// length so callers can decide whether to print "cleanup: N step(s)
// executed".

async fn runs(
    limit: usize,
    status_filter: Option<&str>,
    issue_filter: Option<&str>,
    no_color: bool,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let runs_dir = global.join("runs");
    let store = rupu_orchestrator::RunStore::new(runs_dir.clone());
    let mut all = store
        .list()
        .map_err(|e| anyhow::anyhow!("run-store list failed: {e}"))?;

    // Lazy expiry: any AwaitingApproval row whose expires_at is in
    // the past gets resolved per the gate's `on_timeout` policy
    // (default `fail`) before we render. Operators learn about
    // expired runs the next time they look at the list.
    let now = chrono::Utc::now();
    for r in &mut all {
        let on_timeout = gate_on_timeout_for(&store, r);

        // ISSUES.md I-25: `rupu workflow runs` is a *listing* command and
        // must have no external side effects — merely running it must
        // never post comments, open PRs, or run agent steps against
        // external systems. `expire_if_overdue` itself is safe to call
        // lazily here for `on_timeout: approve` (it deliberately leaves the
        // record `AwaitingApproval`; nothing but an informational
        // `println!` follows) and for `fail`/unset (the record is fully
        // finalized inside the expire call itself — there is no cleanup
        // chain to run). `reject` is the one case that is NOT safe: this
        // listing must never itself execute the gate's `on_reject` chain,
        // and it also must not even flip the record to `Rejected`, because
        // that finalization and the chain execution are supposed to
        // happen together and only the `cp serve` gate sweep is allowed to
        // do both. `sweep_decision` (`crates/rupu-cli/src/cmd/cp.rs`) only
        // ever produces `ExpireThenCleanupReject` for a run still
        // `AwaitingApproval`; every other status falls through to `Skip`.
        // So if this listing expired the run to `Rejected` on its own (as
        // it used to, right before running the chain inline), the sweep
        // would classify it `Skip` on every later tick and the
        // `on_reject` chain would never run at all — trading a
        // side-effecting read for a silently dropped cleanup, which is
        // worse. We therefore skip calling `expire_if_overdue` entirely
        // for `on_timeout: reject`: the run stays parked `AwaitingApproval`
        // and resolving it (expiring it AND running its `on_reject` chain,
        // together, in the same tick) becomes solely the `cp serve` gate
        // sweep's job. If the sweep is disabled
        // (`[cp].gate_sweep_enabled = false`) or `rupu cp serve` is never
        // started, such a gate simply stays parked until an operator runs
        // `rupu workflow reject` by hand — there is no other path that
        // will resolve it.
        if on_timeout == Some(rupu_orchestrator::TimeoutAction::Reject) {
            // ISSUES.md I-80: this listing deliberately leaves the gate
            // parked (see the I-25 note above), which means an operator who
            // never runs `rupu cp serve` would otherwise see an overdue gate
            // sit here forever with no explanation. Say so once, with the
            // manual remedy, rather than letting the dependency stay
            // invisible. Only fires for a gate that is ACTUALLY overdue —
            // a reject gate still within its timeout is just parked normally
            // and needs no comment.
            if r.expires_at.is_some_and(|exp| exp <= now) {
                println!(
                    "rupu: run {} has an overdue gate with on_timeout: reject — \
                     resolution is performed by `rupu cp serve`'s gate sweep.\n      \
                     If you don't run `cp serve`, resolve it with \
                     `rupu workflow reject {} --reason \"...\"`.",
                    r.id, r.id
                );
            }
            continue;
        }

        match store.expire_if_overdue(r, now, on_timeout) {
            Ok(Some(rupu_orchestrator::TimeoutAction::Approve)) => {
                println!(
                    "rupu: gate timed out with on_timeout: approve — \
                     run `rupu workflow approve {}` to resume",
                    r.id
                );
            }
            // Unreachable: `on_timeout == Some(Reject)` is filtered out
            // above before this call, so `expire_if_overdue` can never
            // return `Reject` here.
            Ok(Some(rupu_orchestrator::TimeoutAction::Reject)) => {}
            Ok(Some(rupu_orchestrator::TimeoutAction::Fail)) | Ok(None) => {}
            Err(e) => {
                eprintln!("warning: expiry check failed for run {}: {e}", r.id);
            }
        }
    }

    // Normalize the optional issue filter once. Accepts the same
    // forms `rupu issues show / run` accept; we resolve to the
    // canonical `<tracker>:<project>/issues/<N>` text and compare
    // against `RunRecord.issue_ref` verbatim.
    let issue_filter_canonical: Option<String> = match issue_filter {
        None => None,
        Some(s) => Some(super::issues::canonical_issue_ref(s)?),
    };

    let filtered: Vec<_> = all
        .into_iter()
        .filter(|r| match status_filter {
            None => true,
            Some(s) => r.status.as_str() == s,
        })
        .filter(|r| match &issue_filter_canonical {
            None => true,
            Some(canonical) => r.issue_ref.as_deref() == Some(canonical.as_str()),
        })
        .take(limit)
        .collect();

    if filtered.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        let scope = match (status_filter, issue_filter_canonical.as_deref()) {
            (None, None) => "(no runs yet — use `rupu workflow run <name>` to create one)".into(),
            (Some(s), None) => format!("(no runs match status={s})"),
            (None, Some(i)) => format!("(no runs match issue={i})"),
            (Some(s), Some(i)) => format!("(no runs match status={s}, issue={i})"),
        };
        println!("{scope}");
        return Ok(());
    }

    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = layered_config_workflow(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None, None);

    let rows: Vec<WorkflowRunsRow> = filtered
        .iter()
        .map(|run| {
            let agg = aggregate_run_usage_from_store(&store, &run.id);
            WorkflowRunsRow {
                run_id: run.id.clone(),
                status: run.status.as_str().to_string(),
                started_at: run.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                duration_seconds: run
                    .finished_at
                    .map(|finished| (finished - run.started_at).num_seconds()),
                expires_in_seconds: run.expires_at.map(|expires| (expires - now).num_seconds()),
                total_tokens: total_tokens(&agg),
                cost_usd: run_cost_usd(&agg, &cfg.pricing),
                workflow: run.workflow_name.clone(),
            }
        })
        .collect();
    let output = WorkflowRunsOutput {
        prefs,
        report: WorkflowRunsReport {
            kind: "workflow_runs",
            version: 1,
            summary: WorkflowRunsSummary {
                count: rows.len(),
                limit,
                status_filter: status_filter.map(str::to_string),
                issue_filter: issue_filter_canonical,
            },
            rows,
        },
    };
    report::emit_collection(global_format, &output)
}

/// Per-step transcripts for one run, sourced from the run's
/// `step_results.jsonl`. Includes panel sub-run transcripts
/// (`items[].transcript_path`) so a panel-of-3 review counts all
/// three reviewers' tokens.
///
/// This is the version used by `rupu workflow runs`: scoping to one
/// run via the run-store avoids the double-count you'd get from
/// scanning the project-wide `transcript_dir` (which collects every
/// run's transcripts together).
fn aggregate_run_usage_from_store(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
) -> Vec<rupu_transcript::UsageRow> {
    let Ok(records) = store.read_step_results(run_id) else {
        return Vec::new();
    };
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for rec in &records {
        paths.push(rec.transcript_path.clone());
        for item in &rec.items {
            paths.push(item.transcript_path.clone());
        }
    }
    rupu_transcript::aggregate(&paths, rupu_transcript::TimeWindow::default())
}

fn total_tokens(rows: &[rupu_transcript::UsageRow]) -> u64 {
    rows.iter().map(|r| r.input_tokens + r.output_tokens).sum()
}

fn format_tokens_total(total: u64) -> String {
    if total >= 1_000_000 {
        format!("{:.2}M", total as f64 / 1_000_000.0)
    } else if total >= 1_000 {
        format!("{:.1}K", total as f64 / 1_000.0)
    } else {
        total.to_string()
    }
}

fn run_cost_usd(
    rows: &[rupu_transcript::UsageRow],
    pricing: &rupu_config::PricingConfig,
) -> Option<f64> {
    let mut total = 0.0f64;
    let mut any = false;
    for r in rows {
        if let Some(p) = rupu_config::pricing::lookup(pricing, &r.provider, &r.model, &r.agent) {
            total += p.cost_usd(r.input_tokens, r.output_tokens, r.cached_tokens);
            any = true;
        }
    }
    any.then_some(total)
}

fn layered_config_workflow(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> rupu_config::Config {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

/// Minimal per-run metadata needed to disambiguate a run-id fragment
/// match. Deliberately not `RunRecord` itself: building a full record
/// in a unit test means spelling out every field, and that crate's
/// `sample_record` helper is test-private. This holds only what the
/// ambiguity message needs.
struct RunCandidate {
    id: String,
    workflow_name: String,
    status: rupu_orchestrator::RunStatus,
    started_at: chrono::DateTime<chrono::Utc>,
}

/// Resolve a run-id fragment against a candidate list.
///
/// Pure, so it is unit-testable without a `RunStore`.
///
/// Accepts a full id, the compact `head…tail` form printed by tables, a
/// bare suffix, or an unambiguous prefix.
fn resolve_run_id(candidates: &[RunCandidate], fragment: &str) -> anyhow::Result<String> {
    use crate::output::ids::{resolve, Resolution};

    let ids: Vec<String> = candidates.iter().map(|c| c.id.clone()).collect();
    match resolve(&ids, fragment) {
        Resolution::Unique(id) => Ok(id),
        Resolution::NotFound => anyhow::bail!("unknown run: {fragment}"),
        Resolution::Ambiguous(matches) => {
            // Every candidate at full length, with disambiguating
            // context (workflow name, status, started-at) — the fast
            // path above (`store.exists`) already returned for the
            // common case, so this branch, which needs the records
            // anyway to detect the ambiguity, is the only place this
            // context is needed. No extra I/O.
            let now = chrono::Utc::now();
            let name_width = matches
                .iter()
                .filter_map(|id| candidates.iter().find(|c| &c.id == id))
                .map(|c| c.workflow_name.chars().count())
                .max()
                .unwrap_or(0);
            let status_width = matches
                .iter()
                .filter_map(|id| candidates.iter().find(|c| &c.id == id))
                .map(|c| c.status.as_str().chars().count())
                .max()
                .unwrap_or(0);
            let mut msg = format!(
                "ambiguous run id — {} runs match `{fragment}`",
                matches.len()
            );
            for id in &matches {
                match candidates.iter().find(|c| &c.id == id) {
                    Some(c) => {
                        let when = crate::output::fmt::relative_time(c.started_at, now);
                        msg.push_str(&format!(
                            "\n  {id}  {:<name_width$}  {:<status_width$}  {when}",
                            c.workflow_name,
                            c.status.as_str(),
                        ));
                    }
                    None => msg.push_str(&format!("\n  {id}")),
                }
            }
            anyhow::bail!(msg)
        }
    }
}

/// Gather active and archived run ids and resolve `fragment` against
/// both, so an archived run is never shadowed by an active one.
///
/// `pub(crate)` so `cmd::run` can route through the SAME resolution
/// `cmd::workflow` uses — one shared implementation means `rupu run show`
/// and `rupu workflow show-run` accept identical identifiers instead of
/// silently diverging.
///
/// Checks `store.exists(fragment)` first: an exact id — the common case of
/// pasting back what a previous command just printed — costs two `is_file`
/// stats via [`RunStore::exists`], not a full deserialize of every run on
/// disk. `exists` covers both the active and archived scopes, so an
/// archived run's exact id still resolves without falling through to the
/// `list()`/`list_archived()` scan below.
pub(crate) fn resolve_run_fragment(
    store: &rupu_orchestrator::RunStore,
    fragment: &str,
) -> anyhow::Result<String> {
    if store.exists(fragment) {
        return Ok(fragment.to_string());
    }
    // NOTE: the field is `id`, not `run_id` (runs.rs:107).
    let mut records = store.list().context("list runs")?;
    records.extend(store.list_archived().context("list archived runs")?);
    let candidates: Vec<RunCandidate> = records
        .into_iter()
        .map(|r| RunCandidate {
            id: r.id,
            workflow_name: r.workflow_name,
            status: r.status,
            started_at: r.started_at,
        })
        .collect();
    resolve_run_id(&candidates, fragment)
}

async fn show_run(
    run_id: &str,
    view: Option<LiveViewMode>,
    no_color: bool,
    pager_flag: Option<bool>,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = layered_config_workflow(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, pager_flag, view);
    let runs_dir = global.join("runs");
    let store = rupu_orchestrator::RunStore::new(runs_dir.clone());
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();
    let record = store.load(run_id).map_err(|e| {
        anyhow::anyhow!(
            "run not found: {e}\n  hint: list runs with `rupu workflow runs` \
                 or start one with `rupu workflow run <name>`"
        )
    })?;
    let rows = store
        .read_step_results(run_id)
        .map_err(|e| anyhow::anyhow!("read step results failed: {e}"))?;

    let usage_rows = aggregate_run_usage_from_store(&store, run_id);
    let usage_detail_rows = usage_rows
        .iter()
        .map(|r| WorkflowShowRunUsageRow {
            provider: r.provider.clone(),
            model: r.model.clone(),
            agent: r.agent.clone(),
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            cached_tokens: r.cached_tokens,
            cost_usd: rupu_config::pricing::lookup(&cfg.pricing, &r.provider, &r.model, &r.agent)
                .map(|p| p.cost_usd(r.input_tokens, r.output_tokens, r.cached_tokens)),
        })
        .collect::<Vec<_>>();
    let usage_totals = (!usage_rows.is_empty()).then(|| WorkflowShowRunUsageTotals {
        input_tokens: usage_rows.iter().map(|r| r.input_tokens).sum(),
        output_tokens: usage_rows.iter().map(|r| r.output_tokens).sum(),
        cached_tokens: usage_rows.iter().map(|r| r.cached_tokens).sum(),
        cost_usd: run_cost_usd(&usage_rows, &cfg.pricing),
    });
    let step_rows = rows
        .iter()
        .map(|row| WorkflowShowRunStep {
            step_id: row.step_id.clone(),
            status: if row.skipped {
                "skipped".into()
            } else if row.success {
                "ok".into()
            } else {
                "fail".into()
            },
            transcript_path: row.transcript_path.display().to_string(),
            items: row
                .items
                .iter()
                .map(|item| WorkflowShowRunStepItem {
                    label: if !item.sub_id.is_empty() {
                        item.sub_id.clone()
                    } else {
                        format!("[{}]", item.index)
                    },
                    status: if item.success {
                        "ok".into()
                    } else {
                        "fail".into()
                    },
                    transcript_path: item.transcript_path.display().to_string(),
                })
                .collect(),
        })
        .collect();
    let view_mode = prefs.live_view;
    let output = WorkflowShowRunOutput {
        prefs,
        view_mode,
        record: record.clone(),
        step_results_log: runs_dir.join(run_id).join("step_results.jsonl"),
        report: WorkflowShowRunReport {
            kind: "workflow_show_run",
            version: 1,
            item: WorkflowShowRunItem {
                run_id: record.id,
                workflow: record.workflow_name,
                status: record.status.as_str().to_string(),
                workspace_id: record.workspace_id,
                workspace_path: record.workspace_path.display().to_string(),
                started_at: record
                    .started_at
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string(),
                finished_at: record
                    .finished_at
                    .map(|value| value.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                inputs: record.inputs,
                error: record.error_message,
                awaiting_step: record.awaiting_step_id,
                awaiting_since: record
                    .awaiting_since
                    .map(|value| value.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                expires_at: record
                    .expires_at
                    .map(|value| value.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                steps: step_rows,
                usage_rows: usage_detail_rows,
                usage_totals,
            },
        },
    };
    report::emit_event(global_format, &output)
}

/// User-facing message for `ApprovalError::AmbiguousGate` (Task 5b-2a) —
/// pulled out of [`resolve_approve_gate`]/[`resolve_reject_gate`] so it's
/// unit-testable without the full CLI wiring those async fns need
/// (config/resolver/MCP registry). Lists every candidate gate id and
/// tells the operator which flag resolves it.
fn ambiguous_gate_message(action: &str, run_id: &str, candidates: &[String]) -> String {
    format!(
        "run `{run_id}` has {} pending gates ({}) — pass --gate <STEP_ID> to {action} one, \
         e.g. `rupu workflow {action} {run_id} --gate {}`",
        candidates.len(),
        candidates.join(", "),
        candidates
            .first()
            .map(String::as_str)
            .unwrap_or("<STEP_ID>"),
    )
}

/// Outcome of [`resolve_approve_gate`] — phase 1 of `approve` (Task
/// 5b-2a).
#[derive(Debug)]
enum ApproveGateOutcome {
    /// The targeted gate was approved; resume from this step id.
    Approved {
        step_id: String,
        /// I-36: the operator's identity, threaded into
        /// `resume::resume_run` -> `ResumeState::from_approval_with_actor`
        /// so the gate's persisted decision names who approved.
        approver: String,
        /// I-38: `true` when this approve landed on an already-overdue
        /// `on_timeout: approve` gate (this call merely observed/confirmed
        /// a decision the gate's own policy already made) — flips the
        /// persisted decision's `via` from `"human"` to `"timeout"`.
        via_timeout: bool,
    },
    /// The gate had already auto-rejected on `on_timeout: reject` before
    /// this operator approve landed — the store already finalized the run
    /// `Rejected`. The caller (phase 2, async) still needs to run the
    /// on_reject cleanup chain for `step_id`.
    ExpiredRejected {
        step_id: String,
        reason: String,
        /// I-36: who observed/reported the already-expired gate (this
        /// call's operator, even though the actual decision was
        /// timeout-driven — see `run_reject_cleanup`'s `approver` doc).
        approver: String,
    },
}

/// Phase 1 of `approve` (Task 5b-2a): the overdue pre-check message +
/// `store.approve_gate` call + friendly error mapping, split out of
/// `approve` itself so this gate-targeting logic is unit-testable without
/// the full resume wiring (config/resolver/MCP registry/dispatcher) phase
/// 2 needs. `gate: None` is the legacy sole-gate case — identical
/// behavior to before this task; `gate: Some(id)` targets that gate,
/// leaving any other parked gate untouched.
fn resolve_approve_gate(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
    gate: Option<&str>,
    approver_override: Option<&str>,
) -> anyhow::Result<ApproveGateOutcome> {
    // ISSUES.md I-82: a web-initiated approve threads its true actor in via
    // `approver_override` (ultimately `--approver`, set by the cp-serve
    // resume worker from the run's `resume_approver` marker); a direct
    // operator invocation passes `None` and falls back to the OS user, as
    // before this override existed.
    let approver = approver_override
        .map(str::to_string)
        .unwrap_or_else(whoami::username);

    // `store.approve_gate()` treats a timed-out `on_timeout: approve`
    // gate identically to an operator approve (it falls through and
    // resumes normally) — it has no way to tell the caller that's what
    // happened, so detect it here first purely to print a distinct
    // message before making the same library call. Task 5b-2a: check the
    // TARGETED gate's own overdue-ness/policy (falling back to the
    // compat-mirrored first gate when `--gate` is omitted) rather than
    // always the first gate in the set — a multi-gate run's other gates
    // may have different policies entirely.
    //
    // I-38: `via_timeout` captures this same detection (previously used
    // only for the `println!` below and then discarded) so the caller can
    // thread it into the resumed run's gate decision — a sweep-driven
    // `on_timeout: approve` must record `via: "timeout"`, not `"human"`.
    let mut via_timeout = false;
    if let Ok(record) = store.load(run_id) {
        let target_step_id = gate
            .map(str::to_string)
            .or_else(|| record.awaiting_step_id.clone());
        if let Some(step_id) = target_step_id {
            let gate_expires_at = record
                .awaiting_gates()
                .into_iter()
                .find(|g| g.step_id == step_id)
                .and_then(|g| g.expires_at);
            let overdue = record.status == rupu_orchestrator::RunStatus::AwaitingApproval
                && gate_expires_at.is_some_and(|exp| chrono::Utc::now() > exp);
            if overdue
                && gate_on_timeout_for_step(store, &record, &step_id)
                    == Some(rupu_orchestrator::TimeoutAction::Approve)
            {
                via_timeout = true;
                println!(
                    "rupu: gate `{step_id}` timed out with on_timeout: approve — \
                     auto-approving and resuming"
                );
            }
        }
    }

    // Library call replaces inline load + expire-check + status check +
    // mutate + update. Re-entering run_workflow stays in the CLI because
    // the TUI uses a different resume model.
    match store.approve_gate(run_id, &approver, chrono::Utc::now(), gate) {
        Ok(rupu_orchestrator::ApprovalDecision::Approved { step_id, .. }) => {
            Ok(ApproveGateOutcome::Approved {
                step_id,
                approver,
                via_timeout,
            })
        }
        Err(rupu_orchestrator::ApprovalError::Expired(msg)) => {
            anyhow::bail!("approval expired before it was acted on — {msg}");
        }
        Err(rupu_orchestrator::ApprovalError::ExpiredRejected { step_id, reason }) => {
            // The gate's own `on_timeout: reject` policy fired before
            // this operator approve landed — the store already
            // finalized the run as `Rejected`. Report it; the caller
            // runs the same `on_reject` cleanup chain a normal reject
            // does.
            println!(
                "rupu: gate timed out (on_timeout: reject) — run {run_id} auto-rejected at step `{step_id}`"
            );
            Ok(ApproveGateOutcome::ExpiredRejected {
                step_id,
                reason,
                approver,
            })
        }
        Err(rupu_orchestrator::ApprovalError::NotAwaiting(s)) => {
            anyhow::bail!(
                "run is `{s}`, not `awaiting_approval` — only paused runs can be approved",
            );
        }
        Err(rupu_orchestrator::ApprovalError::NoAwaitingStep) => {
            anyhow::bail!("run has no awaiting_step_id; record may be corrupt");
        }
        Err(rupu_orchestrator::ApprovalError::NotFound(id)) => {
            anyhow::bail!(
                "run not found: {id}\n  hint: \
                 list paused runs with `rupu workflow runs --status awaiting_approval`"
            );
        }
        Err(rupu_orchestrator::ApprovalError::AmbiguousGate { run_id, candidates }) => {
            anyhow::bail!(ambiguous_gate_message("approve", &run_id, &candidates));
        }
        Err(rupu_orchestrator::ApprovalError::GateNotFound { run_id, step_id }) => {
            anyhow::bail!(
                "gate `{step_id}` is not awaiting approval on run `{run_id}` — check \
                 `rupu workflow show-run {run_id}` for the currently parked gate ids"
            );
        }
        Err(e) => Err(anyhow::anyhow!("approve: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    }
}

async fn approve(
    run_id: &str,
    mode: Option<&str>,
    gate: Option<&str>,
    approver_override: Option<&str>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let runs_dir = global.join("runs");
    let store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();

    let (awaited_step_id, approver, via_timeout) =
        match resolve_approve_gate(&store, run_id, gate, approver_override)? {
            ApproveGateOutcome::Approved {
                step_id,
                approver,
                via_timeout,
            } => (step_id, approver, via_timeout),
            ApproveGateOutcome::ExpiredRejected {
                step_id,
                reason,
                approver,
            } => {
                // I-36: run the cleanup chain unconditionally, not only when
                // `cheap_on_reject_chain_len` reports a non-empty chain — an
                // empty chain must still record the gate's rejected decision
                // (`run_reject_cleanup` itself already handles a zero-length
                // chain fine; `chain_len` is only used below to decide whether
                // to print "cleanup: N step(s) executed").
                match crate::resume::build_reject_cleanup_opts(
                    &store, run_id, &step_id, &reason, mode,
                )
                .await
                {
                    Ok((opts, chain_len)) => {
                        match rupu_orchestrator::runner::run_reject_cleanup(
                            opts,
                            &step_id,
                            &reason,
                            "timeout",
                            Some(&approver),
                        )
                        .await
                        {
                            Ok(()) => {
                                if chain_len > 0 {
                                    println!("cleanup: {chain_len} step(s) executed");
                                }
                            }
                            Err(e) => eprintln!("warning: on_reject cleanup chain errored: {e}"),
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: could not load workflow for on_reject cleanup: {e} \
                         (run is already correctly rejected)"
                        );
                    }
                }
                return Ok(());
            }
        };
    // Phase 2 — the resume — lives in `crate::resume::resume_run` so the
    // background session worker can resume an approved gate identically.
    // `awaited_step_id` is threaded in because `approve` clears the
    // record's `awaiting_step_id`, so it can't be recovered post-flip.
    let outcome = crate::resume::resume_run(
        &store,
        run_id,
        &awaited_step_id,
        mode,
        &approver,
        via_timeout,
    )
    .await?;
    let awaited_step_id = outcome.awaited_step_id;
    let result = outcome.result;
    println!(
        "rupu: resumed run {} from step `{}`",
        result.run_id, awaited_step_id
    );
    for sr in &result.step_results {
        if sr.run_id.is_empty() {
            continue;
        }
        // Only show the steps the resume actually dispatched —
        // priors have run_id from a previous process and were
        // already printed when the run originally started.
        let was_prior = sr.transcript_path.exists() && sr.run_id.starts_with("run_");
        if was_prior {
            // Heuristic: the persisted prior steps will satisfy
            // both conditions; `run_workflow` records the freshly
            // dispatched ones too, but we don't have an easy way
            // to distinguish from inside the result. Print both for
            // now; future polish can dedupe via a stored boundary.
        }
        println!(
            "rupu: step {} run {} -> {}",
            sr.step_id,
            sr.run_id,
            sr.transcript_path.display()
        );
    }
    match &result.awaiting {
        Some(info) => {
            println!();
            println!(
                "rupu: workflow paused again at step `{}` (run {})",
                info.step_id, result.run_id
            );
            println!("      prompt: {}", info.prompt);
            println!(
                "      approve with: rupu workflow approve {}",
                result.run_id
            );
        }
        None => {
            println!(
                "rupu: workflow run {} finished (inspect with: rupu workflow show-run {})",
                result.run_id, result.run_id
            );
        }
    }
    Ok(())
}

/// Resume a terminal (`failed` / `cancelled` / `rejected`) run, or a
/// cooperatively-`paused` one, re-running only the agent runs that didn't
/// already succeed. Completed steps are skipped wholesale (their
/// `step_results.jsonl` rows are replayed into context); a
/// partially-completed fan-out step re-runs but its already-succeeded units
/// are replayed from the `unit_checkpoints.jsonl` instead of re-dispatched.
///
/// A `Paused` run additionally carries a persisted mid-step seed transcript
/// (`RunStore::read_paused_seed`) when the pause landed inside a linear
/// step's agent turn (see `docs/superpowers/plans/2026-07-01-rupu-pause-resume-plan.md`
/// Task 4) — when present it seeds `ResumeState::paused_step` so the exact
/// paused step re-runs from where the agent left off instead of from
/// scratch. A step-boundary pause (no seed) just replays like a terminal
/// resume.
pub(crate) async fn resume_run(
    run_id: &str,
    mode: Option<&str>,
    plain: bool,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let runs_dir = global.join("runs");
    let store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();

    let mut record = store.load(run_id).map_err(|e| match e {
        rupu_orchestrator::RunStoreError::NotFound(id) => {
            anyhow::anyhow!("run not found: {id}\n  hint: list runs with `rupu workflow runs`")
        }
        other => anyhow::anyhow!("load run record: {other}"),
    })?;

    // Guard: don't double-run an in-flight run, and don't re-run a
    // run that already completed.
    use rupu_orchestrator::RunStatus;
    let original_status = record.status;
    match record.status {
        RunStatus::Running | RunStatus::Pending => {
            anyhow::bail!(
                "run {run_id} is `{}` — refusing to resume an in-flight run (cancel it first with `rupu workflow cancel {run_id}`)",
                record.status.as_str()
            );
        }
        RunStatus::Completed => {
            anyhow::bail!("run {run_id} already completed — nothing to resume");
        }
        RunStatus::AwaitingApproval => {
            anyhow::bail!(
                "run {run_id} is awaiting approval — use `rupu workflow approve {run_id}` (or `reject`) instead of `resume`"
            );
        }
        // Resume applies to terminal failure states AND a cooperatively
        // paused run (non-terminal, but equally "not currently running").
        RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled | RunStatus::Paused => {}
    }

    // Duplicate-execution guard. A cooperatively `Paused` run whose original
    // process is still alive has NOT honored the pause yet — the process
    // clears `runner_pid` only once it stops at a safe boundary and re-writes
    // the record as `Paused`. Re-launching now would spawn a SECOND process
    // racing the first over the same run dir (duplicate side effects). Refuse
    // until the original exits; the caller (operator or CP resume worker)
    // retries shortly. Terminal states (Failed/Rejected/Cancelled) carry
    // `runner_pid = None`, so this only fires for a still-stopping pause.
    if let Some(pid) = resume_blocked_by_live_runner(record.runner_pid, std::process::id()) {
        anyhow::bail!(
            "run {run_id} is still stopping (runner pid {pid} is live) — the pause hasn't taken effect yet; retry `rupu workflow resume {run_id}` shortly"
        );
    }

    // Clear any pause marker so the resumed run isn't immediately re-paused
    // by its own marker poller (see `spawn_pause_marker_poller`).
    if let Err(e) = store.clear_pause_marker(run_id) {
        tracing::warn!(run_id, error = %e, "failed to clear pause marker on resume");
    }

    // Rebuild context from disk: workflow YAML snapshot + prior step
    // results + per-unit fan-out checkpoints.
    let body = store
        .read_workflow_snapshot(run_id)
        .map_err(|e| anyhow::anyhow!("read workflow snapshot: {e}"))?;
    let workflow = Workflow::parse(&body)?;
    let prior_records = store
        .read_step_results(run_id)
        .map_err(|e| anyhow::anyhow!("read step results: {e}"))?;
    let prior_step_results: Vec<rupu_orchestrator::StepResult> = prior_records
        .iter()
        .map(rupu_orchestrator::StepResult::from)
        .collect();

    // Group the unit checkpoints by step_id, keeping only the units
    // that SUCCEEDED. Last write wins per index (a prior partial
    // re-run may have appended a fresh checkpoint for the same index).
    let checkpoints = store
        .read_unit_checkpoints(run_id)
        .map_err(|e| anyhow::anyhow!("read unit checkpoints: {e}"))?;
    let mut completed_units: BTreeMap<String, BTreeMap<usize, rupu_orchestrator::ItemResult>> =
        BTreeMap::new();
    let mut replayed_units = 0usize;
    for cp in &checkpoints {
        let per_step = completed_units.entry(cp.step_id.clone()).or_default();
        if cp.success {
            per_step.insert(
                cp.index,
                rupu_orchestrator::ItemResult {
                    index: cp.index,
                    item: cp.item.clone(),
                    sub_id: String::new(),
                    rendered_prompt: String::new(),
                    run_id: cp.run_id.clone(),
                    transcript_path: cp.transcript_path.clone(),
                    output: cp.output.clone(),
                    success: true,
                },
            );
        } else {
            // A later failure for the same index supersedes an earlier
            // success only if it's the most recent record; the
            // append-order iteration here makes last-write-win natural.
            per_step.remove(&cp.index);
        }
    }
    // Steps that already appear as completed in step_results don't need
    // unit replay (they're skipped wholesale); drop their unit maps so
    // we only carry partially-completed fan-out steps.
    let done_step_ids: std::collections::BTreeSet<String> = prior_step_results
        .iter()
        .map(|s| s.step_id.clone())
        .collect();
    completed_units.retain(|step_id, units| {
        if done_step_ids.contains(step_id) {
            return false;
        }
        replayed_units += units.len();
        !units.is_empty()
    });

    // Restore inputs, event, issue, workspace path from the record.
    let inputs_map: BTreeMap<String, String> = record.inputs.clone();
    let event = record.event.clone();
    let issue_payload = record.issue.clone();
    let issue_ref_text = record.issue_ref.clone();
    let workspace_path = record.workspace_path.clone();
    let transcripts = record.transcript_dir.clone();
    paths::ensure_dir(&transcripts)?;

    // Flip the persisted record back to Running and clear the prior
    // terminal markers so the runner's terminal-flip block at the end
    // updates it coherently.
    record.status = RunStatus::Running;
    record.finished_at = None;
    record.error_message = None;
    record.runner_pid = Some(std::process::id());
    store
        .update(&record)
        .map_err(|e| anyhow::anyhow!("flip run to running: {e}"))?;

    // Resolve project_root from the persisted workspace path so
    // agent/config discovery picks up the same `.rupu/` dir the
    // original run used.
    let project_root = paths::project_root_for(&workspace_path)?;

    // Standard wiring (mirrors `approve` above).
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    let mode_str = mode.unwrap_or("ask").to_string();

    // Hoisted above the dispatcher build (was created a few lines further
    // down, right before `opts`) so `CliAgentDispatcher` can be handed a
    // clone of the same sink and emit `DispatchStarted` / `DispatchCompleted`
    // into the same `events.jsonl` the resumed run's opts carry.
    let event_sink_for_resume = {
        let events_path = global.join("runs").join(run_id).join("events.jsonl");
        match rupu_orchestrator::executor::JsonlSink::create(&events_path) {
            Ok(sink) => Some(Arc::new(sink) as Arc<dyn rupu_orchestrator::executor::EventSink>),
            Err(e) => {
                tracing::warn!(error = %e, "failed to open events.jsonl for resume; continuing without event sink");
                None
            }
        }
    };

    // Hoisted above the dispatcher build: sub-agent dispatch resolves its
    // provider/model through the same config-derived defaults the step
    // factory below uses (ISSUES.md I-8).
    let openai_compatible = rupu_runtime::provider_factory::openai_compatible_map(&cfg.providers);
    let provider_tuning = rupu_runtime::provider_factory::provider_tuning_map(&cfg.providers);
    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        project_root.clone(),
        record.workspace_id.clone(),
        workspace_path.clone(),
        Arc::clone(&resolver),
        mode_str.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&store),
        event_sink_for_resume.clone(),
        cfg.default_provider.clone(),
        cfg.default_model.clone(),
        openai_compatible.clone(),
        provider_tuning.clone(),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    let action_dispatcher = crate::resume::action_dispatcher_for(&mcp_registry, &mode_str);
    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix: None,
        dispatcher: Some(dispatcher_dyn),
        openai_compatible,
        provider_tuning,
        default_provider: cfg.default_provider.clone(),
        default_model: cfg.default_model.clone(),
        bash_timeout_secs: cfg.bash.timeout_secs.unwrap_or(120),
        bash_env_allowlist: cfg.bash.env_allowlist.clone().unwrap_or_default(),
    });

    // A cooperatively-paused run may carry a persisted mid-step seed
    // transcript (written by `run_workflow` when a linear step's agent
    // paused mid-turn — see `RunStore::write_paused_seed`). Read + clear it
    // now, before the resumed run potentially pauses again and writes a
    // fresh one. `None`/empty for a step-boundary pause or a terminal
    // (Failed/Rejected/Cancelled) resume — those replay from
    // `step_results.jsonl` alone, same as today.
    let (reason, paused_step) = if original_status == RunStatus::Paused {
        // Distinguish "no sidecar" (a step-boundary pause — expected empty)
        // from a real read/parse failure of an existing seed. `read_paused_seed`
        // returns an empty `Vec` for a missing file and an `Err` only for an
        // IO/JSON failure; surface the latter loudly rather than silently
        // resuming from scratch and dropping a mid-step transcript.
        let seed = match store.read_paused_seed(run_id) {
            Ok(seed) => seed,
            Err(e) => {
                tracing::warn!(
                    run_id,
                    error = %e,
                    "failed to read persisted paused-step seed; resuming from the step boundary without the mid-step transcript (the paused step will re-run from its prompt)"
                );
                Vec::new()
            }
        };
        if let Err(e) = store.clear_paused_seed(run_id) {
            tracing::warn!(run_id, error = %e, "failed to clear persisted paused-step seed");
        }
        let paused_step = if seed.is_empty() {
            None
        } else {
            record
                .awaiting_step_id
                .clone()
                .map(|step_id| rupu_orchestrator::PausedStep {
                    step_id,
                    seed_messages: seed,
                })
        };
        (rupu_orchestrator::PauseReason::Manual, paused_step)
    } else {
        (rupu_orchestrator::PauseReason::Approval, None)
    };

    let already_done_steps: Vec<String> = done_step_ids.iter().cloned().collect();
    let resume = rupu_orchestrator::ResumeState {
        run_id: run_id.to_string(),
        prior_step_results,
        approved_step_id: String::new(),
        completed_units,
        reason,
        paused_step,
        rejected_reason: None,
        ..Default::default()
    };

    // Clone the workflow for the live view before `opts` consumes it.
    let view_workflow = workflow.clone();

    let unit_dispatcher =
        build_dispatcher_if_needed(&workflow, &global, Arc::clone(&store), cfg.pricing.clone());

    // Same cooperative-pause delivery as a fresh run: hand the resumed
    // (possibly detached) process a token + marker poller so it can be
    // paused again. The marker was cleared above, so the poller starts
    // clean.
    let pause_token = tokio_util::sync::CancellationToken::new();
    let pause_poller =
        spawn_pause_marker_poller(Arc::clone(&store), run_id.to_string(), pause_token.clone());

    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: record.workspace_id.clone(),
        workspace_path,
        transcript_dir: transcripts,
        factory,
        event,
        issue: issue_payload,
        issue_ref: issue_ref_text,
        run_store: Some(store),
        workflow_yaml: Some(body),
        resume_from: Some(resume),
        run_id_override: None,
        strict_templates: false,
        event_sink: event_sink_for_resume,
        unit_dispatcher,
        action_dispatcher: Some(action_dispatcher),
        pause: Some(pause_token.clone()),
    };

    println!("rupu: resuming run {run_id}");
    if already_done_steps.is_empty() {
        println!("      no completed steps to skip; re-running from the start");
    } else {
        println!(
            "      skipping {} already-completed step(s): {}",
            already_done_steps.len(),
            already_done_steps.join(", ")
        );
    }
    if replayed_units > 0 {
        println!(
            "      replaying {replayed_units} already-succeeded fan-out unit(s) from disk; only failed/missing units re-run"
        );
    }

    // Open the same live three-zone view on resume (same gate as `run`).
    // The view seeds step status live from `events.jsonl` going forward:
    // resumed steps emit `StepSkipped`, re-run units emit `UnitStarted` /
    // `UnitCompleted`, so the spine fills in as the run progresses. We do
    // NOT pre-seed prior ✓ from the run-store into LiveRunState here
    // (the events stream re-establishes status as it replays).
    let result = if live_view_enabled(io::stdout().is_terminal(), plain) {
        run_workflow_with_live_view(
            opts,
            view_workflow,
            global.join("runs"),
            run_id.to_string(),
            cfg.pricing.clone(),
        )
        .await?
    } else {
        run_workflow(opts).await?
    };
    // The resumed run has finished (terminal or paused again); stop the
    // marker poller so it doesn't outlive the run.
    pause_poller.abort();
    for sr in &result.step_results {
        if sr.run_id.is_empty() {
            continue;
        }
        println!(
            "rupu: step {} run {} -> {}",
            sr.step_id,
            sr.run_id,
            sr.transcript_path.display()
        );
    }
    match &result.awaiting {
        Some(info) => {
            println!();
            println!(
                "rupu: workflow paused at step `{}` (run {})",
                info.step_id, result.run_id
            );
            println!("      prompt: {}", info.prompt);
            println!(
                "      approve with: rupu workflow approve {}",
                result.run_id
            );
        }
        None => {
            println!(
                "rupu: workflow run {} finished (inspect with: rupu workflow show-run {})",
                result.run_id, result.run_id
            );
        }
    }
    Ok(())
}

/// Outcome of [`resolve_reject_gate`] — phase 1 of `reject` (Task 5b-2a).
#[derive(Debug)]
struct RejectGateOutcome {
    step_id: String,
    reason: String,
    /// spec §3.1's `via` attribution: `"timeout"` when the gate's own
    /// `on_timeout: reject` policy had already fired before this operator
    /// reject landed, `"human"` for a genuine operator decision.
    via: &'static str,
    /// I-36: the rejecting operator's identity, threaded into
    /// `run_reject_cleanup`'s `approver` argument.
    approver: String,
}

/// Phase 1 of `reject` (Task 5b-2a): the `via`-attribution pre-check +
/// `store.reject_gate` call + friendly error mapping, split out of
/// `reject` itself so it's unit-testable without the async on_reject
/// cleanup wiring. `gate: None` is the legacy sole-gate case; `gate:
/// Some(id)` targets that gate, leaving any other parked gate untouched.
fn resolve_reject_gate(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
    reason: Option<&str>,
    gate: Option<&str>,
) -> anyhow::Result<RejectGateOutcome> {
    let approver = whoami::username();
    let reason_str = reason.unwrap_or("rejected by operator");

    // `store.reject_gate()` treats a gate that already timed out with
    // `on_timeout: reject` identically to an explicit operator reject
    // (both return `Ok(ApprovalDecision::Rejected { .. })`) — it has no
    // way to tell this caller which one actually happened. Detect it
    // here, before the library call, purely to attribute the gate
    // output's `via` correctly (spec §3.1): "timeout" when the policy
    // already fired, "human" for a genuine operator decision. Task
    // 5b-2a: check the TARGETED gate's own overdue-ness/policy (falling
    // back to the compat-mirrored first gate when `--gate` is omitted).
    let via = if let Ok(record) = store.load(run_id) {
        let target_step_id = gate
            .map(str::to_string)
            .or_else(|| record.awaiting_step_id.clone());
        match target_step_id {
            Some(step_id) => {
                let gate_expires_at = record
                    .awaiting_gates()
                    .into_iter()
                    .find(|g| g.step_id == step_id)
                    .and_then(|g| g.expires_at);
                let overdue = record.status == rupu_orchestrator::RunStatus::AwaitingApproval
                    && gate_expires_at.is_some_and(|exp| chrono::Utc::now() > exp);
                if overdue
                    && gate_on_timeout_for_step(store, &record, &step_id)
                        == Some(rupu_orchestrator::TimeoutAction::Reject)
                {
                    println!(
                        "rupu: gate `{step_id}` timed out with on_timeout: reject — \
                         already auto-rejected"
                    );
                    "timeout"
                } else {
                    "human"
                }
            }
            None => "human",
        }
    } else {
        "human"
    };

    // Library call replaces inline load + expire-check + status check +
    // mutate + update.
    match store.reject_gate(run_id, &approver, reason_str, chrono::Utc::now(), gate) {
        Ok(rupu_orchestrator::ApprovalDecision::Rejected {
            step_id, reason, ..
        }) => Ok(RejectGateOutcome {
            step_id,
            reason,
            via,
            approver,
        }),
        Err(rupu_orchestrator::ApprovalError::Expired(msg)) => {
            anyhow::bail!("approval expired before it was acted on — {msg}");
        }
        Err(rupu_orchestrator::ApprovalError::NotAwaiting(s)) => {
            anyhow::bail!(
                "run is `{s}`, not `awaiting_approval` — only paused runs can be rejected",
            );
        }
        Err(rupu_orchestrator::ApprovalError::NotFound(id)) => {
            anyhow::bail!(
                "run not found: {id}\n  hint: \
                 list paused runs with `rupu workflow runs --status awaiting_approval`"
            );
        }
        Err(rupu_orchestrator::ApprovalError::AmbiguousGate { run_id, candidates }) => {
            anyhow::bail!(ambiguous_gate_message("reject", &run_id, &candidates));
        }
        Err(rupu_orchestrator::ApprovalError::GateNotFound { run_id, step_id }) => {
            anyhow::bail!(
                "gate `{step_id}` is not awaiting approval on run `{run_id}` — check \
                 `rupu workflow show-run {run_id}` for the currently parked gate ids"
            );
        }
        Err(e) => Err(anyhow::anyhow!("reject: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    }
}

async fn reject(run_id: &str, reason: Option<&str>, gate: Option<&str>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();

    let RejectGateOutcome {
        step_id: rejected_step_id,
        reason: rejected_reason,
        via,
        approver,
    } = resolve_reject_gate(&store, run_id, reason, gate)?;
    // Task 5b-2a: rejecting one gate of a still-parked multi-gate set
    // leaves the run `AwaitingApproval` (the other gates stay parked) —
    // "marked rejected" only holds once the set is empty and the run
    // itself flipped terminal.
    match store.load(run_id) {
        Ok(rec) if rec.status == rupu_orchestrator::RunStatus::AwaitingApproval => {
            println!(
                "rupu: gate `{rejected_step_id}` rejected on run {run_id}; {} other gate(s) \
                 still awaiting approval",
                rec.awaiting.len()
            );
        }
        _ => println!("rupu: run {run_id} marked rejected"),
    }

    // The run is already correctly `Rejected` at this point (the library
    // call above is what finalized it) — a cleanup-load failure is warned,
    // never turned into a command error.
    //
    // I-36: run this unconditionally, not only when
    // `cheap_on_reject_chain_len` reports a non-empty `on_reject:` chain —
    // `run_reject_cleanup` is the ONLY caller of `emit_gate_result`, so an
    // empty chain must still reach it to record the gate's rejected
    // decision (reason + actor). `on_reject` chains being optional is not
    // a reason to skip recording that the gate WAS rejected.
    match crate::resume::build_reject_cleanup_opts(
        &store,
        run_id,
        &rejected_step_id,
        &rejected_reason,
        None,
    )
    .await
    {
        Ok((opts, chain_len)) => {
            match rupu_orchestrator::runner::run_reject_cleanup(
                opts,
                &rejected_step_id,
                &rejected_reason,
                via,
                Some(&approver),
            )
            .await
            {
                Ok(()) => {
                    if chain_len > 0 {
                        println!("cleanup: {chain_len} step(s) executed");
                    }
                    // I-35: the chain just ran synchronously — clear
                    // the pending-cleanup marker `reject_gate` may
                    // have set so the `cp serve` gate sweep doesn't
                    // run it again on its next tick. Best-effort: a
                    // clear failure is warned, not fatal (the run is
                    // already correctly rejected either way).
                    if let Err(e) = store.clear_reject_cleanup(run_id) {
                        eprintln!("warning: could not clear on_reject cleanup marker: {e}");
                    }
                }
                Err(e) => eprintln!("warning: on_reject cleanup chain errored: {e}"),
            }
        }
        Err(e) => {
            eprintln!(
                "warning: could not load workflow for on_reject cleanup: {e} \
                 (run is already correctly rejected)"
            );
        }
    }
    Ok(())
}

async fn cancel(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();
    let outcome = cancel_with_store(&store, run_id, "cancelled by operator")?;
    match outcome {
        CancelOutcome::RejectedAwaitingApproval => {
            println!("rupu: cancelled paused run {run_id}");
        }
        CancelOutcome::MarkedCancelled { pid, was_running } => match (pid, was_running) {
            (Some(pid), true) => {
                println!("rupu: cancelled run {run_id} (sent TERM to pid {pid})");
            }
            (Some(pid), false) => {
                println!("rupu: marked run {run_id} cancelled (pid {pid} was not running)");
            }
            (None, _) => {
                println!("rupu: marked run {run_id} cancelled");
            }
        },
    }
    Ok(())
}

/// Delegates to the canonical [`rupu_orchestrator::RunStore::cancel`]
/// (Pending/Running → `Cancelled` with a SIGTERM to a live runner pid;
/// AwaitingApproval → reject; terminal → error). Maps the library's
/// [`CancelError`] onto an `anyhow::Error` with the same user-facing
/// message shape the CLI printed before.
fn cancel_with_store(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
    reason: &str,
) -> anyhow::Result<CancelOutcome> {
    store
        .cancel(run_id, &whoami::username(), reason, chrono::Utc::now())
        .map_err(|e| match e {
            CancelError::AlreadyTerminal(status) => {
                anyhow::anyhow!("run {run_id} is already terminal ({})", status.as_str())
            }
            CancelError::NotFound(_) => anyhow::anyhow!("load run record: {e}"),
            CancelError::Store(_) => anyhow::anyhow!("cancel run: {e}"),
        })
}

/// Cooperatively pause a `Pending`/`Running` run: flips the persisted record
/// to `Paused` ([`rupu_orchestrator::RunStore::pause`]) then writes the
/// pause marker ([`rupu_orchestrator::RunStore::set_pause_marker`]) that a
/// *detached* `rupu workflow run <id>` subprocess polls (~every 250ms) to
/// cooperatively stop at its next safe boundary.
///
/// This is the exact mechanism `LocalHostConnector::pause_run` uses
/// in-process; exposing it as a bare CLI command gives the SSH transport a
/// one-shot remote command to reach it (mirrors how `rupu workflow cancel`
/// gives `cancel_run` its remote reach) — see
/// `docs/superpowers/plans/2026-07-01-rupu-pause-resume-plan.md` Task 5.
///
/// `pub(crate)` so `rupu run pause <run_id>` (Task 7,
/// `crate::cmd::run::RunCommand::Pause`) can delegate here directly —
/// same primitive, same user-facing message, no parallel implementation.
pub(crate) async fn pause(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();
    pause_with_store(&store, run_id)?;
    println!(
        "rupu: pause requested for run {run_id} (resume with `rupu workflow resume {run_id}`)"
    );
    Ok(())
}

/// Delegates to [`rupu_orchestrator::RunStore::pause`] (Pending/Running →
/// `Paused`; already-paused/awaiting-approval → `PauseError::NotRunning`;
/// terminal → `PauseError::AlreadyTerminal`), then writes the pause marker
/// so a detached runner process cooperatively stops at its next safe
/// boundary. Maps the library's [`PauseError`] onto an `anyhow::Error` with
/// a stable user-facing message shape (mirrors `cancel_with_store`).
///
/// `pub(crate)` so the live-run view's Esc handler
/// ([`crate::output::live_run`], Task 7) can request a pause for the run
/// it is tailing using the exact same primitive, without going through
/// `pause`'s stdout `println!` (which would corrupt the alt-screen).
pub(crate) fn pause_with_store(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now();
    store.pause(run_id, now).map_err(|e| match e {
        PauseError::AlreadyTerminal(status) => {
            anyhow::anyhow!("run {run_id} is already terminal ({})", status.as_str())
        }
        PauseError::NotRunning(status) => {
            anyhow::anyhow!(
                "run {run_id} is `{}` — only a running run can be paused",
                status.as_str()
            )
        }
        PauseError::NotFound(_) => anyhow::anyhow!("load run record: {e}"),
        PauseError::Store(_) => anyhow::anyhow!("pause run: {e}"),
    })?;
    // Deliver the pause to the detached runner process via the marker.
    store
        .set_pause_marker(run_id)
        .map_err(|e| anyhow::anyhow!("set pause marker: {e}"))
}

async fn archive_run(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();
    store.archive(run_id)?;
    println!("archived run {run_id}");
    Ok(())
}

async fn restore_run(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();
    store.restore(run_id)?;
    println!("restored run {run_id}");
    Ok(())
}

async fn delete_run(run_id: &str, force: bool) -> anyhow::Result<()> {
    if !force {
        anyhow::bail!("run delete requires --force");
    }
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let run_id = resolve_run_fragment(&store, run_id)?;
    let run_id = run_id.as_str();
    delete_run_with_store(&store, run_id)?;
    println!("deleted run {run_id}");
    Ok(())
}

/// Shared guard+delete for `rupu workflow delete-run`: refuses a non-terminal
/// run even under `--force` — `--force` only skips the "did you mean it"
/// confirmation this command otherwise requires (see the bail above); it was
/// never meant to authorize deleting a live run's directory out from under
/// the process still writing to it. Mirrors
/// `rupu_cp::api::runs::delete_run_checked`'s guard exactly, so this CLI verb
/// and the CP's local branch never diverge — and, by extension, neither does
/// the SSH `HostConnector::delete_run` path, which shells this exact command
/// one hop removed (`crates/rupu-cp/src/host/ssh.rs`'s `delete_run`).
///
/// `pub(crate)` so tests can drive it against a temp `RunStore` directly,
/// the same shape as `cancel_with_store`/`pause_with_store` above.
pub(crate) fn delete_run_with_store(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
) -> anyhow::Result<()> {
    if let Ok(rec) = store.load(run_id) {
        if !rec.status.is_terminal() {
            anyhow::bail!(
                "run {run_id} is not terminal ({}) — cancel it first",
                rec.status.as_str()
            );
        }
    }
    store.delete(run_id)?;
    Ok(())
}

pub(crate) fn locate_workflow_in(
    global: &Path,
    project_root: Option<&Path>,
    name: &str,
) -> anyhow::Result<PathBuf> {
    if let Some(project_root) = project_root {
        let candidate = project_root
            .join(".rupu/workflows")
            .join(format!("{name}.yaml"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let candidate = global.join("workflows").join(format!("{name}.yaml"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(anyhow::anyhow!("workflow not found: {name}"))
}

fn locate_workflow(name: &str) -> anyhow::Result<PathBuf> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global = paths::global_dir()?;
    locate_workflow_in(&global, project_root.as_deref(), name)
}

/// Lightweight outcome surface for [`run_by_name`] callers (the
/// webhook receiver in particular) that need to know the run-id and
/// whether the run paused at an approval gate. The full per-step
/// result list is intentionally excluded — it's heavy and the
/// callers can fetch it via the run-store if they care.
#[derive(Debug, Clone, Default)]
pub struct RunOutcomeSummary {
    pub run_id: String,
    pub awaiting_step_id: Option<String>,
    pub artifact_manifest_path: Option<PathBuf>,
    pub backend_id: Option<String>,
    pub worker_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecutionWorkerContext {
    pub worker_id: String,
    pub kind: WorkerKind,
    pub name: String,
}

#[derive(Clone)]
pub struct ExplicitWorkflowRunContext {
    pub project_root: Option<PathBuf>,
    pub workspace_path: PathBuf,
    pub workspace_id: String,
    pub inputs: Vec<(String, String)>,
    pub mode: String,
    pub invocation_source: RunTriggerSource,
    pub event: Option<serde_json::Value>,
    pub issue: Option<serde_json::Value>,
    pub issue_ref: Option<String>,
    pub system_prompt_suffix: Option<String>,
    pub attach_ui: bool,
    pub run_id_override: Option<String>,
    pub strict_templates: bool,
    pub run_envelope_template: Option<RunEnvelopeTemplate>,
    pub worker: Option<ExecutionWorkerContext>,
    pub live_event_hook: Option<crate::output::workflow_printer::LiveWorkflowEventHook>,
    pub shared_printer: Option<Arc<Mutex<crate::output::LineStreamPrinter>>>,
    pub live_view: LiveViewMode,
    /// Force the plain line printer (the `--plain` opt-out). When the
    /// run is non-interactive this is irrelevant (the line printer is
    /// always used); on a tty it disables the default live graph view.
    pub plain: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RunEnvelopeTemplate {
    pub repo_ref: Option<String>,
    pub wake_id: Option<String>,
    pub event_id: Option<String>,
    pub backend: Option<String>,
    pub workspace_strategy: Option<String>,
    pub autoflow_name: Option<String>,
    pub autoflow_claim_id: Option<String>,
    pub autoflow_priority: Option<i32>,
    pub requested_worker: Option<String>,
    pub target: Option<String>,
    pub correlation: Option<RunCorrelation>,
}

struct LocalWorktreeBackend;

impl ExecutionBackend for LocalWorktreeBackend {
    fn id(&self) -> &'static str {
        "local_worktree"
    }

    fn can_execute(&self, envelope: &RunEnvelope) -> bool {
        matches!(
            envelope.execution.backend.as_deref(),
            None | Some("local_worktree") | Some("local_checkout")
        )
    }
}

/// Public wrapper around the workflow-run pipeline so other
/// subcommands (notably `rupu cron tick` and the webhook receiver)
/// can invoke a workflow by name without going through the clap
/// layer. Same behavior as `rupu workflow run <name>`. The optional
/// `event` argument carries the SCM-vendor JSON payload that
/// triggered the run (when applicable); it lands as `{{event.*}}`
/// bindings in step prompts and `when:` filters.
pub async fn run_by_name(
    name: &str,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
) -> anyhow::Result<RunOutcomeSummary> {
    run_with_outcome(name, None, inputs, mode, event, false, None, None, false).await
}

/// Variant of [`run_by_name`] that pins the run-id. Used by the
/// `rupu cron tick` polled-events tier, which derives a deterministic
/// id (`evt-<workflow>-<vendor>-<delivery>`) so re-delivered or
/// re-polled events don't double-fire. On collision, the underlying
/// `RunStore::create` returns `AlreadyExists`; this wrapper surfaces
/// that as `Err(...)` and the caller logs + skips.
pub async fn run_by_name_with_run_id(
    name: &str,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    run_id: String,
) -> anyhow::Result<RunOutcomeSummary> {
    run_with_outcome(
        name,
        None,
        inputs,
        mode,
        event,
        false,
        Some(run_id),
        None,
        false,
    )
    .await
}

/// Run a specific workflow file using the same execution pipeline as
/// `rupu workflow run`, but with an explicit workflow path and
/// workspace context. Used by repo-aware webhook dispatch, where the
/// candidate workflow may live in a tracked checkout outside the
/// server's current working directory.
pub async fn run_by_path(
    workflow_path: PathBuf,
    project_root: Option<PathBuf>,
    workspace_path: PathBuf,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
) -> anyhow::Result<RunOutcomeSummary> {
    run_path_with_outcome(
        workflow_path,
        project_root,
        workspace_path,
        inputs,
        mode,
        event,
        false,
        None,
        None,
    )
    .await
}

/// Public wrapper for `rupu issues run <name> <ref>` and similar
/// callers that need to invoke a workflow with a specific
/// run-target string. Same UI semantics as `rupu workflow run`
/// (interactive line-stream by default) so the issue-targeted run
/// looks identical to the user.
pub async fn run_by_target(name: &str, target: &str, mode: Option<&str>) -> anyhow::Result<()> {
    run(
        name,
        Some(target),
        Vec::new(),
        mode,
        None,
        None,
        false,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    view: Option<LiveViewMode>,
    plain: bool,
    run_id: Option<String>,
) -> anyhow::Result<()> {
    run_with_outcome(name, target, inputs, mode, event, true, run_id, view, plain)
        .await
        .map(|_| ())
}

/// Same as [`run`] but returns a [`RunOutcomeSummary`] so non-CLI
/// callers (the webhook receiver) can surface run-id + pause state.
/// `run` itself thin-wraps this and discards the value.
#[allow(clippy::too_many_arguments)]
async fn run_with_outcome(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    attach_ui: bool,
    run_id_override: Option<String>,
    view: Option<LiveViewMode>,
    plain: bool,
) -> anyhow::Result<RunOutcomeSummary> {
    let path = locate_workflow(name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;

    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Workspace upsert (mirrors `rupu run`).
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &pwd)?;
    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &pwd) {
        warn!(path = %pwd.display(), error = %err, "failed to auto-track checkout");
    }

    // Credential resolver (shared across all steps in this workflow run).
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());

    // Resolve config (global + project) so Registry::discover can read
    // [scm] platform settings.
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let live_view = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None, view).live_view;

    // Build the SCM/issue registry once for the entire workflow run.
    // Cheap when no platforms are configured; missing credentials are
    // skipped with INFO logs.
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    // Parse the workflow-level target (if any) and derive a system-prompt
    // suffix that each step prepends. Clone-to-tmpdir for Repo/Pr targets
    // follows the same pattern as `rupu run`; the tmpdir lives for the
    // entire workflow execution.
    let _clone_guard: Option<tempfile::TempDir>;
    let workspace_path: std::path::PathBuf;
    let system_prompt_suffix: Option<String>;
    // Issue context — populated when run-target resolves to an issue.
    // The orchestrator's StepContext binds this as `{{issue.*}}` in
    // step prompts + `when:` expressions; RunRecord persists the
    // textual ref so `rupu workflow runs --issue <ref>` can filter.
    let mut issue_payload: Option<serde_json::Value> = None;
    let mut issue_ref_text: Option<String> = None;
    match target {
        None => {
            _clone_guard = None;
            workspace_path = pwd.clone();
            system_prompt_suffix = None;
        }
        Some(s) => match crate::run_target::parse_run_target(s) {
            Err(_) => {
                // Not a valid target — ignore silently (workflow inputs
                // don't have a free-form prompt field to absorb it).
                _clone_guard = None;
                workspace_path = pwd.clone();
                system_prompt_suffix = None;
            }
            Ok(run_target) => {
                let suffix = Some(crate::run_target::format_run_target_for_prompt(&run_target));
                let (guard, path) = match &run_target {
                    crate::run_target::RunTarget::Repo {
                        platform,
                        owner,
                        repo,
                        ..
                    }
                    | crate::run_target::RunTarget::Pr {
                        platform,
                        owner,
                        repo,
                        ..
                    } => {
                        let r = rupu_scm::RepoRef {
                            platform: *platform,
                            owner: owner.clone(),
                            repo: repo.clone(),
                        };
                        let tmp = tempfile::tempdir()?;
                        rupu_scm::clone_repo_ref(&mcp_registry, &r, tmp.path())
                            .await
                            .map_err(|e| anyhow::anyhow!("{e}"))?;
                        let p = tmp.path().to_path_buf();
                        (Some(tmp), p)
                    }
                    crate::run_target::RunTarget::Issue {
                        tracker,
                        project,
                        number,
                    } => {
                        // Pre-fetch the issue once at run-start so step
                        // prompts can reference `{{issue.title}}` /
                        // `{{issue.body}}` / `{{issue.labels}}` etc.
                        // without each step having to call the
                        // IssueConnector.
                        let i = rupu_scm::IssueRef {
                            tracker: *tracker,
                            project: project.clone(),
                            number: *number,
                        };
                        let conn = mcp_registry.issues(*tracker).ok_or_else(|| {
                            anyhow::anyhow!(
                                "no {} credential — run `rupu auth login --provider {}`",
                                tracker,
                                tracker
                            )
                        })?;
                        match conn.get_issue(&i).await {
                            Ok(issue) => {
                                issue_payload = serde_json::to_value(&issue).ok();
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "failed to fetch issue at run-start; {{issue.*}} will be empty"
                                );
                            }
                        }
                        issue_ref_text = Some(format!("{}:{}/issues/{}", tracker, project, number));
                        (None, pwd.clone())
                    }
                };
                _clone_guard = guard;
                workspace_path = path;
                system_prompt_suffix = suffix;
            }
        },
    }

    let invocation_source = if target.is_some() {
        RunTriggerSource::IssueCommand
    } else if run_id_override.is_some() && event.is_some() {
        RunTriggerSource::CronEvent
    } else if event.is_some() {
        RunTriggerSource::EventDispatch
    } else {
        RunTriggerSource::WorkflowCli
    };

    execute_workflow_invocation(
        name,
        workflow,
        body,
        path,
        global,
        ExplicitWorkflowRunContext {
            project_root: project_root.clone(),
            workspace_path,
            workspace_id: ws.id,
            inputs,
            mode: {
                warn_if_ask_mode_is_effectively_bypass(mode, mode.unwrap_or("ask"));
                mode.unwrap_or("ask").to_string()
            },
            invocation_source,
            event,
            issue: issue_payload,
            issue_ref: issue_ref_text,
            system_prompt_suffix,
            attach_ui,
            run_id_override,
            strict_templates: false,
            run_envelope_template: None,
            worker: None,
            live_event_hook: None,
            shared_printer: None,
            live_view,
            plain,
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_path_with_outcome(
    workflow_path: PathBuf,
    project_root: Option<PathBuf>,
    workspace_path: PathBuf,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    attach_ui: bool,
    run_id_override: Option<String>,
    view: Option<LiveViewMode>,
) -> anyhow::Result<RunOutcomeSummary> {
    let body = std::fs::read_to_string(&workflow_path)?;
    let workflow = Workflow::parse(&body)?;
    let workflow_name = workflow.name.clone();

    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &workspace_path)?;
    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &workspace_path) {
        warn!(
            path = %workspace_path.display(),
            error = %err,
            "failed to auto-track checkout"
        );
    }

    let invocation_source = if event.is_some() {
        RunTriggerSource::EventDispatch
    } else {
        RunTriggerSource::WorkflowCli
    };

    execute_workflow_invocation(
        &workflow_name,
        workflow,
        body,
        workflow_path,
        global,
        ExplicitWorkflowRunContext {
            project_root,
            workspace_path,
            workspace_id: ws.id,
            inputs,
            mode: {
                warn_if_ask_mode_is_effectively_bypass(mode, mode.unwrap_or("ask"));
                mode.unwrap_or("ask").to_string()
            },
            invocation_source,
            event,
            issue: None,
            issue_ref: None,
            system_prompt_suffix: None,
            attach_ui,
            run_id_override,
            strict_templates: false,
            run_envelope_template: None,
            worker: None,
            live_event_hook: None,
            shared_printer: None,
            live_view: view.unwrap_or(LiveViewMode::Focused),
            plain: false,
        },
    )
    .await
}

pub async fn run_with_explicit_context(
    name: &str,
    ctx: ExplicitWorkflowRunContext,
) -> anyhow::Result<RunOutcomeSummary> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let path = locate_workflow_in(&global, ctx.project_root.as_deref(), name)?;
    let body = std::fs::read_to_string(&path)?;
    let workflow = Workflow::parse(&body)?;
    execute_workflow_invocation(name, workflow, body, path, global, ctx).await
}

fn build_run_envelope(
    run_id: String,
    workflow: &Workflow,
    workflow_body: &str,
    workflow_path: &Path,
    ctx: &ExplicitWorkflowRunContext,
    worker: &ExecutionWorkerContext,
) -> RunEnvelope {
    let template = ctx.run_envelope_template.clone().unwrap_or_default();
    let repo_ref = template.repo_ref.clone().or_else(|| {
        ctx.project_root
            .as_deref()
            .or(Some(ctx.workspace_path.as_path()))
            .and_then(|path| crate::cmd::issues::autodetect_repo_from_path(path).ok())
            .map(|repo| crate::cmd::issues::canonical_repo_ref(&repo))
    });
    let issue_ref = ctx.issue_ref.clone();
    let target = template.target.clone().or_else(|| issue_ref.clone());

    RunEnvelope {
        version: RunEnvelope::VERSION,
        run_id,
        kind: RunKind::WorkflowRun,
        workflow: WorkflowBinding {
            name: workflow.name.clone(),
            source_path: workflow_path.to_path_buf(),
            fingerprint: workflow_fingerprint(workflow_body),
        },
        repo: Some(RepoBinding {
            repo_ref,
            project_root: ctx.project_root.clone(),
            workspace_id: ctx.workspace_id.clone(),
            workspace_path: ctx.workspace_path.clone(),
        }),
        trigger: RunTrigger {
            source: ctx.invocation_source.clone(),
            wake_id: template.wake_id,
            event_id: template.event_id,
        },
        inputs: ctx.inputs.iter().cloned().collect(),
        context: Some(RunContext {
            issue_ref,
            target,
            event_present: ctx.event.is_some(),
            issue_present: ctx.issue.is_some(),
        }),
        execution: ExecutionRequest {
            backend: Some(
                template
                    .backend
                    .unwrap_or_else(|| "local_worktree".to_string()),
            ),
            permission_mode: ctx.mode.clone(),
            workspace_strategy: template.workspace_strategy,
            strict_templates: ctx.strict_templates,
            attach_ui: ctx.attach_ui,
        },
        autoflow: template.autoflow_name.map(|name| AutoflowEnvelope {
            name,
            claim_id: template.autoflow_claim_id,
            priority: template.autoflow_priority.unwrap_or_default(),
        }),
        correlation: template.correlation,
        worker: Some(WorkerRequest {
            requested_worker: template.requested_worker,
            assigned_worker_id: Some(worker.worker_id.clone()),
        }),
    }
}

fn workflow_fingerprint(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

pub(crate) fn local_host_name() -> String {
    whoami::fallible::hostname().unwrap_or_else(|_| "unknown-host".to_string())
}

pub(crate) fn sanitize_worker_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) fn default_execution_worker_context(
    kind: WorkerKind,
    name_override: Option<&str>,
) -> ExecutionWorkerContext {
    let host = local_host_name();
    let display_name = name_override
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("{}@{}", whoami::username(), host));
    let suffix = match kind {
        WorkerKind::Cli => "cli",
        WorkerKind::AutoflowServe => "serve",
    };
    let worker_id = format!(
        "worker_local_{}_{}",
        sanitize_worker_component(&display_name),
        suffix
    );
    ExecutionWorkerContext {
        worker_id,
        kind,
        name: display_name,
    }
}

fn repo_host_from_ref(repo_ref: Option<&str>) -> Option<String> {
    repo_ref
        .and_then(|value| value.split(':').next())
        .map(ToOwned::to_owned)
}

pub(crate) fn upsert_worker_record(
    global: &Path,
    worker: &ExecutionWorkerContext,
    backend_id: &str,
    permission_mode: &str,
    repo_ref: Option<&str>,
) -> anyhow::Result<WorkerRecord> {
    let store = WorkerStore {
        root: paths::autoflow_workers_dir(global),
    };
    let now = chrono::Utc::now().to_rfc3339();
    let existing = store
        .load(&worker.worker_id)
        .map_err(|e| anyhow::anyhow!("load worker record: {e}"))?;
    let registered_at = existing
        .as_ref()
        .map(|record| record.registered_at.clone())
        .unwrap_or_else(|| now.clone());
    let mut capabilities = existing
        .map(|record| record.capabilities)
        .unwrap_or_default();
    if !capabilities
        .backends
        .iter()
        .any(|value| value == backend_id)
    {
        capabilities.backends.push(backend_id.to_string());
        capabilities.backends.sort();
    }
    if !capabilities
        .permission_modes
        .iter()
        .any(|value| value == permission_mode)
    {
        capabilities
            .permission_modes
            .push(permission_mode.to_string());
        capabilities.permission_modes.sort();
    }
    if let Some(host) = repo_host_from_ref(repo_ref) {
        if !capabilities.scm_hosts.iter().any(|value| value == &host) {
            capabilities.scm_hosts.push(host);
            capabilities.scm_hosts.sort();
        }
    }
    let record = WorkerRecord {
        version: WorkerRecord::VERSION,
        worker_id: worker.worker_id.clone(),
        kind: worker.kind,
        name: worker.name.clone(),
        host: local_host_name(),
        capabilities,
        registered_at,
        last_seen_at: now,
    };
    store
        .save(&record)
        .map_err(|e| anyhow::anyhow!("save worker record: {e}"))?;
    Ok(record)
}

fn prepare_local_run(envelope: &RunEnvelope, worker_id: &str) -> anyhow::Result<PreparedRun> {
    let backend = LocalWorktreeBackend;
    if !backend.can_execute(envelope) {
        let backend_id = envelope
            .execution
            .backend
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        return Err(anyhow::anyhow!(
            "unsupported execution backend `{backend_id}` for local workflow invocation"
        ));
    }
    let repo = envelope.repo.as_ref().ok_or_else(|| {
        anyhow::anyhow!("run envelope is missing repo binding for local workflow invocation")
    })?;
    Ok(PreparedRun {
        version: PreparedRun::VERSION,
        run_id: envelope.run_id.clone(),
        backend_id: envelope
            .execution
            .backend
            .clone()
            .unwrap_or_else(|| backend.id().to_string()),
        workspace_path: repo.workspace_path.clone(),
        project_root: repo.project_root.clone(),
        repo_ref: repo.repo_ref.clone(),
        issue_ref: envelope
            .context
            .as_ref()
            .and_then(|ctx| ctx.issue_ref.clone()),
        workspace_strategy: envelope.execution.workspace_strategy.clone(),
        worker_id: Some(worker_id.to_string()),
    })
}

fn build_artifact_manifest(
    run_store: &rupu_orchestrator::RunStore,
    run: &rupu_orchestrator::RunRecord,
    prepared: &PreparedRun,
) -> anyhow::Result<ArtifactManifest> {
    let mut manifest = ArtifactManifest::new(run.id.clone(), prepared.backend_id.clone());
    manifest.worker_id = prepared.worker_id.clone();
    manifest.artifacts.push(ArtifactRef {
        id: "art_run_record".into(),
        kind: ArtifactKind::RunRecord,
        name: "run-record".into(),
        producer: "run".into(),
        local_path: Some(run_store.run_json_path(&run.id)),
        uri: None,
        inline_json: None,
    });
    manifest.artifacts.push(ArtifactRef {
        id: "art_run_envelope".into(),
        kind: ArtifactKind::RunEnvelope,
        name: "run-envelope".into(),
        producer: "run".into(),
        local_path: Some(run_store.run_envelope_path(&run.id)),
        uri: None,
        inline_json: None,
    });
    manifest.artifacts.push(ArtifactRef {
        id: "art_workflow_snapshot".into(),
        kind: ArtifactKind::WorkflowSnapshot,
        name: "workflow-snapshot".into(),
        producer: "run".into(),
        local_path: Some(run_store.workflow_snapshot_path(&run.id)),
        uri: None,
        inline_json: None,
    });
    for step in run_store.read_step_results(&run.id)? {
        manifest.artifacts.push(ArtifactRef {
            id: format!(
                "art_step_{}_transcript",
                sanitize_worker_component(&step.step_id)
            ),
            kind: ArtifactKind::StepTranscript,
            name: format!("{} transcript", step.step_id),
            producer: format!("step.{}", step.step_id),
            local_path: Some(step.transcript_path.clone()),
            uri: None,
            inline_json: None,
        });
    }
    manifest.artifacts.push(ArtifactRef {
        id: "art_run_summary".into(),
        kind: ArtifactKind::Summary,
        name: "run-summary".into(),
        producer: "run".into(),
        local_path: None,
        uri: None,
        inline_json: Some(serde_json::json!({
            "status": run.status.as_str(),
            "awaiting_step_id": run.awaiting_step_id,
            "error_message": run.error_message,
            "issue_ref": run.issue_ref,
            "workspace_id": run.workspace_id,
        })),
    });
    Ok(manifest)
}

fn run_result_status(status: rupu_orchestrator::RunStatus) -> RunResultStatus {
    match status {
        rupu_orchestrator::RunStatus::AwaitingApproval => RunResultStatus::AwaitingApproval,
        rupu_orchestrator::RunStatus::Failed
        | rupu_orchestrator::RunStatus::Rejected
        | rupu_orchestrator::RunStatus::Cancelled => RunResultStatus::Failed,
        rupu_orchestrator::RunStatus::Pending
        | rupu_orchestrator::RunStatus::Running
        | rupu_orchestrator::RunStatus::Paused
        | rupu_orchestrator::RunStatus::Completed => RunResultStatus::Completed,
    }
}

fn persist_portable_run_metadata(
    run_store: &rupu_orchestrator::RunStore,
    prepared: &PreparedRun,
    source_wake_id: Option<&str>,
) -> anyhow::Result<Option<(PathBuf, RunResult)>> {
    let Ok(mut run) = run_store.load(&prepared.run_id) else {
        return Ok(None);
    };
    run.backend_id = Some(prepared.backend_id.clone());
    run.worker_id = prepared.worker_id.clone();
    run.source_wake_id = source_wake_id.map(ToOwned::to_owned);

    let manifest = build_artifact_manifest(run_store, &run, prepared)?;
    let manifest_path = run_store.write_artifact_manifest(&prepared.run_id, &manifest)?;
    run.artifact_manifest_path = Some(manifest_path.clone());
    run_store.update(&run)?;

    let result = RunResult {
        version: RunResult::VERSION,
        run_id: run.id.clone(),
        backend_id: prepared.backend_id.clone(),
        status: run_result_status(run.status),
        worker_id: prepared.worker_id.clone(),
        source_wake_id: source_wake_id.map(ToOwned::to_owned),
        artifact_manifest: Some(manifest),
    };
    Ok(Some((manifest_path, result)))
}

async fn execute_workflow_invocation(
    name: &str,
    workflow: Workflow,
    body: String,
    workflow_path: PathBuf,
    global: PathBuf,
    ctx: ExplicitWorkflowRunContext,
) -> anyhow::Result<RunOutcomeSummary> {
    // Borrow alias used by the input-snippet renderer at the
    // `run_workflow` call sites below. `body` is consumed by `opts`
    // (cloned) so we keep the `Path` and `&str` references local.
    let path = workflow_path;
    let run_id = ctx
        .run_id_override
        .clone()
        .unwrap_or_else(|| format!("run_{}", Ulid::new()));
    let worker_ctx = ctx
        .worker
        .clone()
        .unwrap_or_else(|| default_execution_worker_context(WorkerKind::Cli, None));
    let run_envelope =
        build_run_envelope(run_id.clone(), &workflow, &body, &path, &ctx, &worker_ctx);
    let backend_id = run_envelope
        .execution
        .backend
        .clone()
        .unwrap_or_else(|| "local_worktree".to_string());
    let worker_record = upsert_worker_record(
        &global,
        &worker_ctx,
        &backend_id,
        &ctx.mode,
        run_envelope
            .repo
            .as_ref()
            .and_then(|repo| repo.repo_ref.as_deref()),
    )?;
    let prepared_run = prepare_local_run(&run_envelope, &worker_record.worker_id)?;
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = ctx
        .project_root
        .as_ref()
        .map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    let transcripts = paths::transcripts_dir(&global, ctx.project_root.as_deref());
    paths::ensure_dir(&transcripts)?;
    let transcripts_dir_snapshot = transcripts.clone();

    let registry_for_notify = Arc::clone(&mcp_registry);
    let notify_issue_enabled = workflow.notify_issue;
    let workflow_name_for_notify = workflow.name.clone();
    let issue_ref_text_for_notify = ctx.issue_ref.clone();
    let issue_payload_for_notify = ctx.issue.clone();

    // Run-store first so the dispatcher can be constructed alongside the
    // factory and threaded onto every step's `ToolContext`.
    let inputs_map: BTreeMap<String, String> = ctx.inputs.into_iter().collect();
    let runs_dir = global.join("runs");
    paths::ensure_dir(&runs_dir)?;
    let run_store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir.clone()));
    run_store
        .write_run_envelope(&run_id, &run_envelope)
        .map_err(|e| anyhow::anyhow!("persist run envelope: {e}"))?;

    // Hoisted above the dispatcher build (was created a few lines further
    // down, right before `opts`) so `CliAgentDispatcher` can be handed a
    // clone of the same sink and emit `DispatchStarted` / `DispatchCompleted`
    // into the same `events.jsonl` this run's opts carry.
    let event_sink_for_run = {
        let events_path = runs_dir.join(&run_id).join("events.jsonl");
        match rupu_orchestrator::executor::JsonlSink::create(&events_path) {
            Ok(sink) => Some(Arc::new(sink) as Arc<dyn rupu_orchestrator::executor::EventSink>),
            Err(e) => {
                tracing::warn!(error = %e, "failed to create events.jsonl; continuing without event sink");
                None
            }
        }
    };

    // Hoisted above the dispatcher build: sub-agent dispatch resolves its
    // provider/model through the same config-derived defaults the step
    // factory below uses (ISSUES.md I-8).
    let openai_compatible = rupu_runtime::provider_factory::openai_compatible_map(&cfg.providers);
    let provider_tuning = rupu_runtime::provider_factory::provider_tuning_map(&cfg.providers);
    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        ctx.project_root.clone(),
        ctx.workspace_id.clone(),
        ctx.workspace_path.clone(),
        Arc::clone(&resolver),
        ctx.mode.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&run_store),
        event_sink_for_run.clone(),
        cfg.default_provider.clone(),
        cfg.default_model.clone(),
        openai_compatible.clone(),
        provider_tuning.clone(),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    // Shared across this run's initial `opts` AND the inline
    // approve-resume `resume_opts` built further down this function —
    // same registry + mode the `DefaultStepFactory` below carries, so
    // agent steps and action steps see identical permissions across a
    // pause/resume boundary too.
    let action_dispatcher = crate::resume::action_dispatcher_for(&mcp_registry, &ctx.mode);

    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: ctx.project_root.clone(),
        resolver,
        mode_str: ctx.mode.clone(),
        mcp_registry,
        system_prompt_suffix: ctx.system_prompt_suffix.clone(),
        dispatcher: Some(dispatcher_dyn),
        openai_compatible,
        provider_tuning,
        default_provider: cfg.default_provider.clone(),
        default_model: cfg.default_model.clone(),
        bash_timeout_secs: cfg.bash.timeout_secs.unwrap_or(120),
        bash_env_allowlist: cfg.bash.env_allowlist.clone().unwrap_or_default(),
    });

    let workflow_for_resume = workflow.clone();
    let workspace_path_for_resume = ctx.workspace_path.clone();
    let transcripts_for_resume = transcripts.clone();
    let event_for_resume = ctx.event.clone();
    let issue_for_resume = ctx.issue.clone();
    let issue_ref_for_resume = ctx.issue_ref.clone();
    let workspace_id_for_resume = ctx.workspace_id.clone();
    let factory_for_resume = Arc::clone(&factory);
    let run_store_for_resume = Arc::clone(&run_store);
    let body_for_resume = body.clone();
    let inputs_for_resume = inputs_map.clone();
    let strict_templates = ctx.strict_templates;

    let unit_dispatcher = build_dispatcher_if_needed(
        &workflow,
        &global,
        Arc::clone(&run_store),
        cfg.pricing.clone(),
    );

    // Cooperative pause delivery for this (possibly detached) run process.
    // `cp serve` launches runs as detached `rupu workflow run <id>`
    // subprocesses that can't receive an in-memory pause signal, so we build
    // a token, hand it to `run_workflow`, and spawn a poller that trips it
    // when `cp serve`'s `pause_run` writes the pause marker. Backward
    // compatible: if no marker is ever written the token never trips and the
    // run behaves exactly as before.
    let pause_token = tokio_util::sync::CancellationToken::new();
    let pause_poller = spawn_pause_marker_poller(
        Arc::clone(&run_store_for_resume),
        run_id.clone(),
        pause_token.clone(),
    );

    let opts = OrchestratorRunOpts {
        workflow,
        inputs: inputs_map,
        workspace_id: ctx.workspace_id,
        workspace_path: ctx.workspace_path,
        transcript_dir: transcripts,
        factory,
        event: ctx.event,
        issue: ctx.issue,
        issue_ref: ctx.issue_ref,
        run_store: Some(run_store),
        workflow_yaml: Some(body.clone()),
        resume_from: None,
        run_id_override: Some(run_id.clone()),
        strict_templates,
        event_sink: event_sink_for_run,
        unit_dispatcher,
        action_dispatcher: Some(Arc::clone(&action_dispatcher)),
        pause: Some(pause_token.clone()),
    };

    // Opt-in live three-zone view (dashboard + git-graph spine + focus
    // feed). Gated behind `RUPU_LIVE_VIEW=1` + a tty so the default
    // line-printer path (with its approval loop) is unchanged. The live
    // view does not handle approval gates; runs that pause render the
    // awaiting glyph and the loop exits when the run reaches a terminal
    // state.
    let use_live_view = ctx.attach_ui
        && ctx.shared_printer.is_none()
        && live_view_enabled(io::stdout().is_terminal(), ctx.plain);

    let workflow_result = if use_live_view {
        run_workflow_with_live_view(
            opts,
            workflow_for_resume.clone(),
            runs_dir.clone(),
            run_id.clone(),
            cfg.pricing.clone(),
        )
        .await
        .map_err(|e| to_anyhow_with_input_snippet(e, &path, &body))?
    } else if ctx.attach_ui {
        let runner_task = tokio::spawn(run_workflow(opts));
        let rid = run_id.clone();

        let mut attach_opts = crate::output::workflow_printer::AttachOpts {
            skip_header: false,
            skip_count: 0,
            live_event_hook: ctx.live_event_hook.clone(),
            view_mode: ctx.live_view,
        };
        let mut current_runner = runner_task;
        let mut current_run_id = rid.clone();
        let shared_printer = ctx.shared_printer.clone();

        loop {
            let name_owned = name.to_string();
            let rid_for_attach = current_run_id.clone();
            let runs_dir_for_attach = runs_dir.clone();
            let transcripts_for_attach = transcripts_dir_snapshot.clone();
            let attach_opts_for_attach = attach_opts.clone();
            let shared_printer_for_attach = shared_printer.clone();
            let outcome_result = tokio::task::spawn_blocking(move || {
                let printer_store = rupu_orchestrator::RunStore::new(runs_dir_for_attach.clone());
                let interactive_retained = shared_printer_for_attach.is_none()
                    && io::stdin().is_terminal()
                    && io::stdout().is_terminal()
                    && attach_opts_for_attach.skip_count == 0;
                if interactive_retained {
                    crate::output::workflow_printer::attach_and_render_interactive_with(
                        &name_owned,
                        &rid_for_attach,
                        &runs_dir_for_attach,
                        &printer_store,
                        attach_opts_for_attach,
                    )
                } else if let Some(shared_printer) = shared_printer_for_attach {
                    let mut printer = shared_printer
                        .lock()
                        .map_err(|_| io::Error::other("shared printer poisoned"))?;
                    crate::output::workflow_printer::attach_and_print_with(
                        &name_owned,
                        &rid_for_attach,
                        &runs_dir_for_attach,
                        &transcripts_for_attach,
                        &mut printer,
                        &printer_store,
                        attach_opts_for_attach,
                    )
                } else {
                    let mut printer = crate::output::LineStreamPrinter::new();
                    crate::output::workflow_printer::attach_and_print_with(
                        &name_owned,
                        &rid_for_attach,
                        &runs_dir_for_attach,
                        &transcripts_for_attach,
                        &mut printer,
                        &printer_store,
                        attach_opts_for_attach,
                    )
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("workflow printer task panicked: {e}"))?;
            let outcome = match outcome_result {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("rupu: printer error: {e}");
                    crate::output::workflow_printer::AttachOutcome::Detached
                }
            };

            use crate::output::workflow_printer::AttachOutcome;
            if matches!(outcome, AttachOutcome::Cancelled) {
                current_runner.abort();
                let _ = current_runner.await;
                cancel_with_store(
                    run_store_for_resume.as_ref(),
                    &current_run_id,
                    "cancelled by operator",
                )?;
                return Ok(RunOutcomeSummary {
                    run_id: current_run_id,
                    awaiting_step_id: None,
                    artifact_manifest_path: None,
                    backend_id: Some(prepared_run.backend_id.clone()),
                    worker_id: prepared_run.worker_id.clone(),
                });
            }

            let result = current_runner
                .await
                .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                .map_err(|e| to_anyhow_with_input_snippet(e, &path, &body))?;

            match outcome {
                AttachOutcome::Done | AttachOutcome::Detached | AttachOutcome::Rejected => {
                    break result;
                }
                AttachOutcome::Cancelled => {
                    unreachable!("cancelled outcome is handled before join")
                }
                AttachOutcome::Approved { awaited_step_id } => {
                    let prior_records = run_store_for_resume
                        .read_step_results(&current_run_id)
                        .map_err(|e| anyhow::anyhow!("read step results for resume: {e}"))?;
                    let prior_count = prior_records.len();
                    let prior_step_results: Vec<rupu_orchestrator::StepResult> = prior_records
                        .iter()
                        .map(rupu_orchestrator::StepResult::from)
                        .collect();
                    let resume = rupu_orchestrator::ResumeState::from_approval(
                        current_run_id.clone(),
                        prior_step_results,
                        awaited_step_id,
                    );
                    let factory_dyn: Arc<dyn rupu_orchestrator::StepFactory> =
                        factory_for_resume.clone();
                    let resume_event_sink = {
                        let events_path = runs_dir.join(&current_run_id).join("events.jsonl");
                        match rupu_orchestrator::executor::JsonlSink::create(&events_path) {
                            Ok(sink) => {
                                Some(Arc::new(sink)
                                    as Arc<dyn rupu_orchestrator::executor::EventSink>)
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "failed to open events.jsonl for inline resume; continuing without event sink"
                                );
                                None
                            }
                        }
                    };
                    let resume_unit_dispatcher = build_dispatcher_if_needed(
                        &workflow_for_resume,
                        &global,
                        Arc::clone(&run_store_for_resume),
                        cfg.pricing.clone(),
                    );
                    let resume_opts = OrchestratorRunOpts {
                        workflow: workflow_for_resume.clone(),
                        inputs: inputs_for_resume.clone(),
                        workspace_id: workspace_id_for_resume.clone(),
                        workspace_path: workspace_path_for_resume.clone(),
                        transcript_dir: transcripts_for_resume.clone(),
                        factory: factory_dyn,
                        event: event_for_resume.clone(),
                        issue: issue_for_resume.clone(),
                        issue_ref: issue_ref_for_resume.clone(),
                        run_store: Some(Arc::clone(&run_store_for_resume)),
                        workflow_yaml: Some(body_for_resume.clone()),
                        resume_from: Some(resume),
                        run_id_override: None,
                        strict_templates,
                        event_sink: resume_event_sink,
                        unit_dispatcher: resume_unit_dispatcher,
                        action_dispatcher: Some(Arc::clone(&action_dispatcher)),
                        pause: Some(pause_token.clone()),
                    };
                    current_runner = tokio::spawn(run_workflow(resume_opts));
                    current_run_id = result.run_id.clone();
                    attach_opts = crate::output::workflow_printer::AttachOpts {
                        skip_header: true,
                        skip_count: prior_count,
                        live_event_hook: ctx.live_event_hook.clone(),
                        view_mode: ctx.live_view,
                    };
                    let _ = result;
                }
            }
        }
    } else {
        run_workflow(opts)
            .await
            .map_err(|e| to_anyhow_with_input_snippet(e, &path, &body))?
    };

    // The run has finished (terminal or paused); the marker poller has no
    // further use — stop it so it never outlives the run.
    pause_poller.abort();

    let artifact_manifest_path = persist_portable_run_metadata(
        run_store_for_resume.as_ref(),
        &prepared_run,
        run_envelope.trigger.wake_id.as_deref(),
    )?
    .map(|(path, _)| path);

    if notify_issue_enabled {
        if let (Some(ref_text), Some(payload)) =
            (&issue_ref_text_for_notify, &issue_payload_for_notify)
        {
            post_run_summary_to_issue(
                &registry_for_notify,
                ref_text,
                payload,
                &workflow_name_for_notify,
                &workflow_result,
            )
            .await;
        }
    }

    Ok(RunOutcomeSummary {
        run_id: workflow_result.run_id,
        awaiting_step_id: workflow_result.awaiting.map(|a| a.step_id),
        artifact_manifest_path,
        backend_id: Some(prepared_run.backend_id.clone()),
        worker_id: prepared_run.worker_id.clone(),
    })
}

/// Post a one-line summary comment to the targeted issue describing
/// the run's outcome. Best-effort — surfaces a `tracing::warn!` on
/// failure rather than propagating, so a slow / down issue tracker
/// doesn't fail an otherwise-successful run.
async fn post_run_summary_to_issue(
    registry: &rupu_scm::Registry,
    ref_text: &str,
    payload: &serde_json::Value,
    workflow_name: &str,
    result: &rupu_orchestrator::OrchestratorRunResult,
) {
    // Reconstruct an `IssueRef` from the persisted text + payload.
    // The text carries the canonical
    // `<tracker>:<project>/issues/<N>` form; the JSON payload's
    // `r.tracker` field is more reliable for the typed value.
    let tracker_str = payload
        .pointer("/r/tracker")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tracker = match tracker_str {
        "github" => rupu_scm::IssueTracker::Github,
        "gitlab" => rupu_scm::IssueTracker::Gitlab,
        other => {
            tracing::warn!(tracker = %other, "notifyIssue: unknown tracker; skipping comment");
            return;
        }
    };
    let project = payload
        .pointer("/r/project")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let number = payload
        .pointer("/r/number")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if project.is_empty() || number == 0 {
        tracing::warn!(ref_text, "notifyIssue: malformed payload; skipping comment");
        return;
    }
    let r = rupu_scm::IssueRef {
        tracker,
        project,
        number,
    };

    let Some(conn) = registry.issues(tracker) else {
        tracing::warn!(
            tracker = %tracker,
            "notifyIssue: no credential for tracker; skipping comment"
        );
        return;
    };

    let outcome = match &result.awaiting {
        Some(info) => format!("paused at step `{}` awaiting approval", info.step_id),
        None => {
            // Distinguish failure from success by checking that
            // every step in the result succeeded. The orchestrator
            // would have returned Err earlier if there was a hard
            // failure, so reaching here means a clean run.
            let step_count = result.step_results.len();
            format!("completed ({step_count} steps)")
        }
    };

    let body = format!(
        "🤖 rupu workflow `{}` (run `{}`) {}.\n\n\
         Inspect: `rupu workflow show-run {}`\n\
         Live: `rupu watch {}`",
        workflow_name, result.run_id, outcome, result.run_id, result.run_id,
    );

    if let Err(e) = conn.comment_issue(&r, &body).await {
        tracing::warn!(
            error = %e,
            ref_text,
            "notifyIssue: posting comment failed"
        );
    }
}

// DefaultStepFactory is now defined in rupu-orchestrator::step_factory.
// Construction sites below use rupu_orchestrator::DefaultStepFactory directly.

/// Warn when a workflow run will execute its agent steps at `bypass`
/// because no `--mode` was given (ISSUES.md I-78).
///
/// `ask` is the default mode, but a workflow step resolves `ask` to
/// `BypassDecider` — the agent runtime's interactive `ask` decider blocks on
/// stdin, and an unattended workflow step has no operator to answer it, so a
/// genuinely-prompting `ask` would hang every scheduled run.
///
/// The operator decision (2026-07-28) was to keep that behavior rather than
/// tighten it: making `ask` deny writers would break every existing workflow
/// that writes without an explicit mode, precisely *because* `ask` is the
/// default. So the gap is made loud instead of silent.
///
/// Only fires when `--mode` was **omitted** — someone who typed
/// `--mode ask`/`--mode bypass` has made a choice and does not need nagging.
///
/// Split from the printing so the condition is unit-testable without
/// capturing stderr.
fn should_warn_ask_is_bypass(mode: Option<&str>, mode_str: &str) -> bool {
    mode.is_none() && mode_str == "ask"
}

fn warn_if_ask_mode_is_effectively_bypass(mode: Option<&str>, mode_str: &str) {
    if should_warn_ask_is_bypass(mode, mode_str) {
        eprintln!(
            "warning: --mode not set; workflow steps run at `bypass` \
             (there is no operator to answer `ask` mid-run).\n         \
             Pass --mode readonly to deny bash/write_file/edit_file."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::{RunRecord, RunStatus};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[test]
    fn live_view_gate_defaults_on_for_tty() {
        // Run env-sensitive assertions under one test so the global
        // `RUPU_LIVE_VIEW` mutation is not racing parallel tests.
        let prev = std::env::var("RUPU_LIVE_VIEW").ok();
        std::env::remove_var("RUPU_LIVE_VIEW");

        // tty + not plain + no env override => default ON.
        assert!(live_view_enabled(true, false));
        // --plain forces the line printer even on a tty.
        assert!(!live_view_enabled(true, true));
        // non-tty always uses the line printer.
        assert!(!live_view_enabled(false, false));
        assert!(!live_view_enabled(false, true));

        // RUPU_LIVE_VIEW=0 / false is an OFF escape hatch.
        std::env::set_var("RUPU_LIVE_VIEW", "0");
        assert!(!live_view_enabled(true, false));
        std::env::set_var("RUPU_LIVE_VIEW", "false");
        assert!(!live_view_enabled(true, false));
        // Any other value leaves the default-on view enabled.
        std::env::set_var("RUPU_LIVE_VIEW", "1");
        assert!(live_view_enabled(true, false));

        match prev {
            Some(v) => std::env::set_var("RUPU_LIVE_VIEW", v),
            None => std::env::remove_var("RUPU_LIVE_VIEW"),
        }
    }

    fn sample_run_record(status: RunStatus, runner_pid: Option<u32>) -> RunRecord {
        RunRecord {
            id: "run_test_cancel".into(),
            workflow_name: "sample".into(),
            status,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_test".into(),
            workspace_path: PathBuf::from("/tmp/workspace"),
            transcript_dir: PathBuf::from("/tmp/transcripts"),
            started_at: Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: Some("step_approve".into()),
            approval_prompt: Some("approve?".into()),
            awaiting_since: Some(Utc::now()),
            expires_at: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid,
            source_wake_id: None,
            active_step_id: Some("step_run".into()),
            active_step_kind: None,
            active_step_agent: Some("writer".into()),
            active_step_transcript_path: Some(PathBuf::from("/tmp/transcripts/step.jsonl")),
            final_output: None,
            loop_progress: Default::default(),
        }
    }

    #[test]
    fn resume_blocked_by_live_runner_covers_all_cases() {
        let own_pid = std::process::id();

        // No recorded runner_pid → nothing to be blocked by.
        assert_eq!(resume_blocked_by_live_runner(None, own_pid), None);

        // The recorded pid IS us (e.g. an in-process resume) → not a
        // foreign live process, safe to resume.
        assert_eq!(resume_blocked_by_live_runner(Some(own_pid), own_pid), None);

        // A dead pid (process no longer running) → safe to resume.
        // u32::MAX is not a valid pid on any supported platform.
        let dead_pid = u32::MAX;
        assert!(!rupu_orchestrator::runs::pid_is_running(dead_pid));
        assert_eq!(resume_blocked_by_live_runner(Some(dead_pid), own_pid), None);

        // A live, FOREIGN pid → blocked. Use our own process's pid as the
        // "live" pid but pass a different `own_pid` sentinel so the guard
        // sees it as a foreign live runner.
        let live_pid = own_pid;
        let different_own_pid = own_pid.wrapping_add(1);
        assert_ne!(live_pid, different_own_pid);
        assert!(rupu_orchestrator::runs::pid_is_running(live_pid));
        assert_eq!(
            resume_blocked_by_live_runner(Some(live_pid), different_own_pid),
            Some(live_pid)
        );
    }

    #[tokio::test]
    async fn pause_marker_poller_trips_token_when_marker_appears() {
        // The delivery mechanism for a pause requested against a detached
        // run process: the poller watches `RunStore::pause_marker_exists`
        // and trips `token` once the marker file shows up.
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
        let run_id = "run_poll_test".to_string();
        let token = tokio_util::sync::CancellationToken::new();

        let handle = spawn_pause_marker_poller(store.clone(), run_id.clone(), token.clone());

        // No marker yet — give the poller a couple of ticks; it must not
        // trip the token on its own.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!token.is_cancelled());

        store.set_pause_marker(&run_id).unwrap();

        // Poll interval is 250ms; wait up to ~1s for the poller to notice.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !token.is_cancelled() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(token.is_cancelled());

        handle.abort();
    }

    #[test]
    fn cancel_with_store_marks_running_run_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::Running, Some(999_999));
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        let outcome =
            cancel_with_store(&store, "run_test_cancel", "cancelled by operator").unwrap();
        assert_eq!(
            outcome,
            CancelOutcome::MarkedCancelled {
                pid: Some(999_999),
                was_running: false,
            }
        );

        let persisted = store.load("run_test_cancel").unwrap();
        assert_eq!(persisted.status, RunStatus::Cancelled);
        assert_eq!(
            persisted.error_message.as_deref(),
            Some("cancelled by operator")
        );
        assert!(persisted.finished_at.is_some());
        assert_eq!(persisted.runner_pid, None);
        assert_eq!(persisted.awaiting_step_id, None);
        assert_eq!(persisted.active_step_id, None);
        assert_eq!(persisted.active_step_agent, None);
        assert_eq!(persisted.active_step_transcript_path, None);
    }

    #[tokio::test]
    async fn delete_run_requires_force() {
        let err = delete_run("run_x", false).await.unwrap_err();
        assert!(err.to_string().contains("--force"));
    }

    // I2 (final-review finding): `rupu workflow delete-run --force` must
    // still refuse a non-terminal run — `--force` only bypasses the missing
    // flag `bail!` above, it never authorized deleting a live run's
    // directory out from under the process still writing it. Before this
    // fix `delete_run` called `store.delete` directly with no such guard,
    // which the SSH `HostConnector::delete_run` path inherited verbatim.
    #[test]
    fn delete_run_with_store_refuses_non_terminal_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::Running, Some(999_999));
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        let err = delete_run_with_store(&store, "run_test_cancel").unwrap_err();
        assert!(
            err.to_string().contains("not terminal"),
            "unexpected error: {err}"
        );

        // The run directory must survive the refused delete.
        assert!(store.load("run_test_cancel").is_ok());
    }

    #[test]
    fn delete_run_with_store_allows_terminal_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::Completed, None);
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        delete_run_with_store(&store, "run_test_cancel").unwrap();

        assert!(store.load("run_test_cancel").is_err());
    }

    #[test]
    fn cancel_with_store_rejects_awaiting_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::AwaitingApproval, None);
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        let outcome =
            cancel_with_store(&store, "run_test_cancel", "cancelled by operator").unwrap();
        assert_eq!(outcome, CancelOutcome::RejectedAwaitingApproval);

        let persisted = store.load("run_test_cancel").unwrap();
        assert_eq!(persisted.status, RunStatus::Rejected);
        assert_eq!(
            persisted.error_message.as_deref(),
            Some("rejected: cancelled by operator")
        );
    }

    #[test]
    fn pause_with_store_marks_running_run_paused_and_writes_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::Running, Some(999_999));
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        pause_with_store(&store, "run_test_cancel").unwrap();

        let persisted = store.load("run_test_cancel").unwrap();
        assert_eq!(persisted.status, RunStatus::Paused);
        assert!(
            store.pause_marker_exists("run_test_cancel"),
            "pause_with_store must deliver the marker so a detached runner honors it"
        );
    }

    #[test]
    fn pause_with_store_rejects_terminal_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::Completed, None);
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        let err = pause_with_store(&store, "run_test_cancel").unwrap_err();
        assert!(err.to_string().contains("already terminal"));
        assert!(
            !store.pause_marker_exists("run_test_cancel"),
            "a rejected pause must not write the marker"
        );
    }

    #[test]
    fn pause_with_store_rejects_already_paused_run() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let record = sample_run_record(RunStatus::Paused, None);
        store.create(record, "name: sample\nsteps: []\n").unwrap();

        let err = pause_with_store(&store, "run_test_cancel").unwrap_err();
        assert!(err.to_string().contains("only a running run can be paused"));
    }

    // ── Task 5b-2a: CLI `--gate` wiring (approve/reject phase 1) ────────

    /// Record with TWO parked gates (`gate_a`, `gate_b`), neither overdue —
    /// the CLI-level counterpart to `runs.rs`'s `two_gate_record`, built
    /// straight off `sample_run_record` so it carries the same
    /// otherwise-valid `RunRecord` shape these CLI tests need.
    fn two_gate_awaiting_record(id: &str) -> RunRecord {
        let mut rec = sample_run_record(RunStatus::AwaitingApproval, None);
        rec.id = id.to_string();
        let since = Utc::now();
        rec.awaiting = vec![
            rupu_orchestrator::runs::AwaitingGate {
                step_id: "gate_a".into(),
                prompt: Some("approve a?".into()),
                since,
                expires_at: None,
            },
            rupu_orchestrator::runs::AwaitingGate {
                step_id: "gate_b".into(),
                prompt: Some("approve b?".into()),
                since,
                expires_at: None,
            },
        ];
        rec.sync_awaiting_compat();
        rec
    }

    const TWO_GATE_YAML: &str =
        "name: g\nsteps:\n  - id: gate_a\n    approval: {}\n  - id: gate_b\n    approval: {}\n";

    #[test]
    fn ambiguous_gate_message_lists_every_candidate_and_the_gate_flag() {
        let msg = ambiguous_gate_message(
            "approve",
            "run_x",
            &["gate_a".to_string(), "gate_b".to_string()],
        );
        assert!(msg.contains("gate_a"), "message: {msg}");
        assert!(msg.contains("gate_b"), "message: {msg}");
        assert!(msg.contains("--gate"), "message: {msg}");
        assert!(msg.contains("run_x"), "message: {msg}");
    }

    #[test]
    fn resolve_approve_gate_with_explicit_gate_targets_only_that_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = two_gate_awaiting_record("run_cli_approve_b");
        store.create(rec.clone(), TWO_GATE_YAML).unwrap();

        let outcome = resolve_approve_gate(&store, &rec.id, Some("gate_b"), None).unwrap();
        match outcome {
            ApproveGateOutcome::Approved { step_id, .. } => assert_eq!(step_id, "gate_b"),
            ApproveGateOutcome::ExpiredRejected { .. } => panic!("must not have expired"),
        }

        // gate_a is untouched and still parked; the run stays
        // AwaitingApproval (NOT Running — the set is non-empty).
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::AwaitingApproval);
        assert_eq!(reloaded.awaiting.len(), 1);
        assert_eq!(reloaded.awaiting[0].step_id, "gate_a");
    }

    #[test]
    fn resolve_approve_gate_with_no_gate_on_a_multi_gate_run_errors_listing_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = two_gate_awaiting_record("run_cli_approve_ambiguous");
        store.create(rec.clone(), TWO_GATE_YAML).unwrap();

        let err = resolve_approve_gate(&store, &rec.id, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("gate_a"), "message: {msg}");
        assert!(msg.contains("gate_b"), "message: {msg}");
        assert!(msg.contains("--gate"), "message: {msg}");

        // Nothing mutated by the failed attempt.
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::AwaitingApproval);
        assert_eq!(reloaded.awaiting.len(), 2);
    }

    #[test]
    fn resolve_approve_gate_with_no_gate_on_a_single_gate_run_works_as_today() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = sample_run_record(RunStatus::AwaitingApproval, None);
        store
            .create(rec.clone(), "name: sample\nsteps: []\n")
            .unwrap();

        let outcome = resolve_approve_gate(&store, &rec.id, None, None).unwrap();
        match outcome {
            ApproveGateOutcome::Approved { step_id, .. } => assert_eq!(step_id, "step_approve"),
            ApproveGateOutcome::ExpiredRejected { .. } => panic!("must not have expired"),
        }
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Running);
        assert!(reloaded.awaiting_step_id.is_none());
    }

    #[test]
    fn resolve_approve_gate_with_unknown_gate_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = two_gate_awaiting_record("run_cli_approve_unknown");
        store.create(rec.clone(), TWO_GATE_YAML).unwrap();

        let err = resolve_approve_gate(&store, &rec.id, Some("no_such_gate"), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no_such_gate"), "message: {msg}");
        assert!(msg.contains("not awaiting approval"), "message: {msg}");
    }

    #[test]
    fn resolve_reject_gate_with_explicit_gate_targets_only_that_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = two_gate_awaiting_record("run_cli_reject_a");
        store.create(rec.clone(), TWO_GATE_YAML).unwrap();

        let outcome =
            resolve_reject_gate(&store, &rec.id, Some("no thanks"), Some("gate_a")).unwrap();
        assert_eq!(outcome.step_id, "gate_a");
        assert_eq!(outcome.via, "human");

        // gate_b is untouched and still parked; the run stays
        // AwaitingApproval (NOT Rejected — the set is non-empty).
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::AwaitingApproval);
        assert_eq!(reloaded.awaiting.len(), 1);
        assert_eq!(reloaded.awaiting[0].step_id, "gate_b");
    }

    #[test]
    fn resolve_reject_gate_with_no_gate_on_a_multi_gate_run_errors_listing_candidates() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = two_gate_awaiting_record("run_cli_reject_ambiguous");
        store.create(rec.clone(), TWO_GATE_YAML).unwrap();

        let err = resolve_reject_gate(&store, &rec.id, None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("gate_a"), "message: {msg}");
        assert!(msg.contains("gate_b"), "message: {msg}");
        assert!(msg.contains("--gate"), "message: {msg}");
    }

    #[test]
    fn resolve_reject_gate_with_no_gate_on_a_single_gate_run_works_as_today() {
        let tmp = tempfile::tempdir().unwrap();
        let store = rupu_orchestrator::RunStore::new(tmp.path().join("runs"));
        let rec = sample_run_record(RunStatus::AwaitingApproval, None);
        store
            .create(rec.clone(), "name: sample\nsteps: []\n")
            .unwrap();

        let outcome = resolve_reject_gate(&store, &rec.id, Some("no"), None).unwrap();
        assert_eq!(outcome.step_id, "step_approve");
        let reloaded = store.load(&rec.id).unwrap();
        assert_eq!(reloaded.status, RunStatus::Rejected);
    }

    // ── I-36 / I-38: gate decision provenance ─────────────────────────
    //
    // The gate audit record must always be written (even for an empty
    // `on_reject` chain), and must say WHO decided and WHETHER a human
    // decided at all. Full end-to-end tests, driving the real `reject`/
    // `approve` command handlers in-process (same code a spawned `rupu
    // workflow approve`/`reject` runs) against a real disk-backed
    // `RunStore` under a temp `RUPU_HOME`.

    /// Single-gate `AwaitingApproval` record with a real-on-disk workspace
    /// (required: the resume/cleanup rebuild path canonicalizes
    /// `workspace_path`). `expires_at` lets callers construct an overdue
    /// gate for the timeout-provenance test.
    fn single_gate_awaiting_record(
        id: &str,
        workspace: &std::path::Path,
        since: chrono::DateTime<Utc>,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> RunRecord {
        let mut rec = sample_run_record(RunStatus::AwaitingApproval, None);
        rec.id = id.to_string();
        rec.workspace_path = workspace.to_path_buf();
        rec.transcript_dir = workspace.join(".rupu/transcripts");
        rec.awaiting = vec![rupu_orchestrator::runs::AwaitingGate {
            step_id: "gate".into(),
            prompt: Some("Approve?".into()),
            since,
            expires_at,
        }];
        rec.sync_awaiting_compat();
        rec
    }

    /// I-36: pre-fix, `reject()` only ran `run_reject_cleanup` (the sole
    /// `emit_gate_result` caller) when `cheap_on_reject_chain_len` reported
    /// a non-empty `on_reject:` chain — an empty chain (this fixture) short-
    /// circuited past it entirely, so NO gate decision row was ever written.
    #[tokio::test(flavor = "multi_thread")]
    async fn reject_with_empty_on_reject_chain_still_records_gate_decision_with_actor() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let store = rupu_orchestrator::RunStore::new(home.join("runs"));
        let now = Utc::now();
        let rec = single_gate_awaiting_record("run_i36_empty_reject", &workspace, now, None);
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate\n    approval:\n      prompt: \"Approve?\"\n",
            )
            .unwrap();

        std::env::set_var("RUPU_HOME", &home);
        let result = reject(&rec.id, Some("not today"), None).await;
        std::env::remove_var("RUPU_HOME");
        result.expect("reject should succeed even with an empty on_reject chain");

        let step_results = store.read_step_results(&rec.id).unwrap();
        let gate_record = step_results.iter().find(|r| r.step_id == "gate").expect(
            "I-36: an empty on_reject chain must still record the gate's rejected decision",
        );
        let output: serde_json::Value = serde_json::from_str(&gate_record.output).unwrap();
        assert_eq!(output["decision"], "rejected");
        assert_eq!(output["reason"], "not today");
        assert_eq!(
            output["approver"],
            whoami::username(),
            "the rejecting operator's identity must be recorded, not discarded"
        );
    }

    /// I-38: the `cp serve` gate sweep resolves an overdue `on_timeout:
    /// approve` gate by spawning `rupu workflow approve --gate <id>` — the
    /// SAME code this test drives in-process. Pre-fix, `resolve_approve_gate`
    /// detected the overdue-timeout condition only to `println!` it, then
    /// still resumed via the plain approve-resume path, which hardcoded
    /// `via: "human"` — indistinguishable from a genuine operator decision.
    #[tokio::test(flavor = "multi_thread")]
    async fn approve_of_an_overdue_on_timeout_approve_gate_records_via_timeout() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let store = rupu_orchestrator::RunStore::new(home.join("runs"));
        let now = Utc::now();
        let since = now - chrono::Duration::seconds(120);
        let overdue = now - chrono::Duration::seconds(30);
        let rec = single_gate_awaiting_record("run_i38_overdue", &workspace, since, Some(overdue));
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate\n    approval:\n      prompt: \"Approve?\"\n      timeout_seconds: 10\n      on_timeout: approve\n",
            )
            .unwrap();

        std::env::set_var("RUPU_HOME", &home);
        let result = approve(&rec.id, None, None, None).await;
        std::env::remove_var("RUPU_HOME");
        result.expect("approve of an overdue on_timeout: approve gate should succeed");

        let step_results = store.read_step_results(&rec.id).unwrap();
        let gate_record = step_results
            .iter()
            .find(|r| r.step_id == "gate")
            .expect("gate decision must be recorded");
        let output: serde_json::Value = serde_json::from_str(&gate_record.output).unwrap();
        assert_eq!(output["decision"], "approved");
        assert_eq!(
            output["via"], "timeout",
            "a sweep/timeout-driven approve must not be indistinguishable from a human one"
        );
    }

    /// I-38 guard: a genuine, NOT-overdue operator approve must still record
    /// `via: "human"` and the operator's own identity — proves the timeout
    /// detection above doesn't over-apply to the ordinary case.
    #[tokio::test(flavor = "multi_thread")]
    async fn approve_of_a_fresh_gate_still_records_via_human_and_operator_identity() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let store = rupu_orchestrator::RunStore::new(home.join("runs"));
        let now = Utc::now();
        let rec = single_gate_awaiting_record("run_i38_fresh", &workspace, now, None);
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate\n    approval:\n      prompt: \"Approve?\"\n",
            )
            .unwrap();

        std::env::set_var("RUPU_HOME", &home);
        let result = approve(&rec.id, None, None, None).await;
        std::env::remove_var("RUPU_HOME");
        result.expect("approve of a fresh gate should succeed");

        let step_results = store.read_step_results(&rec.id).unwrap();
        let gate_record = step_results
            .iter()
            .find(|r| r.step_id == "gate")
            .expect("gate decision must be recorded");
        let output: serde_json::Value = serde_json::from_str(&gate_record.output).unwrap();
        assert_eq!(output["decision"], "approved");
        assert_eq!(output["via"], "human");
        assert_eq!(output["approver"], whoami::username());
    }

    /// ISSUES.md I-82: a web-initiated approve's true actor must reach the
    /// gate decision row, not a placeholder or the OS user running `cp
    /// serve`. Drives the exact same two-step handoff the real system
    /// does across its process boundary:
    ///   1. `request_resume_approval(..., "web", ...)` — what the CP web
    ///      `approve_run` handler calls; this is the marker-only step that
    ///      now persists the actor onto `record.resume_approver`.
    ///   2. `approve(..., approver_override: Some("web"))` — what `cp
    ///      serve`'s resume worker spawns as `rupu workflow approve
    ///      --approver web <id>` once it reads `resume_approver` back off
    ///      the marker (`resume_one_run` / `build_resume_argv`).
    /// Pre-fix, step 2 had no override and always fell back to
    /// `whoami::username()`, so the recorded `approver` was never `"web"`
    /// — it was whatever account happened to run the worker process.
    #[tokio::test(flavor = "multi_thread")]
    async fn approve_via_web_path_records_web_actor_not_operator_identity() {
        let _guard = crate::test_support::ENV_LOCK.lock().await;
        crate::test_support::ensure_crypto_provider();

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let store = rupu_orchestrator::RunStore::new(home.join("runs"));
        let now = Utc::now();
        let rec = single_gate_awaiting_record("run_i82_web_approve", &workspace, now, None);
        store
            .create(
                rec.clone(),
                "name: g\nsteps:\n  - id: gate\n    approval:\n      prompt: \"Approve?\"\n",
            )
            .unwrap();

        // Step 1: the CP web handler's marker-only approve.
        store
            .request_resume_approval(&rec.id, "web", None, Utc::now(), None)
            .expect("web approve marker should be recorded");
        assert_eq!(
            store.load(&rec.id).unwrap().resume_approver.as_deref(),
            Some("web"),
            "the resolved actor must be persisted onto the resume marker"
        );

        // Step 2: the resume worker's spawned `workflow approve --approver
        // web`, simulated in-process (see `resume_one_run`'s own test for
        // the argv-plumbing half of this round-trip).
        std::env::set_var("RUPU_HOME", &home);
        let result = approve(&rec.id, None, None, Some("web")).await;
        std::env::remove_var("RUPU_HOME");
        result.expect("web-initiated approve should succeed");

        let step_results = store.read_step_results(&rec.id).unwrap();
        let gate_record = step_results
            .iter()
            .find(|r| r.step_id == "gate")
            .expect("gate decision must be recorded");
        let output: serde_json::Value = serde_json::from_str(&gate_record.output).unwrap();
        assert_eq!(output["decision"], "approved");
        assert_eq!(
            output["approver"], "web",
            "gate decision must name the web approver, not a placeholder or \
             the OS user running the resume worker (got {:?})",
            output["approver"]
        );
        assert_ne!(
            output["approver"],
            serde_json::json!(whoami::username()),
            "must not silently fall back to the OS user's identity"
        );
    }

    // ── I-41: gate/action arms in the steps table ────────────────────
    //
    // Before this, both fell through to the `linear` arm, printing
    // KIND=linear with a BLANK primary column — a gate has no `agent:`,
    // and an action step's whole identity is the tool it calls, which was
    // never named anywhere in the table. The graph renderer already
    // handled both; this table was the only view that didn't.

    fn parse_one_step(yaml: &str) -> rupu_orchestrator::Step {
        let wf = rupu_orchestrator::Workflow::parse(yaml).expect("fixture must parse");
        wf.steps.into_iter().next().expect("one step")
    }

    #[test]
    fn steps_table_renders_a_gate_node_as_kind_gate_not_linear() {
        let step = parse_one_step(
            r#"
name: gated
steps:
  - id: sign-off
    approval:
      required: true
      prompt: "Ship this to production?"
      timeout_seconds: 3600
      on_timeout: reject
"#,
        );
        let (kind, primary, detail) = workflow_step_table_summary(&step);
        assert_eq!(kind, "gate", "a gate must not render as `linear`");
        assert!(
            primary.contains("Ship this"),
            "the gate prompt belongs in PRIMARY, which was blank before: {primary}"
        );
        assert!(detail.contains("timeout 3600s"), "detail was: {detail}");
        assert!(detail.contains("on_timeout reject"), "detail was: {detail}");
    }

    #[test]
    fn steps_table_renders_an_action_node_naming_the_tool() {
        let step = parse_one_step(
            r#"
name: acts
steps:
  - id: comment
    action: issues.comment
    with:
      project: "acme/widget"
      number: 7
      body: "triaged"
"#,
        );
        let (kind, primary, detail) = workflow_step_table_summary(&step);
        assert_eq!(kind, "action", "an action must not render as `linear`");
        assert_eq!(
            primary, "issues.comment",
            "the tool name is the action's identity and was never shown before"
        );
        // `with:` keys are listed sorted so the column is stable across runs.
        assert!(
            detail.contains("with body, number, project"),
            "detail was: {detail}"
        );
    }

    #[test]
    fn steps_table_still_renders_a_plain_agent_step_as_linear() {
        // Guard against the new arms swallowing ordinary steps: an agent step
        // that merely *carries* an inline `approval:` is NOT a gate node.
        let step = parse_one_step(
            r#"
name: plain
steps:
  - id: build
    agent: coder
    prompt: "do the thing"
    approval:
      required: true
"#,
        );
        let (kind, primary, _detail) = workflow_step_table_summary(&step);
        assert_eq!(kind, "linear");
        assert_eq!(primary, "coder");
    }

    // ── I-78: `ask` is effectively `bypass` for workflow steps ───────
    //
    // Operator decision (2026-07-28): keep the behavior, make it visible.
    // Tightening `ask` would break every workflow that writes without an
    // explicit --mode, because `ask` is ALSO the default.

    #[test]
    fn omitting_mode_warns_that_steps_run_at_bypass() {
        // The warning must fire only when the operator made no choice.
        assert!(should_warn_ask_is_bypass(None, "ask"));
    }

    #[test]
    fn an_explicit_mode_never_warns() {
        // Someone who typed --mode has decided; nagging them is noise.
        assert!(!should_warn_ask_is_bypass(Some("ask"), "ask"));
        assert!(!should_warn_ask_is_bypass(Some("bypass"), "bypass"));
        assert!(!should_warn_ask_is_bypass(Some("readonly"), "readonly"));
    }

    #[test]
    fn a_resolved_non_ask_mode_never_warns() {
        // Defensive: if the default ever stops being `ask`, this must not
        // start warning about a mode that does gate writes.
        assert!(!should_warn_ask_is_bypass(None, "readonly"));
    }

    // ── Task 6: run-id fragment resolution ────────────────────────────

    fn run_candidates() -> Vec<RunCandidate> {
        let now = chrono::Utc::now();
        vec![
            RunCandidate {
                id: "run_01KYSMDNG84N9Z8XXHQZP3GKYJ".to_string(),
                workflow_name: "nightly-health".to_string(),
                status: rupu_orchestrator::RunStatus::Completed,
                started_at: now,
            },
            RunCandidate {
                id: "run_01KYSM3KE60KM2P2EDJR1V1BCP".to_string(),
                workflow_name: "pr-code-review".to_string(),
                status: rupu_orchestrator::RunStatus::Failed,
                started_at: now,
            },
            RunCandidate {
                id: "run_01KYPASX18NYRER5NQPDWB2HZV".to_string(),
                workflow_name: "issue-triage".to_string(),
                status: rupu_orchestrator::RunStatus::Running,
                started_at: now,
            },
        ]
    }

    #[test]
    fn resolve_run_id_accepts_compact_form() {
        let c = run_candidates();
        let compact = crate::output::ids::compact_id(&c[0].id);
        assert_eq!(compact, "run_01KYSMDN…GKYJ");
        assert_eq!(resolve_run_id(&c, &compact).expect("resolves"), c[0].id);
    }

    #[test]
    fn resolve_run_id_accepts_bare_suffix() {
        let c = run_candidates();
        assert_eq!(resolve_run_id(&c, "P3GKYJ").expect("resolves"), c[0].id);
    }

    #[test]
    fn resolve_run_id_accepts_full_id() {
        let c = run_candidates();
        assert_eq!(resolve_run_id(&c, &c[2].id).expect("resolves"), c[2].id);
    }

    #[test]
    fn resolve_run_id_errors_on_ambiguity_listing_candidates() {
        // ULIDs from the same era share a long prefix — the common case.
        let c = run_candidates();
        let err = resolve_run_id(&c, "run_01KYSM").expect_err("ambiguous");
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got: {msg}");
        assert!(msg.contains(&c[0].id), "got: {msg}");
        assert!(msg.contains(&c[1].id), "got: {msg}");
        // The non-matching run must not be listed.
        assert!(!msg.contains(&c[2].id), "got: {msg}");
    }

    #[test]
    fn resolve_run_id_ambiguity_message_includes_disambiguating_context() {
        // Finding 3: bare ids alone are indistinguishable ULIDs. The
        // ambiguity error must carry workflow name and status per
        // candidate — the data is already in hand from the records
        // used to detect the ambiguity, so this costs no extra I/O.
        let c = run_candidates();
        let err = resolve_run_id(&c, "run_01KYSM").expect_err("ambiguous");
        let msg = err.to_string();
        assert!(msg.contains(&c[0].workflow_name), "got: {msg}");
        assert!(msg.contains(&c[1].workflow_name), "got: {msg}");
        assert!(msg.contains(c[0].status.as_str()), "got: {msg}");
        assert!(msg.contains(c[1].status.as_str()), "got: {msg}");
    }

    #[test]
    fn resolve_run_id_reports_unknown() {
        let err = resolve_run_id(&run_candidates(), "zzzzzz").expect_err("unknown");
        assert!(err.to_string().contains("unknown run"));
    }
}
