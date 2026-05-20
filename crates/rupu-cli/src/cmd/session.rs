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
use crate::output::printer::{
    sanitize_terminal_text, visible_len, wrap_block_with_ansi, wrap_with_ansi,
};
use crate::output::report::{self, CollectionOutput, DetailOutput};
use crate::output::rich_payload::{
    render_assistant_content, render_payload, render_payload_preview_lines, render_tool_input,
    RenderedPayload,
};
use crate::output::viewport::ViewportState;
use crate::output::TranscriptHistoryPager;
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
use rupu_config::PricingConfig;
use rupu_providers::model_tier::{ContextWindow, ThinkingLevel};
use rupu_providers::types::{
    ContextManagement, Message, OutputFormat as ProviderOutputFormat, Speed, StreamEvent,
};
use rupu_providers::AuthMode;
use rupu_runtime::provider_factory;
use rupu_runtime::WorkerKind;
use rupu_tools::{PermissionMode, ToolContext};
use rupu_transcript::{Event as TranscriptEvent, FileEditKind, JsonlReader, RunMode, RunStatus};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
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
        #[arg(long, value_enum)]
        view: Option<LiveViewMode>,
        #[arg(long)]
        no_color: bool,
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        #[arg(long, conflicts_with = "pager")]
        no_pager: bool,
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
    #[command(name = "_worker", hide = true)]
    RunWorker(RunWorkerArgs),
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

#[derive(ClapArgs, Debug, Clone)]
pub struct RunWorkerArgs {
    #[arg(long)]
    pub session_id: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    Routed(RoutedAttachCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoutedAttachCommand {
    root: String,
    args: Vec<String>,
    display: String,
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
    total_tokens_cached: u64,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionTurnRequest {
    version: u32,
    request_id: String,
    run_id: String,
    prompt: String,
    transcript_path: PathBuf,
    enqueued_at: DateTime<Utc>,
}

impl SessionTurnRequest {
    const VERSION: u32 = 1;
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
    worker_pid: Option<u32>,
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
    total_tokens_cached: u64,
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
    prefs: UiPrefs,
    scope: SessionScope,
    session: SessionRecord,
    view_mode: LiveViewMode,
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
        let width = terminal::size()
            .map(|(value, _)| value.max(40) as usize)
            .unwrap_or(100);
        let body = render_session_show_snapshot(
            &self.session,
            self.scope,
            self.view_mode,
            &self.prefs,
            width,
        );
        crate::cmd::ui::paginate(&body, &self.prefs)
    }
}

fn render_session_show_snapshot(
    session: &SessionRecord,
    scope: SessionScope,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
    width: usize,
) -> String {
    let mut rows = vec![
        render_session_show_header_line(session, view_mode, width),
        String::new(),
        retained_session_kv_row(
            "scope",
            scope.as_str(),
            width,
            crate::output::palette::Status::Active,
        ),
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
        "transcript",
        &truncate_single_line(&session.transcripts_dir.display().to_string(), 96),
        width,
        crate::output::palette::Status::Active,
    ));
    rows.push(retained_session_kv_row(
        "usage",
        &session_usage_detail(session),
        width,
        crate::output::palette::Status::Active,
    ));
    rows.push(retained_session_kv_row(
        "created",
        &session.created_at.to_rfc3339(),
        width,
        crate::output::palette::Status::Active,
    ));
    rows.push(retained_session_kv_row(
        "updated",
        &session.updated_at.to_rfc3339(),
        width,
        crate::output::palette::Status::Active,
    ));
    if let Some(error) = session
        .last_error
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        rows.push(retained_session_kv_row(
            "last error",
            error,
            width,
            crate::output::palette::Status::Failed,
        ));
    }
    rows.push(String::new());
    rows.extend(render_session_show_run_rows(
        session, view_mode, prefs, width,
    ));
    rows.push(String::new());
    rows.push(render_session_show_footer_line(session, view_mode, width));
    rows.join("\n") + "\n"
}

fn render_session_show_header_line(
    session: &SessionRecord,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let agent = truncate_single_line(&session.agent_name, 24);
    let sid = truncate_single_line(&session.session_id, 24);
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "session show", BRAND);
    buf.push_str("  ");
    let _ = palette::write_bold_colored(&mut buf, &agent, BRAND);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, &sid, DIM);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, view_mode.as_str(), DIM);
    truncate_ansi_line(&buf, width)
}

fn render_session_show_footer_line(
    session: &SessionRecord,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    retained_session_kv_row(
        "view",
        &format!(
            "{}  ·  runs {}  ·  current {}",
            view_mode.as_str(),
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

fn render_session_show_run_rows(
    session: &SessionRecord,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
    width: usize,
) -> Vec<String> {
    if session.runs.is_empty() {
        return vec![render_session_show_section_header(
            "runs",
            "no runs yet",
            width,
        )];
    }

    let mut rows = vec![render_session_show_section_header(
        "runs",
        "recent turns",
        width,
    )];
    for run in session.runs.iter().rev().take(10) {
        let status = run
            .status
            .map(|value| format!("{:?}", value).to_lowercase())
            .unwrap_or_else(|| "running".into());
        let prompt = truncate_single_line(&run.prompt, 96);
        let detail = format!(
            "{}  ·  {}  ·  {}",
            compact_session_run_id(&run.run_id),
            status,
            prompt
        );
        rows.extend(render_session_show_event_lines(
            session_run_status_ui(run.status),
            "run",
            &detail,
            width,
            "",
        ));

        match view_mode {
            LiveViewMode::Focused => {}
            LiveViewMode::Compact => {
                let preview = format!(
                    "transcript {}  ·  in {} out {}",
                    truncate_single_line(&run.transcript_path.display().to_string(), 60),
                    run.total_tokens_in,
                    run.total_tokens_out
                );
                rows.extend(render_session_show_event_lines(
                    crate::output::palette::Status::Active,
                    "",
                    &preview,
                    width,
                    "│  ",
                ));
            }
            LiveViewMode::Full => {
                let timing = format!(
                    "started {}{}",
                    run.started_at.to_rfc3339(),
                    run.completed_at
                        .map(|value| format!("  ·  completed {}", value.to_rfc3339()))
                        .unwrap_or_default()
                );
                rows.extend(render_session_show_event_lines(
                    crate::output::palette::Status::Active,
                    "",
                    &timing,
                    width,
                    "│  ",
                ));
                let usage = format!(
                    "transcript {}  ·  in {} out {}  ·  {}ms",
                    truncate_single_line(&run.transcript_path.display().to_string(), 60),
                    run.total_tokens_in,
                    run.total_tokens_out,
                    run.duration_ms
                );
                rows.extend(render_session_show_event_lines(
                    crate::output::palette::Status::Active,
                    "",
                    &usage,
                    width,
                    "│  ",
                ));
                if let Some(error) = run
                    .error
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    rows.extend(render_session_show_event_lines(
                        crate::output::palette::Status::Failed,
                        "",
                        &truncate_single_line(error, 88),
                        width,
                        "│  ",
                    ));
                }
                rows.extend(render_session_show_transcript_rows(run, prefs, width));
            }
        }
    }
    rows
}

fn render_session_show_transcript_rows(
    run: &SessionRunRecord,
    prefs: &UiPrefs,
    width: usize,
) -> Vec<String> {
    let mut rows = Vec::new();
    let iter = match JsonlReader::iter(&run.transcript_path) {
        Ok(iter) => iter,
        Err(err) => {
            rows.extend(render_session_show_event_lines(
                crate::output::palette::Status::Failed,
                "",
                &format!(
                    "transcript unavailable  ·  {}",
                    truncate_single_line(&err.to_string(), 88)
                ),
                width,
                "│  ",
            ));
            return rows;
        }
    };

    for event in iter {
        match event {
            Ok(event) => rows.extend(render_session_show_transcript_event_rows(
                &transcript_event_lines(&event, LiveViewMode::Full, prefs),
                width,
            )),
            Err(err) => rows.extend(render_session_show_event_lines(
                crate::output::palette::Status::Failed,
                "",
                &format!(
                    "transcript parse error  ·  {}",
                    truncate_single_line(&err.to_string(), 88)
                ),
                width,
                "│  ",
            )),
        }
    }

    rows
}

fn render_session_show_transcript_event_rows(
    lines: &[SessionViewLine],
    width: usize,
) -> Vec<String> {
    let mut rendered = Vec::new();
    for line in lines {
        let prefix = if line.continuation {
            {
                let mut value = String::new();
                let _ = palette::write_colored(&mut value, "│", DIM);
                value.push_str("    ");
                value
            }
        } else {
            let mut value = String::new();
            let _ = palette::write_colored(&mut value, "│", DIM);
            value.push_str("  ");
            let _ = palette::write_bold_colored(
                &mut value,
                &line.status.glyph().to_string(),
                line.status.color(),
            );
            value.push(' ');
            value
        };
        let content_width = width.saturating_sub(visible_len(&prefix)).max(1);
        let wrapped = wrap_block_with_ansi(&line.text, content_width);
        for (idx, segment) in wrapped.into_iter().enumerate() {
            let text = if idx == 0 {
                format!("{prefix}{segment}")
            } else if line.continuation {
                let mut continuation = String::new();
                let _ = palette::write_colored(&mut continuation, "│", DIM);
                continuation.push_str("    ");
                format!("{continuation}{segment}")
            } else {
                let mut continuation = String::new();
                let _ = palette::write_colored(&mut continuation, "│", DIM);
                continuation.push_str("    ");
                format!("{continuation}{segment}")
            };
            rendered.push(truncate_ansi_line(&text, width));
        }
    }
    rendered
}

fn render_session_show_section_header(label: &str, detail: &str, width: usize) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, BRAND);
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
        let _ = palette::write_colored(&mut buf, detail, DIM);
    }
    truncate_ansi_line(&buf, width)
}

fn render_session_show_event_lines(
    status: crate::output::palette::Status,
    label: &str,
    detail: &str,
    width: usize,
    prefix: &str,
) -> Vec<String> {
    let raw = if label.is_empty() {
        detail.to_string()
    } else {
        retained_session_event_line(status, label, detail)
    };
    let available = width.saturating_sub(visible_len(prefix)).max(12);
    let wrapped = wrap_block_with_ansi(&raw, available);
    if wrapped.is_empty() {
        return vec![prefix.to_string()];
    }
    wrapped
        .into_iter()
        .enumerate()
        .map(|(idx, line)| {
            let row_prefix = if idx == 0 {
                prefix.to_string()
            } else if prefix.is_empty() {
                "│  ".to_string()
            } else {
                prefix.to_string()
            };
            truncate_ansi_line(&format!("{row_prefix}{line}"), width)
        })
        .collect()
}

