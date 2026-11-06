//! Autoflow cycle history as rows: what `rupu autoflow history` lists and
//! `rupu autoflow explain` shows as an issue's recent cycle events.

use rupu_runtime::{AutoflowCycleEvent, AutoflowHistoryEventRecord};
use serde::Serialize;

/// One history event, as `rupu autoflow history` lists it.
#[derive(Debug, Clone, Serialize)]
pub struct AutoflowHistoryRow {
    pub at: String,
    pub cycle_id: String,
    pub mode: String,
    pub worker: String,
    pub event: String,
    pub issue: String,
    pub source: String,
    pub workflow: String,
    pub repo: String,
    pub run: String,
    pub wake: String,
    pub detail: String,
}

/// The name an event goes by in the history and monitor views.
pub fn monitor_event_name(event: &AutoflowCycleEvent) -> String {
    match event.kind {
        rupu_runtime::AutoflowCycleEventKind::WakeConsumed => "wake_consumed",
        rupu_runtime::AutoflowCycleEventKind::WakeSkipped => "wake_skipped",
        rupu_runtime::AutoflowCycleEventKind::ClaimAcquired => "claim_acquired",
        rupu_runtime::AutoflowCycleEventKind::ClaimReleased => "claim_released",
        rupu_runtime::AutoflowCycleEventKind::ClaimTakeover => "claim_takeover",
        rupu_runtime::AutoflowCycleEventKind::RunLaunched => "run_launched",
        rupu_runtime::AutoflowCycleEventKind::IssueCommented => "issue_commented",
        rupu_runtime::AutoflowCycleEventKind::IssueStateChanged => match event.status.as_deref() {
            Some("closed") => "issue_closed",
            Some("open") => "issue_reopened",
            _ => "issue_state_changed",
        },
        rupu_runtime::AutoflowCycleEventKind::PullRequestOpened => match event.status.as_deref() {
            Some("draft") => "draft_pr_opened",
            _ => "pr_opened",
        },
        rupu_runtime::AutoflowCycleEventKind::AwaitingHuman => "awaiting_human",
        rupu_runtime::AutoflowCycleEventKind::AwaitingExternal => "awaiting_external",
        rupu_runtime::AutoflowCycleEventKind::RetryScheduled => "retry_scheduled",
        rupu_runtime::AutoflowCycleEventKind::DispatchQueued => "dispatch_queued",
        rupu_runtime::AutoflowCycleEventKind::CleanupPerformed => "cleanup_performed",
        rupu_runtime::AutoflowCycleEventKind::CycleSkipped => "cycle_skipped",
        rupu_runtime::AutoflowCycleEventKind::CycleFailed => "cycle_failed",
    }
    .into()
}

/// Whether a worker (by id or name) passes `filter`.
pub fn worker_filter_matches(filter: Option<&str>, worker_id: &str, worker_name: &str) -> bool {
    filter.is_none_or(|filter| filter == worker_id || filter == worker_name)
}

/// The events of `repo_filter` (when given) by `worker_filter` (when given).
pub fn filter_monitor_events(
    events: Vec<AutoflowHistoryEventRecord>,
    repo_filter: Option<&str>,
    worker_filter: Option<&str>,
) -> Vec<AutoflowHistoryEventRecord> {
    events
        .into_iter()
        .filter(|record| {
            repo_filter.is_none_or(|filter| {
                record.repo_filter.as_deref() == Some(filter)
                    || record.event.repo_ref.as_deref() == Some(filter)
            })
        })
        .filter(|record| {
            worker_filter_matches(
                worker_filter,
                record.worker_id.as_deref().unwrap_or(""),
                record.worker_name.as_deref().unwrap_or(""),
            )
        })
        .collect()
}

/// `events` as history rows, filtered by issue, source and event name.
pub fn history_rows_from_events(
    events: &[AutoflowHistoryEventRecord],
    issue_filter: Option<&str>,
    source_filter: Option<&str>,
    event_filter: Option<&str>,
) -> Vec<AutoflowHistoryRow> {
    events
        .iter()
        .filter_map(|record| {
            let event = &record.event;
            let event_name = monitor_event_name(event);
            let issue = event
                .issue_display_ref
                .clone()
                .or_else(|| event.issue_ref.clone())
                .unwrap_or_else(|| "-".into());
            let issue_ref = event.issue_ref.as_deref().unwrap_or("");
            let source = event.source_ref.clone().unwrap_or_else(|| "-".into());
            let worker = record
                .worker_name
                .clone()
                .or_else(|| record.worker_id.clone())
                .unwrap_or_else(|| "-".into());
            let repo = event.repo_ref.clone().unwrap_or_else(|| "-".into());
            let workflow = event.workflow.clone().unwrap_or_else(|| "-".into());
            let run = event.run_id.clone().unwrap_or_else(|| "-".into());
            let wake = event
                .wake_id
                .clone()
                .or_else(|| event.wake_event_id.clone())
                .unwrap_or_else(|| "-".into());
            let detail = event.detail.clone().unwrap_or_else(|| "-".into());
            let source_match = source_filter.is_none_or(|filter| filter == source);
            let issue_match =
                issue_filter.is_none_or(|filter| filter == issue_ref || filter == issue);
            let event_match = event_filter.is_none_or(|filter| filter == event_name);
            if !(source_match && issue_match && event_match) {
                return None;
            }
            Some(AutoflowHistoryRow {
                at: record.at.clone(),
                cycle_id: record.cycle_id.clone(),
                mode: match record.mode {
                    rupu_runtime::AutoflowCycleMode::Tick => "tick".into(),
                    rupu_runtime::AutoflowCycleMode::Serve => "serve".into(),
                },
                worker,
                event: event_name.to_string(),
                issue,
                source,
                workflow,
                repo,
                run,
                wake,
                detail,
            })
        })
        .collect()
}
