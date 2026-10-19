//! Killing everything a `bash` call's shell started.
//!
//! The shell stays in its parent's process group on purpose: when a whole
//! launched unit or agentiflow coordinator is stopped, the group signal its
//! stopper sends (`rupu-launch`'s process backend, the agentiflow reaper) is
//! what reaches a bash call's descendants, since the stopped process dies by
//! the signal without running destructors. A call that ends early while its
//! process lives on — its timeout fires, or its future is dropped — has to stop
//! them itself, and SIGKILLing the shell alone leaves its children running as
//! orphans.
//!
//! So [`kill_tree`] freezes the tree first: SIGSTOP the shell, then each
//! descendant as it is found, until a pass finds nothing new (a stopped
//! process cannot fork) or the passes run out. Then it SIGKILLs every process
//! it found, deepest first, and the shell last. A descendant that was already
//! reparented to init (a backgrounded job whose shell exited) is not found.

use rustix::process::{kill_process, Pid, Signal};
use std::time::{Duration, Instant};

/// At most this many freeze passes...
const FREEZE_PASSES: usize = 8;
/// ...within at most this long; whatever was found by then is killed.
const FREEZE_BUDGET: Duration = Duration::from_secs(1);

/// A process found in the tree: its pid and when it started, so a pid that
/// is reused before it is signalled is never signalled.
struct Member {
    pid: u32,
    start: u64,
}

/// SIGKILL `root` and every descendant of it, frozen first so none escapes.
///
/// `root` must be an unreaped child of this process (so its pid cannot have
/// been reused). Synchronous and bounded ([`FREEZE_PASSES`],
/// [`FREEZE_BUDGET`]); needs no async runtime, so it can run in a `Drop`.
pub(crate) fn kill_tree(root: u32) {
    let Some(root_pid) = rustix_pid(root) else {
        return;
    };
    let _ = kill_process(root_pid, Signal::STOP);
    let deadline = Instant::now() + FREEZE_BUDGET;
    let mut found: Vec<Member> = Vec::new();
    for _ in 0..FREEZE_PASSES {
        let parents: Vec<u32> = std::iter::once(root)
            .chain(found.iter().map(|m| m.pid))
            .collect();
        let new: Vec<Member> = children_of(&parents)
            .into_iter()
            .filter(|c| !found.iter().any(|m| m.pid == c.pid))
            .collect();
        if new.is_empty() {
            break;
        }
        for m in &new {
            signal(m, root, &found, &new, Signal::STOP);
        }
        found.extend(new);
        if Instant::now() >= deadline {
            break;
        }
    }
    // Deepest first: each process is killed while its (stopped) parent still
    // holds it, so its parent link still vouches for it.
    for (i, m) in found.iter().enumerate().rev() {
        signal(m, root, &found[..i], &[], Signal::KILL);
    }
    let _ = kill_process(root_pid, Signal::KILL);
}

/// Signal `m` only while it is still the process that was found: same start
/// time, and its parent still `root` or another member.
fn signal(m: &Member, root: u32, found: &[Member], new: &[Member], sig: Signal) {
    let Some((ppid, start)) = identity(m.pid) else {
        return;
    };
    let parent_is_ours = ppid == root || found.iter().chain(new).any(|p| p.pid == ppid);
    if start != m.start || !parent_is_ours {
        return;
    }
    if let Some(pid) = rustix_pid(m.pid) {
        let _ = kill_process(pid, sig);
    }
}

/// `None` for anything that cannot name one process: 0 (which `kill(2)`
/// reads as our whole group), 1 (init) and anything past `i32::MAX`.
fn rustix_pid(pid: u32) -> Option<Pid> {
    if pid <= 1 {
        return None;
    }
    Pid::from_raw(i32::try_from(pid).ok()?)
}

/// `pid`'s parent and start time, or `None` when it can't be read (it is gone).
#[cfg(target_os = "linux")]
fn identity(pid: u32) -> Option<(u32, u64)> {
    parse_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// `ppid` (field 4) and `starttime` (field 22) of a `/proc/<pid>/stat` line,
/// counted after the parenthesised command name (which may contain `)`).
#[cfg(target_os = "linux")]
fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let (_, rest) = stat.rsplit_once(')')?;
    let mut fields = rest.split_whitespace();
    let ppid = fields.nth(4 - 3)?.parse().ok()?;
    let start = fields.nth(22 - 4 - 1)?.parse().ok()?;
    Some((ppid, start))
}

/// Every process whose parent is one of `parents`, read in one `/proc` scan.
#[cfg(target_os = "linux")]
fn children_of(parents: &[u32]) -> Vec<Member> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| {
            let (ppid, start) = identity(pid)?;
            parents.contains(&ppid).then_some(Member { pid, start })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn identity(pid: u32) -> Option<(u32, u64)> {
    use libproc::bsd_info::BSDInfo;
    let info = libproc::proc_pid::pidinfo::<BSDInfo>(i32::try_from(pid).ok()?, 0).ok()?;
    let start = info
        .pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)?;
    Some((info.pbi_ppid, start))
}

#[cfg(target_os = "macos")]
fn children_of(parents: &[u32]) -> Vec<Member> {
    use libproc::processes::{pids_by_type, ProcFilter};
    parents
        .iter()
        .flat_map(|&ppid| {
            pids_by_type(ProcFilter::ByParentProcess { ppid })
                .unwrap_or_default()
                .into_iter()
                .filter_map(move |pid| {
                    let (actual, start) = identity(pid)?;
                    (actual == ppid && pid > 1).then_some(Member { pid, start })
                })
        })
        .collect()
}

/// Elsewhere there is no process table to read: only the shell is killed.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn identity(_pid: u32) -> Option<(u32, u64)> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn children_of(_parents: &[u32]) -> Vec<Member> {
    Vec::new()
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_are_counted_after_the_command_name() {
        let stat = "42 (a) b) S 7 42 42 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 98765 1 2";
        assert_eq!(parse_stat(stat), Some((7, 98765)));
    }
}
