//! The agent loop. Wires provider → tool dispatch (with permission
//! gating) → transcript writes → turn accounting → run-complete.
//!
//! This is the integration point of `rupu-providers`, `rupu-tools`,
//! and `rupu-transcript`. The CLI (Plan 2 Phase 3) calls [`run_agent`]
//! once per `rupu run` invocation.

use crate::coverage_tools;
use crate::mcp_tool::McpToolAdapter;
use crate::permission::PermissionDecision;
use crate::tool_registry::{default_tool_registry, ToolRegistry};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::{
    flatten, render_prompt_section, target_id, write_snapshot, CoveragePaths,
    CoverageWriterHandle, FlatCatalog, DEFAULT_FULL_MODE_THRESHOLD,
};
use rupu_mcp::{McpPermission, ServeHandle};
use rupu_providers::provider::LlmProvider;
use rupu_providers::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, Role, StopReason, StreamEvent, Usage,
};
use rupu_scm::Registry;
use rupu_tools::{DerivedEvent, PermissionMode, Tool, ToolContext};
use rupu_transcript::{Event, FileEditKind, JsonlWriter, RunMode, RunStatus};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;

/// Coverage bundle built from a `concerns:` block. Holds catalog, paths,
/// and prompt section for use across the run. The `CoverageWriterHandle`
/// is stored separately so it can be consumed (shutdown) at each exit point.
struct CoverageBundle {
    catalog: FlatCatalog,
    paths: CoveragePaths,
    prompt_section: String,
}

const MAX_TOOL_RESULT_BYTES: usize = 256 * 1024;

/// Callback invoked by `run_agent` immediately before each tool
/// dispatch. The runner translates this into `Event::StepWorking
/// { note: Some(tool_name) }` so the Graph view can pulse the
/// active node. Called from the agent's tokio task — must be
/// non-blocking.
pub type OnToolCallCallback = std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type OnStreamEventCallback = std::sync::Arc<dyn Fn(StreamEvent) + Send + Sync>;

fn truncate_utf8_bytes(input: &str, max_bytes: usize) -> &str {
    if input.len() <= max_bytes {
        return input;
    }
    let mut end = max_bytes;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
}

fn clamp_tool_result_text(input: &str) -> String {
    if input.len() <= MAX_TOOL_RESULT_BYTES {
        return input.to_string();
    }
    let prefix = truncate_utf8_bytes(input, MAX_TOOL_RESULT_BYTES);
    format!(
        "{prefix}\n… [truncated {} bytes]",
        input.len().saturating_sub(prefix.len())
    )
}

