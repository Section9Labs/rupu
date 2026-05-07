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
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
    ) -> AgentRunOpts {
        // Produce a single assistant text turn that echoes the
        // rendered prompt + records which (parent step, sub agent)
        // pair dispatched it. Tests assert against this output.
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
    let b_prompt = &res.step_results[1].rendered_prompt;
    assert!(
        b_prompt.contains("step a agent ag echo: First step says: hello A"),
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 1);
    let prompt = &res.step_results[0].rendered_prompt;
    assert!(
        prompt.contains("PR #99") && prompt.contains("Section9Labs/rupu"),
        "step prompt should bind {{event.*}} fields, got: {prompt}"
    );
}

const WF_ISSUE: &str = r#"
name: issue-aware
steps:
  - id: read
    agent: ag
    actions: []
    prompt: |
      Reviewing issue #{{ issue.number }} on {{ issue.r.project }}: {{ issue.title }}
      Labels: {{ issue.labels | join(", ") }}
  - id: gate
    agent: ag
    actions: []
    when: "{{ 'bug' in issue.labels }}"
    prompt: "Triage as bug"
"#;

#[tokio::test]
async fn issue_payload_is_visible_in_step_prompts_and_when_filters() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_ISSUE).unwrap();
    // Shape mirrors `rupu_scm::Issue` after serde_json::to_value.
    let issue = serde_json::json!({
        "r": { "tracker": "github", "project": "Section9Labs/rupu", "number": 42 },
        "title": "Cron tick crashes on empty workflows dir",
        "body": "When `<global>/workflows/` doesn't exist...",
        "state": "open",
        "labels": ["bug", "cron"],
        "author": "matt",
        "number": 42
    });
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_issue".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        issue: Some(issue),
        issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
    let read_prompt = &res.step_results[0].rendered_prompt;
    assert!(
        read_prompt.contains("issue #42")
            && read_prompt.contains("Section9Labs/rupu")
            && read_prompt.contains("Cron tick crashes")
            && read_prompt.contains("bug, cron"),
        "issue.* fields should bind: {read_prompt}"
    );
    // The `when:` filter on step 2 evaluates `'bug' in issue.labels`
    // → truthy → step ran (skipped == false).
    assert!(!res.step_results[1].skipped, "bug-label gate should fire");
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
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
        summary_prompt.contains("first: step review_each agent ag echo: review a.rs"),
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let fan = &res.step_results[0];
    assert_eq!(fan.items.len(), 2);
    assert!(fan.items[0].rendered_prompt.contains("review a.rs (rust)"));
    assert!(fan.items[1]
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
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
        agent_name: &str,
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
                text: format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }
        };
        let provider = MockProvider::new(vec![turn]);
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let fan = &res.step_results[0];
    assert_eq!(fan.items.len(), 3);
    assert!(
        !fan.success,
        "step success should be false when any item failed"
    );
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
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let err = run_workflow(opts).await.expect_err("should abort");
    let msg = err.to_string();
    assert!(
        msg.contains("review_each[0]") && msg.contains("simulated failure"),
        "unexpected error message: {msg}"
    );
}

// -- Parallel (agent fan-out) -----------------------------------------------

const WF_PARALLEL: &str = r#"
name: triage
inputs:
  diff: { type: string, default: "+ added line" }
steps:
  - id: triage
    actions: []
    parallel:
      - id: sec
        agent: security-reviewer
        prompt: "security review of: {{ inputs.diff }}"
      - id: perf
        agent: perf-reviewer
        prompt: "perf review of: {{ inputs.diff }}"
      - id: maint
        agent: maintainability-reviewer
        prompt: "maintainability review of: {{ inputs.diff }}"
    max_parallel: 2
  - id: summarize
    agent: writer
    actions: []
    prompt: |
      sec: {{ steps.triage.sub_results.sec.output }}
      perf: {{ steps.triage.sub_results.perf.output }}
      list-len: {{ steps.triage.results | length }}
"#;

