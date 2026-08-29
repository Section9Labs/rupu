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
use rupu_mcp::{McpPermission, ToolDispatcher};
use rupu_orchestrator::executor::JsonlSink;
use rupu_orchestrator::runner::{
    run_reject_cleanup, run_workflow, OrchestratorRunOpts, ResumeState, StepFactory,
};
use rupu_orchestrator::{
    ApprovalDecision, ApprovalError, RunStatus, RunStore, StepKind, StepResult, Workflow,
};
use rupu_providers::types::StopReason;
use rupu_scm::{
    Branch, Comment, CreatePr, Diff, FileContent, Platform, Pr, PrFilter, PrRef, Registry,
    RepoConnector, RepoRef, ScmError,
};
use rupu_tools::{PermissionMode, ToolContext};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

/// Records every `comment_pr` call it receives (as `(PrRef, rendered body)`)
/// so tests can assert on the exact templated value the dispatcher sent —
/// everything else is `unimplemented!()`. Mirrors `RecordingConnector` in
/// `tests/action_step.rs`.
#[derive(Default)]
struct RecordingConnector {
    calls: Mutex<Vec<(PrRef, String)>>,
    /// When `true`, `comment_pr` returns a `ScmError` instead of recording.
    fail: bool,
}

#[async_trait]
impl RepoConnector for RecordingConnector {
    fn platform(&self) -> Platform {
        Platform::Github
    }
    async fn list_repos(&self) -> Result<Vec<rupu_scm::Repo>, ScmError> {
        unimplemented!()
    }
    async fn get_repo(&self, _r: &RepoRef) -> Result<rupu_scm::Repo, ScmError> {
        unimplemented!()
    }
    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        unimplemented!()
    }
    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        unimplemented!()
    }
    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        unimplemented!()
    }
    async fn list_prs(&self, _r: &RepoRef, _f: PrFilter) -> Result<Vec<Pr>, ScmError> {
        unimplemented!()
    }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        unimplemented!()
    }
    async fn comment_pr(&self, p: &PrRef, body: &str) -> Result<Comment, ScmError> {
        if self.fail {
            return Err(ScmError::BadRequest {
                message: "boom: connector rejected the notify comment".into(),
            });
        }
        self.calls
            .lock()
            .unwrap()
            .push((p.clone(), body.to_string()));
        Ok(Comment {
            id: "comment_1".into(),
            author: "rupu-bot".into(),
            body: body.to_string(),
            created_at: chrono::Utc::now(),
            author_association: None,
        })
    }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
        unimplemented!()
    }
}

