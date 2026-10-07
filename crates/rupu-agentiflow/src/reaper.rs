//! The orphan reaper: [`reap_orphaned_agentiflows`] finalizes the record of an
//! agentiflow whose coordinator died without finishing it, and group-kills the
//! detached units that coordinator left running. [`hard_stop`] is the operator's
//! counterpart for a coordinator that is still alive.
//!
//! A coordinator can vanish mid-run (SIGKILL, a crash, the machine going down)
//! and take its own bookkeeping with it: `agentiflow.json` stays `running`
//! forever, `events.jsonl` never gets a terminal event, and the units it
//! launched as their own process groups keep burning tokens with nobody
//! steering them. The sweep cleans all of that up; `agentiflow serve` and
//! `cp serve` call it. A hard stop (`rupu agentiflow stop --now`) ends up in
//! the same place from the other side: the coordinator dies on SIGTERM without
//! running any cleanup (the CLI's SIGTERM handler exits the process), so the
//! stop itself must close out the record and the event log. Both therefore
//! share [`finalize_failed`] (the terminal record + `run_stopped` event) and the
//! unit wind-down (SIGTERM, a bounded grace, SIGKILL), so a run closed by force
//! reads the same whoever closed it.
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
//! [`hard_stop`] blocks the same way.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::proc::{kill_group, pid_is_running, terminate_group, terminate_pid};
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
    if let Err(e) = finalize_failed(run_dir, &mut record, &reason, &reason, now) {
        tracing::warn!(run = %run_dir.display(), error = %e, "could not finalize an orphaned agentiflow; it stays `running` until the next sweep");
        return false;
    }
    tracing::info!(id = %record.id, pid, "reaped an orphaned agentiflow");
    true
}

/// Close a run out as `failed`: rewrite its record (`status = "failed"`,
/// `stop_reason`, `ended_at = now`, no `runner_pid`) and THEN append the
/// terminal `run_stopped` event, so a live events view stops spinning.
///
/// The one finalizer for a run that is closed by force rather than by its own
/// coordinator: the orphan reaper (`stop_reason` and `detail` both
/// `orphaned: coordinator pid <p> not running`) and the operator's
/// [`hard_stop`] (`operator_stop:now` / `operator hard stop (--now)`). Without
/// the event a `failed` record would be final (the reaper skips it) yet its log
/// would end mid-run. `stop_reason` is the stable code the record persists;
/// `detail` is the human text the event and its summary carry.
///
/// The record is the durable fact, so it is written first and a failure to
/// write it is returned with no event appended. A failed event append is the
/// writer's own best-effort warning, not an error. `record` is mutated in
/// place; pass the CURRENT on-disk record (re-read just before), so nothing it
/// gained meanwhile (spend, rounds) is lost.
pub fn finalize_failed(
    run_dir: &Path,
    record: &mut AgentiflowRecord,
    stop_reason: &str,
    detail: &str,
    now: DateTime<Utc>,
) -> std::io::Result<()> {
    record.status = "failed".into();
    record.stop_reason = Some(stop_reason.to_string());
    record.ended_at = Some(now);
    record.runner_pid = None;
    record.write(run_dir)?;

    EventLog::new(run_dir).emit(
        now,
        "run_stopped",
        json!({
            "stop_reason": stop_reason,
            "detail": detail,
            "rounds": record.rounds,
            "goals": record.goals,
            "summary": format!("Stopped: {detail} after {} round(s).", record.rounds),
            "spent_usd": record.spent_usd.unwrap_or(0.0),
            "spent_tokens": record.spent_tokens,
        }),
    );
    Ok(())
}

/// What [`hard_stop`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardStopOutcome {
    /// The run was already past `running` (the status it is in), either when
    /// the stop began or by the time its processes had been signalled: it
    /// finished on its own in between. Nothing was written.
    AlreadyTerminal(String),
    /// The coordinator and the units were signalled and the run is closed out
    /// `failed` (`operator_stop:now`).
    Stopped,
}

/// The operator's hard stop of ONE run (`rupu agentiflow stop --now`): act on a
/// live coordinator now, without waiting for its round.
///
/// 1. Read the record; a run that is not `running` is
///    [`AlreadyTerminal`](HardStopOutcome::AlreadyTerminal), untouched, nothing
///    signalled.
/// 2. SIGTERM the coordinator (`runner_pid`) if it is running. Its SIGTERM
///    handler exits the process without running any cleanup, so nothing below
///    is left to it.
/// 3. Wind down the run's units: SIGTERM each live unit group, a bounded
///    grace, SIGKILL whatever outlived it (the reaper's own wind-down). An
///    operator who asks for a hard stop gets the escalation.
/// 4. RE-READ the record and, only if it is STILL `running`, [`finalize_failed`]
///    it (`operator_stop:now`). The grace is long enough for the run to have
///    finished on its own meanwhile; the stale snapshot from step 1 must never
///    overwrite that. This is the re-check `reap_one` makes too.
///
/// # Known residual
///
/// A late coordinator round-write can revert the record from `failed` back to
/// `running` after step 4. The orphan reaper then re-closes it as
/// `failed`/`orphaned` with a second `run_stopped` line: self-healing and
/// harmless. `hard_stop` deliberately does not wait on the coordinator pid to
/// close that window, because a zombie parent makes such a wait flaky.
///
/// Blocking (file IO and up to [`TERM_GRACE`] of grace): call it from
/// `spawn_blocking` in async code. A record that cannot be read, or the final
/// write failing, is an error; every signal failure is only logged.
pub fn hard_stop(run_dir: &Path, now: DateTime<Utc>) -> std::io::Result<HardStopOutcome> {
    let record = AgentiflowRecord::read(run_dir)?;
    if record.status != "running" {
        return Ok(HardStopOutcome::AlreadyTerminal(record.status));
    }

    if let Some(pid) = record.runner_pid {
        if pid_is_running(pid) && !terminate_pid(pid) {
            tracing::warn!(run = %run_dir.display(), pid, "could not SIGTERM the coordinator");
        }
    }
    wind_down_units(run_dir);

    let mut record = AgentiflowRecord::read(run_dir)?;
    if record.status != "running" {
        return Ok(HardStopOutcome::AlreadyTerminal(record.status));
    }
    finalize_failed(
        run_dir,
        &mut record,
        "operator_stop:now",
        "operator hard stop (--now)",
        now,
    )?;
    tracing::info!(id = %record.id, "hard-stopped an agentiflow");
    Ok(HardStopOutcome::Stopped)
}

/// SIGTERM every live, non-terminal unit group the run recorded, give them
/// [`TERM_GRACE`], then SIGKILL the ones still standing. One pass, bounded:
/// neither a sweep nor a hard stop may hang on a unit that will not die.
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
            tracing::warn!(unit = %unit.unit_id, pgid, "could not SIGTERM a unit's process group");
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
            tracing::warn!(pgid, "a unit outlived its SIGTERM grace; sending SIGKILL");
            if !kill_group(pgid) {
                tracing::warn!(pgid, "could not SIGKILL a unit's process group");
            }
        }
    }
}
