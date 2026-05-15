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
use crate::output::formats::OutputFormat;
use crate::output::palette::{self, BRAND, DIM, Status as UiStatus};
use crate::output::report::{self, CollectionOutput, DetailOutput, EventOutput};
use crate::output::printer::{visible_len, wrap_with_ansi};
use crate::paths;
use clap::Subcommand;
use clap_complete::ArgValueCompleter;
use rupu_app_canvas::{render_rows as render_graph_rows, GraphCell, NodeStatus};
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
        /// Control live output density (`focused` | `full`).
        #[arg(long, value_enum)]
        view: Option<LiveViewMode>,
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
    /// Cancel a running workflow run.
    Cancel {
        /// Full run id (`run_<ULID>`) as printed by
        /// `rupu workflow run`.
        run_id: String,
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
        Action::Run {
            name,
            target,
            input,
            mode,
            view,
        } => run(&name, target.as_deref(), input, mode.as_deref(), None, view).await,
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
        Action::Approve { run_id, mode } => approve(&run_id, mode.as_deref()).await,
        Action::Reject { run_id, reason } => reject(&run_id, reason.as_deref()).await,
        Action::Cancel { run_id } => cancel(&run_id).await,
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
        Action::Cancel { .. } => ("workflow cancel", report::TABLE_ONLY),
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
        let rendered = render_workflow_show_snapshot(
            &self.report.item,
            self.view_mode,
            &self.prefs,
            width,
        );
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

    let mut lines = vec![styled_usage_line(UiStatus::Active, "usage", "provider/model/agent usage")];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["PROVIDER", "MODEL", "AGENT", "INPUT", "OUTPUT", "CACHED", "COST"]);
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
    let _ =
        crate::output::palette::write_colored(&mut buf, detail, crate::output::palette::DIM);
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
    let mut rows = vec![render_workflow_show_header_line(item, view_mode, width), String::new()];

    match Workflow::parse(&item.body) {
        Ok(workflow) => {
            rows.extend(render_workflow_show_summary_rows(&workflow, item, width));
            rows.push(String::new());
            rows.push(render_workflow_show_section_header("graph", "workflow structure", width));
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
                rows.push(render_workflow_show_section_header("yaml", "raw definition", width));
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
            rows.push(render_workflow_show_section_header("yaml", "raw definition", width));
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
        render_workflow_show_kv_row("trigger", &workflow_trigger_summary(&workflow.trigger), width, UiStatus::Active),
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
        &collect_workflow_agents(workflow).into_iter().collect::<Vec<_>>().join(", "),
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

    let mut rows = vec![render_workflow_show_section_header("inputs", "declared inputs", width)];
    let mut table = crate::output::tables::new_table();
    table.set_header(vec!["NAME", "TYPE", "REQUIRED", "DEFAULT", "ENUM", "DESCRIPTION"]);
    for (name, input) in &workflow.inputs {
        table.add_row(vec![
            comfy_table::Cell::new(name),
            comfy_table::Cell::new(workflow_input_type_name(input.ty)),
            comfy_table::Cell::new(if input.required { "yes" } else { "no" }),
            comfy_table::Cell::new(
                input.default
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

    let mut rows = vec![render_workflow_show_section_header("outputs", "declared outputs", width)];
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
    let mut rows = vec![render_workflow_show_section_header("steps", "declared steps", width)];
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
                let _ = palette::write_colored(&mut buf, glyph.as_str(), node_status_color(*status));
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

fn render_workflow_show_kv_row(
    label: &str,
    value: &str,
    width: usize,
    status: UiStatus,
) -> String {
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
            if let Some(filter) = trigger.filter.as_deref().filter(|value| !value.trim().is_empty())
            {
                parts.push(format!("filter {}", crate::cmd::transcript::truncate_single_line(filter, 40)));
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
    if let Some(ttl) = autoflow.claim.as_ref().and_then(|claim| claim.ttl.as_deref()) {
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
        return ("parallel", primary, if detail.is_empty() { "—".into() } else { detail });
    }
    if let Some(panel) = &step.panel {
        let primary = format!("{} panelists", panel.panelists.len());
        let mut parts =
            vec![crate::cmd::transcript::truncate_single_line(&panel.panelists.join(", "), 40)];
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
        if step.approval.as_ref().is_some_and(|approval| approval.required) {
            parts.push("approval".into());
        }
        return ("for_each", primary, parts.join("  ·  "));
    }

    let primary = step.agent.clone().unwrap_or_default();
    let mut parts = Vec::new();
    if !step.actions.is_empty() {
        parts.push(format!("actions {}", step.actions.join(", ")));
    }
    if let Some(when) = step.when.as_deref().filter(|value| !value.trim().is_empty()) {
        parts.push(format!(
            "when {}",
            crate::cmd::transcript::truncate_single_line(when, 28)
        ));
    }
    if step.approval.as_ref().is_some_and(|approval| approval.required) {
        parts.push("approval".into());
    }
    if let Some(contract) = &step.contract {
        parts.push(format!(
            "emits {} ({})",
            contract.emits,
            workflow_contract_format_name(contract.format)
        ));
    }
    ("linear", primary, if parts.is_empty() { "—".into() } else { parts.join("  ·  ") })
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
    let runs_dir = global.join("runs");
    let store = rupu_orchestrator::RunStore::new(runs_dir.clone());
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

async fn cancel(run_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = rupu_orchestrator::RunStore::new(global.join("runs"));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelOutcome {
    RejectedAwaitingApproval,
    MarkedCancelled { pid: Option<u32>, was_running: bool },
}

fn cancel_with_store(
    store: &rupu_orchestrator::RunStore,
    run_id: &str,
    reason: &str,
) -> anyhow::Result<CancelOutcome> {
    let mut record = store
        .load(run_id)
        .map_err(|e| anyhow::anyhow!("load run record: {e}"))?;
    match record.status {
        rupu_orchestrator::RunStatus::Completed
        | rupu_orchestrator::RunStatus::Failed
        | rupu_orchestrator::RunStatus::Rejected => {
            anyhow::bail!(
                "run {run_id} is already terminal ({})",
                record.status.as_str()
            );
        }
        rupu_orchestrator::RunStatus::AwaitingApproval => {
            let approver = whoami::username();
            store
                .reject(run_id, &approver, reason, chrono::Utc::now())
                .map_err(|e| anyhow::anyhow!("cancel awaiting run: {e}"))?;
            Ok(CancelOutcome::RejectedAwaitingApproval)
        }
        rupu_orchestrator::RunStatus::Pending | rupu_orchestrator::RunStatus::Running => {
            let pid = record.runner_pid;
            let was_running = pid.is_some_and(pid_is_running);
            if let Some(pid) = pid.filter(|pid| pid_is_running(*pid)) {
                let _ = terminate_pid(pid);
            }
            record.status = rupu_orchestrator::RunStatus::Failed;
            record.finished_at = Some(chrono::Utc::now());
            record.error_message = Some(reason.to_string());
            record.awaiting_step_id = None;
            record.approval_prompt = None;
            record.awaiting_since = None;
            record.expires_at = None;
            record.runner_pid = None;
            record.active_step_id = None;
            record.active_step_kind = None;
            record.active_step_agent = None;
            record.active_step_transcript_path = None;
            store
                .update(&record)
                .map_err(|e| anyhow::anyhow!("persist cancelled run: {e}"))?;
            Ok(CancelOutcome::MarkedCancelled { pid, was_running })
        }
    }
}

fn pid_is_running(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn terminate_pid(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
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
    run_with_outcome(name, None, inputs, mode, event, false, None, None).await
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
    run_with_outcome(name, None, inputs, mode, event, false, Some(run_id), None).await
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
    run(name, Some(target), Vec::new(), mode, None, None).await
}

async fn run(
    name: &str,
    target: Option<&str>,
    inputs: Vec<(String, String)>,
    mode: Option<&str>,
    event: Option<serde_json::Value>,
    view: Option<LiveViewMode>,
) -> anyhow::Result<()> {
    run_with_outcome(name, target, inputs, mode, event, true, None, view)
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
            mode: mode.unwrap_or("ask").to_string(),
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
            mode: mode.unwrap_or("ask").to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::{RunRecord, RunStatus};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

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
            awaiting_step_id: Some("step_approve".into()),
            approval_prompt: Some("approve?".into()),
            awaiting_since: Some(Utc::now()),
            expires_at: None,
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
        }
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
        assert_eq!(persisted.status, RunStatus::Failed);
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
}
