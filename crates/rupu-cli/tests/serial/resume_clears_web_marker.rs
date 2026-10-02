//! `rupu workflow resume` takes a run back from a pause (or a cancel) that a
//! web resume request was recorded against — the run's `resume_requested_at`
//! marker, with the gate, approver and mode it carries — and must consume
//! that marker as it does. Left behind, the marker sits on the running run
//! (the cp-serve resume worker lists only `AwaitingApproval` / `Paused`
//! runs) until the run parks at its next gate, and the worker then spawns
//! `workflow approve --approver web` for it: the gate is approved as "web"
//! although nobody approved it.
//!
//! Driven end to end through the real CLI (`rupu_cli::run(...)`). The run
//! is created `Paused` before its first step; `prep` and `ship` are
//! deterministic `run:` steps that append to marker files, so whether the
//! gate between them was approved is a binding effect, not a status read.

use crate::ENV_LOCK;
use assert_fs::prelude::*;
use rupu_orchestrator::{RunRecord, RunStatus, RunStore};

const WORKFLOW_PREP_GATE_SHIP: &str = r#"
name: prep-gate-ship
steps:
  - id: prep
    run:
      cmd: sh
      args: ["-c", "echo ran >> {{ inputs.prep }}"]
  - id: gate
    approval:
      prompt: "Ship it?"
  - id: ship
    run:
      cmd: sh
      args: ["-c", "echo ran >> {{ inputs.ship }}"]
"#;

struct Fixture {
    _tmp: assert_fs::TempDir,
    global: std::path::PathBuf,
    prep: std::path::PathBuf,
    ship: std::path::PathBuf,
    store: RunStore,
}

/// `<tmp>/.rupu` (with `run:` steps enabled) as `RUPU_HOME`, and a run of
/// `prep-gate-ship` paused before `prep` ran — as a cooperative pause at
/// the first step boundary leaves it.
fn paused_run(run_id: &str) -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global
        .child("config.toml")
        .write_str("[workflow]\nrun_step_enabled = true\n")
        .unwrap();
    let project = tmp.child("proj");
    project.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(project.path()).unwrap();

    let (prep, ship) = (tmp.path().join("prep.txt"), tmp.path().join("ship.txt"));
    let now = chrono::Utc::now();
    let record: RunRecord = serde_json::from_value(serde_json::json!({
        "id": run_id,
        "workflow_name": "prep-gate-ship",
        "status": "paused",
        "inputs": {
            "prep": prep.display().to_string(),
            "ship": ship.display().to_string(),
        },
        "workspace_id": "ws_test",
        "workspace_path": project.path(),
        "transcript_dir": global.path().join("transcripts"),
        "started_at": now,
        "awaiting_step_id": "prep",
        "awaiting_since": now,
    }))
    .unwrap();
    let store = RunStore::new(global.path().join("runs"));
    store.create(record, WORKFLOW_PREP_GATE_SHIP).unwrap();
    Fixture {
        global: global.path().to_path_buf(),
        _tmp: tmp,
        prep,
        ship,
        store,
    }
}

fn lines(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

async fn rupu(args: &[&str]) {
    let mut argv = vec!["rupu".to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let _exit = rupu_cli::run(argv).await;
}

/// The web's `POST /api/runs/:id/resume` on the paused run (the CP calls
/// exactly this), checked to have left the marker the resume worker acts on.
fn web_resume(store: &RunStore, run_id: &str) {
    store
        .request_resume_approval(run_id, "web", Some("bypass"), chrono::Utc::now(), None)
        .unwrap();
    let rec = store.load(run_id).unwrap();
    assert!(rec.resume_requested_at.is_some());
    assert_eq!(rec.resume_approver.as_deref(), Some("web"));
}

/// The run parked at `gate` after the CLI resume, with no resume request
/// left on it: none of the marker's fields, and nothing the resume worker
/// lists — so nothing approves the gate.
fn assert_parked_with_no_resume_request(fx: &Fixture, run_id: &str) {
    let rec = fx.store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::AwaitingApproval);
    let parked: Vec<&str> = rec.awaiting.iter().map(|g| g.step_id.as_str()).collect();
    assert_eq!(parked, ["gate"]);
    assert!(rec.gate_decisions.is_empty(), "nobody decided the gate");
    assert_eq!(
        (
            rec.resume_requested_at,
            rec.resume_gate_id.as_deref(),
            rec.resume_approver.as_deref(),
            rec.resume_mode.as_deref(),
        ),
        (None, None, None, None),
        "the web's resume request was for the pause the CLI resumed; it must not outlive it"
    );
    let pending = fx
        .store
        .list_pending_resume(chrono::Utc::now() + chrono::Duration::seconds(1))
        .unwrap();
    assert!(
        pending.iter().all(|r| r.id != run_id),
        "the resume worker would approve the new gate as \"web\": {pending:?}"
    );
    assert_eq!(lines(&fx.prep), 1, "the resumed run ran `prep` once");
    assert_eq!(lines(&fx.ship), 0, "nothing approved the gate");
}

/// The web asked to resume the pause; the operator resumed it from the CLI
/// before the resume worker's next poll. The CLI resume consumes the web's
/// request, so the gate the run parks at next stays parked — and a web
/// approve of THAT gate, recorded after, is the resume worker's to act on
/// as usual.
#[tokio::test(flavor = "multi_thread")]
async fn cli_resume_of_a_paused_run_consumes_the_web_resume_request() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_paused_web_then_cli";
    let fx = paused_run(run_id);
    web_resume(&fx.store, run_id);

    rupu(&["workflow", "resume", run_id, "--plain"]).await;
    assert_parked_with_no_resume_request(&fx, run_id);

    // A web approve of the gate the run now waits at.
    fx.store
        .request_resume_approval(run_id, "web", None, chrono::Utc::now(), None)
        .unwrap();
    let pending = fx
        .store
        .list_pending_resume(chrono::Utc::now() + chrono::Duration::seconds(1))
        .unwrap();
    assert!(
        pending.iter().any(|r| r.id == run_id),
        "a web approve of the new gate is the resume worker's: {pending:?}"
    );
    // The worker spawns `workflow resume` for a recorded decision; its
    // runner applies the approval and serves the request.
    rupu(&["workflow", "resume", run_id, "--plain"]).await;
    let rec = fx.store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert_eq!(lines(&fx.prep), 1);
    assert_eq!(
        lines(&fx.ship),
        1,
        "the web's approval of the gate ran `ship`"
    );
    assert_eq!(rec.resume_requested_at, None);
    let gate = RunStore::new(fx.global.join("runs"))
        .read_step_results(run_id)
        .unwrap()
        .into_iter()
        .find(|r| r.step_id == "gate")
        .expect("the gate's result row");
    let decision: serde_json::Value = serde_json::from_str(&gate.output).unwrap();
    assert_eq!(
        (&decision["decision"], &decision["approver"]),
        (&serde_json::json!("approved"), &serde_json::json!("web")),
        "the gate was approved by the web, as recorded: {decision}"
    );
}

