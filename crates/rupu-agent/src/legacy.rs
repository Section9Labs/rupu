//! TEMPORARY adapter (spec 2026-10-07 W3a): the flat pre-W3 run options,
//! kept so the two launch sites W3a does not migrate — workflow steps
//! (`rupu-orchestrator`'s `DefaultStepFactory` + `dispatch_one`) and the
//! agentiflow lead (`rupu-agentiflow`'s `lead.rs`) — compile unchanged.
//! [`LegacyRunOpts::into_run_opts`] does what the runner's identity
//! overwrite block used to: it fills the run's [`RunIdentity`] from the flat
//! fields, and resolves the grant.
//!
//! W3b migrates those two sites onto `rupu_runtime::assembly` and DELETES
//! this module. Nothing new may use it; `rupu-runtime`'s
//! `no_hand_built_run_opts` test allows its literals only in those two sites.

use crate::runner::{
    AgentPins, AgentRunOpts, Hooks, OnStreamEventCallback, OnToolCallCallback, OnUsageCallback,
    RunError, RunExit, RunResult, StreamOpts, UserTurn,
};
use rupu_providers::provider::LlmProvider;
use rupu_providers::types::Message;
use rupu_scm::Registry;
use rupu_tools::{AliasScope, ParentLink, PermissionPolicy, RunIdentity, Surface, ToolContext};
use std::path::PathBuf;
use std::sync::Arc;

