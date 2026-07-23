//! Action-step runtime (Plan 2, task 2): `action:` steps execute for real
//! through the in-process MCP `ToolDispatcher`.
//!
//! Mirrors the harness shape of `tests/gate_node.rs`: a real disk-backed
//! `RunStore`, `run_workflow` driven directly through its public
//! `OrchestratorRunOpts`, and a `RecordingConnector` (modeled on
//! `FakePrConnector` in `crates/rupu-cli/src/cmd/autoflow.rs`) that records
//! every `comment_pr` call it receives so tests can assert on the exact
//! (already-templated) body the dispatcher sent.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn, DEFAULT_MAX_TOKENS};
use rupu_agent::AgentRunOpts;
use rupu_mcp::{McpPermission, ToolDispatcher};
use rupu_orchestrator::executor::JsonlSink;
use rupu_orchestrator::runner::{
    run_reject_cleanup, run_workflow, OrchestratorRunOpts, ResumeState, RunWorkflowError,
    StepFactory,
};
use rupu_orchestrator::{ApprovalDecision, RunStore, StepKind, Workflow};
use rupu_providers::types::StopReason;
use rupu_scm::{
    Branch, Comment, CreatePr, Diff, FileContent, Platform, Pr, PrFilter, PrRef, Registry,
    RepoConnector, RepoRef, ScmError,
};
use rupu_tools::{PermissionMode, ToolContext};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Records every `comment_pr` call it receives (as `(PrRef, rendered body)`)
/// so tests can assert on the exact templated value the dispatcher sent —
/// everything else is `unimplemented!()`, matching `FakePrConnector`'s shape
/// (`crates/rupu-cli/src/cmd/autoflow.rs`).
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
                message: "boom: connector rejected the comment".into(),
            });
        }
        self.calls.lock().unwrap().push((p.clone(), body.to_string()));
        Ok(Comment {
            id: "comment_1".into(),
            author: "rupu-bot".into(),
            body: body.to_string(),
            created_at: chrono::Utc::now(),
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
/// after the run.
fn dispatcher_with_connector(
    mode: PermissionMode,
    fail: bool,
) -> (Arc<ToolDispatcher>, Arc<RecordingConnector>) {
    let connector = Arc::new(RecordingConnector {
        calls: Mutex::new(Vec::new()),
        fail,
    });
    let mut reg = Registry::empty();
    // `insert_repo_connector` takes `Arc<dyn RepoConnector>`; this clone is
    // coerced to the trait object while `connector` keeps the concrete
    // `Arc<RecordingConnector>` handle tests read `.calls` off of afterward.
    reg.insert_repo_connector(Platform::Github, connector.clone());
    let dispatcher = Arc::new(ToolDispatcher::new(
        Arc::new(reg),
        McpPermission::new(mode, vec!["*".into()]),
    ));
    (dispatcher, connector)
}

/// Echoes the rendered prompt back as the step's final assistant text —
/// used for the `seed` step whose output the action step's `with:` templates
/// reference. Mirrors `EchoFactory` in `tests/gate_node.rs`.
#[derive(Default)]
struct EchoFactory;
#[async_trait]
impl StepFactory for EchoFactory {
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

/// Panics if ever asked to dispatch an agent — used by tests whose workflow
/// has no linear/agent steps at all.
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
        panic!("PanicFactory: build_opts_for_step must not be called")
    }
}

/// Read every event line out of a (flushed) `events.jsonl` file as
/// `(type, step_id)` pairs, filtered to `step_id == step`, so tests can
/// assert on the exact event sequence for one step without depending on
/// `Event`'s full field list.
fn event_types_for_step(path: &std::path::Path, step: &str) -> Vec<String> {
    let body = std::fs::read_to_string(path).unwrap_or_default();
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("step_id").and_then(|s| s.as_str()) == Some(step))
        .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .collect()
}

const WF_ACTION_HAPPY: &str = r#"
name: action-happy
steps:
  - id: seed
    agent: worker
    prompt: "hello world"
  - id: comment
    action: scm.prs.comment
    with:
      platform: github
      owner: acme
      repo: widget
      number: 7
      body: "{{ steps.seed.output }}"
"#;

