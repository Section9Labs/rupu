//! `rupu transcript list | show`.
//!
//! `list` scans `<project>/.rupu/transcripts/*.jsonl` and
//! `<global>/transcripts/*.jsonl`, summarises each file via
//! [`rupu_transcript::JsonlReader::summary`], and renders a comfy
//! table sorted newest-first by `started_at`. The TITLE column is a
//! one-line preview of the run's first assistant chunk — gives the
//! otherwise-opaque ULID `run_id` enough context for the operator to
//! recognise which run is which without `transcript show`-ing each.
//!
//! `show <run_id>` finds `<run_id>.jsonl` in either transcripts directory
//! and renders it as a timeline (`pretty`, the default), a structured
//! `json` envelope, or raw `jsonl`.

use crate::cmd::completers::{standalone_transcript_run_ids, transcript_run_ids};
use crate::cmd::retention::parse_retention_duration;
use crate::cmd::ui::LiveViewMode;
use crate::output::formats::OutputFormat;
use crate::output::palette;
use crate::output::palette::Status;
use crate::output::printer::{visible_len, wrap_with_ansi};
use crate::output::report::{self, CollectionOutput, EventOutput};
use crate::output::workflow_printer::tool_summary;
use crate::output::LineStreamPrinter;
use crate::paths;
use crate::standalone_run_metadata::{metadata_path_for_run, read_metadata};
use clap::{Args as ClapArgs, Subcommand};
use clap_complete::ArgValueCompleter;
use comfy_table::Cell;
use rupu_transcript::{Event as TranscriptEvent, JsonlReader, RunStatus};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List all transcripts (project-local + global) sorted newest first.
    List {
        /// Disable terminal colors. Honors `NO_COLOR` and the
        /// `[ui].color` config knob too — flag is the explicit override.
        #[arg(long)]
        no_color: bool,
        /// Include both active and archived transcripts.
        #[arg(long, conflicts_with = "archived")]
        all: bool,
        /// Show only archived transcripts.
        #[arg(long, conflicts_with = "all")]
        archived: bool,
    },
    /// Print a transcript's full event stream.
    Show {
        #[arg(add = ArgValueCompleter::new(transcript_run_ids))]
        run_id: String,
        #[arg(long, value_enum)]
        view: Option<LiveViewMode>,
        #[arg(long)]
        no_color: bool,
        #[arg(long, conflicts_with = "no_pager")]
        pager: bool,
        #[arg(long, conflicts_with = "pager")]
        no_pager: bool,
    },
    /// Archive a standalone transcript and its metadata.
    Archive {
        #[arg(add = ArgValueCompleter::new(standalone_transcript_run_ids))]
        run_id: String,
    },
    /// Permanently delete a standalone transcript and its metadata.
    Delete(DeleteArgs),
    /// Delete archived standalone transcripts older than a cutoff.
    Prune(PruneArgs),
}

#[derive(ClapArgs, Debug)]
pub struct DeleteArgs {
    #[arg(add = ArgValueCompleter::new(standalone_transcript_run_ids))]
    pub run_id: String,
    #[arg(long)]
    pub force: bool,
}

#[derive(ClapArgs, Debug)]
pub struct PruneArgs {
    /// Retention cutoff, e.g. `30d`, `12h`, or `1w`.
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Preview deletions without removing files.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List {
            no_color,
            all,
            archived,
        } => list(no_color, all, archived, global_format).await,
        Action::Show {
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
            show(&run_id, view, no_color, pager_flag, global_format).await
        }
        Action::Archive { run_id } => archive(&run_id).await,
        Action::Delete(args) => delete(args).await,
        Action::Prune(args) => prune(args, global_format).await,
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List { .. } => ("transcript list", report::TABLE_JSON_CSV),
        Action::Show { .. } => ("transcript show", report::PRETTY_TABLE_JSON_JSONL),
        Action::Archive { .. } => ("transcript archive", report::TABLE_ONLY),
        Action::Delete(_) => ("transcript delete", report::TABLE_ONLY),
        Action::Prune(_) => ("transcript prune", report::TABLE_JSON_CSV),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

/// Truncate to a single-line preview — strip newlines, collapse runs
/// of whitespace, cap at `max` graphemes, append `…` if cut. Used for
/// the TITLE column so a chunk that opens with a code fence or a
/// markdown header still reads as one row of the table.
fn one_line_preview(s: &str, max: usize) -> String {
    // Replace any run of whitespace (including newlines) with a single
    // space so multi-line chunks render on one row.
    let mut squashed = String::with_capacity(s.len());
    let mut prev_was_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_was_ws {
                squashed.push(' ');
            }
            prev_was_ws = true;
        } else {
            squashed.push(ch);
            prev_was_ws = false;
        }
    }
    let trimmed = squashed.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    // Cap at `max - 1` graphemes (chars proxy) and add the ellipsis.
    let mut out: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[derive(Serialize)]
struct TranscriptListRow {
    run_id: String,
    scope: String,
    title: Option<String>,
    agent: String,
    status: String,
    total_tokens: u64,
    started_at: String,
}

#[derive(Serialize)]
struct TranscriptListCsvRow {
    run_id: String,
    scope: String,
    title: String,
    agent: String,
    status: String,
    total_tokens: u64,
    started_at: String,
}

#[derive(Serialize)]
struct TranscriptListReport {
    kind: &'static str,
    version: u8,
    rows: Vec<TranscriptListRow>,
}

struct TranscriptListOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: TranscriptListReport,
    csv_rows: Vec<TranscriptListCsvRow>,
}

#[derive(Serialize)]
struct TranscriptPruneRow {
    run_id: String,
    scope: String,
    location: String,
    archived_at: String,
    action: String,
}

#[derive(Serialize)]
struct TranscriptPruneCsvRow {
    run_id: String,
    scope: String,
    location: String,
    archived_at: String,
    action: String,
}

