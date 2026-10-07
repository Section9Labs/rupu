//! The orphan reaper: [`reap_orphaned_agentiflows`] finalizes the record of an
//! agentiflow whose coordinator died without finishing it, and group-kills the
//! detached units that coordinator left running.
//!
//! A coordinator can vanish mid-run (SIGKILL, a crash, the machine going down)
//! and take its own bookkeeping with it: `agentiflow.json` stays `running`
//! forever, `events.jsonl` never gets a terminal event, and the units it
//! launched as their own process groups keep burning tokens with nobody
//! steering them. This is the one place that cleans all of that up. `stop
//! --now`, `agentiflow serve` and `cp serve` all call it, so a reaped orphan
//! reads the same wherever it was found.
//!
//! # What gets reaped
//!
//! A record is reaped only when ALL of these hold; each is a guard against
//! failing a healthy run, or signalling a stranger:
//!
//! - `status == "running"` (a finished record is never touched);
//! - `runner_pid == Some(p)` (a `None` is **owner unknown**, the window between
//!   creating the run dir and stamping the pid, or a record from before pids
//!   were recorded; it is skipped, never read as dead);
//! - `p` is not running ([`pid_is_running`]).
//!
//! A unit's process group is signalled only when its last mirrored status is
//! non-terminal, it recorded a `pgid`, and that pgid's leader is still running
//! (a fully dead group's number could have been recycled onto an unrelated
//! process, so a group whose leader is gone is left alone).
//!
//! # What it does
//!
//! For a reaped record: one SIGTERM pass over every live unit group, one
//! bounded grace (polled, so it ends as soon as every group is gone), one
//! SIGKILL for whatever outlived it. Then the record is rewritten `failed` with
//! `stop_reason = "orphaned: coordinator pid <p> not running"`, `ended_at`, and
//! no `runner_pid`, and a terminal `run_stopped` event is appended to
//! `events.jsonl` (the same writer and shape `run_agentiflow` uses) so a live
//! events view stops spinning.
//!
//! # Tolerance
//!
//! The reaper is a sweep that runs unattended: it never panics on a bad run dir
//! and never aborts over one. An unreadable record, a torn `unit.json`, or an
//! incomplete `units/` (a coordinator killed mid-dispatch) is skipped with a
//! `tracing::warn!` and the rest of the sweep goes on. A signal that fails is
//! logged, never fatal.
//!
//! The function blocks (file IO, and up to [`TERM_GRACE`] per reaped run when a
//! unit group needs winding down). Call it from `spawn_blocking` in async code.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::proc::{kill_group, pid_is_running, terminate_group};
use crate::run::{agentiflow_dir, AgentiflowRecord, EventLog};
use crate::supervisor::units_on_disk;

/// How long a SIGTERMed unit group gets to exit before SIGKILL. Matches the
/// grace `SubprocessUnitLauncher`'s `Drop` gives a unit on the graceful path.
const TERM_GRACE: Duration = Duration::from_millis(1500);
/// How often the grace re-checks whether the signalled groups are gone.
const GRACE_POLL: Duration = Duration::from_millis(25);

/// What one [`reap_orphaned_agentiflows`] sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReapSummary {
    /// Run directories looked at (`<global>/agentiflows/af_*`), reaped or not.
    pub scanned: usize,
    /// The ids (run directory names) of the runs it finalized as orphaned.
    pub reaped: Vec<String>,
}

/// Sweep `<global>/agentiflows/af_*` and finalize every orphaned run (see the
/// module docs for what that is and how it is decided). `now` stamps
/// `ended_at` and the terminal event.
///
/// Infallible by design: whatever cannot be read or written is logged and
/// skipped. Blocking; see the module docs.
pub fn reap_orphaned_agentiflows(global: &Path, now: DateTime<Utc>) -> ReapSummary {
    let mut summary = ReapSummary::default();
    for (id, run_dir) in run_dirs(global) {
        summary.scanned += 1;
        if reap_one(&run_dir, now) {
            summary.reaped.push(id);
        }
    }
    summary
}

/// Every `af_*` run directory under `<global>/agentiflows`, oldest first (ids
/// are ULIDs). A symlink is not followed; a stray file or a dir that is not a
/// run is not one.
fn run_dirs(global: &Path) -> Vec<(String, PathBuf)> {
    let root = agentiflow_dir(global);
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(dir = %root.display(), error = %e, "could not list agentiflow runs; skipping the sweep");
            return Vec::new();
        }
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let id = entry.file_name().into_string().ok()?;
            id.starts_with("af_").then(|| (id, entry.path()))
        })
        .collect();
    out.sort();
    out
}

