//! Raise the process's open-file soft limit at startup.
//!
//! macOS hands every launchd/GUI-spawned process (rupu.app, the `cp serve`
//! it spawns, and every detached run subprocess those inherit into) a soft
//! `RLIMIT_NOFILE` of 256. A fan-out of concurrent agent units each holds
//! a handful of descriptors for its whole run (HTTP/1 TLS sockets, the
//! transcript writer, the netflow ledger), so a few dozen parallel units
//! exhaust 256 and every subsequent open fails with EMFILE — surfacing as
//! e.g. "agent `x` not found or failed to load: ... Too many open files".
//! The hard limit is far higher; raising soft → hard is what cargo/rustc do.

use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

/// macOS `OPEN_MAX`: the ceiling `setrlimit(RLIMIT_NOFILE)` accepts when
/// the hard limit is `RLIM_INFINITY` (setting the soft limit to infinity
/// is rejected with EINVAL there).
const OPEN_MAX_FALLBACK: u64 = 10_240;

/// Soft-limit values to try, most generous first. Empty when the current
/// soft limit is already unlimited or already at/above every candidate —
/// this only ever raises, never lowers.
pub(crate) fn candidates(current: Option<u64>, maximum: Option<u64>) -> Vec<u64> {
    let Some(current) = current else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(max) = maximum {
        out.push(max);
    }
    let fallback = maximum.map_or(OPEN_MAX_FALLBACK, |m| m.min(OPEN_MAX_FALLBACK));
    if !out.contains(&fallback) {
        out.push(fallback);
    }
    out.retain(|&c| c > current);
    out
}

/// Best-effort: raise the soft `RLIMIT_NOFILE` toward the hard limit.
/// Returns the resulting soft limit (`None` = unlimited). Never fails the
/// process — a refusal just leaves the inherited limit in place.
pub fn raise_nofile_limit() -> Option<u64> {
    let lim = getrlimit(Resource::Nofile);
    for target in candidates(lim.current, lim.maximum) {
        let new = Rlimit {
            current: Some(target),
            maximum: lim.maximum,
        };
        if setrlimit(Resource::Nofile, new).is_ok() {
            return Some(target);
        }
    }
    lim.current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raises_to_finite_hard_limit_with_open_max_fallback() {
        assert_eq!(candidates(Some(256), Some(65_536)), vec![65_536, 10_240]);
    }

    #[test]
    fn infinite_hard_limit_falls_back_to_open_max() {
        assert_eq!(candidates(Some(256), None), vec![10_240]);
    }

    #[test]
    fn small_finite_hard_limit_is_the_only_candidate() {
        assert_eq!(candidates(Some(256), Some(4_096)), vec![4_096]);
    }

    #[test]
    fn never_lowers() {
        assert!(candidates(Some(1_048_576), Some(1_048_576)).is_empty());
        assert!(candidates(Some(20_000), None).is_empty());
        assert!(candidates(None, None).is_empty());
    }

    #[test]
    fn raise_leaves_soft_limit_at_least_as_high() {
        let lim = getrlimit(Resource::Nofile);
        let before = lim.current;
        let after = raise_nofile_limit();
        // From a low inherited limit (e.g. macOS's 256) it must actually rise.
        if let Some(b) = before {
            let floor = lim
                .maximum
                .map_or(OPEN_MAX_FALLBACK, |m| m.min(OPEN_MAX_FALLBACK));
            assert!(
                after.is_some_and(|a| a >= floor.max(b)),
                "{before:?} -> {after:?}"
            );
        }
        match (before, after) {
            (Some(b), Some(a)) => assert!(a >= b),
            (None, None) => {}
            other => panic!("unexpected limits {other:?}"),
        }
        assert_eq!(getrlimit(Resource::Nofile).current, after);
    }
}
