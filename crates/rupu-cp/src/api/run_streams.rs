// NOTE(multi-host slice 1.5): /api/runs/agents, /api/runs/autoflows, and
// /api/runs/autoflows/events are now host-aware — they fan out across all
// registered hosts when ?host= is absent or "all", tag every row with
// `host_id`, and tolerate per-host failures gracefully (warn + skip).
// Autoflow CLAIMS (/api/autoflows/claims) remain local-only.
// /api/runs and /api/runs/workflows are host-aware via Slice 1's fan_out_list_runs.

use crate::{error::ApiResult, state::AppState};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use rupu_runtime::{
    AutoflowCycleEventKind, AutoflowCycleRecord, AutoflowHistoryStore, AutoflowHistoryStoreError,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    usage: crate::usage::UsageSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
}

/// Harvest every distinct `run_id` embedded in a cycle record's events,
/// sorted. Shared by `AutoflowCycleRow::from` and
/// `LocalHostConnector::dashboard_summary`'s cycle-rollup builder
/// (`host/local.rs`) so there is exactly one place that reads run ids out of
/// an `AutoflowCycleRecord`.
pub(crate) fn harvest_run_ids(r: &AutoflowCycleRecord) -> Vec<String> {
    let mut run_ids: Vec<String> = r
        .events
        .iter()
        .filter_map(|e| e.run_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    run_ids.sort();
    run_ids
}

impl From<AutoflowCycleRecord> for AutoflowCycleRow {
    fn from(r: AutoflowCycleRecord) -> Self {
        let run_ids = harvest_run_ids(&r);

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
            usage: crate::usage::UsageSummary::default(),
            host_id: None,
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
    usage: crate::usage::UsageSummary,
    turns: u64,
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
}

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
    usage: crate::usage::UsageSummary,
    /// The on-disk event's `detail` field (the failure/error text for
    /// `cycle_failed` events). Additive/optional — omitted from the wire
    /// payload when absent so older clients see no shape change.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
}

/// Stringify whatever serde_json::Value the status field carries.
fn stringify_status(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    }
}

/// Read only the FIRST event line of a transcript `.jsonl` file and, if it is
/// a `run_start` event, return its `(agent, started_at)`. Deliberately does
/// NOT walk the rest of the file (that's `rupu_transcript::aggregate`'s job,
/// for usage rollups) — this is a cheap peek used purely to backfill the
/// standalone-run row's `agent`/`started_at` columns.
///
/// Tolerant by design: a missing file, an IO error, an empty file, a corrupt
/// first line, or a first line that isn't `run_start` all fall through to
/// `(None, None)` rather than erroring the row.
fn read_transcript_run_start(path: &std::path::Path) -> (Option<String>, Option<String>) {
    let mut iter = match rupu_transcript::JsonlReader::iter(path) {
        Ok(it) => it,
        Err(_) => return (None, None),
    };
    match iter.next() {
        Some(Ok(rupu_transcript::Event::RunStart {
            agent, started_at, ..
        })) => (Some(agent), Some(started_at.to_rfc3339())),
        _ => (None, None),
    }
}

