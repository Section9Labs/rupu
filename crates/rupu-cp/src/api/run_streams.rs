use crate::{error::ApiResult, state::AppState};
use axum::{extract::State, routing::get, Json, Router};
use rupu_runtime::{
    AutoflowCycleEventKind, AutoflowCycleRecord, AutoflowHistoryStore, AutoflowHistoryStoreError,
};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs/autoflows", get(list_autoflow_runs))
        .route("/api/runs/autoflows/events", get(list_autoflow_events))
        .route("/api/runs/agents", get(list_agent_runs))
}

/// Slim DTO returned for each autoflow cycle.
///
/// A *cycle* is a single batch tick of the autoflow worker — it may have
/// dispatched zero or more workflow runs. `run_ids` collects every `run_id`
/// found inside the cycle's embedded events (those that carry one).
#[derive(serde::Serialize)]
struct AutoflowCycleRow {
    cycle_id: String,
    mode: String,
    worker_name: Option<String>,
    started_at: String,
    finished_at: String,
    workflow_count: usize,
    ran_cycles: usize,
    skipped_cycles: usize,
    failed_cycles: usize,
    run_ids: Vec<String>,
}

impl From<AutoflowCycleRecord> for AutoflowCycleRow {
    fn from(r: AutoflowCycleRecord) -> Self {
        // Harvest every distinct run_id from the cycle's embedded event list.
        let mut run_ids: Vec<String> = r
            .events
            .iter()
            .filter_map(|e| e.run_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        run_ids.sort();

        // Stringify the mode enum via serde's snake_case tag.
        let mode = serde_json::to_value(r.mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("{:?}", r.mode).to_lowercase());

        Self {
            cycle_id: r.cycle_id,
            mode,
            worker_name: r.worker_name,
            started_at: r.started_at,
            finished_at: r.finished_at,
            workflow_count: r.workflow_count,
            ran_cycles: r.ran_cycles,
            skipped_cycles: r.skipped_cycles,
            failed_cycles: r.failed_cycles,
            run_ids,
        }
    }
}

// ---------------------------------------------------------------------------
// Agent runs — /api/runs/agents
// ---------------------------------------------------------------------------

/// Minimal CP-side projection of a standalone `<run_id>.meta.json`.
///
/// All fields use `#[serde(default)]` so that partial / evolving files still
/// parse. We deliberately avoid depending on `rupu-cli`'s full struct.
#[derive(Debug, Deserialize)]
struct StandaloneMetaDto {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    trigger_source: Option<String>,
}

/// Minimal CP-side projection of one entry in `session.json`'s `runs` array.
#[derive(Debug, Deserialize)]
struct SessionRunRecordDto {
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    /// `status` is serialised as `"ok"` / `"error"` / `"aborted"` by the CLI.
    #[serde(default)]
    status: Option<serde_json::Value>,
}

/// Minimal CP-side projection of `session.json`, capturing only what we need
/// for the runs list. `message_history` is not included (can be very large).
#[derive(Debug, Deserialize)]
struct SessionForRunsDto {
    #[serde(default)]
    agent_name: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    runs: Vec<SessionRunRecordDto>,
}

/// Wire row returned by `GET /api/runs/agents`.
#[derive(Serialize)]
struct AgentRunRow {
    run_id: String,
    source: &'static str, // "standalone" | "session"
    agent: Option<String>,
    session_id: Option<String>,
    trigger_source: Option<String>,
    status: Option<String>,
    started_at: Option<String>,
    transcript_path: Option<String>,
}

/// Stringify whatever serde_json::Value the status field carries.
fn stringify_status(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Load all `*.meta.json` files from `<global>/transcripts/` and convert to
/// `AgentRunRow`s with `source = "standalone"`.
fn collect_standalone_runs(global_dir: &std::path::Path) -> Vec<AgentRunRow> {
    let transcripts_dir = global_dir.join("transcripts");
    if !transcripts_dir.is_dir() {
        return Vec::new();
    }

    let entries = match std::fs::read_dir(&transcripts_dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                dir = %transcripts_dir.display(),
                error = %e,
                "failed to read transcripts directory"
            );
            return Vec::new();
        }
    };

    let mut rows = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.ends_with(".meta.json") {
            continue;
        }

        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable meta.json");
                continue;
            }
        };
        let dto = match serde_json::from_str::<StandaloneMetaDto>(&text) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unparseable meta.json");
                continue;
            }
        };

        // Derive the companion .jsonl path from the run_id.
        let transcript_path = if dto.run_id.is_empty() {
            None
        } else {
            Some(
                transcripts_dir
                    .join(format!("{}.jsonl", dto.run_id))
                    .to_string_lossy()
                    .into_owned(),
            )
        };

        rows.push(AgentRunRow {
            run_id: dto.run_id,
            source: "standalone",
            agent: None,
            session_id: dto.session_id,
            trigger_source: dto.trigger_source,
            status: None, // standalone meta does not carry run status
            started_at: None, // standalone meta does not carry a started_at field
            transcript_path,
        });
    }
    rows
}

