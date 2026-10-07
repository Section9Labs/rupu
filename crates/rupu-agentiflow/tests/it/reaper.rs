//! `reap_orphaned_agentiflows`: a `running` record whose coordinator is dead is
//! finalized `failed` and its still-live unit groups are killed; anything whose
//! owner is alive or unknown is left alone; a bad run dir never aborts a sweep.
//! `hard_stop` (the operator's `stop --now`) shares the unit wind-down and the
//! finalize, and is tested at the bottom.
//!
//! Fixtures are written straight onto disk (a record via the public
//! `AgentiflowRecord::write`, a `unit.json` as the supervisor lays it out), and
//! the "live unit" is a real `sleep` in its own process group — the shape a
//! detached unit has — so the kill is observed, never assumed. No test signals
//! a made-up pgid.

use std::io::BufRead as _;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Utc;
use rupu_agentiflow::{
    agentiflow_dir, hard_stop, kill_group, pid_is_running, reap_orphaned_agentiflows,
    AgentiflowRecord, HardStopOutcome,
};
use rupu_runtime::RunTriggerSource;
use serde_json::{json, Value};
use tempfile::{tempdir, TempDir};

/// A unit's stand-in: a `sleep` leading its own process group, with a thread
/// reaping it the moment it dies. (Without the waiter the killed leader stays a
/// zombie of this test process, and `kill(pid, 0)` reads a zombie as alive.)
struct Sleeper {
    pgid: u32,
    waiter: Option<JoinHandle<ExitStatus>>,
}

impl Sleeper {
    fn spawn() -> Self {
        Self::spawn_script("sleep 60")
    }

    /// A unit that has SIGTERM ignored (inherited across the `exec` of its
    /// `sleep`, so the whole group shrugs it off): only SIGKILL ends it.
    fn spawn_ignoring_sigterm() -> Self {
        Self::spawn_script("trap '' TERM; echo ready; sleep 60")
    }

    fn spawn_script(script: &str) -> Self {
        let mut child = Command::new("/bin/sh")
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = child.id();
        if script.contains("echo ready") {
            // Do not let the reaper's SIGTERM land before the trap is in place.
            let mut line = String::new();
            std::io::BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line.trim(), "ready");
        }
        let waiter = std::thread::spawn(move || child.wait().unwrap());
        Self {
            pgid,
            waiter: Some(waiter),
        }
    }
}

impl Drop for Sleeper {
    /// A test that expected the sleeper to survive (or whose reaper failed to
    /// kill it) must not leave a `sleep` behind.
    fn drop(&mut self) {
        if let Some(waiter) = self.waiter.take() {
            kill_group(self.pgid);
            let _ = waiter.join();
        }
    }
}

/// Wait (bounded) for a group the reaper signalled to be gone. The reaper
/// returns as soon as it has sent SIGKILL (in production it is not the units'
/// parent and cannot reap them), so the kernel's termination and this test's
/// own waiter reaping the zombie race a bare `!pid_is_running`: `kill(pid, 0)`
/// reads a dying or zombie leader as alive for a brief window.
fn wait_until_dead(pgid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && pid_is_running(pgid) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!pid_is_running(pgid), "group should have been killed");
}

/// A pid that is certainly not running: a child that has exited and been reaped.
fn a_dead_pid() -> u32 {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!pid_is_running(pid), "the fixture pid must be dead");
    pid
}

fn run_dir(global: &TempDir, id: &str) -> PathBuf {
    agentiflow_dir(global.path()).join(id)
}

fn write_record(global: &TempDir, id: &str, status: &str, runner_pid: Option<u32>) {
    let rec = AgentiflowRecord {
        id: id.into(),
        name: "itest".into(),
        engagement_profiles: vec![],
        trigger: RunTriggerSource::Agentiflow,
        status: status.into(),
        stop_reason: None,
        rounds: 2,
        goals: vec![],
        started_at: Utc::now() - chrono::Duration::minutes(5),
        ended_at: None,
        codename: None,
        spent_usd: Some(0.5),
        spent_tokens: 1000,
        runner_pid,
    };
    rec.write(&run_dir(global, id)).unwrap();
}

fn write_running_record(global: &TempDir, id: &str, runner_pid: Option<u32>) {
    write_record(global, id, "running", runner_pid);
}