/// Errors that can occur during an agent run.
#[derive(Debug, Error)]
pub enum RunError {
    #[error("provider: {0}")]
    Provider(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("transcript: {0}")]
    Transcript(#[from] rupu_transcript::WriteError),
    #[error("context overflow at turn {turn}")]
    ContextOverflow { turn: u32 },
    #[error("max turns ({max}) reached")]
    MaxTurns { max: u32 },
    #[error("non-tty + ask mode aborted before first prompt")]
    NonTtyAskAbort,
    #[error("operator stopped run at turn {turn}")]
    OperatorStop { turn: u32 },
    #[error("coverage setup: {0}")]
    Coverage(String),
}

/// Pluggable permission decider. Three production impls + a `Bypass`
/// for tests.
pub trait PermissionDecider: Send + Sync {
    /// Decide whether `tool` may run with `input`. Called once per
    /// tool call before dispatch.
    fn decide(
        &self,
        mode: PermissionMode,
        tool: &str,
        input: &serde_json::Value,
        workspace_path: &str,
    ) -> Result<PermissionDecision, RunError>;
}

/// Test/CI decider: always Allow regardless of mode.
pub struct BypassDecider;

impl PermissionDecider for BypassDecider {
    fn decide(
        &self,
        _mode: PermissionMode,
        _tool: &str,
        _input: &serde_json::Value,
        _workspace_path: &str,
    ) -> Result<PermissionDecision, RunError> {
        Ok(PermissionDecision::Allow)
    }
}

/// Inputs to a single agent run.
pub struct AgentRunOpts {
    pub agent_name: String,
    pub agent_system_prompt: String,
    /// `None` = use all six tools; `Some(list)` = filter the registry.
    pub agent_tools: Option<Vec<String>>,
    pub provider: Box<dyn LlmProvider>,
    pub provider_name: String,
    pub model: String,
    pub run_id: String,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    pub transcript_path: PathBuf,
    pub max_turns: u32,
    pub decider: Arc<dyn PermissionDecider>,
    pub tool_context: ToolContext,
    pub user_message: String,
    /// Existing conversation state to prepend before `user_message`.
    /// Empty for one-shot runs.
    pub initial_messages: Vec<Message>,
    /// Absolute turn index for the first turn in this run.
    pub turn_index_offset: u32,
    pub mode_str: String,
    /// If true, skip token streaming and use `provider.send` for one-shot
    /// completions. Default is false (streaming). Used by --no-stream.
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
    /// Desired context-window tier. Anthropic api-key path uses this to
    /// gate the `context-1m-2025-08-07` beta header; other providers
    /// currently ignore it.
    pub context_window: Option<rupu_providers::model_tier::ContextWindow>,
    /// Cross-provider output-format hint. Anthropic emits as
    /// `output_config.format`; OpenAI emits as `response_format.type`;
    /// other providers ignore.
    pub output_format: Option<rupu_providers::types::OutputFormat>,
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
    /// Coverage concerns block. When `Some`, the runner flattens the
    /// catalog, writes a snapshot, injects coverage tools, and prepends
    /// the catalog to the system prompt. `None` (default) disables all
    /// coverage harness machinery.
    pub concerns: Option<rupu_coverage::ConcernsBlock>,
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
}

/// Outcome of a finished run.
pub struct RunResult {
    pub status: RunStatus,
    pub turns: u32,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub final_messages: Vec<Message>,
}

/// Drive one agent run to completion. Writes a JSONL transcript at
/// `opts.transcript_path` and returns turn/token counts on success.
pub async fn run_agent(mut opts: AgentRunOpts) -> Result<RunResult, RunError> {
    let mut writer = JsonlWriter::create(&opts.transcript_path)?;
    let started = Instant::now();
    writer.write(&Event::RunStart {
        run_id: opts.run_id.clone(),
        workspace_id: opts.workspace_id.clone(),
        agent: opts.agent_name.clone(),
        provider: opts.provider_name.clone(),
        model: opts.model.clone(),
        started_at: Utc::now(),
        mode: parse_mode_for_event(&opts.mode_str),
    })?;
    writer.flush()?;

    let mut registry: ToolRegistry = match &opts.agent_tools {
        Some(list) => default_tool_registry().filter_to(list),
        None => default_tool_registry(),
    };

    // Coverage harness: flatten catalog, write snapshot, spawn writer.
    // The handle is kept separate so it can be consumed (shutdown) at each
    // exit point, including early returns on provider/operator errors.
    let (coverage, mut coverage_handle): (Option<CoverageBundle>, Option<CoverageWriterHandle>) =
        if let Some(block) = opts.concerns.clone() {
            let catalog = flatten(&block)
                .map_err(|e| RunError::Coverage(format!("flatten coverage catalog: {e}")))?;
            let resolved_scope = opts
                .scope_name
                .as_deref()
                .unwrap_or(&opts.agent_name);
            let target = target_id(&opts.workspace_path, resolved_scope);
            let paths = CoveragePaths::new(&opts.workspace_path, &target);
            paths
                .ensure_dir()
                .map_err(|e| RunError::Coverage(format!("ensure coverage dir: {e}")))?;
            write_snapshot(&catalog, &paths.catalog)
                .map_err(|e| RunError::Coverage(format!("write catalog snapshot: {e}")))?;
            let handle = CoverageWriterHandle::spawn(paths.clone())
                .map_err(|e| RunError::Coverage(format!("spawn coverage writer: {e}")))?;
            let prompt_section = render_prompt_section(&catalog, DEFAULT_FULL_MODE_THRESHOLD);
            let bundle = CoverageBundle {
                catalog,
                paths,
                prompt_section,
            };
            (Some(bundle), Some(handle))
        } else {
            (None, None)
        };

    // Append catalog prompt section to system prompt when coverage is active.
    if let Some(bundle) = &coverage {
        opts.agent_system_prompt.push_str("\n\n");
        opts.agent_system_prompt.push_str(&bundle.prompt_section);
    }

    // Wire coverage into tool context.
    if let Some(h) = &coverage_handle {
        opts.tool_context.coverage_writer = Some(h.writer.clone());
        opts.tool_context.surface_tag = Some(
            opts.surface_tag
                .clone()
                .unwrap_or_else(|| "agent".to_string()),
        );
        opts.tool_context.run_id = Some(opts.run_id.clone());
        opts.tool_context.model = Some(opts.model.clone());
    }

    // Register coverage tools when coverage is enabled.
    if let Some(bundle) = &coverage {
        coverage_tools::register(&mut registry, bundle.catalog.clone(), bundle.paths.clone());
    }

    // MCP server: spin up before the loop if we have a Registry.
    let mcp_guard: Option<(rupu_mcp::InProcessTransport, ServeHandle)> =
        if let Some(scm_registry) = opts.mcp_registry.clone() {
            let allowlist = opts
                .agent_tools
                .clone()
                .unwrap_or_else(|| vec!["*".to_string()]);
            let mode = parse_mode_for_runtime(&opts.mode_str);
            let permission = McpPermission::new(mode, allowlist);
            let (transport, handle) =
                rupu_mcp::serve_in_process(scm_registry.clone(), permission.clone());
            let dispatcher = Arc::new(rupu_mcp::ToolDispatcher::new(scm_registry, permission));

            // Insert each MCP tool into the agent's tool registry,
            // BUT respect the agent's `tools:` allowlist when present.
            // Otherwise the model would see scm.* / issues.* / vendor
            // tools advertised even when the agent declared a narrower
            // surface — the model picks one of the leaked tools, the
            // dispatcher denies it (correctly), and we waste turns on
            // permission_denied retries. With this gate the model only
            // sees what it's allowed to call.
            //
            // `None` means "no agent allowlist" → register everything,
            // matching the prior unrestricted behavior.
            for spec in rupu_mcp::tool_catalog() {
                let allowed = match &opts.agent_tools {
                    None => true,
                    Some(list) => mcp_tool_name_matches_allowlist(spec.name, list),
                };
                if !allowed {
                    continue;
                }
                let adapter = Arc::new(McpToolAdapter::new(
                    spec.name,
                    spec.description,
                    spec.input_schema.clone(),
                    dispatcher.clone(),
                ));
                registry.insert(spec.name.to_string(), adapter as Arc<dyn Tool>);
            }

            Some((transport, handle))
        } else {
            None
        };

    let tool_defs = registry.to_tool_definitions();

    let mut messages: Vec<Message> = opts.initial_messages.clone();
    messages.push(Message::user(&opts.user_message));
    let mut turn_idx: u32 = opts.turn_index_offset;
    let initial_turn_idx = turn_idx;
    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut runtime_mode = parse_mode_for_runtime(&opts.mode_str);

    let result_status = loop {
        if turn_idx >= opts.max_turns {
            break RunStatus::Error;
        }
        writer.write(&Event::TurnStart { turn_idx })?;
        let req = LlmRequest {
            model: opts.model.clone(),
            system: Some(opts.agent_system_prompt.clone()),
            messages: messages.clone(),
            max_tokens: 4096,
            tools: tool_defs.clone(),
            cell_id: None,
            trace_id: None,
            thinking: opts.effort,
            context_window: opts.context_window,
            task_type: None,
            output_format: opts.output_format,
            anthropic_task_budget: opts.anthropic_task_budget,
            anthropic_context_management: opts.anthropic_context_management,
            anthropic_speed: opts.anthropic_speed,
        };
        let resp: LlmResponse = if opts.no_stream {
            match opts.provider.send(&req).await {
                Ok(r) => r,
                Err(e) => {
                    writer.write(&Event::RunComplete {
                        run_id: opts.run_id.clone(),
                        status: RunStatus::Error,
                        total_tokens: total_in + total_out,
                        duration_ms: started.elapsed().as_millis() as u64,
                        error: Some(format!("provider: {e}")),
                    })?;
                    writer.flush()?;
                    opts.tool_context.coverage_writer = None;
                    if let Some(h) = coverage_handle.take() {
                        h.shutdown().await;
                    }
                    return Err(RunError::Provider(e.to_string()));
                }
            }
        } else {
            let suppress = opts.suppress_stream_stdout;
            let mut stream_transcript_error: Option<rupu_transcript::WriteError> = None;
            let mut on_event = |ev: StreamEvent| {
                if let Some(cb) = opts.on_stream_event.as_ref() {
                    cb(ev.clone());
                }
                match ev {
                    StreamEvent::TextDelta(chunk) => {
                        if !chunk.is_empty() && stream_transcript_error.is_none() {
                            if let Err(err) = writer
                                .write(&Event::AssistantDelta {
                                    content: chunk.clone(),
                                })
                                .and_then(|_| writer.flush())
                            {
                                stream_transcript_error = Some(err);
                            }
                        }
                        if !suppress {
                            use std::io::Write;
                            print!("{chunk}");
                            let _ = std::io::stdout().flush();
                        }
                    }
                    StreamEvent::UsageSnapshot(_) => {}
                    StreamEvent::ToolUseStart { .. } | StreamEvent::InputJsonDelta(_) => {}
                }
            };
            match opts.provider.stream(&req, &mut on_event).await {
                Ok(r) => {
                    if !suppress {
                        println!();
                    }
                    if let Some(err) = stream_transcript_error {
                        opts.tool_context.coverage_writer = None;
                        if let Some(h) = coverage_handle.take() {
                            h.shutdown().await;
                        }
                        return Err(RunError::Transcript(err));
                    }
                    r
                }
                Err(e) => {
                    writer.write(&Event::RunComplete {
                        run_id: opts.run_id.clone(),
                        status: RunStatus::Error,
                        total_tokens: total_in + total_out,
                        duration_ms: started.elapsed().as_millis() as u64,
                        error: Some(format!("provider: {e}")),
                    })?;
                    writer.flush()?;
                    opts.tool_context.coverage_writer = None;
                    if let Some(h) = coverage_handle.take() {
                        h.shutdown().await;
                    }
                    return Err(RunError::Provider(e.to_string()));
                }
            }
        };
        total_in += resp.usage.input_tokens as u64;
        total_out += resp.usage.output_tokens as u64;
        writer.write(&Event::Usage {
            provider: opts.provider_name.clone(),
            model: if resp.model.is_empty() {
                opts.model.clone()
            } else {
                resp.model.clone()
            },
            input_tokens: resp.usage.input_tokens,
            output_tokens: resp.usage.output_tokens,
            cached_tokens: resp.usage.cached_tokens,
        })?;

        // Emit any text content as assistant_message events; collect
        // tool_use blocks for dispatch.
        let mut tool_uses: Vec<(String, String, serde_json::Value)> = Vec::new();
        for block in &resp.content {
            match block {
                ContentBlock::Text { text } => {
                    writer.write(&Event::AssistantMessage {
                        content: text.clone(),
                        thinking: None,
                    })?;
                }
                ContentBlock::ToolUse { id, name, input } => {
                    writer.write(&Event::ToolCall {
                        call_id: id.clone(),
                        tool: name.clone(),
                        input: input.clone(),
                    })?;
                    tool_uses.push((id.clone(), name.clone(), input.clone()));
                }
                ContentBlock::ToolResult { .. } => {
                    // Models don't produce tool_result blocks themselves;
                    // those originate from the runtime feeding tool
                    // outputs back. Ignore if seen.
                }
            }
        }

        // Dispatch tool calls in order.
        let mut tool_results: Vec<(String, String, Option<String>)> = Vec::new();
        for (call_id, tool_name, input) in tool_uses {
            // Permission gate.
            let decision = opts.decider.decide(
                runtime_mode,
                &tool_name,
                &input,
                &opts.workspace_path.display().to_string(),
            )?;
            match decision {
                PermissionDecision::Deny => {
                    writer.write(&Event::ToolResult {
                        call_id: call_id.clone(),
                        output: String::new(),
                        error: Some("permission_denied".into()),
                        duration_ms: 0,
                    })?;
                    tool_results.push((call_id, String::new(), Some("permission_denied".into())));
                    continue;
                }
                PermissionDecision::StopRun => {
                    writer.write(&Event::RunComplete {
                        run_id: opts.run_id.clone(),
                        status: RunStatus::Aborted,
                        total_tokens: total_in + total_out,
                        duration_ms: started.elapsed().as_millis() as u64,
                        error: Some("operator_stop".into()),
                    })?;
                    writer.flush()?;
                    opts.tool_context.coverage_writer = None;
                    if let Some(h) = coverage_handle.take() {
                        h.shutdown().await;
                    }
                    return Err(RunError::OperatorStop { turn: turn_idx });
                }
                PermissionDecision::AllowAlwaysForToolThisRun => {
                    runtime_mode = PermissionMode::Bypass;
                }
                PermissionDecision::Allow => {}
            }

            let tool: Arc<dyn Tool> = match registry.get(&tool_name) {
                Some(t) => t,
                None => {
                    writer.write(&Event::ToolResult {
                        call_id: call_id.clone(),
                        output: String::new(),
                        error: Some(format!("unknown tool: {tool_name}")),
                        duration_ms: 0,
                    })?;
                    tool_results.push((call_id, String::new(), Some("unknown_tool".into())));
                    continue;
                }
            };
            if let Some(cb) = opts.on_tool_call.as_ref() {
                cb(&opts.step_id, &tool_name);
            }
            let started_tool = Instant::now();
            match tool.invoke(input.clone(), &opts.tool_context).await {
                Ok(out) => {
                    let clamped_stdout = clamp_tool_result_text(&out.stdout);
                    writer.write(&Event::ToolResult {
                        call_id: call_id.clone(),
                        output: clamped_stdout.clone(),
                        error: out.error.clone(),
                        duration_ms: started_tool.elapsed().as_millis() as u64,
                    })?;
                    if let Some(d) = out.derived.clone() {
                        match d {
                            DerivedEvent::FileEdit { path, kind, diff } => {
                                writer.write(&Event::FileEdit {
                                    path,
                                    kind: parse_file_edit_kind(&kind),
                                    diff,
                                })?;
                            }
                            DerivedEvent::CommandRun {
                                argv,
                                cwd,
                                exit_code,
                                stdout_bytes,
                                stderr_bytes,
                            } => {
                                writer.write(&Event::CommandRun {
                                    argv,
                                    cwd,
                                    exit_code,
                                    stdout_bytes,
                                    stderr_bytes,
                                })?;
                            }
                        }
                    }
                    tool_results.push((call_id, clamped_stdout, out.error));
                }
                Err(e) => {
                    let msg = format!("{e}");
                    writer.write(&Event::ToolResult {
                        call_id: call_id.clone(),
                        output: String::new(),
                        error: Some(msg.clone()),
                        duration_ms: started_tool.elapsed().as_millis() as u64,
                    })?;
                    tool_results.push((call_id, String::new(), Some(msg)));
                }
            }
        }

        writer.write(&Event::TurnEnd {
            turn_idx,
            tokens_in: Some(resp.usage.input_tokens as u64),
            tokens_out: Some(resp.usage.output_tokens as u64),
        })?;
        writer.flush()?;

        // Append assistant + tool_result(s) to messages so the next
        // turn sees them. `Message::assistant` only takes &str, so we
        // construct a multi-block assistant message manually.
        messages.push(Message {
            role: Role::Assistant,
            content: resp.content.clone(),
        });
        if !tool_results.is_empty() {
            let mut blocks: Vec<ContentBlock> = Vec::new();
            for (call_id, output, error) in tool_results {
                let is_error = error.is_some();
                let content = if let Some(e) = error {
                    clamp_tool_result_text(&format!("error: {e}\n{output}"))
                } else {
                    output
                };
                blocks.push(ContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content,
                    is_error,
                });
            }
            messages.push(Message {
                role: Role::User,
                content: blocks,
            });
        }

        turn_idx += 1;
        // `stop_reason` is `Option<StopReason>`. None → keep looping.
        if matches!(resp.stop_reason, Some(StopReason::EndTurn)) {
            break RunStatus::Ok;
        }
    };

