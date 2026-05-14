use crate::cmd::completers::{active_session_ids, archived_session_ids, session_ids};
use crate::cmd::retention::parse_retention_duration;
use crate::cmd::run::{
    canonicalize_if_exists, resolve_clone_dest, standalone_issue_ref, standalone_repo_ref,
    standalone_workspace_strategy, ReadonlyDecider,
};
use crate::cmd::transcript::{
    render_pretty_transcript_event, truncate_single_line, TranscriptPrettyContext,
};
use crate::cmd::ui::{LiveViewMode, UiPrefs};
use crate::output::formats::OutputFormat;
use crate::output::palette::{self, BRAND, DIM};
use crate::output::printer::{visible_len, wrap_with_ansi};
use crate::output::report::{self, CollectionOutput, DetailOutput};
use crate::output::rich_payload::{render_payload, render_tool_input, RenderedPayload};
use crate::output::viewport::ViewportState;
use crate::paths;
use crate::standalone_run_metadata::{
    metadata_path_for_run, write_metadata, StandaloneRunMetadata,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use clap::{Args as ClapArgs, Subcommand};
use clap_complete::ArgValueCompleter;
use comfy_table::Cell;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::style::Print;
use crossterm::terminal;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use rupu_agent::runner::{AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_agent::{load_agent, parse_mode, resolve_mode};
use rupu_providers::model_tier::{ContextWindow, ThinkingLevel};
use rupu_providers::types::{
    ContextManagement, Message, OutputFormat as ProviderOutputFormat, Speed,
};
use rupu_providers::AuthMode;
use rupu_runtime::provider_factory;
use rupu_runtime::WorkerKind;
use rupu_tools::{PermissionMode, ToolContext};
use rupu_transcript::{Event as TranscriptEvent, RunStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use tracing::warn;
use ulid::Ulid;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Start a persistent agent session.
    Start(StartArgs),
    /// List persistent agent sessions.
    List(ListArgs),
    /// Show session details.
    Show {
        #[arg(add = ArgValueCompleter::new(session_ids))]
        session_id: String,
    },
    /// Archive an inactive session and its owned transcripts.
    Archive {
        #[arg(add = ArgValueCompleter::new(active_session_ids))]
        session_id: String,
    },
    /// Restore an archived session and its owned transcripts.
    Restore {
        #[arg(add = ArgValueCompleter::new(archived_session_ids))]
        session_id: String,
    },
    /// Permanently delete a session and its owned transcripts.
    Delete(DeleteArgs),
    /// Delete archived sessions older than a cutoff.
    Prune(PruneArgs),
    /// Send a follow-up prompt to an existing session.
    Send(SendArgs),
    /// Attach to the current or last run in a session.
    Attach {
        #[arg(add = ArgValueCompleter::new(active_session_ids))]
        session_id: String,
        #[arg(long, value_enum)]
        view: Option<LiveViewMode>,
    },
    /// Stop an active session worker.
    Stop {
        #[arg(add = ArgValueCompleter::new(active_session_ids))]
        session_id: String,
    },
    #[command(name = "_run-turn", hide = true)]
    RunTurn(RunTurnArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub struct StartArgs {
    /// Agent name (matches an `agents/*.md` file).
    pub agent: String,
    /// Optional target reference, e.g. `github:owner/repo#42`.
    pub target: Option<String>,
    /// Optional initial user message. Defaults to `go` if omitted.
    pub prompt: Option<String>,
    /// Override permission mode (`ask` | `bypass` | `readonly`).
    #[arg(long)]
    pub mode: Option<String>,
    /// Skip token streaming provider mode for every turn in this session.
    #[arg(long)]
    pub no_stream: bool,
    /// For repo targets: clone into this directory instead of `./<repo>/`.
    #[arg(long, value_name = "PATH")]
    pub into: Option<PathBuf>,
    /// Start the first turn without auto-attaching.
    #[arg(long)]
    pub detach: bool,
    /// Live renderer mode when auto-attaching.
    #[arg(long, value_enum)]
    pub view: Option<LiveViewMode>,
}

#[derive(ClapArgs, Debug, Clone, Default)]
pub struct ListArgs {
    /// Include both active and archived sessions.
    #[arg(long, conflicts_with = "archived")]
    pub all: bool,
    /// Show only archived sessions.
    #[arg(long, conflicts_with = "all")]
    pub archived: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct SendArgs {
    #[arg(add = ArgValueCompleter::new(active_session_ids))]
    pub session_id: String,
    pub prompt: String,
    #[arg(long)]
    pub detach: bool,
    /// Live renderer mode when auto-attaching.
    #[arg(long, value_enum)]
    pub view: Option<LiveViewMode>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct DeleteArgs {
    #[arg(add = ArgValueCompleter::new(session_ids))]
    pub session_id: String,
    #[arg(long)]
    pub force: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct PruneArgs {
    /// Retention cutoff, e.g. `30d`, `12h`, or `1w`.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct RunTurnArgs {
    #[arg(long)]
    pub session_id: String,
    #[arg(long)]
    pub run_id: String,
    #[arg(long)]
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionStatus {
    Idle,
    Running,
    Failed,
    Stopped,
}

impl SessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    fn ui_status(self) -> crate::output::palette::Status {
        match self {
            Self::Idle => crate::output::palette::Status::Skipped,
            Self::Running => crate::output::palette::Status::Working,
            Self::Failed => crate::output::palette::Status::Failed,
            Self::Stopped => crate::output::palette::Status::Awaiting,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionScope {
    Active,
    Archived,
}

enum AttachExit {
    Detach,
    Quit,
}

enum AttachCommand {
    Help,
    Status,
    Detach,
    Quit,
    Cancel,
    Stop,
    History,
    Transcript,
    Runs,
}

enum SessionInput {
    Submit(String),
    Cancelled,
    Empty,
}

enum AttachControl {
    Continue,
    Exit(AttachExit),
}

impl SessionScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRunRecord {
    run_id: String,
    prompt: String,
    transcript_path: PathBuf,
    started_at: DateTime<Utc>,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    status: Option<RunStatus>,
    #[serde(default)]
    total_tokens_in: u64,
    #[serde(default)]
    total_tokens_out: u64,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecord {
    version: u32,
    session_id: String,
    agent_name: String,
    #[serde(default)]
    description: Option<String>,
    provider_name: String,
    #[serde(default)]
    auth_mode: Option<AuthMode>,
    model: String,
    agent_system_prompt: String,
    #[serde(default)]
    agent_tools: Option<Vec<String>>,
    max_turns: u32,
    permission_mode: String,
    no_stream: bool,
    #[serde(default)]
    anthropic_oauth_prefix: Option<bool>,
    #[serde(default)]
    effort: Option<ThinkingLevel>,
    #[serde(default)]
    context_window: Option<ContextWindow>,
    #[serde(default)]
    output_format: Option<ProviderOutputFormat>,
    #[serde(default)]
    anthropic_task_budget: Option<u32>,
    #[serde(default)]
    anthropic_context_management: Option<ContextManagement>,
    #[serde(default)]
    anthropic_speed: Option<Speed>,
    #[serde(default)]
    dispatchable_agents: Option<Vec<String>>,
    workspace_id: String,
    workspace_path: PathBuf,
    #[serde(default)]
    project_root: Option<PathBuf>,
    transcripts_dir: PathBuf,
    #[serde(default)]
    repo_ref: Option<String>,
    #[serde(default)]
    issue_ref: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    workspace_strategy: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: SessionStatus,
    #[serde(default)]
    active_run_id: Option<String>,
    #[serde(default)]
    active_transcript_path: Option<PathBuf>,
    #[serde(default)]
    active_pid: Option<u32>,
    #[serde(default)]
    last_run_id: Option<String>,
    #[serde(default)]
    last_transcript_path: Option<PathBuf>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    total_turns: u32,
    #[serde(default)]
    total_tokens_in: u64,
    #[serde(default)]
    total_tokens_out: u64,
    #[serde(default)]
    message_history: Vec<Message>,
    #[serde(default)]
    runs: Vec<SessionRunRecord>,
}

impl SessionRecord {
    const VERSION: u32 = 1;
}

#[derive(Serialize)]
struct SessionListRow {
    session_id: String,
    agent: String,
    scope: String,
    status: String,
    target: Option<String>,
    active_run_id: Option<String>,
    updated_at: String,
}

#[derive(Serialize)]
struct SessionListCsvRow {
    session_id: String,
    agent: String,
    scope: String,
    status: String,
    target: String,
    active_run_id: String,
    updated_at: String,
}

#[derive(Serialize)]
struct SessionListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<SessionListRow>,
}

struct SessionListOutput {
    report: SessionListReport,
    csv_rows: Vec<SessionListCsvRow>,
}

#[derive(Serialize)]
struct SessionShowItem {
    session_id: String,
    agent: String,
    scope: String,
    status: String,
    provider: String,
    model: String,
    permission_mode: String,
    target: Option<String>,
    repo_ref: Option<String>,
    issue_ref: Option<String>,
    workspace_path: String,
    transcripts_dir: String,
    active_run_id: Option<String>,
    last_run_id: Option<String>,
    active_pid: Option<u32>,
    total_turns: u32,
    total_tokens_in: u64,
    total_tokens_out: u64,
    created_at: String,
    updated_at: String,
    last_error: Option<String>,
    runs: Vec<SessionRunView>,
}

#[derive(Serialize)]
struct SessionRunView {
    run_id: String,
    prompt: String,
    status: Option<String>,
    started_at: String,
    completed_at: Option<String>,
    transcript_path: String,
    total_tokens_in: u64,
    total_tokens_out: u64,
    error: Option<String>,
}

#[derive(Serialize)]
struct SessionShowReport {
    kind: &'static str,
    version: u8,
    item: SessionShowItem,
}

struct SessionShowOutput {
    report: SessionShowReport,
}

#[derive(Serialize)]
struct SessionPruneRow {
    session_id: String,
    scope: String,
    status: String,
    updated_at: String,
    action: String,
}

#[derive(Serialize)]
struct SessionPruneCsvRow {
    session_id: String,
    scope: String,
    status: String,
    updated_at: String,
    action: String,
}

#[derive(Serialize)]
struct SessionPruneReport {
    kind: &'static str,
    version: u8,
    rows: Vec<SessionPruneRow>,
}

struct SessionPruneOutput {
    report: SessionPruneReport,
    csv_rows: Vec<SessionPruneCsvRow>,
}

#[derive(Debug, Clone)]
pub(crate) struct PrunedSession {
    pub session_id: String,
    pub scope: String,
    pub status: String,
    pub updated_at: String,
    pub action: String,
}

impl CollectionOutput for SessionListOutput {
    type JsonReport = SessionListReport;
    type CsvRow = SessionListCsvRow;

    fn command_name(&self) -> &'static str {
        "session list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "session_id",
            "agent",
            "scope",
            "status",
            "target",
            "active_run_id",
            "updated_at",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "SESSION", "AGENT", "SCOPE", "STATUS", "TARGET", "RUN", "UPDATED",
        ]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.session_id),
                Cell::new(&row.agent),
                Cell::new(&row.scope),
                Cell::new(&row.status),
                Cell::new(row.target.as_deref().unwrap_or("—")),
                Cell::new(row.active_run_id.as_deref().unwrap_or("—")),
                Cell::new(&row.updated_at),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

impl DetailOutput for SessionShowOutput {
    type JsonReport = SessionShowReport;

    fn command_name(&self) -> &'static str {
        "session show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let item = &self.report.item;
        println!("session: {}", item.session_id);
        println!("agent: {}", item.agent);
        println!("scope: {}", item.scope);
        println!("status: {}", item.status);
        println!("provider: {}", item.provider);
        println!("model: {}", item.model);
        println!("permission mode: {}", item.permission_mode);
        if let Some(target) = item.target.as_deref() {
            println!("target: {target}");
        }
        if let Some(repo_ref) = item.repo_ref.as_deref() {
            println!("repo: {repo_ref}");
        }
        if let Some(issue_ref) = item.issue_ref.as_deref() {
            println!("issue: {issue_ref}");
        }
        println!("workspace: {}", item.workspace_path);
        println!("transcripts: {}", item.transcripts_dir);
        println!(
            "active run: {}",
            item.active_run_id.as_deref().unwrap_or("—")
        );
        println!("last run: {}", item.last_run_id.as_deref().unwrap_or("—"));
        println!(
            "usage: turns {}  ·  in {}  ·  out {}",
            item.total_turns, item.total_tokens_in, item.total_tokens_out
        );
        println!("created: {}", item.created_at);
        println!("updated: {}", item.updated_at);
        if let Some(error) = item.last_error.as_deref() {
            println!("last error: {error}");
        }
        if !item.runs.is_empty() {
            println!("runs:");
            for run in item.runs.iter().rev().take(10) {
                let status = run.status.as_deref().unwrap_or("running");
                println!(
                    "- {}  ·  {}  ·  {}",
                    run.run_id,
                    status,
                    truncate_single_line(&run.prompt, 72)
                );
                println!("  transcript: {}", run.transcript_path);
            }
        }
        Ok(())
    }
}

impl CollectionOutput for SessionPruneOutput {
    type JsonReport = SessionPruneReport;
    type CsvRow = SessionPruneCsvRow;

    fn command_name(&self) -> &'static str {
        "session prune"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["session_id", "scope", "status", "updated_at", "action"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["SESSION", "SCOPE", "STATUS", "UPDATED", "ACTION"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.session_id),
                Cell::new(&row.scope),
                Cell::new(&row.status),
                Cell::new(&row.updated_at),
                Cell::new(&row.action),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::Start(args) => start(args).await,
        Action::List(args) => list(args, global_format).await,
        Action::Show { session_id } => show(&session_id, global_format).await,
        Action::Archive { session_id } => archive(&session_id).await,
        Action::Restore { session_id } => restore(&session_id).await,
        Action::Delete(args) => delete(args).await,
        Action::Prune(args) => prune(args, global_format).await,
        Action::Send(args) => send(args).await,
        Action::Attach { session_id, view } => attach(&session_id, view).await,
        Action::Stop { session_id } => stop(&session_id).await,
        Action::RunTurn(args) => run_turn(args).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List(_) => ("session list", report::TABLE_JSON_CSV),
        Action::Show { .. } => ("session show", report::TABLE_JSON),
        Action::Archive { .. } => ("session archive", report::TABLE_ONLY),
        Action::Restore { .. } => ("session restore", report::TABLE_ONLY),
        Action::Delete(_) => ("session delete", report::TABLE_ONLY),
        Action::Prune(_) => ("session prune", report::TABLE_JSON_CSV),
        Action::Start(_) => ("session start", report::TABLE_ONLY),
        Action::Send(_) => ("session send", report::TABLE_ONLY),
        Action::Attach { .. } => ("session attach", report::TABLE_ONLY),
        Action::Stop { .. } => ("session stop", report::TABLE_ONLY),
        Action::RunTurn(_) => ("session _run-turn", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

async fn list(args: ListArgs, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let mut rows = Vec::new();
    let scopes: &[SessionScope] = if args.all {
        &[SessionScope::Active, SessionScope::Archived]
    } else if args.archived {
        &[SessionScope::Archived]
    } else {
        &[SessionScope::Active]
    };
    for &scope in scopes {
        for mut session in load_sessions_in_scope(&global, scope)? {
            if scope == SessionScope::Active && reconcile_stale_session(&mut session) {
                write_session(&global, scope, &session)?;
            }
            rows.push(SessionListRow {
                session_id: session.session_id.clone(),
                agent: session.agent_name.clone(),
                scope: scope.as_str().to_string(),
                status: session.status.as_str().to_string(),
                target: session.target.clone(),
                active_run_id: session.active_run_id.clone(),
                updated_at: session.updated_at.to_rfc3339(),
            });
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at.clone()));
    let csv_rows = rows
        .iter()
        .map(|row| SessionListCsvRow {
            session_id: row.session_id.clone(),
            agent: row.agent.clone(),
            scope: row.scope.clone(),
            status: row.status.clone(),
            target: row.target.clone().unwrap_or_default(),
            active_run_id: row.active_run_id.clone().unwrap_or_default(),
            updated_at: row.updated_at.clone(),
        })
        .collect();
    let output = SessionListOutput {
        report: SessionListReport {
            kind: "session_list",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

async fn show(session_id: &str, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, session_id)?;
    if scope == SessionScope::Active && reconcile_stale_session(&mut session) {
        write_session(&global, scope, &session)?;
    }
    let output = SessionShowOutput {
        report: SessionShowReport {
            kind: "session_show",
            version: 1,
            item: SessionShowItem {
                session_id: session.session_id.clone(),
                agent: session.agent_name.clone(),
                scope: scope.as_str().to_string(),
                status: session.status.as_str().to_string(),
                provider: session.provider_name.clone(),
                model: session.model.clone(),
                permission_mode: session.permission_mode.clone(),
                target: session.target.clone(),
                repo_ref: session.repo_ref.clone(),
                issue_ref: session.issue_ref.clone(),
                workspace_path: session.workspace_path.display().to_string(),
                transcripts_dir: session.transcripts_dir.display().to_string(),
                active_run_id: session.active_run_id.clone(),
                last_run_id: session.last_run_id.clone(),
                active_pid: session.active_pid,
                total_turns: session.total_turns,
                total_tokens_in: session.total_tokens_in,
                total_tokens_out: session.total_tokens_out,
                created_at: session.created_at.to_rfc3339(),
                updated_at: session.updated_at.to_rfc3339(),
                last_error: session.last_error.clone(),
                runs: session
                    .runs
                    .iter()
                    .map(|run| SessionRunView {
                        run_id: run.run_id.clone(),
                        prompt: run.prompt.clone(),
                        status: run
                            .status
                            .map(|value| format!("{:?}", value).to_lowercase()),
                        started_at: run.started_at.to_rfc3339(),
                        completed_at: run.completed_at.map(|value| value.to_rfc3339()),
                        transcript_path: run.transcript_path.display().to_string(),
                        total_tokens_in: run.total_tokens_in,
                        total_tokens_out: run.total_tokens_out,
                        error: run.error.clone(),
                    })
                    .collect(),
            },
        },
    };
    report::emit_detail(global_format, &output)
}

async fn start(args: StartArgs) -> anyhow::Result<()> {
    if args.mode.as_deref() == Some("ask") {
        anyhow::bail!(
            "session start does not support `ask` mode; use `--mode bypass` or `--mode readonly`"
        );
    }
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    paths::ensure_dir(&paths::sessions_dir(&global))?;

    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let project_agents_parent = project_root.as_ref().map(|p| p.join(".rupu"));
    let spec = load_agent(&global, project_agents_parent.as_deref(), &args.agent)?;

    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;

    let cli_mode = args.mode.as_deref().and_then(parse_mode);
    let agent_mode = spec.permission_mode.as_deref().and_then(parse_mode);
    let global_mode = cfg.permission_mode.as_deref().and_then(parse_mode);
    let mode = resolve_mode(cli_mode, agent_mode, None, global_mode);
    if matches!(mode, PermissionMode::Ask) {
        anyhow::bail!("sessions require a non-interactive permission mode; use `--mode bypass` or `--mode readonly`");
    }

    let resolver = rupu_auth::KeychainResolver::new();
    let scm_registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);

    let (run_target, user_message) = match args.target.as_deref() {
        None => (None, args.prompt.clone().unwrap_or_else(|| "go".into())),
        Some(s) => match crate::run_target::parse_run_target(s) {
            Ok(t) => (Some(t), args.prompt.clone().unwrap_or_else(|| "go".into())),
            Err(_) => {
                let combined = match args.prompt.as_deref() {
                    Some(p) => format!("{s} {p}"),
                    None => s.to_string(),
                };
                (None, combined)
            }
        },
    };

    let agent_system_prompt = match run_target.as_ref() {
        Some(t) => format!(
            "{}\n\n## Run target\n\n{}",
            spec.system_prompt,
            crate::run_target::format_run_target_for_prompt(t)
        ),
        None => spec.system_prompt.clone(),
    };

    let workspace_path: PathBuf = match run_target.as_ref() {
        Some(crate::run_target::RunTarget::Repo {
            platform,
            owner,
            repo,
            ..
        })
        | Some(crate::run_target::RunTarget::Pr {
            platform,
            owner,
            repo,
            ..
        }) => {
            let r = rupu_scm::RepoRef {
                platform: *platform,
                owner: owner.clone(),
                repo: repo.clone(),
            };
            let conn = scm_registry.repo(*platform).ok_or_else(|| {
                anyhow::anyhow!(
                    "no {} credential — run `rupu auth login --provider {}`",
                    platform,
                    platform
                )
            })?;
            let (dest, _guard) = resolve_clone_dest(&pwd, repo, args.into.as_deref(), false)?;
            eprintln!("  cloning {}/{} → {}", owner, repo, dest.display());
            conn.clone_to(&r, &dest).await?;
            dest
        }
        _ => pwd.clone(),
    };

    if let Err(err) = crate::cmd::repos::auto_track_checkout(&global, &workspace_path) {
        warn!(path = %workspace_path.display(), error = %err, "failed to auto-track checkout");
    }

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, &workspace_path)?;

    let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
    let model = spec
        .model
        .clone()
        .or_else(|| cfg.default_model.clone())
        .unwrap_or_else(|| "claude-sonnet-4-6".into());

    let mode_str = match mode {
        PermissionMode::Ask => "ask",
        PermissionMode::Bypass => "bypass",
        PermissionMode::Readonly => "readonly",
    }
    .to_string();

    let repo_ref = standalone_repo_ref(run_target.as_ref(), &workspace_path);
    let issue_ref = standalone_issue_ref(run_target.as_ref());
    let workspace_strategy =
        standalone_workspace_strategy(run_target.as_ref(), &workspace_path, false);
    let session_id = format!("ses_{}", Ulid::new());
    let transcripts_dir = paths::transcripts_dir(&global, project_root.as_deref());
    paths::ensure_dir(&transcripts_dir)?;
    let now = Utc::now();
    let session = SessionRecord {
        version: SessionRecord::VERSION,
        session_id: session_id.clone(),
        agent_name: spec.name.clone(),
        description: spec.description.clone(),
        provider_name,
        auth_mode: spec.auth,
        model,
        agent_system_prompt,
        agent_tools: spec.tools.clone(),
        max_turns: spec.max_turns.unwrap_or(50),
        permission_mode: mode_str,
        no_stream: args.no_stream,
        anthropic_oauth_prefix: spec.anthropic_oauth_prefix,
        effort: spec.effort,
        context_window: spec.context_window,
        output_format: spec.output_format,
        anthropic_task_budget: spec.anthropic_task_budget,
        anthropic_context_management: spec.anthropic_context_management,
        anthropic_speed: spec.anthropic_speed,
        dispatchable_agents: spec.dispatchable_agents.clone(),
        workspace_id: ws.id,
        workspace_path: canonicalize_if_exists(&workspace_path),
        project_root,
        transcripts_dir,
        repo_ref,
        issue_ref,
        target: if run_target.is_some() {
            args.target.clone()
        } else {
            None
        },
        workspace_strategy,
        created_at: now,
        updated_at: now,
        status: SessionStatus::Idle,
        active_run_id: None,
        active_transcript_path: None,
        active_pid: None,
        last_run_id: None,
        last_transcript_path: None,
        last_error: None,
        total_turns: 0,
        total_tokens_in: 0,
        total_tokens_out: 0,
        message_history: Vec::new(),
        runs: Vec::new(),
    };
    write_session(&global, SessionScope::Active, &session)?;
    launch_turn(&global, &session_id, user_message, args.detach, args.view).await
}

async fn send(args: SendArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, &args.session_id)?;
    ensure_active_scope(scope, "session send")?;
    if reconcile_stale_session(&mut session) {
        write_session(&global, scope, &session)?;
    }
    if session.status == SessionStatus::Stopped {
        anyhow::bail!("session {} is stopped", session.session_id);
    }
    ensure_session_available(&session)?;
    launch_turn(
        &global,
        &session.session_id,
        args.prompt,
        args.detach,
        args.view,
    )
    .await
}

async fn launch_turn(
    global: &Path,
    session_id: &str,
    prompt: String,
    detach: bool,
    view: Option<LiveViewMode>,
) -> anyhow::Result<()> {
    let run_id = launch_turn_detached(global, session_id, prompt)?;

    if detach {
        println!("session: {session_id}");
        println!("run: {run_id}");
        println!("attach: rupu session attach {session_id}");
        return Ok(());
    }

    attach(session_id, view).await
}

fn launch_turn_detached(global: &Path, session_id: &str, prompt: String) -> anyhow::Result<String> {
    let (mut session, scope) = read_session(global, session_id)?;
    ensure_active_scope(scope, "session send")?;
    if reconcile_stale_session(&mut session) {
        write_session(global, scope, &session)?;
    }
    ensure_session_available(&session)?;

    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = session.transcripts_dir.join(format!("{run_id}.jsonl"));
    let started_at = Utc::now();
    session.status = SessionStatus::Running;
    session.updated_at = started_at;
    session.active_run_id = Some(run_id.clone());
    session.active_transcript_path = Some(transcript_path.clone());
    session.active_pid = None;
    session.last_error = None;
    session.runs.push(SessionRunRecord {
        run_id: run_id.clone(),
        prompt: prompt.clone(),
        transcript_path: transcript_path.clone(),
        started_at,
        completed_at: None,
        status: None,
        total_tokens_in: 0,
        total_tokens_out: 0,
        duration_ms: 0,
        pid: None,
        error: None,
    });
    write_session(global, scope, &session)?;

    let exe = std::env::current_exe()?;
    let child = Command::new(exe)
        .arg("session")
        .arg("_run-turn")
        .arg("--session-id")
        .arg(session_id)
        .arg("--run-id")
        .arg(&run_id)
        .arg("--prompt")
        .arg(&prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn session worker for {session_id}"))?;

    let pid = child.id();
    let (mut session, scope) = read_session(global, session_id)?;
    if let Some(run) = session.runs.iter_mut().find(|run| run.run_id == run_id) {
        run.pid = Some(pid);
    }
    session.active_pid = Some(pid);
    session.updated_at = Utc::now();
    write_session(global, scope, &session)?;
    drop(child);
    Ok(run_id)
}

async fn attach(session_id: &str, view: Option<LiveViewMode>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let session_id = session_id.to_string();
    tokio::task::spawn_blocking(move || attach_blocking(&global, &session_id, view))
        .await
        .map_err(|e| anyhow::anyhow!("session attach task failed: {e}"))?
}

fn attach_blocking(
    global: &Path,
    session_id: &str,
    view: Option<LiveViewMode>,
) -> anyhow::Result<()> {
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = rupu_config::layer_files(
        Some(&global.join("config.toml")),
        project_root
            .as_deref()
            .map(|root| root.join(".rupu/config.toml"))
            .as_deref(),
    )?;
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, false, None, None, view);
    let view_mode = prefs.live_view;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    if interactive {
        return attach_blocking_interactive(global, session_id, prefs);
    }
    let (mut session, scope) = read_session(global, session_id)?;
    ensure_active_scope(scope, "session attach")?;
    if reconcile_stale_session(&mut session) {
        write_session(global, scope, &session)?;
    }
    let mut transcript_path = session
        .active_transcript_path
        .clone()
        .or_else(|| session.last_transcript_path.clone())
        .ok_or_else(|| anyhow::anyhow!("session {} has no runs yet", session.session_id))?;
    let mut followed_run_id = session
        .active_run_id
        .clone()
        .or_else(|| session.last_run_id.clone());
    let mut tailer = crate::output::TranscriptTailer::new(&transcript_path);
    let mut printer = crate::output::LineStreamPrinter::new();
    let mut saw_header = false;
    let mut saw_any = false;
    let _raw_mode = RawModeGuard::new(interactive)?;
    if view_mode == LiveViewMode::Focused {
        render_session_attach_intro(&mut printer, &session);
    } else {
        render_attach_help_hint(&mut printer);
        if let Some(run_id) = followed_run_id.as_deref() {
            printer.sideband_event(
                crate::output::palette::Status::Working,
                "waiting for session output",
                Some(&compact_session_run_id(run_id)),
            );
        }
    }
    loop {
        let events = tailer.drain();
        for event in &events {
            render_pretty_transcript_event(
                &mut printer,
                event,
                &mut saw_header,
                view_mode,
                TranscriptPrettyContext::SessionAttached,
            );
            saw_any = true;
        }

        let (mut session, scope) = read_session(global, session_id)?;
        ensure_active_scope(scope, "session attach")?;
        if reconcile_stale_session(&mut session) {
            write_session(global, scope, &session)?;
        }
        let desired_run_id = session
            .active_run_id
            .clone()
            .or_else(|| session.last_run_id.clone());
        let desired_transcript_path = session
            .active_transcript_path
            .clone()
            .or_else(|| session.last_transcript_path.clone());
        if desired_run_id != followed_run_id {
            let final_events = tailer.drain();
            for event in &final_events {
                render_pretty_transcript_event(
                    &mut printer,
                    event,
                    &mut saw_header,
                    view_mode,
                    TranscriptPrettyContext::SessionAttached,
                );
                saw_any = true;
            }
            if let Some(next_path) = desired_transcript_path {
                transcript_path = next_path;
                followed_run_id = desired_run_id.clone();
                tailer = crate::output::TranscriptTailer::new(&transcript_path);
                saw_header = false;
                saw_any = false;
                if let Some(run_id) = desired_run_id.as_deref() {
                    printer.sideband_event(
                        crate::output::palette::Status::Working,
                        if view_mode == LiveViewMode::Focused {
                            "active run"
                        } else {
                            "following run"
                        },
                        Some(&compact_session_run_id(run_id)),
                    );
                }
            }
        }

        if interactive {
            match handle_attach_keypress(global, &session, &mut printer)? {
                AttachControl::Continue => {}
                AttachControl::Exit(kind) => {
                    printer.stop_ticker();
                    match kind {
                        AttachExit::Detach => printer.sideband_event(
                            crate::output::palette::Status::Awaiting,
                            "detached",
                            Some(&format!(
                                "re-attach with: rupu session attach {}",
                                session.session_id
                            )),
                        ),
                        AttachExit::Quit => printer.sideband_event(
                            crate::output::palette::Status::Skipped,
                            "viewer closed",
                            Some(&format!("session still available: {}", session.session_id)),
                        ),
                    }
                    return Ok(());
                }
            }
        }

        if !interactive && session.status != SessionStatus::Running {
            printer.stop_ticker();
            let final_events = tailer.drain();
            for event in &final_events {
                render_pretty_transcript_event(
                    &mut printer,
                    event,
                    &mut saw_header,
                    view_mode,
                    TranscriptPrettyContext::SessionAttached,
                );
                saw_any = true;
            }
            if !saw_any && transcript_path.exists() {
                let bytes = fs::read(&transcript_path)?;
                for line in bytes
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                {
                    let event: TranscriptEvent = serde_json::from_slice(line)?;
                    render_pretty_transcript_event(
                        &mut printer,
                        &event,
                        &mut saw_header,
                        view_mode,
                        TranscriptPrettyContext::SessionAttached,
                    );
                }
            }
            return Ok(());
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[derive(Clone)]
struct SessionViewLine {
    status: crate::output::palette::Status,
    text: String,
    continuation: bool,
}

struct SessionInteractiveState {
    followed_run_id: Option<String>,
    transcript_path: PathBuf,
    tailer: crate::output::TranscriptTailer,
    lines: Vec<SessionViewLine>,
    view_mode: LiveViewMode,
    viewport: ViewportState,
    compose: Option<String>,
}

impl SessionInteractiveState {
    fn new(
        transcript_path: PathBuf,
        followed_run_id: Option<String>,
        view_mode: LiveViewMode,
    ) -> Self {
        let tailer = crate::output::TranscriptTailer::new(transcript_path.clone());
        Self {
            followed_run_id,
            transcript_path,
            tailer,
            lines: Vec::new(),
            view_mode,
            viewport: ViewportState::default(),
            compose: None,
        }
    }

    fn push_line(&mut self, status: crate::output::palette::Status, text: impl Into<String>) {
        self.lines.push(SessionViewLine {
            status,
            text: text.into(),
            continuation: false,
        });
    }

    fn push_transcript_event(&mut self, event: &TranscriptEvent, prefs: &UiPrefs) {
        for line in transcript_event_lines(event, self.view_mode, prefs) {
            self.lines.push(line);
        }
    }
}

struct SessionScreenGuard;

impl SessionScreenGuard {
    fn enter() -> anyhow::Result<Self> {
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for SessionScreenGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

fn attach_blocking_interactive(
    global: &Path,
    session_id: &str,
    prefs: UiPrefs,
) -> anyhow::Result<()> {
    let (mut session, scope) = read_session(global, session_id)?;
    ensure_active_scope(scope, "session attach")?;
    if reconcile_stale_session(&mut session) {
        write_session(global, scope, &session)?;
    }
    let transcript_path = session
        .active_transcript_path
        .clone()
        .or_else(|| session.last_transcript_path.clone())
        .ok_or_else(|| anyhow::anyhow!("session {} has no runs yet", session.session_id))?;
    let followed_run_id = session
        .active_run_id
        .clone()
        .or_else(|| session.last_run_id.clone());

    let outcome = {
        let _raw_mode = RawModeGuard::new(true)?;
        let _screen = SessionScreenGuard::enter()?;
        let mut state =
            SessionInteractiveState::new(transcript_path, followed_run_id, prefs.live_view);
        let mut last_rows: Vec<String> = Vec::new();
        if let Some(run_id) = state.followed_run_id.as_deref() {
            state.push_line(
                crate::output::palette::Status::Working,
                format!("attached to {}", compact_session_run_id(run_id)),
            );
        }

        loop {
            for event in state.tailer.drain() {
                state.push_transcript_event(&event, &prefs);
            }

            let (mut session, scope) = read_session(global, session_id)?;
            ensure_active_scope(scope, "session attach")?;
            if reconcile_stale_session(&mut session) {
                write_session(global, scope, &session)?;
            }

            let desired_run_id = session
                .active_run_id
                .clone()
                .or_else(|| session.last_run_id.clone());
            let desired_transcript_path = session
                .active_transcript_path
                .clone()
                .or_else(|| session.last_transcript_path.clone());
            if desired_run_id != state.followed_run_id {
                for event in state.tailer.drain() {
                    state.push_transcript_event(&event, &prefs);
                }
                if let Some(next_path) = desired_transcript_path {
                    state.transcript_path = next_path.clone();
                    state.followed_run_id = desired_run_id.clone();
                    state.tailer = crate::output::TranscriptTailer::new(next_path);
                    if let Some(run_id) = desired_run_id.as_deref() {
                        state.push_line(
                            crate::output::palette::Status::Working,
                            format!("following {}", compact_session_run_id(run_id)),
                        );
                    }
                }
            }

            let rows = build_session_screen_rows(&session, &mut state, &prefs);
            if rows != last_rows {
                render_session_screen_rows(&rows)?;
                last_rows = rows;
            }

            if event::poll(std::time::Duration::from_millis(100))? {
                match event::read()? {
                    CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        match handle_session_live_keypress(
                            global, &session, &mut state, &prefs, key,
                        )? {
                            AttachControl::Continue => {}
                            AttachControl::Exit(AttachExit::Detach) => {
                                break AttachExit::Detach;
                            }
                            AttachControl::Exit(AttachExit::Quit) => {
                                break AttachExit::Quit;
                            }
                        }
                    }
                    CrosstermEvent::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
    };

    let (session, _scope) = read_session(global, session_id)?;
    ensure_active_scope(_scope, "session attach")?;

    match outcome {
        AttachExit::Detach => {
            println!("session: {}", session.session_id);
            println!("attach: rupu session attach {}", session.session_id);
            if let Some(run_id) = session
                .active_run_id
                .as_deref()
                .or(session.last_run_id.as_deref())
            {
                println!("run: {}", compact_session_run_id(run_id));
            }
        }
        AttachExit::Quit => {
            println!("session still available: {}", session.session_id);
            println!("attach: rupu session attach {}", session.session_id);
        }
    }
    Ok(())
}

fn handle_session_live_keypress(
    global: &Path,
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    prefs: &UiPrefs,
    key: KeyEvent,
) -> anyhow::Result<AttachControl> {
    if let Some(buffer) = state.compose.as_mut() {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                state.compose = None;
                state.push_line(crate::output::palette::Status::Skipped, "prompt cancelled");
                return Ok(AttachControl::Continue);
            }
            (KeyCode::Enter, _) => {
                let input = buffer.trim().to_string();
                state.compose = None;
                if input.is_empty() {
                    return Ok(AttachControl::Continue);
                }
                return handle_session_live_input(global, session, state, input);
            }
            (KeyCode::Backspace, _) => {
                buffer.pop();
                return Ok(AttachControl::Continue);
            }
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                buffer.push(ch);
                return Ok(AttachControl::Continue);
            }
            _ => return Ok(AttachControl::Continue),
        }
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('f'), _) => {
            state.view_mode = state.view_mode.toggled();
            rebuild_session_transcript_lines(state, prefs);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
            state.viewport.scroll_up(1);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
            state.viewport.scroll_down(1);
            Ok(AttachControl::Continue)
        }
        (KeyCode::PageUp, _) => {
            state.viewport.page_up();
            Ok(AttachControl::Continue)
        }
        (KeyCode::PageDown, _) => {
            state.viewport.page_down();
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('g'), KeyModifiers::NONE) => {
            state.viewport.jump_top();
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
            state.viewport.jump_bottom();
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('d'), _) | (KeyCode::Char(']'), KeyModifiers::CONTROL) => {
            Ok(AttachControl::Exit(AttachExit::Detach))
        }
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            Ok(AttachControl::Exit(AttachExit::Quit))
        }
        (KeyCode::Char('?'), _) => {
            append_session_help_lines(state);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('x'), _) => {
            let detail = cancel_active_turn_in_place(global, &session.session_id)?
                .unwrap_or_else(|| "no active turn".into());
            state.push_line(
                crate::output::palette::Status::Awaiting,
                format!("cancel requested  ·  {detail}"),
            );
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('s'), _) => {
            stop_session_in_place(global, &session.session_id)?;
            state.push_line(crate::output::palette::Status::Failed, "session stopped");
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('p'), _) | (KeyCode::Enter, _) => {
            if session.status == SessionStatus::Running {
                state.push_line(
                    crate::output::palette::Status::Awaiting,
                    format!(
                        "session busy  ·  {}",
                        session
                            .active_run_id
                            .as_deref()
                            .map(compact_session_run_id)
                            .unwrap_or_else(|| "turn still running".into())
                    ),
                );
                return Ok(AttachControl::Continue);
            }
            if session.status == SessionStatus::Stopped {
                state.push_line(
                    crate::output::palette::Status::Failed,
                    "session stopped  ·  use `rupu session start` to create a new one",
                );
                return Ok(AttachControl::Continue);
            }
            state.compose = Some(String::new());
            Ok(AttachControl::Continue)
        }
        (KeyCode::Esc, _) => {
            if session.status == SessionStatus::Running {
                let detail = cancel_active_turn_in_place(global, &session.session_id)?
                    .unwrap_or_else(|| "no active turn".into());
                state.push_line(
                    crate::output::palette::Status::Awaiting,
                    format!(
                        "cancel requested  ·  {detail}  ·  press p once the session is idle to send the next prompt"
                    ),
                );
                return Ok(AttachControl::Continue);
            }
            if session.status == SessionStatus::Stopped {
                state.push_line(
                    crate::output::palette::Status::Failed,
                    "session stopped  ·  use `rupu session start` to create a new one",
                );
                return Ok(AttachControl::Continue);
            }
            state.compose = Some(String::new());
            Ok(AttachControl::Continue)
        }
        _ => Ok(AttachControl::Continue),
    }
}

fn rebuild_session_transcript_lines(state: &mut SessionInteractiveState, prefs: &UiPrefs) {
    let transcript_path = state.transcript_path.clone();
    let followed_run_id = state.followed_run_id.clone();
    state.lines.clear();
    if let Some(run_id) = followed_run_id.as_deref() {
        state.push_line(
            crate::output::palette::Status::Working,
            format!("attached to {}", compact_session_run_id(run_id)),
        );
    }
    state.tailer = crate::output::TranscriptTailer::new(transcript_path);
    for event in state.tailer.drain() {
        state.push_transcript_event(&event, prefs);
    }
}

fn handle_session_live_input(
    global: &Path,
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    input: String,
) -> anyhow::Result<AttachControl> {
    if let Some(command) = parse_attach_command(&input) {
        return execute_session_live_command(global, session, state, command);
    }
    if input.starts_with('/') && !input.starts_with("//") {
        state.push_line(
            crate::output::palette::Status::Failed,
            format!("unknown command  ·  {}", input.trim()),
        );
        return Ok(AttachControl::Continue);
    }
    let prompt = if let Some(escaped) = input.strip_prefix("//") {
        format!("/{escaped}")
    } else {
        input.clone()
    };
    let run_id = launch_turn_detached(global, &session.session_id, prompt.clone())?;
    state.push_line(
        crate::output::palette::Status::Working,
        format!(
            "queued prompt  ·  {}  ·  {}",
            compact_session_run_id(&run_id),
            truncate_single_line(prompt.as_str(), 72)
        ),
    );
    Ok(AttachControl::Continue)
}

fn execute_session_live_command(
    global: &Path,
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    command: AttachCommand,
) -> anyhow::Result<AttachControl> {
    match command {
        AttachCommand::Help => append_session_help_lines(state),
        AttachCommand::Status => append_session_status_lines(state, session),
        AttachCommand::History => append_session_history_lines(state, session),
        AttachCommand::Transcript => append_session_transcript_lines(state, session),
        AttachCommand::Runs => append_session_runs_lines(state, session),
        AttachCommand::Cancel => {
            let detail = cancel_active_turn_in_place(global, &session.session_id)?
                .unwrap_or_else(|| "no active turn".into());
            state.push_line(
                crate::output::palette::Status::Awaiting,
                format!("cancel requested  ·  {detail}"),
            );
        }
        AttachCommand::Stop => {
            stop_session_in_place(global, &session.session_id)?;
            state.push_line(crate::output::palette::Status::Failed, "session stopped");
        }
        AttachCommand::Detach => return Ok(AttachControl::Exit(AttachExit::Detach)),
        AttachCommand::Quit => return Ok(AttachControl::Exit(AttachExit::Quit)),
    }
    Ok(AttachControl::Continue)
}

fn build_session_screen_rows(
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    prefs: &UiPrefs,
) -> Vec<String> {
    let (width, height) = terminal::size().unwrap_or((100, 30));
    let width = width.max(40) as usize;
    let height = height.max(12) as usize;
    build_session_screen_rows_for_size(session, state, prefs, width, height)
}

fn build_session_screen_rows_for_size(
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    _prefs: &UiPrefs,
    width: usize,
    height: usize,
) -> Vec<String> {
    let view_mode = state.view_mode;
    let mut rows = vec![
        render_session_header_line(session, view_mode, width),
        String::new(),
        retained_session_kv_row(
            "status",
            &session_status_detail(session),
            width,
            session.status.ui_status(),
        ),
    ];
    if let Some(route) = session_route_detail(session) {
        rows.push(retained_session_kv_row(
            "route",
            &route,
            width,
            crate::output::palette::Status::Active,
        ));
    }
    rows.push(retained_session_kv_row(
        "workspace",
        &session_workspace_detail(session),
        width,
        crate::output::palette::Status::Active,
    ));
    rows.push(retained_session_kv_row(
        "usage",
        &session_usage_detail(session),
        width,
        crate::output::palette::Status::Active,
    ));
    if let Some(prompt) = session_recent_prompt(session) {
        rows.push(retained_session_kv_row(
            "prompt",
            &prompt,
            width,
            crate::output::palette::Status::Active,
        ));
    }
    rows.push(String::new());

    let footer_reserved = 2usize;
    let available_event_rows = height
        .saturating_sub(rows.len())
        .saturating_sub(footer_reserved)
        .max(1);
    let event_rows = render_session_event_rows(state, width, available_event_rows);
    rows.extend(event_rows);
    while rows.len() < height.saturating_sub(footer_reserved) {
        rows.push(String::new());
    }

    rows.push(render_session_controls_line(width));
    rows.push(render_session_prompt_line(session, state, width));
    rows.truncate(height);
    rows
}

fn render_session_screen_rows(rows: &[String]) -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
    for (idx, row) in rows.iter().enumerate() {
        queue!(stdout, MoveTo(0, idx as u16), Print(row))?;
    }
    stdout.flush()?;
    Ok(())
}

fn render_session_header_line(
    session: &SessionRecord,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let agent = truncate_single_line(&session.agent_name, 24);
    let sid = truncate_single_line(&session.session_id, 24);
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "session attach", BRAND);
    buf.push_str("  ");
    let _ = palette::write_bold_colored(&mut buf, &agent, BRAND);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, &sid, DIM);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, view_mode.as_str(), DIM);
    truncate_ansi_line(&buf, width)
}

fn render_session_controls_line(width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, "controls", BRAND);
    let _ = palette::write_colored(
        &mut buf,
        "  f cycle view  ·  ↑/↓ scroll  ·  PgUp/PgDn page  ·  g top  ·  G tail  ·  p prompt  ·  Esc prompt/cancel turn  ·  x cancel turn  ·  d detach  ·  q quit  ·  ? help",
        DIM,
    );
    truncate_ansi_line(&buf, width)
}

fn render_session_prompt_line(
    session: &SessionRecord,
    state: &SessionInteractiveState,
    width: usize,
) -> String {
    if let Some(buffer) = state.compose.as_deref() {
        let mut buf = String::new();
        let _ = palette::write_bold_colored(&mut buf, "prompt", BRAND);
        let _ = palette::write_colored(
            &mut buf,
            &format!("  {}> {}", session.session_id, buffer),
            DIM,
        );
        truncate_ansi_line(&buf, width)
    } else {
        retained_session_kv_row(
            "view",
            &format!(
                "{}  ·  {}  ·  runs {}  ·  current {}",
                state.view_mode.as_str(),
                state.viewport.status_text(),
                session.runs.len(),
                session
                    .active_run_id
                    .as_deref()
                    .or(session.last_run_id.as_deref())
                    .map(compact_session_run_id)
                    .unwrap_or_else(|| "none".into())
            ),
            width,
            session.status.ui_status(),
        )
    }
}

fn render_session_event_rows(
    state: &mut SessionInteractiveState,
    width: usize,
    max_rows: usize,
) -> Vec<String> {
    let mut rendered = Vec::new();
    for line in &state.lines {
        let prefix = if line.continuation {
            "  ".to_string()
        } else {
            let mut value = String::new();
            let _ = palette::write_bold_colored(
                &mut value,
                &line.status.glyph().to_string(),
                line.status.color(),
            );
            value.push(' ');
            value
        };
        let content_width = width.saturating_sub(2).max(1);
        let wrapped = wrap_with_ansi(&line.text, content_width);
        for (idx, segment) in wrapped.into_iter().enumerate() {
            let text = if idx == 0 && !line.continuation {
                format!("{prefix}{segment}")
            } else {
                format!("  {segment}")
            };
            rendered.push(text);
        }
    }
    state.viewport.apply(rendered, max_rows).rows
}

fn transcript_event_lines(
    event: &TranscriptEvent,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
) -> Vec<SessionViewLine> {
    use crate::output::palette::Status;
    match event {
        TranscriptEvent::RunStart {
            run_id,
            workspace_id,
            mode,
            started_at,
            ..
        } => {
            if view_mode == LiveViewMode::Compact {
                return Vec::new();
            }
            vec![SessionViewLine {
                status: Status::Active,
                text: retained_session_event_line(
                    Status::Active,
                    "run started",
                    &format!(
                        "{}  ·  workspace {}  ·  mode {}  ·  {}",
                        compact_session_run_id(run_id),
                        workspace_id,
                        format!("{mode:?}").to_lowercase(),
                        started_at.format("%Y-%m-%d %H:%M:%S UTC")
                    ),
                ),
                continuation: false,
            }]
        }
        TranscriptEvent::TurnStart { turn_idx } => {
            if view_mode == LiveViewMode::Compact {
                return Vec::new();
            }
            vec![SessionViewLine {
                status: Status::Working,
                text: retained_session_event_line(
                    Status::Working,
                    &format!("turn {turn_idx}"),
                    "assistant turn started",
                ),
                continuation: false,
            }]
        }
        TranscriptEvent::AssistantMessage { content, thinking } => {
            let mut out = Vec::new();
            if let Some(thinking) = thinking.as_deref().filter(|value| !value.trim().is_empty()) {
                out.push(SessionViewLine {
                    status: Status::Active,
                    text: retained_session_event_line(
                        Status::Active,
                        "thinking",
                        &truncate_single_line(thinking, 96),
                    ),
                    continuation: false,
                });
            }
            if !content.trim().is_empty() {
                match view_mode {
                    LiveViewMode::Focused => out.push(SessionViewLine {
                        status: Status::Active,
                        text: retained_session_event_line(
                            Status::Active,
                            "assistant output",
                            &truncate_single_line(content, 96),
                        ),
                        continuation: false,
                    }),
                    LiveViewMode::Compact => {}
                    LiveViewMode::Full => {
                        let highlighted =
                            crate::output::rich_payload::render_assistant_content(
                                content.trim(),
                                prefs,
                            )
                            .rendered;
                        let mut lines = highlighted.split('\n');
                        if let Some(first) = lines.next() {
                            out.push(SessionViewLine {
                                status: Status::Active,
                                text: retained_session_event_line_raw(
                                    Status::Active,
                                    "assistant output",
                                    first,
                                ),
                                continuation: false,
                            });
                            for line in lines {
                                out.push(SessionViewLine {
                                    status: Status::Active,
                                    text: line.to_string(),
                                    continuation: true,
                                });
                            }
                        }
                    }
                }
            }
            out
        }
        TranscriptEvent::ToolCall { tool, input, .. } => match view_mode {
            LiveViewMode::Focused | LiveViewMode::Compact => vec![SessionViewLine {
                status: Status::Working,
                text: retained_session_event_line(
                    Status::Working,
                    &format!("tool {tool}"),
                    &transcript_tool_summary(tool, input),
                ),
                continuation: false,
            }],
            LiveViewMode::Full => {
                let mut out = vec![SessionViewLine {
                    status: Status::Working,
                    text: retained_session_event_line(
                        Status::Working,
                        &format!("tool {tool}"),
                        &transcript_tool_summary(tool, input),
                    ),
                    continuation: false,
                }];
                if let Some(rendered) = render_tool_input(tool, input, prefs) {
                    out.extend(rendered.lines().map(|line| SessionViewLine {
                        status: Status::Working,
                        text: line.to_string(),
                        continuation: true,
                    }));
                }
                out
            }
        },
        TranscriptEvent::ToolResult {
            output,
            error,
            duration_ms,
            ..
        } => {
            let status = if error.is_some() {
                Status::Failed
            } else {
                Status::Complete
            };
            let label = if error.is_some() {
                "tool error"
            } else {
                "tool result"
            };
            let raw = error.as_deref().unwrap_or(output.as_str());
            let payload = render_payload(raw, prefs);
            match view_mode {
                LiveViewMode::Focused | LiveViewMode::Compact => vec![SessionViewLine {
                    status,
                    text: retained_session_event_line(
                        status,
                        label,
                        &session_payload_summary(&payload, *duration_ms),
                    ),
                    continuation: false,
                }],
                LiveViewMode::Full => {
                    let mut out = vec![SessionViewLine {
                        status,
                        text: retained_session_event_line(
                            status,
                            label,
                            &session_payload_summary(&payload, *duration_ms),
                        ),
                        continuation: false,
                    }];
                    for line in session_rich_payload_lines(&payload) {
                        out.push(SessionViewLine {
                            status,
                            text: line,
                            continuation: true,
                        });
                    }
                    out
                }
            }
        }
        TranscriptEvent::FileEdit { path, kind, diff } => match view_mode {
            LiveViewMode::Focused | LiveViewMode::Compact => vec![SessionViewLine {
                status: Status::Complete,
                text: retained_session_event_line(
                    Status::Complete,
                    "file edit",
                    &format!("{} {}", format!("{kind:?}").to_lowercase(), path),
                ),
                continuation: false,
            }],
            LiveViewMode::Full => {
                let payload = render_payload(diff, prefs);
                let mut out = vec![SessionViewLine {
                    status: Status::Complete,
                    text: retained_session_event_line(
                        Status::Complete,
                        "file edit",
                        &format!(
                            "{} {}  ·  {}",
                            format!("{kind:?}").to_lowercase(),
                            path,
                            payload.headline
                        ),
                    ),
                    continuation: false,
                }];
                for line in session_rich_payload_lines(&payload) {
                    out.push(SessionViewLine {
                        status: Status::Complete,
                        text: line,
                        continuation: true,
                    });
                }
                out
            }
        },
        TranscriptEvent::CommandRun {
            argv,
            cwd,
            exit_code,
            ..
        } => vec![SessionViewLine {
            status: if *exit_code == 0 {
                Status::Complete
            } else {
                Status::Failed
            },
            text: retained_session_event_line(
                if *exit_code == 0 {
                    Status::Complete
                } else {
                    Status::Failed
                },
                "command",
                &format!(
                    "{}  ·  cwd {}  ·  exit {}",
                truncate_single_line(&argv.join(" "), 64),
                truncate_single_line(cwd, 24),
                exit_code
                ),
            ),
            continuation: false,
        }],
        TranscriptEvent::ActionEmitted {
            kind,
            allowed,
            applied,
            reason,
            ..
        } => {
            let mut detail = format!("action  ·  {kind}  ·  allowed={allowed} applied={applied}");
            if let Some(reason) = reason.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(reason, 64));
            }
            vec![SessionViewLine {
                status: if *applied {
                    Status::Complete
                } else if *allowed {
                    Status::Awaiting
                } else {
                    Status::Failed
                },
                text: retained_session_event_line_raw(
                    if *applied {
                        Status::Complete
                    } else if *allowed {
                        Status::Awaiting
                    } else {
                        Status::Failed
                    },
                    "action",
                    &detail,
                ),
                continuation: false,
            }]
        }
        TranscriptEvent::GateRequested {
            gate_id,
            prompt,
            decision,
            decided_by,
        } => {
            let mut detail = format!(
                "approval gate  ·  {gate_id}  ·  {}",
                truncate_single_line(prompt, 72)
            );
            if let Some(decision) = decision.as_deref() {
                detail.push_str(&format!("  ·  decision {decision}"));
            }
            if let Some(decided_by) = decided_by.as_deref() {
                detail.push_str(&format!("  ·  by {decided_by}"));
            }
            vec![SessionViewLine {
                status: Status::Awaiting,
                text: retained_session_event_line_raw(Status::Awaiting, "approval gate", &detail),
                continuation: false,
            }]
        }
        TranscriptEvent::TurnEnd {
            turn_idx,
            tokens_in,
            tokens_out,
        } => {
            if view_mode == LiveViewMode::Compact {
                return Vec::new();
            }
            vec![SessionViewLine {
                status: Status::Complete,
                text: retained_session_event_line(
                    Status::Complete,
                    "turn complete",
                    &format!(
                        "turn {turn_idx}  ·  in {} out {}",
                        tokens_in.unwrap_or(0),
                        tokens_out.unwrap_or(0)
                    ),
                ),
                continuation: false,
            }]
        }
        TranscriptEvent::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => {
            if view_mode == LiveViewMode::Compact {
                return Vec::new();
            }
            vec![SessionViewLine {
                status: Status::Active,
                text: retained_session_event_line_raw(
                    Status::Active,
                    "usage",
                    &format!(
                        "{provider} · {model}  ·  in {input_tokens} out {output_tokens} cached {cached_tokens}"
                    ),
                ),
                continuation: false,
            }]
        }
        TranscriptEvent::RunComplete {
            status,
            total_tokens,
            duration_ms,
            error,
            ..
        } => {
            let mut detail = format!(
                "run complete  ·  status {}  ·  {}ms  ·  {} tokens",
                format!("{status:?}").to_lowercase(),
                duration_ms,
                total_tokens
            );
            if let Some(error) = error.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(error, 72));
            }
            vec![SessionViewLine {
                status: match status {
                    RunStatus::Ok => Status::Complete,
                    RunStatus::Error | RunStatus::Aborted => Status::Failed,
                },
                text: retained_session_event_line_raw(
                    match status {
                        RunStatus::Ok => Status::Complete,
                        RunStatus::Error | RunStatus::Aborted => Status::Failed,
                    },
                    "run complete",
                    &detail,
                ),
                continuation: false,
            }]
        }
    }
}

fn session_payload_summary(payload: &RenderedPayload, duration_ms: u64) -> String {
    let detail = truncate_single_line(&payload.headline, 84);
    if duration_ms > 0 {
        format!("{detail}  ·  {duration_ms}ms")
    } else {
        detail
    }
}

fn session_rich_payload_lines(payload: &RenderedPayload) -> Vec<String> {
    payload
        .rendered
        .lines()
        .map(|line| line.to_string())
        .collect()
}

fn retained_session_kv_row(
    label: &str,
    value: &str,
    width: usize,
    status: crate::output::palette::Status,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), status.color());
    let _ = palette::write_colored(
        &mut buf,
        &truncate_single_line(value, width.saturating_sub(11)),
        DIM,
    );
    truncate_ansi_line(&buf, width)
}

fn retained_session_event_line(
    status: crate::output::palette::Status,
    label: &str,
    detail: &str,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, status.color());
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, detail, DIM);
    buf
}

