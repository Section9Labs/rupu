//! The one detached spawn of a `rupu` child process.
//!
//! Every launcher that starts a `rupu` child to outlive the moment (the CP's
//! local launchers, the node executor, agentiflow units and coordinators, the
//! session worker) goes through [`spawn_detached`]: the child leads its own
//! process group, so a Ctrl-C at the parent's terminal or the parent exiting
//! doesn't take it down, and a group signal reaches its grandchildren (bash
//! tools).
//!
//! Spec: `docs/superpowers/specs/2026-10-07-rupu-tool-and-launch-architecture/W6-process-spawn.md`.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::argv::RunArgv;

/// What to start and how.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// The `rupu` binary (the caller's `current_exe()`, normally).
    pub program: PathBuf,
    /// Arguments before the rendered argv, for a `program` that wraps
    /// `rupu`. Empty in production; tests run `/bin/sh -c <script> sh`,
    /// which hands the `rupu` argv to the script as `"$@"`.
    pub pre_args: Vec<OsString>,
    pub argv: RunArgv,
    /// The child's working directory; `None` inherits ours.
    pub cwd: Option<PathBuf>,
    /// Extra environment (`RUPU_HOME`, …), on top of [`RunArgv::env`] and
    /// ours. Never secrets the child can resolve itself.
    pub env: Vec<(OsString, OsString)>,
    pub stdio: SpawnStdio,
    /// Hand the [`Child`] back (a caller that polls `try_wait` or waits on
    /// it). Otherwise a thread reaps it when it exits, so a long-lived parent
    /// (`cp serve`) never collects zombies — which `kill(pid, 0)` would read
    /// as live runners.
    pub keep_child: bool,
}

/// Where the child's standard streams go. stdin is always null.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnStdio {
    /// stdout and stderr to `/dev/null`.
    Null,
    /// stdout to `/dev/null`, stderr to this file (created or truncated), so
    /// a child that dies at startup leaves its error behind.
    Log(PathBuf),
}

/// A started child.
#[derive(Debug)]
pub struct Spawned {
    pub pid: u32,
    /// The child's process group: its own pid, since it leads one.
    pub pgid: u32,
    /// Present iff [`SpawnSpec::keep_child`].
    pub child: Option<Child>,
}

impl SpawnSpec {
    /// Run `argv` with `program`, null stdio, our cwd and environment, the
    /// child not kept.
    pub fn new(program: impl Into<PathBuf>, argv: RunArgv) -> Self {
        Self {
            program: program.into(),
            pre_args: Vec::new(),
            argv,
            cwd: None,
            env: Vec::new(),
            stdio: SpawnStdio::Null,
            keep_child: false,
        }
    }

    /// The configured, not yet started command — for a caller that needs
    /// tokio's `Child` (`tokio::process::Command::from`). [`spawn_detached`]
    /// is the same command, started.
    pub fn command(&self) -> io::Result<Command> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.pre_args).args(self.argv.to_args());
        cmd.envs(self.argv.env()).envs(self.env.iter().cloned());
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::null());
        match &self.stdio {
            SpawnStdio::Null => cmd.stderr(Stdio::null()),
            SpawnStdio::Log(path) => cmd.stderr(Stdio::from(std::fs::File::create(path)?)),
        };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        Ok(cmd)
    }
}

/// Start `spec`'s child detached: its own process group, stdin null, stdout
/// null, stderr per [`SpawnSpec::stdio`].
pub fn spawn_detached(spec: SpawnSpec) -> io::Result<Spawned> {
    let child = spec.command()?.spawn()?;
    let pid = child.id();
    let child = if spec.keep_child {
        Some(child)
    } else {
        reap_in_background(child);
        None
    };
    Ok(Spawned {
        pid,
        pgid: pid,
        child,
    })
}

/// Wait for `child` on its own thread, so it never lingers as a zombie of a
/// long-lived parent. A thread that can't start leaves it unreaped (logged).
fn reap_in_background(mut child: Child) {
    let pid = child.id();
    let spawned = std::thread::Builder::new()
        .name(format!("reap-{pid}"))
        .spawn(move || match child.wait() {
            Ok(status) => tracing::debug!(pid, %status, "detached rupu child exited"),
            Err(e) => tracing::warn!(pid, error = %e, "waiting for a detached rupu child failed"),
        });
    if let Err(e) = spawned {
        tracing::warn!(pid, error = %e, "could not start a reaper thread; the detached child is left unreaped");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::argv::{AgentRun, CODENAME_ENV};

    /// `/bin/sh -c <script> sh <rupu argv…>`: the script sees the argv as `"$@"`.
    fn sh(script: &str, argv: RunArgv) -> SpawnSpec {
        let mut spec = SpawnSpec::new("/bin/sh", argv);
        spec.pre_args = vec!["-c".into(), script.into(), "sh".into()];
        spec
    }

    fn agent() -> RunArgv {
        let mut r = AgentRun::new("recon", "run_1");
        r.prompt = Some("-x y".into());
        RunArgv::Agent(r)
    }

    #[test]
    fn the_child_leads_its_own_group_and_gets_the_rendered_argv() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let mut spec = sh(
            r#"printf '%s\n' "$@" > "$OUT.tmp"; mv "$OUT.tmp" "$OUT"; exec sleep 30"#,
            agent(),
        );
        spec.env = vec![("OUT".into(), out.clone().into())];
        spec.keep_child = true;
        let mut spawned = spawn_detached(spec).unwrap();
        let mut child = spawned.child.take().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !out.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "child never wrote its argv"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let pid = rustix::process::Pid::from_raw(spawned.pid as i32).unwrap();
        let pgid = rustix::process::getpgid(Some(pid)).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(pgid.as_raw_nonzero().get() as u32, spawned.pid);
        assert_eq!(spawned.pgid, spawned.pid);
        let args = std::fs::read_to_string(&out).unwrap();
        assert_eq!(args, "run\nrecon\n--run-id\nrun_1\n--prompt=-x y\n");
    }

    #[test]
    fn cwd_env_and_the_argv_env_reach_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = AgentRun::new("recon", "run_1");
        r.codename = Some("jade-reef/numbat".into());
        r.codename_env = true;
        let mut spec = sh(
            &format!(r#"printf '%s|%s|%s' "$(pwd -P)" "$EXTRA" "${CODENAME_ENV}" > out"#),
            RunArgv::Agent(r),
        );
        spec.cwd = Some(dir.path().to_path_buf());
        spec.env = vec![("EXTRA".into(), "e".into())];
        spec.keep_child = true;
        let mut spawned = spawn_detached(spec).unwrap();
        spawned.child.as_mut().unwrap().wait().unwrap();
        let got = std::fs::read_to_string(dir.path().join("out")).unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        assert_eq!(got, format!("{}|e|jade-reef/numbat", cwd.display()));
    }

    #[test]
    fn log_stdio_keeps_stderr_and_an_unkept_child_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("launch.log");
        let mut spec = sh("echo boom >&2; echo quiet", agent());
        spec.stdio = SpawnStdio::Log(log.clone());
        let spawned = spawn_detached(spec).unwrap();
        assert!(spawned.child.is_none());
        // The reaper waits it out: the pid stops existing rather than
        // lingering as a zombie.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while rustix::process::test_kill_process(
            rustix::process::Pid::from_raw(spawned.pid as i32).unwrap(),
        )
        .is_ok()
        {
            assert!(std::time::Instant::now() < deadline, "child never reaped");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "boom\n");
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(spawn_detached(SpawnSpec::new(dir.path().join("nope"), agent())).is_err());
    }
}
