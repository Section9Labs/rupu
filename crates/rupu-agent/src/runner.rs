//! The agent loop. Wires provider → tool dispatch (with permission
//! gating) → transcript writes → turn accounting → run-complete.
//!
//! This is the integration point of `rupu-providers`, `rupu-tools`,
//! and `rupu-transcript`. The CLI (Plan 2 Phase 3) calls [`run_agent`]
//! once per `rupu run` invocation.

use crate::permission::PermissionDecision;
use crate::tool_registry::{default_tool_registry, ToolRegistry};
use async_trait::async_trait;
use chrono::Utc;
use rupu_providers::provider::LlmProvider;
use rupu_providers::types::{
    ContentBlock, LlmRequest, LlmResponse, Message, Role, StopReason, StreamEvent, Usage,
};
use rupu_tools::{DerivedEvent, PermissionMode, Tool, ToolContext};
use rupu_transcript::{Event, FileEditKind, JsonlWriter, RunMode, RunStatus};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;

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
    pub mode_str: String,
}

/// Outcome of a finished run.
pub struct RunResult {
    pub turns: u32,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
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

    let registry: ToolRegistry = match &opts.agent_tools {
        Some(list) => default_tool_registry().filter_to(list),
        None => default_tool_registry(),
    };
    let tool_defs = registry.to_tool_definitions();

    let mut messages: Vec<Message> = vec![Message::user(&opts.user_message)];
    let mut turn_idx: u32 = 0;
    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut runtime_mode = parse_mode_for_runtime(&opts.mode_str);

    let result_status = loop {
        if turn_idx >= opts.max_turns {
            break RunStatus::Error;
        }
        writer.write(&Event::TurnStart { turn_idx })?;
        // LlmRequest does not derive Default — construct explicitly.
        let req = LlmRequest {
            model: opts.model.clone(),
            system: Some(opts.agent_system_prompt.clone()),
            messages: messages.clone(),
            max_tokens: 4096,
            tools: tool_defs.clone(),
            cell_id: None,
            trace_id: None,
            thinking: None,
            task_type: None,
        };
        let resp: LlmResponse = match opts.provider.send(&req).await {
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
                return Err(RunError::Provider(e.to_string()));
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
            let started_tool = Instant::now();
            match tool.invoke(input.clone(), &opts.tool_context).await {
                Ok(out) => {
                    writer.write(&Event::ToolResult {
                        call_id: call_id.clone(),
                        output: out.stdout.clone(),
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
                    tool_results.push((call_id, out.stdout, out.error));
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
                blocks.push(ContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content: if let Some(e) = error {
                        format!("error: {e}\n{output}")
                    } else {
                        output
                    },
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

    Ok(RunResult {
        turns: turn_idx,
        total_tokens_in: total_in,
        total_tokens_out: total_out,
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
