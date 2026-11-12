#![cfg(unix)]
use super::*;
use crate::runs::RunStore;
use std::{
    fs,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn fixture() -> (tempfile::TempDir, RunStore, RunRecord, File) {
    let temp = tempfile::tempdir().unwrap();
    let store = RunStore::new(temp.path().join("runs"));
    let record: RunRecord = serde_json::from_value(serde_json::json!({
        "id":"run_execution", "workflow_name":"execution-fixture", "status":"running",
        "inputs":{}, "workspace_id":"ws_execution", "workspace_path":temp.path(),
        "transcript_dir":temp.path().join("transcripts"), "started_at":"2026-11-12T12:00:00Z"
    }))
    .unwrap();
    let record = store
        .create(record, "name: execution-fixture\nsteps: []\n")
        .unwrap();
    let dir = File::open(store.root.join(&record.id)).unwrap();
    (temp, store, record, dir)
}
fn path(store: &RunStore, record: &RunRecord) -> std::path::PathBuf {
    store.root.join(&record.id)
}
fn read(dir: &File, record: &RunRecord) -> ExecutionObservation {
    read_for_owner(dir, record, false).envelope
}

#[test]
fn production_owner_admits_exact_bodies_and_leaves_independently() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let first = owner
        .handle()
        .enter(
            "first",
            vec![ExecutionIteration {
                loop_id: "refine".into(),
                iteration: 2,
            }],
        )
        .unwrap();
    let second = owner.handle().enter("second", vec![]).unwrap();
    let both = read(&dir, &record);
    assert_eq!(both.validity, ExecutionValidity::Current);
    assert!(both.complete);
    assert_eq!(both.members.len(), 2);
    assert_eq!(both.members[0].iterations[0].iteration, 2);
    assert_ne!(both.members[0].activation_id, both.members[1].activation_id);
    first.finish().unwrap();
    let remaining = read(&dir, &record);
    assert_eq!(remaining.members.len(), 1);
    assert_eq!(remaining.members[0].step_id, "second");
    assert!(
        remaining.revision.as_ref().unwrap().parse::<u64>().unwrap()
            > both.revision.unwrap().parse::<u64>().unwrap()
    );
    second.finish().unwrap();
    owner.finish().unwrap();
    let closed = read(&dir, &record);
    assert_eq!(closed.validity, ExecutionValidity::Stale);
    assert_eq!(closed.reason.as_deref(), Some("scheduler_finished"));
    assert!(closed.members.is_empty());
    assert!(!closed.complete);
}

#[test]
fn duplicate_owner_is_refused_without_revoking_the_incumbent() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    assert!(ExecutionOwner::start(&path(&store, &record), &record).is_err());
    assert_eq!(read(&dir, &record).validity, ExecutionValidity::Current);
    owner.finish().unwrap();
}

#[test]
fn pre_body_snapshot_failure_finalizes_own_registered_run_and_emits_terminal_event() {
    use std::os::unix::fs::symlink;
    let (temp, store, mut record, _dir) = fixture();
    record.runner_pid = Some(std::process::id());
    store.update(&record).unwrap();
    let _registered = store.register_runner(&record.id);
    assert_eq!(store.live_runner(&record), Some(std::process::id()));
    let outside = temp.path().join("outside");
    fs::write(&outside, b"preserve").unwrap();
    symlink(&outside, path(&store, &record).join(SNAPSHOT_FILE)).unwrap();
    let failure = match ExecutionOwner::start(&path(&store, &record), &record) {
        Ok(_) => panic!("unsafe snapshot destination was accepted"),
        Err(error) => error.to_string(),
    };
    let now = Utc::now();
    assert!(store
        .fail_unowned_execution(&record, &failure, now)
        .unwrap());
    let failed = store.load(&record.id).unwrap();
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.finished_at, Some(now));
    assert_eq!(failed.error_message.as_deref(), Some(failure.as_str()));
    assert!(failed.runner_pid.is_none());
    let events = fs::read_to_string(path(&store, &record).join("events.jsonl")).unwrap();
    let event: crate::executor::Event =
        serde_json::from_str(events.lines().last().unwrap()).unwrap();
    assert!(
        matches!(event, crate::executor::Event::RunFailed { run_id, error, .. } if run_id == record.id && error == failure)
    );
    assert_eq!(fs::read(outside).unwrap(), b"preserve");
}