#[derive(Serialize)]
struct TranscriptPruneReport {
    kind: &'static str,
    version: u8,
    rows: Vec<TranscriptPruneRow>,
}

struct TranscriptPruneOutput {
    report: TranscriptPruneReport,
    csv_rows: Vec<TranscriptPruneCsvRow>,
}

#[derive(Debug, Clone)]
pub(crate) struct PrunedTranscript {
    pub run_id: String,
    pub scope: String,
    pub location: String,
    pub archived_at: String,
    pub action: String,
}

#[derive(Serialize)]
struct TranscriptShowItem {
    run_id: String,
    path: String,
    events: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct TranscriptShowReport {
    kind: &'static str,
    version: u8,
    item: TranscriptShowItem,
}

struct TranscriptShowOutput {
    prefs: crate::cmd::ui::UiPrefs,
    report: TranscriptShowReport,
    events: Vec<TranscriptEvent>,
    view_mode: LiveViewMode,
}

impl CollectionOutput for TranscriptListOutput {
    type JsonReport = TranscriptListReport;
    type CsvRow = TranscriptListCsvRow;

    fn command_name(&self) -> &'static str {
        "transcript list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "run_id",
            "scope",
            "title",
            "agent",
            "status",
            "total_tokens",
            "started_at",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec![
            "RUN ID", "SCOPE", "TITLE", "AGENT", "STATUS", "TOKENS", "STARTED",
        ]);
        for row in &self.report.rows {
            let title_cell = match &row.title {
                Some(title) => Cell::new(one_line_preview(title, 60)),
                None => {
                    if self.prefs.use_color() {
                        Cell::new("\x1b[2m—\x1b[0m")
                    } else {
                        Cell::new("—")
                    }
                }
            };
            table.add_row(vec![
                Cell::new(&row.run_id),
                Cell::new(&row.scope),
                title_cell,
                Cell::new(&row.agent),
                crate::output::tables::status_cell(&row.status, &self.prefs),
                Cell::new(row.total_tokens.to_string()),
                Cell::new(&row.started_at),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

impl CollectionOutput for TranscriptPruneOutput {
    type JsonReport = TranscriptPruneReport;
    type CsvRow = TranscriptPruneCsvRow;

    fn command_name(&self) -> &'static str {
        "transcript prune"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["run_id", "scope", "location", "archived_at", "action"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let mut table = crate::output::tables::new_table();
        table.set_header(vec!["RUN ID", "SCOPE", "LOCATION", "ARCHIVED", "ACTION"]);
        for row in &self.report.rows {
            table.add_row(vec![
                Cell::new(&row.run_id),
                Cell::new(&row.scope),
                Cell::new(&row.location),
                Cell::new(&row.archived_at),
                Cell::new(&row.action),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

impl EventOutput for TranscriptShowOutput {
    type JsonReport = TranscriptShowReport;
    type JsonlRow = TranscriptEvent;

    fn command_name(&self) -> &'static str {
        "transcript show"
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        report::PRETTY_TABLE_JSON_JSONL
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn jsonl_rows(&self) -> Option<&[Self::JsonlRow]> {
        Some(&self.events)
    }

    fn render_pretty(&self) -> anyhow::Result<()> {
        render_pretty_transcript(&self.events, &self.prefs, self.view_mode)
    }
}

fn render_pretty_transcript(
    events: &[TranscriptEvent],
    prefs: &crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
) -> anyhow::Result<()> {
    let width = crossterm::terminal::size()
        .map(|(value, _)| value.max(40) as usize)
        .unwrap_or(100);
    let body = render_transcript_snapshot_body(events, prefs, view_mode, width);
    crate::cmd::ui::paginate(&body, prefs)
}

#[derive(Debug, Clone)]
struct TranscriptSnapshotMeta {
    run_id: Option<String>,
    agent: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    workspace_id: Option<String>,
    mode: Option<String>,
    started_at: Option<String>,
    final_status: Option<String>,
    duration_ms: Option<u64>,
    total_tokens: Option<u64>,
    error: Option<String>,
    turn_count: usize,
}

#[derive(Debug, Clone)]
struct TranscriptViewLine {
    status: Status,
    text: String,
    continuation: bool,
    indent: usize,
}

fn render_transcript_snapshot_body(
    events: &[TranscriptEvent],
    prefs: &crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let meta = extract_transcript_snapshot_meta(events);
    let mut rows = Vec::new();
    rows.push(render_transcript_header_line(&meta, view_mode, width));
    rows.push(String::new());
    if let Some(status) = meta.final_status.as_deref() {
        rows.push(render_transcript_kv_row("status", status, width, transcript_status_ui(status)));
    }
    if let Some(agent) = meta.agent.as_deref() {
        let mut detail = agent.to_string();
        if let Some(provider) = meta.provider.as_deref() {
            detail.push_str("  ·  ");
            detail.push_str(provider);
        }
        if let Some(model) = meta.model.as_deref() {
            detail.push_str("  ·  ");
            detail.push_str(model);
        }
        rows.push(render_transcript_kv_row("agent", &detail, width, Status::Active));
    }
    if let Some(workspace_id) = meta.workspace_id.as_deref() {
        rows.push(render_transcript_kv_row("workspace", workspace_id, width, Status::Active));
    }
    if let Some(started_at) = meta.started_at.as_deref() {
        rows.push(render_transcript_kv_row("started", started_at, width, Status::Active));
    }
    if let Some(mode) = meta.mode.as_deref() {
        rows.push(render_transcript_kv_row("mode", mode, width, Status::Active));
    }
    if let Some(error) = meta.error.as_deref() {
        rows.push(render_transcript_kv_row("error", error, width, Status::Failed));
    }
    rows.push(String::new());
    rows.extend(render_transcript_event_rows(events, prefs, view_mode, width));
    rows.push(String::new());
    rows.push(render_transcript_footer_line(&meta, view_mode, width));
    rows.join("\n") + "\n"
}

fn extract_transcript_snapshot_meta(events: &[TranscriptEvent]) -> TranscriptSnapshotMeta {
    let mut meta = TranscriptSnapshotMeta {
        run_id: None,
        agent: None,
        provider: None,
        model: None,
        workspace_id: None,
        mode: None,
        started_at: None,
        final_status: None,
        duration_ms: None,
        total_tokens: None,
        error: None,
        turn_count: 0,
    };
    for event in events {
        match event {
            TranscriptEvent::RunStart {
                run_id,
                workspace_id,
                agent,
                provider,
                model,
                started_at,
                mode,
            } => {
                meta.run_id = Some(run_id.clone());
                meta.workspace_id = Some(workspace_id.clone());
                meta.agent = Some(agent.clone());
                meta.provider = Some(provider.clone());
                meta.model = Some(model.clone());
                meta.started_at = Some(started_at.format("%Y-%m-%d %H:%M:%S UTC").to_string());
                meta.mode = Some(format!("{mode:?}").to_lowercase());
            }
            TranscriptEvent::TurnStart { .. } => {
                meta.turn_count += 1;
            }
            TranscriptEvent::RunComplete {
                status,
                total_tokens,
                duration_ms,
                error,
                ..
            } => {
                meta.final_status = Some(format!("{status:?}").to_lowercase());
                meta.total_tokens = Some(*total_tokens);
                meta.duration_ms = Some(*duration_ms);
                meta.error = error.clone();
            }
            _ => {}
        }
    }
    meta
}

fn render_transcript_header_line(
    meta: &TranscriptSnapshotMeta,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", palette::BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "transcript show", palette::BRAND);
    if let Some(agent) = meta.agent.as_deref() {
        let _ = palette::write_colored(&mut buf, "  ", palette::DIM);
        let _ = palette::write_bold_colored(&mut buf, &truncate_single_line(agent, 24), palette::BRAND);
    }
    if let Some(run_id) = meta.run_id.as_deref() {
        let _ = palette::write_colored(&mut buf, "  ·  ", palette::DIM);
        let _ = palette::write_colored(&mut buf, &compact_run_id(run_id), palette::DIM);
    }
    let _ = palette::write_colored(&mut buf, "  ·  ", palette::DIM);
    let _ = palette::write_colored(&mut buf, view_mode.as_str(), palette::DIM);
    truncate_transcript_ansi_line(&buf, width)
}

fn render_transcript_kv_row(label: &str, value: &str, width: usize, status: Status) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), status.color());
    let _ = palette::write_colored(
        &mut buf,
        &truncate_single_line(value, width.saturating_sub(11)),
        palette::DIM,
    );
    truncate_transcript_ansi_line(&buf, width)
}

fn render_transcript_footer_line(
    meta: &TranscriptSnapshotMeta,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let mut detail = format!("view {}  ·  static snapshot", view_mode.as_str());
    if meta.turn_count > 0 {
        detail.push_str(&format!("  ·  turns {}", meta.turn_count));
    }
    if let Some(total_tokens) = meta.total_tokens {
        detail.push_str(&format!("  ·  total tokens {total_tokens}"));
    }
    if let Some(duration_ms) = meta.duration_ms {
        detail.push_str(&format!("  ·  {}ms", duration_ms));
    }
    render_transcript_kv_row(
        "view",
        &detail,
        width,
        meta.final_status
            .as_deref()
            .map(transcript_status_ui)
            .unwrap_or(Status::Active),
    )
}

fn render_transcript_event_rows(
    events: &[TranscriptEvent],
    prefs: &crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
    width: usize,
) -> Vec<String> {
    let mut view_lines = Vec::new();
    for event in events {
        view_lines.extend(transcript_event_lines(event, prefs, view_mode));
    }
    render_transcript_view_lines(&view_lines, width)
}

fn transcript_event_lines(
    event: &TranscriptEvent,
    prefs: &crate::cmd::ui::UiPrefs,
    view_mode: LiveViewMode,
) -> Vec<TranscriptViewLine> {
    match event {
        TranscriptEvent::RunStart {
            run_id,
            workspace_id,
            mode,
            started_at,
            ..
        } => vec![transcript_event_line(
            Status::Active,
            0,
            false,
            transcript_event_text(
                Status::Active,
                "run started",
                &format!(
                    "{}  ·  workspace {}  ·  mode {}  ·  {}",
                    compact_run_id(run_id),
                    workspace_id,
                    format!("{mode:?}").to_lowercase(),
                    started_at.format("%Y-%m-%d %H:%M:%S UTC")
                ),
            ),
        )],
        TranscriptEvent::TurnStart { turn_idx } => vec![transcript_event_line(
            Status::Working,
            0,
            false,
            transcript_event_text(
                Status::Working,
                &format!("turn {turn_idx}"),
                "assistant turn started",
            ),
        )],
        TranscriptEvent::AssistantMessage { content, thinking } => {
            let mut out = Vec::new();
            if let Some(thinking) = thinking.as_deref().filter(|value| !value.trim().is_empty()) {
                out.push(transcript_event_line(
                    Status::Active,
                    0,
                    false,
                    transcript_event_text(
                        Status::Active,
                        "thinking",
                        &truncate_single_line(thinking, 96),
                    ),
                ));
            }
            if !content.trim().is_empty() {
                match view_mode {
                    LiveViewMode::Focused => out.push(transcript_event_line(
                        Status::Active,
                        0,
                        false,
                        transcript_event_text(
                            Status::Active,
                            "assistant output",
                            &truncate_single_line(content, 96),
                        ),
                    )),
                    LiveViewMode::Compact | LiveViewMode::Full => {
                        let rendered = crate::output::rich_payload::render_assistant_content(
                            content.trim(),
                            prefs,
                        );
                        out.extend(render_payload_body_lines(
                            Status::Active,
                            "assistant output",
                            &rendered.rendered,
                            0,
                        ));
                    }
                }
            }
            out
        }
        TranscriptEvent::ToolCall { tool, input, .. } => {
            let mut out = vec![transcript_event_line(
                Status::Working,
                0,
                false,
                transcript_event_text(Status::Working, &format!("tool {tool}"), &tool_summary(tool, input)),
            )];
            if view_mode == LiveViewMode::Full {
                if let Some(rendered) = crate::output::rich_payload::render_tool_input(tool, input, prefs) {
                    out.extend(render_payload_body_lines(Status::Working, "", &rendered, 1));
                }
            }
            out
        }
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
            let payload = crate::output::rich_payload::render_payload(
                error.as_deref().unwrap_or(output.as_str()),
                prefs,
            );
            let mut detail = truncate_single_line(&payload.headline, 84);
            if *duration_ms > 0 {
                detail.push_str(&format!("  ·  {}ms", duration_ms));
            }
            let mut out = vec![transcript_event_line(
                status,
                0,
                false,
                transcript_event_text(status, label, &detail),
            )];
            match view_mode {
                LiveViewMode::Focused => {}
                LiveViewMode::Compact => {
                    out.extend(
                        crate::output::rich_payload::render_payload_preview_lines(&payload, 5)
                            .into_iter()
                            .map(|line| transcript_event_line(status, 1, true, line)),
                    );
                }
                LiveViewMode::Full => {
                    out.extend(render_payload_body_lines(status, "", &payload.rendered, 1));
                }
            }
            out
        }
        TranscriptEvent::FileEdit { path, kind, diff } => {
            let payload = crate::output::rich_payload::render_payload(diff, prefs);
            let detail = format!(
                "{} {}  ·  {}",
                format!("{kind:?}").to_lowercase(),
                path,
                payload.headline
            );
            let mut out = vec![transcript_event_line(
                Status::Complete,
                0,
                false,
                transcript_event_text(Status::Complete, "file edit", &detail),
            )];
            match view_mode {
                LiveViewMode::Focused => {}
                LiveViewMode::Compact => {
                    out.extend(
                        crate::output::rich_payload::render_payload_preview_lines(&payload, 8)
                            .into_iter()
                            .map(|line| transcript_event_line(Status::Complete, 1, true, line)),
                    );
                }
                LiveViewMode::Full => {
                    out.extend(render_payload_body_lines(
                        Status::Complete,
                        "",
                        &payload.rendered,
                        1,
                    ));
                }
            }
            out
        }
        TranscriptEvent::CommandRun {
            argv,
            cwd,
            exit_code,
            ..
        } => {
            let status = if *exit_code == 0 {
                Status::Complete
            } else {
                Status::Failed
            };
            vec![transcript_event_line(
                status,
                0,
                false,
                transcript_event_text(
                    status,
                    "command",
                    &format!(
                        "{}  ·  cwd {}  ·  exit {}",
                        truncate_single_line(&argv.join(" "), 64),
                        truncate_single_line(cwd, 24),
                        exit_code
                    ),
                ),
            )]
        }
        TranscriptEvent::ActionEmitted {
            kind,
            allowed,
            applied,
            reason,
            ..
        } => {
            let status = if *applied {
                Status::Complete
            } else if *allowed {
                Status::Awaiting
            } else {
                Status::Failed
            };
            let mut detail = format!("{kind}  ·  allowed={allowed} applied={applied}");
            if let Some(reason) = reason.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(reason, 64));
            }
            vec![transcript_event_line(
                status,
                0,
                false,
                transcript_event_text(status, "action", &detail),
            )]
        }
        TranscriptEvent::GateRequested {
            gate_id,
            prompt,
            decision,
            decided_by,
        } => {
            let mut detail = format!("{gate_id}  ·  {}", truncate_single_line(prompt, 72));
            if let Some(decision) = decision.as_deref() {
                detail.push_str(&format!("  ·  decision {decision}"));
            }
            if let Some(decided_by) = decided_by.as_deref() {
                detail.push_str(&format!("  ·  by {decided_by}"));
            }
            vec![transcript_event_line(
                Status::Awaiting,
                0,
                false,
                transcript_event_text(Status::Awaiting, "approval gate", &detail),
            )]
        }
        TranscriptEvent::TurnEnd {
            turn_idx,
            tokens_in,
            tokens_out,
        } => vec![transcript_event_line(
            Status::Complete,
            0,
            false,
            transcript_event_text(
                Status::Complete,
                "turn complete",
                &format!(
                    "turn {turn_idx}  ·  in {} out {}",
                    tokens_in.unwrap_or(0),
                    tokens_out.unwrap_or(0)
                ),
            ),
        )],
        TranscriptEvent::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => vec![transcript_event_line(
            Status::Active,
            0,
            false,
            transcript_event_text(
                Status::Active,
                "usage",
                &format!(
                    "{provider} · {model}  ·  in {input_tokens} out {output_tokens} cached {cached_tokens}"
                ),
            ),
        )],
        TranscriptEvent::RunComplete {
            status,
            total_tokens,
            duration_ms,
            error,
            ..
        } => {
            let ui_status = match status {
                RunStatus::Ok => Status::Complete,
                RunStatus::Error | RunStatus::Aborted => Status::Failed,
            };
            let mut detail = format!(
                "status {}  ·  {}ms  ·  {} tokens",
                format!("{status:?}").to_lowercase(),
                duration_ms,
                total_tokens
            );
            if let Some(error) = error.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(error, 72));
            }
            vec![transcript_event_line(
                ui_status,
                0,
                false,
                transcript_event_text(ui_status, "run complete", &detail),
            )]
        }
    }
}

fn transcript_event_line(
    status: Status,
    indent: usize,
    continuation: bool,
    text: String,
) -> TranscriptViewLine {
    TranscriptViewLine {
        status,
        text,
        continuation,
        indent,
    }
}

fn transcript_event_text(status: Status, label: &str, detail: &str) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, status.color());
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", palette::DIM);
        let _ = palette::write_colored(&mut buf, detail, palette::DIM);
    }
    buf
}