/// Builds a `ToolDispatcher` wired to a single `RecordingConnector` on
/// `Platform::Github`, returning both so tests can assert on recorded calls
/// after the run. Mirrors `dispatcher_with_connector` in `tests/action_step.rs`.
fn dispatcher_with_connector(fail: bool) -> (Arc<ToolDispatcher>, Arc<RecordingConnector>) {
    let connector = Arc::new(RecordingConnector {
        calls: Mutex::new(Vec::new()),
        fail,
    });
    let mut reg = Registry::empty();
    reg.insert_repo_connector(Platform::Github, connector.clone());
    let dispatcher = Arc::new(ToolDispatcher::new(
        Arc::new(reg),
        McpPermission::new(PermissionMode::Bypass, vec!["*".into()]),
    ));
    (dispatcher, connector)
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
        run_step: Default::default(),
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
    assert!(types.contains(&"step_started".to_string()), "got {types:?}");
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
        run_step: Default::default(),
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
        run_step: Default::default(),
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
    let prior_step_results: Vec<StepResult> = prior_records.iter().map(StepResult::from).collect();
    assert!(
        prior_step_results.is_empty(),
        "the gate never completed in phase 1, so no prior step results exist"
    );

    let factory2 = Arc::new(EchoFactory::default());
    let opts2 = OrchestratorRunOpts {
        run_step: Default::default(),
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
    assert_eq!(
        factory2.seen.lock().unwrap().clone(),
        vec!["after".to_string()]
    );

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
        run_step: Default::default(),
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
    let prior_step_results: Vec<StepResult> = prior_records.iter().map(StepResult::from).collect();
    assert_eq!(prior_step_results.len(), 1, "setup checkpointed on disk");

    let opts2 = OrchestratorRunOpts {
        run_step: Default::default(),
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
        run_step: Default::default(),
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
    let prior_step_results: Vec<StepResult> = prior_records.iter().map(StepResult::from).collect();
    assert!(
        prior_step_results.is_empty(),
        "the gate never completed, so no prior step results exist yet"
    );

    let factory = Arc::new(EchoFactory::default());
    let opts2 = OrchestratorRunOpts {
        run_step: Default::default(),
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

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human", None)
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
        cleanup_record
            .output
            .contains("cleanup after reject: rejected"),
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
        run_step: Default::default(),
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
        run_step: Default::default(),
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

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human", None)
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
        run_step: Default::default(),
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
        run_step: Default::default(),
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

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human", None)
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
        run_step: Default::default(),
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
        run_step: Default::default(),
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

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "timeout", None)
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

// ---------------------------------------------------------------------------
// Plan 4, Task 1 — gate `notify:` hooks fire best-effort when a gate parks.
// ---------------------------------------------------------------------------

const WF_GATE_NOTIFY: &str = r#"
name: gate-notify
inputs:
  env:
    type: string
    default: "prod"
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      notify:
        - action: scm.prs.comment
          with:
            platform: github
            owner: acme
            repo: widget
            number: 42
            body: "gate parking for approval, env={{ inputs.env }}"
"#;

// Test 9 — notify fires exactly once when the gate actually parks.
#[tokio::test]
async fn notify_fires_when_gate_parks() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_NOTIFY).unwrap();
    let (dispatcher, connector) = dispatcher_with_connector(false);

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_notify".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_NOTIFY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let res = run_workflow(opts).await.expect("a pause is Ok, not Err");
    let awaiting = res.awaiting.clone().expect("gate must pause the run");
    assert_eq!(awaiting.step_id, "gate");

    let calls = connector.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "notify must fire exactly once on park");
    assert_eq!(calls[0].0.number, 42);
    assert_eq!(calls[0].0.repo.owner, "acme");
    assert_eq!(calls[0].0.repo.repo, "widget");
    assert_eq!(
        calls[0].1, "gate parking for approval, env=prod",
        "notify body must be rendered (templated), not the raw source"
    );

    let record = store.load(&res.run_id).unwrap();
    assert_eq!(
        record.status,
        RunStatus::AwaitingApproval,
        "notify firing must not change the park outcome"
    );
}

// Test 10 — notify does NOT fire when the gate auto-approves.
#[tokio::test]
async fn notify_does_not_fire_on_auto_approve() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let yaml = r#"
name: gate-notify-auto
steps:
  - id: gate
    approval:
      auto_approve: "true"
      notify:
        - action: scm.prs.comment
          with:
            platform: github
            owner: acme
            repo: widget
            number: 42
            body: "should never be sent"
"#;
    let wf = Workflow::parse(yaml).unwrap();
    let (dispatcher, connector) = dispatcher_with_connector(false);

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_notify_auto".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let res = run_workflow(opts).await.expect("run completes");
    assert!(res.awaiting.is_none(), "auto-approved gate must not pause");

    let calls = connector.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        0,
        "notify must NOT fire when the gate auto-approves; got {calls:?}"
    );

    let record = store.load(&res.run_id).unwrap();
    assert_eq!(record.status, RunStatus::Completed);
}

// Test 11 — a notify failure never blocks the park (best-effort).
#[tokio::test]
async fn notify_failure_does_not_block_the_park() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_NOTIFY).unwrap();
    let (dispatcher, _connector) = dispatcher_with_connector(true);

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_notify_fail".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_NOTIFY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("a failing notify must never turn the park into an Err");
    let awaiting = res.awaiting.clone().expect("gate must still pause");
    assert_eq!(awaiting.step_id, "gate");

    let record = store.load(&res.run_id).unwrap();
    assert_eq!(
        record.status,
        RunStatus::AwaitingApproval,
        "a notify error must not block the park"
    );
}