    writer.write(&Event::RunComplete {
        run_id: opts.run_id.clone(),
        status: result_status,
        total_tokens: total_in + total_out,
        duration_ms: started.elapsed().as_millis() as u64,
        error: None,
    })?;
    writer.flush()?;

    // Drop the MCP transport so the server's recv() returns None and exits.
    // Then await its JoinHandle for deterministic shutdown.
    if let Some((transport, handle)) = mcp_guard {
        drop(transport);
        let _ = handle.join.await;
    }

    // Shutdown the coverage writer. Drop the ToolContext's Arc<CoverageWriter>
    // clone BEFORE calling shutdown so the writer task's mpsc channel closes
    // cleanly (shutdown drops the handle's Arc, but if ToolContext still holds
    // a clone the task's recv() would never return None and task.await hangs).
    opts.tool_context.coverage_writer = None;
    if let Some(h) = coverage_handle {
        h.shutdown().await;
    }

    Ok(RunResult {
        status: result_status,
        turns: turn_idx.saturating_sub(initial_turn_idx),
        total_tokens_in: total_in,
        total_tokens_out: total_out,
        final_messages: messages,
    })
}

fn parse_mode_for_event(s: &str) -> RunMode {
    match s {
        "bypass" => RunMode::Bypass,
        "readonly" => RunMode::Readonly,
        _ => RunMode::Ask,
    }
}

