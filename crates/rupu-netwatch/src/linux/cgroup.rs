//! Cgroup-mode decision (spec §9.1). Pure: it maps plain inputs to an enum.
//! The Linux-only cgroup filesystem operations follow it in this file.

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

#[cfg(target_os = "linux")]
pub use fsops::{setup_root, CallCgroup, CaptureRoot};

/// Filesystem side of the capture cgroup (Linux only): discover the
/// environment, obtain a delegated root per [`choose_mode`], and hand out one
/// child cgroup per tool call.
#[cfg(target_os = "linux")]
mod fsops {
    use std::fs;
    use std::io;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use rustix::fs::{access, statfs, Access};
    use rustix::process::geteuid;

    use super::{choose_mode, CgroupEnv, Mode};

    /// Where the cgroup2 hierarchy is mounted.
    const CGROUP_MOUNT: &str = "/sys/fs/cgroup";
    /// `CGROUP2_SUPER_MAGIC`.
    const CGROUP2_SUPER_MAGIC: i64 = 0x6367_7270;
    /// How long to wait for systemd to move us into the new scope.
    const SCOPE_TIMEOUT: Duration = Duration::from_secs(2);
    /// Poll interval while waiting for the scope.
    const SCOPE_POLL: Duration = Duration::from_millis(5);

    /// A delegated cgroup directory under which per-call cgroups are made.
    #[derive(Debug, Clone)]
    pub struct CaptureRoot {
        root: PathBuf,
        /// The cgroup we created `root` under (Direct mode only). `Some`
        /// means we own `root` and must tear it down in
        /// [`shutdown`](CaptureRoot::shutdown); `None` (Scope mode) leaves
        /// teardown to systemd, which reaps the transient scope.
        parent: Option<PathBuf>,
    }

    /// The cgroup of one tool call.
    #[derive(Debug, Clone)]
    pub struct CallCgroup {
        /// The cgroup directory's inode, which is the kernel's cgroup id
        /// (`INET_DIAG_CGROUP_ID`).
        pub id: u64,
        /// The cgroup's `cgroup.procs` file.
        pub procs_path: PathBuf,
    }

    impl CaptureRoot {
        /// The delegated root directory.
        pub fn path(&self) -> &Path {
            &self.root
        }

        /// Create `call-<seq>` under the root.
        pub fn create_call(&self, seq: u64) -> io::Result<CallCgroup> {
            let dir = self.root.join(format!("call-{seq}"));
            fs::create_dir(&dir)?;
            let id = match fs::metadata(&dir) {
                Ok(m) => m.ino(),
                Err(e) => {
                    let _ = fs::remove_dir(&dir);
                    return Err(e);
                }
            };
            Ok(CallCgroup {
                id,
                procs_path: dir.join("cgroup.procs"),
            })
        }

