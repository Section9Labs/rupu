//! Read-only local orphan projection. Never repairs storage or signals a PID.
//! A recorded running status is not evidence that its worker survived a reboot.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Read, os::unix::fs::OpenOptionsExt, path::Path};

pub(crate) const EXITED: &str =
    "Session worker exited before its completion was recorded. The saved transcript is available; no response was restarted.";

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Observation {
    pub state: &'static str,
    pub recorded_status: String,
    pub recorded_active_run_id: String,
    pub recorded_run_status: Value,
    pub recorded_run_error: Option<String>,
    pub recorded_last_error: Option<String>,
    pub transcript_path: String,
}

#[derive(Deserialize)]
struct Record {
    session_id: String,
    status: String,
    active_run_id: Option<String>,
    active_transcript_path: Option<String>,
    active_pid: Option<u32>,
    worker_pid: Option<u32>,
    last_error: Option<String>,
    #[serde(default)]
    runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    run_id: String,
    transcript_path: Option<String>,
    #[serde(default)]
    status: Value,
    completed_at: Option<String>,
    error: Option<String>,
    pid: Option<u32>,
}

#[derive(Deserialize)]
struct Owner {
    pid: u32,
    start: Option<u64>,
}

fn read_owner(path: &Path) -> Option<Owner> {
    const LIMIT: u64 = 16 * 1024;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ProcessState {
    Present,
    Exited,
    Unknown,
}

/// ESRCH or a verified zombie establishes absence. Permission/probe failures
/// remain unknown. A reused PID is absent only for a matching stamped owner.
fn process_state(pid: u32, start: Option<u64>) -> ProcessState {
    let Some(pid_value) = i32::try_from(pid)
        .ok()
        .filter(|p| *p > 0)
        .and_then(rustix::process::Pid::from_raw)
    else {
        return ProcessState::Unknown;
    };
    match rustix::process::test_kill_process(pid_value) {
        Err(rustix::io::Errno::SRCH) => ProcessState::Exited,
        Err(_) => ProcessState::Unknown,
        Ok(()) => {
            if rupu_launch::proc::is_zombie(pid) {
                return ProcessState::Exited;
            }
            match start {
                Some(recorded) => match rupu_launch::proc::start_stamp(pid) {
                    Some(current) if current != recorded => ProcessState::Exited,
                    Some(_) => ProcessState::Present,
                    None => ProcessState::Unknown,
                },
                None => ProcessState::Present,
            }
        }
    }
}

pub(crate) fn observe(dir: &Path, text: &str) -> Option<Observation> {
    observe_current(dir, text, process_state)
}

fn observe_current(
    dir: &Path,
    text: &str,
    probe: impl Fn(u32, Option<u64>) -> ProcessState,
) -> Option<Observation> {
    let observed = observe_with(dir, text, probe)?;
    // Do not override a completion or replacement worker written during the
    // process checks. Bound this second read to the already-loaded snapshot.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(dir.join("session.json"))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() != text.len() as u64 {
        return None;
    }
    let mut current = Vec::new();
    file.take(text.len() as u64 + 1)
        .read_to_end(&mut current)
        .ok()?;
    (current == text.as_bytes()).then_some(observed)
}