fn render_payload_body_lines(
    status: Status,
    label: &str,
    rendered: &str,
    indent: usize,
) -> Vec<TranscriptViewLine> {
    let mut out = Vec::new();
    let mut lines = rendered.lines();
    if let Some(first) = lines.next() {
        if label.is_empty() {
            out.push(transcript_event_line(status, indent, true, first.to_string()));
        } else {
            out.push(transcript_event_line(
                status,
                indent,
                false,
                transcript_event_text_raw(status, label, first),
            ));
        }
        for line in lines {
            out.push(transcript_event_line(status, indent, true, line.to_string()));
        }
    }
    out
}

fn transcript_event_text_raw(status: Status, label: &str, detail: &str) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, status.color());
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", palette::DIM);
        buf.push_str(detail);
    }
    buf
}

fn render_transcript_view_lines(lines: &[TranscriptViewLine], width: usize) -> Vec<String> {
    let mut rendered = Vec::new();
    for line in lines {
        let prefix = transcript_line_prefix(line);
        let content_width = width.saturating_sub(visible_len(&prefix)).max(1);
        for (idx, segment) in wrap_with_ansi(&line.text, content_width)
            .into_iter()
            .enumerate()
        {
            if idx == 0 && !line.continuation {
                rendered.push(format!("{prefix}{segment}"));
            } else {
                rendered.push(format!(
                    "{}{}",
                    transcript_continuation_prefix(line),
                    segment
                ));
            }
        }
    }
    rendered
}

