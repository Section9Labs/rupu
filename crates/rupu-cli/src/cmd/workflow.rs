//! `rupu workflow list | show | run`.
//!
//! Lists workflows from `<global>/workflows/*.yaml` and (if any)
//! `<project>/.rupu/workflows/*.yaml`; project entries shadow global by
//! filename. `show` prints the YAML body. `run` parses the workflow,
//! builds a [`StepFactory`] that wires real providers via
//! [`rupu_runtime::provider_factory::build_for_provider`], and dispatches
//! [`rupu_orchestrator::run_workflow`].
//!
//! The factory carries a clone of the parsed [`Workflow`] so each
//! step's `agent:` field is honored (no hardcoded agent name).

use crate::cmd::completers::workflow_names;
use crate::output::formats::OutputFormat;
use crate::output::palette::Status as UiStatus;
use crate::output::report::{self, CollectionOutput, DetailOutput, EventOutput};
use crate::output::LineStreamPrinter;
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts};
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
use rupu_orchestrator::Workflow;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use tracing::warn;
use ulid::Ulid;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all workflows (global + project).
    List,
    /// Print a workflow's YAML body.
    Show {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
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
    /// Run a workflow.
    Run {
        /// Workflow name (filename stem under `workflows/`).
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Optional run-target. Accepts repo (`github:owner/repo`,
        /// `gitlab:group/proj`), PR (`github:owner/repo#42`), or
        /// issue (`github:owner/repo/issues/42`). Repo / PR targets
        /// clone to a tmpdir for the run; issue targets pre-fetch
        /// the issue payload and bind it as `{{ issue.* }}` in step
        /// prompts.
        target: Option<String>,
        /// `KEY=VALUE` template inputs (repeatable).
        #[arg(long, value_parser = parse_kv)]
        input: Vec<(String, String)>,
        /// Override permission mode (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
        /// Use the alt-screen TUI canvas instead of the default line-stream
        /// output. The canvas offers a DAG view and live status glyphs but
        /// requires an interactive terminal.
        #[arg(long)]
        canvas: bool,
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
    },
    /// Approve a paused run and resume execution from the awaited
    /// step. The run must be in `awaiting_approval` status.
    Approve {
        run_id: String,
        /// Override permission mode for the resumed run
        /// (`ask` | `bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
    },
    /// Reject a paused run. Marks it `rejected`; no further steps
    /// dispatch.
    Reject {
        run_id: String,
        /// Optional human-readable reason recorded in the run's
        /// `error_message`.
        #[arg(long)]
        reason: Option<String>,
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
            show(&name, no_color, theme.as_deref(), pager_flag, global_format).await
        }
        Action::Edit {
            name,
            scope,
            editor,
        } => edit(&name, scope.as_deref(), editor.as_deref()).await,
        Action::Run {
            name,
            target,
            input,
            mode,
            canvas,
        } => {
            run(
                &name,
                target.as_deref(),
                input,
                mode.as_deref(),
                None,
                canvas,
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
        Action::ShowRun { run_id } => show_run(&run_id, global_format).await,
        Action::Approve { run_id, mode } => approve(&run_id, mode.as_deref()).await,
        Action::Reject { run_id, reason } => reject(&run_id, reason.as_deref()).await,
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
        Action::Run { .. } => ("workflow run", report::TABLE_ONLY),
        Action::Approve { .. } => ("workflow approve", report::TABLE_ONLY),
        Action::Reject { .. } => ("workflow reject", report::TABLE_ONLY),
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
        let rendered = crate::cmd::ui::highlight_yaml(&self.report.item.body, &self.prefs);
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
        let item = &self.report.item;
        render_pretty_workflow_run(item)
    }
}

fn render_pretty_workflow_run(item: &WorkflowShowRunItem) -> anyhow::Result<()> {
    let mut printer = LineStreamPrinter::new();
    let started_at = chrono::DateTime::parse_from_str(&item.started_at, "%Y-%m-%d %H:%M:%S UTC")
        .map(|value| value.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());

    printer.workflow_header(&item.workflow, &item.run_id, started_at);
    printer.sideband_event(
        workflow_run_status(item.status.as_str()),
        "status",
        Some(&item.status),
    );

    let workspace = format!("{}  ·  {}", item.workspace_id, item.workspace_path);
    printer.sideband_event(UiStatus::Active, "workspace", Some(&workspace));
    printer.sideband_event(UiStatus::Active, "started", Some(&item.started_at));

    if let Some(finished_at) = item.finished_at.as_deref() {
        printer.sideband_event(UiStatus::Complete, "finished", Some(finished_at));
    }
    if let Some(error) = item.error.as_deref() {
        printer.sideband_event(UiStatus::Failed, "error", Some(error));
    }
    if let Some(step) = item.awaiting_step.as_deref() {
        let mut detail = step.to_string();
        if let Some(since) = item.awaiting_since.as_deref() {
            detail.push_str(&format!("  ·  since {since}"));
        }
        if let Some(expires) = item.expires_at.as_deref() {
            detail.push_str(&format!("  ·  expires {expires}"));
        }
        printer.sideband_event(UiStatus::Awaiting, "awaiting", Some(&detail));
    }

    for (key, value) in &item.inputs {
        let detail = format!("{key} = {}", truncate_pretty(value, 72));
        printer.sideband_event(UiStatus::Active, "input", Some(&detail));
    }

    if !item.steps.is_empty() {
        printer.phase_separator();
        for step in &item.steps {
            let detail = format!(
                "{}  ·  {}",
                step.status,
                truncate_pretty(&step.transcript_path, 72)
            );
            printer.sideband_event(
                workflow_step_status(step.status.as_str()),
                &format!("step {}", step.step_id),
                Some(&detail),
            );
            for child in &step.items {
                let sub_detail = format!(
                    "{}  ·  {}",
                    child.status,
                    truncate_pretty(&child.transcript_path, 68)
                );
                printer.sideband_event(
                    workflow_step_status(child.status.as_str()),
                    &format!("item {}", child.label),
                    Some(&sub_detail),
                );
            }
        }
    }

    if !item.usage_rows.is_empty() {
        printer.phase_separator();
        for row in &item.usage_rows {
            let detail = format!(
                "{} · {} · {}  ·  in {} out {} cached {}{}",
                row.provider,
                row.model,
                row.agent,
                row.input_tokens,
                row.output_tokens,
                row.cached_tokens,
                row.cost_usd
                    .map(|value| format!("  ·  ${value:.4}"))
                    .unwrap_or_default()
            );
            printer.sideband_event(UiStatus::Active, "usage", Some(&detail));
        }
        if let Some(totals) = &item.usage_totals {
            let detail = format!(
                "in {} out {} cached {}{}",
                totals.input_tokens,
                totals.output_tokens,
                totals.cached_tokens,
                totals
                    .cost_usd
                    .map(|value| format!("  ·  ${value:.4}"))
                    .unwrap_or_default()
            );
            printer.sideband_event(UiStatus::Complete, "usage total", Some(&detail));
        }
    }

    Ok(())
}

fn workflow_run_status(status: &str) -> UiStatus {
    match status {
        "completed" => UiStatus::Complete,
        "failed" | "rejected" => UiStatus::Failed,
        "awaiting_approval" => UiStatus::Awaiting,
        "running" => UiStatus::Working,
        _ => UiStatus::Active,
    }
}

fn workflow_step_status(status: &str) -> UiStatus {
    match status {
        "ok" | "completed" => UiStatus::Complete,
        "fail" | "failed" => UiStatus::Failed,
        "skipped" => UiStatus::Skipped,
        _ => UiStatus::Active,
    }
}

fn truncate_pretty(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let mut out = value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>();
        out.push('…');
        out
    }
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
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, theme, pager_flag, None);
    let output = WorkflowShowOutput {
        prefs,
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

async fn runs(
    limit: usize,
    status_filter: Option<&str>,
    issue_filter: Option<&str>,
    no_color: bool,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let mut all = store
        .list()
        .map_err(|e| anyhow::anyhow!("run-store list failed: {e}"))?;

    // Lazy expiry: any AwaitingApproval row whose expires_at is in
    // the past gets transitioned to Failed and persisted before we
    // render. Operators learn about expired runs the next time they
    // look at the list.
    let now = chrono::Utc::now();
    for r in &mut all {
        let _ = store.expire_if_overdue(r, now);
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
        if let Some(p) = crate::pricing::lookup(pricing, &r.provider, &r.model, &r.agent) {
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
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

async fn show_run(run_id: &str, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = layered_config_workflow(&global, project_root.as_deref());
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None, None);
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
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
            cost_usd: crate::pricing::lookup(&cfg.pricing, &r.provider, &r.model, &r.agent)
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
    let output = WorkflowShowRunOutput {
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
    let _ = prefs;
    report::emit_event(global_format, &output)
}

async fn approve(run_id: &str, mode: Option<&str>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let runs_dir = global.join("runs");
    let store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));
    let approver = whoami::username();

    // Library call replaces inline load + expire-check + status check
    // + mutate + update. Re-entering run_workflow stays in the CLI
    // because the TUI uses a different resume model.
    let awaited_step_id = match store.approve(run_id, &approver, chrono::Utc::now()) {
        Ok(rupu_orchestrator::ApprovalDecision::Approved { step_id, .. }) => step_id,
        Err(rupu_orchestrator::ApprovalError::Expired(msg)) => {
            anyhow::bail!("approval expired before it was acted on — {msg}");
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
        Err(e) => return Err(anyhow::anyhow!("approve: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    };
    // Reload the record from disk to get inputs, event, workspace path
    // for the run_workflow re-entry. The library call already persisted
    // the status flip to Running, so the record is coherent.
    let record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("reload run record: {e}"))?;

    // Rebuild context from disk: workflow YAML snapshot + prior
    // step results.
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

    // Restore inputs, event, issue, workspace path from the record.
    let inputs_map: BTreeMap<String, String> = record.inputs.clone();
    let event = record.event.clone();
    let issue_payload = record.issue.clone();
    let issue_ref_text = record.issue_ref.clone();
    let workspace_path = record.workspace_path.clone();
    let transcripts = record.transcript_dir.clone();
    paths::ensure_dir(&transcripts)?;

    // Resolve project_root from the persisted workspace path so
    // agent/config discovery picks up the same `.rupu/` dir the
    // original run used.
    let project_root = paths::project_root_for(&workspace_path)?;

    // Standard wiring (mirrors `run` above; refactor candidate but
    // keeping inline for now to avoid spreading the resume path
    // across the CLI surface).
    let resolver = Arc::new(rupu_auth::KeychainResolver::new());
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let mcp_registry = Arc::new(rupu_scm::Registry::discover(resolver.as_ref(), &cfg).await);

    let mode_str = mode.unwrap_or("ask").to_string();
    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        project_root.clone(),
        record.workspace_id.clone(),
        workspace_path.clone(),
        Arc::clone(&resolver),
        mode_str.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&store),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;
    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: project_root.clone(),
        resolver,
        mode_str,
        mcp_registry,
        system_prompt_suffix: None,
        dispatcher: Some(dispatcher_dyn),
    });

    let resume = rupu_orchestrator::ResumeState {
        run_id: run_id.to_string(),
        prior_step_results,
        approved_step_id: awaited_step_id.clone(),
    };
    let event_sink_for_resume = {
        let runs_dir = global.join("runs");
        let events_path = runs_dir.join(run_id).join("events.jsonl");
        match rupu_orchestrator::executor::JsonlSink::create(&events_path) {
            Ok(sink) => Some(Arc::new(sink) as Arc<dyn rupu_orchestrator::executor::EventSink>),
            Err(e) => {
                tracing::warn!(error = %e, "failed to open events.jsonl for resume; continuing without event sink");
                None
            }
        }
    };

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
    };

    let result = run_workflow(opts).await?;
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

async fn reject(run_id: &str, reason: Option<&str>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
    let approver = whoami::username();
    let reason_str = reason.unwrap_or("rejected by operator");

    // Library call replaces inline load + expire-check + status check
    // + mutate + update.
    match store.reject(run_id, &approver, reason_str, chrono::Utc::now()) {
        Ok(rupu_orchestrator::ApprovalDecision::Rejected { .. }) => {}
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
        Err(e) => return Err(anyhow::anyhow!("reject: {e}")),
        Ok(other) => anyhow::bail!("unexpected decision: {other:?}"),
    }
    println!("rupu: run {run_id} marked rejected");
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
    pub use_canvas: bool,
    pub run_id_override: Option<String>,
    pub strict_templates: bool,
    pub run_envelope_template: Option<RunEnvelopeTemplate>,
    pub worker: Option<ExecutionWorkerContext>,
    pub live_event_hook: Option<crate::output::workflow_printer::LiveWorkflowEventHook>,
    pub shared_printer: Option<Arc<Mutex<crate::output::LineStreamPrinter>>>,
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
    run_with_outcome(name, None, inputs, mode, event, false, false, None).await
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
    run_with_outcome(name, None, inputs, mode, event, false, false, Some(run_id)).await
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
        false,
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
    run(name, Some(target), Vec::new(), mode, None, false).await
}

async fn run(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    use_canvas: bool,
) -> anyhow::Result<()> {
    run_with_outcome(name, target, inputs, mode, event, true, use_canvas, None)
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
    use_canvas: bool,
    run_id_override: Option<String>,
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
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;

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
            mode: mode.unwrap_or("ask").to_string(),
            invocation_source,
            event,
            issue: issue_payload,
            issue_ref: issue_ref_text,
            system_prompt_suffix,
            attach_ui,
            use_canvas,
            run_id_override,
            strict_templates: false,
            run_envelope_template: None,
            worker: None,
            live_event_hook: None,
            shared_printer: None,
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
    use_canvas: bool,
    run_id_override: Option<String>,
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
            mode: mode.unwrap_or("ask").to_string(),
            invocation_source,
            event,
            issue: None,
            issue_ref: None,
            system_prompt_suffix: None,
            attach_ui,
            use_canvas,
            run_id_override,
            strict_templates: false,
            run_envelope_template: None,
            worker: None,
            live_event_hook: None,
            shared_printer: None,
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
            use_canvas: ctx.use_canvas,
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
        rupu_orchestrator::RunStatus::Failed | rupu_orchestrator::RunStatus::Rejected => {
            RunResultStatus::Failed
        }
        rupu_orchestrator::RunStatus::Pending
        | rupu_orchestrator::RunStatus::Running
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
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;
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

    let dispatcher = crate::cmd::dispatch::CliAgentDispatcher::new(
        global.clone(),
        ctx.project_root.clone(),
        ctx.workspace_id.clone(),
        ctx.workspace_path.clone(),
        Arc::clone(&resolver),
        ctx.mode.clone(),
        Arc::clone(&mcp_registry),
        Arc::clone(&run_store),
    );
    let dispatcher_dyn: Arc<dyn rupu_tools::AgentDispatcher> = dispatcher;

    let factory = Arc::new(DefaultStepFactory {
        workflow: workflow.clone(),
        global: global.clone(),
        project_root: ctx.project_root.clone(),
        resolver,
        mode_str: ctx.mode.clone(),
        mcp_registry,
        system_prompt_suffix: ctx.system_prompt_suffix.clone(),
        dispatcher: Some(dispatcher_dyn),
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
    };

    let workflow_result = if ctx.attach_ui {
        let runner_task = tokio::spawn(run_workflow(opts));
        let rid = run_id.clone();

        if ctx.use_canvas {
            if let Err(e) = rupu_tui::run_attached(rid.clone(), runs_dir.clone()) {
                eprintln!("rupu: TUI exited early: {e}");
            }
            runner_task
                .await
                .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                .map_err(|e| to_anyhow_with_input_snippet(e, &path, &body))?
        } else {
            let mut attach_opts = crate::output::workflow_printer::AttachOpts {
                skip_header: false,
                skip_count: 0,
                live_event_hook: ctx.live_event_hook.clone(),
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
                    let printer_store =
                        rupu_orchestrator::RunStore::new(runs_dir_for_attach.clone());
                    if let Some(shared_printer) = shared_printer_for_attach {
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

                let result = current_runner
                    .await
                    .map_err(|e| anyhow::anyhow!("workflow task panicked: {e}"))?
                    .map_err(|e| to_anyhow_with_input_snippet(e, &path, &body))?;

                use crate::output::workflow_printer::AttachOutcome;
                match outcome {
                    AttachOutcome::Done | AttachOutcome::Detached | AttachOutcome::Rejected => {
                        break result;
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
                        let resume = rupu_orchestrator::ResumeState {
                            run_id: current_run_id.clone(),
                            prior_step_results,
                            approved_step_id: awaited_step_id,
                        };
                        let factory_dyn: Arc<dyn rupu_orchestrator::StepFactory> =
                            factory_for_resume.clone();
                        let resume_event_sink = {
                            let events_path = runs_dir.join(&current_run_id).join("events.jsonl");
                            match rupu_orchestrator::executor::JsonlSink::create(&events_path) {
                                Ok(sink) => Some(Arc::new(sink)
                                    as Arc<dyn rupu_orchestrator::executor::EventSink>),
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "failed to open events.jsonl for inline resume; continuing without event sink"
                                    );
                                    None
                                }
                            }
                        };
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
                        };
                        current_runner = tokio::spawn(run_workflow(resume_opts));
                        current_run_id = result.run_id.clone();
                        attach_opts = crate::output::workflow_printer::AttachOpts {
                            skip_header: true,
                            skip_count: prior_count,
                            live_event_hook: ctx.live_event_hook.clone(),
                        };
                        let _ = result;
                    }
                }
            }
        }
    } else {
        run_workflow(opts)
            .await
            .map_err(|e| to_anyhow_with_input_snippet(e, &path, &body))?
    };

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
