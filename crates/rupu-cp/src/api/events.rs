//! `GET /api/events/stream` — global SSE event stream.
//!
//! The optional `?run=<id>` query param tails a single specific run. Without
//! it, the endpoint returns a **multiplexed firehose**: events from every
//! active run (and any run that starts while connected) merged into one stream
//! — see [`crate::sse::tail_all_events_sse`]. This is what the Live Events page
//! consumes.
//!
//! The optional `?host=<id>` param scopes the request to a specific host. When
//! the host is remote, `stream_run_events` on the `HostConnector` is called and
//! its pre-formatted SSE byte stream is passed through as-is. The `?run=` param
//! is required when `?host=` names a remote host.
//!
//! Also `GET /api/events` — the "load history" counterpart to the firehose.
//! See [`recent_events`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader};

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{sse::Sse, IntoResponse as _, Response},
    routing::get,
    Json, Router,
};
use rupu_orchestrator::{
    executor::Event,
    runs::{RunStore, RunStoreError},
};

use crate::{
    error::ApiError,
    host::connector::{EventByteStream, HostConnectorError},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/events/stream", get(events_stream))
        .route("/api/events", get(recent_events))
}

/// Wrap a pre-formatted `EventByteStream` in an axum `Response` with the
/// correct `text/event-stream` content-type. The bytes are already SSE frames
/// (`data: {...}\n\n`), so we pass them through without re-encoding.
///
/// `pub(crate)` so that other handlers (e.g. `api::runs::get_run_log`) can
/// reuse this builder when they proxy `stream_run_events` from a connector.
pub(crate) fn proxy_event_byte_stream(stream: EventByteStream) -> Result<Response, ApiError> {
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .map(|r| r.into_response())
        .map_err(|e| ApiError::internal(format!("event proxy response: {e}")))
}