fn retained_session_event_line_raw(
    status: crate::output::palette::Status,
    label: &str,
    detail: &str,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, status.color());
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
        buf.push_str(detail);
    }
    buf
}

fn truncate_ansi_line(value: &str, width: usize) -> String {
    if visible_len(value) <= width {
        value.to_string()
    } else {
        wrap_with_ansi(value, width)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

fn transcript_tool_summary(tool: &str, input: &serde_json::Value) -> String {
    crate::output::workflow_printer::tool_summary(tool, input)
}

fn append_session_help_lines(state: &mut SessionInteractiveState) {
    state.push_line(
        crate::output::palette::Status::Active,
        "help  ·  f cycle view  ·  ↑/↓ scroll  ·  PgUp/PgDn page  ·  g top  ·  G tail  ·  p prompt  ·  Esc prompt/cancel turn  ·  x cancel turn  ·  s stop session  ·  d detach  ·  q quit",
    );
    state.push_line(
        crate::output::palette::Status::Active,
        "commands  ·  /help /status /history /runs /transcript /cancel /stop /detach /quit",
    );
}

fn append_session_status_lines(state: &mut SessionInteractiveState, session: &SessionRecord) {
    state.push_line(
        session_attach_status(session.status),
        format!("status  ·  {}", session_status_detail(session)),
    );
    if let Some(detail) = session_route_detail(session) {
        state.push_line(
            crate::output::palette::Status::Active,
            format!("route  ·  {detail}"),
        );
    }
    state.push_line(
        crate::output::palette::Status::Active,
        format!("workspace  ·  {}", session_workspace_detail(session)),
    );
    state.push_line(
        crate::output::palette::Status::Active,
        format!("usage  ·  {}", session_usage_detail(session)),
    );
    if let Some(prompt) = session_recent_prompt(session) {
        state.push_line(
            crate::output::palette::Status::Active,
            format!("last prompt  ·  {prompt}"),
        );
    }
    if let Some(error) = session
        .last_error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        state.push_line(
            crate::output::palette::Status::Failed,
            format!("last error  ·  {}", truncate_single_line(error, 96)),
        );
    }
}

fn append_session_history_lines(state: &mut SessionInteractiveState, session: &SessionRecord) {
    if session.runs.is_empty() {
        state.push_line(
            crate::output::palette::Status::Skipped,
            "history  ·  no prior turns",
        );
        return;
    }
    for run in session.runs.iter().rev().take(5) {
        state.push_line(
            crate::output::palette::Status::Active,
            format!(
                "history  ·  {}  ·  {}",
                compact_session_run_id(&run.run_id),
                truncate_single_line(&run.prompt, 72)
            ),
        );
    }
}

fn append_session_transcript_lines(state: &mut SessionInteractiveState, session: &SessionRecord) {
    let detail = session
        .active_transcript_path
        .as_ref()
        .or(session.last_transcript_path.as_ref())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "no transcript".into());
    state.push_line(
        crate::output::palette::Status::Active,
        format!("transcript  ·  {detail}"),
    );
}

