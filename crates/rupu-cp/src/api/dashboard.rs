//! `GET /api/dashboard` — the ops-first dashboard, fanned out across every
//! registered host.
//!
//! Was the one list-ish view in CP that never learned about hosts: it used to
//! read `s.run_store` directly, so every number silently meant "local only"
//! while the app has five transports (local / http / ssh / tunnel / bucket).
//!
//! The rule this whole endpoint turns on: **a host that cannot report is NOT a
//! host with no runs.** A non-reporting host (offline, or `Unsupported` —
//! the trait default for Tunnel/Bucket/too-old-SSH) contributes NOTHING to
//! the aggregate; its state is carried in `hosts[]` as `offline` /
//! `unavailable` with a human-readable reason. Collapsing a non-reporting
//! host into zeroed counts would make an outage invisible.

use crate::{
    api::hosts::transport_fields,
    error::{ApiError, ApiResult},
    host::{
        connector::HostConnectorError,
        dashboard_summary::{
            ActiveCounts, ActiveRunBar, CycleRollup, DashboardRange, DashboardSummary, RecentRun,
            TerminalBucket,
        },
        summary_build,
    },
    state::AppState,
};
use axum::{extract::State, routing::get, Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/dashboard", get(get_dashboard))
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// One host's reporting state, for the freshness strip.
///
/// `state` is deliberately three-valued. A host that cannot report is NOT a
/// host with no runs, so `unavailable` and `offline` must never collapse into
/// zeroed counts.
#[derive(Serialize)]
struct HostFreshness {
    host_id: String,
    name: String,
    transport_kind: String,
    /// `"ok"` | `"offline"` | `"unavailable"`.
    state: &'static str,
    /// Present only when `state == "ok"`.
    captured_at: Option<DateTime<Utc>>,
    /// Human-readable cause when `state != "ok"`, e.g. "needs rupu >= 0.49".
    reason: Option<String>,
}

/// The dashboard payload: one aggregate summary plus per-host reporting state.
///
/// `summary` is `#[serde(flatten)]`ed, so the wire form carries `DashboardSummary`'s
/// fields at the top level. That is load-bearing: `HttpHostConnector::dashboard_summary`
/// proxies this endpoint and parses the body as a bare `DashboardSummary`. Flattening
/// makes remote-CP fan-out work BY CONSTRUCTION — serde ignores the extra `hosts` key —
/// instead of via a mapper that can drift. Do not un-flatten this.
#[derive(Serialize)]
struct DashboardResponse {
    hosts: Vec<HostFreshness>,
    #[serde(flatten)]
    summary: crate::host::dashboard_summary::DashboardSummary,
}

#[derive(serde::Deserialize)]
struct DashboardQuery {
    range: Option<String>,
    host: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn get_dashboard(
    State(s): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<DashboardQuery>,
) -> ApiResult<Json<DashboardResponse>> {
    let range = match q.range.as_deref() {
        None => DashboardRange::default(),
        Some(r) => DashboardRange::parse(r).ok_or_else(|| {
            ApiError::bad_request(format!("unknown range {r:?}; expected 7d | 30d | all"))
        })?,
    };

    // Which hosts to ask: one named host, or every registered host.
    // `HostRegistry` has no per-id lookup (`list_hosts()` is the only
    // enumeration surface), so scope by filtering it rather than adding a
    // registry method for this one caller.
    let targets: Vec<_> = match q.host.as_deref() {
        Some(id) => {
            let found = s
                .hosts
                .list_hosts()
                .into_iter()
                .find(|h| h.id == id)
                .ok_or_else(|| ApiError::not_found(format!("unknown host {id}")))?;
            vec![found]
        }
        None => s.hosts.list_hosts(),
    };

    let futs = targets.into_iter().map(|h| {
        let registry = Arc::clone(&s.hosts);
        let host_id = h.id.clone();
        let name = h.name.clone();
        let (transport_kind, _base_url) = transport_fields(&h.transport);
        async move {
            let conn = match registry.resolve(&host_id) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(host_id = %host_id, error = %e, "dashboard: could not resolve host connector");
                    return (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.to_string()),
                        },
                        None,
                    );
                }
            };
            match conn.dashboard_summary(range).await {
                Ok(sum) => (
                    HostFreshness {
                        host_id,
                        name,
                        transport_kind,
                        state: "ok",
                        captured_at: Some(sum.captured_at),
                        reason: None,
                    },
                    Some(sum),
                ),
                Err(HostConnectorError::Unsupported(_)) => (
                    HostFreshness {
                        host_id,
                        name,
                        transport_kind,
                        state: "unavailable",
                        captured_at: None,
                        reason: Some(
                            "host does not report dashboard data (needs a newer rupu)".into(),
                        ),
                    },
                    None,
                ),
                Err(e) => {
                    tracing::warn!(host_id = %host_id, error = %e, "dashboard_summary failed");
                    (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "offline",
                            captured_at: None,
                            reason: Some(e.to_string()),
                        },
                        None,
                    )
                }
            }
        }
    });

    let results = futures_util::future::join_all(futs).await;

    // Merge ONLY hosts that actually reported. A non-reporting host
    // contributes nothing rather than zeros — its state is carried in
    // `hosts` instead.
    let mut hosts = Vec::new();
    let mut active = ActiveCounts::default();
    let mut active_runs: Vec<ActiveRunBar> = Vec::new();
    let mut cycles: Vec<CycleRollup> = Vec::new();
    let mut recent_manual: Vec<RecentRun> = Vec::new();
    let mut findings_open: u64 = 0;
    let mut bucket_merge: BTreeMap<DateTime<Utc>, TerminalBucket> = BTreeMap::new();
    // The oldest `captured_at` among hosts that actually reported — the
    // honest staleness bound for the merged aggregate ("this is at best this
    // fresh"), not the newest, which would understate how stale the slowest
    // host's contribution is. `None` until the first reporting host is seen;
    // falls back to `Utc::now()` when no host reported at all.
    let mut oldest_captured_at: Option<DateTime<Utc>> = None;

    for (freshness, summary) in results {
        hosts.push(freshness);
        let Some(sum) = summary else { continue };
        oldest_captured_at = Some(match oldest_captured_at {
            Some(oldest) => oldest.min(sum.captured_at),
            None => sum.captured_at,
        });
        active.running += sum.active.running;
        active.awaiting_approval += sum.active.awaiting_approval;
        active.paused += sum.active.paused;
        active.pending += sum.active.pending;
        findings_open += sum.findings_open;
        active_runs.extend(sum.active_runs);
        cycles.extend(sum.cycles);
        recent_manual.extend(sum.recent_manual);
        for b in sum.terminal_buckets {
            let e = bucket_merge.entry(b.ts).or_insert(TerminalBucket {
                ts: b.ts,
                completed: 0,
                failed: 0,
                rejected: 0,
                cancelled: 0,
            });
            e.completed += b.completed;
            e.failed += b.failed;
            e.rejected += b.rejected;
            e.cancelled += b.cancelled;
        }
    }

    // Fill the merged bucket grid — zero-fill every day in `range`, not just
    // days that had terminal runs. Without this the trend area silently
    // closes gaps and reads as continuous activity across days that had
    // none.
    //
    // This MUST happen here, after the merge: the local connector zero-fills
    // its own grid but the SSH connector emits only days with runs, so a
    // fleet with no local host would otherwise produce a holed grid. The
    // merged output is the only place that sees every host. Reuses
    // `summary_build::fill_bucket_grid` rather than a second "which days
    // exist" implementation.
    let terminal_buckets = summary_build::fill_bucket_grid(bucket_merge, range, Utc::now());
    active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    cycles.sort_by_key(|c| std::cmp::Reverse(c.started_at));
    recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

    let resp = DashboardResponse {
        hosts,
        summary: DashboardSummary {
            active,
            terminal_buckets,
            active_runs,
            cycles,
            recent_manual,
            findings_open,
            captured_at: oldest_captured_at.unwrap_or_else(Utc::now),
        },
    };

    Ok(Json(resp))
}