// ---------------------------------------------------------------------------
// Case 1 — happy path: action step dispatches for real, StepResult +
// events reflect it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_action_step_dispatches_through_tool_dispatcher() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_ACTION_HAPPY).unwrap();
    let (dispatcher, connector) = dispatcher_with_connector(PermissionMode::Ask, false);

    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_action_happy".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(EchoFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_ACTION_HAPPY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let res = run_workflow(opts).await.expect("run completes");
    assert!(res.awaiting.is_none());
    assert_eq!(res.step_results.len(), 2);

    let comment = &res.step_results[1];
    assert_eq!(comment.step_id, "comment");
    assert_eq!(comment.kind, StepKind::Action);
    assert!(comment.success, "action step must succeed");
    let output: serde_json::Value =
        serde_json::from_str(&comment.output).expect("action output is the dispatcher's JSON");
    assert_eq!(output["body"], "done: hello world");

    // The connector actually recorded the call.
    let calls = connector.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.number, 7);
    assert_eq!(calls[0].0.repo.owner, "acme");
    assert_eq!(calls[0].0.repo.repo, "widget");
    assert_eq!(calls[0].1, "done: hello world");
    drop(calls);

    let types = event_types_for_step(&events_path, "comment");
    assert!(
        types.contains(&"step_started".to_string()),
        "got {types:?}"
    );
    assert!(
        types.contains(&"step_completed".to_string()),
        "got {types:?}"
    );
}

// ---------------------------------------------------------------------------
// Case 2 — templated `with:` values render before dispatch (same workflow;
// isolated assertion on the recorded call).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn templated_with_values_render_before_reaching_the_connector() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_ACTION_HAPPY).unwrap();
    let (dispatcher, connector) = dispatcher_with_connector(PermissionMode::Bypass, false);

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_action_templated".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(EchoFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_ACTION_HAPPY.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    run_workflow(opts).await.expect("run completes");

    let calls = connector.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].1, "done: hello world",
        "the connector must see the RENDERED body, never the raw `{{ steps.seed.output }}` template"
    );
}

// ---------------------------------------------------------------------------
// Case 3 — connector error: aborts the run by default; `continue_on_error:
// true` records success=false and lets the run continue.
// ---------------------------------------------------------------------------

const WF_ACTION_FAILS: &str = r#"
name: action-fails
steps:
  - id: comment
    action: scm.prs.comment
    with:
      platform: github
      owner: acme
      repo: widget
      number: 3
      body: "oops"
"#;

#[tokio::test]
async fn connector_error_fails_the_run_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_ACTION_FAILS).unwrap();
    let (dispatcher, _connector) = dispatcher_with_connector(PermissionMode::Bypass, true);

    let events_path = tmp.path().join("events.jsonl");
    let sink = Arc::new(JsonlSink::create(&events_path).expect("create jsonl sink"));

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_action_fail".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_ACTION_FAILS.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let err = run_workflow(opts).await.expect_err("connector error aborts the run");
    assert!(matches!(err, RunWorkflowError::Action { .. }), "got: {err:?}");
    assert!(
        err.to_string().contains("boom"),
        "error must carry the connector's message; got: {err}"
    );

    let types = event_types_for_step(&events_path, "comment");
    assert!(
        types.contains(&"step_failed".to_string()),
        "got {types:?}"
    );
}

#[tokio::test]
async fn connector_error_with_continue_on_error_records_failure_and_continues() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let yaml = r#"
name: action-fails-tolerated
steps:
  - id: comment
    action: scm.prs.comment
    continue_on_error: true
    with:
      platform: github
      owner: acme
      repo: widget
      number: 3
      body: "oops"
  - id: after
    agent: worker
    prompt: "still ran"
"#;
    let wf = Workflow::parse(yaml).unwrap();
    let (dispatcher, _connector) = dispatcher_with_connector(PermissionMode::Bypass, true);

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_action_fail_tolerated".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(EchoFactory),
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

    let res = run_workflow(opts).await.expect("continue_on_error tolerates the failure");
    assert!(res.awaiting.is_none());
    assert_eq!(res.step_results.len(), 2);

    let comment = &res.step_results[0];
    assert_eq!(comment.step_id, "comment");
    assert_eq!(comment.kind, StepKind::Action);
    assert!(!comment.success, "the connector error must be recorded as a failure");

    let after = &res.step_results[1];
    assert_eq!(after.step_id, "after");
    assert!(after.success, "the workflow must continue past the tolerated failure");
}

