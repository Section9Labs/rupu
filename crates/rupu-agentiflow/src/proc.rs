//! Process liveness + termination for fleet units (`kill(2)` via rustix).
//!
//! These mirror `rupu-orchestrator::runs::{pid_is_running, terminate_pid}`. They
//! are reimplemented here rather than imported because `rupu-agentiflow` does
//! not depend on `rupu-orchestrator`; the semantics (EPERM = alive, never
//! signal ourselves, pid 0 / out-of-range = not a process) are kept identical.

#[cfg(not(unix))]
compile_error!("agentiflow unit control is unix-only");

/// Convert a recorded `u32` pid into a rustix `Pid`.
///
/// `None` for anything that cannot be a real pid: 0 (which `kill(2)` reads as
/// "our whole process group", never what a recorded unit pid means) and
/// anything past `i32::MAX`.
fn rustix_pid(pid: u32) -> Option<rustix::process::Pid> {
    rustix::process::Pid::from_raw(i32::try_from(pid).ok()?)
}

/// Whether a process with this pid is alive (`kill(pid, 0)`).
///
/// `EPERM` means the process exists but belongs to someone else, so it is
/// alive. Only `ESRCH` — and any other error, conservatively — means "no such
/// process".
pub fn pid_is_running(pid: u32) -> bool {
    let Some(pid) = rustix_pid(pid) else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(rustix::io::Errno::PERM) => true,
        Err(_) => false,
    }
}

/// Send SIGTERM to `pid`. Returns whether the signal was delivered.
///
/// Never signals this process: a recorded pid equal to our own is always a
/// corrupt state (a bug that wrote the wrong pid, or an OS pid recycled onto
/// us), and honoring it would have the coordinator SIGTERM itself. Callers read
/// `false` as "could not terminate" and move on.
pub fn terminate_pid(pid: u32) -> bool {
    if pid == std::process::id() {
        tracing::warn!(
            pid,
            "refusing to terminate this process: a recorded unit pid matched our own"
        );
        return false;
    }
    let Some(pid) = rustix_pid(pid) else {
        return false;
    };
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).is_ok()
}

/// A process-group id as a rustix `Pid`, or `None` when signalling it could
/// only be a mistake.
///
/// Beyond [`rustix_pid`]'s rules (0 and out-of-range are not groups), this
/// refuses this process's pid and its own process group: `kill(-pgid, …)` on
/// the coordinator's group would take the coordinator (and its shell) down with
/// the unit.
fn group_target(pgid: u32) -> Option<rustix::process::Pid> {
    let target = rustix_pid(pgid)?;
    if pgid == std::process::id() || target == rustix::process::getpgrp() {
        tracing::warn!(
            pgid,
            "refusing to signal this process's own group: a recorded unit pgid matched our own"
        );
        return None;
    }
    Some(target)
}

/// Send SIGTERM to every process in group `pgid`. Returns whether the signal
/// was delivered.
///
/// A unit is spawned as its own process-group leader (`process_group(0)`), so
/// its pid *is* its pgid; signalling the group reaches the unit's own children
/// (a bash tool's long scan) as well as the unit, which a pid-only SIGTERM
/// would orphan. Only call this for a group whose leader is still unreaped: a
/// live or zombie leader keeps its pid, and so the pgid, reserved, whereas the
/// number of a fully dead group could be recycled.
pub fn terminate_group(pgid: u32) -> bool {
    let Some(target) = group_target(pgid) else {
        return false;
    };
    rustix::process::kill_process_group(target, rustix::process::Signal::TERM).is_ok()
}

/// Send SIGKILL to every process in group `pgid` (see [`terminate_group`]).
/// The escalation for a group that outlived its SIGTERM grace.
pub fn kill_group(pgid: u32) -> bool {
    let Some(target) = group_target(pgid) else {
        return false;
    };
    rustix::process::kill_process_group(target, rustix::process::Signal::KILL).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_reads_as_running() {
        assert!(pid_is_running(std::process::id()));
    }

    #[test]
    fn impossible_pids_are_not_running() {
        // Above i32::MAX can never name a process; 0 means "our process group"
        // to kill(2) and must never be treated as a live unit.
        assert!(!pid_is_running(u32::MAX));
        assert!(!pid_is_running(0));
        // An in-range pid that is overwhelmingly unlikely to be live.
        assert!(!pid_is_running(0x7fff_fffe));
    }

    #[test]
    fn terminate_refuses_own_pid_and_impossible_pids() {
        assert!(!terminate_pid(std::process::id()));
        assert!(!terminate_pid(0));
        assert!(!terminate_pid(u32::MAX));
    }

    #[test]
    fn group_signals_refuse_our_own_group_and_impossible_ids() {
        let own_pgrp = rustix::process::getpgrp().as_raw_nonzero().get() as u32;
        for f in [terminate_group, kill_group] {
            assert!(!f(std::process::id()));
            assert!(!f(own_pgrp), "never signal the coordinator's own group");
            assert!(!f(0));
            assert!(!f(u32::MAX));
        }
    }

    #[test]
    fn kill_group_reaches_a_unit_leader_in_its_own_group() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        assert!(kill_group(child.id()));
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(9));
    }
}
