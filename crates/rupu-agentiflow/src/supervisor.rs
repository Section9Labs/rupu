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

use crate::unit::{UnitError, UnitId, UnitKind, UnitLauncher, UnitSpec, UnitStatus};

/// How often `join` re-polls a unit that has not finished.
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
struct UnitRec {
    /// The agent (or, for a workflow unit, the workflow) the unit runs.
    agent: String,
    kind: UnitKind,
    participant: String,
    started_at: DateTime<Utc>,
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

        let id = self.launcher.spawn(&spec, &self.run_dir)?;
        let rec = UnitRec {
            agent: spec.agent,
            kind: spec.kind,
            participant: spec.participant,
            started_at: Utc::now(),
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

    fn lock(&self) -> MutexGuard<'_, HashMap<String, UnitRec>> {
        self.units.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Best-effort mirror of `rec` to `units/<id>/unit.json` (tmp + rename).
    fn write_unit_json(&self, id: &str, rec: &UnitRec) {
        // The id is a path component; only ever write for a plain run id.
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
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
            "status": status_json(&rec.status),
        });
        if let Err(e) = write_json_atomic(&dir, &body) {
            tracing::warn!(unit = id, error = %e, "could not write unit.json");
        }
    }
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
            fn spawn(&self, _: &UnitSpec, _: &Path) -> Result<UnitId, UnitError> {
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