/// Resolve `session_id`'s `agent_name` by loading its `session.json` from
/// either `<global>/sessions/<id>/` or `<global>/sessions-archive/<id>/`,
/// caching the (possibly-absent) result so a session shared by multiple
/// standalone rows is only read from disk once per request.
fn lookup_session_agent_name(
    global_dir: &std::path::Path,
    session_id: &str,
    cache: &mut HashMap<String, Option<String>>,
) -> Option<String> {
    if let Some(cached) = cache.get(session_id) {
        return cached.clone();
    }
    let agent = [
        global_dir.join("sessions").join(session_id),
        global_dir.join("sessions-archive").join(session_id),
    ]
    .iter()
    .find_map(|dir| try_load_session_for_runs(&dir.join("session.json")))
    .and_then(|dto| dto.agent_name);
    cache.insert(session_id.to_string(), agent.clone());
    agent
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

    // Per-request cache so a session.json shared by multiple standalone rows
    // (all `trigger_source == "session_turn"` runs from the same session) is
    // only read from disk once.
    let mut session_cache: HashMap<String, Option<String>> = HashMap::new();

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

        // The standalone meta.json genuinely has no `agent`/`started_at`
        // fields — recover them from the transcript's first line.
        let (mut agent, started_at) = match &transcript_path {
            Some(tp) => read_transcript_run_start(std::path::Path::new(tp)),
            None => (None, None),
        };

        // For session-turn runs whose transcript didn't yield an agent
        // (missing/corrupt transcript), fall back to the session's
        // `agent_name`.
        if agent.is_none() && dto.trigger_source.as_deref() == Some("session_turn") {
            if let Some(session_id) = &dto.session_id {
                agent = lookup_session_agent_name(global_dir, session_id, &mut session_cache);
            }
        }

        rows.push(AgentRunRow {
            run_id: dto.run_id,
            source: "standalone",
            agent,
            session_id: dto.session_id,
            trigger_source: dto.trigger_source,
            status: None, // standalone meta does not carry run status
            started_at,
            transcript_path,
            usage: crate::usage::UsageSummary::default(),
            turns: 0,
            duration_ms: None,
            host_id: None,
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
                    status: run.status.as_ref().and_then(stringify_status),
                    started_at: run.started_at,
                    transcript_path: run.transcript_path,
                    usage: crate::usage::UsageSummary::default(),
                    turns: 0,
                    duration_ms: None,
                    host_id: None,
                });
            }
        }
    }
}

#[derive(Deserialize)]
struct AgentRunsQuery {
    // Flat fields, NOT `#[serde(flatten)] PageQuery` — serde_urlencoded (axum
    // `Query`) cannot deserialize integers through a flattened struct.
    offset: Option<usize>,
    limit: Option<usize>,
    lifecycle: Option<String>,
    /// Absent or `"all"` → fan-out across all hosts.
    /// `"local"` → local only.
    /// Any other value → proxy to that remote host.
    #[serde(default)]
    host: Option<String>,
}

impl AgentRunsQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }
}

/// Classify an agent-run status string into a lifecycle group.
/// `active`: still in progress. `failed`: errored/aborted/rejected.
/// `completed`: everything else (ok/completed/None standalone runs).
fn agent_in_lifecycle(status: Option<&str>, group: Option<&str>) -> bool {
    match group {
        None => true,
        Some("active") => matches!(
            status,
            Some("running") | Some("awaiting_approval") | Some("pending")
        ),
        Some("failed") => matches!(
            status,
            Some("error") | Some("failed") | Some("rejected") | Some("aborted")
        ),
        Some("completed") => !matches!(
            status,
            Some("running")
                | Some("awaiting_approval")
                | Some("pending")
                | Some("error")
                | Some("failed")
                | Some("rejected")
                | Some("aborted")
        ),
        _ => true, // unknown group → no filter
    }
}

// ---------------------------------------------------------------------------
// Shared fan-out helpers (re-exported from host_fanout)
// ---------------------------------------------------------------------------

use crate::api::host_fanout::{fan_out_via, sort_values_newest_first};

// ---------------------------------------------------------------------------
// `GET /api/runs/agents`
// ---------------------------------------------------------------------------

