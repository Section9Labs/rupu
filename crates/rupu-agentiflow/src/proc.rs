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
}
