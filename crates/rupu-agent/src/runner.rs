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
    flatten, render_prompt_section, target_id, write_snapshot, CoveragePaths, CoverageWriterHandle,
    FlatCatalog, DEFAULT_FULL_MODE_THRESHOLD,
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

/// Maximum times a single provider call is retried on a transient error
/// (network/decode/SSE/5xx/rate-limit) before the step is failed. The run
/// itself continues or aborts per the workflow's `continue_on_error`.
const MAX_HTTP_RETRIES: u32 = 10;

/// Capped exponential backoff for transient-error retries: 250ms, 500ms,
/// 1s, 2s, 4s, then 8s for every further attempt.
fn retry_backoff(attempt: u32) -> std::time::Duration {
    let shift = attempt.saturating_sub(1).min(5);
    std::time::Duration::from_millis(250u64 * (1u64 << shift))
}

/// Whether a provider error is worth retrying. Transient failures —
/// network/transport, response-decode, SSE-parse, unexpected stream end,
/// token-refresh blips, rate limits, and 429/5xx API errors — are retried.
/// Config/auth/client errors (unknown provider, unauthorized, bad request,
/// model unavailable, …) fail fast so we don't spin on a permanent problem.
fn is_retryable_provider_error(e: &rupu_providers::ProviderError) -> bool {
    use rupu_providers::ProviderError as E;
    match e {
        E::Http(_)
        | E::SseParse(_)
        | E::Json(_)
        | E::UnexpectedEndOfStream
        | E::TokenRefreshFailed(_)
        | E::RateLimited { .. }
        | E::Transient(_) => true,
        E::Api { status, .. } => *status == 429 || *status >= 500,
        E::MissingAuth { .. }
        | E::AuthConfig(_)
        | E::Unauthorized { .. }
        | E::QuotaExceeded { .. }
        | E::NotImplemented { .. }
        | E::BadRequest { .. }
        | E::ModelUnavailable { .. }
        | E::Other(_) => false,
    }
}

/// Default per-request output-token budget when an agent doesn't set
/// `maxTokens`. 4096 was too low for output-heavy agents (it truncated
/// responses before a tool call could be emitted, especially with extended
/// thinking, which draws from the same budget).
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

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

/// Drop the oldest assistant↔user exchange from the conversation so the
/// next request fits the model's context window. Preserves Anthropic's
/// invariants: the message list still starts with the original (user)
/// message, roles stay alternating, and tool_use/tool_result pairs are
/// removed together (we remove a single adjacent [Assistant, User] pair,
/// which keeps strict alternation intact). Never touches the first message
/// (the anchor user turn) or the last message (the current turn). Returns
/// the number of messages dropped (0 if there is nothing safe to drop).
fn trim_oldest_exchange(messages: &mut Vec<Message>) -> usize {
    // Need at least: anchor user [0], one droppable exchange [1,2], and the
    // current turn [last]. So len >= 4 and indices 1,2 must not be the last.
    if messages.len() < 4 {
        return 0;
    }
    if messages[1].role == Role::Assistant && messages[2].role == Role::User {
        messages.drain(1..=2);
        return 2;
    }
    0
}

/// Return true when the provider error string signals a context-window overflow.
fn is_context_overflow(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("prompt is too long")
        || e.contains("too many tokens")
        || e.contains("context window")
}

/// Estimate token count for a slice of messages using chars/4 approximation.
/// Kept for reference and tests; production sizing now uses calibrated char budgets.
#[allow(dead_code)]
pub(crate) fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|block| match block {
            ContentBlock::Text { text } => text.len() / 4,
            ContentBlock::ToolUse { name, input, .. } => {
                let input_str = input.to_string();
                (name.len() + input_str.len()) / 4
            }
            ContentBlock::ToolResult { content, .. } => content.len() / 4,
            // raw is what goes on the wire (thinking text + signature); text is
            // only a display summary and is None when display is "omitted".
            ContentBlock::Reasoning { raw, .. } => raw.to_string().len() / 4,
            ContentBlock::Unknown => 0,
        })
        .sum()
}

/// Sum raw character counts across all content blocks in a message.
/// Used for char-budget partitioning (calibrated via the provider's real token count).
fn message_chars(m: &Message) -> usize {
    m.content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.len(),
            ContentBlock::ToolUse { name, input, .. } => {
                let input_str = input.to_string();
                name.len() + input_str.len()
            }
            ContentBlock::ToolResult { content, .. } => content.len(),
            // raw is what goes on the wire (thinking text + signature); text is
            // only a display summary and is None when display is "omitted".
            ContentBlock::Reasoning { raw, .. } => raw.to_string().len(),
            ContentBlock::Unknown => 0,
        })
        .sum()
}

/// Compute the token count at which compaction triggers.
/// Returns `None` if `context_window_tokens` is `None` (compaction disabled).
pub(crate) fn effective_compact_threshold(
    context_window_tokens: Option<u32>,
    compact_at_percent: Option<u8>,
) -> Option<u64> {
    let window = context_window_tokens?;
    let pct = compact_at_percent.unwrap_or(80).clamp(10, 95) as u64;
    Some(window as u64 * pct / 100)
}

/// Partition `messages` into `(middle_start, recent_start)` for compaction.
///
/// - `messages[0..middle_start]` = task (always index 0, so `middle_start = 1`)
/// - `messages[middle_start..recent_start]` = middle to summarise
/// - `messages[recent_start..]` = recent verbatim
///
/// `recent_budget_chars` is a raw character budget derived from the provider's
/// real `input_tokens` (see `compact_context`). Walking in chars avoids the
/// ~2x undercount that the old `chars/4` token estimate produced for code/JSON
/// heavy conversations, which caused compaction to treat the entire history as
/// "recent" and skip.
///
/// Returns `None` if there is nothing in the middle (i.e. `recent_start <= 1`).
pub(crate) fn partition_for_compaction(
    messages: &[Message],
    recent_budget_chars: usize,
) -> Option<(usize, usize)> {
    let middle_start = 1usize;
    if messages.len() <= 2 {
        return None;
    }

    // Walk from the end, accumulating chars, to find recent_start.
    let mut accumulated = 0usize;
    let mut recent_start = messages.len();

    // Always keep at least the last 2 messages (one complete exchange).
    let min_recent_start = messages.len().saturating_sub(2);

    for i in (middle_start..messages.len()).rev() {
        let msg_chars = message_chars(&messages[i]);
        if recent_start <= min_recent_start {
            // We've reached the minimum we must keep — stop.
            break;
        }
        if accumulated + msg_chars > recent_budget_chars && recent_start <= messages.len() - 2 {
            // Adding this message would exceed the budget and we already have ≥2.
            break;
        }
        accumulated += msg_chars;
        recent_start = i;
    }

    // Don't split a tool_use / tool_result pair:
    // if messages[recent_start] is a User message with ToolResult content
    // AND messages[recent_start - 1] is an Assistant with ToolUse content,
    // move recent_start back by 1 to include the assistant turn.
    if recent_start > middle_start && recent_start < messages.len() {
        let is_tool_result_user = messages[recent_start].role == Role::User
            && messages[recent_start]
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }));
        let prev_is_tool_use_assistant = messages[recent_start - 1].role == Role::Assistant
            && messages[recent_start - 1]
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        if is_tool_result_user && prev_is_tool_use_assistant {
            recent_start -= 1;
        }
    }

    if recent_start <= middle_start {
        return None;
    }

    Some((middle_start, recent_start))
}

/// Rebuild the compacted message list from task text, a summary, and the recent messages.
pub(crate) fn rebuild_compacted(
    task_text: &str,
    summary: &str,
    recent: Vec<Message>,
) -> Vec<Message> {
    let combined = format!("{task_text}\n\n## Summary of prior work\n{summary}");
    let mut result = vec![Message::user(&combined)];
    result.extend(recent);
    result
}

/// Result of compacting a conversation: the new (shorter) message list and how
/// many middle messages were replaced by the summary.
pub struct CompactionOutcome {
    pub messages: Vec<Message>,
    pub summarized_messages: usize,
}

