//! The process backend: how a `kind: agent` or `kind: workflow` child runs —
//! a detached `rupu run` / `rupu workflow run` started from its typed argv
//! (W6), watched through the retained `Child`, its transcript and its run
//! record. [`ProcessBackend`] is the seam: [`Subprocess`] is the real one,
//! [`crate::testing::ScriptedProcesses`] a test double.
//!
//! Why the backend owns liveness itself. `rupu run` writes its `run.json`
//! only at the end, so the orchestrator's orphan reaper cannot see a child
//! mid-flight. This backend instead:
//!
//! * runs the child under the run id the launcher minted (`--run-id`), so the
//!   handle is known before the child starts;
//! * retains the [`Child`] and its pid, so liveness is `try_wait` and
//!   termination is a SIGTERM to the child's process group;
//! * reads started-evidence (a non-empty transcript) and the final outcome
//!   (`final_answer`) from an agent child's transcript, which the runner
//!   appends incrementally.
//!
//! A workflow child differs only in where that evidence lives. Its run id is
//! the *workflow run's* id, and its answer is not a final turn, so liveness
//! and the outcome come from the run store instead: `<global>/runs/<id>/
//! run.json` (the status, written at start with `runner_pid`) and
//! `step_results.jsonl` (the last non-skipped step's output is its answer).
//! `Child::try_wait` is still the one terminal signal for both kinds.
//!
//! # Zombies
//!
//! `kill(pid, 0)` succeeds for a zombie (an exited child nobody has reaped),
//! so [`pid_is_running`](crate::proc::pid_is_running) alone would report an
//! exited child as alive forever. [`ProcessBackend::poll`] therefore treats
//! the retained child's `try_wait` as the *only* terminal signal: it both
//! reports the exit status and reaps the process. Dropping the backend
//! terminates and reaps every child it still holds.
//!
//! # Process groups
//!
//! Each child is spawned as its own process-group leader
//! (`rupu_runtime::spawn::spawn_detached`), so its pid is its pgid. `rupu
//! run`'s SIGTERM handler exits at once without waiting for the run, so the
//! bash tool's own cleanup never fires; signalling only the child's pid would
//! leave an in-flight bash grandchild re-parented to init. Termination
//! therefore signals the whole group.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::proc::{kill_group, terminate_group};
use rupu_orchestrator::{RunStatus, RunStore, RunStoreError};
use rupu_runtime::argv::RunArgv;
use rupu_runtime::spawn::{spawn_detached, SpawnSpec};
use rupu_tools::ChildKind;

/// How long `Drop` waits for SIGTERMed children to exit before it SIGKILLs
/// them.
const DROP_GRACE: Duration = Duration::from_millis(1500);

/// One process child to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLaunch {
    /// The run id the launcher minted (`--run-id`).
    pub run_id: String,
    pub kind: ChildKind,
    pub argv: RunArgv,
    /// The child's working directory: its parent's workspace.
    pub cwd: Option<PathBuf>,
}

/// A process child's final answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcOutcome {
    pub output: String,
    pub success: bool,
}

/// Where a process child is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcState {
    /// Spawned, no evidence yet that its run started.
    Pending,
    /// Started and alive.
    Running,
    /// Ended with an answer (which may itself report `success: false`).
    Done(ProcOutcome),
    /// Ended without an answer (crashed, killed, vanished); carries why.
    Failed(String),
    /// Its process was killed by a signal before its run ended: nothing
    /// says the run itself failed.
    Died(String),
}

impl ProcState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ProcState::Done(_) | ProcState::Failed(_) | ProcState::Died(_)
        )
    }
}

/// A spawned process child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedProcess {
    /// It leads its own process group, so this is its pgid too.
    pub pid: u32,
    /// Its start time ([`crate::proc::start_stamp`]), read right after the
    /// spawn while the child is still unreaped. `None` when it can't be
    /// read: the process is then never signalled on its recorded pid.
    pub start: Option<u64>,
}

/// Why a process child is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Its parent paused or is being stopped: the run that continues the
    /// parent continues it, so its own run is left as it stands (a workflow
    /// run stays resumable).
    Interrupt,
    /// Nothing will continue it (a workflow run is recorded `Cancelled`).
    Abandon,
}

/// Starts, watches and stops process children. `spawn` never blocks on the
/// child; `poll` is cheap and idempotent.
pub trait ProcessBackend: Send + Sync {
    /// Start `launch`; return its process, or `None` when there is no
    /// process to point at.
    fn spawn(&self, launch: &ProcessLaunch) -> Result<Option<SpawnedProcess>, String>;
    /// The child's state now.
    fn poll(&self, run_id: &str) -> ProcState;
    /// SIGTERM the child's group; best-effort.
    fn terminate(&self, run_id: &str, why: Stop);
}

/// One tracked child.
struct Live {
    /// The retained child: `try_wait` is the liveness signal and reaps it.
    child: Child,
    /// Cached at spawn (`Child::id`). The child leads its own process group,
    /// so this is also its pgid: the target of group termination.
    pid: u32,
    kind: ChildKind,
    cwd: Option<PathBuf>,
    /// The terminal state once observed, so a later `poll` is idempotent and
    /// never re-reads the transcript.
    done: Option<ProcState>,
}