#[test]
fn pre_body_failure_never_finalizes_an_incumbent_or_changed_run() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    assert!(!store
        .fail_unowned_execution(&record, "second owner refused", Utc::now())
        .unwrap());
    assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Running);
    assert_eq!(read(&dir, &record).validity, ExecutionValidity::Current);
    owner.finish().unwrap();
    for changed in [
        {
            let mut value = record.clone();
            value.runner_pid = Some(12345);
            value
        },
        {
            let mut value = record.clone();
            value.workspace_id = "ws_changed".into();
            value
        },
        {
            let mut value = record.clone();
            value.status = RunStatus::Paused;
            value
        },
        {
            let mut value = record.clone();
            value.status = RunStatus::Completed;
            value
        },
    ] {
        store.update(&changed).unwrap();
        assert!(!store
            .fail_unowned_execution(&record, "stale startup failure", Utc::now())
            .unwrap());
        let preserved = store.load(&record.id).unwrap();
        assert_eq!(preserved.status, changed.status);
        assert_eq!(preserved.runner_pid, changed.runner_pid);
        assert_eq!(preserved.workspace_id, changed.workspace_id);
    }
}

#[test]
fn pre_body_failure_preserves_cancel_barrier_and_fails_closed_without_owner_proof() {
    use std::os::unix::fs::symlink;
    let (temp, store, record, _dir) = fixture();
    store
        .cancel(&record.id, "fixture", "cancel", Utc::now())
        .unwrap();
    let barrier = fs::read(path(&store, &record).join(CANCELLATION_FILE)).unwrap();
    assert!(!store
        .fail_unowned_execution(&record, "cancelled startup", Utc::now())
        .unwrap());
    assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Cancelled);
    assert_eq!(
        fs::read(path(&store, &record).join(CANCELLATION_FILE)).unwrap(),
        barrier
    );
    // No usable owner proof: a run must not be finalized on an assumption.
    store.update(&record).unwrap();
    let outside = temp.path().join("owner-lock-outside");
    fs::write(&outside, b"").unwrap();
    symlink(&outside, path(&store, &record).join(OWNER_LOCK_FILE)).unwrap();
    assert!(store
        .fail_unowned_execution(&record, "unprovable startup", Utc::now())
        .is_err());
    assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Running);
    assert_eq!(
        fs::read(path(&store, &record).join(CANCELLATION_FILE)).unwrap(),
        barrier
    );
}

#[test]
fn unfinished_body_revokes_the_whole_generation_even_with_handles_retained() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let handle = owner.handle();
    let first = handle.enter("first", vec![]).unwrap();
    let _second = handle.enter("second", vec![]).unwrap();
    drop(first);
    let stale = read(&dir, &record);
    assert_eq!(stale.validity, ExecutionValidity::Stale);
    assert!(!stale.complete);
    assert_eq!(stale.members.len(), 2);
    assert!(handle.enter("late", vec![]).is_err());
    assert!(!handle.is_valid());
}

#[test]
fn old_generation_drop_cannot_remove_new_generation_members() {
    let (_temp, store, record, dir) = fixture();
    let first = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let old_handle = first.handle();
    let old_body = old_handle.enter("repeated", vec![]).unwrap();
    let old_generation = read(&dir, &record).execution_generation;
    drop(first);
    let second = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let current = second.handle().enter("repeated", vec![]).unwrap();
    drop(old_body);
    assert!(old_handle.enter("late", vec![]).is_err());
    let observed = read(&dir, &record);
    assert_eq!(observed.validity, ExecutionValidity::Current);
    assert_ne!(observed.execution_generation, old_generation);
    assert_eq!(observed.members.len(), 1);
    current.finish().unwrap();
    second.finish().unwrap();
}