// Test 12 — no action dispatcher wired: notify is skipped (warned), the gate
// still parks normally rather than erroring.
#[tokio::test]
async fn notify_skips_gracefully_with_no_action_dispatcher() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_NOTIFY).unwrap();

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_notify_no_dispatcher".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_NOTIFY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res = run_workflow(opts)
        .await
        .expect("no dispatcher must not error the run; notify is best-effort");
    let awaiting = res.awaiting.clone().expect("gate must still pause");
    assert_eq!(awaiting.step_id, "gate");
}

// ---------------------------------------------------------------------------
// Test 13 (Task 5b-2a, spec §7) — MULTI-GATE reject with cleanup: rejecting
// ONE gate of a two-gate parked set runs THAT gate's own `on_reject` chain,
// leaves the sibling gate untouched/still parked, and does NOT falsely close
// `events.jsonl` with a terminal `RunCompleted` while the run is still
// genuinely `AwaitingApproval` on the other gate.
// ---------------------------------------------------------------------------

const WF_MULTI_GATE_REJECT: &str = r#"
name: multi-gate-reject
steps:
  - id: fanout
    split: [gate_a, gate_b]
  - id: gate_a
    approval:
      prompt: "Approve A?"
      on_reject:
        - id: notify_fail_a
          agent: worker
          prompt: "cleanup after gate_a reject: {{ steps.gate_a.decision }}"
    next: [a]
  - id: gate_b
    approval:
      prompt: "Approve B?"
    next: [b]
  - id: a
    agent: worker
    prompt: "do a"
  - id: b
    agent: worker
    prompt: "do b"
"#;

