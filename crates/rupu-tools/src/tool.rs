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
    /// What this run may launch and where its children are recorded (W7).
    #[serde(default)]
    pub launch: LaunchScope,
}

/// What a run may launch, and where its children are recorded (spec W7
/// §3.4). Set once by the run assembler from the agent's
/// `dispatchableAgents` / `dispatchableWorkflows`, the flow's pool and the cap
/// its parent passed down; the launcher only reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchScope {
    /// The agents a `dispatch` may name: the agent's own allowlist, its flow's
    /// pool when it is in one (intersected when it declares both), within the
    /// cap its parent passed down. Empty = none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    /// The workflows a `dispatch {kind: workflow}` may name, by the same rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<String>,
    /// The agentiflow pool this run is inside, which an in-process child
    /// inherits; `None` outside a flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<LaunchPool>,
    /// The connector tools this run is narrowed to (a step's `actions:`, or
    /// the cap a parent passed down), which its children inherit as a cap.
    /// `None` = not narrowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_cap: Option<Vec<String>>,
    /// The connector tools this run's grant offers, narrowed or not: a
    /// workflow it generates may call no other connector tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub granted_connectors: Vec<String>,
    /// The directory whose `children.jsonl` records what this run launched.
    /// `None` = this run can't launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children_dir: Option<PathBuf>,
    /// The usage ledger this run's in-process children are charged to (its
    /// own, or its in-process root's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_root: Option<PathBuf>,
    /// The ledger of the root of the whole launch tree, when this run is a
    /// process child (or descends from one): each of its calls is also
    /// written there as a rollup row, for a budget over the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollup_root: Option<PathBuf>,
    /// The agentiflow this run takes part in: a process child of it joins the
    /// flow's board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<PathBuf>,
}

/// An agentiflow's pool: the agents and workflows its runs may launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchPool {
    pub agents: Vec<String>,
    pub workflows: Vec<String>,
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
    /// The roots of the run's granted skills, which the fs read tools may
    /// read even outside the workspace (spec §3.4). Never writable.
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
}

impl Default for WorkspaceScope {
    fn default() -> Self {
        Self {
            id: String::new(),
            path: PathBuf::from("."),
            bash: BashConfig::default(),
            read_roots: Vec::new(),
        }
    }
}

/// The services a run provides its tools (P7: a granted tool whose service
/// is absent is not offered, and the grant names it `tool_unavailable`).
/// Built by the run assembler; the agent loop adds only the coverage writer
/// and concern catalog it owns.
#[derive(Clone, Default)]
pub struct ToolServices {
    /// The launcher behind `dispatch` / `join`. `None` = this run can't
    /// start children.
    pub launcher: Option<Arc<dyn crate::ports::Launcher>>,
    /// What `workflows.generate` authors a workflow with.
    pub workflow_generator: Option<Arc<dyn crate::ports::WorkflowGenerator>>,
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
    /// The flattened concern catalog of a `concerns:` run, which the
    /// `coverage.*` tools read. Set by the agent loop with the writer.
    pub coverage_catalog: Option<Arc<rupu_coverage::FlatCatalog>>,
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
    /// The run's address on its flow's board and mailboxes, behind `board.*`
    /// / `msg.send`. Built by the run's owner (an agentiflow lead or unit);
    /// `None` outside a flow.
    pub message_bus: Option<Arc<rupu_fleet::Bus>>,
    /// A flow's live goals, coverage and budget, behind `goal.*` /
    /// `budget.status`. Set by an agentiflow for its lead.
    pub run_status: Option<Arc<dyn crate::ports::RunStatus>>,
    /// The agents and workflows the run can draw on, behind `agents.*`,
    /// `workflows.list` / `workflows.get` and `catalog.search`. Every launch
    /// site provides one.
    pub catalog: Option<Arc<dyn crate::ports::CatalogPort>>,
    /// The skills this run was granted (spec §2.4); `skills.load` reads it.
    /// `Service::Skills` is provided only when the model may load one of
    /// them.
    pub skills: Option<Arc<rupu_skills::ResolvedSkills>>,
}

impl ToolServices {
    /// The services these provide, for a grant (W2). Exact by construction:
    /// a service is here only when [`crate::bodies::body`] can build every
    /// tool needing it from a context holding these. `coverage` says the run
    /// has a `concerns:` block (its catalog is flattened once the loop
    /// starts, so it isn't in [`Self::coverage_catalog`] yet at assembly).
    pub fn provided(&self, coverage: bool) -> crate::grant::ServiceSet {
        use crate::descriptor::Service;
        // The findings ledger lives in the workspace: every run can open it.
        let mut s = crate::grant::ServiceSet::new().with(Service::Findings);
        if coverage || self.coverage_catalog.is_some() {
            s.insert(Service::Coverage);
        }
        if self
            .findings
            .as_ref()
            .is_some_and(|f| f.engagement.is_some())
        {
            s.insert(Service::Engagement);
        }
        if self.scm.is_some() {
            s.insert(Service::Scm);
        }
        if self.launcher.is_some() {
            s.insert(Service::Launcher);
        }
        if self.workflow_generator.is_some() {
            s.insert(Service::WorkflowGenerator);
        }
        if self.netflow_sink.is_some() {
            s.insert(Service::Netflow);
        }
        if self.message_bus.is_some() {
            s.insert(Service::MessageBus);
        }
        if self.run_status.is_some() {
            s.insert(Service::RunStatus);
        }
        if self.catalog.is_some() {
            s.insert(Service::Catalog);
        }
        if self.skills.as_ref().is_some_and(|s| s.has_loadable()) {
            s.insert(Service::Skills);
        }
        s
    }
}

// Hand-written because the trait objects are not `Debug`.
impl std::fmt::Debug for ToolServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolServices")
            .field("launcher", &self.launcher.as_ref().map(|_| "<launcher>"))
            .field(
                "workflow_generator",
                &self.workflow_generator.as_ref().map(|_| "<generator>"),
            )
            .field("scm", &self.scm.as_ref().map(|_| "<registry>"))
            .field("findings", &self.findings)
            .field("coverage_writer", &self.coverage_writer)
            .field(
                "coverage_catalog",
                &self.coverage_catalog.as_ref().map(|_| "<catalog>"),
            )
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
            .field("message_bus", &self.message_bus)
            .field(
                "run_status",
                &self.run_status.as_ref().map(|_| "<run status>"),
            )
            .field("catalog", &self.catalog.as_ref().map(|_| "<catalog>"))
            .field("skills", &self.skills.as_ref().map(|s| s.names()))
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
    /// The name the model called the tool by — its canonical name or a
    /// legacy alias. A tool that answers its aliases in their old shapes
    /// (`dispatch_agent`, `run_workflow`, …) reads it; `None` = canonical.
    pub called_as: Option<String>,
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
