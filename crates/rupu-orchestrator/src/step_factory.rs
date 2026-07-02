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

/// Resolve which concerns block a workflow step runs against.
///
/// Workflow-level concerns take precedence over the agent's own
/// (`workflow.or(agent)`): when a workflow declares `concerns:`, every
/// step uses it and the agent frontmatter's block is ignored. When the
/// workflow declares none, the agent's block flows through.
pub(crate) fn resolve_step_concerns(
    workflow_concerns: Option<rupu_coverage::ConcernsBlock>,
    agent_concerns: Option<rupu_coverage::ConcernsBlock>,
) -> Option<rupu_coverage::ConcernsBlock> {
    workflow_concerns.or(agent_concerns)
}

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
    /// OpenAI-compatible provider params resolved from `config.toml`, keyed by
    /// provider name. Lets workflow steps build custom providers (e.g.
    /// `oracle`) the same way `rupu run` does. Empty when no
    /// `[providers.<name>] kind = "openai-compatible"` is declared.
    pub openai_compatible:
        std::collections::HashMap<String, provider_factory::OpenAiCompatibleParams>,
}

/// Resolve a step's agent spec from a `load_agent` result. On success the
/// spec passes through. On failure (the agent file is missing or unparseable)
/// return a minimal spec carrying NO provider/model plus a loud, actionable
/// error message — the caller then wires an error-stub provider so the step
/// fails immediately instead of silently substituting the default
/// provider/model (which previously billed `anthropic` for a step that named
/// a nonexistent agent).
fn resolve_step_agent_spec(
    load: Result<rupu_agent::AgentSpec, String>,
    agent_name: &str,
    rendered_prompt: &str,
) -> (rupu_agent::AgentSpec, Option<String>) {
    match load {
        Ok(spec) => (spec, None),
        Err(e) => (
            rupu_agent::AgentSpec {
                name: agent_name.to_string(),
                description: None,
                provider: None,
                model: None,
                auth: None,
                tools: None,
                max_turns: Some(50),
                permission_mode: None,
                anthropic_oauth_prefix: None,
                effort: None,
                context_window: None,
                output_format: None,
                anthropic_task_budget: None,
                anthropic_context_management: None,
                anthropic_speed: None,
                dispatchable_agents: None,
                concerns: None,
                max_tokens: None,
                context_window_tokens: None,
                compact_at_percent: None,
                system_prompt: rendered_prompt.to_string(),
                raw: rendered_prompt.to_string(),
            },
            Some(format!(
                "agent `{agent_name}` not found or failed to load: {e}"
            )),
        ),
    }
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
        let load =
            rupu_agent::load_agent(&self.global, project_agents_parent.as_deref(), agent_name)
                .map_err(|e| e.to_string());
        let (spec, load_err) = resolve_step_agent_spec(load, agent_name, &rendered_prompt);

        // A missing or unparseable agent file is a hard error: fail loudly via
        // the error-stub provider instead of silently running on the default
        // provider/model. (Previously a step naming a nonexistent agent ran on
        // `anthropic`/`claude-sonnet-4-6` and billed it.) A present agent that
        // merely omits `provider:`/`model:` still defaults, as before.
        let auth_hint = spec.auth;
        // Build the provider. On a load error OR a build failure substitute a
        // stub provider that returns the error on first call; the runner's
        // `RunComplete { status: Error }` path surfaces it as a clean
        // `✗ <step_id>` line — no panic, no crash log, no provider call.
        // Custom OpenAI-compatible providers (declared as
        // `[providers.<name>] kind = "openai-compatible"`) are resolved from
        // the config-derived `openai_compatible` map and built via
        // `build_for_provider_with_config` — the same path `rupu run` uses, so
        // a workflow step on e.g. `oracle` reaches the configured endpoint
        // instead of failing with "unknown provider".
        let provider_name: String;
        let model: String;
        let provider: Box<dyn rupu_providers::LlmProvider> = match load_err {
            Some(msg) => {
                provider_name = "unresolved".to_string();
                model = "-".to_string();
                Box::new(provider_build_error_stub(
                    provider_name.clone(),
                    model.clone(),
                    msg,
                ))
            }
            None => {
                provider_name = spec.provider.clone().unwrap_or_else(|| "anthropic".into());
                let oai_params = self.openai_compatible.get(&provider_name).cloned();
                // Prefer the agent's pinned model; for an openai-compatible
                // provider fall back to its configured default_model.
                model = spec
                    .model
                    .clone()
                    .or_else(|| oai_params.as_ref().map(|p| p.default_model.clone()))
                    .unwrap_or_else(|| "claude-sonnet-4-6".into());
                let provider_config = provider_factory::ProviderConfig {
                    anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
                    openai_compatible: oai_params,
                };
                match provider_factory::build_for_provider_with_config(
                    &provider_name,
                    &model,
                    auth_hint,
                    self.resolver.as_ref(),
                    &provider_config,
                )
                .await
                {
                    Ok((_resolved_auth, p)) => p,
                    Err(e) => Box::new(provider_build_error_stub(
                        provider_name.clone(),
                        model.clone(),
                        e.to_string(),
                    )),
                }
            }
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
                coverage_writer: None,
                surface_tag: None,
                run_id: None,
                model: None,
                tool_mappings: None,
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
            on_stream_event: None,
            // Workflow-level concerns take precedence over agent-level concerns.
            // When the workflow declares `concerns:`, every step uses it —
            // the agent frontmatter's `concerns:` is ignored for this run.
            concerns: resolve_step_concerns(self.workflow.concerns.clone(), spec.concerns),
            max_tokens: spec
                .max_tokens
                .unwrap_or(rupu_agent::runner::DEFAULT_MAX_TOKENS),
            // All steps of a workflow share the same target_id (keyed on the
            // workflow name) so ledger entries accumulate per-workflow, not
            // per-step-agent.
            scope_name: Some(self.workflow.name.clone()),
            // Workflow steps must report as "workflow" surface so coverage
            // FileTouchEvents are correctly attributed; the runner defaults
            // to "agent" when this is None.
            surface_tag: Some("workflow".to_string()),
            context_window_tokens: spec.context_window_tokens,
            compact_at_percent: spec.compact_at_percent,
            pause: None,
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

/// Tests for workflow-level concerns resolution.
///
/// `build_opts_for_step` requires an async provider build and live
/// credentials, so we can't drive it directly. Instead these tests call
/// the same `resolve_step_concerns` helper that `build_opts_for_step`
/// uses, with real parsed `Workflow` and `AgentSpec` values — so they
/// genuinely guard the production resolution (not a re-implementation).
#[cfg(test)]
mod concerns_resolution_tests {
    use super::resolve_step_concerns;
    use rupu_agent::AgentSpec;
    use rupu_coverage::ConcernsEntry;

    use crate::workflow::Workflow;

    /// Helper: extract the `include` string from the first entry of a
    /// concerns block, panicking if the entry is not an `Include` variant.
    fn first_include(block: &rupu_coverage::ConcernsBlock) -> &str {
        match &block.entries[0] {
            ConcernsEntry::Include(d) => &d.include,
            other => panic!("expected Include entry, got {other:?}"),
        }
    }

    /// Parse a minimal Workflow YAML with the given `include` template name
    /// in its `concerns:` block.
    fn workflow_with_concerns(name: &str, include: &str) -> Workflow {
        let yaml = format!(
            "name: {name}\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\nconcerns:\n  - include: {include}\n"
        );
        Workflow::parse(&yaml).expect("workflow should parse")
    }

    /// Parse a minimal AgentSpec with the given `include` template name
    /// in its `concerns:` frontmatter.
    fn agent_with_concerns(include: &str) -> AgentSpec {
        let src = format!(
            "---\nname: test-agent\nconcerns:\n  - include: {include}\n---\nDo the thing.\n"
        );
        AgentSpec::parse(&src).expect("agent spec should parse")
    }

    /// Parse a minimal Workflow with no `concerns:` key at all.
    fn workflow_without_concerns() -> Workflow {
        let yaml =
            "name: bare\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";
        Workflow::parse(yaml).expect("workflow should parse")
    }

    // ── Case 1: both declare concerns → workflow wins ────────────────────────

    #[test]
    fn workflow_concerns_override_agent_concerns() {
        let workflow = workflow_with_concerns("wf-security-scan", "stride");
        let agent = agent_with_concerns("owasp-top10-2021");

        // Call the same helper build_opts_for_step uses.
        let resolved = resolve_step_concerns(workflow.concerns.clone(), agent.concerns);

        let block = resolved.expect("concerns should be Some after resolution");
        assert_eq!(
            block.entries.len(),
            1,
            "resolved block should have exactly one entry"
        );
        assert_eq!(
            first_include(&block),
            "stride",
            "workflow's concerns (stride) must win over agent's (owasp-top10-2021)"
        );
    }

    // ── Case 2: only agent declares concerns → agent's flow through ──────────

    #[test]
    fn agent_concerns_used_when_workflow_has_none() {
        let workflow = workflow_without_concerns();
        let agent = agent_with_concerns("owasp-top10-2021");

        // Same helper.
        let resolved = resolve_step_concerns(workflow.concerns.clone(), agent.concerns);

        let block = resolved.expect("agent concerns should flow through when workflow has none");
        assert_eq!(
            first_include(&block),
            "owasp-top10-2021",
            "agent's concerns should be the resolved value when workflow has none"
        );
    }

    // ── Case 3: scope_name is derived from the workflow name ─────────────────

    #[test]
    fn scope_name_is_workflow_name() {
        // The scope_name assignment on line 212 is:
        //   scope_name: Some(self.workflow.name.clone())
        // Verify that the workflow name is correctly accessible after parse.
        let workflow = workflow_with_concerns("my-workflow", "stride");
        // Mimic what build_opts_for_step does.
        let scope_name: Option<String> = Some(workflow.name.clone());
        assert_eq!(
            scope_name.as_deref(),
            Some("my-workflow"),
            "scope_name must equal the workflow's name"
        );
    }
}

#[cfg(test)]
mod missing_agent_tests {
    use super::resolve_step_agent_spec;
    use rupu_agent::AgentSpec;

    #[test]
    fn present_agent_passes_through_without_error() {
        let spec =
            AgentSpec::parse("---\nname: real\nprovider: oracle\nmodel: glm\n---\nbody\n").unwrap();
        let (out, err) = resolve_step_agent_spec(Ok(spec), "real", "prompt");
        assert!(err.is_none());
        assert_eq!(out.provider.as_deref(), Some("oracle"));
        assert_eq!(out.model.as_deref(), Some("glm"));
    }

    #[test]
    fn missing_agent_fails_loudly_without_defaulting_to_anthropic() {
        let (out, err) = resolve_step_agent_spec(
            Err("agents/oracle-enumerator-glm.md: no such file".to_string()),
            "oracle-enumerator-glm",
            "prompt",
        );
        let msg = err.expect("a missing agent must produce a loud error");
        assert!(msg.contains("oracle-enumerator-glm"), "msg: {msg}");
        assert!(
            msg.to_lowercase().contains("not found") || msg.contains("failed to load"),
            "msg should be actionable: {msg}"
        );
        // The whole point: do NOT silently substitute the default provider/model.
        assert_ne!(out.provider.as_deref(), Some("anthropic"));
        assert!(
            out.provider.is_none(),
            "missing agent must not carry a provider"
        );
        assert!(out.model.is_none(), "missing agent must not carry a model");
    }
}
