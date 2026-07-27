//! ISSUES.md I-25: `rupu workflow runs` is a *listing* command and must
//! have no external side effects.
//!
//! Pre-fix, the runs-listing loop (`crates/rupu-cli/src/cmd/workflow.rs`,
//! the `runs()` fn's lazy-expiry loop) called `store.expire_if_overdue(...)`
//! for every `AwaitingApproval` row and, on the `TimeoutAction::Reject` arm,
//! went on to inline-run `build_reject_cleanup_opts` +
//! `run_reject_cleanup` — so merely *listing* runs could post comments,
//! open PRs, or execute agent steps against external systems for any gate
//! whose `timeout_seconds` had elapsed with `on_timeout: reject`.
//!
//! The verified fix shape (see the code comment at the changed call site):
//! for `on_timeout: reject` the listing must not call `expire_if_overdue`
//! at all, leaving the run `AwaitingApproval` so the `cp serve` gate
//! sweep's `ExpireThenCleanupReject` arm (`crates/rupu-cli/src/cmd/cp.rs`)
//! is the only thing that ever expires it AND runs its `on_reject` chain.
//! Naively deleting only the cleanup call while leaving the expiry call in
//! place would still flip the record to `Rejected`, and because
//! `sweep_decision` only produces `ExpireThenCleanupReject` for a run
//! still `AwaitingApproval` (falling through to `Skip` for every other
//! status), that would silently drop the cleanup chain forever — a worse
//! bug than the one being fixed. `on_timeout: approve` / `fail` / unset are
//! unaffected: lazy expiry for those stays exactly as it was.
//!
//! These tests drive the real CLI end to end: `rupu_cli::run(...)`
//! in-process (model after `reject_mode_inheritance.rs` / `policy_lock.rs`)
//! to set up + launch + force the gate overdue, then a real spawned `rupu`
//! binary (`assert_cmd`) for the listing invocation itself, so its stdout
//! can be asserted on directly instead of guessing at what the in-process
//! call printed.

use assert_cmd::Command as AssertCommand;
use assert_fs::prelude::*;

/// This file's setup mutates process-global state (`RUPU_HOME`, cwd); each
/// `tests/*.rs` file is its own test binary/process, but within THIS file
/// multiple `#[tokio::test]`s can still run concurrently on separate
/// threads, so they serialize on this module-local lock. (Mirrors
/// `policy_lock.rs` / `reject_mode_inheritance.rs` — `crate::test_support::
/// ENV_LOCK` lives behind rupu-cli's `#[cfg(test)]`, which isn't compiled
/// into the rlib an integration test binary links against, so each
/// integration-test file that needs this owns its own static lock rather
/// than sharing a second ad hoc mechanism.)
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The `on_reject` cleanup step's only turn: write a marker file, then end
/// the turn. If this fix regresses and the listing runs the chain again,
/// this script executing is what creates `reject_cleanup_marker.txt`.
const WRITE_SCRIPT: &str = r#"
[
  { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "write_file", "tool_input": {"path": "reject_cleanup_marker.txt", "content": "cleanup ran"}, "stop": "tool_use" } },
  { "AssistantText": { "text": "done", "stop": "end_turn" } }
]
"#;

const WRITER_AGENT: &str =
    "---\nname: writer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 2\ntools: [write_file]\n---\nyou write files.";

/// A standalone gate node (no agent step before it, so parking happens
/// with no provider call at all) whose `on_reject` chain would create the
/// marker file above.
const WORKFLOW_GATE_REJECT: &str = r#"
name: gate-runs-reject
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      timeout_seconds: 3600
      on_timeout: reject
      on_reject:
        - id: cleanup
          agent: writer
          prompt: "cleanup after reject: {{ steps.gate.decision }}"
"#;

/// Same shape, but `on_timeout: fail` and no `on_reject` chain — the
/// harmless case the fix must NOT break: the listing's lazy expiry still
/// finalizes this one.
const WORKFLOW_GATE_FAIL: &str = r#"
name: gate-runs-fail
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      timeout_seconds: 3600
      on_timeout: fail
"#;

/// Set up `<tmp>/.rupu` (global, with the `writer` agent) +
/// `<tmp>/proj/.rupu/workflows/*.yaml` (project, both gate workflows), and
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
        .child(".rupu/workflows/gate-runs-reject.yaml")
        .write_str(WORKFLOW_GATE_REJECT)
        .unwrap();
    project
        .child(".rupu/workflows/gate-runs-fail.yaml")
        .write_str(WORKFLOW_GATE_FAIL)
        .unwrap();

    let project_path = project.path().to_path_buf();
    (tmp, project_path)
}

