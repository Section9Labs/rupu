//! The agentiflow entry point: [`run_agentiflow`] validates a definition,
//! lays out a run directory, wires the envelope to a `run_agent`-backed lead,
//! runs it to a stop, and persists the result.
//!
//! # Run-directory layout
//!
//! ```text
//! <global>/agentiflows/<id>/
//!   agentiflow.json            AgentiflowRecord, rewritten atomically (tmp + rename)
//!   agentiflow.yaml            the definition this run was started from (written once, atomically)
//!   events.jsonl               append-only event log (shape below)
//!   lead/transcript.r<N>.jsonl the lead's transcript for round N
//!   steering/*.json            the operator queue (OperatorQueue rooted at the run dir)
//!   board/                     the coordination board (posts, claims, directives)
//!   mailboxes/<participant>/   per-participant inboxes
//! ```
//!
//! `board/` and `mailboxes/` are created lazily by the stores on first write.
//! The lead reaches them through always-on `board.*` / `msg.send` tools and
//! reads them back each turn through the inbox / standing-directive collectors.
//!
//! The lead's configured base transcript path is `lead/transcript.jsonl`; the
//! driver writes one file per round beside it (`transcript.r0.jsonl`, ...)
//! because the runner truncates a transcript when a run starts.
//!
//! # Pooled evidence scope
//!
//! All evidence this run produces (findings, assets, coverage) pools under ONE
//! coverage target inside the workspace:
//! `CoveragePaths::new(&workspace, &target_id(&workspace, &id))`, i.e.
//! `<workspace>/.rupu/coverage/<target_id>/`. The agentiflow id is the scope
//! name, so two runs never share a target, and the envelope's goal / coverage
//! evaluation reads exactly the evidence this run's lead and units write.
//!
//! # `events.jsonl`
//!
//! One JSON object per line, appended as things happen. Every line has `ts`
//! (RFC 3339, from the run's injected clock) and `kind`; the rest depends on
//! the kind:
//!
//! - `run_started`: `id`, `name`, `goals` (count), `engagement_profiles`
//! - `round`: written when a lead round FINISHES. `round` (zero-based index),
//!   `budget` (`ok` / `soft` / `hard:<dimension>`), `converge` (bool),
//!   `goals_met` / `goals_total` (as the round began), `steering` (messages
//!   delivered), `outcome` (`yielded` / `turn_budget_hit` / `error`), and
//!   `error` when the outcome is `error`
//! - `run_stopped`: `stop_reason` (a stable code, see [`AgentiflowRecord`]),
//!   `detail` (human text), `rounds`, `goals` (`id` / `met` / `current` /
//!   `target`), `summary`
//!
//! The log is deliberately local and minimal: a best-effort append. A failed
//! append is logged (`tracing::warn!`) and never stops the run; the durable
//! state of record is `agentiflow.json`.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use rupu_coverage::{target_id, ActiveSet, CoveragePaths};
use rupu_providers::model_limits::ModelLimits;
use rupu_runtime::RunTriggerSource;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::budget::{parse_duration, BudgetStage, UsageSource};
use crate::def::{AgentiflowDef, WorkflowsSpec};
use crate::dispatch_tools::{fleet_unit_tools, WorkflowToolCtx};
use crate::envelope::{
    Envelope, EnvelopeConfig, EnvelopeOutcome, LeadDriver, RoundContext, RoundOutcome, StopReason,
};
use crate::error::AgentiflowError;
use crate::lead::{LeadConfig, ProviderFactory, RunAgentLeadDriver};
use crate::operator::OperatorQueue;
use crate::roster::{roster_collector, roster_tools, RosterCtx};
use crate::status_tools::status_tools;
use crate::subprocess::SubprocessUnitLauncher;
use crate::supervisor::FleetSupervisor;
use crate::unit::UnitLauncher;

/// `agentiflow.json`, the durable record of one run.
const RECORD_FILE: &str = "agentiflow.json";
/// `agentiflow.yaml`, the snapshot of the definition a run was started from.
/// It is the re-serialized parsed definition (comments and key order are not
/// preserved), so a run stays inspectable even if the on-disk definition is
/// later edited or removed.
const DEF_FILE: &str = "agentiflow.yaml";
const EVENTS_FILE: &str = "events.jsonl";
/// Turns the lead may take per round when the definition does not say.
const DEFAULT_LEAD_MAX_TURNS: u32 = 50;

/// One goal's status as of the last evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalStatus {
    pub id: String,
    pub met: bool,
    pub current: u64,
    pub target: u64,
}

/// The persisted record of an agentiflow run (`<run dir>/agentiflow.json`).
///
/// `status` is `running` from the moment the run directory is laid out,
/// `completed` once the envelope reached a stop (whatever the reason: the
/// reason is `stop_reason`), or `failed` if the run could not start.
///
/// `stop_reason` is a stable snake_case code: `goals_met`,
/// `coverage_reached`, `budget_exhausted:<dimension>`, `operator_stop`,
/// `ceiling`; for a `failed` run it is `error: <message>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentiflowRecord {
    pub id: String,
    pub name: String,
    pub engagement_profiles: Vec<String>,
    pub trigger: RunTriggerSource,
    pub status: String,
    pub stop_reason: Option<String>,
    pub rounds: u32,
    pub goals: Vec<GoalStatus>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// Human codename; minted by the daemon (Plan 4), `None` before then.
    pub codename: Option<String>,
}

impl AgentiflowRecord {
    /// Atomically write this record to `<run_dir>/agentiflow.json`: the JSON
    /// goes to a sibling temp file which is then renamed over the target, so
    /// a reader sees the old record or the new one, never a torn write.
    pub fn write(&self, run_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(run_dir)?;
        let tmp = run_dir.join(format!(".{RECORD_FILE}.tmp"));
        let mut bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, run_dir.join(RECORD_FILE)).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Read `<run_dir>/agentiflow.json` back.
    pub fn read(run_dir: &Path) -> std::io::Result<Self> {
        let raw = std::fs::read(run_dir.join(RECORD_FILE))?;
        serde_json::from_slice(&raw).map_err(std::io::Error::other)
    }
}

/// Atomically write the definition snapshot `<run_dir>/agentiflow.yaml`: the
/// YAML goes to a sibling temp file which is then renamed over the target, so
/// a reader sees no file or a whole one, never a torn write.
fn write_def_snapshot(run_dir: &Path, def: &AgentiflowDef) -> Result<(), AgentiflowError> {
    let yaml = serde_yaml::to_string(def)?;
    std::fs::create_dir_all(run_dir)?;
    let tmp = run_dir.join(format!(".{DEF_FILE}.tmp"));
    std::fs::write(&tmp, yaml)?;
    std::fs::rename(&tmp, run_dir.join(DEF_FILE)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AgentiflowError::Io(e)
    })
}

/// `<global>/agentiflows`: the directory holding every run's directory.
pub fn agentiflow_dir(global: &Path) -> PathBuf {
    global.join("agentiflows")
}

/// A fresh agentiflow id (`af_<ULID>`). The caller mints it so it can show
/// the operator the id (and enqueue steering) before the run starts.
pub fn new_run_id() -> String {
    format!("af_{}", ulid::Ulid::new())
}

/// The caller-resolved lead: which agent, with which prompt / provider /
/// model / tools. (Plan 4 wires the real resolver; tests hand in a mock.)
pub struct LeadInputs {
    pub agent_name: String,
    pub system_prompt: String,
    pub provider_name: String,
    pub model: String,
    /// The builtins/MCP allowlist (the runner filters its registry to this
    /// list); empty grants none of those. `report_finding` is appended by
    /// `run_agentiflow` and the board/mailbox tools are always-on `extra_tools`,
    /// so an empty list does not mean the lead is toolless.
    pub agent_tools: Vec<String>,
}

/// Everything [`run_agentiflow`] needs.
pub struct RunAgentiflowOpts {
    pub def: AgentiflowDef,
    /// The workspace the evidence is pooled in and the lead's tools run in.
    pub workspace: PathBuf,
    /// The rupu global dir; the run directory lives under it.
    pub global: PathBuf,
    /// The resolved engagement profiles (`def.resolve_profiles`).
    pub active: ActiveSet,
    /// When the run began: the zero of the budget's and ceiling's wall clock.
    pub started: DateTime<Utc>,
    /// The injected clock.
    pub now: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    pub make_provider: ProviderFactory,
    pub lead: LeadInputs,
    /// This run's id (see [`new_run_id`]). Must be a safe path component.
    pub run_id: String,
    /// How the lead's `dispatch` tool starts units. `None` is production: a
    /// [`SubprocessUnitLauncher`] over the running `rupu` binary, so each unit
    /// is a detached, process-isolated `rupu run`. A caller (a test, or the
    /// Plan-4 daemon with its own launcher) injects an implementation here.
    pub unit_launcher: Option<Arc<dyn UnitLauncher>>,
    /// The lead's workflow-authoring capability: which provider/model writes a
    /// new workflow and a factory minting a provider for each generation call.
    /// Built by the launch site exactly as `make_provider` is (wired for real in
    /// Plan 4; tests inject a mock). `None` disables `generate_workflow`
    /// entirely: the lead is not even offered the tool.
    pub generation: Option<crate::lead::GenerationCapability>,
}

/// A run id becomes a directory name and an evidence scope name, so it must
/// not be able to name anything but a direct child of `agentiflows/`.
fn validate_run_id(id: &str) -> Result<(), AgentiflowError> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(AgentiflowError::Invalid(format!(
            "run id `{id}` must be 1-128 characters of [A-Za-z0-9_-]"
        )))
    }
}

/// Whether anything but an operator `stop` can end this flow: a required
/// goal, a coverage target, a wall-clock / round cap on the budget, or a
/// round / wall-clock ceiling.
///
/// `budget.usd` / `budget.tokens` do NOT count: the usage-ledger-backed
/// [`UsageSource`] is not wired yet (see [`UnmeteredUsage`]), so those caps
/// cannot fire in this build and a flow bounded only by them is, honestly,
/// unbounded.
fn has_automatic_terminator(def: &AgentiflowDef) -> bool {
    def.goals.iter().any(|g| g.required)
        || def.coverage.is_some()
        || def
            .budget
            .as_ref()
            .is_some_and(|b| b.wall_clock.is_some() || b.rounds.is_some())
        || def
            .round
            .as_ref()
            .and_then(|r| r.ceiling.as_ref())
            .is_some_and(|c| c.rounds.is_some() || c.wall_clock.is_some())
}

/// The workflow ids the lead's roster index names. A listed pool is taken as
/// written; `workflows: all` is resolved against the catalog as it stands at
/// run start (a workflow added mid-run is not indexed, though `workflows.list`
/// still shows it).
fn pool_workflow_ids(spec: &WorkflowsSpec, ctx: &RosterCtx) -> Vec<String> {
    match spec {
        WorkflowsSpec::List(ids) => ids.clone(),
        WorkflowsSpec::All(_) => {
            rupu_orchestrator::list_workflow_summaries(&ctx.global, ctx.project.as_deref())
                .into_iter()
                .map(|w| w.id)
                .collect()
        }
    }
}

/// The usage source for this build: reports nothing spent.
///
/// The ledger-backed source (summing the lead's and the dispatched units'
/// token / cost usage) lands with real dispatch in Plan 3b. Until then
/// `budget.usd` / `budget.tokens` are NOT enforced and only the wall-clock and
/// round dimensions can stop a run; `run_agentiflow` warns when a definition
/// sets the unenforced ones.
struct UnmeteredUsage;

impl UsageSource for UnmeteredUsage {
    fn spent_usd(&self) -> f64 {
        0.0
    }
    fn spent_tokens(&self) -> u64 {
        0
    }
}

/// The lead's round-0 mission text: each goal's objective, the coverage
/// target if there is one, falling back to the definition's description.
fn mission_objective(def: &AgentiflowDef) -> String {
    let mut lines: Vec<String> = def
        .goals
        .iter()
        .map(|g| format!("- {}: {}", g.id, g.objective.trim()))
        .collect();
    if let Some(c) = &def.coverage {
        lines.push(format!(
            "- coverage: reach {:.0}% of the discovered assets{}",
            c.reach * 100.0,
            c.depth
                .as_deref()
                .map(|d| format!(" at depth `{d}` or deeper"))
                .unwrap_or_default()
        ));
    }
    if lines.is_empty() {
        return def.description.clone().unwrap_or_else(|| def.name.clone());
    }
    lines.join("\n")
}

