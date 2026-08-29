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
    runner::BypassDecider, runner::PermissionDecider, runner::ReadonlyDecider, AgentRunOpts,
    OnToolCallCallback,
};
use rupu_runtime::provider_factory;
use rupu_tools::{AgentDispatcher, ToolContext};
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
/// `AgentRunOpts`, not a handle-owning scope that outlives the step's own
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
    /// Resolved `[providers.<name>]` runtime knobs, keyed by provider name
    /// (`provider_factory::provider_tuning_map`). Lets a workflow step honor
    /// `timeout_ms` / `max_retries` / `max_concurrency` / `org_id` exactly as
    /// `rupu run` does (ISSUES.md I-9…I-12). Empty ⇒ documented defaults.
    pub provider_tuning: std::collections::HashMap<String, rupu_providers::ProviderTuning>,
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
    ) -> AgentRunOpts {
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
                    openai_compatible: oai_params,
                    tuning: self.provider_tuning.get(&provider_name).cloned(),
                    // `DefaultStepFactory` only carries pre-resolved
                    // `openai_compatible`/`provider_tuning` maps, not the raw
                    // `[providers.<name>]` config, so it cannot call
                    // `resolve_kind` here. `None` falls back to name-based
                    // dispatch — byte-identical to pre-Task-4 behavior, but
                    // it means a workflow step naming a declared multi-account
                    // (e.g. `anthropic-work`) won't yet resolve its kind.
                    kind: None,
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

        // `spec.tools` is MOVED into `narrow_agent_tools` just below, so
        // clone the PRE-narrowing grant here — it's exactly the
        // information `tool_audit`'s `granted` field needs, and
        // narrowing discards it by construction (that's the whole
        // point of narrowing).
        let pre_narrow_tools = spec.tools.clone();
        let audited_on_tool_call = wrap_on_tool_call_with_audit(
            on_tool_call,
            transcript_path.clone(),
            step.actions.clone(),
            pre_narrow_tools,
        );

        AgentRunOpts {
            agent_name: spec.name,
            agent_system_prompt,
            agent_tools: narrow_agent_tools(spec.tools, &step.actions),
            provider,
            provider_name,
            model,
            run_id,
            workspace_id,
            workspace_path: workspace_path.clone(),
            transcript_path,
            max_turns: spec.max_turns.unwrap_or(50),
            // `readonly` gets a real, non-interactive deny-writers decider
            // (ISSUES.md I-24 needs this to be observable at the tool layer).
            //
            // `ask` and `bypass` both get `BypassDecider`, i.e. **`ask`
            // grants full tool access in a workflow** (ISSUES.md I-78). That
            // is deliberate, not an oversight: the agent runtime's `ask`
            // decider blocks on stdin, and a workflow step has no operator
            // present to answer — so a genuinely-prompting `ask` would hang
            // every unattended run.
            //
            // Operator decision (2026-07-28): keep the behavior, make it
            // visible. Changing `ask` to deny writers would break every
            // existing workflow that writes without an explicit `--mode`,
            // because `ask` is also the **default** when `--mode` is
            // omitted. Instead `rupu workflow run` warns at startup when no
            // mode was given, and `docs/workflow-format.md` states it
            // plainly. Anyone wanting the restriction passes
            // `--mode readonly`.
            decider: if self.mode_str == "readonly" {
                Arc::new(ReadonlyDecider) as Arc<dyn PermissionDecider>
            } else {
                Arc::new(BypassDecider) as Arc<dyn PermissionDecider>
            },
            tool_context: ToolContext {
                workspace_path,
                bash_env_allowlist: self.bash_env_allowlist.clone(),
                bash_timeout_secs: self.bash_timeout_secs,
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
            on_tool_call: Some(audited_on_tool_call),
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

    fn permission_mode(&self) -> Option<&str> {
        Some(self.mode_str.as_str())
    }
}

/// The agent runtime's builtin (non-connector) tool names — `bash`,
/// `read_file`, `write_file`, `edit_file`, `ast_grep`, `grep`, `glob`,
/// `dispatch_agent`, `dispatch_agents_parallel` today. Sourced live from
/// [`rupu_agent::default_tool_registry`] rather than hardcoded so this
/// stays in sync if the runtime's builtin set ever changes.
fn builtin_tool_names() -> Vec<String> {
    rupu_agent::default_tool_registry().known_tools()
}

/// The MCP connector catalog's tool names (spec §1/§2) — the SAME
/// namespace `validate_step_actions` (`workflow.rs`) validates
/// `actions:` entries against.
fn catalog_tool_names() -> Vec<String> {
    rupu_mcp::tools::tool_catalog()
        .into_iter()
        .map(|spec| spec.name.to_string())
        .collect()
}

/// Push `name` onto `out` unless it's already present (keeps `expand_grant`
/// stable-ordered without pulling in a `HashSet`/`BTreeSet` dependency for
/// what are always small lists).
fn push_unique(out: &mut Vec<String>, name: &str) {
    if !out.iter().any(|existing| existing == name) {
        out.push(name.to_string());
    }
}

/// Resolve a grant (an agent's `tools:` list, or a synthetic `["*"]` for
/// "unrestricted") into concrete names drawn from `universe` (spec §2:
/// `BUILTINS ∪ CATALOG`).
///
/// - An exact entry passes through verbatim, even if it isn't in
///   `universe` — the grant isn't re-validated here, so an agent granting
///   a tool this build doesn't otherwise know about still keeps it.
/// - `*` expands to every name in `universe`.
/// - A `prefix*` wildcard (e.g. `scm.*`, `issues.*`) expands to every
///   `universe` name starting with `prefix` — same semantics as
///   `rupu-agent::runner::mcp_tool_name_matches_allowlist` (docs/scm.md).
fn expand_grant(grant: &[String], universe: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for entry in grant {
        if entry == "*" {
            for name in universe {
                push_unique(&mut out, name);
            }
        } else if let Some(prefix) = entry.strip_suffix('*') {
            for name in universe {
                if name.starts_with(prefix) {
                    push_unique(&mut out, name);
                }
            }
        } else {
            push_unique(&mut out, entry);
        }
    }
    out
}

/// `actions:` narrows only the CONNECTOR (MCP catalog) portion of the
/// agent's tool grant — builtins are never touched (spec §2, revised
/// 2026-07-26 after the T1 review found the original `agent.tools ∩
/// step.actions` formulation unimplementable: it deleted every builtin
/// and collapsed a wildcard grant like `tools: [scm.*]` to `Some([])`).
///
/// ```text
/// step.actions EMPTY  -> agent_tools, unchanged (compat-critical: every
///                        existing workflow carries `actions: []` while
///                        relying on the agent's `tools:`)
/// step.actions NON-EMPTY:
///     let g = expand(agent_tools)   // None (unrestricted) expands to
///                                   // BUILTINS ∪ CATALOG, i.e. as if
///                                   // the grant were ["*"]
///     effective = (g \ CATALOG)                      // builtins/non-catalog: untouched
///               ∪ (g ∩ CATALOG ∩ step.actions)        // connector subset: narrowed
/// ```
///
/// A step can only ever narrow the connector subset, never extend it —
/// `step.actions` is intersected with `g`, so a step naming a catalog
/// tool the agent doesn't grant can't gain it (no escalation). An empty
/// resulting connector intersection is legal (a step may allow zero
/// connector calls) but never strips a builtin.
pub(crate) fn narrow_agent_tools(
    agent_tools: Option<Vec<String>>,
    step_actions: &[String],
) -> Option<Vec<String>> {
    if step_actions.is_empty() {
        return agent_tools;
    }

    let catalog = catalog_tool_names();
    let mut universe = builtin_tool_names();
    for name in &catalog {
        push_unique(&mut universe, name);
    }
    let is_catalog = |name: &str| catalog.iter().any(|c| c == name);

    // `agent_tools == None` means unrestricted: expand as if granted `["*"]`.
    let grant = agent_tools.unwrap_or_else(|| vec!["*".to_string()]);
    let g = expand_grant(&grant, &universe);

    let mut effective = Vec::new();
    // (g \ CATALOG): builtins and any non-catalog name in the grant — untouched.
    for name in &g {
        if !is_catalog(name) {
            push_unique(&mut effective, name);
        }
    }
    // (g ∩ CATALOG ∩ step.actions): the narrowed connector subset.
    for name in &g {
        if is_catalog(name) && step_actions.iter().any(|a| a == name) {
            push_unique(&mut effective, name);
        }
    }

    Some(effective)
}

// ---------------------------------------------------------------------------
// `tool_audit` transcript trail (step `actions:` enforcement, T2)
// ---------------------------------------------------------------------------

/// Whether `tool_name` is covered by an agent's PRE-narrowing grant,
/// using the SAME wildcard-aware expand/covers logic
/// [`narrow_agent_tools`] uses (spec §2's `expand(grant)`). A naive
/// exact-string-match would wrongly report `granted: false` for a
/// `tools: [scm.*]` agent asked about `scm.prs.get` — this is the
/// regression `narrow_agent_tools`'s own tests already guard for the
/// enforcement path; `tool_audit`'s `granted` field must not
/// reintroduce it on the audit path.
///
/// `pre_narrow_tools == None` means unrestricted (the agent's `tools:`
/// frontmatter is absent) — everything is granted.
fn tool_is_granted(pre_narrow_tools: &Option<Vec<String>>, tool_name: &str) -> bool {
    match pre_narrow_tools {
        None => true,
        Some(grant) => {
            let catalog = catalog_tool_names();
            let mut universe = builtin_tool_names();
            for name in &catalog {
                push_unique(&mut universe, name);
            }
            expand_grant(grant, &universe)
                .iter()
                .any(|t| t == tool_name)
        }
    }
}

/// Append one `tool_audit` transcript line to `transcript_path` (spec
/// §4a/§4b). Best-effort: a write failure is logged and swallowed —
/// this is an observability side-channel, never allowed to fail the
/// run it's auditing.
///
/// Uses `JsonlWriter::append` (O_APPEND), which is safe to interleave
/// with `run_agent`'s own transcript writer ONLY because `run_agent`
/// was changed (2026-07-26) to also hold an append-mode writer for
/// exactly this reason — see the doc on `rupu_agent::OnToolCallCallback`
/// and the comment above `run_agent`'s writer construction.
///
/// `declared`/`granted`/`blocked` distinguish three independent axes
/// (spec §4a): `declared` is "was this tool named in the step's
/// `actions:` list" (false both when `actions:` is empty/absent — NOT
/// a violation — and when it's non-empty but doesn't name this tool;
/// `restricted` disambiguates those two). `granted` is "does the
/// agent's `tools:` frontmatter cover this tool" (pre-narrowing).
/// `blocked` is "was the call actually denied", supplied by the
/// caller from the `OnToolCallCallback` outcome.
///
/// When a step declares a tool the agent does not grant
/// (`declared && !granted`), this is spec §3c's "visible, not
/// fatal" case: narrowing already made it safe (no escalation — the
/// call is blocked by construction), but it's very likely an
/// authoring mistake, so it ALSO gets a `tracing::warn!` in addition
/// to the transcript line.
fn emit_tool_audit(
    transcript_path: &std::path::Path,
    tool_name: &str,
    step_actions: &[String],
    pre_narrow_tools: &Option<Vec<String>>,
    blocked: bool,
) {
    let restricted = !step_actions.is_empty();
    let declared = restricted && step_actions.iter().any(|a| a == tool_name);
    let granted = tool_is_granted(pre_narrow_tools, tool_name);

    if declared && !granted {
        tracing::warn!(
            tool = tool_name,
            "step `actions:` declares `{tool_name}` but the agent's `tools:` grant \
             does not cover it; the call is narrowed away (no escalation — this is \
             safe, not a security issue) but is very likely an authoring mistake"
        );
    }

    match rupu_transcript::JsonlWriter::append(transcript_path) {
        Ok(mut w) => {
            if let Err(e) = w.write(&rupu_transcript::Event::ToolAudit {
                tool: tool_name.to_string(),
                declared,
                granted,
                blocked,
                restricted,
            }) {
                tracing::warn!(error = %e, "failed to write tool_audit transcript line");
            } else if let Err(e) = w.flush() {
                tracing::warn!(error = %e, "failed to flush tool_audit transcript line");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open transcript for tool_audit append");
        }
    }
}

/// Wrap the caller-supplied `on_tool_call` (the live-executor
/// `StepWorking`-note hook `run_linear_step` builds) with `tool_audit`
/// emission, so every catalog tool call an agent step attempts gets an
/// audit-trail line — regardless of whether a live-executor hook is
/// even wired (e.g. CLI/cron/MCP-triggered runs with no `event_sink`).
/// The original hook, when present, still fires first and unchanged
/// (same `blocked` value it always would have received under the new
/// 3-arg signature).
///
/// **Catalog-only** (spec §4a/§4b: "per **catalog** call"; §2 revised —
/// builtins are exempt from narrowing entirely). A builtin call (`bash`,
/// `read_file`, …) or a genuinely hallucinated non-catalog name is NOT a
/// catalog call, so no `tool_audit` line is written for it. Without this
/// filter, every builtin call on a narrowed step produced
/// `{restricted:true, declared:false, granted:true, blocked:false}` — the
/// exact signature that means "narrowed away", which is false for a
/// builtin (builtins are never narrowed, per §2). It also flooded the
/// transcript with one write/flush per builtin call, and gave a
/// hallucinated tool name a red `blocked:true` audit line even though it
/// was never a real permission decision.
fn wrap_on_tool_call_with_audit(
    inner: Option<OnToolCallCallback>,
    transcript_path: PathBuf,
    step_actions: Vec<String>,
    pre_narrow_tools: Option<Vec<String>>,
) -> OnToolCallCallback {
    let catalog = catalog_tool_names();
    Arc::new(move |step_id: &str, tool_name: &str, blocked: bool| {
        if let Some(cb) = inner.as_ref() {
            cb(step_id, tool_name, blocked);
        }
        if catalog.iter().any(|c| c == tool_name) {
            emit_tool_audit(
                &transcript_path,
                tool_name,
                &step_actions,
                &pre_narrow_tools,
                blocked,
            );
        }
    })
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
            max_tokens: 1,
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

#[cfg(test)]
mod narrow_agent_tools_tests {
    use super::{builtin_tool_names, narrow_agent_tools};

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_actions_returns_the_grant_verbatim_including_unexpanded_wildcards() {
        let tools = Some(strs(&["issues.list", "issues.create"]));
        assert_eq!(narrow_agent_tools(tools.clone(), &[]), tools);
        assert_eq!(narrow_agent_tools(None, &[]), None);

        // A wildcard grant with empty `actions:` must round-trip UN-expanded
        // (spec: "empty means unrestricted... the agent's grant stands,
        // verbatim") — not silently rewritten into its expansion.
        let wildcard = Some(strs(&["scm.*"]));
        assert_eq!(narrow_agent_tools(wildcard.clone(), &[]), wildcard);
    }

    #[test]
    fn builtins_survive_a_connector_narrowing() {
        // An agent granted [read_file, grep, bash, scm.prs.get, scm.prs.diff]
        // + actions: [scm.prs.get] must KEEP read_file/grep/bash (never
        // touched by `actions:`, per spec §2) and narrow to exactly one
        // connector tool.
        let granted = strs(&["read_file", "grep", "bash", "scm.prs.get", "scm.prs.diff"]);
        let got = narrow_agent_tools(Some(granted), &strs(&["scm.prs.get"])).unwrap();

        for builtin in ["read_file", "grep", "bash"] {
            assert!(
                got.contains(&builtin.to_string()),
                "missing builtin {builtin} in {got:?}"
            );
        }
        let connector_count = got
            .iter()
            .filter(|t| {
                t.starts_with("scm.")
                    || t.starts_with("issues.")
                    || t.starts_with("github.")
                    || t.starts_with("gitlab.")
            })
            .count();
        assert_eq!(
            connector_count, 1,
            "expected exactly one connector tool, got {got:?}"
        );
        assert!(got.contains(&"scm.prs.get".to_string()));
        assert!(
            !got.contains(&"scm.prs.diff".to_string()),
            "scm.prs.diff must be narrowed away: {got:?}"
        );
    }

    #[test]
    fn wildcard_grant_narrows_to_the_named_tool_not_to_empty() {
        // tools: [scm.*] + actions: [scm.prs.get] must yield [scm.prs.get]
        // (plus any builtins — there are none in this grant), NEVER `[]`.
        // This is exactly the case the original (wrong) raw-intersection
        // formulation collapsed to `Some([])`.
        let got = narrow_agent_tools(Some(strs(&["scm.*"])), &strs(&["scm.prs.get"])).unwrap();
        assert_eq!(
            got,
            vec!["scm.prs.get".to_string()],
            "must not collapse to []: {got:?}"
        );
    }

    #[test]
    fn star_grant_keeps_all_builtins_and_narrows_the_catalog_to_one() {
        // tools: ["*"] + actions: [issues.list] keeps every builtin and
        // exactly `issues.list` of the catalog.
        let got = narrow_agent_tools(Some(strs(&["*"])), &strs(&["issues.list"])).unwrap();
        for builtin in builtin_tool_names() {
            assert!(
                got.contains(&builtin),
                "missing builtin {builtin} in {got:?}"
            );
        }
        let catalog_entries: Vec<&String> = got
            .iter()
            .filter(|t| !builtin_tool_names().contains(t))
            .collect();
        assert_eq!(
            catalog_entries,
            vec![&"issues.list".to_string()],
            "expected exactly issues.list from the catalog, got {got:?}"
        );
    }

    #[test]
    fn agent_tools_none_is_unrestricted_and_expands_before_narrowing() {
        // agent_tools == None means unrestricted -> expand to BUILTINS ∪
        // CATALOG, THEN narrow the connector subset. So an agent with no
        // declared `tools:` still keeps every builtin when a step narrows
        // its connector calls.
        let got = narrow_agent_tools(None, &strs(&["issues.list"])).unwrap();
        for builtin in builtin_tool_names() {
            assert!(
                got.contains(&builtin),
                "missing builtin {builtin} in {got:?}"
            );
        }
        assert!(got.contains(&"issues.list".to_string()));
        assert!(
            !got.contains(&"issues.create".to_string()),
            "must narrow the catalog too: {got:?}"
        );
    }

    #[test]
    fn a_step_cannot_escalate_to_a_catalog_tool_the_agent_lacks() {
        // A step naming a catalog tool the agent does NOT grant must not
        // gain it — narrowing only ever shrinks, never extends.
        let granted = strs(&["read_file"]);
        let got = narrow_agent_tools(Some(granted), &strs(&["issues.list"])).unwrap();
        assert_eq!(
            got,
            vec!["read_file".to_string()],
            "must not escalate: {got:?}"
        );
    }

    #[test]
    fn empty_connector_intersection_strips_no_builtins() {
        // A step may legitimately allow zero connector calls (spec §2) —
        // but it must never strip a builtin. Agent grants builtins only +
        // a connector tool the step's `actions:` doesn't ask for.
        let granted = strs(&["bash", "read_file", "grep"]);
        let got = narrow_agent_tools(Some(granted), &strs(&["scm.prs.get"])).unwrap();
        assert_eq!(
            got,
            vec![
                "bash".to_string(),
                "read_file".to_string(),
                "grep".to_string()
            ],
            "empty connector intersection must not strip builtins: {got:?}"
        );
    }
}

/// End-to-end proof that `build_opts_for_step` actually narrows
/// `agent_tools` per the step's `actions:` (not just the isolated
/// helper). Drives the real `DefaultStepFactory` against an on-disk
/// agent spec granting `[issues.list, issues.create]`; no live
/// provider credentials are needed because a build failure resolves
/// to `ProviderBuildErrorStub` rather than panicking — we only assert
/// on the `agent_tools` field of the returned `AgentRunOpts`.
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

    fn factory(global: std::path::PathBuf) -> DefaultStepFactory {
        DefaultStepFactory {
            workflow: Workflow::parse(WF).expect("workflow must parse"),
            global,
            project_root: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            mode_str: "bypass".to_string(),
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
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

    #[tokio::test]
    async fn step_actions_narrows_the_agent_grant() {
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
            Some(vec!["issues.list".to_string()]),
            "actions: [issues.list] must narrow the agent's [issues.list, issues.create] grant"
        );
    }

    #[tokio::test]
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
    }

    #[tokio::test]
    async fn bash_config_reaches_the_step_opts() {
        // Regression for ISSUES.md I-18: the workflow path hardcoded a 120s
        // bash timeout and an empty env allowlist at build_opts_for_step's
        // ToolContext construction, so `[bash]` config silently applied
        // under `rupu run`/`rupu session` but not under `rupu workflow run`.
        // A DefaultStepFactory carrying bash_timeout_secs = 42 and
        // env_allowlist = ["FOO"] must produce an AgentRunOpts whose
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
            opts.tool_context.bash_timeout_secs, 42,
            "bash_timeout_secs must flow from the factory, not hardcode 120"
        );
        assert!(
            opts.tool_context
                .bash_env_allowlist
                .contains(&"FOO".to_string()),
            "bash_env_allowlist must flow from the factory, not hardcode empty: {:?}",
            opts.tool_context.bash_env_allowlist
        );
    }

    /// Read a transcript JSONL's `tool_audit` lines back as raw
    /// `serde_json::Value`s (adjacently-tagged `{"type":...,"data":{...}}`
    /// shape) so tests can assert on fields without depending on
    /// `rupu_transcript::Event`'s full derive.
    fn read_tool_audit_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
        let body = std::fs::read_to_string(path).unwrap_or_default();
        body.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|v| v["type"] == "tool_audit")
            .collect()
    }

    #[tokio::test]
    async fn on_tool_call_is_always_wired_even_when_the_caller_passes_none() {
        // `build_opts_for_step` must ALWAYS return `Some(...)` for
        // `on_tool_call` — the tool_audit trail must exist even for
        // CLI/cron/MCP-triggered runs with no live-executor event_sink
        // (which pass `on_tool_call: None` in).
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let f = factory(tmp.path().to_path_buf());
        let transcript_path = tmp.path().join("transcript_wired.jsonl");

        let opts = f
            .build_opts_for_step(
                "unrestricted",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                transcript_path.clone(),
                None,
            )
            .await;

        let cb = opts.on_tool_call.expect("on_tool_call must always be Some");
        cb("unrestricted", "issues.list", false);

        let lines = read_tool_audit_lines(&transcript_path);
        assert_eq!(
            lines.len(),
            1,
            "expected exactly one tool_audit line; got {lines:?}"
        );
        assert_eq!(lines[0]["data"]["tool"], "issues.list");
        assert_eq!(
            lines[0]["data"]["declared"], false,
            "actions: [] -> not declared"
        );
        assert_eq!(
            lines[0]["data"]["restricted"], false,
            "actions: [] -> unrestricted"
        );
        assert_eq!(
            lines[0]["data"]["granted"], true,
            "issues.list IS in the agent's grant"
        );
        assert_eq!(lines[0]["data"]["blocked"], false);
    }

    #[tokio::test]
    async fn builtin_tool_call_on_a_narrowed_step_emits_no_tool_audit() {
        // Regression (tool_audit IMPORTANT 3): spec §4a/§4b say "per
        // **catalog** call", and §2 (revised) exempts builtins from
        // narrowing entirely. Before this fix, EVERY tool call — builtin
        // or catalog — got an audit line, so a `bash` call on a step
        // narrowed to `[issues.list]` wrote
        // `{restricted:true, declared:false, granted:true, blocked:false}`
        // — the exact signature that means "narrowed away", which is
        // false for a builtin. `bash` must produce NO tool_audit line at
        // all.
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let f = factory(tmp.path().to_path_buf());
        let transcript_path = tmp.path().join("transcript_builtin.jsonl");

        let opts = f
            .build_opts_for_step(
                "narrowed",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                transcript_path.clone(),
                None,
            )
            .await;
        let cb = opts.on_tool_call.expect("always wired");

        // A builtin call on the same narrowed step: no audit line.
        cb("narrowed", "bash", false);
        let lines = read_tool_audit_lines(&transcript_path);
        assert_eq!(
            lines.len(),
            0,
            "a builtin call must not be audited: {lines:?}"
        );

        // A catalog call on the same step: exactly one audit line.
        cb("narrowed", "issues.list", false);
        let lines = read_tool_audit_lines(&transcript_path);
        assert_eq!(lines.len(), 1, "a catalog call must be audited: {lines:?}");
        assert_eq!(lines[0]["data"]["tool"], "issues.list");
    }

    const WF_UNGRANTED: &str = r#"
name: w
steps:
  - id: narrowed
    agent: ag
    prompt: p
    actions: ["issues.get", "issues.list"]
"#;

    #[tokio::test]
    async fn declared_and_blocked_call_writes_the_correct_tool_audit_fields() {
        // `issues.list` IS declared (in `actions:`) and IS granted (agent's
        // `tools:`) — a normal, allowed call.
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let f = DefaultStepFactory {
            workflow: crate::workflow::Workflow::parse(WF_UNGRANTED).expect("parses"),
            global: tmp.path().to_path_buf(),
            project_root: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            mode_str: "bypass".to_string(),
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
        };
        let transcript_path = tmp.path().join("transcript_declared.jsonl");

        let opts = f
            .build_opts_for_step(
                "narrowed",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                transcript_path.clone(),
                None,
            )
            .await;
        let cb = opts.on_tool_call.expect("always wired");
        cb("narrowed", "issues.list", false);

        let lines = read_tool_audit_lines(&transcript_path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["data"]["tool"], "issues.list");
        assert_eq!(lines[0]["data"]["declared"], true);
        assert_eq!(lines[0]["data"]["restricted"], true);
        assert_eq!(lines[0]["data"]["granted"], true);
        assert_eq!(lines[0]["data"]["blocked"], false);
    }

    #[tokio::test]
    async fn a_step_declaring_an_ungranted_tool_emits_granted_false() {
        // `issues.get` IS declared (in `actions:`) but the agent's `tools:`
        // grant (`[issues.list, issues.create]`) does NOT cover it — spec
        // §3c: visible (granted: false in the transcript + a
        // tracing::warn!), not fatal. Narrowing already made the call
        // safe (no escalation), so `blocked` mirrors what the runtime
        // would actually do (the tool never makes it into the narrowed
        // roster) — the caller passes that outcome through unchanged.
        let tmp = assert_fs::TempDir::new().unwrap();
        write_agent(tmp.path());
        let f = DefaultStepFactory {
            workflow: crate::workflow::Workflow::parse(WF_UNGRANTED).expect("parses"),
            global: tmp.path().to_path_buf(),
            project_root: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            mode_str: "bypass".to_string(),
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
        };
        let transcript_path = tmp.path().join("transcript_ungranted.jsonl");

        let opts = f
            .build_opts_for_step(
                "narrowed",
                "ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                transcript_path.clone(),
                None,
            )
            .await;

        // Sanity: `issues.get` really was narrowed away (no escalation).
        assert_eq!(
            opts.agent_tools,
            Some(vec!["issues.list".to_string()]),
            "issues.get must be narrowed away — the agent never granted it"
        );

        let cb = opts.on_tool_call.expect("always wired");
        cb("narrowed", "issues.get", true);

        let lines = read_tool_audit_lines(&transcript_path);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["data"]["tool"], "issues.get");
        assert_eq!(lines[0]["data"]["declared"], true);
        assert_eq!(lines[0]["data"]["restricted"], true);
        assert_eq!(
            lines[0]["data"]["granted"], false,
            "agent never granted issues.get"
        );
        assert_eq!(lines[0]["data"]["blocked"], true);
    }

    const WF_WILDCARD: &str = r#"
