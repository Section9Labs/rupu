//! Cgroup-mode decision (spec §9.1). Pure: it maps plain inputs to an enum.
//! The cgroup filesystem operations come in a later, Linux-only task.

/// How to obtain a delegated capture cgroup root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Ask systemd for a transient scope (`systemd-run --user --scope`).
    Scope,
    /// Create a child of the current cgroup directly.
    Direct,
    /// Neither works; the reason is shown to the operator.
    Unavailable(String),
}

/// What the caller learned about its cgroup environment.
#[derive(Debug, Clone)]
pub struct CgroupEnv<'a> {
    /// The cgroup path from `/proc/self/cgroup` (e.g. `/user.slice/foo.service`).
    pub self_cgroup: &'a str,
    /// cgroup v2 is mounted.
    pub cgroup2_mounted: bool,
    /// The current cgroup directory is writable by us.
    pub writable: bool,
    /// The current cgroup is delegated to us (systemd `Delegate=`).
    pub delegated: bool,
    /// A systemd user manager is reachable.
    pub systemd_available: bool,
    /// Effective uid.
    pub uid: u32,
}

/// True for a bare service leaf (`...*.service`), which systemd manages and
/// will not let us create children under.
fn is_bare_service(cgroup: &str) -> bool {
    cgroup
        .rsplit('/')
        .next()
        .is_some_and(|leaf| leaf.ends_with(".service"))
}

/// Decide how to obtain a delegated capture root; first match wins.
///
/// 1. cgroup v2 must be mounted.
/// 2. [`Mode::Scope`] when the leaf is not a bare `*.service` and systemd is
///    reachable.
/// 3. [`Mode::Direct`] when the current cgroup is writable and delegated, or
///    is a cgroup-namespace root (`/`) and writable.
/// 4. Otherwise [`Mode::Unavailable`], naming the cgroup and why.
pub fn choose_mode(env: &CgroupEnv) -> Mode {
    if !env.cgroup2_mounted {
        return Mode::Unavailable("cgroup v2 not mounted".to_string());
    }
    if env.systemd_available && !is_bare_service(env.self_cgroup) {
        return Mode::Scope;
    }
    let ns_root = env.self_cgroup == "/";
    if env.writable && (env.delegated || ns_root) {
        return Mode::Direct;
    }
    let why = if !env.writable {
        "is not writable"
    } else {
        "is not delegated"
    };
    let hint = if env.systemd_available {
        "it is a bare service and has no delegation"
    } else {
        "no systemd user manager is reachable"
    };
    Mode::Unavailable(format!(
        "cgroup {} {why} by uid {} and {hint}",
        env.self_cgroup, env.uid
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(cg: &str) -> CgroupEnv<'_> {
        CgroupEnv {
            self_cgroup: cg,
            cgroup2_mounted: true,
            writable: false,
            delegated: false,
            systemd_available: false,
            uid: 1000,
        }
    }

    #[test]
    fn user_session_with_systemd_uses_scope() {
        let e = CgroupEnv {
            systemd_available: true,
            ..env("/user.slice/user-1000.slice/session-3.scope")
        };
        assert_eq!(choose_mode(&e), Mode::Scope);
    }

    #[test]
    fn bare_service_not_writable_with_systemd_is_unavailable() {
        let e = CgroupEnv {
            systemd_available: true,
            ..env("/system.slice/rupu.service")
        };
        assert!(matches!(choose_mode(&e), Mode::Unavailable(_)));
    }

    #[test]
    fn delegated_writable_service_uses_direct() {
        let e = CgroupEnv {
            writable: true,
            delegated: true,
            systemd_available: true,
            ..env("/system.slice/rupu.service")
        };
        assert_eq!(choose_mode(&e), Mode::Direct);
    }

    #[test]
    fn writable_namespace_root_uses_direct() {
        let e = CgroupEnv {
            writable: true,
            ..env("/")
        };
        assert_eq!(choose_mode(&e), Mode::Direct);
    }

    #[test]
    fn nothing_available_names_cgroup_and_reason() {
        match choose_mode(&env("/system.slice/x.service")) {
            Mode::Unavailable(r) => {
                assert!(r.contains("/system.slice/x.service"), "{r}");
                assert!(r.contains("not writable"), "{r}");
            }
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn cgroup2_missing_wins_over_everything() {
        let e = CgroupEnv {
            cgroup2_mounted: false,
            writable: true,
            delegated: true,
            systemd_available: true,
            ..env("/")
        };
        assert_eq!(
            choose_mode(&e),
            Mode::Unavailable("cgroup v2 not mounted".to_string())
        );
    }

    #[test]
    fn scope_beats_direct_when_both_apply() {
        let e = CgroupEnv {
            writable: true,
            delegated: true,
            systemd_available: true,
            ..env("/user.slice/user-1000.slice/session-3.scope")
        };
        assert_eq!(choose_mode(&e), Mode::Scope);
    }

    #[test]
    fn namespace_root_with_systemd_uses_scope() {
        let e = CgroupEnv {
            systemd_available: true,
            ..env("/")
        };
        assert_eq!(choose_mode(&e), Mode::Scope);
    }

    #[test]
    fn writable_but_not_delegated_is_unavailable() {
        let e = CgroupEnv {
            writable: true,
            ..env("/user.slice/foo.scope")
        };
        match choose_mode(&e) {
            Mode::Unavailable(r) => assert!(r.contains("not delegated"), "{r}"),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn delegated_but_not_writable_is_unavailable() {
        let e = CgroupEnv {
            delegated: true,
            ..env("/user.slice/foo.scope")
        };
        match choose_mode(&e) {
            Mode::Unavailable(r) => assert!(r.contains("not writable"), "{r}"),
            m => panic!("{m:?}"),
        }
    }

    #[test]
    fn cgroup2_not_mounted_is_unavailable() {
        let e = CgroupEnv {
            cgroup2_mounted: false,
            ..env("/user.slice/foo.scope")
        };
        match choose_mode(&e) {
            Mode::Unavailable(r) => assert!(r.contains("cgroup v2 not mounted"), "{r}"),
            m => panic!("{m:?}"),
        }
    }
}