/// `units/<unit_id>/unit.json` as the supervisor writes it.
fn write_unit(global: &TempDir, run_id: &str, unit_id: &str, pgid: Option<u32>, state: &str) {
    let dir = run_dir(global, run_id).join("units").join(unit_id);
    std::fs::create_dir_all(&dir).unwrap();
    let body = json!({
        "run_id": unit_id,
        "agent": "scanner",
        "kind": "agent",
        "participant": null,
        "started_at": Utc::now().to_rfc3339(),
        "pgid": pgid,
        "status": { "state": state },
    });
    std::fs::write(dir.join("unit.json"), serde_json::to_vec(&body).unwrap()).unwrap();
}

fn read_record(global: &TempDir, id: &str) -> AgentiflowRecord {
    AgentiflowRecord::read(&run_dir(global, id)).unwrap()
}

fn events(global: &TempDir, id: &str) -> Vec<Value> {
    let path = run_dir(global, id).join("events.jsonl");
    match std::fs::read_to_string(path) {
        Ok(raw) => raw
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn reaps_a_dead_coordinator_and_signals_its_units() {
    let global = tempdir().unwrap();
    let unit = Sleeper::spawn();
    write_running_record(&global, "af_dead", Some(a_dead_pid()));
    write_unit(&global, "af_dead", "u1", Some(unit.pgid), "running");
    // A healthy one: its runner_pid is THIS process (alive) → must not be reaped.
    write_running_record(&global, "af_live", Some(std::process::id()));

    let now = Utc::now();
    let summary = reap_orphaned_agentiflows(global.path(), now);

    assert_eq!(summary.scanned, 2);
    assert_eq!(summary.reaped, vec!["af_dead".to_string()]);

    let dead = read_record(&global, "af_dead");
    assert_eq!(dead.status, "failed");
    let reason = dead.stop_reason.as_deref().unwrap();
    assert!(reason.starts_with("orphaned: coordinator pid "), "{reason}");
    assert!(reason.ends_with(" not running"), "{reason}");
    assert_eq!(dead.ended_at, Some(now));
    assert_eq!(dead.runner_pid, None);
    assert_eq!(dead.rounds, 2, "the rest of the record is kept");
    assert_eq!(dead.spent_tokens, 1000);
    wait_until_dead(unit.pgid); // the unit group was killed

    // A terminal event, in the shape `run_agentiflow` writes, so a live events
    // view stops spinning.
    let evs = events(&global, "af_dead");
    assert_eq!(evs.len(), 1, "{evs:?}");
    let stopped = &evs[0];
    assert_eq!(stopped["kind"], "run_stopped");
    assert_eq!(stopped["stop_reason"], reason);
    assert_eq!(stopped["detail"], reason);
    assert_eq!(stopped["rounds"], 2);
    assert_eq!(stopped["spent_tokens"], 1000);
    assert!(stopped["ts"].as_str().is_some());
    assert!(stopped["goals"].is_array());
    assert!(stopped["summary"].is_string());

    let live = read_record(&global, "af_live");
    assert_eq!(live.status, "running", "a healthy run is untouched");
    assert_eq!(live.runner_pid, Some(std::process::id()));
    assert!(events(&global, "af_live").is_empty());
}

#[test]
fn a_none_pid_record_is_never_reaped() {
    // The create→write-back window (and a record from before pids were
    // recorded): the owner is UNKNOWN, not dead.
    let global = tempdir().unwrap();
    let unit = Sleeper::spawn();
    write_running_record(&global, "af_new", None);
    write_unit(&global, "af_new", "u1", Some(unit.pgid), "running");

    let s = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert_eq!(s.scanned, 1);
    assert!(s.reaped.is_empty());
    let rec = read_record(&global, "af_new");
    assert_eq!(rec.status, "running");
    assert_eq!(rec.stop_reason, None);
    assert!(pid_is_running(unit.pgid), "its units are left alone too");
    assert!(events(&global, "af_new").is_empty());
}

#[test]
fn a_healthy_record_and_its_units_are_untouched() {
    let global = tempdir().unwrap();
    let unit = Sleeper::spawn();
    write_running_record(&global, "af_ok", Some(std::process::id()));
    write_unit(&global, "af_ok", "u1", Some(unit.pgid), "running");

    let s = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert!(s.reaped.is_empty());
    assert_eq!(read_record(&global, "af_ok").status, "running");
    assert!(pid_is_running(unit.pgid));
    assert!(events(&global, "af_ok").is_empty());
}

#[test]
fn only_a_running_record_is_reaped() {
    // A finished record keeps its status even though its runner_pid (a stale
    // value that should have been cleared) names a dead process.
    let global = tempdir().unwrap();
    write_record(&global, "af_done", "completed", Some(a_dead_pid()));
    write_record(&global, "af_failed", "failed", Some(a_dead_pid()));

    let s = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert_eq!(s.scanned, 2);
    assert!(s.reaped.is_empty());
    assert_eq!(read_record(&global, "af_done").status, "completed");
    assert_eq!(read_record(&global, "af_failed").status, "failed");
    assert!(events(&global, "af_done").is_empty());
}

#[test]
fn a_finished_units_group_is_left_alone() {
    // Only a NON-terminal unit's group is signalled: a `done` / `failed` unit
    // whose recorded pgid still answers is not ours to kill (that number may
    // have been recycled onto something unrelated).
    let global = tempdir().unwrap();
    let done = Sleeper::spawn();
    let failed = Sleeper::spawn();
    let running = Sleeper::spawn();
    let pending = Sleeper::spawn();
    write_running_record(&global, "af_dead", Some(a_dead_pid()));
    write_unit(&global, "af_dead", "u1", Some(done.pgid), "done");
    write_unit(&global, "af_dead", "u2", Some(failed.pgid), "failed");
    write_unit(&global, "af_dead", "u3", Some(running.pgid), "running");
    write_unit(&global, "af_dead", "u4", Some(pending.pgid), "pending");

    let s = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert_eq!(s.reaped, vec!["af_dead".to_string()]);
    assert!(pid_is_running(done.pgid));
    assert!(pid_is_running(failed.pgid));
    wait_until_dead(running.pgid);
    wait_until_dead(pending.pgid);
}

#[test]
fn a_unit_that_ignores_sigterm_is_killed_after_the_grace() {
    let global = tempdir().unwrap();
    let stubborn = Sleeper::spawn_ignoring_sigterm();
    write_running_record(&global, "af_dead", Some(a_dead_pid()));
    write_unit(&global, "af_dead", "u1", Some(stubborn.pgid), "running");

    let started = Instant::now();
    let s = reap_orphaned_agentiflows(global.path(), Utc::now());
    let took = started.elapsed();

    assert_eq!(s.reaped, vec!["af_dead".to_string()]);
    wait_until_dead(stubborn.pgid); // SIGKILL ended the group
    assert!(
        took >= Duration::from_millis(1400),
        "SIGTERM was given its grace first: {took:?}"
    );
    assert!(
        took < Duration::from_secs(5),
        "the escalation is bounded: {took:?}"
    );
    assert_eq!(read_record(&global, "af_dead").status, "failed");
}

#[test]
fn a_dead_unit_group_is_not_signalled_and_does_not_block_the_reap() {
    // A unit whose leader already exited (pgid not running) and one with no
    // recorded pgid: nothing to signal, the record is still finalized.
    let global = tempdir().unwrap();
    write_running_record(&global, "af_dead", Some(a_dead_pid()));
    write_unit(&global, "af_dead", "u1", Some(a_dead_pid()), "running");
    write_unit(&global, "af_dead", "u2", None, "running");

    let started = Instant::now();
    let s = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert_eq!(s.reaped, vec!["af_dead".to_string()]);
    assert_eq!(read_record(&global, "af_dead").status, "failed");
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "no group was signalled, so there is no grace to wait out"
    );
}

#[test]
fn a_second_sweep_does_not_reap_again() {
    let global = tempdir().unwrap();
    write_running_record(&global, "af_dead", Some(a_dead_pid()));

    let first = reap_orphaned_agentiflows(global.path(), Utc::now());
    let second = reap_orphaned_agentiflows(global.path(), Utc::now());

    assert_eq!(first.reaped, vec!["af_dead".to_string()]);
    assert!(second.reaped.is_empty());
    assert_eq!(second.scanned, 1);
    let evs = events(&global, "af_dead");
    assert_eq!(
        evs.iter().filter(|e| e["kind"] == "run_stopped").count(),
        1,
        "exactly one terminal event: {evs:?}"
    );
}

#[test]
fn an_existing_event_log_is_appended_to_not_replaced() {
    let global = tempdir().unwrap();
    write_running_record(&global, "af_dead", Some(a_dead_pid()));
    std::fs::write(
        run_dir(&global, "af_dead").join("events.jsonl"),
        "{\"ts\":\"2026-10-06T00:00:00.000Z\",\"kind\":\"run_started\"}\n",
    )
    .unwrap();

    reap_orphaned_agentiflows(global.path(), Utc::now());

    let kinds: Vec<String> = events(&global, "af_dead")
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["run_started", "run_stopped"]);
}

