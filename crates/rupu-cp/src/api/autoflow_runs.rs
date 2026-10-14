//! Autoflow runs as one table (spec
//! `docs/superpowers/specs/2026-10-14-rupu-cp-autoflow-runs-one-table-design.md`).
//!
//! The run list is the source of truth for autoflow runs (a run row whose
//! `autoflow` is non-null); the autoflow history contributes only the
//! attempts that never produced a recorded run ([`attempt_rows`]) and the
//! failures that name no workflow at all ([`problems`]).

use crate::api::autoflow_claims::ClaimIndex;
use crate::api::runs::RunAutoflow;
use chrono::{DateTime, Utc};
use rupu_orchestrator::RunStatus;
use rupu_runtime::{AutoflowCycleEventKind, AutoflowHistoryEventRecord};
use std::collections::HashMap;

/// What an attempt row says about the attempt itself (web `AutoflowAttempt`).
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct AttemptInfo {
    /// The failure text, when the event carried one.
    pub detail: Option<String>,
    pub cycle_id: String,
    pub worker: Option<String>,
    /// The claim's status as the event recorded it.
    pub claim_status: Option<String>,
}

/// A failed autoflow attempt in `RunListRow`'s shape, so the web renders it
/// with the Workflows columns: zero usage, `autoflow` set, and `attempt`.
#[derive(serde::Serialize, Clone, Debug)]
pub struct AttemptRow {
    /// The event's run id when it had one, else the history event id.
    pub id: String,
    pub workflow_name: String,
    pub status: RunStatus,
    /// The event time.
    pub started_at: DateTime<Utc>,
    /// Same as `started_at`: an attempt has no duration.
    pub finished_at: Option<DateTime<Utc>>,
    /// `event` when the event names the wake event it answered, else `cron`.
    pub trigger: &'static str,
    pub usage: crate::usage::UsageSummary,
    pub turns: u64,
    pub duration_ms: Option<u64>,
    pub codename: String,
    pub codename_derived: bool,
    /// Always `Some`: an attempt is an autoflow's.
    pub autoflow: Option<RunAutoflow>,
    pub attempt: AttemptInfo,
    /// `Some(false)` when `id` is a run id with no run record (the run page
    /// renders such a run from history); absent when `id` is an event id.
    /// Never `true`: an attempt whose run was recorded is the run row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_recorded: Option<bool>,
}

/// The fields of one history event an attempt row is built from — a local
/// [`AutoflowHistoryEventRecord`] or a remote host's event row JSON.
struct AttemptSource {
    event_id: String,
    cycle_id: String,
    at: DateTime<Utc>,
    /// `Some(id)` only when the row may use the run id as its own: the
    /// coordinator could check that no run record exists for it.
    unrecorded_run_id: Option<String>,
    workflow: String,
    status: RunStatus,
    autoflow: RunAutoflow,
    worker: Option<String>,
    claim_status: Option<String>,
    detail: Option<String>,
    event_trigger: bool,
}

impl AttemptSource {
    fn into_row(self) -> AttemptRow {
        let run_recorded = self.unrecorded_run_id.as_ref().map(|_| false);
        let id = self.unrecorded_run_id.unwrap_or(self.event_id);
        let (codename, codename_derived) = crate::codename::named(None, &id, None);
        AttemptRow {
            id,
            workflow_name: self.workflow,
            status: self.status,
            started_at: self.at,
            finished_at: Some(self.at),
            trigger: if self.event_trigger { "event" } else { "cron" },
            usage: crate::usage::UsageSummary::default(),
            turns: 0,
            duration_ms: None,
            codename,
            codename_derived,
            autoflow: Some(self.autoflow),
            attempt: AttemptInfo {
                detail: self.detail,
                cycle_id: self.cycle_id,
                worker: self.worker,
                claim_status: self.claim_status,
            },
            run_recorded,
        }
    }
}

