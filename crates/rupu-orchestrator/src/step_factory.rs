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
use rupu_agent::{LegacyRunOpts, OnToolCallCallback};
use rupu_runtime::provider_factory;
use rupu_tools::{AgentDispatcher, PermissionMode, PermissionPolicy, ToolContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Build this step's netflow sink: a ledger writer rooted at
/// [`rupu_netflow::netflow_dir`] (the one shared write-side routing rule
/// -- see its doc comment for why this crate calls it directly instead of
/// keeping its own copy: `rupu-orchestrator` cannot depend on `rupu-cli`,
/// the dependency runs the other way, and a second copy of this rule is
/// exactly the kind of write/read routing divergence that made CP's own
/// netflow API unable to find ledgers `rupu-cli` had actually written)
/// plus a `TranscriptSink` streaming into the step's own transcript.
/// Best-effort -- a ledger that cannot be opened logs at debug and the
/// step continues with transcript-only capture.
///
/// Called once per [`DefaultStepFactory::build_opts_for_step`] invocation
/// — i.e. once per dispatched step, using THAT call's `run_id` (the
/// workflow's own run id for a linear step; a freshly minted id for a
/// `parallel:`/`on_reject` sub-step — see `runner.rs`'s `dispatch_one`
/// call sites) and `transcript_path`. This is deliberately NOT built once
/// and cached on `DefaultStepFactory` itself: the factory is long-lived
/// across every step of a run (and, for autoflow / `rupu cp serve`,
/// across many runs sharing one process), so a sink built once at
/// construction time would reintroduce the exact "first run wins" defect
/// this plan removes. Building it fresh per call, scoped to that call's
/// own run id, is what keeps a sink's lifetime matched to one run.
///
/// The returned `NetflowWriterHandle` is intentionally NOT kept alive or
/// explicitly shut down by the caller — this function returns an
/// `LegacyRunOpts`, not a handle-owning scope that outlives the step's own
/// async work, so there is nowhere to hold it until the step's HTTP
/// traffic is done. This is safe: `Arc<NetflowWriter>` (cloned into the
/// returned sink) keeps the writer task's channel open independent of the
/// local `NetflowWriterHandle`, and the background task naturally closes
/// once every sink holder (the step's own provider/registry clients) is
/// dropped at the end of the step's run — see
/// `NetflowWriterHandle::shutdown`'s doc comment for why a caller-held
/// `Arc<NetflowWriter>` clone is exactly the case that keeps a dropped
/// handle's task alive rather than hanging it.
///
/// KNOWN, ACCEPTED OVERHEAD (whole-branch review, Minor #15): a workflow
/// run's linear steps share the WORKFLOW's own `run_id` (see the `run_id`
/// doc above), so calling this once per linear step spawns one
/// independent `NetflowWriterHandle` — its own bounded channel,
/// background task, open fd, and `dropped` counter — per step, all
/// pointed at the SAME `<run_id>.jsonl` file rather than one shared
/// writer. This is safe in practice, not silently lossy: each writer
/// issues one `write_all` of a complete JSON line per append (see
/// `writer.rs`) — that's a loop over partial writes in general, not an
/// atomic syscall, so the real guarantee against a torn line rests on
/// `O_APPEND` plus regular-file write behaviour, not on the `write_all`
/// API itself. In practice these appenders barely overlap: each one only
/// runs for the brief window around its own step's dispatch. And the
/// read side sums every `Dropped` line it finds in a file regardless of
/// which writer instance produced it
/// (`rupu_netflow::ledger::read_flows_and_dropped`) — so a step's own
/// overflow is still counted, just via its own line rather than a
/// merged counter. The cost is purely resource waste (N
/// short-lived tasks/fds instead of one long-lived one for one workflow
/// run), not a correctness gap. Left as-is rather than caching/sharing a
/// writer keyed by run id: a process-wide cache keyed by run id is
/// shaped exactly like the `OnceLock` this whole plan removed, just
/// scoped smaller — worth reconsidering only if this overhead is ever
/// shown to matter in practice (heavy fan-out with many steps sharing one
/// id), not preemptively.
fn step_netflow_sink(
    global: &Path,
    project_root: Option<&Path>,
    run_id: &str,
    transcript_path: &Path,
) -> Arc<dyn rupu_netflow::FlowSink> {
    let dir = rupu_netflow::netflow_dir(global, project_root);
    let netflow_paths = rupu_netflow::NetflowPaths::for_run(&dir, run_id);
    let mut sinks: Vec<Arc<dyn rupu_netflow::FlowSink>> = vec![Arc::new(
        rupu_transcript::TranscriptSink::new(transcript_path.to_path_buf()),
    )];
    match rupu_netflow::NetflowWriterHandle::spawn(netflow_paths) {
        Ok(handle) => sinks.push(handle.writer.clone()),
        Err(e) => {
            tracing::debug!(error = %e, run_id, "netflow ledger unavailable for this step");
        }
    }
    Arc::new(rupu_netflow::FanoutSink::new(sinks))
}

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
    /// The mode the workflow run was launched under; every agent step runs
    /// under it (unattended: no operator prompter).
    pub mode: PermissionMode,
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
    /// Resolved `[providers.<name>]` runtime knobs, keyed by provider name
    /// (`provider_factory::provider_tuning_map`). Lets a workflow step honor
    /// `timeout_ms` / `max_retries` / `max_concurrency` / `org_id` exactly as
    /// `rupu run` does (ISSUES.md I-9…I-12). Empty ⇒ documented defaults.
    pub provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
    /// Resolved vendor kind per declared account name
    /// (`provider_factory::resolve_kind_map`). Lets a workflow step's
    /// `agent:` resolve to a named multi-account (e.g. `anthropic-work`) the
    /// same way `rupu run` does — without this, a step naming such an
    /// account falls into `build_for_provider_with_config`'s `_` arm and
    /// fails with "unknown provider" (Task 4 review, concern #2). Empty ⇒
    /// every step's provider dispatches by name only, same as before this
    /// field existed.
    pub kinds: std::collections::HashMap<String, String>,
    /// `default_provider` from `config.toml`. Used when a step's agent pins no
    /// `provider:`. `None` falls back to `provider_factory::FALLBACK_PROVIDER`.
    pub default_provider: Option<String>,
    /// `default_model` from `config.toml`. Used when a step's agent pins no
    /// `model:`. Threaded in so a workflow step resolves the same model
    /// `rupu run` would for the same agent (ISSUES.md I-2).
    pub default_model: Option<String>,
    /// `[bash].timeout_secs` from `config.toml`. Threaded in so a workflow
    /// step's `bash` calls honor the same timeout `rupu run`/`rupu session`
    /// apply for the same agent (ISSUES.md I-18 — this used to hardcode
    /// 120 here regardless of config).
    pub bash_timeout_secs: u64,
    /// `[bash].env_allowlist` from `config.toml`. Threaded in so a workflow
    /// step's `bash` calls forward the same extra env vars `rupu
    /// run`/`rupu session` do for the same agent (ISSUES.md I-18 — this
    /// used to hardcode an empty allowlist here regardless of config).
    pub bash_env_allowlist: Vec<String>,
    /// Artifact store + limits for recording findings; the per-step profile
    /// is resolved in `build_opts_for_step` and set on a clone of this.
    pub findings_base: rupu_coverage::FindingWriteOptions,
    /// Model-limit discovery context (cache dir + `[providers.*].models`
    /// overrides). Each step resolves the limits of ITS agent's own
    /// provider/model through it (spec 2026-09-30 §6.1).
    pub limits_ctx: rupu_runtime::model_limits::LimitsContext,
    /// `[providers.<name>]` from `config.toml`: what a step's fallback hops
    /// are built from (`rupu_runtime::hop_builder`).
    pub providers: std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
    /// `[recovery]` from `config.toml`: the fallback table a step uses when
    /// its agent declares no `fallbacks:`, and the server-side-fallback
    /// toggle for the step's provider and its hops.
    pub recovery: rupu_config::RecoveryConfig,
    /// Coverage/findings scope every step reports under. `None` (the
    /// default for every ordinary workflow run) keeps the historical
    /// per-workflow scope — the workflow's own name. `Some(name)` is set
    /// only by an agentiflow-launched workflow unit (`rupu workflow run
    /// --fleet-run-dir`), so its steps' findings pool into the agentiflow's
    /// scope instead of the workflow's.
    pub scope_name_override: Option<String>,
    /// The process-wide subprocess network-capture backend, handed to every
    /// step's `ToolContext` so its `bash` calls are captured. The factory
    /// never builds it: `rupu_runtime::net_capture::shared` blocks on first
    /// call, so the (async) caller warms it off-runtime and threads the
    /// `Arc` in (`rupu-cli`'s `netflow_sink::net_capture`). `None` (tests)
    /// means no capture.
    pub net_capture: Option<Arc<dyn rupu_netflow::SubprocessCapture>>,
    /// The customer this run is attributed to — the CLI resolves it once,
    /// at launch (`ConfigPaths.customer_slug`), or a resume reads it off the
    /// run's `RunRecord`. Recorded on the run (`StepFactory::customer`) and
    /// set on every step's `ToolContext`. `None` = no customer.
    pub customer: Option<String>,
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
                findings_profile: None,
                fallbacks: None,
                name: agent_name.to_string(),
                description: None,
                provider: None,
                model: None,
                auth: None,
                tools: None,
                max_turns: Some(50),
                permission_mode: None,
                anthropic_oauth_prefix: None,
                anthropic_prompt_cache: None,
                effort: None,
                thinking_display: None,
                context_window: None,
                output_format: None,
                output_schema: None,
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
    ) -> LegacyRunOpts {
        // We still verify the parent step exists in the workflow so
        // unknown step ids surface clearly, but we drive the agent
        // load off `agent_name` (which differs from the parent's
        // `agent:` for `parallel:` sub-steps).
        //
        // An `on_reject:` cleanup sub-step's id is NOT in `workflow.steps`
        // — it lives nested under its gate's `approval.on_reject`
        // (`crate::workflow::Approval::on_reject`), the same way a
        // `parallel:`/`for_each:` sub-step's agent differs from its
        // parent's. `run_reject_cleanup` dispatches those sub-steps
        // through this same factory (`dispatch_one` with the sub-step's
        // own id), so the lookup falls back to searching every gate's
        // cleanup chain before giving up.
        let step = self
            .workflow
            .steps
            .iter()
            .find(|s| s.id == step_id)
            .or_else(|| {
                self.workflow.steps.iter().find_map(|s| {
                    s.approval
                        .as_ref()
                        .and_then(|a| a.on_reject.iter().find(|sub| sub.id == step_id))
                })
            })
            .expect(
                "step_id from orchestrator must match a workflow step or an on_reject cleanup sub-step",
            );

        // The agent loader takes the parent of `agents/`. For the
        // project layer that's `<project>/.rupu`; the global layer is
        // `<global>` directly (which already contains `agents/`).
        let project_agents_parent = self.project_root.as_ref().map(|p| p.join(".rupu"));
        // Admission-paced: under fd pressure (a wide fan-out) this grows the
        // open-file limit or waits for running agents to release descriptors
        // instead of failing with EMFILE. See `rupu_agent::fd_budget`.
        let load = rupu_agent::load_agent_admitted(
            &self.global,
            project_agents_parent.as_deref(),
            agent_name,
        )
        .await
        .map_err(|e| e.to_string());
        let (spec, load_err) = resolve_step_agent_spec(load, agent_name, &rendered_prompt);

        let findings = rupu_coverage::FindingWriteOptions {
            profile: rupu_coverage::FindingProfile::resolve(
                step.findings_profile,
                self.workflow.defaults.findings_profile,
                spec.findings_profile,
            ),
            ..self.findings_base.clone()
        };

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
        // False once `provider` is an error stub (the agent failed to load, or
        // its provider failed to build): there is no model to ask about limits.
        let mut provider_is_real = true;
        // This step's netflow sink, set once a real provider is being built;
        // its fallback hops reuse it so their requests land in the same ledger.
        let mut step_sink: Option<Arc<dyn rupu_netflow::FlowSink>> = None;
        let mut provider: Box<dyn rupu_providers::LlmProvider> = match load_err {
            Some(msg) => {
                provider_is_real = false;
                provider_name = "unresolved".to_string();
                model = "-".to_string();
                Box::new(agent_load_error_stub(msg))
            }
            None => {
                provider_name = provider_factory::resolve_provider_name(
                    spec.provider.as_deref(),
                    self.default_provider.as_deref(),
                );
                let oai_params = self.openai_compatible.get(&provider_name).cloned();
                // Prefer the agent's pinned model; for an openai-compatible
                // provider fall back to its configured default_model.
                model = provider_factory::resolve_model(
                    spec.model.as_deref(),
                    self.default_model.as_deref(),
                    oai_params.as_ref().map(|p| p.default_model.as_str()),
                );
                let provider_config = provider_factory::ProviderConfig {
                    anthropic_oauth_system_prefix: spec.anthropic_oauth_prefix,
                    anthropic_prompt_cache: spec.anthropic_prompt_cache,
                    anthropic_server_side_fallback: Some(self.recovery.server_side_fallback),
                    openai_compatible: oai_params,
                    tuning: self.provider_tuning.get(&provider_name).cloned(),
                    kind: self.kinds.get(&provider_name).cloned(),
                };
                // This step's netflow sink — built fresh per step call,
                // scoped to THIS call's `run_id`/`transcript_path`. See
                // `step_netflow_sink`'s doc comment for why it is built
                // here rather than once on the factory.
                let netflow_sink = step_netflow_sink(
                    &self.global,
                    self.project_root.as_deref(),
                    &run_id,
                    &transcript_path,
                );
                step_sink = Some(netflow_sink.clone());
                match provider_factory::build_for_provider_with_config(
                    &provider_name,
                    &model,
                    auth_hint,
                    self.resolver.as_ref(),
                    &provider_config,
                    netflow_sink,
                )
                .await
                {
                    Ok((_resolved_auth, p)) => p,
                    Err(e) => {
                        provider_is_real = false;
                        Box::new(provider_build_error_stub(
                            provider_name.clone(),
                            model.clone(),
                            e.to_string(),
                        ))
                    }
                }
            }
        };

        // Discover the step agent's real limits (agent pin → config → live
        // model list → unknown). Computed while `spec` is still whole. An
        // error stub (agent load or provider build failure) has no real
        // provider to ask — asking it would only report a false "exposes no
        // model limits" note in front of the error the run is about to raise —
        // so it gets `unknown()`.
        let limits = if !provider_is_real {
            rupu_providers::model_limits::ModelLimits::unknown()
        } else {
            rupu_runtime::model_limits::resolve(
                rupu_runtime::model_limits::LimitOverrides::from_spec(&spec),
                &provider_name,
                &model,
                provider.as_mut(),
                &self.limits_ctx,
            )
            .await
        };

        // The step's fallback ladder: its agent's `fallbacks:` (else the
        // `[recovery].fallbacks` table) and a hop builder over this step's
        // resolver, provider table, limits and sink. An error-stub step (the
        // agent did not load, or its provider did not build) gets none: there
        // is no reply to recover, only a configuration error to report.
        // The same sink also goes on the step's `ToolContext`, so bash's
        // subprocess flows land in this step's ledger/transcript. `None`
        // only when the agent failed to load (an error-stub step runs no tools).
        let tool_netflow_sink = step_sink.clone();
        let recovery = match (provider_is_real, step_sink) {
            (true, Some(sink)) => rupu_runtime::hop_builder::recovery_opts(
                &self.recovery,
                spec.fallbacks.as_deref(),
                self.resolver.clone(),
                self.providers.clone(),
                self.limits_ctx.clone(),
                sink,
                // Exactly what the step's primary `ProviderConfig` and auth
                // hint above carry, so a hop keeps the agent's settings.
                rupu_runtime::hop_builder::AgentOverrides {
                    oauth_prefix: spec.anthropic_oauth_prefix,
                    prompt_cache: spec.anthropic_prompt_cache,
                    auth: auth_hint,
                    origin_provider: provider_name.clone(),
                },
            ),
            _ => Default::default(),
        };

        let agent_system_prompt = match self.system_prompt_suffix.as_deref() {
            Some(suffix) => format!("{}\n\n## Run target\n\n{}", spec.system_prompt, suffix),
            None => spec.system_prompt,
        };

        LegacyRunOpts {
            seed_source: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            step_actions: step.actions.clone(),
            alias_scope: Default::default(),
            agent_name: spec.name,
            agent_system_prompt,
            // The agent's own grant; the runner resolves it with the step's
            // `actions:` (connector narrowing, W2) into the run's grant.
            agent_tools: spec.tools,
            provider,
            provider_name,
            model,
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: spec.max_turns.unwrap_or(50),
            // Unattended: a workflow step has no operator to prompt, so
            // `ask` allows writes and says so once per step with a
            // `permission_mode_degraded` notice (ISSUES.md I-78, D5). That is
            // deliberate: `ask` is also the default when `--mode` is omitted,
            // and a genuinely prompting `ask` would hang every unattended run.
            // `rupu workflow run` warns at startup when no mode was given;
            // `--mode readonly` denies writes and external actions.
            permission: PermissionPolicy::unattended(self.mode),
            // The identity (run id, dispatchable agents, …) comes from the
            // flat fields below (`LegacyRunOpts::into_run_opts`).
            tool_context: ToolContext {
                workspace: rupu_tools::WorkspaceScope {
                    path: workspace_path,
                    bash: rupu_tools::BashConfig {
                        env_allowlist: self.bash_env_allowlist.clone(),
                        timeout_secs: self.bash_timeout_secs,
                    },
                    ..Default::default()
                },
                services: rupu_tools::ToolServices {
                    dispatcher: self.dispatcher.clone(),
                    findings: Some(findings),
                    netflow_sink: tool_netflow_sink,
                    net_capture: self.net_capture.clone(),
                    customer: self.customer.clone(),
                    ..Default::default()
                },
                ..Default::default()
            },
            user_message: rendered_prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            no_stream: false,
            // Workflow runs stream through the workflow printer by
            // tailing JSONL transcripts. Suppress direct stdout
            // writes here so they don't corrupt the live view.
            suppress_stream_stdout: true,
            mcp_registry: Some(Arc::clone(&self.mcp_registry)),
            effort: spec.effort,
            thinking_display: spec.thinking_display,
            context_window: spec.context_window,
            output_format: spec.output_format,
            output_schema: spec.output_schema.clone(),
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
            on_usage: None,
            // Workflow-level concerns take precedence over agent-level concerns.
            // When the workflow declares `concerns:`, every step uses it —
            // the agent frontmatter's `concerns:` is ignored for this run.
            concerns: resolve_step_concerns(self.workflow.concerns.clone(), spec.concerns),
            limits,
            // All steps of a workflow share the same target_id (keyed on the
            // workflow name) so ledger entries accumulate per-workflow, not
            // per-step-agent.
            // An agentiflow-launched workflow unit overrides it so its steps
            // pool into the agentiflow's scope; every other run stays on the
            // workflow's own name.
            scope_name: self
                .scope_name_override
                .clone()
                .or_else(|| Some(self.workflow.name.clone())),
            // Workflow steps must report as "workflow" surface so coverage
            // FileTouchEvents are correctly attributed; the runner defaults
            // to "agent" when this is None.
            surface_tag: Some("workflow".to_string()),
            pause: None,
            codename: None,
            recovery,
        }
    }

    fn permission_mode(&self) -> Option<&str> {
        Some(self.mode.as_str())
    }

    fn customer(&self) -> Option<&str> {
        self.customer.as_deref()
    }

    fn system_prompt_suffix(&self) -> Option<&str> {
        self.system_prompt_suffix.as_deref()
    }

    fn engagement_profiles(&self) -> Vec<String> {
        self.findings_base
            .engagement
            .as_deref()
            .map(|set| set.ids().into_iter().map(str::to_string).collect())
            .unwrap_or_default()
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
        kind: ErrorStubKind::ProviderBuild,
        provider_name,
        model,
        error,
    }
}