/// The flat run options workflow steps (`DefaultStepFactory` + the
/// orchestrator's `dispatch_one`) and the agentiflow lead still build by
/// hand. Same fields as before W3; `tool_context` is the new shape, its
/// identity filled from the flat fields by [`LegacyRunOpts::into_run_opts`].
pub struct LegacyRunOpts {
    /// Human codename for this instance. Written to `RunStart`, announced in
    /// the system prompt, and copied onto `tool_context` for attribution.
    pub codename: Option<String>,
    pub agent_name: String,
    pub agent_system_prompt: String,
    /// The agent's `tools:` grant; `None` = [`rupu_tools::DEFAULT_GRANT`].
    /// Resolved with [`Self::step_actions`] and the run's ambient grants into
    /// the run's [`rupu_tools::ResolvedGrant`], the only input to its tool registry.
    pub agent_tools: Option<Vec<String>>,
    /// The workflow step's `actions:`: narrows the connector tools of the
    /// grant, nothing else. Empty = no narrowing (every non-step run).
    pub step_actions: Vec<String>,
    /// Where tool aliases resolve: [`AliasScope::FlowLead`] for an
    /// agentiflow lead (`coverage.status` → `goal.coverage`), else
    /// [`AliasScope::Everywhere`].
    pub alias_scope: AliasScope,
    pub provider: Box<dyn LlmProvider>,
    pub provider_name: String,
    pub model: String,
    pub run_id: String,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    pub transcript_path: PathBuf,
    pub max_turns: u32,
    /// The run's permission policy: its mode and, for an interactive run,
    /// the operator prompter. Every tool call is decided by it from the
    /// tool's declared effect.
    pub permission: PermissionPolicy,
    pub tool_context: ToolContext,
    pub user_message: String,
    /// Existing conversation state to prepend before `user_message`.
    /// Empty for one-shot runs.
    pub initial_messages: Vec<Message>,
    /// Absolute turn index for the first turn in this run.
    pub turn_index_offset: u32,
    /// If true, don't render tokens as they arrive and don't write
    /// `AssistantDelta`/`ThinkingDelta` transcript events while a reply
    /// streams (the transcript keeps its final-message-only shape). A
    /// discarded turn has no final message, so its text is written as
    /// `AssistantDelta`s once the reply is in. Display only: the request still streams
    /// on the wire, so a long response cannot hit the HTTP request timeout and
    /// the full output cap applies. Default is false. Used by --no-stream.
    pub no_stream: bool,
    /// If true, suppress stdout writes from the streaming code path.
    /// The provider's text deltas still flow into the JSONL transcript
    /// writer; only the `print!`/`println!` calls used by the legacy
    /// line-stream UI are skipped. The TUI sets this to `true` because
    /// it owns the alt-screen — any stdout write corrupts the canvas.
    /// Default false (preserves the line-stream UI for `rupu run`).
    pub suppress_stream_stdout: bool,
    /// SCM/issue registry. When `Some`, the runner spins up an in-process
    /// MCP server before the first turn and tears it down before returning.
    /// `None` means MCP tools are unavailable for this run (test harness,
    /// pre-Task-19 CLI invocations, etc.).
    pub mcp_registry: Option<Arc<Registry>>,
    /// Reasoning / thinking effort level for every turn. Provider-specific
    /// translation (Anthropic `thinking.budget_tokens` / `thinking.type:adaptive`,
    /// OpenAI/Copilot `reasoning.effort`, Gemini `thinkingBudget`).
    pub effort: Option<rupu_providers::model_tier::ThinkingLevel>,
    /// Reasoning display hint (`thinking.display`). Provider-generic like
    /// `effort` — carried unchanged across fallback hops, never a model pin.
    /// Honored by Anthropic adaptive models only.
    pub thinking_display: Option<rupu_providers::types::ThinkingDisplay>,
    /// Desired context-window tier. Anthropic api-key path uses this to
    /// gate the `context-1m-2025-08-07` beta header; other providers
    /// currently ignore it.
    pub context_window: Option<rupu_providers::model_tier::ContextWindow>,
    /// Cross-provider output-format hint. Anthropic emits as
    /// `output_config.format`; OpenAI emits as `response_format.type`;
    /// other providers ignore.
    pub output_format: Option<rupu_providers::types::OutputFormat>,
    /// JSON Schema for Anthropic structured outputs. Threaded from
    /// `AgentSpec::output_schema`. `Some` gets emitted as
    /// `output_config.format = {type: "json_schema", schema}` by the
    /// Anthropic provider; `None` preserves prompt-driven-only
    /// `output_format` behavior (no schema-less mode exists). Ignored
    /// by other providers.
    pub output_schema: Option<serde_json::Value>,
    /// Anthropic-only soft cap on output tokens (model self-paces).
    /// Distinct from `max_turns` (hard ceiling). Ignored by other
    /// providers.
    pub anthropic_task_budget: Option<u32>,
    /// Anthropic-only auto context-pruning strategy. Ignored by
    /// other providers.
    pub anthropic_context_management: Option<rupu_providers::types::ContextManagement>,
    /// Anthropic-only fast-mode toggle (account-gated). Ignored by
    /// other providers.
    pub anthropic_speed: Option<rupu_providers::types::Speed>,
    /// When this run is a sub-agent dispatch, the parent run's id.
    /// `None` for top-level workflow runs.
    pub parent_run_id: Option<String>,
    /// Dispatch depth — 0 for top-level workflow steps, 1 for direct
    /// children of the parent, 2 for grandchildren, etc. The
    /// `dispatch_agent` tool checks this against the per-agent +
    /// workspace max-depth limit before spawning a child. See
    /// `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`
    /// § 4.3.
    pub depth: u32,
    /// Per-agent allowlist of children this agent can dispatch via
    /// `dispatch_agent` / `dispatch_agents_parallel`. Pulled from the
    /// agent's `dispatchableAgents:` frontmatter field. `None`
    /// (default) ⇒ no dispatches allowed.
    pub dispatchable_agents: Option<Vec<String>>,
    /// Step id that owns this agent run. Threaded through so
    /// `on_tool_call` can identify which step is calling. Empty
    /// for free-standing agent runs (no orchestrator).
    pub step_id: String,
    /// Optional callback invoked before each tool dispatch.
    pub on_tool_call: Option<OnToolCallCallback>,
    /// Optional callback invoked for live stream events while the
    /// provider is generating. Used by session attach to surface
    /// real-time usage/progress without changing transcript schema.
    pub on_stream_event: Option<OnStreamEventCallback>,
    /// Per-LLM-call usage hook (spec 2026-09-29 §3.3). Invoked synchronously
    /// once per billed call — normal turns AND the compaction summariser —
    /// alongside its transcript `Usage` event and before any early exit, so
    /// no billed call can be missed: a normal turn's hook fires just BEFORE
    /// its `Usage` write (a failed write, which aborts the run, still leaves
    /// the call reported); the compaction hook fires after its best-effort
    /// write, whose failure is only logged. Must be cheap and must never
    /// panic; the orchestrator uses it to append the run's usage ledger.
    pub on_usage: Option<OnUsageCallback>,
    /// Coverage concerns block. When `Some`, the runner flattens the
    /// catalog, writes a snapshot, injects coverage tools, and prepends
    /// the catalog to the system prompt. `None` (default) disables all
    /// coverage harness machinery.
    pub concerns: Option<rupu_coverage::ConcernsBlock>,
    /// Resolved model limits (spec 2026-09-30 §5): the request `max_tokens`
    /// (`output`), the compaction threshold (`compact_threshold()`), and the
    /// run-start notice. Launch sites build this with
    /// `rupu_runtime::model_limits::resolve`; tests use `ModelLimits::unknown()`
    /// / `fixed(..)`.
    pub limits: rupu_providers::model_limits::ModelLimits,
    /// Override the `scope_name` used when deriving the coverage `target_id`.
    /// When `None` (default, standalone agent runs), falls back to `agent_name`.
    /// Workflow runs set this to the workflow name so all steps accumulate
    /// ledger entries under the same `target_id`, regardless of which agent
    /// handled each step.
    pub scope_name: Option<String>,
    /// Override the surface tag written into coverage `FileTouchEvent`s.
    /// When `None` (default), the runner falls back to `"agent"`.
    /// The workflow step factory sets this to `"workflow"` so coverage events
    /// from workflow runs are correctly attributed to the workflow surface.
    pub surface_tag: Option<String>,
    /// Cooperative pause signal. When `Some` and the token is cancelled,
    /// the loop stops at the next safe boundary — after the in-flight
    /// stream is dropped (partial assistant text is discarded, not
    /// committed) or after a running tool finishes and its result is
    /// recorded — and `run_agent` returns a `RunResult` with
    /// `paused == true` instead of erroring. A resume is just another
    /// `run_agent` call seeded with the persisted transcript messages.
    /// `None` (default) preserves today's behavior exactly.
    pub pause: Option<tokio_util::sync::CancellationToken>,
    /// Path of a transcript whose replay-reconstruction equals
    /// `initial_messages` byte-exact (spec §3 seed dedup rule). When set,
    /// the Seed event stores this reference instead of re-embedding the
    /// messages; the caller owns that invariant and replay verifies it via
    /// the recorded sha256. `None` → the seed is embedded inline in full.
    pub seed_source: Option<PathBuf>,
    /// Pre-turn collectors (spec §8). Empty = no injection; the loop behaves
    /// exactly as before. Run off the async runtime via spawn_blocking.
    pub collectors: Vec<std::sync::Arc<dyn crate::collector::TurnCollector>>,
    /// Pre-built, run-scoped tools injected into this run regardless of
    /// `agent_tools`: an ambient grant with reason `origin:injected`, so they
    /// appear in the run's `tool_grant` and are audited like any tool; the
    /// `PermissionPolicy` still gates each call. Empty for ordinary runs; the
    /// agentiflow envelope uses it for its board/mailbox/dispatch tools until
    /// W5 moves them into the catalog.
    pub extra_tools: Vec<std::sync::Arc<dyn rupu_tools::Tool>>,
    /// Recovery ladder inputs (spec 2026-10-01 §5–§6). Default: no fallback chain and no hop builder — rungs 1 and 2 are unavailable, rung 0 still applies.
    pub recovery: crate::recovery::RecoveryOpts,
}

