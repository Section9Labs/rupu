//! Ports: the effects a catalog tool needs from a runtime that sits above
//! this crate (D1/D2). A tool calls a store crate directly when the store
//! is a plain local-file library (`rupu-coverage`, `rupu-fleet`,
//! `rupu-scm`); it calls a port when the effect needs something that
//! depends on this crate — loading agent and workflow files
//! ([`CatalogPort`], adapted by `rupu-orchestrator`'s `FsCatalog`) or a
//! flow's live status ([`RunStatus`], implemented by `rupu-agentiflow`),
//! starting and watching child runs ([`Launcher`], implemented by
//! `rupu-launch`) and authoring a workflow ([`WorkflowGenerator`]).
//!
//! The report types are the JSON shapes the tools return; each `to_json`
//! builds exactly that shape, so the model sees the same output whoever
//! implements the port.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::tool::{RunIdentity, ToolContext};

/// A port call that could not produce its answer. The message is what the
/// tool reports to the model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PortError(pub String);

impl PortError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

// ---- the agent / workflow catalog --------------------------------------------

/// One agent, as the roster tools (`agents.*`, `catalog.search`) show it:
/// its frontmatter, never its system prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: String,
    pub description: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The declared `tools:`; `None` = the default tool set.
    pub tools: Option<Vec<String>>,
    pub dispatchable_agents: Option<Vec<String>>,
    pub permission_mode: Option<String>,
    pub max_turns: Option<u32>,
}

/// One workflow, as `workflows.*` / `catalog.search` show it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowEntry {
    /// The runnable identifier: the file stem.
    pub id: String,
    /// The declared `name:` (the stem when the file did not parse).
    pub name: String,
    pub description: Option<String>,
    /// `"global"` or `"project"`.
    pub scope: String,
    /// Declared input names, sorted.
    pub input_keys: Vec<String>,
    pub step_count: usize,
    /// Set when the file did not parse; the entry is still listed.
    pub parse_error: Option<String>,
}

/// The agents and workflows a run can draw on (global + project).
pub trait CatalogPort: Send + Sync {
    /// Every agent. `Err` names why the agents could not be listed (one
    /// unparseable agent file): the tools report it rather than abort, and
    /// `catalog.search` still searches the workflows.
    fn agents(&self) -> Result<Vec<AgentEntry>, PortError>;

    /// Every workflow, sorted by id. Never fails: a workflow that does not
    /// parse is listed with its [`WorkflowEntry::parse_error`].
    fn workflows(&self) -> Vec<WorkflowEntry>;
}

// ---- a flow's live status ------------------------------------------------------

/// A flow's goals, its coverage target and its budget, re-checked now.
/// Implementations may read whole ledgers: the tools call them off the async
/// runtime.
pub trait RunStatus: Send + Sync {
    /// `goal.status`.
    fn goals(&self) -> Result<GoalReport, PortError>;
    /// `goal.coverage`.
    fn coverage(&self) -> Result<GoalCoverageReport, PortError>;
    /// `budget.status`.
    fn budget(&self) -> Result<BudgetReport, PortError>;
}

/// `goal.status`: one row per goal, in definition order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GoalReport(pub Vec<GoalRow>);

#[derive(Debug, Clone, PartialEq)]
pub struct GoalRow {
    pub id: String,
    pub objective: String,
    pub required: bool,
    /// The goal's tally, or why it could not be evaluated (reported on this
    /// row only; the other goals are still evaluated).
    pub result: Result<GoalTally, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoalTally {
    pub satisfied: bool,
    pub current: u64,
    pub target: u64,
    pub detail: String,
}

impl GoalReport {
    /// `[{id, objective, required, satisfied, current, target, detail}]`, or
    /// `{id, objective, required, error}` for a goal that could not be
    /// evaluated.
    pub fn to_json(&self) -> Value {
        Value::Array(
            self.0
                .iter()
                .map(|g| match &g.result {
                    Ok(t) => json!({
                        "id": g.id,
                        "objective": g.objective,
                        "required": g.required,
                        "satisfied": t.satisfied,
                        "current": t.current,
                        "target": t.target,
                        "detail": t.detail,
                    }),
                    Err(e) => json!({
                        "id": g.id,
                        "objective": g.objective,
                        "required": g.required,
                        "error": e,
                    }),
                })
                .collect(),
        )
    }
}

/// `goal.coverage`.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalCoverageReport {
    /// The flow sets no coverage target.
    NoTarget,
    Evaluated {
        /// The required fraction.
        reach: f64,
        /// The rung assets must reach (`None`: the ladder's terminal rung).
        depth: Option<String>,
        fraction: f64,
        satisfied: bool,
        /// Per-asset-kind fractions.
        per_kind: Vec<(String, f64)>,
    },
}