fn parse_mode_for_runtime(s: &str) -> PermissionMode {
    match s {
        "bypass" => PermissionMode::Bypass,
        "readonly" => PermissionMode::Readonly,
        _ => PermissionMode::Ask,
    }
}

fn parse_file_edit_kind(s: &str) -> FileEditKind {
    match s {
        "create" => FileEditKind::Create,
        "delete" => FileEditKind::Delete,
        _ => FileEditKind::Modify,
    }
}

/// Mirror the allowlist match `McpPermission::tool_in_allowlist` uses,
/// scoped to the registration-time decision (do we even ADVERTISE
/// this tool to the model?). Same wildcard semantics:
///
/// - `*` matches everything
/// - `prefix*` matches any tool whose name starts with `prefix`
///   (e.g. `scm.*` matches `scm.repos.list`, `scm.files.read`, …)
/// - exact match otherwise
///
/// Built-in (non-MCP) tool names like `bash` / `read` in the agent's
/// `tools:` list don't appear in the MCP catalog, so they correctly
/// don't match anything here — they're registered separately by
/// `default_tool_registry`.
fn mcp_tool_name_matches_allowlist(name: &str, allowlist: &[String]) -> bool {
    allowlist.iter().any(|entry| {
        if entry == "*" || entry == name {
            return true;
        }
        if let Some(prefix) = entry.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            false
        }
    })
}