/// `GET /api/runs/agents[?host=<id>]` — returns agent runs from both standalone
/// transcripts and session invocations, merged and sorted newest-first by
/// `started_at`.
///
/// `?host=local` or absent-with-no-remotes: returns only local runs.
/// `?host=all` or absent-with-remotes: fans out across all registered hosts and
/// tags every row with `host_id`.
/// `?host=<remote-id>`: proxies to that host and tags rows.
///
/// Missing directories return `[]` (no 500). Offline hosts contribute nothing
/// (warn + skip) and do not cause 500.
async fn list_agent_runs(
    State(s): State<AppState>,
    Query(q): Query<AgentRunsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let host = q.host.as_deref().unwrap_or("all");

    // ── Single remote host ────────────────────────────────────────────────────
    if host != "local" && host != "all" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        // Structured agent-run listing — works for SSH hosts (which can't serve
        // the generic proxy) by shelling `rupu transcript list`.
        let mut rows = conn
            .list_agent_runs()
            .await
            .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
        let lifecycle = q.lifecycle.as_deref();
        rows.retain(|r| agent_in_lifecycle(r.get("status").and_then(|v| v.as_str()), lifecycle));
        return Ok(Json(
            crate::pagination::paginate(rows, &q.page())
                .into_iter()
                .map(|mut row| {
                    row["host_id"] = serde_json::json!(host);
                    row
                })
                .collect(),
        ));
    }

    // ── Collect local runs ────────────────────────────────────────────────────
    let mut local_rows = collect_standalone_runs(&s.global_dir);
    collect_session_runs_from_dir(&s.global_dir.join("sessions"), &mut local_rows);
    collect_session_runs_from_dir(&s.global_dir.join("sessions-archive"), &mut local_rows);

    // Sort newest-first: rows with a timestamp sort before those without;
    // ISO-8601 strings sort lexicographically.
    local_rows.sort_by(|a, b| match (&b.started_at, &a.started_at) {
        (Some(bt), Some(at)) => bt.cmp(at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    // ── Local-only path ───────────────────────────────────────────────────────
    if host == "local" {
        let lifecycle = q.lifecycle.clone();
        local_rows.retain(|r| agent_in_lifecycle(r.status.as_deref(), lifecycle.as_deref()));
        let mut page_rows = crate::pagination::paginate(local_rows, &q.page());
        for row in &mut page_rows {
            row.host_id = Some("local".to_string());
            if let Some(tp) = &row.transcript_path {
                let m =
                    crate::usage::run_metrics_paths(&[std::path::PathBuf::from(tp)], &s.pricing);
                row.usage = m.usage;
                row.turns = m.turns;
                row.duration_ms = m.duration_ms;
            }
        }
        return Ok(Json(
            page_rows
                .into_iter()
                .map(|r| serde_json::to_value(r).unwrap())
                .collect(),
        ));
    }

    // ── Fan-out path (host == "all") ──────────────────────────────────────────
    let local_values: Vec<serde_json::Value> = local_rows
        .into_iter()
        .map(|mut r| {
            r.host_id = Some("local".to_string());
            serde_json::to_value(r).unwrap()
        })
        .collect();

    let mut all_values = fan_out_via(&s.hosts, local_values, "agent_runs", |c| async move {
        c.list_agent_runs().await
    })
    .await;

    sort_values_newest_first(&mut all_values, "started_at");

    // Lifecycle filter after merge
    let lifecycle = q.lifecycle.as_deref();
    all_values.retain(|row| agent_in_lifecycle(row["status"].as_str(), lifecycle));

    let mut page_values = crate::pagination::paginate(all_values, &q.page());

    // Fill usage for local rows on this page only (remote rows already have it)
    for row in &mut page_values {
        if row["host_id"].as_str() == Some("local") {
            if let Some(tp) = row["transcript_path"].as_str() {
                let m =
                    crate::usage::run_metrics_paths(&[std::path::PathBuf::from(tp)], &s.pricing);
                row["usage"] = serde_json::to_value(m.usage).unwrap();
                row["turns"] = serde_json::json!(m.turns);
                row["duration_ms"] = serde_json::to_value(m.duration_ms).unwrap();
            }
        }
    }

    Ok(Json(page_values))
}

// ---------------------------------------------------------------------------
// Autoflow runs — /api/runs/autoflows
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AutoflowRunsQuery {
    // Flat fields, NOT `#[serde(flatten)] PageQuery` — serde_urlencoded (axum
    // `Query`) cannot deserialize integers through a flattened struct.
    offset: Option<usize>,
    limit: Option<usize>,
    /// Absent or `"all"` → fan-out across all hosts.
    /// `"local"` → local only.
    /// Any other value → proxy to that remote host.
    #[serde(default)]
    host: Option<String>,
}

impl AutoflowRunsQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }
}

