//! Integration tests: InProcessExecutor wraps the linear runner and
//! exposes start / list_runs / tail / approve / cancel.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::executor::{
    Event, InProcessExecutor, RunFilter, WorkflowExecutor, WorkflowRunOpts,
};
use rupu_orchestrator::runner::StepFactory;
use rupu_orchestrator::RunStatus;
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use tempfile::TempDir;

struct FakeFactory;

#[async_trait]
impl StepFactory for FakeFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "echo".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id,
            workspace_id,
            workspace_path,
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: ToolContext::default(),
            user_message: rendered_prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: false,
            suppress_stream_stdout: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: step_id.to_string(),
            on_tool_call,
        }
    }
}

fn make_executor(tmp: &TempDir) -> InProcessExecutor {
    let runs_dir = tmp.path().join("runs");
    let store = Arc::new(rupu_orchestrator::RunStore::new(runs_dir));
    InProcessExecutor::new(
        store,
        "ws_test".into(),
        tmp.path().to_path_buf(),
        tmp.path().join("transcripts"),
    )
}

fn fake_factory() -> Arc<FakeFactory> {
    Arc::new(FakeFactory)
}

const WF_TWO_STEPS: &str = r#"
name: two-step
steps:
  - id: alpha
    agent: ag
    actions: []
    prompt: "hello alpha"
  - id: beta
    agent: ag
    actions: []
    prompt: "hello beta ({{ steps.alpha.output }})"
"#;

const WF_APPROVAL_STEP: &str = r#"
name: needs-approval
steps:
  - id: guarded
    agent: ag
    actions: []
    prompt: "do something important"
    approval:
      required: true
      prompt: "approve the important step?"
  - id: after
    agent: ag
    actions: []
    prompt: "step after approval"
"#;

#[tokio::test]
async fn start_then_tail_yields_events_in_order() {
    let tmp = TempDir::new().unwrap();

    // Write workflow to disk.
    let wf_path = tmp.path().join("two-step.yaml");
    std::fs::write(&wf_path, WF_TWO_STEPS).unwrap();

    let exec = make_executor(&tmp);

    let handle = exec
        .start(
            WorkflowRunOpts {
                workflow_path: wf_path,
                vars: Default::default(),
            },
            fake_factory(),
        )
        .await
        .expect("start");

    let mut stream = exec.tail(&handle.run_id).expect("tail");

    // Drain until RunCompleted (or RunFailed).
    let mut events: Vec<Event> = Vec::new();
    while let Some(ev) = stream.next().await {
        let done = matches!(ev, Event::RunCompleted { .. } | Event::RunFailed { .. });
        events.push(ev);
        if done {
            break;
        }
    }

    assert!(
        !events.is_empty(),
        "should have received at least one event"
    );
    assert!(
        matches!(events.first(), Some(Event::RunStarted { .. })),
        "first event must be RunStarted, got {:?}",
        events.first()
    );
    assert!(
        matches!(events.last(), Some(Event::RunCompleted { .. })),
        "last event must be RunCompleted, got {:?}",
        events.last()
    );

    // Verify events.jsonl was written.
    let runs_dir = tmp.path().join("runs");
    let run_dir = runs_dir.join(&handle.run_id);
    assert!(
        run_dir.join("events.jsonl").exists(),
        "events.jsonl must be created in the run directory"
    );
}

#[tokio::test]
async fn list_runs_returns_active_for_in_flight() {
    let tmp = TempDir::new().unwrap();

    let wf_path = tmp.path().join("two-step.yaml");
    std::fs::write(&wf_path, WF_TWO_STEPS).unwrap();

    let exec = make_executor(&tmp);

    let handle = exec
        .start(
            WorkflowRunOpts {
                workflow_path: wf_path,
                vars: Default::default(),
            },
            fake_factory(),
        )
        .await
        .expect("start");

    // The run is in-flight immediately after start; list Active.
    // (It may have already completed by the time we check if the
    // runtime is very fast, so we just verify the run id appears
    // at some point in the union of active + completed records.)
    let all = exec.list_runs(RunFilter::All);
    let ids: Vec<_> = all.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains(&handle.run_id.as_str()),
        "run_id must appear in list_runs(All)"
    );

    // Wait for completion by draining the event stream.
    let mut stream = exec.tail(&handle.run_id).expect("tail");
    while let Some(ev) = stream.next().await {
        if matches!(ev, Event::RunCompleted { .. } | Event::RunFailed { .. }) {
            break;
        }
    }

    // Give the spawned task a moment to persist terminal status.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // After completion the run should no longer be Active.
    let active = exec.list_runs(RunFilter::Active);
    let active_ids: Vec<_> = active.iter().map(|r| r.id.as_str()).collect();
    assert!(
        !active_ids.contains(&handle.run_id.as_str()),
        "completed run must not appear in list_runs(Active)"
    );
}

#[tokio::test]
async fn approve_unsticks_an_awaiting_step() {
    let tmp = TempDir::new().unwrap();

    let wf_path = tmp.path().join("needs-approval.yaml");
    std::fs::write(&wf_path, WF_APPROVAL_STEP).unwrap();

    let exec = make_executor(&tmp);

    let handle = exec
        .start(
            WorkflowRunOpts {
                workflow_path: wf_path,
                vars: Default::default(),
            },
            fake_factory(),
        )
        .await
        .expect("start");

    // Drain until StepAwaitingApproval (or RunCompleted if something
    // unexpected happened).
    let mut stream = exec.tail(&handle.run_id).expect("tail");
    let mut got_awaiting = false;
    while let Some(ev) = stream.next().await {
        if matches!(ev, Event::StepAwaitingApproval { .. }) {
            got_awaiting = true;
            break;
        }
        if matches!(ev, Event::RunCompleted { .. } | Event::RunFailed { .. }) {
            break;
        }
    }
    drop(stream); // release the broadcast receiver

    assert!(
        got_awaiting,
        "expected StepAwaitingApproval event from approval-gated workflow"
    );

    // Give the runner task a moment to persist AwaitingApproval status.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // approve() should succeed: the run is in AwaitingApproval.
    exec.approve(&handle.run_id, "test-operator")
        .await
        .expect("approve should succeed");

    // Verify the persisted status was flipped to Running by approve().
    let runs_dir = tmp.path().join("runs");
    let store = rupu_orchestrator::RunStore::new(runs_dir);
    let rec = store.load(&handle.run_id).expect("load record");
    assert_eq!(
        rec.status,
        RunStatus::Running,
        "approve() must flip status to Running"
    );
}
