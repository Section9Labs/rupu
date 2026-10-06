//! `CliAgentDispatcher` — the cli-side `AgentDispatcher` impl that the
//! `dispatch_agent` builtin tool calls into.
//!
//! Spawns a child agent run synchronously: loads the agent spec,
//! allocates a sub-run directory under the parent's run dir, builds a
//! provider via [`rupu_runtime::provider_factory`], threads the same
//! dispatcher Arc into the child's [`ToolContext`] (so grandchildren
//! up to `MAX_DEPTH` can dispatch too), runs the child to completion,
//! and reads the final assistant text out of the persisted transcript.
//!
//! See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.

use async_trait::async_trait;
use rupu_agent::runner::{run_agent, AgentRunOpts, BypassDecider, PermissionDecider};
use rupu_orchestrator::executor::{Event as OrchEvent, EventSink};
use rupu_orchestrator::RunStore;
use rupu_runtime::provider_factory;
use rupu_tools::{AgentDispatcher, DispatchError, DispatchOutcome, ToolContext};
use rupu_transcript::{Event as TxEvent, JsonlReader, JsonlWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// CLI-side dispatcher. Holds the shared run-store + auth + workspace
/// state needed to spawn a child run, and a self-reference so children
/// inherit the same dispatcher Arc on their tool context.
pub struct CliAgentDispatcher {
    global: PathBuf,
    project_root: Option<PathBuf>,
    workspace_id: String,
    workspace_path: PathBuf,
    resolver: Arc<rupu_auth::KeychainResolver>,
    parent_mode_str: String,
    mcp_registry: Arc<rupu_scm::Registry>,
    run_store: Arc<RunStore>,
    /// Self-reference as a trait object so each child's `ToolContext`
    /// can carry the same dispatcher Arc — without it, grandchildren
    /// would see `dispatcher: None` and fail with "no dispatcher".
    /// Populated by [`Self::new`] after Arc construction.
    self_dyn: OnceLock<Arc<dyn AgentDispatcher>>,
    /// The parent run's event sink, if one is wired up. Lets `dispatch()`
    /// emit `DispatchStarted`/`DispatchCompleted` so the live view can
    /// render the child as a node under the active step. `None` in
    /// contexts with no run-level events.jsonl (e.g. some test harnesses)
    /// — emission is then a no-op and behavior is unchanged.
    event_sink: Option<Arc<dyn EventSink>>,
    /// `default_provider` from `config.toml`. Used when the dispatched
    /// agent pins no `provider:`. `None` falls back to
    /// `provider_factory::FALLBACK_PROVIDER`.
    default_provider: Option<String>,
    /// `default_model` from `config.toml`. Used when the dispatched agent
    /// pins no `model:` — without it a sub-agent resolved a different model
    /// than the very same agent would as a top-level `rupu run` or a
    /// workflow step (ISSUES.md I-8, the fourth I-1/I-2 site).
    default_model: Option<String>,
    /// OpenAI-compatible provider params resolved from `config.toml`, keyed
    /// by provider name. Lets a dispatched sub-agent reach a config-declared
    /// `[providers.<name>] kind = "openai-compatible"` endpoint the same way
    /// `rupu run` and workflow steps do. Empty when none are declared.
    openai_compatible: std::collections::HashMap<String, provider_factory::OpenAiCompatibleParams>,
    /// Resolved `[providers.<name>]` runtime knobs, keyed by provider name.
    /// Lets a dispatched sub-agent honor `timeout_ms` / `max_retries` /
    /// `max_concurrency` / `org_id` exactly as `rupu run` does
    /// (ISSUES.md I-9…I-12). Empty ⇒ documented defaults.
    provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
    /// Resolved vendor kind per declared account name
    /// (`provider_factory::resolve_kind_map`). Lets `dispatch_agent`/
    /// `dispatch_agents_parallel` reach a named multi-account (e.g.
    /// `anthropic-work`) the same way `rupu run` does — without this, a
    /// dispatched sub-agent naming such an account falls into
    /// `build_for_provider_with_config`'s `_` arm and fails with "unknown
    /// provider" (Task 4 review, concern #2). Empty ⇒ every dispatched
    /// sub-agent's provider dispatches by name only, same as before this
    /// field existed.
    kinds: std::collections::HashMap<String, String>,
    /// Findings artifact store + `[findings]` limits handed to every
    /// dispatched child's `ToolContext`; the child's own resolved profile
    /// (`spec.findings_profile`, else `full`) is set on a clone per dispatch.
    findings_base: rupu_coverage::FindingWriteOptions,
    /// The run's codename namer, shared with the orchestrator's
    /// `RunNaming` (see [`Self::set_namer`]) so sub-agent role words and
    /// `#n` counters come from — and persist to — the same
    /// `codenames.json` as the run's static slots. `None` until installed;
    /// [`Self::namer_for`] then falls back to an in-memory namer.
    namer: std::sync::Mutex<Option<rupu_codename::SharedNamer>>,
    /// The process-wide subprocess-capture backend (see
    /// [`Self::set_net_capture`]); handed to every child's `ToolContext`.
    net_capture: std::sync::Mutex<Option<Arc<dyn rupu_netflow::SubprocessCapture>>>,
    /// The ROOT workflow run's usage ledger (`<run>/usage.jsonl`), when this
    /// dispatcher serves a workflow run. Every dispatched child (and its own
    /// grandchildren, which reuse this dispatcher) appends its per-LLM-call
    /// rows here, tagged with the child's own run id and the dispatching
    /// agent's run id as parent — the CP fold attributes them to the
    /// ancestor's step via `parent_agent_run_id`. `None` for standalone
    /// `rupu run` (no workflow run to charge; those children are counted by
    /// the CP's fallback over sub-run transcripts).
    usage_ledger: Option<rupu_orchestrator::usage_ledger::UsageLedger>,
    /// Model-limit discovery context (cache dir + `[providers.*].models`
    /// overrides). Every dispatched child resolves its OWN provider/model's
    /// limits through it (spec 2026-09-30 §6.1), not the parent's.
    limits_ctx: rupu_runtime::model_limits::LimitsContext,
    /// The parent `rupu run`'s coverage stream; children append to it.
    coverage_stream: Option<PathBuf>,
    /// `[providers.<name>]` from `config.toml`: what a dispatched child's
    /// fallback hops are built from (`rupu_runtime::hop_builder`).
    providers: std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
    /// `[recovery]` from `config.toml`: the fallback table a child uses when
    /// its agent declares no `fallbacks:`, and the server-side-fallback
    /// toggle for the child's provider and its hops.
    recovery: rupu_config::RecoveryConfig,
}

impl std::fmt::Debug for CliAgentDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CliAgentDispatcher")
            .field("global", &self.global)
            .field("project_root", &self.project_root)
            .field("workspace_id", &self.workspace_id)
            .field("workspace_path", &self.workspace_path)
            .field("parent_mode_str", &self.parent_mode_str)
            .finish()
    }
}

