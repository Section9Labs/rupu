//! Integration tests: the runner EXECUTES `branch:` steps — it renders the
//! condition, records `then`/`else` on the branch step's output, and skips
//! every step on the not-taken arm (`skipped == true`, empty output, a
//! "not taken by branch" reason) while the taken arm runs and downstream
//! reconvergence sees the skipped arm's empty output.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, PauseReason, ResumeState, StepFactory,
};
use rupu_orchestrator::{StepKind, StepResult, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;

#[derive(Default)]
struct CollectSink {
    events: Mutex<Vec<Event>>,
}

impl EventSink for CollectSink {
    fn emit(&self, _run_id: &str, ev: &Event) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

/// Echoes the rendered prompt back as the step's output. The branch step
/// itself dispatches NO agent, so it needs no scripted response — every
/// agent step (classify / arm_a / arm_b / join) gets exactly one turn.
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
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: None,
            step_id: step_id.to_string(),
            on_tool_call,
            on_stream_event: None,
            concerns: None,
            max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause: None,
        }
    }
}

fn opts_for(wf: Workflow, tmp: &assert_fs::TempDir, sink: Arc<CollectSink>) -> OrchestratorRunOpts {
    OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_branch".into(),
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
        strict_templates: false,
        event_sink: Some(sink as Arc<dyn EventSink>),
        unit_dispatcher: None,
        pause: None,
    }
}

fn step<'a>(
    res: &'a rupu_orchestrator::runner::OrchestratorRunResult,
    id: &str,
) -> &'a rupu_orchestrator::StepResult {
    res.step_results
        .iter()
        .find(|sr| sr.step_id == id)
        .unwrap_or_else(|| panic!("missing step result for `{id}`"))
}

// classify → route(branch) → arm_a (then) / arm_b (else) → join (reconverge).
const WF_BRANCH: &str = r#"
name: routed
steps:
  - id: classify
    agent: ag
    actions: []
    prompt: "classify this"
  - id: route
    branch:
      condition: "CONDITION"
      then: [arm_a]
      else: [arm_b]
  - id: arm_a
    agent: ag
    actions: []
    prompt: "arm a work"
  - id: arm_b
    agent: ag
    actions: []
    prompt: "arm b work"
  - id: join
    agent: ag
    actions: []
    prompt: |
      a=[{{ steps.arm_a.output }}]
      b=[{{ steps.arm_b.output }}]
"#;

#[tokio::test]
async fn branch_take_then_skips_else_arm_and_reconverges() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    let wf = Workflow::parse(&WF_BRANCH.replace("CONDITION", "true")).unwrap();
    let res = run_workflow(opts_for(wf, &tmp, sink.clone()))
        .await
        .unwrap();

    // The branch step records which arm it took.
    let route = step(&res, "route");
    assert_eq!(route.output, "then");
    assert!(route.success && !route.skipped);
    assert_eq!(route.kind, rupu_orchestrator::StepKind::Branch);

    // arm_a (the taken `then` arm) ran; arm_b (the `else` arm) was skipped.
    let arm_a = step(&res, "arm_a");
    assert!(!arm_a.skipped, "taken arm_a should run");
    assert!(arm_a.success);
    let arm_b = step(&res, "arm_b");
    assert!(arm_b.skipped, "not-taken arm_b should be skipped");
    assert_eq!(arm_b.output, "", "skipped arm has empty output");

    // Reconvergence: join ran, and saw arm_b's output as empty.
    let join = step(&res, "join");
    assert!(!join.skipped, "join should run after the branch");
    assert!(
        join.rendered_prompt.contains("b=[]"),
        "join should see arm_b's empty output, got: {}",
        join.rendered_prompt
    );
    assert!(
        join.rendered_prompt.contains("a=[step arm_a agent ag echo:"),
        "join should see arm_a's real output, got: {}",
        join.rendered_prompt
    );

    // The StepSkipped event for arm_b carries the branch reason.
    let events = sink.events.lock().unwrap();
    let skip_reason = events.iter().find_map(|e| match e {
        Event::StepSkipped { step_id, reason, .. } if step_id == "arm_b" => Some(reason.clone()),
        _ => None,
    });
    assert!(
        skip_reason
            .as_deref()
            .is_some_and(|r| r.contains("not taken by branch")),
        "arm_b StepSkipped reason should mention the branch, got: {skip_reason:?}"
    );
}