// ---------------------------------------------------------------------------
// Case 4 — readonly mode blocks a Write-class tool; no connector call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn readonly_mode_blocks_write_tool_before_the_connector_is_called() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_ACTION_FAILS).unwrap();
    let (dispatcher, connector) = dispatcher_with_connector(PermissionMode::Readonly, false);

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_action_readonly".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_ACTION_FAILS.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    let err = run_workflow(opts)
        .await
        .expect_err("readonly mode must refuse a write-class tool");
    assert!(
        err.to_string().contains("readonly"),
        "error must mention readonly; got: {err}"
    );
    assert!(
        connector.calls.lock().unwrap().is_empty(),
        "a denied call must never reach the connector"
    );
}

// ---------------------------------------------------------------------------
// Case 5 — no dispatcher wired: ActionDispatcherMissing, naming the step.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_action_dispatcher_errors_naming_the_step() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_ACTION_FAILS).unwrap();

    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: BTreeMap::new(),
        workspace_id: "ws_action_missing".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_ACTION_FAILS.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
    };

    let err = run_workflow(opts)
        .await
        .expect_err("no dispatcher wired must fail loudly, never silently no-op");
    assert!(
        matches!(err, RunWorkflowError::ActionDispatcherMissing { ref step } if step == "comment"),
        "got: {err:?}"
    );
    assert!(err.to_string().contains("comment"), "got: {err}");
}

// ---------------------------------------------------------------------------
// Case 6 — an action step inside an on_reject cleanup chain dispatches for
// real through the same `execute_action_step` helper.
// ---------------------------------------------------------------------------

const WF_GATE_REJECT_ACTION: &str = r#"
name: gate-reject-action
steps:
  - id: gate
    approval:
      prompt: "Approve the deploy?"
      on_reject:
        - id: notify_action
          action: scm.prs.comment
          with:
            platform: github
            owner: acme
            repo: widget
            number: 9
            body: "cleanup: {{ steps.gate.decision }}"
"#;

#[tokio::test]
async fn on_reject_cleanup_dispatches_action_step_for_real() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let wf = Workflow::parse(WF_GATE_REJECT_ACTION).unwrap();

    // --- Phase 1: pause at the gate (no dispatcher needed — gates never
    // dispatch actions themselves). ---
    let opts1 = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_gate_reject_action".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF_GATE_REJECT_ACTION.to_string()),
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

    // --- Operator rejects. ---
    let decision = store
        .reject(&run_id, "operator", "not today", chrono::Utc::now())
        .expect("reject succeeds");
    let (rejected_step_id, reason) = match decision {
        ApprovalDecision::Rejected {
            step_id, reason, ..
        } => (step_id, reason),
        other => panic!("expected Rejected, got {other:?}"),
    };

    let record_after_reject = store.load(&run_id).unwrap();
    let prior_records = store.read_step_results(&run_id).unwrap();
    let prior_step_results: Vec<rupu_orchestrator::StepResult> =
        prior_records.iter().map(rupu_orchestrator::StepResult::from).collect();

    // --- Cleanup: dispatch the on_reject chain with a REAL dispatcher. ---
    let (dispatcher, connector) = dispatcher_with_connector(PermissionMode::Bypass, false);
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
        workflow_yaml: Some(WF_GATE_REJECT_ACTION.to_string()),
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
        action_dispatcher: Some(dispatcher),
        pause: None,
    };

    run_reject_cleanup(opts2, &rejected_step_id, &reason, "human")
        .await
        .expect("cleanup never errors");

    // The on_reject action step actually dispatched through the connector,
    // with the gate's real decision templated in.
    let calls = connector.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.number, 9);
    assert_eq!(calls[0].1, "cleanup: rejected");
    drop(calls);

    let records = store.read_step_results(&run_id).unwrap();
    let cleanup_record = records
        .iter()
        .find(|r| r.step_id == "notify_action")
        .expect("on_reject action step result persisted");
    assert_eq!(cleanup_record.kind, StepKind::Action);
    assert!(cleanup_record.success);
    let output: serde_json::Value =
        serde_json::from_str(&cleanup_record.output).expect("action output is JSON");
    assert_eq!(output["body"], "cleanup: rejected");
}