impl CliAgentDispatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        global: PathBuf,
        project_root: Option<PathBuf>,
        workspace_id: String,
        workspace_path: PathBuf,
        resolver: Arc<rupu_auth::KeychainResolver>,
        parent_mode_str: String,
        mcp_registry: Arc<rupu_scm::Registry>,
        run_store: Arc<RunStore>,
        event_sink: Option<Arc<dyn EventSink>>,
        default_provider: Option<String>,
        default_model: Option<String>,
        openai_compatible: std::collections::HashMap<
            String,
            provider_factory::OpenAiCompatibleParams,
        >,
        provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
        kinds: std::collections::HashMap<String, String>,
        findings_base: rupu_coverage::FindingWriteOptions,
        usage_ledger: Option<rupu_orchestrator::usage_ledger::UsageLedger>,
        limits_ctx: rupu_runtime::model_limits::LimitsContext,
        coverage_stream: Option<PathBuf>,
        providers: std::collections::BTreeMap<String, rupu_config::ProviderConfig>,
        recovery: rupu_config::RecoveryConfig,
    ) -> Arc<Self> {
        let arc = Arc::new(Self {
            global,
            project_root,
            workspace_id,
            workspace_path,
            resolver,
            parent_mode_str,
            mcp_registry,
            run_store,
            self_dyn: OnceLock::new(),
            event_sink,
            default_provider,
            default_model,
            openai_compatible,
            provider_tuning,
            kinds,
            findings_base,
            namer: std::sync::Mutex::new(None),
            net_capture: std::sync::Mutex::new(None),
            usage_ledger,
            limits_ctx,
            coverage_stream,
            providers,
            recovery,
        });
        let dyn_arc: Arc<dyn AgentDispatcher> = arc.clone();
        let _ = arc.self_dyn.set(dyn_arc);
        arc
    }

    /// Install the process-wide subprocess-capture backend (warmed via
    /// [`crate::netflow_sink::net_capture`]) so every dispatched child's
    /// `ToolContext` shares the parent's one capture while keeping its own
    /// per-run sink. Production callers MUST install it (the choke-point
    /// test enforces this); an uninstalled dispatcher (tests) gives children
    /// no capture.
    pub fn set_net_capture(&self, capture: Arc<dyn rupu_netflow::SubprocessCapture>) {
        if let Ok(mut g) = self.net_capture.lock() {
            *g = Some(capture);
        }
    }

    /// Install the run's codename namer (`RunNaming::namer()`), so
    /// sub-agents are named from the same allocator + counters as the
    /// run's static slots.
    pub fn set_namer(&self, namer: rupu_codename::SharedNamer) {
        if let Ok(mut g) = self.namer.lock() {
            *g = Some(namer);
        }
    }

    /// The run's namer, or — when none was installed (a dispatcher whose
    /// caller had no RunNaming) — an in-memory one for the parent's crew, so
    /// sub-agents are always named.
    fn namer_for(&self, parent: &rupu_codename::Codename) -> Option<rupu_codename::SharedNamer> {
        let mut g = self.namer.lock().ok()?;
        Some(
            g.get_or_insert_with(|| {
                rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new(
                    parent.crew.clone(),
                ))
            })
            .clone(),
        )
    }

    fn self_arc_dyn(&self) -> Arc<dyn AgentDispatcher> {
        self.self_dyn
            .get()
            .expect("CliAgentDispatcher::new always populates self_dyn")
            .clone()
    }

    /// Best-effort `DispatchCompleted` emission — guards the `Option` and
    /// never fails the child (or parent) run. Called from every exit
    /// path of `dispatch()` reached after the matching `DispatchStarted`
    /// was emitted. `cause` is the child's typed failure, when it failed
    /// on a classified response outcome.
    fn emit_dispatch_completed(
        &self,
        parent_run_id: &str,
        sub_run_id: &str,
        success: bool,
        tokens_in: u64,
        tokens_out: u64,
        cause: Option<rupu_transcript::OutcomeRecord>,
    ) {
        if let Some(sink) = &self.event_sink {
            sink.emit(
                parent_run_id,
                &OrchEvent::DispatchCompleted {
                    run_id: parent_run_id.to_string(),
                    sub_run_id: sub_run_id.to_string(),
                    success,
                    tokens_in,
                    tokens_out,
                    cause,
                },
            );
        }
    }
}

