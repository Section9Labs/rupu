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
        /// Skip the still-running check; only use when the recorded pid was
        /// reused by an unrelated process — deleting a live run's transcript
        /// loses it.
        #[arg(long)]
        ignore_liveness: bool,
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
    /// Skip the still-running check; only use when the recorded pid was
    /// reused by an unrelated process — deleting a live run's transcript
    /// loses it. Independent of `--force`: both are required to delete a
    /// transcript that still looks live.
    #[arg(long)]
    pub ignore_liveness: bool,
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

pub async fn handle(
    action: Action,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> ExitCode {
    let result = match action {
        Action::List {
            no_color,
            all,
            archived,
        } => {
            list(
                no_color,
                all,
                archived,
                global_format,
                absolute,
                all_columns,
            )
            .await
        }
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
        Action::Archive {
            run_id,
            ignore_liveness,
        } => archive(&run_id, ignore_liveness).await,
        Action::Delete(args) => delete(args).await,
        Action::Prune(args) => prune(args, global_format, absolute, all_columns).await,
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
    prefs: crate::cmd::ui::UiPrefs,
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
        println!(
            "{}",
            render_transcript_list_table(&self.report.rows, &self.prefs, chrono::Utc::now())
        );
        Ok(())
    }
}

/// Build the `transcript list` human table: compacted run ids, coloured
/// SCOPE/STATUS, a one-line TITLE preview (or an em dash), relative
/// start times, and a STATUS breakdown in the summary. Extracted from
/// `render_table` so it can be asserted directly against its returned
/// string instead of through captured stdout. `now` is a parameter so
/// tests are deterministic; `render_table` passes `Utc::now()`.
fn render_transcript_list_table(
    rows: &[TranscriptListRow],
    prefs: &crate::cmd::ui::UiPrefs,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    build_transcript_list_table(rows, prefs, now).render(now)
}

/// Build the `transcript list` table's `EntityTable`. Split out of
/// `render_transcript_list_table` so tests can force a narrow render
/// width (`EntityTable::render_at_width`) to exercise the AGENT
/// column's wrapping under real squeeze pressure — `render_transcript_list_table`
/// itself only exposes the production `render` path, which picks up the
/// real terminal width and cannot be forced narrow in a test.
fn build_transcript_list_table<'a>(
    rows: &[TranscriptListRow],
    prefs: &'a crate::cmd::ui::UiPrefs,
    // Unused today (STARTED is a `CellValue::Timestamp`, which only
    // needs `now` at `render` time), kept in the signature to mirror
    // `render_transcript_list_table` / `cmd::workflow`'s
    // `build_workflow_list_table` and so a future hand-built relative
    // cell here doesn't need a signature change to get it.
    _now: chrono::DateTime<chrono::Utc>,
) -> crate::output::entity_table::EntityTable<'a> {
    use crate::output::entity_table::{CellValue, EntityTable};

    let mut table = EntityTable::new(
        prefs,
        prefs.render_opts(),
        vec![
            "RUN ID", "SCOPE", "TITLE", "AGENT", "STATUS", "TOKENS", "STARTED",
        ],
    )
    .with_summary("transcript");

    for row in rows {
        // `TranscriptListRow.started_at` is written (`list()`) as
        // `row.started_at.format("%Y-%m-%d %H:%M:%S")` — the same naive
        // format `render_workflow_runs_table` parses (workflow.rs). Try
        // that exact format first (the source `DateTime<Utc>` was UTC
        // before stringifying, so interpreting the naive result as UTC
        // is correct, not a guess), fall back to RFC3339 for any other
        // writer of this field, and finally to the verbatim text so a
        // malformed value never drops the row.
        let started = chrono::NaiveDateTime::parse_from_str(&row.started_at, "%Y-%m-%d %H:%M:%S")
            .map(|naive| CellValue::Timestamp(naive.and_utc()))
            .or_else(|_| {
                chrono::DateTime::parse_from_rfc3339(&row.started_at)
                    .map(|ts| CellValue::Timestamp(ts.with_timezone(&chrono::Utc)))
            })
            .unwrap_or_else(|_| CellValue::Text(row.started_at.clone()));
        table = table.row(vec![
            CellValue::Id(row.run_id.clone()),
            CellValue::Status(row.scope.clone()),
            row.title
                .clone()
                .map(|t| CellValue::Text(one_line_preview(&t, 60)))
                .unwrap_or(CellValue::Missing),
            // I-1: AGENT is plain `CellValue::Text`, not `CellValue::Name`.
            // An agent name comes from that agent file's own `name:`
            // frontmatter and nobody bounds its length; `Name`'s no-wrap
            // `ContentWidth` constraint has no upper bound, so a long
            // agent name at a normal terminal width starved every other
            // column (SCOPE/TITLE/STATUS/TOKENS/STARTED down to one
            // character per line) instead of wrapping onto its own row.
            // This matches `session list`, which already renders the
            // same concept (its AGENT column) as `Text`.
            CellValue::Text(row.agent.clone()),
            CellValue::Status(row.status.clone()),
            CellValue::Text(row.total_tokens.to_string()),
            started,
        ]);
    }
    table
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
        println!(
            "{}",
            render_transcript_prune_table(&self.report.rows, &self.prefs, chrono::Utc::now())
        );
        Ok(())
    }
}

