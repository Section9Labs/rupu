//! Tool trait. Each tool implements one verb the agent can invoke.
//!
//! Tools are dispatched by the agent runtime in Plan 2. Inputs and
//! outputs are JSON-encoded so the runtime stays decoupled from any
//! particular tool's parameter schema. A subset of tools (write_file,
//! edit_file, bash) emit a [`DerivedEvent`] alongside their normal
//! `tool_result`, which the transcript layer indexes for cheap
//! "all file edits in this run" queries.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Errors a tool can return at the dispatch boundary. Tool-internal
/// failures (file not found, exit code != 0, edit didn't match) are
/// NOT modeled here — they are surfaced as `error: Some(...)` on the
/// returned [`ToolOutput`] so the agent sees them as part of normal
/// flow rather than as Rust-level errors.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("timeout")]
    Timeout,
    #[error("permission denied")]
    PermissionDenied,
    #[error("execution: {0}")]
    Execution(String),
}

/// Per-invocation context the runtime passes to every tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    /// Workspace root. Read/write tools restrict their scope to this
    /// directory; `bash` runs with this as its cwd.
    pub workspace_path: PathBuf,
    /// Environment variables (beyond the always-allowed
    /// PATH/HOME/USER/TERM/LANG) forwarded into `bash` subprocess
    /// envs. Used for things like AWS_PROFILE.
    pub bash_env_allowlist: Vec<String>,
    /// Default timeout for a single `bash` invocation, in seconds.
    pub bash_timeout_secs: u64,
    /// Optional handle the orchestrator wires in so dispatch tools
    /// (`dispatch_agent` / `dispatch_agents_parallel`) can spawn
    /// child agent runs. `None` = the runtime hosting this tool
    /// invocation can't dispatch (e.g. a bare `rupu run`, a unit
    /// test). Skipped in serde because trait objects don't round-trip
    /// — production callers always populate it programmatically.
    /// See `docs/superpowers/specs/2026-05-08-rupu-sub-agent-dispatch-design.md`.
    #[serde(skip)]
    pub dispatcher: Option<Arc<dyn AgentDispatcher>>,
    /// Per-agent allowlist of children this agent can dispatch.
    /// Pulled from the agent's `dispatchableAgents:` frontmatter
    /// field by the runner before tool invocation. The dispatch
    /// tools check the requested agent name against this list.
    /// `None` = agent has no dispatchable_agents declaration ⇒
    /// no dispatches allowed.
    #[serde(skip)]
    pub dispatchable_agents: Option<Vec<String>>,
    /// Parent run id of the run currently invoking the tool. Threaded
    /// through so the dispatcher can record `parent_run_id` on the
    /// child run and so child transcripts persist under the parent's
    /// directory. `None` for top-level runs (the parent IS top-level).
    #[serde(skip)]
    pub parent_run_id: Option<String>,
    /// Dispatch depth of the current agent run. The dispatch tools
    /// check this against the per-agent + workspace max-depth
    /// before spawning a child. Top-level workflow steps have
    /// depth 0; first child has depth 1; etc.
    #[serde(skip)]
    pub depth: u32,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            bash_env_allowlist: Vec::new(),
            bash_timeout_secs: 120,
            dispatcher: None,
            dispatchable_agents: None,
            parent_run_id: None,
            depth: 0,
        }
    }
}

/// Pluggable handle the orchestrator implements so dispatch tools can
/// spawn child agent runs without rupu-tools depending on
/// rupu-orchestrator (which would be circular). Most tools never
/// touch this — only the new `dispatch_agent` family does.
#[async_trait]
pub trait AgentDispatcher: Send + Sync + std::fmt::Debug {
    /// Spawn a child agent run synchronously. The dispatcher resolves
    /// the agent file by name, allocates a sub-run id under
    /// `parent_run_id`, builds [`AgentRunOpts`] (with `depth =
    /// parent_depth + 1`), and runs the agent to completion. Returns
    /// the child's outcome — final assistant text, tokens used,
    /// duration, and the path to the persisted child transcript.
    async fn dispatch(
        &self,
        agent_name: &str,
        prompt: String,
        parent_run_id: &str,
        parent_depth: u32,
    ) -> Result<DispatchOutcome, DispatchError>;
}

