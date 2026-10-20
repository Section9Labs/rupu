//! Wake rows and the wake checks claim repair and the doctor share.

use crate::paths;
use crate::refs::normalize_repo_filter;
use rupu_runtime::{
    WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeRecord, WakeSource, WakeStore,
};
use serde::Serialize;
use std::path::Path;

/// One row of the `autoflow_wakes` report.
#[derive(Debug, Clone, Serialize)]
pub struct AutoflowWakeRow {
    pub wake_id: String,
    pub state: String,
    pub source: String,
    pub event: String,
    pub entity: String,
    pub not_before: String,
    pub repo: String,
}

/// `rupu autoflow wakes`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct AutoflowWakesReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<AutoflowWakeRow>,
}

pub fn wake_source_name(source: WakeSource) -> &'static str {
    match source {
        WakeSource::Manual => "manual",
        WakeSource::CronPoll => "cron_poll",
        WakeSource::Webhook => "webhook",
        WakeSource::AutoflowDispatch => "dispatch",
        WakeSource::Retry => "retry",
        WakeSource::ApprovalResume => "approval_resume",
        WakeSource::Repair => "repair",
    }
}

pub fn wake_row_from_record(wake: &WakeRecord, state: &str) -> AutoflowWakeRow {
    AutoflowWakeRow {
        wake_id: wake.wake_id.clone(),
        state: state.to_string(),
        source: wake_source_name(wake.source).into(),
        event: wake.event.id.clone(),
        entity: wake.entity.ref_text.clone(),
        not_before: wake.not_before.clone(),
        repo: wake.repo_ref.clone(),
    }
}

pub fn filter_wakes_by_repo(wakes: Vec<WakeRecord>, repo_filter: Option<&str>) -> Vec<WakeRecord> {
    match repo_filter {
        Some(repo_ref) => wakes
            .into_iter()
            .filter(|wake| wake.repo_ref == repo_ref)
            .collect(),
        None => wakes,
    }
}

pub fn wakes_for_issue(wakes: Vec<WakeRecord>, issue_ref: &str) -> Vec<WakeRecord> {
    wakes
        .into_iter()
        .filter(|wake| {
            wake.entity.kind == WakeEntityKind::Issue && wake.entity.ref_text == issue_ref
        })
        .collect()
}

pub fn describe_wake_source(global: &Path, wake_store: &WakeStore, wake_id: &str) -> String {
    match wake_store.load(wake_id) {
        Ok(wake) => {
            let state = if paths::autoflow_wake_queue_dir(global)
                .join(format!("{wake_id}.json"))
                .is_file()
            {
                "queued"
            } else if paths::autoflow_wake_processed_dir(global)
                .join(format!("{wake_id}.json"))
                .is_file()
            {
                "processed"
            } else {
                "unknown"
            };
            format!(
                "{wake_id} [{state}] {} {}",
                wake_source_name(wake.source),
                wake.event.id
            )
        }
        Err(error) => format!("{wake_id} (missing: {error})"),
    }
}

pub fn invalid_wake_payload_detail(wake_store: &WakeStore, wake: &WakeRecord) -> Option<String> {
    let payload_ref = wake.payload_ref.as_ref()?;
    if !payload_ref.is_file() {
        return Some(format!(
            "references missing payload `{}`",
            payload_ref.display()
        ));
    }
    match wake_store.read_payload(&wake.wake_id) {
        Ok(_) => None,
        Err(error) => Some(format!("has unreadable payload: {error}")),
    }
}

pub fn enqueue_issue_wake(
    wake_store: &WakeStore,
    source: WakeSource,
    repo_ref: &str,
    issue_ref: &str,
    event_id: &str,
    not_before: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<WakeRecord> {
    Ok(wake_store.enqueue(WakeEnqueueRequest {
        source,
        repo_ref: repo_ref.to_string(),
        entity: WakeEntity {
            kind: WakeEntityKind::Issue,
            ref_text: issue_ref.to_string(),
        },
        event: WakeEvent {
            id: event_id.to_string(),
            delivery_id: None,
            dedupe_key: None,
        },
        payload: None,
        received_at: chrono::Utc::now().to_rfc3339(),
        not_before: not_before.to_rfc3339(),
    })?)
}

/// The `autoflow_wakes` report: queued wakes, then the ten most recently
/// processed. `rupu autoflow wakes --format json` prints it and
/// `GET /api/autoflows/wakes` serves it.
pub fn wakes_report(global: &Path, repo: Option<&str>) -> anyhow::Result<AutoflowWakesReport> {
    let repo_filter = normalize_repo_filter(repo)?;
    let store = WakeStore::new(paths::autoflow_wakes_dir(global));
    let queued = filter_wakes_by_repo(store.list_queued()?, repo_filter.as_deref());
    let mut processed = filter_wakes_by_repo(store.list_processed()?, repo_filter.as_deref());
    processed.sort_by(|left, right| {
        right
            .received_at
            .cmp(&left.received_at)
            .then_with(|| right.wake_id.cmp(&left.wake_id))
    });
    let recent_processed = processed.into_iter().take(10).collect::<Vec<_>>();
    let mut rows = queued
        .into_iter()
        .map(|wake| AutoflowWakeRow {
            wake_id: wake.wake_id,
            state: "queued".into(),
            source: wake_source_name(wake.source).into(),
            event: wake.event.id,
            entity: wake.entity.ref_text,
            not_before: wake.not_before,
            repo: wake.repo_ref,
        })
        .collect::<Vec<_>>();
    rows.extend(recent_processed.into_iter().map(|wake| AutoflowWakeRow {
        wake_id: wake.wake_id,
        state: "processed".into(),
        source: wake_source_name(wake.source).into(),
        event: wake.event.id,
        entity: wake.entity.ref_text,
        not_before: wake.not_before,
        repo: wake.repo_ref,
    }));
    Ok(AutoflowWakesReport {
        kind: "autoflow_wakes",
        version: 1,
        rows,
    })
}
