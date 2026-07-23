//! Approval GATE NODE runtime (spec §4.1, plan 1 / task 3): auto-approve,
//! pause, approve-resume result synthesis.
//!
//! Mirrors the harness shape of `tests/pause_resume_e2e.rs` and
//! `tests/linear_runner.rs`'s `resume_from_approval_picks_up_at_awaited_step`:
//! a real disk-backed `RunStore`, `run_workflow` driven directly through its
//! public `OrchestratorRunOpts`, and a fake `StepFactory`. A gate node never
//! dispatches an agent itself, so `PanicFactory` (never called) proves that
//! for the gate-only cases; a small `EchoFactory` covers the cases with a
//! following linear step.

use async_trait::async_trait;
use rupu_agent::runner::MockProvider;
use rupu_agent::runner::{BypassDecider, ScriptedTurn, DEFAULT_MAX_TOKENS};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::executor::JsonlSink;
use rupu_orchestrator::runner::{
    run_reject_cleanup, run_workflow, OrchestratorRunOpts, ResumeState, StepFactory,
};
use rupu_orchestrator::{
    ApprovalDecision, ApprovalError, RunStatus, RunStore, StepKind, StepResult, Workflow,
};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Panics if ever asked to dispatch an agent — a gate node never does.
struct PanicFactory;
#[async_trait]
impl StepFactory for PanicFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        _rendered_prompt: String,
        _run_id: String,
        _workspace_id: String,
        _workspace_path: PathBuf,
        _transcript_path: PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        panic!("PanicFactory: build_opts_for_step must not be called — the workflow is gate-only")
    }
}

/// Echoes the rendered prompt back as the step's final assistant text.
/// Used for the (non-gate) linear step that follows a gate in tests 3/4.
#[derive(Default)]
struct EchoFactory {
    seen: Mutex<Vec<String>>,
}
#[async_trait]
impl StepFactory for EchoFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        self.seen.lock().unwrap().push(step_id.to_string());
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("done: {rendered_prompt}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: agent_name.to_string(),
            agent_system_prompt: "test".into(),
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
            no_stream: true,
            suppress_stream_stdout: true,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: String::new(),
            on_tool_call,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            context_window_tokens: None,
            compact_at_percent: None,
            scope_name: None,
            surface_tag: None,
            pause: None,
        }
    }
}

/// Always fails its agent run with a `ProviderError` — used by test 6 to
/// prove an `on_reject` cleanup step's failure doesn't derail the chain or
/// the run's terminal `Rejected` status.
struct FailFactory;
#[async_trait]
impl StepFactory for FailFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let provider = MockProvider::new(vec![ScriptedTurn::ProviderError(
            "simulated on_reject cleanup failure".into(),
        )]);
        AgentRunOpts {
            agent_name: agent_name.to_string(),
            agent_system_prompt: "test".into(),
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
            no_stream: true,
            suppress_stream_stdout: true,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: String::new(),
            on_tool_call,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            context_window_tokens: None,
            compact_at_percent: None,
            scope_name: None,
            surface_tag: None,
            pause: None,
        }
    }
}