/// `at` as a UTC instant, or `None` when it isn't RFC 3339.
fn parse_at(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Newest first by `(started_at, id)`.
fn newest_first(rows: &mut [AttemptRow]) {
    rows.sort_by(|a, b| (b.started_at, &b.id).cmp(&(a.started_at, &a.id)));
}

/// The failed attempts among the local history `events`, newest first:
///
/// - a `cycle_failed` event naming a `workflow` → status `failed`;
/// - with `check_launched`, a `run_launched` event whose `run_id` has no
///   run record (`!run_exists(id)`) → status from the claim status the way
///   the run page maps it ([`crate::api::run_resolve::map_claim_status_str`]).
///
/// An event whose `run_id` names a recorded run is never an attempt (the
/// run row is listed). An event whose `at` isn't a timestamp can't be placed
/// and is left out. Claim fields come from `claims`.
pub fn attempt_rows(
    events: &[AutoflowHistoryEventRecord],
    run_exists: &dyn Fn(&str) -> bool,
    check_launched: bool,
    claims: &ClaimIndex,
) -> Vec<AttemptRow> {
    let mut rows: Vec<AttemptRow> = events
        .iter()
        .filter_map(|rec| {
            let e = &rec.event;
            if e.run_id.as_deref().is_some_and(run_exists) {
                return None;
            }
            let status = match e.kind {
                AutoflowCycleEventKind::CycleFailed if e.workflow.is_some() => RunStatus::Failed,
                AutoflowCycleEventKind::RunLaunched if check_launched && e.run_id.is_some() => {
                    crate::api::run_resolve::map_claim_status_str(e.status.as_deref().unwrap_or(""))
                }
                _ => return None,
            };
            let at = parse_at(&rec.at)?;
            let id = e.run_id.clone().unwrap_or_else(|| rec.event_id.clone());
            let mut autoflow = RunAutoflow {
                issue_ref: e.issue_ref.clone(),
                ..Default::default()
            };
            claims.fill(&id, &mut autoflow);
            if autoflow.issue_display_ref.is_none() {
                autoflow.issue_display_ref = e.issue_display_ref.clone();
            }
            Some(
                AttemptSource {
                    event_id: rec.event_id.clone(),
                    cycle_id: rec.cycle_id.clone(),
                    at,
                    unrecorded_run_id: e.run_id.clone(),
                    workflow: e.workflow.clone().unwrap_or_default(),
                    status,
                    autoflow,
                    worker: rec.worker_name.clone().or_else(|| rec.worker_id.clone()),
                    claim_status: e.status.clone(),
                    detail: e.detail.clone(),
                    event_trigger: e.wake_event_id.is_some(),
                }
                .into_row(),
            )
        })
        .collect();
    newest_first(&mut rows);
    rows
}

/// The failed attempts among a remote host's `/api/runs/autoflows/events`
/// rows: only `cycle_failed` events naming a workflow — the coordinator
/// can't check whether a remote `run_launched` run was recorded — each under
/// its event id (a remote run id can't be vouched for either).
pub fn remote_attempt_rows(rows: &[serde_json::Value]) -> Vec<AttemptRow> {
    let str_of = |row: &serde_json::Value, key: &str| {
        row.get(key).and_then(|v| v.as_str()).map(str::to_owned)
    };
    let mut out: Vec<AttemptRow> = rows
        .iter()
        .filter_map(|row| {
            if str_of(row, "kind").as_deref() != Some("cycle_failed") {
                return None;
            }
            let workflow = str_of(row, "workflow")?;
            let at = parse_at(&str_of(row, "at")?)?;
            Some(
                AttemptSource {
                    event_id: str_of(row, "event_id")?,
                    cycle_id: str_of(row, "cycle_id").unwrap_or_default(),
                    at,
                    unrecorded_run_id: None,
                    workflow,
                    status: RunStatus::Failed,
                    autoflow: RunAutoflow {
                        issue_display_ref: str_of(row, "issue_display_ref"),
                        ..Default::default()
                    },
                    worker: str_of(row, "worker_name"),
                    claim_status: str_of(row, "status"),
                    detail: str_of(row, "detail"),
                    event_trigger: false,
                }
                .into_row(),
            )
        })
        .collect();
    newest_first(&mut out);
    out
}

/// One line of the problems strip: `cycle_failed` events naming no
/// workflow, grouped by host and subject.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct Problem {
    pub host_id: String,
    /// `"repo"` (the event named a repo) or `"worker"`.
    pub subject_kind: &'static str,
    pub subject: String,
    /// The newest event's detail.
    pub detail: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub count: usize,
}

/// The fields [`problems`] needs, from a local record or a remote event row.
#[derive(Clone, Debug, PartialEq)]
pub struct ProblemEvent {
    pub at: String,
    pub repo_ref: Option<String>,
    pub worker: Option<String>,
    pub detail: Option<String>,
}

/// The subject a failure that names no workflow is about.
const UNNAMED_WORKER: &str = "autoflow worker";