fn append_session_runs_lines(state: &mut SessionInteractiveState, session: &SessionRecord) {
    if session.runs.is_empty() {
        state.push_line(crate::output::palette::Status::Skipped, "runs  ·  no runs");
        return;
    }
    for run in session.runs.iter().rev().take(5) {
        let status = run
            .status
            .map(|value| format!("{:?}", value).to_lowercase())
            .unwrap_or_else(|| "running".into());
        state.push_line(
            crate::output::palette::Status::Active,
            format!(
                "run  ·  {status}  ·  {}",
                compact_session_run_id(&run.run_id)
            ),
        );
    }
}

fn handle_attach_keypress(
    global: &Path,
    session: &SessionRecord,
    printer: &mut crate::output::LineStreamPrinter,
) -> anyhow::Result<AttachControl> {
    if !event::poll(std::time::Duration::from_millis(10))? {
        return Ok(AttachControl::Continue);
    }
    let CrosstermEvent::Key(key) = event::read()? else {
        return Ok(AttachControl::Continue);
    };
    if key.kind != KeyEventKind::Press {
        return Ok(AttachControl::Continue);
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('d'), _) | (KeyCode::Char(']'), KeyModifiers::CONTROL) => {
            Ok(AttachControl::Exit(AttachExit::Detach))
        }
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            Ok(AttachControl::Exit(AttachExit::Quit))
        }
        (KeyCode::Esc, _) => {
            if session.status == SessionStatus::Running {
                cancel_active_turn(global, &session.session_id, printer)?;
                printer.sideband_event(
                    crate::output::palette::Status::Awaiting,
                    "cancel requested",
                    Some("press p once the session is idle to send the next prompt"),
                );
                return Ok(AttachControl::Continue);
            }
            if session.status == SessionStatus::Stopped {
                printer.sideband_event(
                    crate::output::palette::Status::Failed,
                    "session stopped",
                    Some("use `rupu session start` to create a new one"),
                );
                return Ok(AttachControl::Continue);
            }
            match prompt_for_session_input(printer, &session.session_id)? {
                SessionInput::Submit(input) => {
                    return handle_session_input(global, session, printer, input)
                }
                SessionInput::Cancelled => {
                    printer.sideband_event(
                        crate::output::palette::Status::Skipped,
                        "prompt cancelled",
                        None,
                    );
                }
                SessionInput::Empty => {}
            }
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('?'), _) => {
            render_attach_help(printer);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('x'), _) => {
            cancel_active_turn(global, &session.session_id, printer)?;
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('s'), _) => {
            stop_session_in_place(global, &session.session_id)?;
            printer.sideband_event(
                crate::output::palette::Status::Failed,
                "session stopped",
                Some(&session.session_id),
            );
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('p'), _) | (KeyCode::Enter, _) => {
            if session.status == SessionStatus::Running {
                printer.sideband_event(
                    crate::output::palette::Status::Awaiting,
                    "session busy",
                    Some(
                        session
                            .active_run_id
                            .as_deref()
                            .map(compact_session_run_id)
                            .as_deref()
                            .unwrap_or("turn still running"),
                    ),
                );
                return Ok(AttachControl::Continue);
            }
            if session.status == SessionStatus::Stopped {
                printer.sideband_event(
                    crate::output::palette::Status::Failed,
                    "session stopped",
                    Some("use `rupu session start` to create a new one"),
                );
                return Ok(AttachControl::Continue);
            }
            match prompt_for_session_input(printer, &session.session_id)? {
                SessionInput::Submit(input) => {
                    return handle_session_input(global, session, printer, input)
                }
                SessionInput::Cancelled => {
                    printer.sideband_event(
                        crate::output::palette::Status::Skipped,
                        "prompt cancelled",
                        None,
                    );
                }
                SessionInput::Empty => {}
            }
            Ok(AttachControl::Continue)
        }
        _ => Ok(AttachControl::Continue),
    }
}