fn session_run_status_ui(status: Option<RunStatus>) -> crate::output::palette::Status {
    match status {
        Some(RunStatus::Ok) => crate::output::palette::Status::Complete,
        Some(RunStatus::Error) => crate::output::palette::Status::Failed,
        Some(RunStatus::Aborted) => crate::output::palette::Status::Awaiting,
        None => crate::output::palette::Status::Working,
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
        Action::Show {
            session_id,
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
            show(&session_id, view, no_color, pager_flag, global_format).await
        }
        Action::Archive { session_id } => archive(&session_id).await,
        Action::Restore { session_id } => restore(&session_id).await,
        Action::Delete(args) => delete(args).await,
        Action::Prune(args) => prune(args, global_format).await,
        Action::Send(args) => send(args).await,
        Action::Attach { session_id, view } => attach(&session_id, view).await,
        Action::Stop { session_id } => stop(&session_id).await,
        Action::RunWorker(args) => run_worker(args).await,
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
        Action::RunWorker(_) => ("session _worker", report::TABLE_ONLY),
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

async fn show(
    session_id: &str,
    view: Option<LiveViewMode>,
    no_color: bool,
    pager_flag: Option<bool>,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let (mut session, scope) = read_session(&global, session_id)?;
    if scope == SessionScope::Active && reconcile_stale_session(&mut session) {
        write_session(&global, scope, &session)?;
    }
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = rupu_config::layer_files(
        Some(&global.join("config.toml")),
        project_root
            .as_deref()
            .map(|root| root.join(".rupu/config.toml"))
            .as_deref(),
    )?;
    let prefs = UiPrefs::resolve(&cfg.ui, no_color, None, pager_flag, view);
    let view_mode = prefs.live_view;
    let output = SessionShowOutput {
        prefs,
        scope,
        session: session.clone(),
        view_mode,
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
        anyhow::bail!(
            "sessions require a non-interactive permission mode; use `--mode bypass` or `--mode readonly`"
        );
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
        worker_pid: None,
        last_run_id: None,
        last_transcript_path: None,
        last_error: None,
        total_turns: 0,
        total_tokens_in: 0,
        total_tokens_out: 0,
        total_tokens_cached: 0,
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
    enqueue_turn_request(global, session_id, prompt)
}

fn enqueue_turn_request(global: &Path, session_id: &str, prompt: String) -> anyhow::Result<String> {
    let (mut session, scope) = read_session(global, session_id)?;
    ensure_active_scope(scope, "session send")?;
    if reconcile_stale_session(&mut session) {
        write_session(global, scope, &session)?;
    }
    if session.status == SessionStatus::Stopped {
        anyhow::bail!("session {} is stopped", session.session_id);
    }

    let worker_pid = ensure_session_worker(global, &mut session, scope)?;

    let run_id = format!("run_{}", Ulid::new());
    let transcript_path = session.transcripts_dir.join(format!("{run_id}.jsonl"));
    let request_id = Ulid::new().to_string();
    let enqueued_at = Utc::now();
    if session.status != SessionStatus::Running {
        session.status = SessionStatus::Running;
        session.active_run_id = Some(run_id.clone());
        session.active_transcript_path = Some(transcript_path.clone());
        session.active_pid = Some(worker_pid);
        clear_session_live_usage(global, scope, &session.session_id)?;
    }
    session.updated_at = enqueued_at;
    session.last_error = None;
    session.runs.push(SessionRunRecord {
        run_id: run_id.clone(),
        prompt: prompt.clone(),
        transcript_path: transcript_path.clone(),
        started_at: enqueued_at,
        completed_at: None,
        status: None,
        total_tokens_in: 0,
        total_tokens_out: 0,
        total_tokens_cached: 0,
        duration_ms: 0,
        pid: None,
        error: None,
    });
    write_session(global, scope, &session)?;

    enqueue_session_turn_request(
        global,
        scope,
        &session.session_id,
        SessionTurnRequest {
            version: SessionTurnRequest::VERSION,
            request_id,
            run_id: run_id.clone(),
            prompt,
            transcript_path,
            enqueued_at,
        },
    )?;
    Ok(run_id)
}

fn ensure_session_worker(
    global: &Path,
    session: &mut SessionRecord,
    scope: SessionScope,
) -> anyhow::Result<u32> {
    if let Some(pid) = session.worker_pid.filter(|pid| pid_is_running(*pid)) {
        return Ok(pid);
    }
    session.worker_pid = None;
    if session.status != SessionStatus::Running {
        session.active_pid = None;
    }
    write_session(global, scope, session)?;

    let exe = std::env::current_exe()?;
    let child = Command::new(exe)
        .arg("session")
        .arg("_worker")
        .arg("--session-id")
        .arg(&session.session_id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn warm session worker for {}", session.session_id))?;

    let pid = child.id();
    session.worker_pid = Some(pid);
    session.updated_at = Utc::now();
    if session.status == SessionStatus::Running {
        session.active_pid = Some(pid);
    }
    write_session(global, scope, session)?;
    drop(child);
    Ok(pid)
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
        return attach_blocking_interactive(global, session_id, prefs, cfg.pricing);
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

#[derive(Debug, Clone)]
struct SessionViewLine {
    status: crate::output::palette::Status,
    text: String,
    continuation: bool,
}

#[derive(Debug, Clone)]
enum SessionEntry {
    Notice(SessionViewLine),
    UserPrompt {
        content: String,
        run_id: Option<String>,
        queued: bool,
    },
    Assistant {
        content: String,
        thinking: Option<String>,
        streaming: bool,
    },
    ToolCall {
        tool: String,
        input: serde_json::Value,
    },
    ToolResult {
        output: String,
        error: Option<String>,
        duration_ms: u64,
    },
    FileEdit {
        path: String,
        kind: FileEditKind,
        diff: String,
    },
    CommandRun {
        argv: Vec<String>,
        cwd: String,
        exit_code: i32,
    },
    ActionEmitted {
        kind: String,
        allowed: bool,
        applied: bool,
        reason: Option<String>,
    },
    GateRequested {
        gate_id: String,
        prompt: String,
        decision: Option<String>,
        decided_by: Option<String>,
    },
    RunStart {
        run_id: String,
        workspace_id: String,
        mode: RunMode,
        started_at: DateTime<Utc>,
    },
    TurnStart,
    TurnEnd,
    Usage {
        provider: String,
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cached_tokens: u32,
    },
    RunComplete {
        status: RunStatus,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        duration_ms: u64,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionActivity {
    Idle,
    Thinking,
    Typing,
    Tool { tool: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SessionLiveUsage {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cached_tokens: u64,
    #[serde(default)]
    output_tokens_estimated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionLiveUsageRecord {
    version: u32,
    session_id: String,
    run_id: String,
    updated_at: DateTime<Utc>,
    usage: SessionLiveUsage,
}

impl SessionLiveUsageRecord {
    const VERSION: u32 = 1;
}

#[derive(Debug, Clone)]
struct SessionLiveUsageWriterState {
    usage: SessionLiveUsage,
    streamed_chars: usize,
    last_persisted: SessionLiveUsage,
    last_written_at: Option<Instant>,
}

impl SessionLiveUsageWriterState {
    fn new(provider: &str, model: &str) -> Self {
        let usage = SessionLiveUsage {
            provider: Some(provider.to_string()),
            model: Some(model.to_string()),
            ..SessionLiveUsage::default()
        };
        Self {
            last_persisted: usage.clone(),
            usage,
            streamed_chars: 0,
            last_written_at: None,
        }
    }

    fn apply_event(&mut self, event: &StreamEvent) -> bool {
        match event {
            StreamEvent::TextDelta(chunk) => {
                if chunk.is_empty() {
                    return false;
                }
                self.streamed_chars += chunk.chars().count();
                if self.usage.output_tokens_estimated || self.usage.output_tokens == 0 {
                    let next = estimate_stream_output_tokens(self.streamed_chars);
                    if next == self.usage.output_tokens && self.usage.output_tokens_estimated {
                        return false;
                    }
                    self.usage.output_tokens = next;
                    self.usage.output_tokens_estimated = true;
                    return true;
                }
                false
            }
            StreamEvent::UsageSnapshot(usage) => {
                let next = SessionLiveUsage {
                    provider: self.usage.provider.clone(),
                    model: self.usage.model.clone(),
                    input_tokens: usage.input_tokens as u64,
                    output_tokens: usage.output_tokens as u64,
                    cached_tokens: usage.cached_tokens as u64,
                    output_tokens_estimated: false,
                };
                if self.usage == next {
                    return false;
                }
                self.usage = next;
                true
            }
            StreamEvent::ToolUseStart { .. } | StreamEvent::InputJsonDelta(_) => false,
        }
    }

    fn should_flush(&self, force: bool) -> bool {
        if self.usage == self.last_persisted {
            return false;
        }
        force
            || self
                .last_written_at
                .is_none_or(|timestamp| timestamp.elapsed() >= Duration::from_millis(80))
    }

    fn mark_persisted(&mut self) {
        self.last_persisted = self.usage.clone();
        self.last_written_at = Some(Instant::now());
    }
}

#[derive(Debug, Clone)]
struct CachedSessionRows {
    width: usize,
    view_mode: LiveViewMode,
    next_is_nested: bool,
    rows: Vec<String>,
}

#[derive(Debug)]
struct SessionHistorySegment {
    start_entry: usize,
    pager: TranscriptHistoryPager,
}

const INITIAL_SESSION_HISTORY_EVENTS: usize = 120;
const SESSION_HISTORY_BACKFILL_EVENTS: usize = 160;
const SESSION_HISTORY_BACKFILL_MARGIN_ROWS: usize = 24;

struct SessionInteractiveState {
    followed_run_id: Option<String>,
    transcript_path: PathBuf,
    tailer: crate::output::TranscriptTailer,
    entries: Vec<SessionEntry>,
    entry_row_cache: Vec<Option<CachedSessionRows>>,
    history_segments: Vec<SessionHistorySegment>,
    view_mode: LiveViewMode,
    viewport: ViewportState,
    prompt_active: bool,
    input_buffer: String,
    queued_prompts: VecDeque<(usize, String)>,
    activity: SessionActivity,
    current_run_started_at: Option<DateTime<Utc>>,
    /// Authoritative accumulator. Driven only by transcript `Event::Usage`
    /// entries via `push_transcript_event`. Reset on `RunStart`. Must not
    /// be touched by the on-disk live-usage sync, or the next Event::Usage
    /// double-counts on top of the streaming estimate.
    live_usage: SessionLiveUsage,
    /// Streaming overlay for the in-flight turn — read from the on-disk
    /// `live-usage.json` file (the worker's per-chunk estimate / partial
    /// snapshot). Combined additively with `live_usage` for display, then
    /// cleared when the turn's `Event::Usage` lands so we don't keep it
    /// stacking against the now-authoritative accumulator value.
    streaming_overlay: SessionLiveUsage,
    pricing: PricingConfig,
    completion: Option<CompletionState>,
}

#[derive(Clone, Debug)]
struct CompletionState {
    /// The prefix the user has typed after the leading `/`.
    query: String,
    /// Indices into `SLASH_COMMANDS` for the filtered candidate list.
    candidates: Vec<usize>,
    /// Currently highlighted index *within* `candidates`.
    index: usize,
    /// First visible row of the 8-row window when `candidates` exceeds it.
    scroll_offset: usize,
}

impl SessionInteractiveState {
    fn empty_history_builder(view_mode: LiveViewMode) -> Self {
        Self {
            followed_run_id: None,
            transcript_path: PathBuf::new(),
            tailer: crate::output::TranscriptTailer::with_offset(PathBuf::new(), 0),
            entries: Vec::new(),
            entry_row_cache: Vec::new(),
            history_segments: Vec::new(),
            view_mode,
            viewport: ViewportState::default(),
            prompt_active: false,
            input_buffer: String::new(),
            queued_prompts: VecDeque::new(),
            activity: SessionActivity::Idle,
            current_run_started_at: None,
            live_usage: SessionLiveUsage::default(),
            streaming_overlay: SessionLiveUsage::default(),
            pricing: PricingConfig::default(),
            completion: None,
        }
    }

    fn new(
        transcript_path: PathBuf,
        followed_run_id: Option<String>,
        view_mode: LiveViewMode,
    ) -> Self {
        let mut state = Self {
            followed_run_id,
            transcript_path: transcript_path.clone(),
            tailer: crate::output::TranscriptTailer::with_offset(transcript_path.clone(), 0),
            entries: Vec::new(),
            entry_row_cache: Vec::new(),
            history_segments: Vec::new(),
            view_mode,
            viewport: ViewportState::default(),
            prompt_active: false,
            input_buffer: String::new(),
            queued_prompts: VecDeque::new(),
            activity: SessionActivity::Idle,
            current_run_started_at: None,
            live_usage: SessionLiveUsage::default(),
            streaming_overlay: SessionLiveUsage::default(),
            pricing: PricingConfig::default(),
            completion: None,
        };
        state.follow_transcript(
            transcript_path,
            state.followed_run_id.clone(),
            INITIAL_SESSION_HISTORY_EVENTS,
        );
        state
    }

    fn push_line(&mut self, status: crate::output::palette::Status, text: impl Into<String>) {
        self.push_entry(SessionEntry::Notice(SessionViewLine {
            status,
            text: text.into(),
            continuation: false,
        }));
    }

    fn push_entry(&mut self, entry: SessionEntry) -> usize {
        if !self.entries.is_empty() {
            self.invalidate_entry(self.entries.len() - 1);
        }
        self.entries.push(entry);
        self.entry_row_cache.push(None);
        self.entries.len() - 1
    }

    fn invalidate_entry(&mut self, index: usize) {
        if let Some(slot) = self.entry_row_cache.get_mut(index) {
            *slot = None;
        }
    }

    fn follow_transcript(
        &mut self,
        transcript_path: PathBuf,
        followed_run_id: Option<String>,
        preload_events: usize,
    ) {
        self.followed_run_id = followed_run_id;
        self.transcript_path = transcript_path.clone();
        let mut pager = TranscriptHistoryPager::new(&transcript_path);
        let preload = pager.load_previous(preload_events);
        self.tailer = crate::output::TranscriptTailer::with_offset(
            transcript_path.clone(),
            pager.end_offset(),
        );
        self.history_segments.push(SessionHistorySegment {
            start_entry: self.entries.len(),
            pager,
        });
        for event in &preload {
            self.push_transcript_event(event);
        }
        let (started_at, live_usage) = rebuild_live_usage_from_transcript(&transcript_path);
        self.current_run_started_at = started_at;
        self.live_usage = live_usage;
        self.activity = SessionActivity::Idle;
    }

    fn insert_entries(&mut self, at: usize, new_entries: Vec<SessionEntry>) {
        if new_entries.is_empty() {
            return;
        }
        let inserted = new_entries.len();
        if at > 0 {
            self.invalidate_entry(at - 1);
        }
        let none_cache = std::iter::repeat_with(|| None).take(inserted);
        self.entries.splice(at..at, new_entries);
        self.entry_row_cache.splice(at..at, none_cache);
        for segment in &mut self.history_segments {
            if segment.start_entry > at {
                segment.start_entry += inserted;
            }
        }
    }

    fn maybe_load_older_history(&mut self) {
        if !self
            .viewport
            .is_near_top(SESSION_HISTORY_BACKFILL_MARGIN_ROWS)
        {
            return;
        }
        self.load_older_history_batch(SESSION_HISTORY_BACKFILL_EVENTS);
    }

    fn load_all_older_history(&mut self) {
        while self.load_older_history_batch(SESSION_HISTORY_BACKFILL_EVENTS) {}
    }

    fn load_older_history_batch(&mut self, batch_events: usize) -> bool {
        let Some(segment_index) = self
            .history_segments
            .iter()
            .position(|segment| !segment.pager.exhausted())
        else {
            return false;
        };

        let insert_at = self.history_segments[segment_index].start_entry;
        let events = {
            let segment = &mut self.history_segments[segment_index];
            segment.pager.load_previous(batch_events)
        };
        if events.is_empty() {
            return false;
        }

        let entries = build_session_history_entries(self.view_mode, &events);
        self.insert_entries(insert_at, entries);
        true
    }

    fn push_transcript_event(&mut self, event: &TranscriptEvent) {
        match event {
            TranscriptEvent::RunStart {
                run_id,
                workspace_id,
                mode,
                started_at,
                ..
            } => {
                self.activity = SessionActivity::Thinking;
                self.current_run_started_at = Some(*started_at);
                self.live_usage = SessionLiveUsage::default();
                self.streaming_overlay = SessionLiveUsage::default();
                self.push_entry(SessionEntry::RunStart {
                    run_id: run_id.clone(),
                    workspace_id: workspace_id.clone(),
                    mode: *mode,
                    started_at: *started_at,
                });
            }
            TranscriptEvent::TurnStart { .. } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::TurnStart);
            }
            TranscriptEvent::AssistantDelta { content } => {
                self.push_assistant_delta(content);
            }
            TranscriptEvent::AssistantMessage { content, thinking } => {
                self.push_assistant_message(content, thinking.clone());
            }
            TranscriptEvent::ToolCall { tool, input, .. } => {
                self.activity = SessionActivity::Tool { tool: tool.clone() };
                self.push_entry(SessionEntry::ToolCall {
                    tool: tool.clone(),
                    input: input.clone(),
                });
            }
            TranscriptEvent::ToolResult {
                output,
                error,
                duration_ms,
                ..
            } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::ToolResult {
                    output: output.clone(),
                    error: error.clone(),
                    duration_ms: *duration_ms,
                });
            }
            TranscriptEvent::FileEdit { path, kind, diff } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::FileEdit {
                    path: path.clone(),
                    kind: *kind,
                    diff: diff.clone(),
                });
            }
            TranscriptEvent::CommandRun {
                argv,
                cwd,
                exit_code,
                ..
            } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::CommandRun {
                    argv: argv.clone(),
                    cwd: cwd.clone(),
                    exit_code: *exit_code,
                });
            }
            TranscriptEvent::ActionEmitted {
                kind,
                allowed,
                applied,
                reason,
                ..
            } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::ActionEmitted {
                    kind: kind.clone(),
                    allowed: *allowed,
                    applied: *applied,
                    reason: reason.clone(),
                });
            }
            TranscriptEvent::GateRequested {
                gate_id,
                prompt,
                decision,
                decided_by,
            } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::GateRequested {
                    gate_id: gate_id.clone(),
                    prompt: prompt.clone(),
                    decision: decision.clone(),
                    decided_by: decided_by.clone(),
                });
            }
            TranscriptEvent::TurnEnd { .. } => {
                self.activity = SessionActivity::Thinking;
                self.push_entry(SessionEntry::TurnEnd);
            }
            TranscriptEvent::Usage {
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_tokens,
            } => {
                self.live_usage.provider = Some(provider.clone());
                self.live_usage.model = Some(model.clone());
                self.live_usage.input_tokens += *input_tokens as u64;
                self.live_usage.output_tokens += *output_tokens as u64;
                self.live_usage.cached_tokens += *cached_tokens as u64;
                // Authoritative per-turn usage just landed. The on-disk
                // streaming estimate for THIS turn is now redundant —
                // zero the overlay so we don't keep displaying it on top
                // of the value it was estimating.
                self.streaming_overlay = SessionLiveUsage::default();
                self.push_entry(SessionEntry::Usage {
                    provider: provider.clone(),
                    model: model.clone(),
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    cached_tokens: *cached_tokens,
                });
            }
            TranscriptEvent::RunComplete {
                status,
                duration_ms,
                error,
                ..
            } => {
                self.activity = SessionActivity::Idle;
                self.current_run_started_at = None;
                // Snapshot the live usage triple at completion — gives us
                // authoritative in/out/cached for this run without relying
                // on the legacy `total_tokens` rollup field.
                let usage = self.live_usage.clone();
                self.push_entry(SessionEntry::RunComplete {
                    status: *status,
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cached_tokens: usage.cached_tokens,
                    duration_ms: *duration_ms,
                    error: error.clone(),
                });
            }
        }
    }

    fn push_user_prompt(
        &mut self,
        content: impl Into<String>,
        run_id: Option<String>,
        queued: bool,
    ) -> usize {
        self.push_entry(SessionEntry::UserPrompt {
            content: content.into(),
            run_id,
            queued,
        })
    }

    fn push_assistant_delta(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        self.activity = SessionActivity::Typing;
        let last_index = self.entries.len().checked_sub(1);
        match last_index.and_then(|index| self.entries.get_mut(index).map(|entry| (index, entry))) {
            Some((
                index,
                SessionEntry::Assistant {
                    content, streaming, ..
                },
            )) => {
                content.push_str(chunk);
                *streaming = true;
                self.invalidate_entry(index);
            }
            _ => {
                self.push_entry(SessionEntry::Assistant {
                    content: chunk.to_string(),
                    thinking: None,
                    streaming: true,
                });
            }
        }
    }

    fn push_assistant_message(&mut self, content: &str, thinking: Option<String>) {
        self.activity = SessionActivity::Thinking;
        if content.trim().is_empty() && thinking.is_none() {
            let last_index = self.last_mergeable_assistant_index();
            if let Some(index) = last_index {
                if let Some(SessionEntry::Assistant { streaming, .. }) = self.entries.get_mut(index)
                {
                    *streaming = false;
                }
                self.invalidate_entry(index);
            }
            return;
        }
        match self.last_mergeable_assistant_index() {
            Some(index) => match self.entries.get_mut(index) {
                Some(entry @ SessionEntry::Assistant { .. }) => {
                    let mut merged = false;
                    if let SessionEntry::Assistant {
                        content: existing,
                        thinking: existing_thinking,
                        streaming,
                    } = entry
                    {
                        if *streaming || existing.trim() == content.trim() {
                            *existing = content.to_string();
                            *existing_thinking = thinking.clone();
                            *streaming = false;
                            merged = true;
                        }
                    }
                    if merged {
                        self.invalidate_entry(index);
                    } else {
                        self.push_entry(SessionEntry::Assistant {
                            content: content.to_string(),
                            thinking,
                            streaming: false,
                        });
                    }
                }
                _ => {
                    self.push_entry(SessionEntry::Assistant {
                        content: content.to_string(),
                        thinking,
                        streaming: false,
                    });
                }
            },
            None => {
                self.push_entry(SessionEntry::Assistant {
                    content: content.to_string(),
                    thinking,
                    streaming: false,
                });
            }
        }
    }

    fn last_mergeable_assistant_index(&self) -> Option<usize> {
        for idx in (0..self.entries.len()).rev() {
            match self.entries.get(idx) {
                Some(SessionEntry::Usage { .. }) => continue,
                Some(SessionEntry::Assistant { .. }) => return Some(idx),
                _ => break,
            }
        }
        None
    }

    fn enqueue_prompt(&mut self, entry_index: usize, run_id: String) -> usize {
        self.queued_prompts.push_back((entry_index, run_id));
        self.queued_prompts.len()
    }

    fn sync_runtime_status(&mut self, status: SessionStatus) {
        if status == SessionStatus::Running {
            if self.activity == SessionActivity::Idle {
                self.activity = SessionActivity::Thinking;
            }
        } else {
            self.activity = SessionActivity::Idle;
        }
    }

    fn sync_live_usage_from_record(
        &mut self,
        session: &SessionRecord,
        live_usage: Option<&SessionLiveUsageRecord>,
    ) {
        // The on-disk live-usage file holds the WORKER's per-turn
        // estimate / partial snapshot for the in-flight turn. Funnel it
        // into `streaming_overlay` only — never touch `live_usage`,
        // which is the authoritative accumulator driven by transcript
        // `Event::Usage` entries. See [streaming_overlay] field docs.
        if session.status != SessionStatus::Running {
            self.streaming_overlay = SessionLiveUsage::default();
            return;
        }
        if let Some(record) = live_usage
            .filter(|record| session.active_run_id.as_deref() == Some(record.run_id.as_str()))
        {
            self.streaming_overlay = record.usage.clone();
            // Provider/model on the accumulator are unset until the first
            // transcript Event::Usage; hydrate from the on-disk record so
            // the UI knows them during streaming.
            if self.live_usage.provider.is_none() {
                self.live_usage.provider = record.usage.provider.clone();
            }
            if self.live_usage.model.is_none() {
                self.live_usage.model = record.usage.model.clone();
            }
            return;
        }
        self.streaming_overlay = SessionLiveUsage::default();
        if self.live_usage.provider.is_none() {
            self.live_usage.provider = Some(session.provider_name.clone());
        }
        if self.live_usage.model.is_none() {
            self.live_usage.model = Some(session.model.clone());
        }
    }

    /// Combined view of `live_usage + streaming_overlay`. Use this for
    /// any user-facing token display during an active turn.
    fn display_usage(&self) -> SessionLiveUsage {
        SessionLiveUsage {
            provider: self
                .live_usage
                .provider
                .clone()
                .or_else(|| self.streaming_overlay.provider.clone()),
            model: self
                .live_usage
                .model
                .clone()
                .or_else(|| self.streaming_overlay.model.clone()),
            input_tokens: self.live_usage.input_tokens + self.streaming_overlay.input_tokens,
            output_tokens: self.live_usage.output_tokens + self.streaming_overlay.output_tokens,
            cached_tokens: self.live_usage.cached_tokens + self.streaming_overlay.cached_tokens,
            // If the overlay is non-zero, the output_tokens are an estimate
            // for the in-flight turn; mark the combined value accordingly.
            output_tokens_estimated: self.streaming_overlay.output_tokens > 0
                && self.streaming_overlay.output_tokens_estimated,
        }
    }

    fn has_dynamic_render_state(&self, status: SessionStatus) -> bool {
        status == SessionStatus::Running || self.prompt_active || !self.input_buffer.is_empty()
    }

    fn mark_user_prompt_sent(&mut self, entry_index: usize) {
        if let Some(SessionEntry::UserPrompt { queued, .. }) = self.entries.get_mut(entry_index) {
            *queued = false;
            self.invalidate_entry(entry_index);
        }
    }

    fn reconcile_queued_prompts(&mut self, session: &SessionRecord) {
        let mut remaining = VecDeque::new();
        while let Some((entry_index, run_id)) = self.queued_prompts.pop_front() {
            let started_or_finished = session.active_run_id.as_deref() == Some(run_id.as_str())
                || session.last_run_id.as_deref() == Some(run_id.as_str())
                || session
                    .runs
                    .iter()
                    .find(|run| run.run_id == run_id)
                    .is_some_and(|run| run.status.is_some() || run.completed_at.is_some());
            if started_or_finished {
                self.mark_user_prompt_sent(entry_index);
            } else {
                remaining.push_back((entry_index, run_id));
            }
        }
        self.queued_prompts = remaining;
    }

    fn render_cached_entry_rows(
        &mut self,
        session: &SessionRecord,
        index: usize,
        next_is_nested: bool,
        prefs: &UiPrefs,
        width: usize,
    ) -> &[String] {
        debug_assert_eq!(
            self.entries.len(),
            self.entry_row_cache.len(),
            "entry_row_cache must stay in lockstep with entries — \
             always mutate via push_entry/insert_entry, never bare \
             entries.push"
        );
        let dynamic_entry = matches!(
            self.entries.get(index),
            Some(SessionEntry::Assistant {
                streaming: true,
                ..
            })
        );
        let cache_valid = self
            .entry_row_cache
            .get(index)
            .and_then(|slot| slot.as_ref())
            .is_some_and(|cached| {
                !dynamic_entry
                    && cached.width == width
                    && cached.view_mode == self.view_mode
                    && cached.next_is_nested == next_is_nested
            });
        if !cache_valid {
            let dynamic_detail = dynamic_entry.then(|| session_live_typing_detail(session, self));
            let rows = render_session_entry_rows(
                &self.entries[index],
                next_is_nested,
                self.view_mode,
                prefs,
                width,
                dynamic_detail.as_deref(),
            );
            self.entry_row_cache[index] = Some(CachedSessionRows {
                width,
                view_mode: self.view_mode,
                next_is_nested,
                rows,
            });
        }
        &self.entry_row_cache[index]
            .as_ref()
            .expect("cache populated")
            .rows
    }
}

