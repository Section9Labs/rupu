//! Default [`StepFactory`] implementation that wires real providers.
//!
//! `DefaultStepFactory` resolves each step's `agent:` field against
//! the project- and global-scope `agents/` dirs and constructs a real
//! provider via [`rupu_runtime::provider_factory::build_for_provider`].
//!
//! `mcp_registry` is built once in the `run` function and shared
//! across all steps; this avoids redundant credential probes and
//! ensures consistent SCM tool availability throughout the workflow.

use crate::runner::StepFactory;
use crate::workflow::Workflow;
use async_trait::async_trait;
use rupu_agent::{
    runner::BypassDecider, runner::PermissionDecider, AgentRunOpts, OnToolCallCallback,
};
use rupu_runtime::provider_factory;
use rupu_tools::{AgentDispatcher, ToolContext};
use std::path::PathBuf;
use std::sync::Arc;

/// `StepFactory` impl that resolves each step's `agent:` against
/// the project- and global-scope `agents/` dirs and constructs a
/// real provider via [`rupu_runtime::provider_factory::build_for_provider`].
///
/// `mcp_registry` is built once in the `run` function and shared
/// across all steps; this avoids redundant credential probes and
/// ensures consistent SCM tool availability throughout the workflow.
pub struct DefaultStepFactory {
    pub workflow: Workflow,
    pub global: PathBuf,
    pub project_root: Option<PathBuf>,
    pub resolver: Arc<rupu_auth::KeychainResolver>,
    pub mode_str: String,
    pub mcp_registry: Arc<rupu_scm::Registry>,
    /// Formatted `## Run target` text to append to each step's system prompt.
    /// `None` when no `--target` was supplied at workflow invocation.
    pub system_prompt_suffix: Option<String>,
    /// Sub-agent dispatcher wired into every step's `ToolContext`.
    /// `None` if the caller didn't construct one (no behavior change
    /// from pre-dispatch builds; `dispatch_agent` calls fail with
    /// "no dispatcher" in that case). The orchestrator constructs
    /// this alongside the factory so it has access to the same
    /// run_store + factory + agent loader.
    pub dispatcher: Option<Arc<dyn AgentDispatcher>>,
}

#[async_trait]
impl StepFactory for DefaultStepFactory {
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
        // We still verify the parent step exists in the workflow so
        // unknown step ids surface clearly, but we drive the agent
        // load off `agent_name` (which differs from the parent's
        // `agent:` for `parallel:` sub-steps).
        let _step = self
            .workflow
            .steps
            .iter()
            .find(|s| s.id == step_id)
            .expect("step_id from orchestrator must match a workflow step");

        // The agent loader takes the parent of `agents/`. For the
        // project layer that's `<project>/.rupu`; the global layer is
        // `<global>` directly (which already contains `agents/`).
        let project_agents_parent = self.project_root.as_ref().map(|p| p.join(".rupu"));
        let spec =
            rupu_agent::load_agent(&self.global, project_agents_parent.as_deref(), agent_name)
                .unwrap_or_else(|_| {
                    // Fallback: synthesize a minimal AgentSpec with the
                    // rendered prompt as system prompt so the factory contract
                    // is honored even when the agent file is missing. The
                    // agent loop will surface the failure via run_complete{
                    // status: Error}.
                    rupu_agent::AgentSpec {
                        name: agent_name.to_string(),
                        description: None,
                        provider: Some("anthropic".to_string()),
                        model: Some("claude-sonnet-4-6".to_string()),
                        auth: None,
                        tools: None,
                        max_turns: Some(50),
                        permission_mode: Some(self.mode_str.clone()),
                        anthropic_oauth_prefix: None,
                        effort: None,
                        context_window: None,
                        output_format: None,
                        anthropic_task_budget: None,
                        anthropic_context_management: None,
                        anthropic_speed: None,
                        dispatchable_agents: None,
                        system_prompt: rendered_prompt.clone(),
                    }
                });

        let provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
        let model = spec
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-6".into());
        let auth_hint = spec.auth;
        // Build the provider; on failure (missing credential, bad
        // auth config, etc.) substitute a stub provider that returns
        // the same error on first call. The runner's existing
        // `RunComplete { status: Error }` path then surfaces it as a
        // clean `✗ <step_id>` line via the line printer — no panic,
        // no crash log. (See `ProviderBuildErrorStub` below.)
        let provider: Box<dyn rupu_providers::LlmProvider> =
            match provider_factory::build_for_provider(
                &provider_name,
                &model,
                auth_hint,
                self.resolver.as_ref(),
            )
            .await
            {
                Ok((_resolved_auth, p)) => p,
                Err(e) => Box::new(provider_build_error_stub(
                    provider_name.clone(),
                    model.clone(),
                    e.to_string(),
                )),
            };

        let agent_system_prompt = match self.system_prompt_suffix.as_deref() {
            Some(suffix) => format!("{}\n\n## Run target\n\n{}", spec.system_prompt, suffix),
            None => spec.system_prompt,
        };

