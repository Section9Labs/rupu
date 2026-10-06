//! The real [`UnitLauncher`]: each unit is a detached `rupu run` subprocess.
//!
//! Why the launcher owns liveness itself. `rupu run` writes its `run.json`
//! only at the end, with `runner_pid: None`, so the orchestrator's orphan
//! reaper cannot see a unit mid-flight, and the existing launchers discard the
//! `Child` and ignore the supplied run id. This launcher instead:
//!
//! * mints the unit's run id up front (`run_<ULID>`) and passes it as
//!   `--run-id`, so the handle is known before the unit starts;
//! * retains the [`Child`] and its pid, so liveness is `try_wait` and
//!   termination is SIGTERM-by-pid;
//! * reads started-evidence (a non-empty transcript) and the final outcome
//!   (`final_turn_text`) from the unit's transcript, which the runner appends
//!   incrementally.
//!
//! # Zombies
//!
//! `kill(pid, 0)` succeeds for a zombie (an exited child nobody has reaped),
//! so [`pid_is_running`](crate::pid_is_running) alone would report an exited
//! unit as alive forever. [`poll`](UnitLauncher::poll) therefore treats the
//! retained child's `try_wait` as the *only* terminal signal: it both reports
//! the exit status and reaps the process. Dropping the launcher terminates
//! and reaps every unit it still holds, so none is orphaned or left a zombie.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::proc::terminate_pid;
use crate::unit::{UnitError, UnitId, UnitLauncher, UnitOutcome, UnitSpec, UnitStatus};

/// How long `Drop` waits for SIGTERMed units to exit before it SIGKILLs them.
const DROP_GRACE: Duration = Duration::from_millis(1500);

/// Builds the argv (everything after the executable) for one unit.
///
/// Arguments: the unit's spec, the pre-minted run id, the agentiflow run dir.
/// The default is the real `rupu run …` argv ([`rupu_run_argv`]); tests swap it
/// to assert the argv or to run a trivial command (`/bin/sh -c 'sleep 0.3'`).
pub type ArgvBuilder = Box<dyn Fn(&UnitSpec, &str, &Path) -> Vec<OsString> + Send + Sync>;

/// The real argv for one unit:
/// `run <agent> --run-id <id> --mode bypass --prompt <p>
/// [--engagement-profile a,b] --fleet-run-dir <run_dir> --fleet-participant <p>`.
///
/// `--mode bypass` because a unit has no tty to answer an approval prompt.
/// `--engagement-profile` is omitted for an empty set so the unit takes the
/// default `code` path; a non-empty set is comma-joined (the flag's
/// delimiter), never dropped.
pub fn rupu_run_argv(spec: &UnitSpec, run_id: &str, run_dir: &Path) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        "run".into(),
        spec.agent.clone().into(),
        "--run-id".into(),
        run_id.into(),
        "--mode".into(),
        "bypass".into(),
        "--prompt".into(),
        spec.prompt.clone().into(),
    ];
    if !spec.engagement.is_empty() {
        argv.push("--engagement-profile".into());
        argv.push(spec.engagement.join(",").into());
    }
    argv.push("--fleet-run-dir".into());
    argv.push(run_dir.as_os_str().to_owned());
    argv.push("--fleet-participant".into());
    argv.push(spec.participant.clone().into());
    argv
}

/// One tracked unit.
struct Live {
    /// The retained child: `try_wait` is the liveness signal and reaps it.
    child: Child,
    /// Cached at spawn (`Child::id`); the SIGTERM target.
    pid: u32,
    /// The terminal status once observed, so a later `poll` is idempotent and
    /// never re-reads the transcript.
    done: Option<UnitStatus>,
}

/// Launches each unit as a detached `rupu run` subprocess. See the module docs.
pub struct SubprocessUnitLauncher {
    exe: PathBuf,
    global: PathBuf,
    workspace: Option<PathBuf>,
    argv: ArgvBuilder,
    live: Mutex<HashMap<String, Live>>,
}