#[test]
fn a_missing_agentiflows_dir_is_an_empty_sweep() {
    let global = tempdir().unwrap();
    let s = reap_orphaned_agentiflows(global.path(), Utc::now());
    assert_eq!(s.scanned, 0);
    assert!(s.reaped.is_empty());
}

#[test]
fn a_bad_run_dir_is_skipped_and_never_aborts_the_sweep() {
    let global = tempdir().unwrap();
    let root = agentiflow_dir(global.path());
    std::fs::create_dir_all(&root).unwrap();

    // Unreadable / torn records, an empty dir, a stray file, a non-run dir.
    std::fs::create_dir_all(root.join("af_garbage")).unwrap();
    std::fs::write(
        root.join("af_garbage").join("agentiflow.json"),
        b"{ not json",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("af_empty")).unwrap();
    std::fs::write(root.join("af_stray_file"), b"x").unwrap();
    std::fs::create_dir_all(root.join("not_a_run")).unwrap();
    std::fs::write(
        root.join("not_a_run").join("agentiflow.json"),
        b"{ not json",
    )
    .unwrap();

    // A real orphan among them, whose `units/` is incomplete: a torn unit.json,
    // a unit dir with none, a stray file, an unsafe-looking name, a corrupt
    // pgid 0, and this very process's own group (never signalled).
    write_running_record(&global, "af_dead", Some(a_dead_pid()));
    let units = run_dir(&global, "af_dead").join("units");
    std::fs::create_dir_all(units.join("torn")).unwrap();
    std::fs::write(units.join("torn").join("unit.json"), b"{ \"status\": ").unwrap();
    std::fs::create_dir_all(units.join("no_json")).unwrap();
    std::fs::write(units.join("stray_file"), b"x").unwrap();
    write_unit(&global, "af_dead", "zero", Some(0), "running");
    let own_group = rustix::process::getpgrp().as_raw_nonzero().get() as u32;
    write_unit(&global, "af_dead", "own", Some(own_group), "running");

    let started = Instant::now();
    let s = reap_orphaned_agentiflows(global.path(), Utc::now());

    // Only directories named `af_*` are run dirs: the stray file and `not_a_run`
    // are not looked at.
    assert_eq!(s.scanned, 3, "af_garbage + af_empty + af_dead");
    assert_eq!(s.reaped, vec!["af_dead".to_string()]);
    assert_eq!(read_record(&global, "af_dead").status, "failed");
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "nothing was signalled, so no grace was waited"
    );
    // The bad ones are exactly as they were.
    assert_eq!(
        std::fs::read(root.join("af_garbage").join("agentiflow.json")).unwrap(),
        b"{ not json"
    );
    assert!(!Path::new(&root.join("af_empty").join("agentiflow.json")).exists());
}