/// A stable machine code for a stop reason (what `stop_reason` persists).
fn stop_code(stop: &StopReason) -> String {
    match stop {
        StopReason::GoalsMet => "goals_met".into(),
        StopReason::CoverageReached => "coverage_reached".into(),
        StopReason::BudgetExhausted { dimension } => format!("budget_exhausted:{dimension}"),
        StopReason::OperatorStop => "operator_stop".into(),
        StopReason::Ceiling => "ceiling".into(),
    }
}

fn goal_statuses(outcome: &EnvelopeOutcome) -> Vec<GoalStatus> {
    outcome
        .goals
        .iter()
        .map(|g| GoalStatus {
            id: g.id.clone(),
            met: g.met,
            current: g.current,
            target: g.target,
        })
        .collect()
}

/// The best-effort `events.jsonl` appender (shape in the module docs).
struct EventLog {
    path: PathBuf,
}

impl EventLog {
    fn emit(&self, ts: DateTime<Utc>, kind: &str, fields: Value) {
        let mut line = serde_json::Map::new();
        line.insert(
            "ts".into(),
            Value::String(ts.to_rfc3339_opts(SecondsFormat::Millis, true)),
        );
        line.insert("kind".into(), Value::String(kind.into()));
        if let Value::Object(extra) = fields {
            line.extend(extra);
        }
        if let Err(e) = self.append(&Value::Object(line)) {
            tracing::warn!(path = %self.path.display(), error = %e, kind, "could not append agentiflow event");
        }
    }

    fn append(&self, line: &Value) -> std::io::Result<()> {
        let mut buf = serde_json::to_vec(line).map_err(std::io::Error::other)?;
        buf.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        // One `write_all` of the whole line: an appended write this small is
        // not interleaved with another writer's.
        f.write_all(&buf)
    }
}

fn budget_label(b: &BudgetStage) -> String {
    match b {
        BudgetStage::Ok => "ok".into(),
        BudgetStage::Soft => "soft".into(),
        BudgetStage::Hard { dimension } => format!("hard:{dimension}"),
    }
}

/// Wraps the real lead driver to write a `round` event as each round ends.
/// The envelope owns the round loop and has no hook of its own, so the event
/// is recorded at the one seam every round passes through.
struct RecordingLead<'a> {
    inner: RunAgentLeadDriver,
    events: &'a EventLog,
    now: &'a (dyn Fn() -> DateTime<Utc> + Send + Sync),
}

impl LeadDriver for RecordingLead<'_> {
    fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome {
        let out = self.inner.run_round(ctx);
        let d = &ctx.digest;
        let (outcome, error) = match &out {
            RoundOutcome::Yielded => ("yielded", None),
            RoundOutcome::TurnBudgetHit => ("turn_budget_hit", None),
            RoundOutcome::Error(e) => ("error", Some(e.as_str())),
        };
        let mut fields = json!({
            "round": ctx.round,
            "budget": budget_label(&d.budget),
            "converge": d.converge,
            "goals_met": d.goals.iter().filter(|g| g.met).count(),
            "goals_total": d.goals.len(),
            "steering": d.steering.len(),
            "outcome": outcome,
        });
        if let (Some(e), Some(obj)) = (error, fields.as_object_mut()) {
            obj.insert("error".into(), Value::String(e.to_string()));
        }
        self.events.emit((self.now)(), "round", fields);
        out
    }
}