#[test]
fn cancelled_before_first_snapshot_cannot_be_reauthorized_from_stale_record() {
    let (_temp, store, record, dir) = fixture();
    store
        .cancel(&record.id, "fixture", "stop", Utc::now())
        .unwrap();
    assert!(ExecutionOwner::start(&path(&store, &record), &record).is_err());
    assert_ne!(read(&dir, &record).validity, ExecutionValidity::Current);
    assert!(path(&store, &record).join(CANCELLATION_FILE).is_file());
}

#[test]
fn cancel_revokes_admissions_and_preserves_last_qualified_members() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let handle = owner.handle();
    let _body = handle.enter("running", vec![]).unwrap();
    store
        .cancel(&record.id, "fixture", "stop", Utc::now())
        .unwrap();
    let stopped = read(&dir, &record);
    assert_eq!(stopped.validity, ExecutionValidity::Stale);
    assert_eq!(stopped.reason.as_deref(), Some("run_cancelled"));
    assert_eq!(stopped.members.len(), 1);
    assert!(!stopped.complete);
    assert!(handle.enter("queued", vec![]).is_err());
}

#[test]
fn cancellation_never_proceeds_without_its_strict_lock() {
    let (_temp, store, record, dir) = fixture();
    let _locked = disk::lock(&dir, MUTATION_LOCK_FILE, true).unwrap();
    assert!(store
        .cancel(&record.id, "fixture", "stop", Utc::now())
        .is_err());
    assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Running);
}

#[test]
fn brief_reader_contention_waits_without_revoking_but_timeout_revokes() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let held = disk::lock(&dir, MUTATION_LOCK_FILE, false).unwrap();
    let handle = owner.handle();
    let (sent, received) = std::sync::mpsc::channel();
    let task = std::thread::spawn(move || {
        sent.send(handle.enter("waited", vec![])).unwrap();
    });
    assert!(received.recv_timeout(Duration::from_millis(30)).is_err());
    drop(held);
    let guard = received
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    task.join().unwrap();
    assert_eq!(read(&dir, &record).validity, ExecutionValidity::Current);
    guard.finish().unwrap();
    let held = disk::lock(&dir, MUTATION_LOCK_FILE, false).unwrap();
    let began = Instant::now();
    assert!(owner.handle().enter("timeout", vec![]).is_err());
    assert!(began.elapsed() >= Duration::from_secs(2));
    assert!(began.elapsed() < Duration::from_secs(4));
    drop(held);
    assert_ne!(read(&dir, &record).validity, ExecutionValidity::Current);
}

#[test]
fn explicit_terminal_retry_clears_barrier_only_after_old_owner_exits() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let old_generation = read(&dir, &record).execution_generation;
    store
        .cancel(&record.id, "fixture", "stop", Utc::now())
        .unwrap();
    let retry = || {
        store.modify_if_status(&record.id, RunStatus::Cancelled, |rec| {
            rec.status = RunStatus::Running
        })
    };
    assert!(retry().is_err(), "an old invocation still owns the run");
    assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Cancelled);
    assert!(path(&store, &record).join(CANCELLATION_FILE).exists());
    drop(owner);
    let resumed = retry().unwrap().unwrap();
    assert!(!path(&store, &record).join(CANCELLATION_FILE).exists());
    let resumed_owner = ExecutionOwner::start(&path(&store, &record), &resumed).unwrap();
    let observed = read(&dir, &resumed);
    assert_eq!(observed.validity, ExecutionValidity::Current);
    assert_ne!(observed.execution_generation, old_generation);
    resumed_owner.finish().unwrap();
}

#[test]
fn stale_retry_cas_never_clears_a_cancellation_barrier() {
    let (_temp, store, record, _dir) = fixture();
    store
        .cancel(&record.id, "fixture", "stop", Utc::now())
        .unwrap();
    let result = store
        .modify_if_status(&record.id, RunStatus::Failed, |rec| {
            rec.status = RunStatus::Running
        })
        .unwrap();
    assert!(result.is_err());
    assert_eq!(store.load(&record.id).unwrap().status, RunStatus::Cancelled);
    assert!(path(&store, &record).join(CANCELLATION_FILE).exists());
}

