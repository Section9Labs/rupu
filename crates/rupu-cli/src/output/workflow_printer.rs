//! Workflow run → line-stream printer wiring.
//!
//! `WorkflowPrinter` drives a `LineStreamPrinter` from a live workflow run:
//! it polls `run.json` for the active step so it can attach to that
//! transcript immediately, and it also watches `step_results.jsonl` for
//! completed-step metadata / fan-out summaries.
//!
//! Layout assumption (matches the orchestrator's on-disk format):
//! - `runs_dir/<run_id>/run.json` — status + metadata.
//! - `runs_dir/<run_id>/step_results.jsonl` — one line per completed step.
//!   Each line carries `transcript_path` (or empty for panel steps) plus
//!   per-panelist `items[]` for panels.
//! - Individual step / panelist transcripts live at the absolute paths
//!   referenced from those records.
//!
//! The poller also reads `run.json` to detect when the run transitions
//! to a terminal state (`completed` / `failed`) or an approval gate
//! (`awaiting_approval`). On `awaiting_approval` it fires the printer's
//! `approval_prompt` method and then calls back into the run-store to
//! approve or reject based on the user's response.

use super::{
    palette::Status as UiStatus, printer::LineStreamPrinter, SpinnerHandle, TranscriptTailer,
};
use crate::cmd::transcript::truncate_single_line;
use crate::cmd::ui::{LiveViewMode, UiPrefs};
use crate::output::palette::{self, BRAND, DIM};
use crate::output::printer::{visible_len, wrap_with_ansi};
use crate::paths;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use rupu_orchestrator::{FindingRecord, ItemResultRecord, RunRecord, StepKind, StepResultRecord};
use rupu_transcript::Event as TxEvent;
use serde_json::Value as JsonValue;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Poll interval between run-status checks.
const POLL_MS: u64 = 250;
/// How long to wait for the run dir to appear before giving up.
const RUN_DIR_TIMEOUT_MS: u64 = 5_000;
/// How long to keep polling after RunComplete before declaring done.
const DRAIN_EXTRA_MS: u64 = 200;

/// Tracks an in-flight dispatch tool call so the matching `ToolResult`
/// can render the child / children inline. Single-child dispatches
/// carry the requested agent name; parallel dispatches need no extra
/// state because the result payload is keyed by the caller-chosen `id`.
enum InFlightDispatch {
    Single { agent: String },
    Parallel,
}

/// Per-step printer state for a non-panel (linear) step.
struct StepState {
    tailer: TranscriptTailer,
    run_id: String,
    step_id: String,
    agent: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    spinner: Option<SpinnerHandle>,
    /// In-flight dispatch tool calls keyed by `call_id`. Seeded by
    /// `ToolCall` events for `dispatch_agent` /
    /// `dispatch_agents_parallel`; consumed by the matching
    /// `ToolResult`. Other tool calls (bash, read_file, …) bypass this
    /// map and use the existing `printer.tool_call` summary line.
    dispatches: BTreeMap<String, InFlightDispatch>,
    promoted_actions: BTreeMap<String, (String, JsonValue)>,
}

#[derive(Debug, Clone)]
pub enum LiveWorkflowEvent {
    ToolSucceeded {
        run_id: String,
        step_id: String,
        tool: String,
        input: JsonValue,
    },
}

#[derive(Debug, Clone)]
pub struct LiveWorkflowRender {
    pub status: UiStatus,
    pub label: String,
    pub detail: Option<String>,
}

pub type LiveWorkflowEventHook =
    Arc<dyn Fn(&LiveWorkflowEvent) -> Option<LiveWorkflowRender> + Send + Sync>;

#[derive(Clone, PartialEq, Eq)]
struct WorkflowViewLine {
    status: UiStatus,
    text: String,
    continuation: bool,
}

struct WorkflowInteractiveState {
    seen_step_results: usize,
    followed_step_id: Option<String>,
    followed_transcript_path: Option<PathBuf>,
    tailer: Option<TranscriptTailer>,
    lines: Vec<WorkflowViewLine>,
    total_tokens: u64,
}

impl WorkflowInteractiveState {
    fn new(skip_count: usize) -> Self {
        Self {
            seen_step_results: skip_count,
            followed_step_id: None,
            followed_transcript_path: None,
            tailer: None,
            lines: Vec::new(),
            total_tokens: 0,
        }
    }

    fn push_line(&mut self, status: UiStatus, text: impl Into<String>) {
        self.lines.push(WorkflowViewLine {
            status,
            text: text.into(),
            continuation: false,
        });
        if self.lines.len() > 500 {
            let keep_from = self.lines.len().saturating_sub(500);
            self.lines.drain(0..keep_from);
        }
    }
}

struct WorkflowRawModeGuard;

impl WorkflowRawModeGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for WorkflowRawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

struct WorkflowScreenGuard;

