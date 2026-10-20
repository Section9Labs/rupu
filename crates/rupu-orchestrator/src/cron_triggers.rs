//! What the cron and event-trigger tick reads, and what `rupu cron list`,
//! `rupu cron events` and the control plane's `GET /api/cron` /
//! `GET /api/cron/events` report: the cron- and event-triggered workflows
//! of the global and project layers, the event-poll cursors, and the lock
//! that keeps two ticks from running at once.
//!
//! The tick itself (firing runs, polling sources) stays with the CLI's
//! workflow runtime (`rupu cron tick`, `cp serve`'s cron loop).

use crate::{TriggerKind, Workflow};
use chrono::{DateTime, Utc};
use rupu_config::PollSourceEntry;
use rupu_scm::EventSourceRef;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// `<global>/cron-state`: last-fired stamps, event cursors, the tick lock.
pub fn state_dir(global: &Path) -> PathBuf {
    global.join("cron-state")
}

/// `<global>/cron-state/event-cursors`.
pub fn event_cursors_dir(global: &Path) -> PathBuf {
    state_dir(global).join("event-cursors")
}

/// Held while a tick runs: an exclusive `fs2` lock on
/// `<global>/cron-state/tick.lock`, so `cp serve`'s cron loop, a
/// `POST /api/cron/tick`, a crontab `rupu cron tick` and a second
/// `cp serve` on the same `RUPU_HOME` never tick at once. Released when
/// dropped (or when the process dies).
#[derive(Debug)]
pub struct TickLock {
    _file: std::fs::File,
}

impl TickLock {
    /// The lock file.
    pub fn path(global: &Path) -> PathBuf {
        state_dir(global).join("tick.lock")
    }

    /// Take the lock, or `None` while another tick holds it.
    pub fn try_acquire(global: &Path) -> std::io::Result<Option<Self>> {
        use fs2::FileExt;
        std::fs::create_dir_all(state_dir(global))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(Self::path(global))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(e) if e.kind() == fs2::lock_contended_error().kind() => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// A workflow with `trigger.on: cron`.
pub struct CronWorkflow {
    /// The file stem.
    pub name: String,
    pub schedule: String,
}

/// Walk the global and project workflows directories, parse each YAML
/// file, and collect every workflow with `trigger.on: cron`. Project
/// entries shadow global by name (same precedence as
/// `rupu workflow list`).
pub fn collect_cron_workflows(global: &Path, project_root: Option<&Path>) -> Vec<CronWorkflow> {
    let mut by_name: BTreeMap<String, CronWorkflow> = BTreeMap::new();
    push_cron(&global.join("workflows"), &mut by_name);
    if let Some(p) = project_root {
        push_cron(&p.join(".rupu/workflows"), &mut by_name);
    }
    by_name.into_values().collect()
}

pub fn push_cron(dir: &Path, into: &mut BTreeMap<String, CronWorkflow>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping unreadable workflow");
                continue;
            }
        };
        let wf = match Workflow::parse(&body) {
            Ok(w) => w,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping malformed workflow");
                continue;
            }
        };
        if wf.trigger.on != TriggerKind::Cron {
            continue;
        }
        // An autoflow the operator disabled must not fire on the cron tick.
        //
        // The CP web UI's disable toggle (`api::autoflows::set_autoflow_enabled`)
        // writes `autoflow.enabled: false` into this YAML, and the autoflow
        // ENGINE honors it (`cmd/autoflow.rs`'s `.filter(|a| a.enabled)`). The
        // cron tick is a SECOND, independent dispatch subsystem, and it used to
        // select purely on `trigger.on: cron` — so a cron-triggered workflow
        // showing as "disabled" in the UI kept firing on schedule. Observed
        // live on `nightly-health`.
        //
        // Deliberately keyed on an EXPLICIT autoflow block: a workflow that
        // never opted into autoflow has no such flag to consult and must keep
        // firing, or this would silently break every ordinary cron job.
        if wf.autoflow.as_ref().is_some_and(|a| !a.enabled) {
            info!(
                workflow = %stem,
                "skipping: autoflow is disabled (autoflow.enabled: false)"
            );
            continue;
        }
        let Some(schedule) = wf.trigger.cron.clone() else {
            // The schema validator should have caught this, but be
            // defensive — a malformed cron-trigger workflow is just
            // skipped, not fatal to the whole tick.
            warn!(path = %p.display(), "trigger.on=cron without cron: field; skipping");
            continue;
        };
        into.insert(
            stem.to_string(),
            CronWorkflow {
                name: stem.to_string(),
                schedule,
            },
        );
    }
}

