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
            seed_source: None,
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
            on_usage: None,
            concerns: None,
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            scope_name: None,
            surface_tag: None,
            pause: None,
            codename: None,
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
        run_step: Default::default(),
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
        action_dispatcher: None,
        pause: None,
        naming: None,
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
    // RunStarted,
    // StepStarted(alpha), StepWorking(alpha, transcript_path), AgentStarted(alpha), StepCompleted(alpha),
    // StepStarted(beta),  StepWorking(beta, transcript_path),  AgentStarted(beta),  StepCompleted(beta),
    // RunCompleted.
    // Each linear step emits a StepWorking carrying its (lazily-generated)
    // transcript path so the live UI can tail the file while the step runs,
    // then an AgentStarted announcing the agent instance (codename, agent,
    // provider, model) the moment its opts are built.
    assert_eq!(
        events.len(),
        10,
        "expected 10 events for a two-step run, got {:?}",
        events.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>()
    );

    // Verify ordering: StepStarted → StepWorking(path) → AgentStarted → StepCompleted per step.
    assert!(matches!(events[1], Event::StepStarted { step_id: ref s, .. } if s == "alpha"));
    assert!(
        matches!(events[2], Event::StepWorking { step_id: ref s, transcript_path: Some(_), .. } if s == "alpha"),
        "alpha StepWorking must carry a transcript_path, got {:?}",
        events[2]
    );
    assert!(
        matches!(events[3], Event::AgentStarted { step_id: ref s, .. } if s == "alpha"),
        "alpha AgentStarted must follow StepWorking, got {:?}",
        events[3]
    );
    assert!(
        matches!(events[4], Event::StepCompleted { step_id: ref s, success: true, .. } if s == "alpha")
    );
    assert!(matches!(events[5], Event::StepStarted { step_id: ref s, .. } if s == "beta"));
    assert!(
        matches!(events[6], Event::StepWorking { step_id: ref s, transcript_path: Some(_), .. } if s == "beta")
    );
    assert!(matches!(events[7], Event::AgentStarted { step_id: ref s, .. } if s == "beta"));
    assert!(
        matches!(events[8], Event::StepCompleted { step_id: ref s, success: true, .. } if s == "beta")
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
        run_step: Default::default(),
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
        action_dispatcher: None,
        pause: None,
        naming: None,
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
        run_step: Default::default(),
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
        action_dispatcher: None,
        pause: None,
        naming: None,
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

/// Panelists emit a high-severity finding (so a gate never clears); the
/// fixer (`fixer-bot`) echoes its prompt.
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
            seed_source: None,
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
            on_usage: None,
            concerns: None,
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            scope_name: None,
            surface_tag: None,
            pause: None,
            codename: None,
        }
    }
}

/// A `parallel:` step's declared sub-steps are units of it, exactly like a
/// `for_each` item or a panelist: one `UnitStarted` / `UnitCompleted` pair per
/// sub-step, `index` = its declared position and `unit_key` = its sub-step id.
/// Without them a live view has no record of the sub-steps and shows them
/// un-started under a finished parent.
#[tokio::test]
async fn parallel_emits_per_sub_step_unit_events() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    // A run store makes the usage hook real, so the completion carries tokens.
    let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));

    let wf_yaml = r#"
name: parallel-units
steps:
  - id: lanes
    parallel:
      - id: spec
        agent: writer
        prompt: "write the spec"
      - id: verify
        agent: reviewer
        prompt: "review it"