impl GoalCoverageReport {
    /// `{reach, depth, fraction, satisfied, per_kind}`, or `{coverage: null}`.
    pub fn to_json(&self) -> Value {
        match self {
            GoalCoverageReport::NoTarget => json!({ "coverage": null }),
            GoalCoverageReport::Evaluated {
                reach,
                depth,
                fraction,
                satisfied,
                per_kind,
            } => {
                let per_kind: Map<String, Value> = per_kind
                    .iter()
                    .map(|(k, f)| (k.clone(), json!(f)))
                    .collect();
                json!({
                    "reach": reach,
                    "depth": depth,
                    "fraction": fraction,
                    "satisfied": satisfied,
                    "per_kind": per_kind,
                })
            }
        }
    }
}

/// Where a budget (or one of its dimensions) stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetState {
    Ok,
    /// Crossed its soft threshold: converge.
    Soft,
    /// A cap is reached.
    Hard,
}

impl BudgetState {
    pub fn as_str(self) -> &'static str {
        match self {
            BudgetState::Ok => "ok",
            BudgetState::Soft => "soft",
            BudgetState::Hard => "hard",
        }
    }
}

/// `budget.status`.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetReport {
    /// The flow sets no budget (or a `budget:` block setting nothing).
    NoBudget,
    Set(BudgetSet),
}

