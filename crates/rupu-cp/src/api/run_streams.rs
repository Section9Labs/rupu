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
///
/// `started_at` MUST be formatted with a `Z` suffix, NOT `.to_rfc3339()`'s
/// `+00:00` — see `83c3494c` ("serialize run list timestamps with serde, not
/// to_rfc3339()"). `AgentRunRow.started_at` mixes this transcript-derived
/// value with session-branch rows whose `started_at` was serde-serialized
/// from a `chrono::DateTime<Utc>` (which emits `Z`), and both
/// `list_agent_runs`'s local sort and `host_fanout::sort_values_newest_first`
/// merge them with a plain LEXICOGRAPHIC string compare. `'+'` (0x2B) sorts
/// before `'Z'` (0x5A), so a `+00:00`-suffixed row silently sorts as older
/// than it is. Do not "tidy" this back into `.to_rfc3339()`.
fn read_transcript_run_start(path: &std::path::Path) -> (Option<String>, Option<String>) {
    let mut iter = match rupu_transcript::JsonlReader::iter(path) {
        Ok(it) => it,
        Err(_) => return (None, None),
    };
    match iter.next() {
        Some(Ok(rupu_transcript::Event::RunStart {
            agent, started_at, ..
        })) => (
            Some(agent),
            Some(started_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
        ),
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

/// Dedupe `rows` by `run_id`, merging any row that was collected from BOTH
/// `collect_standalone_runs` (`<global>/transcripts/*.meta.json`) and
/// `collect_session_runs_from_dir` (`session.json`'s `runs[]` array).
///
/// A `trigger_source == "session_turn"` run is recorded on disk in BOTH
/// places, so before this pass every such run_id appears twice in the
/// concatenated list — 55 duplicate rows on the operator's data (2026-07-23
/// feedback round). Merge rule:
///   - Every field prefers whichever side has a non-`None` value.
///   - When BOTH sides carry a value, the SESSION-derived value wins by
///     default (sessions carry the real end-state, e.g. `status`) — EXCEPT
///     `trigger_source`, where the STANDALONE value wins, because
///     `collect_session_runs_from_dir` never sets it (there being no real
///     conflict, standalone's value must simply survive the merge).
///   - `source` is recomputed on the merged row rather than kept from
///     whichever side happened to be seen first: `"session"` when the
///     merged row carries a `session_id` (the run belongs to a session),
///     else `"standalone"` — preserving the DTO's existing meaning of
///     `source` for the one row that now represents both origins.
///
/// Rows are matched by `run_id`, EXCEPT an empty `run_id` (a defensively
/// tolerated but never-expected shape from a corrupt `.meta.json` — see
/// `StandaloneMetaDto`'s `#[serde(default)]`) never collides with anything,
/// even another empty `run_id` row — merging those would silently fuse two
/// unrelated broken records into one.
///
/// Preserves the first-seen order of non-colliding rows (stable).
fn dedupe_agent_runs_by_run_id(rows: Vec<AgentRunRow>) -> Vec<AgentRunRow> {
    let mut order: Vec<String> = Vec::with_capacity(rows.len());
    let mut by_key: HashMap<String, AgentRunRow> = HashMap::with_capacity(rows.len());

    for (idx, row) in rows.into_iter().enumerate() {
        let key = if row.run_id.is_empty() {
            format!("\u{0}empty#{idx}")
        } else {
            row.run_id.clone()
        };
        match by_key.remove(&key) {
            Some(existing) => {
                by_key.insert(key, merge_agent_run_rows(existing, row));
                // `key` is already present in `order` from its first insertion.
            }
            None => {
                order.push(key.clone());
                by_key.insert(key, row);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

/// Merge two `AgentRunRow`s that collided on the same `run_id` — see
/// `dedupe_agent_runs_by_run_id`'s doc comment for the merge rule.
fn merge_agent_run_rows(a: AgentRunRow, b: AgentRunRow) -> AgentRunRow {
    // Identify the session-sourced side for the field-level tie-breaks
    // below. In the realistic case exactly one of `a`/`b` is
    // `source == "session"` (the other `"standalone"`) — a session-turn run
    // colliding across the two collectors. If BOTH (or neither) happen to be
    // tagged the same way (e.g. the same session recorded under both
    // `sessions/` and `sessions-archive/`), there is no real
    // session-vs-standalone distinction to make; `a` is treated as the
    // "session" side arbitrarily so the merge stays deterministic.
    let (session, standalone) = if b.source == "session" && a.source != "session" {
        (b, a)
    } else {
        (a, b)
    };

    let session_id = session.session_id.or(standalone.session_id);
    let source: &'static str = if session_id.is_some() {
        "session"
    } else {
        "standalone"
    };

    AgentRunRow {
        run_id: session.run_id, // == standalone.run_id (the dedupe key)
        source,
        agent: session.agent.or(standalone.agent),
        session_id,
        // Sessions never set `trigger_source` (see
        // `collect_session_runs_from_dir`) — standalone's value must win or
        // it's lost entirely on merge.
        trigger_source: standalone.trigger_source.or(session.trigger_source),
        // Standalone meta.json never sets `status` — session's real
        // end-state wins whenever present.
        status: session.status.or(standalone.status),
        started_at: session.started_at.or(standalone.started_at),
        transcript_path: session.transcript_path.or(standalone.transcript_path),
        // Usage/turns/duration are filled in AFTER this merge (by
        // `list_agent_runs`, keyed off the merged `transcript_path`) — the
        // defaults carried by both operands here are inert.
        usage: crate::usage::UsageSummary::default(),
        turns: 0,
        duration_ms: session.duration_ms.or(standalone.duration_ms),
        host_id: session.host_id.or(standalone.host_id),
    }
}

/// Collect every local agent-run row (standalone transcripts + active and
/// archived session runs), dedupe by `run_id` (see
/// `dedupe_agent_runs_by_run_id`), and sort newest-first by `started_at`.
///
/// Extracted out of `list_agent_runs` so it's directly unit-testable: the
/// standalone and session branches populate `started_at` from two different
/// sources (a transcript's `run_start` line vs. a session.json field
/// deserialized as a plain `String`), and both this sort AND
/// `host_fanout::sort_values_newest_first`'s fan-out merge compare those
/// strings LEXICOGRAPHICALLY. If the two sources ever disagree on timestamp
/// format (e.g. one emits `+00:00`, the other `Z`), rows silently mis-order —
/// see `read_transcript_run_start`'s doc comment and `83c3494c`.
fn collect_and_sort_local_agent_runs(global_dir: &std::path::Path) -> Vec<AgentRunRow> {
    let mut local_rows = collect_standalone_runs(global_dir);
    collect_session_runs_from_dir(&global_dir.join("sessions"), &mut local_rows);
    collect_session_runs_from_dir(&global_dir.join("sessions-archive"), &mut local_rows);

    let mut local_rows = dedupe_agent_runs_by_run_id(local_rows);

    // Sort newest-first: rows with a timestamp sort before those without;
    // ISO-8601 strings sort lexicographically.
    local_rows.sort_by(|a, b| match (&b.started_at, &a.started_at) {
        (Some(bt), Some(at)) => bt.cmp(at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    local_rows
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
    let mut local_rows = collect_and_sort_local_agent_runs(&s.global_dir);

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

    /// Like `write_session_json`, but embeds one run entry — the shape
    /// `collect_session_runs_from_dir` actually reads `started_at` from
    /// (`SessionRunRecordDto`), for sort-order regression tests.
    fn write_session_json_with_run(
        global: &std::path::Path,
        dirname: &str,
        session_id: &str,
        run_id: &str,
        started_at: &str,
    ) {
        let dir = global.join(dirname).join(session_id);
        fs::create_dir_all(&dir).unwrap();
        let session = serde_json::json!({
            "agent_name": "session-agent",
            "session_id": session_id,
            "runs": [
                {
                    "run_id": run_id,
                    "started_at": started_at,
                    "status": "ok",
                }
            ],
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
        assert_eq!(rows[0].started_at.as_deref(), Some("2026-01-01T00:00:00Z"));
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

    /// Regression for the timestamp-format sort bug fixed in `83c3494c`
    /// ("serialize run list timestamps with serde, not to_rfc3339()") and
    /// reintroduced here: `read_transcript_run_start` must emit a `Z` suffix,
    /// matching the `Z` that session-branch rows already carry (their
    /// `started_at` is whatever was serde-serialized to `session.json` on
    /// disk), because both `list_agent_runs`'s local sort and
    /// `host_fanout::sort_values_newest_first`'s fan-out merge order rows
    /// with a plain LEXICOGRAPHIC string compare.
    ///
    /// A standalone (transcript-derived) row and a session row at the exact
    /// SAME instant are the deterministic way to pin this: digit differences
    /// anywhere in the date/time (even a single second) always decide a
    /// lexicographic comparison before either string's offset suffix is
    /// reached, so only an exact tie ever reaches the suffix — where `'+'`
    /// (0x2B) sorting before `'Z'` (0x5A) used to make a `+00:00`-suffixed
    /// row compare as strictly GREATER than (not equal to) an otherwise
    /// identical `Z`-suffixed row. Under the old `.to_rfc3339()` output that
    /// broke the stable sort's tie: the standalone row (inserted first, by
    /// `collect_standalone_runs`) got pushed behind the session row despite
    /// sharing its timestamp. With the `Z` fix the two compare as truly
    /// equal, and the stable sort keeps the standalone row first (its
    /// original position).
    #[test]
    fn standalone_and_session_rows_at_the_same_instant_are_not_misordered_by_timestamp_notation() {
        let tmp = tempfile::tempdir().unwrap();
        // Standalone row: agent/started_at recovered from the transcript's
        // run_start line via `read_transcript_run_start` (the code path this
        // regression protects).
        write_standalone_meta(tmp.path(), "run_tie_standalone", None, None);
        write_transcript_run_start(
            tmp.path(),
            "run_tie_standalone",
            "tie-agent",
            "2026-03-01T10:05:00Z",
        );
        // Session row: started_at is whatever's on disk verbatim — the
        // realistic shape is a serde-serialized `Z` string, at the SAME
        // instant as the standalone row above.
        write_session_json_with_run(
            tmp.path(),
            "sessions",
            "sess_tie",
            "run_tie_session",
            "2026-03-01T10:05:00Z",
        );

        let rows = collect_and_sort_local_agent_runs(tmp.path());
        assert_eq!(rows.len(), 2);
        // Sanity: both rows genuinely share the same started_at string —
        // this is a tie, not a "which is chronologically later" case.
        assert_eq!(rows[0].started_at, rows[1].started_at);
        // The stable sort must not reorder a tie: the standalone row,
        // collected first, stays first. Under the old `.to_rfc3339()`
        // (`+00:00`) output this assertion fails — the session row's `Z`
        // suffix compares as greater, pushing the standalone row second.
        assert_eq!(rows[0].run_id, "run_tie_standalone");
        assert_eq!(rows[1].run_id, "run_tie_session");
    }

    // ── Amendment #2 (2026-07-23 feedback round): dedupe by run_id ─────────────

    /// A session-turn run recorded in BOTH the standalone `.meta.json` AND
    /// its session's `runs[]` array (the exact shape that produced 55
    /// duplicate rows on the operator's data) collapses to ONE row, with
    /// fields merged per `dedupe_agent_runs_by_run_id`'s documented rule.
    #[test]
    fn session_turn_run_present_in_both_sources_dedupes_to_one_merged_row() {
        let tmp = tempfile::tempdir().unwrap();
        // Standalone side: agent resolved from the transcript, carries
        // trigger_source, never sets status.
        write_standalone_meta(
            tmp.path(),
            "run_dup",
            Some("sess_dup"),
            Some("session_turn"),
        );
        write_transcript_run_start(tmp.path(), "run_dup", "dup-agent", "2026-04-01T00:00:00Z");
        // Session side: same run_id embedded in session.json's runs[], with
        // a status (session.json's runs[] always has one) and a
        // deliberately different agent name, to pin the tie-break.
        write_session_json_with_run(
            tmp.path(),
            "sessions",
            "sess_dup",
            "run_dup",
            "2026-04-01T00:00:05Z",
        );

        let rows = collect_and_sort_local_agent_runs(tmp.path());
        assert_eq!(
            rows.len(),
            1,
            "the two sources' run_dup rows must collapse to one"
        );
        let row = &rows[0];
        assert_eq!(row.run_id, "run_dup");
        // session_id present on the merged row ⇒ source recomputed as
        // "session", regardless of which side inserted first.
        assert_eq!(row.source, "session");
        assert_eq!(row.session_id.as_deref(), Some("sess_dup"));
        // trigger_source only the standalone side ever sets — it must
        // survive the merge, not get clobbered by the session side's None.
        assert_eq!(row.trigger_source.as_deref(), Some("session_turn"));
        // status: only session.json's runs[] entries carry one
        // (`write_session_json_with_run` sets `"ok"`) — session wins.
        assert_eq!(row.status.as_deref(), Some("ok"));
        // agent: both sides have a value here — session wins per the
        // documented default tie-break.
        assert_eq!(row.agent.as_deref(), Some("session-agent"));
    }

    /// Two rows from different sources with DIFFERENT run_ids are untouched
    /// by the dedupe pass — no merging, no dropped rows.
    #[test]
    fn distinct_run_ids_across_sources_are_unaffected_by_dedupe() {
        let tmp = tempfile::tempdir().unwrap();
        write_standalone_meta(tmp.path(), "run_solo_standalone", None, None);
        write_transcript_run_start(
            tmp.path(),
            "run_solo_standalone",
            "solo-agent",
            "2026-04-02T00:00:00Z",
        );
        write_session_json_with_run(
            tmp.path(),
            "sessions",
            "sess_solo",
            "run_solo_session",
            "2026-04-03T00:00:00Z",
        );

        let rows = collect_and_sort_local_agent_runs(tmp.path());
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.run_id == "run_solo_standalone"));
        assert!(rows.iter().any(|r| r.run_id == "run_solo_session"));
    }

    /// Defensive edge case: a `run_id` that is empty (a corrupt/incomplete
    /// `.meta.json` — see `StandaloneMetaDto`'s `#[serde(default)]`) must
    /// NEVER collide with another empty-`run_id` row. Deduping those would
    /// silently fuse two unrelated broken records into one.
    #[test]
    fn dedupe_never_merges_rows_sharing_an_empty_run_id() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("transcripts");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.meta.json"), r#"{"run_id":""}"#).unwrap();
        fs::write(dir.join("b.meta.json"), r#"{"run_id":""}"#).unwrap();

        let rows = collect_and_sort_local_agent_runs(tmp.path());
        assert_eq!(rows.len(), 2);
    }

    /// Direct unit test of the merge helper itself (no filesystem), pinning
    /// the exact field-by-field precedence documented on
    /// `dedupe_agent_runs_by_run_id`.
    #[test]
    fn merge_agent_run_rows_applies_the_documented_precedence() {
        fn row(
            source: &'static str,
            session_id: Option<&str>,
            trigger_source: Option<&str>,
            status: Option<&str>,
            agent: Option<&str>,
        ) -> AgentRunRow {
            AgentRunRow {
                run_id: "run_z".to_string(),
                source,
                agent: agent.map(str::to_string),
                session_id: session_id.map(str::to_string),
                trigger_source: trigger_source.map(str::to_string),
                status: status.map(str::to_string),
                started_at: None,
                transcript_path: None,
                usage: crate::usage::UsageSummary::default(),
                turns: 0,
                duration_ms: None,
                host_id: None,
            }
        }

        let standalone = row(
            "standalone",
            Some("sess_z"),
            Some("session_turn"),
            None,
            Some("standalone-agent"),
        );
        let session = row(
            "session",
            Some("sess_z"),
            None,
            Some("ok"),
            Some("session-agent"),
        );

        let merged = merge_agent_run_rows(standalone, session);
        assert_eq!(merged.source, "session"); // session_id present
        assert_eq!(merged.trigger_source.as_deref(), Some("session_turn")); // standalone wins
        assert_eq!(merged.status.as_deref(), Some("ok")); // session wins
        assert_eq!(merged.agent.as_deref(), Some("session-agent")); // session wins (default)
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