/// The stub for a step whose agent file failed to load. Unlike a provider
/// build failure this is not a credentials problem, so it must NOT surface
/// as `auth config error:` with a `rupu auth login` hint (it used to point
/// at a provider literally named `unresolved`). The loader's message goes
/// out verbatim via `ProviderError::Preflight`, plus a where-to-look hint.
pub(crate) fn agent_load_error_stub(error: String) -> ProviderBuildErrorStub {
    ProviderBuildErrorStub {
        kind: ErrorStubKind::AgentLoad,
        provider_name: "unresolved".to_string(),
        model: "-".to_string(),
        error,
    }
}

enum ErrorStubKind {
    ProviderBuild,
    AgentLoad,
}

pub(crate) struct ProviderBuildErrorStub {
    kind: ErrorStubKind,
    provider_name: String,
    model: String,
    error: String,
}

impl ProviderBuildErrorStub {
    fn to_error(&self) -> rupu_providers::ProviderError {
        match self.kind {
            ErrorStubKind::ProviderBuild => rupu_providers::ProviderError::AuthConfig(format!(
                "{}: {}\n  Run: rupu auth login --provider {} --mode <api-key|sso>",
                self.provider_name, self.error, self.provider_name,
            )),
            ErrorStubKind::AgentLoad => rupu_providers::ProviderError::Preflight(format!(
                "{}\n  Checked the project agents dir (.rupu/agents/) and the global agents dir.",
                self.error,
            )),
        }
    }
}

