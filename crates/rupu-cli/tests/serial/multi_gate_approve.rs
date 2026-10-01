//! `rupu workflow approve|reject --gate` on a run whose two independent
//! paths each parked a gate (spec §7: gate decisions are path-scoped),
//! driven end to end through the real CLI (`rupu_cli::run(...)`).
//!
//! Each gated path ends in a deterministic `run:` step that APPENDS a line
//! to its own marker file, so the assertions are on binding effects: which
//! path ran, and how many times. Approving one gate runs only its own path;
//! the sibling stays parked and approvable; deciding it runs (or prunes)
//! only the sibling's path — never the first one again.

use crate::ENV_LOCK;
use assert_fs::prelude::*;

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

struct Fixture {
    _tmp: assert_fs::TempDir,
    project: std::path::PathBuf,
    global: std::path::PathBuf,
    marker_a: std::path::PathBuf,
    marker_b: std::path::PathBuf,
}

fn fixture() -> Fixture {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global
        .child("config.toml")
        .write_str("[workflow]\nrun_step_enabled = true\n")
        .unwrap();
    let project = tmp.child("proj");
    project.child(".rupu/workflows").create_dir_all().unwrap();
    project
        .child(".rupu/workflows/two-gated-paths.yaml")
        .write_str(WORKFLOW_TWO_GATED_PATHS)
        .unwrap();
    Fixture {
        marker_a: tmp.path().join("a.txt"),
        marker_b: tmp.path().join("b.txt"),
        project: project.path().to_path_buf(),
        global: global.path().to_path_buf(),
        _tmp: tmp,
    }
}

fn runs(marker: &std::path::Path) -> usize {
    std::fs::read_to_string(marker)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

async fn rupu(args: &[&str]) {
    let mut argv = vec!["rupu".to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let _exit = rupu_cli::run(argv).await;
}

/// Launch the workflow: `fanout` parks both gates in one wave.
async fn park_both(fx: &Fixture, run_id: &str) -> rupu_orchestrator::RunStore {
    std::env::set_var("RUPU_HOME", &fx.global);
    std::env::set_current_dir(&fx.project).unwrap();
    rupu(&[
        "workflow",
        "run",
        "two-gated-paths",
        "--input",
        &format!("marker_a={}", fx.marker_a.display()),
        "--input",
        &format!("marker_b={}", fx.marker_b.display()),
        "--run-id",
        run_id,
        "--plain",
    ])
    .await;
    let store = rupu_orchestrator::RunStore::new(fx.global.join("runs"));
    let record = store.load(run_id).expect("run record");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval
    );
    assert_eq!(record.awaiting.len(), 2, "both gates park together");
    store
}

#[tokio::test(flavor = "multi_thread")]
async fn approve_gate_runs_only_its_path_then_the_sibling_is_still_approvable() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture();
    let run_id = "run_two_gates_approve_both";
    let store = park_both(&fx, run_id).await;

    rupu(&["workflow", "approve", run_id, "--gate", "gate_a"]).await;
    assert_eq!(runs(&fx.marker_a), 1, "gate_a's path ran");
    assert_eq!(
        runs(&fx.marker_b),
        0,
        "gate_b's path still waits for gate_b"
    );
    let record = store.load(run_id).unwrap();
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval
    );
    let parked: Vec<&str> = record.awaiting.iter().map(|g| g.step_id.as_str()).collect();
    assert_eq!(parked, ["gate_b"]);

    rupu(&["workflow", "approve", run_id, "--gate", "gate_b"]).await;
    assert_eq!(runs(&fx.marker_a), 1, "gate_a's path never runs twice");
    assert_eq!(runs(&fx.marker_b), 1);
    let record = store.load(run_id).unwrap();
    assert_eq!(record.status, rupu_orchestrator::RunStatus::Completed);
    assert!(record.awaiting.is_empty() && record.gate_decisions.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn reject_gate_prunes_only_its_path_and_the_approved_path_stands() {
    let _guard = ENV_LOCK.lock().await;
    let fx = fixture();
    let run_id = "run_two_gates_approve_reject";
    let store = park_both(&fx, run_id).await;

    rupu(&["workflow", "approve", run_id, "--gate", "gate_a"]).await;
    rupu(&[
        "workflow",
        "reject",
        run_id,
        "--gate",
        "gate_b",
        "--reason",
        "not this one",
    ])
    .await;

    assert_eq!(runs(&fx.marker_a), 1);
    assert_eq!(runs(&fx.marker_b), 0, "a rejected gate's path never runs");
    let record = store.load(run_id).unwrap();
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Completed,
        "gate_a's completed path stands; gate_b's rejection is path-scoped"
    );
    assert!(record.awaiting.is_empty() && record.gate_decisions.is_empty());
}