fn prompt_for_session_input(
    printer: &crate::output::LineStreamPrinter,
    session_id: &str,
) -> anyhow::Result<SessionInput> {
    let multi = printer.multi_handle();
    multi.suspend(|| -> anyhow::Result<SessionInput> {
        let mut stderr = io::stderr();
        let mut line = String::new();
        draw_session_prompt(&mut stderr, session_id, &line)?;
        loop {
            let CrosstermEvent::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = event::read()?
            else {
                continue;
            };
            if kind != KeyEventKind::Press {
                continue;
            }
            match (code, modifiers) {
                (KeyCode::Esc, _) => {
                    clear_session_prompt(&mut stderr)?;
                    writeln!(stderr)?;
                    return Ok(SessionInput::Cancelled);
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    clear_session_prompt(&mut stderr)?;
                    writeln!(stderr)?;
                    return Ok(SessionInput::Cancelled);
                }
                (KeyCode::Enter, _) => {
                    clear_session_prompt(&mut stderr)?;
                    writeln!(stderr)?;
                    let value = line.trim().to_string();
                    if value.is_empty() {
                        return Ok(SessionInput::Empty);
                    }
                    return Ok(SessionInput::Submit(value));
                }
                (KeyCode::Backspace, _) => {
                    line.pop();
                }
                (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    line.push(ch);
                }
                _ => {}
            }
            draw_session_prompt(&mut stderr, session_id, &line)?;
        }
    })
}