/// Build the `transcript prune` human table: compacted run ids, coloured
/// SCOPE (`global` / `project`) and ACTION (`would_delete` / `deleted`),
/// and `ARCHIVED` rendered relative unless `--absolute`. No summary
/// line: `ACTION` isn't literally `STATUS`, and renaming it to force a
/// breakdown would misrepresent the column. Extracted from
/// `render_table` so it can be asserted directly against its returned
/// string; `now` is a parameter so tests are deterministic.
fn render_transcript_prune_table(
    rows: &[TranscriptPruneRow],
    prefs: &crate::cmd::ui::UiPrefs,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    use crate::output::entity_table::{CellValue, EntityTable};

    let mut table = EntityTable::new(
        prefs,
        prefs.render_opts(),
        vec!["RUN ID", "SCOPE", "LOCATION", "ARCHIVED", "ACTION"],
    );

    for row in rows {
        // `archived_at` is written (`prune_archived_transcripts`) via
        // `archived_at.to_rfc3339()` — try RFC3339 first, falling back
        // to the verbatim text so a malformed value never drops the row.
        let archived = chrono::DateTime::parse_from_rfc3339(&row.archived_at)
            .map(|ts| CellValue::Timestamp(ts.with_timezone(&chrono::Utc)))
            .unwrap_or_else(|_| CellValue::Text(row.archived_at.clone()));
        table = table.row(vec![
            CellValue::Id(row.run_id.clone()),
            CellValue::Status(row.scope.clone()),
            CellValue::Text(row.location.clone()),
            archived,
            CellValue::Status(row.action.clone()),
        ]);
    }
    table.render(now)
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
        rows.push(render_transcript_kv_row(
            "status",
            status,
            width,
            transcript_status_ui(status),
        ));
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
        rows.push(render_transcript_kv_row(
            "agent",
            &detail,
            width,
            Status::Active,
        ));
    }
    if let Some(workspace_id) = meta.workspace_id.as_deref() {
        rows.push(render_transcript_kv_row(
            "workspace",
            workspace_id,
            width,
            Status::Active,
        ));
    }
    if let Some(started_at) = meta.started_at.as_deref() {
        rows.push(render_transcript_kv_row(
            "started",
            started_at,
            width,
            Status::Active,
        ));
    }
    if let Some(mode) = meta.mode.as_deref() {
        rows.push(render_transcript_kv_row(
            "mode",
            mode,
            width,
            Status::Active,
        ));
    }
    if let Some(error) = meta.error.as_deref() {
        rows.push(render_transcript_kv_row(
            "error",
            error,
            width,
            Status::Failed,
        ));
    }
    rows.push(String::new());
    rows.extend(render_transcript_event_rows(
        events, prefs, view_mode, width,
    ));
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
        let _ =
            palette::write_bold_colored(&mut buf, &truncate_single_line(agent, 24), palette::BRAND);
    }
    if let Some(run_id) = meta.run_id.as_deref() {
        let _ = palette::write_colored(&mut buf, "  ·  ", palette::DIM);
        let _ = palette::write_colored(
            &mut buf,
            &crate::output::ids::compact_id(run_id),
            palette::DIM,
        );
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
                    crate::output::ids::compact_id(run_id),
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
        TranscriptEvent::AssistantDelta { .. } => Vec::new(),
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
        TranscriptEvent::ToolAudit {
            tool,
            declared,
            granted,
            blocked,
            ..
        } => {
            let status = if *blocked { Status::Failed } else { Status::Complete };
            let detail = format!("{tool}  ·  declared={declared} granted={granted} blocked={blocked}");
            vec![transcript_event_line(
                status,
                0,
                false,
                transcript_event_text(status, "tool audit", &detail),
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
            ..
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
            out.push(transcript_event_line(
                status,
                indent,
                true,
                first.to_string(),
            ));
        } else {
            out.push(transcript_event_line(
                status,
                indent,
                false,
                transcript_event_text_raw(status, label, first),
            ));
        }
        for line in lines {
            out.push(transcript_event_line(
                status,
                indent,
                true,
                line.to_string(),
            ));
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
                crate::output::ids::compact_id(run_id),
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
        TranscriptEvent::AssistantDelta { .. } => {}
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
        TranscriptEvent::ToolAudit {
            tool,
            declared,
            granted,
            blocked,
            ..
        } => {
            let status = if *blocked {
                Status::Failed
            } else {
                Status::Complete
            };
            let detail =
                format!("{tool}  ·  declared={declared} granted={granted} blocked={blocked}");
            printer.sideband_event(status, "tool audit", Some(&detail));
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
            ..
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
    absolute: bool,
    all_columns: bool,
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
        // UI prefs only — lock does not apply (I-7)
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default()
    };
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None, None)
        .with_table_flags(absolute, all_columns);
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
    let location = locate_transcript(run_id)?;
    let path = location.transcript_path;
    let run_id = location.run_id.as_str();
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    // UI prefs only — lock does not apply (I-7)
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
    /// The resolved run id — always the full id, even when
    /// `locate_transcript` was called with a fragment. Callers that
    /// build further paths or messages from a run id MUST use this
    /// field, not whatever string the user originally typed: a fragment
    /// used verbatim would point at a file that doesn't exist.
    run_id: String,
    transcript_path: PathBuf,
    metadata_path: PathBuf,
    archived: bool,
}

async fn archive(run_id: &str, ignore_liveness: bool) -> anyhow::Result<()> {
    let location = locate_transcript(run_id)?;
    // Resolved full id from here on — `run_id` may have been a
    // fragment, and paths built below must use the real id.
    let run_id = location.run_id.as_str();
    if location.archived {
        anyhow::bail!("transcript already archived: {run_id}");
    }
    let mut metadata = load_metadata_if_present(&location)?;
    ensure_standalone_transcript(run_id, metadata.as_ref())?;
    if !ignore_liveness {
        ensure_standalone_not_running(run_id, "archive", metadata.as_ref())?;
    }
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
    // Resolved full id — see the comment in `archive`.
    let run_id = location.run_id.as_str();
    let metadata = load_metadata_if_present(&location)?;
    ensure_standalone_transcript(run_id, metadata.as_ref())?;
    if !args.ignore_liveness {
        ensure_standalone_not_running(run_id, "delete", metadata.as_ref())?;
    }
    remove_file_if_exists(&location.transcript_path)?;
    remove_file_if_exists(&location.metadata_path)?;
    println!("deleted transcript {run_id}");
    Ok(())
}

async fn prune(
    args: PruneArgs,
    global_format: Option<OutputFormat>,
    absolute: bool,
    all_columns: bool,
) -> anyhow::Result<()> {
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
    let prefs = prune_ui_prefs()?.with_table_flags(absolute, all_columns);
    let output = TranscriptPruneOutput {
        prefs,
        report: TranscriptPruneReport {
            kind: "transcript_prune",
            version: 1,
            rows,
        },
        csv_rows,
    };
    report::emit_collection(global_format, &output)
}

/// `transcript prune` has no `--no-color` flag of its own (unlike
/// `list`/`show`) — resolve UI prefs the same way `autoflow_ui_prefs`
/// does for its own flagless commands: config + env only, `no_color`
/// hardcoded `false` so `NO_COLOR` / `[ui].color = "never"` still work.
fn prune_ui_prefs() -> anyhow::Result<crate::cmd::ui::UiPrefs> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    // UI prefs only — lock does not apply (I-7)
    let cfg =
        rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref()).unwrap_or_default();
    Ok(crate::cmd::ui::UiPrefs::resolve(
        &cfg.ui, false, None, None, None,
    ))
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

/// Resolve `fragment` to a transcript and locate it on disk.
///
/// Tries an exact-id lookup first (the common case of pasting back a
/// full id costs only a handful of `is_file` stats). When that misses,
/// falls back to fragment resolution over every transcript id actually
/// present across the same roots, via the shared [`crate::output::ids`]
/// resolver — the same acceptance rule (compact form / prefix / suffix)
/// as `rupu run show` and `rupu session show`, so an id this CLI prints
/// (e.g. the compact form `transcript show` puts in its own header) is
/// always an id it will accept back.
fn locate_transcript(fragment: &str) -> anyhow::Result<TranscriptLocation> {
    if let Some(location) = locate_transcript_exact(fragment)? {
        return Ok(location);
    }

    use crate::output::ids::{resolve, Resolution};
    let candidates = transcript_ids_present();
    match resolve(&candidates, fragment) {
        Resolution::Unique(id) => locate_transcript_exact(&id)?
            .ok_or_else(|| anyhow::anyhow!("transcript not found: {id}")),
        Resolution::NotFound => Err(anyhow::anyhow!("unknown transcript: {fragment}")),
        Resolution::Ambiguous(matches) => {
            let mut msg = format!(
                "ambiguous transcript id — {} transcripts match `{fragment}`",
                matches.len()
            );
            for id in &matches {
                msg.push_str(&format!("\n  {id}"));
            }
            Err(anyhow::anyhow!(msg))
        }
    }
}

/// Every transcript id (`.jsonl` stem) present across the project-local
/// and global, active and archived transcript roots — exactly the roots
/// `locate_transcript_exact` probes. Reuses the shell-completion scan
/// (`cmd::completers::transcript_run_ids`) rather than re-walking the
/// same directories a second way; an empty prefix matches everything.
fn transcript_ids_present() -> Vec<String> {
    crate::cmd::completers::transcript_run_ids(std::ffi::OsStr::new(""))
        .into_iter()
        .map(|c| c.get_value().to_string_lossy().into_owned())
        .collect()
}

/// Exact-id-only lookup: the four roots `locate_transcript` searches, in
/// precedence order. Returns `Ok(None)` on a clean miss (not an error —
/// the fragment-resolution fallback in `locate_transcript` needs to tell
/// "no exact file" apart from a real IO failure).
fn locate_transcript_exact(run_id: &str) -> anyhow::Result<Option<TranscriptLocation>> {
    let filename = format!("{run_id}.jsonl");

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Project-local first, active then archived.
    if let Some(ref proj) = project_root {
        let active_root = proj.join(".rupu/transcripts");
        let candidate = active_root.join(&filename);
        if candidate.is_file() {
            return Ok(Some(TranscriptLocation {
                run_id: run_id.to_string(),
                metadata_path: metadata_path_for_run(&active_root, run_id),
                transcript_path: candidate,
                archived: false,
            }));
        }
        let archived_root = paths::archived_transcripts_dir(&active_root);
        let archived_candidate = archived_root.join(&filename);
        if archived_candidate.is_file() {
            return Ok(Some(TranscriptLocation {
                run_id: run_id.to_string(),
                metadata_path: metadata_path_for_run(&archived_root, run_id),
                transcript_path: archived_candidate,
                archived: true,
            }));
        }
    }

    // Global fallback, active then archived.
    let active_root = global.join("transcripts");
    let candidate = active_root.join(&filename);
    if candidate.is_file() {
        return Ok(Some(TranscriptLocation {
            run_id: run_id.to_string(),
            metadata_path: metadata_path_for_run(&active_root, run_id),
            transcript_path: candidate,
            archived: false,
        }));
    }
    let archived_root = paths::archived_transcripts_dir(&active_root);
    let archived_candidate = archived_root.join(&filename);
    if archived_candidate.is_file() {
        return Ok(Some(TranscriptLocation {
            run_id: run_id.to_string(),
            metadata_path: metadata_path_for_run(&archived_root, run_id),
            transcript_path: archived_candidate,
            archived: true,
        }));
    }

    Ok(None)
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

/// Refuse to archive/delete a standalone transcript whose owning process is
/// still alive — `rupu run` writes `.meta.json` BEFORE the agent loop
/// starts, and standalone metadata carries no status field, so an in-flight
/// run is otherwise indistinguishable from a finished one (the CP's
/// `agent_in_lifecycle` even labels it "Completed" by default). Mirrors
/// `ensure_session_not_running`'s dead-pid check
/// (`rupu_orchestrator::runs::pid_is_running`, the same primitive
/// `RunStore::reap_if_orphaned` uses) against the `pid` captured in
/// `StandaloneRunMetadata`.
///
/// A `None` metadata or `None` pid means no liveness signal is available
/// (no metadata file, or metadata predating this field) — proceeds rather
/// than blocking unconditionally, matching `ensure_standalone_transcript`'s
/// same "no metadata → no guard" shape above.
fn ensure_standalone_not_running(
    run_id: &str,
    action: &str,
    metadata: Option<&crate::standalone_run_metadata::StandaloneRunMetadata>,
) -> anyhow::Result<()> {
    let Some(pid) = metadata.and_then(|m| m.pid) else {
        return Ok(());
    };
    if rupu_orchestrator::runs::pid_is_running(pid) {
        anyhow::bail!(
            "cannot {action} transcript {run_id}: it is still running (owning process {pid} is alive)"
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
            let metadata_path = metadata_path_for_run(root, &run_id);
            out.push(TranscriptLocation {
                run_id,
                metadata_path,
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
        let cfg = rupu_config::layer_files_locked(Some(&path), None)?;
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

    fn sample_metadata(pid: Option<u32>) -> crate::standalone_run_metadata::StandaloneRunMetadata {
        crate::standalone_run_metadata::StandaloneRunMetadata {
            version: crate::standalone_run_metadata::StandaloneRunMetadata::VERSION,
            run_id: "run_live".into(),
            session_id: None,
            archived_at: None,
            workspace_path: PathBuf::from("/tmp/repo"),
            project_root: None,
            repo_ref: None,
            issue_ref: None,
            backend_id: "local_checkout".into(),
            worker_id: Some("worker_local_cli".into()),
            trigger_source: "run_cli".into(),
            target: None,
            workspace_strategy: None,
            pid,
        }
    }

    // I4 (final-review finding): a standalone run's `.meta.json` is written
    // before the agent loop starts and carries no status field, so an
    // in-flight run is otherwise indistinguishable from a finished one.
    // `ensure_standalone_not_running` refuses archive/delete while the
    // captured pid is still alive.
    #[test]
    fn ensure_standalone_not_running_refuses_a_live_pid() {
        // Our own pid is guaranteed alive for the duration of the test.
        let own_pid = std::process::id();
        let meta = sample_metadata(Some(own_pid));
        let err = ensure_standalone_not_running("run_live", "delete", Some(&meta)).unwrap_err();
        assert!(
            err.to_string().contains("still running"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ensure_standalone_not_running_allows_a_dead_pid() {
        // pid 1 owned by init/launchd is never our process and, for any pid
        // that IS dead, `pid_is_running` (kill -0) returns false. Use an
        // implausibly large pid instead, which is reliably not a running
        // process on any platform this test runs on.
        let dead_pid = 999_999;
        let meta = sample_metadata(Some(dead_pid));
        ensure_standalone_not_running("run_done", "delete", Some(&meta)).unwrap();
    }

    #[test]
    fn ensure_standalone_not_running_allows_missing_pid_signal() {
        let meta = sample_metadata(None);
        ensure_standalone_not_running("run_legacy", "archive", Some(&meta)).unwrap();
        ensure_standalone_not_running("run_no_meta", "archive", None).unwrap();
    }

    fn transcript_list_test_now() -> chrono::DateTime<chrono::Utc> {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(2026, 7, 30, 17, 0, 0).unwrap()
    }

    fn transcript_list_test_prefs() -> crate::cmd::ui::UiPrefs {
        let cfg = rupu_config::UiConfig::default();
        crate::cmd::ui::UiPrefs::resolve(&cfg, true, None, None, None)
    }

    fn transcript_row_for_test(
        run_id: &str,
        scope: &str,
        title: Option<&str>,
        agent: &str,
        status: &str,
        started_at: &str,
    ) -> TranscriptListRow {
        TranscriptListRow {
            run_id: run_id.to_string(),
            scope: scope.to_string(),
            title: title.map(str::to_string),
            agent: agent.to_string(),
            status: status.to_string(),
            total_tokens: 1_200,
            started_at: started_at.to_string(),
        }
    }

    #[test]
    fn transcript_list_table_compacts_id_and_colours_scope_and_status() {
        let rows = vec![transcript_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "active",
            Some("fix the flaky test"),
            "issue-reader",
            "completed",
            "2026-07-30 13:00:00",
        )];
        let out = render_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("run_01KYSMDN…GKYJ"), "got: {out}");
        assert!(
            !out.contains("run_01KYSMDNG84N9Z8XXHQZP3GKYJ"),
            "full id leaked into a cell: {out}"
        );
        assert!(out.contains("✓ completed"), "got: {out}");
    }

    #[test]
    fn transcript_list_started_at_renders_relative_for_the_real_stored_format() {
        // `TranscriptListRow.started_at` is written (see `list()`) as
        // `row.started_at.format("%Y-%m-%d %H:%M:%S")` — not RFC3339. The
        // real stored format must recover the instant and render a
        // relative age, not fall through to the literal-text branch.
        let rows = vec![transcript_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "active",
            None,
            "issue-reader",
            "completed",
            "2026-07-30 13:00:00",
        )];
        let out = render_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("4h ago"), "got: {out}");
        assert!(
            !out.contains("2026-07-30 13:00:00"),
            "literal text leaked instead of a relative age: {out}"
        );
    }

    #[test]
    fn transcript_list_started_at_renders_iso_under_absolute() {
        // The behavioural proof that Task 2's `--absolute` plumbing
        // actually reaches this renderer, not just clap parsing.
        let rows = vec![transcript_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "active",
            None,
            "issue-reader",
            "completed",
            "2026-07-30 13:00:00",
        )];
        let prefs = transcript_list_test_prefs().with_table_flags(true, false);
        let out = render_transcript_list_table(&rows, &prefs, transcript_list_test_now());
        assert!(out.contains("2026-07-30T13:00:00"), "got: {out}");
        assert!(!out.contains("4h ago"), "got: {out}");
    }

    #[test]
    fn transcript_list_survives_an_unparseable_started_at() {
        let rows = vec![transcript_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "active",
            None,
            "issue-reader",
            "completed",
            "not-a-timestamp",
        )];
        let out = render_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("not-a-timestamp"), "row was dropped: {out}");
    }

    #[test]
    fn transcript_list_missing_title_renders_an_em_dash() {
        // A single row with no title would leave TITLE entirely empty
        // and eligible for suppression — mix in a row that does have
        // one so the column survives and the em dash is visible.
        let rows = vec![
            transcript_row_for_test(
                "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
                "active",
                Some("has a title"),
                "issue-reader",
                "completed",
                "2026-07-30 13:00:00",
            ),
            transcript_row_for_test(
                "run_01KYPASX18NYRER5NQPDWB2HZV",
                "active",
                None,
                "issue-reader",
                "completed",
                "2026-07-29 07:00:43",
            ),
        ];
        let out = render_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains('—'), "got: {out}");
        assert!(out.contains("has a title"), "got: {out}");
    }

    #[test]
    fn transcript_list_summary_breaks_down_by_status() {
        let rows = vec![
            transcript_row_for_test(
                "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
                "active",
                Some("first run"),
                "issue-reader",
                "completed",
                "2026-07-30 13:00:00",
            ),
            transcript_row_for_test(
                "run_01KYPASX18NYRER5NQPDWB2HZV",
                "active",
                None,
                "issue-reader",
                "failed",
                "2026-07-29 07:00:43",
            ),
        ];
        let out = render_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(
            out.contains("2 transcripts · 1 completed · 1 failed"),
            "got: {out}"
        );
    }

    #[test]
    fn transcript_list_agent_renders_verbatim() {
        // AGENT is `CellValue::Text` (I-1) — rendered verbatim, just no
        // longer no-wrap. The narrow-squeeze regression guard for the
        // fix itself is `transcript_list_long_agent_name_does_not_collapse_other_columns`
        // below.
        let rows = vec![transcript_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "active",
            None,
            "issue-reader",
            "completed",
            "2026-07-30 13:00:00",
        )];
        let out = render_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("issue-reader"), "got: {out}");
    }

    #[test]
    fn transcript_list_long_agent_name_does_not_collapse_other_columns() {
        // I-1 regression guard: AGENT used to map to `CellValue::Name`,
        // whose no-wrap `ContentWidth` constraint has no upper bound —
        // an agent name comes from that agent file's own `name:`
        // frontmatter and nobody bounds its length. A 63-char agent name
        // at 80 columns squeezed SCOPE/TITLE/STATUS/TOKENS/STARTED down
        // to one character per line. AGENT is now plain `CellValue::Text`
        // (matching `session list`), which wraps under pressure instead
        // of starving its neighbours. Force real squeeze pressure via
        // the shared `render_at_width` test hook, since production
        // `render_transcript_list_table` only exposes real-terminal-width
        // rendering, and assert a known value from a later column
        // (TOKENS) still renders intact on one line.
        //
        // Verified this actually discriminates the bug: temporarily
        // reverting AGENT to `CellValue::Name` and re-running this exact
        // scenario prints TOKENS as `1\n2\n0\n0` (and every other column
        // similarly one-character-per-line) — "1200" does not appear as
        // a contiguous substring. With `CellValue::Text`, TOKENS wraps
        // normally and "1200" survives intact.
        let long_agent = "a".repeat(63);
        let rows = vec![transcript_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "active",
            Some("fix the flaky test"),
            &long_agent,
            "completed",
            "2026-07-30 13:00:00",
        )];
        let out = build_transcript_list_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        )
        .render_at_width(transcript_list_test_now(), 80);
        assert!(
            out.contains("1200"),
            "TOKENS column collapsed under a long AGENT name: {out}"
        );
    }

    fn prune_row(
        run_id: &str,
        scope: &str,
        location: &str,
        archived_at: &str,
        action: &str,
    ) -> TranscriptPruneRow {
        TranscriptPruneRow {
            run_id: run_id.to_string(),
            scope: scope.to_string(),
            location: location.to_string(),
            archived_at: archived_at.to_string(),
            action: action.to_string(),
        }
    }

    #[test]
    fn transcript_prune_table_id_is_compacted() {
        // LOCATION is a real filesystem path and legitimately embeds the
        // full run id in its filename — that's a `CellValue::Text` cell,
        // not the identifier cell, so the "full id absent" guard below
        // only checks the compacted RUN ID cell renders short, using a
        // location string that doesn't itself embed the id.
        let rows = vec![prune_row(
            "run_01KRJDKSBE7X4J49094149WFJS",
            "global",
            "/home/.rupu/transcripts/archived/transcript.jsonl",
            "2026-07-30T13:00:00Z",
            "would_delete",
        )];
        let out = render_transcript_prune_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("run_01KRJDKS…WFJS"), "got: {out}");
        assert!(
            !out.contains("run_01KRJDKSBE7X4J49094149WFJS"),
            "full id must not appear in a table cell: {out}"
        );
    }

    #[test]
    fn transcript_prune_table_archived_renders_relative() {
        let rows = vec![prune_row(
            "run_01KRJDKSBE7X4J49094149WFJS",
            "project",
            "/proj/.rupu/transcripts/archived/run_01KRJDKSBE7X4J49094149WFJS.jsonl",
            "2026-07-30T13:00:00Z",
            "deleted",
        )];
        let out = render_transcript_prune_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("ago"), "got: {out}");
        assert!(!out.contains("2026-07-30T13:00:00"), "got: {out}");
    }

    #[test]
    fn transcript_prune_table_absolute_flag_renders_iso() {
        let rows = vec![prune_row(
            "run_01KRJDKSBE7X4J49094149WFJS",
            "project",
            "/proj/.rupu/transcripts/archived/run_01KRJDKSBE7X4J49094149WFJS.jsonl",
            "2026-07-30T13:00:00Z",
            "deleted",
        )];
        let absolute_prefs = transcript_list_test_prefs().with_table_flags(true, false);
        let out = render_transcript_prune_table(&rows, &absolute_prefs, transcript_list_test_now());
        assert!(out.contains("2026-07-30T13:00:00"), "got: {out}");
    }

    #[test]
    fn transcript_prune_table_survives_an_unparseable_archived_at() {
        let rows = vec![prune_row(
            "run_01KRJDKSBE7X4J49094149WFJS",
            "global",
            "/home/.rupu/transcripts/archived/run_01KRJDKSBE7X4J49094149WFJS.jsonl",
            "not-a-timestamp",
            "would_delete",
        )];
        let out = render_transcript_prune_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(out.contains("not-a-timestamp"), "row was dropped: {out}");
    }

    #[test]
    fn transcript_prune_table_has_no_summary_line() {
        // No literal `STATUS` header — SCOPE and ACTION are both
        // `CellValue::Status` for their colour, but neither is named
        // `STATUS`, so `.with_summary` is deliberately not called.
        let rows = vec![
            prune_row(
                "run_01KRJDKSBE7X4J49094149WFJS",
                "global",
                "/home/.rupu/transcripts/archived/run_01KRJDKSBE7X4J49094149WFJS.jsonl",
                "2026-07-30T13:00:00Z",
                "would_delete",
            ),
            prune_row(
                "run_01KRJDKSBE7X4J49094149ABCD",
                "project",
                "/proj/.rupu/transcripts/archived/run_01KRJDKSBE7X4J49094149ABCD.jsonl",
                "2026-07-29T13:00:00Z",
                "deleted",
            ),
        ];
        let out = render_transcript_prune_table(
            &rows,
            &transcript_list_test_prefs(),
            transcript_list_test_now(),
        );
        assert!(
            !out.contains(" transcripts\n\n"),
            "unexpected summary line: {out}"
        );
    }
}