/// Read every event line out of a (flushed) `events.jsonl` file as raw JSON
/// values, tagged by `type`, so tests can assert on the exact sequence
/// without depending on `Event`'s full field list.
fn read_event_types(path: &std::path::Path) -> Vec<String> {
    let body = std::fs::read_to_string(path).unwrap_or_default();
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// Test 1 — auto_approve truthy: completes without pausing.
// ---------------------------------------------------------------------------

const WF_GATE_AUTO: &str = r#"
name: gate-auto
steps:
  - id: gate
    approval:
      auto_approve: "true"
"#;

#[tokio::test]
async fn gate_auto_approve_completes_without_pausing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_AUTO).unwrap();

    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_auto".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_AUTO.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res = run_workflow(opts).await.expect("run completes");
    assert!(
        res.awaiting.is_none(),
        "an auto-approved gate must not pause the run"
    );
    assert_eq!(res.step_results.len(), 1);
    let gate = &res.step_results[0];
    assert_eq!(gate.step_id, "gate");
    assert_eq!(gate.kind, StepKind::ApprovalGate);
    assert!(gate.success);

    let output: serde_json::Value =
        serde_json::from_str(&gate.output).expect("gate output is JSON");
    assert_eq!(output["decision"], "approved");
    assert_eq!(output["via"], "auto");
    assert!(output["decided_at"].is_string());

    let record = store.load(&res.run_id).unwrap();
    assert_eq!(record.status, RunStatus::Completed);

    let types = read_event_types(&events_path);
    assert!(
        types.contains(&"step_started".to_string()),
        "got {types:?}"
    );
    assert!(
        types.contains(&"step_completed".to_string()),
        "got {types:?}"
    );
    assert!(
        !types.contains(&"step_awaiting_approval".to_string()),
        "an auto-approved gate must never emit step_awaiting_approval; got {types:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — auto_approve falsy/absent: parks AwaitingApproval.
// ---------------------------------------------------------------------------

const WF_GATE_MANUAL: &str = r#"
name: gate-manual
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
"#;

#[tokio::test]
async fn gate_without_auto_approve_parks_awaiting_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_MANUAL).unwrap();

    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_manual".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_MANUAL.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res = run_workflow(opts).await.expect("a pause is Ok, not Err");
    let awaiting = res.awaiting.clone().expect("gate must pause the run");
    assert_eq!(awaiting.step_id, "gate");
    assert!(awaiting.prompt.contains("Approve the deploy?"));
    assert!(
        res.step_results.is_empty(),
        "a paused gate has no completed result yet"
    );

    let record = store.load(&res.run_id).unwrap();
    assert_eq!(record.status, RunStatus::AwaitingApproval);
    assert_eq!(record.awaiting_step_id.as_deref(), Some("gate"));

    let types = read_event_types(&events_path);
    assert_eq!(
        types.last().map(String::as_str),
        Some("step_awaiting_approval"),
        "events.jsonl must end with step_awaiting_approval for the gate; got {types:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — approve + resume synthesizes the gate result, run continues.
// ---------------------------------------------------------------------------

const WF_GATE_THEN_STEP: &str = r#"
name: gate-then-step
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
  - id: after
    agent: worker
    prompt: "post-gate decision: {{ steps.gate.decision }}"
"#;

#[tokio::test]
async fn gate_approve_resume_continues_to_next_step() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_THEN_STEP).unwrap();

    // --- Phase 1: pause at the gate. ---
    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_resume".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_THEN_STEP.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let awaiting = res1.awaiting.clone().expect("must pause at the gate");
    assert_eq!(awaiting.step_id, "gate");
    let run_id = res1.run_id.clone();

    // --- Operator approves (mirrors `rupu workflow approve`): flip the
    // persisted record back to Running, clear the awaiting fields. ---
    let mut record = store.load(&run_id).unwrap();
    record.status = RunStatus::Running;
    record.awaiting_step_id = None;
    record.approval_prompt = None;
    store.update(&record).unwrap();

    // --- Phase 2: resume with the gate as the approved step. ---
    let prior_records = store.read_step_results(&run_id).unwrap();
    let prior_step_results: Vec<StepResult> =
        prior_records.iter().map(StepResult::from).collect();
    assert!(
        prior_step_results.is_empty(),
        "the gate never completed in phase 1, so no prior step results exist"
    );

    let factory2 = Arc::new(EchoFactory::default());
    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record.workspace_id.clone(),
        workspace_path: record.workspace_path.clone(),
        transcript_dir: record.transcript_dir.clone(),
        factory: factory2.clone(),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_THEN_STEP.to_string()),
        resume_from: Some(ResumeState::from_approval(
            run_id.clone(),
            prior_step_results,
            "gate".into(),
        )),
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res2 = run_workflow(opts2).await.expect("resume completes");
    assert!(res2.awaiting.is_none(), "resumed run must complete");
    assert_eq!(res2.step_results.len(), 2);

    let gate = &res2.step_results[0];
    assert_eq!(gate.step_id, "gate");
    assert_eq!(gate.kind, StepKind::ApprovalGate);
    assert!(gate.success);
    let output: serde_json::Value =
        serde_json::from_str(&gate.output).expect("gate output is JSON");
    assert_eq!(output["decision"], "approved");
    assert_eq!(output["via"], "human");

    let after = &res2.step_results[1];
    assert_eq!(after.step_id, "after");
    assert!(after.success);
    assert!(
        after.output.contains("post-gate decision: approved"),
        "the following step must see steps.gate.decision == approved; got {:?}",
        after.output
    );

    // Only the (non-gate) linear step ever went through the agent factory —
    // the gate never dispatches one.
    assert_eq!(factory2.seen.lock().unwrap().clone(), vec!["after".to_string()]);

    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Completed);
}

