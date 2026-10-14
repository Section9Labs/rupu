//! Agentiflows (LOCAL only): the runs `rupu agentiflow run` lays out under
//! `<global>/agentiflows/af_<ULID>/`. Reads, plus one write — steering.
//!
//! - `GET /api/agentiflows` — every run's slim row, newest first.
//! - `GET /api/agentiflows/:id` — one run: its record, the definition snapshot
//!   it started from, the `events.jsonl` log, and the units (branches) it
//!   dispatched.
//! - `POST /api/agentiflows/:id/steer` — message the lead, or queue a stop
//!   (`cp serve` only; refused unless its coordinator is alive). A message is
//!   `POST /api/agentiflows/:id/messages` to the lead (`crate::api::messages`,
//!   which serves the run's messages too).
//!
//! The run-dir layout and `events.jsonl` line shapes are documented on
//! `rupu_agentiflow::run`'s module docs; the readers here mirror
//! `rupu-cli`'s `agentiflow list` / `agentiflow status` (`cmd/agentiflow.rs`)
//! without depending on it. Apart from steering, nothing here writes:
//! the CP only observes a run the coordinator (or its reaper) owns.
//!
//! Transcripts are NOT served here. The lead's per-round transcripts
//! (`lead/transcript.r<N>.jsonl`) and a unit's transcript are plain `.jsonl`
//! files under the global dir (or a workspace), so the detail carries their
//! paths and the existing `/api/transcript` endpoint reads them.
//!
//! Remote hosts and SSE are a later pass; this module reads
//! `AppState::global_dir` directly.