#[async_trait]
impl AgentDispatcher for CliAgentDispatcher {
    async fn dispatch(
        &self,
        agent_name: &str,
        prompt: String,
        parent_run_id: &str,
        parent_depth: u32,
        parent_codename: Option<&str>,
    ) -> Result<DispatchOutcome, DispatchError> {
        // KNOWN LIMITATION (tool_audit design §4/review IMPORTANT 4): the
        // `AgentDispatcher::dispatch` trait (rupu-tools) takes no narrowed
        // tool roster and no audit callback, and this is the ONLY
        // production impl — `dispatch_agent`/`dispatch_agents_parallel`
        // call straight into it via `ctx.dispatcher`. So a step narrowed by
        // `actions:` (e.g. read-only `issues.*`) whose agent retains
        // `dispatch_agent` (a builtin, itself correctly exempt from
        // narrowing per spec §2) can have its CHILD agent run with the
        // child's OWN unrestricted `tools:` grant — `agent_tools:
        // spec.tools.clone()` below never intersects with whatever the
        // calling step narrowed — and every catalog call the child makes
        // is invisible to this run's `tool_audit` trail (the child gets
        // its own transcript/audit machinery, but nothing here ties it
        // back to the parent step's narrowing). Threading the narrowed
        // roster + an audit callback through would require changing the
        // `AgentDispatcher` trait signature and `ToolContext` (rupu-tools,
        // a port crate `rupu-agent` itself must not gain a reverse
        // dependency on) plus every impl/mock — out of scope for this
        // fix. Never silent: warn on every dispatch, and leave a
        // transcript-visible notice on the child's own run (below).
        tracing::warn!(
            child_agent = agent_name,
            parent_run_id,
            "dispatch_agent bypasses step `actions:` narrowing: the child inherits its OWN \
             agent's `tools:` grant verbatim, not the parent step's narrowed roster, and its \
             tool calls are not covered by the parent step's tool_audit trail (known \
             limitation — see docs/superpowers/specs/2026-07-26-rupu-step-actions-enforcement-design.md)"
        );

        let project_agents_parent = self.project_root.as_ref().map(|p| p.join(".rupu"));
        // Admission-paced (see `rupu_agent::fd_budget`): a parallel dispatch
        // near the open-file limit grows it or waits instead of failing.
        let spec = rupu_agent::load_agent_admitted(
            &self.global,
            project_agents_parent.as_deref(),
            agent_name,
        )
        .await
        .map_err(|e| match e {
            rupu_agent::AgentLoadError::NotFound(_) => DispatchError::AgentNotFound {
                agent: agent_name.to_string(),
            },
            // Surface the real cause (e.g. fd exhaustion, bad frontmatter)
            // instead of a misleading "not found".
            other => DispatchError::Io(std::io::Error::other(format!(
                "load agent `{agent_name}`: {other}"
            ))),
        })?;

        let (sub_run_id, transcript_path) = self
            .run_store
            .create_sub_run(parent_run_id, agent_name)
            .map_err(|e| DispatchError::RunStore(e.to_string()))?;

        // `<parent>><role>#n`, from the run's shared namer. A parent with
        // no (or an unparseable) codename leaves the child unnamed.
        let codename = parent_codename.and_then(|p| {
            let parent: rupu_codename::Codename = p.parse().ok()?;
            let namer = self.namer_for(&parent)?;
            child_codename(&namer, p, agent_name)
        });

        // Netflow capture for the CHILD run this dispatch is about to
        // start — its own sink, scoped to `sub_run_id`, not the parent's.
        // A dispatched sub-agent is its own run with its own transcript
        // (`create_sub_run` above), so its outbound HTTP must land in its
        // own ledger, never the parent's and never nothing.
        let (netflow_sink, netflow_handle) = crate::netflow_sink::for_run(
            &self.global,
            self.project_root.as_deref(),
            &sub_run_id,
            &transcript_path,
        );

        // Provider/model resolution goes through the SHARED resolvers, the
        // same sequence `rupu run` (`cmd/run.rs`), `rupu session` and
        // `DefaultStepFactory` use. This used to hardcode
        // `anthropic`/`claude-sonnet-4-6`, so a dispatched sub-agent that
        // pinned neither ignored `default_provider`/`default_model` and could
        // never reach a config-declared openai-compatible provider — the same
        // agent resolved differently depending on how it was launched
        // (ISSUES.md I-8, the fourth I-1/I-2 call site).
        let provider_name = provider_factory::resolve_provider_name(
            spec.provider.as_deref(),
            self.default_provider.as_deref(),
        );
        let oai_params = self.openai_compatible.get(&provider_name).cloned();
        // Prefer the agent's pinned model; for an openai-compatible provider
        // fall back to its configured default_model.
        let model = provider_factory::resolve_model(
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
        let built = provider_factory::build_for_provider_with_config(
            &provider_name,
            &model,
            spec.auth,
            self.resolver.as_ref(),
            &provider_config,
            netflow_sink.clone(),
        )
        .await;

        // Emitted once the provider build has been attempted, so it can
        // carry who (codename) runs on what (provider · model). Emitted on
        // the build-failure path too, so the live view still shows the
        // failed child node that the `DispatchCompleted` below closes.
        if let Some(sink) = &self.event_sink {
            sink.emit(
                parent_run_id,
                &OrchEvent::DispatchStarted {
                    run_id: parent_run_id.to_string(),
                    sub_run_id: sub_run_id.clone(),
                    agent: Some(agent_name.to_string()),
                    transcript_path: transcript_path.clone(),
                    codename: codename.clone(),
                    provider: Some(provider_name.clone()),
                    model: Some(model.clone()),
                },
            );
        }

        let mut provider = match built {
            Ok((_resolved, p)) => p,
            Err(e) => {
                self.emit_dispatch_completed(parent_run_id, &sub_run_id, false, 0, 0, None);
                if let Some(h) = netflow_handle {
                    h.shutdown().await;
                }
                return Err(DispatchError::ProviderBuild(e.to_string()));
            }
        };

        // Resolve the child's own limits: its provider and model, its agent's
        // pins (spec 2026-09-30 §6.1).
        let limits = rupu_runtime::model_limits::resolve(
            rupu_runtime::model_limits::LimitOverrides::from_spec(&spec),
            &provider_name,
            &model,
            provider.as_mut(),
            &self.limits_ctx,
        )
        .await;

        let child_mode_str = spec
            .permission_mode
            .clone()
            .unwrap_or_else(|| self.parent_mode_str.clone());
        let child_depth = parent_depth + 1;

        let child_tool_ctx = ToolContext {
            findings: Some(self.findings_base.clone().with_profile(
                rupu_coverage::FindingProfile::resolve(None, None, spec.findings_profile),
            )),
            workspace_path: self.workspace_path.clone(),
            bash_env_allowlist: Vec::new(),
            bash_timeout_secs: 120,
            dispatcher: Some(self.self_arc_dyn()),
            dispatchable_agents: spec.dispatchable_agents.clone(),
            parent_run_id: Some(sub_run_id.clone()),
            depth: child_depth,
            coverage_writer: None,
            surface_tag: None,
            run_id: None,
            model: None,
            tool_mappings: None,
            codename: codename.clone(),
            agent: None,
            provider: None,
            coverage_stream: self.coverage_stream.clone(),
            netflow_sink: Some(netflow_sink.clone()),
            net_capture: self.net_capture.lock().ok().and_then(|g| g.clone()),
            tool_call_id: None,
        };

        // What a fallback hop keeps from this child's agent: exactly what its
        // primary provider's `ProviderConfig` and auth hint above carry.
        let hop_overrides = rupu_runtime::hop_builder::AgentOverrides {
            oauth_prefix: spec.anthropic_oauth_prefix,
            prompt_cache: spec.anthropic_prompt_cache,
            auth: spec.auth,
            origin_provider: provider_name.clone(),
        };
        let opts = AgentRunOpts {
            seed_source: None,
            collectors: Vec::new(),
            extra_tools: Vec::new(),
            agent_name: spec.name.clone(),
            agent_system_prompt: spec.system_prompt.clone(),
            agent_tools: spec.tools.clone(),
            provider,
            provider_name,
            model,
            run_id: sub_run_id.clone(),
            workspace_id: self.workspace_id.clone(),
            workspace_path: self.workspace_path.clone(),
            transcript_path: transcript_path.clone(),
            max_turns: spec.max_turns.unwrap_or(50),
            decider: Arc::new(BypassDecider) as Arc<dyn PermissionDecider>,
            tool_context: child_tool_ctx,
            user_message: prompt,
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: child_mode_str,
            no_stream: false,
            // The parent's printer renders the child as a callout from
            // the `dispatch_agent` tool result; suppress the child's
            // own stdout writes so they don't double up.
            suppress_stream_stdout: true,
            mcp_registry: Some(Arc::clone(&self.mcp_registry)),
            effort: spec.effort,
            context_window: spec.context_window,
            output_format: spec.output_format,
            output_schema: spec.output_schema.clone(),
            anthropic_task_budget: spec.anthropic_task_budget,
            anthropic_context_management: spec.anthropic_context_management,
            anthropic_speed: spec.anthropic_speed,
            parent_run_id: Some(parent_run_id.to_string()),
            depth: child_depth,
            dispatchable_agents: spec.dispatchable_agents.clone(),
            step_id: String::new(),
            on_tool_call: None,
            on_stream_event: None,
            // Charge this child's LLM calls to the ROOT run's ledger. No
            // step tag: the CP fold attributes it to its ancestor's step via
            // `parent_agent_run_id` (the dispatching agent's run id).
            on_usage: self.usage_ledger.as_ref().map(|l| {
                l.hook(
                    rupu_orchestrator::usage_ledger::LedgerTag::default(),
                    sub_run_id.clone(),
                    Some(parent_run_id.to_string()),
                    transcript_path.clone(),
                    agent_name.to_string(),
                    None,
                )
            }),
            concerns: spec.concerns.clone(),
            limits,
            scope_name: None,
            surface_tag: None,
            pause: None,
            codename: codename.clone(),
            recovery: rupu_runtime::hop_builder::recovery_opts(
                &self.recovery,
                spec.fallbacks.as_deref(),
                self.resolver.clone(),
                self.providers.clone(),
                self.limits_ctx.clone(),
                netflow_sink,
                hop_overrides,
            ),
        };

        let started = std::time::Instant::now();
        let run_result = match run_agent(opts).await {
            Ok(r) => r,
            Err(e) => {
                self.emit_dispatch_completed(
                    parent_run_id,
                    &sub_run_id,
                    false,
                    0,
                    0,
                    e.outcome().cloned(),
                );
                if let Some(h) = netflow_handle {
                    h.shutdown().await;
                }
                return Err(DispatchError::ChildRun(e.to_string()));
            }
        };
        let duration_ms = started.elapsed().as_millis() as u64;

        // Flush the child's ledger now that its run is over — see
        // `crate::netflow_sink::for_run`'s doc comment.
        if let Some(h) = netflow_handle {
            h.shutdown().await;
        }

        write_delegation_narrowing_notice(&transcript_path, agent_name, parent_run_id);

        // `run_agent` returning `Ok` does NOT mean the child did its job: an
        // `Err` is reserved for failures that stopped the loop from
        // proceeding at all, so a child that burned its whole `max_turns`
        // budget comes back `Ok(RunResult { status: Error, .. })`. This used
        // to be hard-coded `true`, which reported a truncated child to the
        // parent agent as `"ok": true` (see `rupu-tools`'s `dispatch_agent`
        // / `dispatch_agents_parallel` result bodies) — the delegation
        // sibling of the workflow-step bug fixed in `rupu-orchestrator`'s
        // `dispatch_one`.
        //
        // A failed child is still an `Ok(DispatchOutcome)` rather than a
        // `DispatchError::ChildRun`, so the parent gets the child's sub-run
        // id and transcript with an honest `ok: false`. Its `output` is
        // empty (spec 2026-10-01 §5.3): the child's last text is an interim
        // message or a cut-off chain, not an answer. `error` carries why it
        // failed, which the dispatch tools put in the body the parent reads —
        // without the rung-3 hint (`RunResult::failure_reason`): the parent
        // model cannot `rupu run --continue` its child. The child's own
        // transcript and run record keep the hint.
        let terminal = run_result.terminal_error();
        if let Some(err) = &terminal {
            tracing::warn!(
                agent = %agent_name,
                sub_run_id = %sub_run_id,
                error = %err,
                "child agent run did not complete cleanly; reporting the dispatch as failed"
            );
        }
        let success = terminal.is_none();
        let output = if success {
            read_final_assistant_text(&transcript_path).unwrap_or_default()
        } else {
            String::new()
        };
        let error = run_result.failure_reason();

        self.emit_dispatch_completed(
            parent_run_id,
            &sub_run_id,
            success,
            run_result.total_tokens_in,
            run_result.total_tokens_out,
            terminal.as_ref().and_then(|e| e.outcome()).cloned(),
        );

        Ok(DispatchOutcome {
            agent: agent_name.to_string(),
            sub_run_id,
            codename,
            transcript_path,
            output,
            success,
            error,
            tokens_used: run_result.total_tokens_in + run_result.total_tokens_out,
            duration_ms,
        })
    }
}

/// Mint a dispatched child's codename: `<parent>><role>#n`, where `role`
/// is the agent def's canonical word in the crew and `n` the next
/// per-(parent, role) instance. `None` when `parent` is not a codename.
pub(crate) fn child_codename(
    namer: &rupu_codename::SharedNamer,
    parent: &str,
    agent: &str,
) -> Option<String> {
    let parent: rupu_codename::Codename = parent.parse().ok()?;
    Some(namer.with(|n| {
        let role = n.canonical_role(agent);
        let k = n.next_instance(&parent, &role);
        parent.child(&role, Some(k)).to_string()
    }))
}

/// Append a transcript-visible notice to the CHILD's OWN transcript
/// recording that its tool calls are not covered by the parent step's
/// `actions:` narrowing or `tool_audit` trail (IMPORTANT 4's fallback —
/// see the doc comment on `dispatch()`). Written AFTER `run_agent`
/// returns (appending before it would be wiped: `run_agent` truncates
/// `transcript_path` via `JsonlWriter::create` as its very first step).
///
/// Deliberately reuses the plain `ToolCall`/`ToolResult` shapes (already
/// rendered by the CP transcript panel with zero new frontend code)
/// rather than overloading `Event::ToolAudit`'s `declared`/`granted`/
/// `blocked`/`restricted` fields — this call site has no way to know
/// whether the parent step was actually narrowed, so inventing values
/// for those fields would itself be a false record in an audit trail
/// that must never lie. No `error:` set either: this is a known
/// architectural limitation, not necessarily evidence anything went
/// wrong on this particular call, so it renders as a neutral note, not
/// an alarm.
///
/// Best-effort: a write failure is logged and swallowed, same as every
/// other observability side-channel in this arc — never allowed to
/// fail the dispatch it's annotating.
fn write_delegation_narrowing_notice(
    transcript_path: &Path,
    child_agent: &str,
    parent_run_id: &str,
) {
    let call_id = format!("delegation_narrowing_notice_{child_agent}");
    let note = format!(
        "KNOWN LIMITATION: this child agent (`{child_agent}`, dispatched from parent run \
         `{parent_run_id}`) was launched via `dispatch_agent` with its OWN agent's `tools:` \
         grant verbatim. If the parent workflow step declared a non-empty `actions:` \
         allowlist, that narrowing was NOT applied to this child, and none of the child's \
         own tool calls are covered by the parent step's `tool_audit` trail. See \
         docs/superpowers/specs/2026-07-26-rupu-step-actions-enforcement-design.md."
    );
    match JsonlWriter::append(transcript_path) {
        Ok(mut w) => {
            let write_result = w
                .write(&TxEvent::ToolCall {
                    call_id: call_id.clone(),
                    tool: "dispatch_agent_narrowing_notice".to_string(),
                    input: serde_json::json!({ "child_agent": child_agent, "parent_run_id": parent_run_id }),
                })
                .and_then(|_| {
                    w.write(&TxEvent::ToolResult {
                        call_id,
                        output: note,
                        error: None,
                        duration_ms: 0,
                        structured: None,
                    })
                });
            if let Err(e) = write_result {
                tracing::warn!(error = %e, "failed to write delegation-narrowing notice");
            } else if let Err(e) = w.flush() {
                tracing::warn!(error = %e, "failed to flush delegation-narrowing notice");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open child transcript for delegation-narrowing notice");
        }
    }
}

/// Walk the persisted transcript and return the final turn's text
/// ([`rupu_transcript::final_turn_text`]). Used as the child's `output` in
/// the dispatch tool's return payload — the same rule a workflow step's
/// output uses.
fn read_final_assistant_text(path: &Path) -> Option<String> {
    let iter = JsonlReader::iter(path).ok()?;
    rupu_transcript::final_turn_text(iter.flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ENV_LOCK;
    use rupu_transcript::{Event, JsonlWriter, RunMode, RunStatus};
    use tempfile::TempDir;

    #[test]
    fn read_final_assistant_text_returns_last_non_empty_assistant_chunk() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut w = JsonlWriter::create(&path).unwrap();
        w.write(&Event::RunStart {
            run_id: "r".into(),
            workspace_id: "ws".into(),
            agent: "a".into(),
            provider: "anthropic".into(),
            model: "m".into(),
            started_at: chrono::Utc::now(),
            mode: RunMode::Bypass,
            schema: None,
            system_prompt: None,
            codename: None,
        })
        .unwrap();
        w.write(&Event::AssistantMessage {
            content: "first".into(),
            thinking: None,
        })
        .unwrap();
        w.write(&Event::AssistantMessage {
            content: "  ".into(),
            thinking: None,
        })
        .unwrap();
        w.write(&Event::AssistantMessage {
            content: "final answer".into(),
            thinking: None,
        })
        .unwrap();
        w.write(&Event::RunComplete {
            run_id: "r".into(),
            status: RunStatus::Ok,
            total_tokens: 0,
            duration_ms: 0,
            error: None,
            outcome: None,
        })
        .unwrap();
        w.flush().unwrap();

        assert_eq!(
            read_final_assistant_text(&path),
            Some("final answer".to_string())
        );
    }

    /// A final turn text → thinking → text yields both fragments, the same
    /// rule as a workflow step's output.
    #[test]
    fn read_final_assistant_text_joins_the_final_turn_s_fragments() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut w = JsonlWriter::create(&path).unwrap();
        for e in [
            Event::TurnStart { turn_idx: 0 },
            Event::AssistantMessage {
                content: "checking".into(),
                thinking: None,
            },
            Event::TurnStart { turn_idx: 1 },
            Event::AssistantMessage {
                content: "part one".into(),
                thinking: None,
            },
            Event::Thinking {
                text: Some("hmm".into()),
                provider: "anthropic".into(),
                model: "m".into(),
                raw: serde_json::json!({}),
            },
            Event::AssistantMessage {
                content: "part two".into(),
                thinking: None,
            },
        ] {
            w.write(&e).unwrap();
        }
        w.flush().unwrap();
        assert_eq!(
            read_final_assistant_text(&path),
            Some("part one\n\npart two".to_string())
        );
    }

    #[test]
    fn read_final_assistant_text_returns_none_for_empty_transcript() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_final_assistant_text(&path), None);
    }

    /// Records every emitted event as `(run_id, Event)` for assertion.
    #[derive(Default)]
    struct CapturingSink {
        events: std::sync::Mutex<Vec<(String, OrchEvent)>>,
    }

    impl EventSink for CapturingSink {
        fn emit(&self, run_id: &str, ev: &OrchEvent) {
            self.events
                .lock()
                .unwrap()
                .push((run_id.to_string(), ev.clone()));
        }
    }

    /// Exercises `CliAgentDispatcher::dispatch()` end to end against the
    /// `RUPU_MOCK_PROVIDER_SCRIPT` seam (the same test-only provider
    /// factory hook `rupu-cli`'s own CLI integration tests use — see
    /// `tests/serial/cli_run.rs`) so the child's agent loop runs for real
    /// without any network access. Asserts `DispatchStarted` lands
    /// before `DispatchCompleted`, both carrying the same `sub_run_id`
    /// as the returned `DispatchOutcome`, and that token counts flow
    /// through to `DispatchCompleted`.
    #[tokio::test]
    async fn dispatch_emits_started_then_completed_with_matching_sub_run_id() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let sink = Arc::new(CapturingSink::default());
        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        // Root workflow run's usage ledger. Nothing else creates this file, so
        // any row in it was appended by the dispatched child's `on_usage`.
        let ledger_path = dir.path().join("usage.jsonl");

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            Some(sink.clone() as Arc<dyn EventSink>),
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            Some(rupu_orchestrator::usage_ledger::UsageLedger::open(
                ledger_path.clone(),
            )),
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(
            events.len(),
            2,
            "expected exactly Started + Completed, got {events:?}"
        );

        match &events[0] {
            (
                run_id,
                OrchEvent::DispatchStarted {
                    sub_run_id,
                    agent,
                    transcript_path,
                    ..
                },
            ) => {
                assert_eq!(run_id, "parent_run_1");
                assert_eq!(sub_run_id, &outcome.sub_run_id);
                assert_eq!(agent.as_deref(), Some("child"));
                assert_eq!(transcript_path, &outcome.transcript_path);
            }
            other => panic!("expected DispatchStarted first, got {other:?}"),
        }

        match &events[1] {
            (
                run_id,
                OrchEvent::DispatchCompleted {
                    sub_run_id,
                    success,
                    tokens_in,
                    tokens_out,
                    ..
                },
            ) => {
                assert_eq!(run_id, "parent_run_1");
                assert_eq!(sub_run_id, &outcome.sub_run_id);
                assert!(*success);
                assert_eq!(*tokens_in, 1);
                assert_eq!(*tokens_out, 1);
            }
            other => panic!("expected DispatchCompleted second, got {other:?}"),
        }

        // The child's LLM calls land in the ROOT run's ledger: no step_id
        // (the CP fold attributes a dispatch child to its ancestor's step),
        // the child's own run id, and the dispatching agent's run id as
        // parent.
        let body = std::fs::read_to_string(&ledger_path)
            .expect("dispatched child should have appended to the root run's usage ledger");
        let rows: Vec<rupu_orchestrator::usage_ledger::LedgerRow> = body
            .lines()
            .map(|l| serde_json::from_str(l).expect("ledger row parses"))
            .collect();
        assert!(!rows.is_empty(), "expected >=1 ledger row, got none");
        for row in &rows {
            assert_eq!(row.step_id, None);
            assert_eq!(row.agent_run_id, outcome.sub_run_id);
            assert_eq!(row.parent_agent_run_id.as_deref(), Some("parent_run_1"));
            assert_eq!(row.agent, "child");
            assert_eq!(row.transcript, outcome.transcript_path);
        }
    }

    /// Spec 2026-09-30 §6.1: a dispatched child resolves limits for ITS OWN
    /// agent (pins, provider, model), through the dispatcher's limits context.
    /// Observable on the child's transcript: the run-start `model_limits`
    /// notice states what the child resolved.
    #[tokio::test]
    async fn dispatched_child_resolves_its_own_agents_limits() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/pinnedchild.md"),
            "---\nname: pinnedchild\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n\
             contextWindowTokens: 7000\nmaxTokens: 900\n---\nyou are a child agent.",
        )
        .unwrap();
        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            Arc::new(rupu_auth::KeychainResolver::new()),
            "bypass".into(),
            Arc::new(rupu_scm::Registry::default()),
            Arc::new(RunStore::new(runs_dir)),
            None,
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("pinnedchild", "go".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let outcome = result.expect("dispatch should succeed against the mock provider");

        let transcript = std::fs::read_to_string(&outcome.transcript_path).unwrap();
        assert!(
            transcript.contains("\"kind\":\"model_limits\""),
            "child transcript has no model_limits notice: {transcript}"
        );
        assert!(
            transcript.contains("input 7,000 · output 900"),
            "the notice must state the child's own pinned limits: {transcript}"
        );
    }

    /// Spec 2026-09-30-rupu-remote-findings-transport-design.md deviation 4:
    /// a `dispatch_agent` child shares the dispatching run's coverage stream,
    /// so its findings travel to the coordinator with the unit's. Observable
    /// on the stream file the dispatcher was built with: the child's catalog
    /// and its `report_finding` call both land there, tagged with the child's
    /// scope, after the begin line the parent wrote.
    #[tokio::test]
    async fn dispatched_child_streams_its_coverage_into_the_parents_stream() {
        use rupu_coverage::StreamLine;

        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 4\n\
             findingsProfile: summary\nconcerns:\n  - include: stride\n---\nyou are a child agent.",
        )
        .unwrap();
        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        // The parent's stream, as `rupu run` opens it before any child runs.
        let stream = rupu_coverage::stream_path(&runs_dir, "parent_run_1");
        rupu_coverage::write_stream_begin(&stream, "parent_run_1").unwrap();

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            Arc::new(rupu_auth::KeychainResolver::new()),
            "bypass".into(),
            Arc::new(rupu_scm::Registry::default()),
            Arc::new(RunStore::new(runs_dir)),
            None,
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            Some(stream.clone()),
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[
              { "AssistantToolUse": { "text": null, "tool_id": "call_1", "tool_name": "report_finding", "tool_input": {"scope": "repo", "summary": "child found it", "severity": "low", "evidence": {"rationale": "because"}}, "stop": "tool_use" } },
              { "AssistantText": { "text": "child done", "stop": "end_turn" } }
            ]"#,
        );
        let result = dispatcher
            .dispatch("child", "go".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        result.expect("dispatch should succeed against the mock provider");

        let lines: Vec<StreamLine> = std::fs::read_to_string(&stream)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).expect("stream line parses"))
            .collect();
        assert!(
            matches!(&lines[0], StreamLine::Begin { run_id, .. } if run_id == "parent_run_1"),
            "the parent's begin line stays first: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| matches!(
                l,
                StreamLine::Catalog { scope_name, .. } if scope_name == "child"
            )),
            "the child's catalog must stream into the parent's file: {lines:?}"
        );
        let finding = lines
            .iter()
            .find_map(|l| match l {
                StreamLine::Findings { scope_name, record } => Some((scope_name, record)),
                _ => None,
            })
            .expect("the child's finding must stream into the parent's file");
        assert_eq!(finding.0, "child");
        assert_eq!(finding.1.summary, "child found it");
    }

    #[test]
    fn child_codename_numbers_per_parent_and_role() {
        let namer =
            rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new("jade-reef"));
        let a = child_codename(&namer, "jade-reef/hedgehog", "security-reviewer").unwrap();
        let b = child_codename(&namer, "jade-reef/hedgehog", "security-reviewer").unwrap();
        let c = child_codename(&namer, "jade-reef/heron", "security-reviewer").unwrap();
        assert_eq!(a, "jade-reef/hedgehog>ferret#1");
        assert_eq!(b, "jade-reef/hedgehog>ferret#2");
        assert_eq!(c, "jade-reef/heron>ferret#1");
        assert!(child_codename(&namer, "garbage", "x").is_none());
    }

    /// End to end: a dispatch whose parent has a codename mints the
    /// child's `<parent>><role>#n` from the INSTALLED run namer (so a
    /// second dispatch of the same role climbs to `#2`), stamps it on
    /// `DispatchStarted` together with the resolved provider + model,
    /// returns it on `DispatchOutcome`, and threads it into the child's
    /// own transcript (`RunStart.codename`). A parent with no codename
    /// leaves the child unnamed.
    #[tokio::test]
    async fn dispatch_mints_child_codename_and_stamps_dispatch_started() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/security-reviewer.md"),
            "---\nname: security-reviewer\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou review.",
        )
        .unwrap();
        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));
        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();
        let sink = Arc::new(CapturingSink::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            Arc::new(rupu_auth::KeychainResolver::new()),
            "bypass".into(),
            Arc::new(rupu_scm::Registry::default()),
            run_store,
            Some(sink.clone() as Arc<dyn EventSink>),
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );
        let namer =
            rupu_codename::SharedNamer::in_memory(rupu_codename::CrewNamer::new("jade-reef"));
        dispatcher.set_namer(namer.clone());

        let script = r#"[{ "AssistantText": { "text": "done", "stop": "end_turn" } }]"#;
        let mut outcomes = Vec::new();
        for parent in [Some("jade-reef/heron"), Some("jade-reef/heron"), None] {
            std::env::set_var("RUPU_MOCK_PROVIDER_SCRIPT", script);
            let r = dispatcher
                .dispatch("security-reviewer", "go".into(), "parent_run_1", 0, parent)
                .await;
            std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
            outcomes.push(r.expect("dispatch should succeed against the mock provider"));
        }

        assert_eq!(
            outcomes[0].codename.as_deref(),
            Some("jade-reef/heron>ferret#1")
        );
        assert_eq!(
            outcomes[1].codename.as_deref(),
            Some("jade-reef/heron>ferret#2")
        );
        assert_eq!(outcomes[2].codename, None, "no parent codename => unnamed");
        // The installed namer is the one that was advanced.
        let parent: rupu_codename::Codename = "jade-reef/heron".parse().unwrap();
        assert_eq!(namer.with(|n| n.next_instance(&parent, "ferret")), 3);

        let started: Vec<_> = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, ev)| match ev {
                OrchEvent::DispatchStarted {
                    codename,
                    provider,
                    model,
                    ..
                } => Some((codename.clone(), provider.clone(), model.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(started.len(), 3);
        assert_eq!(
            started[0],
            (
                Some("jade-reef/heron>ferret#1".to_string()),
                Some("anthropic".to_string()),
                Some("claude-sonnet-4-6".to_string())
            )
        );
        assert_eq!(started[2].0, None);

        let run_start_codename = JsonlReader::iter(&outcomes[0].transcript_path)
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|ev| match ev {
                TxEvent::RunStart { codename, .. } => Some(codename),
                _ => None,
            })
            .expect("child transcript has a RunStart");
        assert_eq!(
            run_start_codename.as_deref(),
            Some("jade-reef/heron>ferret#1")
        );
    }

    /// A dispatcher with no namer installed (its caller had no
    /// `RunNaming`) still names sub-agents, from an in-memory namer for
    /// the parent's crew.
    #[tokio::test]
    async fn dispatch_without_installed_namer_still_names_child() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();
        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();
        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            Arc::new(rupu_auth::KeychainResolver::new()),
            "bypass".into(),
            Arc::new(rupu_scm::Registry::default()),
            Arc::new(RunStore::new(runs_dir)),
            None,
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );
        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "done", "stop": "end_turn" } }]"#,
        );
        let r = dispatcher
            .dispatch(
                "child",
                "go".into(),
                "parent_run_1",
                0,
                Some("amber-lantern/otter"),
            )
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
        let role = rupu_codename::role_word("child");
        assert_eq!(
            r.unwrap().codename,
            Some(format!("amber-lantern/otter>{role}#1"))
        );
    }

    /// `event_sink: None` (the harness other dispatch tests already use)
    /// must not change `dispatch()`'s behavior — it's a pure no-op path.
    #[tokio::test]
    async fn dispatch_with_no_sink_still_succeeds() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        assert!(
            result.is_ok(),
            "dispatch with no event sink should behave exactly as before"
        );
    }

    /// Spec 2026-10-01 §5.3: a child that ran and failed reports `ok:
    /// false`, an EMPTY `output` (not its interim text from an earlier
    /// turn), and the reason its run recorded as `error`.
    #[tokio::test]
    async fn a_failed_child_returns_no_output_and_its_error() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();
        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            Arc::new(rupu_auth::KeychainResolver::new()),
            "bypass".into(),
            Arc::new(rupu_scm::Registry::default()),
            Arc::new(RunStore::new(runs_dir)),
            None,
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        // An interim message with a tool call, then a refusal on the final
        // turn with no fallback chain.
        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[
              { "AssistantToolUse": { "text": "Let me read the settings first", "tool_id": "call_1", "tool_name": "read_file", "tool_input": { "path": "absent.toml" }, "stop": "tool_use" } },
              { "Reply": {
                  "content": [{ "type": "text", "text": "I won't do that." }],
                  "stop": { "reason": "refusal", "wire": { "provider": "anthropic", "value": "refusal" } }
              } }
            ]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("a failed child is an outcome, not a dispatch error");
        assert!(!outcome.success);
        assert_eq!(outcome.output, "", "no stale interim text as the answer");
        let error = outcome.error.expect("the child's error reaches the parent");
        assert!(error.contains("refused"), "{error}");
        assert!(
            !error.contains("no recovery left") && !error.contains("--continue"),
            "the parent gets no rung-3 hint it cannot act on: {error}"
        );
        // The child's own transcript keeps the hint.
        let child_complete = rupu_transcript::JsonlReader::iter(&outcome.transcript_path)
            .unwrap()
            .filter_map(Result::ok)
            .find_map(|e| match e {
                rupu_transcript::Event::RunComplete { error, .. } => error,
                _ => None,
            })
            .expect("the child's RunComplete records its error");
        assert!(
            child_complete.contains("no recovery left"),
            "{child_complete}"
        );
    }

    /// Regression for ISSUES.md I-8: `dispatch()` was the FOURTH I-1/I-2
    /// call site and hardcoded `anthropic` / `claude-sonnet-4-6`, ignoring
    /// `default_provider` / `default_model` from `config.toml` entirely (the
    /// I-1/I-2 fix only covered `cmd/run.rs`, `cmd/session.rs` and
    /// `step_factory.rs`). The child agent here declares NEITHER `provider:`
    /// nor `model:`, so the config-derived defaults threaded into the
    /// dispatcher must supply both.
    ///
    /// Observation seam: `run_agent` writes the resolved pair verbatim into
    /// the child's own transcript as `Event::RunStart { provider, model }`
    /// (`rupu-agent/src/runner.rs:739`), so the assertion reads real
    /// persisted output rather than any mock internals.
    #[tokio::test]
    async fn dispatch_honors_config_default_provider_and_model() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        // NOTE: no `provider:` and no `model:` in the frontmatter.
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
            Some("cfg-provider".to_string()),
            Some("cfg-model".to_string()),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(
            model, "cfg-model",
            "dispatch must resolve the model through config's default_model, not the hardcoded fallback"
        );
        assert_eq!(
            provider, "cfg-provider",
            "dispatch must resolve the provider through config's default_provider, not the hardcoded fallback"
        );
    }

    /// Agent frontmatter still wins over the config defaults — the
    /// precedence `resolve_provider_name`/`resolve_model` encode.
    #[tokio::test]
    async fn dispatch_agent_frontmatter_overrides_config_defaults() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: pinned-provider\nmodel: pinned-model\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
            Some("cfg-provider".to_string()),
            Some("cfg-model".to_string()),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(model, "pinned-model");
        assert_eq!(provider, "pinned-provider");
    }

    /// A config-declared `kind = "openai-compatible"` provider must be
    /// reachable from a dispatched sub-agent: the params come from the
    /// threaded-in map, and its `default_model` is the last fallback before
    /// `FALLBACK_MODEL` when neither the agent nor `default_model` pins one.
    #[tokio::test]
    async fn dispatch_resolves_openai_compatible_provider_default_model() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: oracle\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let mut oai = std::collections::HashMap::new();
        oai.insert(
            "oracle".to_string(),
            provider_factory::OpenAiCompatibleParams {
                base_url: "https://example.invalid/v1".to_string(),
                default_model: "oracle-default".to_string(),
                stream: false,
                models: Vec::new(),
            },
        );

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
            None,
            None,
            oai,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(provider, "oracle");
        assert_eq!(model, "oracle-default");
    }

    /// ISSUES.md I-3: a global `default_model` must NOT shadow a more
    /// specific `[providers.<name>].default_model`. An agent pinned to a
    /// custom openai-compatible provider (no `model:` of its own) must
    /// resolve to *that provider's* default, not the global one — the
    /// global value would typically be rejected by the custom endpoint as
    /// an unknown model.
    #[tokio::test]
    async fn dispatch_prefers_provider_scoped_default_model_over_global_default() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: oracle\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let mut oai = std::collections::HashMap::new();
        oai.insert(
            "oracle".to_string(),
            provider_factory::OpenAiCompatibleParams {
                base_url: "https://example.invalid/v1".to_string(),
                default_model: "oracle-default".to_string(),
                stream: false,
                models: Vec::new(),
            },
        );

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
            None,
            // A global default_model IS set here — the regression this
            // test guards against is this value winning over the more
            // specific provider-scoped one below.
            Some("global-default-model".to_string()),
            oai,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let (provider, model) = events
            .iter()
            .find_map(|e| match e {
                Event::RunStart {
                    provider, model, ..
                } => Some((provider.clone(), model.clone())),
                _ => None,
            })
            .expect("child transcript must carry a RunStart");

        assert_eq!(provider, "oracle");
        assert_eq!(model, "oracle-default");
    }

    /// IMPORTANT 4 fallback: `dispatch()` cannot thread the parent step's
    /// `actions:` narrowing (or an audit callback) into the child launch
    /// (see the doc comment on `dispatch()` for why), so it must never be
    /// a SILENT bypass. Assert the child's own transcript carries a
    /// visible notice recording the limitation.
    #[tokio::test]
    async fn dispatch_leaves_a_visible_delegation_narrowing_notice_on_the_child_transcript() {
        let _guard = ENV_LOCK.lock().await;
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global");
        std::fs::create_dir_all(global.join("agents")).unwrap();
        std::fs::write(
            global.join("agents/child.md"),
            "---\nname: child\nprovider: anthropic\nmodel: claude-sonnet-4-6\nmaxTurns: 3\n---\nyou are a child agent.",
        )
        .unwrap();

        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let run_store = Arc::new(RunStore::new(runs_dir));

        let workspace_path = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_path).unwrap();

        let resolver = Arc::new(rupu_auth::KeychainResolver::new());
        let mcp_registry = Arc::new(rupu_scm::Registry::default());

        let dispatcher = CliAgentDispatcher::new(
            global,
            None,
            "ws_test".into(),
            workspace_path,
            resolver,
            "bypass".into(),
            mcp_registry,
            run_store,
            None,
            None,
            None,
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            rupu_coverage::FindingWriteOptions::default(),
            None,
            rupu_runtime::model_limits::LimitsContext::for_cache_dir(
                dir.path().join("cache/models"),
            ),
            None,
            Default::default(),
            Default::default(),
        );

        std::env::set_var(
            "RUPU_MOCK_PROVIDER_SCRIPT",
            r#"[{ "AssistantText": { "text": "child done", "stop": "end_turn" } }]"#,
        );
        let result = dispatcher
            .dispatch("child", "do the thing".into(), "parent_run_1", 0, None)
            .await;
        std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");

        let outcome = result.expect("dispatch should succeed against the mock provider");

        let events: Vec<Event> = JsonlReader::iter(&outcome.transcript_path)
            .expect("child transcript readable")
            .filter_map(Result::ok)
            .collect();

        let notice_call = events.iter().any(|e| {
            matches!(
                e,
                Event::ToolCall { tool, .. } if tool == "dispatch_agent_narrowing_notice"
            )
        });
        assert!(
            notice_call,
            "expected a dispatch_agent_narrowing_notice ToolCall on the child transcript; got {events:?}"
        );
        let notice_result = events.iter().any(|e| {
            matches!(
                e,
                Event::ToolResult { output, error, .. }
                    if error.is_none() && output.contains("KNOWN LIMITATION")
            )
        });
        assert!(
            notice_result,
            "expected the paired ToolResult carrying the limitation text; got {events:?}"
        );

        // The real final-answer extraction must be unaffected by the
        // notice (it only scans AssistantMessage events).
        assert_eq!(outcome.output, "child done");
    }
}