#[tokio::test]
async fn parallel_dispatches_each_sub_step_with_its_own_agent_and_prompt() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_PARALLEL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_par".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);

    let triage = &res.step_results[0];
    assert_eq!(triage.items.len(), 3);
    assert!(triage.success);
    // Sub-step ids preserved in declared order.
    let sub_ids: Vec<&str> = triage.items.iter().map(|i| i.sub_id.as_str()).collect();
    assert_eq!(sub_ids, vec!["sec", "perf", "maint"]);
    // Each sub-step's rendered prompt referenced its declared agent
    // (asserted via the FakeFactory echo format).
    assert!(triage.items[0].output.contains("agent security-reviewer"));
    assert!(triage.items[1].output.contains("agent perf-reviewer"));
    assert!(triage.items[2]
        .output
        .contains("agent maintainability-reviewer"));

    // The follow-up step can address sub-results by name (named map)
    // and read the positional list length.
    let summary_prompt = &res.step_results[1].rendered_prompt;
    assert!(
        summary_prompt.contains("agent security-reviewer")
            && summary_prompt.contains("agent perf-reviewer")
            && summary_prompt.contains("list-len: 3"),
        "summarize should address sub_results by name + list length, got: {summary_prompt}"
    );
}

const WF_PARALLEL_FAIL: &str = r#"
name: triage-with-failure
steps:
  - id: triage
    actions: []
    continue_on_error: true
    parallel:
      - id: ok
        agent: writer
        prompt: "ok"
      - id: bad
        agent: writer
        prompt: "FAIL on purpose"
"#;

#[tokio::test]
async fn parallel_continue_on_error_records_per_sub_step_failures() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_PARALLEL_FAIL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_par_fail".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FailingFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let triage = &res.step_results[0];
    assert_eq!(triage.items.len(), 2);
    assert!(!triage.success);
    let ok = triage.items.iter().find(|i| i.sub_id == "ok").unwrap();
    let bad = triage.items.iter().find(|i| i.sub_id == "bad").unwrap();
    assert!(ok.success);
    assert!(!bad.success);
}

const WF_PARALLEL_ABORTS: &str = r#"
name: triage-no-tolerance
steps:
  - id: triage
    actions: []
    parallel:
      - id: ok
        agent: writer
        prompt: "ok"
      - id: bad
        agent: writer
        prompt: "FAIL on purpose"
"#;

#[tokio::test]
async fn parallel_without_continue_on_error_aborts_with_sub_step_id_in_message() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_PARALLEL_ABORTS).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_orch_par_aborts".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FailingFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let err = run_workflow(opts).await.expect_err("should abort");
    let msg = err.to_string();
    assert!(
        msg.contains("triage.bad") && msg.contains("simulated failure"),
        "expected triage.bad in error message, got: {msg}"
    );
}

// -- Persistent run state ---------------------------------------------------

const WF_PERSIST: &str = r#"
name: persist-me
steps:
  - id: a
    agent: ag
    actions: []
    prompt: "hello a"
  - id: b
    agent: ag
    actions: []
    prompt: "hello b ({{ steps.a.output }})"
"#;

#[tokio::test]
async fn run_store_records_run_metadata_and_per_step_rows() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(runs_root.clone()));
    let wf = Workflow::parse(WF_PERSIST).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_persist".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(WF_PERSIST.to_string()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert!(!res.run_id.is_empty(), "run_id should be populated");

    // run.json reflects the terminal Completed state.
    let record = store.load(&res.run_id).expect("load run record");
    assert_eq!(record.workflow_name, "persist-me");
    assert_eq!(record.status, rupu_orchestrator::RunStatus::Completed);
    assert!(record.finished_at.is_some());
    assert!(record.error_message.is_none());

    // workflow.yaml round-trips.
    let snap = store.read_workflow_snapshot(&res.run_id).unwrap();
    assert_eq!(snap, WF_PERSIST);

    // step_results.jsonl has one row per step in declared order.
    let rows = store.read_step_results(&res.run_id).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].step_id, "a");
    assert_eq!(rows[1].step_id, "b");
    assert!(rows[0].success);
    assert!(rows[1].success);
    // The second step's persisted rendered_prompt shows `steps.a.output` resolved.
    assert!(
        rows[1]
            .rendered_prompt
            .contains("hello b (step a agent ag echo: hello a)"),
        "step b prompt should have rendered against step a's output, got: {}",
        rows[1].rendered_prompt
    );
}

