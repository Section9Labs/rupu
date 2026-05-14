//! `rupu autoflow ...` — manual/autonomous workflow entrypoints.

use super::autoflow_wake::wake_requests_from_polled_event_for_repo;
use crate::cmd::completers::workflow_names;
use crate::cmd::issues::canonical_issue_ref as canonical_repo_issue_ref;
use crate::cmd::issues::{autodetect_repo_from_path, canonical_repo_ref};
use crate::cmd::transcript::truncate_single_line;
use crate::cmd::ui::UiPrefs;
use crate::cmd::workflow::{
    locate_workflow_in, run_with_explicit_context, ExecutionWorkerContext,
    ExplicitWorkflowRunContext, RunEnvelopeTemplate,
};
use crate::output::palette::{self, Status as UiStatus, BRAND, DIM};
use crate::output::printer::format_duration;
use crate::output::report::{self as output_report, CollectionOutput, DetailOutput};
use crate::output::workflow_printer::{
    LiveWorkflowEvent, LiveWorkflowEventHook, LiveWorkflowRender,
};
use crate::output::LineStreamPrinter;
use crate::paths;
use anyhow::{anyhow, bail, Context};
use clap::{Args as ClapArgs, Subcommand};
use clap_complete::ArgValueCompleter;
use comfy_table::Cell;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::style::Print;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use jsonschema::JSONSchema;
use rupu_auth::{CredentialResolver, KeychainResolver};
use rupu_config::{AutoflowCheckout, Config, PollSourceEntry};
use rupu_orchestrator::templates::{render_step_prompt, RenderMode, StepContext};
use rupu_orchestrator::{
    AutoflowWorkspaceStrategy, ContractFormat, RunStatus, RunStore, StepResultRecord, Workflow,
    WorkflowOutputContract,
};
use rupu_runtime::{
    AutoflowCycleEvent, AutoflowCycleRecord, AutoflowHistoryEventRecord, AutoflowHistoryStore,
    RunTriggerSource, WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeRecord,
    WakeSource, WakeStore, WakeStoreError,
};
use rupu_scm::{EventSourceRef, Issue, IssueFilter, IssueRef, IssueState, IssueTracker, Platform};
use rupu_transcript::event::{Event as TranscriptEvent, FileEditKind};
use rupu_transcript::reader::JsonlReader;
use rupu_workspace::autoflow_claim_store::issue_key;
use rupu_workspace::{
    ensure_issue_worktree, issue_dir_name, remove_issue_worktree, AutoflowClaimRecord,
    AutoflowClaimStore, AutoflowContender, ClaimStatus, PendingDispatch, RepoRegistryStore,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tracing::warn;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List autoflow-enabled workflows.
    List(RepoFilterArgs),
    /// Show one autoflow workflow and its resolved metadata.
    Show {
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Limit resolution to one tracked repo.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Execute one autonomous cycle for one issue target.
    Run {
        #[arg(add = ArgValueCompleter::new(workflow_names))]
        name: String,
        /// Issue target in full run-target form:
        /// `github:owner/repo/issues/42` or `gitlab:group/project/issues/9`.
        target: String,
        /// Limit resolution to one bound repo.
        #[arg(long)]
        repo: Option<String>,
        /// Override permission mode (`bypass` | `readonly`).
        #[arg(long)]
        mode: Option<String>,
    },
    /// Reconcile every discovered autoflow once.
    Tick,
    /// Run the autoflow reconciler as a long-lived local worker.
    Serve {
        /// Limit reconciliation to one tracked repo.
        #[arg(long)]
        repo: Option<String>,
        /// Optional worker name override, for example `team-mini-01`.
        #[arg(long)]
        worker: Option<String>,
        /// Idle sleep between reconciliation passes, for example `10s`.
        #[arg(long, default_value = "10s")]
        idle_sleep: String,
        /// Live operator view mode (`focused` or `full`).
        #[arg(long, value_enum)]
        view: Option<crate::cmd::ui::LiveViewMode>,
        /// Suppress the live interactive serve view.
        #[arg(long)]
        quiet: bool,
    },
    /// Inspect queued and recently processed wakes.
    Wakes(RepoFilterArgs),
    /// Show a live operator view across workers, claims, wakes, and recent activity.
    Monitor {
        /// Limit results to one repo target, for example `github:owner/repo`.
        #[arg(long)]
        repo: Option<String>,
        /// Limit results to one worker id or display name.
        #[arg(long)]
        worker: Option<String>,
        /// Refresh the table view until interrupted.
        #[arg(long)]
        watch: bool,
        /// Refresh interval for `--watch`, for example `2s`.
        #[arg(long, default_value = "2s")]
        interval: String,
    },
    /// Show durable autoflow cycle and event history.
    History {
        /// Optional issue ref filter, for example `linear:eng-team/issues/42`.
        r#ref: Option<String>,
        /// Limit results to one repo target, for example `github:owner/repo`.
        #[arg(long)]
        repo: Option<String>,
        /// Limit results to one source target, for example `linear:eng-team`.
        #[arg(long)]
        source: Option<String>,
        /// Limit results to one worker id or display name.
        #[arg(long)]
        worker: Option<String>,
        /// Limit results to one event kind, for example `run_launched`.
        #[arg(long)]
        event: Option<String>,
        /// Maximum number of history rows to show.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Refresh the table view until interrupted.
        #[arg(long)]
        watch: bool,
        /// Refresh interval for `--watch`, for example `2s`.
        #[arg(long, default_value = "2s")]
        interval: String,
    },
    /// Explain the current autonomous state for one issue.
    Explain {
        r#ref: String,
        /// Limit resolution to one bound repo.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Run consistency checks across claims, wakes, and runs.
    Doctor(RepoFilterArgs),
    /// Apply safe, bounded remediation to one issue claim.
    Repair {
        r#ref: String,
        /// Explicitly release the claim after repair.
        #[arg(long)]
        release: bool,
        /// Explicitly enqueue a follow-up wake after repair.
        #[arg(long)]
        requeue: bool,
    },
    /// Enqueue one manual wake for an issue.
    Requeue {
        r#ref: String,
        /// Override the synthetic event id.
        #[arg(long)]
        event: Option<String>,
        /// Delay wake visibility by a relative duration like `10m`.
        #[arg(long)]
        not_before: Option<String>,
    },
    /// Summarize persisted autoflow claim state.
    Status(RepoFilterArgs),
    /// Inspect persisted autoflow claims.
    Claims(RepoFilterArgs),
    /// Force-release one claim.
    Release { r#ref: String },
}

#[derive(ClapArgs, Debug, Clone, Default)]
pub struct RepoFilterArgs {
    /// Limit results to one repo target, for example `github:owner/repo`.
    #[arg(long)]
    pub repo: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedAutoflowWorkflow {
    pub(crate) scope: String,
    pub(crate) name: String,
    pub(crate) workflow: Workflow,
    pub(crate) workflow_path: PathBuf,
    pub(crate) project_root: Option<PathBuf>,
    pub(crate) repo_ref: String,
    pub(crate) preferred_checkout: PathBuf,
    pub(crate) cfg: Config,
}

impl ResolvedAutoflowWorkflow {
    fn autoflow(&self) -> anyhow::Result<&rupu_orchestrator::Autoflow> {
        self.workflow
            .autoflow
            .as_ref()
            .filter(|autoflow| autoflow.enabled)
            .ok_or_else(|| anyhow!("workflow `{}` is not autoflow-enabled", self.workflow.name))
    }
}

#[derive(Debug, Clone)]
struct VisibleAutoflowWorkflow {
    scope: String,
    name: String,
    workflow: Workflow,
    workflow_path: PathBuf,
    project_root: Option<PathBuf>,
    repo_ref: Option<String>,
    preferred_checkout: Option<PathBuf>,
}

impl VisibleAutoflowWorkflow {
    fn autoflow(&self) -> anyhow::Result<&rupu_orchestrator::Autoflow> {
        self.workflow
            .autoflow
            .as_ref()
            .filter(|autoflow| autoflow.enabled)
            .ok_or_else(|| anyhow!("workflow `{}` is not autoflow-enabled", self.workflow.name))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IssueMatch {
    pub(crate) resolved: ResolvedAutoflowWorkflow,
    pub(crate) issue: Issue,
    pub(crate) issue_ref_text: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AutoflowOutcomeDoc {
    status: AutoflowOutcomeStatus,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    dispatch: Option<DispatchDoc>,
    #[serde(default)]
    retry_after: Option<String>,
    #[serde(default)]
    pr_url: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    artifacts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum AutoflowOutcomeStatus {
    Continue,
    AwaitHuman,
    AwaitExternal,
    Retry,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct DispatchDoc {
    workflow: String,
    target: String,
    #[serde(default)]
    inputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WakeHints {
    pub(crate) by_issue: BTreeMap<String, BTreeSet<String>>,
    pub(crate) by_repo: BTreeMap<String, BTreeSet<String>>,
    pub(crate) total_polled_events: usize,
    pub(crate) total_webhook_events: usize,
    pub(crate) due_wake_ids: Vec<String>,
}

impl WakeHints {
    pub(crate) fn events_for(&self, issue_ref: &str, repo_ref: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        if let Some(events) = self.by_repo.get(repo_ref) {
            out.extend(events.iter().cloned());
        }
        if let Some(events) = self.by_issue.get(issue_ref) {
            out.extend(events.iter().cloned());
        }
        out
    }
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowListRow {
    name: String,
    scope: String,
    entity: String,
    source: String,
    priority: i32,
    repo: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<AutoflowListRow>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowWakeRow {
    wake_id: String,
    state: String,
    source: String,
    event: String,
    entity: String,
    not_before: String,
    repo: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowWakesReport {
    kind: &'static str,
    version: u8,
    rows: Vec<AutoflowWakeRow>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowStatusRow {
    status: String,
    count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowContestedRow {
    issue: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_cycle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_event: Option<String>,
    contenders: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowStatusReport {
    kind: &'static str,
    version: u8,
    rows: Vec<AutoflowStatusRow>,
    contested: Vec<AutoflowContestedRow>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowClaimRow {
    issue: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tracker: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    repo: String,
    workflow: String,
    priority: String,
    status: String,
    next: String,
    branch: String,
    pr: String,
    summary: String,
    contenders: String,
    workspace: String,
    last_cycle: String,
    last_event: String,
    last_run: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowClaimsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<AutoflowClaimRow>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowClaimCsvRow {
    issue: String,
    issue_display: String,
    tracker: String,
    state: String,
    source: String,
    repo: String,
    workflow: String,
    priority: String,
    status: String,
    next: String,
    branch: String,
    pr: String,
    summary: String,
    contenders: String,
    workspace: String,
    last_cycle: String,
    last_event: String,
    last_run: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowMonitorWorkerRow {
    worker: String,
    kind: String,
    last_seen: String,
    last_cycle: String,
    repo_scope: String,
    ran: usize,
    failed: usize,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowMonitorActivityRow {
    at: String,
    worker: String,
    event: String,
    issue: String,
    workflow: String,
    repo: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowMonitorWakeSummary {
    queued: usize,
    due: usize,
    processed_recent: usize,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowMonitorReport {
    kind: &'static str,
    version: u8,
    workers: Vec<AutoflowMonitorWorkerRow>,
    claims: Vec<AutoflowClaimRow>,
    activity: Vec<AutoflowMonitorActivityRow>,
    wakes: AutoflowMonitorWakeSummary,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowHistoryRow {
    at: String,
    cycle_id: String,
    mode: String,
    worker: String,
    event: String,
    issue: String,
    source: String,
    workflow: String,
    repo: String,
    run: String,
    wake: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowHistoryReport {
    kind: &'static str,
    version: u8,
    total_cycles: usize,
    total_events: usize,
    rows: Vec<AutoflowHistoryRow>,
}

struct AutoflowListOutput {
    prefs: UiPrefs,
    report: AutoflowListReport,
}

impl CollectionOutput for AutoflowListOutput {
    type JsonReport = AutoflowListReport;
    type CsvRow = AutoflowListRow;

    fn command_name(&self) -> &'static str {
        "autoflow list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["name", "scope", "entity", "source", "priority", "repo"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "Name", "Scope", "Entity", "Source", "Priority", "Repo",
        ]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.name),
                crate::output::tables::status_cell(&row.scope, &self.prefs),
                Cell::new(&row.entity),
                Cell::new(&row.source),
                Cell::new(row.priority),
                Cell::new(&row.repo),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

struct AutoflowWakesOutput {
    report: AutoflowWakesReport,
}

impl CollectionOutput for AutoflowWakesOutput {
    type JsonReport = AutoflowWakesReport;
    type CsvRow = AutoflowWakeRow;

    fn command_name(&self) -> &'static str {
        "autoflow wakes"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "wake_id",
            "state",
            "source",
            "event",
            "entity",
            "not_before",
            "repo",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "Wake",
            "State",
            "Source",
            "Event",
            "Entity",
            "Not Before",
            "Repo",
        ]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.wake_id),
                Cell::new(&row.state),
                Cell::new(&row.source),
                Cell::new(&row.event),
                Cell::new(&row.entity),
                Cell::new(compact_timestamp(&row.not_before)),
                Cell::new(&row.repo),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

struct AutoflowStatusOutput {
    prefs: UiPrefs,
    report: AutoflowStatusReport,
}

impl CollectionOutput for AutoflowStatusOutput {
    type JsonReport = AutoflowStatusReport;
    type CsvRow = AutoflowStatusRow;

    fn command_name(&self) -> &'static str {
        "autoflow status"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["status", "count"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["Status", "Count"]);
        for row in &self.report.rows {
            table.add_row(vec![
                crate::output::tables::status_cell(&row.status, &self.prefs),
                Cell::new(row.count),
            ]);
        }
        println!("{table}");
        if !self.report.contested.is_empty() {
            println!();
            let mut contested_table = crate::output::tables::new_table();
            contested_table.set_header(vec![
                "Issue",
                "Source",
                "State",
                "Repo",
                "Recent",
                "Contenders",
            ]);
            for claim in &self.report.contested {
                let recent = match (claim.last_event.as_deref(), claim.last_cycle.as_deref()) {
                    (Some(event), Some(at)) => format!("{event} @ {}", compact_timestamp(at)),
                    (Some(event), None) => event.to_string(),
                    _ => "-".into(),
                };
                contested_table.add_row(vec![
                    Cell::new(claim.issue_display.as_deref().unwrap_or(&claim.issue)),
                    Cell::new(
                        claim
                            .source
                            .as_deref()
                            .or(claim.tracker.as_deref())
                            .unwrap_or("-"),
                    ),
                    Cell::new(claim.state.as_deref().unwrap_or("-")),
                    Cell::new(&claim.repo),
                    Cell::new(recent),
                    Cell::new(&claim.contenders),
                ]);
            }
            println!("{contested_table}");
        }
        Ok(())
    }
}

struct AutoflowClaimsOutput {
    prefs: UiPrefs,
    report: AutoflowClaimsReport,
    csv_rows: Vec<AutoflowClaimCsvRow>,
}

impl CollectionOutput for AutoflowClaimsOutput {
    type JsonReport = AutoflowClaimsReport;
    type CsvRow = AutoflowClaimCsvRow;

    fn command_name(&self) -> &'static str {
        "autoflow claims"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "issue",
            "issue_display",
            "tracker",
            "state",
            "source",
            "repo",
            "workflow",
            "priority",
            "status",
            "next",
            "branch",
            "pr",
            "summary",
            "contenders",
            "workspace",
            "last_cycle",
            "last_event",
            "last_run",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "Issue", "Source", "State", "Workflow", "Status", "Next", "Run", "Repo", "Branch",
            "Summary",
        ]);
        for claim in &self.report.rows {
            table.add_row(vec![
                Cell::new(claim.issue_display.as_deref().unwrap_or(&claim.issue)),
                Cell::new(
                    claim
                        .source
                        .as_deref()
                        .or(claim.tracker.as_deref())
                        .unwrap_or("-"),
                ),
                Cell::new(claim.state.as_deref().unwrap_or("-")),
                Cell::new(&claim.workflow),
                crate::output::tables::status_cell(&claim.status, &self.prefs),
                Cell::new(&claim.next),
                Cell::new(&claim.last_run),
                Cell::new(&claim.repo),
                Cell::new(&claim.branch),
                Cell::new(truncate_text(&claim.summary, 56)),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

struct AutoflowMonitorOutput {
    report: AutoflowMonitorReport,
}

impl CollectionOutput for AutoflowMonitorOutput {
    type JsonReport = AutoflowMonitorReport;
    type CsvRow = AutoflowMonitorActivityRow;

    fn command_name(&self) -> &'static str {
        "autoflow monitor"
    }

    fn supported_formats(&self) -> &'static [crate::output::formats::OutputFormat] {
        output_report::TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.activity
    }

    fn render_table(&self) -> anyhow::Result<()> {
        render_monitor_snapshot(&self.report, false)
    }
}

struct AutoflowHistoryOutput {
    report: AutoflowHistoryReport,
}

impl CollectionOutput for AutoflowHistoryOutput {
    type JsonReport = AutoflowHistoryReport;
    type CsvRow = AutoflowHistoryRow;

    fn command_name(&self) -> &'static str {
        "autoflow history"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "at", "cycle_id", "mode", "worker", "event", "issue", "source", "workflow", "repo",
            "run", "wake", "detail",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        render_history_table(&self.report)
    }
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowDoctorRow {
    scope: String,
    problem: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowDoctorReport {
    kind: &'static str,
    version: u8,
    ok: bool,
    rows: Vec<AutoflowDoctorRow>,
}

struct AutoflowDoctorOutput {
    report: AutoflowDoctorReport,
}

impl CollectionOutput for AutoflowDoctorOutput {
    type JsonReport = AutoflowDoctorReport;
    type CsvRow = AutoflowDoctorRow;

    fn command_name(&self) -> &'static str {
        "autoflow doctor"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["scope", "problem", "detail"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        if self.report.rows.is_empty() {
            println!("autoflow doctor: ok");
            return Ok(());
        }

        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["Scope", "Problem", "Detail"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.scope),
                Cell::new(&row.problem),
                Cell::new(&row.detail),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowShowItem {
    name: String,
    scope: String,
    path: String,
    repo: Option<String>,
    project_root: Option<String>,
    preferred_checkout: Option<String>,
    description: Option<String>,
    entity: String,
    source: String,
    priority: i32,
    workspace: String,
    workspace_branch: Option<String>,
    reconcile_every: Option<String>,
    claim_ttl: Option<String>,
    outcome_output: Option<String>,
    wake_on: Vec<String>,
    labels_all: Vec<String>,
    labels_any: Vec<String>,
    labels_none: Vec<String>,
    selector_states: Vec<String>,
    selector_limit: Option<u32>,
    body: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowShowReport {
    kind: &'static str,
    version: u8,
    item: AutoflowShowItem,
}

struct AutoflowShowOutput {
    prefs: UiPrefs,
    report: AutoflowShowReport,
    rendered: String,
}

impl DetailOutput for AutoflowShowOutput {
    type JsonReport = AutoflowShowReport;

    fn command_name(&self) -> &'static str {
        "autoflow show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        crate::cmd::ui::paginate(&self.rendered, &self.prefs)
    }
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowExplainLock {
    owner: String,
    acquired_at: String,
    lease_expires: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowExplainPendingDispatch {
    workflow: String,
    target: String,
    inputs: String,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowExplainRun {
    run_id: String,
    status: String,
    backend: String,
    worker: String,
    source_wake: Option<String>,
    approval_step: Option<String>,
    approval_expires: Option<String>,
    execution: Option<String>,
    models: Option<String>,
    usage: Option<String>,
    changes: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    workspace: Option<String>,
    merge_target: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowExplainClaim {
    issue_display: Option<String>,
    issue_title: Option<String>,
    issue_tracker: Option<String>,
    issue_state: Option<String>,
    source: Option<String>,
    issue_url: Option<String>,
    workflow: String,
    status: String,
    priority: Option<String>,
    contenders: String,
    workspace: String,
    branch: String,
    pr: String,
    claim_owner: String,
    lease_expires: String,
    active_lock: Option<AutoflowExplainLock>,
    last_run: Option<AutoflowExplainRun>,
    next_action: String,
    pending_dispatch: Option<AutoflowExplainPendingDispatch>,
    next_retry: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowExplainItem {
    issue: String,
    repo: String,
    status: String,
    source: Option<String>,
    candidate_workflows: Vec<String>,
    claim: Option<AutoflowExplainClaim>,
    queued_wakes: Vec<AutoflowWakeRow>,
    recent_processed_wake: Option<AutoflowWakeRow>,
    recent_cycle_events: Vec<AutoflowHistoryRow>,
}

#[derive(Debug, Clone, Serialize)]
struct AutoflowExplainReport {
    kind: &'static str,
    version: u8,
    item: AutoflowExplainItem,
}

struct AutoflowExplainOutput {
    report: AutoflowExplainReport,
    rendered: String,
}

impl DetailOutput for AutoflowExplainOutput {
    type JsonReport = AutoflowExplainReport;

    fn command_name(&self) -> &'static str {
        "autoflow explain"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        print!("{}", self.rendered);
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct ServeProgressSnapshot {
    cycles: usize,
    total: crate::cmd::autoflow_runtime::TickReport,
    last_cycle_rendered_at: Option<std::time::Instant>,
    cycle_running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AutoflowServeViewLine {
    status: UiStatus,
    text: String,
}

struct AutoflowServeScreenGuard;

impl AutoflowServeScreenGuard {
    fn enter() -> anyhow::Result<Self> {
        execute!(std::io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for AutoflowServeScreenGuard {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
    }
}

#[derive(Debug, Clone)]
struct AutoflowRunStepSummary {
    step_id: String,
    status: UiStatus,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
struct AutoflowUsageSummary {
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    total_tokens: u64,
    cost_usd: Option<f64>,
    cost_partial: bool,
}

#[derive(Debug, Clone)]
struct AutoflowWorkspaceDiffSummary {
    files_changed: usize,
    created: usize,
    modified: usize,
    deleted: usize,
    renamed: usize,
    insertions: u64,
    deletions: u64,
    top_files: Vec<String>,
    merge_target: Option<String>,
}

#[derive(Debug, Clone)]
struct AutoflowRunSummary {
    run_id: String,
    workflow: String,
    status: String,
    awaiting_step: Option<String>,
    error: Option<String>,
    worker: Option<String>,
    backend: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    finished_at: Option<chrono::DateTime<chrono::Utc>>,
    duration_ms: Option<u64>,
    workspace: Option<String>,
    agents: Vec<String>,
    providers: Vec<String>,
    models: Vec<String>,
    assistant_messages: u64,
    tool_calls: u64,
    command_runs: u64,
    actions_emitted: u64,
    file_edit_events: u64,
    file_creates: u64,
    file_modifies: u64,
    file_deletes: u64,
    usage: Option<AutoflowUsageSummary>,
    diff: Option<AutoflowWorkspaceDiffSummary>,
    steps: Vec<AutoflowRunStepSummary>,
}

#[derive(Debug, Clone, Default)]
struct AutoflowCostTally {
    sum_usd: f64,
    priced_items: u64,
    unpriced_items: u64,
}

impl AutoflowCostTally {
    fn add(&mut self, cost_usd: Option<f64>) {
        match cost_usd {
            Some(value) => {
                self.sum_usd += value;
                self.priced_items += 1;
            }
            None => self.unpriced_items += 1,
        }
    }

    fn cost_usd(&self) -> Option<f64> {
        (self.priced_items > 0).then_some(self.sum_usd)
    }

    fn partial(&self) -> bool {
        self.priced_items > 0 && self.unpriced_items > 0
    }
}

#[derive(Debug, Clone, Default)]
struct TranscriptUsageAccumulator {
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cost: AutoflowCostTally,
}

#[derive(Debug, Clone, Default)]
struct TranscriptSummaryAccumulator {
    agents: BTreeSet<String>,
    providers: BTreeSet<String>,
    models: BTreeSet<String>,
    assistant_messages: u64,
    tool_calls: u64,
    command_runs: u64,
    actions_emitted: u64,
    file_edit_events: u64,
    file_creates: u64,
    file_modifies: u64,
    file_deletes: u64,
    usage: TranscriptUsageAccumulator,
}

#[derive(Debug, Clone)]
struct RecentIssueActivity {
    at: String,
    event: String,
    run_id: Option<String>,
}

struct AutoflowHistoryQuery<'a> {
    issue_filter: Option<&'a str>,
    repo_filter: Option<&'a str>,
    source_filter: Option<&'a str>,
    worker_filter: Option<&'a str>,
    event_filter: Option<&'a str>,
    limit: usize,
}

pub async fn handle(
    action: Action,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> ExitCode {
    let resolver: Arc<dyn CredentialResolver> = Arc::new(KeychainResolver::new());
    let result = handle_with_resolver(action, resolver, global_format).await;
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn handle_with_resolver(
    action: Action,
    resolver: Arc<dyn CredentialResolver>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    match action {
        Action::List(args) => list(args.repo.as_deref(), global_format).await,
        Action::Show { name, repo } => show(&name, repo.as_deref(), global_format).await,
        Action::Run {
            name,
            target,
            repo,
            mode,
        } => run(&name, &target, repo.as_deref(), mode.as_deref(), resolver).await,
        Action::Tick => tick_with_resolver(resolver).await,
        Action::Serve {
            repo,
            worker,
            idle_sleep,
            view,
            quiet,
        } => {
            serve(
                repo.as_deref(),
                worker.as_deref(),
                &idle_sleep,
                view,
                quiet,
                resolver,
            )
            .await
        }
        Action::Wakes(args) => wakes(args.repo.as_deref(), global_format).await,
        Action::Monitor {
            repo,
            worker,
            watch,
            interval,
        } => {
            monitor(
                repo.as_deref(),
                worker.as_deref(),
                watch,
                &interval,
                global_format,
            )
            .await
        }
        Action::History {
            r#ref,
            repo,
            source,
            worker,
            event,
            limit,
            watch,
            interval,
        } => {
            let query = AutoflowHistoryQuery {
                issue_filter: r#ref.as_deref(),
                repo_filter: repo.as_deref(),
                source_filter: source.as_deref(),
                worker_filter: worker.as_deref(),
                event_filter: event.as_deref(),
                limit,
            };
            history(query, watch, &interval, global_format).await
        }
        Action::Explain { r#ref, repo } => explain(&r#ref, repo.as_deref(), global_format).await,
        Action::Doctor(args) => doctor(args.repo.as_deref(), global_format).await,
        Action::Repair {
            r#ref,
            release,
            requeue,
        } => repair(&r#ref, release, requeue).await,
        Action::Requeue {
            r#ref,
            event,
            not_before,
        } => requeue(&r#ref, event.as_deref(), not_before.as_deref()).await,
        Action::Status(args) => status(args.repo.as_deref(), global_format).await,
        Action::Claims(args) => claims(args.repo.as_deref(), global_format).await,
        Action::Release { r#ref } => release(&r#ref).await,
    }
}

pub fn ensure_output_format(
    action: &Action,
    format: crate::output::formats::OutputFormat,
) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List(_) => ("autoflow list", output_report::TABLE_JSON_CSV),
        Action::Show { .. } => ("autoflow show", output_report::TABLE_JSON),
        Action::Run { .. } => ("autoflow run", output_report::TABLE_ONLY),
        Action::Tick => ("autoflow tick", output_report::TABLE_ONLY),
        Action::Serve { .. } => ("autoflow serve", output_report::TABLE_ONLY),
        Action::Wakes(_) => ("autoflow wakes", output_report::TABLE_JSON_CSV),
        Action::Monitor { watch, .. } => {
            if *watch {
                ("autoflow monitor", output_report::TABLE_ONLY)
            } else {
                ("autoflow monitor", output_report::TABLE_JSON)
            }
        }
        Action::History { watch, .. } => {
            if *watch {
                ("autoflow history", output_report::TABLE_ONLY)
            } else {
                ("autoflow history", output_report::TABLE_JSON_CSV)
            }
        }
        Action::Explain { .. } => ("autoflow explain", output_report::TABLE_JSON),
        Action::Doctor(_) => ("autoflow doctor", output_report::TABLE_JSON_CSV),
        Action::Repair { .. } => ("autoflow repair", output_report::TABLE_ONLY),
        Action::Requeue { .. } => ("autoflow requeue", output_report::TABLE_ONLY),
        Action::Status(_) => ("autoflow status", output_report::TABLE_JSON_CSV),
        Action::Claims(_) => ("autoflow claims", output_report::TABLE_JSON_CSV),
        Action::Release { .. } => ("autoflow release", output_report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

async fn list(
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let repo_filter = normalize_repo_filter(repo)?;
    let entries = visible_autoflows()?;
    let entries = filter_visible_autoflows(entries, repo_filter.as_deref());
    if entries.is_empty()
        && matches!(
            global_format.unwrap_or(crate::output::formats::OutputFormat::Table),
            crate::output::formats::OutputFormat::Table
        )
    {
        println!("(no autoflows)");
        return Ok(());
    }
    let rows = entries
        .iter()
        .map(|entry| {
            let autoflow = entry.autoflow()?;
            Ok(AutoflowListRow {
                name: entry.name.clone(),
                scope: entry.scope.clone(),
                entity: match autoflow.entity {
                    rupu_orchestrator::AutoflowEntity::Issue => "issue".into(),
                },
                source: autoflow.source.clone().unwrap_or_else(|| "-".into()),
                priority: autoflow.priority,
                repo: entry.repo_ref.clone().unwrap_or_else(|| "-".into()),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let prefs = autoflow_ui_prefs()?;
    let output = AutoflowListOutput {
        prefs,
        report: AutoflowListReport {
            kind: "autoflow_list",
            version: 1,
            rows,
        },
    };
    output_report::emit_collection(global_format, &output)
}

fn build_autoflow_show_output(
    entry: &VisibleAutoflowWorkflow,
    body: String,
) -> anyhow::Result<AutoflowShowOutput> {
    let autoflow = entry.autoflow()?;
    let prefs = autoflow_ui_prefs()?;
    let rendered_yaml = crate::cmd::ui::highlight_yaml(&body, &prefs);
    let rendered = format!(
        "{}\n\n{}\n",
        render_autoflow_show_summary(entry, autoflow),
        rendered_yaml
    );
    let selector_states = autoflow
        .selector
        .states
        .iter()
        .map(|state| match state {
            rupu_orchestrator::AutoflowIssueState::Open => "open".to_string(),
            rupu_orchestrator::AutoflowIssueState::Closed => "closed".to_string(),
        })
        .collect::<Vec<_>>();
    let item = AutoflowShowItem {
        name: entry.name.clone(),
        scope: entry.scope.clone(),
        path: entry.workflow_path.display().to_string(),
        repo: entry.repo_ref.clone(),
        project_root: entry
            .project_root
            .as_ref()
            .map(|path| path.display().to_string()),
        preferred_checkout: entry
            .preferred_checkout
            .as_ref()
            .map(|path| path.display().to_string()),
        description: entry.workflow.description.clone(),
        entity: match autoflow.entity {
            rupu_orchestrator::AutoflowEntity::Issue => "issue".to_string(),
        },
        source: autoflow
            .source
            .clone()
            .or_else(|| entry.repo_ref.clone())
            .unwrap_or_else(|| "-".to_string()),
        priority: autoflow.priority,
        workspace: autoflow
            .workspace
            .as_ref()
            .map(|workspace| match workspace.strategy {
                AutoflowWorkspaceStrategy::Worktree => "worktree".to_string(),
                AutoflowWorkspaceStrategy::InPlace => "in_place".to_string(),
            })
            .unwrap_or_else(|| "worktree".to_string()),
        workspace_branch: autoflow
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.branch.clone()),
        reconcile_every: autoflow.reconcile_every.clone(),
        claim_ttl: autoflow.claim.as_ref().and_then(|claim| claim.ttl.clone()),
        outcome_output: autoflow
            .outcome
            .as_ref()
            .map(|outcome| outcome.output.clone()),
        wake_on: autoflow.wake_on.clone(),
        labels_all: autoflow.selector.labels_all.clone(),
        labels_any: autoflow.selector.labels_any.clone(),
        labels_none: autoflow.selector.labels_none.clone(),
        selector_states,
        selector_limit: autoflow.selector.limit,
        body,
    };
    Ok(AutoflowShowOutput {
        prefs,
        report: AutoflowShowReport {
            kind: "autoflow_show",
            version: 1,
            item,
        },
        rendered,
    })
}

async fn show(
    name: &str,
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let matches = visible_autoflow_matches(name, repo)?;
    let entry = match matches.as_slice() {
        [] => match repo {
            Some(repo) => bail!("no autoflow named `{name}` is visible for repo `{repo}`"),
            None => bail!("no autoflow named `{name}` is visible"),
        },
        [entry] => entry,
        many => {
            let mut options = many
                .iter()
                .map(|entry| {
                    format!(
                        "- {} ({}, {})",
                        entry.repo_ref.as_deref().unwrap_or("-"),
                        entry.scope,
                        entry.workflow_path.display()
                    )
                })
                .collect::<Vec<_>>();
            options.sort();
            bail!(
                "multiple autoflows named `{name}` are visible:\n{}\npass `--repo <platform>:<owner>/<repo>` to disambiguate",
                options.join("\n")
            );
        }
    };
    let body = std::fs::read_to_string(&entry.workflow_path)?;
    let output = build_autoflow_show_output(entry, body)?;
    output_report::emit_detail(global_format, &output)
}

async fn run(
    name: &str,
    target: &str,
    repo: Option<&str>,
    mode: Option<&str>,
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<()> {
    let issue_ref = parse_full_issue_target(target)?;
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let resolved = resolve_autoflow_workflow_for_issue(&global, name, &issue_ref, repo)?;
    let _ =
        resolve_autoflow_permission_mode(mode, resolved.cfg.autoflow.permission_mode.as_deref())?;
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let fetch_ref = issue_ref_for_autoflow(&issue_ref, &resolved)?;
    let issue = fetch_issue(&resolved.cfg, resolver.as_ref(), &fetch_ref).await?;
    let issue_ref_text = format_issue_ref(&issue.r);
    ensure_manual_run_can_take_claim(&claim_store, &issue_ref_text)?;
    execute_autoflow_cycle(
        &global,
        &claim_store,
        &resolved,
        &issue,
        &issue_ref_text,
        mode,
        true,
        BTreeMap::new(),
        vec![AutoflowContender {
            workflow: resolved.workflow.name.clone(),
            priority: resolved.autoflow()?.priority,
            scope: Some(resolved.scope.clone()),
            selected: true,
        }],
        None,
        None,
        None,
    )
    .await
}

fn ensure_manual_run_can_take_claim(
    claim_store: &AutoflowClaimStore,
    issue_ref_text: &str,
) -> anyhow::Result<()> {
    let Some(claim) = claim_store.load(issue_ref_text)? else {
        return Ok(());
    };
    if matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released) {
        return Ok(());
    }
    if claim.status == ClaimStatus::Blocked {
        bail!(
            "issue `{issue_ref_text}` has a blocked autoflow claim for workflow `{}`; release it first with `rupu autoflow release {issue_ref_text}`",
            claim.workflow
        );
    }
    if claim_store.read_active_lock(issue_ref_text)?.is_some() {
        bail!(
            "issue `{issue_ref_text}` already has an active autoflow cycle for workflow `{}`",
            claim.workflow
        );
    }
    if !claim_lease_expired(&claim)? {
        bail!(
            "issue `{issue_ref_text}` already has an owned autoflow claim for workflow `{}` with status `{}`; wait for it, run `rupu autoflow tick`, or release it first",
            claim.workflow,
            status_name(claim.status)
        );
    }
    Ok(())
}

async fn tick_with_resolver(resolver: Arc<dyn CredentialResolver>) -> anyhow::Result<()> {
    let report = crate::cmd::autoflow_runtime::tick_with_resolver(resolver).await?;
    if report.workflow_count == 0 {
        if report.cleaned_claims == 0 {
            println!("(no autoflows)");
        } else {
            println!(
                "autoflow tick: 0 workflow(s), 0 polled event(s), 0 webhook event(s), 0 cycle(s) ran, 0 skipped, {} cleaned",
                report.cleaned_claims
            );
        }
        return Ok(());
    }
    println!(
        "autoflow tick: {} workflow(s), {} polled event(s), {} webhook event(s), {} cycle(s) ran, {} skipped, {} failed, {} cleaned",
        report.workflow_count,
        report.polled_event_count,
        report.webhook_event_count,
        report.ran_cycles,
        report.skipped_cycles,
        report.failed_cycles,
        report.cleaned_claims
    );
    Ok(())
}

async fn serve(
    repo: Option<&str>,
    worker: Option<&str>,
    idle_sleep: &str,
    view: Option<crate::cmd::ui::LiveViewMode>,
    quiet: bool,
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<()> {
    let cfg = autoflow_ui_config().unwrap_or_default();
    let prefs = UiPrefs::resolve(&cfg.ui, false, None, None, view);
    let view_mode = prefs.live_view;
    let repo_filter = normalize_repo_filter(repo)?;
    let idle_sleep = parse_duration(idle_sleep)?
        .to_std()
        .map_err(|_| anyhow!("idle sleep must be non-negative"))?;
    let live_view = std::io::stdout().is_terminal() && !quiet;
    if live_view {
        return serve_retained(
            repo_filter.as_deref(),
            worker,
            idle_sleep,
            view_mode,
            resolver,
        )
        .await;
    }

    let live_printer = live_view.then(|| Arc::new(Mutex::new(LineStreamPrinter::new())));
    let progress = Arc::new(Mutex::new(ServeProgressSnapshot::default()));
    let progress_capture = Arc::clone(&progress);
    let progress_for_cycle_start = Arc::clone(&progress);
    let repo_filter_for_task = repo_filter.clone();
    let worker_name = worker.map(ToOwned::to_owned);
    let worker_filter_for_task = worker_name.clone();
    let resolver_for_task = Arc::clone(&resolver);
    let live_printer_for_task = live_printer.clone();
    let live_printer_for_cycle_start = live_printer.clone();
    let mut idle_cycles = 0usize;
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(1));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    if let Some(printer) = &live_printer {
        update_serve_ticker(
            printer,
            repo_filter.as_deref(),
            worker,
            Arc::clone(&progress),
        )?;
    }

    let mut serve_task = tokio::spawn(async move {
        crate::cmd::autoflow_runtime::serve_with_resolver_and_hooks(
            resolver_for_task,
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: repo_filter_for_task.clone(),
                worker_name: worker_name.clone(),
                idle_sleep,
                max_cycles: None,
                shared_printer: live_printer_for_task.clone(),
                attach_workflow_ui: true,
            },
            move || {
                if let Ok(mut snapshot) = progress_for_cycle_start.lock() {
                    snapshot.cycle_running = true;
                }
                if !live_view {
                    return Ok(());
                }
                if let Some(printer) = &live_printer_for_cycle_start {
                    if let Ok(mut printer) = printer.lock() {
                        printer.stop_ticker();
                    }
                }
                Ok(())
            },
            move |report, last_tick, cycle| {
                if let Ok(mut snapshot) = progress_capture.lock() {
                    snapshot.cycles = report.cycles;
                    snapshot.total = report.total.clone();
                    snapshot.last_cycle_rendered_at = Some(std::time::Instant::now());
                    snapshot.cycle_running = false;
                }
                if !live_view {
                    return Ok(());
                }
                if let Some(printer) = &live_printer_for_task {
                    if let Ok(mut printer) = printer.lock() {
                        printer.stop_ticker();
                    }
                }
                render_serve_cycle_timeline(
                    report,
                    last_tick,
                    cycle,
                    repo_filter_for_task.as_deref(),
                    worker_filter_for_task.as_deref(),
                    &mut idle_cycles,
                )?;
                if let Some(printer) = &live_printer_for_task {
                    update_serve_ticker(
                        printer,
                        repo_filter_for_task.as_deref(),
                        worker_filter_for_task.as_deref(),
                        Arc::clone(&progress_capture),
                    )?;
                }
                Ok(())
            },
        )
        .await
    });

    loop {
        tokio::select! {
            result = &mut serve_task => {
                let report = result.map_err(|error| anyhow!("autoflow serve task failed: {error}"))??;
                if let Some(printer) = &live_printer {
                    if let Ok(mut printer) = printer.lock() {
                        printer.stop_ticker();
                    }
                }
                if live_view {
                    println!();
                }
                println!(
                    "autoflow serve stopped after {} cycle(s): ran={} skipped={} failed={} cleaned={}",
                    report.cycles,
                    report.total.ran_cycles,
                    report.total.skipped_cycles,
                    report.total.failed_cycles,
                    report.total.cleaned_claims
                );
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                serve_task.abort();
                let _ = serve_task.await;
                let snapshot = progress.lock().map(|guard| guard.clone()).unwrap_or_default();
                if let Some(printer) = &live_printer {
                    if let Ok(mut printer) = printer.lock() {
                        printer.stop_ticker();
                    }
                }
                if live_view {
                    println!();
                }
                println!(
                    "autoflow serve interrupted after {} cycle(s): ran={} skipped={} failed={} cleaned={}",
                    snapshot.cycles,
                    snapshot.total.ran_cycles,
                    snapshot.total.skipped_cycles,
                    snapshot.total.failed_cycles,
                    snapshot.total.cleaned_claims
                );
                break;
            }
            _ = heartbeat.tick(), if live_view => {
                if let Some(printer) = &live_printer {
                    update_serve_ticker(
                        printer,
                        repo_filter.as_deref(),
                        worker,
                        Arc::clone(&progress),
                    )?;
                }
            }
        }
    }
    Ok(())
}

enum RetainedServeExit {
    Completed(crate::cmd::autoflow_runtime::ServeReport),
    Interrupted(ServeProgressSnapshot),
}

async fn serve_retained(
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
    idle_sleep: std::time::Duration,
    view_mode: crate::cmd::ui::LiveViewMode,
    resolver: Arc<dyn CredentialResolver>,
) -> anyhow::Result<()> {
    let progress = Arc::new(Mutex::new(ServeProgressSnapshot::default()));
    let progress_capture = Arc::clone(&progress);
    let progress_for_cycle_start = Arc::clone(&progress);
    let repo_filter_for_task = repo_filter.map(ToOwned::to_owned);
    let worker_name = worker_filter.map(ToOwned::to_owned);
    let resolver_for_task = Arc::clone(&resolver);

    let mut serve_task = tokio::spawn(async move {
        crate::cmd::autoflow_runtime::serve_with_resolver_and_hooks(
            resolver_for_task,
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: repo_filter_for_task.clone(),
                worker_name: worker_name.clone(),
                idle_sleep,
                max_cycles: None,
                shared_printer: None,
                attach_workflow_ui: false,
            },
            move || {
                if let Ok(mut snapshot) = progress_for_cycle_start.lock() {
                    snapshot.cycle_running = true;
                }
                Ok(())
            },
            move |report, _last_tick, _cycle| {
                if let Ok(mut snapshot) = progress_capture.lock() {
                    snapshot.cycles = report.cycles;
                    snapshot.total = report.total.clone();
                    snapshot.last_cycle_rendered_at = Some(std::time::Instant::now());
                    snapshot.cycle_running = false;
                }
                Ok(())
            },
        )
        .await
    });

    let exit = {
        let _screen = AutoflowServeScreenGuard::enter()?;
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_millis(250));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_rows: Vec<String> = Vec::new();

        loop {
            tokio::select! {
                result = &mut serve_task => {
                    let report = result
                        .map_err(|error| anyhow!("autoflow serve task failed: {error}"))??;
                    break RetainedServeExit::Completed(report);
                }
                _ = tokio::signal::ctrl_c() => {
                    serve_task.abort();
                    let _ = serve_task.await;
                    let snapshot = progress.lock().map(|guard| guard.clone()).unwrap_or_default();
                    break RetainedServeExit::Interrupted(snapshot);
                }
                _ = heartbeat.tick() => {
                    let snapshot = progress.lock().map(|guard| guard.clone()).unwrap_or_default();
                    let rows = build_retained_serve_rows(
                        repo_filter,
                        worker_filter,
                        idle_sleep,
                        view_mode,
                        &snapshot,
                    )?;
                    if rows != last_rows {
                        render_retained_serve_rows(&rows)?;
                        last_rows = rows;
                    }
                }
            }
        }
    };

    match exit {
        RetainedServeExit::Completed(report) => {
            println!(
                "autoflow serve stopped after {} cycle(s): ran={} skipped={} failed={} cleaned={}",
                report.cycles,
                report.total.ran_cycles,
                report.total.skipped_cycles,
                report.total.failed_cycles,
                report.total.cleaned_claims
            );
        }
        RetainedServeExit::Interrupted(snapshot) => {
            println!(
                "autoflow serve interrupted after {} cycle(s): ran={} skipped={} failed={} cleaned={}",
                snapshot.cycles,
                snapshot.total.ran_cycles,
                snapshot.total.skipped_cycles,
                snapshot.total.failed_cycles,
                snapshot.total.cleaned_claims
            );
        }
    }

    Ok(())
}

fn build_retained_serve_rows(
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
    idle_sleep: std::time::Duration,
    view_mode: crate::cmd::ui::LiveViewMode,
    snapshot: &ServeProgressSnapshot,
) -> anyhow::Result<Vec<String>> {
    let monitor = build_monitor_report(repo_filter, worker_filter)?;
    let global = paths::global_dir()?;
    let run_store = RunStore::new(global.join("runs"));
    let pricing = autoflow_pricing_config();
    let (width, height) = crossterm::terminal::size().unwrap_or((100, 30));
    Ok(build_retained_serve_rows_for_size(
        &monitor,
        &run_store,
        &pricing,
        repo_filter,
        worker_filter,
        idle_sleep,
        view_mode,
        snapshot,
        width.max(50) as usize,
        height.max(14) as usize,
    ))
}

fn build_retained_serve_rows_for_size(
    monitor: &AutoflowMonitorReport,
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
    idle_sleep: std::time::Duration,
    view_mode: crate::cmd::ui::LiveViewMode,
    snapshot: &ServeProgressSnapshot,
    width: usize,
    height: usize,
) -> Vec<String> {
    let mut rows = vec![
        truncate_single_line(
            &format!(
                "▶ autoflow serve  ·  repo {}  ·  worker {}  ·  idle {}s",
                repo_filter.unwrap_or("(all)"),
                worker_filter.unwrap_or("(auto)"),
                idle_sleep.as_secs_f32(),
            ),
            width,
        ),
        String::new(),
        format!(
            "serve      {}",
            truncate_single_line(
                &format!(
                    "cycles {}  ·  ran {}  ·  skipped {}  ·  failed {}  ·  cleaned {}  ·  {}",
                    snapshot.cycles,
                    snapshot.total.ran_cycles,
                    snapshot.total.skipped_cycles,
                    snapshot.total.failed_cycles,
                    snapshot.total.cleaned_claims,
                    if snapshot.cycle_running {
                        "reconciling"
                    } else {
                        "watching"
                    }
                ),
                width.saturating_sub(11),
            )
        ),
    ];

    let primary = build_serve_issue_entries(&monitor.claims, &[])
        .into_iter()
        .find_map(|entry| entry.claim.cloned());

    if let Some(claim) = primary.as_ref() {
        rows.push(format!(
            "active     {}",
            truncate_single_line(
                &format!(
                    "{}  ·  {}  ·  {}",
                    display_issue_headline(claim),
                    truncate_text(&claim.workflow, 30),
                    claim.status
                ),
                width.saturating_sub(11),
            )
        ));

        let mut route = vec![format!("repo {}", short_locator(&claim.repo, 28))];
        if claim.branch != "-" {
            route.push(format!("branch {}", truncate_text(&claim.branch, 22)));
        }
        if claim.pr != "-" {
            route.push(format!("pr {}", short_url_like(&claim.pr, 22)));
        }
        rows.push(format!(
            "route      {}",
            truncate_single_line(&route.join("  ·  "), width.saturating_sub(11))
        ));

        if let Some((record, summary, live_lines)) =
            resolve_live_claim_run(run_store, pricing, claim)
        {
            rows.push(format!(
                "run        {}",
                truncate_single_line(
                    &format!(
                        "{}  ·  {}  ·  {}",
                        short_run_id(&summary.run_id),
                        summary.status,
                        format_duration(std::time::Duration::from_millis(
                            summary.duration_ms.unwrap_or_default()
                        ))
                    ),
                    width.saturating_sub(11),
                )
            ));

            let current_step = record
                .active_step_id
                .as_deref()
                .or(record.awaiting_step_id.as_deref())
                .unwrap_or("-");
            rows.push(format!(
                "step       {}",
                truncate_single_line(
                    &format!(
                        "{}  ·  {}/{} complete",
                        current_step,
                        completed_steps(&summary),
                        summary.steps.len()
                    ),
                    width.saturating_sub(11),
                )
            ));

            if let Some(usage) = &summary.usage {
                rows.push(format!(
                    "usage      {}",
                    truncate_single_line(
                        &format!(
                            "in {}  ·  out {}  ·  total {}{}",
                            format_count(usage.input_tokens),
                            format_count(usage.output_tokens),
                            format_count(usage.total_tokens),
                            usage
                                .cost_usd
                                .map(|cost| format!("  ·  ${cost:.2}"))
                                .unwrap_or_default()
                        ),
                        width.saturating_sub(11),
                    )
                ));
            }

            rows.push(String::new());
            rows.extend(render_retained_serve_event_rows(
                &live_lines,
                width,
                match view_mode {
                    crate::cmd::ui::LiveViewMode::Focused => 6,
                    crate::cmd::ui::LiveViewMode::Full => 10,
                },
            ));
        } else if claim.summary != "-" {
            rows.push(format!(
                "summary    {}",
                truncate_single_line(&claim.summary, width.saturating_sub(11))
            ));
        }
    } else {
        rows.push("active     no active issues".into());
    }

    if view_mode == crate::cmd::ui::LiveViewMode::Full && !monitor.claims.is_empty() {
        rows.push(String::new());
        rows.push("claims".into());
        for claim in monitor.claims.iter().take(6) {
            let status = claim_status_ui(&claim.status);
            rows.push(truncate_single_line(
                &format!(
                    "{} {}  ·  {}  ·  {}",
                    status.glyph(),
                    display_issue_headline(claim),
                    truncate_text(&claim.workflow, 24),
                    claim.status
                ),
                width,
            ));
        }
    }

    rows.push(String::new());
    rows.push("recent".into());
    let recent_limit = match view_mode {
        crate::cmd::ui::LiveViewMode::Focused => 4,
        crate::cmd::ui::LiveViewMode::Full => 8,
    };
    for activity in monitor.activity.iter().take(recent_limit) {
        let (status, label) = activity_status_and_label(&activity.event);
        rows.push(truncate_single_line(
            &format!(
                "{} {}  ·  {}  ·  {}",
                status.glyph(),
                activity.issue,
                label,
                truncate_text(&activity.workflow, 24)
            ),
            width,
        ));
        if activity.detail != "-" {
            rows.push(truncate_single_line(
                &format!(
                    "  {}",
                    truncate_text(&activity.detail, width.saturating_sub(2))
                ),
                width,
            ));
        }
    }

    rows.push(String::new());
    rows.push(format!(
        "queue      queued {}  ·  due {}  ·  processed {}",
        monitor.wakes.queued, monitor.wakes.due, monitor.wakes.processed_recent
    ));

    rows.truncate(height);
    rows
}

fn render_retained_serve_rows(rows: &[String]) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
    for (idx, row) in rows.iter().enumerate() {
        queue!(stdout, MoveTo(0, idx as u16), Print(row))?;
    }
    stdout.flush()?;
    Ok(())
}

fn render_retained_serve_event_rows(
    lines: &[AutoflowServeViewLine],
    width: usize,
    max_rows: usize,
) -> Vec<String> {
    let mut rendered = Vec::new();
    for line in lines {
        let prefix = format!("{} ", line.status.glyph());
        let content_width = width.saturating_sub(2).max(1);
        for (idx, segment) in wrap_retained_serve_plain(&line.text, content_width)
            .into_iter()
            .enumerate()
        {
            if idx == 0 {
                rendered.push(format!("{prefix}{segment}"));
            } else {
                rendered.push(format!("  {segment}"));
            }
        }
    }
    if rendered.len() > max_rows {
        rendered.split_off(rendered.len() - max_rows)
    } else {
        rendered
    }
}

fn wrap_retained_serve_plain(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        let current_len = current.chars().count();
        if current.is_empty() {
            if word_len <= width {
                current.push_str(word);
            } else {
                let mut chunk = String::new();
                for ch in word.chars() {
                    if chunk.chars().count() >= width {
                        out.push(chunk);
                        chunk = String::new();
                    }
                    chunk.push(ch);
                }
                if !chunk.is_empty() {
                    current = chunk;
                }
            }
        } else if current_len + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(current);
            current = String::new();
            if word_len <= width {
                current.push_str(word);
            } else {
                let mut chunk = String::new();
                for ch in word.chars() {
                    if chunk.chars().count() >= width {
                        out.push(chunk);
                        chunk = String::new();
                    }
                    chunk.push(ch);
                }
                current = chunk;
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn resolve_live_claim_run(
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
    claim: &AutoflowClaimRow,
) -> Option<(
    rupu_orchestrator::RunRecord,
    AutoflowRunSummary,
    Vec<AutoflowServeViewLine>,
)> {
    let record = find_live_autoflow_run(claim).or_else(|| {
        (claim.last_run != "-")
            .then(|| run_store.load(&claim.last_run).ok())
            .flatten()
    })?;
    let summary = load_run_summary(run_store, &record.id, pricing)?;
    let live_lines = load_live_run_lines(&record, 6);
    Some((record, summary, live_lines))
}

fn find_live_autoflow_run(claim: &AutoflowClaimRow) -> Option<rupu_orchestrator::RunRecord> {
    let global = paths::global_dir().ok()?;
    let runs_root = global.join("runs");
    let mut best: Option<rupu_orchestrator::RunRecord> = None;
    for entry in std::fs::read_dir(runs_root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path().join("run.json");
        if !path.is_file() {
            continue;
        }
        let body = std::fs::read(&path).ok()?;
        let record: rupu_orchestrator::RunRecord = serde_json::from_slice(&body).ok()?;
        if record.parent_run_id.is_some() {
            continue;
        }
        if record.issue_ref.as_deref() != Some(claim.issue.as_str()) {
            continue;
        }
        if record.workflow_name != claim.workflow {
            continue;
        }
        if matches!(
            record.status,
            RunStatus::Completed | RunStatus::Failed | RunStatus::Rejected
        ) {
            continue;
        }
        if best
            .as_ref()
            .map(|current| record.started_at > current.started_at)
            .unwrap_or(true)
        {
            best = Some(record);
        }
    }
    best
}

fn load_live_run_lines(
    record: &rupu_orchestrator::RunRecord,
    max_rows: usize,
) -> Vec<AutoflowServeViewLine> {
    let Some(path) = record.active_step_transcript_path.as_ref() else {
        return Vec::new();
    };
    let Ok(iter) = JsonlReader::iter(path) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for event in iter.flatten() {
        if let Some(line) = live_run_event_line(&event) {
            lines.push(line);
            if lines.len() > max_rows {
                let keep_from = lines.len().saturating_sub(max_rows);
                lines.drain(0..keep_from);
            }
        }
    }
    lines
}

fn live_run_event_line(event: &TranscriptEvent) -> Option<AutoflowServeViewLine> {
    match event {
        TranscriptEvent::TurnStart { turn_idx } => Some(AutoflowServeViewLine {
            status: UiStatus::Working,
            text: format!("turn {turn_idx}  ·  assistant turn started"),
        }),
        TranscriptEvent::AssistantMessage { content, .. } if !content.trim().is_empty() => {
            Some(AutoflowServeViewLine {
                status: UiStatus::Active,
                text: format!("assistant  ·  {}", truncate_single_line(content, 96)),
            })
        }
        TranscriptEvent::ToolCall { tool, input, .. } => Some(AutoflowServeViewLine {
            status: UiStatus::Working,
            text: format!(
                "{}  ·  {}",
                tool,
                crate::output::workflow_printer::tool_summary(tool, input)
            ),
        }),
        TranscriptEvent::ToolResult {
            output,
            error,
            duration_ms,
            ..
        } => {
            let status = if error.is_some() {
                UiStatus::Failed
            } else {
                UiStatus::Complete
            };
            let mut detail =
                truncate_single_line(error.as_deref().unwrap_or(output.as_str()), 88);
            if *duration_ms > 0 {
                detail.push_str(&format!("  ·  {}ms", duration_ms));
            }
            Some(AutoflowServeViewLine {
                status,
                text: format!(
                    "{}  ·  {}",
                    if error.is_some() {
                        "tool error"
                    } else {
                        "tool result"
                    },
                    detail
                ),
            })
        }
        TranscriptEvent::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => Some(AutoflowServeViewLine {
            status: UiStatus::Active,
            text: format!(
                "usage  ·  {provider} · {model}  ·  in {input_tokens} out {output_tokens} cached {cached_tokens}"
            ),
        }),
        TranscriptEvent::TurnEnd {
            turn_idx,
            tokens_in,
            tokens_out,
        } => Some(AutoflowServeViewLine {
            status: UiStatus::Complete,
            text: format!(
                "turn complete  ·  turn {turn_idx}  ·  in {} out {}",
                tokens_in.unwrap_or(0),
                tokens_out.unwrap_or(0)
            ),
        }),
        TranscriptEvent::RunComplete {
            status,
            total_tokens,
            duration_ms,
            error,
            ..
        } => Some(AutoflowServeViewLine {
            status: match status {
                rupu_transcript::RunStatus::Ok => UiStatus::Complete,
                rupu_transcript::RunStatus::Error | rupu_transcript::RunStatus::Aborted => {
                    UiStatus::Failed
                }
            },
            text: {
                let mut text = format!(
                    "run complete  ·  status {}  ·  {}ms  ·  {} tokens",
                    format!("{status:?}").to_lowercase(),
                    duration_ms,
                    total_tokens
                );
                if let Some(error) = error.as_deref().filter(|value| !value.trim().is_empty()) {
                    text.push_str("  ·  ");
                    text.push_str(&truncate_single_line(error, 64));
                }
                text
            },
        }),
        _ => None,
    }
}

async fn wakes(
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let repo_filter = normalize_repo_filter(repo)?;
    let store = WakeStore::new(paths::autoflow_wakes_dir(&global));
    let queued = filter_wakes_by_repo(store.list_queued()?, repo_filter.as_deref());
    let mut processed = filter_wakes_by_repo(store.list_processed()?, repo_filter.as_deref());
    processed.sort_by(|left, right| {
        right
            .received_at
            .cmp(&left.received_at)
            .then_with(|| right.wake_id.cmp(&left.wake_id))
    });
    let recent_processed = processed.into_iter().take(10).collect::<Vec<_>>();

    if queued.is_empty()
        && recent_processed.is_empty()
        && matches!(
            global_format.unwrap_or(crate::output::formats::OutputFormat::Table),
            crate::output::formats::OutputFormat::Table
        )
    {
        println!("(no autoflow wakes)");
        return Ok(());
    }
    let mut rows = queued
        .into_iter()
        .map(|wake| AutoflowWakeRow {
            wake_id: wake.wake_id,
            state: "queued".into(),
            source: wake_source_name(wake.source).into(),
            event: wake.event.id,
            entity: wake.entity.ref_text,
            not_before: wake.not_before,
            repo: wake.repo_ref,
        })
        .collect::<Vec<_>>();
    rows.extend(recent_processed.into_iter().map(|wake| AutoflowWakeRow {
        wake_id: wake.wake_id,
        state: "processed".into(),
        source: wake_source_name(wake.source).into(),
        event: wake.event.id,
        entity: wake.entity.ref_text,
        not_before: wake.not_before,
        repo: wake.repo_ref,
    }));
    let output = AutoflowWakesOutput {
        report: AutoflowWakesReport {
            kind: "autoflow_wakes",
            version: 1,
            rows,
        },
    };
    output_report::emit_collection(global_format, &output)
}

async fn monitor(
    repo: Option<&str>,
    worker: Option<&str>,
    watch: bool,
    interval: &str,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let format =
        crate::output::formats::resolve(global_format, crate::output::formats::OutputFormat::Table);
    if watch && format != crate::output::formats::OutputFormat::Table {
        bail!("`rupu autoflow monitor --watch` only supports table output");
    }

    let refresh = parse_duration(interval)?
        .to_std()
        .map_err(|_| anyhow!("monitor interval must be non-negative"))?;
    let repo_filter = normalize_repo_filter(repo)?;

    if !watch {
        let report = build_monitor_report(repo_filter.as_deref(), worker)?;
        let output = AutoflowMonitorOutput { report };
        return output_report::emit_collection(global_format, &output);
    }

    loop {
        let report = build_monitor_report(repo_filter.as_deref(), worker)?;
        render_monitor_watch_frame(&report)?;

        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(refresh) => {}
        }
    }
    Ok(())
}

async fn history(
    query: AutoflowHistoryQuery<'_>,
    watch: bool,
    interval: &str,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let format =
        crate::output::formats::resolve(global_format, crate::output::formats::OutputFormat::Table);
    if watch && format != crate::output::formats::OutputFormat::Table {
        bail!("`rupu autoflow history --watch` only supports table output");
    }

    let refresh = parse_duration(interval)?
        .to_std()
        .map_err(|_| anyhow!("history interval must be non-negative"))?;
    let repo_filter = normalize_repo_filter(query.repo_filter)?;
    let query = AutoflowHistoryQuery {
        repo_filter: repo_filter.as_deref(),
        ..query
    };

    if !watch {
        let report = build_history_report(&query)?;
        let output = AutoflowHistoryOutput { report };
        return output_report::emit_collection(global_format, &output);
    }

    loop {
        let report = build_history_report(&query)?;
        render_history_watch_frame(&report)?;

        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(refresh) => {}
        }
    }
    Ok(())
}

fn build_monitor_report(
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
) -> anyhow::Result<AutoflowMonitorReport> {
    let global = paths::global_dir()?;
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
    let worker_store = rupu_workspace::WorkerStore {
        root: paths::autoflow_workers_dir(&global),
    };

    let claims = filter_claims_by_repo(claim_store.list()?, repo_filter);
    let cycles = filter_monitor_cycles(history_store.list_recent(100)?, repo_filter, worker_filter);
    let history_events = filter_monitor_events(
        history_store.list_recent_events(400)?,
        repo_filter,
        worker_filter,
    );
    let recent = recent_activity_map_from_events(&history_events);
    let latest_by_worker = latest_cycle_by_worker(&cycles);

    let mut workers = worker_store
        .list()?
        .into_iter()
        .filter(|record| worker_filter_matches(worker_filter, &record.worker_id, &record.name))
        .filter_map(|record| {
            let cycle = latest_by_worker
                .get(record.worker_id.as_str())
                .or_else(|| latest_by_worker.get(record.name.as_str()));
            if repo_filter.is_some() && cycle.is_none() {
                return None;
            }
            Some(AutoflowMonitorWorkerRow {
                worker: record.name,
                kind: match record.kind {
                    rupu_workspace::WorkerKind::Cli => "cli".into(),
                    rupu_workspace::WorkerKind::AutoflowServe => "autoflow_serve".into(),
                },
                last_seen: record.last_seen_at,
                last_cycle: cycle
                    .map(|cycle| cycle.started_at.clone())
                    .unwrap_or_else(|| "-".into()),
                repo_scope: cycle
                    .and_then(|cycle| cycle.repo_filter.clone())
                    .unwrap_or_else(|| "-".into()),
                ran: cycle.map(|cycle| cycle.ran_cycles).unwrap_or_default(),
                failed: cycle.map(|cycle| cycle.failed_cycles).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    workers.sort_by(|left, right| left.worker.cmp(&right.worker));

    let claim_rows = claims
        .iter()
        .map(|claim| AutoflowClaimRow {
            issue: claim.issue_ref.clone(),
            issue_display: claim.issue_display_ref.clone(),
            tracker: claim.issue_tracker.clone(),
            state: claim.issue_state_name.clone(),
            source: claim.source_ref.clone(),
            repo: claim.repo_ref.clone(),
            workflow: claim.workflow.clone(),
            priority: selected_priority(claim)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            status: status_name(claim.status).into(),
            next: next_action_summary(claim),
            branch: claim.branch.clone().unwrap_or_else(|| "-".into()),
            pr: claim.pr_url.clone().unwrap_or_else(|| "-".into()),
            summary: claim_summary(claim),
            contenders: format_contenders(&claim.contenders),
            workspace: claim.worktree_path.clone().unwrap_or_else(|| "-".into()),
            last_cycle: recent
                .get(&claim.issue_ref)
                .map(|activity| activity.at.clone())
                .unwrap_or_else(|| "-".into()),
            last_event: recent
                .get(&claim.issue_ref)
                .map(|activity| activity.event.clone())
                .unwrap_or_else(|| "-".into()),
            last_run: recent
                .get(&claim.issue_ref)
                .and_then(|activity| activity.run_id.clone())
                .or_else(|| claim.last_run_id.clone())
                .unwrap_or_else(|| "-".into()),
        })
        .collect::<Vec<_>>();

    let activity = history_events
        .iter()
        .map(|record| AutoflowMonitorActivityRow {
            at: record.at.clone(),
            worker: record
                .worker_name
                .clone()
                .or_else(|| record.worker_id.clone())
                .unwrap_or_else(|| "-".into()),
            event: monitor_event_name(&record.event),
            issue: record
                .event
                .issue_display_ref
                .clone()
                .or_else(|| record.event.issue_ref.clone())
                .unwrap_or_else(|| "-".into()),
            workflow: record.event.workflow.clone().unwrap_or_else(|| "-".into()),
            repo: record.event.repo_ref.clone().unwrap_or_else(|| "-".into()),
            detail: record.event.detail.clone().unwrap_or_else(|| "-".into()),
        })
        .take(20)
        .collect::<Vec<_>>();

    let queued = filter_wakes_by_repo(wake_store.list_queued()?, repo_filter);
    let due = filter_wakes_by_repo(wake_store.list_due(chrono::Utc::now())?, repo_filter);
    let processed_recent = filter_wakes_by_repo(wake_store.list_processed()?, repo_filter).len();

    Ok(AutoflowMonitorReport {
        kind: "autoflow_monitor",
        version: 1,
        workers,
        claims: claim_rows,
        activity,
        wakes: AutoflowMonitorWakeSummary {
            queued: queued.len(),
            due: due.len(),
            processed_recent,
        },
    })
}

fn build_history_report(query: &AutoflowHistoryQuery<'_>) -> anyhow::Result<AutoflowHistoryReport> {
    let global = paths::global_dir()?;
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
    let events = filter_monitor_events(
        history_store.list_recent_events(1000)?,
        query.repo_filter,
        query.worker_filter,
    );
    let cycles = filter_monitor_cycles(
        history_store.list_recent(200)?,
        query.repo_filter,
        query.worker_filter,
    );
    let total_cycles = cycles.len();
    let mut rows = history_rows_from_events(
        &events,
        query.issue_filter,
        query.source_filter,
        query.event_filter,
    );
    let total_events = rows.len();
    if rows.len() > query.limit {
        rows.truncate(query.limit);
    }
    Ok(AutoflowHistoryReport {
        kind: "autoflow_history",
        version: 1,
        total_cycles,
        total_events,
        rows,
    })
}

fn history_rows_from_events(
    events: &[AutoflowHistoryEventRecord],
    issue_filter: Option<&str>,
    source_filter: Option<&str>,
    event_filter: Option<&str>,
) -> Vec<AutoflowHistoryRow> {
    events
        .iter()
        .filter_map(|record| {
            let event = &record.event;
            let event_name = monitor_event_name(event);
            let issue = event
                .issue_display_ref
                .clone()
                .or_else(|| event.issue_ref.clone())
                .unwrap_or_else(|| "-".into());
            let issue_ref = event.issue_ref.as_deref().unwrap_or("");
            let source = event.source_ref.clone().unwrap_or_else(|| "-".into());
            let worker = record
                .worker_name
                .clone()
                .or_else(|| record.worker_id.clone())
                .unwrap_or_else(|| "-".into());
            let repo = event.repo_ref.clone().unwrap_or_else(|| "-".into());
            let workflow = event.workflow.clone().unwrap_or_else(|| "-".into());
            let run = event.run_id.clone().unwrap_or_else(|| "-".into());
            let wake = event
                .wake_id
                .clone()
                .or_else(|| event.wake_event_id.clone())
                .unwrap_or_else(|| "-".into());
            let detail = event.detail.clone().unwrap_or_else(|| "-".into());
            let source_match = source_filter.is_none_or(|filter| filter == source);
            let issue_match =
                issue_filter.is_none_or(|filter| filter == issue_ref || filter == issue);
            let event_match = event_filter.is_none_or(|filter| filter == event_name);
            if !(source_match && issue_match && event_match) {
                return None;
            }
            Some(AutoflowHistoryRow {
                at: record.at.clone(),
                cycle_id: record.cycle_id.clone(),
                mode: match record.mode {
                    rupu_runtime::AutoflowCycleMode::Tick => "tick".into(),
                    rupu_runtime::AutoflowCycleMode::Serve => "serve".into(),
                },
                worker,
                event: event_name.to_string(),
                issue,
                source,
                workflow,
                repo,
                run,
                wake,
                detail,
            })
        })
        .collect()
}

fn recent_activity_by_issue(
    history_store: &AutoflowHistoryStore,
    repo_filter: Option<&str>,
) -> anyhow::Result<BTreeMap<String, RecentIssueActivity>> {
    let events = filter_monitor_events(history_store.list_recent_events(400)?, repo_filter, None);
    Ok(recent_activity_map_from_events(&events))
}

fn recent_activity_map_from_events(
    events: &[AutoflowHistoryEventRecord],
) -> BTreeMap<String, RecentIssueActivity> {
    let mut out = BTreeMap::new();
    for record in events {
        let Some(issue_ref) = record.event.issue_ref.as_ref() else {
            continue;
        };
        out.entry(issue_ref.clone())
            .or_insert_with(|| RecentIssueActivity {
                at: record.at.clone(),
                event: monitor_event_name(&record.event).to_string(),
                run_id: record.event.run_id.clone(),
            });
    }
    out
}

fn render_monitor_watch_frame(report: &AutoflowMonitorReport) -> anyhow::Result<()> {
    print!("\x1B[2J\x1B[H");
    render_monitor_snapshot(report, true)
}

fn render_history_watch_frame(report: &AutoflowHistoryReport) -> anyhow::Result<()> {
    print!("\x1B[2J\x1B[H");
    println!(
        "rupu autoflow history  refreshed={}  cycles={}  events={}  rows={}",
        chrono::Utc::now().to_rfc3339(),
        report.total_cycles,
        report.total_events,
        report.rows.len()
    );
    println!();
    render_history_table(report)
}

fn render_monitor_snapshot(report: &AutoflowMonitorReport, watch_mode: bool) -> anyhow::Result<()> {
    let run_store = RunStore::new(paths::global_dir()?.join("runs"));
    let pricing = autoflow_pricing_config();
    render_autoflow_header(
        "autoflow monitor",
        &[
            format!(
                "refreshed {}",
                short_timestamp(&chrono::Utc::now().to_rfc3339())
            ),
            format!("claims {}", report.claims.len()),
            format!("workers {}", report.workers.len()),
            format!("queued wakes {}", report.wakes.queued),
            if watch_mode {
                "Ctrl-C to stop".into()
            } else {
                String::new()
            },
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>(),
    );

    render_claim_snapshot_section("active claims", &report.claims, 6, &run_store, &pricing);
    render_activity_section("recent activity", &report.activity, 10);
    render_worker_section(&report.workers, 6);
    render_wake_summary(&report.wakes);
    std::io::stdout().flush()?;
    Ok(())
}

fn render_serve_cycle_timeline(
    report: &crate::cmd::autoflow_runtime::ServeReport,
    last_tick: &crate::cmd::autoflow_runtime::TickReport,
    cycle: &AutoflowCycleRecord,
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
    idle_cycles: &mut usize,
) -> anyhow::Result<()> {
    let monitor = build_monitor_report(repo_filter, worker_filter)?;
    let run_store = RunStore::new(paths::global_dir()?.join("runs"));
    let pricing = autoflow_pricing_config();
    let interesting_events = cycle
        .events
        .iter()
        .filter(|event| {
            !matches!(
                event.kind,
                rupu_runtime::AutoflowCycleEventKind::WakeConsumed
            )
        })
        .collect::<Vec<_>>();
    let eventful = !interesting_events.is_empty()
        || last_tick.ran_cycles > 0
        || last_tick.failed_cycles > 0
        || report.cycles == 1;

    if eventful {
        *idle_cycles = 0;
    } else {
        *idle_cycles += 1;
        if *idle_cycles > 1 && !(*idle_cycles).is_multiple_of(6) {
            return Ok(());
        }
    }

    render_cycle_header(report.cycles, last_tick, cycle);
    if eventful {
        render_frame_spacer(0);
        render_cycle_operator_layout(&monitor.claims, &interesting_events, &run_store, &pricing)?;
    } else {
        render_dim_branch("idle");
    }
    render_queue_section(&monitor.wakes);
    std::io::stdout().flush()?;
    Ok(())
}

fn update_serve_ticker(
    printer: &Arc<Mutex<LineStreamPrinter>>,
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
    progress: Arc<Mutex<ServeProgressSnapshot>>,
) -> anyhow::Result<()> {
    let snapshot = progress
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    if snapshot.cycle_running {
        if let Ok(mut printer) = printer.lock() {
            printer.stop_ticker();
        }
        return Ok(());
    }
    if snapshot
        .last_cycle_rendered_at
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(2))
    {
        return Ok(());
    }

    let monitor = build_monitor_report(repo_filter, worker_filter)?;
    let active_claim = monitor
        .claims
        .iter()
        .find(|claim| claim.status == "running")
        .or_else(|| {
            monitor
                .claims
                .iter()
                .find(|claim| claim_is_live_for_heartbeat(claim))
        });

    let message = if let Some(claim) = active_claim {
        let issue = claim.issue_display.as_deref().unwrap_or(&claim.issue);
        let headline = format!(
            "{} issue {issue}  ·  {}",
            heartbeat_action_label(claim),
            truncate_text(&claim.workflow, 28),
        );
        let mut detail = Vec::new();
        detail.push(claim.status.clone());
        if claim.branch != "-" {
            detail.push(format!("branch {}", truncate_text(&claim.branch, 28)));
        }
        if claim.last_run != "-" {
            detail.push(format!("run {}", claim.last_run));
        }
        detail.push(format!(
            "wakes queued={} due={}",
            monitor.wakes.queued, monitor.wakes.due
        ));
        format!("{headline}  ·  {}", detail.join("  ·  "))
    } else {
        format!(
            "polling for work  ·  cycles={}  ·  wakes queued={} due={}  ·  no active claims",
            snapshot.cycles, monitor.wakes.queued, monitor.wakes.due
        )
    };

    if let Ok(mut printer) = printer.lock() {
        printer.start_ticker(message);
    }
    Ok(())
}

fn claim_is_live_for_heartbeat(claim: &AutoflowClaimRow) -> bool {
    claim.status != "complete"
}

fn heartbeat_action_label(claim: &AutoflowClaimRow) -> &'static str {
    match claim.status.as_str() {
        "running" | "claimed" => "processing",
        "await_human" => "awaiting input on",
        "await_external" => "waiting on",
        "blocked" => "blocked on",
        _ => "tracking",
    }
}

#[cfg(test)]
mod serve_heartbeat_tests {
    use super::*;
    use tempfile::tempdir;

    fn claim_with_status(status: &str) -> AutoflowClaimRow {
        AutoflowClaimRow {
            issue: "github:Section9Labs/rupu/issues/1".into(),
            issue_display: Some("1".into()),
            tracker: Some("github".into()),
            state: Some("open".into()),
            source: Some("github:Section9Labs/rupu".into()),
            repo: "github:Section9Labs/rupu".into(),
            workflow: "demo".into(),
            priority: "100".into(),
            status: status.into(),
            next: "-".into(),
            branch: "-".into(),
            pr: "-".into(),
            summary: "-".into(),
            contenders: "*demo[100]".into(),
            workspace: "-".into(),
            last_cycle: "-".into(),
            last_event: "-".into(),
            last_run: "-".into(),
        }
    }

    fn sample_monitor_report() -> AutoflowMonitorReport {
        let mut running = claim_with_status("running");
        running.issue = "github:Section9Labs/rupu/issues/7".into();
        running.issue_display = Some("7".into());
        running.workflow = "storefront-feature-delivery".into();
        running.branch = "rupu/issue-7".into();
        running.summary = "implementing storefront feature".into();

        let mut blocked = claim_with_status("blocked");
        blocked.issue = "github:Section9Labs/rupu/issues/8".into();
        blocked.issue_display = Some("8".into());
        blocked.workflow = "verify-followup".into();
        blocked.summary = "waiting on approval".into();

        AutoflowMonitorReport {
            kind: "autoflow_monitor",
            version: 1,
            workers: Vec::new(),
            claims: vec![running],
            activity: vec![
                AutoflowMonitorActivityRow {
                    at: "2026-05-14T05:00:00Z".into(),
                    worker: "team-mini-01".into(),
                    event: "issue_commented".into(),
                    issue: "github:Section9Labs/rupu/issues/7".into(),
                    workflow: "storefront-feature-delivery".into(),
                    repo: "github:Section9Labs/rupu".into(),
                    detail: "commented on issue".into(),
                },
                AutoflowMonitorActivityRow {
                    at: "2026-05-14T05:01:00Z".into(),
                    worker: "team-mini-01".into(),
                    event: "cycle_failed".into(),
                    issue: "github:Section9Labs/rupu/issues/8".into(),
                    workflow: "verify-followup".into(),
                    repo: "github:Section9Labs/rupu".into(),
                    detail: "waiting on approval".into(),
                },
            ],
            wakes: AutoflowMonitorWakeSummary {
                queued: 1,
                due: 2,
                processed_recent: 3,
            },
        }
    }

    #[test]
    fn heartbeat_ignores_complete_claims() {
        assert!(!claim_is_live_for_heartbeat(&claim_with_status("complete")));
        assert!(claim_is_live_for_heartbeat(&claim_with_status("running")));
    }

    #[test]
    fn heartbeat_uses_status_specific_labels() {
        assert_eq!(
            heartbeat_action_label(&claim_with_status("running")),
            "processing"
        );
        assert_eq!(
            heartbeat_action_label(&claim_with_status("await_external")),
            "waiting on"
        );
        assert_eq!(
            heartbeat_action_label(&claim_with_status("blocked")),
            "blocked on"
        );
    }

    #[test]
    fn cycle_frame_hides_complete_claim_without_current_events() {
        let claim = claim_with_status("complete");
        assert!(!claim_should_render_in_cycle_frame(&claim, &[]));
    }

    #[test]
    fn cycle_frame_keeps_complete_claim_with_current_events() {
        let claim = claim_with_status("complete");
        let event = AutoflowCycleEvent {
            kind: rupu_runtime::AutoflowCycleEventKind::CycleSkipped,
            issue_ref: Some(claim.issue.clone()),
            ..Default::default()
        };
        assert!(claim_should_render_in_cycle_frame(&claim, &[&event]));
    }

    #[test]
    fn serve_entries_prioritize_running_issue_with_events() {
        let running = claim_with_status("running");
        let mut complete = claim_with_status("complete");
        complete.issue = "github:Section9Labs/rupu/issues/2".into();
        complete.issue_display = Some("2".into());
        let event = AutoflowCycleEvent {
            kind: rupu_runtime::AutoflowCycleEventKind::RunLaunched,
            issue_ref: Some(running.issue.clone()),
            ..Default::default()
        };

        let claims = [complete, running.clone()];
        let entries = build_serve_issue_entries(&claims, &[&event]);
        assert_eq!(
            entries
                .first()
                .and_then(|entry| entry.claim)
                .map(|claim| claim.issue.as_str()),
            Some(running.issue.as_str())
        );
    }

    #[test]
    fn serve_entries_include_event_only_issues() {
        let event = AutoflowCycleEvent {
            kind: rupu_runtime::AutoflowCycleEventKind::IssueCommented,
            issue_ref: Some("github:Section9Labs/rupu/issues/99".into()),
            ..Default::default()
        };

        let entries = build_serve_issue_entries(&[], &[&event]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].issue_ref, "github:Section9Labs/rupu/issues/99");
        assert!(entries[0].claim.is_none());
    }

    #[test]
    fn promoted_issue_events_use_specific_labels() {
        let commented = AutoflowCycleEvent {
            kind: rupu_runtime::AutoflowCycleEventKind::IssueCommented,
            ..Default::default()
        };
        assert_eq!(cycle_event_status_and_label(&commented).1, "commented");
        assert_eq!(monitor_event_name(&commented), "issue_commented");

        let closed = AutoflowCycleEvent {
            kind: rupu_runtime::AutoflowCycleEventKind::IssueStateChanged,
            status: Some("closed".into()),
            ..Default::default()
        };
        assert_eq!(cycle_event_status_and_label(&closed).1, "closed issue");
        assert_eq!(monitor_event_name(&closed), "issue_closed");

        let draft_pr = AutoflowCycleEvent {
            kind: rupu_runtime::AutoflowCycleEventKind::PullRequestOpened,
            status: Some("draft".into()),
            ..Default::default()
        };
        assert_eq!(cycle_event_status_and_label(&draft_pr).1, "opened draft PR");
        assert_eq!(monitor_event_name(&draft_pr), "draft_pr_opened");
    }

    #[test]
    fn retained_serve_focused_mode_omits_claims_section() {
        let tmp = tempdir().unwrap();
        let run_store = RunStore::new(tmp.path().join("runs"));
        let rows = build_retained_serve_rows_for_size(
            &sample_monitor_report(),
            &run_store,
            &autoflow_pricing_config(),
            Some("github:Section9Labs/rupu"),
            Some("team-mini-01"),
            std::time::Duration::from_secs(10),
            crate::cmd::ui::LiveViewMode::Focused,
            &ServeProgressSnapshot::default(),
            80,
            24,
        );
        assert!(rows.iter().any(|row| row.starts_with("active     ")));
        assert!(!rows.iter().any(|row| row == "claims"));
    }

    #[test]
    fn retained_serve_full_mode_includes_claims_section() {
        let tmp = tempdir().unwrap();
        let run_store = RunStore::new(tmp.path().join("runs"));
        let rows = build_retained_serve_rows_for_size(
            &sample_monitor_report(),
            &run_store,
            &autoflow_pricing_config(),
            Some("github:Section9Labs/rupu"),
            Some("team-mini-01"),
            std::time::Duration::from_secs(10),
            crate::cmd::ui::LiveViewMode::Full,
            &ServeProgressSnapshot::default(),
            80,
            24,
        );
        assert!(rows.iter().any(|row| row == "claims"));
        assert!(rows
            .iter()
            .any(|row| row.contains("storefront-feature-delivery")));
    }
}

fn render_autoflow_header(title: &str, meta: &[String]) {
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "▶", BRAND);
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, title, BRAND);
    if !meta.is_empty() {
        line.push_str("  ");
        let _ = palette::write_colored(&mut line, &meta.join("  ·  "), DIM);
    }
    println!("{line}");
    println!();
}

fn render_cycle_header(
    cycle_number: usize,
    tick: &crate::cmd::autoflow_runtime::TickReport,
    cycle: &AutoflowCycleRecord,
) {
    let status = if tick.failed_cycles > 0 {
        UiStatus::Failed
    } else if tick.ran_cycles > 0 {
        UiStatus::Working
    } else {
        UiStatus::Waiting
    };
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "├─", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, &status.glyph().to_string(), status.color());
    line.push(' ');
    let _ =
        palette::write_bold_colored(&mut line, &format!("cycle {cycle_number}"), status.color());
    line.push_str("  ");
    let _ = palette::write_colored(
        &mut line,
        &format!(
            "ran={}  skipped={}  failed={}  polled={}  webhook={}  {}",
            tick.ran_cycles,
            tick.skipped_cycles,
            tick.failed_cycles,
            tick.polled_event_count,
            tick.webhook_event_count,
            short_timestamp(&cycle.finished_at),
        ),
        DIM,
    );
    println!("{line}");
}

#[derive(Debug, Clone)]
struct ServeIssueEntry<'a> {
    issue_ref: String,
    claim: Option<&'a AutoflowClaimRow>,
    events: Vec<&'a AutoflowCycleEvent>,
}

fn render_cycle_operator_layout(
    claims: &[AutoflowClaimRow],
    events: &[&AutoflowCycleEvent],
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
) -> anyhow::Result<()> {
    let entries = build_serve_issue_entries(claims, events);
    let Some(primary) = entries.first() else {
        render_dim_branch("no active issues");
        return Ok(());
    };

    match primary.claim {
        Some(claim) => render_issue_frame(claim, &primary.events, run_store, pricing)?,
        None => render_event_only_frame(&primary.issue_ref, &primary.events),
    }

    if entries.len() > 1 {
        render_frame_spacer(0);
        render_recent_issue_section(&entries[1..], run_store, pricing, 4);
    }
    Ok(())
}

fn claim_should_render_in_cycle_frame(
    claim: &AutoflowClaimRow,
    claim_events: &[&AutoflowCycleEvent],
) -> bool {
    !claim_events.is_empty() || claim_is_live_for_heartbeat(claim)
}

fn build_serve_issue_entries<'a>(
    claims: &'a [AutoflowClaimRow],
    events: &[&'a AutoflowCycleEvent],
) -> Vec<ServeIssueEntry<'a>> {
    let mut event_map: BTreeMap<String, Vec<&'a AutoflowCycleEvent>> = BTreeMap::new();
    for event in events {
        if let Some(issue_ref) = event.issue_ref.as_ref() {
            event_map.entry(issue_ref.clone()).or_default().push(*event);
        }
    }

    let mut entries = Vec::new();
    for claim in claims {
        let claim_events = event_map.remove(&claim.issue).unwrap_or_default();
        if !claim_should_render_in_cycle_frame(claim, &claim_events) {
            continue;
        }
        entries.push(ServeIssueEntry {
            issue_ref: claim.issue.clone(),
            claim: Some(claim),
            events: claim_events,
        });
    }

    for (issue_ref, claim_events) in event_map {
        entries.push(ServeIssueEntry {
            issue_ref,
            claim: None,
            events: claim_events,
        });
    }

    entries.sort_by(|left, right| {
        serve_issue_entry_sort_key(left).cmp(&serve_issue_entry_sort_key(right))
    });
    entries
}

fn serve_issue_entry_sort_key(entry: &ServeIssueEntry<'_>) -> (u8, u8, String) {
    let claim_status = entry
        .claim
        .map(|claim| claim.status.as_str())
        .unwrap_or("detached");
    let primary_rank = match (claim_status, entry.events.is_empty()) {
        ("running" | "claimed", false) => 0,
        ("running" | "claimed", true) => 1,
        ("await_human" | "await_external" | "retry_backoff" | "blocked", false) => 2,
        (_, false) => 3,
        ("complete", true) => 5,
        _ => 4,
    };
    let status_rank = match claim_status {
        "running" => 0,
        "claimed" => 1,
        "await_human" => 2,
        "await_external" => 3,
        "retry_backoff" => 4,
        "blocked" => 5,
        "complete" => 7,
        _ => 6,
    };
    (primary_rank, status_rank, entry.issue_ref.clone())
}

fn render_recent_issue_section(
    entries: &[ServeIssueEntry<'_>],
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
    max_rows: usize,
) {
    if entries.is_empty() {
        return;
    }
    render_section_heading("recent");
    for entry in entries.iter().take(max_rows) {
        match entry.claim {
            Some(claim) => render_compact_issue_row(claim, &entry.events, run_store, pricing),
            None => render_compact_event_row(&entry.issue_ref, &entry.events),
        }
    }
    if entries.len() > max_rows {
        render_dim_detail(&format!(
            "+{} more recent item(s)",
            entries.len() - max_rows
        ));
    }
}

fn render_compact_issue_row(
    claim: &AutoflowClaimRow,
    events: &[&AutoflowCycleEvent],
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
) {
    let status = claim_status_ui(&claim.status);
    let issue = display_issue_headline(claim);
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, &status.glyph().to_string(), status.color());
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, &issue, status.color());
    line.push_str("  ");
    let _ = palette::write_bold_colored(
        &mut line,
        &truncate_text(&claim.workflow, 30),
        palette::BRAND,
    );
    line.push_str("  ");
    let _ = palette::write_colored(&mut line, &claim.status, DIM);
    println!("{line}");

    if let Some(detail) = compact_issue_detail(claim, events, run_store, pricing) {
        render_dim_detail(&truncate_text(&detail, 104));
    }
}

fn render_compact_event_row(issue_ref: &str, events: &[&AutoflowCycleEvent]) {
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, "○", palette::SKIPPED);
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, issue_ref, palette::SKIPPED);
    line.push_str("  ");
    let _ = palette::write_colored(&mut line, "detached", DIM);
    println!("{line}");
    if let Some(event) = events.first() {
        let (_, label) = cycle_event_status_and_label(event);
        let detail = event.detail.as_deref().unwrap_or("").trim();
        let message = if detail.is_empty() {
            label.to_string()
        } else {
            format!("{label}  ·  {detail}")
        };
        render_dim_detail(&truncate_text(&message, 104));
    }
}

fn compact_issue_detail(
    claim: &AutoflowClaimRow,
    events: &[&AutoflowCycleEvent],
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
) -> Option<String> {
    if let Some(event) = events.first() {
        let (_, label) = cycle_event_status_and_label(event);
        let detail = event.detail.as_deref().unwrap_or("").trim();
        return Some(if detail.is_empty() {
            label.to_string()
        } else {
            format!("{label}  ·  {detail}")
        });
    }

    if claim.last_run != "-" {
        if let Some(summary) = load_run_summary(run_store, &claim.last_run, pricing) {
            if let Some(diff) = &summary.diff {
                return Some(format!(
                    "changes {} file(s)  ·  +{}/-{}",
                    diff.files_changed,
                    format_count(diff.insertions),
                    format_count(diff.deletions),
                ));
            }
            if let Some(duration_ms) = summary.duration_ms {
                return Some(format!(
                    "{}  ·  duration {}",
                    summary.status,
                    format_duration(std::time::Duration::from_millis(duration_ms))
                ));
            }
        }
    }

    if claim.next != "-" {
        return Some(format!("next  {}", claim.next));
    }
    if claim.summary != "-" {
        return Some(claim.summary.clone());
    }
    if claim.branch != "-" {
        return Some(format!("branch  {}", claim.branch));
    }
    None
}

fn render_claim_snapshot_section(
    title: &str,
    claims: &[AutoflowClaimRow],
    max_rows: usize,
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
) {
    if claims.is_empty() {
        return;
    }
    render_section_heading(title);
    for claim in claims.iter().take(max_rows) {
        let status = claim_status_ui(&claim.status);
        let issue = display_issue_headline(claim);
        let mut line = String::new();
        let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
        line.push(' ');
        let _ = palette::write_bold_colored(&mut line, &status.glyph().to_string(), status.color());
        line.push(' ');
        let _ = palette::write_bold_colored(&mut line, &issue, status.color());
        line.push_str("  ");
        let _ = palette::write_bold_colored(
            &mut line,
            &truncate_text(&claim.workflow, 30),
            palette::BRAND,
        );
        line.push_str("  ");
        let _ = palette::write_colored(&mut line, &claim.status, DIM);
        println!("{line}");

        let mut route = Vec::new();
        if let Some(tracker) = claim.tracker.as_deref() {
            route.push(("tracker", human_tracker_name(tracker).to_string()));
        }
        route.push(("repo", short_locator(&claim.repo, 34)));
        if claim.branch != "-" {
            route.push(("branch", truncate_text(&claim.branch, 24)));
        }
        if claim.pr != "-" {
            route.push(("pr", short_url_like(&claim.pr, 26)));
        }
        render_key_value_detail(0, "⌁", palette::BRAND, &route);

        let mut progress = vec![
            ("state", claim.state.as_deref().unwrap_or("-").to_string()),
            ("next", truncate_text(&claim.next, 34)),
        ];
        if claim.last_run != "-" {
            progress.push(("run", short_run_id(&claim.last_run)));
        }
        render_key_value_detail(0, "◈", palette::RUNNING, &progress);

        if claim.last_run != "-" {
            if let Some(summary) = load_run_summary(run_store, &claim.last_run, pricing) {
                render_dim_detail(&truncate_text(&format_run_metrics_line(&summary), 84));
            }
        }
        if claim.summary != "-" {
            render_dim_detail(&truncate_text(&claim.summary, 88));
        }
    }
    if claims.len() > max_rows {
        render_dim_detail(&format!(
            "+{} more active claim(s)",
            claims.len() - max_rows
        ));
    }
}

fn render_issue_frame(
    claim: &AutoflowClaimRow,
    events: &[&AutoflowCycleEvent],
    run_store: &RunStore,
    pricing: &rupu_config::PricingConfig,
) -> anyhow::Result<()> {
    let status = claim_status_ui(&claim.status);
    let issue = display_issue_headline(claim);
    let mut open = String::new();
    push_serve_frame_open(&mut open, 1);
    open.push(' ');
    let _ = palette::write_bold_colored(&mut open, &status.glyph().to_string(), status.color());
    open.push(' ');
    let _ = palette::write_bold_colored(&mut open, &issue, status.color());
    open.push(' ');
    let rule = "─".repeat(6);
    let _ = palette::write_colored(&mut open, &rule, palette::BRAND_300);
    open.push_str("  ");
    let _ = palette::write_bold_colored(
        &mut open,
        &truncate_text(&claim.workflow, 30),
        palette::BRAND,
    );
    open.push_str("  ");
    let _ = palette::write_colored(
        &mut open,
        &format!(
            "{}  ·  {}",
            claim.status,
            claim.state.as_deref().unwrap_or("-")
        ),
        DIM,
    );
    println!("{open}");

    let mut route = Vec::new();
    if let Some(tracker) = claim.tracker.as_deref() {
        route.push(("tracker", human_tracker_name(tracker).to_string()));
    }
    if let Some(source) = claim.source.as_deref() {
        if source != &claim.repo {
            route.push(("source", short_locator(source, 30)));
        }
    }
    route.push(("repo", short_locator(&claim.repo, 34)));
    if claim.branch != "-" {
        route.push(("branch", truncate_text(&claim.branch, 24)));
    }
    if claim.pr != "-" {
        route.push(("pr", short_url_like(&claim.pr, 26)));
    }
    render_frame_key_value_detail(1, "⌁", palette::BRAND, &route);
    if claim.last_run != "-" || claim.next != "-" {
        let mut progress = Vec::new();
        if claim.last_run != "-" {
            progress.push(("run", short_run_id(&claim.last_run)));
        }
        if claim.next != "-" {
            progress.push(("next", truncate_text(&claim.next, 30)));
        }
        render_frame_key_value_detail(1, "⇢", palette::RUNNING, &progress);
    }
    if claim.summary != "-" || claim.last_run != "-" || !events.is_empty() {
        render_frame_spacer(1);
    }
    if claim.summary != "-" {
        render_frame_note_detail(1, "summary", &truncate_text(&claim.summary, 88));
    }
    if claim.last_run != "-" {
        if let Some(run_summary) = load_run_summary(run_store, &claim.last_run, pricing) {
            render_run_summary(&run_summary);
        }
    }
    if !events.is_empty() {
        if claim.last_run != "-" || claim.summary != "-" {
            render_frame_spacer(1);
        }
        render_frame_subheading(1, "timeline");
        for event in events.iter().take(5) {
            render_frame_event(1, event);
        }
    }

    let mut close = String::new();
    push_serve_frame_close(&mut close, 1);
    close.push(' ');
    let _ = palette::write_bold_colored(&mut close, &status.glyph().to_string(), status.color());
    close.push(' ');
    let tail = if claim.last_run != "-" {
        format!("{}  ·  watch {}", claim.status, claim.last_run)
    } else {
        claim.status.clone()
    };
    let _ = palette::write_colored(&mut close, &tail, status.color());
    println!("{close}");
    Ok(())
}

fn render_event_only_frame(issue_ref: &str, events: &[&AutoflowCycleEvent]) {
    let mut open = String::new();
    push_serve_frame_open(&mut open, 1);
    open.push(' ');
    let _ = palette::write_bold_colored(&mut open, "○", palette::SKIPPED);
    open.push(' ');
    let _ = palette::write_bold_colored(&mut open, issue_ref, palette::SKIPPED);
    println!("{open}");
    for event in events.iter().take(5) {
        render_frame_event(1, event);
    }
    let mut close = String::new();
    push_serve_frame_close(&mut close, 1);
    close.push(' ');
    let _ = palette::write_colored(&mut close, "detached", DIM);
    println!("{close}");
}

fn render_activity_section(title: &str, activity: &[AutoflowMonitorActivityRow], max_rows: usize) {
    if activity.is_empty() {
        return;
    }
    render_section_heading(title);
    for row in activity.iter().take(max_rows) {
        let (status, label) = activity_status_and_label(&row.event);
        let mut line = String::new();
        let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
        line.push(' ');
        let _ = palette::write_bold_colored(&mut line, &status.glyph().to_string(), status.color());
        line.push(' ');
        let _ = palette::write_bold_colored(&mut line, &row.issue, status.color());
        line.push_str("  ");
        line.push_str(label);
        if row.workflow != "-" {
            line.push_str("  ");
            let _ = palette::write_colored(&mut line, &truncate_text(&row.workflow, 28), DIM);
        }
        line.push_str("  ");
        let _ = palette::write_colored(&mut line, &short_timestamp(&row.at), DIM);
        println!("{line}");

        if row.detail != "-" {
            render_dim_detail(&truncate_text(&row.detail, 96));
        }
    }
}

fn render_worker_section(workers: &[AutoflowMonitorWorkerRow], max_rows: usize) {
    if workers.is_empty() {
        return;
    }
    render_section_heading("workers");
    for worker in workers.iter().take(max_rows) {
        let status = if worker.failed > 0 {
            UiStatus::Failed
        } else {
            UiStatus::Active
        };
        let mut line = String::new();
        let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
        line.push(' ');
        let _ = palette::write_bold_colored(&mut line, &status.glyph().to_string(), status.color());
        line.push(' ');
        let _ = palette::write_bold_colored(&mut line, &worker.worker, status.color());
        line.push_str("  ");
        let _ = palette::write_colored(
            &mut line,
            &format!(
                "{}  ·  last seen {}  ·  last cycle {}  ·  ran {}  ·  failed {}",
                worker.kind,
                short_timestamp(&worker.last_seen),
                short_timestamp(&worker.last_cycle),
                worker.ran,
                worker.failed,
            ),
            DIM,
        );
        println!("{line}");
    }
}

fn render_run_summary(summary: &AutoflowRunSummary) {
    let run_status = claim_status_ui(&summary.status);
    let mut line = String::new();
    push_serve_frame_open(&mut line, 2);
    line.push(' ');
    let _ = palette::write_bold_colored(
        &mut line,
        &run_status.glyph().to_string(),
        run_status.color(),
    );
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, &summary.workflow, run_status.color());
    line.push(' ');
    let rule = "─".repeat(6);
    let _ = palette::write_colored(&mut line, &rule, palette::BRAND_300);
    line.push_str("  ");
    let _ = palette::write_colored(
        &mut line,
        &format!(
            "run {}  ·  {}",
            short_run_id(&summary.run_id),
            summary.status
        ),
        DIM,
    );
    println!("{line}");

    let mut execution = vec![(
        "steps",
        format!("{}/{}", completed_steps(summary), summary.steps.len()),
    )];
    if let Some(duration_ms) = summary.duration_ms {
        execution.push((
            "time",
            format_duration(std::time::Duration::from_millis(duration_ms)),
        ));
    }
    if let Some(worker) = summary.worker.as_deref() {
        execution.push(("worker", truncate_text(worker, 18)));
    }
    if let Some(backend) = summary.backend.as_deref() {
        execution.push(("backend", truncate_text(backend, 18)));
    }
    render_frame_key_value_detail(2, "⏱", palette::RUNNING, &execution);

    let model_value = compact_join(&summary.models, 1, 28);
    let provider_value = compact_join(&summary.providers, 1, 18);
    let agent_value = compact_join(&summary.agents, 2, 32);
    let mut agent_line = Vec::new();
    if !agent_value.is_empty() {
        agent_line.push(("agents", agent_value));
    }
    if !provider_value.is_empty() {
        agent_line.push(("provider", provider_value));
    }
    if !model_value.is_empty() {
        agent_line.push(("model", model_value));
    }
    if !agent_line.is_empty() {
        render_frame_key_value_detail(2, "⚙", palette::BRAND, &agent_line);
    }

    render_frame_key_value_detail(2, "◈", palette::AWAITING, &run_usage_pairs(summary));
    render_frame_key_value_detail(2, "✎", palette::COMPLETE, &run_change_pairs(summary));
    if summary.awaiting_step.is_some() || summary.error.is_some() || !summary.steps.is_empty() {
        render_frame_spacer(2);
    }
    if let Some(step) = summary.awaiting_step.as_deref() {
        render_frame_note_detail(2, "awaiting", step);
    }
    if let Some(error) = summary.error.as_deref() {
        render_frame_note_detail(2, "error", &truncate_text(error, 96));
    }
    for step in summary.steps.iter().take(5) {
        let mut step_line = String::new();
        push_serve_body_prefix(&mut step_line, 2);
        let _ = palette::write_bold_colored(
            &mut step_line,
            &step.status.glyph().to_string(),
            step.status.color(),
        );
        step_line.push(' ');
        let _ = palette::write_colored(
            &mut step_line,
            &truncate_text(&step.step_id, 28),
            step.status.color(),
        );
        if let Some(detail) = step.detail.as_deref() {
            step_line.push_str("  ");
            let _ = palette::write_colored(&mut step_line, &truncate_text(detail, 60), DIM);
        }
        println!("{step_line}");
    }
    let mut close = String::new();
    push_serve_frame_close(&mut close, 2);
    close.push(' ');
    let _ = palette::write_colored(
        &mut close,
        &format!("{}  ·  watch {}", summary.status, summary.run_id),
        run_status.color(),
    );
    println!("{close}");
}

fn render_wake_summary(wakes: &AutoflowMonitorWakeSummary) {
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "╰─", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_colored(
        &mut line,
        &format!(
            "wakes  queued={}  due={}  processed_recent={}",
            wakes.queued, wakes.due, wakes.processed_recent
        ),
        DIM,
    );
    println!("{line}");
}

fn render_queue_section(wakes: &AutoflowMonitorWakeSummary) {
    render_section_heading("queue");
    render_key_value_detail(
        0,
        "◌",
        palette::AWAITING,
        &[
            ("queued", wakes.queued.to_string()),
            ("due", wakes.due.to_string()),
            ("processed", wakes.processed_recent.to_string()),
        ],
    );
}

fn render_section_heading(title: &str) {
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_colored(&mut line, title, DIM);
    println!("{line}");
}

fn render_dim_branch(message: &str) {
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_colored(&mut line, "○", palette::SKIPPED);
    line.push(' ');
    let _ = palette::write_colored(&mut line, message, DIM);
    println!("{line}");
}

fn render_dim_detail(message: &str) {
    let mut line = String::new();
    let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_colored(&mut line, &format!("  {message}"), DIM);
    println!("{line}");
}

fn render_frame_subheading(indent: usize, title: &str) {
    let mut line = String::new();
    push_serve_body_prefix(&mut line, indent);
    let _ = palette::write_colored(&mut line, title, DIM);
    println!("{line}");
}

fn render_key_value_detail(
    indent: usize,
    icon: &str,
    color: owo_colors::Rgb,
    items: &[(&str, String)],
) {
    if items.is_empty() {
        return;
    }
    let mut line = String::new();
    if indent == 0 {
        let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
        line.push(' ');
    } else {
        push_serve_body_prefix(&mut line, indent);
    }
    let _ = palette::write_bold_colored(&mut line, icon, color);
    line.push(' ');
    append_key_value_segments(&mut line, items);
    println!("{line}");
}

fn render_frame_key_value_detail(
    indent: usize,
    icon: &str,
    color: owo_colors::Rgb,
    items: &[(&str, String)],
) {
    render_key_value_detail(indent, icon, color, items);
}

fn render_frame_note_detail(indent: usize, label: &str, message: &str) {
    let mut line = String::new();
    push_serve_body_prefix(&mut line, indent);
    let _ = palette::write_bold_colored(&mut line, label, palette::BRAND_300);
    line.push(' ');
    let _ = palette::write_colored(&mut line, message, DIM);
    println!("{line}");
}

fn render_frame_run_detail(indent: usize, message: &str) {
    let mut line = String::new();
    push_serve_body_prefix(&mut line, indent);
    let _ = palette::write_colored(&mut line, &format!("  {message}"), DIM);
    println!("{line}");
}

fn render_frame_spacer(indent: usize) {
    let mut line = String::new();
    if indent == 0 {
        let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    } else {
        push_serve_indent_pipes(&mut line, indent);
        let _ = palette::write_colored(&mut line, "│", palette::BRAND_300);
    }
    println!("{line}");
}

fn render_frame_event(indent: usize, event: &AutoflowCycleEvent) {
    let (status, label) = cycle_event_status_and_label(event);
    let mut line = String::new();
    push_serve_body_prefix(&mut line, indent);
    let _ = palette::write_bold_colored(&mut line, &status.glyph().to_string(), status.color());
    line.push(' ');
    let _ = palette::write_bold_colored(&mut line, label, status.color());
    if let Some(workflow) = event.workflow.as_deref() {
        line.push_str("  ");
        let _ = palette::write_colored(&mut line, &truncate_text(workflow, 28), DIM);
    }
    if let Some(run_id) = event.run_id.as_deref() {
        line.push_str("  ");
        let _ = palette::write_colored(&mut line, run_id, DIM);
    }
    println!("{line}");
    if let Some(detail) = event.detail.as_deref().filter(|detail| !detail.is_empty()) {
        render_frame_run_detail(indent, &truncate_text(detail, 112));
    }
}

fn push_serve_frame_open(buf: &mut String, indent: usize) {
    push_serve_indent_pipes(buf, indent);
    let _ = palette::write_colored(buf, "├─", palette::BRAND_300);
    let _ = palette::write_colored(buf, "╭─", palette::BRAND_300);
}

fn push_serve_frame_close(buf: &mut String, indent: usize) {
    push_serve_indent_pipes(buf, indent);
    let _ = palette::write_colored(buf, "│", palette::BRAND_300);
    buf.push(' ');
    let _ = palette::write_colored(buf, "╰─", palette::BRAND_300);
}

fn push_serve_body_prefix(buf: &mut String, indent: usize) {
    push_serve_indent_pipes(buf, indent);
    let _ = palette::write_colored(buf, "│", palette::BRAND_300);
    buf.push(' ');
    let _ = palette::write_colored(buf, "┃", BRAND);
    buf.push_str("  ");
}

fn push_serve_indent_pipes(buf: &mut String, indent: usize) {
    for _ in 0..indent {
        let _ = palette::write_colored(buf, "│", palette::BRAND_300);
        buf.push(' ');
    }
}

fn push_frame_detail_line(out: &mut String, indent: usize, message: &str) {
    push_serve_body_prefix(out, indent);
    let _ = palette::write_colored(out, message, DIM);
    out.push('\n');
}

fn append_key_value_segments(buf: &mut String, items: &[(&str, String)]) {
    let mut first = true;
    for (label, value) in items.iter() {
        if value.is_empty() || value == "-" || value == "—" {
            continue;
        }
        if !first {
            let _ = palette::write_colored(buf, "  ·  ", DIM);
        }
        first = false;
        let _ = palette::write_bold_colored(buf, label, palette::BRAND_300);
        buf.push(' ');
        let _ = palette::write_colored(buf, value, DIM);
    }
}

fn claim_status_ui(status: &str) -> UiStatus {
    match status {
        "claimed" | "running" => UiStatus::Working,
        "await_human" | "await_external" => UiStatus::Awaiting,
        "retry_backoff" => UiStatus::Retrying,
        "blocked" => UiStatus::Failed,
        "complete" => UiStatus::Complete,
        "released" => UiStatus::Skipped,
        _ => UiStatus::Waiting,
    }
}

fn activity_status_and_label(event: &str) -> (UiStatus, &'static str) {
    match event {
        "claim_acquired" => (UiStatus::Active, "picked up"),
        "claim_released" => (UiStatus::Skipped, "released"),
        "claim_takeover" => (UiStatus::Active, "took over"),
        "run_launched" => (UiStatus::Working, "launched run"),
        "issue_commented" => (UiStatus::Working, "commented"),
        "issue_closed" => (UiStatus::Complete, "closed issue"),
        "issue_reopened" => (UiStatus::Active, "reopened issue"),
        "issue_state_changed" => (UiStatus::Working, "updated issue"),
        "pr_opened" => (UiStatus::Active, "opened PR"),
        "draft_pr_opened" => (UiStatus::Active, "opened draft PR"),
        "awaiting_human" => (UiStatus::Awaiting, "awaiting approval"),
        "awaiting_external" => (UiStatus::Awaiting, "awaiting external"),
        "retry_scheduled" => (UiStatus::Retrying, "scheduled retry"),
        "dispatch_queued" => (UiStatus::Working, "queued dispatch"),
        "cleanup_performed" => (UiStatus::Skipped, "cleaned up"),
        "cycle_failed" => (UiStatus::Failed, "cycle failed"),
        _ => (UiStatus::Waiting, "updated"),
    }
}

fn cycle_event_status_and_label(event: &AutoflowCycleEvent) -> (UiStatus, &'static str) {
    match event.kind {
        rupu_runtime::AutoflowCycleEventKind::ClaimAcquired => (UiStatus::Active, "picked up"),
        rupu_runtime::AutoflowCycleEventKind::ClaimReleased => (UiStatus::Skipped, "released"),
        rupu_runtime::AutoflowCycleEventKind::ClaimTakeover => (UiStatus::Active, "took over"),
        rupu_runtime::AutoflowCycleEventKind::RunLaunched => (UiStatus::Working, "launched run"),
        rupu_runtime::AutoflowCycleEventKind::IssueCommented => (UiStatus::Working, "commented"),
        rupu_runtime::AutoflowCycleEventKind::IssueStateChanged => match event.status.as_deref() {
            Some("closed") => (UiStatus::Complete, "closed issue"),
            Some("open") => (UiStatus::Active, "reopened issue"),
            _ => (UiStatus::Working, "updated issue"),
        },
        rupu_runtime::AutoflowCycleEventKind::PullRequestOpened => match event.status.as_deref() {
            Some("draft") => (UiStatus::Active, "opened draft PR"),
            _ => (UiStatus::Active, "opened PR"),
        },
        rupu_runtime::AutoflowCycleEventKind::AwaitingHuman => {
            (UiStatus::Awaiting, "awaiting approval")
        }
        rupu_runtime::AutoflowCycleEventKind::AwaitingExternal => {
            (UiStatus::Awaiting, "awaiting external")
        }
        rupu_runtime::AutoflowCycleEventKind::RetryScheduled => {
            (UiStatus::Retrying, "scheduled retry")
        }
        rupu_runtime::AutoflowCycleEventKind::DispatchQueued => {
            (UiStatus::Working, "queued dispatch")
        }
        rupu_runtime::AutoflowCycleEventKind::CleanupPerformed => (UiStatus::Skipped, "cleaned up"),
        rupu_runtime::AutoflowCycleEventKind::CycleFailed => (UiStatus::Failed, "cycle failed"),
        rupu_runtime::AutoflowCycleEventKind::CycleSkipped => (UiStatus::Waiting, "skipped"),
        rupu_runtime::AutoflowCycleEventKind::WakeConsumed => (UiStatus::Waiting, "consumed wake"),
        rupu_runtime::AutoflowCycleEventKind::WakeSkipped => (UiStatus::Skipped, "skipped wake"),
    }
}

fn short_timestamp(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| truncate_text(value, 24))
}

fn compact_timestamp(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| truncate_text(value, 24))
}

fn load_run_summary(
    run_store: &RunStore,
    run_id: &str,
    pricing: &rupu_config::PricingConfig,
) -> Option<AutoflowRunSummary> {
    let record = run_store.load(run_id).ok()?;
    let rows = run_store.read_step_results(run_id).unwrap_or_default();
    let transcript = summarize_run_transcripts(&rows, pricing);
    let diff = summarize_workspace_diff(&record.workspace_path);
    let usage = usage_summary_from_transcript(&transcript);
    let duration_ms = run_duration_ms(&record);
    let workspace = Some(record.workspace_path.display().to_string());
    let run_id = record.id.clone();
    let workflow = record.workflow_name.clone();
    let status = record.status.as_str().to_string();
    let awaiting_step = record.awaiting_step_id.clone();
    let error = record.error_message.clone();
    let worker = record.worker_id.clone();
    let backend = record.backend_id.clone();
    let started_at = record.started_at;
    let finished_at = record.finished_at;
    let steps = rows
        .into_iter()
        .map(|row| {
            let status = step_status_ui(&row);
            let detail = step_summary_detail(&row);
            AutoflowRunStepSummary {
                step_id: row.step_id,
                status,
                detail,
            }
        })
        .collect::<Vec<_>>();
    Some(AutoflowRunSummary {
        run_id,
        workflow,
        status,
        awaiting_step,
        error,
        worker,
        backend,
        started_at,
        finished_at,
        duration_ms: Some(duration_ms),
        workspace,
        agents: transcript.agents.into_iter().collect(),
        providers: transcript.providers.into_iter().collect(),
        models: transcript.models.into_iter().collect(),
        assistant_messages: transcript.assistant_messages,
        tool_calls: transcript.tool_calls,
        command_runs: transcript.command_runs,
        actions_emitted: transcript.actions_emitted,
        file_edit_events: transcript.file_edit_events,
        file_creates: transcript.file_creates,
        file_modifies: transcript.file_modifies,
        file_deletes: transcript.file_deletes,
        usage,
        diff,
        steps,
    })
}

fn summarize_run_transcripts(
    rows: &[StepResultRecord],
    pricing: &rupu_config::PricingConfig,
) -> TranscriptSummaryAccumulator {
    let mut out = TranscriptSummaryAccumulator::default();
    let mut seen = BTreeSet::new();
    for row in rows {
        summarize_transcript_path(&row.transcript_path, pricing, &mut out, &mut seen);
        for item in &row.items {
            summarize_transcript_path(&item.transcript_path, pricing, &mut out, &mut seen);
        }
    }
    out
}

fn summarize_transcript_path(
    path: &Path,
    pricing: &rupu_config::PricingConfig,
    out: &mut TranscriptSummaryAccumulator,
    seen: &mut BTreeSet<PathBuf>,
) {
    if !seen.insert(path.to_path_buf()) {
        return;
    }

    let Ok(iter) = JsonlReader::iter(path) else {
        return;
    };
    let mut transcript_agent = String::new();
    for event in iter.flatten() {
        match event {
            TranscriptEvent::RunStart {
                agent,
                provider,
                model,
                ..
            } => {
                transcript_agent = agent.clone();
                out.agents.insert(agent);
                out.providers.insert(provider);
                out.models.insert(model);
            }
            TranscriptEvent::AssistantMessage { content, .. } => {
                if !content.trim().is_empty() {
                    out.assistant_messages += 1;
                }
            }
            TranscriptEvent::ToolCall { .. } => out.tool_calls += 1,
            TranscriptEvent::CommandRun { .. } => out.command_runs += 1,
            TranscriptEvent::ActionEmitted { .. } => out.actions_emitted += 1,
            TranscriptEvent::FileEdit { kind, .. } => {
                out.file_edit_events += 1;
                match kind {
                    FileEditKind::Create => out.file_creates += 1,
                    FileEditKind::Modify => out.file_modifies += 1,
                    FileEditKind::Delete => out.file_deletes += 1,
                }
            }
            TranscriptEvent::Usage {
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_tokens,
            } => {
                out.providers.insert(provider.clone());
                out.models.insert(model.clone());
                out.usage.input_tokens += u64::from(input_tokens);
                out.usage.output_tokens += u64::from(output_tokens);
                out.usage.cached_tokens += u64::from(cached_tokens);
                let cost = crate::pricing::lookup(pricing, &provider, &model, &transcript_agent)
                    .map(|price| {
                        price.cost_usd(
                            u64::from(input_tokens),
                            u64::from(output_tokens),
                            u64::from(cached_tokens),
                        )
                    });
                out.usage.cost.add(cost);
            }
            _ => {}
        }
    }
}

fn usage_summary_from_transcript(
    transcript: &TranscriptSummaryAccumulator,
) -> Option<AutoflowUsageSummary> {
    let total_tokens = transcript.usage.input_tokens + transcript.usage.output_tokens;
    if total_tokens == 0 && transcript.usage.cached_tokens == 0 {
        return None;
    }
    Some(AutoflowUsageSummary {
        input_tokens: transcript.usage.input_tokens,
        output_tokens: transcript.usage.output_tokens,
        cached_tokens: transcript.usage.cached_tokens,
        total_tokens,
        cost_usd: transcript.usage.cost.cost_usd(),
        cost_partial: transcript.usage.cost.partial(),
    })
}

fn summarize_workspace_diff(path: &Path) -> Option<AutoflowWorkspaceDiffSummary> {
    let status_out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !status_out.status.success() {
        return None;
    }

    let mut created = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    let mut renamed = 0usize;
    let mut changed_files = BTreeSet::new();
    let mut top_files = Vec::new();
    let mut untracked = Vec::new();

    for line in String::from_utf8_lossy(&status_out.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let raw_path = line[3..].trim();
        let path_name = raw_path
            .split(" -> ")
            .last()
            .unwrap_or(raw_path)
            .trim()
            .to_string();
        if path_name.is_empty() {
            continue;
        }
        if changed_files.insert(path_name.clone()) && top_files.len() < 4 {
            top_files.push(path_name.clone());
        }

        let mut chars = status.chars();
        let left = chars.next().unwrap_or(' ');
        let right = chars.next().unwrap_or(' ');
        if status == "??" {
            created += 1;
            untracked.push(path.join(&path_name));
        } else if left == 'R' || right == 'R' {
            renamed += 1;
        } else if left == 'D' || right == 'D' {
            deleted += 1;
        } else if left == 'A' || right == 'A' {
            created += 1;
        } else {
            modified += 1;
        }
    }

    let numstat_out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["diff", "--numstat", "--find-renames", "HEAD", "--"])
        .output()
        .ok();
    let mut insertions = 0u64;
    let mut deletions = 0u64;
    if let Some(out) = numstat_out.filter(|out| out.status.success()) {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next().unwrap_or("0");
            let removed = parts.next().unwrap_or("0");
            insertions += added.parse::<u64>().unwrap_or(0);
            deletions += removed.parse::<u64>().unwrap_or(0);
        }
    }
    for file in untracked {
        insertions += count_file_lines(&file).unwrap_or(0);
    }

    Some(AutoflowWorkspaceDiffSummary {
        files_changed: changed_files.len(),
        created,
        modified,
        deleted,
        renamed,
        insertions,
        deletions,
        top_files,
        merge_target: detect_origin_default_branch(path),
    })
}

fn count_file_lines(path: &Path) -> Option<u64> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(content.lines().count() as u64)
}

fn detect_origin_default_branch(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .ok()?;
    if out.status.success() {
        let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
        if let Some(branch) = value.strip_prefix("origin/") {
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }

    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn run_duration_ms(record: &rupu_orchestrator::RunRecord) -> u64 {
    match record.finished_at {
        Some(finished_at) => (finished_at - record.started_at).num_milliseconds().max(0) as u64,
        None => (chrono::Utc::now() - record.started_at)
            .num_milliseconds()
            .max(0) as u64,
    }
}

fn step_status_ui(step: &StepResultRecord) -> UiStatus {
    if step.skipped {
        UiStatus::Skipped
    } else if step.success {
        UiStatus::Complete
    } else {
        UiStatus::Failed
    }
}

fn step_summary_detail(step: &StepResultRecord) -> Option<String> {
    if step.skipped {
        return Some("skipped".into());
    }
    if !step.findings.is_empty() {
        return Some(format!("{} finding(s)", step.findings.len()));
    }
    if !step.items.is_empty() {
        let ok = step.items.iter().filter(|item| item.success).count();
        return Some(format!("{ok}/{} item(s) ok", step.items.len()));
    }
    let first_line = step
        .output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first_line.is_empty() {
        return None;
    }
    Some(first_line.to_string())
}

fn completed_steps(summary: &AutoflowRunSummary) -> usize {
    summary
        .steps
        .iter()
        .filter(|step| matches!(step.status, UiStatus::Complete | UiStatus::Skipped))
        .count()
}

fn format_run_models_line(summary: &AutoflowRunSummary) -> String {
    let agents = if summary.agents.is_empty() {
        "—".into()
    } else {
        truncate_text(&summary.agents.join(", "), 28)
    };
    let providers = if summary.providers.is_empty() {
        "—".into()
    } else {
        truncate_text(&summary.providers.join(", "), 24)
    };
    let models = if summary.models.is_empty() {
        "—".into()
    } else {
        truncate_text(&summary.models.join(", "), 36)
    };
    format!("agents {agents}  ·  providers {providers}  ·  models {models}")
}

fn format_run_usage_line(summary: &AutoflowRunSummary) -> String {
    let counts = format!(
        "messages {}  ·  tools {}  ·  commands {}  ·  actions {}",
        format_count(summary.assistant_messages),
        format_count(summary.tool_calls),
        format_count(summary.command_runs),
        format_count(summary.actions_emitted),
    );
    match &summary.usage {
        Some(usage) => format!(
            "{counts}  ·  tokens {} in / {} out / {} cached  ·  total {}  ·  cost {}",
            format_count(usage.input_tokens),
            format_count(usage.output_tokens),
            format_count(usage.cached_tokens),
            format_count(usage.total_tokens),
            format_cost(usage.cost_usd, usage.cost_partial),
        ),
        None => counts,
    }
}

fn format_run_diff_line(summary: &AutoflowRunSummary) -> String {
    let edit_counts = format!(
        "edit events {}  ·  creates {}  ·  modifies {}  ·  deletes {}",
        format_count(summary.file_edit_events),
        format_count(summary.file_creates),
        format_count(summary.file_modifies),
        format_count(summary.file_deletes),
    );
    match &summary.diff {
        Some(diff) => {
            let mut parts = vec![format!(
                "workspace {} file(s)  ·  {} new  ·  {} modified  ·  {} deleted  ·  {} renamed  ·  +{}/-{}",
                diff.files_changed,
                diff.created,
                diff.modified,
                diff.deleted,
                diff.renamed,
                format_count(diff.insertions),
                format_count(diff.deletions),
            )];
            if !diff.top_files.is_empty() {
                parts.push(format!(
                    "files {}",
                    truncate_text(&diff.top_files.join(", "), 44)
                ));
            }
            if let Some(target) = diff.merge_target.as_deref() {
                parts.push(format!("merge -> {target}"));
            }
            format!("{edit_counts}  ·  {}", parts.join("  ·  "))
        }
        None => edit_counts,
    }
}

fn format_run_metrics_line(summary: &AutoflowRunSummary) -> String {
    let mut parts = vec![format!(
        "steps {}/{}",
        completed_steps(summary),
        summary.steps.len()
    )];
    if let Some(duration_ms) = summary.duration_ms {
        parts.push(format!(
            "duration {}",
            format_duration(std::time::Duration::from_millis(duration_ms))
        ));
    }
    parts.push(format!("tools {}", format_count(summary.tool_calls)));
    if let Some(usage) = &summary.usage {
        parts.push(format!("tokens {}", format_count(usage.total_tokens)));
        parts.push(format!(
            "cost {}",
            format_cost(usage.cost_usd, usage.cost_partial)
        ));
    }
    if let Some(diff) = &summary.diff {
        parts.push(format!(
            "diff {} file(s) +{}/-{}",
            diff.files_changed,
            format_count(diff.insertions),
            format_count(diff.deletions)
        ));
    }
    parts.join("  ·  ")
}

fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn format_compact_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => compact_with_suffix(n as f64 / 1_000.0, "k"),
        1_000_000..=999_999_999 => compact_with_suffix(n as f64 / 1_000_000.0, "M"),
        _ => compact_with_suffix(n as f64 / 1_000_000_000.0, "B"),
    }
}

fn compact_with_suffix(value: f64, suffix: &str) -> String {
    if (value.fract() - 0.0).abs() < f64::EPSILON {
        format!("{value:.0}{suffix}")
    } else {
        format!("{value:.1}{suffix}")
    }
}

fn format_cost(cost_usd: Option<f64>, partial: bool) -> String {
    match cost_usd {
        Some(value) => format!("${value:.4}{}", if partial { "*" } else { "" }),
        None => "—".into(),
    }
}

fn run_usage_pairs(summary: &AutoflowRunSummary) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("msg", format_compact_count(summary.assistant_messages)),
        ("tools", format_compact_count(summary.tool_calls)),
    ];
    if summary.command_runs > 0 {
        out.push(("cmd", format_compact_count(summary.command_runs)));
    }
    if summary.actions_emitted > 0 {
        out.push(("act", format_compact_count(summary.actions_emitted)));
    }
    if let Some(usage) = &summary.usage {
        out.push((
            "tok",
            format!(
                "{} in / {} out",
                format_compact_count(usage.input_tokens),
                format_compact_count(usage.output_tokens)
            ),
        ));
        if usage.cached_tokens > 0 {
            out.push(("cache", format_compact_count(usage.cached_tokens)));
        }
        if let Some(cost) = usage.cost_usd {
            out.push(("cost", format_cost(Some(cost), usage.cost_partial)));
        }
    }
    out
}

fn run_change_pairs(summary: &AutoflowRunSummary) -> Vec<(&'static str, String)> {
    if let Some(diff) = &summary.diff {
        if diff.files_changed == 0
            && diff.insertions == 0
            && diff.deletions == 0
            && summary.file_edit_events == 0
        {
            return vec![("changes", "none".into())];
        }

        let mut out = vec![(
            "changes",
            format!(
                "{} file{}  +{}/-{}",
                diff.files_changed,
                if diff.files_changed == 1 { "" } else { "s" },
                format_compact_count(diff.insertions),
                format_compact_count(diff.deletions),
            ),
        )];
        if !diff.top_files.is_empty() {
            out.push(("files", truncate_text(&diff.top_files.join(", "), 34)));
        }
        if let Some(target) = diff.merge_target.as_deref() {
            out.push(("merge", target.to_string()));
        }
        return out;
    }
    if summary.file_edit_events == 0 {
        vec![("changes", "none".into())]
    } else {
        vec![(
            "changes",
            format!(
                "{} edit{}",
                summary.file_edit_events,
                if summary.file_edit_events == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        )]
    }
}

fn compact_join(values: &[String], max_items: usize, max_len: usize) -> String {
    if values.is_empty() {
        return String::new();
    }
    let mut shown = values.iter().take(max_items).cloned().collect::<Vec<_>>();
    if values.len() > max_items {
        shown.push(format!("+{}", values.len() - max_items));
    }
    truncate_text(&shown.join(", "), max_len)
}

fn human_tracker_name(value: &str) -> &str {
    match value {
        "github" => "GitHub",
        "jira" => "Jira",
        "linear" => "Linear",
        other => other,
    }
}

fn short_locator(value: &str, max: usize) -> String {
    let trimmed = value
        .strip_prefix("github:")
        .or_else(|| value.strip_prefix("jira:"))
        .or_else(|| value.strip_prefix("linear:"))
        .unwrap_or(value);
    truncate_text(trimmed, max)
}

fn short_url_like(value: &str, max: usize) -> String {
    let trimmed = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    truncate_text(trimmed, max)
}

fn short_run_id(value: &str) -> String {
    if value.chars().count() <= 22 {
        return value.to_string();
    }
    let chars = value.chars().collect::<Vec<_>>();
    let head = chars.iter().take(12).collect::<String>();
    let tail = chars
        .iter()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{head}…{tail}")
}

fn display_issue_headline(claim: &AutoflowClaimRow) -> String {
    let display = claim.issue_display.as_deref().unwrap_or(&claim.issue);
    match claim.tracker.as_deref() {
        Some("github") if display.chars().all(|ch| ch.is_ascii_digit()) => {
            format!("GitHub #{display}")
        }
        Some("jira") | Some("linear") => display.to_string(),
        Some(tracker) => format!("{} {}", human_tracker_name(tracker), display),
        None => display.to_string(),
    }
}

fn truncate_text(value: &str, max: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= max {
        return value.to_string();
    }
    let keep = max.saturating_sub(1);
    format!("{}…", chars.into_iter().take(keep).collect::<String>())
}

fn autoflow_pricing_config() -> rupu_config::PricingConfig {
    autoflow_ui_config()
        .map(|cfg| cfg.pricing)
        .unwrap_or_default()
}

fn autoflow_ui_config() -> anyhow::Result<Config> {
    let global = paths::global_dir()?;
    let Ok(pwd) = std::env::current_dir() else {
        return Ok(Config::default());
    };
    let project_root = paths::project_root_for(&pwd)?;
    resolve_config(&global, project_root.as_deref())
}

fn autoflow_ui_prefs() -> anyhow::Result<UiPrefs> {
    let cfg = autoflow_ui_config().unwrap_or_default();
    Ok(UiPrefs::resolve(&cfg.ui, false, None, None, None))
}

fn render_autoflow_show_summary(
    entry: &VisibleAutoflowWorkflow,
    autoflow: &rupu_orchestrator::Autoflow,
) -> String {
    let mut out = String::new();
    let _ = palette::write_colored(&mut out, "▶", BRAND);
    out.push(' ');
    let _ = palette::write_bold_colored(&mut out, &entry.name, BRAND);
    out.push_str("  ");
    let _ = palette::write_colored(&mut out, entry.repo_ref.as_deref().unwrap_or("-"), DIM);
    out.push('\n');
    out.push('\n');

    let entity = match autoflow.entity {
        rupu_orchestrator::AutoflowEntity::Issue => "issue",
    };
    let source = autoflow
        .source
        .as_deref()
        .unwrap_or(entry.repo_ref.as_deref().unwrap_or("-"));
    let workspace = autoflow
        .workspace
        .as_ref()
        .map(|w| match w.strategy {
            AutoflowWorkspaceStrategy::Worktree => "worktree",
            AutoflowWorkspaceStrategy::InPlace => "in_place",
        })
        .unwrap_or("worktree");

    push_serve_frame_open(&mut out, 0);
    out.push(' ');
    let _ = palette::write_bold_colored(&mut out, "◐", BRAND);
    out.push(' ');
    let _ = palette::write_bold_colored(&mut out, &entry.name, BRAND);
    out.push(' ');
    let _ = palette::write_colored(&mut out, &"─".repeat(6), palette::BRAND_300);
    out.push_str("  ");
    let _ = palette::write_colored(
        &mut out,
        &format!(
            "entity: {entity}  ·  source: {source}  ·  priority: {}",
            autoflow.priority
        ),
        DIM,
    );
    out.push('\n');

    push_frame_detail_line(
        &mut out,
        0,
        &format!("scope: {}  ·  workspace: {}", entry.scope, workspace),
    );
    if let Some(branch) = autoflow
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.branch.as_deref())
    {
        push_frame_detail_line(&mut out, 0, &format!("workspace branch: {}", branch));
    }
    if let Some(reconcile_every) = autoflow.reconcile_every.as_deref() {
        push_frame_detail_line(&mut out, 0, &format!("reconcile every: {reconcile_every}"));
    }
    if let Some(ttl) = autoflow
        .claim
        .as_ref()
        .and_then(|claim| claim.ttl.as_deref())
    {
        push_frame_detail_line(&mut out, 0, &format!("claim ttl: {ttl}"));
    }
    if let Some(outcome) = &autoflow.outcome {
        push_frame_detail_line(&mut out, 0, &format!("outcome output: {}", outcome.output));
    }
    if let Some(project_root) = &entry.project_root {
        push_frame_detail_line(
            &mut out,
            0,
            &format!("project root: {}", project_root.display()),
        );
    }
    if let Some(preferred_checkout) = &entry.preferred_checkout {
        push_frame_detail_line(
            &mut out,
            0,
            &format!("preferred checkout: {}", preferred_checkout.display()),
        );
    }
    push_frame_detail_line(
        &mut out,
        0,
        &format!("path: {}", entry.workflow_path.display()),
    );
    if !autoflow.wake_on.is_empty() {
        push_frame_detail_line(
            &mut out,
            0,
            &format!("wake on: {}", autoflow.wake_on.join(", ")),
        );
    }

    if !autoflow.selector.labels_all.is_empty() {
        push_frame_detail_line(
            &mut out,
            0,
            &format!("labels all: {}", autoflow.selector.labels_all.join(", ")),
        );
    }
    if !autoflow.selector.labels_any.is_empty() {
        push_frame_detail_line(
            &mut out,
            0,
            &format!("labels any: {}", autoflow.selector.labels_any.join(", ")),
        );
    }
    if !autoflow.selector.labels_none.is_empty() {
        push_frame_detail_line(
            &mut out,
            0,
            &format!("labels none: {}", autoflow.selector.labels_none.join(", ")),
        );
    }
    if !autoflow.selector.states.is_empty() {
        let states = autoflow
            .selector
            .states
            .iter()
            .map(|state| match state {
                rupu_orchestrator::AutoflowIssueState::Open => "open",
                rupu_orchestrator::AutoflowIssueState::Closed => "closed",
            })
            .collect::<Vec<_>>()
            .join(", ");
        push_frame_detail_line(&mut out, 0, &format!("selector states: {states}"));
    }
    if let Some(limit) = autoflow.selector.limit {
        push_frame_detail_line(&mut out, 0, &format!("selector limit: {limit}"));
    }

    push_serve_frame_close(&mut out, 0);
    out.push(' ');
    let _ = palette::write_colored(&mut out, "summary", DIM);
    out.push('\n');

    out.push('\n');
    let mut yaml_title = String::new();
    let _ = palette::write_colored(&mut yaml_title, "workflow yaml", DIM);
    let _ = writeln!(&mut out, "{yaml_title}");
    let _ = writeln!(&mut out, "{}", "─".repeat(80));
    out
}

fn render_history_table(report: &AutoflowHistoryReport) -> anyhow::Result<()> {
    if report.rows.is_empty() {
        println!("(no history)");
        return Ok(());
    }
    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "At", "Event", "Issue", "Source", "Workflow", "Repo", "Worker", "Run", "Detail",
    ]);
    for row in &report.rows {
        table.add_row(vec![
            Cell::new(compact_timestamp(&row.at)),
            Cell::new(&row.event),
            Cell::new(&row.issue),
            Cell::new(&row.source),
            Cell::new(&row.workflow),
            Cell::new(&row.repo),
            Cell::new(&row.worker),
            Cell::new(&row.run),
            Cell::new(truncate_text(&row.detail, 56)),
        ]);
    }
    println!("{table}");
    Ok(())
}

fn filter_monitor_cycles(
    cycles: Vec<AutoflowCycleRecord>,
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
) -> Vec<AutoflowCycleRecord> {
    cycles
        .into_iter()
        .filter(|cycle| {
            repo_filter.is_none_or(|filter| {
                cycle.repo_filter.as_deref() == Some(filter)
                    || cycle
                        .events
                        .iter()
                        .any(|event| event.repo_ref.as_deref() == Some(filter))
            })
        })
        .filter(|cycle| {
            worker_filter_matches(
                worker_filter,
                cycle.worker_id.as_deref().unwrap_or(""),
                cycle.worker_name.as_deref().unwrap_or(""),
            )
        })
        .collect()
}

fn filter_monitor_events(
    events: Vec<AutoflowHistoryEventRecord>,
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
) -> Vec<AutoflowHistoryEventRecord> {
    events
        .into_iter()
        .filter(|record| {
            repo_filter.is_none_or(|filter| {
                record.repo_filter.as_deref() == Some(filter)
                    || record.event.repo_ref.as_deref() == Some(filter)
            })
        })
        .filter(|record| {
            worker_filter_matches(
                worker_filter,
                record.worker_id.as_deref().unwrap_or(""),
                record.worker_name.as_deref().unwrap_or(""),
            )
        })
        .collect()
}

fn latest_cycle_by_worker(cycles: &[AutoflowCycleRecord]) -> BTreeMap<&str, &AutoflowCycleRecord> {
    let mut out = BTreeMap::new();
    for cycle in cycles {
        if let Some(worker_id) = cycle.worker_id.as_deref() {
            out.entry(worker_id).or_insert(cycle);
        }
        if let Some(worker_name) = cycle.worker_name.as_deref() {
            out.entry(worker_name).or_insert(cycle);
        }
    }
    out
}

fn worker_filter_matches(filter: Option<&str>, worker_id: &str, worker_name: &str) -> bool {
    filter.is_none_or(|filter| filter == worker_id || filter == worker_name)
}

fn monitor_event_name(event: &AutoflowCycleEvent) -> String {
    match event.kind {
        rupu_runtime::AutoflowCycleEventKind::WakeConsumed => "wake_consumed",
        rupu_runtime::AutoflowCycleEventKind::WakeSkipped => "wake_skipped",
        rupu_runtime::AutoflowCycleEventKind::ClaimAcquired => "claim_acquired",
        rupu_runtime::AutoflowCycleEventKind::ClaimReleased => "claim_released",
        rupu_runtime::AutoflowCycleEventKind::ClaimTakeover => "claim_takeover",
        rupu_runtime::AutoflowCycleEventKind::RunLaunched => "run_launched",
        rupu_runtime::AutoflowCycleEventKind::IssueCommented => "issue_commented",
        rupu_runtime::AutoflowCycleEventKind::IssueStateChanged => match event.status.as_deref() {
            Some("closed") => "issue_closed",
            Some("open") => "issue_reopened",
            _ => "issue_state_changed",
        },
        rupu_runtime::AutoflowCycleEventKind::PullRequestOpened => match event.status.as_deref() {
            Some("draft") => "draft_pr_opened",
            _ => "pr_opened",
        },
        rupu_runtime::AutoflowCycleEventKind::AwaitingHuman => "awaiting_human",
        rupu_runtime::AutoflowCycleEventKind::AwaitingExternal => "awaiting_external",
        rupu_runtime::AutoflowCycleEventKind::RetryScheduled => "retry_scheduled",
        rupu_runtime::AutoflowCycleEventKind::DispatchQueued => "dispatch_queued",
        rupu_runtime::AutoflowCycleEventKind::CleanupPerformed => "cleanup_performed",
        rupu_runtime::AutoflowCycleEventKind::CycleSkipped => "cycle_skipped",
        rupu_runtime::AutoflowCycleEventKind::CycleFailed => "cycle_failed",
    }
    .into()
}

fn build_autoflow_live_event_hook(
    claim: &AutoflowClaimRecord,
    live_cycle_recorder: Option<Arc<crate::cmd::autoflow_runtime::LiveCycleRecorder>>,
) -> LiveWorkflowEventHook {
    let issue_ref = claim.issue_ref.clone();
    let claim_snapshot = claim.clone();
    let issue_display = claim
        .issue_display_ref
        .clone()
        .unwrap_or_else(|| issue_ref.clone());
    let tracker = claim
        .issue_tracker
        .as_deref()
        .map(|value| human_tracker_name(value).to_string())
        .unwrap_or_else(|| tracker_name_from_issue_ref(&issue_ref));
    let workflow = claim.workflow.clone();
    Arc::new(move |event| {
        let LiveWorkflowEvent::ToolSucceeded {
            run_id,
            step_id,
            tool,
            input,
            ..
        } = event;
        if let Some(recorder) = live_cycle_recorder.as_ref() {
            if let Some(cycle_event) = crate::cmd::autoflow_runtime::promoted_autoflow_tool_event(
                tool,
                input,
                &claim_snapshot,
                run_id,
                &issue_ref,
            ) {
                if let Err(error) = recorder.record_event(cycle_event) {
                    warn!(%error, issue_ref = %issue_ref, workflow = %workflow, "failed to persist live autoflow event");
                }
            }
        }
        let issue_label = format!("{tracker} {issue_display}");
        let step_label = truncate_text(step_id, 20);
        let workflow_label = truncate_text(&workflow, 28);
        match tool.as_str() {
            "issues.comment" => Some(LiveWorkflowRender {
                status: UiStatus::Working,
                label: format!("{issue_label} commented"),
                detail: Some(format!(
                    "{}  ·  step {}  ·  {}",
                    workflow_label,
                    step_label,
                    comment_preview_from_input(input)
                        .unwrap_or_else(|| "comment posted".to_string())
                )),
            }),
            "issues.update_state" => {
                let state = input
                    .get("state")
                    .and_then(|value| value.as_str())
                    .unwrap_or("updated");
                let (status, label) = match state {
                    "closed" => (UiStatus::Complete, format!("{issue_label} closed")),
                    "open" => (UiStatus::Active, format!("{issue_label} reopened")),
                    other => (
                        UiStatus::Working,
                        format!("{issue_label} → {}", truncate_text(other, 24)),
                    ),
                };
                Some(LiveWorkflowRender {
                    status,
                    label,
                    detail: Some(format!("{workflow_label}  ·  step {step_label}")),
                })
            }
            "scm.prs.create" => {
                let draft = input
                    .get("draft")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                Some(LiveWorkflowRender {
                    status: UiStatus::Active,
                    label: if draft {
                        format!("{issue_label} opened draft PR")
                    } else {
                        format!("{issue_label} opened PR")
                    },
                    detail: Some(format!(
                        "{}  ·  step {}  ·  {}",
                        workflow_label,
                        step_label,
                        pr_preview_from_input(input)
                            .unwrap_or_else(|| "pull request created".into())
                    )),
                })
            }
            _ => None,
        }
    })
}

fn tracker_name_from_issue_ref(issue_ref: &str) -> String {
    issue_ref
        .split_once(':')
        .map(|(tracker, _)| human_tracker_name(tracker).to_string())
        .unwrap_or_else(|| "Issue".to_string())
}

fn comment_preview_from_input(input: &serde_json::Value) -> Option<String> {
    let body = input.get("body").and_then(|value| value.as_str())?;
    let first_line = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first_line.is_empty() {
        None
    } else {
        Some(truncate_text(first_line, 72))
    }
}

fn pr_preview_from_input(input: &serde_json::Value) -> Option<String> {
    let title = input.get("title").and_then(|value| value.as_str())?;
    if title.trim().is_empty() {
        None
    } else {
        Some(truncate_text(title.trim(), 72))
    }
}

fn wake_row_from_record(wake: &WakeRecord, state: &str) -> AutoflowWakeRow {
    AutoflowWakeRow {
        wake_id: wake.wake_id.clone(),
        state: state.to_string(),
        source: wake_source_name(wake.source).into(),
        event: wake.event.id.clone(),
        entity: wake.entity.ref_text.clone(),
        not_before: wake.not_before.clone(),
        repo: wake.repo_ref.clone(),
    }
}

fn render_autoflow_explain_report(report: &AutoflowExplainReport) -> String {
    let item = &report.item;
    let mut out = String::new();
    let _ = writeln!(&mut out, "issue: {}", item.issue);
    let _ = writeln!(&mut out, "repo: {}", item.repo);
    if let Some(claim) = &item.claim {
        if let Some(display_ref) = claim.issue_display.as_deref() {
            let _ = writeln!(&mut out, "issue display: {display_ref}");
        }
        if let Some(title) = claim.issue_title.as_deref() {
            let _ = writeln!(&mut out, "issue title: {title}");
        }
        if let Some(tracker) = claim.issue_tracker.as_deref() {
            let _ = writeln!(&mut out, "issue tracker: {tracker}");
        }
        if let Some(state_name) = claim.issue_state.as_deref() {
            let _ = writeln!(&mut out, "issue state: {state_name}");
        }
        if let Some(source_ref) = claim.source.as_deref() {
            let _ = writeln!(&mut out, "source: {source_ref}");
        }
        if let Some(issue_url) = claim.issue_url.as_deref() {
            let _ = writeln!(&mut out, "issue url: {issue_url}");
        }
        let _ = writeln!(&mut out, "workflow: {}", claim.workflow);
        let _ = writeln!(&mut out, "status: {}", claim.status);
        let _ = writeln!(
            &mut out,
            "priority: {}",
            claim.priority.as_deref().unwrap_or("-")
        );
        let _ = writeln!(&mut out, "contenders: {}", claim.contenders);
        let _ = writeln!(&mut out, "workspace: {}", claim.workspace);
        let _ = writeln!(&mut out, "branch: {}", claim.branch);
        let _ = writeln!(&mut out, "pr: {}", claim.pr);
        let _ = writeln!(&mut out, "claim owner: {}", claim.claim_owner);
        let _ = writeln!(&mut out, "lease expires: {}", claim.lease_expires);
        if let Some(lock) = &claim.active_lock {
            let _ = writeln!(
                &mut out,
                "active lock: owner={} acquired_at={} lease_expires={}",
                lock.owner,
                lock.acquired_at,
                lock.lease_expires.as_deref().unwrap_or("-")
            );
        } else {
            let _ = writeln!(&mut out, "active lock: -");
        }
        if let Some(run) = &claim.last_run {
            let _ = writeln!(&mut out, "last run: {}", run.run_id);
            let _ = writeln!(&mut out, "watch hint: rupu watch {}", run.run_id);
            let _ = writeln!(&mut out, "last run status: {}", run.status);
            let _ = writeln!(&mut out, "last run backend: {}", run.backend);
            let _ = writeln!(&mut out, "last run worker: {}", run.worker);
            if let Some(source_wake) = run.source_wake.as_deref() {
                let _ = writeln!(&mut out, "source wake: {source_wake}");
            }
            if let Some(step) = run.approval_step.as_deref() {
                let _ = writeln!(
                    &mut out,
                    "approval gate: step={} expires={}",
                    step,
                    run.approval_expires.as_deref().unwrap_or("-")
                );
            }
            if let Some(execution) = run.execution.as_deref() {
                let _ = writeln!(&mut out, "last run execution: {execution}");
            }
            if let Some(models) = run.models.as_deref() {
                let _ = writeln!(&mut out, "last run models: {models}");
            }
            if let Some(usage) = run.usage.as_deref() {
                let _ = writeln!(&mut out, "last run usage: {usage}");
            }
            if let Some(changes) = run.changes.as_deref() {
                let _ = writeln!(&mut out, "last run changes: {changes}");
            }
            if let Some(started_at) = run.started_at.as_deref() {
                let _ = writeln!(&mut out, "last run started: {started_at}");
            }
            if let Some(finished_at) = run.finished_at.as_deref() {
                let _ = writeln!(&mut out, "last run finished: {finished_at}");
            }
            if let Some(workspace) = run.workspace.as_deref() {
                let _ = writeln!(&mut out, "last run workspace: {workspace}");
            }
            if let Some(merge_target) = run.merge_target.as_deref() {
                let _ = writeln!(&mut out, "last run merge target: {merge_target}");
            }
        } else {
            let _ = writeln!(&mut out, "last run: -");
        }
        let _ = writeln!(&mut out, "next action: {}", claim.next_action);
        if let Some(dispatch) = &claim.pending_dispatch {
            let _ = writeln!(
                &mut out,
                "pending dispatch: {} target={} inputs={}",
                dispatch.workflow, dispatch.target, dispatch.inputs
            );
        }
        if let Some(next_retry_at) = claim.next_retry.as_deref() {
            let _ = writeln!(&mut out, "next retry: {next_retry_at}");
        }
        if let Some(summary) = claim.summary.as_deref() {
            let _ = writeln!(&mut out, "summary: {summary}");
        }
    } else {
        let _ = writeln!(&mut out, "status: {}", item.status);
        if let Some(source) = item.source.as_deref() {
            let _ = writeln!(&mut out, "source: {source}");
        }
        if !item.candidate_workflows.is_empty() {
            let _ = writeln!(
                &mut out,
                "candidate workflows: {}",
                item.candidate_workflows.join(", ")
            );
        }
    }

    if item.queued_wakes.is_empty() {
        let _ = writeln!(&mut out, "queued wakes: -");
    } else {
        let _ = writeln!(&mut out, "queued wakes:");
        for wake in &item.queued_wakes {
            let _ = writeln!(
                &mut out,
                "- {} {} {} not_before={}",
                wake.wake_id, wake.source, wake.event, wake.not_before
            );
        }
    }
    if let Some(wake) = &item.recent_processed_wake {
        let _ = writeln!(
            &mut out,
            "recent processed wake: {} {} {}",
            wake.wake_id, wake.source, wake.event
        );
    }
    if item.recent_cycle_events.is_empty() {
        let _ = writeln!(&mut out, "recent cycle events: -");
    } else {
        let _ = writeln!(&mut out, "recent cycle events:");
        for row in &item.recent_cycle_events {
            let _ = writeln!(
                &mut out,
                "- {} {} workflow={} run={} detail={}",
                row.at, row.event, row.workflow, row.run, row.detail
            );
        }
    }
    out
}

async fn explain(
    r#ref: &str,
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let issue_ref = canonical_autoflow_issue_ref(r#ref)?;
    let global = paths::global_dir()?;
    let pricing = autoflow_pricing_config();
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
    let run_store = RunStore::new(global.join("runs"));
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
    let claim = claim_store.load(&issue_ref)?;
    let issue_ref_parsed = parse_issue_ref_text(&issue_ref)?;
    let source_matches = if claim.is_none() {
        visible_autoflow_matches_for_issue(&issue_ref_parsed, repo)?
    } else {
        Vec::new()
    };
    let repo_ref = match claim.as_ref() {
        Some(claim) => claim.repo_ref.clone(),
        None => resolve_repo_binding_for_issue(&issue_ref_parsed, repo)?,
    };
    let queued = wakes_for_issue(
        filter_wakes_by_repo(wake_store.list_queued()?, Some(&repo_ref)),
        &issue_ref,
    );
    let mut processed = wakes_for_issue(
        filter_wakes_by_repo(wake_store.list_processed()?, Some(&repo_ref)),
        &issue_ref,
    );
    processed.sort_by(|left, right| right.received_at.cmp(&left.received_at));
    let recent_history = history_rows_from_events(
        &filter_monitor_events(
            history_store.list_recent_events(400)?,
            Some(&repo_ref),
            None,
        ),
        Some(&issue_ref),
        None,
        None,
    )
    .into_iter()
    .take(5)
    .collect::<Vec<_>>();
    let queued_wakes = queued
        .iter()
        .map(|wake| wake_row_from_record(wake, "queued"))
        .collect::<Vec<_>>();
    let recent_processed_wake = processed
        .first()
        .map(|wake| wake_row_from_record(wake, "processed"));
    let candidate_workflows = source_matches
        .iter()
        .map(|entry| entry.name.clone())
        .collect::<Vec<_>>();

    let claim_report = if let Some(claim) = claim {
        let active_lock =
            claim_store
                .read_active_lock(&issue_ref)?
                .map(|lock| AutoflowExplainLock {
                    owner: lock.owner,
                    acquired_at: lock.acquired_at,
                    lease_expires: lock.lease_expires_at,
                });
        let last_run = match claim.last_run_id.as_deref() {
            Some(run_id) => match run_store.load(run_id) {
                Ok(run) => {
                    let summary = load_run_summary(&run_store, run_id, &pricing);
                    Some(AutoflowExplainRun {
                        run_id: run_id.to_string(),
                        status: run.status.as_str().to_string(),
                        backend: run.backend_id.unwrap_or_else(|| "-".into()),
                        worker: run.worker_id.unwrap_or_else(|| "-".into()),
                        source_wake: run.source_wake_id.as_deref().map(|source_wake_id| {
                            describe_wake_source(&global, &wake_store, source_wake_id)
                        }),
                        approval_step: (run.status == RunStatus::AwaitingApproval)
                            .then(|| run.awaiting_step_id.unwrap_or_else(|| "-".into())),
                        approval_expires: (run.status == RunStatus::AwaitingApproval).then(|| {
                            run.expires_at
                                .map(|value| value.to_rfc3339())
                                .unwrap_or_else(|| "-".into())
                        }),
                        execution: summary.as_ref().map(format_run_metrics_line),
                        models: summary.as_ref().map(format_run_models_line),
                        usage: summary.as_ref().map(format_run_usage_line),
                        changes: summary.as_ref().map(format_run_diff_line),
                        started_at: summary
                            .as_ref()
                            .map(|summary| summary.started_at.to_rfc3339()),
                        finished_at: summary.as_ref().map(|summary| {
                            summary
                                .finished_at
                                .as_ref()
                                .map(|value| value.to_rfc3339())
                                .unwrap_or_else(|| "-".into())
                        }),
                        workspace: summary
                            .as_ref()
                            .map(|summary| summary.workspace.as_deref().unwrap_or("-").to_string()),
                        merge_target: summary.as_ref().map(|summary| {
                            summary
                                .diff
                                .as_ref()
                                .and_then(|diff| diff.merge_target.as_deref())
                                .unwrap_or("-")
                                .to_string()
                        }),
                    })
                }
                Err(error) => Some(AutoflowExplainRun {
                    run_id: run_id.to_string(),
                    status: format!("missing ({error})"),
                    backend: "-".into(),
                    worker: "-".into(),
                    source_wake: None,
                    approval_step: None,
                    approval_expires: None,
                    execution: None,
                    models: None,
                    usage: None,
                    changes: None,
                    started_at: None,
                    finished_at: None,
                    workspace: None,
                    merge_target: None,
                }),
            },
            None => None,
        };
        let priority = selected_priority(&claim).map(|value| value.to_string());
        let contenders = format_contenders(&claim.contenders);
        let next_action = next_action_summary(&claim);
        Some(AutoflowExplainClaim {
            issue_display: claim.issue_display_ref,
            issue_title: claim.issue_title,
            issue_tracker: claim.issue_tracker,
            issue_state: claim.issue_state_name,
            source: claim.source_ref,
            issue_url: claim.issue_url,
            workflow: claim.workflow,
            status: status_name(claim.status).to_string(),
            priority,
            contenders,
            workspace: claim.worktree_path.unwrap_or_else(|| "-".into()),
            branch: claim.branch.unwrap_or_else(|| "-".into()),
            pr: claim.pr_url.unwrap_or_else(|| "-".into()),
            claim_owner: claim.claim_owner.unwrap_or_else(|| "-".into()),
            lease_expires: claim.lease_expires_at.unwrap_or_else(|| "-".into()),
            active_lock,
            last_run,
            next_action,
            pending_dispatch: claim.pending_dispatch.map(|dispatch| {
                AutoflowExplainPendingDispatch {
                    workflow: dispatch.workflow,
                    target: dispatch.target,
                    inputs: format_inputs(&dispatch.inputs),
                }
            }),
            next_retry: claim.next_retry_at,
            summary: claim.last_summary,
        })
    } else {
        None
    };

    let report = AutoflowExplainReport {
        kind: "autoflow_explain",
        version: 1,
        item: AutoflowExplainItem {
            issue: issue_ref,
            repo: repo_ref,
            status: claim_report
                .as_ref()
                .map(|claim| claim.status.clone())
                .unwrap_or_else(|| "unclaimed".into()),
            source: claim_report
                .as_ref()
                .and_then(|claim| claim.source.clone())
                .or_else(|| {
                    source_matches
                        .first()
                        .and_then(|entry| entry.autoflow().ok())
                        .and_then(|autoflow| autoflow.source.clone())
                }),
            candidate_workflows,
            claim: claim_report,
            queued_wakes,
            recent_processed_wake,
            recent_cycle_events: recent_history,
        },
    };
    let rendered = render_autoflow_explain_report(&report);
    output_report::emit_detail(global_format, &AutoflowExplainOutput { report, rendered })
}

async fn doctor(
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let repo_filter = normalize_repo_filter(repo)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
    let run_store = RunStore::new(global.join("runs"));
    let mut findings = Vec::new();
    let claims = filter_claims_by_repo(claim_store.list()?, repo_filter.as_deref());
    let queued_wakes = filter_wakes_by_repo(wake_store.list_queued()?, repo_filter.as_deref());

    for claim in &claims {
        if repo_store.load(&claim.repo_ref)?.is_none() {
            findings.push(DoctorFinding::new(
                &claim.issue_ref,
                "missing_repo_binding",
                format!("repo `{}` is not tracked", claim.repo_ref),
            ));
        }
        if !matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released)
            && claim
                .worktree_path
                .as_deref()
                .is_some_and(|path| !Path::new(path).exists())
        {
            findings.push(DoctorFinding::new(
                &claim.issue_ref,
                "missing_worktree",
                format!(
                    "workspace path `{}` does not exist",
                    claim.worktree_path.as_deref().unwrap_or("-")
                ),
            ));
        }
        if let Some(manifest_path) = claim.artifact_manifest_path.as_deref() {
            if !Path::new(manifest_path).exists() {
                findings.push(DoctorFinding::new(
                    &claim.issue_ref,
                    "artifact_missing",
                    format!("artifact manifest `{manifest_path}` does not exist"),
                ));
            }
        }
        if let Some(run_id) = claim.last_run_id.as_deref() {
            match run_store.load(run_id) {
                Ok(run) => {
                    if run.issue_ref.as_deref() != Some(claim.issue_ref.as_str()) {
                        findings.push(DoctorFinding::new(
                            &claim.issue_ref,
                            "claim_run_mismatch",
                            format!(
                                "run `{run_id}` is bound to `{}`",
                                run.issue_ref.as_deref().unwrap_or("-")
                            ),
                        ));
                    }
                    if run.workflow_name != claim.workflow && claim.pending_dispatch.is_none() {
                        findings.push(DoctorFinding::new(
                            &claim.issue_ref,
                            "claim_run_mismatch",
                            format!(
                                "claim workflow `{}` disagrees with run workflow `{}`",
                                claim.workflow, run.workflow_name
                            ),
                        ));
                    }
                }
                Err(error) => findings.push(DoctorFinding::new(
                    &claim.issue_ref,
                    "missing_run",
                    format!("last run `{run_id}` could not be loaded: {error}"),
                )),
            }
        }
        let lock_path = claim_store
            .root
            .join(issue_key(&claim.issue_ref))
            .join(".lock");
        if lock_path.exists() {
            match claim_store.read_active_lock(&claim.issue_ref) {
                Ok(Some(lock)) if claim.status != ClaimStatus::Running => {
                    findings.push(DoctorFinding::new(
                        &claim.issue_ref,
                        "stale_lock",
                        format!(
                            "active lock owned by `{}` remains while claim status is `{}`",
                            lock.owner,
                            status_name(claim.status)
                        ),
                    ))
                }
                Err(error) => findings.push(DoctorFinding::new(
                    &claim.issue_ref,
                    "stale_lock",
                    format!("active lock is unreadable: {error}"),
                )),
                _ => {}
            }
        }
    }

    for entry in std::fs::read_dir(&claim_store.root)
        .into_iter()
        .flatten()
        .flatten()
    {
        let issue_dir = entry.path();
        let lock_path = issue_dir.join(".lock");
        let claim_path = issue_dir.join("claim.toml");
        if lock_path.is_file() && !claim_path.is_file() {
            findings.push(DoctorFinding::new(
                issue_dir
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("unknown"),
                "stale_lock",
                format!(
                    "orphan active lock `{}` has no claim.toml",
                    lock_path.display()
                ),
            ));
        }
    }

    for wake in &queued_wakes {
        if let Some(problem) = invalid_wake_payload_detail(&wake_store, wake) {
            findings.push(DoctorFinding::new(
                &wake.entity.ref_text,
                "invalid_wake_payload",
                format!("wake `{}` {problem}", wake.wake_id),
            ));
        }
    }

    let rows = findings
        .into_iter()
        .map(|finding| AutoflowDoctorRow {
            scope: finding.scope,
            problem: finding.kind.to_string(),
            detail: finding.detail,
        })
        .collect::<Vec<_>>();

    let output = AutoflowDoctorOutput {
        report: AutoflowDoctorReport {
            kind: "autoflow_doctor",
            version: 1,
            ok: rows.is_empty(),
            rows,
        },
    };
    output_report::emit_collection(global_format, &output)
}

async fn repair(r#ref: &str, release: bool, requeue_requested: bool) -> anyhow::Result<()> {
    if release && requeue_requested {
        bail!("repair accepts at most one of `--release` or `--requeue`");
    }

    let issue_ref = canonical_autoflow_issue_ref(r#ref)?;
    let global = paths::global_dir()?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
    let Some(mut claim) = claim_store.load(&issue_ref)? else {
        bail!("no autoflow claim for `{issue_ref}`");
    };

    let mut repairs = Vec::new();
    let queued = wakes_for_issue(
        filter_wakes_by_repo(wake_store.list_queued()?, Some(&claim.repo_ref)),
        &issue_ref,
    );
    for wake in queued {
        if invalid_wake_payload_detail(&wake_store, &wake).is_some() {
            wake_store.delete(&wake.wake_id)?;
            repairs.push(format!("deleted invalid queued wake {}", wake.wake_id));
        }
    }

    if !release
        && claim
            .worktree_path
            .as_deref()
            .zip(claim.branch.as_deref())
            .is_some_and(|(path, _)| !Path::new(path).exists())
    {
        let tracked = repo_store
            .load(&claim.repo_ref)?
            .ok_or_else(|| anyhow!("repo `{}` is not tracked", claim.repo_ref))?;
        let branch = claim
            .branch
            .clone()
            .unwrap_or_else(|| format!("rupu/{}", issue_dir_name(&claim.issue_ref)));
        let worktree = ensure_issue_worktree(
            Path::new(&tracked.preferred_path),
            &paths::autoflow_worktrees_dir(&global),
            &claim.repo_ref,
            &claim.issue_ref,
            &branch,
            Some("HEAD"),
        )?;
        claim.worktree_path = Some(worktree.path.display().to_string());
        claim.branch = Some(branch);
        claim.updated_at = chrono::Utc::now().to_rfc3339();
        claim_store.save(&claim)?;
        repairs.push(format!(
            "rebuilt worktree {}",
            claim.worktree_path.as_deref().unwrap_or("-")
        ));
    }

    if release {
        cleanup_claim_artifacts(&repo_store, &claim)?;
        claim_store.delete(&issue_ref)?;
        repairs.push(format!("released claim {issue_ref}"));
    } else if requeue_requested {
        let wake = enqueue_issue_wake(
            &wake_store,
            WakeSource::Repair,
            &claim.repo_ref,
            &issue_ref,
            "autoflow.repair.requeue",
            chrono::Utc::now(),
        )?;
        repairs.push(format!("queued repair wake {}", wake.wake_id));
    }

    if repairs.is_empty() {
        println!("no repairs applied for {issue_ref}");
    } else {
        println!("repaired {issue_ref}:");
        for repair in repairs {
            println!("- {repair}");
        }
    }
    Ok(())
}

async fn requeue(r#ref: &str, event: Option<&str>, not_before: Option<&str>) -> anyhow::Result<()> {
    let issue_ref = canonical_autoflow_issue_ref(r#ref)?;
    let global = paths::global_dir()?;
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let repo_ref = claim_store
        .load(&issue_ref)?
        .map(|claim| claim.repo_ref)
        .unwrap_or_else(|| issue_ref_to_repo_ref(&issue_ref).unwrap_or_else(|_| "-".into()));
    if repo_ref == "-" {
        bail!("could not resolve repo for `{issue_ref}`");
    }
    let when = match not_before {
        Some(value) => chrono::Utc::now() + parse_duration(value)?,
        None => chrono::Utc::now(),
    };
    let wake = enqueue_issue_wake(
        &WakeStore::new(paths::autoflow_wakes_dir(&global)),
        WakeSource::Manual,
        &repo_ref,
        &issue_ref,
        event.unwrap_or("autoflow.manual.requeue"),
        when,
    )?;
    println!(
        "queued {} for {} not_before={}",
        wake.wake_id, issue_ref, wake.not_before
    );
    Ok(())
}

async fn status(
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
    let repo_filter = normalize_repo_filter(repo)?;
    let claims = filter_claims_by_repo(store.list()?, repo_filter.as_deref());
    let recent = recent_activity_by_issue(&history_store, repo_filter.as_deref())?;
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for claim in &claims {
        *counts.entry(status_name(claim.status)).or_insert(0) += 1;
    }
    if counts.is_empty() {
        println!("(no autoflow claims)");
        return Ok(());
    }
    let rows = counts
        .into_iter()
        .map(|(status, count)| AutoflowStatusRow {
            status: status.into(),
            count,
        })
        .collect::<Vec<_>>();
    let contested = claims
        .iter()
        .filter(|claim| claim.contenders.len() > 1)
        .map(|claim| AutoflowContestedRow {
            issue: claim.issue_ref.clone(),
            issue_display: claim.issue_display_ref.clone(),
            tracker: claim.issue_tracker.clone(),
            state: claim.issue_state_name.clone(),
            source: claim.source_ref.clone(),
            repo: claim.repo_ref.clone(),
            last_cycle: recent
                .get(&claim.issue_ref)
                .map(|activity| activity.at.clone()),
            last_event: recent
                .get(&claim.issue_ref)
                .map(|activity| activity.event.clone()),
            contenders: format_contenders(&claim.contenders),
        })
        .collect::<Vec<_>>();
    let prefs = autoflow_ui_prefs()?;
    let output = AutoflowStatusOutput {
        prefs,
        report: AutoflowStatusReport {
            kind: "autoflow_status",
            version: 2,
            rows,
            contested,
        },
    };
    output_report::emit_collection(global_format, &output)
}

async fn claims(
    repo: Option<&str>,
    global_format: Option<crate::output::formats::OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
    let repo_filter = normalize_repo_filter(repo)?;
    let claims = filter_claims_by_repo(store.list()?, repo_filter.as_deref());
    let recent = recent_activity_by_issue(&history_store, repo_filter.as_deref())?;
    if claims.is_empty()
        && matches!(
            global_format.unwrap_or(crate::output::formats::OutputFormat::Table),
            crate::output::formats::OutputFormat::Table
        )
    {
        println!("(no autoflow claims)");
        return Ok(());
    }
    let rows = claims
        .iter()
        .map(|claim| AutoflowClaimRow {
            issue: claim.issue_ref.clone(),
            issue_display: claim.issue_display_ref.clone(),
            tracker: claim.issue_tracker.clone(),
            state: claim.issue_state_name.clone(),
            source: claim.source_ref.clone(),
            repo: claim.repo_ref.clone(),
            workflow: claim.workflow.clone(),
            priority: selected_priority(claim)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            status: status_name(claim.status).into(),
            next: next_action_summary(claim),
            branch: claim.branch.clone().unwrap_or_else(|| "-".into()),
            pr: claim.pr_url.clone().unwrap_or_else(|| "-".into()),
            summary: claim_summary(claim),
            contenders: format_contenders(&claim.contenders),
            workspace: claim.worktree_path.clone().unwrap_or_else(|| "-".into()),
            last_cycle: recent
                .get(&claim.issue_ref)
                .map(|activity| activity.at.clone())
                .unwrap_or_else(|| "-".into()),
            last_event: recent
                .get(&claim.issue_ref)
                .map(|activity| activity.event.clone())
                .unwrap_or_else(|| "-".into()),
            last_run: recent
                .get(&claim.issue_ref)
                .and_then(|activity| activity.run_id.clone())
                .or_else(|| claim.last_run_id.clone())
                .unwrap_or_else(|| "-".into()),
        })
        .collect::<Vec<_>>();
    let prefs = autoflow_ui_prefs()?;
    let output = AutoflowClaimsOutput {
        prefs,
        csv_rows: rows
            .iter()
            .map(|claim| AutoflowClaimCsvRow {
                issue: claim.issue.clone(),
                issue_display: claim.issue_display.clone().unwrap_or_default(),
                tracker: claim.tracker.clone().unwrap_or_default(),
                state: claim.state.clone().unwrap_or_default(),
                source: claim.source.clone().unwrap_or_default(),
                repo: claim.repo.clone(),
                workflow: claim.workflow.clone(),
                priority: claim.priority.clone(),
                status: claim.status.clone(),
                next: claim.next.clone(),
                branch: claim.branch.clone(),
                pr: claim.pr.clone(),
                summary: claim.summary.clone(),
                contenders: claim.contenders.clone(),
                workspace: claim.workspace.clone(),
                last_cycle: claim.last_cycle.clone(),
                last_event: claim.last_event.clone(),
                last_run: claim.last_run.clone(),
            })
            .collect(),
        report: AutoflowClaimsReport {
            kind: "autoflow_claims",
            version: 2,
            rows,
        },
    };
    output_report::emit_collection(global_format, &output)
}

async fn release(r#ref: &str) -> anyhow::Result<()> {
    let issue_ref = canonical_autoflow_issue_ref(r#ref)?;
    let global = paths::global_dir()?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let claim_store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(&global),
    };
    if let Some(claim) = claim_store.load(&issue_ref)? {
        cleanup_claim_artifacts(&repo_store, &claim)?;
        claim_store.delete(&issue_ref)?;
        println!("released {issue_ref}");
    } else {
        println!("{issue_ref} was not claimed");
    }
    Ok(())
}

fn visible_autoflow_matches(
    name: &str,
    repo: Option<&str>,
) -> anyhow::Result<Vec<VisibleAutoflowWorkflow>> {
    let repo_filter = normalize_repo_filter(repo)?;
    let mut entries = visible_autoflows()?
        .into_iter()
        .filter(|entry| entry.name == name)
        .collect::<Vec<_>>();
    entries = filter_visible_autoflows(entries, repo_filter.as_deref());
    entries.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.workflow_path.cmp(&right.workflow_path))
    });
    Ok(entries)
}

fn visible_autoflow_matches_for_issue(
    issue_ref: &IssueRef,
    repo: Option<&str>,
) -> anyhow::Result<Vec<VisibleAutoflowWorkflow>> {
    let repo_filter = normalize_repo_filter(repo)?;
    let mut entries = visible_autoflows()?
        .into_iter()
        .filter(|entry| issue_matches_visible_source(issue_ref, entry))
        .collect::<Vec<_>>();
    entries = filter_visible_autoflows(entries, repo_filter.as_deref());
    entries.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.workflow_path.cmp(&right.workflow_path))
    });
    Ok(entries)
}

fn issue_matches_visible_source(issue_ref: &IssueRef, entry: &VisibleAutoflowWorkflow) -> bool {
    let Ok(autoflow) = entry.autoflow() else {
        return false;
    };
    if let Some(source) = autoflow.source.as_deref() {
        return source_matches_issue_ref(source, issue_ref);
    }
    entry
        .repo_ref
        .as_deref()
        .is_some_and(|repo_ref| repo_ref_matches_issue_ref(repo_ref, issue_ref))
}

fn source_matches_issue_ref(source: &str, issue_ref: &IssueRef) -> bool {
    let Ok(source_ref) = source.parse::<EventSourceRef>() else {
        return false;
    };
    match source_ref {
        EventSourceRef::Repo { repo } => match issue_ref.tracker {
            IssueTracker::Github => {
                repo.platform == Platform::Github
                    && issue_ref.project == format!("{}/{}", repo.owner, repo.repo)
            }
            IssueTracker::Gitlab => {
                repo.platform == Platform::Gitlab
                    && issue_ref.project == format!("{}/{}", repo.owner, repo.repo)
            }
            IssueTracker::Linear | IssueTracker::Jira => false,
        },
        EventSourceRef::TrackerProject { tracker, project } => {
            tracker == issue_ref.tracker
                && match tracker {
                    IssueTracker::Jira => {
                        project == issue_ref.project
                            || (!project.contains('/')
                                && issue_ref
                                    .project
                                    .rsplit('/')
                                    .next()
                                    .is_some_and(|suffix| suffix == project))
                    }
                    IssueTracker::Github | IssueTracker::Gitlab | IssueTracker::Linear => {
                        project == issue_ref.project
                    }
                }
        }
    }
}

fn repo_ref_matches_issue_ref(repo_ref: &str, issue_ref: &IssueRef) -> bool {
    issue_ref_to_repo_ref(&format_issue_ref(issue_ref))
        .map(|expected| expected == repo_ref)
        .unwrap_or(false)
}

fn resolve_repo_binding_for_issue(
    issue_ref: &IssueRef,
    repo: Option<&str>,
) -> anyhow::Result<String> {
    let issue_ref_text = format_issue_ref(issue_ref);
    if let Some(repo) = normalize_repo_filter(repo)? {
        return Ok(repo);
    }
    if let Ok(repo_ref) = issue_ref_to_repo_ref(&issue_ref_text) {
        return Ok(repo_ref);
    }
    let matches = visible_autoflow_matches_for_issue(issue_ref, None)?;
    let repos = matches
        .iter()
        .filter_map(|entry| entry.repo_ref.clone())
        .collect::<BTreeSet<_>>();
    match repos.len() {
        0 => bail!("no visible autoflow is bound to tracker issue `{issue_ref_text}`"),
        1 => Ok(repos.into_iter().next().expect("one repo binding")),
        _ => bail!(
            "multiple repo bindings are visible for tracker issue `{issue_ref_text}`: {}\npass `--repo <platform>:<owner>/<repo>` to disambiguate",
            repos.into_iter().collect::<Vec<_>>().join(", ")
        ),
    }
}

fn issue_ref_for_autoflow(
    issue_ref: &IssueRef,
    resolved: &ResolvedAutoflowWorkflow,
) -> anyhow::Result<IssueRef> {
    let autoflow = resolved.autoflow()?;
    let Some(source) = autoflow.source.as_deref() else {
        return Ok(issue_ref.clone());
    };
    if issue_ref.tracker != IssueTracker::Jira || !source_matches_issue_ref(source, issue_ref) {
        return Ok(issue_ref.clone());
    }
    let Ok(EventSourceRef::TrackerProject { tracker, project }) = source.parse::<EventSourceRef>()
    else {
        return Ok(issue_ref.clone());
    };
    if tracker != IssueTracker::Jira {
        return Ok(issue_ref.clone());
    }
    let mut adjusted = issue_ref.clone();
    adjusted.project = project;
    Ok(adjusted)
}

fn normalize_repo_filter(repo: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(repo) = repo else {
        return Ok(None);
    };
    let parsed = crate::run_target::parse_run_target(repo)
        .map_err(|err| anyhow!("invalid repo filter `{repo}`: {err}"))?;
    match parsed {
        crate::run_target::RunTarget::Repo {
            platform,
            owner,
            repo,
            ..
        } => Ok(Some(format!("{platform}:{owner}/{repo}"))),
        _ => bail!("invalid repo filter `{repo}`: expected `<platform>:<owner>/<repo>`"),
    }
}

fn filter_visible_autoflows(
    entries: Vec<VisibleAutoflowWorkflow>,
    repo_filter: Option<&str>,
) -> Vec<VisibleAutoflowWorkflow> {
    match repo_filter {
        Some(repo_ref) => entries
            .into_iter()
            .filter(|entry| entry.repo_ref.as_deref() == Some(repo_ref))
            .collect(),
        None => entries,
    }
}

fn filter_claims_by_repo(
    claims: Vec<AutoflowClaimRecord>,
    repo_filter: Option<&str>,
) -> Vec<AutoflowClaimRecord> {
    match repo_filter {
        Some(repo_ref) => claims
            .into_iter()
            .filter(|claim| claim.repo_ref == repo_ref)
            .collect(),
        None => claims,
    }
}

fn filter_wakes_by_repo(wakes: Vec<WakeRecord>, repo_filter: Option<&str>) -> Vec<WakeRecord> {
    match repo_filter {
        Some(repo_ref) => wakes
            .into_iter()
            .filter(|wake| wake.repo_ref == repo_ref)
            .collect(),
        None => wakes,
    }
}

fn wakes_for_issue(wakes: Vec<WakeRecord>, issue_ref: &str) -> Vec<WakeRecord> {
    wakes
        .into_iter()
        .filter(|wake| {
            wake.entity.kind == WakeEntityKind::Issue && wake.entity.ref_text == issue_ref
        })
        .collect()
}

fn wake_source_name(source: WakeSource) -> &'static str {
    match source {
        WakeSource::Manual => "manual",
        WakeSource::CronPoll => "cron_poll",
        WakeSource::Webhook => "webhook",
        WakeSource::AutoflowDispatch => "dispatch",
        WakeSource::Retry => "retry",
        WakeSource::ApprovalResume => "approval_resume",
        WakeSource::Repair => "repair",
    }
}

fn canonical_autoflow_issue_ref(value: &str) -> anyhow::Result<String> {
    if let Ok(issue) = parse_issue_ref_text(value) {
        return Ok(format!(
            "{}:{}/issues/{}",
            issue.tracker.as_str(),
            issue.project,
            issue.number
        ));
    }
    canonical_repo_issue_ref(value)
}

fn issue_ref_to_repo_ref(issue_ref: &str) -> anyhow::Result<String> {
    let issue = parse_issue_ref_text(issue_ref)?;
    match issue.tracker {
        IssueTracker::Github | IssueTracker::Gitlab => {
            Ok(format!("{}:{}", issue.tracker, issue.project))
        }
        IssueTracker::Linear | IssueTracker::Jira => Err(anyhow!(
            "tracker issue `{issue_ref}` does not imply a repo binding"
        )),
    }
}

fn format_inputs(inputs: &BTreeMap<String, String>) -> String {
    if inputs.is_empty() {
        return "-".into();
    }
    inputs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn describe_wake_source(global: &Path, wake_store: &WakeStore, wake_id: &str) -> String {
    match wake_store.load(wake_id) {
        Ok(wake) => {
            let state = if paths::autoflow_wake_queue_dir(global)
                .join(format!("{wake_id}.json"))
                .is_file()
            {
                "queued"
            } else if paths::autoflow_wake_processed_dir(global)
                .join(format!("{wake_id}.json"))
                .is_file()
            {
                "processed"
            } else {
                "unknown"
            };
            format!(
                "{wake_id} [{state}] {} {}",
                wake_source_name(wake.source),
                wake.event.id
            )
        }
        Err(error) => format!("{wake_id} (missing: {error})"),
    }
}

fn invalid_wake_payload_detail(wake_store: &WakeStore, wake: &WakeRecord) -> Option<String> {
    let payload_ref = wake.payload_ref.as_ref()?;
    if !payload_ref.is_file() {
        return Some(format!(
            "references missing payload `{}`",
            payload_ref.display()
        ));
    }
    match wake_store.read_payload(&wake.wake_id) {
        Ok(_) => None,
        Err(error) => Some(format!("has unreadable payload: {error}")),
    }
}

fn enqueue_issue_wake(
    wake_store: &WakeStore,
    source: WakeSource,
    repo_ref: &str,
    issue_ref: &str,
    event_id: &str,
    not_before: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<WakeRecord> {
    Ok(wake_store.enqueue(WakeEnqueueRequest {
        source,
        repo_ref: repo_ref.to_string(),
        entity: WakeEntity {
            kind: WakeEntityKind::Issue,
            ref_text: issue_ref.to_string(),
        },
        event: WakeEvent {
            id: event_id.to_string(),
            delivery_id: None,
            dedupe_key: None,
        },
        payload: None,
        received_at: chrono::Utc::now().to_rfc3339(),
        not_before: not_before.to_rfc3339(),
    })?)
}

#[derive(Debug)]
struct DoctorFinding {
    scope: String,
    kind: &'static str,
    detail: String,
}

impl DoctorFinding {
    fn new(scope: impl Into<String>, kind: &'static str, detail: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            kind,
            detail: detail.into(),
        }
    }
}

fn visible_autoflows() -> anyhow::Result<Vec<VisibleAutoflowWorkflow>> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let repo_store = RepoRegistryStore {
        root: paths::repos_dir(&global),
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    if let Some(project_root) = &project_root {
        let cfg = resolve_config(&global, Some(project_root))?;
        let repo_ref = cfg.autoflow.repo.clone().or_else(|| {
            autodetect_repo_from_path(project_root)
                .ok()
                .map(|repo| canonical_repo_ref(&repo))
        });
        push_visible_workflow_paths(
            &project_root.join(".rupu/workflows"),
            "project",
            Some(project_root),
            repo_ref.clone(),
            Some(project_root.clone()),
            &mut seen,
            &mut out,
        )?;
        push_visible_workflow_paths(
            &global.join("workflows"),
            "global",
            Some(project_root),
            repo_ref,
            Some(project_root.clone()),
            &mut seen,
            &mut out,
        )?;
    } else {
        let global_cfg = resolve_config(&global, None)?;
        push_visible_workflow_paths(
            &global.join("workflows"),
            "global",
            None,
            global_cfg.autoflow.repo.clone(),
            None,
            &mut seen,
            &mut out,
        )?;
    }

    for resolved in discover_tick_autoflows(&global, &repo_store)? {
        let key = visible_autoflow_key(
            Some(&resolved.repo_ref),
            &resolved.scope,
            &resolved.workflow_path,
        );
        if !seen.insert(key) {
            continue;
        }
        out.push(VisibleAutoflowWorkflow {
            scope: resolved.scope,
            name: resolved.name,
            workflow_path: resolved.workflow_path,
            workflow: resolved.workflow,
            project_root: resolved.project_root,
            repo_ref: Some(resolved.repo_ref),
            preferred_checkout: Some(resolved.preferred_checkout),
        });
    }

    out.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
            .then_with(|| left.workflow_path.cmp(&right.workflow_path))
    });
    Ok(out)
}

fn push_visible_workflow_paths(
    dir: &Path,
    scope: &str,
    preferred_checkout: Option<&Path>,
    repo_ref: Option<String>,
    project_root: Option<PathBuf>,
    seen: &mut BTreeSet<String>,
    into: &mut Vec<VisibleAutoflowWorkflow>,
) -> anyhow::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let workflow = Workflow::parse(&body)?;
        if !workflow
            .autoflow
            .as_ref()
            .map(|a| a.enabled)
            .unwrap_or(false)
        {
            continue;
        }
        let key = visible_autoflow_key(repo_ref.as_deref(), scope, &path);
        if !seen.insert(key) {
            continue;
        }
        into.push(VisibleAutoflowWorkflow {
            scope: scope.to_string(),
            name: workflow.name.clone(),
            workflow,
            workflow_path: path,
            project_root: project_root.clone(),
            repo_ref: repo_ref.clone(),
            preferred_checkout: preferred_checkout.map(Path::to_path_buf),
        });
    }
    Ok(())
}

fn visible_autoflow_key(repo_ref: Option<&str>, scope: &str, path: &Path) -> String {
    format!("{}|{}|{}", repo_ref.unwrap_or("-"), scope, path.display())
}

pub(crate) fn discover_tick_autoflows(
    global: &Path,
    repo_store: &RepoRegistryStore,
) -> anyhow::Result<Vec<ResolvedAutoflowWorkflow>> {
    let mut out = Vec::new();
    let global_cfg = resolve_config(global, None)?;
    if global_cfg.autoflow.enabled.unwrap_or(true) {
        if let Some(repo_ref) = global_cfg.autoflow.repo.clone() {
            if let Some(tracked) = repo_store.load(&repo_ref)? {
                let preferred_checkout = PathBuf::from(&tracked.preferred_path);
                let project_root = paths::project_root_for(&preferred_checkout)?
                    .or_else(|| Some(preferred_checkout.clone()));
                push_resolved_autoflow_paths(
                    &global.join("workflows"),
                    "global",
                    global,
                    project_root,
                    repo_ref,
                    preferred_checkout,
                    global_cfg.clone(),
                    &mut out,
                )?;
            } else {
                warn!(
                    repo_ref,
                    "skipping global autoflows because repo is not tracked"
                );
            }
        }
    }

    for tracked in repo_store.list()? {
        let preferred_checkout = PathBuf::from(&tracked.preferred_path);
        if !preferred_checkout.exists() {
            warn!(path = %preferred_checkout.display(), repo_ref = %tracked.repo_ref, "skipping tracked repo because preferred checkout is missing");
            continue;
        }
        let project_root = paths::project_root_for(&preferred_checkout)?.or_else(|| {
            preferred_checkout
                .join(".rupu")
                .is_dir()
                .then_some(preferred_checkout.clone())
        });
        let Some(project_root) = project_root else {
            continue;
        };
        let cfg = resolve_config(global, Some(&project_root))?;
        if cfg.autoflow.enabled == Some(false) {
            continue;
        }
        push_resolved_autoflow_paths(
            &project_root.join(".rupu/workflows"),
            "project",
            global,
            Some(project_root),
            tracked.repo_ref.clone(),
            preferred_checkout,
            cfg,
            &mut out,
        )?;
    }

    out.sort_by(|left, right| {
        left.repo_ref
            .cmp(&right.repo_ref)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.scope.cmp(&right.scope))
    });
    Ok(out)
}

pub(crate) async fn collect_wake_hints(
    global: &Path,
    discovered: &[ResolvedAutoflowWorkflow],
    resolver: &dyn CredentialResolver,
) -> anyhow::Result<WakeHints> {
    let store = WakeStore::new(paths::autoflow_wakes_dir(global));
    enqueue_polled_wakes(global, discovered, resolver).await?;
    let mut wake_hints = WakeHints::default();
    let repo_refs = wake_enabled_repo_refs(discovered)?;
    if repo_refs.is_empty() {
        return Ok(wake_hints);
    }
    for wake in store.list_due(chrono::Utc::now())? {
        if !repo_refs.contains(&wake.repo_ref) {
            continue;
        }
        match wake.source {
            rupu_runtime::WakeSource::CronPoll => wake_hints.total_polled_events += 1,
            rupu_runtime::WakeSource::Webhook => wake_hints.total_webhook_events += 1,
            _ => {}
        }
        wake_hints
            .by_repo
            .entry(wake.repo_ref.clone())
            .or_default()
            .insert(wake.event.id.clone());
        if wake.entity.kind == WakeEntityKind::Issue {
            wake_hints
                .by_issue
                .entry(wake.entity.ref_text.clone())
                .or_default()
                .insert(wake.event.id.clone());
        }
        wake_hints.due_wake_ids.push(wake.wake_id);
    }
    Ok(wake_hints)
}

async fn enqueue_polled_wakes(
    global: &Path,
    discovered: &[ResolvedAutoflowWorkflow],
    resolver: &dyn CredentialResolver,
) -> anyhow::Result<()> {
    let cursors_root = paths::autoflow_event_cursors_dir(global);
    paths::ensure_dir(&cursors_root)?;
    let store = WakeStore::new(paths::autoflow_wakes_dir(global));

    let mut workflows_by_source: BTreeMap<String, Vec<&ResolvedAutoflowWorkflow>> = BTreeMap::new();
    for resolved in discovered {
        if resolved.autoflow()?.wake_on.is_empty() {
            continue;
        }
        let Ok(source_ref) = resolved_source_ref_text(resolved) else {
            warn!(workflow = %resolved.name, repo_ref = %resolved.repo_ref, "invalid autoflow source; skipping wake polling");
            continue;
        };
        workflows_by_source
            .entry(source_ref)
            .or_default()
            .push(resolved);
    }

    for (source_ref, workflows) in workflows_by_source {
        let Some((source, resolved)) = workflows.iter().find_map(|resolved| {
            resolved
                .cfg
                .triggers
                .poll_source(&source_ref)
                .map(|source| (source, *resolved))
        }) else {
            continue;
        };
        let Ok(event_source) = source_ref.parse::<EventSourceRef>() else {
            warn!(source_ref, "invalid autoflow source for wake polling");
            continue;
        };
        let last_polled_file = autoflow_last_polled_at_path(&cursors_root, &event_source);
        match autoflow_poll_source_due(source, &last_polled_file, chrono::Utc::now()) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) => {
                warn!(source_ref, error = %err, "invalid autoflow poll interval; polling anyway");
            }
        }
        let registry = Arc::new(rupu_scm::Registry::discover(resolver, &resolved.cfg).await);
        let Some(connector) = registry.events_for_source(&event_source) else {
            warn!(
                source_ref,
                "no event connector configured for autoflow wake polling"
            );
            continue;
        };
        let cursor_file = autoflow_cursor_path(&cursors_root, &event_source);
        let cursor = read_cursor(&cursor_file).ok();
        let max = resolved.cfg.triggers.effective_max_events_per_tick();
        let polled = match connector
            .poll_events(&event_source, cursor.as_deref(), max)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                warn!(source_ref, error = %err, "failed to poll autoflow wake events");
                continue;
            }
        };
        if let Err(err) = write_cursor(&cursor_file, &polled.next_cursor) {
            warn!(
                source_ref,
                error = %err,
                "failed to persist autoflow wake cursor; events may be replayed on next tick"
            );
        }
        if let Err(err) = write_last_polled_at(&last_polled_file, chrono::Utc::now()) {
            warn!(
                source_ref,
                error = %err,
                "failed to persist autoflow last-polled timestamp; source may poll early next tick"
            );
        }
        let bound_repo_refs = workflows
            .iter()
            .map(|resolved| resolved.repo_ref.clone())
            .collect::<BTreeSet<_>>();
        for event in polled.events {
            for repo_ref in &bound_repo_refs {
                for request in wake_requests_from_polled_event_for_repo(&event, repo_ref) {
                    match store.enqueue(request) {
                        Ok(_) => {}
                        Err(WakeStoreError::DuplicateDedupeKey(_)) => {}
                        Err(err) => {
                            warn!(repo_ref, source_ref, error = %err, "failed to enqueue polled autoflow wake");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn wake_enabled_repo_refs(
    discovered: &[ResolvedAutoflowWorkflow],
) -> anyhow::Result<BTreeSet<String>> {
    let mut repo_refs = BTreeSet::new();
    for resolved in discovered {
        if resolved.autoflow()?.wake_on.is_empty() {
            continue;
        }
        repo_refs.insert(resolved.repo_ref.clone());
    }
    Ok(repo_refs)
}

pub(crate) async fn collect_issue_matches(
    discovered: &[ResolvedAutoflowWorkflow],
    resolver: &dyn CredentialResolver,
) -> anyhow::Result<Vec<IssueMatch>> {
    let mut out = Vec::new();
    for resolved in discovered {
        let autoflow = resolved.autoflow()?;
        let source_ref = match resolved_event_source(resolved) {
            Ok(source_ref) => source_ref,
            Err(err) => {
                warn!(workflow = %resolved.name, repo_ref = %resolved.repo_ref, error = %err, "skipping autoflow because source resolution failed");
                continue;
            }
        };
        let (tracker, project) = issue_discovery_target(&source_ref);
        let registry = Arc::new(rupu_scm::Registry::discover(resolver, &resolved.cfg).await);
        let Some(connector) = registry.issues(tracker) else {
            warn!(source = %source_ref, repo_ref = %resolved.repo_ref, workflow = %resolved.name, "skipping autoflow because no issue connector is configured");
            continue;
        };

        let filter = build_issue_filter(autoflow);
        let mut issues = match connector.list_issues(&project, filter).await {
            Ok(issues) => issues,
            Err(err) => {
                warn!(source = %source_ref, repo_ref = %resolved.repo_ref, workflow = %resolved.name, error = %err, "skipping autoflow because issue listing failed");
                continue;
            }
        };
        issues.retain(|issue| selector_matches(autoflow, issue));
        issues.sort_by_key(|issue| issue.r.number);
        if let Some(limit) = autoflow.selector.limit {
            issues.truncate(limit as usize);
        }
        for issue in issues {
            out.push(IssueMatch {
                issue_ref_text: format_issue_ref(&issue.r),
                resolved: resolved.clone(),
                issue,
            });
        }
    }
    Ok(out)
}

pub(crate) fn choose_winning_matches(matches: Vec<IssueMatch>) -> BTreeMap<String, IssueMatch> {
    let mut grouped: BTreeMap<String, Vec<IssueMatch>> = BTreeMap::new();
    for item in matches {
        grouped
            .entry(item.issue_ref_text.clone())
            .or_default()
            .push(item);
    }

    let mut winners = BTreeMap::new();
    for (issue_ref_text, mut items) in grouped {
        items.sort_by(|left, right| {
            right
                .resolved
                .autoflow()
                .expect("autoflow")
                .priority
                .cmp(&left.resolved.autoflow().expect("autoflow").priority)
                .then_with(|| {
                    left.resolved
                        .workflow
                        .name
                        .cmp(&right.resolved.workflow.name)
                })
        });
        if let Some(winner) = items.into_iter().next() {
            winners.insert(issue_ref_text, winner);
        }
    }
    winners
}

pub(crate) fn summarize_issue_contenders(
    matches: &[IssueMatch],
) -> BTreeMap<String, Vec<AutoflowContender>> {
    let mut grouped: BTreeMap<String, Vec<AutoflowContender>> = BTreeMap::new();
    for item in matches {
        grouped
            .entry(item.issue_ref_text.clone())
            .or_default()
            .push(AutoflowContender {
                workflow: item.resolved.workflow.name.clone(),
                priority: item.resolved.autoflow().expect("autoflow").priority,
                scope: Some(item.resolved.scope.clone()),
                selected: false,
            });
    }
    for contenders in grouped.values_mut() {
        contenders.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.workflow.cmp(&right.workflow))
        });
        let mut deduped = Vec::with_capacity(contenders.len());
        for contender in contenders.drain(..) {
            if deduped
                .iter()
                .any(|existing: &AutoflowContender| existing.workflow == contender.workflow)
            {
                continue;
            }
            deduped.push(contender);
        }
        if let Some(first) = deduped.first_mut() {
            first.selected = true;
        }
        *contenders = deduped;
    }
    grouped
}

pub(crate) fn claim_should_yield_to_winner(
    claim: &AutoflowClaimRecord,
    winner: Option<&IssueMatch>,
    active_lock_held: bool,
) -> bool {
    if active_lock_held {
        return false;
    }
    let Some(winner) = winner else {
        return false;
    };
    if winner.resolved.workflow.name == claim.workflow {
        return false;
    }
    !matches!(
        claim.status,
        ClaimStatus::AwaitHuman
            | ClaimStatus::Blocked
            | ClaimStatus::Complete
            | ClaimStatus::Released
    )
}

pub(crate) fn active_or_fallback_contenders(
    contenders: &[AutoflowContender],
    resolved: Option<&ResolvedAutoflowWorkflow>,
    selected_workflow: &str,
) -> Vec<AutoflowContender> {
    if !contenders.is_empty() {
        let mut cloned = contenders.to_vec();
        let mut matched_selected = false;
        for contender in &mut cloned {
            contender.selected = contender.workflow == selected_workflow;
            matched_selected |= contender.selected;
        }
        if !matched_selected {
            if let Some(first) = cloned.first_mut() {
                first.selected = true;
            }
        }
        return cloned;
    }
    vec![AutoflowContender {
        workflow: selected_workflow.to_string(),
        priority: resolved
            .and_then(|resolved| resolved.autoflow().ok().map(|autoflow| autoflow.priority))
            .unwrap_or_default(),
        scope: resolved.map(|resolved| resolved.scope.clone()),
        selected: true,
    }]
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_autoflow_cycle(
    global: &Path,
    claim_store: &AutoflowClaimStore,
    resolved: &ResolvedAutoflowWorkflow,
    issue: &Issue,
    issue_ref_text: &str,
    mode_override: Option<&str>,
    attach_ui: bool,
    inputs: BTreeMap<String, String>,
    contenders: Vec<AutoflowContender>,
    worker: Option<ExecutionWorkerContext>,
    shared_printer: Option<Arc<Mutex<LineStreamPrinter>>>,
    live_cycle_recorder: Option<Arc<crate::cmd::autoflow_runtime::LiveCycleRecorder>>,
) -> anyhow::Result<()> {
    let autoflow = resolved.autoflow()?;
    let issue_payload = issue_payload(&resolved.cfg, issue)?;
    let workspace_strategy = resolve_workspace_strategy(&resolved.cfg.autoflow, autoflow);
    let branch = resolve_branch_name(
        autoflow
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.branch.as_deref()),
        &issue_payload,
        issue_ref_text,
        &inputs,
    )?;
    let workspace_path = match workspace_strategy {
        AutoflowWorkspaceStrategy::Worktree => {
            let root = resolve_worktree_root(global, &resolved.cfg.autoflow)?;
            ensure_issue_worktree(
                &resolved.preferred_checkout,
                &root,
                &resolved.repo_ref,
                issue_ref_text,
                &branch,
                Some("HEAD"),
            )?
            .path
        }
        AutoflowWorkspaceStrategy::InPlace => resolved.preferred_checkout.clone(),
    };
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &workspace_path)?;

    let owner = format!("{}:pid-{}", whoami::username(), std::process::id());
    let lease_expires_at = match autoflow
        .claim
        .as_ref()
        .and_then(|claim| claim.ttl.as_deref())
    {
        Some(ttl) => Some((chrono::Utc::now() + parse_duration(ttl)?).to_rfc3339()),
        None => None,
    };
    let _lock =
        claim_store.try_acquire_active_lock(issue_ref_text, &owner, lease_expires_at.as_deref())?;

    let mut claim = claim_store
        .load(issue_ref_text)?
        .unwrap_or(AutoflowClaimRecord {
            issue_ref: issue_ref_text.to_string(),
            repo_ref: resolved.repo_ref.clone(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: resolved.workflow.name.clone(),
            status: ClaimStatus::Claimed,
            worktree_path: None,
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        });
    claim.source_ref = Some(resolved_source_ref_text(resolved).unwrap_or_else(|_| {
        autoflow
            .source
            .clone()
            .unwrap_or_else(|| resolved.repo_ref.clone())
    }));
    claim.issue_display_ref = Some(issue_display_ref(issue));
    claim.issue_title = Some(issue.title.clone());
    claim.issue_url = issue_url(&resolved.cfg, issue);
    claim.issue_state_name = Some(issue_state_name(issue).to_string());
    claim.issue_tracker = Some(issue.r.tracker.as_str().to_string());
    claim.workflow = resolved.workflow.name.clone();
    claim.status = ClaimStatus::Running;
    claim.worktree_path = Some(workspace_path.display().to_string());
    claim.branch = Some(branch.clone());
    claim.claim_owner = Some(owner);
    claim.lease_expires_at = lease_expires_at;
    claim.pending_dispatch = None;
    claim.contenders = contenders;
    claim.updated_at = chrono::Utc::now().to_rfc3339();
    claim_store.save(&claim)?;
    let live_event_hook =
        attach_ui.then(|| build_autoflow_live_event_hook(&claim, live_cycle_recorder.clone()));

    let permission_mode = resolve_autoflow_permission_mode(
        mode_override,
        resolved.cfg.autoflow.permission_mode.as_deref(),
    )?;
    let run_result = run_with_explicit_context(
        &resolved.name,
        ExplicitWorkflowRunContext {
            project_root: resolved.project_root.clone(),
            workspace_path,
            workspace_id: ws.id,
            inputs: inputs.into_iter().collect(),
            mode: permission_mode,
            invocation_source: RunTriggerSource::Autoflow,
            event: None,
            issue: Some(issue_payload),
            issue_ref: Some(issue_ref_text.to_string()),
            system_prompt_suffix: Some(crate::run_target::format_run_target_for_prompt(
                &crate::run_target::RunTarget::Issue {
                    tracker: issue.r.tracker,
                    project: issue.r.project.clone(),
                    number: issue.r.number,
                },
            )),
            attach_ui,
            run_id_override: None,
            strict_templates: resolved.cfg.autoflow.strict_templates.unwrap_or(true),
            run_envelope_template: Some(RunEnvelopeTemplate {
                repo_ref: Some(resolved.repo_ref.clone()),
                wake_id: None,
                event_id: None,
                backend: Some("local_worktree".to_string()),
                workspace_strategy: Some(
                    match workspace_strategy {
                        AutoflowWorkspaceStrategy::Worktree => "managed_worktree",
                        AutoflowWorkspaceStrategy::InPlace => "in_place_checkout",
                    }
                    .to_string(),
                ),
                autoflow_name: Some(resolved.workflow.name.clone()),
                autoflow_claim_id: Some(issue_ref_text.to_string()),
                autoflow_priority: Some(autoflow.priority),
                requested_worker: None,
                target: Some(issue_ref_text.to_string()),
                correlation: None,
            }),
            worker,
            live_event_hook,
            shared_printer,
            live_view: crate::cmd::ui::LiveViewMode::Focused,
        },
    )
    .await;

    match run_result {
        Ok(summary) => {
            claim.last_run_id = Some(summary.run_id.clone());
            claim.artifact_manifest_path = summary
                .artifact_manifest_path
                .as_ref()
                .map(|path| path.display().to_string());
            if summary.awaiting_step_id.is_some() {
                claim.status = ClaimStatus::AwaitHuman;
                claim.last_error = None;
                claim.updated_at = chrono::Utc::now().to_rfc3339();
                claim_store.save(&claim)?;
                return Ok(());
            }
            if let Err(err) =
                apply_terminal_run_to_claim(global, resolved, &summary.run_id, &mut claim)
            {
                claim.status = ClaimStatus::Blocked;
                claim.last_error = Some(err.to_string());
                claim.updated_at = chrono::Utc::now().to_rfc3339();
                claim_store.save(&claim)?;
                return Err(err);
            }
            claim_store.save(&claim)?;
            Ok(())
        }
        Err(err) => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = Some(err.to_string());
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            claim_store.save(&claim)?;
            Err(err)
        }
    }
}

fn resolve_autoflow_permission_mode(
    mode_override: Option<&str>,
    config_mode: Option<&str>,
) -> anyhow::Result<String> {
    let mode = mode_override
        .map(ToOwned::to_owned)
        .or_else(|| config_mode.map(ToOwned::to_owned))
        .unwrap_or_else(|| "bypass".to_string());
    match mode.as_str() {
        "bypass" | "readonly" => Ok(mode),
        "ask" => {
            bail!("autoflow does not support `ask` permission mode; use `bypass` or `readonly`")
        }
        _ => bail!("invalid autoflow permission mode `{mode}`; expected `bypass` or `readonly`"),
    }
}

pub(crate) fn reconcile_claim_from_last_run(
    global: &Path,
    resolved: &ResolvedAutoflowWorkflow,
    claim: &mut AutoflowClaimRecord,
) -> anyhow::Result<()> {
    if !matches!(claim.status, ClaimStatus::AwaitHuman | ClaimStatus::Running) {
        return Ok(());
    }
    let Some(run_id) = claim.last_run_id.clone() else {
        return Ok(());
    };
    let run_store = RunStore::new(global.join("runs"));
    let run = match run_store.load(&run_id) {
        Ok(run) => run,
        Err(_) => return Ok(()),
    };
    match run.status {
        RunStatus::AwaitingApproval => {
            claim.status = ClaimStatus::AwaitHuman;
        }
        RunStatus::Completed => {
            apply_terminal_run_to_claim(global, resolved, &run_id, claim)?;
        }
        RunStatus::Failed | RunStatus::Rejected => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = run
                .error_message
                .clone()
                .or_else(|| Some(run.status.as_str().to_string()));
            claim.updated_at = chrono::Utc::now().to_rfc3339();
        }
        RunStatus::Pending | RunStatus::Running => {}
    }
    Ok(())
}

fn apply_terminal_run_to_claim(
    global: &Path,
    resolved: &ResolvedAutoflowWorkflow,
    run_id: &str,
    claim: &mut AutoflowClaimRecord,
) -> anyhow::Result<()> {
    let run_store = RunStore::new(global.join("runs"));
    let run = run_store.load(run_id)?;
    match run.status {
        RunStatus::Completed => {}
        RunStatus::AwaitingApproval => {
            claim.status = ClaimStatus::AwaitHuman;
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            return Ok(());
        }
        RunStatus::Failed | RunStatus::Rejected => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = run
                .error_message
                .clone()
                .or_else(|| Some(run.status.as_str().to_string()));
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            return Ok(());
        }
        RunStatus::Pending | RunStatus::Running => {
            claim.status = ClaimStatus::Running;
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            return Ok(());
        }
    }

    let (contract, output_record) = autoflow_output_record(&run_store, resolved, run_id)?;
    let raw_output = match contract.format {
        ContractFormat::Json => {
            parse_json_contract_output(&output_record.output, &contract.from_step)?
        }
        ContractFormat::Yaml => {
            let yaml_value: serde_yaml::Value = serde_yaml::from_str(&output_record.output)
                .with_context(|| {
                    format!("parse YAML outcome from step `{}`", contract.from_step)
                })?;
            serde_json::to_value(yaml_value)?
        }
    };
    let raw_output = normalize_autoflow_outcome_shape(
        &contract,
        unwrap_schema_named_outcome(&contract, raw_output),
        &claim.issue_ref,
    );
    validate_output_contract(
        global,
        resolved.project_root.as_deref(),
        &contract,
        &raw_output,
    )?;
    let outcome: AutoflowOutcomeDoc = serde_json::from_value(raw_output)?;
    claim.last_error = None;
    claim.last_summary = outcome.summary.clone();
    claim.pr_url = outcome.pr_url.clone();
    claim.artifacts = outcome.artifacts.clone();
    claim.artifact_manifest_path = run
        .artifact_manifest_path
        .as_ref()
        .map(|path| path.display().to_string());
    claim.next_retry_at = None;
    claim.pending_dispatch = None;

    if let Some(dispatch) = outcome.dispatch {
        let target = canonical_autoflow_issue_ref(&dispatch.target)?;
        if target != claim.issue_ref {
            bail!(
                "autoflow dispatch target `{}` does not match claimed issue `{}`",
                target,
                claim.issue_ref
            );
        }
        claim.pending_dispatch = Some(PendingDispatch {
            workflow: dispatch.workflow,
            target,
            inputs: dispatch.inputs,
        });
    }

    if let Some(reason) = outcome.reason {
        claim.last_error = Some(reason);
    }

    claim.status = match outcome.status {
        AutoflowOutcomeStatus::Continue => ClaimStatus::Claimed,
        AutoflowOutcomeStatus::AwaitHuman => ClaimStatus::AwaitHuman,
        AutoflowOutcomeStatus::AwaitExternal => ClaimStatus::AwaitExternal,
        AutoflowOutcomeStatus::Retry => {
            let retry_after = outcome
                .retry_after
                .as_deref()
                .ok_or_else(|| anyhow!("autoflow retry outcome must include `retry_after`"))?;
            claim.next_retry_at = Some(resolve_retry_at(retry_after)?);
            ClaimStatus::RetryBackoff
        }
        AutoflowOutcomeStatus::Blocked => ClaimStatus::Blocked,
        AutoflowOutcomeStatus::Complete => ClaimStatus::Complete,
    };
    if let Some(updated_state) = infer_issue_state_from_run(&run_store, run_id) {
        claim.issue_state_name = Some(updated_state);
    }
    claim.updated_at = chrono::Utc::now().to_rfc3339();
    Ok(())
}

fn unwrap_schema_named_outcome(
    contract: &WorkflowOutputContract,
    raw_output: serde_json::Value,
) -> serde_json::Value {
    let serde_json::Value::Object(map) = &raw_output else {
        return raw_output;
    };
    if map.len() != 1 {
        return raw_output;
    }
    match map.get(&contract.schema) {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => raw_output,
    }
}

fn normalize_autoflow_outcome_shape(
    contract: &WorkflowOutputContract,
    raw_output: serde_json::Value,
    issue_ref: &str,
) -> serde_json::Value {
    if contract.schema != "autoflow_outcome_v1" {
        return raw_output;
    }
    let serde_json::Value::Object(mut map) = raw_output else {
        return raw_output;
    };

    if !map.contains_key("summary") {
        if let Some(reason) = map.get("reason").cloned() {
            map.insert("summary".into(), reason);
        }
    }

    if !map.contains_key("dispatch") {
        if let Some(workflow) = map.get("workflow").cloned() {
            match workflow {
                serde_json::Value::String(workflow) => {
                    map.insert(
                        "dispatch".into(),
                        serde_json::json!({
                            "workflow": workflow,
                            "target": issue_ref,
                            "inputs": {}
                        }),
                    );
                }
                serde_json::Value::Object(mut dispatch_map) => {
                    if !dispatch_map.contains_key("target") {
                        dispatch_map.insert("target".into(), serde_json::json!(issue_ref));
                    }
                    if !dispatch_map.contains_key("inputs") {
                        dispatch_map.insert("inputs".into(), serde_json::json!({}));
                    }
                    map.insert("dispatch".into(), serde_json::Value::Object(dispatch_map));
                }
                _ => {}
            }
        }
    }

    if !map.contains_key("status") {
        match map.get("decision").and_then(|value| value.as_str()) {
            Some("dispatch") => {
                map.insert("status".into(), serde_json::json!("continue"));
            }
            Some(
                "continue" | "await_human" | "await_external" | "retry" | "blocked" | "complete",
            ) => {
                map.insert(
                    "status".into(),
                    serde_json::json!(map["decision"].as_str().unwrap()),
                );
            }
            _ if map.contains_key("dispatch") => {
                map.insert("status".into(), serde_json::json!("continue"));
            }
            _ => {}
        }
    }

    if let Some(dispatch) = map.get("dispatch").cloned() {
        match dispatch {
            serde_json::Value::String(workflow) => {
                map.insert(
                    "dispatch".into(),
                    serde_json::json!({
                        "workflow": workflow,
                        "target": issue_ref,
                        "inputs": {}
                    }),
                );
            }
            serde_json::Value::Object(mut dispatch_map) => {
                if !dispatch_map.contains_key("target") {
                    dispatch_map.insert("target".into(), serde_json::json!(issue_ref));
                }
                if !dispatch_map.contains_key("inputs") {
                    dispatch_map.insert("inputs".into(), serde_json::json!({}));
                }
                dispatch_map.retain(|key, _| {
                    matches!(key.as_str(), "workflow" | "target" | "inputs" | "summary")
                });
                map.insert("dispatch".into(), serde_json::Value::Object(dispatch_map));
            }
            _ => {}
        }
    }

    if !map.contains_key("status") && map.contains_key("dispatch") {
        map.insert("status".into(), serde_json::json!("continue"));
    }

    map.retain(|key, _| {
        matches!(
            key.as_str(),
            "status" | "summary" | "dispatch" | "retry_after" | "pr_url" | "reason" | "artifacts"
        )
    });

    serde_json::Value::Object(map)
}

fn infer_issue_state_from_run(run_store: &RunStore, run_id: &str) -> Option<String> {
    let rows = run_store.read_step_results(run_id).ok()?;
    let mut latest = None;
    for row in rows {
        infer_issue_state_from_transcript_path(&row.transcript_path, &mut latest);
        for item in row.items {
            infer_issue_state_from_transcript_path(&item.transcript_path, &mut latest);
        }
    }
    latest
}

fn infer_issue_state_from_transcript_path(path: &Path, latest: &mut Option<String>) {
    if path.as_os_str().is_empty() || !path.exists() {
        return;
    }
    let Ok(iter) = JsonlReader::iter(path) else {
        return;
    };
    for event in iter.flatten() {
        if let TranscriptEvent::ToolCall { tool, input, .. } = event {
            if tool == "issues.update_state" {
                if let Some(state) = input.get("state").and_then(|value| value.as_str()) {
                    *latest = Some(state.to_string());
                }
            }
        }
    }
}

fn parse_json_contract_output(raw: &str, step_id: &str) -> anyhow::Result<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(raw)
        .or_else(|_| {
            for block in markdown_fenced_blocks(raw) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(block.trim()) {
                    return Ok(value);
                }
            }
            if let Some(candidate) = extract_balanced_json_candidate(raw) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
                    return Ok(value);
                }
            }
            serde_json::from_str::<serde_json::Value>(raw)
        })
        .with_context(|| format!("parse JSON outcome from step `{step_id}`"))
}

fn markdown_fenced_blocks(raw: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("```") {
        let after_ticks = &rest[start + 3..];
        let Some(newline) = after_ticks.find('\n') else {
            break;
        };
        let body = &after_ticks[newline + 1..];
        let Some(end) = body.find("```") else {
            break;
        };
        blocks.push(&body[..end]);
        rest = &body[end + 3..];
    }
    blocks
}

fn extract_balanced_json_candidate(raw: &str) -> Option<String> {
    for opener in ['{', '['] {
        let closer = if opener == '{' { '}' } else { ']' };
        let Some(start) = raw.find(opener) else {
            continue;
        };
        let mut depth = 0usize;
        let mut in_string = false;
        let mut escape = false;
        for (offset, ch) in raw[start..].char_indices() {
            if in_string {
                if escape {
                    escape = false;
                    continue;
                }
                match ch {
                    '\\' => escape = true,
                    '"' => in_string = false,
                    _ => {}
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                c if c == opener => depth += 1,
                c if c == closer => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let end = start + offset + ch.len_utf8();
                        return Some(raw[start..end].to_string());
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn autoflow_output_record(
    run_store: &RunStore,
    resolved: &ResolvedAutoflowWorkflow,
    run_id: &str,
) -> anyhow::Result<(WorkflowOutputContract, StepResultRecord)> {
    let autoflow = resolved.autoflow()?;
    let outcome_ref = autoflow.outcome.as_ref().ok_or_else(|| {
        anyhow!(
            "autoflow `{}` does not declare `autoflow.outcome.output`",
            resolved.name
        )
    })?;
    let contract = resolved
        .workflow
        .contracts
        .outputs
        .get(&outcome_ref.output)
        .ok_or_else(|| {
            anyhow!(
                "workflow `{}` missing output contract `{}`",
                resolved.name,
                outcome_ref.output
            )
        })?;
    let step_result = run_store
        .read_step_results(run_id)?
        .into_iter()
        .rev()
        .find(|record| record.step_id == contract.from_step)
        .ok_or_else(|| {
            anyhow!(
                "run `{run_id}` did not persist output for step `{}`",
                contract.from_step
            )
        })?;
    Ok((contract.clone(), step_result))
}

fn validate_output_contract(
    global: &Path,
    project_root: Option<&Path>,
    contract: &WorkflowOutputContract,
    output: &serde_json::Value,
) -> anyhow::Result<()> {
    let schema_path = resolve_contract_path(global, project_root, &contract.schema)?;
    let schema_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&schema_path)?)
            .with_context(|| format!("parse contract schema {}", schema_path.display()))?;
    let compiled = JSONSchema::compile(&schema_json)
        .map_err(|err| anyhow!("compile {}: {err}", schema_path.display()))?;
    if let Err(errors) = compiled.validate(output) {
        let messages = errors
            .take(5)
            .map(|err| err.to_string())
            .collect::<Vec<_>>();
        bail!(
            "output failed schema `{}` validation: {}",
            contract.schema,
            messages.join("; ")
        );
    }
    Ok(())
}

fn resolve_contract_path(
    global: &Path,
    project_root: Option<&Path>,
    schema: &str,
) -> anyhow::Result<PathBuf> {
    if let Some(project_root) = project_root {
        let path = project_root
            .join(".rupu/contracts")
            .join(format!("{schema}.json"));
        if path.is_file() {
            return Ok(path);
        }
    }
    let global_path = global.join("contracts").join(format!("{schema}.json"));
    if global_path.is_file() {
        return Ok(global_path);
    }
    bail!("contract schema not found: {schema}")
}

fn resolve_retry_at(value: &str) -> anyhow::Result<String> {
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(parsed.with_timezone(&chrono::Utc).to_rfc3339());
    }
    Ok((chrono::Utc::now() + parse_duration(value)?).to_rfc3339())
}

pub(crate) fn should_run_claim(
    claim: &AutoflowClaimRecord,
    resolved: &ResolvedAutoflowWorkflow,
    claim_store: &AutoflowClaimStore,
    tick_started_at: chrono::DateTime<chrono::Utc>,
    wake_events: &BTreeSet<String>,
) -> anyhow::Result<bool> {
    if claim_store.read_active_lock(&claim.issue_ref)?.is_some() {
        return Ok(false);
    }
    let wake_due = wake_events_match(resolved.autoflow()?, wake_events);
    match claim.status {
        ClaimStatus::Eligible
        | ClaimStatus::Claimed
        | ClaimStatus::Running
        | ClaimStatus::AwaitExternal => {
            if wake_due {
                Ok(true)
            } else {
                due_by_reconcile_interval(claim, resolved)
            }
        }
        ClaimStatus::RetryBackoff => due_by_retry_backoff(claim),
        ClaimStatus::AwaitHuman => Ok(false),
        ClaimStatus::Blocked | ClaimStatus::Complete | ClaimStatus::Released => Ok(false),
    }
    .map(|due| {
        if claim.pending_dispatch.is_some() {
            due && updated_before_tick(claim, tick_started_at).unwrap_or(false)
        } else {
            due
        }
    })
}

fn due_by_reconcile_interval(
    claim: &AutoflowClaimRecord,
    resolved: &ResolvedAutoflowWorkflow,
) -> anyhow::Result<bool> {
    let Some(interval) = resolved.autoflow()?.reconcile_every.as_deref() else {
        return Ok(false);
    };
    let last = chrono::DateTime::parse_from_rfc3339(&claim.updated_at)
        .with_context(|| format!("parse claim updated_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(last + parse_duration(interval)? <= chrono::Utc::now())
}

fn due_by_retry_backoff(claim: &AutoflowClaimRecord) -> anyhow::Result<bool> {
    let Some(next_retry_at) = claim.next_retry_at.as_deref() else {
        return Ok(true);
    };
    let retry_at = chrono::DateTime::parse_from_rfc3339(next_retry_at)
        .with_context(|| format!("parse next_retry_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(retry_at <= chrono::Utc::now())
}

pub(crate) fn updated_before_tick(
    claim: &AutoflowClaimRecord,
    tick_started_at: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<bool> {
    let updated = chrono::DateTime::parse_from_rfc3339(&claim.updated_at)
        .with_context(|| format!("parse updated_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(updated < tick_started_at)
}

pub(crate) fn claim_lease_expired(claim: &AutoflowClaimRecord) -> anyhow::Result<bool> {
    let Some(lease_expires_at) = claim.lease_expires_at.as_deref() else {
        return Ok(false);
    };
    let lease = chrono::DateTime::parse_from_rfc3339(lease_expires_at)
        .with_context(|| format!("parse lease expiry for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(lease <= chrono::Utc::now())
}

pub(crate) fn claim_counts_toward_max_active(status: ClaimStatus) -> bool {
    !matches!(status, ClaimStatus::Complete | ClaimStatus::Released)
}

pub(crate) fn adjust_active_claim_count(
    counts: &mut BTreeMap<String, usize>,
    repo_ref: &str,
    before: Option<ClaimStatus>,
    after: Option<ClaimStatus>,
) {
    let counted_before = before.is_some_and(claim_counts_toward_max_active);
    let counted_after = after.is_some_and(claim_counts_toward_max_active);
    match (counted_before, counted_after) {
        (false, true) => {
            *counts.entry(repo_ref.to_string()).or_insert(0) += 1;
        }
        (true, false) => {
            if let Some(count) = counts.get_mut(repo_ref) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(repo_ref);
                }
            }
        }
        _ => {}
    }
}

fn wake_events_match(
    autoflow: &rupu_orchestrator::Autoflow,
    wake_events: &BTreeSet<String>,
) -> bool {
    if autoflow.wake_on.is_empty() || wake_events.is_empty() {
        return false;
    }
    autoflow.wake_on.iter().any(|pattern| {
        wake_events
            .iter()
            .any(|event_id| rupu_orchestrator::event_matches(pattern, event_id))
    })
}

#[allow(clippy::too_many_arguments)]
fn push_resolved_autoflow_paths(
    dir: &Path,
    scope: &str,
    global: &Path,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
    into: &mut Vec<ResolvedAutoflowWorkflow>,
) -> anyhow::Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("yaml" | "yml")) {
            continue;
        }
        let body = std::fs::read_to_string(&path)?;
        let workflow = Workflow::parse(&body)?;
        if !workflow
            .autoflow
            .as_ref()
            .map(|autoflow| autoflow.enabled)
            .unwrap_or(false)
        {
            continue;
        }
        let resolved = resolve_autoflow_from_path(
            global,
            path,
            scope.to_string(),
            project_root.clone(),
            repo_ref.clone(),
            preferred_checkout.clone(),
            cfg.clone(),
        )?;
        into.push(resolved);
    }
    Ok(())
}

pub(crate) fn resolve_autoflow_workflow_for_repo(
    global: &Path,
    repo_store: &RepoRegistryStore,
    repo_ref: &str,
    name: &str,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let tracked = repo_store
        .load(repo_ref)?
        .ok_or_else(|| anyhow!("repo `{repo_ref}` is not tracked"))?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);
    let project_root =
        paths::project_root_for(&preferred_checkout)?.or_else(|| Some(preferred_checkout.clone()));
    let cfg = resolve_config(global, project_root.as_deref())?;
    let workflow_path = locate_workflow_in(global, project_root.as_deref(), name)?;
    resolve_autoflow_from_path(
        global,
        workflow_path,
        "project".into(),
        project_root,
        repo_ref.to_string(),
        preferred_checkout,
        cfg,
    )
}

pub(crate) fn workflow_declares_autoflow_for_repo(
    global: &Path,
    repo_store: &RepoRegistryStore,
    repo_ref: &str,
    name: &str,
) -> anyhow::Result<bool> {
    let tracked = repo_store
        .load(repo_ref)?
        .ok_or_else(|| anyhow!("repo `{repo_ref}` is not tracked"))?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);
    let project_root =
        paths::project_root_for(&preferred_checkout)?.or_else(|| Some(preferred_checkout.clone()));
    let workflow_path = locate_workflow_in(global, project_root.as_deref(), name)?;
    let body = std::fs::read_to_string(&workflow_path)?;
    let workflow = Workflow::parse(&body)?;
    Ok(workflow
        .autoflow
        .as_ref()
        .map(|autoflow| autoflow.enabled)
        .unwrap_or(false))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_pending_dispatch_workflow(
    global: &Path,
    repo_store: &RepoRegistryStore,
    claim_store: &AutoflowClaimStore,
    base_resolved: &ResolvedAutoflowWorkflow,
    claim: &mut AutoflowClaimRecord,
    issue: &Issue,
    issue_ref_text: &str,
    workflow_name: &str,
    inputs: BTreeMap<String, String>,
    attach_ui: bool,
    worker: Option<ExecutionWorkerContext>,
    shared_printer: Option<Arc<Mutex<LineStreamPrinter>>>,
    live_cycle_recorder: Option<Arc<crate::cmd::autoflow_runtime::LiveCycleRecorder>>,
) -> anyhow::Result<()> {
    let tracked = repo_store
        .load(&claim.repo_ref)?
        .ok_or_else(|| anyhow!("repo `{}` is not tracked", claim.repo_ref))?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);
    let project_root =
        paths::project_root_for(&preferred_checkout)?.or_else(|| Some(preferred_checkout.clone()));
    let cfg = resolve_config(global, project_root.as_deref())?;
    let workspace_path = claim
        .worktree_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| preferred_checkout.clone());
    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &workspace_path)?;
    let issue_payload = issue_payload(&cfg, issue)?;
    let permission_mode =
        resolve_autoflow_permission_mode(None, cfg.autoflow.permission_mode.as_deref())?;

    claim.status = ClaimStatus::Running;
    claim.last_error = None;
    claim.pending_dispatch = None;
    claim.updated_at = chrono::Utc::now().to_rfc3339();
    claim_store.save(claim)?;
    let live_event_hook =
        attach_ui.then(|| build_autoflow_live_event_hook(claim, live_cycle_recorder.clone()));

    let result = run_with_explicit_context(
        workflow_name,
        ExplicitWorkflowRunContext {
            project_root,
            workspace_path,
            workspace_id: ws.id,
            inputs: inputs.into_iter().collect(),
            mode: permission_mode,
            invocation_source: RunTriggerSource::Autoflow,
            event: None,
            issue: Some(issue_payload),
            issue_ref: Some(issue_ref_text.to_string()),
            system_prompt_suffix: Some(crate::run_target::format_run_target_for_prompt(
                &crate::run_target::RunTarget::Issue {
                    tracker: issue.r.tracker,
                    project: issue.r.project.clone(),
                    number: issue.r.number,
                },
            )),
            attach_ui,
            run_id_override: None,
            strict_templates: cfg.autoflow.strict_templates.unwrap_or(true),
            run_envelope_template: Some(RunEnvelopeTemplate {
                repo_ref: Some(claim.repo_ref.clone()),
                wake_id: None,
                event_id: None,
                backend: Some("local_worktree".to_string()),
                workspace_strategy: Some("worktree".to_string()),
                autoflow_name: Some(base_resolved.name.clone()),
                autoflow_claim_id: Some(issue_ref_text.to_string()),
                autoflow_priority: base_resolved
                    .autoflow()
                    .ok()
                    .map(|autoflow| autoflow.priority),
                requested_worker: worker.as_ref().map(|value| value.worker_id.clone()),
                target: Some(issue_ref_text.to_string()),
                correlation: None,
            }),
            worker,
            live_event_hook,
            shared_printer,
            live_view: crate::cmd::ui::LiveViewMode::Focused,
        },
    )
    .await;

    match result {
        Ok(summary) => {
            claim.last_run_id = Some(summary.run_id.clone());
            claim.artifact_manifest_path = summary
                .artifact_manifest_path
                .as_ref()
                .map(|path| path.display().to_string());
            if summary.awaiting_step_id.is_some() {
                claim.status = ClaimStatus::AwaitHuman;
            } else {
                claim.status = ClaimStatus::Claimed;
            }
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            claim_store.save(claim)?;
            Ok(())
        }
        Err(error) => {
            claim.status = ClaimStatus::Blocked;
            claim.last_error = Some(error.to_string());
            claim.updated_at = chrono::Utc::now().to_rfc3339();
            claim_store.save(claim)?;
            Err(error)
        }
    }
}

fn resolve_autoflow_workflow_for_issue(
    global: &Path,
    name: &str,
    issue_ref: &IssueRef,
    repo: Option<&str>,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let matches = visible_autoflow_matches_for_issue(issue_ref, repo)?
        .into_iter()
        .filter(|entry| entry.name == name)
        .collect::<Vec<_>>();
    let entry = match matches.as_slice() {
        [] => {
            let issue_ref_text = format_issue_ref(issue_ref);
            match repo {
                Some(repo) => bail!(
                    "no autoflow named `{name}` is visible for issue `{issue_ref_text}` and repo `{repo}`"
                ),
                None => bail!("no autoflow named `{name}` is visible for issue `{issue_ref_text}`"),
            }
        }
        [entry] => entry.clone(),
        many => {
            let mut options = many
                .iter()
                .map(|entry| {
                    format!(
                        "- {} ({}, {})",
                        entry.repo_ref.as_deref().unwrap_or("-"),
                        entry.scope,
                        entry.workflow_path.display()
                    )
                })
                .collect::<Vec<_>>();
            options.sort();
            bail!(
                "multiple autoflows named `{name}` are visible for issue `{}`:\n{}\npass `--repo <platform>:<owner>/<repo>` to disambiguate",
                format_issue_ref(issue_ref),
                options.join("\n")
            );
        }
    };
    resolve_visible_autoflow_workflow(global, &entry)
}

fn resolve_visible_autoflow_workflow(
    global: &Path,
    entry: &VisibleAutoflowWorkflow,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let repo_ref = entry
        .repo_ref
        .clone()
        .ok_or_else(|| anyhow!("autoflow `{}` is missing a repo binding", entry.name))?;
    let preferred_checkout = entry
        .preferred_checkout
        .clone()
        .or_else(|| entry.project_root.clone())
        .ok_or_else(|| anyhow!("autoflow `{}` is missing a preferred checkout", entry.name))?;
    let cfg = resolve_config(global, entry.project_root.as_deref())?;
    resolve_autoflow_from_path(
        global,
        entry.workflow_path.clone(),
        entry.scope.clone(),
        entry.project_root.clone(),
        repo_ref,
        preferred_checkout,
        cfg,
    )
}

fn resolve_autoflow_from_path(
    _global: &Path,
    workflow_path: PathBuf,
    scope: String,
    project_root: Option<PathBuf>,
    repo_ref: String,
    preferred_checkout: PathBuf,
    cfg: Config,
) -> anyhow::Result<ResolvedAutoflowWorkflow> {
    let body = std::fs::read_to_string(&workflow_path)?;
    let workflow = Workflow::parse(&body)?;
    let enabled = workflow
        .autoflow
        .as_ref()
        .map(|autoflow| autoflow.enabled)
        .unwrap_or(false);
    if !enabled {
        bail!(
            "workflow `{}` does not declare `autoflow.enabled = true`",
            workflow.name
        );
    }
    Ok(ResolvedAutoflowWorkflow {
        scope,
        name: workflow.name.clone(),
        workflow,
        workflow_path,
        project_root,
        repo_ref,
        preferred_checkout,
        cfg,
    })
}

fn issue_payload(cfg: &Config, issue: &Issue) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(issue)?;
    if let Some(obj) = value.as_object_mut() {
        obj.entry("number")
            .or_insert_with(|| serde_json::json!(issue.r.number));
        obj.entry("ref")
            .or_insert_with(|| serde_json::json!(format_issue_ref(&issue.r)));
        obj.entry("project")
            .or_insert_with(|| serde_json::json!(issue.r.project));
        obj.entry("tracker")
            .or_insert_with(|| serde_json::json!(issue.r.tracker.to_string()));
        obj.entry("state_name")
            .or_insert_with(|| serde_json::json!(issue_state_name(issue)));
        if let Some(url) = issue_url(cfg, issue) {
            obj.entry("url").or_insert_with(|| serde_json::json!(url));
        }
    }
    Ok(value)
}

fn resolve_config(global: &Path, project_root: Option<&Path>) -> anyhow::Result<Config> {
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.map(|root| root.join(".rupu/config.toml"));
    Ok(rupu_config::layer_files(
        Some(&global_cfg_path),
        project_cfg_path.as_deref(),
    )?)
}

pub(crate) fn cleanup_terminal_claims(
    global: &Path,
    repo_store: &RepoRegistryStore,
    claim_store: &AutoflowClaimStore,
    now: chrono::DateTime<chrono::Utc>,
    repo_filter: Option<&str>,
) -> anyhow::Result<usize> {
    let mut cleaned = 0usize;
    for claim in claim_store.list()? {
        if repo_filter.is_some_and(|repo| claim.repo_ref != repo) {
            continue;
        }
        if !matches!(claim.status, ClaimStatus::Complete | ClaimStatus::Released) {
            continue;
        }
        let Some(cleanup_after) = cleanup_after_for_claim(global, repo_store, &claim)? else {
            continue;
        };
        if !claim_cleanup_due(&claim, now, cleanup_after)? {
            continue;
        }
        match cleanup_claim_artifacts(repo_store, &claim) {
            Ok(_) => {
                claim_store.delete(&claim.issue_ref)?;
                cleaned += 1;
            }
            Err(err) => warn!(
                issue_ref = %claim.issue_ref,
                repo_ref = %claim.repo_ref,
                error = %err,
                "failed to cleanup terminal autoflow claim"
            ),
        }
    }
    Ok(cleaned)
}

fn cleanup_after_for_claim(
    global: &Path,
    repo_store: &RepoRegistryStore,
    claim: &AutoflowClaimRecord,
) -> anyhow::Result<Option<chrono::Duration>> {
    let tracked = repo_store.load(&claim.repo_ref)?;
    let project_root = tracked
        .as_ref()
        .and_then(|tracked| PathBuf::from(&tracked.preferred_path).canonicalize().ok())
        .and_then(|preferred_checkout| {
            paths::project_root_for(&preferred_checkout)
                .ok()
                .flatten()
                .or(Some(preferred_checkout))
        });
    let cfg = resolve_config(global, project_root.as_deref())?;
    cfg.autoflow
        .cleanup_after
        .as_deref()
        .map(parse_duration)
        .transpose()
}

fn claim_cleanup_due(
    claim: &AutoflowClaimRecord,
    now: chrono::DateTime<chrono::Utc>,
    cleanup_after: chrono::Duration,
) -> anyhow::Result<bool> {
    let updated_at = chrono::DateTime::parse_from_rfc3339(&claim.updated_at)
        .with_context(|| format!("parse claim updated_at for `{}`", claim.issue_ref))?
        .with_timezone(&chrono::Utc);
    Ok(updated_at + cleanup_after <= now)
}

fn cleanup_claim_artifacts(
    repo_store: &RepoRegistryStore,
    claim: &AutoflowClaimRecord,
) -> anyhow::Result<()> {
    let Some(worktree_path) = claim.worktree_path.as_deref() else {
        return Ok(());
    };
    let worktree_path = PathBuf::from(worktree_path);
    if !worktree_path.exists() {
        return Ok(());
    }
    let tracked = repo_store
        .load(&claim.repo_ref)?
        .ok_or_else(|| anyhow!("repo `{}` is not tracked", claim.repo_ref))?;
    let preferred_checkout = PathBuf::from(&tracked.preferred_path);
    let preferred_canonical = preferred_checkout
        .canonicalize()
        .unwrap_or_else(|_| preferred_checkout.clone());
    let worktree_canonical = worktree_path
        .canonicalize()
        .unwrap_or_else(|_| worktree_path.clone());
    if worktree_canonical == preferred_canonical {
        return Ok(());
    }
    remove_issue_worktree(&preferred_checkout, &worktree_path)
        .with_context(|| format!("remove worktree {}", worktree_path.display()))?;
    Ok(())
}

fn build_issue_filter(autoflow: &rupu_orchestrator::Autoflow) -> IssueFilter {
    let state = match autoflow.selector.states.as_slice() {
        [rupu_orchestrator::AutoflowIssueState::Open] => Some(IssueState::Open),
        [rupu_orchestrator::AutoflowIssueState::Closed] => Some(IssueState::Closed),
        _ => None,
    };
    // The SCM issue connectors only support a conjunctive label filter,
    // so keep using `labels_all` as the server-side narrowing set and
    // apply `labels_any` / `labels_none` client-side in `selector_matches`.
    IssueFilter {
        state,
        labels: autoflow.selector.labels_all.clone(),
        author: None,
        limit: autoflow.selector.limit,
    }
}

fn selector_matches(autoflow: &rupu_orchestrator::Autoflow, issue: &Issue) -> bool {
    if !autoflow.selector.states.is_empty() {
        let state_matches = autoflow.selector.states.iter().any(|state| {
            matches!(
                (state, issue.state),
                (
                    rupu_orchestrator::AutoflowIssueState::Open,
                    IssueState::Open
                ) | (
                    rupu_orchestrator::AutoflowIssueState::Closed,
                    IssueState::Closed
                )
            )
        });
        if !state_matches {
            return false;
        }
    }
    if !autoflow
        .selector
        .labels_all
        .iter()
        .all(|label| issue.labels.iter().any(|existing| existing == label))
    {
        return false;
    }
    if !autoflow.selector.labels_any.is_empty()
        && !autoflow
            .selector
            .labels_any
            .iter()
            .any(|label| issue.labels.iter().any(|existing| existing == label))
    {
        return false;
    }
    if autoflow
        .selector
        .labels_none
        .iter()
        .any(|label| issue.labels.iter().any(|existing| existing == label))
    {
        return false;
    }
    true
}

pub(crate) async fn fetch_issue(
    cfg: &Config,
    resolver: &dyn CredentialResolver,
    issue_ref: &IssueRef,
) -> anyhow::Result<Issue> {
    let registry = Arc::new(rupu_scm::Registry::discover(resolver, cfg).await);
    let connector = registry.issues(issue_ref.tracker).ok_or_else(|| {
        anyhow!(
            "no {} credential — run `rupu auth login --provider {}`",
            issue_ref.tracker,
            issue_ref.tracker
        )
    })?;
    connector
        .get_issue(issue_ref)
        .await
        .map_err(anyhow::Error::from)
}

fn parse_full_issue_target(target: &str) -> anyhow::Result<IssueRef> {
    match crate::run_target::parse_run_target(target) {
        Ok(crate::run_target::RunTarget::Issue {
            tracker,
            project,
            number,
        }) => Ok(IssueRef {
            tracker,
            project,
            number,
        }),
        _ => {
            bail!("autoflow run requires an issue target in `<platform>:<project>/issues/<N>` form")
        }
    }
}

pub(crate) fn parse_issue_ref_text(value: &str) -> anyhow::Result<IssueRef> {
    let (tracker, rest) = value
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid issue ref `{value}`"))?;
    let (project, number) = rest
        .rsplit_once("/issues/")
        .ok_or_else(|| anyhow!("invalid issue ref `{value}`"))?;
    Ok(IssueRef {
        tracker: IssueTracker::from_str(tracker).map_err(|err| anyhow!(err))?,
        project: project.to_string(),
        number: number.parse()?,
    })
}

fn resolved_source_ref_text(resolved: &ResolvedAutoflowWorkflow) -> anyhow::Result<String> {
    Ok(resolved
        .autoflow()?
        .source
        .clone()
        .unwrap_or_else(|| resolved.repo_ref.clone()))
}

fn resolved_event_source(resolved: &ResolvedAutoflowWorkflow) -> anyhow::Result<EventSourceRef> {
    resolved_source_ref_text(resolved)?
        .parse()
        .map_err(|err: String| anyhow!(err))
}

fn issue_discovery_target(source: &EventSourceRef) -> (IssueTracker, String) {
    match source {
        EventSourceRef::Repo { repo } => (
            match repo.platform {
                Platform::Github => IssueTracker::Github,
                Platform::Gitlab => IssueTracker::Gitlab,
            },
            format!("{}/{}", repo.owner, repo.repo),
        ),
        EventSourceRef::TrackerProject { tracker, project } => (*tracker, project.clone()),
    }
}

fn source_slug(source: &EventSourceRef) -> String {
    let text = match source {
        EventSourceRef::Repo { repo } => format!("repo-{}-{}", repo.owner, repo.repo),
        EventSourceRef::TrackerProject { project, .. } => format!("project-{project}"),
    };
    text.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '-',
        })
        .collect()
}

fn autoflow_cursor_path(root: &Path, source: &EventSourceRef) -> PathBuf {
    root.join(source.vendor())
        .join(format!("{}.cursor", source_slug(source)))
}

fn autoflow_last_polled_at_path(root: &Path, source: &EventSourceRef) -> PathBuf {
    root.join(source.vendor())
        .join(format!("{}.last_polled", source_slug(source)))
}

fn read_cursor(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

fn write_cursor(path: &Path, body: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("cursor.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_last_polled_at(path: &Path) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    Ok(
        chrono::DateTime::parse_from_rfc3339(std::fs::read_to_string(path)?.trim())?
            .with_timezone(&chrono::Utc),
    )
}

fn write_last_polled_at(path: &Path, at: chrono::DateTime<chrono::Utc>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("last_polled.tmp");
    std::fs::write(&tmp, at.to_rfc3339())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn autoflow_poll_source_due(
    source: &PollSourceEntry,
    last_polled_path: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<bool> {
    let Some(interval) = source.poll_interval() else {
        return Ok(true);
    };
    let last_polled = match read_last_polled_at(last_polled_path) {
        Ok(at) => at,
        Err(_) => return Ok(true),
    };
    Ok(last_polled + parse_duration(interval)? <= now)
}

fn format_issue_ref(issue_ref: &IssueRef) -> String {
    format!(
        "{}:{}/issues/{}",
        issue_ref.tracker, issue_ref.project, issue_ref.number
    )
}

fn issue_display_ref(issue: &Issue) -> String {
    match issue.r.tracker {
        IssueTracker::Jira => {
            let project_key = issue
                .r
                .project
                .rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
                .unwrap_or(&issue.r.project);
            format!("{project_key}-{}", issue.r.number)
        }
        _ => issue.r.number.to_string(),
    }
}

fn issue_state_name(issue: &Issue) -> &'static str {
    match issue.state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}

fn issue_url(cfg: &Config, issue: &Issue) -> Option<String> {
    match issue.r.tracker {
        IssueTracker::Github => Some(format!(
            "https://github.com/{}/issues/{}",
            issue.r.project, issue.r.number
        )),
        IssueTracker::Jira => {
            let project_key = issue.r.project.rsplit('/').next()?;
            let base_url = cfg
                .scm
                .platforms
                .get("jira")
                .and_then(|platform| platform.base_url.as_deref())?;
            Some(format!(
                "{}/browse/{}-{}",
                base_url.trim_end_matches('/'),
                project_key,
                issue.r.number
            ))
        }
        IssueTracker::Gitlab | IssueTracker::Linear => None,
    }
}

fn resolve_workspace_strategy(
    cfg: &rupu_config::AutoflowConfig,
    autoflow: &rupu_orchestrator::Autoflow,
) -> AutoflowWorkspaceStrategy {
    if let Some(workspace) = &autoflow.workspace {
        return workspace.strategy;
    }
    match cfg.checkout.unwrap_or(AutoflowCheckout::Worktree) {
        AutoflowCheckout::Worktree => AutoflowWorkspaceStrategy::Worktree,
        AutoflowCheckout::InPlace => AutoflowWorkspaceStrategy::InPlace,
    }
}

fn resolve_worktree_root(
    global: &Path,
    cfg: &rupu_config::AutoflowConfig,
) -> anyhow::Result<PathBuf> {
    match cfg.worktree_root.as_deref() {
        Some(path) => expand_user_path(path),
        None => Ok(paths::autoflow_worktrees_dir(global)),
    }
}

fn resolve_branch_name(
    template: Option<&str>,
    issue_payload: &serde_json::Value,
    issue_ref: &str,
    inputs: &BTreeMap<String, String>,
) -> anyhow::Result<String> {
    if let Some(template) = template {
        let mut ctx = StepContext::new().with_issue(issue_payload.clone());
        for (key, value) in inputs {
            ctx = ctx.with_input(key.clone(), value.clone());
        }
        let rendered = render_step_prompt(template, &ctx, RenderMode::Strict)?;
        let branch = rendered.trim();
        if !branch.is_empty() {
            return Ok(branch.to_string());
        }
    }
    Ok(format!("rupu/{}", issue_dir_name(issue_ref)))
}

fn expand_user_path(input: &str) -> anyhow::Result<PathBuf> {
    if input == "~" {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
        return Ok(home);
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("could not locate home directory"))?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(input))
}

fn parse_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    let trimmed = value.trim();
    let unit = trimmed
        .chars()
        .last()
        .ok_or_else(|| anyhow!("invalid duration `{value}`"))?;
    let amount: i64 = trimmed[..trimmed.len().saturating_sub(1)]
        .parse()
        .map_err(|e| anyhow!("invalid duration `{value}`: {e}"))?;
    let duration = match unit {
        's' => chrono::Duration::seconds(amount),
        'm' => chrono::Duration::minutes(amount),
        'h' => chrono::Duration::hours(amount),
        'd' => chrono::Duration::days(amount),
        _ => bail!("invalid duration `{value}`"),
    };
    Ok(duration)
}

fn status_name(status: ClaimStatus) -> &'static str {
    match status {
        ClaimStatus::Eligible => "eligible",
        ClaimStatus::Claimed => "claimed",
        ClaimStatus::Running => "running",
        ClaimStatus::AwaitHuman => "await_human",
        ClaimStatus::AwaitExternal => "await_external",
        ClaimStatus::RetryBackoff => "retry_backoff",
        ClaimStatus::Blocked => "blocked",
        ClaimStatus::Complete => "complete",
        ClaimStatus::Released => "released",
    }
}

fn selected_priority(claim: &AutoflowClaimRecord) -> Option<i32> {
    claim
        .contenders
        .iter()
        .find(|contender| contender.selected)
        .map(|contender| contender.priority)
}

fn next_action_summary(claim: &AutoflowClaimRecord) -> String {
    if let Some(dispatch) = &claim.pending_dispatch {
        return format!("dispatch {}", dispatch.workflow);
    }
    if let Some(next_retry_at) = &claim.next_retry_at {
        return format!("retry {next_retry_at}");
    }
    match claim.status {
        ClaimStatus::AwaitHuman => "human approval".into(),
        ClaimStatus::AwaitExternal => "external change".into(),
        ClaimStatus::Running => claim
            .last_run_id
            .clone()
            .unwrap_or_else(|| "running".into()),
        _ => claim.last_run_id.clone().unwrap_or_else(|| "-".into()),
    }
}

fn claim_summary(claim: &AutoflowClaimRecord) -> String {
    claim
        .last_summary
        .as_deref()
        .map(|value| truncate_for_table(value, 48))
        .unwrap_or_else(|| "-".into())
}

fn truncate_for_table(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    let chars = trimmed.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return trimmed.to_string();
    }
    let head = chars
        .into_iter()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{head}…")
}

fn format_contenders(contenders: &[AutoflowContender]) -> String {
    if contenders.is_empty() {
        return "-".into();
    }
    contenders
        .iter()
        .map(|contender| {
            let selected = if contender.selected { "*" } else { "" };
            format!("{selected}{}[{}]", contender.workflow, contender.priority)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::autoflow_wake;
    use crate::test_support::ENV_LOCK;
    use httpmock::Method::{GET, POST};
    use httpmock::MockServer;
    use rupu_auth::in_memory::InMemoryResolver;
    use rupu_auth::StoredCredential;
    use rupu_orchestrator::{RunRecord, StepKind, StepResultRecord};
    use rupu_providers::AuthMode;
    use rupu_runtime::{AutoflowCycleEventKind, AutoflowCycleMode, AutoflowHistoryStore};
    use std::io::Write;

    const COMPLETE_SCRIPT: &str = r#"
[
  {
    "AssistantText": {
      "text": "{\"status\":\"complete\",\"summary\":\"done\"}",
      "stop": "end_turn"
    }
  }
]
"#;

    fn init_git_repo(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        assert!(std::process::Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(path)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.email", "test@example.com"])
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "user.name", "Test User"])
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["config", "commit.gpgsign", "false"])
            .status()
            .unwrap()
            .success());
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["commit", "-m", "init"])
            .status()
            .unwrap()
            .success());
    }

    fn write_autoflow_project(
        project: &Path,
        base_url: &str,
        workflow_name: &str,
        workflow_yaml: &str,
    ) {
        std::fs::create_dir_all(project.join(".rupu/agents")).unwrap();
        std::fs::create_dir_all(project.join(".rupu/workflows")).unwrap();
        std::fs::create_dir_all(project.join(".rupu/contracts")).unwrap();
        std::fs::write(
            project.join(".rupu/agents/echo.md"),
            "---\nname: echo\nprovider: anthropic\nmodel: claude-sonnet-4-6\n---\nyou echo.\n",
        )
        .unwrap();
        std::fs::write(
            project.join(format!(".rupu/workflows/{workflow_name}.yaml")),
            workflow_yaml,
        )
        .unwrap();
        std::fs::write(
            project.join(".rupu/contracts/autoflow_outcome_v1.json"),
            r#"{
  "type": "object",
  "required": ["status"],
  "properties": {
    "status": {
      "type": "string",
      "enum": ["continue", "await_human", "await_external", "retry", "blocked", "complete"]
    },
    "summary": { "type": "string" },
    "retry_after": { "type": "string" },
    "dispatch": {
      "type": "object",
      "required": ["workflow", "target", "inputs"],
      "properties": {
        "workflow": { "type": "string" },
        "target": { "type": "string" },
        "inputs": {
          "type": "object",
          "additionalProperties": { "type": "string" }
        }
      }
    }
  }
}"#,
        )
        .unwrap();
        std::fs::write(
            project.join(".rupu/config.toml"),
            format!(
                r#"[autoflow]
enabled = true
permission_mode = "bypass"
strict_templates = true

[scm.github]
base_url = "{base_url}"
"#
            ),
        )
        .unwrap();
    }

    fn github_fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn jira_issue_response(
        key: &str,
        status_category_key: &str,
        labels: &[&str],
    ) -> serde_json::Value {
        serde_json::json!({
            "id": format!("id-{key}"),
            "key": key,
            "self": format!("https://acme.atlassian.net/rest/api/3/issue/{key}"),
            "fields": {
                "summary": format!("Issue {key}"),
                "description": {
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{ "type": "text", "text": format!("Description for {key}") }]
                    }]
                },
                "labels": labels,
                "status": {
                    "id": format!("status-{status_category_key}"),
                    "name": if status_category_key == "done" { "Done" } else { "In Progress" },
                    "statusCategory": {
                        "key": status_category_key,
                        "name": if status_category_key == "done" { "Done" } else { "In Progress" }
                    }
                },
                "reporter": { "displayName": "matt" },
                "created": "2026-05-10T00:00:00.000+0000",
                "updated": "2026-05-10T01:00:00.000+0000"
            }
        })
    }

    #[test]
    fn parse_duration_accepts_supported_units() {
        assert_eq!(
            parse_duration("10m").unwrap(),
            chrono::Duration::minutes(10)
        );
        assert_eq!(parse_duration("2h").unwrap(), chrono::Duration::hours(2));
    }

    #[test]
    fn issue_target_parser_rejects_repo_targets() {
        assert!(parse_full_issue_target("github:Section9Labs/rupu").is_err());
    }

    #[test]
    fn issue_ref_to_repo_ref_rejects_tracker_native_refs() {
        assert_eq!(
            issue_ref_to_repo_ref("github:Section9Labs/rupu/issues/42").unwrap(),
            "github:Section9Labs/rupu"
        );
        assert!(issue_ref_to_repo_ref("linear:team-123/issues/42").is_err());
        assert!(issue_ref_to_repo_ref("jira:acme.atlassian.net/ENG/issues/42").is_err());
    }

    #[test]
    fn canonical_autoflow_issue_ref_accepts_tracker_native_refs() {
        assert_eq!(
            canonical_autoflow_issue_ref("linear:team-123/issues/42").unwrap(),
            "linear:team-123/issues/42"
        );
        assert_eq!(
            canonical_autoflow_issue_ref("jira:acme.atlassian.net/ENG/issues/42").unwrap(),
            "jira:acme.atlassian.net/ENG/issues/42"
        );
        assert_eq!(
            canonical_autoflow_issue_ref("github:Section9Labs/rupu/issues/42").unwrap(),
            "github:Section9Labs/rupu/issues/42"
        );
    }

    #[tokio::test]
    async fn tick_discovers_and_executes_tracker_native_jira_source() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        let server = MockServer::start_async().await;
        init_git_repo(&project);

        server.mock(|when, then| {
            when.method(POST)
                .path("/rest/api/3/search/jql")
                .header_exists("authorization")
                .body_contains(
                    "\"project = \\\"ENG\\\" AND statusCategory != Done ORDER BY updated DESC\"",
                );
            then.status(200).json_body(serde_json::json!({
                "issues": [jira_issue_response("ENG-42", "indeterminate", &["bug"])],
                "nextPageToken": null
            }));
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  source: jira:ENG
  priority: 100
  selector:
    states: ["open"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.ref }} tracker={{ issue.tracker }} state={{ issue.state_name }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/config.toml"),
            format!(
                r#"[autoflow]
enabled = true
permission_mode = "bypass"
strict_templates = true

[scm.jira]
base_url = "{}"
"#,
                server.base_url()
            ),
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Jira,
                AuthMode::ApiKey,
                StoredCredential::api_key("matt@example.com:api-token"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store
            .load("jira:127.0.0.1/ENG/issues/42")
            .unwrap()
            .unwrap();
        assert_eq!(claim.repo_ref, "github:Section9Labs/rupu");
        assert_eq!(claim.source_ref.as_deref(), Some("jira:ENG"));
        assert_eq!(claim.issue_display_ref.as_deref(), Some("ENG-42"));
        assert_eq!(
            claim.issue_url.as_deref(),
            Some(format!("{}/browse/ENG-42", server.base_url()).as_str())
        );
        assert_eq!(claim.issue_tracker.as_deref(), Some("jira"));
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert!(claim.last_run_id.is_some());

        let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
        let recent = history_store.list_recent(5).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].mode, AutoflowCycleMode::Tick);
        assert_eq!(recent[0].ran_cycles, 1);
        assert!(recent[0]
            .events
            .iter()
            .any(|event| event.kind == AutoflowCycleEventKind::ClaimAcquired
                && event.issue_ref.as_deref() == Some("jira:127.0.0.1/ENG/issues/42")));
        assert!(recent[0]
            .events
            .iter()
            .any(|event| event.kind == AutoflowCycleEventKind::RunLaunched
                && event.issue_ref.as_deref() == Some("jira:127.0.0.1/ENG/issues/42")));
    }

    #[tokio::test]
    async fn manual_run_resolves_tracker_native_issue_from_visible_source() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        let server = MockServer::start_async().await;
        let original_cwd = std::env::current_dir().unwrap();
        init_git_repo(&project);

        server.mock(|when, then| {
            when.method(GET)
                .path("/rest/api/3/issue/ENG-42")
                .header_exists("authorization");
            then.status(200)
                .json_body(jira_issue_response("ENG-42", "indeterminate", &["bug"]));
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  source: jira:ENG
  priority: 100
  selector:
    states: ["open"]
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.ref }} source={{ issue.tracker }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/config.toml"),
            format!(
                r#"[autoflow]
enabled = true
repo = "github:Section9Labs/rupu"
permission_mode = "bypass"
strict_templates = true

[scm.jira]
base_url = "{}"
"#,
                server.base_url()
            ),
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Jira,
                AuthMode::ApiKey,
                StoredCredential::api_key("matt@example.com:api-token"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);
        std::env::set_current_dir(&project).unwrap();

        run(
            "issue-supervisor-dispatch",
            "jira:127.0.0.1/ENG/issues/42",
            None,
            None,
            resolver,
        )
        .await
        .unwrap();

        std::env::set_current_dir(original_cwd).unwrap();
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store
            .load("jira:127.0.0.1/ENG/issues/42")
            .unwrap()
            .unwrap();
        assert_eq!(claim.repo_ref, "github:Section9Labs/rupu");
        assert_eq!(claim.source_ref.as_deref(), Some("jira:ENG"));
        assert_eq!(claim.issue_display_ref.as_deref(), Some("ENG-42"));
        assert_eq!(claim.status, ClaimStatus::Complete);
    }

    #[test]
    fn github_issue_polled_event_maps_to_issue_ref() {
        let repo = rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Github,
            owner: "Section9Labs".into(),
            repo: "rupu".into(),
        };
        let event = rupu_scm::PolledEvent {
            id: "github.issue.labeled".into(),
            delivery: "evt_1".into(),
            source: repo.into(),
            subject: None,
            payload: serde_json::json!({
                "payload": {
                    "action": "labeled",
                    "issue": { "number": 123 }
                }
            }),
        };
        assert_eq!(
            autoflow_wake::extract_issue_ref_from_polled_event(&event).as_deref(),
            Some("github:Section9Labs/rupu/issues/123")
        );
    }

    #[test]
    fn github_pr_polled_event_does_not_fake_issue_ref() {
        let repo = rupu_scm::RepoRef {
            platform: rupu_scm::Platform::Github,
            owner: "Section9Labs".into(),
            repo: "rupu".into(),
        };
        let event = rupu_scm::PolledEvent {
            id: "github.pr.closed".into(),
            delivery: "evt_2".into(),
            source: repo.into(),
            subject: None,
            payload: serde_json::json!({
                "payload": {
                    "action": "closed",
                    "pull_request": { "number": 77 }
                }
            }),
        };
        assert!(autoflow_wake::extract_issue_ref_from_polled_event(&event).is_none());
    }

    #[test]
    fn matching_wake_event_makes_await_external_claim_due() {
        let resolved = ResolvedAutoflowWorkflow {
            scope: "project".into(),
            name: "issue-supervisor-dispatch".into(),
            workflow: Workflow::parse(
                r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  wake_on:
    - github.issue.labeled
  reconcile_every: "1d"
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#,
            )
            .unwrap(),
            workflow_path: PathBuf::from(
                "/tmp/repo/.rupu/workflows/issue-supervisor-dispatch.yaml",
            ),
            project_root: None,
            repo_ref: "github:Section9Labs/rupu".into(),
            preferred_checkout: PathBuf::from("/tmp/repo"),
            cfg: Config::default(),
        };
        let claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "issue-supervisor-dispatch".into(),
            status: ClaimStatus::AwaitExternal,
            worktree_path: None,
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };

        assert!(should_run_claim(
            &claim,
            &resolved,
            &store,
            chrono::Utc::now(),
            &BTreeSet::from(["github.issue.labeled".to_string()]),
        )
        .unwrap());
        assert!(!should_run_claim(
            &claim,
            &resolved,
            &store,
            chrono::Utc::now(),
            &BTreeSet::new()
        )
        .unwrap());
    }

    #[test]
    fn selector_matches_honors_labels_all_any_and_none() {
        let workflow = Workflow::parse(
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  selector:
    labels_all: ["autoflow"]
    labels_any: ["bug", "urgent"]
    labels_none: ["blocked"]
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#,
        )
        .unwrap();
        let autoflow = workflow.autoflow.as_ref().unwrap();
        let issue = |labels: &[&str]| Issue {
            r: IssueRef {
                tracker: IssueTracker::Github,
                project: "Section9Labs/rupu".into(),
                number: 42,
            },
            title: "x".into(),
            body: String::new(),
            state: IssueState::Open,
            labels: labels.iter().map(|label| (*label).to_string()).collect(),
            label_colors: BTreeMap::new(),
            author: "matt".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        assert!(selector_matches(autoflow, &issue(&["autoflow", "bug"])));
        assert!(selector_matches(autoflow, &issue(&["autoflow", "urgent"])));
        assert!(!selector_matches(autoflow, &issue(&["bug"])));
        assert!(!selector_matches(autoflow, &issue(&["autoflow"])));
        assert!(!selector_matches(
            autoflow,
            &issue(&["autoflow", "bug", "blocked"])
        ));
    }

    #[test]
    fn terminal_claim_cleanup_respects_grace_period() {
        let claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "issue-supervisor-dispatch".into(),
            status: ClaimStatus::Complete,
            worktree_path: None,
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339(),
        };
        assert!(claim_cleanup_due(&claim, chrono::Utc::now(), chrono::Duration::days(1)).unwrap());
        assert!(!claim_cleanup_due(&claim, chrono::Utc::now(), chrono::Duration::days(7)).unwrap());
    }

    #[test]
    fn resolve_branch_name_renders_inputs_and_issue_fields() {
        let issue_payload = serde_json::json!({
            "number": 42
        });
        let branch = resolve_branch_name(
            Some("rupu/issue-{{ issue.number }}-{{ inputs.phase }}"),
            &issue_payload,
            "github:Section9Labs/rupu/issues/42",
            &BTreeMap::from([("phase".into(), "phase-1".into())]),
        )
        .unwrap();
        assert_eq!(branch, "rupu/issue-42-phase-1");
    }

    #[test]
    fn autoflow_permission_mode_defaults_to_bypass() {
        assert_eq!(
            resolve_autoflow_permission_mode(None, None).unwrap(),
            "bypass"
        );
    }

    #[test]
    fn autoflow_permission_mode_accepts_readonly() {
        assert_eq!(
            resolve_autoflow_permission_mode(Some("readonly"), Some("bypass")).unwrap(),
            "readonly"
        );
    }

    #[test]
    fn autoflow_permission_mode_rejects_ask_override() {
        let err = resolve_autoflow_permission_mode(Some("ask"), Some("bypass")).unwrap_err();
        assert!(err
            .to_string()
            .contains("autoflow does not support `ask` permission mode"));
    }

    #[test]
    fn autoflow_permission_mode_rejects_ask_config() {
        let err = resolve_autoflow_permission_mode(None, Some("ask")).unwrap_err();
        assert!(err
            .to_string()
            .contains("autoflow does not support `ask` permission mode"));
    }

    #[test]
    fn autoflow_permission_mode_rejects_unknown_override() {
        let err = resolve_autoflow_permission_mode(Some("admin"), Some("bypass")).unwrap_err();
        assert!(err
            .to_string()
            .contains("invalid autoflow permission mode `admin`"));
    }

    #[test]
    fn autoflow_permission_mode_rejects_unknown_config() {
        let err = resolve_autoflow_permission_mode(None, Some("admin")).unwrap_err();
        assert!(err
            .to_string()
            .contains("invalid autoflow permission mode `admin`"));
    }

    #[test]
    fn manual_run_rejects_owned_non_terminal_claim() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        store
            .save(&AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "controller".into(),
                status: ClaimStatus::AwaitExternal,
                worktree_path: None,
                branch: None,
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let err = ensure_manual_run_can_take_claim(&store, issue_ref).unwrap_err();
        assert!(err
            .to_string()
            .contains("already has an owned autoflow claim"));
    }

    #[test]
    fn manual_run_allows_expired_claim_takeover() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        store
            .save(&AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "controller".into(),
                status: ClaimStatus::AwaitExternal,
                worktree_path: None,
                branch: None,
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some("2000-01-01T00:00:00Z".into()),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        ensure_manual_run_can_take_claim(&store, issue_ref).unwrap();
    }

    #[test]
    fn manual_run_rejects_blocked_claim_until_release() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        store
            .save(&AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "controller".into(),
                status: ClaimStatus::Blocked,
                worktree_path: None,
                branch: None,
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some("2000-01-01T00:00:00Z".into()),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let err = ensure_manual_run_can_take_claim(&store, issue_ref).unwrap_err();
        assert!(err.to_string().contains("blocked autoflow claim"));
        assert!(err.to_string().contains("rupu autoflow release"));
    }

    #[test]
    fn idle_claim_yields_to_higher_priority_winner() {
        let winner = IssueMatch {
            resolved: ResolvedAutoflowWorkflow {
                scope: "project".into(),
                name: "phase-delivery-cycle".into(),
                workflow: Workflow::parse(
                    r#"name: phase-delivery-cycle
autoflow:
  enabled: true
  priority: 200
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#,
                )
                .unwrap(),
                workflow_path: PathBuf::from("/tmp/repo/.rupu/workflows/phase-delivery-cycle.yaml"),
                project_root: None,
                repo_ref: "github:Section9Labs/rupu".into(),
                preferred_checkout: PathBuf::from("/tmp/repo"),
                cfg: Config::default(),
            },
            issue: Issue {
                r: IssueRef {
                    tracker: IssueTracker::Github,
                    project: "Section9Labs/rupu".into(),
                    number: 42,
                },
                title: "x".into(),
                body: String::new(),
                state: IssueState::Open,
                labels: vec!["bug".into()],
                label_colors: BTreeMap::new(),
                author: "matt".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
            issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
        };
        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "issue-supervisor-dispatch".into(),
            status: ClaimStatus::AwaitExternal,
            worktree_path: None,
            branch: None,
            last_run_id: None,
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        assert!(claim_should_yield_to_winner(&claim, Some(&winner), false));
        assert!(!claim_should_yield_to_winner(&claim, Some(&winner), true));
        claim.status = ClaimStatus::AwaitHuman;
        assert!(!claim_should_yield_to_winner(&claim, Some(&winner), false));
    }

    #[test]
    fn winners_prefer_priority_then_workflow_name() {
        let workflow = |name: &str, priority: i32| {
            Workflow::parse(&format!(
                r#"name: {name}
autoflow:
  enabled: true
  priority: {priority}
steps:
  - id: a
    agent: echo
    actions: []
    prompt: hi
"#
            ))
            .unwrap()
        };
        let resolved = |name: &str, priority: i32| ResolvedAutoflowWorkflow {
            scope: "project".into(),
            name: name.into(),
            workflow: workflow(name, priority),
            workflow_path: PathBuf::from(format!("/tmp/repo/.rupu/workflows/{name}.yaml")),
            project_root: None,
            repo_ref: "github:Section9Labs/rupu".into(),
            preferred_checkout: PathBuf::from("/tmp/repo"),
            cfg: Config::default(),
        };
        let issue = Issue {
            r: IssueRef {
                tracker: IssueTracker::Github,
                project: "Section9Labs/rupu".into(),
                number: 42,
            },
            title: "x".into(),
            body: String::new(),
            state: IssueState::Open,
            labels: vec!["autoflow".into()],
            label_colors: BTreeMap::new(),
            author: "matt".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let winners = choose_winning_matches(vec![
            IssueMatch {
                resolved: resolved("beta", 50),
                issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
                issue: issue.clone(),
            },
            IssueMatch {
                resolved: resolved("alpha", 50),
                issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
                issue: issue.clone(),
            },
            IssueMatch {
                resolved: resolved("gamma", 100),
                issue_ref_text: "github:Section9Labs/rupu/issues/42".into(),
                issue,
            },
        ]);
        assert_eq!(
            winners
                .get("github:Section9Labs/rupu/issues/42")
                .unwrap()
                .resolved
                .workflow
                .name,
            "gamma"
        );
    }

    #[test]
    fn terminal_outcome_persists_dispatch_for_next_tick() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
  priority: 100
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        let run = RunRecord {
            id: "run_dispatch".into(),
            workflow_name: "controller".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: project.clone(),
            transcript_dir: global.join("transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
        };
        store.create(run, "name: controller\nsteps: []\n").unwrap();
        store
            .append_step_result(
                "run_dispatch",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"{"status":"continue","summary":"phase 1 is ready","pr_url":"https://github.com/Section9Labs/rupu/pull/42","artifacts":{"review_packet":"docs/reviews/issue-42.json"},"dispatch":{"workflow":"phase-delivery-cycle","target":"github:Section9Labs/rupu/issues/42","inputs":{"phase":"phase-1"}}}"#.into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: ClaimStatus::Running,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_dispatch".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_terminal_run_to_claim(&global, &resolved, "run_dispatch", &mut claim).unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        assert_eq!(claim.last_summary.as_deref(), Some("phase 1 is ready"));
        assert_eq!(
            claim.pr_url.as_deref(),
            Some("https://github.com/Section9Labs/rupu/pull/42")
        );
        assert_eq!(
            claim
                .artifacts
                .as_ref()
                .and_then(|value| value.get("review_packet")),
            Some(&serde_json::json!("docs/reviews/issue-42.json"))
        );
        let dispatch = claim.pending_dispatch.expect("dispatch");
        assert_eq!(dispatch.workflow, "phase-delivery-cycle");
        assert_eq!(
            dispatch.inputs.get("phase").map(String::as_str),
            Some("phase-1")
        );
    }

    #[test]
    fn terminal_outcome_accepts_schema_named_wrapper() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
  priority: 100
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        let run = RunRecord {
            id: "run_wrapped".into(),
            workflow_name: "controller".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: project.clone(),
            transcript_dir: global.join("transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
        };
        store.create(run, "name: controller\nsteps: []\n").unwrap();
        store
            .append_step_result(
                "run_wrapped",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"{"autoflow_outcome_v1":{"status":"continue","summary":"wrapped output works","dispatch":{"workflow":"phase-delivery-cycle","target":"github:Section9Labs/rupu/issues/42","inputs":{"phase":"phase-1"}}}}"#.into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: ClaimStatus::Running,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_wrapped".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_terminal_run_to_claim(&global, &resolved, "run_wrapped", &mut claim).unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        assert_eq!(claim.last_summary.as_deref(), Some("wrapped output works"));
        let dispatch = claim.pending_dispatch.expect("dispatch");
        assert_eq!(dispatch.workflow, "phase-delivery-cycle");
    }

    #[test]
    fn terminal_outcome_accepts_fenced_json_with_prose() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
  priority: 100
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        let run = RunRecord {
            id: "run_fenced".into(),
            workflow_name: "controller".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: project.clone(),
            transcript_dir: global.join("transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
        };
        store.create(run, "name: controller\nsteps: []\n").unwrap();
        store
            .append_step_result(
                "run_fenced",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"I checked the issue state and the planning files.

```json
{
  "autoflow_outcome_v1": {
    "status": "continue",
    "summary": "fenced output works",
    "dispatch": {
      "workflow": "issue-to-spec-and-plan",
      "target": "github:Section9Labs/rupu/issues/42",
      "inputs": {}
    }
  }
}
```"#
                        .into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: ClaimStatus::Running,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_fenced".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_terminal_run_to_claim(&global, &resolved, "run_fenced", &mut claim).unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        assert_eq!(claim.last_summary.as_deref(), Some("fenced output works"));
        let dispatch = claim.pending_dispatch.expect("dispatch");
        assert_eq!(dispatch.workflow, "issue-to-spec-and-plan");
    }

    #[test]
    fn terminal_outcome_accepts_dispatch_string_shorthand() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
  priority: 100
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        let run = RunRecord {
            id: "run_shorthand".into(),
            workflow_name: "controller".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: project.clone(),
            transcript_dir: global.join("transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
        };
        store.create(run, "name: controller\nsteps: []\n").unwrap();
        store
            .append_step_result(
                "run_shorthand",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"{"dispatch":"issue-to-spec-and-plan","reason":"Create the spec and plan first."}"#.into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: ClaimStatus::Running,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_shorthand".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_terminal_run_to_claim(&global, &resolved, "run_shorthand", &mut claim).unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        assert_eq!(
            claim.last_summary.as_deref(),
            Some("Create the spec and plan first.")
        );
        let dispatch = claim.pending_dispatch.expect("dispatch");
        assert_eq!(dispatch.workflow, "issue-to-spec-and-plan");
        assert_eq!(dispatch.target, "github:Section9Labs/rupu/issues/42");
        assert!(dispatch.inputs.is_empty());
    }

    #[test]
    fn terminal_outcome_accepts_decision_and_workflow_shorthand() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
  priority: 100
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        let run = RunRecord {
            id: "run_decision".into(),
            workflow_name: "controller".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".into(),
            workspace_path: project.clone(),
            transcript_dir: global.join("transcripts"),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
        };
        store.create(run, "name: controller\nsteps: []\n").unwrap();
        store
            .append_step_result(
                "run_decision",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"{"decision":"dispatch","workflow":"issue-to-spec-and-plan","reason":"Need the spec and plan first.","issue":{"number":42}}"#.into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: ClaimStatus::Running,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_decision".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        apply_terminal_run_to_claim(&global, &resolved, "run_decision", &mut claim).unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        assert_eq!(
            claim.last_summary.as_deref(),
            Some("Need the spec and plan first.")
        );
        let dispatch = claim.pending_dispatch.expect("dispatch");
        assert_eq!(dispatch.workflow, "issue-to-spec-and-plan");
        assert_eq!(dispatch.target, "github:Section9Labs/rupu/issues/42");
    }

    #[test]
    fn reconcile_claim_blocks_rejected_approval_run() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);
        write_autoflow_project(
            &project,
            "http://localhost.invalid",
            "controller",
            r#"name: controller
autoflow:
  enabled: true
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: hi
"#,
        );

        let resolved = resolve_autoflow_from_path(
            &global,
            project.join(".rupu/workflows/controller.yaml"),
            "project".into(),
            Some(project.clone()),
            "github:Section9Labs/rupu".into(),
            project.clone(),
            resolve_config(&global, Some(&project)).unwrap(),
        )
        .unwrap();
        let store = RunStore::new(global.join("runs"));
        std::fs::create_dir_all(global.join("runs")).unwrap();
        store
            .create(
                RunRecord {
                    id: "run_rejected".into(),
                    workflow_name: "controller".into(),
                    status: RunStatus::Rejected,
                    inputs: BTreeMap::new(),
                    event: None,
                    workspace_id: "ws_1".into(),
                    workspace_path: project.clone(),
                    transcript_dir: global.join("transcripts"),
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    error_message: Some("rejected: needs changes".into()),
                    awaiting_step_id: None,
                    approval_prompt: None,
                    awaiting_since: None,
                    expires_at: None,
                    issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
                    issue: None,
                    parent_run_id: None,
                    backend_id: None,
                    worker_id: None,
                    artifact_manifest_path: None,
                    source_wake_id: None,
                    active_step_id: None,
                    active_step_kind: None,
                    active_step_agent: None,
                    active_step_transcript_path: None,
                },
                "name: controller\nsteps: []\n",
            )
            .unwrap();

        let mut claim = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: None,
            issue_display_ref: None,
            issue_title: None,
            issue_url: None,
            issue_state_name: None,
            issue_tracker: None,
            workflow: "controller".into(),
            status: ClaimStatus::AwaitHuman,
            worktree_path: None,
            branch: None,
            last_run_id: Some("run_rejected".into()),
            last_error: None,
            last_summary: None,
            pr_url: None,
            artifacts: None,
            artifact_manifest_path: None,
            next_retry_at: None,
            claim_owner: None,
            lease_expires_at: None,
            pending_dispatch: None,
            contenders: vec![],
            updated_at: chrono::Utc::now().to_rfc3339(),
        };

        reconcile_claim_from_last_run(&global, &resolved, &mut claim).unwrap();

        assert_eq!(claim.status, ClaimStatus::Blocked);
        assert_eq!(claim.last_error.as_deref(), Some("rejected: needs changes"));
    }

    #[tokio::test]
    async fn tick_preserves_await_human_claims_while_run_is_awaiting_approval() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issues_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let run_store = RunStore::new(global.join("runs"));
        run_store
            .create(
                RunRecord {
                    id: "run_waiting".into(),
                    workflow_name: "issue-supervisor-dispatch".into(),
                    status: RunStatus::AwaitingApproval,
                    inputs: BTreeMap::new(),
                    event: None,
                    workspace_id: "ws_1".into(),
                    workspace_path: project.clone(),
                    transcript_dir: global.join("transcripts"),
                    started_at: chrono::Utc::now(),
                    finished_at: None,
                    error_message: None,
                    awaiting_step_id: Some("approve".into()),
                    approval_prompt: Some("approve?".into()),
                    awaiting_since: Some(chrono::Utc::now()),
                    expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                    issue_ref: Some("github:Section9Labs/rupu/issues/123".into()),
                    issue: None,
                    parent_run_id: None,
                    backend_id: None,
                    worker_id: None,
                    artifact_manifest_path: None,
                    source_wake_id: None,
                    active_step_id: None,
                    active_step_kind: None,
                    active_step_agent: None,
                    active_step_transcript_path: None,
                },
                "name: issue-supervisor-dispatch\nsteps: []\n",
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::AwaitHuman,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: Some("run_waiting".into()),
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        tick_with_resolver(resolver).await.unwrap();
        std::env::remove_var("RUPU_HOME");

        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::AwaitHuman);
        assert_eq!(claim.last_run_id.as_deref(), Some("run_waiting"));

        let run = run_store.load("run_waiting").unwrap();
        assert_eq!(run.status, RunStatus::AwaitingApproval);
    }

    #[tokio::test]
    async fn tick_reconciles_await_human_claim_once_run_completes() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issues_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let run_store = RunStore::new(global.join("runs"));
        run_store
            .create(
                RunRecord {
                    id: "run_done".into(),
                    workflow_name: "issue-supervisor-dispatch".into(),
                    status: RunStatus::Completed,
                    inputs: BTreeMap::new(),
                    event: None,
                    workspace_id: "ws_1".into(),
                    workspace_path: project.clone(),
                    transcript_dir: global.join("transcripts"),
                    started_at: chrono::Utc::now(),
                    finished_at: Some(chrono::Utc::now()),
                    error_message: None,
                    awaiting_step_id: None,
                    approval_prompt: None,
                    awaiting_since: None,
                    expires_at: None,
                    issue_ref: Some("github:Section9Labs/rupu/issues/123".into()),
                    issue: None,
                    parent_run_id: None,
                    backend_id: None,
                    worker_id: None,
                    artifact_manifest_path: None,
                    source_wake_id: None,
                    active_step_id: None,
                    active_step_kind: None,
                    active_step_agent: None,
                    active_step_transcript_path: None,
                },
                "name: issue-supervisor-dispatch\nsteps: []\n",
            )
            .unwrap();
        run_store
            .append_step_result(
                "run_done",
                &StepResultRecord {
                    step_id: "decide".into(),
                    run_id: "step_1".into(),
                    transcript_path: global.join("transcripts/step.jsonl"),
                    output: r#"{"status":"complete","summary":"approved and done"}"#.into(),
                    success: true,
                    skipped: false,
                    rendered_prompt: "hi".into(),
                    kind: StepKind::Linear,
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                },
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::AwaitHuman,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: Some("run_done".into()),
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        tick_with_resolver(resolver).await.unwrap();
        std::env::remove_var("RUPU_HOME");

        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert_eq!(claim.last_summary.as_deref(), Some("approved and done"));
        assert_eq!(claim.last_run_id.as_deref(), Some("run_done"));
    }

    #[tokio::test]
    async fn tick_executes_pending_dispatch_on_next_pass() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issues_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_body.clone());
        });
        let issue_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issue_get_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues/123");
            then.status(200)
                .header("content-type", "application/json")
                .body(issue_body.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "controller {{ issue.number }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/workflows/phase-delivery-cycle.yaml"),
            r#"name: phase-delivery-cycle