#[test]
fn archived_and_missing_records_never_claim_current_empty_sets() {
    let (_temp, store, record, dir) = fixture();
    let missing = read(&dir, &record);
    assert_eq!(missing.validity, ExecutionValidity::Unavailable);
    assert_eq!(missing.reason.as_deref(), Some("observation_missing"));
    assert!(!path(&store, &record).join(MUTATION_LOCK_FILE).exists());
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let archived = read_for_owner(&dir, &record, true).envelope;
    assert_eq!(archived.validity, ExecutionValidity::Stale);
    assert_eq!(archived.reason.as_deref(), Some("archived_record"));
    assert!(!archived.complete);
    owner.finish().unwrap();
}

#[test]
fn malformed_wrong_owner_and_oversized_snapshots_fail_qualified() {
    let (_temp, store, record, dir) = fixture();
    let _owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let snapshot = path(&store, &record).join(SNAPSHOT_FILE);
    let original = fs::read(&snapshot).unwrap();
    fs::write(&snapshot, b"{").unwrap();
    assert_eq!(
        read(&dir, &record).reason.as_deref(),
        Some("observation_malformed")
    );
    let mut wrong: ExecutionObservation = serde_json::from_slice(&original).unwrap();
    wrong.workspace_id = "ws_other".into();
    fs::write(&snapshot, serde_json::to_vec(&wrong).unwrap()).unwrap();
    let rejected = read(&dir, &record);
    assert_eq!(rejected.reason.as_deref(), Some("owner_mismatch"));
    assert!(rejected.members.is_empty());
    fs::write(&snapshot, vec![b' '; MAX_SNAPSHOT_BYTES + 1]).unwrap();
    let oversized = read_for_owner(&dir, &record, false);
    assert_eq!(
        oversized.envelope.reason.as_deref(),
        Some("observation_oversized")
    );
    assert_eq!(oversized.execution_input_bytes, 0);
}

#[test]
fn member_bound_revokes_instead_of_truncating_a_complete_set() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let handle = owner.handle();
    let mut guards = Vec::new();
    for n in 0..MAX_MEMBERS {
        guards.push(handle.enter(&format!("step{n}"), vec![]).unwrap());
    }
    assert!(handle.enter("overflow", vec![]).is_err());
    let observed = read(&dir, &record);
    assert_ne!(observed.validity, ExecutionValidity::Current);
    assert!(!observed.complete);
    assert_eq!(observed.members.len(), MAX_MEMBERS);
    drop(guards);
}

#[test]
fn payload_bound_revokes_without_publishing_an_oversized_snapshot() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let iterations: Vec<_> = (0..16)
        .map(|n| ExecutionIteration {
            loop_id: format!("{n:02}{}", "x".repeat(254)),
            iteration: n,
        })
        .collect();
    let mut guards = Vec::new();
    for _ in 0..MAX_MEMBERS {
        match owner.handle().enter(&"s".repeat(1024), iterations.clone()) {
            Ok(guard) => guards.push(guard),
            Err(_) => break,
        }
    }
    assert!(guards.len() < MAX_MEMBERS);
    assert!(!owner.handle().is_valid());
    assert!(
        fs::metadata(path(&store, &record).join(SNAPSHOT_FILE))
            .unwrap()
            .len()
            <= MAX_SNAPSHOT_BYTES as u64
    );
    let observed = read(&dir, &record);
    assert_ne!(observed.validity, ExecutionValidity::Current);
    assert!(!observed.complete);
    assert_eq!(observed.members.len(), guards.len());
}

#[test]
fn restarted_owner_cannot_make_an_old_saved_generation_look_current() {
    let (_temp, store, record, dir) = fixture();
    let root = path(&store, &record);
    let first = ExecutionOwner::start(&root, &record).unwrap();
    let saved = fs::read(root.join(SNAPSHOT_FILE)).unwrap();
    first.finish().unwrap();
    let _second = ExecutionOwner::start(&root, &record).unwrap();
    fs::write(root.join(SNAPSHOT_FILE), saved).unwrap();
    let stale = read(&dir, &record);
    assert_eq!(stale.validity, ExecutionValidity::Stale);
    assert_eq!(stale.reason.as_deref(), Some("owner_not_live"));
    assert!(!stale.complete);
}

