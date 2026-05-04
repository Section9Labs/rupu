use async_trait::async_trait;
use rupu_agent::runner::{BypassDecider, MockProvider, ScriptedTurn};
use rupu_agent::AgentRunOpts;
use rupu_orchestrator::runner::{run_workflow, OrchestratorRunOpts, StepFactory};
use rupu_orchestrator::Workflow;
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

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
    };
    let res = run_workflow(opts).await.unwrap();
    assert_eq!(res.step_results.len(), 2);
    let b_prompt = &res.step_results[1].rendered_prompt;
    assert!(
        b_prompt.contains("step a echo: First step says: hello A"),
        "step b should see step a's output, got: {b_prompt}"
    );
}