/// The real backend: each child is a detached `rupu` process. See the module
/// docs.
pub struct Subprocess {
    exe: PathBuf,
    /// Arguments before the child's argv ([`SpawnSpec::pre_args`]).
    pre_args: Vec<OsString>,
    global: PathBuf,
    live: Mutex<HashMap<String, Live>>,
}

/// The transcripts directory a `rupu run` started in `cwd` resolves,
/// mirroring `rupu-cli`'s `paths::project_root_for` + `transcripts_dir`: the
/// first ancestor of `cwd` (else ours) that has a `.rupu` directory is the
/// project root, and its `.rupu/transcripts` is used when that directory
/// exists; otherwise `<global>/transcripts`.
pub fn transcripts_dir(global: &Path, cwd: Option<&Path>) -> PathBuf {
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    let project_root = cwd.and_then(|c| c.canonicalize().ok()).and_then(|c| {
        c.ancestors()
            .find(|d| d.join(".rupu").is_dir())
            .map(Path::to_path_buf)
    });
    if let Some(root) = project_root {
        let local = root.join(".rupu").join("transcripts");
        if local.is_dir() {
            return local;
        }
    }
    global.join("transcripts")
}

impl Subprocess {
    /// A backend for the `rupu` binary at `exe`, whose children use `global`
    /// as `RUPU_HOME`.
    pub fn new(exe: impl Into<PathBuf>, global: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            pre_args: Vec::new(),
            global: global.into(),
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Put `args` before every child's argv: the test seam that runs the
    /// executable `/bin/sh` as `sh -c <script> sh <child argv…>`, where the
    /// script sees the child's argv as `"$@"`.
    pub fn with_pre_args<S: Into<OsString>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.pre_args = args.into_iter().map(Into::into).collect();
        self
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn cwd_of(&self, id: &str) -> Option<PathBuf> {
        self.lock().get(id).and_then(|l| l.cwd.clone())
    }

    /// Where this child's transcript is, if it exists yet: the directory
    /// `rupu run` resolves ([`transcripts_dir`]), then the global one.
    fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        let name = format!("{id}.jsonl");
        let cwd = self.cwd_of(id);
        [
            transcripts_dir(&self.global, cwd.as_deref()),
            self.global.join("transcripts"),
        ]
        .into_iter()
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file())
    }

    /// Started-evidence: the child's transcript exists and is non-empty.
    fn has_started(&self, id: &str) -> bool {
        self.transcript_path(id)
            .and_then(|p| std::fs::metadata(p).ok())
            .is_some_and(|m| m.len() > 0)
    }

    /// The terminal state for an agent child whose process exited with
    /// `exit`.
    ///
    /// A child that was signalled (terminated, OOM-killed, crashed) never
    /// produced a trustworthy answer: `Failed`. One that exited on its own is
    /// `Done` with its final answer from the transcript and
    /// `success = exit.success()` — a nonzero exit that still wrote an answer
    /// is a `Done { success: false }`, and one that wrote nothing is `Failed`.
    fn outcome_of(&self, id: &str, exit: ExitStatus) -> ProcState {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = exit.signal() {
            return ProcState::Died(format!("child killed by signal {sig}"));
        }
        let text = self.final_text(id);
        match (exit.success(), text) {
            (true, text) => ProcState::Done(ProcOutcome {
                output: text.unwrap_or_default(),
                success: true,
            }),
            (false, Some(output)) => ProcState::Done(ProcOutcome {
                output,
                success: false,
            }),
            (false, None) => ProcState::Failed(format!("child exited with {exit}")),
        }
    }

    /// The child's final answer text, from its transcript.
    fn final_text(&self, id: &str) -> Option<String> {
        rupu_transcript::final_answer(&self.transcript_path(id)?)
    }

    /// The run store a workflow child's `rupu workflow run` writes into
    /// (`RUPU_HOME` is `self.global`, see `spawn`).
    fn run_store(&self) -> RunStore {
        RunStore::new(self.global.join("runs"))
    }

    /// The state of a child whose process is still alive.
    ///
    /// Started-evidence is a non-empty transcript for an agent and a readable
    /// `run.json` for a workflow (any non-terminal status: the runner has
    /// begun, even if it is parked or between steps). A `run.json` that does
    /// not parse is read as not started: its writes are atomic renames, so
    /// that is only ever a path that does not exist yet.
    fn alive_state(&self, id: &str, kind: ChildKind) -> ProcState {
        let started = match kind {
            ChildKind::Workflow => self.run_store().load(id).is_ok(),
            ChildKind::Agent | ChildKind::Subagent => self.has_started(id),
        };
        if started {
            ProcState::Running
        } else {
            ProcState::Pending
        }
    }

    /// The terminal state for a workflow child whose process exited with
    /// `exit`: `run.json` is the verdict, the exit status only explains a run
    /// that died without writing one.
    fn workflow_outcome_of(&self, id: &str, exit: ExitStatus) -> ProcState {
        use std::os::unix::process::ExitStatusExt;
        let store = self.run_store();
        let record = match store.load(id) {
            Ok(record) => record,
            Err(RunStoreError::NotFound(_)) => {
                return ProcState::Failed("workflow exited before writing run.json".into());
            }
            Err(e) => return ProcState::Failed(format!("could not read run.json: {e}")),
        };
        match record.status {
            RunStatus::Completed => ProcState::Done(ProcOutcome {
                output: last_step_output(&store, id),
                success: true,
            }),
            RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled => ProcState::Failed(
                record
                    .error_message
                    .unwrap_or_else(|| format!("workflow run {}", record.status.as_str())),
            ),
            // Backstop: a gated workflow is refused when the child is
            // requested, so a child never legitimately parks.
            RunStatus::AwaitingApproval | RunStatus::Paused => {
                ProcState::Failed("workflow child parked at an approval gate".into())
            }
            // The process is gone but the run never reached a terminal state:
            // it was killed or crashed mid-run.
            RunStatus::Pending | RunStatus::Running => match exit.signal() {
                Some(sig) => ProcState::Died(format!(
                    "workflow killed by signal {sig} while {}",
                    record.status.as_str()
                )),
                None => ProcState::Failed(format!(
                    "workflow exited with {exit} while its run was still {}",
                    record.status.as_str()
                )),
            },
        }
    }
}

