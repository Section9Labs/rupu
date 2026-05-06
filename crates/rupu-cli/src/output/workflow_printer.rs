//! Workflow run → line-stream printer wiring.
//!
//! `WorkflowPrinter` drives a `LineStreamPrinter` from a live workflow run:
//! it polls `step_results.jsonl` for new completed steps and tails each
//! step's JSONL transcript file as events arrive.
//!
//! **Layout assumption** (matches the orchestrator's on-disk format):
//! - `runs_dir/<run_id>/run.json` — status + metadata.
//! - `runs_dir/<run_id>/step_results.jsonl` — one line per completed step;
//!   each line carries `transcript_path` pointing to the JSONL transcript.
//! - Individual step transcripts live at `transcript_path` (absolute path
//!   stored in `step_results.jsonl`).
//!
//! The poller also reads `run.json` to detect when the run transitions
//! to a terminal state (`completed` / `failed`) or an approval gate
//! (`awaiting_approval`). On `awaiting_approval` it fires the printer's
//! `approval_prompt` method and then calls back into the run-store to
//! approve or reject based on the user's response.

use super::{SpinnerHandle, TranscriptTailer, printer::LineStreamPrinter};
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

/// Per-step printer state.
struct StepState {
    tailer: TranscriptTailer,
    step_id: String,
    agent: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    /// Spinner handle for the running glyph. Dropped when the step ends.
    spinner: Option<SpinnerHandle>,
}

