//! Process liveness + termination for launched process children and the
//! agentiflow coordinator (`kill(2)` via rustix).
//!
//! These mirror `rupu-orchestrator::runs::{pid_is_running, terminate_pid}`;
//! the semantics (EPERM = alive, never signal ourselves, pid 0 / out-of-range
//! = not a process) are kept identical.

#[cfg(not(unix))]
compile_error!("process child control is unix-only");

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

/// Whether `pid` has exited but has not yet been reaped (a zombie).
///
/// `kill(pid, 0)` succeeds for a zombie, so [`pid_is_running`] reports it alive
/// forever; this reads `/proc/<pid>/stat` on Linux to tell the two apart. A
/// zombie coordinator is as dead as a vanished one — its process has exited and
/// will never run again — so the orphan reaper must treat it as dead, or a run
/// whose coordinator was SIGKILLed is never reaped. A reaping init (systemd,
/// launchd) collects a zombie within moments, so this only bites where PID 1
/// reaps nothing: a container whose entry point is the coordinator's distant
/// ancestor (e.g. CI's `docker run … cargo test`). Always `false` off Linux,
/// where the supported inits reap.
#[cfg(target_os = "linux")]
pub fn is_zombie(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            // The state is the first field after the parenthesised command
            // name, which may itself contain `)`, so split on the LAST one.
            let (_, rest) = stat.rsplit_once(')')?;
            rest.trim_start().chars().next()
        })
        .is_some_and(|state| state == 'Z' || state == 'X')
}

/// Off Linux the supported inits reap, so an exited process does not linger as
/// a zombie worth distinguishing from a vanished one.
#[cfg(not(target_os = "linux"))]
pub fn is_zombie(_pid: u32) -> bool {
    false
}

/// Whether a coordinator with this pid is still running: it exists and is not
/// a zombie ([`is_zombie`]) — the liveness the orphan reaper uses, for any
/// caller that must not mistake an exited-but-unreaped coordinator for a
/// live one (the control plane's steering check).
pub fn coordinator_alive(pid: u32) -> bool {
    pid_is_running(pid) && !is_zombie(pid)
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

/// Whether `pid` leads its own process group (its pgid is its pid).
///
/// A detached coordinator (`rupu agentiflow run --detach`) does: it is spawned
/// with `process_group(0)`, so the children its lead's `bash` tool starts are in
/// its group. A foreground coordinator shares the group of the shell that
/// started it, which must never be signalled on its account. `false` for a pid
/// that is not running or not a real pid.
pub fn leads_own_group(pid: u32) -> bool {
    let Some(target) = rustix_pid(pid) else {
        return false;
    };
    rustix::process::getpgid(Some(target)).is_ok_and(|pgid| pgid == target)
}

/// Whether any process is still in group `pgid` (`kill(-pgid, 0)`). A
/// group's id is not reused as a new process's pid while it has members, so
/// a group whose leader exited is still the one the leader made.
pub fn group_has_members(pgid: u32) -> bool {
    rustix_pid(pgid).is_some_and(|g| rustix::process::test_kill_process_group(g).is_ok())
}

/// The processes in group `pgid`, read from `/proc/*/stat` (each one's
/// `pgrp`).
#[cfg(target_os = "linux")]
pub fn group_members(pgid: u32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| {
                    // `pgrp`, field 5, counted after the parenthesised
                    // command name (which may itself contain `)`).
                    let (_, rest) = stat.rsplit_once(')')?;
                    rest.split_whitespace().nth(5 - 3)?.parse::<u32>().ok()
                })
                == Some(pgid)
        })
        .collect()
}

/// The processes in group `pgid`, as `ps` lists them.
#[cfg(not(target_os = "linux"))]
pub fn group_members(pgid: u32) -> Vec<u32> {
    let Ok(out) = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,pgid="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(str::parse::<u32>);
            match (cols.next(), cols.next()) {
                (Some(Ok(pid)), Some(Ok(g))) if g == pgid => Some(pid),
                _ => None,
            }
        })
        .collect()
}