/// A completed workflow's answer: the output of the last step that was not
/// skipped, in append order. Empty when there is none (or the log is
/// unreadable: the run still completed).
pub(crate) fn last_step_output(store: &RunStore, id: &str) -> String {
    match store.read_step_results(id) {
        Ok(rows) => rows
            .into_iter()
            .rev()
            .find(|r| !r.skipped)
            .map(|r| r.output)
            .unwrap_or_default(),
        Err(e) => {
            tracing::warn!(run_id = id, error = %e, "could not read a completed workflow's step results");
            String::new()
        }
    }
}

impl ProcessBackend for Subprocess {
    fn spawn(&self, launch: &ProcessLaunch) -> Result<Option<SpawnedProcess>, String> {
        let mut spec = SpawnSpec::new(&self.exe, launch.argv.clone());
        spec.pre_args = self.pre_args.clone();
        // The child must write its records where `poll` looks for them; the
        // marker names its run in everything it spawns, for the sweep.
        spec.env = vec![
            ("RUPU_HOME".into(), self.global.clone().into()),
            (rupu_tools::RUN_ID_ENV.into(), launch.run_id.clone().into()),
        ];
        spec.cwd = launch.cwd.clone();
        // `try_wait` on the retained child is the liveness signal. The child
        // leads its own process group (`spawn_detached`), so a Ctrl-C at the
        // parent's terminal does not signal it directly: the launcher decides
        // its fate.
        spec.keep_child = true;
        let spawned = spawn_detached(spec).map_err(|e| format!("{}: {e}", self.exe.display()))?;
        let child = spawned
            .child
            .ok_or_else(|| "the spawned child was not kept".to_string())?;
        let pid = child.id();
        // Read before anything can reap it: a child that already exited is
        // still a zombie here.
        let start = crate::proc::start_stamp(pid);
        self.lock().insert(
            launch.run_id.clone(),
            Live {
                child,
                pid,
                kind: launch.kind,
                cwd: launch.cwd.clone(),
                done: None,
            },
        );
        Ok(Some(SpawnedProcess { pid, start }))
    }

    fn poll(&self, run_id: &str) -> ProcState {
        // Phase 1, under the lock and cheap: `try_wait` is the terminal signal
        // (and reaps the child), never `pid_is_running`, which a zombie passes.
        let (exit, kind) = {
            let mut live = self.lock();
            let Some(unit) = live.get_mut(run_id) else {
                return ProcState::Failed(format!("unknown child {run_id}"));
            };
            if let Some(done) = &unit.done {
                return done.clone();
            }
            let kind = unit.kind;
            match unit.child.try_wait() {
                Ok(Some(status)) => (status, kind),
                Ok(None) => {
                    drop(live);
                    return self.alive_state(run_id, kind);
                }
                Err(e) => {
                    let failed = ProcState::Failed(format!("could not poll the child: {e}"));
                    unit.done = Some(failed.clone());
                    return failed;
                }
            }
        };
        // Phase 2, off the lock: a transcript or step log can be large.
        let state = match kind {
            ChildKind::Workflow => self.workflow_outcome_of(run_id, exit),
            ChildKind::Agent | ChildKind::Subagent => self.outcome_of(run_id, exit),
        };
        if let Some(unit) = self.lock().get_mut(run_id) {
            unit.done = Some(state.clone());
        }
        state
    }

    fn terminate(&self, run_id: &str, why: Stop) {
        let mut live = self.lock();
        let Some(unit) = live.get_mut(run_id) else {
            return;
        };
        if unit.done.is_some() {
            return;
        }
        // A pid recorded for a child that has since exited may be recycled by
        // an unrelated process, so only a child `try_wait` still reports live
        // is signalled. A live (or unreaped) leader keeps its pid, and so its
        // pgid, reserved: the group below cannot be a recycled one.
        if matches!(unit.child.try_wait(), Ok(None)) {
            if unit.kind == ChildKind::Workflow && why == Stop::Abandon {
                // Mark the run `Cancelled` (and let the store signal the
                // runner) BEFORE the group dies, so `run.json` never keeps
                // saying `Running` for a child that is gone. Best-effort, and
                // under the same lock as the group signal below so the leader
                // cannot be reaped (its pid recycled) in between; the store's
                // own wait on `run.json` is bounded (2s).
                if let Err(e) =
                    self.run_store()
                        .cancel(run_id, "launcher", "parent ended", chrono::Utc::now())
                {
                    tracing::debug!(run_id, error = %e, "could not cancel a terminated workflow child's run");
                }
            }
            // The group also reaches a workflow's step grandchildren (bash
            // tools), which the store's pid-only SIGTERM would orphan.
            terminate_group(unit.pid);
        }
    }
}