/// A workflow with `trigger.on: event`.
pub struct EventWorkflow {
    /// The file stem.
    pub name: String,
    pub event: String,
    pub filter: Option<String>,
}

/// [`collect_cron_workflows`] for `trigger.on: event`.
pub fn collect_event_workflows(global: &Path, project_root: Option<&Path>) -> Vec<EventWorkflow> {
    let mut by_name: BTreeMap<String, EventWorkflow> = BTreeMap::new();
    push_event(&global.join("workflows"), &mut by_name);
    if let Some(p) = project_root {
        push_event(&p.join(".rupu/workflows"), &mut by_name);
    }
    by_name.into_values().collect()
}

pub fn push_event(dir: &Path, into: &mut BTreeMap<String, EventWorkflow>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = match std::fs::read_to_string(&p) {
            Ok(b) => b,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping unreadable workflow");
                continue;
            }
        };
        let wf = match Workflow::parse(&body) {
            Ok(w) => w,
            Err(e) => {
                warn!(path = %p.display(), error = %e, "skipping malformed workflow");
                continue;
            }
        };
        if wf.trigger.on != TriggerKind::Event {
            continue;
        }
        let Some(event) = wf.trigger.event.clone() else {
            warn!(path = %p.display(), "trigger.on=event without event: field; skipping");
            continue;
        };
        into.insert(
            stem.to_string(),
            EventWorkflow {
                name: stem.to_string(),
                event,
                filter: wf.trigger.filter.clone(),
            },
        );
    }
}

pub fn format_poll_source_entry(source: &PollSourceEntry) -> String {
    match source.poll_interval() {
        Some(interval) => format!("{}@{interval}", source.source()),
        None => source.source().to_string(),
    }
}

/// `<vendor>/<slug>.cursor` file naming. **Ignores `account`** (Task 6
/// review item 4) — two `poll_sources` entries with the same
/// `tracker`/`project` (or `platform`/`owner`/`repo`) but different
/// `account` overrides collide on the same cursor file and silently
/// share state. Not fixed here: today's data model has no per-account
/// tracker multi-account case in production (`tracker_event_connectors`
/// holds at most one entry per tracker — see its field doc in
/// `rupu-scm/src/registry.rs`), so nobody can actually configure two
/// `account`-distinguished poll sources for the same project yet. Left
/// as a landmine for whoever adds that, not a live bug today.
pub fn source_slug(source: &EventSourceRef) -> String {
    let text = match source {
        EventSourceRef::Repo { repo } => format!("repo-{}-{}", repo.owner, repo.repo),
        EventSourceRef::TrackerProject { project, .. } => format!("project-{project}"),
    };
    text.chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '-',
        })
        .collect()
}

/// `<global>/cron-state/event-cursors/<vendor>/<source>.cursor`.
pub fn cursor_path(root: &Path, source: &EventSourceRef) -> PathBuf {
    root.join(source.vendor())
        .join(format!("{}.cursor", source_slug(source)))
}

/// The cursor a source's last poll left, trimmed.
pub fn read_cursor(path: &Path) -> std::io::Result<String> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

/// One `cron list` row.
#[derive(Debug, Clone, Serialize)]
pub struct CronListRow {
    pub name: String,
    pub schedule: String,
    /// `None` (with `in_seconds`) when the schedule can't be parsed or has
    /// no future occurrence.
    pub next_utc: Option<String>,
    pub in_seconds: Option<i64>,
}

/// `rupu cron list`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct CronListReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<CronListRow>,
}

/// When each of `workflows` fires next after `now`.
pub fn list_report(workflows: &[CronWorkflow], now: DateTime<Utc>) -> CronListReport {
    let rows = workflows
        .iter()
        .map(|workflow| {
            let next = crate::cron_schedule::parse_schedule(&workflow.schedule)
                .ok()
                .and_then(|schedule| crate::cron_schedule::next_fire_after(&schedule, now));
            CronListRow {
                name: workflow.name.clone(),
                schedule: workflow.schedule.clone(),
                next_utc: next.map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string()),
                in_seconds: next.map(|time| (time - now).num_seconds()),
            }
        })
        .collect();
    CronListReport {
        kind: "cron_list",
        version: 1,
        rows,
    }
}