/// A set budget: the overall stage and each dimension the flow sets.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetSet {
    pub stage: BudgetState,
    /// The dimension that tripped a `Hard` stage.
    pub tripped: Option<String>,
    pub soft_at: Option<f64>,
    pub usd: Option<UsdLine>,
    pub tokens: Option<TokensLine>,
    pub rounds: Option<RoundsLine>,
    pub wall_clock: Option<WallClockLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsdLine {
    pub cap: f64,
    pub spent: f64,
    pub enforcement: UsdEnforcement,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsdEnforcement {
    Enforced {
        state: BudgetState,
        remaining: f64,
    },
    /// The cap can't fire (nothing can price the spend); `note` says why.
    NotEnforced {
        note: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokensLine {
    pub cap: u64,
    pub spent: u64,
    pub remaining: u64,
    pub state: BudgetState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoundsLine {
    pub cap: u32,
    /// Rounds completed before the current one.
    pub elapsed: u32,
    pub remaining: u32,
    pub state: BudgetState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WallClockLine {
    /// The cap as the definition wrote it (`"2h"`).
    pub cap: String,
    /// The tally, or why the cap could not be read.
    pub tally: Result<WallClockTally, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WallClockTally {
    pub cap_secs: i64,
    pub elapsed_secs: i64,
    pub remaining_secs: i64,
    pub state: BudgetState,
}

impl BudgetReport {
    /// `{stage, tripped?, soft_at?, usd?, tokens?, rounds?, wall_clock?}`, or
    /// `{budget: null, detail: "no budget set"}`.
    pub fn to_json(&self) -> Value {
        let b = match self {
            BudgetReport::NoBudget => {
                return json!({ "budget": null, "detail": "no budget set" });
            }
            BudgetReport::Set(b) => b,
        };
        let mut out = Map::new();
        out.insert("stage".into(), json!(b.stage.as_str()));
        if let Some(dimension) = &b.tripped {
            out.insert("tripped".into(), json!(dimension));
        }
        if let Some(soft_at) = b.soft_at {
            out.insert("soft_at".into(), json!(soft_at));
        }
        if let Some(usd) = &b.usd {
            let mut d = Map::new();
            d.insert("cap".into(), json!(usd.cap));
            d.insert("spent".into(), json!(usd.spent));
            match &usd.enforcement {
                UsdEnforcement::Enforced { state, remaining } => {
                    d.insert("enforced".into(), json!(true));
                    d.insert("state".into(), json!(state.as_str()));
                    d.insert("remaining".into(), json!(remaining));
                }
                UsdEnforcement::NotEnforced { note } => {
                    d.insert("enforced".into(), json!(false));
                    d.insert("note".into(), json!(note));
                }
            }
            out.insert("usd".into(), Value::Object(d));
        }
        if let Some(t) = &b.tokens {
            out.insert(
                "tokens".into(),
                json!({
                    "cap": t.cap,
                    "spent": t.spent,
                    "remaining": t.remaining,
                    "state": t.state.as_str(),
                }),
            );
        }
        if let Some(r) = &b.rounds {
            out.insert(
                "rounds".into(),
                json!({
                    "cap": r.cap,
                    "elapsed": r.elapsed,
                    "remaining": r.remaining,
                    "state": r.state.as_str(),
                }),
            );
        }
        if let Some(w) = &b.wall_clock {
            let row = match &w.tally {
                Ok(t) => json!({
                    "cap": w.cap,
                    "cap_secs": t.cap_secs,
                    "elapsed_secs": t.elapsed_secs,
                    "remaining_secs": t.remaining_secs,
                    "state": t.state.as_str(),
                }),
                Err(e) => json!({ "cap": w.cap, "error": e }),
            };
            out.insert("wall_clock".into(), row);
        }
        Value::Object(out)
    }
}

// ---- launching child runs (W7) ------------------------------------------------

/// The deepest launch chain allowed: a run at this depth can't start a child.
/// A top-level run is depth 0; a child is its parent's depth + 1, in the same
/// process or across one (`--launch-depth`).
pub const MAX_LAUNCH_DEPTH: u32 = 5;

/// How a child runs (spec W7 §1).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildKind {
    /// A tokio task in the parent's process; its transcript lives under the
    /// parent run (`<runs>/<parent>/sub/<sub_id>/`).
    Subagent,
    /// Its own `rupu run` process and run record.
    Agent,
    /// Its own `rupu workflow run` process and workflow run.
    Workflow,
}

impl ChildKind {
    /// The wire word (`subagent`, `agent`, `workflow`).
    pub fn as_str(self) -> &'static str {
        match self {
            ChildKind::Subagent => "subagent",
            ChildKind::Agent => "agent",
            ChildKind::Workflow => "workflow",
        }
    }

    /// The kind a wire word names.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "subagent" => Some(ChildKind::Subagent),
            "agent" => Some(ChildKind::Agent),
            "workflow" => Some(ChildKind::Workflow),
            _ => None,
        }
    }
}

/// What a child runs: a catalog agent or workflow by name, or a workflow file
/// this run generated (`workflows.generate`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildTarget {
    Name(String),
    File(PathBuf),
}

impl ChildTarget {
    /// The name the child is shown and allowlisted by: the name, or a file's
    /// stem.
    pub fn label(&self) -> String {
        match self {
            ChildTarget::Name(n) => n.clone(),
            ChildTarget::File(f) => f
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.display().to_string()),
        }
    }
}

/// One child to start.
#[derive(Clone, Debug, PartialEq)]
pub struct ChildRequest {
    pub kind: ChildKind,
    pub target: ChildTarget,
    /// The agent's task. A workflow takes `inputs` instead.
    pub prompt: Option<String>,
    /// A workflow's inputs (scalars), or structured agent inputs folded into
    /// the prompt.
    pub inputs: BTreeMap<String, Value>,
}

