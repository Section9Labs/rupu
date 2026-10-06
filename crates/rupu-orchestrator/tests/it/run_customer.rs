//! A workflow run records the customer it ran under (customers Plan 2A,
//! Task 2): `run_workflow` copies `StepFactory::customer()` onto the fresh
//! `RunRecord`, and a `run.json` written before the field existed reads back
//! with no customer.

use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::{RunRecord, RunStore, Workflow};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

const WF: &str = r#"
name: customer-run
steps:
  - id: a
    agent: ag
    actions: []
    prompt: "hello"
"#;

/// One-turn mock factory. `customer: None` leaves `StepFactory::customer`
/// at its default (not overridden), so the default path is exercised too.
struct Factory {
    customer: Option<&'static str>,
}

#[async_trait]
impl StepFactory for Factory {
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
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        AgentRunOpts {
            seed_source: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
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
            recovery: Default::default(),
        }
    }

    fn customer(&self) -> Option<&str> {
        self.customer
    }
}

/// The default `StepFactory::customer` (not overridden) — a factory that
/// predates customers.
struct DefaultFactory(Factory);

#[async_trait]
impl StepFactory for DefaultFactory {
    #[allow(clippy::too_many_arguments)]
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
        self.0
            .build_opts_for_step(
                step_id,
                agent_name,
                rendered_prompt,
                run_id,
                workspace_id,
                workspace_path,
                transcript_path,
                on_tool_call,
            )
            .await
    }
}

async fn run_with(factory: Arc<dyn StepFactory>) -> (assert_fs::TempDir, Arc<RunStore>, String) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let store = Arc::new(RunStore::new(tmp.path().join("runs")));
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(WF).unwrap(),
        inputs: std::collections::BTreeMap::new(),
        workspace_id: "ws_customer".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory,
        event: None,
        run_store: Some(Arc::clone(&store)),
        workflow_yaml: Some(WF.to_string()),
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
    assert!(!res.run_id.is_empty());
    (tmp, store, res.run_id)
}

#[tokio::test]
async fn run_records_the_factory_customer() {
    let (_tmp, store, run_id) = run_with(Arc::new(Factory {
        customer: Some("acme"),
    }))
    .await;
    let record = store.load(&run_id).unwrap();
    assert_eq!(record.customer.as_deref(), Some("acme"));
}

#[tokio::test]
async fn run_with_a_factory_that_names_no_customer_records_none() {
    let (_tmp, store, run_id) =
        run_with(Arc::new(DefaultFactory(Factory { customer: None }))).await;
    let record = store.load(&run_id).unwrap();
    assert_eq!(record.customer, None);
    // `None` is not serialized: run.json stays byte-compatible with
    // readers that predate the field.
    let raw = std::fs::read_to_string(store.run_json_path(&run_id)).unwrap();
    assert!(!raw.contains("\"customer\""), "run.json: {raw}");
}

#[tokio::test]
async fn a_run_json_without_the_customer_key_reads_as_none() {
    let (_tmp, store, run_id) = run_with(Arc::new(Factory {
        customer: Some("acme"),
    }))
    .await;
    let path = store.run_json_path(&run_id);
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value["customer"], "acme");
    value.as_object_mut().unwrap().remove("customer");
    let legacy: RunRecord = serde_json::from_value(value).unwrap();
    assert_eq!(legacy.customer, None);
}
