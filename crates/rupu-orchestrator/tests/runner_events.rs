//! Integration test: the runner emits Run/Step events at every transition.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
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

#[tokio::test]
async fn run_workflow_emits_run_and_step_events_in_order() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());

    let wf = Workflow::parse(WF_TWO_STEPS).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_events".into(),
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
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
    };

    run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
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

    // For a two-step linear workflow the expected sequence is:
    // RunStarted, StepStarted(alpha), StepCompleted(alpha),
    // StepStarted(beta), StepCompleted(beta), RunCompleted.
    assert_eq!(
        events.len(),
        6,
        "expected 6 events for a two-step run, got {:?}",
        events
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
    );

    // Verify ordering: StepStarted always precedes its StepCompleted.
    assert!(matches!(events[1], Event::StepStarted { step_id: ref s, .. } if s == "alpha"));
    assert!(matches!(events[2], Event::StepCompleted { step_id: ref s, success: true, .. } if s == "alpha"));
    assert!(matches!(events[3], Event::StepStarted { step_id: ref s, .. } if s == "beta"));
    assert!(matches!(events[4], Event::StepCompleted { step_id: ref s, success: true, .. } if s == "beta"));
}

#[tokio::test]
async fn skipped_step_emits_step_skipped_event() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());

    let wf_yaml = r#"
name: skip-test
steps:
  - id: always
    agent: ag
    actions: []
    prompt: "always runs"
  - id: never
    agent: ag
    actions: []
    when: "false"
    prompt: "never runs"
"#;

    let wf = Workflow::parse(wf_yaml).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_skip".into(),
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
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
    };

    run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
    // Should have: RunStarted, StepStarted(always), StepCompleted(always),
    //              StepSkipped(never), RunCompleted.
    assert!(matches!(events.first(), Some(Event::RunStarted { .. })));
    assert!(matches!(events.last(), Some(Event::RunCompleted { .. })));
    let has_skipped = events.iter().any(|e| {
        matches!(e, Event::StepSkipped { step_id, .. } if step_id == "never")
    });
    assert!(has_skipped, "expected StepSkipped for 'never' step, got: {events:?}");
}

#[tokio::test]
async fn no_event_sink_does_not_emit_any_events() {
    // Smoke test: running without an event_sink should not panic and
    // should still return correct results.
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_TWO_STEPS).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_no_sink".into(),
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
        event_sink: None,
    };

    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
}
