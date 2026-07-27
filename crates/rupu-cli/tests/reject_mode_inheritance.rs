//! ISSUES.md I-24: `on_reject` cleanup must inherit the run's launch
//! `--mode`, not silently fall back to `ask` (which permits Write tools).
//!
//! `run.json` used to persist only `resume_mode` (set by the web-resume
//! path); a run's own `--mode` was never recorded anywhere. That meant
//! `rebuild_opts_from_disk` (`crates/rupu-cli/src/resume.rs`) fell back to
//! `mode.unwrap_or("ask")` whenever a run's `on_reject` chain had to be
//! rebuilt from disk — so a run launched `--mode readonly` ran its cleanup
//! chain with writes enabled, a privilege escalation across a mode
//! boundary.
//!
//! This test drives the real CLI end to end (`rupu_cli::run(...)`, model
//! after `policy_lock.rs`): launch a gate workflow `--mode readonly`,
//! reject the parked gate, and assert the `on_reject` chain's `write_file`
//! call did NOT land on disk. A config-value assertion would not prove the
//! mode reached the tool layer — the filesystem effect is the only
//! assertion that binds.

use assert_fs::prelude::*;

/// This file's tests mutate process-global state (`RUPU_HOME`, cwd,
/// `RUPU_MOCK_PROVIDER_SCRIPT`); each `tests/*.rs` file is its own test
/// binary/process, but within THIS file multiple `#[tokio::test]`s can
/// still run concurrently on separate threads, so they must serialize on
/// the same lock. (`crate::test_support::ENV_LOCK` lives behind
/// `rupu-cli`'s `#[cfg(test)]`, which is not compiled into the rlib an
/// integration test links against — mirrors `policy_lock.rs`'s own
/// module-local lock, not a second ad hoc mechanism.)
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One turn: the agent immediately attempts `write_file`, then ends its
/// turn. Under `ask`/`bypass` the file lands; under `readonly` the
/// `write_file` call is denied (`ReadonlyDecider`) and the step still
/// completes (the tool call surfaces as denied, not as a hard failure).
const WRITE_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "write_file", "tool_input": {"path": "escaped.txt", "content": "should not land"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "done", "stop": "end_turn" } }
]
"#;

const WRITER_AGENT: &str =
    "---\nname: writer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 2\ntools: [write_file]\n---\nyou write files.";

/// A single gate with nothing before it — parking happens with no agent
/// call at all, so the mock script only has to cover the `on_reject`
/// chain's one step.
const WORKFLOW_GATE_REJECT: &str = r#"
name: gate-reject-mode
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      on_reject:
        - id: cleanup
          agent: writer
          prompt: "cleanup after reject: {{ steps.gate.decision }}"
"#;

/// Set up `<tmp>/.rupu` (global, with the `writer` agent) +
/// `<tmp>/proj/.rupu/workflows/gate-reject-mode.yaml` (project), and
/// return `(tmp, project_dir)`.
fn fixture() -> (assert_fs::TempDir, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/writer.md")
        .write_str(WRITER_AGENT)
        .unwrap();

    let project = tmp.child("proj");
    project.create_dir_all().unwrap();
    project.child(".rupu/workflows").create_dir_all().unwrap();
    project
        .child(".rupu/workflows/gate-reject-mode.yaml")
        .write_str(WORKFLOW_GATE_REJECT)
        .unwrap();

    let project_path = project.path().to_path_buf();
    (tmp, project_path)
}

/// ISSUES.md I-24: a run launched `--mode readonly` must not have its
/// `on_reject` cleanup execute with write tools enabled.
///
/// Pre-fix behavior: `rebuild_opts_from_disk` falls back to
/// `mode.unwrap_or("ask")` (nothing else persists the launch mode), so the
/// cleanup step's `write_file` call is permitted and `escaped.txt` is
/// created.
#[tokio::test(flavor = "multi_thread")]
async fn reject_cleanup_inherits_a_readonly_run_mode() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project) = fixture();
    let global_home = tmp.child(".rupu").path().to_path_buf();

    std::env::set_var("RUPU_HOME", &global_home);
    std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", WRITE_SCRIPT);
    let restore_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project).unwrap();

    // 1. Launch under --mode readonly; the workflow's only step is the
    //    gate, so this call parks immediately (AwaitingApproval) without
    //    ever making a provider call.
    let run_id = "run_i24_reject_mode_test".to_string();
    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        "gate-reject-mode".into(),
        "--mode".into(),
        "readonly".into(),
        "--run-id".into(),
        run_id.clone(),
        "--plain".into(),
    ])
    .await;

    // 2. Reject the parked gate. `rupu workflow reject` takes no --mode
    //    flag (there is no explicit per-command override to fall back
    //    from), so this exercises the record.permission_mode fallback
    //    directly.
    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "reject".into(),
        run_id.clone(),
    ])
    .await;

    std::env::set_current_dir(&restore_cwd).unwrap();
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::remove_var("RUPU_HOME");

    // 3. The binding assertion: the on_reject cleanup step's write_file
    //    call must have been DENIED — the file it would have created does
    //    not exist.
    assert!(
        !project.join("escaped.txt").exists(),
        "on_reject cleanup ran with writes enabled on a --mode readonly run: \
         escaped.txt was created despite the run's launch mode"
    );

    // The run itself must still have finalized as Rejected — the mode fix
    // must not change the terminal outcome, only what the cleanup chain is
    // permitted to do.
    let store = rupu_orchestrator::RunStore::new(global_home.join("runs"));
    let record = store.load(&run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Rejected,
        "run must still end Rejected regardless of the cleanup chain's tool permissions"
    );
}