#[tokio::test]
async fn branch_take_else_skips_then_arm() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    let wf = Workflow::parse(&WF_BRANCH.replace("CONDITION", "false")).unwrap();
    let res = run_workflow(opts_for(wf, &tmp, sink.clone()))
        .await
        .unwrap();

    let route = step(&res, "route");
    assert_eq!(route.output, "else");

    // Falsy condition: arm_b (the `else` arm) runs, arm_a (`then`) is skipped.
    let arm_a = step(&res, "arm_a");
    assert!(arm_a.skipped, "not-taken arm_a should be skipped");
    assert_eq!(arm_a.output, "");
    let arm_b = step(&res, "arm_b");
    assert!(!arm_b.skipped, "taken arm_b should run");
    assert!(arm_b.success);

    // join reconverges seeing arm_a empty and arm_b's real output.
    let join = step(&res, "join");
    assert!(!join.skipped);
    assert!(
        join.rendered_prompt.contains("a=[]"),
        "join should see arm_a's empty output, got: {}",
        join.rendered_prompt
    );
    assert!(
        join.rendered_prompt.contains("b=[step arm_b agent ag echo:"),
        "join should see arm_b's real output, got: {}",
        join.rendered_prompt
    );

    let events = sink.events.lock().unwrap();
    let has_arm_a_skip = events.iter().any(|e| {
        matches!(e, Event::StepSkipped { step_id, reason, .. }
            if step_id == "arm_a" && reason.contains("not taken by branch"))
    });
    assert!(has_arm_a_skip, "arm_a should emit a branch StepSkipped");
}

// Regression: a run pauses AFTER a branch commits its arm (classify + route
// already done) but BEFORE the not-taken arm is reached. On resume the
// already-done branch step hits the resume-skip `continue` before the branch
// arm that populates `branch_skipped`. If the skip-set is not reconstructed
// from the branch's persisted result, the not-taken arm (`arm_b`, the `else`
// arm) is neither already-done nor branch-skipped, so it EXECUTES on resume —
// dispatching the agent the branch explicitly excluded. This proves the
// not-taken arm stays skipped after resume.
#[tokio::test]
async fn resume_after_branch_keeps_not_taken_arm_skipped() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    // Condition rendered "true" → the branch took the `then` arm (arm_a).
    let wf = Workflow::parse(&WF_BRANCH.replace("CONDITION", "true")).unwrap();

    // Simulate the prior process having completed `classify` and the `route`
    // branch step (which committed `output == "then"`), then paused before
    // reaching arm_a / arm_b. Only these two results are pre-seeded.
    let prior_step_results = vec![
        StepResult {
            step_id: "classify".into(),
            output: "classified".into(),
            success: true,
            skipped: false,
            kind: StepKind::Linear,
            ..Default::default()
        },
        StepResult {
            step_id: "route".into(),
            output: "then".into(),
            success: true,
            skipped: false,
            kind: StepKind::Branch,
            ..Default::default()
        },
    ];

    let mut opts = opts_for(wf, &tmp, sink.clone());
    opts.resume_from = Some(ResumeState {
        run_id: "run_resume_branch".into(),
        prior_step_results,
        approved_step_id: String::new(),
        completed_units: std::collections::BTreeMap::new(),
        reason: PauseReason::Manual,
        paused_step: None,
    });

    let res = run_workflow(opts).await.unwrap();

    // The taken arm (arm_a) still runs on resume.
    let arm_a = step(&res, "arm_a");
    assert!(!arm_a.skipped, "taken arm_a should run after resume");
    assert!(arm_a.success);

    // The not-taken `else` arm (arm_b) MUST stay skipped after resume — the
    // skip-set was reconstructed from `route`'s persisted `output == "then"`.
    let arm_b = step(&res, "arm_b");
    assert!(
        arm_b.skipped,
        "not-taken arm_b must remain skipped after resume, not execute"
    );
    assert_eq!(arm_b.output, "", "skipped arm has empty output");

    // join reconverges seeing arm_b's empty output.
    let join = step(&res, "join");
    assert!(!join.skipped, "join should run after the branch on resume");
    assert!(
        join.rendered_prompt.contains("b=[]"),
        "join should see arm_b's empty output, got: {}",
        join.rendered_prompt
    );

    // The StepSkipped event for arm_b carries the branch reason on resume.
    let events = sink.events.lock().unwrap();
    let has_arm_b_skip = events.iter().any(|e| {
        matches!(e, Event::StepSkipped { step_id, reason, .. }
            if step_id == "arm_b" && reason.contains("not taken by branch"))
    });
    assert!(
        has_arm_b_skip,
        "arm_b should emit a branch StepSkipped after resume"
    );
}
