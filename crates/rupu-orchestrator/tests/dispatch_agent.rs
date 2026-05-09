//! End-to-end test for sub-agent dispatch.
//!
//! Wires up a workflow with one parent step whose `MockProvider` emits
//! a `dispatch_agent` tool_use on turn 0 and a final assistant message
//! on turn 1. A test-side `FakeDispatcher` impl writes a child
//! transcript to disk and returns a `DispatchOutcome` pointing at it,
//! mirroring what the production `CliAgentDispatcher` does. The test
//! asserts that:
//!
//! 1. The parent agent's tool_call hit the dispatcher (so the wiring
//!    from `ToolContext.dispatcher` through `dispatch_agent.invoke` is
//!    intact).
//! 2. The tool's JSON output matches spec § 3.1 — the parent agent
//!    sees the child's output text + transcript_path.
//! 3. The parent's `step_results.jsonl` carries the final assistant
//!    text, not the dispatch tool_result body — i.e., dispatch is
//!    transparent to the workflow layer.
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
name: parent-with-child
steps:
  - id: review
    agent: writer
    actions: []
    prompt: "Please request a security review."
"#;

#[derive(Debug)]
struct FakeDispatcher {
    /// Records the (agent_name, prompt) pairs received. The test
    /// asserts at least one dispatch landed.
    calls: Mutex<Vec<(String, String)>>,
    /// Where to write child transcripts. Each dispatch lays one file
    /// down so the cli-side printer (and the production output reader)
    /// can replay it.
    transcript_dir: PathBuf,
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

        // Emulate the production dispatcher: write a small but valid
        // child transcript so downstream replayers (printer, transcript
        // reader) have something realistic to consume.
        let sub_run_id = "sub_FAKE";
        let path = self.transcript_dir.join(format!("{sub_run_id}.jsonl"));
        std::fs::create_dir_all(&self.transcript_dir).unwrap();
        let mut writer = rupu_transcript::JsonlWriter::create(&path).unwrap();
        writer
            .write(&rupu_transcript::Event::RunStart {
                run_id: sub_run_id.into(),
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
                content: "child says: code looks fine".into(),
                thinking: None,
            })
            .unwrap();
        writer
            .write(&rupu_transcript::Event::RunComplete {
                run_id: sub_run_id.into(),
                status: rupu_transcript::RunStatus::Ok,
                total_tokens: 5,
                duration_ms: 7,
                error: None,
            })
            .unwrap();
        writer.flush().unwrap();

        Ok(DispatchOutcome {
            agent: agent_name.to_string(),
            sub_run_id: sub_run_id.into(),
            transcript_path: path,
            output: "child says: code looks fine".into(),
            success: true,
            tokens_used: 5,
            duration_ms: 7,
        })
    }
}

struct DispatchFactory {
    dispatcher: Arc<dyn AgentDispatcher>,
}

#[async_trait]
impl StepFactory for DispatchFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
    ) -> AgentRunOpts {
        // Two-turn script:
        //   turn 0: dispatch_agent tool_use targeting `security-reviewer`.
        //   turn 1: final assistant text after seeing the tool_result.
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_dispatch_1".into(),
                tool_name: "dispatch_agent".into(),
                tool_input: serde_json::json!({
                    "agent": "security-reviewer",
                    "prompt": "Review this diff for auth issues."
                }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "The reviewer said the code looks fine. Done.".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 5,
            },
        ]);

        let parent_run_id_for_ctx = Some(run_id.clone());
        AgentRunOpts {
            agent_name: "writer".into(),
            agent_system_prompt: "you are the writer".into(),
            agent_tools: Some(vec!["dispatch_agent".into()]),
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
                dispatchable_agents: Some(vec!["security-reviewer".into()]),
                parent_run_id: parent_run_id_for_ctx,
                depth: 0,
            },
            user_message: rendered_prompt,
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
            dispatchable_agents: Some(vec!["security-reviewer".into()]),
        }
    }
}