fn build_session_history_entries(
    view_mode: LiveViewMode,
    events: &[TranscriptEvent],
) -> Vec<SessionEntry> {
    let mut builder = SessionInteractiveState::empty_history_builder(view_mode);
    for event in events {
        builder.push_transcript_event(event);
    }
    builder.entries
}

fn rebuild_live_usage_from_transcript(path: &Path) -> (Option<DateTime<Utc>>, SessionLiveUsage) {
    let mut started_at = None;
    let mut usage = SessionLiveUsage::default();
    let Ok(iter) = JsonlReader::iter(path) else {
        return (None, usage);
    };
    for event in iter.flatten() {
        match event {
            TranscriptEvent::RunStart {
                provider,
                model,
                started_at: run_started_at,
                ..
            } => {
                started_at = Some(run_started_at);
                usage.provider = Some(provider);
                usage.model = Some(model);
            }
            TranscriptEvent::Usage {
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_tokens,
            } => {
                usage.provider = Some(provider);
                usage.model = Some(model);
                usage.input_tokens += input_tokens as u64;
                usage.output_tokens += output_tokens as u64;
                usage.cached_tokens += cached_tokens as u64;
            }
            _ => {}
        }
    }
    (started_at, usage)
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
    pricing: PricingConfig,
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
        state.pricing = pricing;
        let mut last_rows: Vec<String> = Vec::new();

        loop {
            for event in state.tailer.drain() {
                state.push_transcript_event(&event);
            }

            let (mut session, scope) = read_session(global, session_id)?;
            ensure_active_scope(scope, "session attach")?;
            if reconcile_stale_session(&mut session) {
                write_session(global, scope, &session)?;
            }
            state.sync_runtime_status(session.status);
            let live_usage = read_session_live_usage(global, scope, session_id)?;
            state.sync_live_usage_from_record(&session, live_usage.as_ref());

            state.reconcile_queued_prompts(&session);

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
                    state.push_transcript_event(&event);
                }
                if let Some(next_path) = desired_transcript_path {
                    state.follow_transcript(
                        next_path,
                        desired_run_id.clone(),
                        INITIAL_SESSION_HISTORY_EVENTS,
                    );
                }
            }

            let rows = build_session_screen_rows(&session, &mut state, &prefs);
            if rows != last_rows || state.has_dynamic_render_state(session.status) {
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
    if state.prompt_active {
        // Slash-completion popup intercepts navigation keys when open.
        if state.completion.is_some() {
            match (key.code, key.modifiers) {
                (KeyCode::Tab, _) | (KeyCode::Down, _) => {
                    slash_completion_advance(state);
                    return Ok(AttachControl::Continue);
                }
                (KeyCode::Up, _) => {
                    slash_completion_retreat(state);
                    return Ok(AttachControl::Continue);
                }
                (KeyCode::Enter, _) => {
                    slash_completion_accept(state);
                    return Ok(AttachControl::Continue);
                }
                (KeyCode::Esc, _) => {
                    state.completion = None;
                    return Ok(AttachControl::Continue);
                }
                _ => {}
            }
        } else if matches!(key.code, KeyCode::Tab) {
            slash_completion_open(state);
            return Ok(AttachControl::Continue);
        }

        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => {
                let input = state.input_buffer.trim().to_string();
                state.prompt_active = false;
                state.input_buffer.clear();
                state.completion = None;
                if input.is_empty() {
                    return Ok(AttachControl::Continue);
                }
                return handle_session_live_input(global, session, state, prefs, input);
            }
            (KeyCode::Backspace, _) | (KeyCode::Delete, _) => {
                state.input_buffer.pop();
                if state.input_buffer.is_empty() {
                    state.prompt_active = false;
                }
                slash_completion_refilter(state);
                return Ok(AttachControl::Continue);
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                state.input_buffer.clear();
                state.prompt_active = false;
                state.completion = None;
                return Ok(AttachControl::Continue);
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                trim_last_prompt_word(&mut state.input_buffer);
                if state.input_buffer.is_empty() {
                    state.prompt_active = false;
                }
                slash_completion_refilter(state);
                return Ok(AttachControl::Continue);
            }
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                state.input_buffer.push(ch);
                slash_completion_refilter(state);
                return Ok(AttachControl::Continue);
            }
            _ => {}
        }
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT)
            if should_begin_session_prompt(ch, session.status) =>
        {
            state.prompt_active = true;
            state.input_buffer.push(ch);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            state.view_mode = state.view_mode.toggled();
            rebuild_session_transcript_lines(state, prefs);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Up, _) => {
            state.viewport.scroll_up(1);
            state.maybe_load_older_history();
            Ok(AttachControl::Continue)
        }
        (KeyCode::Down, _) => {
            state.viewport.scroll_down(1);
            Ok(AttachControl::Continue)
        }
        (KeyCode::PageUp, _) => {
            state.viewport.page_up();
            state.maybe_load_older_history();
            Ok(AttachControl::Continue)
        }
        (KeyCode::PageDown, _) => {
            state.viewport.page_down();
            Ok(AttachControl::Continue)
        }
        (KeyCode::Home, _) => {
            state.load_all_older_history();
            state.viewport.jump_top();
            Ok(AttachControl::Continue)
        }
        (KeyCode::End, _) => {
            state.viewport.jump_bottom();
            Ok(AttachControl::Continue)
        }
        (KeyCode::Char('d'), KeyModifiers::CONTROL)
        | (KeyCode::Char(']'), KeyModifiers::CONTROL) => {
            Ok(AttachControl::Exit(AttachExit::Detach))
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Ok(AttachControl::Exit(AttachExit::Quit)),
        (KeyCode::F(1), _) => {
            append_session_help_lines(state);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Enter, _) => {
            if session.status == SessionStatus::Stopped {
                state.push_line(
                    crate::output::palette::Status::Failed,
                    "session stopped  ·  use `rupu session start` to create a new one",
                );
                return Ok(AttachControl::Continue);
            }
            state.prompt_active = true;
            Ok(AttachControl::Continue)
        }
        (KeyCode::Esc, _) => {
            if session.status == SessionStatus::Running {
                let detail = cancel_active_turn_in_place(global, &session.session_id)?
                    .unwrap_or_else(|| "no active turn".into());
                state.push_line(
                    crate::output::palette::Status::Awaiting,
                    format!(
                        "cancel requested  ·  {detail}  ·  press Enter once the session is idle to send the next prompt"
                    ),
                );
                return Ok(AttachControl::Continue);
            }
            Ok(AttachControl::Continue)
        }
        _ => Ok(AttachControl::Continue),
    }
}

fn rebuild_session_transcript_lines(_state: &mut SessionInteractiveState, _prefs: &UiPrefs) {}

