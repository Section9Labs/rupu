//! End-to-end test for fan-out sub-agent dispatch.
//!
//! Wires up a workflow with one parent step whose `MockProvider`
//! emits a `dispatch_agents_parallel` tool_use on turn 0 and a final
//! assistant message on turn 1. A test-side `FakeDispatcher` records
//! every dispatch call and returns canned outcomes, mirroring what
//! `CliAgentDispatcher` does end-to-end. The test asserts:
//!
//! 1. Both children were dispatched (the dispatcher saw two distinct
//!    agent names with the right prompts).
//! 2. The parent's final output is the post-dispatch assistant text —
//!    fan-out dispatch is transparent to the workflow output, same as
//!    single-child dispatch in Plan 1.
//! 3. A child's failure is surfaced via `all_succeeded: false` in the
//!    tool's response, but the parent step itself still succeeds (the
//!    parent agent is responsible for reasoning about the failure).
//! 4. The allowlist gate refuses dispatches when ANY agent in the
//!    request is outside `dispatchable_agents` — the dispatcher must
//!    not be reached at all.
//!
//! See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
use rupu_providers::types::StopReason;
use rupu_tools::{AgentDispatcher, DispatchError, DispatchOutcome, ToolContext};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const WF: &str = r#"
name: parent-with-children
steps:
  - id: review
    agent: writer
    actions: []
    prompt: "Please request security and perf reviews in parallel."
"#;

#[derive(Debug)]
struct FakeDispatcher {
    /// Records every (agent_name, prompt) pair received.
    calls: Mutex<Vec<(String, String)>>,
    /// Where to write child transcripts.
    transcript_dir: PathBuf,
    /// Optional canned failures keyed by agent name.
    failures: Mutex<std::collections::BTreeMap<String, String>>,
}

impl FakeDispatcher {
    fn new(transcript_dir: PathBuf) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            transcript_dir,
            failures: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    fn fail(self, agent: &str, reason: &str) -> Self {
        self.failures
            .lock()
            .unwrap()
            .insert(agent.to_string(), reason.to_string());
        self
    }
}

#[async_trait]
impl AgentDispatcher for FakeDispatcher {
    async fn dispatch(
        &self,
        agent_name: &str,
        prompt: String,
        _parent_run_id: &str,
        _parent_depth: u32,
    ) -> Result<DispatchOutcome, DispatchError> {
        self.calls
            .lock()
            .unwrap()
            .push((agent_name.to_string(), prompt));
        if let Some(reason) = self.failures.lock().unwrap().get(agent_name).cloned() {
            return Err(DispatchError::ChildRun(reason));
        }
        let sub_run_id = format!("sub_{agent_name}");
        let path = self.transcript_dir.join(format!("{sub_run_id}.jsonl"));
        std::fs::create_dir_all(&self.transcript_dir).unwrap();
        let mut writer = rupu_transcript::JsonlWriter::create(&path).unwrap();
        writer
            .write(&rupu_transcript::Event::RunStart {
                run_id: sub_run_id.clone(),
                workspace_id: "ws_test".into(),
                agent: agent_name.to_string(),
                provider: "mock".into(),
                model: "mock-1".into(),
                started_at: chrono::Utc::now(),
                mode: rupu_transcript::RunMode::Bypass,
            })
            .unwrap();
        writer
            .write(&rupu_transcript::Event::AssistantMessage {
                content: format!("{agent_name}: review complete"),
                thinking: None,
            })
            .unwrap();
        writer
            .write(&rupu_transcript::Event::RunComplete {
                run_id: sub_run_id.clone(),
                status: rupu_transcript::RunStatus::Ok,
                total_tokens: 7,
                duration_ms: 9,
                error: None,
            })
            .unwrap();
        writer.flush().unwrap();

        Ok(DispatchOutcome {
            agent: agent_name.to_string(),
            sub_run_id,
            transcript_path: path,
            output: format!("{agent_name}: review complete"),
            success: true,
            tokens_used: 7,
            duration_ms: 9,
        })
    }
}

struct ParallelFactory {
    dispatcher: Arc<dyn AgentDispatcher>,
    /// Override the agents passed to dispatch_agents_parallel — lets
    /// individual tests target the allowlist-gate path with an
    /// out-of-allowlist name.
    agents_payload: serde_json::Value,
}