use crate::{
    error::{ApiError, ApiResult},
    host::messages::MessageRoute,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rupu_agentiflow::{
    agentiflow_dir, units_on_disk, AgentiflowDef, AgentiflowRecord, Budget, CoverageTarget, Goal,
    GoalTarget, OperatorQueue, Pool, RoundConfig, UnitOnDisk, UnitStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// `<global>/agentiflows/` also holds the definition files (`<name>.yaml`) the
/// runs were started from; only an `af_`-prefixed DIRECTORY is a run.
const RUN_DIR_PREFIX: &str = "af_";
/// A run id is a directory name, so it is bounded the way the writer bounds it
/// (`rupu_agentiflow::run::validate_run_id`: 1-128 of `[A-Za-z0-9_-]`).
const MAX_RUN_ID_LEN: usize = 128;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/agentiflows", get(list_agentiflows))
        .route("/api/agentiflows/:id", get(get_agentiflow))
        .route("/api/agentiflows/:id/events", get(get_agentiflow_events))
        .route("/api/agentiflows/:id/children", get(get_agentiflow_children))
        .route(
            "/api/agentiflows/:id/units/:unit_id/output",
            get(get_agentiflow_unit_output),
        )
        .route("/api/agentiflows/:id/steer", post(steer_agentiflow))
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// `GET /api/agentiflows` body.
#[derive(Serialize)]
struct AgentiflowListResponse {
    rows: Vec<AgentiflowListRow>,
}

/// One row of the list.
#[derive(Serialize)]
struct AgentiflowListRow {
    id: String,
    name: String,
    /// Always derived today: the record's `codename` is minted by a later
    /// daemon pass and is `None` on every record written so far.
    codename: String,
    codename_derived: bool,
    /// `running` | `completed` | `failed`.
    status: String,
    /// `goals_met`, `coverage_reached`, `budget_exhausted:<dim>`,
    /// `operator_stop`, `operator_stop:now`, `ceiling`, `error: <msg>`,
    /// `orphaned: coordinator pid <p> not running`; `null` while running.
    stop_reason: Option<String>,
    rounds: u32,
    goals_met: usize,
    goals_total: usize,
    /// `null` until the run has metered anything.
    spent_usd: Option<f64>,
    spent_tokens: u64,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    engagement_profiles: Vec<String>,
    /// For a `running` record with a recorded `runner_pid`: whether that
    /// process still exists. `false` is a coordinator that died without
    /// finalizing (the orphan reaper will close it out). `null` otherwise.
    runner_alive: Option<bool>,
}

/// `GET /api/agentiflows/:id` body.
#[derive(Serialize)]
struct AgentiflowDetail {
    record: RecordDto,
    /// The definition snapshot the run started from (`agentiflow.yaml`);
    /// `null` when it is absent or does not parse.
    def: Option<DefDto>,
    /// The budget state the last finished round recorded (`ok` | `soft` |
    /// `hard:<dimension>`); `null` before any round has finished.
    budget_state: Option<String>,
    /// `events.jsonl`, one object per parsed line, oldest first (see the
    /// `rupu_agentiflow::run` module docs for the `run_started` / `round` /
    /// `run_stopped` shapes). Unparseable lines are skipped.
    events: Vec<Value>,
    /// The units (branches) the lead dispatched, ordered by unit id.
    units: Vec<UnitDto>,
    /// The lead's per-round transcripts, ordered by round.
    lead_transcripts: Vec<LeadTranscriptDto>,
    /// The coverage scope this engagement's findings and assets pool under, so
    /// the Assets tab shows ONLY this engagement's assets (not every
    /// workspace's). `null` when the lead's workspace can't be resolved — the
    /// Assets tab then says so rather than falling back to every asset.
    #[serde(skip_serializing_if = "Option::is_none")]
    asset_scope: Option<AssetScopeDto>,
}

/// The coverage scope an agentiflow's findings/assets pool under. An
/// agentiflow writes ALL its evidence under ONE coverage target —
/// `target_id(workspace, <af id>)` inside the lead's workspace (the flow's id
/// is the scope name; see `rupu_agentiflow::run`). The web Assets tab passes
/// these to `GET /api/assets?ws_id=&target=`.
#[derive(Serialize)]
struct AssetScopeDto {
    ws_id: String,
    target_id: String,
}

/// The full `agentiflow.json` record, hand-mapped.
#[derive(Serialize)]
struct RecordDto {
    id: String,
    name: String,
    codename: String,
    codename_derived: bool,
    engagement_profiles: Vec<String>,
    /// The record's `trigger` in its snake_case wire form (`agentiflow`).
    trigger: String,
    status: String,
    stop_reason: Option<String>,
    rounds: u32,
    goals: Vec<GoalDto>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    spent_usd: Option<f64>,
    spent_tokens: u64,
    runner_pid: Option<u32>,
    runner_alive: Option<bool>,
}

/// One goal's status as of the last evaluation (join to `def.goals` by `id`
/// for the objective and predicate).
#[derive(Serialize)]
struct GoalDto {
    id: String,
    met: bool,
    current: u64,
    target: u64,
}

/// The definition snapshot. Budget / coverage / pool / round are the
/// authoring schema (`rupu_agentiflow::def`) passed through as-is.
#[derive(Serialize)]
struct DefDto {
    name: String,
    description: Option<String>,
    lead: String,
    engagement_profiles: Vec<String>,
    trigger: Option<String>,
    goals: Vec<DefGoalDto>,
    /// `{ reach, depth?, kinds? }` — the engagement-wide coverage stop.
    coverage: Option<CoverageTarget>,
    /// `{ usd?, tokens?, wall_clock?, rounds?, soft_at? }` — the caps.
    budget: Option<Budget>,
    /// `{ authorized, mode?, roots[] }`.
    scope: Value,
    /// `{ agents[], workflows: "all" | [..] }`.
    pool: Pool,
    /// `{ lead_max_turns?, ceiling?: { rounds?, wall_clock? } }`.
    round: Option<RoundConfig>,
}

#[derive(Serialize)]
struct DefGoalDto {
    id: String,
    objective: String,
    required: bool,
    verify_with: Option<String>,
    /// `target` in words (`findings, count >= 3, verified by x`).
    predicate: String,
    /// The raw `target:` block.
    target: GoalTarget,
}

#[derive(Serialize)]
struct UnitDto {
    unit_id: String,
    /// The unit's stored codename (its transcript's `RunStart`) when its
    /// transcript is on disk, else derived.
    codename: String,
    codename_derived: bool,
    /// The pool agent (or workflow) it runs; `null` for a `unit.json` that
    /// predates the field.
    agent: Option<String>,
    /// The roster name the lead addressed it by (`recon#1`).
    participant: Option<String>,
    /// `agent` | `workflow` | `subagent` (a lead's in-process child).
    kind: &'static str,
    status: UnitStatusDto,
    /// The process group the unit leads, when recorded.
    pgid: Option<u32>,
    started_at: Option<String>,
    /// Absolute path of the unit's transcript when it is on disk — read it
    /// through `/api/transcript?path=`.
    transcript_path: Option<String>,
}

#[derive(Serialize)]
struct UnitStatusDto {
    /// `pending` | `running` | `done` | `failed` | `abandoned`.
    state: &'static str,
    /// `done` only: whether the unit reported success.
    #[serde(skip_serializing_if = "Option::is_none")]
    success: Option<bool>,
    /// `done` only: the unit's final answer (can be long).
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    /// `failed` only: why.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct LeadTranscriptDto {
    round: u32,
    /// Absolute path; read it through `/api/transcript?path=`.
    path: String,
}

// ---------------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------------

/// A valid agentiflow run id: `af_` + `[A-Za-z0-9_-]`, bounded. Anything else
/// can neither name a run directory nor escape `agentiflows/`.
pub(crate) fn valid_run_id(id: &str) -> bool {
    id.len() <= MAX_RUN_ID_LEN
        && id.len() > RUN_DIR_PREFIX.len()
        && id.starts_with(RUN_DIR_PREFIX)
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether a `running` record's coordinator is still alive. `None` for any
/// other status or a record with no recorded pid ("owner unknown", never
/// "dead").
fn runner_alive(record: &AgentiflowRecord) -> Option<bool> {
    if record.status != "running" {
        return None;
    }
    // A zombie (exited, unreaped — PID 1 in some containers reaps nothing)
    // is as dead as a vanished pid: it will never drain anything.
    record.runner_pid.map(rupu_agentiflow::coordinator_alive)
}

/// The run's codename: the stored one if the record ever carries it, else
/// derived from the run id (`named` flags which).
fn run_codename(record: &AgentiflowRecord) -> (String, bool) {
    crate::codename::named(record.codename.as_deref(), &record.id, None)
}

fn list_row(record: AgentiflowRecord) -> AgentiflowListRow {
    let (codename, codename_derived) = run_codename(&record);
    AgentiflowListRow {
        goals_met: record.goals.iter().filter(|g| g.met).count(),
        goals_total: record.goals.len(),
        runner_alive: runner_alive(&record),
        id: record.id,
        name: record.name,
        codename,
        codename_derived,
        status: record.status,
        stop_reason: record.stop_reason,
        rounds: record.rounds,
        spent_usd: record.spent_usd,
        spent_tokens: record.spent_tokens,
        started_at: record.started_at,
        ended_at: record.ended_at,
        engagement_profiles: record.engagement_profiles,
    }
}

fn record_dto(record: AgentiflowRecord) -> RecordDto {
    let (codename, codename_derived) = run_codename(&record);
    let trigger = serde_json::to_value(&record.trigger)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    RecordDto {
        runner_alive: runner_alive(&record),
        id: record.id,
        name: record.name,
        codename,
        codename_derived,
        engagement_profiles: record.engagement_profiles,
        trigger,
        status: record.status,
        stop_reason: record.stop_reason,
        rounds: record.rounds,
        goals: record
            .goals
            .into_iter()
            .map(|g| GoalDto {
                id: g.id,
                met: g.met,
                current: g.current,
                target: g.target,
            })
            .collect(),
        started_at: record.started_at,
        ended_at: record.ended_at,
        spent_usd: record.spent_usd,
        spent_tokens: record.spent_tokens,
        runner_pid: record.runner_pid,
    }
}

/// A goal predicate in words: what `target:` asks for, plus the verification
/// bar. Mirrors `rupu-cli`'s `agentiflow status` (`describe_target`), so the
/// web and the terminal word a goal the same way.
fn describe_target(target: &GoalTarget, verify_with: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(f) = &target.findings {
        parts.push(match &f.classification {
            Some(c) => format!("findings classified {c}"),
            None => "findings".to_string(),
        });
    }
    if let Some(a) = &target.asset {
        let locator = a
            .locator
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(if locator.is_empty() {
            format!("asset {}", a.kind)
        } else {
            format!("asset {} ({locator})", a.kind)
        });
    }
    if let Some(n) = target.count_gte {
        parts.push(format!("count >= {n}"));
    }
    if let Some(d) = &target.depth_at_least {
        parts.push(format!("depth >= {d}"));
    }
    if target.verified || verify_with.is_some() {
        let mut v = String::from("verified");
        if let Some(agent) = verify_with {
            v.push_str(&format!(" by {agent}"));
        }
        if matches!(
            target.verify_check,
            Some(rupu_agentiflow::VerifyCheck::WithPoc)
        ) {
            v.push_str(" with a PoC");
        }
        parts.push(v);
    }
    parts.join(", ")
}

fn def_goal_dto(g: Goal) -> DefGoalDto {
    DefGoalDto {
        predicate: describe_target(&g.target, g.verify_with.as_deref()),
        id: g.id,
        objective: g.objective.trim().to_string(),
        required: g.required,
        verify_with: g.verify_with,
        target: g.target,
    }
}

fn def_dto(def: AgentiflowDef) -> DefDto {
    DefDto {
        // `ScopeRoot` flattens opaque YAML coordinates; a value JSON cannot
        // carry (a YAML tag) degrades the scope to `null` rather than the
        // whole detail.
        scope: serde_json::to_value(&def.scope).unwrap_or(Value::Null),
        name: def.name,
        description: def.description,
        lead: def.lead,
        engagement_profiles: def.engagement_profiles,
        trigger: def.trigger,
        goals: def.goals.into_iter().map(def_goal_dto).collect(),
        coverage: def.coverage,
        budget: def.budget,
        pool: def.pool,
        round: def.round,
    }
}

// ---------------------------------------------------------------------------
// Disk readers
// ---------------------------------------------------------------------------

/// Every run with a readable `agentiflow.json`, newest first (the id breaking
/// `started_at` ties so the order is stable). A directory without a parseable
/// record (a run being laid out, a hand-made stray) is skipped, as the other
/// run listers do. The DIRECTORY names the run — what `:id` resolves — exactly
/// as `rupu agentiflow list` does.
fn read_runs(global: &std::path::Path) -> Vec<AgentiflowRecord> {
    let root = agentiflow_dir(global);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut runs: Vec<AgentiflowRecord> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| valid_run_id(name))
        .filter_map(|name| {
            let mut record = AgentiflowRecord::read(&root.join(&name)).ok()?;
            record.id = name;
            Some(record)
        })
        .collect();
    runs.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    runs
}

/// `events.jsonl`'s parsed lines, oldest first. Best-effort: the log is a
/// local append and a reader can catch it mid-line, so a missing file is an
/// empty log and a line that does not parse is skipped.
fn read_events(run_dir: &std::path::Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read(run_dir.join("events.jsonl")) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&raw)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

/// The budget state the last finished `round` event recorded.
fn last_round_budget(events: &[Value]) -> Option<String> {
    events.iter().rev().find_map(|e| {
        if e.get("kind")?.as_str()? != "round" {
            return None;
        }
        Some(e.get("budget")?.as_str()?.to_string())
    })
}

/// The definition snapshot, when present and parseable.
fn read_def(run_dir: &std::path::Path) -> Option<AgentiflowDef> {
    let path = run_dir.join("agentiflow.yaml");
    let raw = std::fs::read_to_string(&path).ok()?;
    match AgentiflowDef::parse_str(&raw) {
        Ok(def) => Some(def),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "unparseable agentiflow definition snapshot");
            None
        }
    }
}