const WORKFLOW_TWO_GATED_PATHS: &str = r#"
name: two-gated-paths
steps:
  - id: fanout
    split: [gate_a, gate_b]
  - id: gate_a
    approval:
      prompt: "Approve A?"
    next: [build_a]
  - id: gate_b
    approval:
      prompt: "Approve B?"
    next: [build_b]
  - id: build_a
    run:
      cmd: sh
      args: ["-c", "echo ran >> {{ inputs.marker_a }}"]
  - id: build_b
    run:
      cmd: sh
      args: ["-c", "echo ran >> {{ inputs.marker_b }}"]
"#;

/// The other way an operator races a web decision: the web approved
/// `gate_a` (decision + marker) and the operator approves `gate_b` from the
/// CLI before the resume worker polls. The CLI approve's runner applies
/// every decision recorded on the run — the web's included — so it serves
/// the marker too, and nothing is left for the worker to act on.
#[tokio::test(flavor = "multi_thread")]
async fn cli_approve_serves_a_sibling_gates_web_approval_and_its_request() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global
        .child("config.toml")
        .write_str("[workflow]\nrun_step_enabled = true\n")
        .unwrap();
    let project = tmp.child("proj");
    project
        .child(".rupu/workflows/two-gated-paths.yaml")
        .write_str(WORKFLOW_TWO_GATED_PATHS)
        .unwrap();
    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_current_dir(project.path()).unwrap();
    let (marker_a, marker_b) = (tmp.path().join("a.txt"), tmp.path().join("b.txt"));
    let run_id = "run_web_a_cli_b";
    rupu(&[
        "workflow",
        "run",
        "two-gated-paths",
        "--input",
        &format!("marker_a={}", marker_a.display()),
        "--input",
        &format!("marker_b={}", marker_b.display()),
        "--run-id",
        run_id,
        "--plain",
    ])
    .await;
    let store = RunStore::new(global.path().join("runs"));
    assert_eq!(store.load(run_id).unwrap().awaiting.len(), 2);

    store
        .request_resume_approval(run_id, "web", None, chrono::Utc::now(), Some("gate_a"))
        .unwrap();
    rupu(&["workflow", "approve", run_id, "--gate", "gate_b"]).await;

    let rec = store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert_eq!((lines(&marker_a), lines(&marker_b)), (1, 1));
    assert_eq!(
        (rec.resume_requested_at, rec.resume_approver.as_deref()),
        (None, None),
        "the CLI approve's runner applied the web's decision, so it served its request"
    );
    assert!(store
        .list_pending_resume(chrono::Utc::now() + chrono::Duration::seconds(1))
        .unwrap()
        .is_empty());
}

/// A cancel keeps the run's web resume request (a cancelled run is never
/// resumed by the worker, so it is inert there) — until the operator
/// resumes the cancelled run from the CLI, which must not bring the
/// request back to life at the next gate.
#[tokio::test(flavor = "multi_thread")]
async fn cli_resume_of_a_cancelled_run_drops_the_web_resume_request_it_carried() {
    let _guard = ENV_LOCK.lock().await;
    let run_id = "run_paused_web_cancel_cli";
    let fx = paused_run(run_id);
    web_resume(&fx.store, run_id);
    rupu(&["workflow", "cancel", run_id]).await;
    let rec = fx.store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Cancelled);
    assert!(
        rec.resume_requested_at.is_some(),
        "the premise: the cancel kept the marker"
    );

    rupu(&["workflow", "resume", run_id, "--plain"]).await;
    assert_parked_with_no_resume_request(&fx, run_id);
}