/// Launch `workflow_name` under `run_id`, which parks immediately at its
/// lone gate step (no preceding agent call — no provider call is made),
/// then force the parked gate overdue by rewriting the persisted record's
/// `expires_at` into the past. This is the same technique
/// `gate_sweep_smoke.rs` uses to make a gate "overdue" deterministically,
/// without a real sleep tied to `timeout_seconds`.
///
/// Caller must hold `ENV_LOCK` before calling this (it mutates `RUPU_HOME`
/// and cwd, restoring both before returning).
async fn park_and_force_overdue(
    global_home: &std::path::Path,
    project: &std::path::Path,
    workflow_name: &str,
    run_id: &str,
) {
    std::env::set_var("RUPU_HOME", global_home);
    let restore_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project).unwrap();

    let _exit = rupu_cli::run(vec![
        "rupu".into(),
        "workflow".into(),
        "run".into(),
        workflow_name.into(),
        "--run-id".into(),
        run_id.into(),
        "--plain".into(),
    ])
    .await;

    std::env::set_current_dir(&restore_cwd).unwrap();
    std::env::remove_var("RUPU_HOME");

    let store = rupu_orchestrator::RunStore::new(global_home.join("runs"));
    let mut record = store
        .load(run_id)
        .expect("run record must exist after launch");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval,
        "the lone gate step must have parked the run before this test can force it overdue"
    );
    record.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(10));
    store
        .update(&record)
        .expect("force the parked gate's expires_at into the past");
}

/// ISSUES.md I-25, assertion 1: listing an overdue `on_timeout: reject`
/// gate must not execute its `on_reject` cleanup chain.
///
/// Pre-fix behavior: the listing's `TimeoutAction::Reject` arm ran the
/// chain inline, creating `reject_cleanup_marker.txt`.
#[tokio::test(flavor = "multi_thread")]
async fn listing_runs_does_not_execute_a_reject_cleanup_chain() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project) = fixture();
    let global_home = tmp.child(".rupu").path().to_path_buf();
    let run_id = "run_i25_reject_marker_test".to_string();

    park_and_force_overdue(&global_home, &project, "gate-runs-reject", &run_id).await;

    // The listing itself, run as a real separate `rupu` invocation so its
    // stdout can be asserted on directly: it must still print the run (a
    // read command reporting nothing would be its own kind of bug), just
    // without running anything.
    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &global_home)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", WRITE_SCRIPT)
        .current_dir(&project)
        .args(["--format", "json", "workflow", "runs"])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "\"run_id\": \"{run_id}\""
        )));

    assert!(
        !project.join("reject_cleanup_marker.txt").exists(),
        "rupu workflow runs must not execute an on_reject cleanup chain as a side effect of \
         listing runs — the marker file exists, meaning the chain ran"
    );
}

/// ISSUES.md I-25, assertion 2 (the one a naive fix would fail): after
/// listing, the reject-timeout gate must still be `AwaitingApproval`, not
/// `Rejected`. A fix that simply deleted the cleanup call while still
/// calling `expire_if_overdue` would finalize the run `Rejected` here —
/// and because `sweep_decision` (`crates/rupu-cli/src/cmd/cp.rs`) only
/// produces `ExpireThenCleanupReject` for a run still `AwaitingApproval`,
/// the `cp serve` gate sweep would then classify this run `Skip` on every
/// later tick, silently dropping the `on_reject` chain forever.
#[tokio::test(flavor = "multi_thread")]
async fn listing_runs_leaves_a_reject_timeout_gate_awaiting_approval() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project) = fixture();
    let global_home = tmp.child(".rupu").path().to_path_buf();
    let run_id = "run_i25_reject_status_test".to_string();

    park_and_force_overdue(&global_home, &project, "gate-runs-reject", &run_id).await;

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &global_home)
        .env("RUPU_MOCK_PROVIDER_SCRIPT", WRITE_SCRIPT)
        .current_dir(&project)
        .args(["workflow", "runs"])
        .assert()
        .success();

    let store = rupu_orchestrator::RunStore::new(global_home.join("runs"));
    let record = store.load(&run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval,
        "a reject-timeout gate must stay AwaitingApproval after listing so the cp-serve gate \
         sweep can still expire it AND run its on_reject chain together"
    );
}

/// ISSUES.md I-25, assertion 3: the harmless case must keep working. An
/// `on_timeout: fail` gate has no chain to run, so the listing's lazy
/// expiry must still finalize it exactly as before this fix.
#[tokio::test(flavor = "multi_thread")]
async fn listing_runs_still_finalizes_a_fail_timeout_gate() {
    let _guard = ENV_LOCK.lock().await;

    let (tmp, project) = fixture();
    let global_home = tmp.child(".rupu").path().to_path_buf();
    let run_id = "run_i25_fail_timeout_test".to_string();

    park_and_force_overdue(&global_home, &project, "gate-runs-fail", &run_id).await;

    AssertCommand::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &global_home)
        .current_dir(&project)
        .args(["--format", "json", "workflow", "runs"])
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "\"run_id\": \"{run_id}\""
        )));

    let store = rupu_orchestrator::RunStore::new(global_home.join("runs"));
    let record = store.load(&run_id).expect("run record must exist");
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::Failed,
        "an on_timeout: fail gate has no chain to run, so the listing's lazy expiry must still \
         finalize it"
    );
    assert!(record.finished_at.is_some());
}
