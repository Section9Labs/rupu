use crate::cmd::retention::parse_retention_duration;
use crate::cmd::run::{
    canonicalize_if_exists, resolve_clone_dest, standalone_issue_ref, standalone_repo_ref,
    standalone_workspace_strategy, ReadonlyDecider,
};
use crate::cmd::transcript::{render_pretty_transcript_event, truncate_single_line};
use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput, DetailOutput};
use crate::paths;
use crate::standalone_run_metadata::{
    metadata_path_for_run, write_metadata, StandaloneRunMetadata,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
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
    Show { session_id: String },
    /// Archive an inactive session and its owned transcripts.
    Archive { session_id: String },
    /// Restore an archived session and its owned transcripts.
    Restore { session_id: String },
    /// Permanently delete a session and its owned transcripts.
    Delete(DeleteArgs),
    /// Delete archived sessions older than a cutoff.
    Prune(PruneArgs),
    /// Send a follow-up prompt to an existing session.
    Send(SendArgs),
    /// Attach to the current or last run in a session.
    Attach { session_id: String },
    /// Stop an active session worker.
    Stop { session_id: String },
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
    pub session_id: String,
    pub prompt: String,
    #[arg(long)]
    pub detach: bool,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct DeleteArgs {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionScope {
    Active,
    Archived,
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
        Action::Attach { session_id } => attach(&session_id).await,
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
    launch_turn(&global, &session_id, user_message, args.detach).await
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
    launch_turn(&global, &session.session_id, args.prompt, args.detach).await
}

async fn launch_turn(
    global: &Path,
    session_id: &str,
    prompt: String,
    detach: bool,
) -> anyhow::Result<()> {
    let run_id = launch_turn_detached(global, session_id, prompt)?;

    if detach {
        println!("session: {session_id}");
        println!("run: {run_id}");
        println!("attach: rupu session attach {session_id}");
        return Ok(());
    }

    attach(session_id).await
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

async fn attach(session_id: &str) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let session_id = session_id.to_string();
    tokio::task::spawn_blocking(move || attach_blocking(&global, &session_id))
        .await
        .map_err(|e| anyhow::anyhow!("session attach task failed: {e}"))?
}

fn attach_blocking(global: &Path, session_id: &str) -> anyhow::Result<()> {
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
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let _raw_mode = RawModeGuard::new(interactive)?;

    printer.sideband_event(
        crate::output::palette::Status::Active,
        "session attached",
        Some("p prompt  ·  d detach"),
    );
    if let Some(run_id) = followed_run_id.as_deref() {
        printer.sideband_event(
            crate::output::palette::Status::Working,
            "waiting for session output",
            Some(run_id),
        );
    }

    loop {
        let events = tailer.drain();
        for event in &events {
            render_pretty_transcript_event(&mut printer, event, &mut saw_header);
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
                render_pretty_transcript_event(&mut printer, event, &mut saw_header);
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
                        "following run",
                        Some(run_id),
                    );
                }
            }
        }

        if interactive && handle_attach_keypress(global, &session, &mut printer)? {
            printer.sideband_event(
                crate::output::palette::Status::Awaiting,
                "detached",
                Some(&format!(
                    "re-attach with: rupu session attach {}",
                    session.session_id
                )),
            );
            return Ok(());
        }

        if !interactive && session.status != SessionStatus::Running {
            let final_events = tailer.drain();
            for event in &final_events {
                render_pretty_transcript_event(&mut printer, event, &mut saw_header);
                saw_any = true;
            }
            if !saw_any && transcript_path.exists() {
                let bytes = fs::read(&transcript_path)?;
                for line in bytes
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                {
                    let event: TranscriptEvent = serde_json::from_slice(line)?;
                    render_pretty_transcript_event(&mut printer, &event, &mut saw_header);
                }
            }
            return Ok(());
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn handle_attach_keypress(
    global: &Path,
    session: &SessionRecord,
    printer: &mut crate::output::LineStreamPrinter,
) -> anyhow::Result<bool> {
    if !event::poll(std::time::Duration::from_millis(10))? {
        return Ok(false);
    }
    let CrosstermEvent::Key(key) = event::read()? else {
        return Ok(false);
    };
    if key.kind != KeyEventKind::Press {
        return Ok(false);
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('d'), _) | (KeyCode::Char(']'), KeyModifiers::CONTROL) => Ok(true),
        (KeyCode::Char('p'), _) | (KeyCode::Enter, _) => {
            if session.status == SessionStatus::Running {
                printer.sideband_event(
                    crate::output::palette::Status::Awaiting,
                    "session busy",
                    Some(
                        session
                            .active_run_id
                            .as_deref()
                            .unwrap_or("turn still running"),
                    ),
                );
                return Ok(false);
            }
            if session.status == SessionStatus::Stopped {
                printer.sideband_event(
                    crate::output::palette::Status::Failed,
                    "session stopped",
                    Some("use `rupu session start` to create a new one"),
                );
                return Ok(false);
            }
            if let Some(prompt) = prompt_for_session_input(printer, &session.session_id)? {
                let run_id = launch_turn_detached(global, &session.session_id, prompt.clone())?;
                let detail = format!("{run_id}  ·  {}", truncate_single_line(prompt.as_str(), 72));
                printer.sideband_event(
                    crate::output::palette::Status::Working,
                    "queued prompt",
                    Some(&detail),
                );
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn prompt_for_session_input(
    printer: &crate::output::LineStreamPrinter,
    session_id: &str,
) -> anyhow::Result<Option<String>> {
    let multi = printer.multi_handle();
    multi.suspend(|| -> anyhow::Result<Option<String>> {
        let _ = terminal::disable_raw_mode();
        eprint!("session {session_id} prompt> ");
        io::stderr().flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        terminal::enable_raw_mode()?;
        let prompt = line.trim().to_string();
        if prompt.is_empty() {
            return Ok(None);
        }
        Ok(Some(prompt))
    })
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
    let global = paths::global_dir()?;
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
    println!("stopped session {}", session.session_id);
    Ok(())
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