/// Run an agentiflow to a stop and persist the result.
///
/// 1. Validates `def` against `active` (fail closed) and the run id, and
///    rejects an unparseable `round.ceiling.wall_clock` or a zero
///    `round.lead_max_turns`, all before touching the filesystem.
/// 2. Warns (`tracing::warn!`, never rejects) when nothing but an operator
///    `stop` can end the flow, and when `budget.usd` / `budget.tokens` are set
///    but not yet enforced. Operator-only is a valid, explicit mode.
/// 3. Lays out `<global>/agentiflows/<run_id>/` (see the module docs), writes
///    a `running` [`AgentiflowRecord`], and runs the envelope over the pooled
///    evidence scope with a [`RunAgentLeadDriver`] lead.
/// 4. On a stop, writes the final record (`completed`, with the stop reason,
///    round count and goal statuses) and returns the [`EnvelopeOutcome`].
///
/// # Threading
///
/// The lead driver owns a tokio runtime and `block_on`s it each round, so the
/// flow must not run on a thread that already has an ambient tokio runtime
/// (that panics, and so does dropping the driver's runtime there).
/// `run_agentiflow` therefore runs the whole flow on a dedicated OS thread and
/// blocks the caller until it finishes, which makes it safe to call from any
/// context: a plain thread, a sync fn, or `tokio::task::spawn_blocking`.
/// Because it blocks the calling thread for the life of the flow, an async
/// caller should invoke it from `spawn_blocking`, not inline on an executor
/// thread. A panic on the worker thread is re-raised on the caller.
///
/// # Budget in this build
///
/// Usage is not metered yet ([`UnmeteredUsage`]): `budget.usd` and
/// `budget.tokens` never trip. Wall-clock and round caps work.
pub fn run_agentiflow(opts: RunAgentiflowOpts) -> Result<EnvelopeOutcome, AgentiflowError> {
    let RunAgentiflowOpts {
        def,
        workspace,
        global,
        active,
        started,
        now,
        make_provider,
        lead,
        run_id: id,
        unit_launcher,
        generation,
    } = opts;

    // A thread-scoped tracing subscriber (a test's, say) does not follow the
    // flow to its worker thread, so carry the caller's dispatcher over.
    let dispatch = tracing::dispatcher::get_default(|d| d.clone());
    std::thread::scope(|s| {
        s.spawn(move || -> Result<EnvelopeOutcome, AgentiflowError> {
            let _log_scope = tracing::dispatcher::set_default(&dispatch);

            // ---- everything that can be rejected without touching the disk ----------
            def.validate(&active)?;
            validate_run_id(&id)?;

            let ceiling = def.round.as_ref().and_then(|r| r.ceiling.as_ref());
            let ceiling_wall_clock = match ceiling.and_then(|c| c.wall_clock.as_deref()) {
                Some(w) => Some(parse_duration(w).map_err(|e| {
                    AgentiflowError::Invalid(format!("round.ceiling.wall_clock: {e}"))
                })?),
                None => None,
            };
            let ceiling_rounds = ceiling.and_then(|c| c.rounds);
            let per_round_max_turns = def
                .round
                .as_ref()
                .and_then(|r| r.lead_max_turns)
                .unwrap_or(DEFAULT_LEAD_MAX_TURNS);
            if per_round_max_turns == 0 {
                return Err(AgentiflowError::Invalid(
                    "round.lead_max_turns must be at least 1".into(),
                ));
            }

            if !has_automatic_terminator(&def) {
                tracing::warn!(
                    name = %def.name,
                    run_id = %id,
                    "agentiflow has no automatic terminator; it will run until an operator stop"
                );
            }
            if def
                .budget
                .as_ref()
                .is_some_and(|b| b.usd.is_some() || b.tokens.is_some())
            {
                tracing::warn!(
                    name = %def.name,
                    run_id = %id,
                    "budget.usd / budget.tokens are not enforced yet (usage is not metered); \
                     only budget.wall_clock / budget.rounds can stop this run"
                );
            }

            // ---- run directory ------------------------------------------------------
            let run_dir = agentiflow_dir(&global).join(&id);
            if run_dir.join(RECORD_FILE).exists() {
                return Err(AgentiflowError::Invalid(format!(
                    "agentiflow run `{id}` already exists at {}",
                    run_dir.display()
                )));
            }
            // `steering/` may already exist: an operator can queue a message for a run
            // they were told the id of before it started.
            std::fs::create_dir_all(run_dir.join("lead"))?;

            // The definition snapshot lands before the record, so any run that
            // has an `agentiflow.json` also has the `agentiflow.yaml` it started from.
            write_def_snapshot(&run_dir, &def)?;

            let mut record = AgentiflowRecord {
                id: id.clone(),
                name: def.name.clone(),
                engagement_profiles: def.engagement_profiles.clone(),
                trigger: RunTriggerSource::Agentiflow,
                status: "running".into(),
                stop_reason: None,
                rounds: 0,
                goals: Vec::new(),
                started_at: started,
                ended_at: None,
                codename: None,
            };
            record.write(&run_dir)?;

            // From here a failure must not leave the record claiming `running`.
            let fail = |record: &mut AgentiflowRecord, e: std::io::Error| -> AgentiflowError {
                record.status = "failed".into();
                record.stop_reason = Some(format!("error: {e}"));
                record.ended_at = Some(now());
                if let Err(werr) = record.write(&run_dir) {
                    tracing::error!(error = %werr, "could not record the failed agentiflow run");
                }
                AgentiflowError::Io(e)
            };

            // ---- envelope and lead ----------------------------------------------------
            let paths = CoveragePaths::new(&workspace, &target_id(&workspace, &id));
            // The lead records findings against the same engagement the envelope
            // evaluates, so hand it its own shared copy before `active` moves.
            let findings_engagement = Arc::new(active.clone());
            let cfg = EnvelopeConfig {
                goals: def.goals.clone(),
                coverage: def.coverage.clone(),
                ceiling_rounds,
                ceiling_wall_clock,
            };
            // The lead's status tools evaluate the same pooled scope the envelope
            // does, so keep a copy of the paths before the envelope takes them.
            let status_paths = paths.clone();
            let mut envelope = Envelope::new(
                paths,
                active,
                cfg,
                def.budget.clone().unwrap_or_default(),
                OperatorQueue::new(&run_dir),
                started,
            );

            // The lead records findings through `report_finding`; the runner only
            // registers it for an agent that lists it. Dedup so a caller that
            // already granted it does not get it twice.
            let mut agent_tools = lead.agent_tools;
            if !agent_tools.iter().any(|t| t == "report_finding") {
                agent_tools.push("report_finding".to_string());
            }
            // The lead's coordination substrate: a file-backed board and mailboxes
            // under the run dir, the tools that act on them, and the collectors that
            // fold its inbox and standing directives into each turn. "lead" is the
            // lead's participant id (the `to: "lead"` inbox other participants address).
            let board = Arc::new(rupu_fleet::Board::new(run_dir.clone()));
            let mailbox = Arc::new(rupu_fleet::Mailbox::new(run_dir.clone()));
            let fleet_ctx = Arc::new(crate::tools::FleetToolCtx::new(
                board.clone(),
                mailbox.clone(),
                "lead",
            ));
            let mut extra_tools = crate::tools::fleet_tools(fleet_ctx);

            // Units: the lead's `dispatch` / `join` start and await pool agents
            // as process-isolated `rupu run`s through one supervisor, which also
            // winds them down when the run stops (below). Production spawns real
            // subprocesses; a caller-supplied launcher replaces them.
            let launcher: Arc<dyn UnitLauncher> = match unit_launcher {
                Some(l) => l,
                None => match std::env::current_exe() {
                    Ok(exe) => Arc::new(
                        SubprocessUnitLauncher::new(exe, global.clone())
                            .with_workspace(workspace.clone()),
                    ),
                    Err(e) => return Err(fail(&mut record, e)),
                },
            };
            let sup = Arc::new(FleetSupervisor::new(launcher, run_dir.clone()));
            // Where the catalog lives: the roster tools read it, and
            // `run_workflow` loads and vets the workflow it is asked to start
            // from it. The pool's workflow ids are resolved ONCE against it, so
            // the roster index and `run_workflow`'s allowlist agree.
            let roster_ctx = Arc::new(RosterCtx {
                global: global.clone(),
                project: Some(workspace.join(".rupu")),
            });
            let pool_workflows = pool_workflow_ids(&def.pool.workflows, &roster_ctx);
            // `dispatch` / `join` / `run_workflow`: one participant counter, so a
            // pool agent and a pool workflow of the same name never share a
            // `<name>#<n>`.
            extra_tools.extend(fleet_unit_tools(
                sup.clone(),
                Arc::new(def.pool.agents.clone()),
                def.engagement_profiles.clone(),
                WorkflowToolCtx {
                    global: global.clone(),
                    project: roster_ctx.project.clone(),
                    pool_workflows: pool_workflows.clone(),
                },
                run_dir.clone(),
                generation,
            ));

            // Roster awareness + steering. The roster tools read the agent and
            // workflow catalog (global + the workspace's `.rupu`); the status tools
            // re-run the envelope's own evaluators over the pooled scope and let
            // the lead write a standing board directive. All appended to the lead's
            // always-on tools, never replacing the fleet / dispatch tools above.
            extra_tools.extend(roster_tools(roster_ctx.clone()));
            extra_tools.extend(status_tools(
                def.goals.clone(),
                def.coverage.clone(),
                status_paths,
                findings_engagement.clone(),
                board.clone(),
            ));

            let mut collectors = crate::collectors::lead_collectors(mailbox, board, "lead");
            // The lead's ambient index of what it can dispatch: its pool, resolved
            // against the catalog.
            collectors.push(roster_collector(
                def.pool.agents.clone(),
                pool_workflows,
                roster_ctx,
            ));
            let lead_cfg = LeadConfig {
                agent_name: lead.agent_name,
                system_prompt: lead.system_prompt,
                provider_name: lead.provider_name,
                model: lead.model,
                per_round_max_turns,
                run_id: id.clone(),
                transcript_path: run_dir.join("lead").join("transcript.jsonl"),
                objective: mission_objective(&def),
                goals: def.goals.clone(),
                workspace_id: format!("ws_{}", target_id(&workspace, "workspace")),
                workspace_path: workspace.clone(),
                agent_tools,
                // Real limit resolution is Plan 4's; unknown limits are the safe start.
                limits: ModelLimits::unknown(),
                // Pool the lead's findings and assets under the run id: the same
                // `target_id(workspace, id)` the envelope's goal evaluator reads.
                scope_name: Some(id.clone()),
                findings_engagement: Some(findings_engagement),
                extra_tools,
                collectors,
            };
            let driver = match RunAgentLeadDriver::new(lead_cfg, make_provider) {
                Ok(d) => d,
                Err(e) => return Err(fail(&mut record, e)),
            };

            let events = EventLog {
                path: run_dir.join(EVENTS_FILE),
            };
            let mut lead = RecordingLead {
                inner: driver,
                events: &events,
                now: &*now,
            };

            events.emit(
                now(),
                "run_started",
                json!({
                    "id": id,
                    "name": def.name,
                    "goals": def.goals.len(),
                    "engagement_profiles": def.engagement_profiles,
                }),
            );
            let outcome = envelope.run(&mut lead, &UnmeteredUsage, &*now);
            // The run is over: SIGTERM every unit still alive so a stop (a
            // goal met, the ceiling, an operator stop) never leaves orphans.
            sup.terminate_all();
            // Drop the driver here, in the blocking context its runtime ran in, before
            // anything else can return early.
            drop(lead);
            let ended = now();
            let goals = goal_statuses(&outcome);
            events.emit(
                ended,
                "run_stopped",
                json!({
                    "stop_reason": stop_code(&outcome.stop),
                    "detail": outcome.stop.to_string(),
                    "rounds": outcome.rounds,
                    "goals": goals,
                    "summary": outcome.summary,
                }),
            );

            record.status = "completed".into();
            record.stop_reason = Some(stop_code(&outcome.stop));
            record.rounds = outcome.rounds;
            record.goals = goals;
            record.ended_at = Some(ended);
            if let Err(e) = record.write(&run_dir) {
                // The run genuinely finished: its outcome is returned below and
                // the `run_stopped` event is already on disk. Only this summary
                // record could not be rewritten, so it may still read `running`
                // until the orphan sweep reconciles it — don't turn a completed
                // run into an error.
                tracing::error!(
                    id = %id,
                    stop = %outcome.stop,
                    error = %e,
                    "agentiflow reached a stop but its final record could not be rewritten; \
                     returning the outcome anyway (record may remain `running` on disk)"
                );
            }
            Ok(outcome)
        })
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator::OperatorMessage;
    use crate::verify_fixtures::full_finding;
    use rupu_agent::runner::CapturingMockProvider;
    use rupu_agent::{MockProvider, ScriptedTurn};
    use rupu_coverage::tools::{verify_finding, VerifyError, VerifyInput};
    use rupu_coverage::{
        append_record, read_findings, Attribution, FindingEvidence, FindingProfile, FindingRecord,
        FindingScope, Ledger, Severity, Surface, Verification, VerificationStatus,
    };

    fn started() -> DateTime<Utc> {
        "2026-10-05T12:00:00Z".parse().unwrap()
    }

    /// A one-goal definition: "record at least one finding" (any
    /// classification, unverified). Extra top-level YAML is appended.
    fn def_with(extra: &str) -> AgentiflowDef {
        let yaml = format!(
            "name: itest\n\
             lead: lead\n\
             engagement_profiles: [network]\n\
             goals:\n  \
               - id: any-finding\n    \
                 objective: \"Record at least one finding.\"\n    \
                 target: {{ findings: {{}}, count_gte: 1 }}\n\
             scope: {{ authorized: true }}\n\
             pool: {{ agents: [lead] }}\n\
             {extra}"
        );
        AgentiflowDef::parse_str(&yaml).unwrap()
    }

    fn active() -> ActiveSet {
        rupu_coverage::builtin_registry()
            .unwrap()
            .active_set(&["network".to_string()])
            .unwrap()
    }

    /// A factory handing out a fresh one-turn `MockProvider` per round.
    fn one_turn_factory() -> ProviderFactory {
        Box::new(|| -> Box<dyn rupu_providers::LlmProvider> {
            Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: "Round done.".into(),
                stop: rupu_agent::StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]))
        })
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        global: PathBuf,
        workspace: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        Fixture {
            _tmp: tmp,
            global,
            workspace,
        }
    }

    fn opts(fx: &Fixture, def: AgentiflowDef, id: &str) -> RunAgentiflowOpts {
        RunAgentiflowOpts {
            def,
            workspace: fx.workspace.clone(),
            global: fx.global.clone(),
            active: active(),
            started: started(),
            // A frozen clock: wall-clock caps never trip in these tests.
            now: Box::new(started),
            make_provider: one_turn_factory(),
            lead: LeadInputs {
                agent_name: "lead".into(),
                system_prompt: "You are the lead.".into(),
                provider_name: "mock".into(),
                model: "mock-1".into(),
                agent_tools: vec![],
            },
            run_id: id.into(),
            unit_launcher: None,
            generation: None,
        }
    }

    fn run_dir(fx: &Fixture, id: &str) -> PathBuf {
        agentiflow_dir(&fx.global).join(id)
    }

    fn pooled_paths(fx: &Fixture, id: &str) -> CoveragePaths {
        CoveragePaths::new(&fx.workspace, &target_id(&fx.workspace, id))
    }

    fn events(fx: &Fixture, id: &str) -> Vec<Value> {
        std::fs::read_to_string(run_dir(fx, id).join(EVENTS_FILE))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn kinds(evs: &[Value]) -> Vec<&str> {
        evs.iter().map(|e| e["kind"].as_str().unwrap()).collect()
    }

    /// Seed one (Summary-profile) finding into the run's pooled ledger.
    fn seed_finding(fx: &Fixture, id: &str) {
        let rec = FindingRecord {
            id: "fnd_seed".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: "seeded".into(),
            severity: Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_seed".into(),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc::now(),
            profile: FindingProfile::Summary,
            report: None,
        };
        append_record(&pooled_paths(fx, id), Ledger::Findings, &rec).unwrap();
    }

    // ---- AgentiflowRecord ---------------------------------------------------

    fn sample_record() -> AgentiflowRecord {
        AgentiflowRecord {
            id: "af_x".into(),
            name: "itest".into(),
            engagement_profiles: vec!["network".into(), "web".into()],
            trigger: RunTriggerSource::Agentiflow,
            status: "completed".into(),
            stop_reason: Some("goals_met".into()),
            rounds: 3,
            goals: vec![GoalStatus {
                id: "g".into(),
                met: true,
                current: 10,
                target: 10,
            }],
            started_at: started(),
            ended_at: Some(started() + chrono::Duration::minutes(5)),
            codename: None,
        }
    }

    #[test]
    fn record_round_trips_through_agentiflow_json() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("run");
        let rec = sample_record();
        rec.write(&run).unwrap();
        assert!(run.join(RECORD_FILE).is_file());
        assert_eq!(AgentiflowRecord::read(&run).unwrap(), rec);
        // The trigger is persisted in its snake_case wire form.
        let raw = std::fs::read_to_string(run.join(RECORD_FILE)).unwrap();
        assert!(raw.contains("\"agentiflow\""), "{raw}");
    }

    #[test]
    fn record_write_is_atomic_and_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let run = tmp.path().join("run");
        let mut rec = sample_record();
        rec.write(&run).unwrap();
        rec.status = "running".into();
        rec.rounds = 9;
        rec.write(&run).unwrap();
        assert_eq!(AgentiflowRecord::read(&run).unwrap().rounds, 9);
        // The temp file was renamed away, not left behind.
        let names: Vec<_> = std::fs::read_dir(&run)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [RECORD_FILE], "{names:?}");
    }

    #[test]
    fn new_run_id_is_a_safe_unique_path_component() {
        let a = new_run_id();
        let b = new_run_id();
        assert_ne!(a, b);
        assert!(a.starts_with("af_"), "{a}");
        assert!(validate_run_id(&a).is_ok());
    }

    #[test]
    fn run_ids_that_could_escape_the_run_dir_are_rejected() {
        for bad in ["", "../x", "a/b", "a\\b", ".", "..", "a b", "x\0y"] {
            assert!(validate_run_id(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(validate_run_id(&"a".repeat(129)).is_err());
        assert!(validate_run_id("af_01J-test_1").is_ok());
    }

    // ---- terminator predicate ----------------------------------------------

    #[test]
    fn terminator_predicate_counts_only_effective_stops() {
        // A required goal terminates.
        assert!(has_automatic_terminator(&def_with("")));
        // No required goal, nothing else: operator-only.
        let with = operator_only;
        assert!(!has_automatic_terminator(&with("")));
        assert!(has_automatic_terminator(&with(
            "round: { ceiling: { rounds: 3 } }"
        )));
        assert!(has_automatic_terminator(&with(
            "round: { ceiling: { wall_clock: 1h } }"
        )));
        assert!(has_automatic_terminator(&with("budget: { rounds: 5 }")));
        assert!(has_automatic_terminator(&with(
            "budget: { wall_clock: 2h }"
        )));
        assert!(has_automatic_terminator(&with("coverage: { reach: 0.5 }")));
        // An empty ceiling / budget block caps nothing.
        assert!(!has_automatic_terminator(&with("round: { ceiling: {} }")));
        assert!(!has_automatic_terminator(&with("budget: {}")));
        // usd / tokens caps are not enforced yet (no usage ledger in 3a), so
        // they are not a terminator either.
        assert!(!has_automatic_terminator(&with(
            "budget: { usd: 5.0, tokens: 1000 }"
        )));
    }

    // ---- run_agentiflow, end to end ----------------------------------------

    #[test]
    fn goals_met_before_round_zero_stops_without_running_the_lead() {
        let fx = fixture();
        let id = "af_goalsmet";
        seed_finding(&fx, id);

        let out = run_agentiflow(opts(&fx, def_with(""), id)).unwrap();
        assert_eq!(out.stop, StopReason::GoalsMet);
        assert_eq!(out.rounds, 0);

        let rec = AgentiflowRecord::read(&run_dir(&fx, id)).unwrap();
        assert_eq!(rec.id, id);
        assert_eq!(rec.name, "itest");
        assert_eq!(rec.status, "completed");
        assert_eq!(rec.stop_reason.as_deref(), Some("goals_met"));
        assert_eq!(rec.rounds, 0);
        assert_eq!(rec.trigger, RunTriggerSource::Agentiflow);
        assert_eq!(rec.engagement_profiles, ["network"]);
        assert_eq!(rec.started_at, started());
        assert_eq!(rec.ended_at, Some(started()));
        assert_eq!(
            rec.goals,
            [GoalStatus {
                id: "any-finding".into(),
                met: true,
                current: 1,
                target: 1
            }]
        );

        let evs = events(&fx, id);
        assert_eq!(kinds(&evs), ["run_started", "run_stopped"], "{evs:?}");
        assert_eq!(evs[0]["id"], id);
        assert_eq!(evs[1]["stop_reason"], "goals_met");
        assert_eq!(evs[1]["rounds"], 0);
        assert!(evs[0]["ts"]
            .as_str()
            .unwrap()
            .starts_with("2026-10-05T12:00:00"));

        // The lead's directory exists, but it never ran a round.
        let lead = run_dir(&fx, id).join("lead");
        assert!(lead.is_dir());
        assert_eq!(std::fs::read_dir(&lead).unwrap().count(), 0);
    }

    #[test]
    fn a_run_snapshots_its_definition_as_agentiflow_yaml() {
        let fx = fixture();
        let id = "af_snapshot";
        let def = def_with(
            "description: Snapshot me.\n\
             coverage: { reach: 0.5, depth: tested }\n\
             budget: { wall_clock: 2h, rounds: 9, soft_at: 0.8 }\n\
             round: { lead_max_turns: 3, ceiling: { rounds: 1 } }\n\
             trigger: manual\n",
        );
        let expected = serde_yaml::to_string(&def).unwrap();

        run_agentiflow(opts(&fx, def, id)).unwrap();

        let dir = run_dir(&fx, id);
        let snap = std::fs::read_to_string(dir.join("agentiflow.yaml")).unwrap();
        // No half-written temp file is left behind.
        assert!(!dir.join(".agentiflow.yaml.tmp").exists());
        let back = AgentiflowDef::parse_str(&snap).unwrap();
        assert_eq!(back.name, "itest");
        assert_eq!(back.description.as_deref(), Some("Snapshot me."));
        assert_eq!(back.lead, "lead");
        assert_eq!(back.engagement_profiles, ["network"]);
        assert_eq!(back.goals.len(), 1);
        assert_eq!(back.goals[0].id, "any-finding");
        assert_eq!(back.goals[0].target.count_gte, Some(1));
        assert!(back.scope.authorized);
        assert_eq!(back.pool.agents, ["lead"]);
        assert_eq!(back.coverage.as_ref().unwrap().reach, 0.5);
        assert_eq!(
            back.coverage.as_ref().unwrap().depth.as_deref(),
            Some("tested")
        );
        let budget = back.budget.as_ref().unwrap();
        assert_eq!(budget.wall_clock.as_deref(), Some("2h"));
        assert_eq!(budget.rounds, Some(9));
        assert_eq!(back.trigger.as_deref(), Some("manual"));
        // Every field survives: the snapshot re-serializes to the same YAML.
        assert_eq!(serde_yaml::to_string(&back).unwrap(), expected);
        assert_eq!(snap, expected);
    }

    #[test]
    fn a_rejected_run_writes_no_snapshot() {
        let fx = fixture();
        // An id that could escape the run dir is rejected before any disk write.
        let o = opts(&fx, def_with(""), "../escape");
        assert!(run_agentiflow(o).is_err());
        assert!(!agentiflow_dir(&fx.global).exists());
    }

    #[test]
    fn ceiling_runs_the_lead_for_the_capped_rounds() {
        let fx = fixture();
        let id = "af_ceiling";
        let def = def_with("round: { lead_max_turns: 3, ceiling: { rounds: 2 } }");

        let out = run_agentiflow(opts(&fx, def, id)).unwrap();
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_eq!(out.rounds, 2);

        let rec = AgentiflowRecord::read(&run_dir(&fx, id)).unwrap();
        assert_eq!(rec.status, "completed");
        assert_eq!(rec.stop_reason.as_deref(), Some("ceiling"));
        assert_eq!(rec.rounds, 2);
        assert!(!rec.goals[0].met);

        let evs = events(&fx, id);
        assert_eq!(
            kinds(&evs),
            ["run_started", "round", "round", "run_stopped"],
            "{evs:?}"
        );
        assert_eq!(evs[1]["round"], 0);
        assert_eq!(evs[2]["round"], 1);
        assert_eq!(evs[1]["outcome"], "yielded");
        assert_eq!(evs[1]["budget"], "ok");
        assert_eq!(evs[1]["goals_met"], 0);
        assert_eq!(evs[1]["goals_total"], 1);
        assert_eq!(evs[3]["stop_reason"], "ceiling");
        assert_eq!(evs[3]["rounds"], 2);

        // Each round wrote its own transcript beside the configured base path.
        let lead = run_dir(&fx, id).join("lead");
        assert!(lead.join("transcript.r0.jsonl").is_file());
        assert!(lead.join("transcript.r1.jsonl").is_file());
        assert!(!lead.join("transcript.r2.jsonl").exists());
    }

    #[test]
    fn an_operator_stop_enqueued_before_the_run_ends_it_at_round_zero() {
        let fx = fixture();
        let id = "af_opstop";
        // The operator queue is rooted at the run dir and exists before the
        // run starts (that is why the caller mints the id).
        OperatorQueue::new(run_dir(&fx, id))
            .enqueue(&OperatorMessage {
                ts: "2026-10-05T12:00:00Z".into(),
                body: "wrap up".into(),
                stop: true,
            })
            .unwrap();

        let out = run_agentiflow(opts(&fx, def_with(""), id)).unwrap();
        assert_eq!(out.stop, StopReason::OperatorStop);
        assert_eq!(out.rounds, 0);

        let rec = AgentiflowRecord::read(&run_dir(&fx, id)).unwrap();
        assert_eq!(rec.status, "completed");
        assert_eq!(rec.stop_reason.as_deref(), Some("operator_stop"));
        let evs = events(&fx, id);
        assert_eq!(kinds(&evs), ["run_started", "run_stopped"], "{evs:?}");
        // The drained message was consumed from the steering queue.
        assert!(OperatorQueue::new(run_dir(&fx, id))
            .drain()
            .unwrap()
            .is_empty());
    }

    /// Run `f` with a thread-local tracing subscriber and return what it
    /// logged. `run_agentiflow` carries the caller's dispatcher to its worker thread.
    fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, String) {
        use std::sync::{Arc, Mutex};
        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf(Arc::new(Mutex::new(Vec::new())));
        let sub = tracing_subscriber::fmt()
            .with_writer({
                let buf = buf.clone();
                move || buf.clone()
            })
            .with_ansi(false)
            .finish();
        let out = tracing::subscriber::with_default(sub, f);
        let logged = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        (out, logged)
    }

    /// One optional goal, no coverage / budget / ceiling: only an operator
    /// stop can end it. `extra` is appended (e.g. a `budget:` block).
    fn operator_only(extra: &str) -> AgentiflowDef {
        AgentiflowDef::parse_str(&format!(
            "name: opsonly\nlead: lead\nengagement_profiles: [network]\n\
             goals:\n  - id: g\n    objective: o\n    required: false\n    \
             target: {{ findings: {{}}, count_gte: 1 }}\n\
             scope: {{ authorized: true }}\npool: {{ agents: [lead] }}\n{extra}"
        ))
        .unwrap()
    }

    fn enqueue_stop(fx: &Fixture, id: &str) {
        OperatorQueue::new(run_dir(fx, id))
            .enqueue(&OperatorMessage {
                ts: "2026-10-05T12:00:00Z".into(),
                body: "stop".into(),
                stop: true,
            })
            .unwrap();
    }

    #[test]
    fn a_flow_with_no_automatic_terminator_warns_but_still_runs() {
        let fx = fixture();
        enqueue_stop(&fx, "af_noterm");
        let (out, logs) =
            capture_logs(|| run_agentiflow(opts(&fx, operator_only(""), "af_noterm")));
        // Operator-only is a valid explicit mode: warned about, never rejected.
        assert_eq!(out.unwrap().stop, StopReason::OperatorStop);
        assert!(
            logs.contains("no automatic terminator; it will run until an operator stop"),
            "{logs}"
        );

        // A flow that HAS a terminator does not warn.
        let id = "af_hasterm";
        enqueue_stop(&fx, id);
        let (out, logs) = capture_logs(|| run_agentiflow(opts(&fx, def_with(""), id)));
        out.unwrap();
        assert!(!logs.contains("no automatic terminator"), "{logs}");
    }

    #[test]
    fn unenforced_usd_and_token_caps_are_warned_about() {
        let fx = fixture();
        let id = "af_usd";
        enqueue_stop(&fx, id);
        let (out, logs) =
            capture_logs(|| run_agentiflow(opts(&fx, operator_only("budget: { usd: 5.0 }"), id)));
        out.unwrap();
        assert!(logs.contains("not enforced yet"), "{logs}");
        // ...and a usd-only budget is not mistaken for a terminator.
        assert!(logs.contains("no automatic terminator"), "{logs}");
    }

    #[test]
    fn evidence_pools_under_the_run_ids_scope() {
        // A finding seeded under ANOTHER run's scope must not satisfy this
        // run's goal: the pooled target is keyed by the agentiflow id.
        let fx = fixture();
        seed_finding(&fx, "af_other");
        let def = def_with("round: { ceiling: { rounds: 1 } }");
        let out = run_agentiflow(opts(&fx, def, "af_mine")).unwrap();
        assert_eq!(out.stop, StopReason::Ceiling);
        assert_ne!(
            pooled_paths(&fx, "af_mine").root,
            pooled_paths(&fx, "af_other").root
        );
    }

    /// A synthetic, complete full-profile report: the same fixture
    /// `rupu-coverage`'s own report tests validate.
    fn sample_report() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        )))
        .unwrap()
    }

    /// A factory whose every round makes one `report_finding` call (a
    /// complete report plus the `network:service` asset the active profile
    /// requires), then stops.
    fn report_finding_factory() -> ProviderFactory {
        Box::new(|| -> Box<dyn rupu_providers::LlmProvider> {
            Box::new(MockProvider::new(vec![
                ScriptedTurn::AssistantToolUse {
                    text: None,
                    tool_id: "t1".into(),
                    tool_name: "report_finding".into(),
                    tool_input: json!({
                        "scope": "repo",
                        "report": sample_report(),
                        "asset": {
                            "kind": "network:service",
                            "coordinates": [
                                { "t": "host", "v": "10.0.0.5" },
                                { "t": "port", "v": { "number": 22, "proto": "tcp" } }
                            ]
                        }
                    }),
                    stop: rupu_agent::StopReason::ToolUse,
                },
                ScriptedTurn::AssistantText {
                    text: "Round done.".into(),
                    stop: rupu_agent::StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                },
            ]))
        })
    }

    #[test]
    fn a_lead_finding_lands_in_the_pooled_scope() {
        // The goal evaluator reads `target_id(workspace, run id)`; the lead's
        // `report_finding` must write there, not under `target_id(workspace,
        // <lead agent name>)`, or no finding-goal could ever be met.
        let fx = fixture();
        let id = "af_finding";
        let mut o = opts(&fx, def_with("round: { ceiling: { rounds: 1 } }"), id);
        o.make_provider = report_finding_factory();
        let out = run_agentiflow(o).unwrap();

        let findings = std::fs::read_to_string(&pooled_paths(&fx, id).findings).unwrap_or_default();
        assert!(
            !findings.trim().is_empty(),
            "the lead's finding is in the pooled scope ledger"
        );
        // ...and the lead's own default scope stayed empty.
        let stray = CoveragePaths::new(&fx.workspace, &target_id(&fx.workspace, "lead"));
        assert!(
            std::fs::read_to_string(&stray.findings)
                .unwrap_or_default()
                .trim()
                .is_empty(),
            "nothing leaked into the lead's own scope"
        );
        // The evaluator saw it: the finding goal is met from the pooled ledger.
        assert_eq!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);
    }

    /// Every request a [`CapturingMockProvider`] saw, shared across the
    /// providers the factory mints (one per round).
    type Captured = Arc<std::sync::Mutex<Vec<rupu_providers::LlmRequest>>>;

    /// A factory handing out one `CapturingMockProvider` per round -- `rounds[i]`
    /// is round i's script -- all recording into the returned [`Captured`].
    fn capturing_factory(rounds: Vec<Vec<ScriptedTurn>>) -> (ProviderFactory, Captured) {
        let captured: Captured = Arc::default();
        let shared = captured.clone();
        let mut rounds = rounds.into_iter();
        let factory: ProviderFactory = Box::new(move || -> Box<dyn rupu_providers::LlmProvider> {
            let mut p = CapturingMockProvider::new(rounds.next().unwrap_or_default());
            p.captured = shared.clone();
            Box::new(p)
        });
        (factory, captured)
    }

    /// The model-visible text of a request: every text block and tool result.
    /// (`ToolUse` inputs are deliberately left out so a body the lead merely
    /// SENT is not mistaken for one that was delivered back to it.)
    fn request_texts(req: &rupu_providers::LlmRequest) -> Vec<String> {
        use rupu_providers::types::ContentBlock;
        req.messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect()
    }

    fn any_contains(texts: &[String], needle: &str) -> bool {
        texts.iter().any(|t| t.contains(needle))
    }

    fn tool_turn(id: &str, name: &str, input: Value) -> ScriptedTurn {
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: id.into(),
            tool_name: name.into(),
            tool_input: input,
            stop: rupu_agent::StopReason::ToolUse,
        }
    }

    fn done_turn() -> ScriptedTurn {
        ScriptedTurn::AssistantText {
            text: "Round done.".into(),
            stop: rupu_agent::StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        }
    }

    #[test]
    fn the_lead_posts_to_the_board_and_sees_a_standing_directive() {
        // The lead lists NO tools of its own (`agent_tools` is empty), so
        // `board.post` can only reach it as an always-on injected tool.
        let fx = fixture();
        let id = "af_board";
        // An operator directive queued on the run's board BEFORE the run starts.
        // This creates `board/` but not `agentiflow.json`, so the run is not
        // refused as already existing, and the run never wipes the directory.
        let seeded = rupu_fleet::Board::new(run_dir(&fx, id));
        seeded
            .put_directive(&rupu_fleet::Directive {
                author: "operator".into(),
                ts: "2026-10-05T11:59:00Z".into(),
                body: "never scan outside 10.0.0.0/24".into(),
                addressed_to: None,
            })
            .unwrap();

        let (factory, captured) = capturing_factory(vec![vec![
            tool_turn(
                "t1",
                "board.post",
                json!({ "kind": "note", "body": "starting on the web tier" }),
            ),
            done_turn(),
        ]]);
        let mut o = opts(&fx, def_with("round: { ceiling: { rounds: 1 } }"), id);
        o.make_provider = factory;
        let out = run_agentiflow(o).unwrap();
        assert_eq!(out.stop, StopReason::Ceiling, "{:?}", out.stop);

        // (a) The post is in the run's board, read back through a FRESH handle.
        let fresh = rupu_fleet::Board::new(run_dir(&fx, id));
        let posts = fresh.read_posts().unwrap();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(posts[0].author, "lead");
        assert_eq!(posts[0].body, "starting on the web tier");
        // The directive survived the run untouched.
        assert_eq!(fresh.read_directives().unwrap().len(), 1);
        // The spec layout: files live at `<run dir>/board/…`, not a doubled subdir.
        assert!(run_dir(&fx, id).join("board/posts.jsonl").is_file());
        assert!(run_dir(&fx, id).join("board/directives.jsonl").is_file());
        assert!(!run_dir(&fx, id).join("board/board").exists());

        // (b) The DirectiveCollector put the standing directive in front of the
        // model on its very first call. Injections are not transcript events, so
        // the request the provider received is where delivery is observable.
        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2, "one model call per scripted turn");
        let first = request_texts(&reqs[0]);
        assert!(
            any_contains(&first, "never scan outside 10.0.0.0/24")
                && any_contains(&first, "source: directive:board"),
            "the directive reaches the lead's first turn: {first:?}"
        );
        // ...and stands: it is re-asserted on the next turn too.
        assert!(any_contains(
            &request_texts(&reqs[1]),
            "never scan outside 10.0.0.0/24"
        ));
        // (c) The tool ran for real: its success result came back to the model.
        assert!(
            any_contains(&request_texts(&reqs[1]), "posted [note]"),
            "{:?}",
            request_texts(&reqs[1])
        );
    }

    #[test]
    fn the_lead_receives_mailbox_messages_and_can_send() {
        let fx = fixture();
        let id = "af_mail";
        // A worker's message already waiting in the lead's inbox.
        let seeded = rupu_fleet::Mailbox::new(run_dir(&fx, id));
        seeded
            .send(
                "lead",
                &rupu_fleet::FleetMessage {
                    from: "worker-1".into(),
                    ts: "2026-10-05T11:59:00Z".into(),
                    body: "port 22 open on 10.0.0.5".into(),
                },
                64,
            )
            .unwrap();

        // The lead then messages its own inbox via the injected `msg.send`.
        let (factory, captured) = capturing_factory(vec![vec![
            tool_turn(
                "t1",
                "msg.send",
                json!({ "to": "lead", "body": "note-to-self: check ssh next" }),
            ),
            done_turn(),
        ]]);
        let mut o = opts(&fx, def_with("round: { ceiling: { rounds: 1 } }"), id);
        o.make_provider = factory;
        let out = run_agentiflow(o).unwrap();
        assert_eq!(out.stop, StopReason::Ceiling, "{:?}", out.stop);

        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2, "one model call per scripted turn");
        let first = request_texts(&reqs[0]);
        let second = request_texts(&reqs[1]);
        // The worker's message was drained into the first turn...
        assert!(
            any_contains(&first, "port 22 open on 10.0.0.5")
                && any_contains(&first, "source: mailbox:lead"),
            "{first:?}"
        );
        assert!(
            !any_contains(&first, "note-to-self"),
            "not sent yet on turn 0"
        );
        // ...and the lead's own message came back on the turn after it sent it.
        assert!(
            any_contains(&second, "note-to-self: check ssh next"),
            "{second:?}"
        );
        // `Once` delivery: the worker's message persisted in history (not
        // re-drained), so it is still there exactly once.
        let delivered = second
            .iter()
            .filter(|t| t.contains("port 22 open on 10.0.0.5"))
            .count();
        assert_eq!(delivered, 1, "{second:?}");
        // The spec layout: inboxes live at `<run dir>/mailboxes/<participant>/`.
        assert!(run_dir(&fx, id).join("mailboxes").is_dir());
        assert!(!run_dir(&fx, id).join("mailboxes/mailboxes").exists());
        // Both inboxes are now empty.
        let fresh = rupu_fleet::Mailbox::new(run_dir(&fx, id));
        assert!(fresh.drain("lead").unwrap().is_empty());
    }

    // ---- dispatch / join over the fleet supervisor -------------------------

    /// A definition whose pool holds the lead and one worker, `recon`.
    fn def_with_recon_pool(extra: &str) -> AgentiflowDef {
        AgentiflowDef::parse_str(&format!(
            "name: itest\n\
             lead: lead\n\
             engagement_profiles: [network]\n\
             goals:\n  \
               - id: any-finding\n    \
                 objective: \"Record at least one finding.\"\n    \
                 target: {{ findings: {{}}, count_gte: 1 }}\n\
             scope: {{ authorized: true }}\n\
             pool: {{ agents: [lead, recon] }}\n\
             {extra}"
        ))
        .unwrap()
    }

    /// A lead scripted to: try an out-of-pool dispatch, dispatch `recon`, then
    /// `join` on the handle that dispatch returned, then stop. The handle is a
    /// fresh run id, so (unlike a static [`ScriptedTurn`]) the third turn is
    /// built from the tool result the lead actually received.
    struct DispatchingLead {
        calls: usize,
        captured: Captured,
        base: MockProvider,
    }

    impl DispatchingLead {
        fn new(captured: Captured) -> Self {
            Self {
                calls: 0,
                captured,
                base: MockProvider::new(vec![]),
            }
        }

        /// The handle in the latest `dispatch` result the request carries.
        fn handle_in(req: &rupu_providers::LlmRequest) -> String {
            request_texts(req)
                .iter()
                .rev()
                .find_map(|t| {
                    serde_json::from_str::<Value>(t)
                        .ok()
                        .and_then(|v| v["handle"].as_str().map(str::to_string))
                })
                .expect("a dispatch result with a handle reached the lead")
        }

        fn next(&mut self, req: &rupu_providers::LlmRequest) -> MockProvider {
            self.captured.lock().unwrap().push(req.clone());
            let turn = match self.calls {
                0 => tool_turn(
                    "t0",
                    "dispatch",
                    json!({ "agent": "not-in-pool", "prompt": "scan" }),
                ),
                1 => tool_turn(
                    "t1",
                    "dispatch",
                    json!({ "agent": "recon", "prompt": "scan" }),
                ),
                2 => tool_turn(
                    "t2",
                    "join",
                    json!({ "handle": Self::handle_in(req), "timeout_secs": 30 }),
                ),
                _ => done_turn(),
            };
            self.calls += 1;
            MockProvider::new(vec![turn])
        }
    }

    #[async_trait::async_trait]
    impl rupu_providers::LlmProvider for DispatchingLead {
        async fn send(
            &mut self,
            req: &rupu_providers::LlmRequest,
        ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
            self.next(req).send(req).await
        }

        async fn stream(
            &mut self,
            req: &rupu_providers::LlmRequest,
            on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
        ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
            self.next(req).stream(req, on_event).await
        }

        fn default_model(&self) -> &str {
            self.base.default_model()
        }

        fn provider_id(&self) -> rupu_providers::ProviderId {
            self.base.provider_id()
        }
    }

    fn a_unit_finding(id: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: "reported by a unit".into(),
            severity: Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_unit".into(),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: Some("recon".into()),
                provider: None,
            },
            declared_at: Utc::now(),
            profile: FindingProfile::Summary,
            report: None,
        }
    }

    #[test]
    fn the_lead_dispatches_a_pool_agent_joins_it_and_the_pooled_ledger_has_its_finding() {
        use crate::unit::{MockUnitLauncher, UnitOutcome, UnitStatus};
        let fx = fixture();
        let id = "af_dispatch";

        // The mock unit does what a real one would, at spawn: it reports a
        // finding into the run's POOLED scope and posts to the run's board.
        let ws = fx.workspace.clone();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![
                UnitStatus::Running,
                UnitStatus::Done(UnitOutcome {
                    output: "recon found one issue".into(),
                    success: true,
                }),
            ])
            .with_on_spawn(move |spec, run_dir| {
                let paths = CoveragePaths::new(&ws, &target_id(&ws, id));
                append_record(&paths, Ledger::Findings, &a_unit_finding("fnd_unit")).unwrap();
                rupu_fleet::Board::new(run_dir.to_path_buf())
                    .post(&rupu_fleet::BoardPost {
                        author: spec.participant.clone(),
                        ts: "2026-10-05T12:00:01Z".into(),
                        kind: rupu_fleet::PostKind::Observation,
                        body: "recon is on it".into(),
                        addressed_to: None,
                    })
                    .unwrap();
            }),
        );

        let captured: Captured = Arc::default();
        let shared = captured.clone();
        let mut o = opts(
            &fx,
            def_with_recon_pool("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        o.make_provider = Box::new(move || -> Box<dyn rupu_providers::LlmProvider> {
            Box::new(DispatchingLead::new(shared.clone()))
        });
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();

        // The unit's finding satisfied the flow's goal, via the pooled scope.
        assert_eq!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);
        let findings = std::fs::read_to_string(&pooled_paths(&fx, id).findings).unwrap();
        assert!(findings.contains("fnd_unit"), "{findings}");

        // Exactly one unit started: the pool agent, with the def's engagement and
        // a minted unit participant. The out-of-pool request spawned nothing.
        let spawned = launcher.spawned();
        assert_eq!(spawned.len(), 1, "{spawned:?}");
        assert_eq!(spawned[0].agent, "recon");
        assert_eq!(spawned[0].prompt, "scan");
        assert_eq!(spawned[0].engagement, ["network"]);
        assert_eq!(spawned[0].participant, "recon#1");

        // The lead saw what each tool call returned.
        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 4, "one model call per scripted turn");
        // (a) fail-closed: the out-of-pool dispatch came back as an error.
        let after_refusal = request_texts(&reqs[1]);
        assert!(
            any_contains(&after_refusal, "not in this flow's pool"),
            "{after_refusal:?}"
        );
        // (b) the real dispatch returned a handle...
        let handle = DispatchingLead::handle_in(&reqs[2]);
        assert!(handle.starts_with("run_"), "{handle}");
        // (c) ...and join returned the unit's outcome for it.
        let after_join = request_texts(&reqs[3]);
        assert!(
            any_contains(&after_join, "recon found one issue")
                && any_contains(&after_join, "\"status\":\"done\""),
            "{after_join:?}"
        );

        // The supervisor recorded the unit under its handle, as done.
        let unit: Value = serde_json::from_str(
            &std::fs::read_to_string(
                run_dir(&fx, id)
                    .join("units")
                    .join(&handle)
                    .join("unit.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(unit["agent"], "recon");
        assert_eq!(unit["participant"], "recon#1");
        assert_eq!(unit["status"]["state"], "done");

        // The unit's board post is on the run's board.
        let posts = rupu_fleet::Board::new(run_dir(&fx, id))
            .read_posts()
            .unwrap();
        assert!(
            posts
                .iter()
                .any(|p| p.author == "recon#1" && p.body == "recon is on it"),
            "{posts:?}"
        );
        // A unit that finished by itself is not signalled at wind-down.
        assert!(launcher.terminated().is_empty());
    }

    // ---- run_workflow over the fleet supervisor ----------------------------

    /// Chooses a lead turn from the turn index and the request it answers.
    type PickTurn = Arc<dyn Fn(usize, &rupu_providers::LlmRequest) -> ScriptedTurn + Send + Sync>;

    /// A lead whose every turn is chosen by `pick(turn index, request)`: for a
    /// script that depends on what an earlier tool call returned (a handle).
    struct PickingLead {
        calls: usize,
        captured: Captured,
        pick: PickTurn,
        base: MockProvider,
    }

    impl PickingLead {
        fn factory(
            captured: Captured,
            pick: impl Fn(usize, &rupu_providers::LlmRequest) -> ScriptedTurn + Send + Sync + 'static,
        ) -> ProviderFactory {
            let pick = Arc::new(pick);
            Box::new(move || -> Box<dyn rupu_providers::LlmProvider> {
                Box::new(PickingLead {
                    calls: 0,
                    captured: captured.clone(),
                    pick: pick.clone(),
                    base: MockProvider::new(vec![]),
                })
            })
        }

        fn next(&mut self, req: &rupu_providers::LlmRequest) -> MockProvider {
            self.captured.lock().unwrap().push(req.clone());
            let turn = (self.pick)(self.calls, req);
            self.calls += 1;
            MockProvider::new(vec![turn])
        }
    }

    #[async_trait::async_trait]
    impl rupu_providers::LlmProvider for PickingLead {
        async fn send(
            &mut self,
            req: &rupu_providers::LlmRequest,
        ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
            self.next(req).send(req).await
        }

        async fn stream(
            &mut self,
            req: &rupu_providers::LlmRequest,
            on_event: &mut (dyn FnMut(rupu_providers::StreamEvent) + Send),
        ) -> Result<rupu_providers::LlmResponse, rupu_providers::ProviderError> {
            self.next(req).stream(req, on_event).await
        }

        fn default_model(&self) -> &str {
            self.base.default_model()
        }

        fn provider_id(&self) -> rupu_providers::ProviderId {
            self.base.provider_id()
        }
    }

    /// A definition whose pool holds the lead and `recon`, plus exactly the
    /// listed pool workflows.
    fn def_with_workflow_pool(workflows: &[&str]) -> AgentiflowDef {
        AgentiflowDef::parse_str(&format!(
            "name: itest\n\
             lead: lead\n\
             engagement_profiles: [network]\n\
             goals:\n  \
               - id: any-finding\n    \
                 objective: \"Record at least one finding.\"\n    \
                 target: {{ findings: {{}}, count_gte: 1 }}\n\
             scope: {{ authorized: true }}\n\
             pool: {{ agents: [lead, recon], workflows: [{}] }}\n\
             round: {{ ceiling: {{ rounds: 1 }} }}\n",
            workflows.join(", ")
        ))
        .unwrap()
    }

    /// Write a workflow file under the fixture workspace's `.rupu/workflows/`.
    fn write_workflow(fx: &Fixture, id: &str, yaml: &str) {
        let dir = fx.workspace.join(".rupu").join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.yaml")), yaml).unwrap();
    }

    #[test]
    fn the_lead_runs_a_pool_workflow_as_a_unit_joins_it_and_the_goal_is_met() {
        use crate::unit::{MockUnitLauncher, UnitKind, UnitOutcome, UnitStatus};
        let fx = fixture();
        let id = "af_workflow";
        // A benign one-step workflow whose only agent (`recon`) is in the pool.
        write_workflow(
            &fx,
            "sweep",
            "name: sweep\n\
             inputs:\n  target: { type: string, required: true }\n\
             steps:\n  - id: only\n    agent: recon\n    prompt: \"scan {{ inputs.target }}\"\n",
        );

        // The mock stands in for the real `rupu workflow run`: at spawn it does
        // what the workflow's agent would -- reports a finding into the run's
        // POOLED scope. (This test proves the tool + dispatch, not a real run.)
        let ws = fx.workspace.clone();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![
                UnitStatus::Running,
                UnitStatus::Done(UnitOutcome {
                    output: "sweep found one issue".into(),
                    success: true,
                }),
            ])
            .with_on_spawn(move |spec, _| {
                assert_eq!(spec.kind, UnitKind::Workflow);
                let paths = CoveragePaths::new(&ws, &target_id(&ws, id));
                append_record(&paths, Ledger::Findings, &a_unit_finding("fnd_wf")).unwrap();
            }),
        );

        let captured: Captured = Arc::default();
        let mut o = opts(&fx, def_with_workflow_pool(&["sweep"]), id);
        o.make_provider = PickingLead::factory(captured.clone(), |i, req| match i {
            0 => tool_turn(
                "t0",
                "run_workflow",
                json!({ "workflow": "sweep", "inputs": { "target": "10.0.0.5" } }),
            ),
            1 => tool_turn(
                "t1",
                "join",
                json!({ "handle": DispatchingLead::handle_in(req), "timeout_secs": 30 }),
            ),
            _ => done_turn(),
        });
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();

        // The workflow unit's finding satisfied the flow's goal via the pooled scope.
        assert_eq!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);
        let findings = std::fs::read_to_string(&pooled_paths(&fx, id).findings).unwrap();
        assert!(findings.contains("fnd_wf"), "{findings}");

        // Exactly one unit started: the workflow, bound to the def's engagement,
        // carrying the lead's inputs and no prompt.
        let spawned = launcher.spawned();
        assert_eq!(spawned.len(), 1, "{spawned:?}");
        assert_eq!(spawned[0].kind, UnitKind::Workflow);
        assert_eq!(spawned[0].agent, "sweep");
        assert_eq!(spawned[0].prompt, "");
        assert_eq!(spawned[0].engagement, ["network"]);
        assert_eq!(spawned[0].participant, "sweep#1");
        assert_eq!(
            spawned[0].inputs,
            [("target".to_string(), "10.0.0.5".to_string())]
        );

        // The lead got a handle back, and join returned the unit's outcome.
        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 3, "one model call per scripted turn");
        let handle = DispatchingLead::handle_in(&reqs[1]);
        assert!(handle.starts_with("run_"), "{handle}");
        let after_join = request_texts(&reqs[2]);
        assert!(
            any_contains(&after_join, "sweep found one issue")
                && any_contains(&after_join, "\"status\":\"done\""),
            "{after_join:?}"
        );

        // The supervisor recorded the unit as a workflow unit, done.
        let unit: Value = serde_json::from_str(
            &std::fs::read_to_string(
                run_dir(&fx, id)
                    .join("units")
                    .join(&handle)
                    .join("unit.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(unit["kind"], "workflow");
        assert_eq!(unit["agent"], "sweep");
        assert_eq!(unit["participant"], "sweep#1");
        assert_eq!(unit["status"]["state"], "done");
        // A unit that finished by itself is not signalled at wind-down.
        assert!(launcher.terminated().is_empty());
    }

    #[test]
    fn run_workflow_fails_closed_on_every_bad_request_and_spawns_nothing() {
        use crate::unit::{MockUnitLauncher, UnitStatus};
        let fx = fixture();
        let id = "af_workflow_refused";
        // Four workflows, each in the pool EXCEPT `unlisted`, each broken one way.
        write_workflow(
            &fx,
            "unlisted",
            "name: unlisted\nsteps:\n  - id: s\n    agent: recon\n    prompt: hi\n",
        );
        write_workflow(
            &fx,
            "wide",
            "name: wide\nsteps:\n  - id: s\n    agent: rogue\n    prompt: hi\n",
        );
        write_workflow(
            &fx,
            "gated",
            "name: gated\nsteps:\n  - id: g\n    approval:\n      required: true\n",
        );
        write_workflow(
            &fx,
            "needs-input",
            "name: needs-input\n\
             inputs:\n  target: { type: string, required: true }\n\
             steps:\n  - id: s\n    agent: recon\n    prompt: \"{{ inputs.target }}\"\n",
        );

        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let captured: Captured = Arc::default();
        let mut o = opts(
            &fx,
            def_with_workflow_pool(&["wide", "gated", "needs-input"]),
            id,
        );
        o.make_provider = PickingLead::factory(captured.clone(), |i, _| match i {
            0 => tool_turn("t0", "run_workflow", json!({ "workflow": "unlisted" })),
            1 => tool_turn("t1", "run_workflow", json!({ "workflow": "wide" })),
            2 => tool_turn("t2", "run_workflow", json!({ "workflow": "gated" })),
            3 => tool_turn("t3", "run_workflow", json!({ "workflow": "needs-input" })),
            _ => done_turn(),
        });
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();

        // Nothing was ever started, so the goal could not be met.
        assert_ne!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);
        assert!(launcher.spawned().is_empty(), "{:?}", launcher.spawned());
        assert!(!run_dir(&fx, id).join("units").exists());

        // The lead saw each refusal, naming its reason.
        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 5, "one model call per scripted turn");
        let reasons = [
            "workflow 'unlisted' is not in this flow's pool",
            "workflow 'wide' dispatches agents outside the pool: rogue",
            "v1 cannot run a gated or host/distribute workflow as a unit (gated)",
            "input `target` is required",
        ];
        for (i, reason) in reasons.iter().enumerate() {
            let after = request_texts(&reqs[i + 1]);
            assert!(any_contains(&after, reason), "turn {i}: {after:?}");
        }
    }

    // ---- generate_workflow threaded through run_agentiflow -----------------

    /// A workflow whose only agent (`recon`) is in `def_with_recon_pool`'s pool.
    const GENERATED_WF: &str =
        "name: fresh-sweep\nsteps:\n  - id: only\n    agent: recon\n    prompt: scan\n";

    /// The names of the tools a request offered the lead.
    fn offered_tools(req: &rupu_providers::LlmRequest) -> Vec<&str> {
        req.tools.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn the_lead_generates_a_workflow_and_it_runs_as_a_file_backed_unit() {
        use crate::lead::{GenerationCapability, GenerationProviderFactory};
        use crate::unit::{MockUnitLauncher, UnitKind, UnitOutcome, UnitStatus};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let fx = fixture();
        let id = "af_generate";

        // The mock unit does what the real `rupu workflow run --file` would:
        // reports a finding into the run's POOLED scope at spawn.
        let ws = fx.workspace.clone();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![
                UnitStatus::Running,
                UnitStatus::Done(UnitOutcome {
                    output: "generated sweep found one issue".into(),
                    success: true,
                }),
            ])
            .with_on_spawn(move |spec, _| {
                assert_eq!(spec.kind, UnitKind::Workflow);
                let paths = CoveragePaths::new(&ws, &target_id(&ws, id));
                append_record(&paths, Ledger::Findings, &a_unit_finding("fnd_gen")).unwrap();
            }),
        );

        // The lead's authoring capability: every generation call gets a fresh
        // `MockProvider` that writes a valid, pool-only workflow.
        let minted = Arc::new(AtomicUsize::new(0));
        let counter = minted.clone();
        let factory: GenerationProviderFactory = Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText {
                text: GENERATED_WF.into(),
                stop: rupu_agent::StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }])) as Box<dyn rupu_providers::LlmProvider>
        });

        let captured: Captured = Arc::default();
        let mut o = opts(
            &fx,
            def_with_recon_pool("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        o.make_provider = PickingLead::factory(captured.clone(), |i, req| match i {
            0 => tool_turn(
                "t0",
                "generate_workflow",
                json!({ "description": "sweep the exposed gateway" }),
            ),
            1 => tool_turn(
                "t1",
                "join",
                json!({ "handle": DispatchingLead::handle_in(req), "timeout_secs": 30 }),
            ),
            _ => done_turn(),
        });
        o.unit_launcher = Some(launcher.clone());
        o.generation = Some(GenerationCapability {
            provider: "mock".into(),
            model: "mock-1".into(),
            factory,
        });
        let out = run_agentiflow(o).unwrap();

        // The generated workflow's finding satisfied the flow's goal.
        assert_eq!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);
        let findings = std::fs::read_to_string(&pooled_paths(&fx, id).findings).unwrap();
        assert!(findings.contains("fnd_gen"), "{findings}");
        assert_eq!(minted.load(Ordering::Relaxed), 1, "one generation call");

        // Exactly one unit started: a file-backed workflow unit bound to the
        // flow's engagement, with no prompt and no catalog id behind it.
        let spawned = launcher.spawned();
        assert_eq!(spawned.len(), 1, "{spawned:?}");
        assert_eq!(spawned[0].kind, UnitKind::Workflow);
        assert_eq!(spawned[0].agent, "fresh-sweep");
        assert_eq!(spawned[0].prompt, "");
        assert_eq!(spawned[0].engagement, ["network"]);
        assert_eq!(spawned[0].participant, "fresh-sweep#1");
        // The materialized file lives under the run dir, holds exactly what
        // the generator wrote, and parses.
        let file = spawned[0]
            .workflow_file
            .clone()
            .expect("a generated workflow is file-backed");
        assert_eq!(file.parent().unwrap(), run_dir(&fx, id).join("generated"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            GENERATED_WF.trim()
        );
        assert_eq!(
            rupu_orchestrator::Workflow::parse_file(&file).unwrap().name,
            "fresh-sweep"
        );

        // The lead was offered the tool, got a handle back, and join returned
        // the unit's outcome for it.
        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 3, "one model call per scripted turn");
        assert!(
            offered_tools(&reqs[0]).contains(&"generate_workflow"),
            "{:?}",
            offered_tools(&reqs[0])
        );
        let after_generate = request_texts(&reqs[1]);
        assert!(
            any_contains(&after_generate, "\"generated_file\"")
                && any_contains(&after_generate, "fresh-sweep#1"),
            "{after_generate:?}"
        );
        let handle = DispatchingLead::handle_in(&reqs[1]);
        assert!(handle.starts_with("run_"), "{handle}");
        let after_join = request_texts(&reqs[2]);
        assert!(
            any_contains(&after_join, "generated sweep found one issue")
                && any_contains(&after_join, "\"status\":\"done\""),
            "{after_join:?}"
        );

        // The supervisor recorded it as a workflow unit, done.
        let unit: Value = serde_json::from_str(
            &std::fs::read_to_string(
                run_dir(&fx, id)
                    .join("units")
                    .join(&handle)
                    .join("unit.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(unit["kind"], "workflow");
        assert_eq!(unit["participant"], "fresh-sweep#1");
        assert_eq!(unit["status"]["state"], "done");
    }

    #[test]
    fn without_a_generation_capability_the_lead_is_not_offered_generate_workflow() {
        use crate::unit::{MockUnitLauncher, UnitStatus};
        let fx = fixture();
        let id = "af_no_generate";
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));

        let captured: Captured = Arc::default();
        let mut o = opts(
            &fx,
            def_with_recon_pool("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        // The lead calls the tool anyway (a model can name a tool it was never
        // offered): it must come back as an unknown-tool error, spawning nothing.
        o.make_provider = PickingLead::factory(captured.clone(), |i, _| match i {
            0 => tool_turn(
                "t0",
                "generate_workflow",
                json!({ "description": "sweep the exposed gateway" }),
            ),
            _ => done_turn(),
        });
        o.unit_launcher = Some(launcher.clone());
        assert!(o.generation.is_none());
        let out = run_agentiflow(o).unwrap();
        assert_ne!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);

        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 2, "one model call per scripted turn");
        // Absence is proven twice: the advertised tool list omits it (while the
        // sibling unit tools are present, so the list is the real one)...
        let offered = offered_tools(&reqs[0]);
        assert!(!offered.contains(&"generate_workflow"), "{offered:?}");
        for sibling in ["dispatch", "join", "run_workflow"] {
            assert!(offered.contains(&sibling), "{sibling} missing: {offered:?}");
        }
        // ...and invoking it anyway is refused as unknown.
        let after = request_texts(&reqs[1]);
        assert!(
            any_contains(&after, "unknown tool: generate_workflow"),
            "{after:?}"
        );
        assert!(launcher.spawned().is_empty(), "{:?}", launcher.spawned());
        assert!(!run_dir(&fx, id).join("generated").exists());
        assert!(!run_dir(&fx, id).join("units").exists());
    }

    // ---- roster awareness + status / steering tools -----------------------

    /// Write one agent file under the fixture's global `agents/` dir.
    fn write_agent(fx: &Fixture, name: &str, description: &str) {
        let dir = fx.global.join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.md")),
            format!("---\nname: {name}\ndescription: {description}\ntools: [read_file]\n---\nYou are {name}.\n"),
        )
        .unwrap();
    }

    #[test]
    fn the_lead_sees_the_roster_steers_the_board_and_checks_goal_status() {
        use crate::unit::{MockUnitLauncher, UnitStatus};
        let fx = fixture();
        let id = "af_roster";

        // The catalog: the two pool agents plus `scout`, which the pool does NOT
        // name -- so it can only reach the lead through `agents.list`, never
        // through the pool-scoped roster index.
        write_agent(&fx, "lead", "Runs the engagement.");
        write_agent(&fx, "recon", "Maps the exposed gateway surface.");
        write_agent(&fx, "scout", "Walks the legacy intranet pages.");

        // The mock unit reports a finding into the run's POOLED scope at spawn.
        let ws = fx.workspace.clone();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![UnitStatus::Running]).with_on_spawn(move |_, _| {
                let paths = CoveragePaths::new(&ws, &target_id(&ws, id));
                append_record(&paths, Ledger::Findings, &a_unit_finding("fnd_unit")).unwrap();
            }),
        );

        let (factory, captured) = capturing_factory(vec![vec![
            tool_turn("t0", "agents.list", json!({})),
            // Before any unit has reported: the goal is not yet met.
            tool_turn("t1", "goal.status", json!({})),
            tool_turn(
                "t2",
                "board.directive",
                json!({ "body": "prioritize the gateway" }),
            ),
            tool_turn(
                "t3",
                "dispatch",
                json!({ "agent": "recon", "prompt": "scan" }),
            ),
            // After the unit's finding is pooled: the goal is met.
            tool_turn("t4", "goal.status", json!({})),
            done_turn(),
        ]]);
        let mut o = opts(
            &fx,
            def_with_recon_pool("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        o.make_provider = factory;
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();
        assert_eq!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);

        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 6, "one model call per scripted turn");

        // (a) The RosterCollector's pool index reached the model on its very
        // first call: the pool's agents, resolved against the catalog. `scout`
        // is in the catalog but not the pool, so it is absent here.
        let first = request_texts(&reqs[0]);
        assert!(
            any_contains(&first, "source: roster")
                && any_contains(&first, "Maps the exposed gateway surface."),
            "the roster index reaches the lead's first turn: {first:?}"
        );
        assert!(
            !any_contains(&first, "scout"),
            "the index is the pool, not the whole catalog: {first:?}"
        );
        // ...and is re-asserted every turn.
        assert!(any_contains(&request_texts(&reqs[5]), "source: roster"));

        // (b) `agents.list` ran for real: the whole catalog (including `scout`)
        // came back to the model after that turn.
        let after_list = request_texts(&reqs[1]);
        assert!(
            any_contains(&after_list, "Walks the legacy intranet pages."),
            "{after_list:?}"
        );

        // (c) `board.directive` wrote a standing directive, read back through a
        // FRESH board handle over the run dir.
        let directives = rupu_fleet::Board::new(run_dir(&fx, id))
            .read_directives()
            .unwrap();
        assert_eq!(directives.len(), 1, "{directives:?}");
        assert_eq!(directives[0].author, "lead");
        assert_eq!(directives[0].body, "prioritize the gateway");

        // (d) `goal.status` re-evaluated live: unmet before the unit's finding
        // was pooled, met after it.
        let before = request_texts(&reqs[2]);
        assert!(
            any_contains(&before, "\"id\":\"any-finding\"")
                && any_contains(&before, "\"satisfied\":false"),
            "{before:?}"
        );
        let after = request_texts(&reqs[5]);
        assert!(
            any_contains(&after, "\"id\":\"any-finding\"")
                && any_contains(&after, "\"satisfied\":true"),
            "{after:?}"
        );
        let findings = std::fs::read_to_string(&pooled_paths(&fx, id).findings).unwrap();
        assert!(findings.contains("fnd_unit"), "{findings}");
    }

    // ---- the verification gate, end to end ---------------------------------

    /// A definition whose single goal is "one CWE-94 finding, verified by the
    /// agent `verifier`", with that verifier (and a filer, `recon`) in the pool.
    fn def_with_verified_goal(extra: &str) -> AgentiflowDef {
        AgentiflowDef::parse_str(&format!(
            "name: itest\n\
             lead: lead\n\
             engagement_profiles: [network]\n\
             goals:\n  \
               - id: verified-rce\n    \
                 objective: \"Land one verified CWE-94 finding.\"\n    \
                 verify_with: verifier\n    \
                 target: {{ findings: {{ classification: \"CWE-94\" }}, count_gte: 1, verified: true }}\n\
             scope: {{ authorized: true }}\n\
             pool: {{ agents: [lead, recon, verifier] }}\n\
             {extra}"
        ))
        .unwrap()
    }

    /// The text of the latest `goal.status` result for `goal_id` the request carries.
    fn latest_goal_row(req: &rupu_providers::LlmRequest, goal_id: &str) -> Value {
        request_texts(req)
            .iter()
            .rev()
            .find(|t| t.contains(&format!("\"id\":\"{goal_id}\"")))
            .map(|t| serde_json::from_str::<Value>(t).expect("goal.status returns JSON"))
            .and_then(|rows| rows.as_array().and_then(|r| r.first().cloned()))
            .expect("a goal.status result reached the lead")
    }

    fn verify_as(finding_id: &str, status: VerificationStatus, by: (&str, &str)) -> VerifyInput {
        VerifyInput {
            finding_id: finding_id.into(),
            status,
            by_run: by.0.into(),
            by_agent: Some(by.1.into()),
            notes: None,
        }
    }

    #[test]
    fn a_verified_goal_is_met_only_once_an_independent_run_confirms_the_finding() {
        use crate::unit::{MockUnitLauncher, UnitStatus};
        let fx = fixture();
        let id = "af_verify";

        // The mock units do what real ones would, at spawn, into the run's
        // POOLED scope: `recon` files a full finding as run_A; `verifier`
        // confirms it as run_B -- a different run, through the real write path.
        let ws = fx.workspace.clone();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![UnitStatus::Running]).with_on_spawn(move |spec, _| {
                let paths = CoveragePaths::new(&ws, &target_id(&ws, id));
                match spec.agent.as_str() {
                    "recon" => {
                        let rec = full_finding("fnd_rce", "run_A", "CWE-94");
                        append_record(&paths, Ledger::Findings, &rec).unwrap();
                    }
                    "verifier" => {
                        let verdict = verify_as(
                            "fnd_rce",
                            VerificationStatus::Confirmed,
                            ("run_B", "verifier"),
                        );
                        verify_finding(&paths, &verdict).unwrap();
                    }
                    other => panic!("unexpected unit agent {other}"),
                }
            }),
        );

        let (factory, captured) = capturing_factory(vec![vec![
            tool_turn(
                "t0",
                "dispatch",
                json!({ "agent": "recon", "prompt": "find an RCE" }),
            ),
            // Filed, but nobody has verified it yet.
            tool_turn("t1", "goal.status", json!({})),
            tool_turn(
                "t2",
                "dispatch",
                json!({ "agent": "verifier", "prompt": "verify fnd_rce" }),
            ),
            // Independently confirmed.
            tool_turn("t3", "goal.status", json!({})),
            done_turn(),
        ]]);
        let mut o = opts(
            &fx,
            def_with_verified_goal("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        o.make_provider = factory;
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();
        assert_eq!(out.stop, StopReason::GoalsMet, "{:?}", out.stop);

        let spawned: Vec<String> = launcher.spawned().iter().map(|s| s.agent.clone()).collect();
        assert_eq!(spawned, ["recon", "verifier"]);

        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 5, "one model call per scripted turn");
        // Filed under run_A and not yet verified: the lead is told so, and who
        // has to verify it.
        let filed = latest_goal_row(&reqs[2], "verified-rce");
        assert_eq!(filed["satisfied"], false, "{filed}");
        assert_eq!(filed["current"], 0, "{filed}");
        let detail = filed["detail"].as_str().unwrap();
        assert!(
            detail.contains("1 matched, 0 verified")
                && detail.contains("1 awaiting verification by verifier"),
            "{detail}"
        );
        // After run_B's confirmation the same goal is met.
        let verified = latest_goal_row(&reqs[4], "verified-rce");
        assert_eq!(verified["satisfied"], true, "{verified}");
        assert_eq!(verified["current"], 1, "{verified}");
        assert!(
            verified["detail"]
                .as_str()
                .unwrap()
                .contains("1 matched, 1 verified"),
            "{verified}"
        );

        // The pooled ledger: one finding, filed by run_A, confirmed by run_B.
        let recs = read_findings(&pooled_paths(&fx, id)).unwrap();
        assert_eq!(recs.len(), 1, "{recs:?}");
        assert_eq!(recs[0].declared_by.run_id, "run_A");
        let v = recs[0]
            .report
            .as_ref()
            .unwrap()
            .verification
            .as_ref()
            .unwrap();
        assert_eq!(v.status, VerificationStatus::Confirmed);
        assert_eq!(v.by_run.as_deref(), Some("run_B"));
        assert_eq!(v.by_agent.as_deref(), Some("verifier"));
    }

    #[test]
    fn a_verified_goal_is_not_met_by_a_filed_self_verified_or_wrong_agent_finding() {
        use crate::unit::{MockUnitLauncher, UnitStatus};
        use std::sync::atomic::{AtomicBool, Ordering};
        let fx = fixture();
        let id = "af_noverify";

        let self_refused = Arc::new(AtomicBool::new(false));
        let refused = self_refused.clone();
        let ws = fx.workspace.clone();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![UnitStatus::Running]).with_on_spawn(move |spec, _| {
                let paths = CoveragePaths::new(&ws, &target_id(&ws, id));
                match spec.agent.as_str() {
                    "recon" => {
                        // Three findings, all filed by run_A.
                        for fid in ["fnd_self", "fnd_forged", "fnd_wrong"] {
                            let mut rec = full_finding(fid, "run_A", "CWE-94");
                            if fid == "fnd_forged" {
                                // A verdict written straight into the ledger,
                                // bypassing the write path: Confirmed, by the
                                // run that filed the finding.
                                rec.report.as_mut().unwrap().verification = Some(Verification {
                                    status: VerificationStatus::Confirmed,
                                    by_run: Some("run_A".into()),
                                    by_agent: Some("verifier".into()),
                                    notes: None,
                                });
                            }
                            append_record(&paths, Ledger::Findings, &rec).unwrap();
                        }
                    }
                    "verifier" => {
                        // run_A tries to verify its own finding: the write path
                        // refuses it.
                        let own = verify_as(
                            "fnd_self",
                            VerificationStatus::Confirmed,
                            ("run_A", "verifier"),
                        );
                        refused.store(
                            matches!(
                                verify_finding(&paths, &own),
                                Err(VerifyError::SelfVerification)
                            ),
                            Ordering::SeqCst,
                        );
                        // An independent run confirms -- but it is not the
                        // agent the goal names (`verifier`).
                        let other = verify_as(
                            "fnd_wrong",
                            VerificationStatus::Confirmed,
                            ("run_B", "somebody-else"),
                        );
                        verify_finding(&paths, &other).unwrap();
                    }
                    other => panic!("unexpected unit agent {other}"),
                }
            }),
        );

        let (factory, captured) = capturing_factory(vec![vec![
            tool_turn(
                "t0",
                "dispatch",
                json!({ "agent": "recon", "prompt": "find RCEs" }),
            ),
            tool_turn(
                "t1",
                "dispatch",
                json!({ "agent": "verifier", "prompt": "verify them" }),
            ),
            tool_turn("t2", "goal.status", json!({})),
            done_turn(),
        ]]);
        let mut o = opts(
            &fx,
            def_with_verified_goal("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        o.make_provider = factory;
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();

        // Three CWE-94 findings are filed and none clears the gate: the flow
        // runs to its ceiling instead of reporting its goal met.
        assert_eq!(out.stop, StopReason::Ceiling, "{:?}", out.stop);
        assert!(
            self_refused.load(Ordering::SeqCst),
            "self-verification is refused at the write path"
        );

        let reqs = captured.lock().unwrap().clone();
        assert_eq!(reqs.len(), 4, "one model call per scripted turn");
        let row = latest_goal_row(&reqs[3], "verified-rce");
        assert_eq!(row["satisfied"], false, "{row}");
        assert_eq!(row["current"], 0, "{row}");
        let detail = row["detail"].as_str().unwrap();
        assert!(
            detail.contains("3 matched, 0 verified")
                && detail.contains("3 awaiting verification by verifier"),
            "{detail}"
        );

        // The refused self-verdict left no trace on its finding.
        let recs = read_findings(&pooled_paths(&fx, id)).unwrap();
        let own = recs.iter().find(|r| r.id == "fnd_self").unwrap();
        assert!(own.report.as_ref().unwrap().verification.is_none());
    }

    #[test]
    fn a_listed_workflow_pool_is_taken_as_written_and_all_resolves_to_the_catalog() {
        let fx = fixture();
        let wf = fx.global.join("workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(
            wf.join("sweep.yaml"),
            "name: Sweep\nsteps:\n  - id: only\n    agent: recon\n    prompt: hi\n",
        )
        .unwrap();
        let ctx = RosterCtx {
            global: fx.global.clone(),
            project: Some(fx.workspace.join(".rupu")),
        };
        // A list is passed through untouched, even for an id the catalog lacks
        // (the collector marks it `(not found)`).
        let listed = WorkflowsSpec::List(vec!["sweep".into(), "ghost".into()]);
        assert_eq!(pool_workflow_ids(&listed, &ctx), ["sweep", "ghost"]);
        // `all` is the catalog's ids.
        let all = AgentiflowDef::parse_str(
            "name: n\nlead: lead\nengagement_profiles: [network]\n\
             goals:\n  - id: g\n    objective: o\n    target: { findings: {}, count_gte: 1 }\n\
             scope: { authorized: true }\npool: { agents: [lead], workflows: all }\n",
        )
        .unwrap()
        .pool
        .workflows;
        assert_eq!(pool_workflow_ids(&all, &ctx), ["sweep"]);
    }

    #[test]
    fn wind_down_terminates_a_unit_the_lead_left_running() {
        use crate::unit::{MockUnitLauncher, UnitStatus};
        let fx = fixture();
        let id = "af_winddown";
        // The unit never finishes, and the lead dispatches it without joining.
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let (factory, _captured) = capturing_factory(vec![vec![
            tool_turn(
                "t1",
                "dispatch",
                json!({ "agent": "recon", "prompt": "scan" }),
            ),
            done_turn(),
        ]]);
        let mut o = opts(
            &fx,
            def_with_recon_pool("round: { ceiling: { rounds: 1 } }"),
            id,
        );
        o.make_provider = factory;
        o.unit_launcher = Some(launcher.clone());
        let out = run_agentiflow(o).unwrap();
        assert_eq!(out.stop, StopReason::Ceiling, "{:?}", out.stop);

        let spawned = launcher.spawned();
        assert_eq!(spawned.len(), 1, "{spawned:?}");
        let units: Vec<String> = std::fs::read_dir(run_dir(&fx, id).join("units"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            launcher.terminated(),
            units,
            "the live unit was SIGTERMed when the run wound down"
        );
        assert_eq!(units.len(), 1);
    }

    #[test]
    fn a_failed_validation_creates_nothing() {
        let fx = fixture();
        let mut def = def_with("");
        def.scope.authorized = false;
        let err = run_agentiflow(opts(&fx, def, "af_bad")).expect_err("rejected");
        assert!(matches!(err, AgentiflowError::Invalid(_)), "{err:?}");
        assert!(!run_dir(&fx, "af_bad").exists());
    }

    #[test]
    fn a_bad_ceiling_wall_clock_or_zero_turns_is_rejected_before_any_dir() {
        let fx = fixture();
        let bad = def_with("round: { ceiling: { wall_clock: \"soon\" } }");
        let err = run_agentiflow(opts(&fx, bad, "af_wc")).expect_err("rejected");
        assert!(err.to_string().contains("wall_clock"), "{err}");
        assert!(!run_dir(&fx, "af_wc").exists());

        let zero = def_with("round: { lead_max_turns: 0 }");
        let err = run_agentiflow(opts(&fx, zero, "af_zero")).expect_err("rejected");
        assert!(err.to_string().contains("lead_max_turns"), "{err}");
        assert!(!run_dir(&fx, "af_zero").exists());
    }

    #[test]
    fn an_existing_run_is_never_overwritten() {
        let fx = fixture();
        let id = "af_dup";
        seed_finding(&fx, id);
        run_agentiflow(opts(&fx, def_with(""), id)).unwrap();
        let before = std::fs::read(run_dir(&fx, id).join(RECORD_FILE)).unwrap();

        let err = run_agentiflow(opts(&fx, def_with(""), id)).expect_err("rejected");
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(
            std::fs::read(run_dir(&fx, id).join(RECORD_FILE)).unwrap(),
            before
        );
    }

    /// One lead round, then the ceiling stops the flow: the lead really runs
    /// (its per-round `block_on`), and the flow terminates without a seeded finding.
    const CEILING_ONE_ROUND: &str = "round: { ceiling: { rounds: 1 } }";

    #[test]
    fn running_from_inside_a_tokio_runtime_succeeds() {
        let fx = fixture();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let o = opts(&fx, def_with(CEILING_ONE_ROUND), "af_async");
        let out = rt.block_on(async move { run_agentiflow(o) });
        assert_eq!(out.expect("flow runs").stop, StopReason::Ceiling);
        assert!(run_dir(&fx, "af_async").exists());
    }

    #[test]
    fn running_from_spawn_blocking_succeeds() {
        let fx = fixture();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let o = opts(&fx, def_with(CEILING_ONE_ROUND), "af_spawnblk");
        let out = rt.block_on(async move {
            tokio::task::spawn_blocking(move || run_agentiflow(o))
                .await
                .unwrap()
        });
        assert_eq!(out.expect("flow runs").stop, StopReason::Ceiling);
        assert!(run_dir(&fx, "af_spawnblk").exists());
    }
}