/// A started child, as `dispatch` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildHandle {
    /// `ch_<ULID>`: what `join` takes.
    pub handle: String,
    pub kind: ChildKind,
    pub name: String,
    /// The child's own run id (`sub_<ULID>` for a subagent, `run_<ULID>`
    /// otherwise).
    pub run_id: String,
    pub codename: Option<String>,
    /// The child's address on its flow's board, inside an agentiflow.
    pub address: Option<String>,
    /// The child's transcript (an agent's; a workflow has one per step).
    pub transcript: Option<PathBuf>,
}

/// Where a child is in its life.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildState {
    /// Started; no evidence yet that its run began (a process child), or
    /// waiting for a concurrency slot.
    Pending,
    Running,
    /// Ended with an answer, which may itself report `success: false`.
    Done,
    /// Ended without an answer: it could not start, crashed or was killed.
    Failed,
    /// Its parent ended without joining it; it was stopped.
    Abandoned,
    /// Its parent paused, crashed or was stopped by a signal, so it was
    /// stopped (or died with it); the run that continues the parent
    /// continues it under the same handle.
    Interrupted,
}

impl ChildState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChildState::Pending => "pending",
            ChildState::Running => "running",
            ChildState::Done => "done",
            ChildState::Failed => "failed",
            ChildState::Abandoned => "abandoned",
            ChildState::Interrupted => "interrupted",
        }
    }

    /// No further transitions under this parent run. An `Interrupted`
    /// child goes on under the run that continues its parent.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ChildState::Done | ChildState::Failed | ChildState::Abandoned | ChildState::Interrupted
        )
    }
}

/// A child's token totals.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildTokens {
    pub input: u64,
    pub output: u64,
}

/// A child's status, as `join` and the children collector report it.
#[derive(Clone, Debug, PartialEq)]
pub struct ChildStatus {
    pub handle: String,
    pub kind: ChildKind,
    pub name: String,
    pub run_id: String,
    pub codename: Option<String>,
    pub address: Option<String>,
    pub transcript: Option<PathBuf>,
    pub state: ChildState,
    /// Set once `Done`: whether the child finished without an error.
    pub success: Option<bool>,
    /// The child's final answer (empty for a failed agent run).
    pub output: Option<String>,
    /// Why it failed, was abandoned or did not succeed.
    pub error: Option<String>,
    /// The typed outcome a failed agent run stopped on
    /// (`rupu_transcript::OutcomeRecord`, serialized).
    pub cause: Option<Value>,
    pub tokens: Option<ChildTokens>,
    pub duration_ms: Option<u64>,
    /// How long ago it started, for a running child's status line.
    pub elapsed: Duration,
}

impl ChildStatus {
    /// `join`'s entry: `{handle, kind, name, run_id, codename?, address?,
    /// transcript?, status, success?, output?, error?, cause?, tokens?,
    /// duration_ms?}`.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("handle".into(), json!(self.handle));
        m.insert("kind".into(), json!(self.kind.as_str()));
        m.insert("name".into(), json!(self.name));
        m.insert("run_id".into(), json!(self.run_id));
        if let Some(c) = &self.codename {
            m.insert("codename".into(), json!(c));
        }
        if let Some(a) = &self.address {
            m.insert("address".into(), json!(a));
        }
        if let Some(t) = &self.transcript {
            m.insert("transcript".into(), json!(t.display().to_string()));
        }
        m.insert("status".into(), json!(self.state.as_str()));
        if let Some(s) = self.success {
            m.insert("success".into(), json!(s));
        }
        if let Some(o) = &self.output {
            m.insert("output".into(), json!(o));
        }
        if let Some(e) = &self.error {
            m.insert("error".into(), json!(e));
        }
        if let Some(c) = &self.cause {
            m.insert("cause".into(), c.clone());
        }
        if let Some(t) = &self.tokens {
            m.insert(
                "tokens".into(),
                json!({ "input": t.input, "output": t.output }),
            );
        }
        if let Some(d) = self.duration_ms {
            m.insert("duration_ms".into(), json!(d));
        }
        Value::Object(m)
    }
}

/// Which children `join` waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinSet {
    /// Every child this run started that `join` has not yet returned
    /// finished.
    All,
    Handles(Vec<String>),
}