impl WorkflowScreenGuard {
    fn enter() -> io::Result<Self> {
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for WorkflowScreenGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

/// What `attach_and_print` returned. The caller decides what to do next:
/// `Done` and `Detached` and `Rejected` are terminal; `Approved` carries
/// the `awaited_step_id` so the caller can spin a resume run and re-attach
/// the printer to it.
#[derive(Debug, Clone)]
pub enum AttachOutcome {
    /// Run reached `completed` or `failed` while the printer was attached.
    Done,
    /// User pressed `q` (or unrecognized key) at an approval gate. The run
    /// itself is still in `awaiting_approval` on disk; the user can re-attach
    /// later via `rupu watch <run_id>`.
    Detached,
    /// User pressed `a`. The approval was persisted via `RunStore::approve`
    /// (status flipped to `Running`); the caller should now spawn a resumed
    /// run via `OrchestratorRunOpts::resume_from` and reattach the printer.
    Approved { awaited_step_id: String },
    /// User pressed `r`. The rejection was persisted; nothing more to run.
    Rejected,
}

/// Optional knobs for `attach_and_print`. Defaults preserve the
/// pre-resume behavior (print header, start from the first step record).
#[derive(Default, Clone)]
pub struct AttachOpts {
    /// When `true`, skip printing the workflow header. Used on resume
    /// attaches where the header is already on screen from the original
    /// invocation.
    pub skip_header: bool,
    /// Skip this many records from the start of `step_results.jsonl`
    /// before rendering. Used on resume to avoid re-printing prior steps
    /// that the user already saw.
    pub skip_count: usize,
    /// Optional sideband hook for selected live transcript events.
    pub live_event_hook: Option<LiveWorkflowEventHook>,
    /// Control live output density for workflow body rendering.
    pub view_mode: LiveViewMode,
}

/// Drive `printer` from a live or recently-finished workflow run.
pub fn attach_and_print(
    workflow_name: &str,
    run_id: &str,
    runs_dir: &Path,
    transcript_dir: &Path,
    printer: &mut LineStreamPrinter,
    run_store: &rupu_orchestrator::RunStore,
) -> io::Result<AttachOutcome> {
    attach_and_print_with(
        workflow_name,
        run_id,
        runs_dir,
        transcript_dir,
        printer,
        run_store,
        AttachOpts::default(),
    )
}

/// `attach_and_print` with extra knobs (resume support).
pub fn attach_and_print_with(
    workflow_name: &str,
    run_id: &str,
    runs_dir: &Path,
    transcript_dir: &Path,
    printer: &mut LineStreamPrinter,
    run_store: &rupu_orchestrator::RunStore,
    opts: AttachOpts,
) -> io::Result<AttachOutcome> {
    let run_dir = runs_dir.join(run_id);
    let run_json = run_dir.join("run.json");
    let step_results_log = run_dir.join("step_results.jsonl");

    // Wait for run.json to exist.
    let deadline = Instant::now() + Duration::from_millis(RUN_DIR_TIMEOUT_MS);
    loop {
        if run_json.is_file() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "run dir not found after {RUN_DIR_TIMEOUT_MS}ms: {}",
                run_dir.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Read `started_at` from run.json (used for the header and the
    // workflow_done duration calc). On resume the header itself is
    // suppressed but the value is still useful to keep the duration
    // referenced from the same anchor.
    let started_at = {
        let bytes = std::fs::read(&run_json)?;
        let rec: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let started_at_str = rec["started_at"].as_str().unwrap_or("");
        chrono::DateTime::parse_from_rfc3339(started_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    };
    if !opts.skip_header {
        printer.workflow_header(workflow_name, run_id, started_at);
    }
    if let Some(record) = load_run_record(&run_json) {
        render_workflow_intro(printer, &record, opts.view_mode);
    }

    // Workflow-level liveness ticker. `step_results.jsonl` is appended
    // at step COMPLETION, so the per-step `start_ticker` inside
    // `step_start` only fires after the step has already finished —
    // by which point the printer drains the entire transcript at once
    // and the per-step ticker dies in milliseconds. To keep the user
    // visually informed during a long step, we arm a workflow-level
    // ticker upfront and re-arm it at the end of every poll iteration
    // (idempotent — `start_ticker` updates the message in place when
    // already running). Per-step `start_ticker` calls override the
    // message with `running <step_id>…`; their `stop_ticker` at step
    // close still tears it down, but the next iteration's re-arm
    // brings it back before the next sleep so the bottom row is
    // never empty during an in-flight workflow.
    printer.start_ticker(workflow_ticker_message(workflow_name, None));

    let mut seen_step_results: usize = opts.skip_count;
    let mut steps: Vec<StepState> = Vec::new();
    let mut total_tokens: u64 = 0;
    // For the rendered phase separators: start at the same count so
    // resumed steps don't get an extra leading separator.
    let mut step_count: usize = opts.skip_count;

    let mut opened: BTreeSet<PathBuf> = BTreeSet::new();

    loop {
        let record = load_run_record(&run_json);
        if let Some(ref record) = record {
            ensure_active_step_attached(record, &mut opened, &mut steps, printer, &mut step_count);
        }

        drain_step_results(
            &step_results_log,
            transcript_dir,
            &mut seen_step_results,
            &mut opened,
            &mut steps,
            printer,
            &mut step_count,
            opts.view_mode,
        );

        for step in &mut steps {
            let events = step.tailer.drain();
            for ev in events {
                process_event(
                    ev,
                    step,
                    printer,
                    &mut total_tokens,
                    opts.live_event_hook.as_ref(),
                    opts.view_mode,
                );
            }
        }

        let record = match record {
            Some(r) => r,
            None => {
                std::thread::sleep(Duration::from_millis(POLL_MS));
                continue;
            }
        };

        // Re-arm the workflow ticker before any terminal-status branch
        // takes us out of the loop. `start_ticker` updates the message
        // in place when already running, so this is cheap; it's
        // load-bearing only when a step's `step_done` just tore the
        // ticker down (which happens inside `process_event` above).
        printer.start_ticker(workflow_ticker_message(workflow_name, Some(&record)));

        match record.status {
            rupu_orchestrator::RunStatus::AwaitingApproval => {
                flush_all_tailers(&mut steps, printer, &mut total_tokens, opts.view_mode);

                let step_id = record
                    .awaiting_step_id
                    .clone()
                    .unwrap_or_else(|| "approval_gate".to_string());
                let prompt = record
                    .approval_prompt
                    .clone()
                    .unwrap_or_else(|| "Approve this step?".to_string());

                // Tear down the workflow-level ticker before the
                // approval prompt — the prompt reads from stdin and
                // an animating bottom row would compete with the
                // cursor for that row's cells. Terminal-return paths
                // below also drop the ticker.
                printer.stop_ticker();
                loop {
                    let ch = printer.approval_prompt(&step_id, &prompt).unwrap_or('q');

                    match ch {
                        'v' | 'V' => {
                            let groups = load_findings_groups(&step_results_log, &step_id);
                            printer.print_findings(&groups);
                        }
                        'a' | 'A' => {
                            let approver = whoami::username();
                            match run_store.approve(run_id, &approver, chrono::Utc::now()) {
                                Ok(_) => {
                                    // Don't print step_done — the resumed run
                                    // will dispatch the same step's agent and
                                    // emit a real footer. Hand control back to
                                    // the caller so it can spawn the resume.
                                    return Ok(AttachOutcome::Approved {
                                        awaited_step_id: step_id,
                                    });
                                }
                                Err(e) => {
                                    eprintln!("rupu: approve failed: {e}");
                                    return Err(io::Error::other(e.to_string()));
                                }
                            }
                        }
                        'r' | 'R' => {
                            let reason = printer.reject_reason_prompt().unwrap_or_default();
                            let reason = if reason.is_empty() {
                                "rejected by operator"
                            } else {
                                &reason
                            };
                            let approver = whoami::username();
                            let _ = run_store.reject(run_id, &approver, reason, chrono::Utc::now());
                            println!("Run rejected.");
                            return Ok(AttachOutcome::Rejected);
                        }
                        _ => {
                            println!();
                            println!("Detached from run. It is still running.");
                            println!("Re-attach with: rupu watch {run_id}");
                            return Ok(AttachOutcome::Detached);
                        }
                    }
                }
            }
            rupu_orchestrator::RunStatus::Completed => {
                std::thread::sleep(Duration::from_millis(DRAIN_EXTRA_MS));
                drain_step_results(
                    &step_results_log,
                    transcript_dir,
                    &mut seen_step_results,
                    &mut opened,
                    &mut steps,
                    printer,
                    &mut step_count,
                    opts.view_mode,
                );
                flush_all_tailers(&mut steps, printer, &mut total_tokens, opts.view_mode);

                let duration_ms = record
                    .finished_at
                    .map(|fin| (fin - started_at).num_milliseconds().max(0) as u64)
                    .unwrap_or(0);
                let dur = Duration::from_millis(duration_ms);
                printer.stop_ticker();
                printer.workflow_done(workflow_name, run_id, dur, total_tokens);
                return Ok(AttachOutcome::Done);
            }
            rupu_orchestrator::RunStatus::Failed | rupu_orchestrator::RunStatus::Rejected => {
                std::thread::sleep(Duration::from_millis(DRAIN_EXTRA_MS));
                flush_all_tailers(&mut steps, printer, &mut total_tokens, opts.view_mode);

                let err = record.error_message.as_deref().unwrap_or("unknown error");
                printer.stop_ticker();
                printer.workflow_failed(workflow_name, run_id, err);
                return Ok(AttachOutcome::Done);
            }
            _ => {}
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

/// Retained interactive workflow view for direct `rupu workflow run`.
///
/// This path is intentionally scoped to the primary CLI workflow run
/// surface. It keeps resize-safe rendering for the long-running live
/// view without changing the line-stream printer used by non-interactive
/// surfaces or shared-printer callers such as autoflow.
pub fn attach_and_render_interactive_with(
    workflow_name: &str,
    run_id: &str,
    runs_dir: &Path,
    run_store: &rupu_orchestrator::RunStore,
    opts: AttachOpts,
) -> io::Result<AttachOutcome> {
    let run_dir = runs_dir.join(run_id);
    let run_json = run_dir.join("run.json");
    let step_results_log = run_dir.join("step_results.jsonl");

    let deadline = Instant::now() + Duration::from_millis(RUN_DIR_TIMEOUT_MS);
    loop {
        if run_json.is_file() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "run dir not found after {RUN_DIR_TIMEOUT_MS}ms: {}",
                run_dir.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let started_at = {
        let bytes = std::fs::read(&run_json)?;
        let rec: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let started_at_str = rec["started_at"].as_str().unwrap_or("");
        chrono::DateTime::parse_from_rfc3339(started_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    };

    let (final_outcome, final_record, final_tokens) = {
        let _raw_mode = WorkflowRawModeGuard::enter()?;
        let _screen = WorkflowScreenGuard::enter()?;
        let prefs = retained_workflow_ui_prefs().unwrap_or_else(|_| {
            UiPrefs::resolve(
                &rupu_config::UiConfig::default(),
                false,
                None,
                None,
                Some(opts.view_mode),
            )
        });
        let mut state = WorkflowInteractiveState::new(opts.skip_count);
        let mut last_rows: Vec<String> = Vec::new();

        loop {
            let Some(record) = load_run_record(&run_json) else {
                std::thread::sleep(Duration::from_millis(POLL_MS));
                continue;
            };

            drain_step_results_interactive(&step_results_log, &mut state, opts.view_mode);
            follow_active_transcript(&record, &mut state, opts.view_mode, &prefs);
            drain_workflow_transcript_events(&mut state, opts.view_mode, &prefs);

            let rows =
                build_workflow_screen_rows(workflow_name, &record, &state, opts.view_mode, &prefs);
            if rows != last_rows {
                render_workflow_screen_rows(&rows)?;
                last_rows = rows;
            }

            match record.status {
                rupu_orchestrator::RunStatus::AwaitingApproval => {
                    if event::poll(Duration::from_millis(POLL_MS))? {
                        match event::read()? {
                            CrosstermEvent::Resize(_, _) => {}
                            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                                match handle_workflow_approval_keypress(
                                    run_id,
                                    &record,
                                    &step_results_log,
                                    run_store,
                                    &mut state,
                                    key,
                                )? {
                                    Some(outcome) => {
                                        break (outcome, Some(record), state.total_tokens);
                                    }
                                    None => {}
                                }
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                rupu_orchestrator::RunStatus::Completed
                | rupu_orchestrator::RunStatus::Failed
                | rupu_orchestrator::RunStatus::Rejected => {
                    std::thread::sleep(Duration::from_millis(DRAIN_EXTRA_MS));
                    drain_step_results_interactive(&step_results_log, &mut state, opts.view_mode);
                    drain_workflow_transcript_events(&mut state, opts.view_mode, &prefs);

                    let final_rows = build_workflow_screen_rows(
                        workflow_name,
                        &record,
                        &state,
                        opts.view_mode,
                        &prefs,
                    );
                    if final_rows != last_rows {
                        render_workflow_screen_rows(&final_rows)?;
                    }
                    break (AttachOutcome::Done, Some(record), state.total_tokens);
                }
                _ => {
                    if event::poll(Duration::from_millis(POLL_MS))? {
                        match event::read()? {
                            CrosstermEvent::Resize(_, _) => {}
                            CrosstermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                                handle_running_workflow_keypress(&record, &mut state, key);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    };

    if let Some(record) = final_record {
        let mut printer = LineStreamPrinter::new();
        match final_outcome {
            AttachOutcome::Done => match record.status {
                rupu_orchestrator::RunStatus::Completed => {
                    let duration_ms = record
                        .finished_at
                        .map(|fin| (fin - started_at).num_milliseconds().max(0) as u64)
                        .unwrap_or(0);
                    printer.workflow_done(
                        workflow_name,
                        run_id,
                        Duration::from_millis(duration_ms),
                        final_tokens,
                    );
                }
                rupu_orchestrator::RunStatus::Failed | rupu_orchestrator::RunStatus::Rejected => {
                    printer.workflow_failed(
                        workflow_name,
                        run_id,
                        record.error_message.as_deref().unwrap_or("unknown error"),
                    );
                }
                _ => {}
            },
            AttachOutcome::Detached => {
                println!("Detached from run. It is still waiting on approval.");
                println!("Re-attach with: rupu watch {run_id}");
            }
            AttachOutcome::Approved { .. } | AttachOutcome::Rejected => {}
        }
    }

    Ok(final_outcome)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_run_record(run_json: &Path) -> Option<RunRecord> {
    let bytes = std::fs::read(run_json).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn follow_active_transcript(
    record: &RunRecord,
    state: &mut WorkflowInteractiveState,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
) {
    let desired_path = record.active_step_transcript_path.clone();
    if desired_path == state.followed_transcript_path {
        return;
    }
    drain_workflow_transcript_events(state, view_mode, prefs);
    state.followed_step_id = record.active_step_id.clone();
    state.followed_transcript_path = desired_path.clone();
    state.tailer = desired_path.map(TranscriptTailer::new);
    if let Some(step_id) = record.active_step_id.as_deref() {
        let detail = if let Some(agent) = record.active_step_agent.as_deref() {
            format!("{step_id}  ·  {agent}")
        } else {
            step_id.to_string()
        };
        state.push_line(UiStatus::Working, format!("active step  ·  {detail}"));
    }
}

fn drain_workflow_transcript_events(
    state: &mut WorkflowInteractiveState,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
) {
    if let Some(tailer) = state.tailer.as_mut() {
        for event in tailer.drain() {
            if let TxEvent::RunComplete { total_tokens, .. } = &event {
                state.total_tokens += *total_tokens;
            }
            for line in workflow_transcript_event_lines(&event, view_mode, prefs) {
                state.lines.push(line);
                if state.lines.len() > 500 {
                    let keep_from = state.lines.len().saturating_sub(500);
                    state.lines.drain(0..keep_from);
                }
            }
        }
    }
}

fn drain_step_results_interactive(
    log: &Path,
    state: &mut WorkflowInteractiveState,
    view_mode: LiveViewMode,
) {
    let Ok(bytes) = std::fs::read(log) else {
        return;
    };
    let lines: Vec<&[u8]> = bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();

    for line in lines.iter().skip(state.seen_step_results) {
        state.seen_step_results += 1;
        let Ok(rec): Result<StepResultRecord, _> = serde_json::from_slice(line) else {
            continue;
        };
        if rec.skipped {
            state.push_line(UiStatus::Skipped, format!("skipped  ·  {}", rec.step_id));
            continue;
        }
        append_step_result_lines(state, &rec, view_mode);
    }
}

fn append_step_result_lines(
    state: &mut WorkflowInteractiveState,
    rec: &StepResultRecord,
    view_mode: LiveViewMode,
) {
    match rec.kind {
        StepKind::Linear => {
            let status = if rec.success {
                UiStatus::Complete
            } else {
                UiStatus::Failed
            };
            let label = if rec.success {
                "step complete"
            } else {
                "step failed"
            };
            let detail = truncate_single_line(&rec.step_id, 96);
            state.push_line(status, format!("{label}  ·  {detail}"));
            if !rec.success && !rec.output.trim().is_empty() {
                state.push_line(
                    UiStatus::Failed,
                    format!("error  ·  {}", truncate_single_line(&rec.output, 96)),
                );
            }
        }
        StepKind::ForEach | StepKind::Parallel | StepKind::Panel => {
            let status = if rec.success {
                UiStatus::Complete
            } else {
                UiStatus::Failed
            };
            let kind = match rec.kind {
                StepKind::ForEach => "for_each",
                StepKind::Parallel => "parallel",
                StepKind::Panel => "panel",
                StepKind::Linear => unreachable!(),
            };
            state.push_line(
                status,
                format!(
                    "{}  ·  {}  ·  {} {}",
                    rec.step_id,
                    kind,
                    rec.items.len(),
                    if rec.items.len() == 1 {
                        "item"
                    } else {
                        "items"
                    }
                ),
            );
            append_fanout_item_lines(state, rec, view_mode);
        }
    }
}

fn append_fanout_item_lines(
    state: &mut WorkflowInteractiveState,
    rec: &StepResultRecord,
    view_mode: LiveViewMode,
) {
    for item in &rec.items {
        let events: Vec<TxEvent> =
            if !item.transcript_path.as_os_str().is_empty() && item.transcript_path.exists() {
                let mut tailer = TranscriptTailer::new(&item.transcript_path);
                tailer.drain()
            } else {
                Vec::new()
            };
        let summary = summarize_compact_child_events(&events);
        let headline = match rec.kind {
            StepKind::ForEach => {
                let label = item_input_label(&item.item);
                if label.is_empty() {
                    format!("iter[{}]", item.index + 1)
                } else {
                    format!("iter[{}] · {}", item.index + 1, label)
                }
            }
            StepKind::Parallel | StepKind::Panel => item.sub_id.clone(),
            StepKind::Linear => unreachable!(),
        };
        let status = if item.success {
            UiStatus::Complete
        } else {
            UiStatus::Failed
        };
        state.total_tokens += summary.total_tokens;
        let detail = compact_child_detail(
            status,
            &summary.provider,
            &summary.model,
            summary.duration_ms,
            summary.total_tokens,
            match rec.kind {
                StepKind::Panel => Some(
                    rec.findings
                        .iter()
                        .filter(|finding| finding.source == item.sub_id)
                        .count(),
                ),
                _ => None,
            },
        );
        state.push_line(status, format!("  {headline}  ·  {detail}"));
        if !item.success {
            let reason = summary.error.as_deref().unwrap_or("item failed");
            state.push_line(
                UiStatus::Failed,
                format!("    error  {}", truncate_single_line(reason, 88)),
            );
        } else if view_mode == LiveViewMode::Focused {
            if let Some(note) = compact_child_note(&summary) {
                state.push_line(UiStatus::Active, format!("    {note}"));
            }
        } else if let Some(note) = compact_child_note(&summary) {
            state.push_line(UiStatus::Active, format!("    {note}"));
        }
    }
}

fn workflow_transcript_event_lines(
    event: &TxEvent,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
) -> Vec<WorkflowViewLine> {
    match event {
        TxEvent::RunStart {
            run_id,
            workspace_id,
            mode,
            started_at,
            ..
        } => vec![WorkflowViewLine {
            status: UiStatus::Active,
            text: retained_workflow_event_line(
                UiStatus::Active,
                "run started",
                &format!(
                    "{}  ·  workspace {}  ·  mode {}  ·  {}",
                    compact_workflow_run_id(run_id),
                    workspace_id,
                    format!("{mode:?}").to_lowercase(),
                    started_at.format("%Y-%m-%d %H:%M:%S UTC")
                ),
            ),
            continuation: false,
        }],
        TxEvent::TurnStart { turn_idx } => vec![WorkflowViewLine {
            status: UiStatus::Working,
            text: retained_workflow_event_line(
                UiStatus::Working,
                &format!("turn {turn_idx}"),
                "assistant turn started",
            ),
            continuation: false,
        }],
        TxEvent::AssistantMessage { content, thinking } => {
            let mut out = Vec::new();
            if let Some(thinking) = thinking.as_deref().filter(|value| !value.trim().is_empty()) {
                out.push(WorkflowViewLine {
                    status: UiStatus::Active,
                    text: retained_workflow_event_line(
                        UiStatus::Active,
                        "thinking",
                        &truncate_single_line(thinking, 96),
                    ),
                    continuation: false,
                });
            }
            if !content.trim().is_empty() {
                match view_mode {
                    LiveViewMode::Focused => out.push(WorkflowViewLine {
                        status: UiStatus::Active,
                        text: retained_workflow_event_line(
                            UiStatus::Active,
                            "assistant output",
                            &truncate_single_line(content, 96),
                        ),
                        continuation: false,
                    }),
                    LiveViewMode::Full => {
                        let highlighted = crate::cmd::ui::highlight_markdown(content.trim(), prefs);
                        let mut lines = highlighted.split('\n');
                        if let Some(first) = lines.next() {
                            out.push(WorkflowViewLine {
                                status: UiStatus::Active,
                                text: retained_workflow_event_line_raw(
                                    UiStatus::Active,
                                    "assistant output",
                                    first,
                                ),
                                continuation: false,
                            });
                            for line in lines {
                                out.push(WorkflowViewLine {
                                    status: UiStatus::Active,
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
        TxEvent::ToolCall { tool, input, .. } => vec![WorkflowViewLine {
            status: UiStatus::Working,
            text: retained_workflow_event_line(
                UiStatus::Working,
                &format!("tool {tool}"),
                &summarize_tool_input(tool, input),
            ),
            continuation: false,
        }],
        TxEvent::ToolResult {
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
            let label = if error.is_some() {
                "tool error"
            } else {
                "tool result"
            };
            let mut detail =
                truncate_single_line(error.as_deref().unwrap_or(output.as_str()), 90);
            if *duration_ms > 0 {
                detail.push_str(&format!("  ·  {}ms", duration_ms));
            }
            vec![WorkflowViewLine {
                status,
                text: retained_workflow_event_line(status, label, &detail),
                continuation: false,
            }]
        }
        TxEvent::FileEdit { path, kind, .. } => vec![WorkflowViewLine {
            status: UiStatus::Complete,
            text: retained_workflow_event_line(
                UiStatus::Complete,
                "file edit",
                &format!("{} {}", format!("{kind:?}").to_lowercase(), path),
            ),
            continuation: false,
        }],
        TxEvent::CommandRun {
            argv,
            cwd,
            exit_code,
            ..
        } => vec![WorkflowViewLine {
            status: if *exit_code == 0 {
                UiStatus::Complete
            } else {
                UiStatus::Failed
            },
            text: retained_workflow_event_line(
                if *exit_code == 0 {
                    UiStatus::Complete
                } else {
                    UiStatus::Failed
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
        TxEvent::ActionEmitted {
            kind,
            allowed,
            applied,
            reason,
            ..
        } => {
            let status = if *applied {
                UiStatus::Complete
            } else if *allowed {
                UiStatus::Awaiting
            } else {
                UiStatus::Failed
            };
            let mut detail = format!("{kind}  ·  allowed={allowed} applied={applied}");
            if let Some(reason) = reason.as_deref().filter(|value| !value.trim().is_empty()) {
                detail.push_str("  ·  ");
                detail.push_str(&truncate_single_line(reason, 64));
            }
            vec![WorkflowViewLine {
                status,
                text: retained_workflow_event_line_raw(status, "action", &detail),
                continuation: false,
            }]
        }
        TxEvent::GateRequested {
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
            vec![WorkflowViewLine {
                status: UiStatus::Awaiting,
                text: retained_workflow_event_line_raw(UiStatus::Awaiting, "approval gate", &detail),
                continuation: false,
            }]
        }
        TxEvent::TurnEnd {
            turn_idx,
            tokens_in,
            tokens_out,
        } => vec![WorkflowViewLine {
            status: UiStatus::Complete,
            text: retained_workflow_event_line(
                UiStatus::Complete,
                "turn complete",
                &format!(
                    "turn {turn_idx}  ·  in {} out {}",
                    tokens_in.unwrap_or(0),
                    tokens_out.unwrap_or(0)
                ),
            ),
            continuation: false,
        }],
        TxEvent::Usage {
            provider,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
        } => vec![WorkflowViewLine {
            status: UiStatus::Active,
            text: retained_workflow_event_line_raw(
                UiStatus::Active,
                "usage",
                &format!(
                    "{provider} · {model}  ·  in {input_tokens} out {output_tokens} cached {cached_tokens}"
                ),
            ),
            continuation: false,
        }],
        TxEvent::RunComplete {
            status,
            total_tokens,
            duration_ms,
            error,
            ..
        } => {
            let ui_status = match status {
                rupu_transcript::RunStatus::Ok => UiStatus::Complete,
                rupu_transcript::RunStatus::Error | rupu_transcript::RunStatus::Aborted => {
                    UiStatus::Failed
                }
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
            vec![WorkflowViewLine {
                status: ui_status,
                text: retained_workflow_event_line_raw(ui_status, "run complete", &detail),
                continuation: false,
            }]
        }
    }
}

fn handle_running_workflow_keypress(
    record: &RunRecord,
    state: &mut WorkflowInteractiveState,
    key: crossterm::event::KeyEvent,
) {
    match (key.code, key.modifiers) {
        (KeyCode::Char('?'), _) => state.push_line(
            UiStatus::Active,
            "help  ·  resize reflows automatically  ·  approval keys appear when needed",
        ),
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => state.push_line(
            UiStatus::Awaiting,
            format!(
                "workflow stays attached until completion  ·  current step {}",
                record.active_step_id.as_deref().unwrap_or("pending")
            ),
        ),
        _ => {}
    }
}

fn workflow_status_ui(record: &RunRecord) -> UiStatus {
    match record.status {
        rupu_orchestrator::RunStatus::Pending => UiStatus::Waiting,
        rupu_orchestrator::RunStatus::Running => UiStatus::Working,
        rupu_orchestrator::RunStatus::AwaitingApproval => UiStatus::Awaiting,
        rupu_orchestrator::RunStatus::Completed => UiStatus::Complete,
        rupu_orchestrator::RunStatus::Failed | rupu_orchestrator::RunStatus::Rejected => {
            UiStatus::Failed
        }
    }
}

fn retained_workflow_ui_prefs() -> anyhow::Result<UiPrefs> {
    let global = paths::global_dir()?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg = rupu_config::layer_files(
        Some(&global.join("config.toml")),
        project_root
            .as_deref()
            .map(|root| root.join(".rupu/config.toml"))
            .as_deref(),
    )
    .unwrap_or_default();
    Ok(UiPrefs::resolve(&cfg.ui, false, None, None, None))
}

fn handle_workflow_approval_keypress(
    run_id: &str,
    record: &RunRecord,
    step_results_log: &Path,
    run_store: &rupu_orchestrator::RunStore,
    state: &mut WorkflowInteractiveState,
    key: crossterm::event::KeyEvent,
) -> io::Result<Option<AttachOutcome>> {
    let step_id = record
        .awaiting_step_id
        .clone()
        .unwrap_or_else(|| "approval_gate".to_string());
    match (key.code, key.modifiers) {
        (KeyCode::Char('v'), _) | (KeyCode::Char('V'), _) => {
            append_findings_lines(state, &load_findings_groups(step_results_log, &step_id));
            Ok(None)
        }
        (KeyCode::Char('a'), _) | (KeyCode::Char('A'), _) => {
            let approver = whoami::username();
            run_store
                .approve(run_id, &approver, chrono::Utc::now())
                .map_err(|e| io::Error::other(e.to_string()))?;
            Ok(Some(AttachOutcome::Approved {
                awaited_step_id: step_id,
            }))
        }
        (KeyCode::Char('r'), _) | (KeyCode::Char('R'), _) => {
            let approver = whoami::username();
            let _ = run_store.reject(
                run_id,
                &approver,
                "rejected by operator",
                chrono::Utc::now(),
            );
            Ok(Some(AttachOutcome::Rejected))
        }
        (KeyCode::Char('q'), _)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Ok(Some(AttachOutcome::Detached)),
        _ => Ok(None),
    }
}

fn append_findings_lines(
    state: &mut WorkflowInteractiveState,
    groups: &[(String, Vec<FindingRecord>)],
) {
    let total: usize = groups.iter().map(|(_, findings)| findings.len()).sum();
    if total == 0 {
        state.push_line(UiStatus::Skipped, "(no findings)");
        return;
    }
    state.push_line(
        UiStatus::Awaiting,
        if total == 1 {
            "1 finding".to_string()
        } else {
            format!("{total} findings")
        },
    );
    for (step_id, findings) in groups {
        if groups.len() > 1 && !step_id.is_empty() {
            state.push_line(
                UiStatus::Active,
                format!("from {step_id} ({})", findings.len()),
            );
        }
        for finding in findings {
            state.push_line(
                UiStatus::Failed,
                format!(
                    "[{}] {}",
                    finding.severity,
                    truncate_single_line(&finding.title, 88)
                ),
            );
            if !finding.source.is_empty() {
                state.push_line(UiStatus::Active, format!("  source  {}", finding.source));
            }
            if !finding.body.is_empty() {
                state.push_line(
                    UiStatus::Active,
                    format!("  {}", truncate_single_line(&finding.body, 96)),
                );
            }
        }
    }
}

fn build_workflow_screen_rows(
    workflow_name: &str,
    record: &RunRecord,
    state: &WorkflowInteractiveState,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
) -> Vec<String> {
    let (width, height) = terminal::size().unwrap_or((100, 30));
    let width = width.max(40) as usize;
    let height = height.max(12) as usize;
    build_workflow_screen_rows_for_size(
        workflow_name,
        record,
        state,
        view_mode,
        prefs,
        width,
        height,
    )
}

fn build_workflow_screen_rows_for_size(
    workflow_name: &str,
    record: &RunRecord,
    state: &WorkflowInteractiveState,
    view_mode: LiveViewMode,
    _prefs: &UiPrefs,
    width: usize,
    height: usize,
) -> Vec<String> {
    let mut rows = vec![
        render_workflow_header_line(workflow_name, record, view_mode, width),
        String::new(),
        retained_workflow_kv_row(
            "status",
            record.status.as_str(),
            width,
            workflow_status_ui(record),
        ),
    ];
    if let Some(issue_ref) = record.issue_ref.as_deref() {
        rows.push(retained_workflow_kv_row(
            "issue",
            issue_ref,
            width,
            UiStatus::Active,
        ));
    }
    rows.push(retained_workflow_kv_row(
        "workspace",
        &format!(
            "{}  ·  {}",
            record.workspace_id,
            record.workspace_path.display()
        ),
        width,
        UiStatus::Active,
    ));
    let mut route = Vec::new();
    if let Some(backend) = record.backend_id.as_deref() {
        route.push(format!("backend {backend}"));
    }
    if let Some(worker) = record.worker_id.as_deref() {
        route.push(format!("worker {worker}"));
    }
    if !route.is_empty() {
        rows.push(retained_workflow_kv_row(
            "route",
            &route.join("  ·  "),
            width,
            UiStatus::Active,
        ));
    }
    if let Some(step_id) = record.active_step_id.as_deref() {
        let detail = if let Some(agent) = record.active_step_agent.as_deref() {
            format!("{step_id}  ·  {agent}")
        } else {
            step_id.to_string()
        };
        rows.push(retained_workflow_kv_row(
            "step",
            &detail,
            width,
            UiStatus::Working,
        ));
    }
    rows.push(String::new());

    let footer_reserved = 2usize;
    let available_event_rows = height
        .saturating_sub(rows.len())
        .saturating_sub(footer_reserved)
        .max(1);
    let event_rows = render_workflow_event_rows(&state.lines, width, available_event_rows);
    rows.extend(event_rows);
    while rows.len() < height.saturating_sub(footer_reserved) {
        rows.push(String::new());
    }

    rows.push(render_workflow_controls_line(record.status, width));
    rows.push(render_workflow_status_line(record, view_mode, width));
    rows.truncate(height);
    rows
}

fn render_workflow_controls_line(status: rupu_orchestrator::RunStatus, width: usize) -> String {
    let line = match status {
        rupu_orchestrator::RunStatus::AwaitingApproval => {
            "a approve  ·  r reject  ·  v findings  ·  q detach"
        }
        _ => "resize reflows automatically  ·  ? help",
    };
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, "controls", BRAND);
    let _ = palette::write_colored(&mut buf, &format!("  {line}"), DIM);
    truncate_workflow_ansi_line(&buf, width)
}

fn render_workflow_status_line(
    record: &RunRecord,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let current = record
        .active_step_id
        .as_deref()
        .unwrap_or(record.status.as_str());
    let text = match record.status {
        rupu_orchestrator::RunStatus::AwaitingApproval => format!(
            "awaiting  {}  ·  {}",
            record.awaiting_step_id.as_deref().unwrap_or("approval"),
            truncate_single_line(
                record
                    .approval_prompt
                    .as_deref()
                    .unwrap_or("Approve this step?"),
                56
            )
        ),
        _ => format!(
            "view  {}  ·  status {}  ·  current {}",
            view_mode.as_str(),
            record.status.as_str(),
            current
        ),
    };
    retained_workflow_kv_row("view", &text, width, workflow_status_ui(record))
}

fn render_workflow_event_rows(
    lines: &[WorkflowViewLine],
    width: usize,
    max_rows: usize,
) -> Vec<String> {
    let mut rendered = Vec::new();
    for line in lines {
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
        for (idx, segment) in wrap_with_ansi(&line.text, content_width)
            .into_iter()
            .enumerate()
        {
            if idx == 0 && !line.continuation {
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

fn render_workflow_screen_rows(rows: &[String]) -> io::Result<()> {
    let mut stdout = io::stdout();
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
    for (idx, row) in rows.iter().enumerate() {
        queue!(stdout, MoveTo(0, idx as u16), Print(row))?;
    }
    use std::io::Write as _;
    stdout.flush()?;
    Ok(())
}

fn render_workflow_header_line(
    workflow_name: &str,
    record: &RunRecord,
    view_mode: LiveViewMode,
    width: usize,
) -> String {
    let mut buf = String::new();
    let _ = palette::write_colored(&mut buf, "▶", BRAND);
    buf.push(' ');
    let _ = palette::write_bold_colored(&mut buf, "workflow run", BRAND);
    let _ = palette::write_colored(&mut buf, "  ", DIM);
    let _ = palette::write_bold_colored(&mut buf, &truncate_single_line(workflow_name, 24), BRAND);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, &compact_workflow_run_id(&record.id), DIM);
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, view_mode.as_str(), DIM);
    truncate_workflow_ansi_line(&buf, width)
}

fn retained_workflow_kv_row(label: &str, value: &str, width: usize, status: UiStatus) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, &format!("{label:<10}"), status.color());
    let _ = palette::write_colored(
        &mut buf,
        &truncate_single_line(value, width.saturating_sub(11)),
        DIM,
    );
    truncate_workflow_ansi_line(&buf, width)
}

fn retained_workflow_event_line(status: UiStatus, label: &str, detail: &str) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, status.color());
    let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
    let _ = palette::write_colored(&mut buf, detail, DIM);
    buf
}

fn retained_workflow_event_line_raw(status: UiStatus, label: &str, detail: &str) -> String {
    let mut buf = String::new();
    let _ = palette::write_bold_colored(&mut buf, label, status.color());
    if !detail.is_empty() {
        let _ = palette::write_colored(&mut buf, "  ·  ", DIM);
        buf.push_str(detail);
    }
    buf
}

fn truncate_workflow_ansi_line(value: &str, width: usize) -> String {
    if visible_len(value) <= width {
        value.to_string()
    } else {
        wrap_with_ansi(value, width)
            .into_iter()
            .next()
            .unwrap_or_default()
    }
}

fn compact_workflow_run_id(run_id: &str) -> String {
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

fn ensure_active_step_attached(
    record: &RunRecord,
    opened: &mut BTreeSet<PathBuf>,
    steps: &mut Vec<StepState>,
    printer: &mut LineStreamPrinter,
    step_count: &mut usize,
) {
    if !matches!(record.status, rupu_orchestrator::RunStatus::Running) {
        return;
    }
    let Some(step_id) = record.active_step_id.as_deref() else {
        return;
    };
    if !matches!(record.active_step_kind, None | Some(StepKind::Linear)) {
        return;
    }
    let Some(transcript_path) = record.active_step_transcript_path.clone() else {
        return;
    };
    if opened.contains(&transcript_path) {
        return;
    }

    if *step_count > 0 {
        printer.phase_separator();
    }
    *step_count += 1;

    opened.insert(transcript_path.clone());
    let tailer = TranscriptTailer::new(&transcript_path);
    let spinner = printer.step_start(step_id, record.active_step_agent.as_deref(), None, None);
    steps.push(StepState {
        tailer,
        run_id: record.id.clone(),
        step_id: step_id.to_string(),
        agent: record.active_step_agent.clone(),
        provider: None,
        model: None,
        spinner: Some(spinner),
        dispatches: BTreeMap::new(),
        promoted_actions: BTreeMap::new(),
    });
}

/// Walk `step_results.jsonl` and return findings grouped by source step,
/// excluding the awaiting (gate) step itself. Order matches the JSONL file.
/// Falls back to a single empty group if the file is unreadable.
fn load_findings_groups(log: &Path, awaiting_step_id: &str) -> Vec<(String, Vec<FindingRecord>)> {
    let Ok(bytes) = std::fs::read(log) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Vec<FindingRecord>)> = Vec::new();
    for line in bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(rec): Result<StepResultRecord, _> = serde_json::from_slice(line) else {
            continue;
        };
        if rec.step_id == awaiting_step_id {
            continue;
        }
        if rec.findings.is_empty() {
            continue;
        }
        out.push((rec.step_id.clone(), rec.findings));
    }
    out
}

/// Read newly-appended lines from `step_results.jsonl`. For non-panel steps
/// we open a transcript tailer for live event streaming. For panel steps
/// (`items.len() > 0` and `transcript_path` empty) we render the panelist
/// summary inline — there's nothing to tail.
#[allow(clippy::too_many_arguments)]
fn drain_step_results(
    log: &Path,
    transcript_dir: &Path,
    seen: &mut usize,
    opened: &mut BTreeSet<PathBuf>,
    steps: &mut Vec<StepState>,
    printer: &mut LineStreamPrinter,
    step_count: &mut usize,
    view_mode: LiveViewMode,
) {
    let Ok(bytes) = std::fs::read(log) else {
        return;
    };
    let lines: Vec<&[u8]> = bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();

    for line in lines.iter().skip(*seen) {
        *seen += 1;
        // Decode as the typed record so we get items + findings cleanly.
        let Ok(rec): Result<StepResultRecord, _> = serde_json::from_slice(line) else {
            continue;
        };
        if rec.skipped {
            continue;
        }

        if *step_count > 0 {
            printer.phase_separator();
        }
        *step_count += 1;

        match rec.kind {
            StepKind::ForEach | StepKind::Parallel | StepKind::Panel => {
                render_fanout_step(&rec, printer, view_mode);
            }
            StepKind::Linear => {
                // Linear step — open a tailer if we have a transcript.
                if rec.transcript_path.as_os_str().is_empty() || !rec.transcript_path.exists() {
                    // Header + immediate footer (nothing to stream).
                    let spinner = printer.step_start(&rec.step_id, None, None, None);
                    spinner.stop();
                    if rec.success {
                        printer.step_done(&rec.step_id, Duration::ZERO, 0);
                    } else {
                        printer.step_failed(&rec.step_id, "no transcript");
                    }
                    continue;
                }
                if opened.contains(&rec.transcript_path) {
                    continue;
                }
                opened.insert(rec.transcript_path.clone());
                let tailer = TranscriptTailer::new(&rec.transcript_path);
                let spinner = printer.step_start(&rec.step_id, None, None, None);
                steps.push(StepState {
                    tailer,
                    run_id: rec.run_id.clone(),
                    step_id: rec.step_id.clone(),
                    agent: None,
                    provider: None,
                    model: None,
                    spinner: Some(spinner),
                    dispatches: BTreeMap::new(),
                    promoted_actions: BTreeMap::new(),
                });
            }
        }
    }
    let _ = transcript_dir;
}

/// Render a for_each / parallel / panel step as a parent frame
/// holding N child frames at indent+1, one per item / sub-step /
/// panelist. By the time the parent record reaches us, all child
/// transcripts are complete on disk — so we replay each one fully
/// rather than tailing live.
fn render_fanout_step(
    rec: &StepResultRecord,
    printer: &mut LineStreamPrinter,
    view_mode: LiveViewMode,
) {
    // Parent header — emit the same `╭─ … ────  (kind · count)` shape
    // the linear-step header uses, with kind-specific meta. Reuse
    // `panel_start` for panels (it already wires the ticker copy);
    // for_each / parallel get a generic fanout opener.
    let parent_spinner = match rec.kind {
        StepKind::Panel => printer.panel_start(&rec.step_id, rec.items.len()),
        StepKind::ForEach => printer.fanout_start(&rec.step_id, "for_each", rec.items.len()),
        StepKind::Parallel => printer.fanout_start(&rec.step_id, "parallel", rec.items.len()),
        StepKind::Linear => unreachable!("render_fanout_step called for linear step"),
    };

    // Child frames at indent+1.
    printer.push_indent();
    for item in &rec.items {
        render_child_item(rec, item, printer, view_mode);
    }
    printer.pop_indent();
    parent_spinner.stop();

    // Parent footer — kind-specific summary.
    match rec.kind {
        StepKind::Panel => {
            printer.panel_done(
                &rec.step_id,
                rec.success,
                rec.findings.len(),
                Duration::ZERO,
            );
        }
        StepKind::ForEach | StepKind::Parallel => {
            let success_count = rec.items.iter().filter(|i| i.success).count();
            let total = rec.items.len();
            printer.fanout_done(
                &rec.step_id,
                rec.success,
                success_count,
                total,
                Duration::ZERO,
            );
        }
        StepKind::Linear => unreachable!(),
    }
}

/// Render one child item of a fan-out step: pre-scan the item's
/// transcript for the agent / provider / model, open a child frame
/// with a kind-appropriate headline, replay the rest of the
/// transcript inline, then close the child frame. Findings count
/// (for panels) is tallied from the parent's `findings[]`.
fn render_child_item(
    parent: &StepResultRecord,
    item: &ItemResultRecord,
    printer: &mut LineStreamPrinter,
    view_mode: LiveViewMode,
) {
    // Read the full transcript (file is complete by the time we
    // see it). Empty/missing transcripts produce a header+immediate
    // footer with no body.
    let events: Vec<TxEvent> =
        if !item.transcript_path.as_os_str().is_empty() && item.transcript_path.exists() {
            let mut tailer = TranscriptTailer::new(&item.transcript_path);
            tailer.drain()
        } else {
            Vec::new()
        };

    // Extract provider / model from the first RunStart for the meta tail.
    // Agent name isn't used as the headline — the per-item label
    // (`iter[N]` for for_each, sub_id for parallel/panel) is more
    // informative since fan-out items share an agent.
    let (provider, model) = events
        .iter()
        .find_map(|e| match e {
            TxEvent::RunStart {
                provider, model, ..
            } => Some((provider.clone(), model.clone())),
            _ => None,
        })
        .unwrap_or((String::new(), String::new()));

    // Pick the headline. For for_each, the index + a short
    // representation of the input is most useful (so the operator
    // can map "iter[3]" back to the YAML input list). For
    // parallel + panel, the sub_id (which is the YAML-declared
    // sub-step or panelist agent name) is the right label.
    let headline = match parent.kind {
        StepKind::ForEach => {
            let input_label = item_input_label(&item.item);
            if input_label.is_empty() {
                format!("iter[{}]", item.index + 1)
            } else {
                format!("iter[{}] · {}", item.index + 1, input_label)
            }
        }
        StepKind::Parallel | StepKind::Panel => item.sub_id.clone(),
        StepKind::Linear => unreachable!(),
    };

    if view_mode == LiveViewMode::Focused {
        let summary = summarize_compact_child_events(&events);
        render_focused_child_item(parent, item, &headline, &summary, printer);
        return;
    }

    // The headline replaces the agent slot in step_start so it shows
    // as the bold opener line. Provider + model still show in the
    // dim meta tail when present.
    let spinner = printer.step_start(
        &item.sub_id,
        Some(&headline),
        non_empty(&provider),
        non_empty(&model),
    );

    // Replay assistant chunks + tool calls. Track tokens for the
    // child's footer.
    let mut total_tokens = 0u64;
    let mut child_dur = Duration::ZERO;
    let mut child_status_override: Option<String> = None;
    for ev in events {
        match ev {
            TxEvent::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                render_assistant_output(printer, &content, view_mode);
            }
            TxEvent::ToolCall { tool, input, .. } => {
                let summary = summarize_tool_input(&tool, &input);
                printer.tool_call(&tool, &summary);
            }
            TxEvent::RunComplete {
                total_tokens: t,
                duration_ms,
                status,
                error,
                ..
            } => {
                total_tokens += t;
                child_dur = Duration::from_millis(duration_ms);
                if matches!(
                    status,
                    rupu_transcript::RunStatus::Error | rupu_transcript::RunStatus::Aborted
                ) {
                    child_status_override = Some(error.unwrap_or_else(|| "unknown".into()));
                }
            }
            _ => {}
        }
    }
    spinner.stop();

    // Decide footer based on item.success (authoritative) plus any
    // RunComplete error reason from the transcript.
    let _ = ();
    if !item.success {
        let reason = child_status_override
            .clone()
            .unwrap_or_else(|| "item failed".into());
        printer.step_failed(&item.sub_id, &reason);
    } else if matches!(parent.kind, StepKind::Panel) {
        // Panel children show their findings count instead of token
        // count — that's the semantically interesting tally.
        let findings_count = parent
            .findings
            .iter()
            .filter(|f| f.source == item.sub_id)
            .count();
        printer.panelist_done(&item.sub_id, findings_count, child_dur);
    } else {
        printer.step_done(&item.sub_id, child_dur, total_tokens);
    }
}

/// Empty-string → None mapping for the meta-tail extras.
fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Best-effort one-line label for a for_each item's `item:` value.
/// Strings render as themselves; other JSON types render as a short
/// `serde_json::to_string` (stripped of surrounding whitespace and
/// truncated to 60 chars). Empty/null returns empty so the headline
/// degrades to just `iter[N]`.
fn item_input_label(value: &serde_json::Value) -> String {
    let raw = match value {
        serde_json::Value::Null => return String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= 60 {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(57).collect();
        format!("{head}…")
    }
}

fn process_event(
    ev: TxEvent,
    step: &mut StepState,
    printer: &mut LineStreamPrinter,
    total_tokens: &mut u64,
    live_event_hook: Option<&LiveWorkflowEventHook>,
    view_mode: LiveViewMode,
) {
    match ev {
        TxEvent::RunStart {
            agent,
            provider,
            model,
            ..
        } => {
            step.agent = Some(agent);
            step.provider = Some(provider);
            step.model = Some(model);
        }
        TxEvent::AssistantMessage { content, .. } if !content.trim().is_empty() => {
            render_assistant_output(printer, &content, view_mode);
        }
        TxEvent::ToolCall {
            call_id,
            tool,
            input,
        } => match tool.as_str() {
            "dispatch_agent" => {
                // Record the in-flight dispatch so the matching
                // ToolResult can replay the child as a nested frame.
                // Don't emit a `dispatch_agent <agent>` summary line —
                // the child callout itself is the rendering.
                let agent = input
                    .get("agent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                step.dispatches
                    .insert(call_id, InFlightDispatch::Single { agent });
            }
            "dispatch_agents_parallel" => {
                step.dispatches.insert(call_id, InFlightDispatch::Parallel);
            }
            _ => {
                if is_promoted_live_tool(&tool) {
                    step.promoted_actions
                        .insert(call_id.clone(), (tool.clone(), input.clone()));
                }
                let summary = summarize_tool_input(&tool, &input);
                printer.tool_call(&tool, &summary);
            }
        },
        TxEvent::ToolResult {
            call_id,
            output,
            error,
            ..
        } => {
            match step.dispatches.remove(&call_id) {
                Some(InFlightDispatch::Single { agent }) => {
                    render_dispatch_child(&agent, &output, printer, view_mode);
                }
                Some(InFlightDispatch::Parallel) => {
                    render_dispatch_children(&output, printer, view_mode);
                }
                None => {}
            }
            if error.is_none() {
                if let Some((tool, input)) = step.promoted_actions.remove(&call_id) {
                    if let Some(hook) = live_event_hook {
                        let event = LiveWorkflowEvent::ToolSucceeded {
                            run_id: step.run_id.clone(),
                            step_id: step.step_id.clone(),
                            tool,
                            input,
                        };
                        if let Some(render) = hook(&event) {
                            printer.sideband_event(
                                render.status,
                                &render.label,
                                render.detail.as_deref(),
                            );
                        }
                    }
                }
            } else {
                step.promoted_actions.remove(&call_id);
            }
        }
        TxEvent::RunComplete {
            status,
            total_tokens: tokens,
            duration_ms,
            error,
            ..
        } => {
            if let Some(spinner) = step.spinner.take() {
                spinner.stop();
            }
            *total_tokens += tokens;
            let dur = Duration::from_millis(duration_ms);
            match status {
                rupu_transcript::RunStatus::Ok => {
                    printer.step_done(&step.step_id, dur, tokens);
                }
                rupu_transcript::RunStatus::Error | rupu_transcript::RunStatus::Aborted => {
                    let reason = error.as_deref().unwrap_or("unknown");
                    printer.step_failed(&step.step_id, reason);
                }
            }
        }
        _ => {}
    }
}

fn is_promoted_live_tool(tool: &str) -> bool {
    matches!(
        tool,
        "issues.comment" | "issues.update_state" | "scm.prs.create"
    )
}

/// Render a `dispatch_agent` child as an indent+1 callout under the
/// parent's frame. The child transcript is fully written by the time
/// the parent's `ToolResult` arrives (synchronous-replay model — the
/// runner only emits ToolResult after `run_agent` returns), so we open
/// a tailer and drain everything inline rather than streaming.
///
/// `output` is the JSON payload from the `dispatch_agent` tool: see
/// `crates/rupu-tools/src/dispatch_agent.rs` for the shape. We need
/// `transcript_path`, `tokens_used`, and `duration_ms` from it; if any
/// are missing or the file isn't on disk yet we degrade to a
/// header+immediate footer so the parent's stream stays coherent.
fn render_dispatch_child(
    agent_name: &str,
    output: &str,
    printer: &mut LineStreamPrinter,
    view_mode: LiveViewMode,
) {
    let parsed: serde_json::Value = match serde_json::from_str(output) {
        Ok(v) => v,
        Err(_) => return,
    };
    printer.push_indent();
    render_one_child(agent_name, &parsed, printer, view_mode);
    printer.pop_indent();
}

/// Render every child of a `dispatch_agents_parallel` call. The tool
/// returns `{ results: { id: outcome, ... }, all_succeeded }`; we open
/// a single indent+1 frame and emit one child callout per id, headlined
/// by the caller-chosen id (so two `security-reviewer` calls with
/// distinct ids stay distinguishable in the output). serde_json's
/// default `Map` is BTreeMap-backed, so iteration order is alphabetical
/// — the same order the parent agent saw when it parsed the result, so
/// the rendering matches what the model is reasoning about.
fn render_dispatch_children(
    output: &str,
    printer: &mut LineStreamPrinter,
    view_mode: LiveViewMode,
) {
    let parsed: serde_json::Value = match serde_json::from_str(output) {
        Ok(v) => v,
        Err(_) => return,
    };
    let Some(results) = parsed.get("results").and_then(|v| v.as_object()) else {
        return;
    };
    if results.is_empty() {
        return;
    }
    printer.push_indent();
    for (id, outcome) in results.iter() {
        render_one_child(id, outcome, printer, view_mode);
    }
    printer.pop_indent();
}

/// Render one child callout: open frame, replay the persisted
/// transcript inline, close frame. Shared between the single-dispatch
/// and parallel-dispatch renderers.
fn render_one_child(
    headline: &str,
    outcome: &serde_json::Value,
    printer: &mut LineStreamPrinter,
    view_mode: LiveViewMode,
) {
    let transcript_path = outcome["transcript_path"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_default();
    let tokens_used = outcome["tokens_used"].as_u64().unwrap_or(0);
    let duration_ms = outcome["duration_ms"].as_u64().unwrap_or(0);
    let success = outcome["ok"].as_bool().unwrap_or(true);
    let dispatch_error = outcome["error"].as_str();

    let events: Vec<TxEvent> =
        if !transcript_path.as_os_str().is_empty() && transcript_path.exists() {
            TranscriptTailer::new(&transcript_path).drain()
        } else {
            Vec::new()
        };

    let (provider, model) = events
        .iter()
        .find_map(|e| match e {
            TxEvent::RunStart {
                provider, model, ..
            } => Some((provider.clone(), model.clone())),
            _ => None,
        })
        .unwrap_or((String::new(), String::new()));

    if view_mode == LiveViewMode::Focused {
        let mut summary = summarize_compact_child_events(&events);
        if summary.provider.is_empty() {
            summary.provider = provider.clone();
        }
        if summary.model.is_empty() {
            summary.model = model.clone();
        }
        if !success && summary.error.is_none() {
            summary.error = dispatch_error.map(ToOwned::to_owned);
        }
        render_focused_dispatch_child(headline, success, &summary, printer);
        return;
    }

    let spinner = printer.step_start(
        headline,
        Some(headline),
        non_empty(&provider),
        non_empty(&model),
    );
    for ev in events {
        match ev {
            TxEvent::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                render_assistant_output(printer, &content, view_mode);
            }
            TxEvent::ToolCall { tool, input, .. } => {
                let summary = summarize_tool_input(&tool, &input);
                printer.tool_call(&tool, &summary);
            }
            _ => {}
        }
    }
    spinner.stop();
    if success {
        printer.step_done(headline, Duration::from_millis(duration_ms), tokens_used);
    } else {
        let reason = dispatch_error.unwrap_or("dispatch failed");
        printer.step_failed(headline, reason);
    }
}

fn flush_all_tailers(
    steps: &mut [StepState],
    printer: &mut LineStreamPrinter,
    total_tokens: &mut u64,
    view_mode: LiveViewMode,
) {
    for step in steps.iter_mut() {
        let events = step.tailer.drain();
        for ev in events {
            process_event(ev, step, printer, total_tokens, None, view_mode);
        }
        if let Some(spinner) = step.spinner.take() {
            spinner.stop();
        }
    }
}

fn render_assistant_output(
    printer: &mut LineStreamPrinter,
    content: &str,
    view_mode: LiveViewMode,
) {
    match view_mode {
        LiveViewMode::Full => printer.assistant_chunk(content),
        LiveViewMode::Focused => printer.sideband_event(
            UiStatus::Active,
            "assistant output",
            Some(&truncate_single_line(content, 96)),
        ),
    }
}

#[derive(Debug, Default, Clone)]
struct CompactChildSummary {
    provider: String,
    model: String,
    assistant_summary: Option<String>,
    tool_calls: usize,
    total_tokens: u64,
    duration_ms: u64,
    error: Option<String>,
}

fn summarize_compact_child_events(events: &[TxEvent]) -> CompactChildSummary {
    let mut out = CompactChildSummary::default();
    for ev in events {
        match ev {
            TxEvent::RunStart {
                provider, model, ..
            } => {
                if out.provider.is_empty() {
                    out.provider = provider.clone();
                }
                if out.model.is_empty() {
                    out.model = model.clone();
                }
            }
            TxEvent::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                if out.assistant_summary.is_none() {
                    out.assistant_summary = Some(truncate_single_line(content, 84));
                }
            }
            TxEvent::ToolCall { .. } => {
                out.tool_calls += 1;
            }
            TxEvent::RunComplete {
                total_tokens,
                duration_ms,
                status,
                error,
                ..
            } => {
                out.total_tokens += total_tokens;
                out.duration_ms = *duration_ms;
                if matches!(
                    status,
                    rupu_transcript::RunStatus::Error | rupu_transcript::RunStatus::Aborted
                ) {
                    out.error = error.clone();
                }
            }
            _ => {}
        }
    }
    out
}

fn render_focused_child_item(
    parent: &StepResultRecord,
    item: &ItemResultRecord,
    headline: &str,
    summary: &CompactChildSummary,
    printer: &mut LineStreamPrinter,
) {
    let status = if item.success {
        UiStatus::Complete
    } else {
        UiStatus::Failed
    };
    let detail = compact_child_detail(
        status,
        &summary.provider,
        &summary.model,
        summary.duration_ms,
        summary.total_tokens,
        match parent.kind {
            StepKind::Panel => Some(
                parent
                    .findings
                    .iter()
                    .filter(|finding| finding.source == item.sub_id)
                    .count(),
            ),
            _ => None,
        },
    );
    printer.tree_item(headline, status, Some(&detail));
    if !item.success {
        let reason = summary.error.as_deref().unwrap_or("item failed");
        printer.tree_note(&format!("error  {}", truncate_single_line(reason, 88)));
    } else if let Some(note) = compact_child_note(summary) {
        printer.tree_note(&note);
    }
}

fn render_focused_dispatch_child(
    headline: &str,
    success: bool,
    summary: &CompactChildSummary,
    printer: &mut LineStreamPrinter,
) {
    let status = if success {
        UiStatus::Complete
    } else {
        UiStatus::Failed
    };
    let detail = compact_child_detail(
        status,
        &summary.provider,
        &summary.model,
        summary.duration_ms,
        summary.total_tokens,
        None,
    );
    printer.tree_item(headline, status, Some(&detail));
    if !success {
        let reason = summary.error.as_deref().unwrap_or("dispatch failed");
        printer.tree_note(&format!("error  {}", truncate_single_line(reason, 88)));
    } else if let Some(note) = compact_child_note(summary) {
        printer.tree_note(&note);
    }
}

fn compact_child_detail(
    status: UiStatus,
    provider: &str,
    model: &str,
    duration_ms: u64,
    total_tokens: u64,
    findings_count: Option<usize>,
) -> String {
    let mut parts = Vec::new();
    if !model.is_empty() {
        parts.push(truncate_single_line(model, 24));
    } else if !provider.is_empty() {
        parts.push(truncate_single_line(provider, 18));
    }
    if let Some(findings_count) = findings_count {
        parts.push(if findings_count == 1 {
            "1 finding".to_string()
        } else {
            format!("{findings_count} findings")
        });
    } else {
        parts.push(match status {
            UiStatus::Failed => "failed".to_string(),
            _ => "done".to_string(),
        });
    }
    if duration_ms > 0 {
        parts.push(compact_duration(duration_ms));
    }
    if total_tokens > 0 {
        parts.push(format!("{total_tokens} tokens"));
    }
    parts.join("  ·  ")
}

fn compact_child_note(summary: &CompactChildSummary) -> Option<String> {
    if let Some(text) = summary.assistant_summary.as_deref() {
        return Some(format!(
            "assistant output  {}",
            truncate_single_line(text, 88)
        ));
    }
    if summary.tool_calls > 0 {
        let noun = if summary.tool_calls == 1 {
            "tool call"
        } else {
            "tool calls"
        };
        return Some(format!("{} {}", summary.tool_calls, noun));
    }
    None
}

fn compact_duration(duration_ms: u64) -> String {
    if duration_ms >= 1_000 {
        format!("{:.1}s", duration_ms as f64 / 1_000.0)
    } else {
        format!("{duration_ms}ms")
    }
}

fn render_workflow_intro(
    printer: &mut LineStreamPrinter,
    record: &RunRecord,
    view_mode: LiveViewMode,
) {
    if view_mode != LiveViewMode::Focused {
        return;
    }
    printer.sideband_event(
        workflow_run_status(record.status.as_str()),
        "status",
        Some(record.status.as_str()),
    );
    if let Some(issue_ref) = record.issue_ref.as_deref() {
        printer.sideband_event(
            UiStatus::Active,
            "issue",
            Some(&truncate_single_line(issue_ref, 96)),
        );
    }
    let workspace = format!(
        "{}  ·  {}",
        record.workspace_id,
        truncate_single_line(&record.workspace_path.display().to_string(), 72)
    );
    printer.sideband_event(UiStatus::Active, "workspace", Some(&workspace));
    let mut route = Vec::new();
    if let Some(backend) = record.backend_id.as_deref() {
        route.push(format!("backend {backend}"));
    }
    if let Some(worker) = record.worker_id.as_deref() {
        route.push(format!("worker {worker}"));
    }
    if !route.is_empty() {
        printer.sideband_event(UiStatus::Active, "route", Some(&route.join("  ·  ")));
    }
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

fn workflow_ticker_message(workflow_name: &str, record: Option<&RunRecord>) -> String {
    let Some(record) = record else {
        return format!("workflow {workflow_name}  ·  starting");
    };
    if let Some(step_id) = record.active_step_id.as_deref() {
        return format!("workflow {workflow_name}  ·  {step_id}");
    }
    match record.status {
        rupu_orchestrator::RunStatus::Pending => format!("workflow {workflow_name}  ·  pending"),
        rupu_orchestrator::RunStatus::Running => format!("workflow {workflow_name}  ·  running"),
        rupu_orchestrator::RunStatus::Completed => format!("workflow {workflow_name}  ·  complete"),
        rupu_orchestrator::RunStatus::Failed => format!("workflow {workflow_name}  ·  failed"),
        rupu_orchestrator::RunStatus::Rejected => format!("workflow {workflow_name}  ·  rejected"),
        rupu_orchestrator::RunStatus::AwaitingApproval => {
            let step = record.awaiting_step_id.as_deref().unwrap_or("approval");
            format!("workflow {workflow_name}  ·  awaiting {step}")
        }
    }
}

/// Produce a short summary for a tool call to show in the timeline.
pub fn tool_summary(tool: &str, input: &serde_json::Value) -> String {
    summarize_tool_input(tool, input)
}

fn summarize_tool_input(tool: &str, input: &serde_json::Value) -> String {
    // Helpers — `s_str(field)` reads a top-level string field; `truncate`
    // caps at width with an ellipsis. Pulled out so per-tool arms below
    // read as a flat dispatch table.
    let s_str = |k: &str| -> Option<String> {
        input
            .get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
    };
    let truncate = |s: String, max: usize| -> String {
        if s.chars().count() > max {
            let cut: String = s.chars().take(max).collect();
            format!("{cut}…")
        } else {
            s
        }
    };
    let owner_repo = || -> Option<String> {
        let owner = s_str("owner")?;
        let repo = s_str("repo")?;
        Some(format!("{owner}/{repo}"))
    };

    match tool {
        // ── built-in tools ───────────────────────────────────────────
        "bash" => s_str("command")
            .map(|s| truncate(s, 72))
            .unwrap_or_default(),
        "write_file" | "edit_file" => s_str("path").unwrap_or_default(),
        "read_file" => s_str("path").unwrap_or_default(),

        // ── MCP scm.* tools ──────────────────────────────────────────
        "scm.repos.get" | "scm.repos.list" => owner_repo().unwrap_or_default(),
        "scm.branches.list" | "scm.branches.create" => owner_repo().unwrap_or_default(),
        "scm.files.read" => {
            // Show `<owner>/<repo>:<path>` so the operator can see WHICH
            // file is being fetched, not just the owner.
            let or = owner_repo().unwrap_or_default();
            let path = s_str("path").unwrap_or_default();
            if or.is_empty() {
                path
            } else if path.is_empty() {
                or
            } else {
                format!("{or}:{path}")
            }
        }
        "scm.prs.list" => owner_repo().unwrap_or_default(),
        "scm.prs.get" | "scm.prs.diff" | "scm.prs.comment" => {
            // PR refs include `pr` (number); show `<owner>/<repo>#<N>`.
            let or = owner_repo().unwrap_or_default();
            let pr = input.get("pr").and_then(|v| v.as_u64());
            match (or.is_empty(), pr) {
                (false, Some(n)) => format!("{or}#{n}"),
                (false, None) => or,
                (true, Some(n)) => format!("#{n}"),
                _ => String::new(),
            }
        }
        "scm.prs.create" => {
            let or = owner_repo().unwrap_or_default();
            let title = s_str("title").unwrap_or_default();
            if title.is_empty() {
                or
            } else {
                truncate(format!("{or}: {title}"), 72)
            }
        }

        // ── MCP issues.* tools ───────────────────────────────────────
        "issues.list" => s_str("project").unwrap_or_default(),
        "issues.get" => {
            let project = s_str("project").unwrap_or_default();
            let n = input.get("number").and_then(|v| v.as_u64());
            match (project.is_empty(), n) {
                (false, Some(n)) => format!("{project}#{n}"),
                (false, None) => project,
                (true, Some(n)) => format!("#{n}"),
                _ => String::new(),
            }
        }
        "issues.comment" => {
            let project = s_str("project").unwrap_or_default();
            let n = input.get("number").and_then(|v| v.as_u64());
            let target = match (project.is_empty(), n) {
                (false, Some(n)) => format!("{project}#{n}"),
                (false, None) => project,
                (true, Some(n)) => format!("#{n}"),
                _ => "issue".to_string(),
            };
            format!("commented on {target}")
        }
        "issues.update_state" => {
            let project = s_str("project").unwrap_or_default();
            let n = input.get("number").and_then(|v| v.as_u64());
            let state = s_str("state").unwrap_or_else(|| "updated".into());
            let target = match (project.is_empty(), n) {
                (false, Some(n)) => format!("{project}#{n}"),
                (false, None) => project,
                (true, Some(n)) => format!("#{n}"),
                _ => "issue".to_string(),
            };
            format!("{target} → {state}")
        }
        "issues.create" => {
            let project = s_str("project").unwrap_or_default();
            let title = s_str("title").unwrap_or_default();
            if title.is_empty() {
                project
            } else {
                truncate(format!("{project}: {title}"), 72)
            }
        }

        // ── unknown tool: first string field, truncated ──────────────
        _ => {
            if let Some(obj) = input.as_object() {
                for (_, v) in obj.iter().take(1) {
                    if let Some(s) = v.as_str() {
                        return truncate(s.trim().to_string(), 60);
                    }
                }
            }
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    fn sample_run_record() -> RunRecord {
        RunRecord {
            id: "run_test".into(),
            workflow_name: "demo".into(),
            status: rupu_orchestrator::RunStatus::Running,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws".into(),
            workspace_path: PathBuf::from("/tmp/ws"),
            transcript_dir: PathBuf::from("/tmp/transcripts"),
            started_at: Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
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
        }
    }

    #[test]
    fn item_input_label_strings_pass_through() {
        assert_eq!(
            item_input_label(&serde_json::json!("src/foo.rs")),
            "src/foo.rs"
        );
    }

    #[test]
    fn item_input_label_null_returns_empty() {
        assert_eq!(item_input_label(&serde_json::Value::Null), "");
    }

    #[test]
    fn item_input_label_blank_string_returns_empty() {
        assert_eq!(item_input_label(&serde_json::json!("   ")), "");
    }

    #[test]
    fn item_input_label_truncates_long_strings() {
        let long = "a".repeat(120);
        let label = item_input_label(&serde_json::Value::String(long));
        // 57 chars + ellipsis = 58 chars total. The cap is 60 so 58
        // is the ceiling we land on for any string > 60 chars.
        assert_eq!(label.chars().count(), 58);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn item_input_label_renders_objects_as_compact_json() {
        let obj = serde_json::json!({"path": "src/foo.rs", "line": 42});
        let label = item_input_label(&obj);
        // Should be a compact JSON form — exact key order isn't
        // guaranteed but the path string should appear somewhere.
        assert!(label.contains("src/foo.rs"));
    }

    #[test]
    fn non_empty_filters_blank() {
        assert_eq!(non_empty("anthropic"), Some("anthropic"));
        assert_eq!(non_empty(""), None);
    }

    #[test]
    fn tool_summary_makes_issue_comment_explicit() {
        let input = serde_json::json!({"project":"Section9Labs/rupu-sandbox-gh","number":8});
        assert_eq!(
            tool_summary("issues.comment", &input),
            "commented on Section9Labs/rupu-sandbox-gh#8"
        );
    }

    #[test]
    fn tool_summary_makes_issue_state_change_explicit() {
        let input = serde_json::json!({"project":"Section9Labs/rupu-sandbox-gh","number":8,"state":"closed"});
        assert_eq!(
            tool_summary("issues.update_state", &input),
            "Section9Labs/rupu-sandbox-gh#8 → closed"
        );
    }

    #[test]
    fn process_event_emits_live_hook_for_promoted_tool_success() {
        let dir = tempdir().unwrap();
        let transcript = dir.path().join("step.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_hook = Arc::clone(&captured);
        let hook: LiveWorkflowEventHook = Arc::new(move |event| {
            let LiveWorkflowEvent::ToolSucceeded { tool, input, .. } = event;
            captured_hook.lock().unwrap().push(format!(
                "{}:{}",
                tool,
                input
                    .get("state")
                    .and_then(|value| value.as_str())
                    .unwrap_or("ok")
            ));
            Some(LiveWorkflowRender {
                status: UiStatus::Working,
                label: "updated".into(),
                detail: None,
            })
        });
        let mut step = StepState {
            tailer: TranscriptTailer::new(&transcript),
            run_id: "run_live".into(),
            step_id: "finish".into(),
            agent: None,
            provider: None,
            model: None,
            spinner: None,
            dispatches: BTreeMap::new(),
            promoted_actions: BTreeMap::new(),
        };
        let mut printer = LineStreamPrinter::new();
        let mut total_tokens = 0u64;

        process_event(
            TxEvent::ToolCall {
                call_id: "call_1".into(),
                tool: "issues.update_state".into(),
                input: serde_json::json!({
                    "project":"Section9Labs/rupu-sandbox-gh",
                    "number":8,
                    "state":"closed"
                }),
            },
            &mut step,
            &mut printer,
            &mut total_tokens,
            Some(&hook),
            LiveViewMode::Focused,
        );
        process_event(
            TxEvent::ToolResult {
                call_id: "call_1".into(),
                output: "ok".into(),
                error: None,
                duration_ms: 1,
            },
            &mut step,
            &mut printer,
            &mut total_tokens,
            Some(&hook),
            LiveViewMode::Focused,
        );

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            &["issues.update_state:closed".to_string()]
        );
    }

    #[test]
    fn active_step_attach_opens_linear_transcript_before_step_result() {
        let dir = tempdir().unwrap();
        let transcript = dir.path().join("step.jsonl");
        let mut record = sample_run_record();
        record.active_step_id = Some("implement".into());
        record.active_step_kind = Some(StepKind::Linear);
        record.active_step_agent = Some("builder".into());
        record.active_step_transcript_path = Some(transcript.clone());

        let mut opened = BTreeSet::new();
        let mut steps = Vec::new();
        let mut printer = LineStreamPrinter::new();
        let mut step_count = 0usize;

        ensure_active_step_attached(
            &record,
            &mut opened,
            &mut steps,
            &mut printer,
            &mut step_count,
        );

        assert!(opened.contains(&transcript));
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step_id, "implement");
        assert_eq!(steps[0].agent.as_deref(), Some("builder"));
        assert_eq!(step_count, 1);
    }

    #[test]
    fn active_step_attach_ignores_non_linear_steps() {
        let dir = tempdir().unwrap();
        let transcript = dir.path().join("step.jsonl");
        let mut record = sample_run_record();
        record.active_step_id = Some("fanout".into());
        record.active_step_kind = Some(StepKind::ForEach);
        record.active_step_transcript_path = Some(transcript);

        let mut opened = BTreeSet::new();
        let mut steps = Vec::new();
        let mut printer = LineStreamPrinter::new();
        let mut step_count = 0usize;

        ensure_active_step_attached(
            &record,
            &mut opened,
            &mut steps,
            &mut printer,
            &mut step_count,
        );

        assert!(opened.is_empty());
        assert!(steps.is_empty());
        assert_eq!(step_count, 0);
    }

    #[test]
    fn retained_workflow_screen_rows_respect_width_and_height() {
        let mut record = sample_run_record();
        record.issue_ref = Some("github:Section9Labs/rupu/issues/42".into());
        record.active_step_id = Some("implement".into());
        record.active_step_agent = Some("builder".into());
        let prefs = UiPrefs::resolve(
            &rupu_config::UiConfig::default(),
            false,
            None,
            None,
            Some(LiveViewMode::Focused),
        );
        let mut state = WorkflowInteractiveState::new(0);
        state.push_line(
            UiStatus::Active,
            "assistant output  ·  this is a deliberately long workflow event line that should wrap cleanly inside the retained workflow screen",
        );
        let rows = build_workflow_screen_rows_for_size(
            "demo",
            &record,
            &state,
            LiveViewMode::Focused,
            &prefs,
            52,
            14,
        );
        assert!(rows.len() <= 14);
        assert!(rows.iter().all(|row| visible_len(row) <= 52));
    }

    #[test]
    fn workflow_transcript_event_lines_full_keeps_more_assistant_text_than_focused() {
        let event = TxEvent::AssistantMessage {
            content: "This is a longer workflow assistant response that should stay fuller in full mode while focused mode truncates it.".into(),
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
        let focused =
            workflow_transcript_event_lines(&event, LiveViewMode::Focused, &focused_prefs);
        let full = workflow_transcript_event_lines(&event, LiveViewMode::Full, &full_prefs);
        assert_eq!(focused.len(), 1);
        assert!(!full.is_empty());
        assert!(full.len() >= focused.len());
    }
}