/// Compact a conversation by summarising the middle messages. Returns
/// `Ok(Some(outcome))` when compaction was performed, `Ok(None)` when the
/// history is too short or there is nothing to summarise.
///
/// `last_input_tokens` is the provider's real input token count from the most
/// recent turn. It is used to calibrate a char→token ratio so the recent-message
/// budget is expressed in raw chars that correspond to the correct token count,
/// rather than relying on a fixed chars/4 estimate that undercounts code/JSON-heavy
/// conversations by ~2x.
pub async fn compact_messages(
    messages: &[Message],
    provider: &mut dyn LlmProvider,
    model: &str,
    context_window_tokens: u32,
    compact_at_percent: Option<u8>,
    last_input_tokens: u32,
) -> Result<Option<CompactionOutcome>, rupu_providers::ProviderError> {
    // Calibrate: derive tokens-per-char from what the provider actually charged.
    let total_chars: usize = messages.iter().map(message_chars).sum();
    let tokens_per_char = (last_input_tokens as f64 / total_chars.max(1) as f64).max(0.25_f64);

    let threshold = effective_compact_threshold(Some(context_window_tokens), compact_at_percent)
        .unwrap_or(context_window_tokens as u64 / 2);
    let target_recent_tokens = (threshold / 2) as f64;
    let recent_budget_chars = (target_recent_tokens / tokens_per_char) as usize;

    let (middle_start, recent_start) = match partition_for_compaction(messages, recent_budget_chars)
    {
        Some(p) => p,
        None => return Ok(None),
    };

    // Build summary request.
    let summary_prompt = "Summarize the conversation so far for continuation. \
Preserve, as concise structured notes: the objective/task; key decisions and \
conclusions; findings and their locations; files/areas already examined; the \
current state; and any open threads or planned next steps. Omit chit-chat and \
redundant tool output. This summary replaces the omitted turns, so it must be \
self-contained.";
    let summary_max_tokens = 8192u32;
    let summary_req = LlmRequest {
        model: model.to_string(),
        system: Some(summary_prompt.to_string()),
        messages: messages[..recent_start].to_vec(),
        max_tokens: summary_max_tokens,
        tools: vec![],
        cell_id: None,
        trace_id: None,
        // The history carried in this request (messages[..recent_start]) may
        // contain `ContentBlock::Reasoning` blocks echoed back onto the wire
        // as `thinking` blocks (see `restore_reasoning_blocks` in
        // rupu-providers/src/anthropic.rs). Anthropic requires a `thinking`
        // config to be present whenever thinking blocks appear in assistant
        // history, or the request 400s. Setting `Auto` here matches the
        // thinking configuration of the surrounding turns whose history this
        // request carries, and keeps the api-key auth path consistent with
        // OAuth, which already injects adaptive thinking on requests like
        // this one (see `build_request_body` in anthropic.rs).
        thinking: Some(rupu_providers::model_tier::ThinkingLevel::Auto),
        context_window: None,
        task_type: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
    };

    let summary_resp = provider.send(&summary_req).await?;

    // Extract summary text from the response.
    let summary: String = summary_resp
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Extract task text from messages[0].
    let task_text: String = messages[0]
        .content
        .iter()
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let summarized_messages = recent_start - middle_start;
    let recent: Vec<Message> = messages[recent_start..].to_vec();
    let rebuilt = rebuild_compacted(&task_text, &summary, recent);

    Ok(Some(CompactionOutcome {
        messages: rebuilt,
        summarized_messages,
    }))
}

/// Run one compaction cycle on `messages`. Returns `true` if compaction
/// was performed (messages mutated), `false` if it was skipped (no mutation).
///
/// `last_input_tokens` is the provider's real input token count from the
/// triggering turn. It is used to calibrate a char→token ratio so that the
/// recent-message budget is expressed in raw chars that correspond to the
/// correct token count, rather than relying on a fixed chars/4 estimate that
/// undercounts code/JSON-heavy conversations by ~2x.
async fn compact_context(
    messages: &mut Vec<Message>,
    opts: &mut AgentRunOpts,
    run_id: &str,
    seq: u32,
    writer: &mut JsonlWriter,
    last_input_tokens: u32,
) -> bool {
    let context_window_tokens = match opts.context_window_tokens {
        Some(w) => w,
        None => return false,
    };

    // Best-effort dump of the full pre-compaction messages as JSON before
    // calling compact_messages (which may mutate state via the provider).
    let backup_display = if let Some(parent) = opts.transcript_path.parent() {
        let compaction_dir = parent.join("compaction");
        let backup_filename = format!("{run_id}-{seq}.json");
        let backup_path = compaction_dir.join(&backup_filename);
        if let Err(e) = std::fs::create_dir_all(&compaction_dir) {
            tracing::warn!(error = %e, "failed to create compaction dir");
        }
        match serde_json::to_string_pretty(messages) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&backup_path, &json) {
                    tracing::warn!(error = %e, path = %backup_path.display(), "failed to write compaction backup");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize messages for backup");
            }
        }
        backup_path.display().to_string()
    } else {
        "(no backup path)".to_string()
    };

    match compact_messages(
        messages,
        opts.provider.as_mut(),
        &opts.model,
        context_window_tokens,
        opts.compact_at_percent,
        last_input_tokens,
    )
    .await
    {
        Ok(Some(outcome)) => {
            let middle_len = outcome.summarized_messages;
            *messages = outcome.messages;
            let note = format!(
                "[context compacted: summarized {middle_len} turns; backup {backup_display}]\n"
            );
            if let Err(e) = writer
                .write(&Event::AssistantDelta {
                    content: note.clone(),
                })
                .and_then(|_| writer.flush())
            {
                tracing::warn!(error = %e, "failed to write compaction event to transcript");
            }
            tracing::info!(
                middle_len,
                remaining = messages.len(),
                backup = %backup_display,
                "context compacted"
            );
            true
        }
        Ok(None) => false,
        Err(e) => {
            tracing::warn!(error = %e, "compaction summariser call failed — skipping compaction");
            false
        }
    }
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
    /// Coverage concerns block. When `Some`, the runner flattens the
    /// catalog, writes a snapshot, injects coverage tools, and prepends
    /// the catalog to the system prompt. `None` (default) disables all
    /// coverage harness machinery.
    pub concerns: Option<rupu_coverage::ConcernsBlock>,
    /// Per-request output-token budget (`max_tokens`). See `DEFAULT_MAX_TOKENS`.
    pub max_tokens: u32,
    /// Model context-window size in tokens for compaction. `None` = compaction disabled.
    pub context_window_tokens: Option<u32>,
    /// Threshold percentage for compaction. Defaults to 80, clamped to [10, 95].
    pub compact_at_percent: Option<u8>,
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
}

/// Outcome of a finished run.
pub struct RunResult {
    pub status: RunStatus,
    pub turns: u32,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
    pub final_messages: Vec<Message>,
    /// `true` when the run stopped at a cooperative pause boundary rather
    /// than completing or erroring (see [`AgentRunOpts::pause`]). The
    /// transcript / `final_messages` are persisted through the last
    /// complete message or tool result, ready to seed a resume. On a
    /// paused outcome `status` is [`RunStatus::Aborted`] for transcript
    /// compatibility; callers (the orchestrator) map `paused` to their
    /// own `RunStatus::Paused`.
    pub paused: bool,
}

/// Await a cooperative pause signal. Resolves when the token is cancelled;
/// never resolves when there is no token (so a `select!` arm using it is
/// inert on the no-pause path — today's behavior).
async fn wait_pause(pause: &Option<tokio_util::sync::CancellationToken>) {
    match pause {
        Some(t) => t.cancelled().await,
        None => std::future::pending::<()>().await,
    }
}

/// Whether the pause token is currently set. Used at the after-tool
/// boundary to decide whether to stop before the next turn.
fn is_paused(pause: &Option<tokio_util::sync::CancellationToken>) -> bool {
    pause.as_ref().is_some_and(|t| t.is_cancelled())
}

/// Result of a single provider call attempt inside the retry loop.
enum CallStep {
    Ok(LlmResponse),
    Err(rupu_providers::ProviderError),
    /// The pause token fired while the provider call was in flight; the
    /// in-flight stream was dropped and any partial text discarded.
    Paused,
}

/// Outcome of the provider-call retry loop for one turn.
enum CallOutcome {
    Response(LlmResponse),
    Paused,
}