fn handle_session_live_input(
    global: &Path,
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    prefs: &UiPrefs,
    input: String,
) -> anyhow::Result<AttachControl> {
    if let Some(command) = parse_attach_command(&input) {
        return execute_session_live_command(global, session, state, prefs, command);
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
    let queued = session.status == SessionStatus::Running;
    let entry_index = state.push_user_prompt(prompt.clone(), None, queued);
    let run_id = launch_turn_detached(global, &session.session_id, prompt)?;
    if let Some(SessionEntry::UserPrompt {
        run_id: entry_run_id,
        ..
    }) = state.entries.get_mut(entry_index)
    {
        *entry_run_id = Some(run_id.clone());
    }
    if queued {
        let _position = state.enqueue_prompt(entry_index, run_id);
    }
    Ok(AttachControl::Continue)
}

fn execute_session_live_command(
    global: &Path,
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    prefs: &UiPrefs,
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
        AttachCommand::Routed(ref routed) => {
            execute_routed_session_command(global, session, state, prefs, routed)?
        }
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
    prefs: &UiPrefs,
    width: usize,
    height: usize,
) -> Vec<String> {
    let mut rows = vec![
        render_session_header_line(session, state, width),
        String::new(),
    ];

    let footer_reserved = 1usize;
    let completion_rows = render_session_completion_rows(state, width);
    let completion_reserved = completion_rows.len();
    let available_event_rows = height
        .saturating_sub(rows.len())
        .saturating_sub(footer_reserved)
        .saturating_sub(completion_reserved)
        .max(1);
    let event_rows = render_session_event_rows(session, state, prefs, width, available_event_rows);
    rows.extend(event_rows);
    while rows.len() < height.saturating_sub(footer_reserved + completion_reserved) {
        rows.push(String::new());
    }
    rows.extend(completion_rows);
    rows.push(render_session_prompt_line(session, state, width));
    rows.truncate(height);
    rows
}

/// Time-phase for the painted prompt caret. Toggles every ~530ms (matches
/// macOS NSTextView cadence). Source of liveness for the redraw loop: when
/// the phase flips, the prompt row text differs from `last_rows` and the
/// caller re-renders.
fn session_caret_visible() -> bool {
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| (duration.as_millis() / 530) as u64)
        .unwrap_or(0);
    tick % 2 == 0
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
    state: &SessionInteractiveState,
    width: usize,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(
        &mut buf,
        &session_prompt_glyph(session.status).to_string(),
        session.status.ui_status().color(),
    );
    buf.push(' ');
    let _ = palette::write_bold_colored(
        &mut buf,
        &truncate_single_line(&session_header_label(session), 32),
        BRAND,
    );

    let mut parts = vec![
        session.status.as_str().to_string(),
        truncate_single_line(&session.model, 24),
    ];
    if let Some(effort) = session_effort_detail(session) {
        parts.push(effort);
    }
    parts.push(session_session_totals_detail(session));
    if let Some(cost) = session_total_cost_detail(session, state) {
        parts.push(cost);
    }
    let queued_runs = session_pending_run_count(session);
    if queued_runs > 0 {
        parts.push(format!("queued {queued_runs}"));
    }
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, &parts.join("  ·  "), DIM);
    truncate_ansi_line(&buf, width)
}

fn render_session_completion_rows(state: &SessionInteractiveState, width: usize) -> Vec<String> {
    let Some(c) = state.completion.as_ref() else {
        return Vec::new();
    };
    if c.candidates.is_empty() {
        return Vec::new();
    }

    let window = SLASH_COMPLETION_VISIBLE_ROWS;
    let visible_count = c.candidates.len().min(window);
    let start = c
        .scroll_offset
        .min(c.candidates.len().saturating_sub(visible_count));
    let end = (start + visible_count).min(c.candidates.len());
    let visible = &c.candidates[start..end];

    // Pad name column so descriptions align.
    let name_width = visible
        .iter()
        .map(|&idx| SLASH_COMMANDS[idx].name.len() + 1) // +1 for leading '/'
        .max()
        .unwrap_or(0);

    let mut rows = Vec::with_capacity(visible.len() + 1);
    for (vi, &cmd_idx) in visible.iter().enumerate() {
        let absolute_idx = start + vi;
        let cmd = &SLASH_COMMANDS[cmd_idx];
        let mut row = String::new();
        let marker = if absolute_idx == c.index {
            "▸ "
        } else {
            "  "
        };
        let _ = palette::write_colored(&mut row, marker, BRAND);
        let name = format!("/{}", cmd.name);
        let padded = format!("{name:<name_width$}");
        let _ = palette::write_colored(&mut row, &padded, BRAND);
        row.push_str("  ");
        let _ = palette::write_colored(&mut row, cmd.description, DIM);
        rows.push(truncate_ansi_line(&row, width));
    }

    let hidden = c.candidates.len() - end;
    if hidden > 0 {
        let mut row = String::new();
        let _ = palette::write_colored(&mut row, &format!("  ↓ +{hidden} more"), DIM);
        rows.push(truncate_ansi_line(&row, width));
    }

    rows
}

fn render_session_prompt_line(
    session: &SessionRecord,
    state: &SessionInteractiveState,
    width: usize,
) -> String {
    let mut buf = String::new();
    let prompt_status = session.status.ui_status();
    let _ = palette::write_bold_colored(
        &mut buf,
        &session_prompt_glyph(session.status).to_string(),
        prompt_status.color(),
    );
    buf.push(' ');
    let _ = palette::write_bold_colored(
        &mut buf,
        &truncate_single_line(&session.agent_name, 20),
        BRAND,
    );
    let _ = palette::write_colored(&mut buf, " > ", DIM);
    if !state.input_buffer.is_empty() {
        let _ = palette::write_colored(&mut buf, &state.input_buffer, DIM);
    }
    // Painted caret right after the input position. Half-block glyph in the
    // brand color, toggled every ~530ms by `session_caret_visible()`. The
    // attach loop redraws when row text changes, so the phase flip drives
    // the visible blink.
    if session_caret_visible() {
        let _ = palette::write_bold_colored(&mut buf, "▏", BRAND);
    } else {
        buf.push(' ');
    }
    if state.input_buffer.is_empty() {
        let queued_runs = session_pending_run_count(session);
        if queued_runs > 0 {
            let _ = palette::write_colored(&mut buf, &format!(" queued {queued_runs}"), DIM);
        }
    }
    truncate_ansi_line(&buf, width)
}

fn session_prompt_glyph(status: SessionStatus) -> char {
    match status {
        SessionStatus::Running => session_live_frame(),
        SessionStatus::Idle => '●',
        SessionStatus::Failed => '✗',
        SessionStatus::Stopped => '⏸',
    }
}

fn session_live_frame() -> char {
    const FRAMES: [char; 4] = ['◐', '◓', '◑', '◒'];
    let tick = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| (duration.as_millis() / 160) as usize)
        .unwrap_or(0);
    FRAMES[tick % FRAMES.len()]
}

fn session_live_elapsed(
    session: &SessionRecord,
    state: &SessionInteractiveState,
) -> Option<String> {
    let started_at = state.current_run_started_at.or_else(|| {
        let active_run_id = session.active_run_id.as_deref()?;
        session
            .runs
            .iter()
            .rev()
            .find(|run| run.run_id == active_run_id)
            .map(|run| run.started_at)
    })?;
    let seconds = (Utc::now() - started_at).num_seconds().max(0) as u64;
    Some(format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    ))
}

fn session_live_status_detail(session: &SessionRecord, state: &SessionInteractiveState) -> String {
    let label = match &state.activity {
        SessionActivity::Idle | SessionActivity::Thinking => "thinking".to_string(),
        SessionActivity::Typing => "typing".to_string(),
        SessionActivity::Tool { tool } => {
            format!("tool {}", truncate_single_line(tool, 18))
        }
    };
    session_live_status_detail_parts(Some(session), state, &label, true)
}

fn session_live_status_detail_parts(
    session: Option<&SessionRecord>,
    state: &SessionInteractiveState,
    label: &str,
    include_usage: bool,
) -> String {
    let mut detail = format!("{label} {}", session_live_frame());
    if let Some(elapsed) = session.and_then(|session| session_live_elapsed(session, state)) {
        detail.push_str("  ·  ");
        detail.push_str(&elapsed);
    }
    let usage = state.display_usage();
    if include_usage {
        let output_count = if usage.output_tokens_estimated {
            format!("~{}", format_token_count(usage.output_tokens))
        } else {
            format_token_count(usage.output_tokens)
        };
        detail.push_str("  ·  ");
        let _ = palette::write_colored(&mut detail, session_upload_indicator(), BRAND);
        detail.push_str(&format!(" {}  ", format_token_count(usage.input_tokens)));
        let _ = palette::write_colored(&mut detail, session_download_indicator(), BRAND);
        detail.push_str(&format!(" {output_count}  "));
        let _ = palette::write_colored(&mut detail, session_cache_indicator(), BRAND);
        detail.push_str(&format!(" {}", format_token_count(usage.cached_tokens)));
    }
    detail
}

fn session_live_typing_detail(session: &SessionRecord, state: &SessionInteractiveState) -> String {
    session_live_status_detail_parts(Some(session), state, "typing", true)
}

fn session_upload_indicator() -> &'static str {
    "⇡"
}

fn session_download_indicator() -> &'static str {
    "⇣"
}

fn session_cache_indicator() -> &'static str {
    "⟳"
}

fn estimate_stream_output_tokens(streamed_chars: usize) -> u64 {
    if streamed_chars == 0 {
        0
    } else {
        ((streamed_chars as u64) + 3) / 4
    }
}

fn should_begin_session_prompt(ch: char, status: SessionStatus) -> bool {
    if matches!(status, SessionStatus::Stopped) {
        return false;
    }
    !ch.is_control()
}

fn trim_last_prompt_word(buffer: &mut String) {
    while buffer.chars().last().is_some_and(|ch| ch.is_whitespace()) {
        buffer.pop();
    }
    while buffer.chars().last().is_some_and(|ch| !ch.is_whitespace()) {
        buffer.pop();
    }
}

fn render_session_event_rows(
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    prefs: &UiPrefs,
    width: usize,
    max_rows: usize,
) -> Vec<String> {
    state.maybe_load_older_history();
    let mut entry_counts = Vec::with_capacity(state.entries.len());
    let mut total_rows = 0usize;
    for index in 0..state.entries.len() {
        let next_is_nested = session_entry_is_nested(state.entries.get(index + 1));
        let count = state
            .render_cached_entry_rows(session, index, next_is_nested, prefs, width)
            .len();
        entry_counts.push(count);
        total_rows += count;
    }

    let synthetic_rows = render_synthetic_session_activity_rows(session, state, width);
    total_rows += synthetic_rows.len();
    let bounds = state.viewport.window(total_rows, max_rows);

    let mut rows = Vec::with_capacity(max_rows.min(total_rows));
    let mut cursor = 0usize;
    for (index, count) in entry_counts.into_iter().enumerate() {
        let next_cursor = cursor + count;
        if next_cursor <= bounds.start {
            cursor = next_cursor;
            continue;
        }
        if cursor >= bounds.end {
            break;
        }
        let next_is_nested = session_entry_is_nested(state.entries.get(index + 1));
        let rendered = state.render_cached_entry_rows(session, index, next_is_nested, prefs, width);
        let slice_start = bounds.start.saturating_sub(cursor).min(rendered.len());
        let slice_end = bounds.end.saturating_sub(cursor).min(rendered.len());
        rows.extend(rendered[slice_start..slice_end].iter().cloned());
        cursor = next_cursor;
    }

    if cursor < bounds.end && !synthetic_rows.is_empty() {
        let slice_start = bounds
            .start
            .saturating_sub(cursor)
            .min(synthetic_rows.len());
        let slice_end = bounds.end.saturating_sub(cursor).min(synthetic_rows.len());
        rows.extend(synthetic_rows[slice_start..slice_end].iter().cloned());
    }

    rows
}

fn render_synthetic_session_activity_rows(
    session: &SessionRecord,
    state: &SessionInteractiveState,
    width: usize,
) -> Vec<String> {
    if session.status != SessionStatus::Running {
        return Vec::new();
    }
    match &state.activity {
        SessionActivity::Thinking => render_role_header(
            "agent",
            crate::output::palette::Status::Working.color(),
            Some((
                crate::output::palette::Status::Working.glyph(),
                crate::output::palette::Status::Working.color(),
            )),
            Some(&session_live_status_detail(session, state)),
            width,
        ),
        SessionActivity::Idle | SessionActivity::Typing | SessionActivity::Tool { .. } => {
            Vec::new()
        }
    }
}

fn render_session_entry_rows(
    entry: &SessionEntry,
    next_is_nested: bool,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
    width: usize,
    live_detail: Option<&str>,
) -> Vec<String> {
    use crate::output::palette::Status;

    match entry {
        SessionEntry::Notice(line) => render_notice_rows(line, width),
        SessionEntry::UserPrompt {
            content, queued, ..
        } => {
            let normalized = sanitize_terminal_text(content);
            let mut rows = render_role_header(
                "you",
                BRAND,
                Some(('▸', BRAND)),
                if *queued { Some("queued") } else { None },
                width,
            );
            rows.extend(render_role_body(
                &normalized
                    .split('\n')
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
                BRAND,
                width,
            ));
            rows
        }
        SessionEntry::Assistant {
            content,
            thinking,
            streaming,
        } => {
            let streaming_detail = if *streaming { live_detail } else { None };
            let role_color = if *streaming {
                Status::Working.color()
            } else {
                Status::Active.color()
            };
            let role_glyph = if *streaming {
                Status::Working.glyph()
            } else {
                Status::Active.glyph()
            };
            let mut rows = render_role_header(
                "agent",
                role_color,
                Some((role_glyph, role_color)),
                streaming_detail.as_deref(),
                width,
            );
            if let Some(thinking) = thinking.as_deref().filter(|value| !value.trim().is_empty()) {
                if view_mode == LiveViewMode::Full {
                    rows.extend(render_role_body(
                        &[retained_session_event_line(
                            Status::Active,
                            "thinking",
                            &truncate_single_line(thinking, 96),
                        )],
                        role_color,
                        width,
                    ));
                }
            }
            if !content.trim().is_empty() {
                let payload = render_assistant_content(content.trim(), prefs);
                let body_lines = match view_mode {
                    LiveViewMode::Focused => preview_rendered_lines(&payload.rendered, 4),
                    LiveViewMode::Compact | LiveViewMode::Full => payload
                        .rendered
                        .lines()
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                };
                rows.extend(render_role_body(&body_lines, role_color, width));
            }
            rows
        }
        SessionEntry::ToolCall { tool, input } => {
            let mut rows = render_nested_event_rows(
                next_is_nested,
                Status::Working,
                retained_session_event_line(
                    Status::Working,
                    &format!("tool {tool}"),
                    &transcript_tool_summary(tool, input),
                ),
                width,
            );
            match view_mode {
                LiveViewMode::Focused => {}
                LiveViewMode::Compact => {
                    if let Some(rendered) = render_tool_input(tool, input, prefs) {
                        rows.extend(render_nested_body_lines(
                            next_is_nested,
                            &preview_rendered_lines(&rendered, 5),
                            width,
                        ));
                    }
                }
                LiveViewMode::Full => {
                    if let Some(rendered) = render_tool_input(tool, input, prefs) {
                        rows.extend(render_nested_body_lines(
                            next_is_nested,
                            &rendered.lines().map(str::to_string).collect::<Vec<_>>(),
                            width,
                        ));
                    }
                }
            }
            rows
        }
        SessionEntry::ToolResult {
            output,
            error,
            duration_ms,
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
            let mut rows = render_nested_event_rows(
                next_is_nested,
                status,
                retained_session_event_line(
                    status,
                    label,
                    &session_payload_summary(&payload, *duration_ms),
                ),
                width,
            );
            match view_mode {
                LiveViewMode::Focused => {}
                LiveViewMode::Compact => {
                    if error.is_some() {
                        rows.extend(render_nested_body_lines(
                            next_is_nested,
                            &render_payload_preview_lines(&payload, 3),
                            width,
                        ));
                    }
                }
                LiveViewMode::Full => {
                    rows.extend(render_nested_body_lines(
                        next_is_nested,
                        &session_rich_payload_lines(&payload),
                        width,
                    ));
                }
            }
            rows
        }
        SessionEntry::FileEdit { path, kind, diff } => {
            let payload = render_payload(diff, prefs);
            let mut rows = render_nested_event_rows(
                next_is_nested,
                Status::Complete,
                retained_session_event_line(
                    Status::Complete,
                    "file edit",
                    &format!(
                        "{} {}  ·  {}",
                        format!("{kind:?}").to_lowercase(),
                        path,
                        payload.headline
                    ),
                ),
                width,
            );
            match view_mode {
                LiveViewMode::Focused => {}
                LiveViewMode::Compact => rows.extend(render_nested_body_lines(
                    next_is_nested,
                    &render_payload_preview_lines(&payload, 8),
                    width,
                )),
                LiveViewMode::Full => rows.extend(render_nested_body_lines(
                    next_is_nested,
                    &session_rich_payload_lines(&payload),
                    width,
                )),
            }
            rows
        }
        SessionEntry::CommandRun {
            argv,
            cwd,
            exit_code,
        } => render_nested_event_rows(
            next_is_nested,
            if *exit_code == 0 {
                Status::Complete
            } else {
                Status::Failed
            },
            retained_session_event_line(
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
            width,
        ),
        SessionEntry::ActionEmitted {
            kind,
            allowed,
            applied,
            reason,
        } => {
            let status = if *applied {
                Status::Complete
            } else if *allowed {
                Status::Awaiting
            } else {
                Status::Failed
            };
            let mut detail = format!("action  ·  {kind}  ·  allowed={allowed} applied={applied}");
            if let Some(reason) = reason.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(reason, 64));
            }
            render_nested_event_rows(
                next_is_nested,
                status,
                retained_session_event_line_raw(status, "action", &detail),
                width,
            )
        }
        SessionEntry::GateRequested {
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
            render_nested_event_rows(
                next_is_nested,
                Status::Awaiting,
                retained_session_event_line_raw(Status::Awaiting, "approval gate", &detail),
                width,
            )
        }
        SessionEntry::RunStart {
            run_id,
            workspace_id,
            mode,
            started_at,
        } => {
            if view_mode != LiveViewMode::Full {
                return Vec::new();
            }
            render_notice_rows(
                &SessionViewLine {
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
                },
                width,
            )
        }
        SessionEntry::TurnStart | SessionEntry::TurnEnd => Vec::new(),
        SessionEntry::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => {
            if view_mode == LiveViewMode::Focused {
                return Vec::new();
            }
            render_notice_rows(
                &SessionViewLine {
                    status: Status::Active,
                    text: retained_session_event_line_raw(
                        Status::Active,
                        "usage",
                        &format!(
                            "{provider} · {model}  ·  in {input_tokens} out {output_tokens} cached {cached_tokens}"
                        ),
                    ),
                    continuation: false,
                },
                width,
            )
        }
        SessionEntry::RunComplete {
            status,
            input_tokens,
            output_tokens,
            cached_tokens,
            duration_ms,
            error,
        } => {
            let status_color = match status {
                RunStatus::Ok => Status::Complete,
                RunStatus::Error | RunStatus::Aborted => Status::Failed,
            };
            let mut detail = format!(
                "status {}  ·  {}ms  ·  ",
                format!("{status:?}").to_lowercase(),
                duration_ms
            );
            let _ = palette::write_colored(&mut detail, session_upload_indicator(), BRAND);
            detail.push_str(&format!(" {}  ", format_token_count(*input_tokens)));
            let _ = palette::write_colored(&mut detail, session_download_indicator(), BRAND);
            detail.push_str(&format!(" {}  ", format_token_count(*output_tokens)));
            let _ = palette::write_colored(&mut detail, session_cache_indicator(), BRAND);
            detail.push_str(&format!(" {}", format_token_count(*cached_tokens)));
            if let Some(error) = error.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(error, 72));
            }
            render_notice_rows(
                &SessionViewLine {
                    status: status_color,
                    text: retained_session_event_line_raw(status_color, "run complete", &detail),
                    continuation: false,
                },
                width,
            )
        }
    }
}