/// `GET /api/runs/autoflows[?host=<id>]` — returns the most-recent autoflow
/// cycle records.
///
/// `?host=local` or absent-with-no-remotes: local store only.
/// `?host=all` or absent-with-remotes: fan-out across all hosts.
/// `?host=<remote-id>`: proxy to that host.
///
/// A missing store directory is treated as "no cycles yet" and returns `[]`.
/// Offline hosts contribute nothing (warn + skip) and do not cause 500.
async fn list_autoflow_runs(
    State(s): State<AppState>,
    Query(q): Query<AutoflowRunsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let host = q.host.as_deref().unwrap_or("all");

    // ── Single remote host ────────────────────────────────────────────────────
    if host != "local" && host != "all" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        // Structured autoflow-cycle listing — SSH hosts aggregate cycles from
        // `rupu autoflow history` instead of the generic proxy.
        let rows = conn
            .list_autoflow_runs()
            .await
            .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
        return Ok(Json(
            crate::pagination::paginate(rows, &q.page())
                .into_iter()
                .map(|mut row| {
                    row["host_id"] = serde_json::json!(host);
                    row
                })
                .collect(),
        ));
    }

    // ── Load local cycles ─────────────────────────────────────────────────────
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);
    let local_records = match store.list_recent(100) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };
    let local_rows: Vec<AutoflowCycleRow> = local_records
        .into_iter()
        .map(AutoflowCycleRow::from)
        .collect();

    // ── Local-only path ───────────────────────────────────────────────────────
    if host == "local" {
        // list_recent already returns newest-first. Convert, paginate, then roll
        // up usage across each cycle's runs on the page only.
        let mut page_rows = crate::pagination::paginate(local_rows, &q.page());
        for row in &mut page_rows {
            row.host_id = Some("local".to_string());
            row.usage = crate::usage::rollup(
                row.run_ids
                    .iter()
                    .map(|id| crate::usage::summarize_run(&s.run_store, id, &s.pricing)),
            );
        }
        return Ok(Json(
            page_rows
                .into_iter()
                .map(|r| serde_json::to_value(r).unwrap())
                .collect(),
        ));
    }

    // ── Fan-out path (host == "all") ──────────────────────────────────────────
    let local_values: Vec<serde_json::Value> = local_rows
        .into_iter()
        .map(|mut r| {
            r.host_id = Some("local".to_string());
            // Usage is filled after pagination for local rows on the page.
            serde_json::to_value(r).unwrap()
        })
        .collect();

    let mut all_values = fan_out_via(&s.hosts, local_values, "autoflow_runs", |c| async move {
        c.list_autoflow_runs().await
    })
    .await;

    sort_values_newest_first(&mut all_values, "started_at");

    let mut page_values = crate::pagination::paginate(all_values, &q.page());

    // Fill usage for local rows on this page (remote rows already have it)
    for row in &mut page_values {
        if row["host_id"].as_str() == Some("local") {
            let run_ids: Vec<String> = row["run_ids"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            row["usage"] = serde_json::to_value(crate::usage::rollup(
                run_ids
                    .iter()
                    .map(|id| crate::usage::summarize_run(&s.run_store, id, &s.pricing)),
            ))
            .unwrap();
        }
    }

    Ok(Json(page_values))
}

// ---------------------------------------------------------------------------
// Autoflow events — /api/runs/autoflows/events
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AutoflowEventsQuery {
    // Flat fields — same serde_urlencoded reason as above.
    offset: Option<usize>,
    limit: Option<usize>,
    /// Absent or `"all"` → fan-out across all hosts.
    /// `"local"` → local only.
    /// Any other value → proxy to that remote host.
    #[serde(default)]
    host: Option<String>,
}