/// Terminal outcome of the turn loop. `Paused` is a cooperative-pause
/// outcome (not an error); the caller maps it to a paused `RunResult`.
enum LoopOutcome {
    Status(RunStatus),
    Paused,
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
    let (coverage, coverage_handle): (Option<CoverageBundle>, Option<CoverageWriterHandle>) =
        if let Some(block) = opts.concerns.clone() {
            let catalog = flatten(&block)
                .map_err(|e| RunError::Coverage(format!("flatten coverage catalog: {e}")))?;
            let resolved_scope = opts.scope_name.as_deref().unwrap_or(&opts.agent_name);
            let target = target_id(&opts.workspace_path, resolved_scope);
            let paths = CoveragePaths::new(&opts.workspace_path, &target);
            paths
                .ensure_dir()
                .map_err(|e| RunError::Coverage(format!("ensure coverage dir: {e}")))?;
            write_snapshot(&catalog, &paths.catalog)
                .map_err(|e| RunError::Coverage(format!("write catalog snapshot: {e}")))?;
            // Capture a run manifest describing this run's defining inputs.
            // This is the single all-surfaces seam (workflow / agent /
            // autoflow / session all reach run_agent), so every run becomes
            // replay-describable. Failure to write the manifest must not
            // abort the run — log and continue.
            let surface = match opts.surface_tag.as_deref() {
                Some("workflow") => rupu_coverage::Surface::Workflow,
                Some("autoflow") => rupu_coverage::Surface::Autoflow,
                Some("session") => rupu_coverage::Surface::Session,
                _ => rupu_coverage::Surface::Agent,
            };
            let manifest = rupu_coverage::RunManifest {
                run_id: opts.run_id.clone(),
                started_at: chrono::Utc::now(),
                surface,
                agent_name: opts.agent_name.clone(),
                provider: opts.provider_name.clone(),
                model: opts.model.clone(),
                permission_mode: opts.mode_str.clone(),
                user_prompt: opts.user_message.clone(),
                concerns: block.clone(),
                scope_name: resolved_scope.to_string(),
                workspace_path: opts.workspace_path.clone(),
            };
            if let Err(e) = rupu_coverage::append_manifest(&paths, &manifest) {
                tracing::warn!(error = %e, "failed to write coverage run manifest");
            }
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
    // Conditional user-turn append. An EMPTY `user_message` means "seed-only":
    // the caller has supplied a complete, ready-to-send transcript via
    // `initial_messages` (e.g. the orchestrator resuming a tool-boundary pause,
    // where the seed already ends in a `tool_result` that pairs with the
    // preceding assistant `tool_use`). Appending a fresh user turn there would
    // either double the user turn or — worse — strand the assistant's
    // `tool_use` with no matching `tool_result`, which real Anthropic rejects
    // with a 400. So we only append when there is an actual message to add.
    // Every existing caller passes a non-empty `user_message` and is unaffected.
    if !opts.user_message.is_empty() {
        messages.push(Message::user(&opts.user_message));
    }
    let mut turn_idx: u32 = opts.turn_index_offset;
    let initial_turn_idx = turn_idx;
    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut runtime_mode = parse_mode_for_runtime(&opts.mode_str);
    // Clone the cooperative pause token so it can be awaited in `select!`
    // arms without borrowing `opts` (which the provider call borrows
    // mutably). Cheap: `CancellationToken` is `Arc`-backed.
    let pause = opts.pause.clone();

    // -----------------------------------------------------------------------
    // Inner fallible block.  Every `?` inside here propagates out to
    // `inner_result`; the unconditional teardown below then runs on BOTH
    // success and error paths, guaranteeing the coverage writer is always
    // flushed before we return.
    // -----------------------------------------------------------------------
    let inner_result: Result<RunResult, RunError> = async {
        let mut compaction_seq = 0u32;
        let loop_outcome = 'turns: loop {
            if turn_idx >= opts.max_turns {
                break 'turns LoopOutcome::Status(RunStatus::Error);
            }
            writer.write(&Event::TurnStart { turn_idx })?;
            let mut req = LlmRequest {
                model: opts.model.clone(),
                system: Some(opts.agent_system_prompt.clone()),
                messages: messages.clone(),
                max_tokens: opts.max_tokens,
                tools: tool_defs.clone(),
                cell_id: None,
                trace_id: None,
                thinking: opts.effort,
                context_window: opts.context_window,
                task_type: None,
                output_format: opts.output_format,
                output_schema: opts.output_schema.clone(),
                anthropic_task_budget: opts.anthropic_task_budget,
                anthropic_context_management: opts.anthropic_context_management,
                anthropic_speed: opts.anthropic_speed,
            };
            let mut trim_attempts = 0u32;
            let mut http_retries = 0u32;
            let call_outcome: CallOutcome = loop {
                let step: CallStep = if opts.no_stream {
                    // Race the one-shot completion against the pause token so a
                    // pause takes effect immediately rather than only after the
                    // provider returns.
                    tokio::select! {
                        r = opts.provider.send(&req) => match r {
                            Ok(x) => CallStep::Ok(x),
                            Err(e) => CallStep::Err(e),
                        },
                        _ = wait_pause(&pause) => CallStep::Paused,
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
                            // Plan 2: no live reasoning stream-to-stdout wiring yet.
                            StreamEvent::ReasoningDelta(_) => {}
                        }
                    };
                    // Race the stream against the pause token. On pause the
                    // stream future is dropped mid-flight and the partial
                    // assistant text streamed so far is discarded: the assistant
                    // message is committed to `messages` only after a *complete*
                    // response (below), so no dangling partial is ever persisted.
                    let streamed = tokio::select! {
                        r = opts.provider.stream(&req, &mut on_event) => Some(r),
                        _ = wait_pause(&pause) => None,
                    };
                    match streamed {
                        None => CallStep::Paused,
                        Some(result) => {
                            if result.is_ok() {
                                if !suppress {
                                    println!();
                                }
                                if let Some(err) = stream_transcript_error {
                                    return Err(RunError::Transcript(err));
                                }
                            }
                            match result {
                                Ok(x) => CallStep::Ok(x),
                                Err(e) => CallStep::Err(e),
                            }
                        }
                    }
                };
                match step {
                    CallStep::Ok(r) => break CallOutcome::Response(r),
                    CallStep::Paused => break CallOutcome::Paused,
                    CallStep::Err(e) => {
                        let e_str = e.to_string();
                        if is_context_overflow(&e_str) && trim_attempts <= 64 {
                            if trim_oldest_exchange(&mut req.messages) > 0 {
                                trim_attempts += 1;
                                writer.write(&Event::AssistantDelta {
                                    content: "[context trimmed to fit window; retrying]\n".into(),
                                })?;
                                writer.flush()?;
                                tracing::warn!(
                                    "context overflow – trimmed messages, attempt {trim_attempts}"
                                );
                                continue;
                            }
                            // Cannot trim further — surface as ContextOverflow.
                            writer.write(&Event::RunComplete {
                                run_id: opts.run_id.clone(),
                                status: RunStatus::Error,
                                total_tokens: total_in + total_out,
                                duration_ms: started.elapsed().as_millis() as u64,
                                error: Some(format!("context overflow: {e_str}")),
                            })?;
                            writer.flush()?;
                            return Err(RunError::ContextOverflow { turn: turn_idx });
                        }
                        // Transient provider errors (network/decode/SSE/5xx/
                        // rate-limit) are retried with backoff before the step
                        // is failed — a single dropped or malformed response
                        // shouldn't kill the run.
                        if is_retryable_provider_error(&e) && http_retries < MAX_HTTP_RETRIES {
                            http_retries += 1;
                            let backoff = retry_backoff(http_retries);
                            writer.write(&Event::AssistantDelta {
                                content: format!(
                                    "[transient provider error (retry {http_retries}/{MAX_HTTP_RETRIES}): {e_str}]\n"
                                ),
                            })?;
                            writer.flush()?;
                            tracing::warn!(
                                "transient provider error, retry {http_retries}/{MAX_HTTP_RETRIES}: {e_str}"
                            );
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                        writer.write(&Event::RunComplete {
                            run_id: opts.run_id.clone(),
                            status: RunStatus::Error,
                            total_tokens: total_in + total_out,
                            duration_ms: started.elapsed().as_millis() as u64,
                            error: Some(format!(
                                "provider: {e_str}{}",
                                if http_retries > 0 {
                                    format!(" (after {http_retries} retries)")
                                } else {
                                    String::new()
                                }
                            )),
                        })?;
                        writer.flush()?;
                        return Err(RunError::Provider(e_str));
                    }
                }
            };
            // A pause during the provider call stops the run at this safe
            // boundary. The partial response is dropped: no assistant message
            // was committed for this turn, so the transcript ends at the last
            // complete message and is ready to seed a resume.
            let resp: LlmResponse = match call_outcome {
                CallOutcome::Response(r) => r,
                CallOutcome::Paused => break 'turns LoopOutcome::Paused,
            };
            total_in += resp.usage.input_tokens as u64;
            total_out += resp.usage.output_tokens as u64;
            writer.write(&Event::Usage {
                provider: opts.provider_name.clone(),
                model: opts.model.clone(), // requested model (meaningful attribution; priced)
                served_model: {
                    let s = resp.model.trim();
                    if !s.is_empty() && s != opts.model {
                        Some(s.to_string())
                    } else {
                        None
                    }
                },
                input_tokens: resp.usage.input_tokens,
                output_tokens: resp.usage.output_tokens,
                cached_tokens: resp.usage.cached_tokens,
            })?;

            // Proactive context compaction: if the previous turn's input exceeded
            // the configured threshold, summarise older turns before building the
            // next request. Must run after usage accounting.
            if let Some(threshold) =
                effective_compact_threshold(opts.context_window_tokens, opts.compact_at_percent)
            {
                if resp.usage.input_tokens as u64 > threshold {
                    compaction_seq += 1;
                    let run_id_clone = opts.run_id.clone();
                    let last_input_tokens = resp.usage.input_tokens;
                    let _ = compact_context(
                        &mut messages,
                        &mut opts,
                        &run_id_clone,
                        compaction_seq,
                        &mut writer,
                        last_input_tokens,
                    )
                    .await;
                }
            }

            // Emit any text content as assistant_message events; collect
            // tool_use blocks for dispatch.
            let mut tool_uses: Vec<(String, String, serde_json::Value)> = Vec::new();
            // Reasoning precedes text in an assistant turn, so compute it up
            // front and attach it to the turn's first assistant message.
            let mut turn_thinking = resp.reasoning_text();
            for block in &resp.content {
                match block {
                    ContentBlock::Text { text } => {
                        writer.write(&Event::AssistantMessage {
                            content: text.clone(),
                            thinking: turn_thinking.take(),
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
                    ContentBlock::Reasoning { .. } => {
                        // Readable text is consumed via `turn_thinking` above,
                        // which rides on the turn's first assistant message.
                    }
                    ContentBlock::Unknown => {}
                }
            }

            // A tool-only turn has no Text block to hang the reasoning on, but
            // the reasoning is still worth recording — that is the turn where
            // the model decided which tool to call.
            if let Some(thinking) = turn_thinking.take() {
                writer.write(&Event::AssistantMessage {
                    content: String::new(),
                    thinking: Some(thinking),
                })?;
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
                            structured: None,
                        })?;
                        tool_results.push((
                            call_id,
                            String::new(),
                            Some("permission_denied".into()),
                        ));
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
                            structured: None,
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
                            structured: out.structured.clone(),
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
                            structured: None,
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
            // Whether the model requested any tool calls this turn. This — not
            // `stop_reason` — is the real "should I continue?" signal: tool
            // calls produce tool_results that we append below as a user
            // message, so the next request ends with a user turn.
            let made_tool_calls = !tool_results.is_empty();
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
            // Continue only when the model requested tools (we appended their
            // results as a user message, so the next request ends with a user
            // turn). With no tool calls the assistant's message is the final
            // answer — terminate. Continuing would re-send a conversation
            // ending in an assistant message, which models without prefill
            // support reject ("the conversation must end with a user message"),
            // and which is the wrong behaviour in general. `stop_reason` is
            // only a hint: an unrecognized/absent value deserializes to `None`,
            // so it must not be the sole terminator.
            if !made_tool_calls {
                break 'turns LoopOutcome::Status(RunStatus::Ok);
            }
            // Cooperative pause after a full tool-calling turn: the tool(s) ran
            // to completion and their results are recorded in both the
            // transcript and `messages` (no dangling tool_call). Stop here,
            // before issuing the next turn's provider request.
            if is_paused(&pause) {
                break 'turns LoopOutcome::Paused;
            }
        };

        let (result_status, paused) = match loop_outcome {
            LoopOutcome::Status(s) => (s, false),
            LoopOutcome::Paused => (RunStatus::Aborted, true),
        };

        writer.write(&Event::RunComplete {
            run_id: opts.run_id.clone(),
            status: result_status,
            total_tokens: total_in + total_out,
            duration_ms: started.elapsed().as_millis() as u64,
            error: if paused { Some("paused".into()) } else { None },
        })?;
        writer.flush()?;

        // Drop the MCP transport so the server's recv() returns None and exits.
        // Then await its JoinHandle for deterministic shutdown.
        if let Some((transport, handle)) = mcp_guard {
            drop(transport);
            let _ = handle.join.await;
        }

        Ok(RunResult {
            status: result_status,
            turns: turn_idx.saturating_sub(initial_turn_idx),
            total_tokens_in: total_in,
            total_tokens_out: total_out,
            final_messages: messages,
            paused,
        })
    }
    .await;

    // -----------------------------------------------------------------------
    // Unconditional coverage-writer teardown — runs on BOTH success and error.
    //
    // Drop the ToolContext's Arc<CoverageWriter> clone BEFORE calling shutdown
    // so the writer task's mpsc channel closes cleanly.  If ToolContext still
    // holds a clone when shutdown() awaits the JoinHandle, the task's recv()
    // never sees EOF and the await hangs indefinitely.
    // -----------------------------------------------------------------------
    opts.tool_context.coverage_writer = None;
    if let Some(h) = coverage_handle {
        h.shutdown().await;
    }

    inner_result
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
            output_schema: None,
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
            max_tokens: DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause: None,
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

    #[tokio::test]
    async fn final_answer_without_tool_calls_terminates_regardless_of_stop_reason() {
        // Regression: a final assistant turn with NO tool calls must end the
        // run. The loop previously broke only on `Some(EndTurn)`; a non-EndTurn
        // stop reason (e.g. `MaxTokens`, or an unrecognized value that
        // deserializes to `None`) made it loop again and re-send a conversation
        // ending in an assistant message — which some models reject with a 400:
        // "the conversation must end with a user message" (no prefill support).
        //
        // The mock has a SINGLE scripted turn, so if the loop wrongly continues
        // it exhausts the script on the next turn and `run_agent` errors. A
        // clean `Ok` in exactly one turn proves it terminated correctly.
        let tmp_dir = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp_dir.path().join("run_test.jsonl");

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "All done — final answer, no tools to call.".into(),
            stop: StopReason::MaxTokens,
            input_tokens: 1,
            output_tokens: 1,
        }]);

