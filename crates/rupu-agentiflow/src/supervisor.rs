//! `FleetSupervisor`: tracks the units a lead dispatched, polls them through the
//! [`UnitLauncher`] port, joins them with a timeout, and winds them down when
//! the envelope stops. It writes the per-unit record `units/<id>/unit.json`
//! under the run dir so an operator (or a later reader) can see what was fired.
//!
//! The in-memory map is the source of truth; `unit.json` is a best-effort
//! mirror, so a write failure is logged, never fatal. Depth and pool limits are
//! the dispatch TOOL's job (Task 6), not the supervisor's.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rupu_fleet::Mailbox;

use crate::unit::{
    Spawned, UnitError, UnitId, UnitKind, UnitLauncher, UnitOutcome, UnitSpec, UnitStatus,
};

/// How often `join` re-polls a unit that has not finished.
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
struct UnitRec {
    /// The agent (or, for a workflow unit, the workflow) the unit runs.
    agent: String,
    kind: UnitKind,
    participant: String,
    started_at: DateTime<Utc>,
    /// The unit's process-group id, as its launcher reported it at spawn
    /// (`None` when the launcher has no process). Persisted to `unit.json` so
    /// a process that outlives this supervisor can signal the group.
    pgid: Option<u32>,
    /// The last status observed from the launcher.
    status: UnitStatus,
}

pub struct FleetSupervisor {
    launcher: Arc<dyn UnitLauncher>,
    run_dir: PathBuf,
    units: Mutex<HashMap<String, UnitRec>>,
}

impl FleetSupervisor {
    pub fn new(launcher: Arc<dyn UnitLauncher>, run_dir: PathBuf) -> Self {
        Self {
            launcher,
            run_dir,
            units: Mutex::new(HashMap::new()),
        }
    }

    /// Fire a unit and start tracking it. Returns the unit's id (its run id)
    /// and writes `units/<id>/unit.json`.
    pub fn dispatch(&self, spec: UnitSpec) -> Result<String, UnitError> {
        // Register the unit's broadcast cursor at the CURRENT tip of the
        // broadcast log before it exists. `read_broadcast`'s first call for a
        // participant sets its cursor to the tip and returns nothing, so
        // discarding the result is exactly the registration. Without it the
        // cursor would only be created at the unit's first turn, and anything
        // broadcast between spawn and that first turn would be silently
        // swallowed as "backlog".
        if let Err(e) = Mailbox::new(self.run_dir.clone()).read_broadcast(&spec.participant) {
            tracing::warn!(
                participant = %spec.participant,
                error = %e,
                "could not register the unit's broadcast cursor at dispatch"
            );
        }

        let Spawned { id, pgid } = self.launcher.spawn(&spec, &self.run_dir)?;
        let rec = UnitRec {
            agent: spec.agent,
            kind: spec.kind,
            participant: spec.participant,
            started_at: Utc::now(),
            pgid,
            status: UnitStatus::Pending,
        };
        self.write_unit_json(&id.0, &rec);
        self.lock().insert(id.0.clone(), rec);
        Ok(id.0)
    }

    /// The unit's current status, polled from the launcher. A unit already
    /// seen terminal keeps its recorded status (it is not polled again); an id
    /// that was never dispatched here reads as `Failed`.
    pub fn status(&self, id: &str) -> UnitStatus {
        match self.lock().get(id) {
            None => return UnitStatus::Failed(UnitError::Unknown.to_string()),
            Some(rec) if rec.status.is_terminal() => return rec.status.clone(),
            Some(_) => {}
        }

        // Poll without holding the map lock: a launcher poll does file I/O.
        let polled = self.launcher.poll(&UnitId(id.to_string()), &self.run_dir);

        let changed = {
            let mut units = self.lock();
            match units.get_mut(id) {
                // Unreachable while units are never removed; stay total.
                None => return polled,
                // Another caller recorded the end first; its answer stands.
                Some(rec) if rec.status.is_terminal() => return rec.status.clone(),
                Some(rec) if rec.status == polled => None,
                Some(rec) => {
                    rec.status = polled.clone();
                    Some(rec.clone())
                }
            }
        };
        if let Some(rec) = changed {
            self.write_unit_json(id, &rec);
        }
        polled
    }