impl AutoflowEventsQuery {
    fn page(&self) -> crate::pagination::PageQuery {
        crate::pagination::PageQuery {
            offset: self.offset,
            limit: self.limit,
        }
    }
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

/// `GET /api/runs/autoflows/events[?host=<id>]` — returns the most-recent
/// actionable autoflow events (launched runs + awaiting/failed signals),
/// newest-first.
///
/// `?host=local` or absent-with-no-remotes: local store only.
/// `?host=all` or absent-with-remotes: fan-out across all hosts.
/// `?host=<remote-id>`: proxy to that host.
///
/// A missing store directory is treated as "no events yet" and returns `[]`.
/// Offline hosts contribute nothing (warn + skip) and do not cause 500.
async fn list_autoflow_events(
    State(s): State<AppState>,
    Query(q): Query<AutoflowEventsQuery>,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    let host = q.host.as_deref().unwrap_or("all");

    // ── Single remote host ────────────────────────────────────────────────────
    if host != "local" && host != "all" {
        let conn = crate::api::runs::resolve_host(&s, host)?;
        // Structured autoflow-event listing — SSH hosts source events from
        // `rupu autoflow history` instead of the generic proxy.
        let rows = conn
            .list_autoflow_events()
            .await
            .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
        return Ok(Json(
            crate::pagination::paginate(rows, &q.page())
                .into_iter()
                .map(|mut row| {
                    row["host_id"] = serde_json::json!(host);
                    row
                })
                .collect(),
        ));
    }

    // ── Load local events ─────────────────────────────────────────────────────
    let store_root = s.global_dir.join("autoflows").join("history");
    let store = AutoflowHistoryStore::new(store_root);
    let local_records = match store.list_recent_events(200) {
        Ok(r) => r,
        Err(AutoflowHistoryStoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(e) => return Err(crate::error::ApiError::internal(e.to_string())),
    };

    let local_rows: Vec<AutoflowEventRow> = local_records
        .into_iter()
        .filter(|rec| is_actionable_kind(rec.event.kind))
        .map(|rec| AutoflowEventRow {
            event_id: rec.event_id,
            cycle_id: rec.cycle_id,
            at: rec.at,
            kind: kind_to_snake_case(rec.event.kind),
            workflow: rec.event.workflow,
            // `cycle_failed` events only ever populate `issue_ref` (no
            // display-friendly variant is computed for them) — fall back so
            // the UI still gets an issue reference to show. Mirrors
            // `run_resolve.rs`'s `entity` field derivation.
            issue_display_ref: rec.event.issue_display_ref.or(rec.event.issue_ref),
            run_id: rec.event.run_id,
            status: rec.event.status,
            worker_name: rec.worker_name,
            usage: crate::usage::UsageSummary::default(),
            detail: rec.event.detail,
            host_id: None,
        })
        .collect();

    // ── Local-only path ───────────────────────────────────────────────────────
    if host == "local" {
        let mut page_rows = crate::pagination::paginate(local_rows, &q.page());
        for row in &mut page_rows {
            row.host_id = Some("local".to_string());
            if let Some(id) = &row.run_id {
                row.usage = crate::usage::summarize_run(&s.run_store, id, &s.pricing);
            }
        }
        return Ok(Json(
            page_rows
                .into_iter()
                .map(|r| serde_json::to_value(r).unwrap())
                .collect(),
        ));
    }

    // ── Fan-out path (host == "all") ──────────────────────────────────────────
    let local_values: Vec<serde_json::Value> = local_rows
        .into_iter()
        .map(|mut r| {
            r.host_id = Some("local".to_string());
            // Usage filled after pagination for local rows on the page.
            serde_json::to_value(r).unwrap()
        })
        .collect();

    let mut all_values = fan_out_via(&s.hosts, local_values, "autoflow_events", |c| async move {
        c.list_autoflow_events().await
    })
    .await;

    sort_values_newest_first(&mut all_values, "at");

    let mut page_values = crate::pagination::paginate(all_values, &q.page());

    // Fill usage for local rows on this page (remote rows already have it)
    for row in &mut page_values {
        if row["host_id"].as_str() == Some("local") {
            if let Some(run_id) = row["run_id"].as_str() {
                row["usage"] = serde_json::to_value(crate::usage::summarize_run(
                    &s.run_store,
                    run_id,
                    &s.pricing,
                ))
                .unwrap();
            }
        }
    }

    Ok(Json(page_values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_runtime::{AutoflowCycleEvent, AutoflowCycleMode, AutoflowHistoryEventRecord};
    use std::fs;

    #[test]
    fn agent_lifecycle_classifies() {
        assert!(agent_in_lifecycle(Some("running"), Some("active")));
        assert!(agent_in_lifecycle(Some("error"), Some("failed")));
        assert!(agent_in_lifecycle(Some("aborted"), Some("failed")));
        assert!(agent_in_lifecycle(Some("ok"), Some("completed")));
        assert!(agent_in_lifecycle(None, Some("completed"))); // standalone, no status → completed
        assert!(!agent_in_lifecycle(Some("running"), Some("completed")));
        assert!(agent_in_lifecycle(Some("running"), None)); // no filter → all
    }

    // ── A1: collect_standalone_runs fills agent + started_at ──────────────────

    /// Write `<global>/transcripts/<run_id>.meta.json` + a matching
    /// `.jsonl` transcript whose first line is (optionally) a `run_start`
    /// event, or arbitrary text when `first_line` is `Some(garbage)`.
    fn write_standalone_meta(
        global: &std::path::Path,
        run_id: &str,
        session_id: Option<&str>,
        trigger_source: Option<&str>,
    ) {
        let dir = global.join("transcripts");
        fs::create_dir_all(&dir).unwrap();
        let meta = serde_json::json!({
            "run_id": run_id,
            "session_id": session_id,
            "trigger_source": trigger_source,
        });
        fs::write(
            dir.join(format!("{run_id}.meta.json")),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();
    }

    fn write_transcript_run_start(
        global: &std::path::Path,
        run_id: &str,
        agent: &str,
        started_at: &str,
    ) {
        let dir = global.join("transcripts");
        fs::create_dir_all(&dir).unwrap();
        let ev = rupu_transcript::Event::RunStart {
            run_id: run_id.to_string(),
            workspace_id: "ws".into(),
            agent: agent.to_string(),
            provider: "anthropic".into(),
            model: "claude".into(),
            started_at: started_at.parse().unwrap(),
            mode: rupu_transcript::RunMode::Ask,
        };
        let line = serde_json::to_string(&ev).unwrap();
        fs::write(dir.join(format!("{run_id}.jsonl")), format!("{line}\n")).unwrap();
    }

    fn write_session_json(
        global: &std::path::Path,
        dirname: &str,
        session_id: &str,
        agent_name: &str,
    ) {
        let dir = global.join(dirname).join(session_id);
        fs::create_dir_all(&dir).unwrap();
        let session = serde_json::json!({
            "agent_name": agent_name,
            "session_id": session_id,
            "runs": [],
        });
        fs::write(
            dir.join("session.json"),
            serde_json::to_string(&session).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn standalone_run_fills_agent_and_started_at_from_transcript_run_start() {
        let tmp = tempfile::tempdir().unwrap();
        write_standalone_meta(tmp.path(), "run_a", None, None);
        write_transcript_run_start(tmp.path(), "run_a", "reviewer", "2026-01-01T00:00:00Z");

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent.as_deref(), Some("reviewer"));
        assert_eq!(
            rows[0].started_at.as_deref(),
            Some("2026-01-01T00:00:00+00:00")
        );
    }

    #[test]
    fn standalone_run_missing_transcript_leaves_fields_none_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        // meta.json present, but no companion .jsonl at all.
        write_standalone_meta(tmp.path(), "run_b", None, None);

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent, None);
        assert_eq!(rows[0].started_at, None);
    }

    #[test]
    fn standalone_run_corrupt_first_line_leaves_fields_none_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        write_standalone_meta(tmp.path(), "run_c", None, None);
        let dir = tmp.path().join("transcripts");
        fs::write(dir.join("run_c.jsonl"), "not json at all\n").unwrap();

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent, None);
        assert_eq!(rows[0].started_at, None);
    }

    #[test]
    fn session_turn_run_falls_back_to_session_json_agent_name() {
        let tmp = tempfile::tempdir().unwrap();
        // meta references a session but the transcript itself has no usable
        // run_start (missing entirely here) — the session_turn fallback should
        // still resolve the agent from session.json.
        write_standalone_meta(tmp.path(), "run_d", Some("sess_1"), Some("session_turn"));
        write_session_json(tmp.path(), "sessions", "sess_1", "fixer");

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent.as_deref(), Some("fixer"));
    }

    #[test]
    fn session_turn_run_checks_sessions_archive_too() {
        let tmp = tempfile::tempdir().unwrap();
        write_standalone_meta(tmp.path(), "run_e", Some("sess_2"), Some("session_turn"));
        write_session_json(tmp.path(), "sessions-archive", "sess_2", "archived-fixer");

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent.as_deref(), Some("archived-fixer"));
    }

