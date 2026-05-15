use crate::cmd::transcript::truncate_single_line;
use crate::cmd::ui::{LiveViewMode, UiPrefs};
use crate::output::palette::Status as UiStatus;
use crate::output::rich_payload::render_payload;
use crate::output::workflow_printer::{
    attach_and_print_with, attach_and_render_interactive_with, tool_summary, AttachOpts,
};
use crate::output::LineStreamPrinter;
use crate::paths;
use clap::Args;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Run id (e.g. `run_01HZ…`)
    pub run_id: String,

    /// Replay a finished run instead of tailing live.
    #[arg(long)]
    pub replay: bool,

    /// Replay pace in events per second (only with --replay).
    #[arg(long, default_value_t = 10.0)]
    pub pace: f32,

    /// Live view density for retained watch screens.
    #[arg(long)]
    pub view: Option<LiveViewMode>,

    /// Follow a live run (tail mode). Re-scans the transcript dir
    /// every 250ms until the run reaches a terminal state.
    #[arg(long)]
    pub follow: bool,
}

pub async fn handle(args: WatchArgs) -> ExitCode {
    handle_inner(args)
}

fn handle_inner(args: WatchArgs) -> ExitCode {
    let runs_dir = match paths::global_dir() {
        Ok(d) => d.join("runs"),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };

    // Default: line-stream output.
    // Load the run record to get the transcript_dir and workflow_name.
    let store = rupu_orchestrator::RunStore::new(runs_dir.clone());
    let record = match store.load(&args.run_id) {
        Ok(r) => r,
        Err(rupu_orchestrator::RunStoreError::NotFound(_)) => {
            eprintln!(
                "error: run \"{}\" not found. Suggest `rupu workflow runs`",
                args.run_id
            );
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("rupu watch: {e}");
            return ExitCode::from(1);
        }
    };

    let prefs = resolve_watch_prefs(args.view);
    let view_mode = prefs.live_view;
    let transcript_dir = record.transcript_dir.clone();
    let workflow_name = record.workflow_name.clone();
    let run_id = args.run_id.clone();

    if args.replay {
        // Replay: dump the already-written transcripts through the printer.
        let result = replay_with_printer(
            &workflow_name,
            &run_id,
            &runs_dir,
            &transcript_dir,
            args.pace,
            view_mode,
            &prefs,
        );
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => crate::output::diag::fail(e),
        }
    } else {
        // Live watch uses the retained interactive workflow view when
        // attached to a tty; pipes/non-tty keep the line-stream path.
        let attach_opts = AttachOpts {
            view_mode,
            ..AttachOpts::default()
        };
        let outcome = if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            attach_and_render_interactive_with(
                &workflow_name,
                &run_id,
                &runs_dir,
                &store,
                attach_opts,
            )
        } else {
            let mut printer = LineStreamPrinter::new();
            attach_and_print_with(
                &workflow_name,
                &run_id,
                &runs_dir,
                &transcript_dir,
                &mut printer,
                &store,
                attach_opts,
            )
        };
        match outcome {
            Ok(crate::output::workflow_printer::AttachOutcome::Approved { awaited_step_id }) => {
                // We persisted the approval, but `rupu watch` doesn't have
                // the workflow YAML / factory needed to spin a resume run
                // inline. Surface the next step to the operator.
                println!();
                println!(
                    "Step `{awaited_step_id}` approved. The watcher process \
                     can't dispatch the resume itself — run:"
                );
                println!("  rupu workflow approve {run_id}");
                ExitCode::SUCCESS
            }
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => crate::output::diag::fail(e),
        }
    }
}

