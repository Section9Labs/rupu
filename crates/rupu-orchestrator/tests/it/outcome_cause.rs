//! A failed agent's typed cause (`rupu_transcript::OutcomeRecord`) reaches
//! the orchestrator's events and persisted records (response-outcomes
//! Plan 2, Task 11): `StepFailed.cause`, `RunRecord.cause`,
//! `StepResultRecord.{error,cause}` under `continue_on_error`, and the
//! failing fan-out unit's `ItemResultRecord.{error,cause}`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{
    run_reject_cleanup, run_workflow, OrchestratorRunOpts, ResumeState, StepFactory,
};
use rupu_orchestrator::{ApprovalDecision, RunStatus, RunStore, StepResult, Workflow};
use rupu_providers::types::{ContentBlock, Stop, StopReason, Usage};
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

/// Refuses (with no fallback chain to recover on) whenever the rendered
/// prompt contains `REFUSE`; answers normally otherwise.
struct RefusingFactory;

fn refusal_turn() -> ScriptedTurn {
    let mut stop = Stop::synthetic(StopReason::Refusal, "mock");
    stop.refusal = None;
    ScriptedTurn::Reply {
        content: vec![ContentBlock::Text {
            text: "partial".into(),
        }],
        stop,
        usage: Usage::default(),
    }
}

#[async_trait]
impl StepFactory for RefusingFactory {
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
        let turns = if rendered_prompt.contains("INTERIM") {
            // An interim message plus a tool call, then a refusal on the
            // final turn: the interim text is not the step's answer.
            vec![
                ScriptedTurn::AssistantToolUse {
                    text: Some("Let me look at the settings first".into()),
                    tool_id: "c1".into(),
                    tool_name: "read_file".into(),
                    tool_input: serde_json::json!({ "path": "absent-settings.toml" }),
                    stop: StopReason::ToolUse,
                },
                refusal_turn(),
            ]
        } else if rendered_prompt.contains("REFUSE") {
            vec![refusal_turn()]
        } else {
            vec![ScriptedTurn::AssistantText {
                text: format!("done: {rendered_prompt}"),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]
        };
        AgentRunOpts {
            seed_source: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            agent_name: agent_name.to_string(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider: Box::new(MockProvider::new(turns)),
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
            recovery: Default::default(),
        }
    }
}

fn opts(
    tmp: &tempfile::TempDir,
    store: &Arc<RunStore>,
    sink: &Arc<CollectSink>,
    yaml: &str,
    run_id: &str,
) -> OrchestratorRunOpts {
    OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_cause".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(RefusingFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(store)),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: Some(run_id.to_string()),
        strict_templates: false,
        event_sink: Some(sink.clone() as Arc<dyn EventSink>),
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    }
}

const WF_REFUSES: &str = r#"
name: refuses
steps:
  - id: review
    agent: ag
    actions: []
    prompt: "REFUSE this"
"#;

#[tokio::test]
async fn a_refused_linear_step_records_its_cause_on_the_run_and_the_step_failed_event() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let sink = Arc::new(CollectSink::default());
    let run_id = "run_cause_linear";

    let err = run_workflow(opts(&tmp, &store, &sink, WF_REFUSES, run_id))
        .await
        .expect_err("a refusal with no chain fails the run");
    assert!(err.outcome().is_some_and(|o| o.class == "refusal"), "{err}");

    let rec = store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Failed);
    assert_eq!(
        rec.cause.as_ref().map(|c| c.class.as_str()),
        Some("refusal")
    );
    assert!(rec.error_message.is_some());

    let events = sink.events.lock().unwrap();
    let cause = events.iter().find_map(|e| match e {
        Event::StepFailed { step_id, cause, .. } if step_id == "review" => Some(cause.clone()),
        _ => None,
    });
    assert_eq!(
        cause.flatten().map(|c| c.class),
        Some("refusal".to_string()),
        "events: {events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, Event::RunFailed { .. })));
}

const WF_REFUSES_TOLERATED: &str = r#"
name: refuses-tolerated
steps:
  - id: review
    agent: ag
    actions: []
    continue_on_error: true
    prompt: "REFUSE this"
  - id: after
    agent: ag
    actions: []
    prompt: "carry on"
"#;

#[tokio::test]
async fn a_tolerated_refusal_persists_the_step_error_and_cause_and_the_run_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let sink = Arc::new(CollectSink::default());
    let run_id = "run_cause_tolerated";

    let res = run_workflow(opts(&tmp, &store, &sink, WF_REFUSES_TOLERATED, run_id))
        .await
        .expect("continue_on_error tolerates the refusal");
    assert!(!res.step_results[0].success);
    assert_eq!(
        res.step_results[0].cause.as_ref().map(|c| c.class.as_str()),
        Some("refusal")
    );

    let rec = store.load(run_id).unwrap();
    assert_eq!(rec.status, RunStatus::Completed);
    assert_eq!(rec.cause, None);

    let steps = store.read_step_results(run_id).unwrap();
    let review = steps.iter().find(|s| s.step_id == "review").unwrap();
    assert!(!review.success);
    assert!(review.error.is_some(), "{review:?}");
    assert_eq!(
        review.cause.as_ref().map(|c| c.class.as_str()),
        Some("refusal")
    );
    let after = steps.iter().find(|s| s.step_id == "after").unwrap();
    assert!(after.success);
    assert_eq!(after.error, None);
    assert_eq!(after.cause, None);
}

const WF_INTERIM_THEN_REFUSED: &str = r#"
name: interim-then-refused
steps:
  - id: review
    agent: ag
    actions: []
    continue_on_error: true
    prompt: "INTERIM REFUSE this"
  - id: after
    agent: ag
    actions: []
    prompt: "output=[{{ steps.review.output }}] error=[{{ steps.review.error }}]"
