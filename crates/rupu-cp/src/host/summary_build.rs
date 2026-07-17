//! Pure builder for [`DashboardSummary`].
//!
//! Kept free of I/O so the bucketing and tallying can be tested against
//! fixtures. `LocalHostConnector` is the caller; SSH builds its own summary
//! from CLI JSON (see `host/ssh.rs`).

#![deny(clippy::all)]

use crate::host::dashboard_summary::{
    ActiveCounts, ActiveLongest, CycleCounts, DashboardRange, DashboardSummary, TerminalBucket,
    ThroughputBucket,
};
use chrono::{DateTime, Duration, Timelike, Utc};
use rupu_orchestrator::runs::{RunRecord, RunStatus};
use std::collections::BTreeMap;

/// Truncate to the start of the UTC day — the bucket key.
///
/// `pub(crate)` so every producer of a [`TerminalBucket`]/[`ThroughputBucket`]
/// (currently `build_summary` here and `SshHostConnector::dashboard_summary`
/// in `host/ssh.rs`) truncates through this ONE function rather than each
/// hand-rolling its own day-boundary math — two truncations that drift by
/// even a nanosecond would produce buckets that never merge (see
/// `fill_bucket_grid`'s doc comment and the regression test in
/// `api::dashboard`).
pub(crate) fn day_key(t: DateTime<Utc>) -> DateTime<Utc> {
    t.with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t)
}

/// One cycle's rollup, reduced to exactly what [`build_summary`] needs for
/// [`CycleCounts`]: range filtering (`started_at`) and the clean/with-failures
/// split (`failed`).
///
/// This is deliberately NOT the wire DTO — there is no such thing any more
/// (the old row-shaped `CycleRollup`/`CycleRun` in `dashboard_summary.rs` were
/// deleted along with the swimlane/feed they served). `SshHostConnector`
/// builds its own `CycleCounts` directly from history rows (it cannot report
/// the clean/with-failures breakdown at all, so it never goes through this
/// type); only `LocalHostConnector::collect_cycle_rollups` produces this.
pub struct CycleRollup {
    pub started_at: DateTime<Utc>,
    pub failed: u64,
}