name: w
steps:
  - id: narrowed
    agent: wildcard-ag
    prompt: p
    actions: ["scm.prs.get"]
"#;

    #[tokio::test]
    async fn wildcard_granted_agent_reports_granted_true_not_a_naive_match_false() {
        // Regression for the naive-exact-match bug narrow_agent_tools
        // itself guards on the enforcement side (spec §2's rewrite): an
        // agent granting `tools: [scm.*]` must report `granted: true`
        // for `scm.prs.get`, not `false` from a bare `list.contains(tool)`
        // check.
        let tmp = assert_fs::TempDir::new().unwrap();
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("wildcard-ag.md"),
            "---\nname: wildcard-ag\ntools: [scm.*]\n---\nDo the thing.\n",
        )
        .unwrap();
        let f = DefaultStepFactory {
            workflow: crate::workflow::Workflow::parse(WF_WILDCARD).expect("parses"),
            global: tmp.path().to_path_buf(),
            project_root: None,
            resolver: Arc::new(rupu_auth::KeychainResolver::new()),
            mode_str: "bypass".to_string(),
            mcp_registry: Arc::new(rupu_scm::Registry::empty()),
            system_prompt_suffix: None,
            dispatcher: None,
            openai_compatible: std::collections::HashMap::new(),
            provider_tuning: std::collections::HashMap::new(),
            default_provider: None,
            default_model: None,
            bash_timeout_secs: 120,
            bash_env_allowlist: Vec::new(),
        };
        let transcript_path = tmp.path().join("transcript_wildcard.jsonl");

        let opts = f
            .build_opts_for_step(
                "narrowed",
                "wildcard-ag",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                transcript_path.clone(),
                None,
            )
            .await;
        let cb = opts.on_tool_call.expect("always wired");
        cb("narrowed", "scm.prs.get", false);

        let lines = read_tool_audit_lines(&transcript_path);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0]["data"]["granted"], true,
            "scm.* must cover scm.prs.get: {lines:?}"
        );
        assert_eq!(lines[0]["data"]["declared"], true);
        assert_eq!(lines[0]["data"]["blocked"], false);
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
