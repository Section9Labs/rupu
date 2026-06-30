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
                unit_dispatcher: None,
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
        events.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>()
    );

    // Verify ordering: StepStarted always precedes its StepCompleted.
    assert!(matches!(events[1], Event::StepStarted { step_id: ref s, .. } if s == "alpha"));
    assert!(
        matches!(events[2], Event::StepCompleted { step_id: ref s, success: true, .. } if s == "alpha")
    );
    assert!(matches!(events[3], Event::StepStarted { step_id: ref s, .. } if s == "beta"));
    assert!(
        matches!(events[4], Event::StepCompleted { step_id: ref s, success: true, .. } if s == "beta")
    );
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
                unit_dispatcher: None,
    };

    run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
    // Should have: RunStarted, StepStarted(always), StepCompleted(always),
    //              StepSkipped(never), RunCompleted.
    assert!(matches!(events.first(), Some(Event::RunStarted { .. })));
    assert!(matches!(events.last(), Some(Event::RunCompleted { .. })));
    let has_skipped = events
        .iter()
        .any(|e| matches!(e, Event::StepSkipped { step_id, .. } if step_id == "never"));
    assert!(
        has_skipped,
        "expected StepSkipped for 'never' step, got: {events:?}"
    );
}

#[tokio::test]
async fn panel_emits_per_panelist_unit_events() {
    // A gate-less panel with two panelists should emit one
    // UnitStarted/UnitCompleted pair per panelist, keyed
    // `iter1:<panelist>`, so the live view expands the sweep step like
    // a fan-out.
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());

    let wf_yaml = r#"
name: panel-units
steps:
  - id: sweep
    panel:
      subject: "review this"
      panelists:
        - reviewer-a
        - reviewer-b
"#;

    let wf = Workflow::parse(wf_yaml).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_panel".into(),
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
                unit_dispatcher: None,
    };

    run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
    let started: Vec<(usize, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::UnitStarted {
                step_id,
                index,
                unit_key,
                ..
            } if step_id == "sweep" => Some((*index, unit_key.clone())),
            _ => None,
        })
        .collect();
    let completed: Vec<(usize, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::UnitCompleted {
                step_id,
                index,
                unit_key,
                ..
            } if step_id == "sweep" => Some((*index, unit_key.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(
        started.len(),
        2,
        "two panelist UnitStarted events: {started:?}"
    );
    assert_eq!(
        completed.len(),
        2,
        "two panelist UnitCompleted events: {completed:?}"
    );
    // Monotonic indices 0,1 and iter1-prefixed keys.
    let mut indices: Vec<usize> = started.iter().map(|(i, _)| *i).collect();
    indices.sort_unstable();
    assert_eq!(indices, vec![0, 1], "indices grow monotonically");
    assert!(
        started.iter().any(|(_, k)| k == "iter1:reviewer-a"),
        "keyed iter1:reviewer-a: {started:?}"
    );
    assert!(
        started.iter().any(|(_, k)| k == "iter1:reviewer-b"),
        "keyed iter1:reviewer-b: {started:?}"
    );
}

#[tokio::test]
async fn panel_gate_emits_panel_round_events() {
    // A panel with a gate that cannot clear (panelist always emits a `high`
    // severity finding; threshold is `high` → `high < high` is false →
    // never clears) and max_iterations=2, so the loop runs exactly 2
    // rounds, emitting 2 PanelRound events with round=1 and round=2.
    let tmp = assert_fs::TempDir::new().unwrap();

    // CollectSink with scripted panelist that always returns a high-severity
    // finding so the gate never clears.
    #[derive(Default)]
    struct CollectSink2(Mutex<Vec<Event>>);
    impl EventSink for CollectSink2 {
        fn emit(&self, _run_id: &str, ev: &Event) {
            self.0.lock().unwrap().push(ev.clone());
        }
    }

    struct GatePanelFactory;
    #[async_trait::async_trait]
    impl StepFactory for GatePanelFactory {
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
            // Panelists emit a high-severity finding; fixer echoes the prompt.
            let text = if agent_name == "fixer-bot" {
                format!("fixed: {rendered_prompt}")
            } else {
                r#"{"findings":[{"severity":"high","title":"oops","body":"details"}]}"#.to_string()
            };
            let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
                text,
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]);
            AgentRunOpts {
                agent_name: format!("ag-{agent_name}"),
                agent_system_prompt: "panel".into(),
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
            }
        }
    }

    let wf_yaml = r#"
name: gate-panel
steps:
  - id: scan
    panel:
      subject: "check this"
      panelists:
        - reviewer-a
      gate:
        max_iterations: 2
        until_no_findings_at_severity_or_above: high
        fix_with: fixer-bot
"#;

    let sink: Arc<CollectSink2> = Arc::new(CollectSink2::default());
    let wf = Workflow::parse(wf_yaml).unwrap();
    let opts = OrchestratorRunOpts {
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_gate".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(GatePanelFactory),
        event: None,
        run_store: None,
        workflow_yaml: None,
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
                unit_dispatcher: None,
    };

    run_workflow(opts).await.unwrap();

    let events = sink.0.lock().unwrap();
    let rounds: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            Event::PanelRound { step_id, round, .. } if step_id == "scan" => Some(*round),
            _ => None,
        })
        .collect();
    assert_eq!(
        rounds,
        vec![1, 2],
        "expected PanelRound events with round=1 and round=2, got: {events:?}"
    );
    // Verify max_iterations is correctly threaded through.
    let max_iter: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            Event::PanelRound { step_id, max_iterations, .. } if step_id == "scan" => {
                Some(*max_iterations)
            }
            _ => None,
        })
        .collect();
    assert_eq!(max_iter, vec![2, 2], "max_iterations must be 2 for both rounds");
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
                unit_dispatcher: None,
    };

    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
}