fn transcript_line_prefix(line: &TranscriptViewLine) -> String {
    let mut value = transcript_indent_prefix(line.indent);
    let _ = palette::write_bold_colored(
        &mut value,
        &line.status.glyph().to_string(),
        line.status.color(),
    );
    value.push(' ');
    value
}

fn transcript_continuation_prefix(line: &TranscriptViewLine) -> String {
    let mut value = transcript_indent_prefix(line.indent);
    let _ = palette::write_colored(&mut value, "│  ", palette::BRAND);
    value
}

fn transcript_indent_prefix(indent: usize) -> String {
    let mut value = String::new();
    for _ in 0..indent {
        let _ = palette::write_colored(&mut value, "│  ", palette::BRAND);
    }
    value
}

fn truncate_transcript_ansi_line(value: &str, width: usize) -> String {
    if visible_len(value) <= width {
        value.to_string()
    } else {
        wrap_with_ansi(value, width)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

fn transcript_status_ui(status: &str) -> Status {
    match status {
        "ok" | "completed" => Status::Complete,
        "error" | "aborted" | "failed" | "rejected" => Status::Failed,
        "awaiting_approval" => Status::Awaiting,
        "running" => Status::Working,
        _ => Status::Active,
    }
}

pub(crate) fn render_pretty_transcript_event(
    printer: &mut LineStreamPrinter,
    event: &TranscriptEvent,
    saw_header: &mut bool,
    view_mode: LiveViewMode,
    context: TranscriptPrettyContext,
) {
    match event {
        TranscriptEvent::RunStart {
            run_id,
            workspace_id,
            agent,
            provider,
            model,
            started_at,
            mode,
        } => {
            if context == TranscriptPrettyContext::Standalone {
                printer.agent_header(agent, provider, model, run_id);
            }
            let detail = format!(
                "{}  ·  workspace {workspace_id}  ·  mode {}  ·  {}",
                compact_run_id(run_id),
                format!("{mode:?}").to_lowercase(),
                started_at.format("%Y-%m-%d %H:%M:%S UTC")
            );
            printer.sideband_event(Status::Active, "run started", Some(&detail));
            *saw_header = true;
        }
        TranscriptEvent::TurnStart { turn_idx } => {
            printer.sideband_event(
                Status::Working,
                &format!("turn {turn_idx}"),
                Some("assistant turn started"),
            );
        }
        TranscriptEvent::AssistantMessage { content, thinking } => {
            if let Some(thinking) = thinking.as_deref().filter(|value| !value.trim().is_empty()) {
                let detail = truncate_single_line(thinking, 96);
                printer.sideband_event(Status::Active, "thinking", Some(&detail));
            }
            if !content.trim().is_empty() {
                match view_mode {
                    LiveViewMode::Full => printer.assistant_chunk(content),
                    LiveViewMode::Focused => printer.sideband_event(
                        Status::Active,
                        "assistant output",
                        Some(&truncate_single_line(content, 96)),
                    ),
                    LiveViewMode::Compact => {}
                }
            }
        }
        TranscriptEvent::ToolCall { tool, input, .. } => {
            printer.tool_call(tool, &tool_summary(tool, input));
        }
        TranscriptEvent::ToolResult {
            output,
            error,
            duration_ms,
            ..
        } => {
            let label = if error.is_some() {
                "tool error"
            } else {
                "tool result"
            };
            let status = if error.is_some() {
                Status::Failed
            } else {
                Status::Complete
            };
            let payload = crate::output::rich_payload::render_payload(
                error.as_deref().unwrap_or(output.as_str()),
                printer.prefs(),
            );
            let mut detail = truncate_single_line(&payload.headline, 84);
            if *duration_ms > 0 {
                detail.push_str(&format!("  ·  {}ms", duration_ms));
            }
            printer.sideband_event(status, label, Some(&detail));
        }
        TranscriptEvent::FileEdit { path, kind, .. } => {
            let detail = format!("{:?} {}", kind, path).to_lowercase();
            printer.sideband_event(Status::Complete, "file edit", Some(&detail));
        }
        TranscriptEvent::CommandRun {
            argv,
            cwd,
            exit_code,
            ..
        } => {
            let status = if *exit_code == 0 {
                Status::Complete
            } else {
                Status::Failed
            };
            let detail = format!(
                "{}  ·  cwd {}  ·  exit {}",
                truncate_single_line(&argv.join(" "), 64),
                truncate_single_line(cwd, 24),
                exit_code
            );
            printer.sideband_event(status, "command", Some(&detail));
        }
        TranscriptEvent::ActionEmitted {
            kind,
            allowed,
            applied,
            reason,
            ..
        } => {
            let status = if *applied {
                Status::Complete
            } else if *allowed {
                Status::Awaiting
            } else {
                Status::Failed
            };
            let mut detail = format!("{kind}  ·  allowed={allowed} applied={applied}");
            if let Some(reason) = reason.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(reason, 64));
            }
            printer.sideband_event(status, "action", Some(&detail));
        }
        TranscriptEvent::GateRequested {
            gate_id,
            prompt,
            decision,
            decided_by,
        } => {
            let mut detail = format!("{gate_id}  ·  {}", truncate_single_line(prompt, 72));
            if let Some(decision) = decision.as_deref() {
                detail.push_str(&format!("  ·  decision {decision}"));
            }
            if let Some(decided_by) = decided_by.as_deref() {
                detail.push_str(&format!("  ·  by {decided_by}"));
            }
            printer.sideband_event(Status::Awaiting, "approval gate", Some(&detail));
        }
        TranscriptEvent::TurnEnd {
            turn_idx,
            tokens_in,
            tokens_out,
        } => {
            let detail = format!(
                "turn {turn_idx}  ·  in {} out {}",
                tokens_in.unwrap_or(0),
                tokens_out.unwrap_or(0)
            );
            printer.sideband_event(Status::Complete, "turn complete", Some(&detail));
        }
        TranscriptEvent::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => {
            let detail = format!(
                "{provider} · {model}  ·  in {input_tokens} out {output_tokens} cached {cached_tokens}"
            );
            printer.sideband_event(Status::Active, "usage", Some(&detail));
        }
        TranscriptEvent::RunComplete {
            status,
            total_tokens,
            duration_ms,
            error,
            ..
        } => {
            let ui_status = match status {
                RunStatus::Ok => Status::Complete,
                RunStatus::Error | RunStatus::Aborted => Status::Failed,
            };
            let mut detail = format!(
                "status {}  ·  {}ms  ·  {} tokens",
                format!("{status:?}").to_lowercase(),
                duration_ms,
                total_tokens
            );
            if let Some(error) = error.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(error, 72));
            }
            printer.sideband_event(ui_status, "run complete", Some(&detail));
        }
    }
}