    /// Poll `id` until it is terminal or `timeout` elapses, returning the last
    /// status seen (non-terminal on timeout).
    ///
    /// The deadline is measured on the injected clock `now`; a real-time
    /// backstop of the same `timeout` also applies, so a frozen test clock
    /// cannot make a join spin forever. This blocks the calling thread (a short
    /// real sleep between polls) — call it from a blocking context.
    pub fn join(&self, id: &str, timeout: Duration, now: &dyn Fn() -> DateTime<Utc>) -> UnitStatus {
        let real_start = Instant::now();
        let deadline = chrono::Duration::from_std(timeout)
            .ok()
            .and_then(|d| now().checked_add_signed(d))
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        loop {
            let st = self.status(id);
            if st.is_terminal() {
                return st;
            }
            let real_left = timeout.saturating_sub(real_start.elapsed());
            // `to_std` errs on a negative remainder: the deadline has passed.
            let clock_left = (deadline - now()).to_std().unwrap_or(Duration::ZERO);
            let left = real_left.min(clock_left);
            if left.is_zero() {
                return st;
            }
            std::thread::sleep(left.min(JOIN_POLL_INTERVAL));
        }
    }

    /// SIGTERM every unit that has not finished (the envelope's wind-down).
    /// Each candidate is re-polled first, so a unit that already ended is not
    /// signalled (its pid may have been recycled). Best-effort and idempotent.
    pub fn terminate_all(&self) {
        for id in self.live_ids() {
            self.launcher.terminate(&UnitId(id));
        }
    }