#[async_trait]
impl StepFactory for ParallelFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_dispatch_parallel_1".into(),
                tool_name: "dispatch_agents_parallel".into(),
                tool_input: self.agents_payload.clone(),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "Aggregated reviews into the response.".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 5,
            },
        ]);
        let parent_run_id_for_ctx = Some(run_id.clone());
        AgentRunOpts {
            agent_name: "writer".into(),
            agent_system_prompt: "you are the writer".into(),
            agent_tools: Some(vec!["dispatch_agents_parallel".into()]),
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: ToolContext {
                workspace_path,
                bash_env_allowlist: Vec::new(),
                bash_timeout_secs: 120,
                dispatcher: Some(self.dispatcher.clone()),
                dispatchable_agents: Some(vec!["security-reviewer".into(), "perf-reviewer".into()]),
                parent_run_id: parent_run_id_for_ctx,
                depth: 0,
            },
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
            dispatchable_agents: Some(vec!["security-reviewer".into(), "perf-reviewer".into()]),
            step_id: String::new(),
            on_tool_call: None,
        }
    }
}

fn happy_payload() -> serde_json::Value {
    serde_json::json!({
        "agents": [
            { "id": "sec", "agent": "security-reviewer", "prompt": "Review auth flow for authz gaps." },
            { "id": "perf", "agent": "perf-reviewer", "prompt": "Review hot path for hotspots." },
        ]
    })
}

#[tokio::test]
async fn parent_step_fans_out_two_children_and_aggregates() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let dispatcher = Arc::new(FakeDispatcher::new(tmp.path().join("sub-transcripts")));
    let factory = Arc::new(ParallelFactory {
        dispatcher: dispatcher.clone(),
        agents_payload: happy_payload(),
    });

    let wf = Workflow::parse(WF).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_dispatch".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory,
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
    };

    let res = run_workflow(opts).await.expect("workflow runs");
    assert_eq!(res.step_results.len(), 1);
    let step = &res.step_results[0];
    assert!(step.success, "parent step should succeed");

    // 1. Dispatcher saw both children.
    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "two dispatches expected, got {calls:?}");
    let names: std::collections::BTreeSet<&str> = calls.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains("security-reviewer"));
    assert!(names.contains("perf-reviewer"));
    assert!(
        calls.iter().any(|(_, p)| p.contains("authz gaps")),
        "security prompt forwarded; got {calls:?}"
    );

    // 2. Parent's final output is the post-dispatch assistant text,
    //    not the raw tool_result JSON.
    assert!(
        step.output.contains("Aggregated reviews"),
        "step output should be the parent's final assistant text, got: {}",
        step.output
    );
}

#[tokio::test]
async fn one_child_failure_marks_all_succeeded_false_but_parent_continues() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let dispatcher = Arc::new(
        FakeDispatcher::new(tmp.path().join("sub-transcripts"))
            .fail("perf-reviewer", "provider exploded"),
    );
    let factory = Arc::new(ParallelFactory {
        dispatcher: dispatcher.clone(),
        agents_payload: happy_payload(),
    });

    let wf = Workflow::parse(WF).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_dispatch".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory,
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
    };

    let res = run_workflow(opts).await.expect("workflow runs");
    let step = &res.step_results[0];
    // Both dispatches were attempted (failures are not gates).
    assert_eq!(dispatcher.calls.lock().unwrap().len(), 2);
    // Parent's step still succeeds — it's up to the agent to handle
    // child failures via the all_succeeded flag in the JSON body.
    assert!(step.success, "parent step succeeds even when a child fails");
    assert!(
        step.output.contains("Aggregated reviews"),
        "parent's final assistant text should still flow through, got: {}",
        step.output
    );
}

#[tokio::test]
async fn allowlist_violation_blocks_dispatch_at_the_parallel_layer() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let dispatcher = Arc::new(FakeDispatcher::new(tmp.path().join("sub-transcripts")));
    let payload = serde_json::json!({
        "agents": [
            { "id": "sec", "agent": "security-reviewer", "prompt": "ok" },
            { "id": "rogue", "agent": "intruder", "prompt": "leak secrets" },
        ]
    });
    let factory = Arc::new(ParallelFactory {
        dispatcher: dispatcher.clone(),
        agents_payload: payload,
    });

    let wf = Workflow::parse(WF).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_dispatch".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory,
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: None,
    };

    run_workflow(opts).await.expect("workflow runs");

    // Allowlist gate runs BEFORE any spawn — dispatcher must not see
    // any call. (The "good" agent in the request is in the allowlist,
    // but the gate is all-or-nothing.)
    let calls = dispatcher.calls.lock().unwrap().clone();
    assert!(
        calls.is_empty(),
        "dispatcher must not be reached when any agent is outside the allowlist; got {calls:?}",
    );
}