        // Precompute the parent_run_id clone before moving `run_id`
        // into the struct literal (otherwise the borrow-checker
        // flags it because struct-literal field-init order is the
        // *source* order: `run_id` moves before `tool_context` is
        // constructed).
        let parent_run_id_for_tool_ctx = Some(run_id.clone());

        AgentRunOpts {
            agent_name: spec.name,
            agent_system_prompt,
            agent_tools: spec.tools,
            provider,
            provider_name,
            model,
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: spec.max_turns.unwrap_or(50),
            decider: Arc::new(BypassDecider) as Arc<dyn PermissionDecider>,
            tool_context: ToolContext {
                workspace_path,
                bash_env_allowlist: Vec::new(),
                bash_timeout_secs: 120,
                // Sub-agent dispatch wiring. The dispatcher is set on
                // the factory by the workflow runner before
                // `run_workflow` starts; the per-step ToolContext
                // gets the dispatcher Arc plus the agent's declared
                // allowlist + parent run id so the `dispatch_agent`
                // tool can enforce both gates.
                dispatcher: self.dispatcher.clone(),
                dispatchable_agents: spec.dispatchable_agents.clone(),
                parent_run_id: parent_run_id_for_tool_ctx,
                depth: 0,
            },
            user_message: rendered_prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: self.mode_str.clone(),
            no_stream: false,
            // Workflow runs stream through the workflow printer by
            // tailing JSONL transcripts. Suppress direct stdout
            // writes here so they don't corrupt the live view.
            suppress_stream_stdout: true,
            mcp_registry: Some(Arc::clone(&self.mcp_registry)),
            effort: spec.effort,
            context_window: spec.context_window,
            output_format: spec.output_format,
            anthropic_task_budget: spec.anthropic_task_budget,
            anthropic_context_management: spec.anthropic_context_management,
            anthropic_speed: spec.anthropic_speed,
            // Top-level workflow steps run at depth 0 with no parent.
            // Sub-agent dispatch within a step bumps depth via the
            // `dispatch_agent` tool; this struct literal only fires
            // for the workflow → agent direct dispatch.
            parent_run_id: None,
            depth: 0,
            dispatchable_agents: spec.dispatchable_agents,
            step_id: step_id.to_string(),
            on_tool_call,
        }
    }
}

/// Construct a stub `LlmProvider` that errors on first call. Used when
/// the real provider build fails inside the StepFactory (e.g. missing
/// credential): instead of panicking and writing a crash log, we hand
/// the runner a provider that returns the build error from its first
/// `send`/`stream` call. The runner's normal error path then emits
/// `Event::RunComplete { status: Error, error: ... }`, which the line
/// printer renders as `✗ <step_id> <error>` — the user sees a clean,
/// actionable message.
pub(crate) fn provider_build_error_stub(
    provider_name: String,
    model: String,
    error: String,
) -> ProviderBuildErrorStub {
    ProviderBuildErrorStub {
        provider_name,
        model,
        error,
    }
}

pub(crate) struct ProviderBuildErrorStub {
    provider_name: String,
    model: String,
    error: String,
}

#[async_trait::async_trait]
impl rupu_providers::LlmProvider for ProviderBuildErrorStub {
    async fn send(
        &mut self,
        _request: &rupu_providers::LlmRequest,
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(rupu_providers::ProviderError::AuthConfig(format!(
            "{}: {}\n  Run: rupu auth login --provider {} --mode <api-key|sso>",
            self.provider_name, self.error, self.provider_name,
        )))
    }

    async fn stream(
        &mut self,
        _request: &rupu_providers::LlmRequest,
        _on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(rupu_providers::ProviderError::AuthConfig(format!(
            "{}: {}\n  Run: rupu auth login --provider {} --mode <api-key|sso>",
            self.provider_name, self.error, self.provider_name,
        )))
    }

    fn default_model(&self) -> &str {
        &self.model
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        // Pick a stable variant; only used for log attribution.
        rupu_providers::ProviderId::Anthropic
    }
}

#[cfg(test)]
mod provider_build_error_stub_tests {
    use super::*;
    use rupu_providers::{LlmProvider, LlmRequest, ProviderError};

    fn empty_request() -> LlmRequest {
        LlmRequest {
            model: "test-model".into(),
            system: None,
            messages: vec![],
            max_tokens: 1,
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
        }
    }

    #[tokio::test]
    async fn send_returns_authconfig_with_login_hint() {
        // Regression for the v0.4.5 panic: when the StepFactory's
        // build_for_provider() failed (missing credential, etc.) the
        // `.expect()` panicked and a crash log was written. The stub
        // routes the same error through the runner's normal failure
        // path so the line printer can render it cleanly.
        let mut stub = provider_build_error_stub(
            "openai".to_string(),
            "gpt-5".to_string(),
            "no credentials configured for openai".to_string(),
        );
        let err = stub.send(&empty_request()).await.expect_err("must error");
        let ProviderError::AuthConfig(msg) = err else {
            panic!("expected AuthConfig variant, got {err:?}");
        };
        assert!(msg.contains("openai"), "missing provider name: {msg}");
        assert!(
            msg.contains("rupu auth login --provider openai"),
            "missing actionable login hint: {msg}",
        );
    }
}
