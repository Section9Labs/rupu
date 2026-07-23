//! Gate/Action Plan 4 — sweep smoke test (Task 4, Step 2).
//!
//! `run_gate_sweep` (rupu-cli's `cmd/cp.rs`) is the IO tick body the
//! `rupu cp serve` background sweep runs on an interval; it isn't
//! reachable from a cheap test because its `on_timeout: reject` branch
//! rebuilds a full `OrchestratorRunOpts` from disk (`rebuild_opts_from_disk`
//! in `rupu-cli/src/resume.rs`) — real `$RUPU_HOME`/global config, a real
//! `KeychainResolver`, and a real `rupu_scm::Registry::discover` — none of
//! which a unit test can cheaply fabricate, and none of which rupu-cli
//! exposes as an injectable seam.
//!
//! What IS reachable and load-bearing to verify directly: the two
//! `RunStore` primitives the sweep's decision tree is built on,
//! `resolve_gate_timeout` + `expire_if_overdue` + `reap_if_orphaned`
//! (`crates/rupu-orchestrator/src/runs.rs`), exercised here exactly as
//! `run_gate_sweep` calls them — against a REAL persisted gate-node
//! workflow snapshot (not a synthetic `TimeoutAction` value), asserting
//! the same two postconditions the full daemon must produce: the run
//! ends in the correct terminal `RunStatus` AND `events.jsonl`'s last
//! line is the matching terminal event (the exact class of bug PR #501
//! fixed — a live/spinning run because no terminal event was appended).
//!
//! See the Task 4 report for the manual `rupu cp serve` smoke-test
//! commands that exercise the full tick body (config wiring + the
//! on_reject cleanup chain + the detached-approve spawn), which this
//! test does not attempt.

use chrono::Utc;
use rupu_orchestrator::{RunRecord, RunStatus, RunStore, TimeoutAction};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

/// A real gate-node workflow, shaped like `.rupu/workflows/gate-demo.yaml`:
/// a standalone `approval:` step (no agent/prompt alongside it) with
/// `on_timeout: reject` and a non-empty `on_reject:` cleanup chain. The
/// sweep never executes the chain in this test (that needs the full
/// runtime) — its presence here only proves `resolve_gate_timeout` parses
/// a realistic snapshot end-to-end, not a hand-built `TimeoutAction`.
const GATE_WORKFLOW_YAML: &str = "\
name: sweep-smoke
steps:
  - id: ship_gate
    approval:
      prompt: \"approve to continue?\"
      timeout_seconds: 5
      on_timeout: reject
      on_reject:
        - id: note_rejection
          agent: reviewer
          prompt: \"summarize the rejection\"
";

fn base_record(id: &str) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "sweep-smoke".into(),
        status: RunStatus::Pending,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_1".into(),
        workspace_path: PathBuf::from("/tmp/proj"),
        transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
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
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        final_output: None,
    }
}

fn last_event_line(runs_root: &std::path::Path, run_id: &str) -> serde_json::Value {
    let body = std::fs::read_to_string(runs_root.join(run_id).join("events.jsonl"))
        .expect("events.jsonl should exist after a store-side terminal transition");
    serde_json::from_str(body.lines().last().expect("at least one event line"))
        .expect("terminal event line parses as JSON")
}

/// (a) An `AwaitingApproval` gate run whose `expires_at` is in the past,
/// parked at a gate NODE with `on_timeout: reject` in its persisted
/// snapshot. Mirrors `run_gate_sweep`'s `AwaitingApproval` arm:
/// `resolve_gate_timeout` then `expire_if_overdue`. Must end `Rejected`
/// with `events.jsonl` ending in a terminal (rejected) event.
#[test]
fn sweep_reachable_pieces_expire_overdue_gate_to_rejected_with_terminal_event() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().to_path_buf());

    let mut rec = base_record("run_sweep_gate_overdue");
    rec.status = RunStatus::AwaitingApproval;
    rec.awaiting_step_id = Some("ship_gate".into());
    rec.approval_prompt = Some("approve to continue?".into());
    rec.awaiting_since = Some(Utc::now() - chrono::Duration::seconds(30));
    // Overdue: expires_at is in the past relative to `now` below.
    rec.expires_at = Some(Utc::now() - chrono::Duration::seconds(10));
    store.create(rec.clone(), GATE_WORKFLOW_YAML).unwrap();

    let mut loaded = store.load(&rec.id).unwrap();

    // Same call `run_gate_sweep` makes before `expire_if_overdue`: resolve
    // the gate's on_timeout policy from the REAL persisted snapshot.
    let on_timeout = store.resolve_gate_timeout(&loaded);
    assert_eq!(
        on_timeout,
        Some(TimeoutAction::Reject),
        "on_timeout: reject in the persisted gate-node snapshot must resolve to Reject"
    );

    let now = Utc::now();
    let outcome = store
        .expire_if_overdue(&mut loaded, now, on_timeout)
        .unwrap();
    assert_eq!(outcome, Some(TimeoutAction::Reject));

    let reloaded = store.load(&rec.id).unwrap();
    assert_eq!(reloaded.status, RunStatus::Rejected);
    assert!(reloaded.status.is_terminal());
    assert!(reloaded.finished_at.is_some());
    assert!(reloaded.awaiting_step_id.is_none());

    let last = last_event_line(tmp.path(), &rec.id);
    assert_eq!(last["type"], "run_completed", "events.jsonl must end terminal, last: {last}");
    assert_eq!(last["status"], "rejected", "last: {last}");
}

/// (b) A `Running` run with a dead recorded `runner_pid` — the orphan
/// class the gate sweep's second arm reaps. Must end `Failed` with
/// `events.jsonl` ending in a terminal (failed) event.
#[test]
fn sweep_reachable_pieces_reap_orphaned_running_run_to_failed_with_terminal_event() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().to_path_buf());

    // u32::MAX is not a valid pid on any supported platform (same
    // convention `reap_if_orphaned_finalizes_dead_pid_running_run` in
    // runs.rs and `resume_blocked_by_live_runner_covers_all_cases` in
    // rupu-cli/src/cmd/workflow.rs both use).
    let dead_pid = u32::MAX;

    let mut rec = base_record("run_sweep_orphan");
    rec.status = RunStatus::Running;
    rec.runner_pid = Some(dead_pid);
    rec.active_step_id = Some("some_step".into());
    store.create(rec.clone(), GATE_WORKFLOW_YAML).unwrap();

    let mut loaded = store.load(&rec.id).unwrap();
    let now = Utc::now();
    let reaped = store.reap_if_orphaned(&mut loaded, now).unwrap();
    assert!(reaped, "a dead recorded runner_pid must be reaped");

    let reloaded = store.load(&rec.id).unwrap();
    assert_eq!(reloaded.status, RunStatus::Failed);
    assert!(reloaded.status.is_terminal());
    assert!(reloaded.finished_at.is_some());
    assert!(reloaded.runner_pid.is_none());

    let last = last_event_line(tmp.path(), &rec.id);
    assert_eq!(last["type"], "run_failed", "events.jsonl must end terminal, last: {last}");
    assert!(
        last["error"]
            .as_str()
            .unwrap_or_default()
            .contains(&dead_pid.to_string()),
        "last: {last}"
    );
}
