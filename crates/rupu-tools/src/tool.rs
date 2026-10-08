//! Tool trait. Each tool implements one verb the agent can invoke.
//!
//! Tools are dispatched by the agent runtime in Plan 2. Inputs and
//! outputs are JSON-encoded so the runtime stays decoupled from any
//! particular tool's parameter schema. A subset of tools (write_file,
//! edit_file, bash) emit a [`DerivedEvent`] alongside their normal
//! `tool_result`, which the transcript layer indexes for cheap
//! "all file edits in this run" queries.

use crate::descriptor::ToolDescriptor;
use crate::permission::{PermissionMode, PermissionPolicy, Prompter};
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

/// Which kind of launch a run belongs to. Decided once, by the run
/// assembler, from the run's origin; recorded on coverage attribution and the
/// coverage run manifest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// A standalone `rupu run`, or a sub-agent of one.
    #[default]
    Agent,
    /// A workflow step (or a sub-agent of one).
    Workflow,
    /// A `rupu session` turn.
    Session,
    /// An agentiflow lead or unit.
    Agentiflow,
}

impl Surface {
    /// The wire word (`agent`, `workflow`, `session`, `agentiflow`).
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Agent => "agent",
            Surface::Workflow => "workflow",
            Surface::Session => "session",
            Surface::Agentiflow => "agentiflow",
        }
    }

    /// The coverage ledger's surface for this run. The coverage vocabulary
    /// has no agentiflow surface: an agentiflow run records as `agent`, as it
    /// always has.
    pub fn coverage(self) -> rupu_coverage::Surface {
        match self {
            Surface::Agent | Surface::Agentiflow => rupu_coverage::Surface::Agent,
            Surface::Workflow => rupu_coverage::Surface::Workflow,
            Surface::Session => rupu_coverage::Surface::Session,
        }
    }
}

/// The run that started this one, for a child run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentLink {
    /// The parent's own run id.
    pub run_id: String,
    /// The parent's codename, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// The root of the launch tree: the run whose usage ledger charges every
    /// descendant.
    pub root_run_id: String,
}

/// Who a run is. Set once, by the run assembler, and never overwritten
/// (spec 2026-10-07 W3, P6): the agent loop, tools and transcripts read it;
/// none of them write it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIdentity {
    pub run_id: String,
    /// The run's codename (`<crew>/<role>#n`); `None` when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// The agent's name.
    pub agent: String,
    /// The provider the run starts on. A fallback hop serves later turns on
    /// another provider; attribution keeps the one the run started on.
    pub provider: String,
    /// The model the run starts on (see `provider`).
    pub model: String,
    /// Dispatch depth: 0 for a top-level run, the parent's + 1 for a child.
    pub depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ParentLink>,
    pub surface: Surface,
    /// The findings/coverage scope (`target_id(workspace, scope)`). `None`
    /// scopes the run under its agent's name ([`Self::scope`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_name: Option<String>,
    /// The workflow step that owns the run; `None` outside a workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    /// The agents this run may dispatch (`dispatchableAgents:`). `None` =
    /// none. Moves into the launcher's policy in W7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatchable_agents: Option<Vec<String>>,
}

impl RunIdentity {
    /// The findings/coverage scope: `scope_name`, else the agent's name.
    pub fn scope(&self) -> &str {
        self.scope_name.as_deref().unwrap_or(&self.agent)
    }
}

/// `[bash]` settings for a run's `bash` calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashConfig {
    /// Environment variables (beyond the always-allowed
    /// PATH/HOME/USER/TERM/LANG) forwarded into `bash` subprocess envs.
    pub env_allowlist: Vec<String>,
    /// Default timeout for a single `bash` invocation, in seconds.
    pub timeout_secs: u64,
}

impl BashConfig {
    /// The timeout a run without `[bash].timeout_secs` gets.
    pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            env_allowlist: Vec::new(),
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
        }
    }
}

/// Where a run's tools act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceScope {
    /// The workspace record id (`rupu_workspace::upsert`).
    pub id: String,
    /// Workspace root. Read/write tools restrict their scope to this
    /// directory; `bash` runs with this as its cwd.
    pub path: PathBuf,
    pub bash: BashConfig,
}

impl Default for WorkspaceScope {
    fn default() -> Self {
        Self {
            id: String::new(),
            path: PathBuf::from("."),
            bash: BashConfig::default(),
        }
    }
}