fn render_role_header(
    label: &str,
    color: owo_colors::Rgb,
    glyph: Option<(char, owo_colors::Rgb)>,
    detail: Option<&str>,
    width: usize,
) -> Vec<String> {
    let mut header = String::new();
    if let Some((ch, glyph_color)) = glyph {
        let _ = palette::write_bold_colored(&mut header, &ch.to_string(), glyph_color);
        header.push(' ');
        header.push(' ');
    }
    let _ = palette::write_bold_colored(&mut header, label, color);
    if let Some(detail) = detail {
        let _ = palette::write_colored(&mut header, "  ·  ", DIM);
        let _ = palette::write_colored(&mut header, detail, DIM);
    }
    vec![truncate_ansi_line(&header, width)]
}

fn render_role_body(lines: &[String], color: owo_colors::Rgb, width: usize) -> Vec<String> {
    let mut prefix = String::new();
    let _ = palette::write_colored(&mut prefix, "│", color);
    prefix.push(' ');
    let mut rows = Vec::new();
    for line in lines {
        rows.extend(wrap_prefixed_lines(line, &prefix, &prefix, width));
    }
    rows
}

fn render_notice_rows(line: &SessionViewLine, width: usize) -> Vec<String> {
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
    wrap_prefixed_lines(&line.text, &prefix, "  ", width)
}

fn render_indented_body_lines(lines: &[String], width: usize, prefix: &str) -> Vec<String> {
    let mut rows = Vec::new();
    for line in lines {
        rows.extend(wrap_prefixed_lines(line, prefix, prefix, width));
    }
    rows
}

fn render_nested_event_rows(
    next_is_nested: bool,
    status: crate::output::palette::Status,
    text: String,
    width: usize,
) -> Vec<String> {
    let (first_prefix, continuation_prefix) = nested_prefixes(next_is_nested);
    let mut prefix = String::new();
    prefix.push_str(&first_prefix);
    let _ = palette::write_bold_colored(&mut prefix, &status.glyph().to_string(), status.color());
    prefix.push(' ');
    wrap_prefixed_lines(&text, &prefix, &continuation_prefix, width)
}

fn render_nested_body_lines(next_is_nested: bool, lines: &[String], width: usize) -> Vec<String> {
    let (_, continuation_prefix) = nested_prefixes(next_is_nested);
    render_indented_body_lines(lines, width, &continuation_prefix)
}

fn nested_prefixes(next_is_nested: bool) -> (String, String) {
    if next_is_nested {
        ("  ├─ ".to_string(), "  │  ".to_string())
    } else {
        ("  └─ ".to_string(), "     ".to_string())
    }
}

fn session_entry_is_nested(entry: Option<&SessionEntry>) -> bool {
    matches!(
        entry,
        Some(
            SessionEntry::ToolCall { .. }
                | SessionEntry::ToolResult { .. }
                | SessionEntry::FileEdit { .. }
                | SessionEntry::CommandRun { .. }
                | SessionEntry::ActionEmitted { .. }
                | SessionEntry::GateRequested { .. }
        )
    )
}

fn wrap_prefixed_lines(
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
) -> Vec<String> {
    let content_width = width.saturating_sub(visible_len(first_prefix)).max(1);
    let wrapped = wrap_block_with_ansi(text, content_width);
    let mut rows = Vec::new();
    for (idx, segment) in wrapped.into_iter().enumerate() {
        rows.push(format!(
            "{}{}",
            if idx == 0 {
                first_prefix
            } else {
                continuation_prefix
            },
            segment
        ));
    }
    rows
}

fn preview_rendered_lines(rendered: &str, max_lines: usize) -> Vec<String> {
    let max_lines = max_lines.max(1);
    let all_lines = rendered.lines().map(str::to_string).collect::<Vec<_>>();
    if all_lines.is_empty() {
        return Vec::new();
    }
    let total = all_lines.len();
    let shown = total.min(max_lines);
    let mut out = all_lines.into_iter().take(shown).collect::<Vec<_>>();
    if total > shown {
        let hidden = total - shown;
        let mut trailer = String::new();
        let _ = palette::write_colored(&mut trailer, &format!("… +{hidden} more line(s)"), DIM);
        out.push(trailer);
    }
    out
}