        /// Tear down a Direct-mode root: move this process back into the
        /// cgroup the root was created under, then remove any leftover
        /// empty call cgroups, `supervisor` and the root itself. Scope mode
        /// is a no-op (systemd reaps the scope). Best-effort: every error is
        /// ignored, and a cgroup that still holds a process stays behind.
        pub fn shutdown(&self) {
            let Some(parent) = &self.parent else {
                return;
            };
            if fs::write(parent.join("cgroup.procs"), b"0").is_err() {
                // Still inside `supervisor`: it cannot be removed.
                return;
            }
            if let Ok(entries) = fs::read_dir(&self.root) {
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        let _ = fs::remove_dir(entry.path());
                    }
                }
            }
            let _ = fs::remove_dir(&self.root);
        }
    }

    impl CallCgroup {
        /// Shell text that moves the shell that runs it into this cgroup.
        /// Prefix it to the command line; failures are silenced so a command
        /// still runs (uncaptured) if the move is refused.
        ///
        /// The path is single-quoted with embedded `'` escaped as `'\''`, so
        /// a cgroup directory name can never break out of the quoting.
        pub fn shell_prefix(&self) -> String {
            let p = self.procs_path.to_string_lossy().replace('\'', "'\\''");
            format!("{{ printf '%d\\n' \"$$\" > '{p}'; }} 2>/dev/null")
        }

        /// Remove the cgroup directory if no process is left in it. Errors
        /// are ignored: a populated or already-removed cgroup is fine.
        pub fn remove_if_empty(&self) {
            let Some(dir) = self.procs_path.parent() else {
                return;
            };
            match fs::read_to_string(&self.procs_path) {
                Ok(procs) if procs.trim().is_empty() => {
                    let _ = fs::remove_dir(dir);
                }
                _ => {}
            }
        }
    }

    /// The `0::` (cgroup v2) path from `/proc/self/cgroup`, e.g. `/user.slice`.
    fn self_cgroup() -> Result<String, String> {
        let raw = fs::read_to_string("/proc/self/cgroup")
            .map_err(|e| format!("cannot read /proc/self/cgroup: {e}"))?;
        raw.lines()
            .find_map(|l| l.strip_prefix("0::"))
            .map(|p| p.trim().to_string())
            .ok_or_else(|| "no cgroup v2 entry in /proc/self/cgroup".to_string())
    }

    /// Filesystem path of a cgroup path from `/proc/self/cgroup`.
    fn cgroup_dir(cgroup: &str) -> PathBuf {
        let rel = cgroup.trim_start_matches('/');
        if rel.is_empty() {
            PathBuf::from(CGROUP_MOUNT)
        } else {
            Path::new(CGROUP_MOUNT).join(rel)
        }
    }

    // `f_type`'s width varies by target (`c_long`/`u32`); widen it to compare.
    #[allow(clippy::unnecessary_cast)]
    fn cgroup2_mounted() -> bool {
        statfs(CGROUP_MOUNT).is_ok_and(|s| s.f_type as i64 == CGROUP2_SUPER_MAGIC)
    }

    /// Run a command, returning its output when it ran and exited 0.
    fn run_ok(program: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(program).args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// `--user` for an unprivileged caller, nothing for root.
    fn bus_scope(uid: u32) -> &'static [&'static str] {
        if uid == 0 {
            &[]
        } else {
            &["--user"]
        }
    }

    /// Whether systemd reports the current cgroup's unit as delegated.
    fn delegated(self_cgroup: &str, uid: u32) -> bool {
        let leaf = self_cgroup.rsplit('/').next().unwrap_or("");
        if leaf.is_empty() {
            return false;
        }
        // A unit under `user@<uid>.service` belongs to the user manager.
        let user_manager = self_cgroup.contains("/user@");
        let mut args: Vec<&str> = Vec::new();
        if user_manager && uid != 0 {
            args.push("--user");
        }
        args.extend(["show", "-p", "Delegate", "--value", leaf]);
        run_ok("systemctl", &args).is_some_and(|v| v == "yes")
    }

    /// Whether the systemd bus we would call is reachable.
    fn systemd_available(uid: u32) -> bool {
        let mut args: Vec<&str> = bus_scope(uid).to_vec();
        args.push("status");
        run_ok("busctl", &args).is_some()
    }

    /// A short unique suffix: nanoseconds of wall clock.
    fn nonce() -> u32 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    }

    /// Ask systemd for a transient delegated scope containing this process
    /// and wait until `/proc/self/cgroup` shows it. Returns its cgroup path.
    fn start_scope(uid: u32) -> Result<String, String> {
        let pid = std::process::id();
        let unit = format!("rupu-netwatch-{pid}-{:08x}.scope", nonce());
        let pid_s = pid.to_string();
        let mut args: Vec<&str> = bus_scope(uid).to_vec();
        args.extend([
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "StartTransientUnit",
            "ssa(sv)a(sa(sv))",
            unit.as_str(),
            "fail",
            "2",
            "PIDs",
            "au",
            "1",
            pid_s.as_str(),
            "Delegate",
            "b",
            "true",
            "0",
        ]);
        let out = Command::new("busctl")
            .args(&args)
            .output()
            .map_err(|e| format!("cannot run busctl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "systemd refused the transient scope: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let deadline = Instant::now() + SCOPE_TIMEOUT;
        loop {
            if let Ok(cg) = self_cgroup() {
                if cg.rsplit('/').next() == Some(unit.as_str()) {
                    return Ok(cg);
                }
            }
            if Instant::now() >= deadline {
                return Err(format!("timed out waiting to enter scope {unit}"));
            }
            std::thread::sleep(SCOPE_POLL);
        }
    }

    /// Create `<root>/supervisor` and move this process into it, so the root
    /// holds no process itself. Never touches `cgroup.subtree_control`.
    fn enter_supervisor(root: &Path) -> Result<(), String> {
        let leaf = root.join("supervisor");
        fs::create_dir(&leaf).map_err(|e| format!("cannot create {}: {e}", leaf.display()))?;
        if let Err(e) = fs::write(leaf.join("cgroup.procs"), b"0") {
            let _ = fs::remove_dir(&leaf);
            return Err(format!("cannot move into {}: {e}", leaf.display()));
        }
        Ok(())
    }

    /// Obtain a delegated capture root (spec §9.1). `Err` carries the reason
    /// capture is unavailable; partially created directories are removed.
    ///
    /// This BLOCKS: it runs `systemctl`/`busctl` subprocesses and polls for
    /// up to 2s. Call it from a plain thread or `spawn_blocking`, never on an
    /// async runtime worker.
    pub fn setup_root() -> Result<CaptureRoot, String> {
        let uid = geteuid().as_raw();
        let own = self_cgroup()?;
        let dir = cgroup_dir(&own);
        let env = CgroupEnv {
            self_cgroup: &own,
            cgroup2_mounted: cgroup2_mounted(),
            writable: access(&dir, Access::WRITE_OK).is_ok(),
            delegated: delegated(&own, uid),
            systemd_available: systemd_available(uid),
            uid,
        };
        let (root, created_here) = match choose_mode(&env) {
            Mode::Unavailable(why) => return Err(why),
            Mode::Scope => (cgroup_dir(&start_scope(uid)?), false),
            Mode::Direct => {
                let child = dir.join(format!(
                    "rupu-netwatch-{}-{:08x}",
                    std::process::id(),
                    nonce()
                ));
                fs::create_dir(&child)
                    .map_err(|e| format!("cannot create {}: {e}", child.display()))?;
                (child, true)
            }
        };
        if let Err(e) = enter_supervisor(&root) {
            if created_here {
                let _ = fs::remove_dir(&root);
            }
            return Err(e);
        }
        Ok(CaptureRoot {
            root,
            parent: created_here.then_some(dir),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn shell_prefix_quotes_the_path_and_silences_errors() {
            let c = CallCgroup {
                id: 7,
                procs_path: PathBuf::from("/sys/fs/cgroup/x/call-1/cgroup.procs"),
            };
            assert_eq!(
                c.shell_prefix(),
                "{ printf '%d\\n' \"$$\" > '/sys/fs/cgroup/x/call-1/cgroup.procs'; } 2>/dev/null"
            );
        }

        #[test]
        fn shell_prefix_escapes_single_quotes_in_the_path() {
            let c = CallCgroup {
                id: 7,
                procs_path: PathBuf::from("/x/a'b/call-1/cgroup.procs"),
            };
            let p = c.shell_prefix();
            assert!(p.contains("'/x/a'\\''b/call-1/cgroup.procs'"), "{p}");
            // Removing the escape sequences leaves balanced quotes only.
            assert_eq!(p.replace("'\\''", "").matches('\'').count() % 2, 0, "{p}");
        }

        #[test]
        #[ignore = "needs a delegated cgroup v2 environment"]
        fn setup_root_then_create_and_remove_a_call_cgroup() {
            let root = setup_root().expect("setup_root");
            let call = root.create_call(1).expect("create_call");
            assert_ne!(call.id, 0);
            assert!(call.procs_path.exists(), "{:?}", call.procs_path);
            call.remove_if_empty();
            assert!(
                !call.procs_path.exists(),
                "empty call cgroup should be removed"
            );
        }
    }
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