autoflow:
  enabled: true
  priority: 50
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}-{{ inputs.phase }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: implement
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: implement
    agent: echo
    actions: []
    prompt: "phase={{ inputs.phase }} issue={{ issue.number }}"
"#,
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/123";
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::Claimed,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: Some("run_controller".into()),
                last_error: None,
                last_summary: Some("phase 1 is ready".into()),
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: Some(PendingDispatch {
                    workflow: "phase-delivery-cycle".into(),
                    target: issue_ref.into(),
                    inputs: BTreeMap::from([("phase".into(), "phase-1".into())]),
                }),
                contenders: vec![],
                updated_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);

        tick_with_resolver(resolver.clone()).await.unwrap();

        let skipped_claim = claim_store.load(issue_ref).unwrap().unwrap();
        assert_eq!(skipped_claim.status, ClaimStatus::Claimed);
        assert!(skipped_claim.pending_dispatch.is_some());
        assert_eq!(skipped_claim.last_run_id.as_deref(), Some("run_controller"));

        let runs_dir = global.join("runs");
        assert!(!runs_dir.exists() || std::fs::read_dir(&runs_dir).unwrap().next().is_none());

        let mut runnable_claim = skipped_claim;
        runnable_claim.updated_at = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        claim_store.save(&runnable_claim).unwrap();

        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);
        tick_with_resolver(resolver).await.unwrap();
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let final_claim = claim_store.load(issue_ref).unwrap().unwrap();
        assert_eq!(final_claim.status, ClaimStatus::Complete);
        assert!(final_claim.pending_dispatch.is_none());
        assert_eq!(final_claim.last_summary.as_deref(), Some("done"));

        let run_id = final_claim.last_run_id.as_deref().expect("last run id");
        let run_store = RunStore::new(global.join("runs"));
        let run = run_store.load(run_id).unwrap();
        assert_eq!(run.workflow_name, "phase-delivery-cycle");
        assert_eq!(run.inputs.get("phase").map(String::as_str), Some("phase-1"));
    }

    #[tokio::test]
    async fn tick_executes_pending_dispatch_to_non_autoflow_workflow() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issue_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issue_get_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues/123");
            then.status(200)
                .header("content-type", "application/json")
                .body(issue_body.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "controller {{ issue.number }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/workflows/issue-to-spec-and-plan.yaml"),
            r#"name: issue-to-spec-and-plan