impl Drop for Subprocess {
    /// Terminate and reap every child still held: a dropped backend must leave
    /// neither an orphaned child nor a zombie.
    fn drop(&mut self) {
        let live = self.live.get_mut().unwrap_or_else(|e| e.into_inner());
        let mut running: Vec<&mut Live> = live
            .values_mut()
            .filter_map(|u| {
                (u.done.is_none() && matches!(u.child.try_wait(), Ok(None))).then_some(u)
            })
            .collect();
        // SIGTERM every group first so the children wind down concurrently...
        for unit in &running {
            terminate_group(unit.pid);
        }
        // ...then reap the leaders against one shared deadline, SIGKILLing the
        // whole group of any straggler (`Child::kill` would hit only its pid).
        let deadline = Instant::now() + DROP_GRACE;
        while !running.is_empty() && Instant::now() < deadline {
            running.retain_mut(|u| matches!(u.child.try_wait(), Ok(None)));
            if !running.is_empty() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        for unit in running {
            if !kill_group(unit.pid) {
                let _ = unit.child.kill();
            }
            let _ = unit.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::pid_is_running;

    use rupu_runtime::argv::{AgentRun, WorkflowRef, WorkflowRun};

    fn mint() -> String {
        format!("run_{}", ulid::Ulid::new())
    }

    /// Spawn an agent child (in `cwd`) and return its run id.
    fn spawn_in(l: &Subprocess, cwd: Option<&Path>) -> String {
        let id = mint();
        l.spawn(&ProcessLaunch {
            run_id: id.clone(),
            kind: ChildKind::Agent,
            argv: RunArgv::Agent(AgentRun::new("recon", &id)),
            cwd: cwd.map(Path::to_path_buf),
        })
        .unwrap();
        id
    }

    fn spawn_agent(l: &Subprocess) -> String {
        spawn_in(l, None)
    }

    fn wf_launch(id: &str) -> ProcessLaunch {
        ProcessLaunch {
            run_id: id.to_string(),
            kind: ChildKind::Workflow,
            argv: RunArgv::Workflow(WorkflowRun::new(WorkflowRef::Name("web-assess".into()), id)),
            cwd: None,
        }
    }

    /// Spawn a workflow child and return its run id.
    fn spawn_wf(l: &Subprocess) -> String {
        let id = mint();
        l.spawn(&wf_launch(&id)).unwrap();
        id
    }

    #[test]
    fn a_missing_executable_is_a_spawn_error() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Subprocess::new(dir.path().join("no-such-rupu"), dir.path());
        assert!(launcher.spawn(&wf_launch("run_x")).is_err());
    }

    #[test]
    fn unknown_id_polls_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = Subprocess::new("/bin/sh", dir.path());
        assert!(matches!(launcher.poll("run_nope"), ProcState::Failed(_)));
    }

    /// A launcher that runs `/bin/sh -c <script> sh <unit argv…>` instead of
    /// `rupu run`: the script sees the unit's argv as `"$@"`.
    fn sh(global: &Path, script: &str) -> Subprocess {
        Subprocess::new("/bin/sh", global).with_pre_args(["-c", script, "sh"])
    }

    /// Write a transcript for `id` into `dir` (created if need be).
    fn write_transcript_in(dir: &Path, id: &str, answer: Option<&str>) {
        use rupu_transcript::{Event, JsonlWriter};
        std::fs::create_dir_all(dir).unwrap();
        let mut w = JsonlWriter::create(dir.join(format!("{}.jsonl", id))).unwrap();
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

    fn write_transcript(global: &Path, id: &str, answer: Option<&str>) {
        write_transcript_in(&global.join("transcripts"), id, answer);
    }

    fn wait_terminal(l: &Subprocess, id: &str) -> ProcState {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = l.poll(id);
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

    fn pid_of(l: &Subprocess, id: &str) -> u32 {
        l.lock().get(id).expect("tracked child").pid
    }

    /// Wait for `path` to hold a pid (a script's `echo $! > path`).
    fn read_pid_file(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|t| t.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "no pid written to {path:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait for a grandchild to be gone. A killed grandchild is re-parented to
    /// init, which normally reaps it a moment later — but not when PID 1 never
    /// reaps: in CI's `docker run … cargo test` container, `cargo` is PID 1 and
    /// the killed grandchild stays a zombie for good. A zombie has exited, so
    /// it counts as gone here. (Leaders are this process's own children; the
    /// tests assert those are reaped, not merely dead, and keep doing so.)
    fn wait_gone(pid: u32, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while pid_is_running(pid) && !is_zombie(pid) {
            assert!(Instant::now() < deadline, "{what} (pid {pid}) survived");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether `pid` has exited but not been reaped (`/proc/<pid>/stat` state
    /// `Z`, or `X` while it is being torn down). The state follows the
    /// parenthesised command name, which may itself contain `)`.
    #[cfg(target_os = "linux")]
    fn is_zombie(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                let (_, rest) = stat.rsplit_once(')')?;
                rest.trim_start().chars().next()
            })
            .is_some_and(|state| state == 'Z' || state == 'X')
    }

    /// Off Linux the tests run under a reaping init (launchd), so a killed
    /// grandchild's zombie window is brief and waiting it out is enough.
    #[cfg(not(target_os = "linux"))]
    fn is_zombie(_pid: u32) -> bool {
        false
    }

    /// A launcher whose unit is `sh -c 'sleep 30 & echo $! > <file>; wait'`: a
    /// leader with one in-flight grandchild, like `rupu run` mid bash-tool.
    fn spawn_with_grandchild(dir: &Path) -> (Subprocess, String, u32 /* grandchild pid */) {
        let pidfile = dir.join("grandchild.pid");
        let launcher = sh(
            dir,
            &format!("sleep 30 & echo $! > {}; wait", pidfile.display()),
        );
        let id = spawn_agent(&launcher);
        let grandchild = read_pid_file(&pidfile);
        assert!(pid_is_running(grandchild));
        (launcher, id, grandchild)
    }

    #[test]
    fn lifecycle_pending_then_running_then_terminal_and_stays_terminal() {
        let dir = tempfile::tempdir().unwrap();
        // Long enough that the child never exits on its own mid-test: every
        // state below is driven explicitly, not raced against its natural exit.
        let launcher = sh(dir.path(), "sleep 5");
        let id = spawn_agent(&launcher);
        let pid = pid_of(&launcher, &id);

        // Alive but no transcript yet: not started as far as evidence goes.
        assert_eq!(launcher.poll(&id), ProcState::Pending);
        assert!(pid_is_running(pid));

        // Started-evidence is a non-empty transcript.
        write_transcript(dir.path(), &id, Some("partial"));
        assert_eq!(launcher.poll(&id), ProcState::Running);

        launcher.terminate(&id, Stop::Abandon);
        let done = wait_terminal(&launcher, &id);
        assert!(matches!(done, ProcState::Died(_)), "{done:?}");
        // `try_wait` reaped it: asking again is still terminal, never an error.
        assert_eq!(launcher.poll(&id), done);
        assert_eq!(launcher.poll(&id), done);
        assert!(!pid_is_running(pid), "reaped, not a zombie");
    }

    #[test]
    fn a_clean_exit_with_an_answer_is_done_successful_and_stays_done() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = spawn_agent(&launcher);
        // Written before the first poll, so the outcome read cannot race it.
        write_transcript(dir.path(), &id, Some("all clear"));
        let done = wait_terminal(&launcher, &id);
        assert_eq!(
            done,
            ProcState::Done(ProcOutcome {
                output: "all clear".into(),
                success: true,
            })
        );
        assert_eq!(launcher.poll(&id), done);
    }

    #[test]
    fn an_exited_unit_is_terminal_and_reaped_not_left_a_zombie() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = spawn_agent(&launcher);
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
        assert!(launcher.poll(&id).is_terminal());
        assert!(
            !pid_is_running(pid),
            "reaped: the pid is gone, not a lingering zombie"
        );
    }

    #[test]
    fn a_failing_unit_that_wrote_an_answer_is_done_unsuccessful() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 0.2; exit 3");
        let id = spawn_agent(&launcher);
        write_transcript(dir.path(), &id, Some("blocked by waf"));
        assert_eq!(
            wait_terminal(&launcher, &id),
            ProcState::Done(ProcOutcome {
                output: "blocked by waf".into(),
                success: false,
            })
        );
    }

    #[test]
    fn a_failing_unit_with_no_answer_is_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 7");
        let id = spawn_agent(&launcher);
        assert!(matches!(
            wait_terminal(&launcher, &id),
            ProcState::Failed(_)
        ));
    }

    #[test]
    fn a_clean_exit_without_an_answer_is_done_with_empty_output() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = spawn_agent(&launcher);
        assert_eq!(
            wait_terminal(&launcher, &id),
            ProcState::Done(ProcOutcome {
                output: String::new(),
                success: true,
            })
        );
    }