#[test]
fn invalid_coordinates_and_write_failure_revoke_the_live_witness() {
    let (_temp, store, record, dir) = fixture();
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let handle = owner.handle();
    assert!(handle
        .enter(
            "step",
            vec![ExecutionIteration {
                loop_id: "".into(),
                iteration: 0
            }]
        )
        .is_err());
    assert_ne!(read(&dir, &record).validity, ExecutionValidity::Current);
    drop(owner);
    let owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    fs::remove_file(path(&store, &record).join(SNAPSHOT_FILE)).unwrap();
    fs::create_dir(path(&store, &record).join(SNAPSHOT_FILE)).unwrap();
    assert!(owner.handle().enter("write_failure", vec![]).is_err());
    assert!(!owner.handle().is_valid());
    assert_ne!(read(&dir, &record).validity, ExecutionValidity::Current);
}

#[test]
fn lock_and_snapshot_aliases_are_refused_without_following_them() {
    use std::os::unix::fs::symlink;
    let (temp, store, record, dir) = fixture();
    let outside = temp.path().join("outside");
    fs::write(&outside, b"preserve").unwrap();
    symlink(&outside, path(&store, &record).join(MUTATION_LOCK_FILE)).unwrap();
    assert!(ExecutionOwner::start(&path(&store, &record), &record).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"preserve");
    fs::remove_file(path(&store, &record).join(MUTATION_LOCK_FILE)).unwrap();
    let _owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    fs::remove_file(path(&store, &record).join(SNAPSHOT_FILE)).unwrap();
    symlink(&outside, path(&store, &record).join(SNAPSHOT_FILE)).unwrap();
    assert_ne!(read(&dir, &record).validity, ExecutionValidity::Current);
    assert_eq!(fs::read(&outside).unwrap(), b"preserve");
}

#[test]
fn generation_lock_replacement_cannot_reuse_saved_authority() {
    let (_temp, store, record, dir) = fixture();
    let _owner = ExecutionOwner::start(&path(&store, &record), &record).unwrap();
    let snapshot = read(&dir, &record);
    let lock_path = path(&store, &record).join(generation_lock(
        snapshot.execution_generation.as_deref().unwrap(),
    ));
    fs::rename(&lock_path, lock_path.with_extension("old")).unwrap();
    fs::write(&lock_path, b"").unwrap();
    let stale = read(&dir, &record);
    assert_eq!(stale.validity, ExecutionValidity::Stale);
    assert_eq!(stale.reason.as_deref(), Some("owner_not_live"));
}

#[test]
fn independent_process_owner_fixture() {
    let Some(root) = std::env::var_os("RUPU_EXECUTION_OWNER_TEST_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let record: RunRecord =
        serde_json::from_slice(&fs::read(root.join("run.json")).unwrap()).unwrap();
    let _owner = ExecutionOwner::start(&root, &record).unwrap();
    let _body = _owner.handle().enter("child", vec![]).unwrap();
    fs::write(root.join("child-ready"), b"ready").unwrap();
    // Parent kills the fixture process: no Rust destructor runs.
    loop {
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn crash_releases_the_generation_witness_and_restart_uses_a_new_generation() {
    let (_temp, store, record, dir) = fixture();
    let root = path(&store, &record);
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("execution_observation::tests::independent_process_owner_fixture")
        .arg("--exact")
        .env("RUPU_EXECUTION_OWNER_TEST_DIR", &root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("child-ready").exists() {
        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("owner child did not become ready");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let live = read(&dir, &record);
    assert_eq!(live.validity, ExecutionValidity::Current);
    assert_eq!(live.members.len(), 1);
    child.kill().unwrap();
    child.wait().unwrap();
    let crashed = read(&dir, &record);
    assert_eq!(crashed.validity, ExecutionValidity::Stale);
    assert_eq!(crashed.reason.as_deref(), Some("owner_not_live"));
    assert_eq!(crashed.execution_generation, live.execution_generation);
    let replacement = ExecutionOwner::start(&root, &record).unwrap();
    let restarted = read(&dir, &record);
    assert_eq!(restarted.validity, ExecutionValidity::Current);
    assert_ne!(restarted.execution_generation, live.execution_generation);
    assert!(restarted.members.is_empty());
    replacement.finish().unwrap();
}