"#;

    let wf = Workflow::parse(wf_yaml).unwrap();
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_parallel".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(store.clone()),
        workflow_yaml: Some(wf_yaml.into()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: Some("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W".into()),
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };

    run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
    let started: Vec<(usize, String, Option<String>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::UnitStarted {
                step_id,
                index,
                unit_key,
                agent,
                ..
            } if step_id == "lanes" => Some((*index, unit_key.clone(), agent.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![
            (0, "spec".to_string(), Some("writer".to_string())),
            (1, "verify".to_string(), Some("reviewer".to_string())),
        ],
        "one UnitStarted per declared sub-step, in declared order"
    );

    let completed: Vec<(usize, String, bool, u64, u64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::UnitCompleted {
                step_id,
                index,
                unit_key,
                success,
                tokens_in,
                tokens_out,
                ..
            } if step_id == "lanes" => {
                Some((*index, unit_key.clone(), *success, *tokens_in, *tokens_out))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        completed,
        vec![
            (0, "spec".to_string(), true, 1, 1),
            (1, "verify".to_string(), true, 1, 1),
        ],
        "one UnitCompleted per sub-step carrying its real usage totals"
    );

    // Each unit starts before it completes, and the per-unit AgentStarted
    // (which `RunView` can only attach to an existing unit) comes after the
    // unit was announced.
    for idx in [0usize, 1] {
        let pos = |want: &dyn Fn(&Event) -> bool| events.iter().position(want).unwrap();
        let unit_started = pos(
            &|e| matches!(e, Event::UnitStarted { step_id, index, .. } if step_id == "lanes" && *index == idx),
        );
        let agent_started = pos(
            &|e| matches!(e, Event::AgentStarted { step_id, unit_index: Some(i), .. } if step_id == "lanes" && *i == idx),
        );
        let unit_completed = pos(
            &|e| matches!(e, Event::UnitCompleted { step_id, index, .. } if step_id == "lanes" && *index == idx),
        );
        assert!(
            unit_started < agent_started && agent_started < unit_completed,
            "unit {idx}: UnitStarted({unit_started}) < AgentStarted({agent_started}) < UnitCompleted({unit_completed})"
        );
    }
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
        run_step: Default::default(),
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
        action_dispatcher: None,
        pause: None,
        naming: None,
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
            Event::PanelRound {
                step_id,
                max_iterations,
                ..
            } if step_id == "scan" => Some(*max_iterations),
            _ => None,
        })
        .collect();
    assert_eq!(
        max_iter,
        vec![2, 2],
        "max_iterations must be 2 for both rounds"
    );
}

#[tokio::test]
async fn no_event_sink_does_not_emit_any_events() {
    // Smoke test: running without an event_sink should not panic and
    // should still return correct results.
    let tmp = assert_fs::TempDir::new().unwrap();
    let wf = Workflow::parse(WF_TWO_STEPS).unwrap();
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
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
        action_dispatcher: None,
        pause: None,
        naming: None,
    };

    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
}

