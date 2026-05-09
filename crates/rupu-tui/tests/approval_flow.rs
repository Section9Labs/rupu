use chrono::Utc;
use rupu_orchestrator::{RunRecord, RunStatus, RunStore};
use rupu_tui::control::approval::{approve_focused, ApprovalOutcome};
use tempfile::tempdir;

fn paused_record(dir: &std::path::Path) -> RunRecord {
    RunRecord {
        id: "run_t".into(),
        workflow_name: "wf".into(),
        status: RunStatus::AwaitingApproval,
        inputs: Default::default(),
        event: None,
        workspace_id: "ws".into(),
        workspace_path: dir.to_path_buf(),
        transcript_dir: dir.join("transcripts"),
        started_at: Utc::now(),
        finished_at: None,
        error_message: None,
        awaiting_step_id: Some("deploy".into()),
        approval_prompt: Some("ok?".into()),
        awaiting_since: Some(Utc::now()),
        expires_at: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
    }
}

#[test]
fn approve_focused_flips_status_to_running() {
    let runs = tempdir().unwrap();
    let store = RunStore::new(runs.path().to_path_buf());
    let rec = paused_record(runs.path());
    let yaml = "name: x\nsteps:\n  - id: deploy\n    agent: a\n    actions: []\n    prompt: hi\n";
    store.create(rec.clone(), yaml).unwrap();

    let outcome = approve_focused(&store, &rec.id, "matt").unwrap();
    assert!(matches!(outcome, ApprovalOutcome::Approved { .. }));
    let reloaded = store.load(&rec.id).unwrap();
    assert_eq!(reloaded.status, RunStatus::Running);
}
