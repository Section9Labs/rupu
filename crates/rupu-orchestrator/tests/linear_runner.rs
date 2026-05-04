use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

// Note: ItemResult is reachable via res.step_results[i].items; we don't
// import it directly in tests but rely on field access.

const WF: &str = r#"
name: chained
steps:
  - id: a
    agent: ag
    actions: []
    prompt: "First step says: hello A"
  - id: b
    agent: ag
    actions: []
    prompt: |
      A said: {{ steps.a.output }}
"#;

struct FakeFactory;

#[async_trait]
impl StepFactory for FakeFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
    ) -> AgentRunOpts {
        // Produce a single assistant text turn that echoes the rendered prompt.
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: format!("step {step_id} echo: {rendered_prompt}"),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: format!("ag-{step_id}"),
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
            mode_str: "bypass".into(),
            no_stream: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
        }
    }
}

#[tokio::test]
async fn second_step_sees_first_step_output_via_template() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
    let b_prompt = &res.step_results[1].rendered_prompt;
    assert!(
        b_prompt.contains("step a echo: First step says: hello A"),
        "step b should see step a's output, got: {b_prompt}"
    );
}

const WF_EVENT: &str = r#"
name: event-aware
trigger:
  on: event
  event: github.pr.opened
steps:
  - id: greet
    agent: ag
    actions: []
    prompt: |
      reviewing PR #{{ event.pull_request.number }} on {{ event.repository.full_name }}
"#;

#[tokio::test]
async fn event_payload_is_visible_in_step_prompts() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_EVENT).unwrap();
    let event = serde_json::json!({
        "pull_request": { "number": 99 },
        "repository": { "full_name": "Section9Labs/rupu" }
    });
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_evt".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: Some(event),
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 1);
    let prompt = &res.step_results[0].rendered_prompt;
    assert!(
        prompt.contains("PR #99") && prompt.contains("Section9Labs/rupu"),
        "step prompt should bind {{event.*}} fields, got: {prompt}"
    );
}

// -- Fan-out (`for_each:`) --------------------------------------------------
//
// `FakeFactory` always succeeds and echoes the rendered prompt. That's
// fine for prompt-binding + ordering tests. For continue_on_error /
// failure tests we use `FailingFactory` below which emits a
// ProviderError when the rendered prompt contains the marker "FAIL".

const WF_FOREACH: &str = r#"
name: review-each
steps:
  - id: review_each
    agent: ag
    actions: []
    for_each: |
      a.rs
      b.rs
      c.rs
    prompt: "review {{ item }} ({{ loop.index }}/{{ loop.length }})"
  - id: summarize
    agent: ag
    actions: []
    prompt: |
      reviewed {{ steps.review_each.results | length }} files
      first: {{ steps.review_each.results[0] }}
"#;

#[tokio::test]
async fn for_each_dispatches_one_item_per_line_and_binds_loop_metadata() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_FOREACH).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_fanout".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);

    let fan = &res.step_results[0];
    assert_eq!(fan.step_id, "review_each");
    assert_eq!(fan.items.len(), 3);
    assert!(fan.success);
    // Items keep their declared order regardless of finish order.
    let item_paths: Vec<&str> = fan
        .items
        .iter()
        .map(|i| i.item.as_str().unwrap_or(""))
        .collect();
    assert_eq!(item_paths, vec!["a.rs", "b.rs", "c.rs"]);
    // Loop metadata is bound into each item's prompt.
    assert!(fan.items[0].rendered_prompt.contains("review a.rs (1/3)"));
    assert!(fan.items[1].rendered_prompt.contains("review b.rs (2/3)"));
    assert!(fan.items[2].rendered_prompt.contains("review c.rs (3/3)"));

    // The follow-up step sees `steps.review_each.results[*]`.
    let summary_prompt = &res.step_results[1].rendered_prompt;
    assert!(
        summary_prompt.contains("reviewed 3 files"),
        "summarize should see results length, got: {summary_prompt}"
    );
    assert!(
        summary_prompt.contains("first: step review_each echo: review a.rs"),
        "summarize should see first item's output, got: {summary_prompt}"
    );
}