// ---- hard_stop ---------------------------------------------------------------

#[test]
fn a_hard_stop_kills_the_coordinator_and_the_units_and_closes_the_record_and_the_log() {
    let global = tempdir().unwrap();
    // The coordinator is a live process (the CLI's SIGTERM handler would exit
    // it without cleanup); one unit ignores SIGTERM, so only the escalation
    // ends it.
    let coordinator = Sleeper::spawn();
    let polite = Sleeper::spawn();
    let stubborn = Sleeper::spawn_ignoring_sigterm();
    write_running_record(&global, "af_live", Some(coordinator.pgid));
    write_unit(&global, "af_live", "u1", Some(polite.pgid), "running");
    write_unit(&global, "af_live", "u2", Some(stubborn.pgid), "running");

    let now = Utc::now();
    let started = Instant::now();
    let out = hard_stop(&run_dir(&global, "af_live"), now).unwrap();
    let took = started.elapsed();

    assert_eq!(out, HardStopOutcome::Stopped);
    wait_until_dead(coordinator.pgid);
    wait_until_dead(polite.pgid);
    wait_until_dead(stubborn.pgid);
    assert!(
        took >= Duration::from_millis(1400),
        "SIGTERM got its grace before SIGKILL: {took:?}"
    );

    let rec = read_record(&global, "af_live");
    assert_eq!(rec.status, "failed");
    assert_eq!(rec.stop_reason.as_deref(), Some("operator_stop:now"));
    assert_eq!(rec.ended_at, Some(now));
    assert_eq!(rec.runner_pid, None);
    assert_eq!(rec.rounds, 2, "the rest of the record is kept");
    assert_eq!(rec.spent_tokens, 1000);

    // The log's last line closes the run, in the shape the reaper writes.
    let evs = events(&global, "af_live");
    let last = evs.last().expect("a terminal event");
    assert_eq!(last["kind"], "run_stopped");
    assert_eq!(last["stop_reason"], "operator_stop:now");
    assert_eq!(last["detail"], "operator hard stop (--now)");
    assert_eq!(last["rounds"], 2);
    assert_eq!(last["spent_tokens"], 1000);
    assert!(last["goals"].is_array() && last["summary"].is_string());
    assert_eq!(evs.len(), 1);

    // The record is final: neither a second hard stop nor the reaper revisits it.
    assert_eq!(
        hard_stop(&run_dir(&global, "af_live"), Utc::now()).unwrap(),
        HardStopOutcome::AlreadyTerminal("failed".into())
    );
    assert!(reap_orphaned_agentiflows(global.path(), Utc::now())
        .reaped
        .is_empty());
    assert_eq!(events(&global, "af_live").len(), 1);
}

