//! Agentiflows (read-only, LOCAL only): the runs `rupu agentiflow run` lays out
//! under `<global>/agentiflows/af_<ULID>/`.
//!
//! - `GET /api/agentiflows` — every run's slim row, newest first.
//! - `GET /api/agentiflows/:id` — one run: its record, the definition snapshot
//!   it started from, the `events.jsonl` log, and the units (branches) it
//!   dispatched.
//!
//! The run-dir layout and `events.jsonl` line shapes are documented on
//! `rupu_agentiflow::run`'s module docs; the readers here mirror
//! `rupu-cli`'s `agentiflow list` / `agentiflow status` (`cmd/agentiflow.rs`)
//! without depending on it. Nothing here writes: the CP only observes a run the
//! coordinator (or its reaper) owns.
//!
//! Transcripts are NOT served here. The lead's per-round transcripts
//! (`lead/transcript.r<N>.jsonl`) and a unit's transcript are plain `.jsonl`
//! files under the global dir (or a workspace), so the detail carries their
//! paths and the existing `/api/transcript` endpoint reads them.
//!
//! Remote hosts, SSE and steering are a later pass; this module reads
//! `AppState::global_dir` directly.

use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use rupu_agentiflow::{
    agentiflow_dir, pid_is_running, units_on_disk, AgentiflowDef, AgentiflowRecord, Budget,
    CoverageTarget, Goal, GoalTarget, Pool, RoundConfig, UnitOnDisk, UnitStatus,
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
    /// `agent` | `workflow`.
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
    /// `pending` | `running` | `done` | `failed`.
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
fn valid_run_id(id: &str) -> bool {
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
    record.runner_pid.map(pid_is_running)
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

fn read_units(s: &AppState, run_dir: &std::path::Path) -> Vec<UnitDto> {
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