// ---------------------------------------------------------------------------
// Test 4 — boundary: the gate is the LAST step. Approve-resume completes
// the run with the gate result recorded.
// ---------------------------------------------------------------------------

const WF_STEP_THEN_GATE: &str = r#"
name: step-then-gate
steps:
  - id: setup
    agent: worker
    prompt: "do setup"
  - id: gate
    approval:
      prompt: "Final sign-off?"
"#;

#[tokio::test]
async fn gate_as_last_step_approve_resume_completes_run() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_STEP_THEN_GATE).unwrap();

    // --- Phase 1: setup runs, then pauses at the gate. ---
    let factory1 = Arc::new(EchoFactory::default());
    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_last".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: factory1.clone(),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_STEP_THEN_GATE.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let awaiting = res1.awaiting.clone().expect("must pause at the gate");
    assert_eq!(awaiting.step_id, "gate");
    assert_eq!(res1.step_results.len(), 1, "setup must have completed");
    assert_eq!(res1.step_results[0].step_id, "setup");
    let run_id = res1.run_id.clone();

    // --- Operator approves. ---
    let mut record = store.load(&run_id).unwrap();
    record.status = RunStatus::Running;
    record.awaiting_step_id = None;
    record.approval_prompt = None;
    store.update(&record).unwrap();

    // --- Phase 2: resume with the gate as the approved step. ---
    let prior_records = store.read_step_results(&run_id).unwrap();
    let prior_step_results: Vec<StepResult> =
        prior_records.iter().map(StepResult::from).collect();
    assert_eq!(prior_step_results.len(), 1, "setup checkpointed on disk");

    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record.workspace_id.clone(),
        workspace_path: record.workspace_path.clone(),
        transcript_dir: record.transcript_dir.clone(),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_STEP_THEN_GATE.to_string()),
        resume_from: Some(ResumeState::from_approval(
            run_id.clone(),
            prior_step_results,
            "gate".into(),
        )),
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res2 = run_workflow(opts2).await.expect("resume completes");
    assert!(
        res2.awaiting.is_none(),
        "resuming with the gate as the last step must complete the run"
    );
    assert_eq!(res2.step_results.len(), 2);
    assert_eq!(res2.step_results[0].step_id, "setup");
    let gate = &res2.step_results[1];
    assert_eq!(gate.step_id, "gate");
    assert_eq!(gate.kind, StepKind::ApprovalGate);
    assert!(gate.success);
    let output: serde_json::Value =
        serde_json::from_str(&gate.output).expect("gate output is JSON");
    assert_eq!(output["decision"], "approved");
    assert_eq!(output["via"], "human");

    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Completed);
    assert!(record_final.finished_at.is_some());
}

// ---------------------------------------------------------------------------
// Test 5 — reject with cleanup: the gate's own rejected result is recorded,
// the on_reject chain dispatches through the same step-factory machinery,
// and the run stays terminally Rejected.
// ---------------------------------------------------------------------------

const WF_GATE_REJECT: &str = r#"
name: gate-reject
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      on_reject:
        - id: notify_fail
          agent: worker
          prompt: "cleanup after reject: {{ steps.gate.decision }}"
"#;