impl SubprocessUnitLauncher {
    /// A launcher for the `rupu` binary at `exe`, whose units write their
    /// transcripts under `global` (`<global>/transcripts/<id>.jsonl`).
    pub fn new(exe: impl Into<PathBuf>, global: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            global: global.into(),
            workspace: None,
            argv: Box::new(rupu_run_argv),
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Run every unit with this working directory (the engagement workspace).
    pub fn with_workspace(mut self, workspace: impl Into<PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// Replace the argv builder (the test seam; see [`ArgvBuilder`]).
    pub fn with_argv(
        mut self,
        f: impl Fn(&UnitSpec, &str, &Path) -> Vec<OsString> + Send + Sync + 'static,
    ) -> Self {
        self.argv = Box::new(f);
        self
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Where this unit's transcript is, if it exists yet. `rupu run` writes it
    /// to the project's `.rupu/transcripts` when that directory exists in the
    /// workspace and to `<global>/transcripts` otherwise; check both.
    fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        let name = format!("{id}.jsonl");
        self.workspace
            .iter()
            .map(|w| w.join(".rupu").join("transcripts").join(&name))
            .chain(std::iter::once(self.global.join("transcripts").join(&name)))
            .find(|p| p.is_file())
    }

    /// Started-evidence: the unit's transcript exists and is non-empty.
    fn has_started(&self, id: &str) -> bool {
        self.transcript_path(id)
            .and_then(|p| std::fs::metadata(p).ok())
            .is_some_and(|m| m.len() > 0)
    }

    /// The terminal status for a unit whose process has exited with `exit`.
    ///
    /// A unit that was signalled (terminated, OOM-killed, crashed) never
    /// produced a trustworthy answer: `Failed`. One that exited on its own is
    /// `Done` with its final answer from the transcript and
    /// `success = exit.success()` — a nonzero exit that still wrote an answer
    /// is a `Done { success: false }`, and one that wrote nothing is `Failed`.
    fn outcome_of(&self, id: &str, exit: ExitStatus) -> UnitStatus {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = exit.signal() {
            return UnitStatus::Failed(format!("unit killed by signal {sig}"));
        }
        let text = self.final_text(id);
        match (exit.success(), text) {
            (true, text) => UnitStatus::Done(UnitOutcome {
                output: text.unwrap_or_default(),
                success: true,
            }),
            (false, Some(output)) => UnitStatus::Done(UnitOutcome {
                output,
                success: false,
            }),
            (false, None) => UnitStatus::Failed(format!("unit exited with {exit}")),
        }
    }

    /// The unit's final answer text, from its transcript.
    fn final_text(&self, id: &str) -> Option<String> {
        let path = self.transcript_path(id)?;
        let events = rupu_transcript::JsonlReader::iter(path).ok()?;
        rupu_transcript::final_turn_text(events.filter_map(Result::ok))
    }
}

impl UnitLauncher for SubprocessUnitLauncher {
    fn spawn(&self, spec: &UnitSpec, run_dir: &Path) -> Result<UnitId, UnitError> {
        let id = format!("run_{}", ulid::Ulid::new());
        let mut cmd = Command::new(&self.exe);
        cmd.args((self.argv)(spec, &id, run_dir))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // The unit must write its transcript where `poll` looks for it.
            .env("RUPU_HOME", &self.global);
        if let Some(workspace) = &self.workspace {
            cmd.current_dir(workspace);
        }
        // Its own process group, so a Ctrl-C at the coordinator's terminal
        // does not signal the unit directly: the supervisor decides its fate.
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let child = cmd
            .spawn()
            .map_err(|e| UnitError::Spawn(format!("{}: {e}", self.exe.display())))?;
        let pid = child.id();
        self.lock().insert(
            id.clone(),
            Live {
                child,
                pid,
                done: None,
            },
        );
        Ok(UnitId(id))
    }

    fn poll(&self, id: &UnitId, _run_dir: &Path) -> UnitStatus {
        // Phase 1, under the lock and cheap: `try_wait` is the terminal signal
        // (and reaps the child), never `pid_is_running`, which a zombie passes.
        let exit = {
            let mut live = self.lock();
            let Some(unit) = live.get_mut(&id.0) else {
                return UnitStatus::Failed(format!("unknown unit {}", id.0));
            };
            if let Some(done) = &unit.done {
                return done.clone();
            }
            match unit.child.try_wait() {
                Ok(Some(status)) => status,
                Ok(None) => {
                    drop(live);
                    return if self.has_started(&id.0) {
                        UnitStatus::Running
                    } else {
                        UnitStatus::Pending
                    };
                }
                Err(e) => {
                    let failed = UnitStatus::Failed(format!("could not poll unit: {e}"));
                    unit.done = Some(failed.clone());
                    return failed;
                }
            }
        };
        // Phase 2, off the lock: the transcript can be large.
        let status = self.outcome_of(&id.0, exit);
        if let Some(unit) = self.lock().get_mut(&id.0) {
            unit.done = Some(status.clone());
        }
        status
    }

    fn terminate(&self, id: &UnitId) {
        let mut live = self.lock();
        let Some(unit) = live.get_mut(&id.0) else {
            return;
        };
        if unit.done.is_some() {
            return;
        }
        // A pid recorded for a child that has since exited may be recycled by
        // an unrelated process, so only a child `try_wait` still reports live
        // is signalled.
        if matches!(unit.child.try_wait(), Ok(None)) {
            terminate_pid(unit.pid);
        }
    }
}

impl Drop for SubprocessUnitLauncher {
    /// Terminate and reap every unit still held: a dropped launcher must leave
    /// neither an orphaned unit nor a zombie.
    fn drop(&mut self) {
        let live = self.live.get_mut().unwrap_or_else(|e| e.into_inner());
        let mut running: Vec<&mut Live> = live
            .values_mut()
            .filter_map(|u| {
                (u.done.is_none() && matches!(u.child.try_wait(), Ok(None))).then_some(u)
            })
            .collect();
        // SIGTERM everything first so the units wind down concurrently...
        for unit in &running {
            terminate_pid(unit.pid);
        }
        // ...then reap them against one shared deadline, SIGKILLing stragglers.
        let deadline = Instant::now() + DROP_GRACE;
        while !running.is_empty() && Instant::now() < deadline {
            running.retain_mut(|u| matches!(u.child.try_wait(), Ok(None)));
            if !running.is_empty() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        for unit in running {
            let _ = unit.child.kill();
            let _ = unit.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::pid_is_running;

    fn spec(engagement: &[&str]) -> UnitSpec {
        UnitSpec {
            agent: "recon".into(),
            prompt: "scan the host".into(),
            engagement: engagement.iter().map(|s| s.to_string()).collect(),
            participant: "recon#1".into(),
        }
    }

    fn strs(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn value_after<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        let i = argv.iter().position(|a| a == flag)?;
        argv.get(i + 1).map(String::as_str)
    }

    #[test]
    fn default_argv_is_the_real_rupu_run_invocation() {
        let argv = strs(&rupu_run_argv(
            &spec(&["network", "web"]),
            "run_01ABC",
            Path::new("/g/agentiflows/af_1"),
        ));
        assert_eq!(argv[0], "run");
        assert_eq!(argv[1], "recon");
        assert_eq!(value_after(&argv, "--run-id"), Some("run_01ABC"));
        assert_eq!(value_after(&argv, "--mode"), Some("bypass"));
        assert_eq!(value_after(&argv, "--prompt"), Some("scan the host"));
        assert_eq!(
            value_after(&argv, "--engagement-profile"),
            Some("network,web")
        );
        assert_eq!(
            value_after(&argv, "--fleet-run-dir"),
            Some("/g/agentiflows/af_1")
        );
        assert_eq!(value_after(&argv, "--fleet-participant"), Some("recon#1"));
    }

    #[test]
    fn default_argv_omits_engagement_profile_when_the_set_is_empty() {
        let argv = strs(&rupu_run_argv(&spec(&[]), "run_1", Path::new("/rd")));
        assert!(
            !argv.iter().any(|a| a == "--engagement-profile"),
            "an empty set must take the default code path: {argv:?}"
        );
        assert_eq!(value_after(&argv, "--fleet-run-dir"), Some("/rd"));
    }

    #[test]
    fn spawn_passes_the_builder_argv_and_a_pre_minted_run_id() {
        let dir = tempfile::tempdir().unwrap();
        let seen: std::sync::Arc<Mutex<Vec<(String, PathBuf)>>> = Default::default();
        let sink = seen.clone();
        let launcher =
            SubprocessUnitLauncher::new("/bin/sh", dir.path()).with_argv(move |_, id, run_dir| {
                sink.lock()
                    .unwrap()
                    .push((id.to_string(), run_dir.to_path_buf()));
                vec!["-c".into(), "true".into()]
            });
        let id = launcher
            .spawn(&spec(&[]), Path::new("/the/run/dir"))
            .unwrap();
        assert!(id.0.starts_with("run_"), "pre-minted run id: {id:?}");
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![(id.0.clone(), PathBuf::from("/the/run/dir"))],
            "the builder is handed the id the handle carries"
        );
    }

    #[test]
    fn a_missing_executable_is_a_spawn_error() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = SubprocessUnitLauncher::new(dir.path().join("no-such-rupu"), dir.path());
        assert!(matches!(
            launcher.spawn(&spec(&[]), dir.path()),
            Err(UnitError::Spawn(_))
        ));
    }

    #[test]
    fn unknown_id_polls_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = SubprocessUnitLauncher::new("/bin/sh", dir.path());
        assert!(matches!(
            launcher.poll(&UnitId("run_nope".into()), dir.path()),
            UnitStatus::Failed(_)
        ));
    }

    /// A launcher that runs `/bin/sh -c <script>` instead of `rupu run`.
    fn sh(global: &Path, script: &'static str) -> SubprocessUnitLauncher {
        SubprocessUnitLauncher::new("/bin/sh", global)
            .with_argv(move |_, _, _| vec!["-c".into(), script.into()])
    }

    fn write_transcript(global: &Path, id: &UnitId, answer: Option<&str>) {
        use rupu_transcript::{Event, JsonlWriter};
        let dir = global.join("transcripts");
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = JsonlWriter::create(dir.join(format!("{}.jsonl", id.0))).unwrap();
        w.write(&Event::TurnStart { turn_idx: 0 }).unwrap();
        if let Some(text) = answer {
            w.write(&Event::AssistantMessage {
                content: text.into(),
                thinking: None,
            })
            .unwrap();
        }
        w.flush().unwrap();
    }

    fn wait_terminal(l: &SubprocessUnitLauncher, id: &UnitId, dir: &Path) -> UnitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = l.poll(id, dir);
            if s.is_terminal() {
                return s;
            }
            assert!(
                Instant::now() < deadline,
                "unit never became terminal: {s:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn pid_of(l: &SubprocessUnitLauncher, id: &UnitId) -> u32 {
        l.lock().get(&id.0).expect("tracked unit").pid
    }

    #[test]
    fn lifecycle_pending_then_running_then_terminal_and_stays_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 0.3");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let pid = pid_of(&launcher, &id);

        // Alive but no transcript yet: not started as far as evidence goes.
        assert_eq!(launcher.poll(&id, dir.path()), UnitStatus::Pending);
        assert!(pid_is_running(pid));

        // Started-evidence is a non-empty transcript.
        write_transcript(dir.path(), &id, Some("all clear"));
        assert_eq!(launcher.poll(&id, dir.path()), UnitStatus::Running);

        let done = wait_terminal(&launcher, &id, dir.path());
        assert_eq!(
            done,
            UnitStatus::Done(UnitOutcome {
                output: "all clear".into(),
                success: true,
            })
        );
        // `try_wait` reaped it: asking again is still terminal, never an error.
        assert_eq!(launcher.poll(&id, dir.path()), done);
        assert_eq!(launcher.poll(&id, dir.path()), done);
    }

    #[test]
    fn an_exited_unit_is_terminal_and_reaped_not_left_a_zombie() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let pid = pid_of(&launcher, &id);

        // Give it time to exit, and do NOT poll: nobody has reaped it, so it is
        // a zombie, which `kill(pid, 0)` still reports as alive. That is the
        // trap `poll` must not fall into by trusting `pid_is_running`.
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            pid_is_running(pid),
            "an unreaped child still answers kill 0"
        );