/// The services a run provides its tools (P7: a granted tool whose service
/// is absent is not offered, and the grant names it `tool_unavailable`).
/// Built by the run assembler; the agent loop adds only the coverage writer
/// it owns.
#[derive(Clone, Default)]
pub struct ToolServices {
    /// The in-process sub-agent dispatcher behind `dispatch_agent` /
    /// `dispatch_agents_parallel`. `None` = this run can't dispatch.
    pub dispatcher: Option<Arc<dyn AgentDispatcher>>,
    /// The SCM/issue registry behind the connector tools (`scm.*`,
    /// `issues.*`, …). `None` = no connector tools.
    pub scm: Option<Arc<rupu_scm::Registry>>,
    /// How findings are recorded in this run: the resolved profile, the
    /// artifact store, size limits and the engagement. `None` means tools
    /// use `FindingWriteOptions::default()`, the full profile with no
    /// artifact store.
    pub findings: Option<rupu_coverage::FindingWriteOptions>,
    /// The coverage writer the agent loop spawns for a `concerns:` run.
    /// File-touching builtins emit FileTouchEvents to it; `None` disables
    /// coverage capture.
    pub coverage_writer: Option<Arc<rupu_coverage::CoverageWriter>>,
    /// Where this run's coverage is streamed for a coordinator to collect
    /// (`$RUPU_HOME/runs/<run_id>/coverage.jsonl`). Set by `rupu run` — the
    /// command every host connector launches — and shared with its
    /// `dispatch_agent` children; `None` for in-process workflow steps and
    /// sessions, which already write the coordinator's ledgers directly.
    pub coverage_stream: Option<PathBuf>,
    /// Where this run's network flows go, so the `bash` tool can attribute a
    /// child's connections to the run; `None` disables subprocess capture.
    pub netflow_sink: Option<Arc<dyn rupu_netflow::FlowSink>>,
    /// The process-wide subprocess-capture backend the `bash` tool drives
    /// around each spawn. `None` = no capture (the exact pre-capture path).
    pub net_capture: Option<Arc<dyn rupu_netflow::SubprocessCapture>>,
    /// The customer this run is attributed to (resolved once, at launch).
    /// `None` = no customer.
    pub customer: Option<String>,
    /// The run's operator prompter, when it has one: a child run started by
    /// a call asks the same operator.
    pub prompter: Option<Arc<dyn Prompter>>,
}

// Hand-written because the trait objects are not `Debug`.
impl std::fmt::Debug for ToolServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolServices")
            .field("dispatcher", &self.dispatcher)
            .field("scm", &self.scm.as_ref().map(|_| "<registry>"))
            .field("findings", &self.findings)
            .field("coverage_writer", &self.coverage_writer)
            .field("coverage_stream", &self.coverage_stream)
            .field(
                "netflow_sink",
                &self.netflow_sink.as_ref().map(|_| "<sink>"),
            )
            .field(
                "net_capture",
                &self.net_capture.as_ref().map(|_| "<capture>"),
            )
            .field("customer", &self.customer)
            .field("prompter", &self.prompter.as_ref().map(|_| "<prompter>"))
            .finish()
    }
}

/// What the agent loop sets for one tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallContext {
    /// The model's tool-call id for the invocation in flight — attributes a
    /// bash call's captured connections to that call.
    pub tool_call_id: Option<String>,
    /// The ceiling a child run this call starts is capped at: set by the
    /// agent loop for a `Spawn` call from [`Decision::Spawn`]. `None` (a tool
    /// invoked without a permission decision) caps a child at readonly.
    ///
    /// [`Decision::Spawn`]: crate::permission::Decision::Spawn
    pub spawn_ceiling: Option<PermissionMode>,
}

/// Per-invocation context the runtime passes to every tool: who the run is,
/// where it acts, what services it has, and the call in flight.
#[derive(Clone, Debug, Default)]
pub struct ToolContext {
    /// Set once by the run assembler (P6).
    pub identity: Arc<RunIdentity>,
    pub workspace: WorkspaceScope,
    pub services: ToolServices,
    pub call: CallContext,
}

impl ToolContext {
    /// A context acting in `path` with nothing else set — for tests and for
    /// callers that invoke one tool outside an agent run.
    pub fn in_workspace(path: impl Into<PathBuf>) -> Self {
        Self {
            workspace: WorkspaceScope {
                path: path.into(),
                ..WorkspaceScope::default()
            },
            ..Self::default()
        }
    }

    /// The run's identity, for a caller building a context by hand (tests):
    /// copy-on-write, so a shared identity is never changed under a run.
    pub fn identity_mut(&mut self) -> &mut RunIdentity {
        Arc::make_mut(&mut self.identity)
    }
}

/// What a child run started by a `Spawn` call is allowed: the ceiling the
/// permission decision set, and the operator prompter to share.
#[derive(Clone)]
pub struct SpawnPermission {
    pub ceiling: PermissionMode,
    pub prompter: Option<Arc<dyn Prompter>>,
}