        let opts = AgentRunOpts {
            agent_name: "test-agent".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id: "run_terminate_no_tools".into(),
            workspace_id: "ws_test".into(),
            workspace_path: tmp_dir.path().to_path_buf(),
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext {
                workspace_path: tmp_dir.path().to_path_buf(),
                ..Default::default()
            },
            user_message: "do the thing".into(),
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: true,
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
            step_id: "s1".into(),
            on_tool_call: None,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause: None,
        };

        let result = run_agent(opts)
            .await
            .expect("run should complete cleanly, not loop into an exhausted script");
        assert_eq!(
            result.status,
            RunStatus::Ok,
            "a no-tool-call final turn must complete Ok"
        );
        assert_eq!(
            result.turns, 1,
            "must terminate after the single turn, not continue looping"
        );
    }

    #[tokio::test]
    async fn usage_records_requested_model_and_served_model_separately() {
        // The provider echoes a served id ("mock-1") that differs from the
        // requested model. The Usage event must attribute spend to the
        // *requested* model and stash the served id in `served_model`.
        let tmp_dir = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp_dir.path().join("run_usage_model.jsonl");

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 7,
            output_tokens: 3,
        }]);

        let opts = AgentRunOpts {
            agent_name: "test-agent".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            run_id: "run_usage_model".into(),
            workspace_id: "ws_test".into(),
            workspace_path: tmp_dir.path().to_path_buf(),
            transcript_path: transcript_path.clone(),
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext {
                workspace_path: tmp_dir.path().to_path_buf(),
                ..Default::default()
            },
            user_message: "do the thing".into(),
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: true,
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
            step_id: "s1".into(),
            on_tool_call: None,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause: None,
        };

        run_agent(opts).await.expect("agent run succeeds");

        let body = std::fs::read_to_string(&transcript_path).expect("read transcript");
        let usage = body
            .lines()
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .find_map(|ev| match ev {
                Event::Usage {
                    model,
                    served_model,
                    ..
                } => Some((model, served_model)),
                _ => None,
            })
            .expect("transcript has a Usage event");
        assert_eq!(usage.0, "claude-opus-4-8", "model is the requested model");
        assert_eq!(
            usage.1.as_deref(),
            Some("mock-1"),
            "served_model is the provider-echoed id when it differs"
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

#[cfg(test)]
mod retry_tests {
    use super::{is_retryable_provider_error, retry_backoff, MAX_HTTP_RETRIES};
    use rupu_providers::ProviderError as E;

    #[test]
    fn transient_errors_are_retried() {
        assert!(is_retryable_provider_error(&E::Http(
            "error decoding response body".into()
        )));
        assert!(is_retryable_provider_error(&E::UnexpectedEndOfStream));
        assert!(is_retryable_provider_error(&E::SseParse(
            "bad chunk".into()
        )));
        assert!(is_retryable_provider_error(&E::Json("truncated".into())));
        assert!(is_retryable_provider_error(&E::Api {
            status: 503,
            message: "unavailable".into()
        }));
        assert!(is_retryable_provider_error(&E::Api {
            status: 429,
            message: "slow down".into()
        }));
    }

    #[test]
    fn config_and_client_errors_fail_fast() {
        // The oracle "unknown provider" case is AuthConfig — must NOT spin.
        assert!(!is_retryable_provider_error(&E::AuthConfig(
            "oracle: unknown provider: oracle".into()
        )));
        assert!(!is_retryable_provider_error(&E::BadRequest {
            message: "max_tokens too large".into()
        }));
        assert!(!is_retryable_provider_error(&E::Api {
            status: 400,
            message: "bad request".into()
        }));
        assert!(!is_retryable_provider_error(&E::Api {
            status: 404,
            message: "no model".into()
        }));
    }

    #[test]
    fn backoff_is_capped_exponential() {
        assert_eq!(retry_backoff(1).as_millis(), 250);
        assert_eq!(retry_backoff(2).as_millis(), 500);
        assert_eq!(retry_backoff(6).as_millis(), 8000);
        // Capped: all further attempts stay at 8s.
        assert_eq!(retry_backoff(MAX_HTTP_RETRIES).as_millis(), 8000);
    }
}

#[cfg(test)]
mod context_trim_tests {
    use super::{is_context_overflow, trim_oldest_exchange};
    use rupu_providers::types::{ContentBlock, Message, Role};

    fn user_msg(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant_msg(text: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    #[test]
    fn trim_drops_indices_1_and_2_from_5_message_list() {
        // [user0, assistant1, user2, assistant3, user4]
        // After trim: [user0, assistant3, user4]
        let mut msgs = vec![
            user_msg("anchor"),
            assistant_msg("first assistant"),
            user_msg("first tool result"),
            assistant_msg("second assistant"),
            user_msg("second tool result"),
        ];
        let dropped = trim_oldest_exchange(&mut msgs);
        assert_eq!(dropped, 2, "should drop 2 messages");
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].role, Role::User);
        // msgs[0] is still the anchor
        assert_eq!(msgs[1].role, Role::Assistant);
        assert_eq!(msgs[2].role, Role::User);
        // Verify the right messages remain
        match &msgs[1].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "second assistant"),
            _ => panic!("expected text block"),
        }
    }

    #[test]
    fn trim_returns_0_for_list_shorter_than_4() {
        // 3-message list: nothing safe to drop
        let mut msgs = vec![
            user_msg("anchor"),
            assistant_msg("assistant"),
            user_msg("user"),
        ];
        assert_eq!(trim_oldest_exchange(&mut msgs), 0);
        assert_eq!(msgs.len(), 3, "list must be unchanged");
    }

    #[test]
    fn trim_returns_0_for_2_message_list() {
        let mut msgs = vec![user_msg("hi"), assistant_msg("hello")];
        assert_eq!(trim_oldest_exchange(&mut msgs), 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn repeated_trim_shrinks_until_0() {
        // 7-message list: [u, a, u, a, u, a, u]
        // trim 1: [u, a, u, a, u] (dropped indices 1,2)
        // trim 2: [u, a, u]       (dropped indices 1,2)
        // trim 3: returns 0 (len < 4)
        let mut msgs = vec![
            user_msg("u0"),
            assistant_msg("a1"),
            user_msg("u2"),
            assistant_msg("a3"),
            user_msg("u4"),
            assistant_msg("a5"),
            user_msg("u6"),
        ];
        assert_eq!(trim_oldest_exchange(&mut msgs), 2);
        assert_eq!(msgs.len(), 5);
        assert_eq!(trim_oldest_exchange(&mut msgs), 2);
        assert_eq!(msgs.len(), 3);
        assert_eq!(trim_oldest_exchange(&mut msgs), 0);
        assert_eq!(msgs.len(), 3, "no further shrinkage when len < 4");
    }

    #[test]
    fn is_context_overflow_matches_known_phrases() {
        assert!(is_context_overflow("prompt is too long for the model"));
        assert!(is_context_overflow("too many tokens in request"));
        assert!(is_context_overflow("exceeds context window limit"));
        assert!(is_context_overflow("PROMPT IS TOO LONG")); // case-insensitive
    }

    #[test]
    fn is_context_overflow_does_not_match_unrelated_errors() {
        assert!(!is_context_overflow("network error"));
        assert!(!is_context_overflow("invalid api key"));
        assert!(!is_context_overflow("rate limited"));
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
    /// Replay an arbitrary block sequence verbatim. Use when a turn's exact
    /// block shape matters — e.g. `Reasoning` before `Text`, or several `Text`
    /// blocks in one turn — which the higher-level variants can't express.
    AssistantBlocks {
        content: Vec<ContentBlock>,
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
            ScriptedTurn::AssistantBlocks { content, stop } => Ok(LlmResponse {
                id: "mock".to_string(),
                model: "mock-1".to_string(),
                content,
                stop_reason: Some(stop),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
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

#[cfg(test)]
mod compaction_tests {
    use super::*;
    use rupu_providers::types::{ContentBlock, Message, Role};

    fn text_msg(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        }
    }

    fn tool_use_msg() -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "/foo"}),
            }],
        }
    }

    fn tool_result_msg() -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "file content here".to_string(),
                is_error: false,
            }],
        }
    }

    #[test]
    fn estimate_tokens_monotonic_and_approx_chars_over_4() {
        let msgs_short = vec![text_msg(Role::User, "hello")];
        let msgs_long = vec![text_msg(
            Role::User,
            "hello world this is a longer message that has many chars",
        )];
        let short = estimate_tokens(&msgs_short);
        let long = estimate_tokens(&msgs_long);
        assert!(long > short, "longer message should estimate more tokens");
        // "hello" → 5/4 = 1
        assert_eq!(short, 1);
        // 56 chars / 4 = 14
        assert_eq!(long, 14);
    }

    #[test]
    fn effective_compact_threshold_none_when_window_none() {
        assert_eq!(effective_compact_threshold(None, None), None);
        assert_eq!(effective_compact_threshold(None, Some(75)), None);
    }

    #[test]
    fn effective_compact_threshold_1m_at_75_pct() {
        assert_eq!(
            effective_compact_threshold(Some(1_000_000), Some(75)),
            Some(750_000)
        );
    }

    #[test]
    fn effective_compact_threshold_default_80_when_percent_none() {
        assert_eq!(
            effective_compact_threshold(Some(1_000_000), None),
            Some(800_000)
        );
    }

    #[test]
    fn effective_compact_threshold_clamps_high_to_95() {
        assert_eq!(
            effective_compact_threshold(Some(1_000_000), Some(99)),
            Some(950_000)
        );
    }

    #[test]
    fn effective_compact_threshold_clamps_low_to_10() {
        assert_eq!(
            effective_compact_threshold(Some(1_000_000), Some(5)),
            Some(100_000)
        );
    }

    #[test]
    fn partition_keeps_last_2_and_returns_middle() {
        // 10-message alternating convo
        let mut msgs = vec![text_msg(Role::User, "task")];
        for i in 0..4 {
            msgs.push(text_msg(Role::Assistant, &format!("assistant {i}")));
            msgs.push(text_msg(Role::User, &format!("user {i}")));
        }
        // msgs has 9 messages: [task, a0, u0, a1, u1, a2, u2, a3, u3]
        // With very small char budget (4 chars), we still keep at least 2.
        let result = partition_for_compaction(&msgs, 4);
        assert!(result.is_some(), "should partition a 9-message convo");
        let (middle_start, recent_start) = result.unwrap();
        assert_eq!(middle_start, 1);
        assert!(
            recent_start >= msgs.len() - 2,
            "recent should include at least last 2"
        );
        assert!(recent_start > middle_start, "middle must be non-empty");
    }

    #[test]
    fn partition_tiny_convo_returns_none() {
        // 2-message convo: [task, assistant] — nothing to summarise
        let msgs = vec![
            text_msg(Role::User, "task"),
            text_msg(Role::Assistant, "done"),
        ];
        // Budget in chars: 4000 chars (well above the tiny convo)
        assert_eq!(partition_for_compaction(&msgs, 4000), None);
    }

    #[test]
    fn partition_does_not_split_tool_use_tool_result_pair() {
        // [task, assistant_text, assistant_tool_use, user_tool_result, assistant_final]
        let msgs = vec![
            text_msg(Role::User, "task"),
            text_msg(Role::Assistant, "step 1"),
            tool_use_msg(),
            tool_result_msg(),
            text_msg(Role::Assistant, "step 2"),
        ];
        // With a very small char budget (4), recent_start might initially land on the tool_result msg.
        // The pair-protection should move it back to include the tool_use assistant msg.
        let result = partition_for_compaction(&msgs, 4);
        if let Some((_, recent_start)) = result {
            // If recent_start points into the tool_use/tool_result pair,
            // it must start at the assistant tool_use, not the user tool_result.
            if recent_start < msgs.len() {
                assert!(
                    !(msgs[recent_start].role == Role::User
                        && msgs[recent_start]
                            .content
                            .iter()
                            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
                        && recent_start > 0
                        && msgs[recent_start - 1].role == Role::Assistant
                        && msgs[recent_start - 1]
                            .content
                            .iter()
                            .any(|b| matches!(b, ContentBlock::ToolUse { .. }))),
                    "recent_start must not split a tool_use/tool_result pair"
                );
            }
        }
    }

    /// Regression test for the dense-content compaction bug.
    ///
    /// Before the fix, `partition_for_compaction` used `estimate_tokens` (chars/4)
    /// against a token budget (`context_window_tokens / 2`). For code/JSON-heavy
    /// conversations the chars/4 estimate undercounts real tokens ~2x, so a history
    /// that is really ~990k tokens looked like ~520k estimated tokens — within the
    /// ~500k budget — and the entire history was treated as "recent", leaving nothing
    /// to summarize. This test asserts that a char budget well below the total chars
    /// of a dense conversation correctly identifies a non-empty middle section.
    #[test]
    fn partition_budget_in_chars_reclaims_middle_for_dense_content() {
        // Build 8 messages with large char payloads (simulating JSON/code content).
        let dense_chunk = "x".repeat(50_000); // 50k chars per message
        let mut msgs = vec![text_msg(Role::User, "task")];
        for i in 0..3 {
            msgs.push(text_msg(
                Role::Assistant,
                &format!("assistant {i}: {dense_chunk}"),
            ));
            msgs.push(text_msg(Role::User, &format!("user {i}: {dense_chunk}")));
        }
        msgs.push(text_msg(
            Role::Assistant,
            &format!("final assistant: {dense_chunk}"),
        ));
        // Total: 1 task + 3 assistant + 3 user + 1 assistant = 8 messages
        // Each of the 7 non-task messages has ~50k+ chars → ~350k total chars.

        // recent_budget_chars = 60k chars → only fits 1 message (~50k chars),
        // meaning recent_start > 1 and there IS a non-empty middle to summarize.
        let recent_budget_chars = 60_000usize;
        let result = partition_for_compaction(&msgs, recent_budget_chars);

        assert!(
            result.is_some(),
            "dense conversation with small char budget must find a middle section"
        );
        let (middle_start, recent_start) = result.unwrap();
        assert_eq!(middle_start, 1, "task is always at index 0");
        assert!(
            recent_start > 1,
            "recent_start must be > 1 so there is something in the middle to summarize; got {recent_start}"
        );

        // Verify the middle is non-empty.
        assert!(
            recent_start > middle_start,
            "middle (messages[{middle_start}..{recent_start}]) must be non-empty"
        );
    }

    /// Calibration sanity check: when the provider reports ~2× the chars/4 estimate
    /// (as happens with dense code), the derived recent_budget_chars should be
    /// roughly half what a naive chars/4==tokens assumption would give.
    #[test]
    fn calibration_halves_budget_when_tokens_are_2x_char_estimate() {
        // Construct a scenario: 400k chars of messages, chars/4 estimate = 100k tokens.
        // Provider actually reports 200k tokens (2× the estimate).
        let total_chars: usize = 400_000;
        let last_input_tokens: u32 = 200_000;

        // Compute tokens_per_char (mirroring compact_context logic).
        let tokens_per_char = (last_input_tokens as f64 / total_chars.max(1) as f64).max(0.25_f64);

        // Threshold at 75% of 1M window = 750k tokens; target recent = half = 375k tokens.
        let threshold: u64 = 750_000;
        let target_recent_tokens = (threshold / 2) as f64;
        let recent_budget_chars = (target_recent_tokens / tokens_per_char) as usize;

        // Naive chars/4 path: recent_budget_tokens = window/2 = 500k tokens.
        // In chars that would be 500k * 4 = 2_000_000 chars.
        // Calibrated path with 2x real density: recent_budget_chars ≈ 375k / 0.5 = 750_000 chars.
        // So calibrated is ~750k vs naive ~2M — roughly 2.7× smaller.
        // The key property: calibrated_budget < naive_budget by a significant factor.
        let naive_budget_chars: usize = 2_000_000;
        assert!(
            recent_budget_chars < naive_budget_chars / 2,
            "calibrated char budget ({recent_budget_chars}) should be much smaller than naive budget ({naive_budget_chars}) when real tokens are 2x the estimate"
        );
    }

    #[test]
    fn rebuild_compacted_starts_with_user_and_contains_summary() {
        let recent = vec![
            text_msg(Role::Assistant, "recent assistant turn"),
            text_msg(Role::User, "recent user turn"),
        ];
        let result = rebuild_compacted("do the task", "summary here", recent.clone());
        assert!(!result.is_empty());
        assert_eq!(result[0].role, Role::User, "first message must be user");
        let first_text = match &result[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text block"),
        };
        assert!(
            first_text.contains("do the task"),
            "task text must be in first message"
        );
        assert!(
            first_text.contains("summary here"),
            "summary must be in first message"
        );
        // Recent messages are preserved after the task/summary message.
        assert_eq!(result.len(), 1 + recent.len());
        assert_eq!(result[1].role, Role::Assistant);
    }

    #[tokio::test]
    async fn compact_context_returns_false_on_provider_error_and_leaves_messages_unchanged() {
        let tmp_dir = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp_dir.path().join("run_compaction_test.jsonl");

        // Provider that errors on every call.
        let provider = MockProvider::new(vec![ScriptedTurn::ProviderError(
            "summary call failed".to_string(),
        )]);

        let mut opts = AgentRunOpts {
            agent_name: "test".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider: Box::new(provider),
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id: "run_compact_test".into(),
            workspace_id: "ws_test".into(),
            workspace_path: tmp_dir.path().to_path_buf(),
            transcript_path: transcript_path.clone(),
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext {
                workspace_path: tmp_dir.path().to_path_buf(),
                ..Default::default()
            },
            user_message: "task".into(),
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: true,
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
            step_id: "s1".into(),
            on_tool_call: None,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: Some(1_000_000),
            compact_at_percent: Some(75),
            pause: None,
        };

        let mut messages = vec![
            Message::user("task"),
            Message::assistant("step 1"),
            Message::user("result 1"),
            Message::assistant("step 2"),
        ];
        let original_len = messages.len();

        let mut writer = JsonlWriter::create(&transcript_path).expect("writer");
        // Pass a realistic last_input_tokens (simulating 800k real tokens for
        // the 4-message test conversation).
        let result = compact_context(
            &mut messages,
            &mut opts,
            "run_compact_test",
            1,
            &mut writer,
            800_000,
        )
        .await;

        assert!(
            !result,
            "compact_context must return false on provider error"
        );
        assert_eq!(
            messages.len(),
            original_len,
            "messages must be unchanged after error"
        );
    }

    #[tokio::test]
    async fn compact_messages_returns_outcome_with_mock_provider() {
        // Build a dense message list that will exceed compaction threshold.
        // Use a tiny window (1000 tokens) and many messages so partition finds a middle.
        let dense_chunk = "x".repeat(1000); // 1000 chars per message
        let mut msgs = vec![text_msg(Role::User, &format!("task: {dense_chunk}"))];
        for i in 0..5 {
            msgs.push(text_msg(
                Role::Assistant,
                &format!("assistant {i}: {dense_chunk}"),
            ));
            msgs.push(text_msg(Role::User, &format!("user {i}: {dense_chunk}")));
        }
        // 11 messages total, each ~1k chars

        let mut provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "Summary of prior work.".to_string(),
            stop: StopReason::EndTurn,
            input_tokens: 500,
            output_tokens: 10,
        }]);

        // With a 1000-token window and ~11k chars at high token density, compaction
        // should find a non-empty middle.
        let result = compact_messages(
            &msgs,
            &mut provider,
            "mock-1",
            1000,
            Some(80),
            900, // simulate near-threshold input tokens
        )
        .await;

        let outcome = result.expect("no provider error").expect("should compact");
        assert_eq!(
            outcome.messages[0].role,
            Role::User,
            "first message must be user"
        );
        // The combined task+summary message must contain the summary text.
        let first_text = match &outcome.messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => panic!("expected text block"),
        };
        assert!(
            first_text.contains("Summary of prior work"),
            "summary text must appear in first message"
        );
        assert!(
            outcome.summarized_messages > 0,
            "summarized_messages must be > 0"
        );
    }

    #[tokio::test]
    async fn compact_messages_sets_thinking_on_summary_request() {
        // The compaction summary request carries history that may contain
        // `ContentBlock::Reasoning` blocks, which get echoed back onto the
        // wire as `thinking` blocks by the Anthropic provider. Anthropic
        // requires a `thinking` config whenever thinking blocks appear in
        // assistant history, or the request 400s (this used to be `None`,
        // which was fine only on the OAuth path, where the provider injects
        // adaptive thinking regardless of the request's `thinking` field).
        let dense_chunk = "x".repeat(1000);
        let mut msgs = vec![text_msg(Role::User, &format!("task: {dense_chunk}"))];
        for i in 0..5 {
            msgs.push(text_msg(
                Role::Assistant,
                &format!("assistant {i}: {dense_chunk}"),
            ));
            msgs.push(text_msg(Role::User, &format!("user {i}: {dense_chunk}")));
        }

        let mut provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "Summary of prior work.".to_string(),
            stop: StopReason::EndTurn,
            input_tokens: 500,
            output_tokens: 10,
        }]);

        let result = compact_messages(&msgs, &mut provider, "mock-1", 1000, Some(80), 900).await;
        result.expect("no provider error").expect("should compact");

        let captured = provider.captured_requests();
        assert_eq!(captured.len(), 1, "expected exactly one summary request");
        assert_eq!(
            captured[0].thinking,
            Some(rupu_providers::model_tier::ThinkingLevel::Auto),
            "compaction summary request must set thinking so echoed reasoning \
             blocks in its history don't 400 on the api-key auth path"
        );
    }

    #[tokio::test]
    async fn compact_messages_returns_none_for_tiny_history() {
        let msgs = vec![
            text_msg(Role::User, "task"),
            text_msg(Role::Assistant, "done"),
        ];

        let mut provider = MockProvider::new(vec![]);

        let result = compact_messages(&msgs, &mut provider, "mock-1", 1_000_000, None, 100).await;

        assert!(
            result.expect("no error").is_none(),
            "2-message history should return None"
        );
    }
}