/// Replay a finished run by walking already-written step transcripts and
/// printing events through the line-stream printer with a pace delay.
fn replay_with_printer(
    workflow_name: &str,
    run_id: &str,
    runs_dir: &std::path::Path,
    _transcript_dir: &std::path::Path,
    pace: f32,
    view_mode: LiveViewMode,
    prefs: &UiPrefs,
) -> std::io::Result<()> {
    let run_dir = runs_dir.join(run_id);
    let step_results_log = run_dir.join("step_results.jsonl");
    let run_json = run_dir.join("run.json");

    let pace_delay = std::time::Duration::from_micros((1_000_000.0 / pace.max(0.1)) as u64);

    // Load run record for the header.
    let started_at = load_run_started_at(&run_json).unwrap_or_else(chrono::Utc::now);
    let mut printer = LineStreamPrinter::new();
    printer.workflow_header(workflow_name, run_id, started_at);

    // Read step_results.jsonl to get ordered transcript paths.
    let Ok(bytes) = std::fs::read(&step_results_log) else {
        return Ok(());
    };
    let mut total_tokens = u64::MAX; // sentinel; overwritten per step

    for line in bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        let Ok(rec): Result<serde_json::Value, _> = serde_json::from_slice(line) else {
            continue;
        };
        let step_id = rec["step_id"].as_str().unwrap_or("?").to_string();
        let skipped = rec["skipped"].as_bool().unwrap_or(false);
        if skipped {
            continue;
        }
        let transcript_path_str = rec["transcript_path"].as_str().unwrap_or("");
        if transcript_path_str.is_empty() {
            continue;
        }
        let transcript_path = PathBuf::from(transcript_path_str);
        let Ok(tx_bytes) = std::fs::read(&transcript_path) else {
            continue;
        };

        printer.step_start(&step_id, None, None, None);

        for tx_line in tx_bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
            let Ok(ev) = serde_json::from_slice::<rupu_transcript::Event>(tx_line) else {
                continue;
            };
            std::thread::sleep(pace_delay);
            match ev {
                rupu_transcript::Event::AssistantMessage { content, .. }
                    if !content.trim().is_empty() =>
                {
                    render_replay_assistant_output(&mut printer, &content, view_mode);
                }
                rupu_transcript::Event::ToolCall { tool, input, .. } => {
                    let summary = tool_summary(&tool, &input);
                    printer.tool_call(&tool, &summary);
                }
                rupu_transcript::Event::ToolResult {
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
                    let label = if error.is_some() { "tool error" } else { "tool result" };
                    let raw = error.as_deref().unwrap_or(output.as_str());
                    let rendered = render_payload(raw, prefs);
                    let mut detail = rendered.headline.clone();
                    if duration_ms > 0 {
                        detail.push_str(&format!("  ·  {duration_ms}ms"));
                    }
                    printer.sideband_event(status, label, Some(&detail));
                }
                rupu_transcript::Event::FileEdit { path, kind, diff } => {
                    let rendered = render_payload(&diff, prefs);
                    let detail = format!(
                        "{} {}  ·  {}",
                        format!("{kind:?}").to_lowercase(),
                        path,
                        rendered.headline
                    );
                    printer.sideband_event(UiStatus::Complete, "file edit", Some(&detail));
                }
                rupu_transcript::Event::RunComplete {
                    status,
                    total_tokens: tokens,
                    duration_ms,
                    error,
                    ..
                } => {
                    let dur = std::time::Duration::from_millis(duration_ms);
                    total_tokens = tokens;
                    match status {
                        rupu_transcript::RunStatus::Ok => {
                            printer.step_done(&step_id, dur, tokens);
                        }
                        _ => {
                            let reason = error.as_deref().unwrap_or("unknown");
                            printer.step_failed(&step_id, reason);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Print the footer.
    if let Some(rec) = load_run_json_value(&run_json) {
        let status = rec["status"].as_str().unwrap_or("unknown");
        let duration_ms = rec["finished_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|fin| {
                (fin.with_timezone(&chrono::Utc) - started_at)
                    .num_milliseconds()
                    .max(0) as u64
            })
            .unwrap_or(0);
        let dur = std::time::Duration::from_millis(duration_ms);
        match status {
            "completed" => {
                printer.workflow_done(workflow_name, run_id, dur, total_tokens);
            }
            "failed" | "rejected" => {
                let err = rec["error_message"].as_str().unwrap_or("unknown");
                printer.workflow_failed(workflow_name, run_id, err);
            }
            _ => {}
        }
    }

    Ok(())
}

fn load_run_started_at(run_json: &std::path::Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let bytes = std::fs::read(run_json).ok()?;
    let rec: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let s = rec["started_at"].as_str()?;
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
}

fn load_run_json_value(run_json: &std::path::Path) -> Option<serde_json::Value> {
    let bytes = std::fs::read(run_json).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn resolve_watch_prefs(view: Option<LiveViewMode>) -> UiPrefs {
    let global = paths::global_dir().ok();
    let project_root = std::env::current_dir()
        .ok()
        .and_then(|pwd| paths::project_root_for(&pwd).ok().flatten());
    let cfg = global
        .as_deref()
        .map(|global| {
            rupu_config::layer_files(
                Some(&global.join("config.toml")),
                project_root
                    .as_deref()
                    .map(|root| root.join(".rupu/config.toml"))
                    .as_deref(),
            )
            .unwrap_or_default()
        })
        .unwrap_or_default();
    UiPrefs::resolve(&cfg.ui, false, None, None, view)
}

fn render_replay_assistant_output(
    printer: &mut LineStreamPrinter,
    content: &str,
    view_mode: LiveViewMode,
) {
    match view_mode {
        LiveViewMode::Focused => printer.sideband_event(
            UiStatus::Active,
            "assistant output",
            Some(&truncate_single_line(content, 96)),
        ),
        LiveViewMode::Compact | LiveViewMode::Full => printer.assistant_chunk(content),
    }
}