/// One `cron events` row.
#[derive(Debug, Clone, Serialize)]
pub struct CronEventsRow {
    pub name: String,
    pub event: String,
    pub sources: Vec<String>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CronEventsSummary {
    pub poll_sources: Vec<String>,
}

/// `rupu cron events`' JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct CronEventsReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<CronEventsRow>,
    pub summary: CronEventsSummary,
}

/// Each event-triggered workflow with `cfg`'s poll sources and the latest
/// cursor among them, read under `cursors_root`.
pub fn events_report(
    cfg: &rupu_config::Config,
    workflows: &[EventWorkflow],
    cursors_root: &Path,
) -> CronEventsReport {
    let poll_sources: Vec<String> = cfg
        .triggers
        .poll_sources
        .iter()
        .map(format_poll_source_entry)
        .collect();
    let rows = workflows
        .iter()
        .map(|wf| {
            let cursor = cfg
                .triggers
                .poll_sources
                .iter()
                .filter_map(|s| s.source().parse::<EventSourceRef>().ok())
                .find_map(|source| read_cursor(&cursor_path(cursors_root, &source)).ok());
            CronEventsRow {
                name: wf.name.clone(),
                event: wf.event.clone(),
                sources: poll_sources.clone(),
                cursor,
            }
        })
        .collect();
    CronEventsReport {
        kind: "cron_events",
        version: 1,
        summary: CronEventsSummary { poll_sources },
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_wf_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        for (name, body) in files {
            std::fs::write(d.path().join(name), body).expect("write workflow");
        }
        d
    }

    const CRON_DISABLED: &str = r#"
name: nightly-health
autoflow:
  enabled: false
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: check
    agent: investigator
    prompt: "check"
"#;

    const CRON_ENABLED: &str = r#"
name: nightly-enabled
autoflow:
  enabled: true
  entity: issue
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: check
    agent: investigator
    prompt: "check"
"#;

    const CRON_PLAIN: &str = r#"
name: plain-cron
trigger:
  on: cron
  cron: "0 6 * * *"
steps:
  - id: check
    agent: investigator
    prompt: "check"
"#;

    #[test]
    fn cron_tick_skips_a_workflow_disabled_in_the_cp_ui() {
        let d = tmp_wf_dir(&[("nightly-health.yaml", CRON_DISABLED)]);
        let mut got = std::collections::BTreeMap::new();
        push_cron(d.path(), &mut got);
        assert!(
            !got.contains_key("nightly-health"),
            "a workflow with `autoflow.enabled: false` must not be collected \
             for the cron tick — the CP disable toggle writes exactly that flag"
        );
    }

    #[test]
    fn cron_tick_still_fires_a_plain_cron_workflow_with_no_autoflow_block() {
        // Guard against over-correcting: a workflow that never opted into
        // autoflow at all has no `enabled` flag to consult and must keep
        // firing. Disabling would silently break every ordinary cron job.
        let d = tmp_wf_dir(&[("plain-cron.yaml", CRON_PLAIN)]);
        let mut got = std::collections::BTreeMap::new();
        push_cron(d.path(), &mut got);
        assert!(
            got.contains_key("plain-cron"),
            "a cron workflow with no `autoflow:` block must still fire"
        );
    }

    #[test]
    fn cron_tick_still_fires_an_enabled_autoflow() {
        let d = tmp_wf_dir(&[("nightly-enabled.yaml", CRON_ENABLED)]);
        let mut got = std::collections::BTreeMap::new();
        push_cron(d.path(), &mut got);
        assert!(
            got.contains_key("nightly-enabled"),
            "`autoflow.enabled: true` must still fire on schedule"
        );
    }

    #[test]
    fn a_second_tick_lock_is_refused_until_the_first_drops() {
        let tmp = tempfile::tempdir().unwrap();
        let first = TickLock::try_acquire(tmp.path()).unwrap().expect("free");
        assert!(TickLock::try_acquire(tmp.path()).unwrap().is_none());
        drop(first);
        assert!(TickLock::try_acquire(tmp.path()).unwrap().is_some());
    }
}