/// `lead/transcript.r<N>.jsonl` files, ordered by round.
fn read_lead_transcripts(run_dir: &std::path::Path) -> Vec<LeadTranscriptDto> {
    let Ok(entries) = std::fs::read_dir(run_dir.join("lead")) else {
        return Vec::new();
    };
    let mut out: Vec<LeadTranscriptDto> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let round = name
                .strip_prefix("transcript.r")?
                .strip_suffix(".jsonl")?
                .parse::<u32>()
                .ok()?;
            Some(LeadTranscriptDto {
                round,
                path: e.path().to_string_lossy().into_owned(),
            })
        })
        .collect();
    out.sort_by_key(|t| t.round);
    out
}

/// What a unit's `unit.json` carries beyond [`UnitOnDisk`] (which keeps only
/// what the reaper needs). Tolerant: every key optional.
#[derive(Default, Deserialize)]
struct UnitMeta {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    participant: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
}

fn read_unit_meta(run_dir: &std::path::Path, unit_id: &str) -> UnitMeta {
    std::fs::read(run_dir.join("units").join(unit_id).join("unit.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// Where a unit's `rupu run` wrote its transcript: the global transcripts dir,
/// else the CP workspace's `.rupu/transcripts` (a unit resolves a project-local
/// directory when its workspace has one). `unit_id` is already vetted by
/// `units_on_disk` as a plain run id.
fn unit_transcript(s: &AppState, unit_id: &str) -> Option<PathBuf> {
    let name = format!("{unit_id}.jsonl");
    [
        s.global_dir.join("transcripts"),
        s.workspace_dir.join(".rupu").join("transcripts"),
    ]
    .into_iter()
    .map(|dir| dir.join(&name))
    .find(|p| p.is_file())
}

/// The `codename` the unit's own transcript recorded in its `RunStart` (minted
/// when the unit's `rupu run` began). `None` when the transcript is missing,
/// empty, or predates stored codenames.
fn stored_unit_codename(path: &std::path::Path) -> Option<String> {
    match rupu_transcript::JsonlReader::iter(path).ok()?.next()? {
        Ok(rupu_transcript::Event::RunStart { codename, .. }) => codename,
        _ => None,
    }
}

fn unit_status_dto(status: UnitStatus) -> UnitStatusDto {
    match status {
        UnitStatus::Pending => UnitStatusDto {
            state: "pending",
            success: None,
            output: None,
            error: None,
        },
        UnitStatus::Running => UnitStatusDto {
            state: "running",
            success: None,
            output: None,
            error: None,
        },
        UnitStatus::Done(o) => UnitStatusDto {
            state: "done",
            success: Some(o.success),
            output: Some(o.output),
            error: None,
        },
        UnitStatus::Failed(why) => UnitStatusDto {
            state: "failed",
            success: None,
            output: None,
            error: Some(why),
        },
    }
}

/// The transcript a launched child wrote: the one its ledger line names,
/// else where a `rupu run` of its id writes one.
fn child_transcript(s: &AppState, started: &rupu_launch::ledger::Started) -> Option<PathBuf> {
    started
        .transcript
        .clone()
        .filter(|p| p.is_file())
        .or_else(|| unit_transcript(s, &started.run_id))
}

/// A child the flow's launcher recorded in `children.jsonl` (W7), as a unit.
fn ledger_unit(s: &AppState, e: rupu_launch::ledger::ChildEntry) -> UnitDto {
    let state = e.state();
    let f = e.finished.as_ref();
    let transcript = child_transcript(s, &e.started);
    // The unit's answer is recorded in its `Finished` ledger line, so it rides
    // along for free — the units list never reads a (possibly multi-MB)
    // transcript. For the rare finished unit whose ledger line carries no
    // output, the Fleet row fetches it on demand from the transcript
    // (`GET …/units/:unit_id/output`).
    let (codename, codename_derived) = crate::codename::named(
        e.started.codename.as_deref(),
        &e.started.run_id,
        Some(&e.started.name),
    );
    UnitDto {
        codename,
        codename_derived,
        agent: Some(e.started.name.clone()),
        participant: e.started.address.clone(),
        kind: e.started.kind.as_str(),
        status: UnitStatusDto {
            state: state.as_str(),
            success: f.and_then(|f| f.success),
            output: f.and_then(|f| f.output.clone()),
            error: f.and_then(|f| f.error.clone()),
        },
        pgid: e.started.pgid,
        started_at: Some(e.started.at.to_rfc3339()),
        transcript_path: transcript.map(|p| p.to_string_lossy().into_owned()),
        unit_id: e.started.run_id,
    }
}

/// The run's units: those a pre-W7 coordinator mirrored to
/// `units/*/unit.json`, then the children its launcher recorded in
/// `children.jsonl`.
fn read_units(s: &AppState, run_dir: &std::path::Path) -> Vec<UnitDto> {
    let mut units = read_legacy_units(s, run_dir);
    units.extend(
        rupu_launch::ledger::read(run_dir)
            .into_iter()
            .map(|e| ledger_unit(s, e)),
    );
    units
}

/// Every run an agentiflow's launch tree holds below its lead, and every
/// transcript the flow wrote.
pub(crate) struct FlowRuns {
    /// Each legacy unit, then each child the flow's launcher recorded and,
    /// down the tree, every child those launched.
    pub run_ids: Vec<String>,
    /// The lead's rounds, then each of those runs' (a workflow child's are
    /// its steps').
    pub transcripts: Vec<PathBuf>,
}

pub(crate) fn flow_runs(s: &AppState, run_dir: &std::path::Path) -> FlowRuns {
    let mut runs = FlowRuns {
        run_ids: Vec::new(),
        transcripts: read_lead_transcripts(run_dir)
            .into_iter()
            .map(|t| PathBuf::from(t.path))
            .collect(),
    };
    for u in units_on_disk(run_dir) {
        runs.transcripts.extend(unit_transcript(s, &u.unit_id));
        runs.run_ids.push(u.unit_id);
    }
    for started in rupu_launch::identity::launch_tree(&s.global_dir, run_dir) {
        match started.kind {
            rupu_tools::ChildKind::Workflow => runs.transcripts.extend(
                crate::usage::run_transcript_paths(&s.run_store, &started.run_id),
            ),
            _ => runs.transcripts.extend(child_transcript(s, &started)),
        }
        runs.run_ids.push(started.run_id);
    }
    runs
}

fn read_legacy_units(s: &AppState, run_dir: &std::path::Path) -> Vec<UnitDto> {
    units_on_disk(run_dir)
        .into_iter()
        .map(|u: UnitOnDisk| {
            let meta = read_unit_meta(run_dir, &u.unit_id);
            let transcript = unit_transcript(s, &u.unit_id);
            // The unit is a real `rupu run`: the name its transcript recorded
            // is the one the Activity list shows for it, so prefer it over a
            // derivation that would name the same run differently.
            let stored = transcript.as_deref().and_then(stored_unit_codename);
            let (codename, codename_derived) =
                crate::codename::named(stored.as_deref(), &u.unit_id, meta.agent.as_deref());
            UnitDto {
                codename,
                codename_derived,
                agent: meta.agent,
                participant: meta.participant,
                kind: u.kind.as_str(),
                status: unit_status_dto(u.status),
                pgid: u.pgid,
                started_at: meta.started_at,
                transcript_path: transcript.map(|p| p.to_string_lossy().into_owned()),
                unit_id: u.unit_id,
            }
        })
        .collect()
}

/// Resolve the [`AssetScopeDto`] for the run in `run_dir` (id `af_id`): the
/// lead's workspace comes from its first-round `RunStart`, and the coverage
/// target is `target_id(workspace_path, af_id)` (the flow's id is its scope
/// name — see `rupu_agentiflow::run`). `None` when the lead transcript is
/// absent/unreadable, carries no `RunStart`, or names a workspace this CP has
/// no registered path for; callers then leave the Assets tab unscoped-but-safe
/// rather than showing every workspace's assets.
fn asset_scope(s: &AppState, run_dir: &std::path::Path, af_id: &str) -> Option<AssetScopeDto> {
    let ws = flow_workspace(s, run_dir)?;
    Some(AssetScopeDto {
        target_id: rupu_coverage::target_id(std::path::Path::new(&ws.path), af_id),
        ws_id: ws.id,
    })
}

/// The workspace the flow ran in: the one its first lead round's `RunStart`
/// names, when this CP has it registered.
pub(crate) fn flow_workspace(
    s: &AppState,
    run_dir: &std::path::Path,
) -> Option<rupu_workspace::Workspace> {
    let lead = read_lead_transcripts(run_dir).into_iter().next()?;
    let ws_id = match rupu_transcript::JsonlReader::iter(std::path::Path::new(&lead.path))
        .ok()?
        .next()?
    {
        Ok(rupu_transcript::Event::RunStart { workspace_id, .. }) => workspace_id,
        _ => return None,
    };
    rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    }
    .list()
    .ok()?
    .into_iter()
    .find(|w| w.id == ws_id)
}

/// Assemble the detail for `id`, or `None` when no such run directory has a
/// record. A record that exists but cannot be parsed is an error, not a 404:
/// the run is there, its state of record is unreadable.
fn load_detail(s: &AppState, id: &str) -> ApiResult<Option<AgentiflowDetail>> {
    let run_dir = agentiflow_dir(&s.global_dir).join(id);
    let mut record = match AgentiflowRecord::read(&run_dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ApiError::internal(format!(
                "agentiflow {id}: agentiflow.json is unreadable: {e}"
            )))
        }
    };
    // The directory names the run (what `:id` resolved), as in the list.
    record.id = id.to_string();
    let events = read_events(&run_dir);
    Ok(Some(AgentiflowDetail {
        budget_state: last_round_budget(&events),
        def: read_def(&run_dir).map(def_dto),
        units: read_units(s, &run_dir),
        lead_transcripts: read_lead_transcripts(&run_dir),
        asset_scope: asset_scope(s, &run_dir, id),
        record: record_dto(record),
        events,
    }))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/agentiflows` — every local run, newest first. Never fails on an
/// unreadable run directory (it is skipped) or a missing `agentiflows/` dir
/// (no runs yet → `{ "rows": [] }`).
async fn list_agentiflows(State(s): State<AppState>) -> ApiResult<Json<AgentiflowListResponse>> {
    let global = s.global_dir.clone();
    let rows = tokio::task::spawn_blocking(move || {
        read_runs(&global)
            .into_iter()
            .map(list_row)
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| ApiError::internal(format!("listing agentiflows panicked: {e}")))?;
    Ok(Json(AgentiflowListResponse { rows }))
}

/// `GET /api/agentiflows/:id` — one run's record, definition snapshot, events
/// and units. 404 for an id that is not `af_…`-shaped or names no run.
async fn get_agentiflow(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<AgentiflowDetail>> {
    if !valid_run_id(&id) {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    let detail = tokio::task::spawn_blocking({
        let id = id.clone();
        move || load_detail(&s, &id)
    })
    .await
    .map_err(|e| ApiError::internal(format!("reading agentiflow {id} panicked: {e}")))??;
    detail
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no agentiflow run `{id}`")))
}

/// `GET /api/agentiflows/:id/events` — the engagement's granular event feed: the
/// coordinator lifecycle (`run_started` / `round` / `run_stopped`, from
/// `events.jsonl`) MERGED with per-unit lifecycle — each dispatched unit's
/// `af_unit_started` and, once terminal, `af_unit_completed`, whose completion
/// time, token totals and provider/model are joined from `usage.jsonl` (the
/// `unit.json` checkpoint carries none of those). Oldest first; the web renders
/// it through the shared RunEventFeed / EventCard. Findings and workflow-step
/// events are a later pass.
#[derive(Serialize)]
struct EventsResponse {
    events: Vec<Value>,
}

/// A unit's completion time, approximated by its transcript file's last-write
/// time — the `unit.json` checkpoint records no finished-at, and the engagement
/// `usage.jsonl` tracks only the LEAD's turns (keyed by `lead/transcript.r<N>`),
/// not the units. `None` when the transcript is absent or its mtime unreadable.
/// (Per-unit tokens come with the findings pass, which reads each unit's run.)
fn transcript_mtime(path: &str) -> Option<String> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified).to_rfc3339())
}

/// Cap a unit's final answer so a multi-KB output never bloats an event row.
fn snippet(s: &str) -> String {
    const MAX: usize = 400;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(MAX).collect();
        out.push('…');
        out
    }
}

/// A line's `ts` as a sort key; a line without one sorts to the start.
fn event_ts(v: &Value) -> &str {
    v.get("ts").and_then(Value::as_str).unwrap_or("")
}

/// Build the merged event feed for the run in `run_dir`. Pure disk reads
/// (`events.jsonl`, the unit checkpoints, `usage.jsonl`), so it runs on the
/// blocking pool.
fn build_events(s: &AppState, id: &str) -> Option<EventsResponse> {
    let run_dir = agentiflow_dir(&s.global_dir).join(id);
    if !run_dir.is_dir() {
        return None;
    }
    let mut out: Vec<Value> = read_events(&run_dir);
    for u in read_units(s, &run_dir) {
        let identity = serde_json::json!({
            "unit_id": u.unit_id,
            "agent": u.agent,
            "codename": u.codename,
            "codename_derived": u.codename_derived,
            "participant": u.participant,
            "unit_kind": u.kind,
            "transcript_path": u.transcript_path,
        });
        if let Some(started) = &u.started_at {
            let mut ev = identity.clone();
            ev["kind"] = Value::String("af_unit_started".into());
            ev["ts"] = Value::String(started.clone());
            out.push(ev);
        }
        if matches!(u.status.state, "done" | "failed") {
            let ts = u
                .transcript_path
                .as_deref()
                .and_then(transcript_mtime)
                .or_else(|| u.started_at.clone());
            let mut ev = identity;
            ev["kind"] = Value::String("af_unit_completed".into());
            if let Some(ts) = ts {
                ev["ts"] = Value::String(ts);
            }
            ev["success"] = serde_json::to_value(u.status.success).unwrap_or(Value::Null);
            if let Some(e) = &u.status.error {
                ev["error"] = Value::String(snippet(e));
            }
            // A unit's answer rides its ledger (or legacy `unit.json`) record,
            // so it is already in hand; only a `done` unit whose record carries
            // none is read from the transcript here, in the events path.
            let output = u.status.output.clone().or_else(|| {
                (u.status.state == "done")
                    .then(|| unit_output(s, &run_dir, &u.unit_id))
                    .flatten()
            });
            if let Some(o) = &output {
                ev["output"] = Value::String(snippet(o));
            }
            out.push(ev);
        }
    }
    // Chronological (oldest first); the web reverses to newest-first. ISO-8601
    // timestamps sort lexically, so a plain string compare is chronological.
    out.sort_by(|a, b| event_ts(a).cmp(event_ts(b)));
    Some(EventsResponse { events: out })
}

async fn get_agentiflow_events(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<EventsResponse>> {
    if !valid_run_id(&id) {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    let resp = tokio::task::spawn_blocking({
        let id = id.clone();
        move || build_events(&s, &id)
    })
    .await
    .map_err(|e| ApiError::internal(format!("reading agentiflow {id} events panicked: {e}")))?;
    resp.map(Json)
        .ok_or_else(|| ApiError::not_found(format!("no agentiflow run `{id}`")))
}

/// `GET /api/agentiflows/:id/children` — every child under the flow, of every
/// kind, as the agentiflow's Children tab shows it: the units the lead
/// launched (re-parented to the flow) and, under each unit that is a local
/// `rupu run`, that run's own launch tree. Same `ChildrenResponse` shape as
/// the run detail's `/api/runs/:id/children`. Local reads only — no `?host=`.
async fn get_agentiflow_children(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::api::children::ChildrenResponse>> {
    if !valid_run_id(&id) {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    let run_dir = agentiflow_dir(&s.global_dir).join(&id);
    if !run_dir.is_dir() {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    let children = tokio::task::spawn_blocking({
        let id = id.clone();
        move || {
            use crate::api::children;
            // Direct children come from both the legacy `units/` mirrors
            // (pre-W7) and the `children.jsonl` ledger (W7); the ledger is the
            // richer record, so it wins when a run id is in both. Each
            // direct child's own launch tree is then walked below it.
            let mut directs = legacy_unit_children(&s, &run_dir, &id);
            let ledgered = children::ledger_units(&run_dir, &id);
            let have: std::collections::HashSet<String> =
                ledgered.iter().map(|c| c.run_id.clone()).collect();
            directs.retain(|c| !have.contains(&c.run_id));
            directs.extend(ledgered);
            children::with_descendants(&s.run_store, directs)
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("reading agentiflow {id} children panicked: {e}")))?;
    Ok(Json(crate::api::children::ChildrenResponse { children }))
}

/// A flow's legacy `units/*/unit.json` mirrors as children-tree rows
/// (`ChildDto`), re-parented to the flow `af_id`. Pre-W7 units predate the
/// `children.jsonl` ledger, so they carry no handle, tokens or duration and
/// are marked `legacy`; each is a real `rupu run`, so its own descendants are
/// still walked from its run directory by [`children::with_descendants`].
fn legacy_unit_children(
    s: &AppState,
    run_dir: &std::path::Path,
    af_id: &str,
) -> Vec<crate::api::children::ChildDto> {
    units_on_disk(run_dir)
        .into_iter()
        .map(|u: UnitOnDisk| {
            let meta = read_unit_meta(run_dir, &u.unit_id);
            let transcript = unit_transcript(s, &u.unit_id);
            let stored = transcript.as_deref().and_then(stored_unit_codename);
            let (codename, _derived) =
                crate::codename::named(stored.as_deref(), &u.unit_id, meta.agent.as_deref());
            let st = unit_status_dto(u.status);
            crate::api::children::ChildDto {
                handle: None,
                kind: u.kind.as_str().to_string(),
                name: meta.agent.clone().unwrap_or_default(),
                run_id: u.unit_id,
                parent_run_id: af_id.to_string(),
                depth: 1,
                codename: Some(codename),
                address: meta.participant,
                status: st.state.to_string(),
                success: st.success,
                error: st.error,
                tokens: None,
                duration_ms: None,
                started_at: meta.started_at,
                finished_at: None,
                transcript_path: transcript.map(|p| p.to_string_lossy().into_owned()),
                legacy: true,
            }
        })
        .collect()
}

/// A unit's final answer, resolved on demand. The transcript's final turn is
/// authoritative when the transcript is on disk (a W7 unit); a legacy
/// `units/*/unit.json` records the answer directly, the fallback when there is
/// no answer in (or no) transcript. `None` unless the unit finished with one.
fn unit_output(s: &AppState, run_dir: &std::path::Path, unit_id: &str) -> Option<String> {
    if let Some(t) = unit_transcript(s, unit_id) {
        if let Some(answer) = rupu_transcript::final_answer(&t) {
            return Some(answer);
        }
    }
    units_on_disk(run_dir)
        .into_iter()
        .find(|u| u.unit_id == unit_id)
        .and_then(|u| match u.status {
            UnitStatus::Done(o) => Some(o.output),
            _ => None,
        })
}

/// `GET /api/agentiflows/:id/units/:unit_id/output` — one unit's final answer,
/// read on demand so the units list (and its live poll) never pays the
/// transcript read. `{ "output": string | null }`; `output` is null when the
/// unit recorded no answer. 404 unless `unit_id` is a unit of this flow. Local
/// reads only — no `?host=`.
async fn get_agentiflow_unit_output(
    State(s): State<AppState>,
    Path((id, unit_id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    if !valid_run_id(&id) {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    if unit_id.is_empty()
        || !unit_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::not_found(format!("`{unit_id}` is not a unit id")));
    }
    let run_dir = agentiflow_dir(&s.global_dir).join(&id);
    if !run_dir.is_dir() {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    let output = tokio::task::spawn_blocking({
        let unit_id = unit_id.clone();
        move || {
            // The unit must belong to this flow — the lookup is cheap now that
            // the list no longer reads transcripts.
            if !read_units(&s, &run_dir).iter().any(|u| u.unit_id == unit_id) {
                return Err(());
            }
            Ok(unit_output(&s, &run_dir, &unit_id))
        }
    })
    .await
    .map_err(|e| ApiError::internal(format!("reading agentiflow {id} unit output panicked: {e}")))?
    .map_err(|()| ApiError::not_found(format!("agentiflow `{id}` has no unit `{unit_id}`")))?;
    Ok(Json(serde_json::json!({ "output": output })))
}

/// `POST /api/agentiflows/:id/steer` — the older spelling of a message to
/// the lead: exactly `POST /api/agentiflows/:id/messages {to: lead, body:
/// message, urgent: now}`, answered as that is (`202 {id, delivery}`). With
/// `stop: true` it instead queues a graceful stop on the control queue
/// (`200 {queued: true}`), refused (409) unless a live coordinator will read
/// it: the record must be `running` and its recorded `runner_pid` alive — a
/// coordinator that died without finalizing leaves `running` on disk, and a
/// stop queued there is never drained ([`steerable`]). A write, so `cp serve`
/// only (501 otherwise).
#[derive(Deserialize)]
struct SteerRequest {
    message: String,
    /// `send --now`: ask for mid-round delivery, not just the round boundary.
    #[serde(default)]
    now: bool,
    /// Ask the envelope to wind the run down.
    #[serde(default)]
    stop: bool,
}

/// Whether `record`'s lead will drain a stop: `running`, and its coordinator
/// not known dead. A record with no `runner_pid` (owner unknown) is accepted
/// — there is nothing to check it against.
fn steerable(id: &str, record: &AgentiflowRecord) -> ApiResult<()> {
    if record.status != "running" {
        return Err(ApiError::conflict(format!(
            "agentiflow `{id}` is not running ({}) — it cannot take a steering message",
            record.status
        )));
    }
    if runner_alive(record) == Some(false) {
        return Err(ApiError::conflict(format!(
            "agentiflow `{id}`'s coordinator (pid {}) is no longer running — nothing \
             would read a steering message; the run will be reaped as interrupted",
            record.runner_pid.unwrap_or_default()
        )));
    }
    Ok(())
}

async fn steer_agentiflow(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SteerRequest>,
) -> ApiResult<Response> {
    if !valid_run_id(&id) {
        return Err(ApiError::not_found(format!("no agentiflow run `{id}`")));
    }
    if !req.stop {
        let body = crate::api::messages::PostBody {
            to: Some(rupu_fleet::Address::Lead.to_string()),
            body: req.message,
            urgent: req.now,
        };
        return crate::api::messages::post(MessageRoute::Agentiflows, &s, &id, None, body).await;
    }
    if req.message.trim().is_empty() {
        return Err(ApiError::bad_request("steering message is empty"));
    }
    crate::api::config::require_writable_to(&s, "steering an agentiflow")?;
    let run_dir = agentiflow_dir(&s.global_dir).join(&id);
    let record = match AgentiflowRecord::read(&run_dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(format!("no agentiflow run `{id}`")))
        }
        Err(e) => {
            return Err(ApiError::internal(format!(
                "agentiflow {id}: agentiflow.json is unreadable: {e}"
            )))
        }
    };
    steerable(&id, &record)?;
    OperatorQueue::new(&run_dir)
        .request_stop()
        .map_err(|e| ApiError::internal(format!("could not queue a stop: {e}")))?;
    Ok(Json(StopQueued { queued: true }).into_response())
}