// ---------------------------------------------------------------------------
// on_tool_call callback test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod on_tool_call_tests {
    use super::*;
    use rupu_providers::types::StopReason;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn on_tool_call_fires_once_per_tool_invocation() {
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls_clone = calls.clone();
        let cb: OnToolCallCallback = Arc::new(move |step_id: &str, tool_name: &str| {
            calls_clone
                .lock()
                .unwrap()
                .push(format!("{step_id}:{tool_name}"));
        });

        // Two-turn script: turn 0 → tool use (read_file), turn 1 → final text.
        // The read_file tool needs a `path` input; we point it at a real
        // filesystem path (the runner.rs source file itself) so the call
        // succeeds and the agent proceeds to the final turn.
        let tmp_dir = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp_dir.path().join("run_test.jsonl");

        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_read_1".into(),
                tool_name: "read_file".into(),
                tool_input: serde_json::json!({
                    "path": tmp_dir.path().to_str().unwrap_or("/tmp")
                }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "done".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]);

        let opts = AgentRunOpts {
            agent_name: "test-agent".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id: "run_test_cb".into(),
            workspace_id: "ws_test".into(),
            workspace_path: tmp_dir.path().to_path_buf(),
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext {
                workspace_path: tmp_dir.path().to_path_buf(),
                ..Default::default()
            },
            user_message: "test prompt".into(),
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: true,
            suppress_stream_stdout: false,
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
            step_id: "s1".into(),
            on_tool_call: Some(cb),
            on_stream_event: None,
            concerns: None,
            scope_name: None,
            surface_tag: None,
        };

        run_agent(opts).await.expect("agent run succeeds");

        let log = calls.lock().unwrap();
        assert_eq!(
            log.len(),
            1,
            "expected exactly one on_tool_call, got {log:?}"
        );
        assert!(
            log[0].starts_with("s1:"),
            "expected step_id 's1' prefix, got {}",
            log[0]
        );
    }
}