pub(crate) fn truncate_single_line(value: &str, max: usize) -> String {
    let squashed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() <= max {
        squashed
    } else {
        let mut out = squashed
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>();
        out.push('…');
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptPrettyContext {
    Standalone,
    SessionAttached,
}

fn compact_run_id(run_id: &str) -> String {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TranscriptScope {
    Active,
    Archived,
}

impl TranscriptScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

async fn list(
    no_color: bool,
    all: bool,
    archived: bool,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let mut paths_to_scan: Vec<(TranscriptScope, PathBuf)> = Vec::new();
    let mut seen = HashSet::new();

    // Collect .jsonl paths from a directory — miss is a silent skip.
    fn collect_jsonl(
        dir: &std::path::Path,
        scope: TranscriptScope,
        seen: &mut HashSet<PathBuf>,
        out: &mut Vec<(TranscriptScope, PathBuf)>,
    ) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            let dedupe_key = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") && seen.insert(dedupe_key) {
                out.push((scope, p));
            }
        }
    }

    let scopes: &[TranscriptScope] = if all {
        &[TranscriptScope::Active, TranscriptScope::Archived]
    } else if archived {
        &[TranscriptScope::Archived]
    } else {
        &[TranscriptScope::Active]
    };

    if let Some(ref proj) = project_root {
        let active_root = proj.join(".rupu/transcripts");
        let archived_root = paths::archived_transcripts_dir(&active_root);
        for &scope in scopes {
            match scope {
                TranscriptScope::Active => {
                    collect_jsonl(&active_root, scope, &mut seen, &mut paths_to_scan)
                }
                TranscriptScope::Archived => {
                    collect_jsonl(&archived_root, scope, &mut seen, &mut paths_to_scan)
                }
            }
        }
    }
    let active_root = global.join("transcripts");
    let archived_root = paths::archived_transcripts_dir(&active_root);
    for &scope in scopes {
        match scope {
            TranscriptScope::Active => {
                collect_jsonl(&active_root, scope, &mut seen, &mut paths_to_scan)
            }
            TranscriptScope::Archived => {
                collect_jsonl(&archived_root, scope, &mut seen, &mut paths_to_scan)
            }
        }
    }

    struct Row {
        run_id: String,
        scope: TranscriptScope,
        title: Option<String>,
        agent: String,
        status: RunStatus,
        total_tokens: u64,
        started_at: chrono::DateTime<chrono::Utc>,
    }

    let mut rows: Vec<Row> = Vec::new();
    for (scope, path) in &paths_to_scan {
        match JsonlReader::summary(path) {
            Ok(s) => rows.push(Row {
                run_id: s.run_id,
                scope: *scope,
                title: s.first_assistant_text,
                agent: s.agent,
                status: s.status,
                total_tokens: s.total_tokens,
                started_at: s.started_at,
            }),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable transcript");
            }
        }
    }

    // Sort newest first.
    rows.sort_by_key(|r| Reverse(r.started_at));

    if rows.is_empty()
        && matches!(
            global_format.unwrap_or(OutputFormat::Table),
            OutputFormat::Table
        )
    {
        println!("(no transcripts yet — `rupu run <agent>` to create one)");
        return Ok(());
    }

    // Resolve UI prefs the same way other list commands do — config +
    // env + flag — so the table honors NO_COLOR / `[ui].color = "never"`.
    let cfg = {
        let global_cfg = global.join("config.toml");
        let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default()
    };
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None, None);
    let report_rows: Vec<TranscriptListRow> = rows
        .iter()
        .map(|row| TranscriptListRow {
            run_id: row.run_id.clone(),
            scope: row.scope.as_str().to_string(),
            title: row.title.clone(),
            agent: row.agent.clone(),
            status: match row.status {
                RunStatus::Ok => "completed".to_string(),
                RunStatus::Error => "failed".to_string(),
                RunStatus::Aborted => "rejected".to_string(),
            },
            total_tokens: row.total_tokens,
            started_at: row.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
        })
        .collect();
    let csv_rows: Vec<TranscriptListCsvRow> = report_rows
        .iter()
        .map(|row| TranscriptListCsvRow {
            run_id: row.run_id.clone(),
            scope: row.scope.clone(),
            title: row.title.clone().unwrap_or_default(),
            agent: row.agent.clone(),
            status: row.status.clone(),
            total_tokens: row.total_tokens,
            started_at: row.started_at.clone(),
        })
        .collect();
    let output = TranscriptListOutput {
        prefs,
        report: TranscriptListReport {
            kind: "transcript_list",
            version: 1,
            rows: report_rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

async fn show(
    run_id: &str,
    view: Option<LiveViewMode>,
    no_color: bool,
    pager_flag: Option<bool>,
    global_format: Option<OutputFormat>,
) -> anyhow::Result<()> {
    let path = locate_transcript(run_id)?.transcript_path;
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = rupu_config::layer_files(
        Some(&global.join("config.toml")),
        project_root
            .as_deref()
            .map(|root| root.join(".rupu/config.toml"))
            .as_deref(),
    )?;
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, pager_flag, view);
    let mut events = Vec::new();
    let mut raw_events = Vec::new();
    for event in JsonlReader::iter(&path)? {
        let event = event?;
        raw_events.push(event.clone());
        events.push(serde_json::to_value(event)?);
    }
    let view_mode = prefs.live_view;
    let output = TranscriptShowOutput {
        prefs,
        events: raw_events,
        view_mode,
        report: TranscriptShowReport {
            kind: "transcript_show",
            version: 1,
            item: TranscriptShowItem {
                run_id: run_id.to_string(),
                path: path.display().to_string(),
                events,
            },
        },
    };
    report::emit_event(global_format, &output)
}

struct TranscriptLocation {
    transcript_path: PathBuf,
    metadata_path: PathBuf,
    archived: bool,
}

async fn archive(run_id: &str) -> anyhow::Result<()> {
    let location = locate_transcript(run_id)?;
    if location.archived {
        anyhow::bail!("transcript already archived: {run_id}");
    }
    let mut metadata = load_metadata_if_present(&location)?;
    ensure_standalone_transcript(run_id, metadata.as_ref())?;
    let archived_dir = paths::archived_transcripts_dir(
        location
            .transcript_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("transcript has no parent directory"))?,
    );
    fs::create_dir_all(&archived_dir)?;
    let archived_transcript = archived_dir.join(format!("{run_id}.jsonl"));
    let archived_metadata = metadata_path_for_run(&archived_dir, run_id);
    move_if_exists(&location.transcript_path, &archived_transcript)?;
    if let Some(meta) = metadata.as_mut() {
        meta.archived_at = Some(chrono::Utc::now().to_rfc3339());
        crate::standalone_run_metadata::write_metadata(&archived_metadata, meta)?;
        remove_file_if_exists(&location.metadata_path)?;
    } else {
        move_if_exists(&location.metadata_path, &archived_metadata)?;
    }
    println!("archived transcript {run_id}");
    Ok(())
}