    /// Ids of the units that have not finished, oldest first (ids are ULIDs).
    /// Each candidate is re-polled, so this is current, not merely the last
    /// status someone happened to read.
    pub fn live_ids(&self) -> Vec<String> {
        let mut candidates: Vec<String> = self
            .lock()
            .iter()
            .filter(|(_, rec)| !rec.status.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        candidates.sort();
        candidates.retain(|id| !self.status(id).is_terminal());
        candidates
    }

    /// The run directories of every unit this supervisor has launched, oldest
    /// first: `<global>/runs/<unit_id>`. A unit is a standalone `rupu run`
    /// under `global` (`RUPU_HOME`), so its ledgers (`usage.jsonl`) live there.
    ///
    /// The supervisor records each id the moment `dispatch` gets it back from
    /// the launcher, whichever launcher that is, so the answer covers finished
    /// units too (their spend still counts) and a dispatch that failed to spawn
    /// adds nothing. A directory may not exist yet (the unit has not written
    /// anything); readers treat a missing file as empty.
    pub fn launched_unit_run_dirs(&self, global: &Path) -> Vec<PathBuf> {
        let mut ids: Vec<String> = self
            .lock()
            .keys()
            .filter(|id| is_safe_unit_id(id))
            .cloned()
            .collect();
        // Ids are ULIDs, so lexical order is launch order.
        ids.sort();
        ids.into_iter()
            .map(|id| global.join("runs").join(id))
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, UnitRec>> {
        self.units.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Best-effort mirror of `rec` to `units/<id>/unit.json` (tmp + rename).
    fn write_unit_json(&self, id: &str, rec: &UnitRec) {
        // The id is a path component; only ever write for a plain run id.
        if !is_safe_unit_id(id) {
            tracing::warn!(
                unit = id,
                "refusing to write unit.json for an unsafe unit id"
            );
            return;
        }
        let dir = self.run_dir.join("units").join(id);
        let body = serde_json::json!({
            "run_id": id,
            "agent": rec.agent,
            "kind": rec.kind.as_str(),
            "participant": rec.participant,
            "started_at": rec.started_at.to_rfc3339(),
            "pgid": rec.pgid,
            "status": status_json(&rec.status),
        });
        if let Err(e) = write_json_atomic(&dir, &body) {
            tracing::warn!(unit = id, error = %e, "could not write unit.json");
        }
    }
}

/// Whether `id` is a plain run id, safe to use as a single path component.
fn is_safe_unit_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn status_json(status: &UnitStatus) -> serde_json::Value {
    match status {
        UnitStatus::Pending => serde_json::json!({ "state": "pending" }),
        UnitStatus::Running => serde_json::json!({ "state": "running" }),
        UnitStatus::Done(o) => serde_json::json!({
            "state": "done",
            "success": o.success,
            "output": o.output,
        }),
        UnitStatus::Failed(why) => serde_json::json!({ "state": "failed", "error": why }),
    }
}

/// One unit as `units/<id>/unit.json` records it, read back by
/// [`units_on_disk`]: the facts a process that did not launch the unit (the
/// orphan reaper, `agentiflow stop`) needs to wind it down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitOnDisk {
    /// The unit's run id (the `units/<id>` directory name).
    pub unit_id: String,
    /// The process group the unit leads. `None` for a `unit.json` written
    /// before the pgid was recorded, or by a launcher with no process.
    pub pgid: Option<u32>,
    pub kind: UnitKind,
    /// The last status the supervisor mirrored to disk.
    pub status: UnitStatus,
}

/// What `unit.json` holds, parsed tolerantly: only what [`UnitOnDisk`] needs,
/// with every key but the status optional so an older file still reads.
#[derive(serde::Deserialize)]
struct UnitJson {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    pgid: Option<u32>,
    status: StatusJson,
}

/// The `status` object [`status_json`] writes.
#[derive(serde::Deserialize)]
struct StatusJson {
    state: String,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl StatusJson {
    /// `None` for a state this build does not know (a bad file, not a guess).
    fn into_status(self) -> Option<UnitStatus> {
        Some(match self.state.as_str() {
            "pending" => UnitStatus::Pending,
            "running" => UnitStatus::Running,
            "done" => UnitStatus::Done(UnitOutcome {
                output: self.output.unwrap_or_default(),
                success: self.success.unwrap_or(false),
            }),
            "failed" => UnitStatus::Failed(self.error.unwrap_or_default()),
            _ => return None,
        })
    }
}

fn parse_unit_json(path: &Path, unit_id: &str) -> Option<UnitOnDisk> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        // A unit dir whose unit.json was never written is not an error worth a
        // log line (the write is tmp + rename, so it is never half there).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(unit = unit_id, error = %e, "could not read unit.json; skipping");
            return None;
        }
    };
    let parsed: UnitJson = match serde_json::from_slice(&raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(unit = unit_id, error = %e, "malformed unit.json; skipping");
            return None;
        }
    };
    let kind = match parsed.kind.as_deref() {
        None | Some("agent") => UnitKind::Agent,
        Some("workflow") => UnitKind::Workflow,
        Some(other) => {
            tracing::warn!(
                unit = unit_id,
                kind = other,
                "unknown unit kind in unit.json; skipping"
            );
            return None;
        }
    };
    let Some(status) = parsed.status.into_status() else {
        tracing::warn!(unit = unit_id, "unknown unit state in unit.json; skipping");
        return None;
    };
    Some(UnitOnDisk {
        unit_id: unit_id.to_string(),
        pgid: parsed.pgid,
        kind,
        status,
    })
}