fn execute_routed_session_command(
    _global: &Path,
    session: &SessionRecord,
    state: &mut SessionInteractiveState,
    prefs: &UiPrefs,
    command: &RoutedAttachCommand,
) -> anyhow::Result<()> {
    let mut argv = vec![command.root.clone()];
    argv.extend(command.args.clone());
    resolve_inline_session_aliases(session, state, &mut argv);
    if command_supports_view(&argv) && !argv.iter().any(|value| value == "--view") {
        argv.push("--view".into());
        argv.push(state.view_mode.as_str().into());
    }
    if !argv.iter().any(|value| value == "--no-pager") {
        argv.push("--no-pager".into());
    }
    if !prefs.use_color() && !argv.iter().any(|value| value == "--no-color") {
        argv.push("--no-color".into());
    }

    state.push_line(
        crate::output::palette::Status::Active,
        retained_session_event_line(
            crate::output::palette::Status::Active,
            "command",
            &format!("/{}", command.display),
        ),
    );

    let output = Command::new(std::env::current_exe()?)
        .args(&argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("run inline session command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = if output.status.success() {
        crate::output::palette::Status::Complete
    } else {
        crate::output::palette::Status::Failed
    };

    for line in stdout.lines() {
        // Must go through push_entry so entry_row_cache stays the same
        // length as entries — render_cached_entry_rows indexes the cache
        // directly and panics if it's shorter than entries.
        state.push_entry(SessionEntry::Notice(SessionViewLine {
            status,
            text: line.to_string(),
            continuation: true,
        }));
    }
    for line in stderr.lines() {
        state.push_entry(SessionEntry::Notice(SessionViewLine {
            status: crate::output::palette::Status::Failed,
            text: line.to_string(),
            continuation: true,
        }));
    }
    if stdout.is_empty() && stderr.is_empty() {
        state.push_line(status, "command completed");
    }
    Ok(())
}

fn resolve_inline_session_aliases(
    session: &SessionRecord,
    state: &SessionInteractiveState,
    argv: &mut Vec<String>,
) {
    if argv.is_empty() {
        return;
    }
    match argv[0].as_str() {
        "session" if argv.get(1).is_some_and(|value| value == "show") => {
            if argv.get(2).is_none() || argv.get(2).is_some_and(|value| value == "current") {
                argv.insert(2, session.session_id.clone());
            }
        }
        "transcript" if argv.get(1).is_some_and(|value| value == "show") => {
            if let Some(run_id) = inline_session_run_alias(session, state, argv.get(2)) {
                if argv.get(2).is_some() {
                    argv[2] = run_id;
                } else {
                    argv.push(run_id);
                }
            }
        }
        "workflow" if argv.get(1).is_some_and(|value| value == "show-run") => {
            if let Some(run_id) = inline_session_run_alias(session, state, argv.get(2)) {
                if argv.get(2).is_some() {
                    argv[2] = run_id;
                } else {
                    argv.push(run_id);
                }
            }
        }
        _ => {}
    }
}

fn inline_session_run_alias(
    session: &SessionRecord,
    state: &SessionInteractiveState,
    value: Option<&String>,
) -> Option<String> {
    let run_id = session
        .active_run_id
        .as_ref()
        .or(state.followed_run_id.as_ref())
        .or(session.last_run_id.as_ref())?;
    match value.map(|value| value.as_str()) {
        None | Some("current") | Some("last") => Some(run_id.clone()),
        Some(_) => None,
    }
}

fn command_supports_view(argv: &[String]) -> bool {
    matches!(
        argv,
        [root, sub, ..]
            if (root == "session" && sub == "show")
                || (root == "transcript" && sub == "show")
                || (root == "workflow" && (sub == "show" || sub == "show-run"))
    )
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
        TranscriptEvent::AssistantDelta { .. } => Vec::new(),
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
                            "assistant",
                            &truncate_single_line(content, 96),
                        ),
                        continuation: false,
                    }),
                    LiveViewMode::Compact | LiveViewMode::Full => {
                        let highlighted = render_assistant_content(content.trim(), prefs).rendered;
                        let mut lines = highlighted.split('\n');
                        if let Some(first) = lines.next() {
                            out.push(SessionViewLine {
                                status: Status::Active,
                                text: retained_session_event_line_raw(
                                    Status::Active,
                                    "assistant",
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
            LiveViewMode::Focused => vec![SessionViewLine {
                status: Status::Working,
                text: retained_session_event_line(
                    Status::Working,
                    &format!("tool {tool}"),
                    &transcript_tool_summary(tool, input),
                ),
                continuation: false,
            }],
            LiveViewMode::Compact => {
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
                    for line in preview_rendered_lines(&rendered, 5) {
                        out.push(SessionViewLine {
                            status: Status::Working,
                            text: line,
                            continuation: true,
                        });
                    }
                }
                out
            }
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
                LiveViewMode::Focused => vec![SessionViewLine {
                    status,
                    text: retained_session_event_line(
                        status,
                        label,
                        &session_payload_summary(&payload, *duration_ms),
                    ),
                    continuation: false,
                }],
                LiveViewMode::Compact => {
                    let mut out = vec![SessionViewLine {
                        status,
                        text: retained_session_event_line(
                            status,
                            label,
                            &session_payload_summary(&payload, *duration_ms),
                        ),
                        continuation: false,
                    }];
                    if error.is_some() {
                        for line in render_payload_preview_lines(&payload, 3) {
                            out.push(SessionViewLine {
                                status,
                                text: line,
                                continuation: true,
                            });
                        }
                    }
                    out
                }
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
            LiveViewMode::Focused => vec![SessionViewLine {
                status: Status::Complete,
                text: retained_session_event_line(
                    Status::Complete,
                    "file edit",
                    &format!("{} {}", format!("{kind:?}").to_lowercase(), path),
                ),
                continuation: false,
            }],
            LiveViewMode::Compact => {
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
                for line in render_payload_preview_lines(&payload, 8) {
                    out.push(SessionViewLine {
                        status: Status::Complete,
                        text: line,
                        continuation: true,
                    });
                }
                out
            }
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
        "help  ·  type or /command  ·  Tab complete /  ·  Enter send or queue  ·  Ctrl-U clear  ·  Ctrl-W word  ·  Esc cancel turn  ·  Ctrl-F cycle view  ·  ↑/↓ scroll  ·  Ctrl-D detach  ·  Ctrl-C quit  ·  F1 help",
    );
    state.push_line(
        crate::output::palette::Status::Active,
        "commands  ·  /help /status /history /runs /transcript /cancel /stop /detach /quit /workflow ... /session ... /issues ...  ·  prompts queue while a turn is running",
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
        (KeyCode::Char('d'), KeyModifiers::CONTROL)
        | (KeyCode::Char(']'), KeyModifiers::CONTROL) => {
            Ok(AttachControl::Exit(AttachExit::Detach))
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Ok(AttachControl::Exit(AttachExit::Quit)),
        (KeyCode::Esc, _) => {
            if session.status == SessionStatus::Running {
                cancel_active_turn(global, &session.session_id, printer)?;
                printer.sideband_event(
                    crate::output::palette::Status::Awaiting,
                    "cancel requested",
                    Some("press Enter once the session is idle to send the next prompt"),
                );
                return Ok(AttachControl::Continue);
            }
            Ok(AttachControl::Continue)
        }
        (KeyCode::F(1), _) => {
            render_attach_help(printer);
            Ok(AttachControl::Continue)
        }
        (KeyCode::Enter, _) => {
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
                    return handle_session_input(global, session, printer, input);
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

struct SlashCommand {
    name: &'static str,
    description: &'static str,
    routed: bool,
}

const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "cancel",
        description: "cancel the active run",
        routed: false,
    },
    SlashCommand {
        name: "detach",
        description: "leave the live view (keep session running)",
        routed: false,
    },
    SlashCommand {
        name: "help",
        description: "show available commands",
        routed: false,
    },
    SlashCommand {
        name: "history",
        description: "replay prompt history",
        routed: false,
    },
    SlashCommand {
        name: "issues",
        description: "(routed) issues show / list",
        routed: true,
    },
    SlashCommand {
        name: "quit",
        description: "quit and stop the session",
        routed: false,
    },
    SlashCommand {
        name: "runs",
        description: "list runs in this session",
        routed: false,
    },
    SlashCommand {
        name: "session",
        description: "(routed) session show / list",
        routed: true,
    },
    SlashCommand {
        name: "status",
        description: "session status detail",
        routed: false,
    },
    SlashCommand {
        name: "stop",
        description: "stop the worker",
        routed: false,
    },
    SlashCommand {
        name: "transcript",
        description: "show the current transcript",
        routed: false,
    },
    SlashCommand {
        name: "workflow",
        description: "(routed) workflow show / list / show-run",
        routed: true,
    },
];

const SLASH_COMPLETION_VISIBLE_ROWS: usize = 8;

fn slash_completion_candidates(query: &str) -> Vec<usize> {
    let needle = query.to_lowercase();
    SLASH_COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, cmd)| cmd.name.starts_with(&needle))
        .map(|(idx, _)| idx)
        .collect()
}

fn slash_completion_query(buffer: &str) -> Option<String> {
    let trimmed = buffer.strip_prefix('/')?;
    // Argument completion is out of scope; once the user types a space we
    // assume they've moved past the root and the popup closes.
    if trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    Some(trimmed.to_string())
}

fn slash_completion_open(state: &mut SessionInteractiveState) {
    let Some(query) = slash_completion_query(&state.input_buffer) else {
        return;
    };
    let candidates = slash_completion_candidates(&query);
    if candidates.is_empty() {
        return;
    }
    state.completion = Some(CompletionState {
        query,
        candidates,
        index: 0,
        scroll_offset: 0,
    });
}

fn slash_completion_refilter(state: &mut SessionInteractiveState) {
    if state.completion.is_none() {
        return;
    }
    let Some(query) = slash_completion_query(&state.input_buffer) else {
        state.completion = None;
        return;
    };
    let candidates = slash_completion_candidates(&query);
    if candidates.is_empty() {
        state.completion = None;
        return;
    }
    if let Some(c) = state.completion.as_mut() {
        c.query = query;
        c.candidates = candidates;
        c.index = 0;
        c.scroll_offset = 0;
    }
}

fn slash_completion_advance(state: &mut SessionInteractiveState) {
    let Some(c) = state.completion.as_mut() else {
        return;
    };
    if c.candidates.is_empty() {
        return;
    }
    c.index = (c.index + 1) % c.candidates.len();
    adjust_scroll_offset(c);
}

fn slash_completion_retreat(state: &mut SessionInteractiveState) {
    let Some(c) = state.completion.as_mut() else {
        return;
    };
    if c.candidates.is_empty() {
        return;
    }
    c.index = if c.index == 0 {
        c.candidates.len() - 1
    } else {
        c.index - 1
    };
    adjust_scroll_offset(c);
}

fn adjust_scroll_offset(c: &mut CompletionState) {
    let window = SLASH_COMPLETION_VISIBLE_ROWS;
    if c.index < c.scroll_offset {
        c.scroll_offset = c.index;
    } else if c.index >= c.scroll_offset + window {
        c.scroll_offset = c.index + 1 - window;
    }
}

fn slash_completion_accept(state: &mut SessionInteractiveState) {
    let Some(c) = state.completion.as_ref() else {
        return;
    };
    let Some(&cmd_idx) = c.candidates.get(c.index) else {
        return;
    };
    let cmd = &SLASH_COMMANDS[cmd_idx];
    state.input_buffer.clear();
    state.input_buffer.push('/');
    state.input_buffer.push_str(cmd.name);
    if cmd.routed {
        state.input_buffer.push(' ');
    }
    state.completion = None;
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
        _ => parse_routed_attach_command(command).map(AttachCommand::Routed),
    }
}

fn parse_routed_attach_command(command: &str) -> Option<RoutedAttachCommand> {
    let argv = tokenize_attach_command(command)?;
    let (root, args) = argv.split_first()?;
    if !matches!(
        root.as_str(),
        "workflow" | "session" | "transcript" | "issues"
    ) {
        return None;
    }
    if !inline_command_is_allowed(root, args) {
        return None;
    }
    Some(RoutedAttachCommand {
        root: root.clone(),
        args: args.to_vec(),
        display: command.to_string(),
    })
}

fn tokenize_attach_command(command: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => quote = Some(ch),
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn inline_command_is_allowed(root: &str, args: &[String]) -> bool {
    let Some(subcommand) = args.first().map(|value| value.as_str()) else {
        return false;
    };
    match root {
        "session" => matches!(subcommand, "show" | "list"),
        "transcript" => matches!(subcommand, "show" | "list"),
        "workflow" => matches!(subcommand, "show" | "show-run" | "runs" | "list"),
        "issues" => matches!(subcommand, "show" | "list"),
        _ => false,
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
        AttachCommand::Routed(_) => {
            printer.sideband_event(
                crate::output::palette::Status::Awaiting,
                "namespace commands",
                Some("inline /workflow /session /transcript /issues are available in retained session attach only"),
            );
        }
    }
    Ok(AttachControl::Continue)
}

fn render_attach_help_hint(printer: &mut crate::output::LineStreamPrinter) {
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "controls",
        Some("Enter prompt  ·  Esc cancel turn  ·  Ctrl-D detach  ·  Ctrl-C quit  ·  F1 help"),
    );
}

fn render_attach_help(printer: &mut crate::output::LineStreamPrinter) {
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "keys",
        Some("Enter prompt  ·  Ctrl-U clear  ·  Ctrl-W delete word  ·  Esc cancel turn  ·  Ctrl-D detach  ·  Ctrl-C quit  ·  F1 help"),
    );
    printer.sideband_event(
        crate::output::palette::Status::Active,
        "commands",
        Some("/help /status /history /runs /transcript /cancel /stop /workflow ... /session ... /issues ..."),
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

/// Workspace-scoped label for the session header. Prefers the user-meaningful
/// repo reference (e.g. `Section9Labs/rupu`) when the session is bound to one,
/// falling back to the workspace directory's basename for local sessions.
fn session_header_label(session: &SessionRecord) -> String {
    if let Some(ref_str) = session.repo_ref.as_deref() {
        let stripped = ref_str
            .strip_prefix("github:")
            .or_else(|| ref_str.strip_prefix("gitlab:"))
            .unwrap_or(ref_str);
        return stripped.to_string();
    }
    session
        .workspace_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| session.workspace_id.clone())
}

fn session_session_totals_detail(session: &SessionRecord) -> String {
    let mut detail = String::new();
    let _ = palette::write_colored(&mut detail, session_upload_indicator(), BRAND);
    detail.push_str(&format!(
        " {}  ",
        format_token_count(session.total_tokens_in)
    ));
    let _ = palette::write_colored(&mut detail, session_download_indicator(), BRAND);
    detail.push_str(&format!(
        " {}  ",
        format_token_count(session.total_tokens_out)
    ));
    let _ = palette::write_colored(&mut detail, session_cache_indicator(), BRAND);
    detail.push_str(&format!(
        " {}",
        format_token_count(session.total_tokens_cached)
    ));
    let grand_total = session
        .total_tokens_in
        .saturating_add(session.total_tokens_out)
        .saturating_add(session.total_tokens_cached);
    detail.push_str(&format!("  ·  total {}", format_token_count(grand_total)));
    detail
}

fn session_effort_detail(session: &SessionRecord) -> Option<String> {
    session
        .effort
        .map(|effort| format!("effort {}", format!("{effort:?}").to_ascii_lowercase()))
}

fn session_total_cost_detail(
    session: &SessionRecord,
    state: &SessionInteractiveState,
) -> Option<String> {
    let pricing = crate::pricing::lookup(
        &state.pricing,
        &session.provider_name,
        &session.model,
        &session.agent_name,
    )?;
    Some(format!(
        "~${:.4}",
        pricing.cost_usd(session.total_tokens_in, session.total_tokens_out, 0)
    ))
}

fn format_token_count(n: u64) -> String {
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

fn session_recent_prompt(session: &SessionRecord) -> Option<String> {
    session
        .runs
        .last()
        .map(|run| truncate_single_line(&run.prompt, 96))
}

fn session_pending_run_count(session: &SessionRecord) -> usize {
    session
        .runs
        .iter()
        .filter(|run| {
            run.completed_at.is_none()
                && session.active_run_id.as_deref() != Some(run.run_id.as_str())
        })
        .count()
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
    if let Some(pid) = session
        .active_pid
        .or(session.worker_pid)
        .filter(|pid| pid_is_running(*pid))
    {
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
    session.worker_pid = None;
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
    clear_session_live_usage(global, scope, session_id)?;
    clear_session_turn_queue(global, scope, session_id)?;
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
    if let Some(pid) = session
        .active_pid
        .or(session.worker_pid)
        .filter(|pid| pid_is_running(*pid))
    {
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
    session.worker_pid = None;
    if let Some(run) = session.runs.iter_mut().find(|run| run.run_id == run_id) {
        run.completed_at = Some(Utc::now());
        run.status = Some(RunStatus::Aborted);
        run.error = Some("turn cancelled by operator".into());
    }
    write_session(global, scope, &session)?;
    clear_session_live_usage(global, scope, session_id)?;
    if session_has_pending_turn_requests(global, scope, session_id)? {
        let (mut refreshed, scope) = read_session(global, session_id)?;
        refreshed.status = SessionStatus::Idle;
        let _ = ensure_session_worker(global, &mut refreshed, scope)?;
    }
    Ok(Some(run_id))
}

async fn run_worker(args: RunWorkerArgs) -> anyhow::Result<()> {
    crate::logging::init_to_file();

    let global = paths::global_dir()?;
    let worker_pid = std::process::id();

    {
        let (mut session, scope) = read_session(&global, &args.session_id)?;
        ensure_active_scope(scope, "session _worker")?;
        if let Some(existing_pid) = session.worker_pid {
            if existing_pid != worker_pid && pid_is_running(existing_pid) {
                return Ok(());
            }
        }
        session.worker_pid = Some(worker_pid);
        session.updated_at = Utc::now();
        write_session(&global, scope, &session)?;
    }

    loop {
        let (mut session, scope) = match read_session(&global, &args.session_id) {
            Ok(value) => value,
            Err(_) => break,
        };
        ensure_active_scope(scope, "session _worker")?;
        if reconcile_stale_session(&mut session) {
            write_session(&global, scope, &session)?;
        }
        if session.worker_pid != Some(worker_pid) {
            break;
        }

        if let Some((request, claimed_path)) =
            claim_next_session_turn_request(&global, scope, &args.session_id)?
        {
            let transcript_path = request.transcript_path.clone();
            session.status = SessionStatus::Running;
            session.updated_at = Utc::now();
            session.active_run_id = Some(request.run_id.clone());
            session.active_transcript_path = Some(transcript_path.clone());
            session.active_pid = Some(worker_pid);
            session.last_error = None;
            if let Some(run) = session
                .runs
                .iter_mut()
                .find(|run| run.run_id == request.run_id)
            {
                run.pid = Some(worker_pid);
                run.transcript_path = transcript_path.clone();
            }
            write_session(&global, scope, &session)?;
            clear_session_live_usage(&global, scope, &args.session_id)?;

            let result = run_turn(RunTurnArgs {
                session_id: args.session_id.clone(),
                run_id: request.run_id.clone(),
                prompt: request.prompt.clone(),
            })
            .await;
            let _ = fs::remove_file(&claimed_path);

            if let Err(err) = result {
                let (mut failed, scope) = read_session(&global, &args.session_id)?;
                ensure_active_scope(scope, "session _worker")?;
                if failed.worker_pid == Some(worker_pid) {
                    failed.status = SessionStatus::Failed;
                    failed.updated_at = Utc::now();
                    failed.active_run_id = None;
                    failed.active_transcript_path = None;
                    failed.active_pid = None;
                    failed.last_run_id = Some(request.run_id.clone());
                    failed.last_transcript_path = Some(transcript_path.clone());
                    failed.last_error = Some(err.to_string());
                    if let Some(run) = failed
                        .runs
                        .iter_mut()
                        .find(|run| run.run_id == request.run_id)
                    {
                        run.completed_at.get_or_insert(Utc::now());
                        run.status.get_or_insert(RunStatus::Error);
                        run.error.get_or_insert_with(|| err.to_string());
                        run.transcript_path = transcript_path.clone();
                    }
                    write_session(&global, scope, &failed)?;
                }
                let _ = clear_session_live_usage(&global, scope, &args.session_id);
            }
            continue;
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    if let Ok((mut session, scope)) = read_session(&global, &args.session_id) {
        if session.worker_pid == Some(worker_pid) {
            session.worker_pid = None;
            if session.active_pid == Some(worker_pid) {
                session.active_pid = None;
            }
            session.updated_at = Utc::now();
            let _ = write_session(&global, scope, &session);
        }
        let _ = clear_session_live_usage(&global, scope, &args.session_id);
    }
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
    let live_usage_state = Arc::new(Mutex::new(SessionLiveUsageWriterState::new(
        &session.provider_name,
        &session.model,
    )));
    let live_usage_global = global.clone();
    let live_usage_session_id = args.session_id.clone();
    let live_usage_run_id = args.run_id.clone();
    let live_usage_state_cb = Arc::clone(&live_usage_state);
    let on_stream_event = Arc::new(move |event: StreamEvent| {
        let force = matches!(event, StreamEvent::UsageSnapshot(_));
        let Ok(mut state) = live_usage_state_cb.lock() else {
            return;
        };
        if !state.apply_event(&event) || !state.should_flush(force) {
            return;
        }
        if persist_session_live_usage_snapshot(
            &live_usage_global,
            scope,
            &live_usage_session_id,
            &live_usage_run_id,
            &state.usage,
        )
        .is_ok()
        {
            state.mark_persisted();
        }
    });

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
        on_stream_event: Some(on_stream_event),
    };

    let outcome = rupu_agent::run_agent(opts).await;
    // Snapshot live cached_tokens before clearing — UsageSnapshot events
    // populated it during streaming, but RunResult (from rupu-agent) only
    // carries in/out. Keep cached as an additive grand-total dimension.
    let cached_tokens = read_session_live_usage(&global, scope, &args.session_id)?
        .map(|record| record.usage.cached_tokens)
        .unwrap_or(0);
    clear_session_live_usage(&global, scope, &args.session_id)?;

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
                run.total_tokens_cached = cached_tokens;
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
            session.total_tokens_cached += cached_tokens;
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

fn ensure_active_scope(scope: SessionScope, command: &str) -> anyhow::Result<()> {
    if scope == SessionScope::Archived {
        anyhow::bail!("{command} does not support archived sessions; restore the session first");
    }
    Ok(())
}

fn ensure_session_not_running(session: &SessionRecord, action: &str) -> anyhow::Result<()> {
    if session
        .active_pid
        .or(session.worker_pid)
        .is_some_and(pid_is_running)
    {
        anyhow::bail!(
            "cannot {action} session {} while the worker is still running",
            session.session_id,
        );
    }
    Ok(())
}

fn reconcile_stale_session(session: &mut SessionRecord) -> bool {
    let worker_alive = session.worker_pid.is_some_and(pid_is_running);
    let active_alive = session.active_pid.is_some_and(pid_is_running);
    if session.status != SessionStatus::Running {
        let mut changed = false;
        if session.worker_pid.is_some() && !worker_alive {
            session.worker_pid = None;
            changed = true;
        }
        if session.active_pid.is_some() && !active_alive {
            session.active_pid = None;
            changed = true;
        }
        if changed {
            session.updated_at = Utc::now();
        }
        return changed;
    }
    if active_alive || worker_alive {
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
    session.worker_pid = None;
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

fn session_live_usage_path(global: &Path, scope: SessionScope, session_id: &str) -> PathBuf {
    session_dir(global, scope, session_id).join("live-usage.json")
}

fn session_turn_queue_dir(global: &Path, scope: SessionScope, session_id: &str) -> PathBuf {
    session_dir(global, scope, session_id).join("queue")
}

fn session_turn_request_path(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
    request_id: &str,
) -> PathBuf {
    session_turn_queue_dir(global, scope, session_id).join(format!("{request_id}.json"))
}

fn enqueue_session_turn_request(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
    request: SessionTurnRequest,
) -> anyhow::Result<()> {
    let dir = session_turn_queue_dir(global, scope, session_id);
    fs::create_dir_all(&dir)?;
    let path = session_turn_request_path(global, scope, session_id, &request.request_id);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&request)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn write_session_live_usage(
    global: &Path,
    scope: SessionScope,
    record: &SessionLiveUsageRecord,
) -> anyhow::Result<()> {
    let dir = session_dir(global, scope, &record.session_id);
    fs::create_dir_all(&dir)?;
    let path = session_live_usage_path(global, scope, &record.session_id);
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(record)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn read_session_live_usage(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
) -> anyhow::Result<Option<SessionLiveUsageRecord>> {
    let path = session_live_usage_path(global, scope, session_id);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

fn clear_session_live_usage(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
) -> anyhow::Result<()> {
    let path = session_live_usage_path(global, scope, session_id);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn persist_session_live_usage_snapshot(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
    run_id: &str,
    usage: &SessionLiveUsage,
) -> anyhow::Result<()> {
    if let Some(active_run_id) = read_session(global, session_id)?
        .0
        .active_run_id
        .filter(|active_run_id| active_run_id == run_id)
    {
        let record = SessionLiveUsageRecord {
            version: SessionLiveUsageRecord::VERSION,
            session_id: session_id.to_string(),
            run_id: active_run_id,
            updated_at: Utc::now(),
            usage: usage.clone(),
        };
        write_session_live_usage(global, scope, &record)?;
    }
    Ok(())
}

fn claim_next_session_turn_request(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
) -> anyhow::Result<Option<(SessionTurnRequest, PathBuf)>> {
    let dir = session_turn_queue_dir(global, scope, session_id);
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut paths = fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let claimed = path.with_extension("processing");
        if fs::rename(&path, &claimed).is_err() {
            continue;
        }
        let bytes = fs::read(&claimed)
            .with_context(|| format!("read session turn request {}", claimed.display()))?;
        let request: SessionTurnRequest = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse session turn request {}", claimed.display()))?;
        return Ok(Some((request, claimed)));
    }
    Ok(None)
}

fn clear_session_turn_queue(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
) -> anyhow::Result<()> {
    let dir = session_turn_queue_dir(global, scope, session_id);
    if dir.is_dir() {
        fs::remove_dir_all(&dir)
            .with_context(|| format!("remove session queue {}", dir.display()))?;
    }
    Ok(())
}

fn session_has_pending_turn_requests(
    global: &Path,
    scope: SessionScope,
    session_id: &str,
) -> anyhow::Result<bool> {
    let dir = session_turn_queue_dir(global, scope, session_id);
    if !dir.is_dir() {
        return Ok(false);
    }
    Ok(fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .any(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json")))
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

    fn write_session_transcript(path: &Path, assistant_lines: impl IntoIterator<Item = String>) {
        let mut lines = Vec::new();
        lines.push(
            serde_json::to_string(&TranscriptEvent::RunStart {
                run_id: "run_live123".into(),
                workspace_id: "ws_live123".into(),
                agent: "issue-reader".into(),
                provider: "openai".into(),
                model: "gpt-5".into(),
                started_at: Utc::now(),
                mode: RunMode::Bypass,
            })
            .unwrap(),
        );
        for content in assistant_lines {
            lines.push(
                serde_json::to_string(&TranscriptEvent::AssistantMessage {
                    content,
                    thinking: None,
                })
                .unwrap(),
            );
        }
        std::fs::write(path, format!("{}\n", lines.join("\n"))).unwrap();
    }

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
        match parse_attach_command("/workflow show-run current --view full") {
            Some(AttachCommand::Routed(cmd)) => {
                assert_eq!(cmd.root, "workflow");
                assert_eq!(cmd.args[0], "show-run");
                assert_eq!(cmd.args[1], "current");
            }
            other => panic!("unexpected routed command: {other:?}"),
        }
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
    fn retained_session_prompt_line_uses_agent_name() {
        let session = test_session_record();
        let state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        let line = render_session_prompt_line(&session, &state, 120);
        // Agent name and prompt arrow are emitted with separate ANSI
        // color escapes between them, so check each segment on its own.
        assert!(line.contains("issue-reader"));
        assert!(line.contains(" > "));
        assert!(!line.contains("ses_test123>"));
    }

    #[test]
    fn plain_letter_keys_start_prompt_instead_of_triggering_actions() {
        let session = SessionRecord {
            status: SessionStatus::Idle,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );

        let control = handle_session_live_keypress(
            Path::new("/tmp/repo"),
            &session,
            &mut state,
            &prefs,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        )
        .expect("keypress handled");

        assert!(matches!(control, AttachControl::Continue));
        assert!(state.prompt_active);
        assert_eq!(state.input_buffer, "s");
        assert!(state.entries.is_empty());
    }

    #[test]
    fn plain_letter_keys_start_prompt_while_running() {
        let session = SessionRecord {
            status: SessionStatus::Running,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );

        let control = handle_session_live_keypress(
            Path::new("/tmp/repo"),
            &session,
            &mut state,
            &prefs,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        )
        .expect("keypress handled");

        assert!(matches!(control, AttachControl::Continue));
        assert!(state.prompt_active);
        assert_eq!(state.input_buffer, "s");
        assert!(state.entries.is_empty());
    }

    #[test]
    fn routed_command_notice_rows_keep_entry_cache_in_lockstep() {
        // Regression for the panic in `render_cached_entry_rows`
        // ("index out of bounds: the len is N but the index is N")
        // triggered by `/issues` / `/workflow` / `/session` / `/transcript`.
        // The routed-command output loop used to call `state.entries.push`
        // directly, leaving `entry_row_cache` shorter than `entries` and
        // causing the next render to panic.
        let session = SessionRecord {
            status: SessionStatus::Running,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        // Mimic the post-subprocess output loop from
        // `execute_routed_session_command`: many notice lines added in
        // sequence. Before the fix this used bare `entries.push` and
        // the cache fell behind by exactly this count.
        for line in 0..40 {
            state.push_entry(SessionEntry::Notice(SessionViewLine {
                status: crate::output::palette::Status::Complete,
                text: format!("routed line {line}"),
                continuation: true,
            }));
        }
        assert_eq!(state.entries.len(), state.entry_row_cache.len());

        // Render should succeed without panic regardless of viewport size.
        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 96, 24);
        assert_eq!(rows.len(), 24);
        // Cache and entries remain in lockstep after rendering.
        assert_eq!(state.entries.len(), state.entry_row_cache.len());
    }

    #[test]
    fn handle_session_live_input_queues_prompt_while_running() {
        let global = tempfile::tempdir().expect("tmpdir");
        let session = SessionRecord {
            status: SessionStatus::Running,
            active_pid: Some(std::process::id()),
            worker_pid: Some(std::process::id()),
            ..test_session_record()
        };
        write_session(global.path(), SessionScope::Active, &session).expect("write session");
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );

        let control = handle_session_live_input(
            global.path(),
            &session,
            &mut state,
            &prefs,
            "summarize the repo".into(),
        )
        .expect("input handled");

        assert!(matches!(control, AttachControl::Continue));
        assert_eq!(state.queued_prompts.len(), 1);
        assert!(session_has_pending_turn_requests(
            global.path(),
            SessionScope::Active,
            &session.session_id
        )
        .expect("queue status"));
        assert!(matches!(
            state.entries.first(),
            Some(SessionEntry::UserPrompt { content, queued: true, .. }) if content == "summarize the repo"
        ));
    }

    #[test]
    fn claim_next_session_turn_request_returns_oldest_request_first() {
        let global = tempfile::tempdir().expect("tmpdir");
        let session = test_session_record();
        write_session(global.path(), SessionScope::Active, &session).expect("write session");
        enqueue_session_turn_request(
            global.path(),
            SessionScope::Active,
            &session.session_id,
            SessionTurnRequest {
                version: SessionTurnRequest::VERSION,
                request_id: "01KTEST0000000000000000001".into(),
                run_id: "run_oldest".into(),
                prompt: "first".into(),
                transcript_path: session.transcripts_dir.join("run_oldest.jsonl"),
                enqueued_at: Utc::now(),
            },
        )
        .expect("enqueue oldest");
        enqueue_session_turn_request(
            global.path(),
            SessionScope::Active,
            &session.session_id,
            SessionTurnRequest {
                version: SessionTurnRequest::VERSION,
                request_id: "01KTEST0000000000000000002".into(),
                run_id: "run_newest".into(),
                prompt: "second".into(),
                transcript_path: session.transcripts_dir.join("run_newest.jsonl"),
                enqueued_at: Utc::now(),
            },
        )
        .expect("enqueue newest");

        let (request, claimed_path) = claim_next_session_turn_request(
            global.path(),
            SessionScope::Active,
            &session.session_id,
        )
        .expect("claim request")
        .expect("request present");

        assert_eq!(request.run_id, "run_oldest");
        assert_eq!(request.prompt, "first");
        assert!(claimed_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "processing"));
    }

    #[test]
    fn reconcile_stale_session_clears_dead_idle_worker() {
        let mut session = SessionRecord {
            status: SessionStatus::Idle,
            worker_pid: Some(u32::MAX),
            active_pid: None,
            ..test_session_record()
        };

        assert!(reconcile_stale_session(&mut session));
        assert_eq!(session.status, SessionStatus::Idle);
        assert_eq!(session.worker_pid, None);
        assert_eq!(session.active_pid, None);
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
    fn retained_session_attach_bootstraps_from_recent_tail_then_backfills_on_jump_top() {
        let dir = tempfile::tempdir().unwrap();
        let transcript_path = dir.path().join("run_live123.jsonl");
        write_session_transcript(
            &transcript_path,
            (0..220).map(|index| format!("assistant line {index:03}")),
        );

        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let mut state = SessionInteractiveState::new(
            transcript_path,
            Some("run_live123".into()),
            LiveViewMode::Focused,
        );

        assert!(state.entries.len() < 221);

        let tail_rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 84, 16);
        assert!(tail_rows
            .iter()
            .any(|row| row.contains("assistant line 219")));
        assert!(!tail_rows
            .iter()
            .any(|row| row.contains("assistant line 000")));

        state.load_all_older_history();
        state.viewport.jump_top();
        let top_rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 84, 16);
        assert!(top_rows
            .iter()
            .any(|row| row.contains("assistant line 000")));
    }

    #[test]
    fn retained_session_viewport_keeps_oldest_rows_visible_when_history_grows() {
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
        for index in 0..120 {
            state.push_line(
                crate::output::palette::Status::Active,
                format!("history line {index:03}"),
            );
        }
        state.viewport.jump_top();

        let initial_rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 72, 16);
        assert!(initial_rows
            .iter()
            .any(|row| row.contains("history line 000")));
        assert!(!initial_rows
            .iter()
            .any(|row| row.contains("history line 119")));

        for index in 120..240 {
            state.push_line(
                crate::output::palette::Status::Active,
                format!("history line {index:03}"),
            );
        }

        let grown_rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 72, 16);
        assert!(grown_rows
            .iter()
            .any(|row| row.contains("history line 000")));
        assert!(!grown_rows
            .iter()
            .any(|row| row.contains("history line 239")));
    }

    #[test]
    fn retained_session_rows_group_assistant_text_and_hide_success_tool_output_in_compact() {
        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.push_user_prompt("summarize the issue", None, false);
        state.push_transcript_event(&TranscriptEvent::AssistantDelta {
            content: "Reading the repo".into(),
        });
        state.push_transcript_event(&TranscriptEvent::ToolCall {
            call_id: "call_123".into(),
            tool: "bash".into(),
            input: serde_json::json!({ "command": "rg session crates/" }),
        });
        state.push_transcript_event(&TranscriptEvent::ToolResult {
            call_id: "call_123".into(),
            output: "line1\nline2\nline3\n".into(),
            error: None,
            duration_ms: 8,
        });

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 96, 20);
        assert!(rows.iter().any(|row| row.contains("you")));
        assert!(rows.iter().any(|row| row.contains("agent")));
        assert!(rows.iter().any(|row| row.contains("Reading the repo")));
        assert!(rows.iter().any(|row| row.contains("tool bash")));
        assert!(rows.iter().any(|row| row.contains("command: |")));
        assert!(rows.iter().any(|row| row.contains("tool result")));
        assert!(!rows.iter().any(|row| row.contains("line1")));
    }

    #[test]
    fn retained_session_rows_wrap_multiline_assistant_output_with_indentation() {
        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.push_transcript_event(&TranscriptEvent::AssistantMessage {
            content: "line one with a tab\tvalue\n\nline three after a blank line".into(),
            thinking: None,
        });

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 36, 18);
        assert!(rows.iter().any(|row| row.contains("agent")));
        assert!(rows.iter().any(|row| row.contains("line one with")));
        assert!(rows.iter().any(|row| row.contains("tab    value")));
        assert!(rows.iter().any(|row| row.trim().is_empty() || row == "  "));
        assert!(rows.iter().all(|row| visible_len(row) <= 36));
    }

    #[test]
    fn retained_session_rows_show_thinking_indicator_while_running() {
        let session = SessionRecord {
            status: SessionStatus::Running,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.push_transcript_event(&TranscriptEvent::TurnStart { turn_idx: 0 });

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 96, 16);
        assert!(rows.iter().any(|row| row.contains("agent")));
        assert!(rows.iter().any(|row| row.contains("thinking")));
    }

    #[test]
    fn retained_session_header_shows_model_effort_totals_and_cost() {
        let session = SessionRecord {
            status: SessionStatus::Running,
            provider_name: "openai".into(),
            model: "gpt-5".into(),
            effort: Some(ThinkingLevel::High),
            ..test_session_record()
        };
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.pricing = PricingConfig::default();
        state.activity = SessionActivity::Thinking;

        let header = render_session_header_line(&session, &state, 160);
        // Header shows the workspace label (repo_ref stripped of platform prefix),
        // not the agent name — the agent appears on the prompt row instead.
        assert!(header.contains("Section9Labs/rupu"));
        assert!(!header.contains("issue-reader"));
        assert!(header.contains("gpt-5"));
        assert!(header.contains("effort high"));
        assert!(header.contains("⇡"));
        assert!(header.contains(" 120"));
        assert!(header.contains("⇣"));
        assert!(header.contains(" 45"));
        assert!(header.contains("⟳"));
        assert!(header.contains(" 18"));
        // grand total = 120 + 45 + 18 = 183
        assert!(header.contains("total 183"));
        assert!(header.contains("~$"));
    }

    #[test]
    fn retained_session_rows_show_live_thinking_tokens_in_assistant_row() {
        let session = SessionRecord {
            status: SessionStatus::Running,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.current_run_started_at = Some(Utc::now());
        state.live_usage.input_tokens = 12;
        state.live_usage.output_tokens = 5;
        state.live_usage.cached_tokens = 2;
        state.activity = SessionActivity::Thinking;

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 120, 16);
        assert!(rows.iter().any(|row| row.contains("agent")));
        assert!(rows.iter().any(|row| row.contains("thinking")));
        assert!(rows.iter().any(|row| row.contains("00:00:")));
        assert!(rows.iter().any(|row| row.contains("⇡")));
        assert!(rows.iter().any(|row| row.contains("⇣")));
        assert!(rows.iter().any(|row| row.contains("⟳")));
        assert!(rows.iter().any(|row| row.contains("12")));
        assert!(rows.iter().any(|row| row.contains("5")));
        assert!(rows.iter().any(|row| row.contains("2")));
    }

    #[test]
    fn retained_session_prompt_line_omits_live_status_counters() {
        let session = SessionRecord {
            status: SessionStatus::Running,
            model: "gpt-5".into(),
            ..test_session_record()
        };
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.live_usage.input_tokens = 12;
        state.live_usage.output_tokens = 5;
        state.live_usage.cached_tokens = 2;
        state.activity = SessionActivity::Thinking;

        let line = render_session_prompt_line(&session, &state, 160);
        assert!(line.contains("issue-reader"));
        assert!(line.contains(" > "));
        assert!(!line.contains("thinking"));
        assert!(!line.contains("⇡"));
        assert!(!line.contains("⇣"));
        assert!(!line.contains("⟳"));
    }

    #[test]
    fn session_live_usage_writer_estimates_output_until_provider_snapshot() {
        let mut writer = SessionLiveUsageWriterState::new("openai", "gpt-5");
        assert!(writer.apply_event(&StreamEvent::TextDelta("hello there".into())));
        assert!(writer.usage.output_tokens > 0);
        assert!(writer.usage.output_tokens_estimated);

        assert!(
            writer.apply_event(&StreamEvent::UsageSnapshot(rupu_providers::types::Usage {
                input_tokens: 123,
                output_tokens: 7,
                cached_tokens: 9,
            },))
        );
        assert_eq!(writer.usage.input_tokens, 123);
        assert_eq!(writer.usage.output_tokens, 7);
        assert_eq!(writer.usage.cached_tokens, 9);
        assert!(!writer.usage.output_tokens_estimated);
    }

    #[test]
    fn retained_session_compact_rows_include_usage_events() {
        let session = SessionRecord {
            status: SessionStatus::Idle,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.push_transcript_event(&TranscriptEvent::Usage {
            provider: "openai".into(),
            model: "gpt-5".into(),
            input_tokens: 12,
            output_tokens: 5,
            cached_tokens: 2,
        });

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 120, 12);
        assert!(rows.iter().any(|row| row.contains("usage")));
        assert!(rows.iter().any(|row| row.contains("in 12 out 5 cached 2")));
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
        assert!(focused[0].text.contains("assistant"));
        assert!(full[0].text.contains("assistant"));
        assert!(full.len() >= focused.len());
    }

    #[test]
    fn transcript_event_lines_compact_keeps_assistant_body() {
        let event = TranscriptEvent::AssistantMessage {
            content: "## Summary\n\n- first\n- second".into(),
            thinking: None,
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let compact = transcript_event_lines(&event, LiveViewMode::Compact, &prefs);
        assert!(!compact.is_empty());
        assert!(compact[0].text.contains("assistant"));
        assert!(compact.len() > 1);
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

    #[test]
    fn transcript_event_lines_compact_previews_tool_result_payload() {
        let event = TranscriptEvent::ToolResult {
            output: "line1\nline2\nline3\nline4\nline5\nline6\n".into(),
            error: None,
            duration_ms: 12,
            call_id: "call_123".into(),
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let compact = transcript_event_lines(&event, LiveViewMode::Compact, &prefs);
        assert_eq!(compact.len(), 1);
        assert!(compact[0].text.contains("tool result"));
        assert!(!compact.iter().any(|line| line.text.contains("line1")));
    }

    #[test]
    fn assistant_delta_merges_into_single_streaming_assistant_entry() {
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );

        state.push_transcript_event(&TranscriptEvent::AssistantDelta {
            content: "Hello".into(),
        });
        state.push_transcript_event(&TranscriptEvent::AssistantDelta {
            content: " world".into(),
        });
        state.push_transcript_event(&TranscriptEvent::Usage {
            provider: "openai".into(),
            model: "gpt-5".into(),
            input_tokens: 10,
            output_tokens: 4,
            cached_tokens: 0,
        });
        state.push_transcript_event(&TranscriptEvent::AssistantMessage {
            content: "Hello world".into(),
            thinking: None,
        });

        assert_eq!(state.entries.len(), 2);
        match &state.entries[0] {
            SessionEntry::Assistant {
                content, streaming, ..
            } => {
                assert_eq!(content, "Hello world");
                assert!(!streaming);
            }
            other => panic!("expected assistant entry, got {other:?}"),
        }
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for inner in chars.by_ref() {
                    if inner == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn retained_session_agent_block_renders_glyph_and_rail() {
        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.push_user_prompt("hello there", None, false);
        state.push_transcript_event(&TranscriptEvent::AssistantMessage {
            content: "hi from the agent".into(),
            thinking: None,
        });

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 96, 20);
        let plain: Vec<String> = rows.iter().map(|row| strip_ansi(row)).collect();

        // Role header has the new label and the Active status glyph.
        assert!(plain.iter().any(|row| row.starts_with("●  agent")));
        // Agent body rows wear the colored rail.
        assert!(plain
            .iter()
            .any(|row| row.starts_with("│ hi from the agent")));
        // User prompt has the brand glyph and matching rail.
        assert!(plain.iter().any(|row| row.starts_with("▸  you")));
        assert!(plain.iter().any(|row| row.starts_with("│ hello there")));
        // Old label and old indent are gone.
        assert!(!plain.iter().any(|row| row.starts_with("assistant")));
        assert!(!plain
            .iter()
            .any(|row| row.starts_with("  hi from the agent")));
    }

    #[test]
    fn retained_session_streaming_agent_block_uses_working_glyph() {
        let session = test_session_record();
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        );
        state.push_transcript_event(&TranscriptEvent::AssistantDelta {
            content: "streaming chunk".into(),
        });

        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 96, 20);
        let plain: Vec<String> = rows.iter().map(|row| strip_ansi(row)).collect();

        // While streaming, the agent header carries the Working glyph (◐).
        assert!(plain.iter().any(|row| row.starts_with("◐  agent")));
        assert!(plain.iter().any(|row| row.starts_with("│ streaming chunk")));
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
            worker_pid: Some(1234),
            last_run_id: Some("run_prev123".into()),
            last_transcript_path: Some(PathBuf::from(
                "/tmp/repo/.rupu/transcripts/run_prev123.jsonl",
            )),
            last_error: None,
            total_turns: 3,
            total_tokens_in: 120,
            total_tokens_out: 45,
            total_tokens_cached: 18,
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
                total_tokens_cached: 15,
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

    fn fresh_completion_state() -> SessionInteractiveState {
        SessionInteractiveState::new(
            PathBuf::from("/tmp/repo/.rupu/transcripts/run_live123.jsonl"),
            Some("run_live123".into()),
            LiveViewMode::Compact,
        )
    }

    fn press(
        state: &mut SessionInteractiveState,
        session: &SessionRecord,
        prefs: &UiPrefs,
        code: KeyCode,
    ) -> AttachControl {
        handle_session_live_keypress(
            Path::new("/tmp/repo"),
            session,
            state,
            prefs,
            KeyEvent::new(code, KeyModifiers::NONE),
        )
        .expect("keypress handled")
    }

    fn typing_session() -> (SessionRecord, UiPrefs, SessionInteractiveState) {
        let session = SessionRecord {
            status: SessionStatus::Running,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = fresh_completion_state();
        state.prompt_active = true;
        state.input_buffer.push('/');
        (session, prefs, state)
    }

    #[test]
    fn slash_completion_filters_by_prefix() {
        let all = slash_completion_candidates("");
        assert_eq!(all.len(), SLASH_COMMANDS.len());

        let h = slash_completion_candidates("h");
        let names: Vec<&str> = h.iter().map(|&i| SLASH_COMMANDS[i].name).collect();
        assert_eq!(names, vec!["help", "history"]);

        assert!(slash_completion_candidates("zz").is_empty());
    }

    #[test]
    fn slash_completion_query_closes_when_user_types_argument() {
        assert_eq!(slash_completion_query("/h"), Some("h".into()));
        assert_eq!(slash_completion_query("/"), Some(String::new()));
        assert_eq!(slash_completion_query("/workflow "), None);
        assert_eq!(slash_completion_query("/workflow show"), None);
        assert_eq!(slash_completion_query("no slash"), None);
    }

    #[test]
    fn slash_completion_tab_opens_popup_when_buffer_starts_with_slash() {
        let (session, prefs, mut state) = typing_session();
        press(&mut state, &session, &prefs, KeyCode::Tab);
        let c = state.completion.as_ref().expect("popup opened");
        assert_eq!(c.query, "");
        assert_eq!(c.candidates.len(), SLASH_COMMANDS.len());
        assert_eq!(c.index, 0);
    }

    #[test]
    fn slash_completion_tab_does_not_open_without_slash() {
        let (session, prefs, mut state) = typing_session();
        state.input_buffer.clear();
        state.input_buffer.push('x');
        press(&mut state, &session, &prefs, KeyCode::Tab);
        assert!(state.completion.is_none());
    }

    #[test]
    fn slash_completion_tab_advances_and_wraps() {
        let (session, prefs, mut state) = typing_session();
        press(&mut state, &session, &prefs, KeyCode::Tab);
        let total = state.completion.as_ref().unwrap().candidates.len();
        for _ in 0..total - 1 {
            press(&mut state, &session, &prefs, KeyCode::Tab);
        }
        assert_eq!(state.completion.as_ref().unwrap().index, total - 1);
        press(&mut state, &session, &prefs, KeyCode::Tab);
        assert_eq!(state.completion.as_ref().unwrap().index, 0);
    }

    #[test]
    fn slash_completion_up_decrements_and_wraps() {
        let (session, prefs, mut state) = typing_session();
        press(&mut state, &session, &prefs, KeyCode::Tab);
        let total = state.completion.as_ref().unwrap().candidates.len();
        press(&mut state, &session, &prefs, KeyCode::Up);
        assert_eq!(state.completion.as_ref().unwrap().index, total - 1);
        press(&mut state, &session, &prefs, KeyCode::Up);
        assert_eq!(state.completion.as_ref().unwrap().index, total - 2);
    }

    #[test]
    fn slash_completion_enter_accepts_builtin_without_trailing_space() {
        let (session, prefs, mut state) = typing_session();
        state.input_buffer.push('h');
        slash_completion_open(&mut state);
        // /h matches help, history — highlight help (index 0).
        let control = press(&mut state, &session, &prefs, KeyCode::Enter);
        assert!(matches!(control, AttachControl::Continue));
        assert!(state.completion.is_none());
        assert_eq!(state.input_buffer, "/help");
        // The buffer is now ready for the *next* Enter to dispatch.
        assert!(state.prompt_active);
    }

    #[test]
    fn slash_completion_enter_accepts_routed_with_trailing_space() {
        let (session, prefs, mut state) = typing_session();
        state.input_buffer.push('w');
        slash_completion_open(&mut state);
        // /w matches only `workflow` (routed).
        let control = press(&mut state, &session, &prefs, KeyCode::Enter);
        assert!(matches!(control, AttachControl::Continue));
        assert!(state.completion.is_none());
        assert_eq!(state.input_buffer, "/workflow ");
    }

    #[test]
    fn slash_completion_esc_closes_without_changes() {
        let (session, prefs, mut state) = typing_session();
        state.input_buffer.push('h');
        slash_completion_open(&mut state);
        let before = state.input_buffer.clone();
        press(&mut state, &session, &prefs, KeyCode::Esc);
        assert!(state.completion.is_none());
        assert_eq!(state.input_buffer, before);
    }

    #[test]
    fn slash_completion_typing_refilters() {
        let (session, prefs, mut state) = typing_session();
        press(&mut state, &session, &prefs, KeyCode::Tab);
        // Popup open with all commands. Type 'h' -> only help, history remain.
        press(&mut state, &session, &prefs, KeyCode::Char('h'));
        let c = state.completion.as_ref().expect("popup still open");
        assert_eq!(c.query, "h");
        let names: Vec<&str> = c
            .candidates
            .iter()
            .map(|&i| SLASH_COMMANDS[i].name)
            .collect();
        assert_eq!(names, vec!["help", "history"]);
        // Backspace -> popup returns to full list.
        press(&mut state, &session, &prefs, KeyCode::Backspace);
        let c = state.completion.as_ref().expect("popup still open");
        assert_eq!(c.query, "");
        assert_eq!(c.candidates.len(), SLASH_COMMANDS.len());
    }

    #[test]
    fn slash_completion_closes_when_buffer_loses_slash() {
        let (session, prefs, mut state) = typing_session();
        press(&mut state, &session, &prefs, KeyCode::Tab);
        assert!(state.completion.is_some());
        press(&mut state, &session, &prefs, KeyCode::Backspace);
        assert!(state.completion.is_none());
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn slash_completion_closes_when_no_candidates_match() {
        let (session, prefs, mut state) = typing_session();
        press(&mut state, &session, &prefs, KeyCode::Tab);
        // /z matches nothing — popup should close.
        press(&mut state, &session, &prefs, KeyCode::Char('z'));
        assert!(state.completion.is_none());
        assert_eq!(state.input_buffer, "/z");
    }

    #[test]
    fn slash_completion_rows_render_with_marker_above_prompt() {
        let session = SessionRecord {
            status: SessionStatus::Running,
            ..test_session_record()
        };
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Compact),
        );
        let mut state = fresh_completion_state();
        state.prompt_active = true;
        state.input_buffer.push('/');
        state.input_buffer.push('h');
        slash_completion_open(&mut state);
        let rows = build_session_screen_rows_for_size(&session, &mut state, &prefs, 120, 20);
        let plain: Vec<String> = rows.iter().map(|row| strip_ansi(row)).collect();

        let help_idx = plain
            .iter()
            .position(|row| row.starts_with("▸ /help"))
            .expect("help row highlighted");
        let history_idx = plain
            .iter()
            .position(|row| row.trim_start().starts_with("/history"))
            .expect("history row");
        let prompt_idx = plain
            .iter()
            .position(|row| row.contains("issue-reader"))
            .expect("prompt row");
        assert!(help_idx < prompt_idx);
        assert!(history_idx < prompt_idx);
        // Help is selected (▸), history is not.
        assert!(!plain[history_idx].starts_with("▸"));
    }

    #[test]
    fn slash_completion_catalog_round_trips_through_parser() {
        for cmd in SLASH_COMMANDS {
            let typed = if cmd.routed {
                format!("/{} list", cmd.name)
            } else {
                format!("/{}", cmd.name)
            };
            assert!(
                parse_attach_command(&typed).is_some(),
                "catalog entry `/{}` should parse",
                cmd.name
            );
        }
    }
}