        // `poll` goes by `try_wait`, so it sees the exit and reaps the child.
        assert!(launcher.poll(&id, dir.path()).is_terminal());
        assert!(
            !pid_is_running(pid),
            "reaped: the pid is gone, not a lingering zombie"
        );
    }

    #[test]
    fn a_failing_unit_that_wrote_an_answer_is_done_unsuccessful() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 0.2; exit 3");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        write_transcript(dir.path(), &id, Some("blocked by waf"));
        assert_eq!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Done(UnitOutcome {
                output: "blocked by waf".into(),
                success: false,
            })
        );
    }

    #[test]
    fn a_failing_unit_with_no_answer_is_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 7");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        assert!(matches!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Failed(_)
        ));
    }

    #[test]
    fn a_clean_exit_without_an_answer_is_done_with_empty_output() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        assert_eq!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Done(UnitOutcome {
                output: String::new(),
                success: true,
            })
        );
    }

    #[test]
    fn terminate_stops_a_running_unit_promptly_and_it_reads_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 5");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let pid = pid_of(&launcher, &id);
        assert!(pid_is_running(pid));

        let t0 = Instant::now();
        launcher.terminate(&id);
        let status = wait_terminal(&launcher, &id, dir.path());
        assert!(
            t0.elapsed() < Duration::from_secs(3),
            "SIGTERM must end a sleep 5 promptly, took {:?}",
            t0.elapsed()
        );
        assert!(
            matches!(&status, UnitStatus::Failed(why) if why.contains("signal")),
            "a signalled unit has no trustworthy answer: {status:?}"
        );
        assert!(!pid_is_running(pid), "reaped, not a zombie");
        // Terminating a finished unit is a harmless no-op.
        launcher.terminate(&id);
    }

    #[test]
    fn terminate_of_an_unknown_unit_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        sh(dir.path(), "true").terminate(&UnitId("run_nope".into()));
    }

    #[test]
    fn dropping_the_launcher_terminates_and_reaps_live_units() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 30");
        let a = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let b = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let (pa, pb) = (pid_of(&launcher, &a), pid_of(&launcher, &b));
        assert!(pid_is_running(pa) && pid_is_running(pb));

        let t0 = Instant::now();
        drop(launcher);

        assert!(
            t0.elapsed() < Duration::from_secs(3),
            "drop must not wait out the sleep: {:?}",
            t0.elapsed()
        );
        // Reaped, not merely signalled: a zombie would still answer kill(pid, 0).
        assert!(!pid_is_running(pa), "unit a left running or a zombie");
        assert!(!pid_is_running(pb), "unit b left running or a zombie");
    }

    #[test]
    fn units_run_in_their_own_process_group_with_rupu_home_pointing_at_global() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("home.txt");
        let script: &'static str =
            Box::leak(format!("echo \"$RUPU_HOME\" > {}; sleep 5", out.display()).into_boxed_str());
        let launcher = sh(dir.path(), script);
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let pid = pid_of(&launcher, &id);

        let child = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        let pgid = rustix::process::getpgid(Some(child)).unwrap();
        assert_eq!(pgid, child, "the unit leads its own process group");

        let deadline = Instant::now() + Duration::from_secs(10);
        while !out.is_file() || std::fs::metadata(&out).unwrap().len() == 0 {
            assert!(Instant::now() < deadline, "unit never wrote its env probe");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            Path::new(std::fs::read_to_string(&out).unwrap().trim()),
            dir.path()
        );
    }
}