/// Every unit `run_dir` records, oldest first (ids are ULIDs), read from
/// `units/*/unit.json`.
///
/// This is what lets a process other than the supervisor find a run's units:
/// the supervisor's in-memory map dies with its process, `unit.json` does not.
/// Reading is tolerant by design (a reaper sweep must survive any run dir): a
/// missing `units/`, a unit dir without a `unit.json`, a torn or unparseable
/// file, or an unknown kind / state is skipped (the unparseable ones with a
/// warning), never a panic or an error.
pub fn units_on_disk(run_dir: &Path) -> Vec<UnitOnDisk> {
    let units_dir = run_dir.join("units");
    let entries = match std::fs::read_dir(&units_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(dir = %units_dir.display(), error = %e, "could not list units; skipping");
            return Vec::new();
        }
    };
    let mut out: Vec<UnitOnDisk> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            // A stray file where a unit dir is expected has no unit.json under it.
            let unit_id = entry.file_name().into_string().ok()?;
            // `write_unit_json` only ever writes plain run ids; the id is handed
            // on as a path component, so anything else is not ours.
            if !is_safe_unit_id(&unit_id) {
                tracing::warn!(unit = %unit_id, "unit dir is not a plain run id; skipping");
                return None;
            }
            parse_unit_json(&entry.path().join("unit.json"), &unit_id)
        })
        .collect();
    out.sort_by(|a, b| a.unit_id.cmp(&b.unit_id));
    out
}