fn observe_with(
    dir: &Path,
    text: &str,
    probe: impl Fn(u32, Option<u64>) -> ProcessState,
) -> Option<Observation> {
    // Archive/mirror records are history, never claims about this machine.
    if dir.parent()?.file_name()? != "sessions" {
        return None;
    }
    let record: Record = serde_json::from_str(text).ok()?;
    if dir.file_name()?.to_str()? != record.session_id || record.status != "running" {
        return None;
    }
    let run_id = record.active_run_id.as_ref()?;
    if run_id.is_empty()
        || run_id.len() > 256
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let transcript = record
        .active_transcript_path
        .as_ref()
        .filter(|p| !p.is_empty())?;
    let mut matching = record.runs.iter().filter(|r| &r.run_id == run_id);
    let run = matching.next()?;
    if matching.next().is_some()
        || run.transcript_path.as_ref() != Some(transcript)
        || run.completed_at.is_some()
        || !(run.status.is_null() || run.status == "running" || run.status == "pending")
    {
        return None;
    }
    let mut pids: Vec<u32> = [record.active_pid, record.worker_pid, run.pid]
        .into_iter()
        .flatten()
        .collect();
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() {
        return None;
    }
    // Only this run's matching owner can qualify a reused PID. Its absence is
    // not proof that a live legacy worker is dead or cannot be cancelled.
    let owner = read_owner(&dir.join(format!("cancel-{run_id}.owner.json")));
    if !pids.iter().all(|pid| {
        let start = owner
            .as_ref()
            .filter(|o| o.pid == *pid)
            .and_then(|o| o.start);
        probe(*pid, start) == ProcessState::Exited
    }) {
        return None;
    }
    Some(Observation {
        state: "exited",
        recorded_status: record.status,
        recorded_active_run_id: run_id.clone(),
        recorded_run_status: run.status.clone(),
        recorded_run_error: run.error.clone(),
        recorded_last_error: record.last_error,
        transcript_path: transcript.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> Value {
        json!({"session_id":"ses_test","status":"running","active_run_id":"run_test",
            "active_transcript_path":"/saved/run.jsonl","active_pid":42,"worker_pid":42,
            "runs":[{"run_id":"run_test","transcript_path":"/saved/run.jsonl","status":null}]})
    }

    #[test]
    fn only_proven_exit_projects_and_preserves_recorded_identity() {
        let dir = Path::new("/fixture/sessions/ses_test");
        for state in [ProcessState::Present, ProcessState::Unknown] {
            assert!(observe_with(dir, &record().to_string(), |_, _| state).is_none());
        }
        let observed =
            observe_with(dir, &record().to_string(), |_, _| ProcessState::Exited).unwrap();
        assert_eq!(observed.recorded_status, "running");
        assert_eq!(observed.recorded_active_run_id, "run_test");
        assert!(observed.recorded_run_status.is_null());
        assert_eq!(observed.transcript_path, "/saved/run.jsonl");
    }

    #[test]
    fn malformed_archive_wrong_owner_and_terminal_records_remain_unchanged() {
        let base = record();
        let dir = Path::new("/fixture/sessions/ses_test");
        for (key, value) in [
            ("status", json!("idle")),
            ("session_id", json!("ses_other")),
            ("active_run_id", json!("../run_test")),
            ("active_transcript_path", json!("/wrong")),
            ("active_pid", json!("not a pid")),
            ("runs", json!([])),
        ] {
            let mut changed = base.clone();
            changed[key] = value;
            assert!(observe_with(dir, &changed.to_string(), |_, _| ProcessState::Exited).is_none());
        }
        let mut missing = base.clone();
        missing["active_pid"] = Value::Null;
        missing["worker_pid"] = Value::Null;
        assert!(observe_with(dir, &missing.to_string(), |_, _| ProcessState::Exited).is_none());
        for status in ["ok", "error", "aborted"] {
            let mut done = base.clone();
            done["runs"][0]["status"] = json!(status);
            assert!(observe_with(dir, &done.to_string(), |_, _| ProcessState::Exited).is_none());
        }
        let mut duplicate = base.clone();
        duplicate["runs"]
            .as_array_mut()
            .unwrap()
            .push(base["runs"][0].clone());
        assert!(observe_with(dir, &duplicate.to_string(), |_, _| ProcessState::Exited).is_none());
        assert!(observe_with(
            Path::new("/fixture/sessions-archive/ses_test"),
            &base.to_string(),
            |_, _| ProcessState::Exited
        )
        .is_none());
    }

    #[test]
    fn any_live_or_unknown_owner_prevents_orphan_projection() {
        let mut record = record();
        record["worker_pid"] = json!(43);
        for second in [ProcessState::Present, ProcessState::Unknown] {
            assert!(observe_with(
                Path::new("/fixture/sessions/ses_test"),
                &record.to_string(),
                |pid, _| if pid == 42 {
                    ProcessState::Exited
                } else {
                    second
                }
            )
            .is_none());
        }
        assert_eq!(process_state(0, None), ProcessState::Unknown);
        assert_eq!(process_state(u32::MAX, None), ProcessState::Unknown);
    }

    #[test]
    fn only_matching_run_owner_qualifies_a_reused_pid() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sessions/ses_test");
        std::fs::create_dir_all(&dir).unwrap();
        let owner = dir.join("cancel-run_test.owner.json");
        for pid in [43, 42] {
            std::fs::write(&owner, json!({"pid":pid,"start":17}).to_string()).unwrap();
            let observed = observe_with(&dir, &record().to_string(), |pid, start| {
                assert_eq!(pid, 42);
                if start == Some(17) {
                    ProcessState::Exited
                } else {
                    ProcessState::Present
                }
            });
            assert_eq!(observed.is_some(), pid == 42);
        }
    }

    #[test]
    fn live_run_pid_cannot_be_hidden_by_stale_session_pids() {
        let mut record = record();
        record["runs"][0]["pid"] = json!(43);
        assert!(observe_with(
            Path::new("/fixture/sessions/ses_test"),
            &record.to_string(),
            |pid, _| if pid == 43 {
                ProcessState::Present
            } else {
                ProcessState::Exited
            }
        )
        .is_none());
    }

    #[test]
    fn owner_receipts_are_bounded_regular_files_without_symlink_following() {
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("owner.json");
        std::fs::write(&owner, json!({"pid":42,"start":17}).to_string()).unwrap();
        assert_eq!(read_owner(&owner).unwrap().start, Some(17));
        let link = temp.path().join("link.json");
        std::os::unix::fs::symlink(&owner, &link).unwrap();
        assert!(read_owner(&link).is_none());
        assert!(read_owner(temp.path()).is_none());
        std::fs::write(&owner, vec![b' '; 16 * 1024 + 1]).unwrap();
        assert!(read_owner(&owner).is_none());
    }

    #[test]
    fn concurrent_session_write_invalidates_the_orphan_observation() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("sessions/ses_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.json");
        let original = record().to_string();
        std::fs::write(&path, &original).unwrap();
        assert!(observe_current(&dir, &original, |_, _| ProcessState::Exited).is_some());
        let observed = observe_current(&dir, &original, |_, _| {
            let mut changed = record();
            changed["active_run_id"] = json!("run_next");
            std::fs::write(&path, changed.to_string()).unwrap();
            ProcessState::Exited
        });
        assert!(observed.is_none());
    }
}