#[tokio::test]
async fn reject_runs_on_reject_cleanup_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_REJECT).unwrap();

    // --- Phase 1: pause at the gate. ---
    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_reject".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let awaiting = res1.awaiting.clone().expect("must pause at the gate");
    assert_eq!(awaiting.step_id, "gate");
    let run_id = res1.run_id.clone();

    // --- Operator rejects (mirrors `rupu workflow reject`): the library
    // call finalizes the run BEFORE any cleanup runs. ---
    let decision = store
        .reject(&run_id, "operator", "not today", chrono::Utc::now())
        .expect("reject succeeds");
    let (rejected_step_id, reason) = match decision {
        ApprovalDecision::Rejected {
            step_id, reason, ..
        } => (step_id, reason),
        other => panic!("expected Rejected, got {other:?}"),
    };
    assert_eq!(rejected_step_id, "gate");

    let record_after_reject = store.load(&run_id).unwrap();
    assert_eq!(record_after_reject.status, RunStatus::Rejected);
    assert!(record_after_reject
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains(&reason));

    // --- Cleanup: dispatch the on_reject chain. ---
    let prior_records = store.read_step_results(&run_id).unwrap();
    let prior_step_results: Vec<StepResult> =
        prior_records.iter().map(StepResult::from).collect();
    assert!(
        prior_step_results.is_empty(),
        "the gate never completed, so no prior step results exist yet"
    );

    let factory = Arc::new(EchoFactory::default());
    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record_after_reject.workspace_id.clone(),
        workspace_path: record_after_reject.workspace_path.clone(),
        transcript_dir: record_after_reject.transcript_dir.clone(),
        factory: factory.clone(),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT.to_string()),
        resume_from: Some(ResumeState::from_rejection(
            run_id.clone(),
            prior_step_results,
            rejected_step_id.clone(),
            reason.clone(),
        )),
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human")
        .await
        .expect("cleanup never errors");

    // The on_reject step actually dispatched through the factory.
    assert_eq!(
        factory.seen.lock().unwrap().clone(),
        vec!["notify_fail".to_string()]
    );

    let records = store.read_step_results(&run_id).unwrap();
    assert_eq!(records.len(), 2, "gate + on_reject step both persisted");

    let gate_record = records
        .iter()
        .find(|r| r.step_id == "gate")
        .expect("gate result persisted");
    assert_eq!(gate_record.kind, StepKind::ApprovalGate);
    assert!(gate_record.success);
    let gate_output: serde_json::Value =
        serde_json::from_str(&gate_record.output).expect("gate output is JSON");
    assert_eq!(gate_output["decision"], "rejected");
    assert_eq!(gate_output["via"], "human");
    assert_eq!(gate_output["reason"], "not today");

    let cleanup_record = records
        .iter()
        .find(|r| r.step_id == "notify_fail")
        .expect("on_reject step result persisted");
    assert!(cleanup_record.success);
    assert!(
        cleanup_record.output.contains("cleanup after reject: rejected"),
        "on_reject step should see steps.gate.decision == rejected; got {:?}",
        cleanup_record.output
    );

    // The terminal status set by `RunStore::reject` is untouched by cleanup.
    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Rejected);
}

// ---------------------------------------------------------------------------
// Test 6 — a failing on_reject step doesn't change the terminal outcome.
// ---------------------------------------------------------------------------

const WF_GATE_REJECT_FAILING_CLEANUP: &str = r#"
name: gate-reject-failing-cleanup
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      on_reject:
        - id: notify_fail
          agent: worker
          prompt: "cleanup after reject"
"#;