impl SpawnPermission {
    /// The spawn permission of the call `ctx` describes. A call that reached
    /// the tool without a permission decision gets readonly (fail closed).
    pub fn from_ctx(ctx: &ToolContext) -> Self {
        Self {
            ceiling: ctx.call.spawn_ceiling.unwrap_or(PermissionMode::Readonly),
            prompter: ctx.services.prompter.clone(),
        }
    }

    /// The policy the child runs under: `min(ceiling, child's own mode)`
    /// (D7), with the shared prompter.
    pub fn child_policy(&self, child_declared: Option<PermissionMode>) -> PermissionPolicy {
        PermissionPolicy::for_child(self.ceiling, child_declared, self.prompter.clone())
    }
}

impl std::fmt::Debug for SpawnPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpawnPermission")
            .field("ceiling", &self.ceiling)
            .field("prompter", &self.prompter.as_ref().map(|_| "<prompter>"))
            .finish()
    }
}

/// Pluggable handle the orchestrator implements so dispatch tools can
/// spawn child agent runs without rupu-tools depending on
/// rupu-orchestrator (which would be circular). Most tools never
/// touch this — only the new `dispatch_agent` family does.
#[async_trait]
pub trait AgentDispatcher: Send + Sync + std::fmt::Debug {
    /// Spawn a child agent run synchronously. The dispatcher resolves
    /// the agent file by name, allocates a sub-run id under the parent's
    /// run, assembles the child (depth `parent.depth + 1`, the parent's
    /// surface and findings scope) and runs it to completion. Returns the
    /// child's outcome — final assistant text, tokens used, duration, and
    /// the path to the persisted child transcript.
    ///
    /// `parent` is the dispatching run's identity; the child's
    /// `<parent>><role>#n` codename is minted from its codename (`None`
    /// leaves the child unnamed).
    ///
    /// `permission` caps the child (D7): it runs under
    /// [`SpawnPermission::child_policy`] of its own declared
    /// `permissionMode`, never above the parent's ceiling.
    async fn dispatch(
        &self,
        agent_name: &str,
        prompt: String,
        parent: &RunIdentity,
        permission: SpawnPermission,
    ) -> Result<DispatchOutcome, DispatchError>;
}

/// Result of a successful child-agent dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchOutcome {
    /// Agent name as resolved by the dispatcher.
    pub agent: String,
    /// Sub-run id (`sub_<ULID>`).
    pub sub_run_id: String,
    /// The child's minted codename (`<parent>><role>#n`). `None` when
    /// the parent had no codename to derive one from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    /// Path to the persisted child transcript. The line-stream
    /// printer uses this to render the child's run inline as a
    /// child callout frame.
    pub transcript_path: PathBuf,
    /// Final assistant text from the child agent. Empty when the child
    /// failed (spec 2026-10-01 §5.3): its last text is interim or cut off,
    /// not an answer; `error` says why.
    pub output: String,
    /// True iff the child finished without an agent error.
    pub success: bool,
    /// Why the child failed — the error its run recorded (the outcome's
    /// title plus the recovery hint). `None` on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    /// Optional structured payload the runtime copies onto the emitted
    /// `tool_result` event, in addition to `stdout`. Lets a tool ship
    /// machine-rendered data (e.g. ast_grep matches + metavariables) to
    /// the control plane without changing the text the agent reads.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured: Option<serde_json::Value>,
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

/// One verb the agent can invoke. What the tool *is* — name, aliases,
/// effect, needs, description, schema — is its [`ToolDescriptor`]; the
/// agent runtime decides permission from the descriptor's effect alone.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The tool's static declaration (see [`crate::catalog`]).
    fn descriptor(&self) -> &'static ToolDescriptor;

    /// Invoke the tool with JSON-encoded input. The boxed `Send +
    /// Sync` future makes this trait object-safe for `Box<dyn Tool>`.
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;

    /// Canonical name the model sees and the runtime dispatches on.
    fn name(&self) -> &'static str {
        self.descriptor().name
    }

    /// Human-readable description shown to the LLM.
    fn description(&self) -> &'static str {
        self.descriptor().description
    }

    /// JSON Schema of the tool's input, sent to the LLM. Overridden only by
    /// a tool whose schema depends on its run.
    fn input_schema(&self) -> serde_json::Value {
        (self.descriptor().input_schema)()
    }
}

#[cfg(test)]
mod coverage_context_tests {
    use super::*;

    #[test]
    fn default_tool_context_has_no_coverage_writer() {
        let ctx = ToolContext::default();
        assert!(ctx.services.coverage_writer.is_none());
    }
}
