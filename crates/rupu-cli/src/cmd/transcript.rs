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

use crate::output::formats::OutputFormat;
use crate::output::palette::Status;
use crate::output::report::{self, CollectionOutput, EventOutput};
use crate::output::workflow_printer::tool_summary;
use crate::output::LineStreamPrinter;
use crate::paths;
use clap::Subcommand;
use comfy_table::Cell;
use rupu_transcript::{Event as TranscriptEvent, JsonlReader, RunStatus};
use serde::Serialize;
use std::cmp::Reverse;
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
    },
    /// Print a transcript's full event stream.
    Show { run_id: String },
}

pub async fn handle(action: Action, global_format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List { no_color } => list(no_color, global_format).await,
        Action::Show { run_id } => show(&run_id, global_format).await,
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
    title: Option<String>,
    agent: String,
    status: String,
    total_tokens: u64,
    started_at: String,
}

#[derive(Serialize)]
struct TranscriptListCsvRow {
    run_id: String,
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
    report: TranscriptShowReport,
    events: Vec<TranscriptEvent>,
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
            "RUN ID", "TITLE", "AGENT", "STATUS", "TOKENS", "STARTED",
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
        render_pretty_transcript(&self.events)
    }
}

fn render_pretty_transcript(events: &[TranscriptEvent]) -> anyhow::Result<()> {
    let mut printer = LineStreamPrinter::new();
    let mut saw_header = false;

    for event in events {
        render_pretty_transcript_event(&mut printer, event, &mut saw_header);
    }

    if !saw_header {
        for event in events {
            println!("{}", serde_json::to_string_pretty(event)?);
        }
    }
    Ok(())
}

pub(crate) fn render_pretty_transcript_event(
    printer: &mut LineStreamPrinter,
    event: &TranscriptEvent,
    saw_header: &mut bool,
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
            printer.agent_header(agent, provider, model, run_id);
            let detail = format!(
                "workspace {workspace_id}  ·  mode {}  ·  {}",
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
                printer.assistant_chunk(content);
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
            let mut detail = truncate_single_line(error.as_deref().unwrap_or(output.as_str()), 84);
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

async fn list(no_color: bool, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    let mut paths_to_scan: Vec<PathBuf> = Vec::new();

    // Collect .jsonl paths from a directory — miss is a silent skip.
    fn collect_jsonl(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                out.push(p);
            }
        }
    }

    if let Some(ref proj) = project_root {
        collect_jsonl(&proj.join(".rupu/transcripts"), &mut paths_to_scan);
    }
    collect_jsonl(&global.join("transcripts"), &mut paths_to_scan);

    struct Row {
        run_id: String,
        title: Option<String>,
        agent: String,
        status: RunStatus,
        total_tokens: u64,
        started_at: chrono::DateTime<chrono::Utc>,
    }

    let mut rows: Vec<Row> = Vec::new();
    for path in &paths_to_scan {
        match JsonlReader::summary(path) {
            Ok(s) => rows.push(Row {
                run_id: s.run_id,
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
    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, no_color, None, None);
    let report_rows: Vec<TranscriptListRow> = rows
        .iter()
        .map(|row| TranscriptListRow {
            run_id: row.run_id.clone(),
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

async fn show(run_id: &str, global_format: Option<OutputFormat>) -> anyhow::Result<()> {
    let path = locate_transcript(run_id)?;
    let mut events = Vec::new();
    let mut raw_events = Vec::new();
    for event in JsonlReader::iter(&path)? {
        let event = event?;
        raw_events.push(event.clone());
        events.push(serde_json::to_value(event)?);
    }
    let output = TranscriptShowOutput {
        events: raw_events,
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

fn locate_transcript(run_id: &str) -> anyhow::Result<PathBuf> {
    let filename = format!("{run_id}.jsonl");

    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;

    // Project-local first.
    if let Some(ref proj) = project_root {
        let candidate = proj.join(".rupu/transcripts").join(&filename);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // Global fallback.
    let candidate = global.join("transcripts").join(&filename);
    if candidate.is_file() {
        return Ok(candidate);
    }

    Err(anyhow::anyhow!("transcript not found: {run_id}"))
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