#[tokio::test]
async fn reject_cleanup_step_failure_does_not_change_terminal_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_REJECT_FAILING_CLEANUP).unwrap();

    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_reject_fail".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT_FAILING_CLEANUP.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let run_id = res1.run_id.clone();

    let decision = store
        .reject(&run_id, "operator", "no", chrono::Utc::now())
        .expect("reject succeeds");
    let (rejected_step_id, reason) = match decision {
        ApprovalDecision::Rejected {
            step_id, reason, ..
        } => (step_id, reason),
        other => panic!("expected Rejected, got {other:?}"),
    };

    let record_after_reject = store.load(&run_id).unwrap();

    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record_after_reject.workspace_id.clone(),
        workspace_path: record_after_reject.workspace_path.clone(),
        transcript_dir: record_after_reject.transcript_dir.clone(),
        factory: Arc::new(FailFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT_FAILING_CLEANUP.to_string()),
        resume_from: Some(ResumeState::from_rejection(
            run_id.clone(),
            Vec::new(),
            rejected_step_id.clone(),
            reason.clone(),
        )),
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human")
        .await
        .expect("a failing cleanup step is logged, not returned as an error");

    let records = store.read_step_results(&run_id).unwrap();
    let cleanup_record = records
        .iter()
        .find(|r| r.step_id == "notify_fail")
        .expect("on_reject step result persisted even on failure");
    assert!(
        !cleanup_record.success,
        "the cleanup step's own failure must be recorded"
    );

    // The run's terminal status is exactly what `RunStore::reject` set —
    // a failing cleanup step never touches it.
    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Rejected);
}

// ---------------------------------------------------------------------------
// Test 7 — a gate with an EMPTY on_reject: cleanup is a true no-op.
// ---------------------------------------------------------------------------

const WF_GATE_REJECT_EMPTY: &str = r#"
name: gate-reject-empty
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
"#;