#[derive(Serialize)]
struct StopQueued {
    queued: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_agentiflow::GoalStatus;
    use rupu_runtime::RunTriggerSource;

    fn record(id: &str, started: &str, goals: Vec<GoalStatus>) -> AgentiflowRecord {
        AgentiflowRecord {
            id: id.into(),
            name: "flow".into(),
            engagement_profiles: vec!["code".into()],
            trigger: RunTriggerSource::Agentiflow,
            status: "completed".into(),
            stop_reason: Some("goals_met".into()),
            rounds: 2,
            goals,
            started_at: started.parse().unwrap(),
            ended_at: None,
            codename: None,
            spent_usd: Some(1.5),
            spent_tokens: 10,
            runner_pid: None,
        }
    }

    fn goal(id: &str, met: bool) -> GoalStatus {
        GoalStatus {
            id: id.into(),
            met,
            current: 1,
            target: 2,
        }
    }

    #[test]
    fn run_ids_must_be_af_prefixed_path_safe_components() {
        assert!(valid_run_id("af_01M4BF0K5YRDZMH5173KQK1B5R"));
        assert!(valid_run_id("af_a-b_c"));
        for bad in [
            "", "af_", "run_01", "../af_x", "af_../x", "af_a/b", "af_a\\b", "af_a b", "af_x\0y",
            "af_.", "AF_x",
        ] {
            assert!(!valid_run_id(bad), "{bad:?} must be rejected");
        }
        assert!(!valid_run_id(&format!("af_{}", "a".repeat(MAX_RUN_ID_LEN))));
    }