fn draw_session_prompt(
    stderr: &mut io::Stderr,
    session_id: &str,
    line: &str,
) -> anyhow::Result<()> {
    write!(stderr, "\r\x1b[2Ksession {session_id}> {line}")?;
    stderr.flush()?;
    Ok(())
}

fn clear_session_prompt(stderr: &mut io::Stderr) -> anyhow::Result<()> {
    write!(stderr, "\r\x1b[2K")?;
    stderr.flush()?;
    Ok(())
}

fn handle_session_input(
    global: &Path,
    session: &SessionRecord,
    printer: &mut crate::output::LineStreamPrinter,
    input: String,
) -> anyhow::Result<AttachControl> {
    if let Some(command) = parse_attach_command(&input) {
        return execute_attach_command(global, session, printer, command);
    }
    if input.starts_with('/') && !input.starts_with("//") {
        printer.sideband_event(
            crate::output::palette::Status::Failed,
            "unknown command",
            Some(input.trim()),
        );
        return Ok(AttachControl::Continue);
    }
    let prompt = if let Some(escaped) = input.strip_prefix("//") {
        format!("/{escaped}")
    } else {
        input.clone()
    };
    let run_id = launch_turn_detached(global, &session.session_id, prompt.clone())?;
    let detail = format!(
        "{}  ·  {}",
        compact_session_run_id(&run_id),
        truncate_single_line(prompt.as_str(), 72)
    );
    printer.sideband_event(
        crate::output::palette::Status::Working,
        "queued prompt",
        Some(&detail),
    );
    Ok(AttachControl::Continue)
}