const WF_PERSIST_FAIL: &str = r#"
name: persist-fail
steps:
  - id: ok
    agent: ag
    actions: []
    prompt: "ok"
  - id: bad
    agent: ag
    actions: []
    prompt: "FAIL on purpose"
"#;

#[tokio::test]
async fn run_store_marks_run_failed_with_error_message() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(runs_root.clone()));
    let wf = Workflow::parse(WF_PERSIST_FAIL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_persist_fail".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FailingFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(WF_PERSIST_FAIL.to_string()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let _ = run_workflow(opts).await.expect_err("workflow should fail");

    // The Completed=>Failed transition must happen even though the
    // top-level call returned an error. Walk the store to find the
    // single run.
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    let rec = &listed[0];
    assert_eq!(rec.status, rupu_orchestrator::RunStatus::Failed);
    assert!(rec.finished_at.is_some());
    assert!(rec
        .error_message
        .as_ref()
        .is_some_and(|m| m.contains("simulated failure")));
    // The successful first step's row should still be persisted —
    // partial step rows are valuable for post-mortem inspection.
    let rows = store.read_step_results(&rec.id).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "only the successful row is appended; the failed step aborts before persist"
    );
    assert_eq!(rows[0].step_id, "ok");
}

#[tokio::test]
async fn no_run_store_skips_persistence_and_emits_empty_run_id() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_PERSIST).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_no_persist".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert!(
        res.run_id.is_empty(),
        "run_id should be empty when no run_store is wired"
    );
    assert_eq!(res.step_results.len(), 2);
}

// -- Approval gates ---------------------------------------------------------

const WF_APPROVAL: &str = r#"
name: deploy-with-approval
inputs:
  tag: { type: string, default: "v1.2.3" }
steps:
  - id: prepare
    agent: ag
    actions: []
    prompt: "preparing {{ inputs.tag }}"
  - id: deploy
    agent: ag
    actions: []
    approval:
      required: true
      prompt: "About to deploy {{ inputs.tag }} (prepared by {{ steps.prepare.output }}). Approve?"
    prompt: "deploying {{ inputs.tag }}"
  - id: notify
    agent: ag
    actions: []
    prompt: "deployed: {{ steps.deploy.output }}"
"#;

#[tokio::test]
async fn approval_gate_pauses_run_and_persists_awaiting_state() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(runs_root));
    let wf = Workflow::parse(WF_APPROVAL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_appr".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(WF_APPROVAL.to_string()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();

    // The first step ran; the second step (with `approval: required`)
    // paused the run before dispatch.
    assert!(res.awaiting.is_some(), "run should have paused");
    let awaiting = res.awaiting.unwrap();
    assert_eq!(awaiting.step_id, "deploy");
    assert!(
        awaiting.prompt.contains("About to deploy v1.2.3"),
        "approval prompt should render against context, got: {}",
        awaiting.prompt
    );
    assert!(
        awaiting
            .prompt
            .contains("prepared by step prepare agent ag echo"),
        "approval prompt should see prior step output, got: {}",
        awaiting.prompt
    );

    // Persisted record reflects AwaitingApproval + the awaiting fields.
    let record = store.load(&res.run_id).unwrap();
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval
    );
    assert_eq!(record.awaiting_step_id.as_deref(), Some("deploy"));
    assert!(record.approval_prompt.is_some());
    assert!(record.finished_at.is_none(), "paused runs aren't finished");

    // Only the prepare step ran; deploy / notify did not.
    let rows = store.read_step_results(&res.run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].step_id, "prepare");
}

