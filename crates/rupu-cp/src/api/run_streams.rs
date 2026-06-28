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
use futures_util::future::join_all;
use rupu_runtime::{
    AutoflowCycleEventKind, AutoflowCycleRecord, AutoflowHistoryStore, AutoflowHistoryStoreError,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
                    status: run
                        .status
                        .as_ref()
                        .and_then(stringify_status),
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
        Some("active") => matches!(status, Some("running") | Some("awaiting_approval") | Some("pending")),
        Some("failed") => matches!(status, Some("error") | Some("failed") | Some("rejected") | Some("aborted")),
        Some("completed") => !matches!(
            status,
            Some("running") | Some("awaiting_approval") | Some("pending")
                | Some("error") | Some("failed") | Some("rejected") | Some("aborted")
        ),
        _ => true, // unknown group → no filter
    }
}

// ---------------------------------------------------------------------------
// Shared fan-out helper
// ---------------------------------------------------------------------------

/// Concurrently proxy `GET list_path` to every registered **remote** host,
/// tag each returned row JSON object with `"host_id": "<that host's id>"`,
/// and return the combined list (local_values + all remote rows).
///
/// `local_values` should already have `"host_id": "local"` set on each element.
///
/// Per-host failures emit a `tracing::warn` and contribute nothing — the
/// caller always gets a 200 even when some hosts are offline.
async fn fan_out_rows(
    hosts: &Arc<crate::host::registry::HostRegistry>,
    list_path: &str,
    local_values: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let all_hosts = hosts.list_hosts();
    let remote_hosts: Vec<_> = all_hosts.into_iter().filter(|h| h.id != "local").collect();

    if remote_hosts.is_empty() {
        return local_values;
    }

    let futs: Vec<_> = remote_hosts
        .into_iter()
        .map(|h| {
            let registry = Arc::clone(hosts);
            let path = list_path.to_string();
            async move {
                let conn = match registry.resolve(&h.id) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(
                            host_id = %h.id,
                            error = %e,
                            "fan_out_rows: could not resolve connector; skipping"
                        );
                        return Vec::<serde_json::Value>::new();
                    }
                };
                match conn.proxy_get_json(&path).await {
                    Ok(v) => {
                        let arr = match v.as_array() {
                            Some(a) => a.clone(),
                            None => {
                                tracing::warn!(
                                    host_id = %h.id,
                                    "fan_out_rows: remote returned non-array JSON; skipping"
                                );
                                return Vec::new();
                            }
                        };
                        let host_id = h.id;
                        arr.into_iter()
                            .map(|mut row| {
                                row["host_id"] = serde_json::json!(&host_id);
                                row
                            })
                            .collect()
                    }
                    Err(e) => {
                        tracing::warn!(
                            host_id = %h.id,
                            error = %e,
                            "fan_out_rows: proxy_get_json failed; skipping"
                        );
                        Vec::new()
                    }
                }
            }
        })
        .collect();

    let remote_results = join_all(futs).await;
    let mut all = local_values;
    all.extend(remote_results.into_iter().flatten());
    all
}

/// Sort a `Vec<Value>` newest-first using the string field named `time_field`.
/// Missing / null values sort after present values.
fn sort_values_newest_first(values: &mut [serde_json::Value], time_field: &str) {
    values.sort_by(|a, b| {
        let ta = a[time_field].as_str().unwrap_or("");
        let tb = b[time_field].as_str().unwrap_or("");
        tb.cmp(ta)
    });
}

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
        let path = {
            let mut p = "/api/runs/agents?host=local".to_string();
            if let Some(lc) = &q.lifecycle {
                p.push_str("&lifecycle=");
                p.push_str(lc);
            }
            if let Some(off) = q.offset {
                p.push_str(&format!("&offset={off}"));
            }
            if let Some(lim) = q.limit {
                p.push_str(&format!("&limit={lim}"));
            }
            p
        };
        let v = conn
            .proxy_get_json(&path)
            .await
            .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
        let arr = v.as_array().cloned().unwrap_or_default();
        return Ok(Json(
            arr.into_iter()
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
                let m = crate::usage::run_metrics_paths(
                    &[std::path::PathBuf::from(tp)],
                    &s.pricing,
                );
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

    let remote_path = format!(
        "/api/runs/agents?host=local&limit=10000{}",
        q.lifecycle
            .as_deref()
            .map(|lc| format!("&lifecycle={lc}"))
            .unwrap_or_default()
    );

    let mut all_values = fan_out_rows(&s.hosts, &remote_path, local_values).await;

    sort_values_newest_first(&mut all_values, "started_at");

    // Lifecycle filter after merge
    let lifecycle = q.lifecycle.as_deref();
    all_values.retain(|row| agent_in_lifecycle(row["status"].as_str(), lifecycle));

    let mut page_values = crate::pagination::paginate(all_values, &q.page());

    // Fill usage for local rows on this page only (remote rows already have it)
    for row in &mut page_values {
        if row["host_id"].as_str() == Some("local") {
            if let Some(tp) = row["transcript_path"].as_str() {
                let m = crate::usage::run_metrics_paths(
                    &[std::path::PathBuf::from(tp)],
                    &s.pricing,
                );
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
        let path = {
            let mut p = "/api/runs/autoflows?host=local".to_string();
            if let Some(off) = q.offset {
                p.push_str(&format!("&offset={off}"));
            }
            if let Some(lim) = q.limit {
                p.push_str(&format!("&limit={lim}"));
            }
            p
        };
        let v = conn
            .proxy_get_json(&path)
            .await
            .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
        let arr = v.as_array().cloned().unwrap_or_default();
        return Ok(Json(
            arr.into_iter()
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
    let local_rows: Vec<AutoflowCycleRow> =
        local_records.into_iter().map(AutoflowCycleRow::from).collect();

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

    let mut all_values =
        fan_out_rows(&s.hosts, "/api/runs/autoflows?host=local&limit=10000", local_values).await;

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
        let path = {
            let mut p = "/api/runs/autoflows/events?host=local".to_string();
            if let Some(off) = q.offset {
                p.push_str(&format!("&offset={off}"));
            }
            if let Some(lim) = q.limit {
                p.push_str(&format!("&limit={lim}"));
            }
            p
        };
        let v = conn
            .proxy_get_json(&path)
            .await
            .map_err(|e| crate::error::ApiError::internal(e.to_string()))?;
        let arr = v.as_array().cloned().unwrap_or_default();
        return Ok(Json(
            arr.into_iter()
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
            issue_display_ref: rec.event.issue_display_ref,
            run_id: rec.event.run_id,
            status: rec.event.status,
            worker_name: rec.worker_name,
            usage: crate::usage::UsageSummary::default(),
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

    let mut all_values = fan_out_rows(
        &s.hosts,
        "/api/runs/autoflows/events?host=local&limit=10000",
        local_values,
    )
    .await;

    sort_values_newest_first(&mut all_values, "at");

    let mut page_values = crate::pagination::paginate(all_values, &q.page());

    // Fill usage for local rows on this page (remote rows already have it)
    for row in &mut page_values {
        if row["host_id"].as_str() == Some("local") {
            if let Some(run_id) = row["run_id"].as_str() {
                row["usage"] =
                    serde_json::to_value(crate::usage::summarize_run(
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
}