fn parse_attach_command(input: &str) -> Option<AttachCommand> {
    let command = input.strip_prefix('/')?.trim();
    match command {
        "help" | "h" | "?" => Some(AttachCommand::Help),
        "status" => Some(AttachCommand::Status),
        "detach" => Some(AttachCommand::Detach),
        "quit" | "exit" => Some(AttachCommand::Quit),
        "cancel" => Some(AttachCommand::Cancel),
        "stop" => Some(AttachCommand::Stop),
        "history" => Some(AttachCommand::History),
        "transcript" => Some(AttachCommand::Transcript),
        "runs" => Some(AttachCommand::Runs),
        _ => None,
    }
}

fn execute_attach_command(
    global: &Path,
    session: &SessionRecord,
    printer: &mut crate::output::LineStreamPrinter,
    command: AttachCommand,
) -> anyhow::Result<AttachControl> {
    match command {
        AttachCommand::Help => render_attach_help(printer),
        AttachCommand::Status => render_session_status(printer, session),
        AttachCommand::History => render_session_history(printer, session),
        AttachCommand::Transcript => render_session_transcript(printer, session),
        AttachCommand::Runs => render_session_runs(printer, session),
        AttachCommand::Cancel => cancel_active_turn(global, &session.session_id, printer)?,
        AttachCommand::Stop => {
            stop_session_in_place(global, &session.session_id)?;
            printer.sideband_event(
                crate::output::palette::Status::Failed,
                "session stopped",
                Some(&session.session_id),
            );
        }
        AttachCommand::Detach => return Ok(AttachControl::Exit(AttachExit::Detach)),
        AttachCommand::Quit => return Ok(AttachControl::Exit(AttachExit::Quit)),
    }
    Ok(AttachControl::Continue)
}

fn render_attach_help_hint(printer: &mut crate::output::LineStreamPrinter) {
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "controls",
        Some("p/Esc prompt  ·  Esc/x cancel turn  ·  d detach  ·  q quit  ·  ? help"),
    );
}

fn render_attach_help(printer: &mut crate::output::LineStreamPrinter) {
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "keys",
        Some("p prompt  ·  Esc prompt/cancel turn  ·  Esc cancel prompt  ·  x cancel turn  ·  s stop session  ·  d detach  ·  q quit"),
    );
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "commands",
        Some("/help /status /history /runs /transcript /cancel /stop"),
    );
}

fn render_session_status(printer: &mut crate::output::LineStreamPrinter, session: &SessionRecord) {
    printer.sideband_event(
        session_attach_status(session.status),
        "status",
        Some(&session_status_detail(session)),
    );
    if let Some(detail) = session_route_detail(session) {
        printer.sideband_event(
            crate::output::palette::Status::Active,
            "route",
            Some(&detail),
        );
    }
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "workspace",
        Some(&session_workspace_detail(session)),
    );
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "usage",
        Some(&session_usage_detail(session)),
    );
    if let Some(prompt) = session_recent_prompt(session) {
        printer.sideband_event(
            crate::output::palette::Status::Active,
            "last prompt",
            Some(&prompt),
        );
    }
    if let Some(error) = session
        .last_error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        printer.sideband_event(
            crate::output::palette::Status::Failed,
            "last error",
            Some(&truncate_single_line(error, 96)),
        );
    }
}

fn render_session_history(printer: &mut crate::output::LineStreamPrinter, session: &SessionRecord) {
    if session.runs.is_empty() {
        printer.sideband_event(
            crate::output::palette::Status::Skipped,
            "history",
            Some("no prior turns"),
        );
        return;
    }
    for run in session.runs.iter().rev().take(5) {
        let detail = format!(
            "{}  ·  {}",
            run.run_id,
            truncate_single_line(&run.prompt, 72)
        );
        printer.sideband_event(
            crate::output::palette::Status::Active,
            "history",
            Some(&detail),
        );
    }
}

fn render_session_transcript(
    printer: &mut crate::output::LineStreamPrinter,
    session: &SessionRecord,
) {
    let detail = session
        .active_transcript_path
        .as_ref()
        .or(session.last_transcript_path.as_ref())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "no transcript".into());
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "transcript",
        Some(&detail),
    );
}

fn render_session_runs(printer: &mut crate::output::LineStreamPrinter, session: &SessionRecord) {
    if session.runs.is_empty() {
        printer.sideband_event(
            crate::output::palette::Status::Skipped,
            "runs",
            Some("no runs"),
        );
        return;
    }
    for run in session.runs.iter().rev().take(5) {
        let status = run
            .status
            .map(|value| format!("{:?}", value).to_lowercase())
            .unwrap_or_else(|| "running".into());
        let detail = format!("{status}  ·  {}", run.run_id);
        printer.sideband_event(crate::output::palette::Status::Active, "run", Some(&detail));
    }
}

fn render_session_attach_intro(
    printer: &mut crate::output::LineStreamPrinter,
    session: &SessionRecord,
) {
    printer.session_header(&session.session_id, &session.agent_name);
    printer.sideband_event(
        session_attach_status(session.status),
        "session",
        Some(&session_status_detail(session)),
    );
    if let Some(detail) = session_route_detail(session) {
        printer.sideband_event(
            crate::output::palette::Status::Active,
            "route",
            Some(&detail),
        );
    }
    render_attach_help_hint(printer);
}

fn session_attach_status(status: SessionStatus) -> crate::output::palette::Status {
    match status {
        SessionStatus::Idle => crate::output::palette::Status::Waiting,
        SessionStatus::Running => crate::output::palette::Status::Working,
        SessionStatus::Failed | SessionStatus::Stopped => crate::output::palette::Status::Failed,
    }
}

fn session_status_detail(session: &SessionRecord) -> String {
    let mut parts = vec![
        session.agent_name.clone(),
        session.status.as_str().to_string(),
        format!("turns {}", session.total_turns),
        format!("mode {}", session.permission_mode),
    ];
    if let Some(run_id) = session
        .active_run_id
        .as_deref()
        .or(session.last_run_id.as_deref())
    {
        parts.push(format!("run {}", compact_session_run_id(run_id)));
    }
    parts.join("  ·  ")
}

fn session_route_detail(session: &SessionRecord) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(repo_ref) = session.repo_ref.as_deref() {
        parts.push(format!("repo {}", truncate_single_line(repo_ref, 40)));
    }
    if let Some(target) = session.target.as_deref() {
        parts.push(format!("target {}", truncate_single_line(target, 52)));
    }
    if let Some(issue_ref) = session.issue_ref.as_deref() {
        parts.push(format!("issue {}", truncate_single_line(issue_ref, 52)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("  ·  "))
    }
}

fn session_workspace_detail(session: &SessionRecord) -> String {
    let mut parts = vec![truncate_single_line(
        &session.workspace_path.display().to_string(),
        52,
    )];
    if let Some(strategy) = session.workspace_strategy.as_deref() {
        parts.push(strategy.to_string());
    }
    parts.join("  ·  ")
}

fn session_usage_detail(session: &SessionRecord) -> String {
    format!(
        "{}  ·  {}  ·  in {} out {}",
        truncate_single_line(&session.provider_name, 14),
        truncate_single_line(&session.model, 24),
        session.total_tokens_in,
        session.total_tokens_out
    )
}

fn session_recent_prompt(session: &SessionRecord) -> Option<String> {
    session
        .runs
        .last()
        .map(|run| truncate_single_line(&run.prompt, 96))
}

fn compact_session_run_id(run_id: &str) -> String {
    if run_id.chars().count() <= 18 {
        run_id.to_string()
    } else {
        let head = run_id.chars().take(12).collect::<String>();
        let tail = run_id
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>();
        format!("{head}…{tail}")
    }
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn new(enabled: bool) -> anyhow::Result<Self> {
        if enabled {
            terminal::enable_raw_mode()?;
        }
        Ok(Self { enabled })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = terminal::disable_raw_mode();
        }
    }
}

async fn archive(session_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, session_id)?;
    if scope == SessionScope::Archived {
        anyhow::bail!("session {session_id} is already archived");
    }
    if reconcile_stale_session(&mut session) {
        write_session(&global, scope, &session)?;
    }
    ensure_session_not_running(&session, "archive")?;
    move_session_owned_transcripts(&mut session, SessionScope::Archived)?;
    move_session_scope(
        &global,
        &session.session_id,
        SessionScope::Active,
        SessionScope::Archived,
    )?;
    write_session(&global, SessionScope::Archived, &session)?;
    println!("archived session {}", session.session_id);
    Ok(())
}

async fn restore(session_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, session_id)?;
    if scope == SessionScope::Active {
        anyhow::bail!("session {session_id} is already active");
    }
    move_session_owned_transcripts(&mut session, SessionScope::Active)?;
    move_session_scope(
        &global,
        &session.session_id,
        SessionScope::Archived,
        SessionScope::Active,
    )?;
    write_session(&global, SessionScope::Active, &session)?;
    println!("restored session {}", session.session_id);
    Ok(())
}

async fn delete(args: DeleteArgs) -> anyhow::Result<()> {
    if !args.force {
        anyhow::bail!("session delete requires --force");
    }
    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, &args.session_id)?;
    if scope == SessionScope::Active && reconcile_stale_session(&mut session) {
        write_session(&global, scope, &session)?;
    }
    ensure_session_not_running(&session, "delete")?;
    delete_session_owned_artifacts(&session)?;
    let dir = session_dir(&global, scope, &session.session_id);
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("remove session dir {}", dir.display()))?;
    }
    println!("deleted session {}", session.session_id);
    Ok(())
}