// ---------------------------------------------------------------------------
// Cooperative pause tests (T2). Exercise the pause token at both safe
// boundaries: mid-stream (drop the partial) and after a running tool finishes.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod pause_tests {
    use super::*;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    /// Provider whose `stream` emits one partial delta then blocks for a long
    /// time. The pause `select!` cancels the in-flight stream, so the full
    /// response is never produced and no assistant message is committed.
    struct SlowStreamProvider;

    #[async_trait]
    impl LlmProvider for SlowStreamProvider {
        async fn send(
            &mut self,
            _req: &LlmRequest,
        ) -> Result<LlmResponse, rupu_providers::ProviderError> {
            Ok(LlmResponse {
                id: "slow".into(),
                model: "mock-1".into(),
                content: vec![ContentBlock::Text {
                    text: "unused".into(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
            })
        }

        async fn stream(
            &mut self,
            _req: &LlmRequest,
            on_event: &mut (dyn FnMut(StreamEvent) + Send),
        ) -> Result<LlmResponse, rupu_providers::ProviderError> {
            // Simulate the model emitting some text, then a long generation
            // that the pause `select!` will cancel before it completes.
            on_event(StreamEvent::TextDelta(
                "partial answer that must be dropped".into(),
            ));
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(LlmResponse {
                id: "slow".into(),
                model: "mock-1".into(),
                content: vec![ContentBlock::Text {
                    text: "the full answer".into(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
            })
        }

        fn default_model(&self) -> &str {
            "mock-1"
        }

        fn provider_id(&self) -> rupu_providers::ProviderId {
            rupu_providers::ProviderId::Anthropic
        }
    }

    fn opts_for(
        provider: Box<dyn LlmProvider>,
        pause: Option<CancellationToken>,
        workspace: &std::path::Path,
        transcript_path: PathBuf,
        initial_messages: Vec<Message>,
        user_message: &str,
    ) -> AgentRunOpts {
        AgentRunOpts {
            agent_name: "test-agent".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider,
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id: "run_pause_test".into(),
            workspace_id: "ws_test".into(),
            workspace_path: workspace.to_path_buf(),
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext {
                workspace_path: workspace.to_path_buf(),
                ..Default::default()
            },
            user_message: user_message.into(),
            initial_messages,
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            // Exercise the streaming path — that is what pause races against.
            no_stream: false,
            suppress_stream_stdout: true,
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
            step_id: "s1".into(),
            on_tool_call: None,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause,
        }
    }

    #[tokio::test]
    async fn pause_during_stream_stops_and_drops_partial_text() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let token = CancellationToken::new();
        let token2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token2.cancel();
        });

        let opts = opts_for(
            Box::new(SlowStreamProvider),
            Some(token),
            tmp.path(),
            transcript_path.clone(),
            Vec::new(),
            "hi",
        );
        let result = run_agent(opts).await.expect("paused run returns Ok result");

        assert!(result.paused, "run must report a paused outcome");
        // Only the seed user message survives — no partial assistant message.
        assert_eq!(
            result.final_messages.len(),
            1,
            "no assistant message should be committed on a paused stream"
        );
        assert_eq!(result.final_messages[0].role, Role::User);

        // The transcript must not carry a committed AssistantMessage; the
        // streamed deltas are UI-only and the message is never finalized.
        let body = std::fs::read_to_string(&transcript_path).expect("read transcript");
        let has_assistant_msg = body
            .lines()
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .any(|e| matches!(e, Event::AssistantMessage { .. }));
        assert!(
            !has_assistant_msg,
            "a paused partial stream must not persist an assistant message"
        );
    }

    #[tokio::test]
    async fn pause_during_tool_lets_it_finish_and_records_result() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let token = CancellationToken::new();
        let token2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token2.cancel();
        });

        // Turn 0 asks for a bash tool that sleeps; pause fires while it runs.
        // Turn 1 would be a final answer but must NOT be reached.
        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantToolUse {
                text: None,
                tool_id: "call_sleep".into(),
                tool_name: "bash".into(),
                tool_input: serde_json::json!({ "command": "sleep 0.3" }),
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "should not be reached".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]);

        let opts = opts_for(
            Box::new(provider),
            Some(token),
            tmp.path(),
            transcript_path.clone(),
            Vec::new(),
            "run the tool",
        );
        let result = run_agent(opts).await.expect("paused run returns Ok result");

        assert!(result.paused, "run must report a paused outcome");

        let events: Vec<Event> = std::fs::read_to_string(&transcript_path)
            .expect("read transcript")
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let tool_calls = events
            .iter()
            .filter(|e| matches!(e, Event::ToolCall { .. }))
            .count();
        let tool_results = events
            .iter()
            .filter(|e| matches!(e, Event::ToolResult { .. }))
            .count();
        assert_eq!(tool_calls, 1, "exactly one tool call was issued");
        assert_eq!(
            tool_results, 1,
            "the mid-flight tool must finish and record its result before pausing"
        );

        // The tool_result is appended to the message history (no dangling
        // tool_call without a matching result).
        let has_tool_result = result.final_messages.iter().any(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        });
        assert!(
            has_tool_result,
            "the recorded tool result must be present in the persisted messages"
        );
    }

    #[tokio::test]
    async fn resume_continues_from_transcript() {
        let tmp = tempfile::tempdir().expect("tmpdir");

        // Phase 1: pause during a running tool to produce a persisted transcript.
        let t1 = tmp.path().join("run1.jsonl");
        let token = CancellationToken::new();
        let token2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token2.cancel();
        });
        let provider1 = MockProvider::new(vec![ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "call_sleep".into(),
            tool_name: "bash".into(),
            tool_input: serde_json::json!({ "command": "sleep 0.3" }),
            stop: StopReason::ToolUse,
        }]);
        let opts1 = opts_for(
            Box::new(provider1),
            Some(token),
            tmp.path(),
            t1,
            Vec::new(),
            "start",
        );
        let paused = run_agent(opts1).await.expect("first run pauses");
        assert!(paused.paused, "first run must be paused");
        let seed = paused.final_messages;

        // Phase 2: resume, seeded with the persisted transcript. A fresh
        // provider request must be issued and the run must complete.
        let t2 = tmp.path().join("run2.jsonl");
        let provider2 = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "final answer".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let captured = provider2.captured.clone();
        let opts2 = opts_for(Box::new(provider2), None, tmp.path(), t2, seed, "continue");
        let result = run_agent(opts2).await.expect("resume completes");

        assert_eq!(result.status, RunStatus::Ok, "resumed run completes Ok");
        assert!(!result.paused, "resumed run is not paused");
        assert_eq!(
            captured.lock().unwrap().len(),
            1,
            "resume must issue exactly one fresh provider request"
        );
    }

    #[tokio::test]
    async fn empty_user_message_seeds_transcript_without_appending() {
        // Seed-only contract: when `user_message` is empty, `run_agent` must
        // NOT append a fresh user turn — the caller (the orchestrator resuming a
        // tool-boundary pause) has supplied a complete, ready-to-send seed via
        // `initial_messages`. The outbound request messages must equal the seed
        // exactly. A tool-boundary seed ends in a `tool_result` paired with the
        // preceding assistant `tool_use`; an extra user turn (or, worse, a
        // flattened tool_result) would strand that tool_use and trigger a
        // provider 400.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let seed = vec![
            Message::user("do work"),
            Message {
                role: rupu_providers::types::Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "read_file".into(),
                    input: serde_json::json!({ "path": "x" }),
                }],
            },
            Message::tool_result("toolu_1", "contents", false),
        ];

        let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "final".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let captured = provider.captured.clone();
        let opts = opts_for(
            Box::new(provider),
            None,
            tmp.path(),
            transcript_path,
            seed.clone(),
            "", // empty → seed-only, no appended user turn
        );
        let result = run_agent(opts).await.expect("run completes");
        assert_eq!(result.status, RunStatus::Ok);

        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 1, "exactly one fresh provider request");
        let msgs = &reqs[0].messages;
        assert_eq!(
            msgs.len(),
            seed.len(),
            "request messages must equal the seed (no appended user turn)"
        );
        // Roles match position-for-position and the trailing tool_result is
        // preserved as a ToolResult block (not flattened / not doubled).
        for (got, want) in msgs.iter().zip(seed.iter()) {
            assert_eq!(got.role, want.role);
        }
        let last_is_tool_result = msgs
            .last()
            .unwrap()
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "toolu_1"));
        assert!(
            last_is_tool_result,
            "trailing tool_result must survive intact, got {:?}",
            msgs.last().unwrap().content
        );
    }

    #[tokio::test]
    async fn no_pause_token_behaves_exactly_as_today() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "done".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let opts = opts_for(
            Box::new(provider),
            None,
            tmp.path(),
            transcript_path,
            Vec::new(),
            "hi",
        );
        let result = run_agent(opts).await.expect("run completes");

        assert_eq!(result.status, RunStatus::Ok);
        assert!(!result.paused, "a None pause token never pauses");
        assert_eq!(result.turns, 1);
    }
}

