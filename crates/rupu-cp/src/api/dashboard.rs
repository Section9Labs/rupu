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
    /// True when at least one host that successfully reported (`state ==
    /// "ok"` in `hosts[]`) did not include an open-findings count (SSH,
    /// today — the CLI has no findings surface). When true, `findings_open`
    /// (below, via the flatten) is a partial sum across only the hosts that
    /// DID report it, never the fleet total — the UI must not present it as
    /// complete. See `DashboardSummary::findings_open`'s doc comment.
    findings_partial: bool,
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
                Ok(mut sum) => {
                    // Tag every row with the host it came from — the
                    // connector doesn't know the id it's registered under
                    // (see `DashboardSummary`'s `host_id` doc comments), so
                    // this is the one place with both the id and the rows in
                    // scope. Mirrors `api/host_fanout.rs`'s `fan_out_via`.
                    for r in sum.active_runs.iter_mut() {
                        r.host_id = Some(host_id.clone());
                    }
                    for c in sum.cycles.iter_mut() {
                        c.host_id = Some(host_id.clone());
                    }
                    for r in sum.recent_manual.iter_mut() {
                        r.host_id = Some(host_id.clone());
                    }
                    (
                        HostFreshness {
                            host_id,
                            name,
                            transport_kind,
                            state: "ok",
                            captured_at: Some(sum.captured_at),
                            reason: None,
                        },
                        Some(sum),
                    )
                }
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

    // Split into per-host freshness (always kept) and the summaries that
    // actually reported (fed to the pure merge below). A non-reporting host
    // contributes nothing rather than zeros — its state is carried in
    // `hosts` instead.
    let mut hosts = Vec::new();
    let mut reported = Vec::new();
    for (freshness, summary) in results {
        hosts.push(freshness);
        if let Some(sum) = summary {
            reported.push(sum);
        }
    }

    let (summary, findings_partial) = merge_dashboard_summaries(reported, range, Utc::now());

    Ok(Json(DashboardResponse {
        hosts,
        findings_partial,
        summary,
    }))
}