#[tokio::test]
async fn resume_from_approval_picks_up_at_awaited_step() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(runs_root));
    let wf = Workflow::parse(WF_APPROVAL).unwrap();

    // First pass — pause at the deploy step.
    let opts = OrchestratorRunOpts {
        workflow: wf.clone(),
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_resume".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(WF_APPROVAL.to_string()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let run_id = res.run_id.clone();
    assert!(res.awaiting.is_some());

    // Operator approves (mutate persisted record back to Running, clear awaiting).
    let mut record = store.load(&run_id).unwrap();
    record.status = rupu_orchestrator::RunStatus::Running;
    record.awaiting_step_id = None;
    record.approval_prompt = None;
    store.update(&record).unwrap();

    // Build the resume opts from persisted state.
    let prior_records = store.read_step_results(&run_id).unwrap();
    let prior_step_results: Vec<rupu_orchestrator::StepResult> = prior_records
        .iter()
        .map(rupu_orchestrator::StepResult::from)
        .collect();
    let body = store.read_workflow_snapshot(&run_id).unwrap();

    let opts = OrchestratorRunOpts {
        workflow: Workflow::parse(&body).unwrap(),
        inputs: std::collections::BTreeMap::new(),
        workspace_id: record.workspace_id.clone(),
        workspace_path: record.workspace_path.clone(),
        transcript_dir: record.transcript_dir.clone(),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(body.clone()),
        issue: None,
        issue_ref: None,
        resume_from: Some(rupu_orchestrator::ResumeState {
            run_id: run_id.clone(),
            prior_step_results,
            approved_step_id: "deploy".into(),
        }),
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();

    // Run completed this time.
    assert!(res.awaiting.is_none(), "resume should run to completion");
    assert_eq!(
        res.step_results.len(),
        3,
        "all three steps now in result list"
    );
    let names: Vec<&str> = res
        .step_results
        .iter()
        .map(|sr| sr.step_id.as_str())
        .collect();
    assert_eq!(names, vec!["prepare", "deploy", "notify"]);

    // Persisted record is Completed.
    let record = store.load(&run_id).unwrap();
    assert_eq!(record.status, rupu_orchestrator::RunStatus::Completed);
    assert!(record.awaiting_step_id.is_none());
    assert!(record.finished_at.is_some());

    // step_results.jsonl now has all three rows. The first was
    // appended on the original run; deploy + notify on resume.
    let rows = store.read_step_results(&run_id).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].step_id, "prepare");
    assert_eq!(rows[1].step_id, "deploy");
    assert_eq!(rows[2].step_id, "notify");

    // The notify step's prompt rendered against the *resumed* deploy
    // step's output, proving context flowed correctly across processes.
    assert!(
        rows[2]
            .rendered_prompt
            .contains("deployed: step deploy agent ag echo: deploying v1.2.3"),
        "notify should see resumed deploy output, got: {}",
        rows[2].rendered_prompt
    );
}

#[tokio::test]
async fn approval_required_false_does_not_pause() {
    let s = r#"
name: x
steps:
  - id: a
    agent: ag
    actions: []
    approval:
      required: false
    prompt: "hi"
"#;
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(s).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_no_pause".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    assert!(res.awaiting.is_none());
    assert_eq!(res.step_results.len(), 1);
    assert!(res.step_results[0].success);
}

// -- Panel steps (commit 1: parallel + findings aggregation) ----------------

const WF_PANEL: &str = r#"
name: code-panel
inputs:
  diff: { type: string, default: "+ added line" }
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - security-reviewer
        - perf-reviewer
      subject: "{{ inputs.diff }}"
      max_parallel: 2
"#;

/// Test factory whose response depends on the agent name. For
/// panel-step tests we want each panelist to emit findings JSON
/// in their final assistant text (the runtime contract). The
/// FakeFactory above echoes the prompt; here we override per-
/// panelist behavior.
struct PanelFactory;

