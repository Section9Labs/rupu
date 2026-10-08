//! `RunStore::pause` refuses a standalone agent run (`rupu run <agent>`,
//! recorded `agent:<name>`): its runner has no pause boundary and never
//! polls the pause marker, so flipping the record to `Paused` would lie
//! while the agent keeps running.

use chrono::Utc;
use rupu_orchestrator::runs::PauseError;
use rupu_orchestrator::{RunRecord, RunStatus, RunStore, AGENT_RUN_PREFIX};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

fn record(id: &str, workflow_name: &str, status: RunStatus) -> RunRecord {
    RunRecord {
        customer: None,
        id: id.into(),
        workflow_name: workflow_name.into(),
        status,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_1".into(),
        workspace_path: PathBuf::from("/tmp/proj"),
        transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at: Utc::now(),
        finished_at: None,
        error_message: None,
        awaiting: Vec::new(),
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
        resume_gate_id: None,
        resume_approver: None,
        resume_rerequested_at: None,
        reject_cleanup_pending: None,
        engagement_profiles: Vec::new(),
        permission_mode: None,
        final_output: None,
        loop_progress: BTreeMap::new(),
        gate_decisions: Vec::new(),
        system_prompt_suffix: None,
        codename: None,
        cause: None,
    }
}

#[test]
fn pause_refuses_a_running_standalone_agent_run() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().to_path_buf());
    let name = format!("{AGENT_RUN_PREFIX}reviewer");
    store
        .create(record("run_agent_live", &name, RunStatus::Running), "")
        .unwrap();

    let err = store.pause("run_agent_live", Utc::now()).unwrap_err();
    assert!(
        matches!(&err, PauseError::NotPausable(n) if n == &name),
        "{err:?}"
    );
    assert!(err.to_string().contains("only workflow runs can be paused"));
    // The record is untouched: still Running, no marker written.
    let reloaded = store.load("run_agent_live").unwrap();
    assert_eq!(reloaded.status, RunStatus::Running);
    assert!(reloaded.awaiting_since.is_none());
    assert!(!store.pause_marker_exists("run_agent_live"));
}

#[test]
fn pause_still_pauses_a_running_workflow_run() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().to_path_buf());
    store
        .create(
            record("run_wf_live", "nightly", RunStatus::Running),
            "name: nightly\nsteps: []\n",
        )
        .unwrap();

    store.pause("run_wf_live", Utc::now()).unwrap();
    assert_eq!(store.load("run_wf_live").unwrap().status, RunStatus::Paused);
}
