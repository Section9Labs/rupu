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
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            bash_env_allowlist: Vec::new(),
            bash_timeout_secs: 120,
        }
    }
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