impl LegacyRunOpts {
    /// The run's [`RunIdentity`] from the flat fields — what the runner's
    /// overwrite block set before W3.
    fn identity(&self) -> RunIdentity {
        RunIdentity {
            run_id: self.run_id.clone(),
            codename: self.codename.clone(),
            agent: self.agent_name.clone(),
            provider: self.provider_name.clone(),
            model: self.model.clone(),
            depth: self.depth,
            parent: self.parent_run_id.clone().map(|run_id| ParentLink {
                root_run_id: run_id.clone(),
                run_id,
                codename: None,
            }),
            surface: match self.surface_tag.as_deref() {
                Some("workflow") => Surface::Workflow,
                Some("session") => Surface::Session,
                _ => Surface::Agent,
            },
            scope_name: self.scope_name.clone(),
            step_id: Some(self.step_id.clone()).filter(|s| !s.is_empty()),
            dispatchable_agents: self.dispatchable_agents.clone(),
        }
    }

    /// The run options the agent loop takes: the identity set on the tool
    /// context, the workspace and SCM registry moved onto it, and the grant
    /// resolved over its services.
    pub fn into_run_opts(self) -> Result<AgentRunOpts, RunError> {
        let identity = Arc::new(self.identity());
        let LegacyRunOpts {
            agent_system_prompt,
            agent_tools,
            step_actions,
            alias_scope,
            provider,
            workspace_id,
            workspace_path,
            transcript_path,
            max_turns,
            permission,
            mut tool_context,
            user_message,
            initial_messages,
            turn_index_offset,
            no_stream,
            suppress_stream_stdout,
            mcp_registry,
            effort,
            thinking_display,
            context_window,
            output_format,
            output_schema,
            anthropic_task_budget,
            anthropic_context_management,
            anthropic_speed,
            on_tool_call,
            on_stream_event,
            on_usage,
            concerns,
            limits,
            pause,
            seed_source,
            collectors,
            extra_tools,
            recovery,
            ..
        } = self;
        tool_context.identity = identity;
        tool_context.workspace.id = workspace_id;
        tool_context.workspace.path = workspace_path;
        tool_context.services.scm = mcp_registry;
        let grant = crate::grant::resolve_run_grant(crate::grant::RunGrantInputs {
            declared: agent_tools.as_deref(),
            step_actions: &step_actions,
            concerns: concerns.is_some(),
            injected: &extra_tools,
            tool_context: &tool_context,
            alias_scope,
        })
        .map_err(|e| {
            // As before: the transcript exists (empty) when the grant fails.
            if let Err(w) = rupu_transcript::JsonlWriter::create(&transcript_path) {
                tracing::warn!(error = %w, "create transcript for a failed grant");
            }
            RunError::ToolGrant(e.to_string())
        })?;
        Ok(AgentRunOpts {
            system_prompt: agent_system_prompt,
            prompt: UserTurn {
                message: user_message,
                initial_messages,
                seed_source,
                turn_index_offset,
            },
            provider,
            limits,
            recovery,
            permission,
            grant,
            alias_scope,
            tool_context,
            pins: AgentPins {
                effort,
                thinking_display,
                context_window,
                output_format,
                output_schema,
                anthropic_task_budget,
                anthropic_context_management,
                anthropic_speed,
            },
            concerns,
            max_turns,
            stream: StreamOpts {
                no_stream,
                suppress_stdout: suppress_stream_stdout,
                on_stream_event,
            },
            hooks: Hooks {
                on_tool_call,
                on_usage,
            },
            pause,
            collectors,
            extra_tools,
            transcript_path,
        })
    }
}

impl crate::continuation::ContinuationTarget for LegacyRunOpts {
    fn seed_continuation(&mut self, messages: Vec<Message>, seed_source: PathBuf, note: String) {
        self.user_message = note;
        self.initial_messages = messages;
        self.seed_source = Some(seed_source);
    }
}

/// [`crate::run_agent`] over legacy options.
pub async fn run_agent(opts: LegacyRunOpts) -> Result<RunResult, RunError> {
    run_agent_full(opts).await.result
}

/// [`crate::run_agent_full`] over legacy options.
pub async fn run_agent_full(opts: LegacyRunOpts) -> RunExit {
    let limits = opts.limits.clone();
    match opts.into_run_opts() {
        Ok(opts) => crate::runner::run_agent_full(opts).await,
        Err(e) => RunExit {
            result: Err(e),
            final_limits: limits,
            messages: Vec::new(),
        },
    }
}