    #[test]
    fn list_row_counts_goals_and_derives_the_codename() {
        let row = list_row(record(
            "af_01J9ZQ3K4M5N6P7Q8R9S0T1V2W",
            "2026-10-07T00:00:00Z",
            vec![goal("a", true), goal("b", false), goal("c", true)],
        ));
        assert_eq!((row.goals_met, row.goals_total), (2, 3));
        assert!(row.codename_derived);
        assert!(!row.codename.is_empty());
        // A finished record has no liveness to report.
        assert_eq!(row.runner_alive, None);
    }

    #[test]
    fn a_stored_codename_wins_and_is_not_flagged_derived() {
        let mut r = record("af_x", "2026-10-07T00:00:00Z", vec![]);
        r.codename = Some("cobalt-harbor".into());
        let dto = record_dto(r);
        assert_eq!(dto.codename, "cobalt-harbor");
        assert!(!dto.codename_derived);
        assert_eq!(dto.trigger, "agentiflow");
    }

    #[test]
    fn a_running_record_reports_whether_its_coordinator_is_alive() {
        let mut r = record("af_x", "2026-10-07T00:00:00Z", vec![]);
        r.status = "running".into();
        // Owner unknown is never "dead".
        assert_eq!(runner_alive(&r), None);
        r.runner_pid = Some(std::process::id());
        assert_eq!(runner_alive(&r), Some(true));
    }

