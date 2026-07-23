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
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, ResumeState, StepFactory};
use rupu_orchestrator::{RunStatus, RunStore, StepKind, StepResult, Workflow};
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