#[tokio::test]
async fn reject_cleanup_with_empty_on_reject_dispatches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_REJECT_EMPTY).unwrap();

    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_reject_empty".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT_EMPTY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let run_id = res1.run_id.clone();

    let decision = store
        .reject(&run_id, "operator", "no thanks", chrono::Utc::now())
        .expect("reject succeeds");
    let (rejected_step_id, reason) = match decision {
        ApprovalDecision::Rejected {
            step_id, reason, ..
        } => (step_id, reason),
        other => panic!("expected Rejected, got {other:?}"),
    };

    let record_after_reject = store.load(&run_id).unwrap();

    // Wire a real JsonlSink at the same path production uses
    // (`<runs_dir>/<run_id>/events.jsonl` — see `rupu-cli`'s reject/resume
    // call sites), so this test exercises the same layering the CLI's
    // live reject path does: `store.reject()` already appended a
    // terminal `RunCompleted` to this file before `run_reject_cleanup`
    // is ever called, and `emit_gate_result` (task 4) unconditionally
    // emits the gate's own `StepStarted`/`StepCompleted` through this
    // sink even though `on_reject` is empty — the exact case task 4
    // fixed the trailing re-append for.
    let events_path = store.root.join(&run_id).join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));

    // PanicFactory proves nothing is ever dispatched — an empty on_reject
    // chain must not call the factory at all.
    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record_after_reject.workspace_id.clone(),
        workspace_path: record_after_reject.workspace_path.clone(),
        transcript_dir: record_after_reject.transcript_dir.clone(),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT_EMPTY.to_string()),
        resume_from: Some(ResumeState::from_rejection(
            run_id.clone(),
            Vec::new(),
            rejected_step_id.clone(),
            reason.clone(),
        )),
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human")
        .await
        .expect("empty on_reject is Ok without dispatching anything");

    // Only the gate's own (pre-existing, since Task 3 doesn't record a
    // rejected gate result outside of Task 4) result set is untouched by
    // an empty chain: still just the terminal record, no new steps.
    let records = store.read_step_results(&run_id).unwrap();
    assert_eq!(
        records.len(),
        1,
        "an empty on_reject still records the gate's own rejected result, nothing else"
    );
    assert_eq!(records[0].step_id, "gate");

    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Rejected);

    // The behavioral contract task 4 locks in: even with an empty
    // on_reject chain, events.jsonl's LAST line is a terminal
    // `run_completed` — never the gate's own trailing `step_completed`
    // that `emit_gate_result` unconditionally emits through the sink.
    let types = read_event_types(&events_path);
    assert_eq!(
        types.last().map(String::as_str),
        Some("run_completed"),
        "events.jsonl must end with run_completed even for an empty cleanup chain; got {types:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 8 — a gate's own `on_timeout: reject` policy firing is recorded as
// `via: "timeout"`, never `via: "human"` (spec §3.1). Mirrors the CLI's
// `approve` command landing on an already-overdue `on_timeout: reject`
// gate: `store.approve()` reports `ApprovalError::ExpiredRejected` and the
// caller (here, and in `rupu workflow approve` / `rupu workflow runs`)
// dispatches `run_reject_cleanup` with `via: "timeout"`.
// ---------------------------------------------------------------------------

const WF_GATE_TIMEOUT_REJECT: &str = r#"
name: gate-timeout-reject
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      timeout_seconds: 60
      on_timeout: reject
      on_reject:
        - id: notify_fail
          agent: worker
          prompt: "cleanup after timeout reject: {{ steps.gate.decision }}"
"#;

#[tokio::test]
async fn timeout_reject_records_via_timeout_not_human() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_TIMEOUT_REJECT).unwrap();

    // --- Phase 1: pause at the gate. ---
    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_timeout_reject".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_TIMEOUT_REJECT.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("phase 1 returns Ok");
    let awaiting = res1.awaiting.clone().expect("must pause at the gate");
    assert_eq!(awaiting.step_id, "gate");
    let run_id = res1.run_id.clone();

    // Force the gate overdue — mirrors a real clock tick landing after
    // `timeout_seconds` elapses.
    let mut record = store.load(&run_id).unwrap();
    record.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    store.update(&record).unwrap();

    // --- An operator's `approve` call lands on the now-overdue gate
    // (mirrors `rupu workflow approve`): the gate's own `on_timeout:
    // reject` policy already fired, so `store.approve()` reports
    // `ExpiredRejected` rather than `Approved`. ---
    let err = store
        .approve(&run_id, "operator", chrono::Utc::now())
        .expect_err("overdue on_timeout: reject gate must error ExpiredRejected");
    let (rejected_step_id, reason) = match err {
        ApprovalError::ExpiredRejected { step_id, reason } => (step_id, reason),
        other => panic!("expected ExpiredRejected, got {other:?}"),
    };
    assert_eq!(rejected_step_id, "gate");

    let record_after_reject = store.load(&run_id).unwrap();
    assert_eq!(record_after_reject.status, RunStatus::Rejected);

    // --- Cleanup: dispatch the on_reject chain with `via: "timeout"` —
    // what the CLI's timeout-driven call sites pass. ---
    let factory = Arc::new(EchoFactory::default());
    let opts2 = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: record_after_reject.workspace_id.clone(),
        workspace_path: record_after_reject.workspace_path.clone(),
        transcript_dir: record_after_reject.transcript_dir.clone(),
        factory: factory.clone(),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_TIMEOUT_REJECT.to_string()),
        resume_from: Some(ResumeState::from_rejection(
            run_id.clone(),
            Vec::new(),
            rejected_step_id.clone(),
            reason.clone(),
        )),
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "timeout")
        .await
        .expect("cleanup never errors");

    let records = store.read_step_results(&run_id).unwrap();
    let gate_record = records
        .iter()
        .find(|r| r.step_id == "gate")
        .expect("gate result persisted");
    let gate_output: serde_json::Value =
        serde_json::from_str(&gate_record.output).expect("gate output is JSON");
    assert_eq!(gate_output["decision"], "rejected");
    assert_eq!(
        gate_output["via"], "timeout",
        "a policy-driven timeout reject must record via=timeout, never via=human"
    );

    let cleanup_record = records
        .iter()
        .find(|r| r.step_id == "notify_fail")
        .expect("on_reject step result persisted");
    assert!(cleanup_record.success);
    assert!(
        cleanup_record
            .output
            .contains("cleanup after timeout reject: rejected"),
        "on_reject step should see steps.gate.decision == rejected; got {:?}",
        cleanup_record.output
    );

    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::Rejected);
}