    #[test]
    fn read_runs_is_newest_first_and_skips_strays() {
        let tmp = tempfile::tempdir().unwrap();
        let root = agentiflow_dir(tmp.path());
        for (id, started) in [
            ("af_old", "2026-10-01T00:00:00Z"),
            ("af_new", "2026-10-07T00:00:00Z"),
        ] {
            record(id, started, vec![]).write(&root.join(id)).unwrap();
        }
        // A definition file, a non-run directory, a run dir with no record,
        // and one with a corrupt record: none is a row.
        std::fs::write(root.join("flow.yaml"), "name: x").unwrap();
        std::fs::create_dir_all(root.join("scratch")).unwrap();
        std::fs::create_dir_all(root.join("af_norecord")).unwrap();
        std::fs::create_dir_all(root.join("af_corrupt")).unwrap();
        std::fs::write(root.join("af_corrupt").join("agentiflow.json"), "{nope").unwrap();

        let ids: Vec<String> = read_runs(tmp.path()).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["af_new", "af_old"]);
        // No `agentiflows/` dir at all is an empty list, not an error.
        let empty = tempfile::tempdir().unwrap();
        assert!(read_runs(empty.path()).is_empty());
    }

    #[test]
    fn events_are_tolerant_and_the_last_round_gives_the_budget_state() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("events.jsonl"),
            concat!(
                "{\"kind\":\"run_started\",\"ts\":\"t0\"}\n",
                "{\"kind\":\"round\",\"budget\":\"ok\",\"round\":0}\n",
                "not json\n",
                "{\"kind\":\"round\",\"budget\":\"soft\",\"round\":1}\n",
                "{\"kind\":\"round\",\"budget\":\"ha", // torn tail
            ),
        )
        .unwrap();
        let events = read_events(tmp.path());
        assert_eq!(events.len(), 3, "{events:?}");
        assert_eq!(last_round_budget(&events).as_deref(), Some("soft"));
        assert_eq!(last_round_budget(&[]), None);
        assert!(read_events(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn lead_transcripts_are_ordered_by_round_not_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let lead = tmp.path().join("lead");
        std::fs::create_dir_all(&lead).unwrap();
        for name in [
            "transcript.r10.jsonl",
            "transcript.r2.jsonl",
            "transcript.r0.jsonl",
            "transcript.jsonl",
            "netflow.jsonl",
            "transcript.rx.jsonl",
        ] {
            std::fs::write(lead.join(name), "").unwrap();
        }
        let rounds: Vec<u32> = read_lead_transcripts(tmp.path())
            .into_iter()
            .map(|t| t.round)
            .collect();
        assert_eq!(rounds, [0, 2, 10]);
    }

    #[test]
    fn a_goal_predicate_reads_in_words() {
        let def = AgentiflowDef::parse_str(
            "name: n\nlead: l\nengagement_profiles: [code]\nscope: { authorized: true }\n\
             pool: { agents: [l] }\ngoals:\n  - id: g\n    objective: \" Find it. \"\n    \
             target: { findings: { classification: CWE-94 }, count_gte: 3, verified: true }\n    \
             verify_with: checker\n",
        )
        .unwrap();
        let dto = def_dto(def);
        let g = &dto.goals[0];
        assert_eq!(g.objective, "Find it.");
        assert_eq!(
            g.predicate,
            "findings classified CWE-94, count >= 3, verified by checker"
        );
        assert!(g.required);
    }
}