#[tokio::test]
async fn reject_one_gate_of_a_multi_gate_set_runs_its_own_cleanup_leaves_sibling_parked() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_MULTI_GATE_REJECT).unwrap();

    // --- Phase 1: fanout batch-parks both gates. `event_sink: None` here —
    // `run_workflow`'s own `RunStore`-backed persistence still writes
    // `events.jsonl` via `append_terminal_event`/the executor plumbing
    // regardless of whether an external sink is wired; the run id isn't
    // known until after this call allocates it, so a caller-supplied sink
    // can't be built ahead of time anyway (every other test in this file
    // takes the same `event_sink: None` shape for phase 1). ---
    let opts1 = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_multi_gate_reject".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_MULTI_GATE_REJECT.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };
    let res1 = run_workflow(opts1).await.expect("both gates batch-park");
    let run_id = res1.run_id.clone();
    let awaiting = res1
        .awaiting
        .clone()
        .expect("both gates must pause the run");
    assert_eq!(awaiting.gates.len(), 2);

    // --- Operator rejects gate_a ONLY (mirrors `rupu workflow reject
    // <run_id> --gate gate_a`): gate_b must stay parked. ---
    let decision = store
        .reject_gate(
            &run_id,
            "operator",
            "not today",
            chrono::Utc::now(),
            Some("gate_a"),
        )
        .expect("reject_gate(gate_a) succeeds");
    let (rejected_step_id, reason) = match decision {
        ApprovalDecision::Rejected {
            step_id, reason, ..
        } => (step_id, reason),
        other => panic!("expected Rejected, got {other:?}"),
    };
    assert_eq!(rejected_step_id, "gate_a");

    let record_after_reject = store.load(&run_id).unwrap();
    assert_eq!(
        record_after_reject.status,
        RunStatus::AwaitingApproval,
        "gate_b is still parked — rejecting gate_a alone must not finalize the run"
    );
    assert_eq!(record_after_reject.awaiting.len(), 1);
    assert_eq!(record_after_reject.awaiting[0].step_id, "gate_b");

    // --- Cleanup: dispatch gate_a's on_reject chain. ---
    // Unlike the single-gate `WF_GATE_REJECT` fixture, this workflow has a
    // preceding `fanout` (split) step that DID complete before either gate
    // parked — so `prior_step_results` carries that one entry, not none.
    let prior_records = store.read_step_results(&run_id).unwrap();
    let prior_step_results: Vec<StepResult> = prior_records.iter().map(StepResult::from).collect();
    assert_eq!(
        prior_step_results
            .iter()
            .map(|r| r.step_id.as_str())
            .collect::<Vec<_>>(),
        vec!["fanout"],
        "only the split node itself has completed; neither gate has"
    );

    let factory = Arc::new(EchoFactory::default());
    let opts2 = OrchestratorRunOpts {
        run_step: Default::default(),
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
        workflow_yaml: Some(WF_MULTI_GATE_REJECT.to_string()),
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

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human", None)
        .await
        .expect("cleanup never errors");

    // gate_a's on_reject step actually dispatched.
    assert_eq!(
        factory.seen.lock().unwrap().clone(),
        vec!["notify_fail_a".to_string()]
    );

    let records = store.read_step_results(&run_id).unwrap();
    let gate_a_record = records
        .iter()
        .find(|r| r.step_id == "gate_a")
        .expect("gate_a result persisted");
    assert!(gate_a_record.success);
    let gate_a_output: serde_json::Value =
        serde_json::from_str(&gate_a_record.output).expect("gate_a output is JSON");
    assert_eq!(gate_a_output["decision"], "rejected");
    let cleanup_record = records
        .iter()
        .find(|r| r.step_id == "notify_fail_a")
        .expect("on_reject step result persisted");
    assert!(cleanup_record.success);
    assert!(
        cleanup_record
            .output
            .contains("cleanup after gate_a reject: rejected"),
        "on_reject step should see steps.gate_a.decision == rejected; got {:?}",
        cleanup_record.output
    );

    // The run is STILL AwaitingApproval on gate_b — cleanup for gate_a
    // must not have touched the run's overall status or gate_b's entry.
    let record_final = store.load(&run_id).unwrap();
    assert_eq!(record_final.status, RunStatus::AwaitingApproval);
    assert_eq!(record_final.awaiting.len(), 1);
    assert_eq!(record_final.awaiting[0].step_id, "gate_b");

    // events.jsonl must NOT get a false terminal `RunCompleted` appended
    // while the run is still genuinely active on gate_b's path — this is
    // the bug Task 5b-2a's fix to `run_reject_cleanup` step 5 closes.
    // `opts1`/`opts2` above both pass `event_sink: None`, and nothing else
    // in this scenario writes to `events.jsonl` (`reject_gate` only calls
    // `append_terminal_event` when it flips the run terminal, which it
    // does NOT for gate_a here since gate_b keeps the set non-empty) —
    // so this file must not exist at all. Before the fix, `run_reject_
    // cleanup` unconditionally called `store.append_terminal_event(..)`
    // regardless of `opts.event_sink`, which WOULD have created it with a
    // `RunCompleted` line even though the run was still `AwaitingApproval`.
    let events_path = store.events_path(&run_id);
    assert!(
        !events_path.is_file(),
        "events.jsonl must not be created/appended while gate_b is still parked \
         (found: {})",
        events_path.display()
    );

    // --- Now reject gate_b too: the set empties, the run finalizes. ---
    let decision2 = store
        .reject_gate(
            &run_id,
            "operator",
            "not today either",
            chrono::Utc::now(),
            Some("gate_b"),
        )
        .expect("reject_gate(gate_b) succeeds");
    assert!(matches!(decision2, ApprovalDecision::Rejected { .. }));
    let record_terminal = store.load(&run_id).unwrap();
    assert_eq!(record_terminal.status, RunStatus::Rejected);
    assert!(record_terminal.awaiting.is_empty());
}

