//! `AppStepFactory` — minimal D-3 smoke factory for the native app.
//!
//! Production-quality provider wiring (keychain, OAuth, SCM registry)
//! lives in `DefaultStepFactory` in `rupu-cli`. For D-3 the app needs a
//! `StepFactory` that satisfies the trait contract so `AppExecutor::new`
//! can be constructed; real provider wiring follows in a later slice
//! when the app gains its own credential flow.
//!
//! For now `AppStepFactory` builds `AgentRunOpts` with a `MockProvider`
//! that immediately returns an empty "no-op" assistant text. Any active
//! run the app attaches to will show events from `events.jsonl` — the
//! factory is only invoked when `start_workflow` is called from within
//! the app itself (not yet exposed in the UI), so this never fires in
//! normal D-3 use.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rupu_agent::{
    AgentRunOpts, BypassDecider, MockProvider, OnToolCallCallback, ScriptedTurn, StopReason,
};
use rupu_orchestrator::runner::StepFactory;
use rupu_tools::ToolContext;

/// Minimal `StepFactory` for the app. Uses `MockProvider` so no
/// credential infrastructure is needed. Suitable for D-3 smoke-testing
/// where the app attaches to runs started externally by `rupu workflow
/// run` rather than starting its own.
pub struct AppStepFactory {
    pub workspace_path: PathBuf,
}

#[async_trait]
impl StepFactory for AppStepFactory {
    async fn build_opts_for_step(
        &self,
        step_id: &str,
        agent_name: &str,
        rendered_prompt: String,
        run_id: String,
        workspace_id: String,
        workspace_path: PathBuf,
        transcript_path: PathBuf,
        on_tool_call: Option<OnToolCallCallback>,
    ) -> AgentRunOpts {
        // No-op mock: one turn that immediately returns empty text.
        let provider = Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: String::new(),
            stop: StopReason::EndTurn,
            input_tokens: 0,
            output_tokens: 0,
        }]));

        AgentRunOpts {
            agent_name: agent_name.to_string(),
            agent_system_prompt: rendered_prompt.clone(),
            agent_tools: None,
            provider,
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: 1,
            decider: Arc::new(BypassDecider),
            tool_context: ToolContext {
                workspace_path,
                bash_env_allowlist: Vec::new(),
                bash_timeout_secs: 120,
                dispatcher: None,
                dispatchable_agents: None,
                parent_run_id: None,
                depth: 0,
            },
            user_message: rendered_prompt,
            mode_str: "bypass".into(),
            no_stream: true,
            suppress_stream_stdout: true,
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