async fn delete(args: DeleteArgs) -> anyhow::Result<()> {
    if !args.force {
        anyhow::bail!("transcript delete requires --force");
    }
    let location = locate_transcript(&args.run_id)?;
    ensure_standalone_transcript(&args.run_id, load_metadata_if_present(&location)?.as_ref())?;
    remove_file_if_exists(&location.transcript_path)?;
    remove_file_if_exists(&location.metadata_path)?;
    println!("deleted transcript {}", args.run_id);
    Ok(())
}

async fn prune(args: PruneArgs, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let mut pruned = prune_archived_transcripts(args.older_than.as_deref(), args.dry_run)?;
    pruned.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    let rows = pruned
        .iter()
        .map(|row| TranscriptPruneRow {
            run_id: row.run_id.clone(),
            scope: row.scope.clone(),
            location: row.location.clone(),
            archived_at: row.archived_at.clone(),
            action: row.action.clone(),
        })
        .collect::<Vec<_>>();
    let csv_rows = pruned
        .iter()
        .map(|row| TranscriptPruneCsvRow {
            run_id: row.run_id.clone(),
            scope: row.scope.clone(),
            location: row.location.clone(),
            archived_at: row.archived_at.clone(),
            action: row.action.clone(),
        })
        .collect();
    let output = TranscriptPruneOutput {
        report: TranscriptPruneReport {
            kind: "transcript_prune",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

pub(crate) fn prune_archived_transcripts(
    older_than: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<Vec<PrunedTranscript>> {
    let global = paths::global_dir()?;
    let cutoff = prune_cutoff(older_than, &global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let mut rows = Vec::new();
    for location in scan_archived_transcripts(&global, project_root.as_deref())? {
        let Some(archived_at) = archived_at_for_location(&location)? else {
            continue;
        };
        if archived_at > cutoff {
            continue;
        }
        let Some(run_id) = run_id_from_transcript_path(&location.transcript_path) else {
            continue;
        };
        let metadata = load_metadata_if_present(&location)?;
        if metadata
            .as_ref()
            .and_then(|value| value.session_id.as_ref())
            .is_some()
        {
            continue;
        }
        let scope = if location
            .transcript_path
            .starts_with(global.join("transcripts"))
        {
            "global"
        } else {
            "project"
        };
        rows.push(PrunedTranscript {
            run_id: run_id.clone(),
            scope: scope.to_string(),
            location: location.transcript_path.display().to_string(),
            archived_at: archived_at.to_rfc3339(),
            action: if dry_run {
                "would_delete".into()
            } else {
                "deleted".into()
            },
        });
        if !dry_run {
            remove_file_if_exists(&location.transcript_path)?;
            remove_file_if_exists(&location.metadata_path)?;
        }
    }
    Ok(rows)
}

fn locate_transcript(run_id: &str) -> anyhow::Result<TranscriptLocation> {
    let filename = format!("{run_id}.jsonl");

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Project-local first, active then archived.
    if let Some(ref proj) = project_root {
        let active_root = proj.join(".rupu/transcripts");
        let candidate = active_root.join(&filename);
        if candidate.is_file() {
            return Ok(TranscriptLocation {
                metadata_path: metadata_path_for_run(&active_root, run_id),
                transcript_path: candidate,
                archived: false,
            });
        }
        let archived_root = paths::archived_transcripts_dir(&active_root);
        let archived_candidate = archived_root.join(&filename);
        if archived_candidate.is_file() {
            return Ok(TranscriptLocation {
                metadata_path: metadata_path_for_run(&archived_root, run_id),
                transcript_path: archived_candidate,
                archived: true,
            });
        }
    }

    // Global fallback, active then archived.
    let active_root = global.join("transcripts");
    let candidate = active_root.join(&filename);
    if candidate.is_file() {
        return Ok(TranscriptLocation {
            metadata_path: metadata_path_for_run(&active_root, run_id),
            transcript_path: candidate,
            archived: false,
        });
    }
    let archived_root = paths::archived_transcripts_dir(&active_root);
    let archived_candidate = archived_root.join(&filename);
    if archived_candidate.is_file() {
        return Ok(TranscriptLocation {
            metadata_path: metadata_path_for_run(&archived_root, run_id),
            transcript_path: archived_candidate,
            archived: true,
        });
    }

    Err(anyhow::anyhow!("transcript not found: {run_id}"))
}

fn ensure_standalone_transcript(
    run_id: &str,
    metadata: Option<&crate::standalone_run_metadata::StandaloneRunMetadata>,
) -> anyhow::Result<()> {
    let Some(metadata) = metadata else {
        return Ok(());
    };
    if metadata.session_id.is_some() {
        anyhow::bail!(
            "transcript {} is managed by session {}; use `rupu session archive|delete` instead",
            run_id,
            metadata.session_id.as_deref().unwrap_or("unknown")
        );
    }
    Ok(())
}

fn load_metadata_if_present(
    location: &TranscriptLocation,
) -> anyhow::Result<Option<crate::standalone_run_metadata::StandaloneRunMetadata>> {
    if !location.metadata_path.is_file() {
        return Ok(None);
    }
    Ok(Some(read_metadata(&location.metadata_path)?))
}

fn scan_archived_transcripts(
    global: &std::path::Path,
    project_root: Option<&std::path::Path>,
) -> anyhow::Result<Vec<TranscriptLocation>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_dir = |root: &std::path::Path| -> anyhow::Result<()> {
        if !root.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let dedupe_key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(dedupe_key) {
                continue;
            }
            let Some(run_id) = run_id_from_transcript_path(&path) else {
                continue;
            };
            out.push(TranscriptLocation {
                metadata_path: metadata_path_for_run(root, &run_id),
                transcript_path: path,
                archived: true,
            });
        }
        Ok(())
    };
    if let Some(project_root) = project_root {
        push_dir(&paths::archived_transcripts_dir(
            &project_root.join(".rupu/transcripts"),
        ))?;
    }
    push_dir(&paths::archived_transcripts_dir(
        &global.join("transcripts"),
    ))?;
    Ok(out)
}

fn archived_at_for_location(
    location: &TranscriptLocation,
) -> anyhow::Result<Option<chrono::DateTime<chrono::Utc>>> {
    if let Some(metadata) = load_metadata_if_present(location)? {
        if let Some(value) = metadata.archived_at.as_deref() {
            return Ok(Some(
                chrono::DateTime::parse_from_rfc3339(value)?.with_timezone(&chrono::Utc),
            ));
        }
    }
    let modified = fs::metadata(&location.transcript_path)?.modified()?;
    Ok(Some(chrono::DateTime::<chrono::Utc>::from(modified)))
}

fn run_id_from_transcript_path(path: &std::path::Path) -> Option<String> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
}

fn prune_cutoff(
    older_than: Option<&str>,
    global: &std::path::Path,
) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    let retention = if let Some(value) = older_than {
        value.to_string()
    } else {
        let path = global.join("config.toml");
        let cfg = rupu_config::layer_files(Some(&path), None)?;
        cfg.storage
            .archived_transcript_retention
            .unwrap_or_else(|| "30d".to_string())
    };
    Ok(chrono::Utc::now() - parse_retention_duration(&retention)?)
}

fn move_if_exists(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    if !from.exists() || from == to {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if to.exists() {
        fs::remove_file(to)?;
    }
    fs::rename(from, to)?;
    Ok(())
}

fn remove_file_if_exists(path: &std::path::Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_preview_passes_short_text_through() {
        assert_eq!(one_line_preview("hello", 60), "hello");
    }

    #[test]
    fn one_line_preview_collapses_newlines_and_runs() {
        assert_eq!(
            one_line_preview("  hello\n\nworld   again  ", 60),
            "hello world again"
        );
    }

    #[test]
    fn one_line_preview_truncates_with_ellipsis() {
        let input = "a".repeat(80);
        let out = one_line_preview(&input, 20);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 20);
    }

    #[test]
    fn one_line_preview_empty_after_trim() {
        assert_eq!(one_line_preview("   \n\n  ", 60), "");
    }
}