steps:
  - id: understand
    agent: echo
    actions: []
    prompt: "spec-plan {{ issue.number }}"
"#,
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/123";
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::Claimed,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: Some("run_controller".into()),
                last_error: None,
                last_summary: Some("Need the spec and plan first.".into()),
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: Some(PendingDispatch {
                    workflow: "issue-to-spec-and-plan".into(),
                    target: issue_ref.into(),
                    inputs: BTreeMap::new(),
                }),
                contenders: vec![],
                updated_at: (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);
        tick_with_resolver(resolver).await.unwrap();
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let final_claim = claim_store.load(issue_ref).unwrap().unwrap();
        assert_eq!(final_claim.status, ClaimStatus::Claimed);
        assert!(final_claim.pending_dispatch.is_none());
        let run_id = final_claim.last_run_id.as_deref().expect("last run id");
        let run_store = RunStore::new(global.join("runs"));
        let run = run_store.load(run_id).unwrap();
        assert_eq!(run.workflow_name, "issue-to-spec-and-plan");
    }

    #[tokio::test]
    async fn tick_discovers_tracked_repo_and_runs_autoflow_cycle() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let body = std::fs::read_to_string(
            "/Users/matt/Code/Oracle/rupu/.worktrees/feat-autoflow-phase-4/crates/rupu-scm/tests/fixtures/github/issues_list_happy.json",
        )
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert!(claim.last_run_id.is_some());
        assert!(claim
            .worktree_path
            .as_deref()
            .unwrap()
            .contains("issue-123"));
    }

    #[tokio::test]
    async fn serve_runs_one_cycle_persists_worker_and_releases_lock() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let body = github_fixture("issues_list_happy.json").replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        let report = crate::cmd::autoflow_runtime::serve_with_resolver(
            resolver.clone(),
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: Some("github:Section9Labs/rupu".into()),
                worker_name: Some("team-mini-01".into()),
                idle_sleep: std::time::Duration::from_millis(1),
                max_cycles: Some(1),
                shared_printer: None,
                attach_workflow_ui: true,
            },
        )
        .await
        .unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        assert_eq!(report.cycles, 1);
        assert_eq!(report.total.ran_cycles, 1);

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);

        let worker_ctx = crate::cmd::workflow::default_execution_worker_context(
            rupu_workspace::WorkerKind::AutoflowServe,
            Some("team-mini-01"),
        );
        let lock_path = paths::autoflow_workers_dir(&global)
            .join(format!("{}.serve.lock", worker_ctx.worker_id));
        assert!(!lock_path.exists(), "serve lock should be removed on exit");

        let worker_store = rupu_workspace::WorkerStore {
            root: paths::autoflow_workers_dir(&global),
        };
        let worker = worker_store
            .load(&worker_ctx.worker_id)
            .unwrap()
            .expect("serve worker record should persist");
        assert_eq!(worker.kind, rupu_workspace::WorkerKind::AutoflowServe);
        assert_eq!(worker.name, "team-mini-01");
        assert!(worker
            .capabilities
            .backends
            .iter()
            .any(|value| value == "local_worktree"));
        assert!(worker
            .capabilities
            .permission_modes
            .iter()
            .any(|value| value == "bypass"));
        assert!(worker
            .capabilities
            .permission_modes
            .iter()
            .any(|value| value == "readonly"));

        let history_store = AutoflowHistoryStore::new(paths::autoflow_history_dir(&global));
        let recent = history_store.list_recent(5).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].mode, AutoflowCycleMode::Serve);
        assert_eq!(
            recent[0].worker_id.as_deref(),
            Some(worker_ctx.worker_id.as_str())
        );
        assert_eq!(recent[0].worker_name.as_deref(), Some("team-mini-01"));
        assert_eq!(
            recent[0].repo_filter.as_deref(),
            Some("github:Section9Labs/rupu")
        );
        assert!(recent[0]
            .events
            .iter()
            .any(|event| event.kind == AutoflowCycleEventKind::RunLaunched
                && event.issue_ref.as_deref() == Some("github:Section9Labs/rupu/issues/123")));

        std::env::set_var("RUPU_HOME", &global);
        let restart = crate::cmd::autoflow_runtime::serve_with_resolver(
            resolver,
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: Some("github:Section9Labs/rupu".into()),
                worker_name: Some("team-mini-01".into()),
                idle_sleep: std::time::Duration::from_millis(1),
                max_cycles: Some(1),
                shared_printer: None,
                attach_workflow_ui: true,
            },
        )
        .await
        .unwrap();
        std::env::remove_var("RUPU_HOME");

        assert_eq!(restart.cycles, 1);
        assert!(
            !lock_path.exists(),
            "serve lock should stay released after restart"
        );
    }

    #[tokio::test]
    async fn serve_hook_receives_per_cycle_reports() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let body = github_fixture("issues_list_happy.json").replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_capture = Arc::clone(&seen);
        let report = crate::cmd::autoflow_runtime::serve_with_resolver_and_hook(
            resolver,
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: Some("github:Section9Labs/rupu".into()),
                worker_name: Some("team-mini-01".into()),
                idle_sleep: std::time::Duration::from_millis(1),
                max_cycles: Some(1),
                shared_printer: None,
                attach_workflow_ui: true,
            },
            move |report, tick, _cycle| {
                let seen_capture = Arc::clone(&seen_capture);
                seen_capture.lock().unwrap().push((
                    report.cycles,
                    tick.ran_cycles,
                    tick.failed_cycles,
                ));
                Ok(())
            },
        )
        .await
        .unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        assert_eq!(report.cycles, 1);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.as_slice(), &[(1, 1, 0)]);
    }

    #[tokio::test]
    async fn serve_respects_repo_filter() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project_a = tmp.path().join("repo-a");
        let project_b = tmp.path().join("repo-b");
        init_git_repo(&project_a);
        init_git_repo(&project_b);

        let server = MockServer::start();
        let issues_rupu =
            github_fixture("issues_list_happy.json").replace("section9labs", "Section9Labs");
        let issues_other = issues_rupu.replace("rupu", "other-repo");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_rupu);
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/repos/Section9Labs/other-repo/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_other);
        });

        let workflow_yaml = r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#;
        write_autoflow_project(
            &project_a,
            &server.base_url(),
            "issue-supervisor-dispatch",
            workflow_yaml,
        );
        write_autoflow_project(
            &project_b,
            &server.base_url(),
            "issue-supervisor-dispatch",
            workflow_yaml,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project_a,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();
        repo_store
            .upsert(
                "github:Section9Labs/other-repo",
                &project_b,
                Some("https://github.com/Section9Labs/other-repo.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        let report = crate::cmd::autoflow_runtime::serve_with_resolver(
            resolver,
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: Some("github:Section9Labs/rupu".into()),
                worker_name: Some("repo-filter".into()),
                idle_sleep: std::time::Duration::from_millis(1),
                max_cycles: Some(1),
                shared_printer: None,
                attach_workflow_ui: true,
            },
        )
        .await
        .unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        assert_eq!(report.cycles, 1);
        assert_eq!(report.total.ran_cycles, 1);

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        assert!(claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .is_some());
        assert!(claim_store
            .load("github:Section9Labs/other-repo/issues/123")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn serve_enqueues_follow_up_dispatch_wake() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let body = github_fixture("issues_list_happy.json").replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"
[
  {
    "AssistantText": {
      "text": "{\"status\":\"continue\",\"summary\":\"phase queued\",\"dispatch\":{\"workflow\":\"phase-delivery-cycle\",\"target\":\"github:Section9Labs/rupu/issues/123\",\"inputs\":{\"phase\":\"phase-1\"}}}",
      "stop": "end_turn"
    }
  }
]
"#,
        );

        let report = crate::cmd::autoflow_runtime::serve_with_resolver(
            resolver,
            crate::cmd::autoflow_runtime::ServeOptions {
                repo_filter: Some("github:Section9Labs/rupu".into()),
                worker_name: Some("dispatcher".into()),
                idle_sleep: std::time::Duration::from_millis(1),
                max_cycles: Some(1),
                shared_printer: None,
                attach_workflow_ui: true,
            },
        )
        .await
        .unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        assert_eq!(report.cycles, 1);
        assert_eq!(report.total.ran_cycles, 1);

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Claimed);
        assert_eq!(claim.last_summary.as_deref(), Some("phase queued"));
        let dispatch = claim
            .pending_dispatch
            .expect("pending dispatch should persist");
        assert_eq!(dispatch.workflow, "phase-delivery-cycle");
        assert_eq!(
            dispatch.inputs.get("phase").map(String::as_str),
            Some("phase-1")
        );

        let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
        let due = wake_store.list_due(chrono::Utc::now()).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].source, rupu_runtime::WakeSource::AutoflowDispatch);
        assert_eq!(due[0].repo_ref, "github:Section9Labs/rupu");
        assert_eq!(due[0].entity.kind, WakeEntityKind::Issue);
        assert_eq!(
            due[0].entity.ref_text,
            "github:Section9Labs/rupu/issues/123"
        );
        assert_eq!(due[0].event.id, "autoflow.dispatch.pending");
    }

    #[tokio::test]
    async fn tick_releases_idle_claim_for_higher_priority_winner() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issues_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_body.clone());
        });
        let issue_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issue_get_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues/123");
            then.status(200)
                .header("content-type", "application/json")
                .body(issue_body.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 50
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "controller {{ issue.number }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/workflows/phase-delivery-cycle.yaml"),
            r#"name: phase-delivery-cycle