async fn prune(args: PruneArgs, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let mut pruned = prune_archived_sessions(args.older_than.as_deref(), args.dry_run)?;
    pruned.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let rows = pruned
        .iter()
        .map(|row| SessionPruneRow {
            session_id: row.session_id.clone(),
            scope: row.scope.clone(),
            status: row.status.clone(),
            updated_at: row.updated_at.clone(),
            action: row.action.clone(),
        })
        .collect::<Vec<_>>();
    let csv_rows = pruned
        .iter()
        .map(|row| SessionPruneCsvRow {
            session_id: row.session_id.clone(),
            scope: row.scope.clone(),
            status: row.status.clone(),
            updated_at: row.updated_at.clone(),
            action: row.action.clone(),
        })
        .collect();
    let output = SessionPruneOutput {
        report: SessionPruneReport {
            kind: "session_prune",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

pub(crate) fn prune_archived_sessions(
    older_than: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<Vec<PrunedSession>> {
    let global = paths::global_dir()?;
    let cutoff = session_prune_cutoff(older_than, &global)?;
    let mut rows = Vec::new();
    for session in load_sessions_in_scope(&global, SessionScope::Archived)? {
        if session.updated_at > cutoff {
            continue;
        }
        rows.push(PrunedSession {
            session_id: session.session_id.clone(),
            scope: SessionScope::Archived.as_str().to_string(),
            status: session.status.as_str().to_string(),
            updated_at: session.updated_at.to_rfc3339(),
            action: if dry_run {
                "would_delete".into()
            } else {
                "deleted".into()
            },
        });
        if !dry_run {
            delete_session_owned_artifacts(&session)?;
            let dir = session_dir(&global, SessionScope::Archived, &session.session_id);
            if dir.exists() {
                fs::remove_dir_all(&dir)
                    .with_context(|| format!("remove session dir {}", dir.display()))?;
            }
        }
    }
    Ok(rows)
}

async fn stop(session_id: &str) -> anyhow::Result<()> {
    stop_session_in_place(&paths::global_dir()?, session_id)?;
    println!("stopped session {session_id}");
    Ok(())
}

fn stop_session_in_place(global: &Path, session_id: &str) -> anyhow::Result<()> {
    let (mut session, scope) = read_session(&global, session_id)?;
    ensure_active_scope(scope, "session stop")?;
    if reconcile_stale_session(&mut session) {
        write_session(&global, scope, &session)?;
    }
    if let Some(pid) = session.active_pid.filter(|pid| pid_is_running(*pid)) {
        let _ = terminate_pid(pid);
    }
    session.status = SessionStatus::Stopped;
    session.updated_at = Utc::now();
    session.last_error = Some("stopped by operator".into());
    session.last_run_id = session
        .active_run_id
        .clone()
        .or_else(|| session.last_run_id.clone());
    session.last_transcript_path = session
        .active_transcript_path
        .clone()
        .or_else(|| session.last_transcript_path.clone());
    session.active_run_id = None;
    session.active_transcript_path = None;
    session.active_pid = None;
    if let Some(run) = session
        .runs
        .last_mut()
        .filter(|run| run.completed_at.is_none())
    {
        run.completed_at = Some(Utc::now());
        run.status = Some(RunStatus::Aborted);
        run.error = Some("stopped by operator".into());
    }
    write_session(&global, scope, &session)?;
    Ok(())
}

fn cancel_active_turn(
    global: &Path,
    session_id: &str,
    printer: &mut crate::output::LineStreamPrinter,
) -> anyhow::Result<()> {
    let detail = cancel_active_turn_in_place(global, session_id)?;
    match detail {
        Some(run_id) => printer.sideband_event(
            crate::output::palette::Status::Awaiting,
            "cancel requested",
            Some(&run_id),
        ),
        None => printer.sideband_event(
            crate::output::palette::Status::Skipped,
            "no active turn",
            Some(session_id),
        ),
    }
    Ok(())
}

fn cancel_active_turn_in_place(global: &Path, session_id: &str) -> anyhow::Result<Option<String>> {
    let (mut session, scope) = read_session(global, session_id)?;
    ensure_active_scope(scope, "session cancel")?;
    if reconcile_stale_session(&mut session) {
        write_session(global, scope, &session)?;
    }
    let Some(run_id) = session.active_run_id.clone() else {
        return Ok(None);
    };
    if let Some(pid) = session.active_pid.filter(|pid| pid_is_running(*pid)) {
        let _ = terminate_pid(pid);
    }
    session.status = SessionStatus::Idle;
    session.updated_at = Utc::now();
    session.last_error = Some("turn cancelled by operator".into());
    session.last_run_id = Some(run_id.clone());
    session.last_transcript_path = session.active_transcript_path.clone();
    session.active_run_id = None;
    session.active_transcript_path = None;
    session.active_pid = None;
    if let Some(run) = session.runs.iter_mut().find(|run| run.run_id == run_id) {
        run.completed_at = Some(Utc::now());
        run.status = Some(RunStatus::Aborted);
        run.error = Some("turn cancelled by operator".into());
    }
    write_session(global, scope, &session)?;
    Ok(Some(run_id))
}

async fn run_turn(args: RunTurnArgs) -> anyhow::Result<()> {
    crate::logging::init_to_file();

    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, &args.session_id)?;
    ensure_active_scope(scope, "session _run-turn")?;
    let global_cfg_path = global.join("config.toml");
    let project_cfg_path = session
        .project_root
        .as_ref()
        .map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())?;
    let resolver = rupu_auth::KeychainResolver::new();
    let scm_registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);

    let provider_config = provider_factory::ProviderConfig {
        anthropic_oauth_system_prefix: session.anthropic_oauth_prefix,
    };
    let (_resolved_auth, provider) = provider_factory::build_for_provider_with_config(
        &session.provider_name,
        &session.model,
        session.auth_mode,
        &resolver,
        &provider_config,
    )
    .await?;

    let backend_id = "local_checkout".to_string();
    let worker_ctx = crate::cmd::workflow::default_execution_worker_context(WorkerKind::Cli, None);
    let worker_record = crate::cmd::workflow::upsert_worker_record(
        &global,
        &worker_ctx,
        &backend_id,
        &session.permission_mode,
        session.repo_ref.as_deref(),
    )?;

    paths::ensure_dir(&session.transcripts_dir)?;
    let transcript_path = session
        .transcripts_dir
        .join(format!("{}.jsonl", args.run_id));
    let metadata = StandaloneRunMetadata {
        version: StandaloneRunMetadata::VERSION,
        run_id: args.run_id.clone(),
        session_id: Some(session.session_id.clone()),
        archived_at: None,
        workspace_path: canonicalize_if_exists(&session.workspace_path),
        project_root: session.project_root.clone(),
        repo_ref: session.repo_ref.clone(),
        issue_ref: session.issue_ref.clone(),
        backend_id,
        worker_id: Some(worker_record.worker_id.clone()),
        trigger_source: "session_turn".into(),
        target: session.target.clone(),
        workspace_strategy: session.workspace_strategy.clone(),
    };
    write_metadata(
        &metadata_path_for_run(&session.transcripts_dir, &args.run_id),
        &metadata,
    )?;

    let tool_context = ToolContext {
        workspace_path: session.workspace_path.clone(),
        bash_env_allowlist: cfg.bash.env_allowlist.clone().unwrap_or_default(),
        bash_timeout_secs: cfg.bash.timeout_secs.unwrap_or(120),
        dispatcher: None,
        dispatchable_agents: session.dispatchable_agents.clone(),
        parent_run_id: None,
        depth: 0,
    };

    let decider: Arc<dyn PermissionDecider> = match session.permission_mode.as_str() {
        "readonly" => Arc::new(ReadonlyDecider),
        _ => Arc::new(BypassDecider),
    };

    let opts = AgentRunOpts {
        agent_name: session.agent_name.clone(),
        agent_system_prompt: session.agent_system_prompt.clone(),
        agent_tools: session.agent_tools.clone(),
        provider,
        provider_name: session.provider_name.clone(),
        model: session.model.clone(),
        run_id: args.run_id.clone(),
        workspace_id: session.workspace_id.clone(),
        workspace_path: session.workspace_path.clone(),
        transcript_path: transcript_path.clone(),
        max_turns: session.max_turns,
        decider,
        tool_context,
        user_message: args.prompt.clone(),
        initial_messages: session.message_history.clone(),
        turn_index_offset: session.total_turns,
        mode_str: session.permission_mode.clone(),
        no_stream: session.no_stream,
        suppress_stream_stdout: true,
        mcp_registry: Some(scm_registry),
        effort: session.effort,
        context_window: session.context_window,
        output_format: session.output_format,
        anthropic_task_budget: session.anthropic_task_budget,
        anthropic_context_management: session.anthropic_context_management,
        anthropic_speed: session.anthropic_speed,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: session.dispatchable_agents.clone(),
        step_id: String::new(),
        on_tool_call: None,
    };

    let outcome = rupu_agent::run_agent(opts).await;

    session = read_session(&global, &args.session_id)?.0;
    session.updated_at = Utc::now();
    session.active_run_id = None;
    session.active_transcript_path = None;
    session.active_pid = None;
    session.last_run_id = Some(args.run_id.clone());
    session.last_transcript_path = Some(transcript_path.clone());

    let mut run_status = Some(RunStatus::Error);
    let mut duration_ms = 0;
    let mut error_message = None;
    if let Some(run) = session
        .runs
        .iter_mut()
        .find(|run| run.run_id == args.run_id)
    {
        run.completed_at = Some(Utc::now());
        run.transcript_path = transcript_path.clone();
        match &outcome {
            Ok(result) => {
                run.status = Some(result.status);
                run_status = Some(result.status);
                run.total_tokens_in = result.total_tokens_in;
                run.total_tokens_out = result.total_tokens_out;
                run.duration_ms = duration_ms_from_transcript(&transcript_path).unwrap_or(0);
                duration_ms = run.duration_ms;
            }
            Err(err) => {
                let error = err.to_string();
                run.status = Some(RunStatus::Error);
                run.error = Some(error.clone());
                error_message = Some(error.clone());
                session.last_error = Some(error);
            }
        }
    }

    match outcome {
        Ok(result) => {
            session.status = if result.status == RunStatus::Ok {
                SessionStatus::Idle
            } else {
                SessionStatus::Failed
            };
            session.total_turns += result.turns;
            session.total_tokens_in += result.total_tokens_in;
            session.total_tokens_out += result.total_tokens_out;
            session.message_history = result.final_messages;
            session.last_error = if result.status == RunStatus::Ok {
                None
            } else {
                Some(format!(
                    "turn ended with status {}",
                    format!("{:?}", result.status).to_lowercase()
                ))
            };
        }
        Err(err) => {
            session.status = SessionStatus::Failed;
            session.last_error = Some(err.to_string());
        }
    }

    if let Some(run) = session
        .runs
        .iter_mut()
        .find(|run| run.run_id == args.run_id)
    {
        run.status = run_status;
        run.duration_ms = duration_ms;
        if run.error.is_none() {
            run.error = error_message.or_else(|| session.last_error.clone());
        }
    }

    write_session(&global, scope, &session)?;
    Ok(())
}

fn duration_ms_from_transcript(path: &Path) -> anyhow::Result<u64> {
    let bytes = fs::read(path)?;
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let event: TranscriptEvent = serde_json::from_slice(line)?;
        if let TranscriptEvent::RunComplete { duration_ms, .. } = event {
            return Ok(duration_ms);
        }
    }
    Ok(0)
}

fn ensure_session_available(session: &SessionRecord) -> anyhow::Result<()> {
    if session.status == SessionStatus::Running {
        if let Some(pid) = session.active_pid {
            if pid_is_running(pid) {
                anyhow::bail!(
                    "session {} is already running {}",
                    session.session_id,
                    session.active_run_id.as_deref().unwrap_or("a turn")
                );
            }
        }
    }
    Ok(())
}

fn ensure_active_scope(scope: SessionScope, command: &str) -> anyhow::Result<()> {
    if scope == SessionScope::Archived {
        anyhow::bail!("{command} does not support archived sessions; restore the session first");
    }
    Ok(())
}

fn ensure_session_not_running(session: &SessionRecord, action: &str) -> anyhow::Result<()> {
    if session.status == SessionStatus::Running && session.active_pid.is_some_and(pid_is_running) {
        anyhow::bail!(
            "cannot {action} session {} while {} is still running",
            session.session_id,
            session.active_run_id.as_deref().unwrap_or("a turn")
        );
    }
    Ok(())
}

fn reconcile_stale_session(session: &mut SessionRecord) -> bool {
    if session.status != SessionStatus::Running {
        return false;
    }
    let Some(pid) = session.active_pid else {
        return false;
    };
    if pid_is_running(pid) {
        return false;
    }
    session.status = SessionStatus::Failed;
    session.updated_at = Utc::now();
    session.last_error = Some("session worker exited unexpectedly".into());
    session.last_run_id = session
        .active_run_id
        .clone()
        .or_else(|| session.last_run_id.clone());
    session.last_transcript_path = session
        .active_transcript_path
        .clone()
        .or_else(|| session.last_transcript_path.clone());
    session.active_run_id = None;
    session.active_transcript_path = None;
    session.active_pid = None;
    if let Some(run) = session
        .runs
        .last_mut()
        .filter(|run| run.completed_at.is_none())
    {
        run.completed_at = Some(Utc::now());
        run.status.get_or_insert(RunStatus::Error);
        run.error
            .get_or_insert_with(|| "session worker exited unexpectedly".into());
    }
    true
}

fn session_dir(global: &Path, scope: SessionScope, session_id: &str) -> PathBuf {
    match scope {
        SessionScope::Active => paths::sessions_dir(global),
        SessionScope::Archived => paths::archived_sessions_dir(global),
    }
    .join(session_id)
}

fn session_record_path(global: &Path, scope: SessionScope, session_id: &str) -> PathBuf {
    session_dir(global, scope, session_id).join("session.json")
}