/// Try to load and parse `session.json` at `path` as a `SessionForRunsDto`.
/// Returns `None` with a warning on any failure.
fn try_load_session_for_runs(path: &std::path::Path) -> Option<SessionForRunsDto> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "skipping unreadable session.json");
            return None;
        }
    };
    match serde_json::from_str::<SessionForRunsDto>(&text) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "skipping unparseable session.json");
            None
        }
    }
}

/// Scan `root` for `<id>/session.json` entries and yield `AgentRunRow`s with
/// `source = "session"` for every run embedded in each session.
fn collect_session_runs_from_dir(root: &std::path::Path, out: &mut Vec<AgentRunRow>) {
    if !root.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(dir = %root.display(), error = %e, "failed to read session directory");
            return;
        }
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let session_file = dir.join("session.json");
        if let Some(dto) = try_load_session_for_runs(&session_file) {
            for run in dto.runs {
                out.push(AgentRunRow {
                    run_id: run.run_id,
                    source: "session",
                    agent: dto.agent_name.clone(),
                    session_id: dto.session_id.clone(),
                    trigger_source: None,
                    status: run
                        .status
                        .as_ref()
                        .and_then(stringify_status),
                    started_at: run.started_at,
                    transcript_path: run.transcript_path,
                });
            }
        }
    }
}

/// `GET /api/runs/agents` — returns agent runs from both standalone transcripts
/// and session invocations, merged and sorted newest-first by `started_at`.
/// Missing directories return `[]` (no 500).
async fn list_agent_runs(State(s): State<AppState>) -> ApiResult<Json<Vec<AgentRunRow>>> {
    let mut rows = collect_standalone_runs(&s.global_dir);

    collect_session_runs_from_dir(&s.global_dir.join("sessions"), &mut rows);
    collect_session_runs_from_dir(&s.global_dir.join("sessions-archive"), &mut rows);

    // Sort newest-first: rows with a `started_at` string come before those
    // without. Among rows with timestamps, lexicographic descending order
    // works correctly for ISO-8601 strings.
    rows.sort_by(|a, b| match (&b.started_at, &a.started_at) {
        (Some(bt), Some(at)) => bt.cmp(at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Autoflow runs — /api/runs/autoflows
// ---------------------------------------------------------------------------

/// `GET /api/runs/autoflows` — returns the most-recent autoflow cycle records.
///
/// The store root matches the CLI canonical path: `<global_dir>/autoflows/history`.
/// A missing store directory is treated as "no cycles yet" and returns `[]`.
async fn list_autoflow_runs(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<AutoflowCycleRow>>> {
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let records = match store.list_recent(100) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };

    Ok(Json(records.into_iter().map(AutoflowCycleRow::from).collect()))
}

// ---------------------------------------------------------------------------
// Autoflow events — /api/runs/autoflows/events
// ---------------------------------------------------------------------------

/// One actionable autoflow *event* — a single launched run or awaiting/failed
/// signal, as opposed to a batch cycle tick.
///
/// This is the per-launch surface the Autoflows page leads with: each row maps
/// to a concrete `RunLaunched` / `AwaitingHuman` / `AwaitingExternal` /
/// `CycleFailed` event, carrying the workflow name, issue, and (when present)
/// the `run_id` that links straight to the run graph.
#[derive(Serialize)]
struct AutoflowEventRow {
    event_id: String,
    cycle_id: String,
    at: String,
    kind: String,
    workflow: Option<String>,
    issue_display_ref: Option<String>,
    run_id: Option<String>,
    status: Option<String>,
    worker_name: Option<String>,
}

/// Stringify an `AutoflowCycleEventKind` into its serde snake_case tag.
fn kind_to_snake_case(kind: AutoflowCycleEventKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{kind:?}").to_lowercase())
}

/// Only these event kinds represent actionable per-launch activity worth
/// surfacing as a row on the Autoflows page.
fn is_actionable_kind(kind: AutoflowCycleEventKind) -> bool {
    matches!(
        kind,
        AutoflowCycleEventKind::RunLaunched
            | AutoflowCycleEventKind::AwaitingHuman
            | AutoflowCycleEventKind::AwaitingExternal
            | AutoflowCycleEventKind::CycleFailed
    )
}

/// `GET /api/runs/autoflows/events` — returns the most-recent actionable
/// autoflow events (launched runs + awaiting/failed signals), newest-first.
///
/// The store root matches `/api/runs/autoflows`: `<global_dir>/autoflows/history`.
/// A missing store directory is treated as "no events yet" and returns `[]`.
async fn list_autoflow_events(
    State(s): State<AppState>,
) -> ApiResult<Json<Vec<AutoflowEventRow>>> {
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);

    let records = match store.list_recent_events(200) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };

    let rows = records
        .into_iter()
        .filter(|rec| is_actionable_kind(rec.event.kind))
        .map(|rec| AutoflowEventRow {
            event_id: rec.event_id,
            cycle_id: rec.cycle_id,
            at: rec.at,
            kind: kind_to_snake_case(rec.event.kind),
            workflow: rec.event.workflow,
            issue_display_ref: rec.event.issue_display_ref,
            run_id: rec.event.run_id,
            status: rec.event.status,
            worker_name: rec.worker_name,
        })
        .collect();

    Ok(Json(rows))
}
