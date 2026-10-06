//! Process-tree walking via libproc.
//!
//! macOS has no cgroups, so a socket is attributed to a bash call by
//! ancestry: the socket's owning pid is walked up its parent chain until it
//! reaches the shell the call spawned.

use libproc::bsd_info::BSDInfo;
use libproc::proc_pid::pidinfo;

/// Walk `pid`'s parent chain (via libproc `pidinfo::<BSDInfo>`), up to `max`
/// hops, returning the first ancestor (inclusive of `pid` itself) for which
/// `is_owner` holds.
///
/// `None` when no such ancestor exists within `max` hops, when the chain
/// reaches `launchd` (pid <= 1), or when libproc cannot describe a process
/// (typically because it already exited — its parent link is then gone).
pub fn owning_ancestor(pid: u32, max: u32, is_owner: impl Fn(u32) -> bool) -> Option<u32> {
    let mut cur = pid;
    for _ in 0..=max {
        if cur <= 1 {
            return None;
        }
        if is_owner(cur) {
            return Some(cur);
        }
        let info = pidinfo::<BSDInfo>(cur as i32, 0).ok()?;
        let parent = info.pbi_ppid;
        if parent == cur {
            return None;
        }
        cur = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This process's own parent (the test harness's launcher) is found by
    /// walking up from this process, and `pid` itself is inclusive.
    #[test]
    #[ignore = "queries live libproc on macOS"]
    fn finds_own_parent_and_self() {
        let me = std::process::id();
        let parent = std::os::unix::process::parent_id();
        assert_eq!(owning_ancestor(me, 8, |p| p == parent), Some(parent));
        assert_eq!(owning_ancestor(me, 8, |p| p == me), Some(me));
        // Nothing matches: the walk ends at launchd instead of looping.
        assert_eq!(owning_ancestor(me, 32, |_| false), None);
        // Zero hops cannot reach the parent.
        assert_eq!(owning_ancestor(me, 0, |p| p == parent), None);
    }

    #[test]
    #[ignore = "queries live libproc on macOS"]
    fn dead_or_invalid_pid_is_none() {
        assert_eq!(owning_ancestor(u32::MAX >> 1, 8, |_| false), None);
    }
}
