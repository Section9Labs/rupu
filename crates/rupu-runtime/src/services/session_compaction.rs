//! A session's pending compaction: `rupu session compact` (in the foreground,
//! or detached by the control plane) reads the whole session record, spends
//! minutes on a model call, then writes the compacted history back. While it
//! runs nothing else may change the session, so the compaction is recorded
//! first — a marker naming the process doing it — and every command that
//! would write the session (send, stop, archive, delete, another compact)
//! refuses while that process is alive.
//!
//! Files in the session's directory:
//! - `session.lock` — the advisory lock every marker read or write, and
//!   compaction's final record write, holds ([`SessionLock`]).
//! - `compaction.json` — the live [`Compaction`], absent when none runs. A
//!   marker whose process is gone is stale and is cleared on the next read.
//! - `compaction.result.json` — how the last compaction ended
//!   ([`CompactionResult`]).
//!
//! Liveness is injected (`alive(pid, start_stamp)`): callers pass
//! `rupu_launch::proc::process_alive`, which also refuses a pid reused by a
//! later process.

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

const LOCK: &str = "session.lock";
const MARKER: &str = "compaction.json";
const RESULT: &str = "compaction.result.json";

/// A compaction in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compaction {
    /// Minted by whoever starts it (the CP, or the CLI), passed to the
    /// compacting process so it adopts the marker rather than refusing it.
    pub id: String,
    /// The process doing it: the CP's own until the child has started, then
    /// the child's.
    pub pid: u32,
    /// `pid`'s start time, so a later process reusing the pid isn't taken
    /// for it.
    #[serde(default)]
    pub pid_start: Option<u64>,
    /// RFC 3339.
    pub started_at: String,
}

/// How a compaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The history was rewritten.
    Compacted,
    /// The history was already short enough; nothing written.
    NothingToCompact,
    /// The session changed while the summary was made; nothing written.
    Discarded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionResult {
    pub id: String,
    pub outcome: Outcome,
    pub detail: String,
    pub finished_at: String,
}

/// The session directory's exclusive lock, held until dropped.
pub struct SessionLock(File);

impl SessionLock {
    /// Block until `dir/session.lock` is ours. `dir` must exist.
    pub fn acquire(dir: &Path) -> io::Result<Self> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))?;
        f.lock_exclusive()?;
        Ok(Self(f))
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join(MARKER)
}

fn read_marker(dir: &Path) -> io::Result<Option<Compaction>> {
    match std::fs::read(marker_path(dir)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn write_atomic(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)
}

fn remove(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The live compaction of the session in `dir`, with its lock held by the
/// caller; a stale marker (process gone, or unparseable) is removed.
fn live_locked(
    dir: &Path,
    alive: &impl Fn(u32, Option<u64>) -> bool,
) -> io::Result<Option<Compaction>> {
    if !marker_path(dir).exists() {
        return Ok(None);
    }
    match read_marker(dir)? {
        Some(m) if alive(m.pid, m.pid_start) => Ok(Some(m)),
        _ => {
            remove(&marker_path(dir))?;
            Ok(None)
        }
    }
}

/// The live compaction of the session in `dir`, if any (takes the lock). A
/// missing `dir` has none.
pub fn live(
    dir: &Path,
    alive: impl Fn(u32, Option<u64>) -> bool,
) -> io::Result<Option<Compaction>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let _lock = SessionLock::acquire(dir)?;
    live_locked(dir, &alive)
}

/// What [`begin`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begin {
    /// The marker now names `pid` for compaction `id`.
    Recorded(Compaction),
    /// Another live compaction holds the session.
    Busy(Compaction),
}

/// Record compaction `id` as run by `pid` (started at `pid_start`). A live
/// marker of the same `id` is taken over (the CP hands its marker to the
/// child it starts); one of another id is [`Begin::Busy`]; a stale one is
/// replaced.
pub fn begin(
    dir: &Path,
    id: &str,
    pid: u32,
    pid_start: Option<u64>,
    alive: impl Fn(u32, Option<u64>) -> bool,
) -> io::Result<Begin> {
    let _lock = SessionLock::acquire(dir)?;
    let started_at = match live_locked(dir, &alive)? {
        Some(m) if m.id != id => return Ok(Begin::Busy(m)),
        Some(m) => m.started_at,
        None => chrono::Utc::now().to_rfc3339(),
    };
    let m = Compaction {
        id: id.to_string(),
        pid,
        pid_start,
        started_at,
    };
    write_atomic(&marker_path(dir), &m)?;
    Ok(Begin::Recorded(m))
}

/// Clear compaction `id`'s marker (another id's is left alone) and, when
/// `result` is given, record how it ended.
pub fn end(dir: &Path, id: &str, result: Option<(Outcome, String)>) -> io::Result<()> {
    let _lock = SessionLock::acquire(dir)?;
    if read_marker(dir)?.is_some_and(|m| m.id == id) {
        remove(&marker_path(dir))?;
    }
    if let Some((outcome, detail)) = result {
        write_atomic(
            &dir.join(RESULT),
            &CompactionResult {
                id: id.to_string(),
                outcome,
                detail,
                finished_at: chrono::Utc::now().to_rfc3339(),
            },
        )?;
    }
    Ok(())
}

/// How the session's last compaction ended, if one has.
pub fn last_result(dir: &Path) -> Option<CompactionResult> {
    let bytes = std::fs::read(dir.join(RESULT)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// A new compaction id.
pub fn new_id() -> String {
    format!("cmp_{}", ulid::Ulid::new())
}

/// The message a command refused while `m` runs gives.
pub fn busy_message(session_id: &str, m: &Compaction) -> String {
    format!(
        "session {session_id} is being compacted (pid {}, since {}); wait for it to finish",
        m.pid, m.started_at
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always(_: u32, _: Option<u64>) -> bool {
        true
    }
    fn never(_: u32, _: Option<u64>) -> bool {
        false
    }

    #[test]
    fn a_live_marker_refuses_another_compaction_and_is_handed_to_its_child() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let Begin::Recorded(first) = begin(dir, "cmp_a", 10, Some(1), always).unwrap() else {
            panic!("first begin is recorded");
        };
        assert!(matches!(
            begin(dir, "cmp_b", 11, None, always).unwrap(),
            Begin::Busy(m) if m == first
        ));
        // The child of `cmp_a` takes it over, keeping its start time.
        let Begin::Recorded(child) = begin(dir, "cmp_a", 12, Some(2), always).unwrap() else {
            panic!("the same id is taken over");
        };
        assert_eq!((child.pid, child.started_at), (12, first.started_at));
        assert_eq!(live(dir, always).unwrap().unwrap().pid, 12);
    }

    #[test]
    fn a_dead_marker_is_stale_and_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        begin(dir, "cmp_a", 10, None, always).unwrap();
        assert_eq!(live(dir, never).unwrap(), None);
        assert!(!dir.join(MARKER).exists());
        assert!(matches!(
            begin(dir, "cmp_b", 11, None, always).unwrap(),
            Begin::Recorded(_)
        ));
    }

    #[test]
    fn end_clears_only_its_own_marker_and_records_the_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        begin(dir, "cmp_a", 10, None, always).unwrap();
        end(dir, "cmp_other", None).unwrap();
        assert!(live(dir, always).unwrap().is_some());
        end(dir, "cmp_a", Some((Outcome::Discarded, "changed".into()))).unwrap();
        assert_eq!(live(dir, always).unwrap(), None);
        let r = last_result(dir).unwrap();
        assert_eq!((r.id.as_str(), r.outcome), ("cmp_a", Outcome::Discarded));
    }
}