"#;

/// Spec 2026-10-01 §5.3: a failed step's output is empty, not the interim
/// text of an earlier turn; the reason reaches a downstream template as
/// `steps.<id>.error`.
#[tokio::test]
async fn a_tolerated_refusal_after_an_interim_turn_publishes_empty_output_and_its_error() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let sink = Arc::new(CollectSink::default());

    let res = run_workflow(opts(
        &tmp,
        &store,
        &sink,
        WF_INTERIM_THEN_REFUSED,
        "run_cause_interim",
    ))
    .await
    .expect("continue_on_error tolerates the refusal");
    let review = &res.step_results[0];
    assert!(!review.success);
    assert_eq!(review.output, "", "no stale interim text as the answer");
    let error = review.error.clone().expect("the step records its error");
    assert!(!error.is_empty());

    let after = &res.step_results[1];
    assert_eq!(
        after.rendered_prompt,
        format!("output=[] error=[{error}]"),
        "the downstream render sees an empty output and the error"
    );
    assert!(!after.rendered_prompt.contains("settings first"));
}

/// A succeeding step binds `steps.<id>.error` as an empty string.
#[tokio::test]
async fn a_succeeding_step_binds_an_empty_error() {
    const WF: &str = r#"
name: fine
steps:
  - id: review
    agent: ag
    actions: []
    prompt: "look"
  - id: after
    agent: ag
    actions: []
    prompt: "error=[{{ steps.review.error }}]"
"#;
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let sink = Arc::new(CollectSink::default());
    let res = run_workflow(opts(&tmp, &store, &sink, WF, "run_cause_fine"))
        .await
        .expect("both steps answer");
    assert_eq!(res.step_results[1].rendered_prompt, "error=[]");
}

const WF_FANOUT: &str = r#"
name: fanout-refuses
steps:
  - id: each
    agent: ag
    actions: []
    continue_on_error: true
    for_each: |
      fine
      REFUSE
    prompt: "look at {{ item }}"
"#;

#[tokio::test]
async fn a_refused_fanout_unit_carries_the_cause_on_its_item_only() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let sink = Arc::new(CollectSink::default());
    let run_id = "run_cause_fanout";

    run_workflow(opts(&tmp, &store, &sink, WF_FANOUT, run_id))
        .await
        .expect("continue_on_error tolerates the failed unit");

    let steps = store.read_step_results(run_id).unwrap();
    let each = steps.iter().find(|s| s.step_id == "each").unwrap();
    assert_eq!(each.items.len(), 2);
    let fine = &each.items[0];
    assert!(fine.success);
    assert_eq!(fine.error, None);
    assert_eq!(fine.cause, None);
    let refused = &each.items[1];
    assert!(!refused.success);
    assert!(refused.error.is_some(), "{refused:?}");
    assert_eq!(
        refused.cause.as_ref().map(|c| c.class.as_str()),
        Some("refusal")
    );

    let checkpoints = store.read_unit_checkpoints(run_id).unwrap();
    let cp = checkpoints
        .iter()
        .find(|c| c.step_id == "each" && c.index == 1)
        .unwrap();
    assert_eq!(cp.cause.as_ref().map(|c| c.class.as_str()), Some("refusal"));
    assert!(cp.error.is_some());

    let events = sink.events.lock().unwrap();
    let unit_causes: Vec<(usize, Option<String>)> = events
        .iter()
        .filter_map(|e| match e {
            Event::UnitCompleted { index, cause, .. } => {
                Some((*index, cause.as_ref().map(|c| c.class.clone())))
            }
            _ => None,
        })
        .collect();
    assert!(unit_causes.contains(&(0, None)), "{unit_causes:?}");
    assert!(
        unit_causes.contains(&(1, Some("refusal".to_string()))),
        "{unit_causes:?}"
    );
}

const WF_GATE_CLEANUP_REFUSES: &str = r#"
name: gate-cleanup-refuses
steps:
  - id: gate
    approval:
      prompt: "Approve?"
      on_reject:
        - id: notify_fail
          agent: ag
          prompt: "REFUSE the cleanup"
"#;

#[tokio::test]
async fn a_refused_on_reject_cleanup_step_records_its_error_and_cause() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let sink = Arc::new(CollectSink::default());
    let run_id = "run_cause_cleanup";

    let res = run_workflow(opts(&tmp, &store, &sink, WF_GATE_CLEANUP_REFUSES, run_id))
        .await
        .expect("parks at the gate");
    assert!(res.awaiting.is_some());

    let decision = store
        .reject(run_id, "operator", "not today", chrono::Utc::now())
        .expect("reject succeeds");
    let ApprovalDecision::Rejected {
        step_id, reason, ..
    } = decision
    else {
        panic!("expected Rejected, got {decision:?}");
    };
    let prior: Vec<StepResult> = store
        .read_step_results(run_id)
        .unwrap()
        .iter()
        .map(StepResult::from)
        .collect();
    let mut cleanup = opts(&tmp, &store, &sink, WF_GATE_CLEANUP_REFUSES, run_id);
    cleanup.run_id_override = None;
    cleanup.resume_from = Some(ResumeState::from_rejection(
        run_id.to_string(),
        prior,
        step_id.clone(),
        reason.clone(),
    ));
    run_reject_cleanup(cleanup, &step_id, &reason, "human", None)
        .await
        .expect("cleanup never errors");

    let steps = store.read_step_results(run_id).unwrap();
    let notify = steps.iter().find(|s| s.step_id == "notify_fail").unwrap();
    assert!(!notify.success);
    assert!(notify.error.is_some(), "{notify:?}");
    assert_eq!(
        notify.cause.as_ref().map(|c| c.class.as_str()),
        Some("refusal")
    );
}