/// When `join` returns.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Until {
    /// Every child in the set is finished (or the timeout passed).
    All,
    /// At least one child in the set is finished (or the timeout passed).
    Any,
}

/// How the parent run ended, recorded on the children it abandons.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParentEnd {
    Completed,
    Failed,
    /// An agentiflow wound down.
    Stopped,
    /// The run paused (a workflow step at a safe boundary), or its process
    /// is being stopped by a signal: its children are stopped and recorded
    /// `interrupted`, for the run that continues it.
    Paused,
}

impl ParentEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            ParentEnd::Completed => "completed",
            ParentEnd::Failed => "failed",
            ParentEnd::Stopped => "stopped",
            ParentEnd::Paused => "paused",
        }
    }
}

/// A launch that was refused or could not start. `message` is what the
/// model reads; `code` is its stable prefix (`agent_not_dispatchable`,
/// `max_dispatch_depth_exceeded`, …).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct LaunchError {
    pub code: &'static str,
    pub message: String,
}

impl LaunchError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Starts and watches a run's children: the one launcher behind `dispatch`
/// and `join`, for every [`ChildKind`] (spec W7 §3.3). Implemented by
/// `rupu-launch`'s `RunLauncher`.
#[async_trait]
pub trait Launcher: Send + Sync {
    /// Check `req` against the caller's policy (allowlist, depth, the
    /// workflow's agents and inputs) without starting anything.
    async fn check(&self, req: &ChildRequest, from: &ToolContext) -> Result<(), LaunchError>;

    /// Start `req` as a child of the run `from` describes. Returns as soon as
    /// the child is started; it runs on its own.
    async fn launch(
        &self,
        req: ChildRequest,
        from: &ToolContext,
    ) -> Result<ChildHandle, LaunchError>;

    /// Wait up to `timeout` for `which` of `from`'s children to finish
    /// (`until`), then report each one's status, in the order asked (every
    /// child: launch order). An unknown handle reports `Failed`.
    async fn join(
        &self,
        from: &RunIdentity,
        which: JoinSet,
        timeout: Duration,
        until: Until,
    ) -> Vec<ChildStatus>;

    /// `from` ended: stop every child it did not join and record each one
    /// `abandoned`. Called by the agent loop when a run ends.
    async fn finish(&self, from: &RunIdentity, how: ParentEnd);

    /// `from`'s children as they stand now, launch order. Cheap and
    /// non-blocking: the children collector calls it before every turn.
    fn children(&self, from: &RunIdentity) -> Vec<ChildStatus>;

    /// `to` continues the conversation of `prior_run`, an earlier attempt
    /// that paused, crashed or was stopped by a signal: before `to`'s first
    /// turn, take on every child `prior_run` was given, under the same
    /// handles, closing `prior_run`'s record of each. One that ended (on its
    /// own records) is restored as it ended; one that never started is
    /// launched afresh; the rest are continued from their own records.
    async fn continue_children(&self, _prior_run: &str, _to: &ToolContext) {}

    /// `prior_run`'s attempt was started over instead of continued: record
    /// every child it left unfinished or interrupted `abandoned` with `why`,
    /// stopping any still running.
    async fn abandon_children(&self, _prior_run: &str, _why: &str) {}

    /// The workflow run `workflow_run` ended for good: record the children
    /// its attempts left `interrupted` (none of its steps continued them)
    /// `abandoned`.
    async fn end_interrupted(&self, _workflow_run: &str) {}
}

/// A workflow `workflows.generate` authored and saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedWorkflow {
    /// Where it was saved: pass it to `dispatch {kind: workflow, file}`.
    pub file: PathBuf,
    pub name: String,
    pub steps: usize,
    /// Its declared inputs, sorted.
    pub input_keys: Vec<String>,
}

/// Authors a new workflow from a description with a model and saves it under
/// the run's directory (`workflows.generate`).
#[async_trait]
pub trait WorkflowGenerator: Send + Sync {
    async fn generate(
        &self,
        description: &str,
        from: &ToolContext,
    ) -> Result<GeneratedWorkflow, LaunchError>;
}