/// `events` (failures that name no workflow) at or after `cutoff`, grouped
/// by `repo_ref`, else the worker (`UNNAMED_WORKER` when unknown), newest
/// `last_seen` first. An event whose `at` isn't a timestamp is left out.
pub fn problems(events: &[ProblemEvent], host_id: &str, cutoff: DateTime<Utc>) -> Vec<Problem> {
    struct Group {
        problem: Problem,
        first: DateTime<Utc>,
        last: DateTime<Utc>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<(&'static str, String), usize> = HashMap::new();
    for e in events {
        let Some(at) = parse_at(&e.at) else { continue };
        if at < cutoff {
            continue;
        }
        let key = match (&e.repo_ref, &e.worker) {
            (Some(repo), _) => ("repo", repo.clone()),
            (None, Some(w)) => ("worker", w.clone()),
            (None, None) => ("worker", UNNAMED_WORKER.to_string()),
        };
        let i = *index.entry(key.clone()).or_insert_with(|| {
            groups.push(Group {
                problem: Problem {
                    host_id: host_id.to_string(),
                    subject_kind: key.0,
                    subject: key.1,
                    detail: e.detail.clone(),
                    first_seen: e.at.clone(),
                    last_seen: e.at.clone(),
                    count: 0,
                },
                first: at,
                last: at,
            });
            groups.len() - 1
        });
        let g = &mut groups[i];
        g.problem.count += 1;
        if at < g.first {
            g.first = at;
            g.problem.first_seen = e.at.clone();
        }
        if at >= g.last {
            g.last = at;
            g.problem.last_seen = e.at.clone();
            g.problem.detail = e.detail.clone();
        }
    }
    groups.sort_by(|a, b| b.last.cmp(&a.last));
    groups.into_iter().map(|g| g.problem).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::autoflow_claims::ClaimIndex;
    use rupu_orchestrator::RunStatus;
    use rupu_runtime::{
        AutoflowCycleEvent, AutoflowCycleEventKind, AutoflowCycleMode, AutoflowCycleRecord,
        AutoflowHistoryEventRecord,
    };

    fn ev(
        kind: AutoflowCycleEventKind,
        at: &str,
        f: impl FnOnce(&mut AutoflowCycleEvent),
    ) -> AutoflowHistoryEventRecord {
        let at = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let cycle = AutoflowCycleRecord::new(AutoflowCycleMode::Tick, at);
        let mut e = AutoflowCycleEvent {
            kind,
            ..Default::default()
        };
        f(&mut e);
        AutoflowHistoryEventRecord::from_cycle_event(&cycle, e, at)
    }

    #[test]
    fn cycle_failed_with_workflow_is_a_failed_attempt() {
        let events = [ev(
            AutoflowCycleEventKind::CycleFailed,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.issue_ref = Some("github:acme/api/issues/7".into());
                e.detail = Some("dispatch failed: boom".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, events[0].event_id);
        assert_eq!(rows[0].workflow_name, "fix-issue");
        assert_eq!(rows[0].status, RunStatus::Failed);
        assert_eq!(rows[0].trigger, "cron");
        assert_eq!(
            rows[0].attempt.detail.as_deref(),
            Some("dispatch failed: boom")
        );
        assert_eq!(rows[0].attempt.cycle_id, events[0].cycle_id);
        assert_eq!(
            rows[0].autoflow.as_ref().unwrap().issue_ref.as_deref(),
            Some("github:acme/api/issues/7")
        );
        assert_eq!(rows[0].run_recorded, None, "its id is a history event id");
        let v = serde_json::to_value(&rows[0]).unwrap();
        assert!(v.get("run_recorded").is_none());
        assert_eq!(v["started_at"], "2026-10-01T10:00:00Z");
        assert_eq!(v["usage"]["total_tokens"], 0);
    }

    #[test]
    fn cycle_failed_without_workflow_is_not_an_attempt() {
        let events = [ev(
            AutoflowCycleEventKind::CycleFailed,
            "2026-10-01T10:00:00Z",
            |e| e.repo_ref = Some("github:acme/api".into()),
        )];
        assert!(attempt_rows(&events, &|_| false, true, &ClaimIndex::default()).is_empty());
    }

    #[test]
    fn attempt_with_recorded_run_is_not_listed() {
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_exists".into());
                e.status = Some("blocked".into());
            },
        )];
        assert!(attempt_rows(
            &events,
            &|id| id == "run_exists",
            true,
            &ClaimIndex::default()
        )
        .is_empty());
    }

    #[test]
    fn launched_run_never_recorded_is_an_attempt_with_its_run_id() {
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_ghost".into());
                e.status = Some("blocked".into());
                e.wake_event_id = Some("evt_1".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        assert_eq!(rows[0].id, "run_ghost");
        assert_eq!(rows[0].run_recorded, Some(false));
        assert_eq!(rows[0].status, RunStatus::Failed);
        assert_eq!(rows[0].trigger, "event");
        assert_eq!(rows[0].attempt.claim_status.as_deref(), Some("blocked"));
        // remote mode never checks launches
        assert!(attempt_rows(&events, &|_| false, false, &ClaimIndex::default()).is_empty());
    }

    #[test]
    fn launched_run_status_follows_the_claim_status_mapping() {
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_ghost".into());
                e.status = Some("await_human".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        assert_eq!(rows[0].status, RunStatus::AwaitingApproval);
    }

    #[test]
    fn attempts_are_newest_first_whatever_the_input_order() {
        let mk = |at: &str| {
            ev(AutoflowCycleEventKind::CycleFailed, at, |e| {
                e.workflow = Some("fix-issue".into())
            })
        };
        let events = [
            mk("2026-10-01T10:00:00Z"),
            mk("2026-10-01T12:00:00Z"),
            mk("2026-10-01T11:00:00Z"),
        ];
        let rows = attempt_rows(&events, &|_| false, true, &ClaimIndex::default());
        let ats: Vec<String> = rows.iter().map(|r| r.started_at.to_rfc3339()).collect();
        assert_eq!(
            ats,
            [
                "2026-10-01T12:00:00+00:00",
                "2026-10-01T11:00:00+00:00",
                "2026-10-01T10:00:00+00:00"
            ]
        );
    }

    #[test]
    fn attempt_takes_its_claim_fields_from_the_index() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = rupu_workspace::AutoflowClaimStore {
            root: tmp.path().join("autoflows").join("claims"),
        };
        let mut claim = crate::api::autoflow_claims::tests::seed_record("github:acme/api/issues/7");
        claim.issue_display_ref = Some("acme/api#7".into());
        claim.issue_url = Some("https://example.test/7".into());
        claim.pr_url = Some("https://example.test/pr/9".into());
        claim.last_run_id = Some("run_ghost".into());
        store.save(&claim).unwrap();
        let idx = ClaimIndex::load(tmp.path());
        let events = [ev(
            AutoflowCycleEventKind::RunLaunched,
            "2026-10-01T10:00:00Z",
            |e| {
                e.workflow = Some("fix-issue".into());
                e.run_id = Some("run_ghost".into());
                e.issue_ref = Some("github:acme/api/issues/7".into());
            },
        )];
        let rows = attempt_rows(&events, &|_| false, true, &idx);
        assert_eq!(
            rows[0].autoflow,
            Some(crate::api::runs::RunAutoflow {
                issue_ref: Some("github:acme/api/issues/7".into()),
                issue_display_ref: Some("acme/api#7".into()),
                issue_url: Some("https://example.test/7".into()),
                pr_url: Some("https://example.test/pr/9".into()),
            })
        );
    }

    #[test]
    fn problems_group_count_and_age_out() {
        let pe = |at: &str, repo: Option<&str>, worker: Option<&str>, d: &str| ProblemEvent {
            at: at.into(),
            repo_ref: repo.map(Into::into),
            worker: worker.map(Into::into),
            detail: Some(d.into()),
        };
        let events = vec![
            pe("2026-10-01T09:00:00Z", Some("github:acme/api"), None, "old"),
            pe(
                "2026-10-01T10:10:00Z",
                Some("github:acme/api"),
                None,
                "config bad",
            ),
            pe(
                "2026-10-01T10:20:00Z",
                Some("github:acme/api"),
                None,
                "config still bad",
            ),
            pe("2026-10-01T10:30:00Z", None, Some("mini"), "rate limited"),
            pe("2026-10-01T10:05:00Z", None, None, "crashed"),
        ];
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-10-01T09:45:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let got = problems(&events, "local", cutoff);
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].subject_kind, got[0].subject.as_str(), got[0].count),
            ("worker", "mini", 1)
        );
        assert_eq!(
            (got[1].subject_kind, got[1].subject.as_str(), got[1].count),
            ("repo", "github:acme/api", 2)
        );
        assert_eq!(got[1].detail.as_deref(), Some("config still bad"));
        assert_eq!(got[1].first_seen, "2026-10-01T10:10:00Z");
        assert_eq!(got[1].last_seen, "2026-10-01T10:20:00Z");
        assert_eq!(got[1].host_id, "local");
        assert_eq!(
            (got[2].subject_kind, got[2].subject.as_str()),
            ("worker", "autoflow worker")
        );
    }
}