fn write_json_atomic(dir: &Path, body: &serde_json::Value) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("unit.json.tmp");
    let bytes = serde_json::to_vec_pretty(body).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dir.join("unit.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{MockUnitLauncher, UnitOutcome};
    use rupu_fleet::FleetMessage;

    fn spec(participant: &str) -> UnitSpec {
        UnitSpec {
            agent: "recon".into(),
            prompt: "scan".into(),
            engagement: vec![],
            participant: participant.into(),
            kind: UnitKind::Agent,
            inputs: vec![],
            workflow_file: None,
        }
    }

    fn msg(body: &str) -> FleetMessage {
        FleetMessage {
            from: "lead".into(),
            ts: "2026-10-05T00:00:00Z".into(),
            body: body.into(),
        }
    }

    fn read_unit_json(dir: &Path, id: &str) -> serde_json::Value {
        let raw = std::fs::read_to_string(dir.join("units").join(id).join("unit.json")).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn supervisor_dispatches_tracks_and_joins_via_the_mock() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![
            UnitStatus::Running,
            UnitStatus::Done(UnitOutcome {
                output: "found it".into(),
                success: true,
            }),
        ]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup
            .dispatch(UnitSpec {
                agent: "recon".into(),
                prompt: "scan".into(),
                engagement: vec![],
                participant: "recon#1".into(),
                kind: UnitKind::Agent,
                inputs: vec![],
                workflow_file: None,
            })
            .unwrap();
        // units/<id>/unit.json written
        assert!(dir
            .path()
            .join("units")
            .join(&id)
            .join("unit.json")
            .is_file());
        // join polls to terminal
        let out = sup.join(&id, Duration::from_secs(5), &|| Utc::now());
        assert!(matches!(out, UnitStatus::Done(o) if o.success && o.output.contains("found it")));
    }

    #[test]
    fn unit_json_records_the_dispatch_and_follows_status_changes() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![
            UnitStatus::Running,
            UnitStatus::Done(UnitOutcome {
                output: "found it".into(),
                success: true,
            }),
        ]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup.dispatch(spec("recon#1")).unwrap();

        let j = read_unit_json(dir.path(), &id);
        assert_eq!(j["run_id"], id.as_str());
        assert_eq!(j["agent"], "recon");
        assert_eq!(j["participant"], "recon#1");
        assert_eq!(j["status"]["state"], "pending");
        assert!(j["started_at"].as_str().is_some());

        assert_eq!(sup.status(&id), UnitStatus::Running);
        assert_eq!(
            read_unit_json(dir.path(), &id)["status"]["state"],
            "running"
        );

        let done = sup.join(&id, Duration::from_secs(5), &|| Utc::now());
        assert!(done.is_terminal());
        let j = read_unit_json(dir.path(), &id);
        assert_eq!(j["status"]["state"], "done");
        assert_eq!(j["status"]["success"], true);
        assert_eq!(j["status"]["output"], "found it");
        // The tmp file never lingers.
        assert!(!dir
            .path()
            .join("units")
            .join(&id)
            .join("unit.json.tmp")
            .exists());
    }

    #[test]
    fn unit_json_records_the_unit_kind() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());

        let agent = sup.dispatch(spec("recon#1")).unwrap();
        assert_eq!(read_unit_json(dir.path(), &agent)["kind"], "agent");

        let wf = sup
            .dispatch(UnitSpec {
                agent: "web-assess".into(),
                prompt: String::new(),
                engagement: vec![],
                participant: "web-assess#1".into(),
                kind: UnitKind::Workflow,
                inputs: vec![("target".into(), "x".into())],
                workflow_file: None,
            })
            .unwrap();
        let j = read_unit_json(dir.path(), &wf);
        assert_eq!(j["kind"], "workflow");
        assert_eq!(
            j["agent"], "web-assess",
            "the workflow name rides in `agent`"
        );
        // A status change rewrites the record and keeps the kind.
        assert_eq!(sup.status(&wf), UnitStatus::Running);
        assert_eq!(read_unit_json(dir.path(), &wf)["kind"], "workflow");
    }

    #[test]
    fn a_terminal_status_is_sticky_and_not_polled_again() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![
            UnitStatus::Failed("boom".into()),
            UnitStatus::Running,
        ]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup.dispatch(spec("recon#1")).unwrap();
        assert_eq!(sup.status(&id), UnitStatus::Failed("boom".into()));
        // The script's next entry would be Running; a finished unit never is.
        assert_eq!(sup.status(&id), UnitStatus::Failed("boom".into()));
        assert_eq!(read_unit_json(dir.path(), &id)["status"]["state"], "failed");
    }

    #[test]
    fn an_unknown_id_reads_as_failed_and_a_spawn_error_tracks_nothing() {
        struct Refusing;
        impl UnitLauncher for Refusing {
            fn spawn(&self, _: &UnitSpec, _: &Path) -> Result<Spawned, UnitError> {
                Err(UnitError::Spawn("no such agent".into()))
            }
            fn poll(&self, _: &UnitId, _: &Path) -> UnitStatus {
                UnitStatus::Pending
            }
            fn terminate(&self, _: &UnitId) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let sup = FleetSupervisor::new(Arc::new(Refusing), dir.path().to_path_buf());
        assert!(matches!(
            sup.status("run_never_dispatched"),
            UnitStatus::Failed(_)
        ));
        let err = sup.dispatch(spec("recon#1")).unwrap_err();
        assert!(matches!(err, UnitError::Spawn(m) if m == "no such agent"));
        assert!(sup.live_ids().is_empty());
        assert!(!dir.path().join("units").exists());
    }

    #[test]
    fn join_returns_the_live_status_when_the_timeout_elapses() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup.dispatch(spec("recon#1")).unwrap();

        let started = Instant::now();
        let out = sup.join(&id, Duration::from_millis(250), &|| Utc::now());
        assert_eq!(out, UnitStatus::Running);
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn join_ends_when_the_injected_clock_passes_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup.dispatch(spec("recon#1")).unwrap();

        // A clock that jumps a minute per reading: the 30s deadline is passed
        // on the first look, so join returns long before 30s of real time.
        let base = Utc::now();
        let ticks = std::sync::atomic::AtomicI64::new(0);
        let clock = move || {
            let n = ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            base + chrono::Duration::minutes(n)
        };
        let started = Instant::now();
        let out = sup.join(&id, Duration::from_secs(30), &clock);
        assert_eq!(out, UnitStatus::Running);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn join_with_a_frozen_clock_is_still_bounded_by_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup.dispatch(spec("recon#1")).unwrap();
        let frozen = Utc::now();
        let started = Instant::now();
        let out = sup.join(&id, Duration::from_millis(150), &|| frozen);
        assert_eq!(out, UnitStatus::Running);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn terminate_all_signals_only_units_that_have_not_finished() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher.clone(), dir.path().to_path_buf());
        let a = sup.dispatch(spec("a#1")).unwrap();
        let b = sup.dispatch(spec("b#1")).unwrap();
        let mut live = sup.live_ids();
        live.sort();
        let mut want = vec![a.clone(), b.clone()];
        want.sort();
        assert_eq!(live, want);

        sup.terminate_all();
        let mut got = launcher.terminated();
        got.sort();
        assert_eq!(got, want);

        // Finished units are neither live nor signalled.
        let dir2 = tempfile::tempdir().unwrap();
        let done = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Done(
            UnitOutcome {
                output: "x".into(),
                success: true,
            },
        )]));
        let sup2 = FleetSupervisor::new(done.clone(), dir2.path().to_path_buf());
        sup2.dispatch(spec("a#1")).unwrap();
        assert!(sup2.live_ids().is_empty());
        sup2.terminate_all();
        assert!(done.terminated().is_empty());
    }

    #[test]
    fn launched_unit_run_dirs_lists_every_dispatched_unit_under_global_runs() {
        let dir = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        // Every unit finishes at once; a finished unit's spend is still spend,
        // so it stays in the list.
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Done(
            UnitOutcome {
                output: "x".into(),
                success: true,
            },
        )]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        assert!(sup.launched_unit_run_dirs(global.path()).is_empty());

        let a = sup.dispatch(spec("a#1")).unwrap();
        assert!(sup.status(&a).is_terminal());
        let b = sup.dispatch(spec("b#1")).unwrap();
        assert!(sup.status(&b).is_terminal());

        let mut want = vec![
            global.path().join("runs").join(&a),
            global.path().join("runs").join(&b),
        ];
        want.sort();
        assert_eq!(sup.launched_unit_run_dirs(global.path()), want);
    }

    #[test]
    fn a_failed_spawn_adds_no_launched_unit_dir() {
        struct Refusing;
        impl UnitLauncher for Refusing {
            fn spawn(&self, _: &UnitSpec, _: &Path) -> Result<Spawned, UnitError> {
                Err(UnitError::Spawn("no such agent".into()))
            }
            fn poll(&self, _: &UnitId, _: &Path) -> UnitStatus {
                UnitStatus::Pending
            }
            fn terminate(&self, _: &UnitId) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let sup = FleetSupervisor::new(Arc::new(Refusing), dir.path().to_path_buf());
        sup.dispatch(spec("recon#1")).unwrap_err();
        assert!(sup.launched_unit_run_dirs(dir.path()).is_empty());
    }

    #[test]
    fn dispatch_registers_the_broadcast_cursor_at_the_current_tip() {
        // F-BCAST-REGISTER: a unit must see broadcasts sent between its spawn
        // and its first turn, but not the backlog from before it existed.
        let dir = tempfile::tempdir().unwrap();
        let mb = Mailbox::new(dir.path().to_path_buf());
        mb.broadcast_send(&msg("before dispatch"), 64).unwrap();

        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        sup.dispatch(spec("recon#1")).unwrap();

        mb.broadcast_send(&msg("after dispatch"), 64).unwrap();

        // The unit's FIRST read (as its first turn would make) delivers the
        // post-dispatch broadcast and not the pre-dispatch one. With no
        // registration at dispatch, this first read would only set the cursor
        // to the tip and return nothing, losing "after dispatch".
        let got = mb.read_broadcast("recon#1").unwrap();
        let bodies: Vec<&str> = got.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, vec!["after dispatch"]);
    }

    #[test]
    fn dispatch_persists_unit_pgid_and_units_on_disk_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let launcher =
            Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]).with_pid(4242));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let id = sup.dispatch(spec("recon#1")).unwrap();

        // The group id is in unit.json next to the rest of the dispatch...
        let j = read_unit_json(dir.path(), &id);
        assert_eq!(j["pgid"], 4242, "{j}");
        assert_eq!(j["agent"], "recon");

        // ...and the disk reader hands it back with the unit's kind + status.
        let units = units_on_disk(dir.path());
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(units[0].unit_id, id);
        assert_eq!(units[0].pgid, Some(4242));
        assert_eq!(units[0].kind, UnitKind::Agent);
        assert_eq!(units[0].status, UnitStatus::Pending);

        // A status change rewrites unit.json and keeps the pgid.
        assert_eq!(sup.status(&id), UnitStatus::Running);
        let units = units_on_disk(dir.path());
        assert_eq!(units[0].pgid, Some(4242));
        assert_eq!(units[0].status, UnitStatus::Running);
    }

    #[test]
    fn a_launcher_with_no_pgid_writes_none_and_units_on_disk_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        sup.dispatch(spec("recon#1")).unwrap();
        let units = units_on_disk(dir.path());
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].pgid, None);
    }

    #[test]
    fn units_on_disk_reads_a_unit_json_written_before_pgid_existed() {
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().join("units").join("run_OLD");
        std::fs::create_dir_all(&unit).unwrap();
        // Exactly the pre-4-3 shape: no `pgid` key at all.
        std::fs::write(
            unit.join("unit.json"),
            r#"{"run_id":"run_OLD","agent":"web-assess","kind":"workflow",
               "participant":"web-assess#1","started_at":"2026-10-06T00:00:00+00:00",
               "status":{"state":"done","success":true,"output":"ok"}}"#,
        )
        .unwrap();
        let units = units_on_disk(dir.path());
        assert_eq!(units.len(), 1, "{units:?}");
        assert_eq!(units[0].unit_id, "run_OLD");
        assert_eq!(units[0].pgid, None);
        assert_eq!(units[0].kind, UnitKind::Workflow);
        assert_eq!(
            units[0].status,
            UnitStatus::Done(UnitOutcome {
                output: "ok".into(),
                success: true
            })
        );
    }

    #[test]
    fn units_on_disk_skips_a_bad_unit_json_and_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        // No `units/` dir at all: nothing, not an error.
        assert!(units_on_disk(dir.path()).is_empty());

        let units = dir.path().join("units");
        let write = |name: &str, body: &str| {
            let d = units.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("unit.json"), body).unwrap();
        };
        write("run_a_torn", "{ not json");
        write(
            "run_b_unknown_state",
            r#"{"kind":"agent","status":{"state":"levitating"}}"#,
        );
        write(
            "run_c_unknown_kind",
            r#"{"kind":"sorcery","status":{"state":"running"}}"#,
        );
        write(
            "run_d_pgid_out_of_range",
            r#"{"kind":"agent","pgid":99999999999,"status":{"state":"running"}}"#,
        );
        write(
            "not a run id",
            r#"{"kind":"agent","pgid":88,"status":{"state":"running"}}"#,
        );
        // A dir with no unit.json, and a stray file where a dir is expected.
        std::fs::create_dir_all(units.join("run_e_empty")).unwrap();
        std::fs::write(units.join("stray.txt"), "x").unwrap();
        write(
            "run_f_good",
            r#"{"kind":"agent","pgid":77,"status":{"state":"failed","error":"boom"}}"#,
        );

        let got = units_on_disk(dir.path());
        let ids: Vec<&str> = got.iter().map(|u| u.unit_id.as_str()).collect();
        assert_eq!(ids, ["run_f_good"], "{got:?}");
        assert_eq!(got[0].pgid, Some(77));
        assert_eq!(got[0].status, UnitStatus::Failed("boom".into()));
    }

    #[test]
    fn units_on_disk_lists_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running]));
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        let mut want = vec![
            sup.dispatch(spec("a#1")).unwrap(),
            sup.dispatch(spec("b#1")).unwrap(),
            sup.dispatch(spec("c#1")).unwrap(),
        ];
        want.sort();
        let got: Vec<String> = units_on_disk(dir.path())
            .into_iter()
            .map(|u| u.unit_id)
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn the_mocks_spawn_side_effect_runs_under_the_supervisors_run_dir() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Arc::new(
            MockUnitLauncher::scripted(vec![UnitStatus::Running]).with_on_spawn(|spec, run_dir| {
                std::fs::write(run_dir.join(format!("{}.marker", spec.agent)), "x").unwrap();
            }),
        );
        let sup = FleetSupervisor::new(launcher, dir.path().to_path_buf());
        sup.dispatch(spec("recon#1")).unwrap();
        assert!(dir.path().join("recon.marker").is_file());
    }
}