/// `GET /api/events/stream[?run=<id>][?host=<id>]`
///
/// 1. If `?host=<remote-id>`: proxy `connector.stream_run_events(run)` — `?run=`
///    is required in this case. Unknown host id or run id → 404.
/// 2. If `?run=<id>` (local): tail that one run's `events.jsonl` (404 if unknown).
/// 3. Otherwise: return the merged live firehose across all runs on the local host.
async fn events_stream(
    State(s): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let run_id = params.get("run").map(String::as_str);
    let host_id = params.get("host").map(String::as_str).unwrap_or("local");

    // --- remote host: proxy stream_run_events ---
    if host_id != "local" {
        let conn = s.hosts.resolve(host_id).map_err(|e| match e {
            HostConnectorError::NotFound(_) => {
                ApiError::not_found(format!("host {host_id} not found"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
        let id = run_id.ok_or_else(|| {
            ApiError::bad_request("?run= is required when ?host= names a remote host")
        })?;
        let stream = conn.stream_run_events(id).await.map_err(|e| match e {
            HostConnectorError::NotFound(_) => {
                ApiError::not_found(format!("run {id} not found on {host_id}"))
            }
            HostConnectorError::Unreachable(m) => {
                ApiError::internal(format!("host {host_id} unreachable: {m}"))
            }
            other => ApiError::internal(other.to_string()),
        })?;
        return proxy_event_byte_stream(stream);
    }

    // --- local: explicit run parameter ---
    if let Some(id) = run_id {
        s.run_store.load(id).map_err(|e| match e {
            RunStoreError::NotFound(_) => ApiError::not_found(format!("run {id} not found")),
            other => ApiError::internal(other.to_string()),
        })?;
        let events_path = s.run_store.events_path(id);
        let sse = crate::sse::tail_events_sse(events_path)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        return Ok(sse.into_response());
    }

    // --- local: merged firehose across every run ---
    let sse: Sse<_> = crate::sse::tail_all_events_sse(s.run_store.clone()).await;
    Ok(sse.into_response())
}

/// Default page size for `GET /api/events` when `?limit=` is absent, zero, or
/// unparseable.
const DEFAULT_RECENT_EVENTS_LIMIT: usize = 200;

/// Upper bound on how many *runs* `GET /api/events` will open `events.jsonl`
/// for while assembling one page, regardless of `?limit=`. `RunStore::list()`
/// is already newest-first by `started_at`, so this caps the endpoint's cost
/// at "the events.jsonl of the N most-recently-started runs" instead of every
/// run ever persisted — there is intentionally no persistent event index
/// behind this endpoint (see module docs), so a deployment with more
/// concurrently-relevant history than this constant covers gets a
/// best-effort "recent" page rather than a complete one.
const MAX_RUNS_SCANNED: usize = 20;

/// `GET /api/events?limit=<n>&before_ts=<unix-ms>&before_run=<id>&before_pos=<n>`
/// — recent events aggregated from recent runs' `events.jsonl`, returned
/// newest-first and cursor-paginated. This is the "load history" counterpart
/// to the live SSE firehose (`GET /api/events/stream`, no `?run=`/`?host=`):
/// each row is the same JSON shape an SSE frame's `data:` payload carries
/// (the tagged [`Event`], which already carries its own `run_id`), plus an
/// injected `ts` (unix-ms) and `pos` (0-based line index within that run's
/// `events.jsonl`) field so the frontend can sort/merge/cursor history rows
/// and live SSE rows uniformly even though most `Event` variants carry no
/// timestamp of their own.
///
/// No new persistent store: aggregates directly from disk, bounded by
/// [`MAX_RUNS_SCANNED`] and by `limit` (see [`collect_recent_events`]).
/// `?before_ts=` alone reproduces the legacy strictly-less-than-`ts` filter,
/// which under-returns when many events share one run's fallback `ts` (see
/// [`EventsCursor`]); the frontend always additionally sends `?before_run=`
/// and `?before_pos=` (both present on every row this endpoint returns) so
/// pagination resumes exactly at the last-returned event instead of at a
/// `ts` boundary, and therefore never permanently skips same-`ts` siblings.
async fn recent_events(
    State(s): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_RECENT_EVENTS_LIMIT);
    let before_ts = params.get("before_ts").and_then(|v| v.parse::<i64>().ok());
    let before_run = params.get("before_run").cloned();
    let before_pos = params
        .get("before_pos")
        .and_then(|v| v.parse::<usize>().ok());
    let cursor = EventsCursor::from_parts(before_ts, before_run, before_pos);

    let rows = collect_recent_events(&s.run_store, limit, cursor)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(rows))
}

/// Pagination cursor for [`recent_events`] / [`collect_recent_events`].
///
/// step/unit `Event` variants carry no timestamp of their own (see
/// [`event_own_ts_ms`]), so they all fall back to their run's single
/// `events.jsonl` mtime — one shared `ts` for the *entire* run. A cursor
/// keyed on `ts` alone (`TsOnly`) excludes every event at that shared `ts`
/// once it's used as `before_ts`, not just the ones already returned, so a
/// run emitting more events than one page permanently loses the rest.
///
/// `Compound` fixes this by keying on `(ts, run_id, pos)` — `pos` is each
/// event's 0-based line index within its own run's file, which together
/// with `run_id` totally orders even events that share an identical
/// fallback `ts` (appends are chronological within one file, so a higher
/// `pos` is a later event). Every row `GET /api/events` returns carries all
/// three fields, so a client can always build the next page's `Compound`
/// cursor from the last row of the current page.
enum EventsCursor {
    /// No cursor — return the newest page.
    None,
    /// Legacy/degrade form (`?before_ts=` with no `?before_run=`/`?before_pos=`).
    /// Reproduces the pre-fix strictly-less-than-`ts` filter.
    TsOnly(i64),
    /// `(ts, run_id, pos)` of the last-returned row on the previous page.
    Compound(i64, String, usize),
}

impl EventsCursor {
    fn from_parts(ts: Option<i64>, run: Option<String>, pos: Option<usize>) -> Self {
        match (ts, run, pos) {
            (Some(ts), Some(run), Some(pos)) => Self::Compound(ts, run, pos),
            (Some(ts), _, _) => Self::TsOnly(ts),
            _ => Self::None,
        }
    }

    /// Whether a candidate event with this `(ts, run_id, pos)` key comes
    /// strictly after this cursor in the newest-first ordering (i.e. should
    /// be included in the next page).
    fn admits(&self, ts: i64, run_id: &str, pos: usize) -> bool {
        match self {
            Self::None => true,
            Self::TsOnly(before_ts) => ts < *before_ts,
            Self::Compound(before_ts, before_run, before_pos) => {
                (ts, run_id, pos) < (*before_ts, before_run.as_str(), *before_pos)
            }
        }
    }
}

/// The own timestamp (unix-ms) carried by the handful of [`Event`] variants
/// that have one. Every other variant returns `None` and the caller falls
/// back to the run's `events.jsonl` mtime.
fn event_own_ts_ms(ev: &Event) -> Option<i64> {
    match ev {
        Event::RunStarted { started_at, .. } => Some(started_at.timestamp_millis()),
        Event::RunCompleted { finished_at, .. } | Event::RunFailed { finished_at, .. } => {
            Some(finished_at.timestamp_millis())
        }
        _ => None,
    }
}

/// Core aggregation behind [`recent_events`], factored out so it is
/// unit-testable without spinning up an axum server.
///
/// Walks `run_store.list()` (already newest-first by `started_at`), reading
/// **at most [`MAX_RUNS_SCANNED`] runs' `events.jsonl`** — this is the bound
/// on the work done per request: cost is capped at a fixed number of small
/// file reads regardless of how much history exists on disk, never
/// "every run ever persisted". A run whose `events.jsonl` is missing is
/// skipped (not an error — e.g. a run that hasn't started emitting yet); an
/// individual malformed line within a file is likewise skipped rather than
/// failing the whole read. Each surviving event is tagged with a `ts` (its
/// own, else the file's mtime, else the run's `started_at`) and an
/// `before_ts` cursor filter is applied inline.
///
/// Deliberately does NOT early-exit the run scan once `limit` candidates
/// have been collected: a run's own events (e.g. `RunCompleted`) can carry a
/// timestamp later than a more-recently-*started* run's early events if the
/// two runs' lifetimes overlap, so stopping on count alone can drop a
/// genuinely-newer event in favor of an older one from a run that merely
/// started later. The [`MAX_RUNS_SCANNED`] cap is therefore the sole bound;
/// within that scanned window the final merge sorts newest-first by
/// `(ts, run_id, pos)` (see [`EventsCursor`] for why `run_id` + `pos` are
/// needed as tie-breakers, not `ts` alone) before truncating to `limit`.
fn collect_recent_events(
    run_store: &RunStore,
    limit: usize,
    cursor: EventsCursor,
) -> Result<Vec<serde_json::Value>, RunStoreError> {
    let runs = run_store.list()?;

    // (ts, run_id, pos-within-file, row). `run_id` + `pos` break ties
    // between events that share a `ts` (most commonly: every fallback-mtime
    // event from one run's file, since it's the same file's single mtime) —
    // appends are chronological within one file, so a higher `pos` is a
    // later (newer) event; `run_id` only matters when two *different* runs'
    // fallback `ts` happen to collide exactly.
    let mut candidates: Vec<(i64, String, usize, serde_json::Value)> = Vec::new();

    for run in runs.iter().take(MAX_RUNS_SCANNED) {
        let path = run_store.events_path(&run.id);
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let fallback_ts = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or_else(|| run.started_at.timestamp_millis());

        for (pos, line) in BufReader::new(file).lines().enumerate() {
            let Ok(line) = line else { continue };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Event>(&line) else {
                continue;
            };
            let ts = event_own_ts_ms(&event).unwrap_or(fallback_ts);
            if !cursor.admits(ts, &run.id, pos) {
                continue;
            }
            let Ok(mut row) = serde_json::to_value(&event) else {
                continue;
            };
            if let Some(obj) = row.as_object_mut() {
                obj.insert("ts".to_string(), serde_json::json!(ts));
                obj.insert("pos".to_string(), serde_json::json!(pos));
            }
            candidates.push((ts, run.id.clone(), pos, row));
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)));
    candidates.truncate(limit);
    Ok(candidates.into_iter().map(|(_, _, _, row)| row).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use rupu_orchestrator::runs::{RunRecord, RunStatus, StepKind};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn seed_run(store: &RunStore, id: &str, started_at: DateTime<Utc>) {
        let record = RunRecord {
            id: id.into(),
            workflow_name: "test-workflow".into(),
            status: RunStatus::Completed,
            inputs: BTreeMap::new(),
            event: None,
            workspace_id: "ws_test".into(),
            workspace_path: PathBuf::from("/tmp/test-proj"),
            transcript_dir: PathBuf::from("/tmp/test-proj/.rupu/transcripts"),
            started_at,
            finished_at: None,
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            final_output: None,
        };
        store
            .create(record, "name: test\nsteps: []\n")
            .expect("create run");
    }

    fn write_events(store: &RunStore, run_id: &str, events: &[Event]) {
        let path = store.events_path(run_id);
        let lines: Vec<String> = events
            .iter()
            .map(|e| serde_json::to_string(e).expect("serialize event"))
            .collect();
        std::fs::write(&path, lines.join("\n") + "\n").expect("write events.jsonl");
    }

    /// Two runs, each contributing one `RunStarted` (has its own `ts`) and one
    /// `RunCompleted` (also has its own `ts`), at four distinct, known
    /// instants — so ordering is asserted on real event timestamps rather
    /// than filesystem mtime.
    fn seed_two_runs_four_events(store: &RunStore) -> [i64; 4] {
        let t0 = ts(1_000);
        let t1 = ts(2_000);
        let t2 = ts(3_000);
        let t3 = ts(4_000);

        seed_run(store, "run_a", t0);
        seed_run(store, "run_b", t1);

        write_events(
            store,
            "run_a",
            &[
                Event::RunStarted {
                    event_version: 1,
                    run_id: "run_a".into(),
                    workflow_path: PathBuf::from("/wf.yaml"),
                    started_at: t0,
                },
                Event::RunCompleted {
                    run_id: "run_a".into(),
                    status: RunStatus::Completed,
                    finished_at: t2,
                },
            ],
        );
        write_events(
            store,
            "run_b",
            &[
                Event::RunStarted {
                    event_version: 1,
                    run_id: "run_b".into(),
                    workflow_path: PathBuf::from("/wf.yaml"),
                    started_at: t1,
                },
                Event::RunCompleted {
                    run_id: "run_b".into(),
                    status: RunStatus::Completed,
                    finished_at: t3,
                },
            ],
        );

        [
            t0.timestamp_millis(),
            t1.timestamp_millis(),
            t2.timestamp_millis(),
            t3.timestamp_millis(),
        ]
    }

    #[test]
    fn recent_events_returns_newest_first_limited() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let [_t0, _t1, t2, t3] = seed_two_runs_four_events(&store);

        let rows = collect_recent_events(&store, 2, EventsCursor::None).expect("collect");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["ts"], serde_json::json!(t3));
        assert_eq!(rows[0]["type"], "run_completed");
        assert_eq!(rows[0]["run_id"], "run_b");
        assert_eq!(rows[1]["ts"], serde_json::json!(t2));
        assert_eq!(rows[1]["type"], "run_completed");
        assert_eq!(rows[1]["run_id"], "run_a");
    }

    #[test]
    fn recent_events_before_ts_paginates() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let [t0, t1, t2, _t3] = seed_two_runs_four_events(&store);

        // Cursor at t2 (run_a's RunCompleted): only events strictly older
        // than t2 should come back — t1 (run_b RunStarted) then t0 (run_a
        // RunStarted), newest-first.
        let rows = collect_recent_events(&store, 100, EventsCursor::TsOnly(t2)).expect("collect");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["ts"], serde_json::json!(t1));
        assert_eq!(rows[0]["type"], "run_started");
        assert_eq!(rows[0]["run_id"], "run_b");
        assert_eq!(rows[1]["ts"], serde_json::json!(t0));
        assert_eq!(rows[1]["type"], "run_started");
        assert_eq!(rows[1]["run_id"], "run_a");
    }

    #[test]
    fn recent_events_empty_when_no_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));

        let rows = collect_recent_events(&store, 200, EventsCursor::None).expect("collect");
        assert!(rows.is_empty());
    }

    #[test]
    fn recent_events_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));

        // Seed more runs than MAX_RUNS_SCANNED, one RunStarted event each, at
        // strictly increasing timestamps so run index N started_at < N+1's.
        // run_store.list() sorts newest-first, so run indices below
        // MAX_RUNS_SCANNED (the most-recently-started ones) are the ones the
        // scan should reach; older ones beyond the cap must NOT appear even
        // though `limit` is set far higher than what the cap can produce.
        let total_runs = MAX_RUNS_SCANNED + 5;
        for i in 0..total_runs {
            let id = format!("run_{i:03}");
            let started_at = ts(1_000 + i as i64);
            seed_run(&store, &id, started_at);
            write_events(
                &store,
                &id,
                &[Event::RunStarted {
                    event_version: 1,
                    run_id: id.clone(),
                    workflow_path: PathBuf::from("/wf.yaml"),
                    started_at,
                }],
            );
        }

        let rows = collect_recent_events(&store, 10_000, EventsCursor::None).expect("collect");
        // Bounded to at most MAX_RUNS_SCANNED runs' worth of events (one
        // event per run here), never the full `total_runs`.
        assert_eq!(rows.len(), MAX_RUNS_SCANNED);

        // The oldest-started run (run_000) must be outside the scanned
        // window and therefore absent from the response.
        assert!(rows.iter().all(|r| r["run_id"] != "run_000"));

        // The most-recently-started run must be present.
        let newest_id = format!("run_{:03}", total_runs - 1);
        assert!(rows.iter().any(|r| r["run_id"] == newest_id.as_str()));
    }

    /// Regression test for the `before_ts` pagination gap: a single run
    /// emitting more events than one page, every one of them a variant with
    /// no own timestamp (`StepStarted`) — so [`event_own_ts_ms`] returns
    /// `None` for all of them and they share one fallback `ts` (the run's
    /// `events.jsonl` mtime). A `before_ts`-only cursor at that shared value
    /// would exclude ALL of them once used, permanently stranding whatever
    /// didn't fit on the first page. Paginating with the full
    /// `(ts, run_id, pos)` `EventsCursor::Compound` cursor instead must
    /// reach every event exactly once.
    #[test]
    fn recent_events_compound_cursor_reaches_every_event_at_a_shared_fallback_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        seed_run(&store, "run_big", ts(1_000));

        let total_events = 23usize;
        let events: Vec<Event> = (0..total_events)
            .map(|i| Event::StepStarted {
                run_id: "run_big".into(),
                step_id: format!("step_{i:03}"),
                kind: StepKind::Linear,
                agent: None,
                host: None,
            })
            .collect();
        write_events(&store, "run_big", &events);

        let limit = 5;
        let mut seen_step_ids = std::collections::HashSet::new();
        let mut cursor = EventsCursor::None;
        let mut pages = 0;
        loop {
            let rows = collect_recent_events(&store, limit, cursor).expect("collect");
            if rows.is_empty() {
                break;
            }
            pages += 1;
            assert!(
                pages <= total_events,
                "pagination did not terminate — likely stuck re-returning the same page"
            );
            assert!(rows.len() <= limit);
            for row in &rows {
                let step_id = row["step_id"].as_str().unwrap().to_string();
                assert!(
                    seen_step_ids.insert(step_id.clone()),
                    "duplicate row for {step_id} across pages"
                );
            }
            let last = rows.last().unwrap();
            cursor = EventsCursor::Compound(
                last["ts"].as_i64().unwrap(),
                last["run_id"].as_str().unwrap().to_string(),
                last["pos"].as_u64().unwrap() as usize,
            );
        }

        assert_eq!(
            seen_step_ids.len(),
            total_events,
            "every event sharing the fallback ts must eventually surface via 'load older', none permanently skipped"
        );
    }
}