#[cfg(test)]
mod reasoning_tests {
    use super::*;

    fn reasoning(text: &str) -> ContentBlock {
        ContentBlock::Reasoning {
            text: Some(text.to_string()),
            provider: "anthropic".into(),
            model: "mock-1".into(),
            raw: serde_json::json!({ "type": "thinking", "thinking": text }),
        }
    }

    fn opts_for(
        provider: Box<dyn LlmProvider>,
        workspace: &std::path::Path,
        transcript_path: PathBuf,
    ) -> AgentRunOpts {
        AgentRunOpts {
            agent_name: "test-agent".into(),
            agent_system_prompt: "test".into(),
            agent_tools: None,
            provider,
            provider_name: "mock".into(),
            model: "mock-1".into(),
            run_id: "run_reasoning_test".into(),
            workspace_id: "ws_test".into(),
            workspace_path: workspace.to_path_buf(),
            transcript_path,
            max_turns: 5,
            decider: Arc::new(BypassDecider),
            tool_context: rupu_tools::ToolContext {
                workspace_path: workspace.to_path_buf(),
                ..Default::default()
            },
            user_message: "test prompt".into(),
            initial_messages: Vec::new(),
            turn_index_offset: 0,
            mode_str: "bypass".into(),
            no_stream: true,
            suppress_stream_stdout: true,
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
            step_id: "s1".into(),
            on_tool_call: None,
            on_stream_event: None,
            concerns: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            scope_name: None,
            surface_tag: None,
            context_window_tokens: None,
            compact_at_percent: None,
            pause: None,
        }
    }