autoflow:
  enabled: true
  priority: 200
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: implement
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: implement
    agent: echo
    actions: []
    prompt: "phase owner {{ issue.number }}"
"#,
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::AwaitExternal,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: Some("run_controller".into()),
                last_error: None,
                last_summary: Some("waiting for external change".into()),
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.workflow, "phase-delivery-cycle");
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert_eq!(claim.last_summary.as_deref(), Some("done"));
        assert_eq!(
            claim
                .contenders
                .iter()
                .find(|contender| contender.selected)
                .map(|contender| contender.workflow.as_str()),
            Some("phase-delivery-cycle")
        );

        let run_id = claim.last_run_id.as_deref().expect("last run id");
        let run_store = RunStore::new(global.join("runs"));
        let run = run_store.load(run_id).unwrap();
        assert_eq!(run.workflow_name, "phase-delivery-cycle");
    }

    #[tokio::test]
    async fn tick_uses_polled_wake_events_to_resume_await_external_claims() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issues_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_body);
        });
        let issue_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issue_get_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues/123");
            then.status(200)
                .header("content-type", "application/json")
                .body(issue_body.clone());
        });
        let events_body = serde_json::json!([
            {
                "id": "evt-123",
                "type": "IssuesEvent",
                "created_at": "2026-05-08T20:10:00Z",
                "payload": {
                    "action": "labeled",
                    "issue": { "number": 123 }
                }
            }
        ]);
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/events");
            then.status(200)
                .header("content-type", "application/json")
                .header("etag", "\"wake-1\"")
                .json_body(events_body.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  wake_on:
    - github.issue.labeled
  reconcile_every: "1d"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(project.join(".rupu/config.toml"))
            .unwrap()
            .write_all(b"\n[triggers]\npoll_sources = [\"github:Section9Labs/rupu\"]\n")
            .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::AwaitExternal,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let cursor_file = autoflow_cursor_path(
            &paths::autoflow_event_cursors_dir(&global),
            &rupu_scm::EventSourceRef::Repo {
                repo: rupu_scm::RepoRef {
                    platform: rupu_scm::Platform::Github,
                    owner: "Section9Labs".into(),
                    repo: "rupu".into(),
                },
            },
        );
        write_cursor(&cursor_file, "etag:|since:2026-05-08T19:00:00Z").unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert!(claim.last_run_id.is_some());
    }

    #[tokio::test]
    async fn tick_uses_webhook_wake_events_to_resume_await_external_claims() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let issues_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(issues_body);
        });
        let issue_body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issue_get_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues/123");
            then.status(200)
                .header("content-type", "application/json")
                .body(issue_body.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  wake_on:
    - github.issue.labeled
  reconcile_every: "1d"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::AwaitExternal,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let wake_store = WakeStore::new(paths::autoflow_wakes_dir(&global));
        wake_store
            .enqueue(
                autoflow_wake::wake_request_from_webhook(&rupu_webhook::WebhookEvent {
                    source: rupu_webhook::WebhookSource::Github,
                    event_id: "github.issue.labeled".into(),
                    delivery_id: Some("delivery-123".into()),
                    payload: serde_json::json!({
                        "issue": { "number": 123 },
                        "repository": {
                            "name": "rupu",
                            "owner": { "login": "Section9Labs" }
                        }
                    }),
                })
                .unwrap(),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert!(claim.last_run_id.is_some());
        assert!(wake_store.list_due(chrono::Utc::now()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn tick_cleans_complete_claims_after_cleanup_window() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        std::fs::create_dir_all(project.join(".rupu")).unwrap();
        std::fs::write(
            project.join(".rupu/config.toml"),
            "[autoflow]\ncleanup_after = \"1d\"\n",
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();

        let worktree = ensure_issue_worktree(
            &project,
            &paths::autoflow_worktrees_dir(&global),
            "github:Section9Labs/rupu",
            "github:Section9Labs/rupu/issues/42",
            "rupu/issue-42",
            Some("HEAD"),
        )
        .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: issue_ref.into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::Complete,
                worktree_path: Some(worktree.path.display().to_string()),
                branch: Some("rupu/issue-42".into()),
                last_run_id: Some("run_done".into()),
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: None,
                pending_dispatch: None,
                contenders: vec![],
                updated_at: (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339(),
            })
            .unwrap();

        std::env::set_var("RUPU_HOME", &global);
        tick_with_resolver(Arc::new(InMemoryResolver::new()))
            .await
            .unwrap();
        std::env::remove_var("RUPU_HOME");

        assert!(claim_store.load(issue_ref).unwrap().is_none());
        assert!(!worktree.path.exists());
    }

    #[tokio::test]
    async fn tick_reaps_stale_orphan_lock_and_runs_cycle() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let body = std::fs::read_to_string(format!(
            "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
        .replace("section9labs", "Section9Labs");
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let issue_ref = "github:Section9Labs/rupu/issues/123";
        let issue_dir = paths::autoflow_claims_dir(&global)
            .join(rupu_workspace::autoflow_claim_store::issue_key(issue_ref));
        std::fs::create_dir_all(&issue_dir).unwrap();
        std::fs::write(
            issue_dir.join(".lock"),
            toml::to_string(&rupu_workspace::ActiveLockRecord {
                owner: "stale-owner".into(),
                acquired_at: "2026-05-08T20:00:00Z".into(),
                lease_expires_at: Some("2000-01-01T00:00:00Z".into()),
            })
            .unwrap(),
        )
        .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let claim = claim_store.load(issue_ref).unwrap().unwrap();
        assert_eq!(claim.status, ClaimStatus::Complete);
        assert!(claim.last_run_id.is_some());
        assert!(!issue_dir.join(".lock").exists());
    }

    #[tokio::test]
    async fn tick_ignores_complete_claims_when_enforcing_max_active() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let mut issues: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        let base_issue = issues.as_array().unwrap()[0].clone();
        let mut issue_123 = base_issue.clone();
        issue_123["repository_url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu");
        issue_123["html_url"] =
            serde_json::json!("https://github.com/Section9Labs/rupu/issues/123");
        issue_123["url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu/issues/123");
        issue_123["number"] = serde_json::json!(123);
        let mut issue_124 = issue_123.clone();
        issue_124["html_url"] =
            serde_json::json!("https://github.com/Section9Labs/rupu/issues/124");
        issue_124["url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu/issues/124");
        issue_124["number"] = serde_json::json!(124);
        issue_124["title"] = serde_json::json!("Add missing regression");
        issue_124["updated_at"] = serde_json::json!("2026-05-03T09:00:00Z");
        issues = serde_json::json!([issue_123, issue_124]);
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(issues.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
    limit: 100
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/config.toml"),
            format!(
                r#"[autoflow]
enabled = true
permission_mode = "bypass"
strict_templates = true
max_active = 1

[scm.github]
base_url = "{}"
"#,
                server.base_url()
            ),
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::Complete,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: Some("run_123".into()),
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: None,
                pending_dispatch: None,
                contenders: vec![],
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let new_claim = claim_store
            .load("github:Section9Labs/rupu/issues/124")
            .unwrap()
            .unwrap();
        assert_eq!(new_claim.status, ClaimStatus::Complete);
        assert!(new_claim.last_run_id.is_some());
    }

    #[tokio::test]
    async fn tick_frees_capacity_after_existing_claim_completes() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let mut issues: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        let base_issue = issues.as_array().unwrap()[0].clone();
        let mut issue_123 = base_issue.clone();
        issue_123["repository_url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu");
        issue_123["html_url"] =
            serde_json::json!("https://github.com/Section9Labs/rupu/issues/123");
        issue_123["url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu/issues/123");
        issue_123["number"] = serde_json::json!(123);
        issue_123["updated_at"] = serde_json::json!("2026-05-01T09:00:00Z");
        let mut issue_124 = issue_123.clone();
        issue_124["html_url"] =
            serde_json::json!("https://github.com/Section9Labs/rupu/issues/124");
        issue_124["url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu/issues/124");
        issue_124["number"] = serde_json::json!(124);
        issue_124["title"] = serde_json::json!("Add missing regression");
        issue_124["updated_at"] = serde_json::json!("2026-05-03T09:00:00Z");
        issues = serde_json::json!([issue_123.clone(), issue_124]);
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(issues.clone());
        });
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues/123");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(issue_123.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "issue-supervisor-dispatch",
            r#"name: issue-supervisor-dispatch
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
    limit: 100
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/config.toml"),
            format!(
                r#"[autoflow]
enabled = true
permission_mode = "bypass"
strict_templates = true
max_active = 1

[scm.github]
base_url = "{}"
"#,
                server.base_url()
            ),
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        claim_store
            .save(&AutoflowClaimRecord {
                issue_ref: "github:Section9Labs/rupu/issues/123".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                source_ref: None,
                issue_display_ref: None,
                issue_title: None,
                issue_url: None,
                issue_state_name: None,
                issue_tracker: None,
                workflow: "issue-supervisor-dispatch".into(),
                status: ClaimStatus::AwaitExternal,
                worktree_path: Some(project.display().to_string()),
                branch: Some("rupu/issue-123".into()),
                last_run_id: None,
                last_error: None,
                last_summary: None,
                pr_url: None,
                artifacts: None,
                artifact_manifest_path: None,
                next_retry_at: None,
                claim_owner: None,
                lease_expires_at: Some(
                    (chrono::Utc::now() + chrono::Duration::hours(3)).to_rfc3339(),
                ),
                pending_dispatch: None,
                contenders: vec![],
                updated_at: (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
            })
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_123 = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        let claim_124 = claim_store
            .load("github:Section9Labs/rupu/issues/124")
            .unwrap()
            .unwrap();
        assert_eq!(claim_123.status, ClaimStatus::Complete);
        assert_eq!(claim_124.status, ClaimStatus::Complete);
        assert!(claim_123.last_run_id.is_some());
        assert!(claim_124.last_run_id.is_some());
    }

    #[tokio::test]
    async fn tick_continues_after_issue_level_failure() {
        let _guard = ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("home");
        let project = tmp.path().join("repo");
        init_git_repo(&project);

        let server = MockServer::start();
        let mut issues: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/../rupu-scm/tests/fixtures/github/issues_list_happy.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap(),
        )
        .unwrap();
        let base_issue = issues.as_array().unwrap()[0].clone();
        let mut issue_123 = base_issue.clone();
        issue_123["repository_url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu");
        issue_123["html_url"] =
            serde_json::json!("https://github.com/Section9Labs/rupu/issues/123");
        issue_123["url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu/issues/123");
        issue_123["number"] = serde_json::json!(123);
        issue_123["labels"] = serde_json::json!([{
            "id": 2,
            "node_id": "LA_2",
            "url": "https://api.github.com/repos/Section9Labs/rupu/labels/bad",
            "name": "bad",
            "color": "5319e7",
            "default": false
        }]);
        let mut issue_124 = base_issue.clone();
        issue_124["repository_url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu");
        issue_124["html_url"] =
            serde_json::json!("https://github.com/Section9Labs/rupu/issues/124");
        issue_124["url"] =
            serde_json::json!("https://api.github.com/repos/Section9Labs/rupu/issues/124");
        issue_124["number"] = serde_json::json!(124);
        issue_124["title"] = serde_json::json!("Valid autoflow issue");
        issue_124["updated_at"] = serde_json::json!("2026-05-03T09:00:00Z");
        issue_124["labels"] = serde_json::json!([{
            "id": 1,
            "node_id": "LA_1",
            "url": "https://api.github.com/repos/Section9Labs/rupu/labels/bug",
            "name": "bug",
            "color": "d73a4a",
            "default": true
        }]);
        issues = serde_json::json!([issue_123, issue_124]);
        server.mock(|when, then| {
            when.method(GET).path("/repos/Section9Labs/rupu/issues");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(issues.clone());
        });

        write_autoflow_project(
            &project,
            &server.base_url(),
            "good-autoflow",
            r#"name: good-autoflow
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bug"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: autoflow_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        );
        std::fs::write(
            project.join(".rupu/workflows/bad-autoflow.yaml"),
            r#"name: bad-autoflow
autoflow:
  enabled: true
  priority: 100
  selector:
    states: ["open"]
    labels_all: ["bad"]
  reconcile_every: "10m"
  claim:
    ttl: "3h"
  workspace:
    strategy: worktree
    branch: "rupu/issue-{{ issue.number }}"
  outcome:
    output: result
contracts:
  outputs:
    result:
      from_step: decide
      format: json
      schema: bad_outcome_v1
steps:
  - id: decide
    agent: echo
    actions: []
    prompt: "issue={{ issue.number }}"
"#,
        )
        .unwrap();
        std::fs::write(
            project.join(".rupu/contracts/bad_outcome_v1.json"),
            r#"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "title": "bad_outcome_v1",
  "type": "object",
  "required": ["status", "must_not_be_missing"],
  "properties": {
    "status": {
      "type": "string",
      "enum": ["complete"]
    },
    "must_not_be_missing": {
      "type": "string"
    }
  },
  "additionalProperties": false
}"#,
        )
        .unwrap();

        std::fs::create_dir_all(&global).unwrap();
        let repo_store = RepoRegistryStore {
            root: paths::repos_dir(&global),
        };
        repo_store
            .upsert(
                "github:Section9Labs/rupu",
                &project,
                Some("https://github.com/Section9Labs/rupu.git"),
                Some("HEAD"),
            )
            .unwrap();

        let resolver = Arc::new(InMemoryResolver::new());
        resolver
            .put(
                rupu_auth::backend::ProviderId::Github,
                AuthMode::ApiKey,
                StoredCredential::api_key("ghp_test"),
            )
            .await;

        std::env::set_var("RUPU_HOME", &global);
        std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", COMPLETE_SCRIPT);

        tick_with_resolver(resolver).await.unwrap();

        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        std::env::remove_var("RUPU_HOME");

        let claim_store = AutoflowClaimStore {
            root: paths::autoflow_claims_dir(&global),
        };
        let bad_claim = claim_store
            .load("github:Section9Labs/rupu/issues/123")
            .unwrap()
            .unwrap();
        let good_claim = claim_store
            .load("github:Section9Labs/rupu/issues/124")
            .unwrap()
            .unwrap();
        assert_eq!(bad_claim.status, ClaimStatus::Blocked);
        assert!(bad_claim
            .last_error
            .as_deref()
            .unwrap()
            .contains("output failed schema"));
        assert_eq!(good_claim.status, ClaimStatus::Complete);
        assert!(good_claim.last_run_id.is_some());
    }
}