#[async_trait]
impl StepFactory for PanelFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
    ) -> AgentRunOpts {
        // Hand-built JSON keyed by agent name. security-reviewer
        // emits one HIGH; perf-reviewer emits one MEDIUM with
        // surrounding prose (tests the loose-parser fallback);
        // any other agent emits no findings.
        let body = match agent_name {
            "security-reviewer" => {
                "{\"findings\":[{\"severity\":\"HIGH\",\"title\":\"hardcoded secret\",\"body\":\"...\"}]}"
                    .to_string()
            }
            "perf-reviewer" => {
                "Here's my review:\n\n```json\n{\"findings\":[{\"severity\":\"medium\",\"title\":\"O(n^2) loop\",\"body\":\"...\"}]}\n```\n\nThanks!"
                    .to_string()
            }
            "no-issues-reviewer" => "{\"findings\": []}".to_string(),
            "fixer" => "diff applied; please re-review".to_string(),
            _ => format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
        };
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: body,
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "review".into(),
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
            suppress_stream_stdout: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        }
    }
}

#[tokio::test]
async fn panel_step_runs_panelists_in_parallel_and_aggregates_findings() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_PANEL).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_panel".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(PanelFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let panel = &res.step_results[0];
    assert!(panel.success);
    assert_eq!(panel.items.len(), 2, "two panelist sub-runs");
    assert_eq!(panel.findings.len(), 2, "one finding per panelist");

    // Findings carry the source panelist + parsed severity.
    let sec = panel
        .findings
        .iter()
        .find(|f| f.source == "security-reviewer")
        .unwrap();
    assert_eq!(sec.severity, rupu_orchestrator::Severity::High);
    assert_eq!(sec.title, "hardcoded secret");
    let perf = panel
        .findings
        .iter()
        .find(|f| f.source == "perf-reviewer")
        .unwrap();
    assert_eq!(perf.severity, rupu_orchestrator::Severity::Medium);
    assert_eq!(perf.title, "O(n^2) loop");

    // Single-pass (no gate) reports iterations=1, resolved=true.
    assert_eq!(panel.iterations, 1);
    assert!(panel.resolved);
}

#[tokio::test]
async fn panel_findings_visible_to_subsequent_steps() {
    let s = r#"
name: panel-then-summarize
inputs:
  diff: { type: string, default: "+ x" }
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - security-reviewer
        - perf-reviewer
      subject: "{{ inputs.diff }}"
  - id: summarize
    agent: writer
    actions: []
    prompt: |
      max_severity={{ steps.panel.max_severity }}
      count={{ steps.panel.findings | length }}
      first={{ steps.panel.findings[0].title }} ({{ steps.panel.findings[0].source }})
"#;
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(s).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_panel_seq".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(PanelFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let summary_prompt = &res.step_results[1].rendered_prompt;
    assert!(
        summary_prompt.contains("max_severity=high"),
        "max_severity should be 'high' (=HIGH > MEDIUM), got: {summary_prompt}"
    );
    assert!(
        summary_prompt.contains("count=2"),
        "findings.length should be 2, got: {summary_prompt}"
    );
    assert!(
        summary_prompt.contains("first=hardcoded secret (security-reviewer)"),
        "first finding should be addressable, got: {summary_prompt}"
    );
}

#[tokio::test]
async fn panel_with_unparseable_panelist_records_zero_findings_for_that_panelist() {
    let s = r#"
name: panel-broken
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - security-reviewer
        - garbage-reviewer
      subject: "{{ inputs.diff | default('x') }}"
"#;
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(s).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_panel_garbage".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(PanelFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let panel = &res.step_results[0];
    assert_eq!(panel.items.len(), 2);
    // garbage-reviewer's output isn't JSON; only security's finding should appear.
    assert_eq!(panel.findings.len(), 1);
    assert_eq!(panel.findings[0].source, "security-reviewer");
}

// -- Panel gate loop (commit 2) ---------------------------------------------

/// Factory whose panelist behavior changes by iteration count. We
/// track per-agent invocation counts and key the response on the
/// invocation index — first call returns findings, second call
/// returns empty findings (gate cleared).
struct LoopingPanelFactory {
    calls: Arc<std::sync::Mutex<std::collections::BTreeMap<String, u32>>>,
}