/// Whether `pid`'s environment holds exactly the entry `entry`
/// (`NAME=value`), read from `/proc/<pid>/environ`. `false` when it can't be
/// read.
#[cfg(target_os = "linux")]
pub fn carries_env(pid: u32, entry: &str) -> bool {
    std::fs::read(format!("/proc/{pid}/environ"))
        .is_ok_and(|env| env.split(|b| *b == 0).any(|e| e == entry.as_bytes()))
}

/// Whether `pid`'s environment holds exactly the entry `entry`
/// (`NAME=value`), as `ps -E` shows it (after the command's arguments). The
/// kernel hides the environment of Apple's own binaries (`/bin/sh`,
/// `/bin/sleep`, …) from other processes, so for those this is `false`.
#[cfg(not(target_os = "linux"))]
pub fn carries_env(pid: u32, entry: &str) -> bool {
    std::process::Command::new("/bin/ps")
        .args(["-wwE", "-o", "command=", "-p", &pid.to_string()])
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .any(|t| t == entry)
        })
}

/// Whether any process in group `pgid` carries `entry` in its environment
/// ([`carries_env`]): what ties a group whose leader is gone to the child
/// that made it.
pub fn group_carries_env(pgid: u32, entry: &str) -> bool {
    group_members(pgid)
        .into_iter()
        .any(|pid| carries_env(pid, entry))
}

/// When `pid` started, as the platform tells it: a process that later reuses
/// the pid has a different one. Comparable only on this host. `None` when
/// there is no such process (or the platform gives no start time).
#[cfg(target_os = "linux")]
pub fn start_stamp(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `starttime`, field 22, counted after the parenthesised command name
    // (which may itself contain `)`): field 3 is the first after it.
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(22 - 3)?.parse().ok()
}

#[cfg(target_os = "macos")]
pub fn start_stamp(pid: u32) -> Option<u64> {
    use libproc::bsd_info::BSDInfo;
    let info = libproc::proc_pid::pidinfo::<BSDInfo>(i32::try_from(pid).ok()?, 0).ok()?;
    info.pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn start_stamp(_pid: u32) -> Option<u64> {
    None
}

/// This process's own [`start_stamp`], read once.
pub fn own_start_stamp() -> Option<u64> {
    static STAMP: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *STAMP.get_or_init(|| start_stamp(std::process::id()))
}

/// Whether `pid` is still the process whose start was `recorded`: a pid
/// recorded with no start time can't be told apart, so it passes.
pub fn same_process(pid: u32, recorded: Option<u64>) -> bool {
    recorded.is_none_or(|r| start_stamp(pid) == Some(r))
}

/// Whether the process `pid`, recorded with start time `recorded`, is still
/// running: alive, not a zombie, and not a later process that reused the pid.
pub fn process_alive(pid: u32, recorded: Option<u64>) -> bool {
    pid_is_running(pid) && !is_zombie(pid) && same_process(pid, recorded)
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
    fn a_process_leads_its_own_group_only_when_spawned_to() {
        use std::os::unix::process::CommandExt;
        let sleeper = |own_group: bool| {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.args(["-c", "sleep 30"]);
            if own_group {
                cmd.process_group(0);
            }
            cmd.spawn().unwrap()
        };
        let (mut leader, mut follower) = (sleeper(true), sleeper(false));
        assert!(leads_own_group(leader.id()));
        // Spawned without `process_group(0)`: in this test process's group.
        assert!(!leads_own_group(follower.id()));
        leader.kill().unwrap();
        follower.kill().unwrap();
        leader.wait().unwrap();
        follower.wait().unwrap();
        // Gone, and never a real pid.
        assert!(!leads_own_group(leader.id()));
        assert!(!leads_own_group(0));
        assert!(!leads_own_group(u32::MAX));
    }

    #[test]
    fn a_start_stamp_names_one_process() {
        let me = start_stamp(std::process::id()).expect("this platform gives start times");
        assert_eq!(own_start_stamp(), Some(me));
        assert!(same_process(std::process::id(), Some(me)));
        assert!(!same_process(std::process::id(), Some(me + 1)));
        assert!(same_process(std::process::id(), None));
        assert_eq!(start_stamp(0x7fff_fffe), None);
        assert!(!process_alive(0x7fff_fffe, None));
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