#[async_trait::async_trait]
impl rupu_providers::LlmProvider for ProviderBuildErrorStub {
    async fn send(
        &mut self,
        _request: &rupu_providers::LlmRequest,
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(self.to_error())
    }

    async fn stream(
        &mut self,
        _request: &rupu_providers::LlmRequest,
        _on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
    ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
        Err(self.to_error())
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
            max_tokens: Some(1),
            tools: vec![],
            cell_id: None,
            trace_id: None,
            thinking: None,
            context_window: None,
            task_type: None,
            output_format: None,
            output_schema: None,
            anthropic_task_budget: None,
            anthropic_context_management: None,
            anthropic_speed: None,
            disable_prompt_cache: false,
            thinking_display: None,
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

    #[tokio::test]
    async fn send_agent_load_error_is_plain_not_found_without_auth_hint() {
        // A step naming a nonexistent agent used to surface as
        // `auth config error: unresolved: agent … not found` plus a bogus
        // `Run: rupu auth login --provider unresolved` hint — an agent-file
        // problem dressed up as a credentials problem. The agent-load stub
        // must surface the loader's message verbatim.
        let mut stub = agent_load_error_stub(
            "agent `dispatch-smoke` not found or failed to load: agent not found: dispatch-smoke"
                .to_string(),
        );
        let err = stub.send(&empty_request()).await.expect_err("must error");
        let ProviderError::Preflight(msg) = err else {
            panic!("expected Preflight variant, got {err:?}");
        };
        assert!(
            msg.contains("agent `dispatch-smoke` not found or failed to load"),
            "missing loader message: {msg}",
        );
        assert!(
            msg.contains(".rupu/agents/"),
            "missing where-to-look hint: {msg}",
        );
        let rendered = ProviderError::Preflight(msg).to_string();
        assert!(
            !rendered.contains("auth config error") && !rendered.contains("rupu auth login"),
            "agent-load failure must not masquerade as an auth failure: {rendered}",
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

/// End-to-end proof that `build_opts_for_step` hands the runner the
/// agent's own grant and the step's `actions:`, which the runner resolves
/// into the run's grant (W2; its narrowing and audit are tested in
/// `rupu-tools`' `grant` and `rupu-agent`'s `tool_grant` tests). Drives the
/// real `DefaultStepFactory` against an on-disk agent spec granting
/// `[issues.list, issues.create]`; no live provider credentials are needed
/// because a build failure resolves to `ProviderBuildErrorStub` rather than
/// panicking.
#[cfg(test)]
mod narrowing_end_to_end_tests {
    use super::DefaultStepFactory;
    use crate::runner::StepFactory;
    use crate::workflow::Workflow;
    use std::sync::Arc;

    const WF: &str = r#"
name: w
steps:
  - id: narrowed
    agent: ag
    prompt: p
    actions: ["issues.list"]
  - id: unrestricted
    agent: ag
    prompt: p
    actions: []
"#;

    /// A limits context for tests that build a step through the REAL factory.
    ///
    /// These agents pin no provider, so they fall back to `anthropic`; on a
    /// machine with a stored (or env) Anthropic credential the provider build
    /// SUCCEEDS and limit discovery (spec 2026-09-30 §6.1) would then run a
    /// live `GET /v1/models` with that credential. Seeding a FRESH (empty) v2
    /// model-list cache for the fallback provider makes `resolve` read the
    /// cache and never fetch, whatever credentials the machine has — no env
    /// mutation needed. (Fresh = `fetched_at` now, inside the 1h TTL.)
    fn hermetic_limits_ctx(global: &std::path::Path) -> rupu_runtime::model_limits::LimitsContext {
        let cache_dir = global.join("cache/models");
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(
            cache_dir.join("anthropic.json"),
            format!(
                r#"{{"schema":2,"fetched_at":"{}","models":[]}}"#,
                chrono::Utc::now().to_rfc3339()
            ),
        )
        .unwrap();
        rupu_runtime::model_limits::LimitsContext::for_cache_dir(cache_dir)
    }

    fn factory(global: std::path::PathBuf) -> DefaultStepFactory {
        let limits_ctx = hermetic_limits_ctx(&global);
        DefaultStepFactory {
            customer: None,
            workflow: Workflow::parse(WF).expect("workflow must parse"),
            global,
            project_root: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            mode: rupu_tools::PermissionMode::Bypass,
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            kinds: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
            findings_base: rupu_coverage::FindingWriteOptions::default(),
            limits_ctx,
            providers: Default::default(),
            recovery: Default::default(),
            scope_name_override: None,
            net_capture: None,
        }
    }

    fn write_agent(global: &std::path::Path) {
        let agents_dir = global.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("ag.md"),
            "---\nname: ag\ntools: [issues.list, issues.create]\n---\nDo the thing.\n",
        )
        .unwrap();
    }

    /// A step whose provider cannot be built runs against an error stub. The
    /// stub has no listing to ask, so it gets `ModelLimits::unknown()` with
    /// no note: resolving it would only put a false "exposes no model
    /// limits" in front of the build error the run is about to raise.
    ///
    /// `#[serial]`: `generate.rs`'s tests set `RUPU_MOCK_PROVIDER_SCRIPT`,
    /// which would make the build succeed against the mock.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_provider_build_error_gets_unknown_limits_without_a_note() {
        const WF_BAD_PROVIDER: &str = r#"
name: w
steps:
  - id: s
    agent: bad
    prompt: p
"#;
        let tmp = assert_fs::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("bad.md"),
            "---\nname: bad\nprovider: no-such-provider\n---\nDo the thing.\n",
        )
        .unwrap();
        let mut f = factory(tmp.path().to_path_buf());
        f.workflow = Workflow::parse(WF_BAD_PROVIDER).expect("workflow must parse");
        let opts = f
            .build_opts_for_step(
                "s",
                "bad",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;
        assert_eq!(
            opts.limits,
            rupu_providers::model_limits::ModelLimits::unknown()
        );
        assert_eq!(opts.limits.note, None);
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn step_actions_reach_the_run_with_the_agent_grant() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let f = factory(tmp.path().to_path_buf());

        let opts = f
            .build_opts_for_step(
                "narrowed",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;

        assert_eq!(
            opts.agent_tools,
            Some(vec!["issues.list".to_string(), "issues.create".to_string()]),
            "the agent's own grant, un-narrowed: the runner narrows it"
        );
        assert_eq!(opts.step_actions, vec!["issues.list".to_string()]);
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn step_scope_is_the_workflow_name_unless_overridden() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());

        // Default (`None`): the historical per-workflow scope — unchanged.
        let f = factory(tmp.path().to_path_buf());
        assert_eq!(f.scope_name_override, None);
        let opts = f
            .build_opts_for_step(
                "narrowed",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;
        assert_eq!(opts.scope_name, Some("w".to_string()));

        // An agentiflow-launched workflow unit pools into the agentiflow scope.
        let mut f = factory(tmp.path().to_path_buf());
        f.scope_name_override = Some("af_123".to_string());
        let opts = f
            .build_opts_for_step(
                "narrowed",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;
        assert_eq!(opts.scope_name, Some("af_123".to_string()));
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn empty_step_actions_leave_the_agent_grant_unrestricted() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let f = factory(tmp.path().to_path_buf());

        let opts = f
            .build_opts_for_step(
                "unrestricted",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;

        assert_eq!(
            opts.agent_tools,
            Some(vec!["issues.list".to_string(), "issues.create".to_string()]),
            "actions: [] must leave the agent's full grant untouched"
        );
        assert!(opts.step_actions.is_empty());
    }

    const WF_FINDINGS: &str = r#"
name: findings-wf
defaults:
  findings_profile: full
steps:
  - id: overridden
    agent: fp
    prompt: p
    findings_profile: summary
  - id: inherits
    agent: fp
    prompt: p
"#;

    const WF_NO_DEFAULT: &str = r#"
name: findings-wf-2
steps:
  - id: agent_decides
    agent: fp
    prompt: p
"#;

    fn write_summary_agent(global: &std::path::Path) {
        let agents_dir = global.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("fp.md"),
            "---\nname: fp\ntools: [report_finding]\nfindingsProfile: summary\n---\nAssess.\n",
        )
        .unwrap();
    }

    async fn profile_for(
        wf: &str,
        step: &str,
        global: &std::path::Path,
    ) -> rupu_coverage::FindingProfile {
        let mut f = factory(global.to_path_buf());
        f.workflow = Workflow::parse(wf).expect("workflow must parse");
        let opts = f
            .build_opts_for_step(
                step,
                "fp",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                global.to_path_buf(),
                global.join(format!("{step}.jsonl")),
                None,
            )
            .await;
        opts.tool_context
            .services
            .findings
            .expect("step factory must always set findings options")
            .profile
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`) through `profile_for`'s `factory()`.
    #[tokio::test]
    #[serial_test::serial]
    async fn findings_profile_resolves_step_then_defaults_then_agent() {
        use rupu_coverage::FindingProfile::{Full, Summary};
        let tmp = assert_fs::TempDir::new().unwrap();
        write_summary_agent(tmp.path());

        // Step override beats the workflow default.
        assert_eq!(
            profile_for(WF_FINDINGS, "overridden", tmp.path()).await,
            Summary
        );
        // Workflow default beats the agent's `findingsProfile: summary`.
        assert_eq!(profile_for(WF_FINDINGS, "inherits", tmp.path()).await, Full);
        // With neither, the agent's frontmatter decides.
        assert_eq!(
            profile_for(WF_NO_DEFAULT, "agent_decides", tmp.path()).await,
            Summary
        );
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn findings_base_limits_reach_the_step() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_summary_agent(tmp.path());
        let mut f = factory(tmp.path().to_path_buf());
        f.workflow = Workflow::parse(WF_NO_DEFAULT).unwrap();
        f.findings_base = rupu_coverage::FindingWriteOptions {
            artifact_root: Some(tmp.path().join("store")),
            artifact_max_bytes: 7,
            ..Default::default()
        };
        let opts = f
            .build_opts_for_step(
                "agent_decides",
                "fp",
                "p".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("t.jsonl"),
                None,
            )
            .await;
        let fo = opts.tool_context.services.findings.unwrap();
        assert_eq!(fo.artifact_max_bytes, 7);
        assert_eq!(fo.artifact_root, Some(tmp.path().join("store")));
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn findings_base_engagement_reaches_the_step() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_summary_agent(tmp.path());
        let engagement = std::sync::Arc::new(
            rupu_coverage::builtin_registry()
                .unwrap()
                .active_set(&[rupu_coverage::DEFAULT_PROFILE.to_string()])
                .unwrap(),
        );
        let mut f = factory(tmp.path().to_path_buf());
        f.workflow = Workflow::parse(WF_NO_DEFAULT).unwrap();
        f.findings_base = rupu_coverage::FindingWriteOptions {
            engagement: Some(engagement.clone()),
            ..Default::default()
        };
        let opts = f
            .build_opts_for_step(
                "agent_decides",
                "fp",
                "p".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("t.jsonl"),
                None,
            )
            .await;
        // A regression that rebuilds the step's `FindingWriteOptions` without
        // cloning `findings_base` would silently drop the engagement, and the
        // step's findings would route as native code findings.
        let got = opts
            .tool_context
            .services
            .findings
            .unwrap()
            .engagement
            .expect("engagement must reach the step's findings");
        assert!(std::sync::Arc::ptr_eq(&got, &engagement));
        assert_eq!(got.ids(), vec![rupu_coverage::DEFAULT_PROFILE]);
    }

    // ── Findings profile: the full precedence table, and the unit shapes
    // that reach the factory under an id other than a top-level step's ──

    /// Agent files for the precedence table: one per frontmatter setting.
    fn write_profile_agents(global: &std::path::Path) {
        let agents_dir = global.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        for (name, line) in [
            ("fp_none", ""),
            ("fp_full", "findingsProfile: full\n"),
            ("fp_summary", "findingsProfile: summary\n"),
        ] {
            std::fs::write(
                agents_dir.join(format!("{name}.md")),
                format!("---\nname: {name}\ntools: [report_finding]\n{line}---\nAssess.\n"),
            )
            .unwrap();
        }
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn findings_profile_precedence_table() {
        use rupu_coverage::FindingProfile::{self, Full, Summary};
        let tmp = assert_fs::TempDir::new().unwrap();
        write_profile_agents(tmp.path());
        let name = |p: Option<FindingProfile>| match p {
            None => None,
            Some(Full) => Some("full"),
            Some(Summary) => Some("summary"),
        };
        // (step, workflow defaults, agent frontmatter) → resolved.
        type Row = (
            Option<FindingProfile>,
            Option<FindingProfile>,
            Option<FindingProfile>,
            FindingProfile,
        );
        let table: &[Row] = &[
            // The step wins over everything.
            (Some(Summary), Some(Full), Some(Full), Summary),
            (Some(Full), Some(Summary), Some(Summary), Full),
            (Some(Summary), None, None, Summary),
            // Then the workflow default, over the agent.
            (None, Some(Summary), Some(Full), Summary),
            (None, Some(Full), Some(Summary), Full),
            (None, Some(Summary), None, Summary),
            // Then the agent's frontmatter.
            (None, None, Some(Summary), Summary),
            (None, None, Some(Full), Full),
            // Then the built-in default.
            (None, None, None, Full),
        ];
        for &(step, defaults, agent, expected) in table {
            let agent_name = match name(agent) {
                None => "fp_none".to_string(),
                Some(p) => format!("fp_{p}"),
            };
            let mut wf = String::from("name: precedence\n");
            if let Some(d) = name(defaults) {
                wf.push_str(&format!("defaults:\n  findings_profile: {d}\n"));
            }
            wf.push_str(&format!(
                "steps:\n  - id: s\n    agent: {agent_name}\n    prompt: p\n"
            ));
            if let Some(p) = name(step) {
                wf.push_str(&format!("    findings_profile: {p}\n"));
            }
            let mut f = factory(tmp.path().to_path_buf());
            f.workflow = Workflow::parse(&wf).expect("workflow must parse");
            let got = f
                .build_opts_for_step(
                    "s",
                    &agent_name,
                    "p".to_string(),
                    "run1".to_string(),
                    "ws1".to_string(),
                    tmp.path().to_path_buf(),
                    tmp.path().join("s.jsonl"),
                    None,
                )
                .await
                .tool_context
                .services
                .findings
                .expect("findings options always set")
                .profile;
            assert_eq!(
                got, expected,
                "step={step:?} defaults={defaults:?} agent={agent:?}"
            );
        }
    }

    /// Delegates to a real [`DefaultStepFactory`] and records the findings
    /// profile each build resolved, then swaps in a scripted provider so the
    /// run completes without a network call.
    struct RecordingFactory {
        inner: DefaultStepFactory,
        seen: std::sync::Mutex<Vec<(String, rupu_coverage::FindingProfile)>>,
    }

    #[async_trait::async_trait]
    impl StepFactory for RecordingFactory {
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
        ) -> rupu_agent::LegacyRunOpts {
            let mut opts = self
                .inner
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
                .await;
            let profile = opts
                .tool_context
                .services
                .findings
                .as_ref()
                .expect("findings options always set")
                .profile;
            self.seen
                .lock()
                .unwrap()
                .push((step_id.to_string(), profile));
            opts.provider = Box::new(rupu_agent::runner::MockProvider::new(vec![
                rupu_agent::runner::ScriptedTurn::AssistantText {
                    text: "done".into(),
                    stop: rupu_providers::types::StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                },
            ]));
            opts
        }
    }

    fn run_opts(
        wf: Workflow,
        dir: &std::path::Path,
        factory: Arc<RecordingFactory>,
    ) -> crate::runner::OrchestratorRunOpts {
        crate::runner::OrchestratorRunOpts {
            run_step: Default::default(),
            workflow: wf,
            inputs: Default::default(),
            workspace_id: "ws_profile".into(),
            naming: None,
            workspace_path: dir.to_path_buf(),
            transcript_dir: dir.join("transcripts"),
            factory,
            event: None,
            issue: None,
            issue_ref: None,
            run_store: None,
            workflow_yaml: None,
            resume_from: None,
            run_id_override: None,
            strict_templates: false,
            event_sink: None,
            unit_dispatcher: None,
            action_dispatcher: None,
            pause: None,
        }
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn for_each_units_resolve_their_steps_profile() {
        use rupu_coverage::FindingProfile::Summary;
        let tmp = assert_fs::TempDir::new().unwrap();
        write_profile_agents(tmp.path());
        // The agent says full and the workflow default says full; only the
        // fan-out step says summary. Every unit must get summary.
        let wf = Workflow::parse(
            "name: fanout\ndefaults:\n  findings_profile: full\nsteps:\n  - id: fan\n    for_each: '[\"a.rs\", \"b.rs\", \"c.rs\"]'\n    agent: fp_full\n    prompt: \"assess {{ item }}\"\n    findings_profile: summary\n",
        )
        .expect("workflow must parse");
        let mut inner = factory(tmp.path().to_path_buf());
        inner.workflow = wf.clone();
        let rec = Arc::new(RecordingFactory {
            inner,
            seen: Default::default(),
        });
        crate::runner::run_workflow(run_opts(wf, tmp.path(), Arc::clone(&rec)))
            .await
            .expect("run completes");
        let seen = rec.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "one build per unit: {seen:?}");
        for (step_id, profile) in seen {
            assert_eq!(step_id, "fan");
            assert_eq!(profile, Summary);
        }
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`) through `factory()`.
    #[tokio::test]
    #[serial_test::serial]
    async fn on_reject_cleanup_sub_steps_resolve_their_own_profile() {
        use rupu_coverage::FindingProfile::{Full, Summary};
        let tmp = assert_fs::TempDir::new().unwrap();
        write_profile_agents(tmp.path());
        // The cleanup sub-step lives under the gate, not in `steps`; the
        // factory finds it there and resolves step → defaults → agent.
        let wf = Workflow::parse(
            "name: gated\ndefaults:\n  findings_profile: full\nsteps:\n  - id: gate\n    approval:\n      required: true\n      on_reject:\n        - id: triage\n          agent: fp_full\n          prompt: p\n          findings_profile: summary\n        - id: note\n          agent: fp_summary\n          prompt: p\n",
        )
        .expect("workflow must parse");
        let mut inner = factory(tmp.path().to_path_buf());
        inner.workflow = wf.clone();
        let rec = Arc::new(RecordingFactory {
            inner,
            seen: Default::default(),
        });
        let mut opts = run_opts(wf, tmp.path(), Arc::clone(&rec));
        opts.resume_from = Some(crate::runner::ResumeState::from_rejection(
            "run_gated".into(),
            Vec::new(),
            "gate".into(),
            "not today".into(),
        ));
        crate::runner::run_reject_cleanup(opts, "gate", "not today", "cli", None)
            .await
            .expect("cleanup completes");
        let seen = rec.seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![("triage".to_string(), Summary), ("note".to_string(), Full)],
            "the step's own profile, else the workflow default over the agent's"
        );
    }

    // `#[serial]`: reaches the provider factory (reads
    // `RUPU_MOCK_PROVIDER_SCRIPT`), which `generate.rs`'s tests set.
    #[tokio::test]
    #[serial_test::serial]
    async fn bash_config_reaches_the_step_opts() {
        // Regression for ISSUES.md I-18: the workflow path hardcoded a 120s
        // bash timeout and an empty env allowlist at build_opts_for_step's
        // ToolContext construction, so `[bash]` config silently applied
        // under `rupu run`/`rupu session` but not under `rupu workflow run`.
        // A DefaultStepFactory carrying bash_timeout_secs = 42 and
        // env_allowlist = ["FOO"] must produce an LegacyRunOpts whose
        // tool_context carries BOTH through — not 120 / empty.
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let mut f = factory(tmp.path().to_path_buf());
        f.bash_timeout_secs = 42;
        f.bash_env_allowlist = vec!["FOO".to_string()];

        let opts = f
            .build_opts_for_step(
                "unrestricted",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript_bash_config.jsonl"),
                None,
            )
            .await;

        assert_eq!(
            opts.tool_context.workspace.bash.timeout_secs, 42,
            "bash_timeout_secs must flow from the factory, not hardcode 120"
        );
        assert!(
            opts.tool_context
                .workspace
                .bash
                .env_allowlist
                .contains(&"FOO".to_string()),
            "bash_env_allowlist must flow from the factory, not hardcode empty: {:?}",
            opts.tool_context.workspace.bash.env_allowlist
        );
    }

    // `#[serial]`: reaches the provider factory, like the test above.
    #[tokio::test]
    #[serial_test::serial]
    async fn the_customer_reaches_every_step_tool_context() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let mut f = factory(tmp.path().to_path_buf());
        assert_eq!(StepFactory::customer(&f), None);
        f.customer = Some("acme".to_string());
        assert_eq!(StepFactory::customer(&f), Some("acme"));

        let opts = f
            .build_opts_for_step(
                "unrestricted",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript_customer.jsonl"),
                None,
            )
            .await;
        assert_eq!(opts.tool_context.services.customer.as_deref(), Some("acme"));
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

/// Task 4 review, concern #2: a workflow step naming a declared multi-account
/// (e.g. `openai-work`) must resolve its vendor kind through
/// `DefaultStepFactory.kinds`, not fall into `build_for_provider_with_config`'s
/// `_` arm and fail with "unknown provider". Drives the real
/// `build_opts_for_step` — the exact code path `rupu workflow run` uses —
/// against a real (temp-file-backed) `KeychainResolver` with a stored
/// credential, so the assertion is on the REAL built provider's identity, not
/// a mock seam (`RUPU_MOCK_PROVIDER_SCRIPT` short-circuits before kind
/// dispatch is ever reached, so it can't prove this). No network call is
/// made: `OpenAiCodexClient` construction is synchronous, and `provider_id()`
/// is a pure accessor — the test never calls `.send()`.
///
/// Deliberately uses kind `openai`, NOT `anthropic`: `ProviderBuildErrorStub`
/// (the stand-in substituted on a build failure, see its doc comment above)
/// hardcodes `provider_id() -> ProviderId::Anthropic` as a stable log-only
/// placeholder. Asserting `== Anthropic` here would therefore pass whether
/// or not `kinds` actually resolved — a real anthropic-kind build and a
/// FAILED build both report the identical `provider_id()`. `OpenaiCodex` has
/// no such collision, so this genuinely distinguishes "the real client was
/// built" from "the build failed and got stubbed" (verified: this test was
/// run against kind `anthropic` first and false-passed with `kinds` left
/// empty, exactly because of that collision — that off-by-one motivated
/// switching to `openai`).
#[cfg(test)]
mod kind_resolution_end_to_end_tests {
    use super::DefaultStepFactory;
    use crate::runner::StepFactory;
    use crate::workflow::Workflow;
    use serial_test::serial;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    const WF: &str = r#"
name: w
steps:
  - id: named_account_step
    agent: ag_acct_x
    prompt: p
"#;

    /// This test mutates the process-global `RUPU_AUTH_FILE` env var,
    /// which `KeychainResolver::new()` reads unconditionally — including
    /// the five other tests in this binary that construct one. Bare
    /// `set_var`/`remove_var` (the prior implementation) also left the
    /// var set for the rest of the process if the test panicked between
    /// the two calls. Neither window could currently produce a false
    /// pass (the other constructions don't depend on the path), but both
    /// are real non-hermetic gaps — closed with the same
    /// `ENV_LOCK` + `EnvVarGuard` pattern `accounts_sso_e2e.rs` and
    /// `cli_auth.rs` already use.
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    /// RAII guard: sets an env var for the test's duration and restores
    /// whatever value (if any) was already there on drop, even on panic.
    struct EnvVarGuard {
        key: &'static str,
        prior: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let prior = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prior }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn write_agent_pinned_to_named_account(global: &std::path::Path) {
        let agents_dir = global.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("ag_acct_x.md"),
            "---\nname: ag_acct_x\nprovider: acct-x\n---\nDo the thing.\n",
        )
        .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn a_step_naming_a_declared_multi_account_resolves_its_kind() {
        let _guard = ENV_LOCK.lock().await;
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent_pinned_to_named_account(tmp.path());

        // Hermetic credential store: a real `KeychainResolver`, but pointed
        // at a temp file via `RUPU_AUTH_FILE` so this never touches the
        // developer's real `~/.rupu/auth.json`.
        let auth_path = tmp.path().join("auth.json");
        let resolver = {
            let _env = EnvVarGuard::set("RUPU_AUTH_FILE", &auth_path);
            let resolver = rupu_auth::KeychainResolver::new();
            // `store_named` writes `<name>/<mode>` unconditionally — no
            // `AccountSpec` declaration needed for this to be readable back
            // via the resolver's `get_named` fallback path.
            resolver
                .store_named(
                    "acct-x",
                    rupu_providers::AuthMode::ApiKey,
                    &rupu_auth::StoredCredential::api_key("dummy-key-for-kind-resolution-test"),
                )
                .await
                .unwrap();
            resolver
            // `_env` drops here, restoring `RUPU_AUTH_FILE` before any
            // other test in this binary can observe our temp path.
        };

        let mut f = DefaultStepFactory {
            customer: None,
            workflow: Workflow::parse(WF).expect("workflow must parse"),
            global: tmp.path().to_path_buf(),
            project_root: None,
            resolver: Arc::new(resolver),
            mode: rupu_tools::PermissionMode::Bypass,
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            kinds: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
            findings_base: rupu_coverage::FindingWriteOptions::default(),
            limits_ctx: rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                tmp.path().join("cache/models"),
            ),
            providers: Default::default(),
            recovery: Default::default(),
            scope_name_override: None,
            net_capture: None,
        };
        // The account `acct-x` is declared as kind `openai` — a builtin
        // vendor, but a name the factory's dispatch `match` would never
        // recognize on its own.
        f.kinds.insert("acct-x".to_string(), "openai".to_string());

        // Limit discovery (spec 2026-09-30 §6.1) asks the REAL provider for its
        // model list when no fresh cache exists. Seed a fresh (empty) one so
        // this test never reaches the network with its dummy credential.
        let registry =
            rupu_providers::ModelRegistry::with_cache_dir(tmp.path().join("cache/models"));
        registry.set_live_cache("acct-x", Vec::new()).await;
        registry.save_cache("acct-x").await.unwrap();

        let opts = f
            .build_opts_for_step(
                "named_account_step",
                "ag_acct_x",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;

        assert_eq!(opts.provider_name, "acct-x");
        // If `kinds` were ignored (the pre-fix bug), `acct-x` falls into the
        // `_` arm with no `openai_compatible` declared, producing a
        // `ProviderBuildErrorStub` whose `provider_id()` is the hardcoded
        // `Anthropic` placeholder — NOT `OpenaiCodex`. Getting a real
        // `OpenAiCodexClient` here proves the kind resolved.
        assert_eq!(
            opts.provider.provider_id(),
            rupu_providers::ProviderId::OpenaiCodex,
            "a declared multi-account's kind did not reach the provider build"
        );
    }

    const WF_LIMITS: &str = r#"
name: w
steps:
  - id: pinned_step
    agent: pinned
    prompt: p
  - id: missing_step
    agent: no_such_agent
    prompt: p
"#;

    fn limits_factory(tmp: &std::path::Path) -> DefaultStepFactory {
        DefaultStepFactory {
            customer: None,
            workflow: Workflow::parse(WF_LIMITS).expect("workflow must parse"),
            global: tmp.to_path_buf(),
            project_root: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            mode: rupu_tools::PermissionMode::Bypass,
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            kinds: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
            findings_base: rupu_coverage::FindingWriteOptions::default(),
            limits_ctx: rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                tmp.join("cache/models"),
            ),
            providers: Default::default(),
            recovery: Default::default(),
            scope_name_override: None,
            net_capture: None,
        }
    }

    /// Spec 2026-09-30 §6.1: a workflow step resolves the limits of ITS
    /// agent's provider/model through the runtime resolver — agent pins first,
    /// then the live model list. Hermetic: a real (non-stub) Anthropic client
    /// built from a temp-file credential store (no network at build), plus a
    /// fresh seeded model-list cache so discovery never leaves the process.
    #[tokio::test]
    #[serial]
    async fn a_step_resolves_its_agents_limits_from_pins_then_the_live_list() {
        let _guard = ENV_LOCK.lock().await;
        let tmp = assert_fs::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("pinned.md"),
            "---\nname: pinned\nprovider: anthropic\nmodel: claude-test-1\n\
             maxTokens: 1234\n---\nDo it.\n",
        )
        .unwrap();

        let resolver = {
            let _env = EnvVarGuard::set("RUPU_AUTH_FILE", &tmp.path().join("auth.json"));
            let resolver = rupu_auth::KeychainResolver::new();
            resolver
                .store_named(
                    "anthropic",
                    rupu_providers::AuthMode::ApiKey,
                    &rupu_auth::StoredCredential::api_key("dummy-key-for-limits-test"),
                )
                .await
                .unwrap();
            resolver
        };
        let mut f = limits_factory(tmp.path());
        f.resolver = Arc::new(resolver);

        let registry =
            rupu_providers::ModelRegistry::with_cache_dir(tmp.path().join("cache/models"));
        registry
            .set_live_cache(
                "anthropic",
                vec![rupu_providers::ModelInfo {
                    id: "claude-test-1".to_string(),
                    provider: rupu_providers::ProviderId::Anthropic,
                    context_window: 300_000,
                    max_output_tokens: 50_000,
                    capabilities: Vec::new(),
                    cost: rupu_providers::ModelCost::default(),
                    status: rupu_providers::ModelStatus::default(),
                }],
            )
            .await;
        registry.save_cache("anthropic").await.unwrap();

        let opts = f
            .build_opts_for_step(
                "pinned_step",
                "pinned",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;

        use rupu_providers::model_limits::LimitSource;
        let l = &opts.limits;
        // Pinned by the agent...
        assert_eq!(l.output.tokens, Some(1234));
        assert!(matches!(l.output.source, LimitSource::Agent));
        // ...and discovered for the field the agent left unpinned.
        assert_eq!(l.input.tokens, Some(300_000));
        assert!(
            matches!(l.input.source, LimitSource::Live { .. }),
            "an unpinned limit comes from the live list: {:?}",
            l.input.source
        );
        assert_eq!(l.note, None);
    }

    /// A step whose agent file is missing gets a load-error stub provider:
    /// there is no provider/model to ask, so its limits are plain unknown —
    /// no discovery attempt against "unresolved", no misleading note.
    #[tokio::test]
    #[serial]
    async fn a_step_whose_agent_fails_to_load_gets_unknown_limits() {
        let _guard = ENV_LOCK.lock().await;
        let tmp = assert_fs::TempDir::new().unwrap();
        let f = limits_factory(tmp.path());

        let opts = f
            .build_opts_for_step(
                "missing_step",
                "no_such_agent",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("transcript.jsonl"),
                None,
            )
            .await;

        assert_eq!(opts.provider_name, "unresolved");
        assert_eq!(opts.limits.input.tokens, None);
        assert_eq!(opts.limits.output.tokens, None);
        assert_eq!(opts.limits.note, None);
        assert!(
            !tmp.path().join("cache/models").exists(),
            "no model-list cache may be written for a stub provider"
        );
    }

    /// A step's agent `fallbacks:` reach the runner with a hop builder; a step
    /// whose agent does not load gets no ladder (there is no reply to recover).
    #[tokio::test]
    #[serial]
    async fn a_step_carries_its_agents_fallback_chain_and_a_hop_builder() {
        let _guard = ENV_LOCK.lock().await;
        let _script = EnvVarGuard::set(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            std::path::Path::new(r#"[{"AssistantText":{"text":"ok","stop":"end_turn"}}]"#),
        );
        let tmp = assert_fs::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("pinned.md"),
            "---\nname: pinned\nprovider: anthropic\nmodel: claude-test-1\n\
             fallbacks:\n  - model: claude-test-2\n---\nDo it.\n",
        )
        .unwrap();
        let mut f = limits_factory(tmp.path());
        f.recovery = rupu_config::RecoveryConfig {
            fallbacks: vec![rupu_config::FallbackEntry {
                provider: None,
                model: "from-config".into(),
            }],
            server_side_fallback: true,
        };
        let build = |step: &'static str, agent: &'static str| {
            f.build_opts_for_step(
                step,
                agent,
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join(format!("{step}.jsonl")),
                None,
            )
        };

        let opts = build("pinned_step", "pinned").await;
        assert_eq!(
            opts.recovery.chain,
            vec![rupu_config::FallbackEntry {
                provider: None,
                model: "claude-test-2".into(),
            }]
        );
        assert!(opts.recovery.hop_builder.is_some());

        let stub = build("missing_step", "no_such_agent").await;
        assert!(stub.recovery.chain.is_empty());
        assert!(stub.recovery.hop_builder.is_none());
    }
}