    #[test]
    fn two_session_turn_rows_sharing_a_session_both_resolve_from_one_session_json() {
        let tmp = tempfile::tempdir().unwrap();
        write_standalone_meta(tmp.path(), "run_f1", Some("sess_3"), Some("session_turn"));
        write_standalone_meta(tmp.path(), "run_f2", Some("sess_3"), Some("session_turn"));
        write_session_json(tmp.path(), "sessions", "sess_3", "shared-agent");

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.agent.as_deref(), Some("shared-agent"));
        }
    }

    #[test]
    fn transcript_agent_takes_priority_over_session_turn_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        write_standalone_meta(tmp.path(), "run_g", Some("sess_4"), Some("session_turn"));
        write_transcript_run_start(
            tmp.path(),
            "run_g",
            "from-transcript",
            "2026-02-02T00:00:00Z",
        );
        write_session_json(tmp.path(), "sessions", "sess_4", "from-session");

        let rows = collect_standalone_runs(tmp.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent.as_deref(), Some("from-transcript"));
    }

    // ── A2: autoflow event DTO forwards detail + issue_ref fallback ────────────

    fn cycle_failed_record(
        detail: Option<&str>,
        issue_ref: Option<&str>,
    ) -> AutoflowHistoryEventRecord {
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, Utc::now());
        let event = AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::CycleFailed,
            issue_ref: issue_ref.map(str::to_owned),
            issue_display_ref: None,
            detail: detail.map(str::to_owned),
            ..Default::default()
        };
        AutoflowHistoryEventRecord::from_cycle_event(&cycle, event, Utc::now())
    }

    #[test]
    fn autoflow_event_row_forwards_detail_and_falls_back_issue_display_ref() {
        let rec = cycle_failed_record(
            Some("workflow validation failed: missing step"),
            Some("github:acme/widgets#7"),
        );
        let row = AutoflowEventRow {
            event_id: rec.event_id.clone(),
            cycle_id: rec.cycle_id.clone(),
            at: rec.at.clone(),
            kind: kind_to_snake_case(rec.event.kind),
            workflow: rec.event.workflow.clone(),
            issue_display_ref: rec
                .event
                .issue_display_ref
                .clone()
                .or_else(|| rec.event.issue_ref.clone()),
            run_id: rec.event.run_id.clone(),
            status: rec.event.status.clone(),
            worker_name: rec.worker_name.clone(),
            usage: crate::usage::UsageSummary::default(),
            detail: rec.event.detail.clone(),
            host_id: None,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(
            v["detail"],
            serde_json::json!("workflow validation failed: missing step")
        );
        assert_eq!(
            v["issue_display_ref"],
            serde_json::json!("github:acme/widgets#7")
        );
    }

    #[test]
    fn autoflow_event_row_omits_detail_when_absent() {
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, Utc::now());
        let event = AutoflowCycleEvent {
            kind: AutoflowCycleEventKind::RunLaunched,
            run_id: Some("run_x".into()),
            ..Default::default()
        };
        let rec = AutoflowHistoryEventRecord::from_cycle_event(&cycle, event, Utc::now());
        let row = AutoflowEventRow {
            event_id: rec.event_id.clone(),
            cycle_id: rec.cycle_id.clone(),
            at: rec.at.clone(),
            kind: kind_to_snake_case(rec.event.kind),
            workflow: rec.event.workflow.clone(),
            issue_display_ref: rec
                .event
                .issue_display_ref
                .clone()
                .or_else(|| rec.event.issue_ref.clone()),
            run_id: rec.event.run_id.clone(),
            status: rec.event.status.clone(),
            worker_name: rec.worker_name.clone(),
            usage: crate::usage::UsageSummary::default(),
            detail: rec.event.detail.clone(),
            host_id: None,
        };
        let s = serde_json::to_string(&row).unwrap();
        assert!(!s.contains("\"detail\""));
        assert_eq!(row.run_id.as_deref(), Some("run_x"));
    }
}