/// Result of a successful child-agent dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchOutcome {
    /// Agent name as resolved by the dispatcher.
    pub agent: String,
    /// Sub-run id (`sub_<ULID>`).
    pub sub_run_id: String,
    /// Path to the persisted child transcript. The line-stream
    /// printer uses this to render the child's run inline as a
    /// child callout frame.
    pub transcript_path: PathBuf,
    /// Final assistant text from the child agent.
    pub output: String,
    /// True iff the child finished without an agent error.
    pub success: bool,
    /// Total tokens consumed by the child run (in + out).
    pub tokens_used: u64,
    /// Wall-clock duration of the child run in milliseconds.
    pub duration_ms: u64,
}

/// Errors a dispatcher can return. Tool-internal failures (allowlist
/// rejection, depth-limit hit) are NOT modeled here — those surface
/// as `error: Some(...)` on the dispatch tool's [`ToolOutput`] so the
/// agent sees them as part of normal tool flow rather than as
/// dispatcher-level failures.
#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("agent `{agent}` not found in any registered agent path")]
    AgentNotFound { agent: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider build failed: {0}")]
    ProviderBuild(String),
    #[error("child run failed: {0}")]
    ChildRun(String),
    #[error("run-store: {0}")]
    RunStore(String),
}

/// What a tool returns. `stdout` is the human-readable result that
/// goes back to the agent. `error` is `Some` when the tool ran but
/// failed (e.g., command exited non-zero, file not found). `derived`
/// carries the optional structured event for indexable tool kinds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Human-readable result returned to the agent.
    pub stdout: String,
    /// Set when the tool ran but produced a failure result.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// Wall-clock time the invocation took, in milliseconds.
    pub duration_ms: u64,
    /// If the tool corresponds to a derived event (file_edit,
    /// command_run), the runtime emits the derived event in addition
    /// to `tool_result`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub derived: Option<DerivedEvent>,
}

/// Tool-emitted side events that the transcript layer indexes
/// separately from raw tool_result events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DerivedEvent {
    /// Emitted by write_file and edit_file when a file is created,
    /// modified, or deleted.
    FileEdit {
        /// Workspace-relative path of the affected file.
        path: String,
        /// One of `"create"`, `"modify"`, or `"delete"`.
        kind: String,
        /// Unified diff of the change.
        diff: String,
    },
    /// Emitted by bash on subprocess completion.
    CommandRun {
        /// Argument vector passed to the subprocess.
        argv: Vec<String>,
        /// Working directory of the subprocess.
        cwd: String,
        /// Exit code returned by the subprocess.
        exit_code: i32,
        /// Total bytes written to stdout.
        stdout_bytes: u64,
        /// Total bytes written to stderr.
        stderr_bytes: u64,
    },
}

/// Render a lightweight unified diff for file edits.
///
/// The output is intentionally simple but starts with standard
/// `diff --git` / `---` / `+++` / `@@` headers so downstream renderers
/// can syntax-highlight it consistently.
pub fn render_file_edit_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let before = before.unwrap_or_default();
    let after = after.unwrap_or_default();
    if before == after {
        return String::new();
    }

    let old_label = if before.is_empty() {
        "/dev/null".to_string()
    } else {
        format!("a/{path}")
    };
    let new_label = if after.is_empty() {
        "/dev/null".to_string()
    } else {
        format!("b/{path}")
    };

    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    if before.is_empty() && !after.is_empty() {
        out.push_str("new file mode 100644\n");
    } else if !before.is_empty() && after.is_empty() {
        out.push_str("deleted file mode 100644\n");
    }
    out.push_str(&format!("--- {old_label}\n"));
    out.push_str(&format!("+++ {new_label}\n"));
    out.push_str("@@\n");

    for line in before.lines() {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in after.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }

    out
}

/// One verb the agent can invoke. Implementations live one per file
/// in this crate (`bash.rs`, `read_file.rs`, etc).
#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable tool name used in agent files and tool calls. The
    /// agent runtime dispatches on this string.
    fn name(&self) -> &'static str;

    /// Human-readable description shown to the LLM. Should explain
    /// what the tool does, when to use it, and any pitfalls.
    fn description(&self) -> &'static str;

    /// JSON Schema describing the tool's input. Sent to the LLM as
    /// part of the request so it knows how to call the tool. Format
    /// matches Anthropic's tool-use input_schema convention.
    fn input_schema(&self) -> serde_json::Value;

    /// Invoke the tool with JSON-encoded input. The boxed `Send +
    /// Sync` future makes this trait object-safe for `Box<dyn Tool>`.
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}