#[cfg(test)]
mod allowlist_tests {
    use super::mcp_tool_name_matches_allowlist;

    #[test]
    fn exact_match() {
        let list = vec!["scm.repos.get".into(), "issues.list".into()];
        assert!(mcp_tool_name_matches_allowlist("scm.repos.get", &list));
        assert!(mcp_tool_name_matches_allowlist("issues.list", &list));
        assert!(!mcp_tool_name_matches_allowlist("scm.files.read", &list));
    }

    #[test]
    fn star_matches_all() {
        let list = vec!["*".into()];
        assert!(mcp_tool_name_matches_allowlist("any.tool", &list));
        assert!(mcp_tool_name_matches_allowlist("scm.files.read", &list));
    }

    #[test]
    fn namespace_wildcard() {
        let list = vec!["scm.*".into()];
        assert!(mcp_tool_name_matches_allowlist("scm.repos.get", &list));
        assert!(mcp_tool_name_matches_allowlist("scm.files.read", &list));
        assert!(!mcp_tool_name_matches_allowlist("issues.list", &list));
    }

    #[test]
    fn builtin_tools_are_not_matched() {
        // `bash` / `read` are agent-side built-ins, not MCP tools.
        // The allowlist may list them but no MCP tool catalog entry
        // should ever match them.
        let list = vec!["bash".into(), "read".into()];
        assert!(!mcp_tool_name_matches_allowlist("scm.repos.get", &list));
        assert!(!mcp_tool_name_matches_allowlist("issues.list", &list));
    }
}