impl LoopingPanelFactory {
    fn new() -> Self {
        Self {
            calls: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
        }
    }
}

#[async_trait]
impl StepFactory for LoopingPanelFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: std::path::PathBuf,
        transcript_path: std::path::PathBuf,
    ) -> AgentRunOpts {
        let invocation = {
            let mut map = self.calls.lock().unwrap();
            let n = map.entry(agent_name.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        let body = match (agent_name, invocation) {
            ("security-reviewer", 1) => {
                "{\"findings\":[{\"severity\":\"high\",\"title\":\"sql injection\",\"body\":\"...\"}]}"
                    .to_string()
            }
            // Second iteration: panel sees zero findings → gate clears.
            ("security-reviewer", _) => "{\"findings\":[]}".to_string(),
            ("fixer", _) => "diff applied; sql injection patched".to_string(),
            ("stubborn-reviewer", _) => {
                // Always emits a HIGH — used in the "max_iterations exhausted" test.
                "{\"findings\":[{\"severity\":\"critical\",\"title\":\"unfixable\",\"body\":\"...\"}]}"
                    .to_string()
            }
            _ => format!("step {step_id} agent {agent_name} echo: {rendered_prompt}"),
        };
        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: body,
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "x".into(),
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
            suppress_stream_stdout: false,
            mcp_registry: None,
            effort: None,
            context_window: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        }
    }
}

const WF_PANEL_GATE: &str = r#"
name: panel-with-gate
inputs:
  diff: { type: string, default: "+ buggy line" }
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - security-reviewer
      subject: "{{ inputs.diff }}"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: fixer
        max_iterations: 3
"#;

#[tokio::test]
async fn panel_gate_loops_with_fixer_until_severity_clears() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let factory = Arc::new(LoopingPanelFactory::new());
    let wf = Workflow::parse(WF_PANEL_GATE).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_gate".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::clone(&factory) as Arc<dyn StepFactory>,
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let panel = &res.step_results[0];
    // First iteration produced 1 HIGH finding; fixer ran; second
    // iteration produced 0 findings → gate cleared.
    assert!(panel.resolved);
    assert_eq!(panel.iterations, 2);
    // The persisted findings are the *final* iteration's findings
    // (which is empty since the gate cleared).
    assert!(panel.findings.is_empty());

    // Verify panelist was invoked exactly twice and fixer once.
    let calls = factory.calls.lock().unwrap().clone();
    assert_eq!(calls.get("security-reviewer").copied(), Some(2));
    assert_eq!(calls.get("fixer").copied(), Some(1));
}

const WF_PANEL_GATE_EXHAUSTED: &str = r#"
name: panel-stubborn
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - stubborn-reviewer
      subject: "anything"
      gate:
        until_no_findings_at_severity_or_above: high
        fix_with: fixer
        max_iterations: 2
"#;

#[tokio::test]
async fn panel_gate_marks_unresolved_when_max_iterations_exhausted() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let factory = Arc::new(LoopingPanelFactory::new());
    let wf = Workflow::parse(WF_PANEL_GATE_EXHAUSTED).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_stubborn".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::clone(&factory) as Arc<dyn StepFactory>,
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let panel = &res.step_results[0];
    assert!(!panel.resolved, "max_iterations exhausted → unresolved");
    assert_eq!(panel.iterations, 2);
    // Findings from the final iteration are preserved so downstream
    // steps (or operators) can still see what's outstanding.
    assert_eq!(panel.findings.len(), 1);
    assert_eq!(panel.findings[0].title, "unfixable");
    assert_eq!(
        panel.findings[0].severity,
        rupu_orchestrator::Severity::Critical
    );

    // Fixer ran (max_iterations - 1) times before the final pass.
    let calls = factory.calls.lock().unwrap().clone();
    assert_eq!(calls.get("stubborn-reviewer").copied(), Some(2));
    assert_eq!(calls.get("fixer").copied(), Some(1));
}

