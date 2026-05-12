//! RunModel::apply is a pure function — test each Event variant.

use rupu_app::run_model::RunModel;
use rupu_app_canvas::NodeStatus;
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStatus, StepKind};

fn fixture(run_id: &str) -> RunModel {
    RunModel::new(run_id.into(), "wf.yaml".into())
}

#[test]
fn run_started_marks_run_running() {
    let model = fixture("r1");
    let model = model.apply(&Event::RunStarted {
        event_version: 1,
        run_id: "r1".into(),
        workflow_path: "wf.yaml".into(),
        started_at: chrono::Utc::now(),
    });
    assert_eq!(model.run_status, RunStatus::Running);
}

#[test]
fn step_started_flips_node_to_active() {
    let model = fixture("r1").apply(&Event::StepStarted {
        run_id: "r1".into(),
        step_id: "s1".into(),
        kind: StepKind::Linear,
        agent: None,
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Active));
    assert_eq!(model.active_step.as_deref(), Some("s1"));
}

#[test]
fn step_working_flips_node_to_working() {
    let model = fixture("r1")
        .apply(&Event::StepStarted {
            run_id: "r1".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: None,
        })
        .apply(&Event::StepWorking {
            run_id: "r1".into(),
            step_id: "s1".into(),
            note: Some("gh_pr_list".into()),
        });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Working));
}

#[test]
fn step_completed_flips_node_to_complete() {
    let model = fixture("r1")
        .apply(&Event::StepStarted {
            run_id: "r1".into(),
            step_id: "s1".into(),
            kind: StepKind::Linear,
            agent: None,
        })
        .apply(&Event::StepCompleted {
            run_id: "r1".into(),
            step_id: "s1".into(),
            success: true,
            duration_ms: 42,
        });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Complete));
}

#[test]
fn step_awaiting_approval_flips_node_and_focus() {
    let model = fixture("r1").apply(&Event::StepAwaitingApproval {
        run_id: "r1".into(),
        step_id: "s1".into(),
        reason: "ok?".into(),
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Awaiting));
    assert_eq!(model.focused_step.as_deref(), Some("s1"));
}

#[test]
fn step_failed_flips_node_to_failed() {
    let model = fixture("r1").apply(&Event::StepFailed {
        run_id: "r1".into(),
        step_id: "s1".into(),
        error: "boom".into(),
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Failed));
}

#[test]
fn step_skipped_flips_node_to_skipped() {
    let model = fixture("r1").apply(&Event::StepSkipped {
        run_id: "r1".into(),
        step_id: "s1".into(),
        reason: "when:false".into(),
    });
    assert_eq!(model.nodes.get("s1"), Some(&NodeStatus::Skipped));
}

#[test]
fn run_completed_finalizes_status() {
    let model = fixture("r1").apply(&Event::RunCompleted {
        run_id: "r1".into(),
        status: RunStatus::Completed,
        finished_at: chrono::Utc::now(),
    });
    assert_eq!(model.run_status, RunStatus::Completed);
    assert!(model.active_step.is_none());
}
