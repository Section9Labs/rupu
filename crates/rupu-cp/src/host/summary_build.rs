//! Pure builder for [`DashboardSummary`].
//!
//! Kept free of I/O so the bucketing and tallying can be tested against
//! fixtures. `LocalHostConnector` is the caller; SSH builds its own summary
//! from CLI JSON (see `host/ssh.rs`).

#![deny(clippy::all)]

use crate::host::dashboard_summary::{
    ActiveCounts, ActiveRunBar, CycleRollup, DashboardRange, DashboardSummary, RecentRun,
    TerminalBucket,
};
use chrono::{DateTime, Duration, Timelike, Utc};
use rupu_orchestrator::runs::{RunRecord, RunStatus};
use std::collections::BTreeMap;

/// Truncate to the start of the UTC day — the bucket key.
fn day_key(t: DateTime<Utc>) -> DateTime<Utc> {
    t.with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t)
}

/// Build one host's dashboard contribution from its runs + cycles.
pub fn build_summary(
    runs: &[RunRecord],
    cycles: &[CycleRollup],
    findings_open: u64,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> DashboardSummary {
    let since = range.since(now);
    let in_range = |t: DateTime<Utc>| since.map(|s| t >= s).unwrap_or(true);

    let mut active = ActiveCounts::default();
    let mut active_runs = Vec::new();
    let mut recent_manual = Vec::new();
    let mut buckets: BTreeMap<DateTime<Utc>, TerminalBucket> = BTreeMap::new();

    // Runs belonging to a cycle are grouped under it in the feed; only manual
    // runs are listed individually (spec §5.5).
    let cycle_of: std::collections::HashMap<&str, &str> = cycles
        .iter()
        .flat_map(|c| {
            c.runs
                .iter()
                .map(move |r| (r.run_id.as_str(), c.cycle_id.as_str()))
        })
        .collect();

    // Join each cycle's runs to their status. The `+N clean` pill needs it, and
    // we already hold every run here — the client should not fetch N runs to
    // expand one cycle.
    let status_of: std::collections::HashMap<&str, &str> = runs
        .iter()
        .map(|r| (r.id.as_str(), r.status.as_str()))
        .collect();
    let cycles: Vec<CycleRollup> = cycles
        .iter()
        // Match the run filter above: without this the 7d/30d control
        // silently doesn't apply to the activity feed, and local would
        // disagree with the SSH implementation (which does filter).
        .filter(|c| in_range(c.started_at))
        .map(|c| {
            let mut c = c.clone();
            for run in c.runs.iter_mut() {
                // "unknown" rather than dropping the run: a cycle whose run list
                // silently shrank would disagree with its own `ran` count.
                run.status = status_of
                    .get(run.run_id.as_str())
                    .copied()
                    .unwrap_or("unknown")
                    .to_string();
            }
            c
        })
        .collect();

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

        // Non-terminal runs become swimlane bars. Paused is deliberately
        // included: is_terminal() excludes it because a paused run expects a
        // resume, so it is still live work.
        if !r.status.is_terminal() {
            active_runs.push(ActiveRunBar {
                run_id: r.id.clone(),
                workflow_name: r.workflow_name.clone(),
                status: r.status.as_str().to_string(),
                started_at: r.started_at,
                trigger: r.trigger_str().to_string(),
                cycle_id: cycle_of.get(r.id.as_str()).map(|c| c.to_string()),
            });
        }

        if r.status.is_terminal() {
            let key = day_key(r.started_at);
            let b = buckets.entry(key).or_insert(TerminalBucket {
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

        // A run belonging to a cycle is grouped under that cycle in the feed
        // (see `cycle_of` above) even when it has no trigger provenance of
        // its own — it must never also leak into recent_manual.
        if r.trigger_str() == "manual" && !cycle_of.contains_key(r.id.as_str()) {
            recent_manual.push(RecentRun {
                id: r.id.clone(),
                workflow_name: r.workflow_name.clone(),
                status: r.status.as_str().to_string(),
                started_at: r.started_at,
                finished_at: r.finished_at,
                trigger: "manual".to_string(),
            });
        }
    }

    // Fill the bucket grid. Without this the trend area silently closes gaps
    // and reads as continuous activity across days that had none.
    let terminal_buckets = fill_bucket_grid(buckets, range, now);

    active_runs.sort_by_key(|b| std::cmp::Reverse(b.started_at));
    recent_manual.sort_by_key(|r| std::cmp::Reverse(r.started_at));

    DashboardSummary {
        active,
        terminal_buckets,
        active_runs,
        cycles,
        recent_manual,
        findings_open,
        captured_at: now,
    }
}

/// Emit a contiguous day-by-day grid, zero-filling days with no terminal runs.
fn fill_bucket_grid(
    mut buckets: BTreeMap<DateTime<Utc>, TerminalBucket>,
    range: DashboardRange,
    now: DateTime<Utc>,
) -> Vec<TerminalBucket> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::dashboard_summary::CycleRun;

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
        let s = build_summary(&runs, &[], 0, DashboardRange::All, chrono::Utc::now());
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
        let s = build_summary(&runs, &[], 0, DashboardRange::All, chrono::Utc::now());
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
        let s = build_summary(&runs, &[], 0, DashboardRange::Days7, chrono::Utc::now());
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
        let s = build_summary(&runs, &[], 0, DashboardRange::Days7, chrono::Utc::now());
        assert!(
            s.terminal_buckets.len() >= 4,
            "expected a filled bucket grid, got {} buckets",
            s.terminal_buckets.len()
        );
    }

    #[test]
    fn manual_runs_are_separated_from_cycle_runs() {
        use rupu_orchestrator::runs::RunStatus::*;
        let mut cron = rec("r_cron", Completed, 1);
        cron.source_wake_id = Some("wake_1".into());
        let manual = rec("r_manual", Completed, 1);
        let s = build_summary(
            &[cron, manual],
            &[],
            0,
            DashboardRange::All,
            chrono::Utc::now(),
        );
        assert_eq!(
            s.recent_manual.len(),
            1,
            "only the manual run belongs in recent_manual"
        );
        assert_eq!(s.recent_manual[0].id, "r_manual");
    }

    #[test]
    fn cycle_runs_get_their_status_joined_from_the_runs_in_hand() {
        use rupu_orchestrator::runs::RunStatus::*;
        let runs = vec![rec("r_ok", Completed, 5), rec("r_bad", Failed, 5)];
        let cycles = vec![CycleRollup {
            cycle_id: "cyc_1".into(),
            worker_name: Some("nightly".into()),
            started_at: chrono::Utc::now() - chrono::Duration::minutes(6),
            finished_at: None,
            ran: 2,
            skipped: 0,
            failed: 1,
            // status starts "unknown" — collect_cycle_rollups does no per-run reads.
            runs: vec![
                CycleRun {
                    run_id: "r_ok".into(),
                    status: "unknown".into(),
                },
                CycleRun {
                    run_id: "r_bad".into(),
                    status: "unknown".into(),
                },
            ],
        }];
        let s = build_summary(&runs, &cycles, 0, DashboardRange::All, chrono::Utc::now());
        let c = &s.cycles[0];
        assert_eq!(
            c.runs.iter().find(|r| r.run_id == "r_ok").unwrap().status,
            "completed"
        );
        assert_eq!(
            c.runs.iter().find(|r| r.run_id == "r_bad").unwrap().status,
            "failed"
        );
    }

    #[test]
    fn a_cycle_run_we_cannot_resolve_stays_unknown_and_is_not_dropped() {
        // A cycle whose run list silently shrank would disagree with its own
        // `ran` count. Unresolvable runs must survive as "unknown".
        let cycles = vec![CycleRollup {
            cycle_id: "cyc_1".into(),
            worker_name: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
            ran: 1,
            skipped: 0,
            failed: 0,
            runs: vec![CycleRun {
                run_id: "r_gone".into(),
                status: "unknown".into(),
            }],
        }];
        let s = build_summary(&[], &cycles, 0, DashboardRange::All, chrono::Utc::now());
        assert_eq!(s.cycles[0].runs.len(), 1, "the run must not be dropped");
        assert_eq!(s.cycles[0].runs[0].status, "unknown");
    }

    #[test]
    fn a_run_belonging_to_a_cycle_is_not_listed_as_a_manual_run() {
        use rupu_orchestrator::runs::RunStatus::*;
        // Cycle-owned runs are grouped under their cycle; only manual runs are
        // listed individually. A cycle-owned run with no trigger provenance
        // must not leak into recent_manual.
        let runs = vec![rec("r_in_cycle", Completed, 5)];
        let cycles = vec![CycleRollup {
            cycle_id: "cyc_1".into(),
            worker_name: None,
            started_at: chrono::Utc::now() - chrono::Duration::minutes(6),
            finished_at: None,
            ran: 1,
            skipped: 0,
            failed: 0,
            runs: vec![CycleRun {
                run_id: "r_in_cycle".into(),
                status: "unknown".into(),
            }],
        }];
        let s = build_summary(&runs, &cycles, 0, DashboardRange::All, chrono::Utc::now());
        assert!(
            s.recent_manual.iter().all(|r| r.id != "r_in_cycle"),
            "a run owned by a cycle must not also appear as a manual run"
        );
    }

    #[test]
    fn cycles_outside_the_range_are_excluded() {
        let old = CycleRollup {
            cycle_id: "old".into(),
            worker_name: None,
            started_at: chrono::Utc::now() - chrono::Duration::days(10),
            finished_at: None,
            ran: 1,
            skipped: 0,
            failed: 0,
            runs: vec![],
        };
        let recent = CycleRollup {
            cycle_id: "recent".into(),
            worker_name: None,
            started_at: chrono::Utc::now() - chrono::Duration::hours(1),
            finished_at: None,
            ran: 1,
            skipped: 0,
            failed: 0,
            runs: vec![],
        };
        let s = build_summary(
            &[],
            &[old, recent],
            0,
            DashboardRange::Days7,
            chrono::Utc::now(),
        );
        assert_eq!(s.cycles.len(), 1);
        assert_eq!(s.cycles[0].cycle_id, "recent");
    }
}