/// The coordinator pid of an orphan: a `running` record whose recorded
/// `runner_pid` is not running. `None` for everything else, which includes the
/// owner-unknown record (`runner_pid: None`).
fn dead_coordinator(record: &AgentiflowRecord) -> Option<u32> {
    if record.status != "running" {
        return None;
    }
    let pid = record.runner_pid?;
    (!pid_is_running(pid)).then_some(pid)
}

/// Read a run's record, quietly for a dir with none yet (a run being created),
/// with a warning for anything else that is wrong with it.
fn read_record(run_dir: &Path) -> Option<AgentiflowRecord> {
    match AgentiflowRecord::read(run_dir) {
        Ok(record) => Some(record),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(run = %run_dir.display(), error = %e, "unreadable agentiflow.json; skipping");
            None
        }
    }
}

/// Reap one run dir if it is an orphan. Whether it was finalized.
fn reap_one(run_dir: &Path, now: DateTime<Utc>) -> bool {
    let Some(record) = read_record(run_dir) else {
        return false;
    };
    let Some(pid) = dead_coordinator(&record) else {
        return false;
    };

    wind_down_units(run_dir);

    // The grace can be long enough for something else to have settled the run (a
    // second reaper, or a resume that stamped a new owner): re-read and judge
    // the CURRENT record, and finalize that one, so nothing it gained meanwhile
    // (spend, rounds) is lost.
    let Some(mut record) = read_record(run_dir) else {
        return false;
    };
    if dead_coordinator(&record) != Some(pid) {
        return false;
    }

    let reason = format!("orphaned: coordinator pid {pid} not running");
    record.status = "failed".into();
    record.stop_reason = Some(reason.clone());
    record.ended_at = Some(now);
    record.runner_pid = None;
    if let Err(e) = record.write(run_dir) {
        tracing::warn!(run = %run_dir.display(), error = %e, "could not finalize an orphaned agentiflow; it stays `running` until the next sweep");
        return false;
    }

    // After the record, like the record is the durable fact; a failed append is
    // the writer's own best-effort warning.
    EventLog::new(run_dir).emit(
        now,
        "run_stopped",
        json!({
            "stop_reason": reason,
            "detail": reason,
            "rounds": record.rounds,
            "goals": record.goals,
            "summary": format!("Stopped: {reason} after {} round(s).", record.rounds),
            "spent_usd": record.spent_usd.unwrap_or(0.0),
            "spent_tokens": record.spent_tokens,
        }),
    );
    tracing::info!(id = %record.id, pid, "reaped an orphaned agentiflow");
    true
}

/// SIGTERM every live, non-terminal unit group the run recorded, give them
/// [`TERM_GRACE`], then SIGKILL the ones still standing. One pass, bounded:
/// a sweep must not hang on a unit that will not die.
fn wind_down_units(run_dir: &Path) {
    let mut signalled: Vec<u32> = Vec::new();
    for unit in units_on_disk(run_dir) {
        if unit.status.is_terminal() {
            continue;
        }
        let Some(pgid) = unit.pgid else { continue };
        // Only a group whose leader is still there: a fully dead group's number
        // may have been recycled onto something that is not ours.
        if !pid_is_running(pgid) {
            continue;
        }
        if terminate_group(pgid) {
            signalled.push(pgid);
        } else {
            // Refused (our own group, an impossible id) or the signal failed.
            tracing::warn!(unit = %unit.unit_id, pgid, "could not SIGTERM an orphaned unit's process group");
        }
    }
    signalled.sort_unstable();
    signalled.dedup();
    if signalled.is_empty() {
        return;
    }

    let deadline = Instant::now() + TERM_GRACE;
    while Instant::now() < deadline && signalled.iter().any(|&g| pid_is_running(g)) {
        std::thread::sleep(GRACE_POLL);
    }
    for pgid in signalled {
        if pid_is_running(pgid) {
            tracing::warn!(
                pgid,
                "an orphaned unit outlived its SIGTERM grace; sending SIGKILL"
            );
            if !kill_group(pgid) {
                tracing::warn!(pgid, "could not SIGKILL an orphaned unit's process group");
            }
        }
    }
}