// ---------------------------------------------------------------------------
// I-42 — a `when:`-skipped gate must keep its `kind`, not silently become
// `Linear` (`StepKind::default()`). Two skip sites build the skipped
// `StepResult`: the linear-loop path (single-cursor declaration-order loop,
// no split/join/branch) and the scheduler path (`run_scheduler`, taken for
// any nonlinear workflow — see `is_nonlinear`). Both must set
// `kind: step_kind_for_run_record(step)`, matching the two adjacent skip
// sites (prune/cancel, branch-not-taken) that already do.
// ---------------------------------------------------------------------------

// Test 14 — linear-loop path: a single gate step with `when: false`.
#[tokio::test]
async fn when_false_gate_preserves_kind_linear_path() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let yaml = r#"
name: gate-when-skip-linear
steps:
  - id: gate
    when: "false"
    approval:
      prompt: "never shown"
"#;
    let wf = Workflow::parse(yaml).unwrap();
    assert!(
        !rupu_orchestrator::workflow::is_nonlinear(&wf),
        "fixture must be linear-loop mode to exercise the linear-loop skip site"
    );

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_when_skip_linear".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res = run_workflow(opts).await.expect("run completes");
    assert_eq!(res.step_results.len(), 1);
    let gate = &res.step_results[0];
    assert_eq!(gate.step_id, "gate");
    assert!(gate.skipped, "when: false must skip the gate");
    assert_eq!(
        gate.kind,
        StepKind::ApprovalGate,
        "a when:-skipped gate must keep kind ApprovalGate, not fall back to Linear"
    );
}

// Test 15 — scheduler path: same shape, but the workflow is nonlinear
// (`split:`) so `run_workflow` routes through `run_scheduler` instead.
#[tokio::test]
async fn when_false_gate_preserves_kind_scheduler_path() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let yaml = r#"
name: gate-when-skip-scheduler
steps:
  - id: fanout
    split: [gate, other]
  - id: gate
    when: "false"
    approval:
      prompt: "never shown"
  - id: other
    agent: worker
    prompt: "hi"
"#;
    let wf = Workflow::parse(yaml).unwrap();
    assert!(
        rupu_orchestrator::workflow::is_nonlinear(&wf),
        "fixture must be nonlinear to exercise the scheduler skip site"
    );

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_when_skip_scheduler".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(EchoFactory::default()),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let res = run_workflow(opts).await.expect("run completes");
    let gate = res
        .step_results
        .iter()
        .find(|r| r.step_id == "gate")
        .expect("gate result present");
    assert!(gate.skipped, "when: false must skip the gate");
    assert_eq!(
        gate.kind,
        StepKind::ApprovalGate,
        "a when:-skipped gate must keep kind ApprovalGate, not fall back to Linear (scheduler path)"
    );
}

// ---------------------------------------------------------------------------
// I-44 — a gate's `notify:` hook transcript must be reachable from a
// persisted `StepResult`, not orphaned. `fire_notify_hooks` used to discard
// `execute_action_step`'s `Ok(StepResult)`, so the transcript it
// unconditionally wrote at a fresh ULID path was referenced by nothing.
// ---------------------------------------------------------------------------

// Test 16 — after a gate with a `notify:` hook parks, the hook's own
// `StepResult` (id `<gate>.notify`) is persisted to `step_results.jsonl`
// and its `transcript_path` points at a real file on disk.
#[tokio::test]
async fn notify_hook_transcript_is_referenced_by_a_persisted_step_result() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_NOTIFY).unwrap();
    let (dispatcher, _connector) = dispatcher_with_connector(false);

    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_notify_persisted".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_NOTIFY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let res = run_workflow(opts).await.expect("a pause is Ok, not Err");
    let awaiting = res.awaiting.clone().expect("gate must pause the run");
    assert_eq!(awaiting.step_id, "gate");

    let records = store.read_step_results(&res.run_id).unwrap();
    let notify_record = records
        .iter()
        .find(|r| r.step_id == "gate.notify")
        .expect("notify hook's StepResult must be persisted to step_results.jsonl");
    assert!(
        notify_record.transcript_path.exists(),
        "notify hook's transcript_path must exist on disk: {:?}",
        notify_record.transcript_path
    );
}