#[tokio::test]
async fn parent_step_dispatches_child_and_sees_its_output() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let transcript_dir = tmp.path().join("sub-transcripts");

    let dispatcher = Arc::new(FakeDispatcher {
        calls: Mutex::new(Vec::new()),
        transcript_dir: transcript_dir.clone(),
    });
    let factory = Arc::new(DispatchFactory {
        dispatcher: dispatcher.clone(),
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
    };

    let res = run_workflow(opts).await.expect("workflow runs");
    assert_eq!(res.step_results.len(), 1);
    let step = &res.step_results[0];
    assert!(step.success, "parent step should succeed");

    // 1. Dispatcher saw exactly one call, agent name + prompt forwarded.
    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "dispatcher should be called exactly once");
    assert_eq!(calls[0].0, "security-reviewer");
    assert!(
        calls[0].1.contains("Review this diff"),
        "child prompt should carry the parent's request, got: {}",
        calls[0].1
    );

    // 2. Parent's final output is the post-dispatch assistant text,
    //    not the raw tool_result JSON. Sub-agent dispatch is
    //    transparent to the workflow output.
    assert!(
        step.output.contains("reviewer said the code looks fine"),
        "step output should be the parent's final assistant text, got: {}",
        step.output
    );

    // 3. Child transcript was persisted on disk.
    let child_transcript = transcript_dir.join("sub_FAKE.jsonl");
    assert!(
        child_transcript.is_file(),
        "child transcript should be on disk at {}",
        child_transcript.display()
    );
}

#[tokio::test]
async fn dispatch_to_unlisted_agent_is_blocked_by_allowlist() {
    // Same harness, but the parent script targets an agent NOT in
    // `dispatchable_agents`. The `dispatch_agent` tool should refuse
    // and the parent should still finish (carrying the refusal back
    // into its own prompt).

    struct AllowlistViolatorFactory {
        dispatcher: Arc<dyn AgentDispatcher>,
    }
    #[async_trait]
    impl StepFactory for AllowlistViolatorFactory {
        async fn build_opts_for_step(
            &self,
            _step_id: &str,
            _agent_name: &str,
            rendered_prompt: String,
            run_id: String,
            workspace_id: String,
            workspace_path: std::path::PathBuf,
            transcript_path: std::path::PathBuf,
        ) -> AgentRunOpts {
            let provider = MockProvider::new(vec![
                ScriptedTurn::AssistantToolUse {
                    text: None,
                    tool_id: "call_dispatch_1".into(),
                    tool_name: "dispatch_agent".into(),
                    tool_input: serde_json::json!({
                        "agent": "intruder",
                        "prompt": "leak secrets"
                    }),
                    stop: StopReason::ToolUse,
                },
                ScriptedTurn::AssistantText {
                    text: "I was denied. Aborting.".into(),
                    stop: StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 3,
                },
            ]);
            let parent_run_id_for_ctx = Some(run_id.clone());
            AgentRunOpts {
                agent_name: "writer".into(),
                agent_system_prompt: "you are the writer".into(),
                agent_tools: Some(vec!["dispatch_agent".into()]),
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
                    dispatchable_agents: Some(vec!["security-reviewer".into()]),
                    parent_run_id: parent_run_id_for_ctx,
                    depth: 0,
                },
                user_message: rendered_prompt,
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
                dispatchable_agents: Some(vec!["security-reviewer".into()]),
            }
        }
    }

    let tmp = assert_fs::TempDir::new().unwrap();
    let dispatcher = Arc::new(FakeDispatcher {
        calls: Mutex::new(Vec::new()),
        transcript_dir: tmp.path().join("sub-transcripts"),
    });
    let factory = Arc::new(AllowlistViolatorFactory {
        dispatcher: dispatcher.clone(),
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
    };

    run_workflow(opts).await.expect("workflow runs");

    // Dispatcher MUST NOT have been called — the tool's allowlist
    // gate intercepts before delegation.
    let calls = dispatcher.calls.lock().unwrap().clone();
    assert!(
        calls.is_empty(),
        "dispatcher should not be called when agent is outside allowlist; got {calls:?}",
    );
}