    /// Every `AssistantMessage` the real run wrote, in transcript order, as
    /// `(content, thinking)`. Reads the on-disk JSONL `run_agent` produced —
    /// no re-derivation of production logic.
    fn assistant_messages(path: &std::path::Path) -> Vec<(String, Option<String>)> {
        let body = std::fs::read_to_string(path).expect("read transcript");
        body.lines()
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .filter_map(|e| match e {
                Event::AssistantMessage { content, thinking } => Some((content, thinking)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn assistant_message_carries_reasoning_text() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![
                reasoning("thought"),
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
            stop: StopReason::EndTurn,
        }]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let msgs = assistant_messages(&transcript_path);
        assert_eq!(
            msgs.len(),
            1,
            "expected one assistant message, got {msgs:?}"
        );
        assert_eq!(msgs[0].0, "answer");
        assert_eq!(
            msgs[0].1.as_deref(),
            Some("thought"),
            "the turn's reasoning must ride on its assistant message"
        );
    }

    #[tokio::test]
    async fn reasoning_only_turn_still_records_thinking() {
        // The turn that matters most: the model reasoned about which tool to
        // call, then called it, emitting no text block at all. Without a
        // post-loop flush that reasoning is silently dropped.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let provider = MockProvider::new(vec![
            ScriptedTurn::AssistantBlocks {
                content: vec![
                    reasoning("planning"),
                    ContentBlock::ToolUse {
                        id: "call_read_1".into(),
                        name: "read_file".into(),
                        input: serde_json::json!({
                            "path": tmp.path().to_str().unwrap_or("/tmp")
                        }),
                    },
                ],
                stop: StopReason::ToolUse,
            },
            ScriptedTurn::AssistantText {
                text: "done".into(),
                stop: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            },
        ]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let msgs = assistant_messages(&transcript_path);
        assert_eq!(
            msgs.len(),
            2,
            "the tool-only turn must still write an assistant message, got {msgs:?}"
        );
        assert_eq!(msgs[0].0, "", "a tool-only turn carries no text");
        assert_eq!(
            msgs[0].1.as_deref(),
            Some("planning"),
            "reasoning must survive a turn with no text block"
        );
        assert_eq!(msgs[1].0, "done");
        assert_eq!(msgs[1].1, None, "the final turn had no reasoning");
    }

    #[tokio::test]
    async fn assistant_message_thinking_is_none_without_reasoning() {
        // Backward compatibility: a response with no reasoning behaves exactly
        // as it does today.
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantText {
            text: "plain answer".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let msgs = assistant_messages(&transcript_path);
        assert_eq!(
            msgs.len(),
            1,
            "expected one assistant message, got {msgs:?}"
        );
        assert_eq!(msgs[0].0, "plain answer");
        assert_eq!(msgs[0].1, None, "no reasoning → no thinking field");
    }

    #[tokio::test]
    async fn thinking_attaches_once_across_multiple_text_blocks() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let transcript_path = tmp.path().join("run.jsonl");

        let provider = MockProvider::new(vec![ScriptedTurn::AssistantBlocks {
            content: vec![
                reasoning("thought"),
                ContentBlock::Text { text: "a".into() },
                ContentBlock::Text { text: "b".into() },
            ],
            stop: StopReason::EndTurn,
        }]);
        let opts = opts_for(Box::new(provider), tmp.path(), transcript_path.clone());
        run_agent(opts).await.expect("run completes");

        let msgs = assistant_messages(&transcript_path);
        assert_eq!(
            msgs.len(),
            2,
            "expected two assistant messages, got {msgs:?}"
        );
        assert_eq!(msgs[0].0, "a");
        assert_eq!(
            msgs[0].1.as_deref(),
            Some("thought"),
            "the turn's first text block takes the reasoning"
        );
        assert_eq!(msgs[1].0, "b");
        assert_eq!(
            msgs[1].1, None,
            "reasoning must not be duplicated per block"
        );
    }
}
