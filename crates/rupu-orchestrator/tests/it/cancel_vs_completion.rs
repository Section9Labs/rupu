//! A cancel that lands on disk while the run is finishing must win.
//!
//! `RunStore::cancel` (another process: the CLI, `cp serve`) signals the
//! runner and writes `Cancelled`. The runner's own terminal flip, racing it,
//! used to write `Completed`/`Failed` unconditionally over that — the run
//! showed as completed although it had been cancelled. Here the cancel
//! lands from inside the step's provider call, i.e. strictly before the
//! terminal flip, and the record must still say `Cancelled` afterwards.

use async_trait::async_trait;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, RunWorkflowError, StepFactory};
use rupu_orchestrator::{RunStatus, RunStore, Workflow};
use rupu_providers::types::{ContentBlock, LlmRequest, LlmResponse, Stop, StopReason, Usage};
use rupu_providers::{LlmProvider, ProviderError, StreamEvent};
use std::collections::BTreeMap;
use std::sync::Arc;

const WF: &str = r#"
name: cancel-race
steps:
  - id: only
    agent: ag
    actions: []
    prompt: "do the thing"
"#;

const RUN_ID: &str = "run_cancel_vs_completion";

/// Answers one turn — after cancelling the run on disk, as another process
/// would at that very moment.
struct CancellingProvider {
    store: Arc<RunStore>,
}

#[async_trait]
impl LlmProvider for CancellingProvider {
    async fn send(&mut self, _req: &LlmRequest) -> Result<LlmResponse, ProviderError> {
        let outcome = self
            .store
            .cancel(RUN_ID, "another-process", "stop it", chrono::Utc::now())
            .expect("the run is running, so it can be cancelled");
        assert!(
            matches!(
                outcome,
                rupu_orchestrator::runs::CancelOutcome::MarkedCancelled { .. }
            ),
            "{outcome:?}"
        );
        Ok(LlmResponse {
            id: "mock".into(),
            model: "mock-1".into(),
            content: vec![ContentBlock::Text {
                text: "done anyway".into(),
            }],
            stop: Stop::synthetic(StopReason::EndTurn, "mock"),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                ..Default::default()
            },
        })
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, ProviderError> {
        self.send(req).await
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        rupu_providers::ProviderId::Anthropic
    }
}

struct Factory {
    store: Arc<RunStore>,
}

#[async_trait]
impl StepFactory for Factory {
    async fn launch_for_step(
        &self,
        request: rupu_orchestrator::StepRequest,
    ) -> Result<rupu_orchestrator::StepLaunch, rupu_runtime::assembly::AssembleError> {
        let agent_name = request.agent_name.clone();
        let agent_name = agent_name.as_str();
        rupu_orchestrator::testing::MockRun {
            step_actions: Vec::new(),
            agent_name: format!("ag-{agent_name}"),
            agent_system_prompt: "echo".into(),
            agent_tools: None,
            provider: Box::new(CancellingProvider {
                store: Arc::clone(&self.store),
            }),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            max_turns: 5,
            no_stream: false,
            suppress_stream_stdout: true,
            dispatchable_agents: None,
            concerns: None,
            limits: rupu_providers::model_limits::ModelLimits::unknown(),
            ..Default::default()
        }
        .launch(request)
    }
}

#[tokio::test]
async fn a_cancel_that_lands_while_the_run_finishes_is_not_overwritten() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(WF).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_cancel_race".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(Factory {
            store: Arc::clone(&store),
        }),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF.to_string()),
        resume_from: None,
        run_id_override: Some(RUN_ID.to_string()),
        strict_templates: false,
        event_sink: None,
        unit_dispatcher: None,
        action_dispatcher: None,
        pause: None,
        naming: None,
    };
    // The step itself ran to completion; what the record says is the point —
    // and the outcome handed back must say the same, or the caller reports
    // a cancelled run as completed.
    let outcome = run_workflow(opts).await;
    match outcome {
        Err(RunWorkflowError::RunCancelled { aborted: 0 }) => {}
        other => panic!("a preserved cancel is returned as cancelled, got {other:?}"),
    }

    let record = store.load(RUN_ID).expect("run record");
    assert_eq!(
        record.status,
        RunStatus::Cancelled,
        "the on-disk cancel survives the runner's terminal flip"
    );
    assert_eq!(record.error_message.as_deref(), Some("stop it"));
}