#[test]
fn a_hard_stop_of_a_finished_run_signals_and_writes_nothing() {
    let global = tempdir().unwrap();
    let bystander = Sleeper::spawn();
    write_record(&global, "af_done", "completed", Some(bystander.pgid));
    write_unit(&global, "af_done", "u1", Some(bystander.pgid), "running");

    let out = hard_stop(&run_dir(&global, "af_done"), Utc::now()).unwrap();

    assert_eq!(out, HardStopOutcome::AlreadyTerminal("completed".into()));
    assert!(pid_is_running(bystander.pgid), "nothing was signalled");
    let rec = read_record(&global, "af_done");
    assert_eq!(rec.status, "completed");
    assert_eq!(rec.stop_reason, None);
    assert_eq!(
        rec.runner_pid,
        Some(bystander.pgid),
        "the record is untouched"
    );
    assert!(events(&global, "af_done").is_empty());
}

#[test]
fn a_run_that_finishes_during_the_grace_is_not_overwritten() {
    let global = tempdir().unwrap();
    // A unit that outlasts SIGTERM keeps the hard stop in its grace for 1.5s.
    let stubborn = Sleeper::spawn_ignoring_sigterm();
    write_running_record(&global, "af_race", None);
    write_unit(&global, "af_race", "u1", Some(stubborn.pgid), "running");

    // The coordinator wins the race: it finishes its run while the stop waits.
    let dir = run_dir(&global, "af_race");
    let finisher = std::thread::spawn({
        let dir = dir.clone();
        move || {
            std::thread::sleep(Duration::from_millis(300));
            let mut rec = AgentiflowRecord::read(&dir).unwrap();
            rec.status = "completed".into();
            rec.stop_reason = Some("goals_met".into());
            rec.write(&dir).unwrap();
        }
    });

    let out = hard_stop(&dir, Utc::now()).unwrap();
    finisher.join().unwrap();

    assert_eq!(out, HardStopOutcome::AlreadyTerminal("completed".into()));
    wait_until_dead(stubborn.pgid); // the units were still wound down
    let rec = read_record(&global, "af_race");
    assert_eq!(rec.status, "completed");
    assert_eq!(rec.stop_reason.as_deref(), Some("goals_met"));
    assert!(
        events(&global, "af_race").is_empty(),
        "no run_stopped over a run that finished"
    );
}

#[test]
fn a_hard_stop_of_a_run_with_no_record_is_an_error_not_a_write() {
    let global = tempdir().unwrap();
    let dir = run_dir(&global, "af_missing");
    std::fs::create_dir_all(&dir).unwrap();
    let err = hard_stop(&dir, Utc::now()).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert!(!dir.join("agentiflow.json").exists());
    assert!(!dir.join("events.jsonl").exists());
}