/// Merge every host's [`DashboardSummary`] that actually reported into one
/// fleet-wide aggregate. Pulled out of the handler so the merge — the exact
/// seam where a per-host `TerminalBucket` or `findings_open` either survives
/// or silently vanishes — can be exercised directly with hand-built fixtures,
/// without standing up an `AppState` + host registry.
///
/// Returns the merged summary plus `findings_partial`: true when at least one
/// reporting host contributed `findings_open: None` (it reports successfully
/// but does not expose findings — SSH, today), meaning the summed
/// `findings_open` is partial, not the fleet total.
fn merge_dashboard_summaries(
    reported: Vec<DashboardSummary>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> (DashboardSummary, bool) {
    let mut active = ActiveCounts::default();
    let mut active_runs: Vec<ActiveRunBar> = Vec::new();
    let mut cycles: Vec<CycleRollup> = Vec::new();
    let mut recent_manual: Vec<RecentRun> = Vec::new();
    let mut findings_open: Option<u64> = None;
    let mut findings_partial = false;
    let mut bucket_merge: BTreeMap<DateTime<Utc>, TerminalBucket> = BTreeMap::new();
    // The oldest `captured_at` among hosts that actually reported — the
    // honest staleness bound for the merged aggregate ("this is at best this
    // fresh"), not the newest, which would understate how stale the slowest
    // host's contribution is. `None` until the first reporting host is seen;
    // falls back to `now` when no host reported at all.
    let mut oldest_captured_at: Option<DateTime<Utc>> = None;

    for sum in reported {
        oldest_captured_at = Some(match oldest_captured_at {
            Some(oldest) => oldest.min(sum.captured_at),
            None => sum.captured_at,
        });
        active.running += sum.active.running;
        active.awaiting_approval += sum.active.awaiting_approval;
        active.paused += sum.active.paused;
        active.pending += sum.active.pending;
        match sum.findings_open {
            Some(n) => findings_open = Some(findings_open.unwrap_or(0) + n),
            // This host reported successfully but has no findings surface
            // (SSH). Never fold it in as a zero — flag the aggregate partial
            // instead.
            None => findings_partial = true,
        }
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
    // exist" implementation — which also normalizes every bucket key through
    // `day_key` (defence-in-depth), so a non-midnight `ts` from any producer
    // merges instead of silently vanishing.
    let terminal_buckets = summary_build::fill_bucket_grid(bucket_merge, range, now);
    active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    cycles.sort_by_key(|c| std::cmp::Reverse(c.started_at));
    recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

    (
        DashboardSummary {
            active,
            terminal_buckets,
            active_runs,
            cycles,
            recent_manual,
            findings_open,
            captured_at: oldest_captured_at.unwrap_or(now),
        },
        findings_partial,
    )
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use chrono::TimeZone;

    fn bucket(ts: DateTime<Utc>, completed: u64, failed: u64) -> TerminalBucket {
        TerminalBucket {
            ts,
            completed,
            failed,
            rejected: 0,
            cancelled: 0,
        }
    }

    fn empty_summary(captured_at: DateTime<Utc>) -> DashboardSummary {
        DashboardSummary {
            active: ActiveCounts::default(),
            terminal_buckets: vec![],
            active_runs: vec![],
            cycles: vec![],
            recent_manual: vec![],
            findings_open: None,
            captured_at,
        }
    }

    /// A local-shaped (midnight ts) bucket and an SSH-shaped bucket for the SAME
    /// day must merge into ONE bucket carrying both counts. Before the fix the SSH
    /// bucket's non-midnight ts never matched a fill cursor and was silently dropped.
    /// Assert: exactly one bucket for that day, completed == local + ssh.
    #[test]
    fn local_and_ssh_shaped_buckets_for_the_same_day_merge_into_one() {
        let now = Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap();
        let day = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
        // The local connector always stamps `ts` at midnight-UTC.
        let local = DashboardSummary {
            terminal_buckets: vec![bucket(day, 3, 0)],
            findings_open: Some(0),
            ..empty_summary(now)
        };
        // The SSH-shaped bucket: same calendar day, but a raw (non-midnight)
        // timestamp — exactly the shape the pre-fix `SshHostConnector` used
        // to emit, and the shape `fill_bucket_grid`'s defence-in-depth
        // normalization must still handle from any future non-conforming
        // producer.
        let ssh_raw_ts = Utc.with_ymd_and_hms(2026, 7, 15, 13, 47, 22).unwrap();
        let ssh = DashboardSummary {
            terminal_buckets: vec![bucket(ssh_raw_ts, 2, 1)],
            findings_open: None,
            ..empty_summary(now)
        };

        let (merged, findings_partial) =
            merge_dashboard_summaries(vec![local, ssh], DashboardRange::Days7, now);

        let day_buckets: Vec<_> = merged
            .terminal_buckets
            .iter()
            .filter(|b| b.ts == day)
            .collect();
        assert_eq!(
            day_buckets.len(),
            1,
            "local and ssh buckets for the same day must merge into exactly one bucket, got {:?}",
            merged.terminal_buckets
        );
        assert_eq!(
            day_buckets[0].completed, 5,
            "completed must be local(3) + ssh(2)"
        );
        assert_eq!(day_buckets[0].failed, 1, "failed must be local(0) + ssh(1)");
        assert!(
            findings_partial,
            "the ssh host reported None, so the merged findings_open must be flagged partial"
        );
    }

    #[test]
    fn findings_open_sums_only_reporting_hosts_and_flags_partial() {
        let now = Utc::now();
        let a = DashboardSummary {
            findings_open: Some(3),
            ..empty_summary(now)
        };
        let b = DashboardSummary {
            findings_open: None,
            ..empty_summary(now)
        };
        let (merged, partial) = merge_dashboard_summaries(vec![a, b], DashboardRange::All, now);
        assert_eq!(
            merged.findings_open,
            Some(3),
            "must sum only the Some contribution, never fabricate 0 for the host that reported None"
        );
        assert!(
            partial,
            "one host did not report findings; must be flagged partial"
        );
    }

    #[test]
    fn findings_open_is_none_and_not_partial_when_no_host_reports_at_all() {
        let now = Utc::now();
        let (merged, partial) = merge_dashboard_summaries(vec![], DashboardRange::All, now);
        assert_eq!(merged.findings_open, None);
        assert!(
            !partial,
            "partial means 'a reporting host omitted findings' — with zero reporting hosts \
             there is nothing to be partial about (hosts[] already carries the full outage)"
        );
    }
}
