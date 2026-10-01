//! Resume after an approval gate must rebuild the run's full
//! `OrchestratorRunOpts`, not silently default the fields a fresh run wires.
//!
//! `rebuild_opts_from_disk` (`crates/rupu-cli/src/resume.rs`) used to build
//! the resumed opts with `run_step: Default::default()` (a `RunStepPolicy`
//! with `run_step_enabled = false`) and `pause: None`. Both are regressions
//! against the fresh-run path (`run_step_policy_for(...)` + a live pause
//! token), and both surface on the ordinary `rupu workflow approve` path:
//!
//! 1. A `run:` step after the gate is refused with `ConfigDisabled` even
//!    when `[workflow] run_step_enabled = true`, because the rebuilt policy
//!    carried the default (disabled) config.
//! 2. The resumed (possibly detached) run no longer honors a cooperative
//!    pause, because the token + marker poller were never rebuilt.
//!
//! Both tests drive the real CLI end to end (`rupu_cli::run(...)`) and assert
//! on binding effects — a file the `run:` step would create, and the run's
//! terminal status — not on reconstructed opts values.

use assert_fs::prelude::*;

/// These tests mutate process-global state (`RUPU_HOME`, cwd); each
/// `tests/*.rs` file is its own test binary, but within THIS file multiple
/// `#[tokio::test]`s can run concurrently on separate threads, so they
/// serialize on one lock (mirrors `reject_mode_inheritance.rs`).
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A single gate, then a deterministic `run:` step that writes a marker
/// file. Parking happens at the gate with no agent call at all; the `run:`
/// step only executes once the gate is approved and the run resumes.
const WORKFLOW_GATE_THEN_RUN: &str = r#"
name: gate-then-run
steps:
  - id: gate
    approval:
      prompt: "Approve the build?"
  - id: build
    run:
      cmd: sh
      args: ["-c", "echo ran > {{ inputs.marker }}"]
"#;

/// `[workflow] run_step_enabled = true` in the GLOBAL config — exactly the
/// opt-in a real user sets to permit `run:` steps. The pre-fix rebuild
/// discarded this and fell back to the disabled default.
const GLOBAL_CONFIG_RUN_STEP_ENABLED: &str = "[workflow]\nrun_step_enabled = true\n";

/// Set up `<tmp>/.rupu` (global, with run_step_enabled) +
/// `<tmp>/proj/.rupu/workflows/gate-then-run.yaml` (project), and return
/// `(tmp, project_dir, global_home)`.
fn fixture() -> (assert_fs::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global
        .child("config.toml")
        .write_str(GLOBAL_CONFIG_RUN_STEP_ENABLED)
        .unwrap();

    let project = tmp.child("proj");
    project.create_dir_all().unwrap();
    project.child(".rupu/workflows").create_dir_all().unwrap();
    project
        .child(".rupu/workflows/gate-then-run.yaml")
        .write_str(WORKFLOW_GATE_THEN_RUN)
        .unwrap();

    let project_path = project.path().to_path_buf();
    let global_home = global.path().to_path_buf();
    (tmp, project_path, global_home)
}

/// Consequence 1: `rupu workflow approve` on a `gate → run:` workflow must
/// execute the `run:` step and complete.
///
/// Pre-fix: `rebuild_opts_from_disk` rebuilt `run_step` from
/// `Default::default()` (disabled), so the `run:` step was refused with
/// `ConfigDisabled`, the step failed, the marker file was never written, and
/// the run finalized `Failed`.
#[tokio::test(flavor = "multi_thread")]
async fn approve_then_run_step_executes_and_run_completes() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project, global_home) = fixture();
    let marker = tmp.path().join("ran.txt");
    let run_id = "run_approve_run_step_test".to_string();

    std::env::set_var("RUPU_HOME", &global_home);
    let restore_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project).unwrap();

    // 1. Launch (default mode). The first step is the gate, so this parks
    //    immediately (AwaitingApproval) without a provider call.
    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "gate-then-run".into(),
        "--input".into(),
        format!("marker={}", marker.display()),
        "--run-id".into(),
        run_id.clone(),
        "--plain".into(),
    ])
    .await;

    // 2. Approve the parked gate — this is the path that rebuilds opts from
    //    disk and resumes into the `run:` step.
    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "approve".into(),
        run_id.clone(),
    ])
    .await;

    std::env::set_current_dir(&restore_cwd).unwrap();
    std::env::remove_var("RUPU_HOME");

    // Binding assertion: the `run:` step actually executed.
    assert!(
        marker.exists(),
        "the run: step after the gate did not execute on resume — its marker \
         file was not created (rebuilt run_step policy still carried the \
         disabled default)"
    );

    let store = rupu_orchestrator::RunStore::new(global_home.join("runs"));
    let record = store.load(&run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Completed,
        "the gate→run: workflow must complete after approve; got {:?}",
        record.status
    );
}

/// Consequence 2: a resumed run must still honor a cooperative pause.
///
/// A pause requested against the parked run (its marker on disk) must stop
/// the resumed run at the next step boundary — before the `run:` step —
/// leaving the marker file uncreated and the run `Paused`.
///
/// Pre-fix: `rebuild_opts_from_disk` set `pause: None`, so the resumed run
/// carried no pause token at all; the marker was ignored, the `run:` step
/// ran, and the run finalized `Completed`.
#[tokio::test(flavor = "multi_thread")]
async fn approve_resumed_run_honors_a_pause_requested_on_the_parked_run() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project, global_home) = fixture();
    let marker = tmp.path().join("ran.txt");
    let run_id = "run_approve_pause_test".to_string();

    std::env::set_var("RUPU_HOME", &global_home);
    let restore_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project).unwrap();

    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "gate-then-run".into(),
        "--input".into(),
        format!("marker={}", marker.display()),
        "--run-id".into(),
        run_id.clone(),
        "--plain".into(),
    ])
    .await;

    // A pause was requested while the run was parked at the gate.
    let store = rupu_orchestrator::RunStore::new(global_home.join("runs"));
    store
        .set_pause_marker(&run_id)
        .expect("set pause marker on the parked run");

    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "approve".into(),
        run_id.clone(),
    ])
    .await;

    std::env::set_current_dir(&restore_cwd).unwrap();
    std::env::remove_var("RUPU_HOME");

    // Binding assertion: the resumed run honored the pause — it stopped at
    // the step boundary before the `run:` step, so the marker never landed.
    assert!(
        !marker.exists(),
        "the resumed run ran its post-gate run: step despite a pending pause \
         marker — the rebuilt opts carried no pause token"
    );

    let record = store.load(&run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Paused,
        "a resumed run with a pending pause marker must finalize Paused; got {:?}",
        record.status
    );
}