#[tokio::test]
async fn panel_gate_clears_immediately_when_first_pass_below_threshold() {
    // Threshold is "critical"; security-reviewer's first pass emits
    // HIGH which is below "critical", so the gate clears on the
    // first iteration without ever calling the fixer.
    let s = r#"
name: panel-medium-tolerance
steps:
  - id: panel
    actions: []
    panel:
      panelists:
        - security-reviewer
      subject: "x"
      gate:
        until_no_findings_at_severity_or_above: critical
        fix_with: fixer
        max_iterations: 3
"#;
    let tmp = assert_fs::TempDir::new().unwrap();
    let factory = Arc::new(LoopingPanelFactory::new());
    let wf = Workflow::parse(s).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_low".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::clone(&factory) as Arc<dyn StepFactory>,
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let panel = &res.step_results[0];
    assert!(panel.resolved);
    assert_eq!(panel.iterations, 1);
    // Findings under the threshold are still surfaced.
    assert_eq!(panel.findings.len(), 1);
    assert_eq!(
        panel.findings[0].severity,
        rupu_orchestrator::Severity::High
    );

    let calls = factory.calls.lock().unwrap().clone();
    assert_eq!(calls.get("security-reviewer").copied(), Some(1));
    assert_eq!(
        calls.get("fixer").copied(),
        None,
        "fixer should not have run"
    );
}

// -- Approval timeout (PR2: timeout_seconds wiring) -------------------------

const WF_APPROVAL_WITH_TIMEOUT: &str = r#"
name: approval-with-timeout
inputs:
  tag: { type: string, default: "v1.0" }
steps:
  - id: prepare
    agent: ag
    actions: []
    prompt: "preparing {{ inputs.tag }}"
  - id: deploy
    agent: ag
    actions: []
    approval:
      required: true
      prompt: "Approve deploy of {{ inputs.tag }}?"
      timeout_seconds: 60
    prompt: "deploying"
"#;

#[tokio::test]
async fn approval_with_timeout_seconds_persists_awaiting_since_and_expires_at() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(runs_root));
    let wf = Workflow::parse(WF_APPROVAL_WITH_TIMEOUT).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_to".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(WF_APPROVAL_WITH_TIMEOUT.to_string()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let info = res.awaiting.expect("workflow should pause");
    let expires_at = info
        .expires_at
        .expect("AwaitingInfo.expires_at should be populated when timeout_seconds is set");

    let record = store.load(&res.run_id).unwrap();
    assert_eq!(
        record.status,
        rupu_orchestrator::RunStatus::AwaitingApproval
    );
    let since = record.awaiting_since.expect("awaiting_since should be set");
    let on_disk_expires = record.expires_at.expect("expires_at should be set");
    // expires_at == awaiting_since + 60s, with allowance for clock
    // drift between AwaitingInfo construction and disk persistence.
    let drift = (on_disk_expires - (since + chrono::Duration::seconds(60)))
        .num_seconds()
        .abs();
    assert!(drift < 2, "expires_at drift too large: {drift}s");
    // AwaitingInfo's expires_at should match the persisted one
    // closely (constructed from the same `now`).
    let info_drift = (expires_at - on_disk_expires).num_seconds().abs();
    assert!(info_drift < 2, "info vs disk drift: {info_drift}s");
}

#[tokio::test]
async fn approval_without_timeout_seconds_leaves_expires_at_unset() {
    let s = r#"
name: no-timeout
steps:
  - id: deploy
    agent: ag
    actions: []
    approval:
      required: true
    prompt: "x"
"#;
    let tmp = assert_fs::TempDir::new().unwrap();
    let runs_root = tmp.path().join("runs");
    let store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(runs_root));
    let wf = Workflow::parse(s).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_no_to".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(std::sync::Arc::clone(&store)),
        workflow_yaml: Some(s.to_string()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let info = res.awaiting.unwrap();
    assert!(info.expires_at.is_none());
    let record = store.load(&res.run_id).unwrap();
    assert!(
        record.awaiting_since.is_some(),
        "awaiting_since always set on pause"
    );
    assert!(record.expires_at.is_none(), "no timeout → no expires_at");
}