/// Drive `printer` from a live or recently-finished workflow run.
///
/// `run_id` — the workflow run id (e.g. `run_01…`).
/// `runs_dir` — `<global>/runs/`.
/// `printer` — the `LineStreamPrinter` to write to.
/// `run_store` — the `RunStore` for approve/reject callbacks.
///
/// Returns `Ok(())` when the run reaches a terminal state.
pub fn attach_and_print(
    workflow_name: &str,
    run_id: &str,
    runs_dir: &Path,
    transcript_dir: &Path,
    printer: &mut LineStreamPrinter,
    run_store: &rupu_orchestrator::RunStore,
) -> io::Result<()> {
    let run_dir = runs_dir.join(run_id);
    let run_json = run_dir.join("run.json");
    let step_results_log = run_dir.join("step_results.jsonl");

    // Wait for run.json to exist (the orchestrator creates the run dir
    // synchronously before dispatching any steps).
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

    // Print the workflow header from run.json.
    let started_at = {
        let bytes = std::fs::read(&run_json)?;
        let rec: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let started_at_str = rec["started_at"].as_str().unwrap_or("");
        chrono::DateTime::parse_from_rfc3339(started_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now())
    };
    printer.workflow_header(workflow_name, run_id, started_at);

    // Step-printer state: ordered by position in step_results.jsonl
    // (which is append-only).
    let mut seen_step_results: usize = 0;
    let mut steps: Vec<StepState> = Vec::new();
    let mut total_tokens: u64 = 0;
    // Print a phase separator before the second and later steps.
    let mut step_count: usize = 0;

    // Track which step transcript paths we've opened tailers for.
    let mut opened: BTreeSet<PathBuf> = BTreeSet::new();

    // Main polling loop.
    loop {
        // ── 1. Drain any newly completed steps from step_results.jsonl ──
        drain_step_results(
            &step_results_log,
            transcript_dir,
            &mut seen_step_results,
            &mut opened,
            &mut steps,
            printer,
            &mut step_count,
        );

        // ── 2. Drain events from all open tailers ──
        for step in &mut steps {
            let events = step.tailer.drain();
            for ev in events {
                process_event(ev, step, printer, &mut total_tokens);
            }
        }

        // ── 3. Check run.json for status transitions ──
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
                // The orchestrator has paused. Drain any remaining events
                // then present the approval prompt.
                flush_all_tailers(&mut steps, printer, &mut total_tokens);

                let step_id = record["awaiting_step_id"]
                    .as_str()
                    .unwrap_or("approval_gate")
                    .to_string();
                let prompt = record["approval_prompt"]
                    .as_str()
                    .unwrap_or("Approve this step?")
                    .to_string();

                // Keep a loop so 'v' can show findings and re-prompt.
                loop {
                    let ch = printer
                        .approval_prompt(&step_id, &prompt)
                        .unwrap_or('q');

                    match ch {
                        'v' | 'V' => {
                            // Load findings for the awaiting step from
                            // step_results.jsonl and display them.
                            let findings =
                                load_step_findings(&step_results_log, &step_id);
                            printer.print_findings(&findings);
                            // Loop back to re-show the prompt.
                        }
                        'a' | 'A' => {
                            let approver = whoami::username();
                            match run_store.approve(run_id, &approver, chrono::Utc::now()) {
                                Ok(_) => {
                                    printer.step_done(&step_id, Duration::ZERO, 0);
                                    println!();
                                    println!("Run paused. Resume with:");
                                    println!("  rupu workflow approve {run_id}");
                                    return Ok(());
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
                            let _ = run_store.reject(
                                run_id,
                                &approver,
                                reason,
                                chrono::Utc::now(),
                            );
                            println!("Run rejected.");
                            return Ok(());
                        }
                        _ => {
                            // q or anything else: abort print loop; run keeps
                            // going in background (spawned by workflow.rs).
                            println!();
                            println!("Detached from run. It is still running.");
                            println!("Re-attach with: rupu watch {run_id}");
                            return Ok(());
                        }
                    }
                }
            }
            "completed" => {
                // Drain remaining events then print footer.
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
                return Ok(());
            }
            "failed" | "rejected" => {
                // Drain remaining events then print failure footer.
                std::thread::sleep(Duration::from_millis(DRAIN_EXTRA_MS));
                flush_all_tailers(&mut steps, printer, &mut total_tokens);

                let err = record["error_message"]
                    .as_str()
                    .unwrap_or("unknown error");
                printer.workflow_failed(workflow_name, run_id, err);
                return Ok(());
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

/// Load findings for a specific step from `step_results.jsonl`.
/// Returns the findings from the *last* record for that step (in case of
/// retries / multiple iterations).
fn load_step_findings(log: &Path, step_id: &str) -> Vec<FindingRecord> {
    let Ok(bytes) = std::fs::read(log) else {
        return Vec::new();
    };
    let mut findings: Vec<FindingRecord> = Vec::new();
    for line in bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(rec): Result<StepResultRecord, _> = serde_json::from_slice(line) else {
            continue;
        };
        if rec.step_id == step_id {
            findings = rec.findings;
        }
    }
    findings
}

/// Read newly-appended lines from `step_results.jsonl`, open tailers
/// for new transcript files, and start the step block in the printer.
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

    // Only process lines we haven't seen yet.
    for line in lines.iter().skip(*seen) {
        *seen += 1;
        let Ok(rec): Result<serde_json::Value, _> = serde_json::from_slice(line) else {
            continue;
        };
        let step_id = rec["step_id"].as_str().unwrap_or("?").to_string();
        let skipped = rec["skipped"].as_bool().unwrap_or(false);
        if skipped {
            // Silently skip; the step had `when:` = false.
            continue;
        }
        let transcript_path_str = rec["transcript_path"].as_str().unwrap_or("");
        if transcript_path_str.is_empty() {
            continue;
        }
        let transcript_path = PathBuf::from(transcript_path_str);
        if opened.contains(&transcript_path) {
            continue;
        }
        opened.insert(transcript_path.clone());
        let tailer = TranscriptTailer::new(&transcript_path);

        // Print a phase separator between steps for visual rhythm.
        if *step_count > 0 {
            printer.phase_separator();
        }
        *step_count += 1;

        // Start the step header with an animated spinner glyph.
        let spinner = printer.step_start(&step_id, None, None, None);
        steps.push(StepState {
            tailer,
            step_id,
            agent: None,
            provider: None,
            model: None,
            spinner: Some(spinner),
        });
    }
    let _ = transcript_dir; // used by the caller for resolution; unused here
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
            // We already printed step_start with no metadata; we can't
            // retroactively update it (it's already on screen). The metadata
            // is visible in the transcript for drill-down.
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
            // Stop the spinner before printing the completion line.
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
        // Ignore TurnStart/TurnEnd/Usage/FileEdit/CommandRun/ActionEmitted/GateRequested
        // for the MVP line-stream view.
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
        // Stop any still-running spinners when we flush at end-of-run.
        if let Some(spinner) = step.spinner.take() {
            spinner.stop();
        }
    }
}

/// Produce a short summary string for a tool call to show in the timeline.
/// Exposed as `pub` for the watch command's replay mode.
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
            // For MCP tools, show the first string field if any.
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