#[tokio::test]
async fn every_agent_instance_is_announced_with_codename_provider_and_model() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    let wf = Workflow::parse(WF_TWO_STEPS).unwrap();
    // `run_workflow` only honours `run_id_override` when a run store is
    // attached (an in-memory run has an empty run id), so pin the crew
    // (`jade-reef`) through a temp store.
    let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: wf,
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_names".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(FakeFactory),
        event: None,
        run_store: Some(store.clone()),
        workflow_yaml: Some(WF_TWO_STEPS.into()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: Some("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W".into()),
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    let res = run_workflow(opts).await.unwrap();

    let events = sink.events.lock().unwrap();
    let started: Vec<(String, String, String, String, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::AgentStarted {
                step_id,
                codename,
                agent,
                provider,
                model,
                ..
            } => Some((
                step_id.clone(),
                codename.clone().unwrap(),
                agent.clone(),
                provider.clone().unwrap(),
                model.clone().unwrap(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        started,
        vec![
            (
                "alpha".into(),
                "jade-reef/hedgehog".into(),
                "ag".into(),
                "mock".into(),
                "mock-1".into()
            ),
            (
                "beta".into(),
                "jade-reef/heron".into(),
                "ag".into(),
                "mock".into(),
                "mock-1".into()
            ),
        ]
    );
    let step_names: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::StepStarted { codename, .. } => codename.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(step_names, vec!["jade-reef/hedgehog", "jade-reef/heron"]);

    // The crew is persisted on the run record, and each step result carries
    // its instance codename.
    let rec = store.load("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W").unwrap();
    assert_eq!(rec.codename.as_deref(), Some("jade-reef"));
    let in_memory: Vec<_> = res
        .step_results
        .iter()
        .map(|r| (r.step_id.clone(), r.codename.clone()))
        .collect();
    let persisted: Vec<_> = store
        .read_step_results("run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W")
        .unwrap()
        .into_iter()
        .map(|r| (r.step_id, r.codename))
        .collect();
    let expected = vec![
        ("alpha".to_string(), Some("jade-reef/hedgehog".to_string())),
        ("beta".to_string(), Some("jade-reef/heron".to_string())),
    ];
    assert_eq!(in_memory, expected);
    assert_eq!(persisted, expected);
}

/// Spec §3/§4.3: a def repeated in a panel is numbered by occurrence
/// (`crew/<a>#1`, `crew/<a>#2`), a def appearing once is a singleton
/// (`crew/<b>`), the gate's fixer is its own static slot, and every gate
/// iteration re-runs the same slots under the same names.
#[tokio::test]
async fn panel_occurrences_and_fixer_slot_are_named() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let sink: Arc<CollectSink> = Arc::new(CollectSink::default());
    let wf_yaml = r#"
name: gate-panel-names
steps:
  - id: scan
    panel:
      subject: "check this"
      panelists:
        - reviewer-a
        - reviewer-b
        - reviewer-a
      gate:
        max_iterations: 2
        until_no_findings_at_severity_or_above: high
        fix_with: fixer-bot
"#;
    let store = Arc::new(rupu_orchestrator::RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(wf_yaml).unwrap(),
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_panel_names".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().to_path_buf(),
        factory: Arc::new(GatePanelFactory),
        event: None,
        run_store: Some(store.clone()),
        workflow_yaml: Some(wf_yaml.into()),
        resume_from: None,
        issue: None,
        issue_ref: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    let res = run_workflow(opts).await.unwrap();
    let crew = store.load(&res.run_id).unwrap().codename.expect("crew");

    let events = sink.events.lock().unwrap();
    let announced: Vec<(String, String)> = events
        .iter()
        .filter_map(|e| match e {
            Event::AgentStarted {
                step_id,
                agent,
                codename,
                ..
            } if step_id == "scan" => Some((agent.clone(), codename.clone().unwrap())),
            _ => None,
        })
        .collect();
    let names_of = |agent: &str| -> Vec<String> {
        announced
            .iter()
            .filter(|(a, _)| a == agent)
            .map(|(_, c)| c.clone())
            .collect()
    };
    let role = |c: &str| -> String {
        let parsed: rupu_codename::Codename = c.parse().unwrap();
        assert_eq!(parsed.crew, crew, "{c}");
        assert_eq!(parsed.segments.len(), 1, "{c}");
        parsed.segments[0].role.clone()
    };

    let a = names_of("reviewer-a");
    let b = names_of("reviewer-b");
    let fixer = names_of("fixer-bot");
    let role_a = role(&a[0]);
    let role_b = role(&b[0]);
    let role_f = role(&fixer[0]);
    // Two rounds: each slot runs twice under the same name.
    let mut a_sorted = a.clone();
    a_sorted.sort();
    assert_eq!(
        a_sorted,
        vec![
            format!("{crew}/{role_a}#1"),
            format!("{crew}/{role_a}#1"),
            format!("{crew}/{role_a}#2"),
            format!("{crew}/{role_a}#2"),
        ]
    );
    assert_eq!(
        b,
        vec![format!("{crew}/{role_b}"); 2],
        "singleton: no number"
    );
    assert!(
        !fixer.is_empty(),
        "the gate never clears, so the fixer runs"
    );
    assert!(
        fixer.iter().all(|c| *c == format!("{crew}/{role_f}")),
        "fixer is one static singleton slot across iterations: {fixer:?}"
    );
    assert!(
        role_a != role_b && role_a != role_f && role_b != role_f,
        "each slot gets its own role word: {role_a} {role_b} {role_f}"
    );

    // Persisted: panel items in panel order, fixer items named by the slot,
    // and the panel step record itself unnamed (instances live on items).
    let rec = &res.step_results[0];
    assert_eq!(rec.codename, None);
    let panel_items: Vec<_> = rec
        .items
        .iter()
        .filter(|i| !i.is_fixer)
        .map(|i| i.codename.clone().unwrap())
        .collect();
    assert_eq!(
        panel_items,
        vec![
            format!("{crew}/{role_a}#1"),
            format!("{crew}/{role_b}"),
            format!("{crew}/{role_a}#2"),
        ]
    );
    let fixer_items: Vec<_> = rec
        .items
        .iter()
        .filter(|i| i.is_fixer)
        .map(|i| i.codename.clone().unwrap())
        .collect();
    assert!(!fixer_items.is_empty(), "fixer runs are recorded");
    assert!(
        fixer_items.iter().all(|c| *c == format!("{crew}/{role_f}")),
        "{fixer_items:?}"
    );
}
