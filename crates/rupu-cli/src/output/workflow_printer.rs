//! Workflow run → line-stream printer wiring.
//!
//! `WorkflowPrinter` drives a `LineStreamPrinter` from a live workflow run:
//! it polls `step_results.jsonl` for new completed steps and tails each
//! step's JSONL transcript file as events arrive.
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

use super::{printer::LineStreamPrinter, SpinnerHandle, TranscriptTailer};
use rupu_orchestrator::{FindingRecord, StepResultRecord};
use rupu_transcript::Event as TxEvent;
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Poll interval between run-status checks.
const POLL_MS: u64 = 250;
/// How long to wait for the run dir to appear before giving up.
const RUN_DIR_TIMEOUT_MS: u64 = 5_000;
/// How long to keep polling after RunComplete before declaring done.
const DRAIN_EXTRA_MS: u64 = 200;

/// Per-step printer state for a non-panel (linear) step.
struct StepState {
    tailer: TranscriptTailer,
    step_id: String,
    agent: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    spinner: Option<SpinnerHandle>,
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
#[derive(Default, Clone, Copy)]
pub struct AttachOpts {
    /// When `true`, skip printing the workflow header. Used on resume
    /// attaches where the header is already on screen from the original
    /// invocation.
    pub skip_header: bool,
    /// Skip this many records from the start of `step_results.jsonl`
    /// before rendering. Used on resume to avoid re-printing prior steps
    /// that the user already saw.
    pub skip_count: usize,
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

    let mut seen_step_results: usize = opts.skip_count;
    let mut steps: Vec<StepState> = Vec::new();
    let mut total_tokens: u64 = 0;
    // For the rendered phase separators: start at the same count so
    // resumed steps don't get an extra leading separator.
    let mut step_count: usize = opts.skip_count;

    let mut opened: BTreeSet<PathBuf> = BTreeSet::new();

    loop {
        drain_step_results(
            &step_results_log,
            transcript_dir,
            &mut seen_step_results,
            &mut opened,
            &mut steps,
            printer,
            &mut step_count,
        );

        for step in &mut steps {
            let events = step.tailer.drain();
            for ev in events {
                process_event(ev, step, printer, &mut total_tokens);
            }
        }

        let record = match load_run_record(&run_json) {
            Some(r) => r,
            None => {
                std::thread::sleep(Duration::from_millis(POLL_MS));
                continue;
            }
        };

        let status = record["status"].as_str().unwrap_or("unknown");
        match status {
            "awaiting_approval" => {
                flush_all_tailers(&mut steps, printer, &mut total_tokens);

                let step_id = record["awaiting_step_id"]
                    .as_str()
                    .unwrap_or("approval_gate")
                    .to_string();
                let prompt = record["approval_prompt"]
                    .as_str()
                    .unwrap_or("Approve this step?")
                    .to_string();

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
            "completed" => {
                std::thread::sleep(Duration::from_millis(DRAIN_EXTRA_MS));
                drain_step_results(
                    &step_results_log,
                    transcript_dir,
                    &mut seen_step_results,
                    &mut opened,
                    &mut steps,
                    printer,
                    &mut step_count,
                );
                flush_all_tailers(&mut steps, printer, &mut total_tokens);

                let duration_ms = record["finished_at"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|fin| {
                        (fin.with_timezone(&chrono::Utc) - started_at)
                            .num_milliseconds()
                            .max(0) as u64
                    })
                    .unwrap_or(0);
                let dur = Duration::from_millis(duration_ms);
                printer.workflow_done(workflow_name, run_id, dur, total_tokens);
                return Ok(AttachOutcome::Done);
            }
            "failed" | "rejected" => {
                std::thread::sleep(Duration::from_millis(DRAIN_EXTRA_MS));
                flush_all_tailers(&mut steps, printer, &mut total_tokens);

                let err = record["error_message"].as_str().unwrap_or("unknown error");
                printer.workflow_failed(workflow_name, run_id, err);
                return Ok(AttachOutcome::Done);
            }
            _ => {}
        }

        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_run_record(run_json: &Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(run_json).ok()?;
    serde_json::from_slice(&bytes).ok()
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

        let is_panel = !rec.items.is_empty();

        if *step_count > 0 {
            printer.phase_separator();
        }
        *step_count += 1;

        if is_panel {
            // Open the panel header, then immediately render each panelist
            // summary line. Panel steps don't have a top-level transcript;
            // their events live in each panelist's transcript.
            let spinner = printer.panel_start(&rec.step_id, rec.items.len());
            for item in &rec.items {
                let count = rec
                    .findings
                    .iter()
                    .filter(|f| f.source == item.sub_id)
                    .count();
                printer.panelist_line(&item.sub_id, item.success, count);
            }
            spinner.stop();
            printer.panel_done(
                &rec.step_id,
                rec.success,
                rec.findings.len(),
                Duration::ZERO,
            );
        } else {
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
                step_id: rec.step_id.clone(),
                agent: None,
                provider: None,
                model: None,
                spinner: Some(spinner),
            });
        }
    }
    let _ = transcript_dir;
}

fn process_event(
    ev: TxEvent,
    step: &mut StepState,
    printer: &mut LineStreamPrinter,
    total_tokens: &mut u64,
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
            printer.assistant_chunk(&content);
        }
        TxEvent::ToolCall { tool, input, .. } => {
            let summary = summarize_tool_input(&tool, &input);
            printer.tool_call(&tool, &summary);
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

fn flush_all_tailers(
    steps: &mut [StepState],
    printer: &mut LineStreamPrinter,
    total_tokens: &mut u64,
) {
    for step in steps.iter_mut() {
        let events = step.tailer.drain();
        for ev in events {
            process_event(ev, step, printer, total_tokens);
        }
        if let Some(spinner) = step.spinner.take() {
            spinner.stop();
        }
    }
}

/// Produce a short summary for a tool call to show in the timeline.
pub fn tool_summary(tool: &str, input: &serde_json::Value) -> String {
    summarize_tool_input(tool, input)
}

fn summarize_tool_input(tool: &str, input: &serde_json::Value) -> String {
    match tool {
        "bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| {
                let trimmed = s.trim();
                if trimmed.len() > 72 {
                    format!("{}…", &trimmed[..72])
                } else {
                    trimmed.to_string()
                }
            })
            .unwrap_or_default(),
        "write_file" | "edit_file" => input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "read_file" => input
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            if let Some(obj) = input.as_object() {
                for (_, v) in obj.iter().take(1) {
                    if let Some(s) = v.as_str() {
                        let trimmed = s.trim();
                        if trimmed.len() > 60 {
                            return format!("{}…", &trimmed[..60]);
                        }
                        return trimmed.to_string();
                    }
                }
            }
            String::new()
        }
    }
}