    #[test]
    fn terminate_stops_a_running_unit_promptly_and_it_reads_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 5");
        let id = spawn_agent(&launcher);
        let pid = pid_of(&launcher, &id);
        assert!(pid_is_running(pid));

        let t0 = Instant::now();
        launcher.terminate(&id, Stop::Abandon);
        let status = wait_terminal(&launcher, &id);
        assert!(
            t0.elapsed() < Duration::from_secs(3),
            "SIGTERM must end a sleep 5 promptly, took {:?}",
            t0.elapsed()
        );
        assert!(
            matches!(&status, ProcState::Died(why) if why.contains("signal")),
            "a signalled unit has no trustworthy answer: {status:?}"
        );
        assert!(!pid_is_running(pid), "reaped, not a zombie");
        // Terminating a finished unit is a harmless no-op.
        launcher.terminate(&id, Stop::Abandon);
    }

    #[test]
    fn terminate_kills_the_whole_group_including_an_in_flight_grandchild() {
        let dir = tempfile::tempdir().unwrap();
        let (launcher, id, grandchild) = spawn_with_grandchild(dir.path());
        let leader = pid_of(&launcher, &id);

        launcher.terminate(&id, Stop::Abandon);

        // The leader dies and is reaped...
        let status = wait_terminal(&launcher, &id);
        assert!(matches!(status, ProcState::Died(_)), "{status:?}");
        assert!(!pid_is_running(leader));
        // ...and so does the grandchild: the group died, not just the leader.
        wait_gone(grandchild, "the unit's grandchild");
    }

    #[test]
    fn terminate_of_an_unknown_unit_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        sh(dir.path(), "true").terminate("run_nope", Stop::Abandon);
    }

    #[test]
    fn dropping_the_launcher_terminates_and_reaps_live_units() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "sleep 30");
        let a = spawn_agent(&launcher);
        let b = spawn_agent(&launcher);
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
    fn dropping_the_launcher_also_kills_a_units_grandchild() {
        let dir = tempfile::tempdir().unwrap();
        let (launcher, id, grandchild) = spawn_with_grandchild(dir.path());
        let leader = pid_of(&launcher, &id);

        drop(launcher);

        assert!(!pid_is_running(leader), "leader left running or a zombie");
        wait_gone(grandchild, "the unit's grandchild");
    }

    #[test]
    fn drop_escalates_to_sigkill_on_the_group_when_sigterm_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("gc.pid");
        // The leader AND its grandchild both ignore SIGTERM, so only the
        // SIGKILL escalation (to the group) can end them.
        let launcher = sh(
            dir.path(),
            &format!(
                "trap '' TERM; (trap '' TERM; sleep 30) & echo $! > {}; while :; do sleep 1; done",
                pidfile.display()
            ),
        );
        let id = spawn_agent(&launcher);
        let grandchild = read_pid_file(&pidfile);
        let leader = pid_of(&launcher, &id);

        drop(launcher);

        assert!(!pid_is_running(leader), "leader left running or a zombie");
        wait_gone(grandchild, "the SIGTERM-ignoring grandchild");
    }

    // ---- workflow units: run.json + step_results.jsonl ----

    /// A real `RunRecord` (deserialized through the store's own serde, so a
    /// field the store renames fails here, not silently in production).
    fn run_record(id: &str, status: &str, error: Option<&str>) -> rupu_orchestrator::RunRecord {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "workflow_name": "web-assess",
            "status": status,
            "inputs": {},
            "workspace_id": "ws_test",
            "workspace_path": "/work",
            "transcript_dir": "/work/transcripts",
            "started_at": "2026-10-06T00:00:00Z",
            "error_message": error,
        }))
        .expect("run.json shape")
    }

    /// A real `StepResultRecord`: `(step_id, output, skipped)`.
    fn step_row(run_id: &str, step: (&str, &str, bool)) -> rupu_orchestrator::StepResultRecord {
        serde_json::from_value(serde_json::json!({
            "step_id": step.0,
            "run_id": run_id,
            "transcript_path": "/work/transcripts/step.jsonl",
            "output": step.1,
            "success": true,
            "skipped": step.2,
            "rendered_prompt": "",
            "finished_at": "2026-10-06T00:00:01Z",
        }))
        .expect("step_results.jsonl row shape")
    }

    /// Stage `<root>/runs/<id>/{run.json,step_results.jsonl,...}` through the
    /// real `RunStore`, exactly as `rupu workflow run` would leave them.
    fn stage_run(
        root: &Path,
        id: &str,
        status: &str,
        error: Option<&str>,
        steps: &[(&str, &str, bool)],
    ) {
        let store = RunStore::new(root.join("runs"));
        store
            .create(
                run_record(id, status, error),
                "name: web-assess\nsteps: []\n",
            )
            .unwrap();
        for step in steps {
            store.append_step_result(id, &step_row(id, *step)).unwrap();
        }
    }

    /// A backend whose "rupu" is a script that, after `delay`, installs a run
    /// dir staged (with the real store types) for the child's own run id into
    /// `<global>/runs/<id>` by an atomic directory rename, then runs `after`:
    /// the fake `rupu workflow run … --run-id <id>`. [`Staged::spawn`] stages
    /// the run before it starts the child.
    fn workflow_launcher(
        global: &Path,
        status: &'static str,
        error: Option<&'static str>,
        steps: &'static [(&'static str, &'static str, bool)],
        delay: &str,
        after: &str,
    ) -> Staged {
        let global_dir = global.to_path_buf();
        let stage = global.join("stage");
        // The script reads the child's id off its own `--run-id`.
        let script = format!(
            "id=$(printf '%s\\n' \"$@\" | sed -n '/^--run-id$/{{n;p;}}'); \
             sleep {delay}; mkdir -p {g}/runs; \
             cp -R {s}/runs/$id {g}/runs/.$id.tmp && mv {g}/runs/.$id.tmp {g}/runs/$id; \
             {after}",
            g = global_dir.display(),
            s = stage.display(),
        );
        Staged {
            backend: Subprocess::new("/bin/sh", global).with_pre_args([
                "-c".to_string(),
                script,
                "sh".to_string(),
            ]),
            stage,
            status,
            error,
            steps,
        }
    }

    struct Staged {
        backend: Subprocess,
        stage: PathBuf,
        status: &'static str,
        error: Option<&'static str>,
        steps: &'static [(&'static str, &'static str, bool)],
    }

    impl Staged {
        fn spawn(&self) -> String {
            let id = mint();
            stage_run(&self.stage, &id, self.status, self.error, self.steps);
            self.backend.spawn(&wf_launch(&id)).unwrap();
            id
        }
    }

    impl std::ops::Deref for Staged {
        type Target = Subprocess;
        fn deref(&self) -> &Subprocess {
            &self.backend
        }
    }

    #[test]
    fn a_completed_workflow_is_done_with_the_last_non_skipped_steps_output() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = workflow_launcher(
            dir.path(),
            "completed",
            None,
            &[
                ("scan", "first", false),
                ("report", "the final report", false),
                ("cleanup", "skipped output", true),
            ],
            "0.3",
            "exit 0",
        );
        let id = launcher.spawn();

        // Alive, and `run.json` is not there yet: not started.
        assert_eq!(launcher.poll(&id), ProcState::Pending);

        let done = wait_terminal(&launcher, &id);
        assert_eq!(
            done,
            ProcState::Done(ProcOutcome {
                output: "the final report".into(),
                success: true,
            })
        );
        assert_eq!(launcher.poll(&id), done, "idempotent");
    }

    #[test]
    fn a_completed_workflow_with_no_steps_is_done_with_empty_output() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = workflow_launcher(dir.path(), "completed", None, &[], "0", "exit 0");
        let id = launcher.spawn();
        assert_eq!(
            wait_terminal(&launcher, &id),
            ProcState::Done(ProcOutcome {
                output: String::new(),
                success: true,
            })
        );
    }

    #[test]
    fn a_failed_workflow_is_failed_with_the_runs_error_message() {
        let dir = tempfile::tempdir().unwrap();
        // The process exits 0 on its own: only `run.json` says it failed.
        let launcher = workflow_launcher(
            dir.path(),
            "failed",
            Some("step `scan` failed: boom"),
            &[("scan", "partial", false)],
            "0",
            "exit 0",
        );
        let id = launcher.spawn();
        assert_eq!(
            wait_terminal(&launcher, &id),
            ProcState::Failed("step `scan` failed: boom".into())
        );
    }

    #[test]
    fn a_rejected_or_cancelled_workflow_without_a_message_is_failed_with_a_default() {
        for status in ["rejected", "cancelled"] {
            let dir = tempfile::tempdir().unwrap();
            let launcher = workflow_launcher(dir.path(), status, None, &[], "0", "exit 0");
            let id = launcher.spawn();
            let got = wait_terminal(&launcher, &id);
            assert_eq!(got, ProcState::Failed(format!("workflow run {status}")));
        }
    }

    #[test]
    fn a_workflow_parked_at_a_gate_is_failed_as_a_backstop() {
        for status in ["awaiting_approval", "paused"] {
            let dir = tempfile::tempdir().unwrap();
            let launcher = workflow_launcher(dir.path(), status, None, &[], "0", "exit 0");
            let id = launcher.spawn();
            assert_eq!(
                wait_terminal(&launcher, &id),
                ProcState::Failed("workflow child parked at an approval gate".into()),
                "{status}"
            );
        }
    }

    #[test]
    fn a_workflow_that_exits_without_writing_run_json_is_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = spawn_wf(&launcher);
        assert_eq!(
            wait_terminal(&launcher, &id),
            ProcState::Failed("workflow exited before writing run.json".into())
        );
    }

    #[test]
    fn a_workflow_killed_mid_run_died_not_stuck_running() {
        let dir = tempfile::tempdir().unwrap();
        // `run.json` says `running` and the runner dies by SIGKILL: the run
        // never reached a terminal state, so the unit must not read as alive.
        let launcher = workflow_launcher(dir.path(), "running", None, &[], "0", "kill -9 $$");
        let id = launcher.spawn();
        let got = wait_terminal(&launcher, &id);
        assert!(
            matches!(&got, ProcState::Died(why) if why.contains("signal 9") && why.contains("running")),
            "{got:?}"
        );
    }

    #[test]
    fn a_live_workflow_is_running_once_run_json_exists_and_is_reaped_when_it_exits() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = workflow_launcher(
            dir.path(),
            "running",
            None,
            &[("scan", "so far", false)],
            "0",
            "sleep 5",
        );
        let id = launcher.spawn();
        let pid = pid_of(&launcher, &id);

        // The copy lands a moment after spawn; poll until it has.
        let deadline = Instant::now() + Duration::from_secs(10);
        while launcher.poll(&id) != ProcState::Running {
            assert!(Instant::now() < deadline, "never became Running");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(pid_is_running(pid));

        launcher.terminate(&id, Stop::Abandon);
        assert!(wait_terminal(&launcher, &id).is_terminal());
        assert!(!pid_is_running(pid), "reaped, not a zombie");
    }

    #[test]
    fn terminating_a_workflow_unit_cancels_its_run_and_kills_the_whole_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("step.pid");
        // A runner with an in-flight step grandchild, like `rupu workflow run`
        // mid bash-tool.
        let launcher = workflow_launcher(
            dir.path(),
            "running",
            None,
            &[],
            "0",
            &format!("sleep 30 & echo $! > {}; wait", pidfile.display()),
        );
        let id = launcher.spawn();
        let grandchild = read_pid_file(&pidfile);
        let leader = pid_of(&launcher, &id);

        launcher.terminate(&id, Stop::Abandon);

        // `run.json` no longer says `running`: the store cancelled the run...
        let rec = RunStore::new(dir.path().join("runs")).load(&id).unwrap();
        assert_eq!(rec.status, RunStatus::Cancelled);
        assert_eq!(rec.error_message.as_deref(), Some("parent ended"));
        // ...the unit reads as failed with that reason...
        assert_eq!(
            wait_terminal(&launcher, &id),
            ProcState::Failed("parent ended".into())
        );
        assert!(!pid_is_running(leader), "reaped, not a zombie");
        // ...and the step's grandchild died with the group.
        wait_gone(grandchild, "the workflow unit's step grandchild");
        // Terminating a finished unit is a harmless no-op.
        launcher.terminate(&id, Stop::Abandon);
    }

    /// Stopped for an interrupt, a workflow unit's run is left as it stands
    /// (`running`, its runner gone) for `workflow resume` to continue, never
    /// recorded `Cancelled` as an operator's cancel is; its group still dies.
    #[test]
    fn interrupting_a_workflow_unit_leaves_its_run_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("step.pid");
        let launcher = workflow_launcher(
            dir.path(),
            "running",
            None,
            &[],
            "0",
            &format!("sleep 30 & echo $! > {}; wait", pidfile.display()),
        );
        let id = launcher.spawn();
        let grandchild = read_pid_file(&pidfile);

        launcher.terminate(&id, Stop::Interrupt);

        assert!(matches!(wait_terminal(&launcher, &id), ProcState::Died(_)));
        wait_gone(grandchild, "the workflow unit's step grandchild");
        let rec = RunStore::new(dir.path().join("runs")).load(&id).unwrap();
        assert_eq!(rec.status, RunStatus::Running);
    }

    /// A child's start time is read as it is spawned, so one that exits at
    /// once still has it.
    #[test]
    fn a_fast_exiting_childs_start_time_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = mint();
        let spawned = launcher
            .spawn(&ProcessLaunch {
                run_id: id.clone(),
                kind: ChildKind::Agent,
                argv: RunArgv::Agent(AgentRun::new("recon", &id)),
                cwd: None,
            })
            .unwrap()
            .unwrap();
        assert!(spawned.start.is_some(), "{spawned:?}");
        wait_terminal(&launcher, &id);
    }

    #[test]
    fn transcript_lookup_walks_workspace_ancestors_to_the_project_rupu() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        // A project whose `.rupu/transcripts` exists; the workspace is a deep
        // SUBDIR of it with no `.rupu` of its own — as in rupu-cli, the first
        // ancestor with a `.rupu` is the project root.
        let project = tmp.path().join("project");
        let transcripts = project.join(".rupu").join("transcripts");
        std::fs::create_dir_all(&transcripts).unwrap();
        let workspace = project.join("sub").join("deep");
        std::fs::create_dir_all(&workspace).unwrap();

        let launcher = sh(&global, "sleep 5");
        let id = spawn_in(&launcher, Some(&workspace));
        assert_eq!(launcher.poll(&id), ProcState::Pending);

        write_transcript_in(&transcripts, &id, Some("found it"));
        assert_eq!(
            launcher.poll(&id),
            ProcState::Running,
            "the transcript under the ancestor project is started-evidence"
        );
        assert_eq!(launcher.final_text(&id).as_deref(), Some("found it"));
    }

    #[test]
    fn transcript_lookup_uses_global_when_the_project_has_no_local_transcripts_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        std::fs::create_dir_all(&global).unwrap();
        // `.rupu` exists but `.rupu/transcripts` does not: rupu-cli falls back
        // to `<global>/transcripts` rather than walking further up.
        let project = tmp.path().join("project");
        std::fs::create_dir_all(project.join(".rupu")).unwrap();
        let workspace = project.join("sub");
        std::fs::create_dir_all(&workspace).unwrap();

        let launcher = sh(&global, "sleep 5");
        let id = spawn_in(&launcher, Some(&workspace));
        write_transcript(&global, &id, Some("global one"));
        assert_eq!(launcher.poll(&id), ProcState::Running);
        assert_eq!(launcher.final_text(&id).as_deref(), Some("global one"));
    }

    #[test]
    fn units_run_in_their_own_process_group_with_rupu_home_pointing_at_global() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("home.txt");
        let launcher = sh(
            dir.path(),
            &format!("echo \"$RUPU_HOME\" > {}; sleep 5", out.display()),
        );
        let id = mint();
        let spawned = launcher
            .spawn(&ProcessLaunch {
                run_id: id.clone(),
                kind: ChildKind::Agent,
                argv: RunArgv::Agent(AgentRun::new("recon", &id)),
                cwd: None,
            })
            .unwrap();
        let pid = pid_of(&launcher, &id);

        let child = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        let pgid = rustix::process::getpgid(Some(child)).unwrap();
        assert_eq!(pgid, child, "the unit leads its own process group");
        // The group id spawn reports (and the supervisor persists) is that group.
        assert_eq!(
            spawned.map(|p| p.pid),
            Some(pgid.as_raw_nonzero().get() as u32)
        );

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