/// Build one host's dashboard contribution from its runs + cycles.
pub fn build_summary(
    runs: &[RunRecord],
    cycles: &[CycleRollup],
    findings_open: Option<u64>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> DashboardSummary {
    let since = range.since(now);
    let in_range = |t: DateTime<Utc>| since.map(|s| t >= s).unwrap_or(true);

    let mut active = ActiveCounts::default();
    let mut terminal_buckets: BTreeMap<DateTime<Utc>, TerminalBucket> = BTreeMap::new();
    let mut throughput_buckets: BTreeMap<DateTime<Utc>, ThroughputBucket> = BTreeMap::new();
    // The non-terminal run with the OLDEST started_at — the longest currently
    // running, fleet-wide "what's stuck" key point (spec §5.2).
    let mut longest: Option<&RunRecord> = None;

    for r in runs {
        if !in_range(r.started_at) {
            continue;
        }
        match r.status {
            RunStatus::Running => active.running += 1,
            RunStatus::AwaitingApproval => active.awaiting_approval += 1,
            RunStatus::Paused => active.paused += 1,
            RunStatus::Pending => active.pending += 1,
            _ => {}
        }

        // Non-terminal runs are candidates for `active_longest`. Paused is
        // deliberately included: is_terminal() excludes it because a paused
        // run expects a resume, so it is still live work.
        if !r.status.is_terminal() {
            longest = Some(match longest {
                // `cur` started at or before `r` — it is the same age or
                // older, so it stays the longest-running candidate.
                Some(cur) if cur.started_at <= r.started_at => cur,
                _ => r,
            });
        }

        // Throughput: every run in range counts once, keyed by the day it
        // STARTED and by trigger — unlike `terminal_buckets`, non-terminal
        // (still-running) runs count here too, so the trend reflects load,
        // not just outcomes.
        let tkey = day_key(r.started_at);
        let tb = throughput_buckets.entry(tkey).or_insert(ThroughputBucket {
            ts: tkey,
            manual: 0,
            cron: 0,
            event: 0,
        });
        match r.trigger_str() {
            "cron" => tb.cron += 1,
            "event" => tb.event += 1,
            // "manual" is the only other value `trigger_str()` returns; treat
            // anything unrecognized the same way rather than silently
            // dropping it from every bucket.
            _ => tb.manual += 1,
        }

        if r.status.is_terminal() {
            let key = day_key(r.started_at);
            let b = terminal_buckets.entry(key).or_insert(TerminalBucket {
                ts: key,
                completed: 0,
                failed: 0,
                rejected: 0,
                cancelled: 0,
            });
            match r.status {
                RunStatus::Completed => b.completed += 1,
                RunStatus::Failed => b.failed += 1,
                RunStatus::Rejected => b.rejected += 1,
                RunStatus::Cancelled => b.cancelled += 1,
                _ => {}
            }
        }
    }

    let active_longest = longest.map(|r| ActiveLongest {
        run_id: r.id.clone(),
        workflow_name: r.workflow_name.clone(),
        age_ms: (now - r.started_at).num_milliseconds().max(0) as u64,
    });

    let cycles_in_range: Vec<&CycleRollup> =
        cycles.iter().filter(|c| in_range(c.started_at)).collect();
    // Local reads the real ran/failed breakdown from its own cycle history,
    // so both `clean` and `with_failures` are always `Some` here — never a
    // fabricated `None`. (SSH, which cannot report the breakdown, builds its
    // own `CycleCounts` directly rather than going through this function.)
    let cycles = CycleCounts {
        total: cycles_in_range.len() as u64,
        clean: Some(cycles_in_range.iter().filter(|c| c.failed == 0).count() as u64),
        with_failures: Some(cycles_in_range.iter().filter(|c| c.failed > 0).count() as u64),
    };

    // Fill the bucket grids. Without this the trend areas silently close gaps
    // and read as continuous activity across days that had none.
    let terminal_buckets = fill_bucket_grid(terminal_buckets, range, now);
    let throughput_buckets = fill_throughput_grid(throughput_buckets, range, now);

    DashboardSummary {
        active,
        active_longest,
        terminal_buckets,
        throughput_buckets,
        cycles,
        findings_open,
        captured_at: now,
    }
}

/// Emit a contiguous day-by-day grid, zero-filling days with no terminal runs.
///
/// `pub(crate)` so `api::dashboard`'s merged-fleet grid can reuse it rather
/// than writing a second "which days exist" implementation — two copies of
/// that logic would drift, and the failure is silent (a chart that closes a
/// gap looks identical to one that had activity).
pub(crate) fn fill_bucket_grid(
    buckets: BTreeMap<DateTime<Utc>, TerminalBucket>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> Vec<TerminalBucket> {
    // Defence-in-depth: normalize every incoming key through `day_key` before
    // it's used as a fill-grid cursor. This is the SAME invariant every
    // producer is already supposed to uphold at construction time (see
    // `day_key`'s doc comment) — re-asserting it here means a future third
    // implementation that forgets to truncate can never reproduce the
    // SSH-bucket-drop bug: a non-midnight key would simply never match a
    // cursor and the whole bucket (and, in the `range=all` case, every OTHER
    // host's buckets too, since `start` is derived from the earliest key)
    // would silently vanish. Buckets that collide after normalization are
    // summed, not overwritten.
    let mut buckets: BTreeMap<DateTime<Utc>, TerminalBucket> =
        buckets
            .into_iter()
            .fold(BTreeMap::new(), |mut acc, (k, b)| {
                let key = day_key(k);
                let entry = acc.entry(key).or_insert(TerminalBucket {
                    ts: key,
                    completed: 0,
                    failed: 0,
                    rejected: 0,
                    cancelled: 0,
                });
                entry.completed += b.completed;
                entry.failed += b.failed;
                entry.rejected += b.rejected;
                entry.cancelled += b.cancelled;
                acc
            });

    let start = match range.since(now) {
        Some(s) => day_key(s),
        // `All`: start at the earliest bucket we actually have.
        None => match buckets.keys().next() {
            Some(k) => *k,
            None => return Vec::new(),
        },
    };
    let end = day_key(now);
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        out.push(buckets.remove(&cursor).unwrap_or(TerminalBucket {
            ts: cursor,
            completed: 0,
            failed: 0,
            rejected: 0,
            cancelled: 0,
        }));
        cursor += Duration::days(1);
    }
    out
}

/// [`ThroughputBucket`] analogue of [`fill_bucket_grid`] — same zero-fill and
/// defence-in-depth day-key normalization discipline, kept as a separate
/// function (rather than a generic) because the two bucket shapes have
/// different fields to sum/zero. `pub(crate)` for the same reason:
/// `api::dashboard`'s merged-fleet grid reuses this rather than re-deriving
/// "which days exist" a second time.
pub(crate) fn fill_throughput_grid(
    buckets: BTreeMap<DateTime<Utc>, ThroughputBucket>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> Vec<ThroughputBucket> {
    let mut buckets: BTreeMap<DateTime<Utc>, ThroughputBucket> =
        buckets
            .into_iter()
            .fold(BTreeMap::new(), |mut acc, (k, b)| {
                let key = day_key(k);
                let entry = acc.entry(key).or_insert(ThroughputBucket {
                    ts: key,
                    manual: 0,
                    cron: 0,
                    event: 0,
                });
                entry.manual += b.manual;
                entry.cron += b.cron;
                entry.event += b.event;
                acc
            });

    let start = match range.since(now) {
        Some(s) => day_key(s),
        None => match buckets.keys().next() {
            Some(k) => *k,
            None => return Vec::new(),
        },
    };
    let end = day_key(now);
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        out.push(buckets.remove(&cursor).unwrap_or(ThroughputBucket {
            ts: cursor,
            manual: 0,
            cron: 0,
            event: 0,
        }));
        cursor += Duration::days(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Explicit field initialization — `RunRecord` has no `Default` impl
    /// (deliberately: see `crates/rupu-orchestrator/src/runs.rs`), so the
    /// fixture must set every field.
    fn rec(id: &str, status: rupu_orchestrator::runs::RunStatus, mins_ago: i64) -> RunRecord {
        RunRecord {
            id: id.to_string(),
            workflow_name: "wf".to_string(),
            status,
            inputs: std::collections::BTreeMap::new(),
            event: None,
            workspace_id: "ws_1".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp/proj"),
            transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
            started_at: chrono::Utc::now() - chrono::Duration::minutes(mins_ago),
            finished_at: None,
            error_message: None,
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
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
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            final_output: None,
        }
    }

    #[test]
    fn active_counts_tally_non_terminal_states_only() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![
            rec("r1", Running, 1),
            rec("r2", Running, 2),
            rec("r3", AwaitingApproval, 3),
            rec("r4", Paused, 4),
            rec("r5", Pending, 5),
            rec("r6", Completed, 6),
            rec("r7", Failed, 7),
        ];
        let s = build_summary(&runs, &[], Some(0), DashboardRange::All, chrono::Utc::now());
        assert_eq!(s.active.running, 2);
        assert_eq!(s.active.awaiting_approval, 1);
        assert_eq!(s.active.paused, 1);
        assert_eq!(s.active.pending, 1);
    }

    #[test]
    fn terminal_buckets_exclude_active_runs() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![
            rec("r1", Completed, 10),
            rec("r2", Failed, 10),
            rec("r3", Running, 10),
        ];
        let s = build_summary(&runs, &[], Some(0), DashboardRange::All, chrono::Utc::now());
        let completed: u64 = s.terminal_buckets.iter().map(|b| b.completed).sum();
        let failed: u64 = s.terminal_buckets.iter().map(|b| b.failed).sum();
        assert_eq!(completed, 1);
        assert_eq!(failed, 1);
    }

    #[test]
    fn range_filters_out_older_runs() {
        use rupu_orchestrator::runs::RunStatus::*;
        // 10 days ago — outside a 7d window.
        let runs = vec![
            rec("old", Completed, 60 * 24 * 10),
            rec("new", Completed, 5),
        ];
        let s = build_summary(
            &runs,
            &[],
            Some(0),
            DashboardRange::Days7,
            chrono::Utc::now(),
        );
        let total: u64 = s.terminal_buckets.iter().map(|b| b.completed).sum();
        assert_eq!(
            total, 1,
            "the 10-day-old run must fall outside the 7d range"
        );
    }

    #[test]
    fn buckets_are_contiguous_so_charts_do_not_lie_about_gaps() {
        use rupu_orchestrator::runs::RunStatus::*;
        // Two runs 3 days apart; the days between must still appear as zeroed
        // buckets, or the trend area silently closes the gap.
        let runs = vec![
            rec("a", Completed, 60 * 24 * 4),
            rec("b", Completed, 60 * 24),
        ];
        let s = build_summary(
            &runs,
            &[],
            Some(0),
            DashboardRange::Days7,
            chrono::Utc::now(),
        );
        assert!(
            s.terminal_buckets.len() >= 4,
            "expected a filled bucket grid, got {} buckets",
            s.terminal_buckets.len()
        );
    }

    #[test]
    fn throughput_buckets_tally_by_trigger() {
        use rupu_orchestrator::runs::RunStatus::*;
        let mut cron = rec("r_cron", Completed, 60);
        cron.source_wake_id = Some("wake_1".into());
        let manual = rec("r_manual", Completed, 30);
        let s = build_summary(
            &[cron, manual],
            &[],
            Some(0),
            DashboardRange::All,
            chrono::Utc::now(),
        );
        let total_manual: u64 = s.throughput_buckets.iter().map(|b| b.manual).sum();
        let total_cron: u64 = s.throughput_buckets.iter().map(|b| b.cron).sum();
        let total_event: u64 = s.throughput_buckets.iter().map(|b| b.event).sum();
        assert_eq!(total_manual, 1);
        assert_eq!(total_cron, 1);
        assert_eq!(total_event, 0);
    }

    #[test]
    fn throughput_buckets_count_non_terminal_runs_too() {
        use rupu_orchestrator::runs::RunStatus::*;
        // Unlike terminal_buckets, a still-running run must still show up in
        // throughput — the point of the panel is load, not just outcomes.
        let s = build_summary(
            &[rec("r1", Running, 5)],
            &[],
            Some(0),
            DashboardRange::All,
            chrono::Utc::now(),
        );
        let total_manual: u64 = s.throughput_buckets.iter().map(|b| b.manual).sum();
        assert_eq!(total_manual, 1);
        let terminal_total: u64 = s
            .terminal_buckets
            .iter()
            .map(|b| b.completed + b.failed + b.rejected + b.cancelled)
            .sum();
        assert_eq!(
            terminal_total, 0,
            "a running run must never appear in terminal_buckets"
        );
    }

    #[test]
    fn throughput_grid_is_contiguous_so_charts_do_not_lie_about_gaps() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![
            rec("a", Completed, 60 * 24 * 4),
            rec("b", Completed, 60 * 24),
        ];
        let s = build_summary(
            &runs,
            &[],
            Some(0),
            DashboardRange::Days7,
            chrono::Utc::now(),
        );
        assert!(
            s.throughput_buckets.len() >= 4,
            "expected a filled throughput grid, got {} buckets",
            s.throughput_buckets.len()
        );
    }

    #[test]
    fn active_longest_picks_the_oldest_running_run() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![
            rec("newer", Running, 5),
            rec("older", Running, 50),
            rec("done", Completed, 100),
        ];
        let s = build_summary(&runs, &[], Some(0), DashboardRange::All, chrono::Utc::now());
        let al = s
            .active_longest
            .expect("two non-terminal runs in hand — expected an active_longest");
        assert_eq!(al.run_id, "older");
        assert!(al.age_ms > 0);
    }

    #[test]
    fn active_longest_is_none_when_nothing_is_running() {
        use rupu_orchestrator::runs::RunStatus::*;
        let s = build_summary(
            &[rec("done", Completed, 5)],
            &[],
            Some(0),
            DashboardRange::All,
            chrono::Utc::now(),
        );
        assert!(s.active_longest.is_none());
    }

    #[test]
    fn cycle_counts_split_clean_vs_with_failures() {
        let now = chrono::Utc::now();
        let cycles = vec![
            CycleRollup {
                started_at: now - chrono::Duration::minutes(5),
                failed: 0,
            },
            CycleRollup {
                started_at: now - chrono::Duration::minutes(4),
                failed: 0,
            },
            CycleRollup {
                started_at: now - chrono::Duration::minutes(3),
                failed: 2,
            },
        ];
        let s = build_summary(&[], &cycles, Some(0), DashboardRange::All, now);
        assert_eq!(s.cycles.total, 3);
        assert_eq!(s.cycles.clean, Some(2));
        assert_eq!(s.cycles.with_failures, Some(1));
    }

    #[test]
    fn cycles_outside_the_range_are_excluded_from_counts() {
        let now = chrono::Utc::now();
        let old = CycleRollup {
            started_at: now - chrono::Duration::days(10),
            failed: 0,
        };
        let recent = CycleRollup {
            started_at: now - chrono::Duration::hours(1),
            failed: 0,
        };
        let s = build_summary(&[], &[old, recent], Some(0), DashboardRange::Days7, now);
        assert_eq!(
            s.cycles.total, 1,
            "the 10-day-old cycle must fall outside the 7d range"
        );
    }
}