#[cfg(test)]
mod tool_result_clamp_tests {
    use super::{clamp_tool_result_text, MAX_TOOL_RESULT_BYTES};

    #[test]
    fn clamp_tool_result_text_truncates_large_payloads() {
        let raw = "a".repeat(MAX_TOOL_RESULT_BYTES + 64);
        let clamped = clamp_tool_result_text(&raw);
        assert!(clamped.len() < raw.len());
        assert!(clamped.contains("[truncated "));
    }
}

// ---------------------------------------------------------------------------
// Mock provider for tests. Public so rupu-cli integration tests can reuse.
// ---------------------------------------------------------------------------

/// One scripted assistant turn the [`MockProvider`] will replay.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ScriptedTurn {
    AssistantText {
        text: String,
        stop: StopReason,
        /// Token counts reported in the mock response. Defaults to 1/1 when
        /// omitted so existing scripts stay valid.
        #[serde(default = "default_mock_tokens")]
        input_tokens: u32,
        #[serde(default = "default_mock_tokens")]
        output_tokens: u32,
    },
    AssistantToolUse {
        text: Option<String>,
        tool_id: String,
        tool_name: String,
        tool_input: serde_json::Value,
        stop: StopReason,
    },
    ProviderError(String),
}

fn default_mock_tokens() -> u32 {
    1
}