fn write_session(
    global: &Path,
    scope: SessionScope,
    session: &SessionRecord,
) -> anyhow::Result<()> {
    let dir = session_dir(global, scope, &session.session_id);
    fs::create_dir_all(&dir)?;
    let path = session_record_path(global, scope, &session.session_id);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(session)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn read_session(global: &Path, session_id: &str) -> anyhow::Result<(SessionRecord, SessionScope)> {
    for scope in [SessionScope::Active, SessionScope::Archived] {
        let path = session_record_path(global, scope, session_id);
        if !path.is_file() {
            continue;
        }
        let bytes =
            fs::read(&path).with_context(|| format!("read session record {}", path.display()))?;
        return Ok((serde_json::from_slice(&bytes)?, scope));
    }
    anyhow::bail!("unknown session: {session_id}")
}

fn load_sessions_in_scope(
    global: &Path,
    scope: SessionScope,
) -> anyhow::Result<Vec<SessionRecord>> {
    let dir = match scope {
        SessionScope::Active => paths::sessions_dir(global),
        SessionScope::Archived => paths::archived_sessions_dir(global),
    };
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path().join("session.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        let record: SessionRecord = serde_json::from_slice(&bytes)?;
        out.push(record);
    }
    Ok(out)
}

fn move_session_scope(
    global: &Path,
    session_id: &str,
    from: SessionScope,
    to: SessionScope,
) -> anyhow::Result<()> {
    let src = session_dir(global, from, session_id);
    let dst = session_dir(global, to, session_id);
    if dst.exists() {
        anyhow::bail!("session {} already exists in {}", session_id, to.as_str());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&src, &dst)
        .with_context(|| format!("move session {} → {}", src.display(), dst.display()))?;
    Ok(())
}

fn move_session_owned_transcripts(
    session: &mut SessionRecord,
    target_scope: SessionScope,
) -> anyhow::Result<()> {
    let active_root = session.transcripts_dir.clone();
    let archived_root = paths::archived_transcripts_dir(&active_root);
    fs::create_dir_all(&active_root)?;
    fs::create_dir_all(&archived_root)?;

    for run in &mut session.runs {
        let active_path = active_root.join(format!("{}.jsonl", run.run_id));
        let archived_path = archived_root.join(format!("{}.jsonl", run.run_id));
        let active_meta = metadata_path_for_run(&active_root, &run.run_id);
        let archived_meta = metadata_path_for_run(&archived_root, &run.run_id);
        match target_scope {
            SessionScope::Active => {
                move_if_exists(&run.transcript_path, &active_path)?;
                move_if_exists(&archived_path, &active_path)?;
                move_if_exists(&archived_meta, &active_meta)?;
                run.transcript_path = active_path;
            }
            SessionScope::Archived => {
                move_if_exists(&run.transcript_path, &archived_path)?;
                move_if_exists(&active_path, &archived_path)?;
                move_if_exists(&active_meta, &archived_meta)?;
                run.transcript_path = archived_path;
            }
        }
    }

    if let Some(run_id) = session.last_run_id.as_deref() {
        session.last_transcript_path =
            Some(transcript_path_for_scope(session, run_id, target_scope));
    }
    if let Some(run_id) = session.active_run_id.as_deref() {
        session.active_transcript_path =
            Some(transcript_path_for_scope(session, run_id, target_scope));
    }
    session.updated_at = Utc::now();
    Ok(())
}

fn delete_session_owned_artifacts(session: &SessionRecord) -> anyhow::Result<()> {
    let active_root = session.transcripts_dir.clone();
    let archived_root = paths::archived_transcripts_dir(&active_root);
    let mut seen = HashSet::new();
    for run in &session.runs {
        if !seen.insert(run.run_id.clone()) {
            continue;
        }
        let active_path = active_root.join(format!("{}.jsonl", run.run_id));
        let archived_path = archived_root.join(format!("{}.jsonl", run.run_id));
        let active_meta = metadata_path_for_run(&active_root, &run.run_id);
        let archived_meta = metadata_path_for_run(&archived_root, &run.run_id);
        remove_file_if_exists(&run.transcript_path)?;
        remove_file_if_exists(&active_path)?;
        remove_file_if_exists(&archived_path)?;
        remove_file_if_exists(&active_meta)?;
        remove_file_if_exists(&archived_meta)?;
    }
    Ok(())
}

fn transcript_path_for_scope(
    session: &SessionRecord,
    run_id: &str,
    scope: SessionScope,
) -> PathBuf {
    match scope {
        SessionScope::Active => session.transcripts_dir.join(format!("{run_id}.jsonl")),
        SessionScope::Archived => paths::archived_transcripts_dir(&session.transcripts_dir)
            .join(format!("{run_id}.jsonl")),
    }
}

fn move_if_exists(from: &Path, to: &Path) -> anyhow::Result<()> {
    if !from.exists() || from == to {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if to.exists() {
        fs::remove_file(to).with_context(|| format!("remove {}", to.display()))?;
    }
    fs::rename(from, to).with_context(|| format!("move {} → {}", from.display(), to.display()))?;
    Ok(())
}

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

fn session_prune_cutoff(
    older_than: Option<&str>,
    global: &Path,
) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    let retention = if let Some(value) = older_than {
        value.to_string()
    } else {
        let path = global.join("config.toml");
        let cfg = rupu_config::layer_files(Some(&path), None)?;
        cfg.storage
            .archived_session_retention
            .unwrap_or_else(|| "30d".to_string())
    };
    Ok(Utc::now() - parse_retention_duration(&retention)?)
}

fn pid_is_running(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn terminate_pid(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parse_attach_command_handles_known_commands() {
        assert!(matches!(
            parse_attach_command("/help"),
            Some(AttachCommand::Help)
        ));
        assert!(matches!(
            parse_attach_command("/status"),
            Some(AttachCommand::Status)
        ));
        assert!(matches!(
            parse_attach_command("/cancel"),
            Some(AttachCommand::Cancel)
        ));
        assert!(matches!(
            parse_attach_command("/stop"),
            Some(AttachCommand::Stop)
        ));
        assert!(matches!(
            parse_attach_command("/runs"),
            Some(AttachCommand::Runs)
        ));
        assert!(parse_attach_command("plain prompt").is_none());
        assert!(parse_attach_command("/unknown").is_none());
    }

    #[test]
    fn session_status_detail_includes_current_run_and_mode() {
        let session = test_session_record();
        let detail = session_status_detail(&session);
        assert!(detail.contains("issue-reader"));
        assert!(detail.contains("running"));
        assert!(detail.contains("turns 3"));
        assert!(detail.contains("mode bypass"));
        assert!(detail.contains("run run_live123"));
    }

    #[test]
    fn session_route_detail_includes_repo_target_and_issue() {
        let session = test_session_record();
        let detail = session_route_detail(&session).expect("route detail");
        assert!(detail.contains("repo github:Section9Labs/rupu"));
        assert!(detail.contains("target github:Section9Labs/rupu/issues/42"));
        assert!(detail.contains("issue github:Section9Labs/rupu/issues/42"));
    }

    #[test]
    fn compact_session_run_id_shortens_long_values() {
        assert_eq!(
            compact_session_run_id("run_01KRJDKSBE7X4J49094149WFJS"),
            "run_01KRJDKS…WFJS"
        );
        assert_eq!(compact_session_run_id("run_short"), "run_short");
    }

    #[test]
    fn session_start_parses_view_mode() {
        let cli = crate::Cli::try_parse_from([
            "rupu",
            "session",
            "start",
            "issue-reader",
            "--view",
            "full",
        ])
        .expect("cli parses");
        match cli.command {
            crate::Cmd::Session {
                action: Action::Start(args),
            } => assert_eq!(args.view, Some(LiveViewMode::Full)),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn session_send_parses_view_mode() {
        let cli = crate::Cli::try_parse_from([
            "rupu",
            "session",
            "send",
            "ses_01",
            "next prompt",
            "--view",
            "focused",
        ])
        .expect("cli parses");
        match cli.command {
            crate::Cmd::Session {
                action: Action::Send(args),
            } => assert_eq!(args.view, Some(LiveViewMode::Focused)),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn retained_session_screen_rows_respect_width_and_height() {
        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Focused,
        );
        state.push_line(
            crate::output::palette::Status::Active,
            "assistant output  ·  this is a deliberately long line that should wrap cleanly inside the retained session screen without spilling past the requested width",
        );
        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 48, 14);
        assert!(rows.len() <= 14);
        assert!(rows.iter().all(|row| visible_len(row) <= 48));
    }

    #[test]
    fn retained_session_viewport_can_reach_oldest_rows() {
        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Focused,
        );
        for index in 0..700 {
            state.push_line(
                crate::output::palette::Status::Active,
                format!("history line {index:03}"),
            );
        }
        state.viewport.jump_top();
        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 72, 16);
        assert!(rows.iter().any(|row| row.contains("history line 000")));
        assert!(!rows.iter().any(|row| row.contains("history line 699")));
    }

    #[test]
    fn transcript_event_lines_full_keeps_more_assistant_text_than_focused() {
        let event = TranscriptEvent::AssistantMessage {
            content: "This is a longer assistant response that should stay fuller in full mode while focused mode truncates it.".into(),
            thinking: None,
        };
        let focused_prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let full_prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Full),
        );
        let focused = transcript_event_lines(&event, LiveViewMode::Focused, &focused_prefs);
        let full = transcript_event_lines(&event, LiveViewMode::Full, &full_prefs);
        assert_eq!(focused.len(), 1);
        assert!(!full.is_empty());
        assert!(focused[0].text.contains("assistant output"));
        assert!(full[0].text.contains("assistant output"));
        assert!(full.len() >= focused.len());
    }

    #[test]
    fn transcript_event_lines_full_expands_json_tool_results() {
        let event = TranscriptEvent::ToolResult {
            output: "{\"status\":\"ok\",\"items\":[1,2]}".into(),
            error: None,
            duration_ms: 12,
            call_id: "call_123".into(),
        };
        let focused_prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let full_prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Full),
        );
        let focused = transcript_event_lines(&event, LiveViewMode::Focused, &focused_prefs);
        let full = transcript_event_lines(&event, LiveViewMode::Full, &full_prefs);
        assert_eq!(focused.len(), 1);
        assert!(full.len() > 1);
        assert!(full[0].text.contains("json payload"));
    }

    fn test_session_record() -> SessionRecord {
        SessionRecord {
            version: SessionRecord::VERSION,
            session_id: "ses_test01".into(),
            agent_name: "issue-reader".into(),
            description: Some("Persistent issue reader".into()),
            provider_name: "anthropic".into(),
            auth_mode: None,
            model: "claude-sonnet-4-6".into(),
            agent_system_prompt: "You are a test agent.".into(),
            agent_tools: Some(vec!["read_file".into()]),
            max_turns: 50,
            permission_mode: "bypass".into(),
            no_stream: false,
            anthropic_oauth_prefix: None,
            effort: None,
            context_window: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            dispatchable_agents: None,
            workspace_id: "ws_test".into(),
            workspace_path: PathBuf::from("/tmp/repo"),
            project_root: Some(PathBuf::from("/tmp/repo")),
            transcripts_dir: PathBuf::from("/tmp/repo/.rupu/transcripts"),
            repo_ref: Some("github:Section9Labs/rupu".into()),
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            target: Some("github:Section9Labs/rupu/issues/42".into()),
            workspace_strategy: Some("direct_checkout".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: SessionStatus::Running,
            active_run_id: Some("run_live123".into()),
            active_transcript_path: Some(PathBuf::from(
                "/tmp/repo/.rupu/transcripts/run_live123.jsonl",
            )),
            active_pid: Some(1234),
            last_run_id: Some("run_prev123".into()),
            last_transcript_path: Some(PathBuf::from(
                "/tmp/repo/.rupu/transcripts/run_prev123.jsonl",
            )),
            last_error: None,
            total_turns: 3,
            total_tokens_in: 120,
            total_tokens_out: 45,
            message_history: Vec::new(),
            runs: vec![SessionRunRecord {
                run_id: "run_prev123".into(),
                prompt: "Summarize the issue.".into(),
                transcript_path: PathBuf::from("/tmp/repo/.rupu/transcripts/run_prev123.jsonl"),
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                status: Some(RunStatus::Ok),
                total_tokens_in: 100,
                total_tokens_out: 40,
                duration_ms: 1234,
                pid: None,
                error: None,
            }],
        }
    }

    #[test]
    fn transcript_event_lines_focused_summarizes_read_file_tool_call() {
        let event = TranscriptEvent::ToolCall {
            call_id: "call_123".into(),
            tool: "read_file".into(),
            input: serde_json::json!({"path": ".git/logs/refs/heads/storefront/issue-19"}),
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let lines = transcript_event_lines(&event, LiveViewMode::Focused, &prefs);
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .text
            .contains(".git/logs/refs/heads/storefront/issue-19"));
        assert!(!lines[0].text.contains("{\"path\""));
    }

    #[test]
    fn transcript_event_lines_full_expands_tool_call_payload() {
        let event = TranscriptEvent::ToolCall {
            call_id: "call_123".into(),
            tool: "read_file".into(),
            input: serde_json::json!({"path": ".git/logs/refs/heads/storefront/issue-19"}),
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Full),
        );
        let lines = transcript_event_lines(&event, LiveViewMode::Full, &prefs);
        assert!(lines.len() > 1);
        assert!(lines[0]
            .text
            .contains(".git/logs/refs/heads/storefront/issue-19"));
        assert!(lines.iter().skip(1).any(|line| line.text.contains("path:")));
    }
}