const WF_FOREACH_JSON: &str = r#"
name: from-json-array
steps:
  - id: review_each
    agent: ag
    actions: []
    for_each: '[{"path": "a.rs", "lang": "rust"}, {"path": "b.py", "lang": "python"}]'
    prompt: "review {{ item.path }} ({{ item.lang }})"
"#;

#[tokio::test]
async fn for_each_accepts_a_json_array_of_objects() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_FOREACH_JSON).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_json".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let fan = &res.step_results[0];
    assert_eq!(fan.items.len(), 2);
    assert!(fan.items[0].rendered_prompt.contains("review a.rs (rust)"));
    assert!(fan
        .items[1]
        .rendered_prompt
        .contains("review b.py (python)"));
}

const WF_FOREACH_FROM_INPUTS: &str = r#"
name: items-from-inputs
inputs:
  files: { type: string, default: "x.rs\ny.rs" }
steps:
  - id: review_each
    agent: ag
    actions: []
    max_parallel: 2
    for_each: "{{ inputs.files }}"
    prompt: "checking {{ item }}"
"#;

#[tokio::test]
async fn for_each_pulls_items_from_workflow_inputs_with_max_parallel_cap() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_FOREACH_FROM_INPUTS).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_inputs".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let fan = &res.step_results[0];
    assert_eq!(fan.items.len(), 2);
    let item_paths: Vec<&str> = fan
        .items
        .iter()
        .map(|i| i.item.as_str().unwrap_or(""))
        .collect();
    assert_eq!(item_paths, vec!["x.rs", "y.rs"]);
}

// Factory that fails any item whose rendered prompt contains "FAIL".
struct FailingFactory;

#[async_trait]
impl StepFactory for FailingFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
    ) -> AgentRunOpts {
        let turn = if rendered_prompt.contains("FAIL") {
            ScriptedTurn::ProviderError("simulated failure for fan-out test".into())
        } else {
            ScriptedTurn::AssistantText {
                text: format!("step {step_id} echo: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }
        };
        let provider = MockProvider::new(vec![turn]);
        AgentRunOpts {
            agent_name: format!("ag-{step_id}"),
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
            mode_str: "bypass".into(),
            no_stream: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
        }
    }
}

const WF_FOREACH_FAILS: &str = r#"
name: review-with-failure
steps:
  - id: review_each
    agent: ag
    actions: []
    continue_on_error: true
    for_each: |
      ok-1
      FAIL-2
      ok-3
    prompt: "{{ item }}"
"#;

#[tokio::test]
async fn for_each_continue_on_error_records_failures_and_keeps_going() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_FOREACH_FAILS).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_fails".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FailingFactory),
        event: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let fan = &res.step_results[0];
    assert_eq!(fan.items.len(), 3);
    assert!(!fan.success, "step success should be false when any item failed");
    assert!(fan.items[0].success);
    assert!(!fan.items[1].success, "FAIL-2 should fail");
    assert!(fan.items[2].success);
}

const WF_FOREACH_ABORTS: &str = r#"
name: review-no-tolerance
steps:
  - id: review_each
    agent: ag
    actions: []
    for_each: |
      FAIL-1
      ok-2
    prompt: "{{ item }}"
"#;

#[tokio::test]
async fn for_each_without_continue_on_error_aborts_workflow_on_first_failure() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_FOREACH_ABORTS).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_aborts".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FailingFactory),
        event: None,
    };
    let err = run_workflow(opts).await.expect_err("should abort");
    let msg = err.to_string();
    assert!(
        msg.contains("review_each[0]") && msg.contains("simulated failure"),
        "unexpected error message: {msg}"
    );
}