/// In-memory `LlmProvider` that replays a fixed script. Used by tests
/// in `rupu-agent` and (later) `rupu-cli`.
pub struct MockProvider {
    script: std::sync::Mutex<std::collections::VecDeque<ScriptedTurn>>,
}

impl MockProvider {
    pub fn new(turns: Vec<ScriptedTurn>) -> Self {
        Self {
            script: std::sync::Mutex::new(turns.into()),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn send(
        &mut self,
        _req: &LlmRequest,
    ) -> Result<LlmResponse, rupu_providers::ProviderError> {
        let next = {
            let mut q = self.script.lock().unwrap();
            q.pop_front()
        };
        let turn = next.ok_or_else(|| {
            rupu_providers::ProviderError::Http("mock script exhausted".to_string())
        })?;
        match turn {
            ScriptedTurn::ProviderError(e) => Err(rupu_providers::ProviderError::Http(e)),
            ScriptedTurn::AssistantText {
                text,
                stop,
                input_tokens,
                output_tokens,
            } => Ok(LlmResponse {
                id: "mock".to_string(),
                model: "mock-1".to_string(),
                content: vec![ContentBlock::Text { text }],
                stop_reason: Some(stop),
                usage: Usage {
                    input_tokens,
                    output_tokens,
                    ..Default::default()
                },
            }),
            ScriptedTurn::AssistantToolUse {
                text,
                tool_id,
                tool_name,
                tool_input,
                stop,
            } => {
                let mut blocks = Vec::new();
                if let Some(t) = text {
                    blocks.push(ContentBlock::Text { text: t });
                }
                blocks.push(ContentBlock::ToolUse {
                    id: tool_id,
                    name: tool_name,
                    input: tool_input,
                });
                Ok(LlmResponse {
                    id: "mock".to_string(),
                    model: "mock-1".to_string(),
                    content: blocks,
                    stop_reason: Some(stop),
                    usage: Usage {
                        input_tokens: 1,
                        output_tokens: 1,
                        ..Default::default()
                    },
                })
            }
        }
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        _on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, rupu_providers::ProviderError> {
        // For v0 the mock doesn't actually stream — it just calls send.
        self.send(req).await
    }

    fn default_model(&self) -> &str {
        "mock-1"
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        rupu_providers::ProviderId::Anthropic
    }
}

/// Like [`MockProvider`], but stores every received [`LlmRequest`] for
/// post-run assertion. Use in tests that need to verify the runner's
/// outbound request shape (e.g., that `tools` is populated).
pub struct CapturingMockProvider {
    inner: MockProvider,
    /// Captured requests in the order they were sent. Populated by
    /// `send` calls.
    pub captured: std::sync::Arc<std::sync::Mutex<Vec<LlmRequest>>>,
}

impl CapturingMockProvider {
    pub fn new(turns: Vec<ScriptedTurn>) -> Self {
        Self {
            inner: MockProvider::new(turns),
            captured: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Snapshot of captured requests. Call after `run_agent` returns.
    pub fn captured_requests(&self) -> Vec<LlmRequest> {
        self.captured.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for CapturingMockProvider {
    async fn send(
        &mut self,
        req: &LlmRequest,
    ) -> Result<LlmResponse, rupu_providers::ProviderError> {
        self.captured.lock().unwrap().push(req.clone());
        self.inner.send(req).await
    }

    async fn stream(
        &mut self,
        req: &LlmRequest,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<LlmResponse, rupu_providers::ProviderError> {
        self.captured.lock().unwrap().push(req.clone());
        self.inner.stream(req, on_event).await
    }

    fn default_model(&self) -> &str {
        self.inner.default_model()
    }

    fn provider_id(&self) -> rupu_providers::ProviderId {
        self.inner.provider_id()
    }
}
