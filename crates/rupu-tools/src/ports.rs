//! Ports: the effects a catalog tool needs from a runtime that sits above
//! this crate (D1/D2). A tool calls a store crate directly when the store
//! is a plain local-file library (`rupu-coverage`, `rupu-fleet`,
//! `rupu-scm`); it calls a port when the effect needs something that
//! depends on this crate — loading agent and workflow files
//! ([`CatalogPort`], adapted by `rupu-orchestrator`'s `FsCatalog`) or a
//! flow's live status ([`RunStatus`], implemented by `rupu-agentiflow`).
//!
//! The report types are the JSON shapes the tools return; each `to_json`
//! builds exactly that shape, so the model sees the same output whoever
//! implements the port.

use serde_json::{json, Map, Value};

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
