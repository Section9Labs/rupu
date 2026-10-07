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
//!   termination is a SIGTERM to the unit's process group;
//! * reads started-evidence (a non-empty transcript) and the final outcome
//!   (`final_turn_text`) from the unit's transcript, which the runner appends
//!   incrementally.
//!
//! A [`UnitKind::Workflow`] unit differs only in where that evidence lives. Its
//! run id is the *workflow run's* id, not a transcript's (each step gets a
//! fresh transcript), and its answer is not a final turn, so liveness and the
//! outcome come from the run store instead: `<global>/runs/<id>/run.json` (the
//! status, written at start with `runner_pid`) and `step_results.jsonl` (the
//! last non-skipped step's output is the unit's answer). `Child::try_wait` is
//! still the one terminal signal for both kinds.
//!
//! # Zombies
//!
//! `kill(pid, 0)` succeeds for a zombie (an exited child nobody has reaped),
//! so [`pid_is_running`](crate::pid_is_running) alone would report an exited
//! unit as alive forever. [`poll`](UnitLauncher::poll) therefore treats the
//! retained child's `try_wait` as the *only* terminal signal: it both reports
//! the exit status and reaps the process. Dropping the launcher terminates
//! and reaps every unit it still holds, so none is orphaned or left a zombie.
//!
//! # Process groups
//!
//! Each unit is spawned as its own process-group leader (`process_group(0)`),
//! so its pid is its pgid. `rupu run`'s SIGTERM handler exits at once without
//! waiting for the run, so the bash tool's own cleanup never fires; signalling
//! only the unit's pid would leave an in-flight bash grandchild (a long scan)
//! re-parented to init. Termination therefore signals the whole group.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::proc::{kill_group, terminate_group};
use crate::unit::{
    Spawned, UnitError, UnitId, UnitKind, UnitLauncher, UnitOutcome, UnitSpec, UnitStatus,
};
use rupu_orchestrator::{RunStatus, RunStore, RunStoreError};

/// How long `Drop` waits for SIGTERMed units to exit before it SIGKILLs them.
const DROP_GRACE: Duration = Duration::from_millis(1500);

/// Builds the argv (everything after the executable) for one unit.
///
/// Arguments: the unit's spec, the pre-minted run id, the agentiflow run dir.
/// The default is the real `rupu run …` argv ([`rupu_run_argv`]); tests swap it
/// to assert the argv or to run a trivial command (`/bin/sh -c 'sleep 0.3'`).
pub type ArgvBuilder = Box<dyn Fn(&UnitSpec, &str, &Path) -> Vec<OsString> + Send + Sync>;

/// The real argv for one unit, by [`UnitSpec::kind`].
///
/// An [`UnitKind::Agent`] unit:
/// `run <agent> --run-id <id> --mode bypass --prompt=<p>
/// [--engagement-profile a,b] --fleet-run-dir <run_dir> --fleet-participant <p>`.
///
/// A [`UnitKind::Workflow`] unit ([`rupu_workflow_argv`]):
/// `workflow run <name> --run-id <id> --mode bypass --plain [--input k=v]…
/// [--engagement-profile a,b] --fleet-run-dir <run_dir> --fleet-participant <p>`,
/// where `<name>` is `spec.agent` (the "thing to run") and there is no
/// `--prompt` (a workflow takes `--input`s instead).
///
/// A generated workflow unit (`spec.workflow_file` is `Some`) instead runs
/// `workflow run --file <path> --run-id <id> …` with no positional name.
///
/// `--mode bypass` because a unit has no tty to answer an approval prompt.
/// The prompt is ONE `--prompt=<p>` argument, not `--prompt <p>`: clap reads a
/// separate value that starts with `-` (`"- enumerate hosts"`, `"--foo"`) as a
/// flag and rejects it, whereas the `=` form takes the value verbatim.
/// `--engagement-profile` is omitted for an empty set so the unit takes the
/// default `code` path; a non-empty set is comma-joined (the flag's
/// delimiter), never dropped.
pub fn rupu_run_argv(spec: &UnitSpec, run_id: &str, run_dir: &Path) -> Vec<OsString> {
    match spec.kind {
        UnitKind::Agent => rupu_agent_argv(spec, run_id, run_dir),
        UnitKind::Workflow => rupu_workflow_argv(spec, run_id, run_dir),
    }
}

/// The agent unit's argv (see [`rupu_run_argv`]).
fn rupu_agent_argv(spec: &UnitSpec, run_id: &str, run_dir: &Path) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        "run".into(),
        spec.agent.clone().into(),
        "--run-id".into(),
        run_id.into(),
        "--mode".into(),
        "bypass".into(),
        format!("--prompt={}", spec.prompt).into(),
    ];
    push_unit_tail(&mut argv, spec, run_dir);
    argv
}

/// The workflow unit's argv (see [`rupu_run_argv`]). `--plain` because the unit
/// is detached with no tty: the plain printer, never the live graph view.
pub fn rupu_workflow_argv(spec: &UnitSpec, run_id: &str, run_dir: &Path) -> Vec<OsString> {
    // A generated workflow unit runs a materialized file (`--file <path>`, no
    // positional name); every other workflow unit runs a catalog id.
    let mut argv: Vec<OsString> = match &spec.workflow_file {
        Some(file) => vec![
            "workflow".into(),
            "run".into(),
            "--file".into(),
            file.as_os_str().to_owned(),
        ],
        None => vec!["workflow".into(), "run".into(), spec.agent.clone().into()],
    };
    argv.extend([
        "--run-id".into(),
        run_id.into(),
        "--mode".into(),
        "bypass".into(),
        "--plain".into(),
    ]);
    for (k, v) in &spec.inputs {
        argv.push("--input".into());
        argv.push(format!("{k}={v}").into());
    }
    push_unit_tail(&mut argv, spec, run_dir);
    argv
}

/// The flags every unit kind ends with: the engagement profile (when any) and
/// the fleet binding.
fn push_unit_tail(argv: &mut Vec<OsString>, spec: &UnitSpec, run_dir: &Path) {
    if !spec.engagement.is_empty() {
        argv.push("--engagement-profile".into());
        argv.push(spec.engagement.join(",").into());
    }
    argv.push("--fleet-run-dir".into());
    argv.push(run_dir.as_os_str().to_owned());
    argv.push("--fleet-participant".into());
    argv.push(spec.participant.clone().into());
}

/// One tracked unit.
struct Live {
    /// The retained child: `try_wait` is the liveness signal and reaps it.
    child: Child,
    /// Cached at spawn (`Child::id`). The unit leads its own process group, so
    /// this is also its pgid: the target of group termination.
    pid: u32,
    /// What the unit runs, which picks where `poll` reads its evidence and how
    /// `terminate` stops it.
    kind: UnitKind,
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

    /// The transcripts directory a unit's `rupu run` resolves, mirroring
    /// `rupu-cli`'s `paths::project_root_for` + `transcripts_dir`: the first
    /// ancestor of the unit's cwd (the workspace, else ours) that has a `.rupu`
    /// directory is the project root, and its `.rupu/transcripts` is used when
    /// that directory exists; otherwise `<global>/transcripts`.
    fn unit_transcripts_dir(&self) -> PathBuf {
        let cwd = self
            .workspace
            .clone()
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
        self.global.join("transcripts")
    }

    /// Where this unit's transcript is, if it exists yet: the directory
    /// `rupu run` resolves ([`Self::unit_transcripts_dir`]), then the global one.
    fn transcript_path(&self, id: &str) -> Option<PathBuf> {
        let name = format!("{id}.jsonl");
        [self.unit_transcripts_dir(), self.global.join("transcripts")]
            .into_iter()
            .map(|dir| dir.join(&name))
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

    /// The run store a workflow unit's `rupu workflow run` writes into
    /// (`RUPU_HOME` is `self.global`, see `spawn`).
    fn run_store(&self) -> RunStore {
        RunStore::new(self.global.join("runs"))
    }

    /// The status of a unit whose process is still alive.
    ///
    /// Started-evidence is a non-empty transcript for an agent and a readable
    /// `run.json` for a workflow (any non-terminal status: the runner has
    /// begun, even if it is parked or between steps). A `run.json` that does
    /// not parse is read as not started: its writes are atomic renames, so
    /// that is only ever a path that does not exist yet.
    fn alive_status(&self, id: &str, kind: UnitKind) -> UnitStatus {
        let started = match kind {
            UnitKind::Agent => self.has_started(id),
            UnitKind::Workflow => self.run_store().load(id).is_ok(),
        };
        if started {
            UnitStatus::Running
        } else {
            UnitStatus::Pending
        }
    }

    /// The terminal status for a workflow unit whose process has exited with
    /// `exit`: `run.json` is the verdict, the exit status only explains a run
    /// that died without writing one.
    fn workflow_outcome_of(&self, id: &str, exit: ExitStatus) -> UnitStatus {
        use std::os::unix::process::ExitStatusExt;
        let store = self.run_store();
        let record = match store.load(id) {
            Ok(record) => record,
            Err(RunStoreError::NotFound(_)) => {
                return UnitStatus::Failed("workflow exited before writing run.json".into());
            }
            Err(e) => return UnitStatus::Failed(format!("could not read run.json: {e}")),
        };
        match record.status {
            RunStatus::Completed => UnitStatus::Done(UnitOutcome {
                output: self.last_step_output(&store, id),
                success: true,
            }),
            RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled => UnitStatus::Failed(
                record
                    .error_message
                    .unwrap_or_else(|| format!("workflow run {}", record.status.as_str())),
            ),
            // Backstop: a gated workflow is refused when the unit is
            // requested, so a unit never legitimately parks.
            RunStatus::AwaitingApproval | RunStatus::Paused => {
                UnitStatus::Failed("workflow unit parked at an approval gate".into())
            }
            // The process is gone but the run never reached a terminal state:
            // it was killed or crashed mid-run.
            RunStatus::Pending | RunStatus::Running => UnitStatus::Failed(match exit.signal() {
                Some(sig) => format!(
                    "workflow killed by signal {sig} while {}",
                    record.status.as_str()
                ),
                None => format!(
                    "workflow exited with {exit} while its run was still {}",
                    record.status.as_str()
                ),
            }),
        }
    }

    /// A completed workflow's answer: the output of the last step that was not
    /// skipped, in append order. Empty when there is none (or the log is
    /// unreadable: the run still completed).
    fn last_step_output(&self, store: &RunStore, id: &str) -> String {
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
}

impl UnitLauncher for SubprocessUnitLauncher {
    fn spawn(&self, spec: &UnitSpec, run_dir: &Path) -> Result<Spawned, UnitError> {
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
                kind: spec.kind,
                done: None,
            },
        );
        // `process_group(0)` made the child its own group leader, so its pid is
        // the pgid the supervisor persists for out-of-process group signalling.
        Ok(Spawned {
            id: UnitId(id),
            pgid: Some(pid),
        })
    }

    fn poll(&self, id: &UnitId, _run_dir: &Path) -> UnitStatus {
        // Phase 1, under the lock and cheap: `try_wait` is the terminal signal
        // (and reaps the child), never `pid_is_running`, which a zombie passes.
        let (exit, kind) = {
            let mut live = self.lock();
            let Some(unit) = live.get_mut(&id.0) else {
                return UnitStatus::Failed(format!("unknown unit {}", id.0));
            };
            if let Some(done) = &unit.done {
                return done.clone();
            }
            let kind = unit.kind;
            match unit.child.try_wait() {
                Ok(Some(status)) => (status, kind),
                Ok(None) => {
                    drop(live);
                    return self.alive_status(&id.0, kind);
                }
                Err(e) => {
                    let failed = UnitStatus::Failed(format!("could not poll unit: {e}"));
                    unit.done = Some(failed.clone());
                    return failed;
                }
            }
        };
        // Phase 2, off the lock: a transcript or step log can be large.
        let status = match kind {
            UnitKind::Agent => self.outcome_of(&id.0, exit),
            UnitKind::Workflow => self.workflow_outcome_of(&id.0, exit),
        };
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
        // is signalled. A live (or unreaped) leader keeps its pid, and so its
        // pgid, reserved: the group below cannot be a recycled one. The pid IS
        // the pgid (`process_group(0)`), and signalling the group also reaches
        // the unit's grandchildren, which a pid-only SIGTERM would orphan.
        if matches!(unit.child.try_wait(), Ok(None)) {
            if unit.kind == UnitKind::Workflow {
                // Mark the run `Cancelled` (and let the store signal the
                // runner) BEFORE the group dies, so `run.json` never keeps
                // saying `Running` for a unit that is gone. Best-effort, and
                // under the same lock as the group signal below so the leader
                // cannot be reaped (its pid recycled) in between; the store's
                // own wait on `run.json` is bounded (2s).
                if let Err(e) = self.run_store().cancel(
                    &id.0,
                    "agentiflow",
                    "fleet terminate",
                    chrono::Utc::now(),
                ) {
                    tracing::debug!(run_id = %id.0, error = %e, "could not cancel a terminated workflow unit's run");
                }
            }
            // The group also reaches a workflow's step grandchildren (bash
            // tools), which the store's pid-only SIGTERM would orphan.
            terminate_group(unit.pid);
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
        // SIGTERM every group first so the units wind down concurrently...
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

    fn spec(engagement: &[&str]) -> UnitSpec {
        UnitSpec {
            agent: "recon".into(),
            prompt: "scan the host".into(),
            engagement: engagement.iter().map(|s| s.to_string()).collect(),
            participant: "recon#1".into(),
            kind: UnitKind::Agent,
            inputs: vec![],
            workflow_file: None,
        }
    }

    fn workflow_spec() -> UnitSpec {
        UnitSpec {
            agent: "web-assess".into(),
            prompt: String::new(),
            engagement: vec!["web".into()],
            participant: "web-assess#1".into(),
            kind: UnitKind::Workflow,
            inputs: vec![("target".into(), "x".into())],
            workflow_file: None,
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
        assert!(
            argv.iter().any(|a| a == "--prompt=scan the host"),
            "the prompt is one --prompt=<p> argument: {argv:?}"
        );
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
    fn an_agent_unit_argv_is_exactly_the_3b2_shape() {
        // Regression guard: the workflow branch must not move the agent argv.
        let argv = strs(&rupu_run_argv(
            &spec(&["network", "web"]),
            "run_01ABC",
            Path::new("/rd"),
        ));
        assert_eq!(
            argv,
            [
                "run",
                "recon",
                "--run-id",
                "run_01ABC",
                "--mode",
                "bypass",
                "--prompt=scan the host",
                "--engagement-profile",
                "network,web",
                "--fleet-run-dir",
                "/rd",
                "--fleet-participant",
                "recon#1",
            ]
        );
    }

    #[test]
    fn a_workflow_unit_argv_is_workflow_run_with_inputs_and_no_prompt() {
        let argv = strs(&rupu_run_argv(
            &workflow_spec(),
            "run_01ABC",
            Path::new("/rd"),
        ));
        assert_eq!(
            argv,
            [
                "workflow",
                "run",
                "web-assess",
                "--run-id",
                "run_01ABC",
                "--mode",
                "bypass",
                "--plain",
                "--input",
                "target=x",
                "--engagement-profile",
                "web",
                "--fleet-run-dir",
                "/rd",
                "--fleet-participant",
                "web-assess#1",
            ]
        );
        assert!(
            !argv.iter().any(|a| a.starts_with("--prompt")),
            "a workflow has no prompt: {argv:?}"
        );
    }

    #[test]
    fn workflow_argv_uses_file_when_set() {
        let spec = UnitSpec {
            agent: "gen-abc".into(),
            prompt: String::new(),
            engagement: vec!["network".into()],
            participant: "gen-abc#1".into(),
            kind: UnitKind::Workflow,
            inputs: vec![("k".into(), "v".into())],
            workflow_file: Some(PathBuf::from("/runs/x/generated/gen-abc.yaml")),
        };
        let argv = strs(&rupu_workflow_argv(&spec, "run-1", Path::new("/runs/x")));
        assert_eq!(
            argv,
            [
                "workflow",
                "run",
                "--file",
                "/runs/x/generated/gen-abc.yaml",
                "--run-id",
                "run-1",
                "--mode",
                "bypass",
                "--plain",
                "--input",
                "k=v",
                "--engagement-profile",
                "network",
                "--fleet-run-dir",
                "/runs/x",
                "--fleet-participant",
                "gen-abc#1",
            ]
        );
        assert!(
            argv.windows(2)
                .any(|w| w[0] == "--file" && w[1] == "/runs/x/generated/gen-abc.yaml"),
            "{argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "gen-abc"),
            "no positional name when --file is set: {argv:?}"
        );
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--engagement-profile" && w[1] == "network"));
        assert!(argv.windows(2).any(|w| w[0] == "--input" && w[1] == "k=v"));
    }

    #[test]
    fn a_workflow_unit_emits_one_input_flag_per_entry_and_omits_empty_extras() {
        let mut s = workflow_spec();
        s.engagement.clear();
        s.inputs = vec![
            ("target".into(), "x".into()),
            ("depth".into(), "a=b c".into()),
        ];
        let argv = strs(&rupu_run_argv(&s, "run_1", Path::new("/rd")));
        let inputs: Vec<&str> = argv
            .iter()
            .enumerate()
            .filter(|(_, a)| *a == "--input")
            .map(|(i, _)| argv[i + 1].as_str())
            .collect();
        assert_eq!(
            inputs,
            ["target=x", "depth=a=b c"],
            "in order, value kept whole"
        );
        assert!(!argv.iter().any(|a| a == "--engagement-profile"));

        s.inputs.clear();
        let argv = strs(&rupu_run_argv(&s, "run_1", Path::new("/rd")));
        assert!(!argv.iter().any(|a| a == "--input"), "{argv:?}");
    }

    #[test]
    fn a_prompt_starting_with_a_dash_is_one_attached_argument() {
        // `--prompt - enumerate` / `--prompt --foo` make clap read the value as
        // a flag (exit 2); `--prompt=<p>` takes it verbatim.
        for prompt in ["- enumerate the hosts", "--foo", "-x"] {
            let mut s = spec(&[]);
            s.prompt = prompt.into();
            let argv = strs(&rupu_run_argv(&s, "run_1", Path::new("/rd")));
            let expected = format!("--prompt={prompt}");
            assert!(
                argv.contains(&expected),
                "expected {expected:?} as a single arg in {argv:?}"
            );
            assert!(
                !argv.iter().any(|a| a == "--prompt"),
                "a bare --prompt would detach the value: {argv:?}"
            );
            assert!(
                !argv.iter().any(|a| a == prompt),
                "the prompt must never stand alone as its own arg: {argv:?}"
            );
        }
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
            .unwrap()
            .id;
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
    fn sh(global: &Path, script: &str) -> SubprocessUnitLauncher {
        let script = script.to_string();
        SubprocessUnitLauncher::new("/bin/sh", global)
            .with_argv(move |_, _, _| vec!["-c".into(), script.clone().into()])
    }

    /// Write a transcript for `id` into `dir` (created if need be).
    fn write_transcript_in(dir: &Path, id: &UnitId, answer: Option<&str>) {
        use rupu_transcript::{Event, JsonlWriter};
        std::fs::create_dir_all(dir).unwrap();
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

    fn write_transcript(global: &Path, id: &UnitId, answer: Option<&str>) {
        write_transcript_in(&global.join("transcripts"), id, answer);
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
    fn spawn_with_grandchild(
        dir: &Path,
    ) -> (
        SubprocessUnitLauncher,
        UnitId,
        u32, /* grandchild pid */
    ) {
        let pidfile = dir.join("grandchild.pid");
        let launcher = sh(
            dir,
            &format!("sleep 30 & echo $! > {}; wait", pidfile.display()),
        );
        let id = launcher.spawn(&spec(&[]), dir).unwrap().id;
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
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
        let pid = pid_of(&launcher, &id);

        // Alive but no transcript yet: not started as far as evidence goes.
        assert_eq!(launcher.poll(&id, dir.path()), UnitStatus::Pending);
        assert!(pid_is_running(pid));

        // Started-evidence is a non-empty transcript.
        write_transcript(dir.path(), &id, Some("partial"));
        assert_eq!(launcher.poll(&id, dir.path()), UnitStatus::Running);

        launcher.terminate(&id);
        let done = wait_terminal(&launcher, &id, dir.path());
        assert!(matches!(done, UnitStatus::Failed(_)), "{done:?}");
        // `try_wait` reaped it: asking again is still terminal, never an error.
        assert_eq!(launcher.poll(&id, dir.path()), done);
        assert_eq!(launcher.poll(&id, dir.path()), done);
        assert!(!pid_is_running(pid), "reaped, not a zombie");
    }

    #[test]
    fn a_clean_exit_with_an_answer_is_done_successful_and_stays_done() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
        // Written before the first poll, so the outcome read cannot race it.
        write_transcript(dir.path(), &id, Some("all clear"));
        let done = wait_terminal(&launcher, &id, dir.path());
        assert_eq!(
            done,
            UnitStatus::Done(UnitOutcome {
                output: "all clear".into(),
                success: true,
            })
        );
        assert_eq!(launcher.poll(&id, dir.path()), done);
    }

    #[test]
    fn an_exited_unit_is_terminal_and_reaped_not_left_a_zombie() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
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
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
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
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
        assert!(matches!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Failed(_)
        ));
    }

    #[test]
    fn a_clean_exit_without_an_answer_is_done_with_empty_output() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
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
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
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
    fn terminate_kills_the_whole_group_including_an_in_flight_grandchild() {
        let dir = tempfile::tempdir().unwrap();
        let (launcher, id, grandchild) = spawn_with_grandchild(dir.path());
        let leader = pid_of(&launcher, &id);

        launcher.terminate(&id);

        // The leader dies and is reaped...
        let status = wait_terminal(&launcher, &id, dir.path());
        assert!(matches!(status, UnitStatus::Failed(_)), "{status:?}");
        assert!(!pid_is_running(leader));
        // ...and so does the grandchild: the group died, not just the leader.
        wait_gone(grandchild, "the unit's grandchild");
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
        let a = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
        let b = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
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
        let id = launcher.spawn(&spec(&[]), dir.path()).unwrap().id;
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

    /// A launcher whose "rupu" is a script that, after `delay`, installs a
    /// run dir staged (with the real store types) for the unit's own run id
    /// into `<global>/runs/<id>` by an atomic directory rename, then runs
    /// `after`: the fake `rupu workflow run … --run-id <id>`.
    fn workflow_launcher(
        global: &Path,
        status: &'static str,
        error: Option<&'static str>,
        steps: &'static [(&'static str, &'static str, bool)],
        delay: &str,
        after: &str,
    ) -> SubprocessUnitLauncher {
        let global_dir = global.to_path_buf();
        let stage = global.join("stage");
        let (delay, after) = (delay.to_string(), after.to_string());
        SubprocessUnitLauncher::new("/bin/sh", global).with_argv(move |_, id, _| {
            stage_run(&stage, id, status, error, steps);
            let script = format!(
                "sleep {delay}; mkdir -p {g}/runs; \
                 cp -R {s}/runs/{id} {g}/runs/.{id}.tmp && mv {g}/runs/.{id}.tmp {g}/runs/{id}; \
                 {after}",
                g = global_dir.display(),
                s = stage.display(),
            );
            vec!["-c".into(), script.into()]
        })
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
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;

        // Alive, and `run.json` is not there yet: not started.
        assert_eq!(launcher.poll(&id, dir.path()), UnitStatus::Pending);

        let done = wait_terminal(&launcher, &id, dir.path());
        assert_eq!(
            done,
            UnitStatus::Done(UnitOutcome {
                output: "the final report".into(),
                success: true,
            })
        );
        assert_eq!(launcher.poll(&id, dir.path()), done, "idempotent");
    }

    #[test]
    fn a_completed_workflow_with_no_steps_is_done_with_empty_output() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = workflow_launcher(dir.path(), "completed", None, &[], "0", "exit 0");
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
        assert_eq!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Done(UnitOutcome {
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
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
        assert_eq!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Failed("step `scan` failed: boom".into())
        );
    }

    #[test]
    fn a_rejected_or_cancelled_workflow_without_a_message_is_failed_with_a_default() {
        for status in ["rejected", "cancelled"] {
            let dir = tempfile::tempdir().unwrap();
            let launcher = workflow_launcher(dir.path(), status, None, &[], "0", "exit 0");
            let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
            let got = wait_terminal(&launcher, &id, dir.path());
            assert_eq!(got, UnitStatus::Failed(format!("workflow run {status}")));
        }
    }

    #[test]
    fn a_workflow_parked_at_a_gate_is_failed_as_a_backstop() {
        for status in ["awaiting_approval", "paused"] {
            let dir = tempfile::tempdir().unwrap();
            let launcher = workflow_launcher(dir.path(), status, None, &[], "0", "exit 0");
            let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
            assert_eq!(
                wait_terminal(&launcher, &id, dir.path()),
                UnitStatus::Failed("workflow unit parked at an approval gate".into()),
                "{status}"
            );
        }
    }

    #[test]
    fn a_workflow_that_exits_without_writing_run_json_is_failed() {
        let dir = tempfile::tempdir().unwrap();
        let launcher = sh(dir.path(), "exit 0");
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
        assert_eq!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Failed("workflow exited before writing run.json".into())
        );
    }

    #[test]
    fn a_workflow_killed_mid_run_is_failed_not_stuck_running() {
        let dir = tempfile::tempdir().unwrap();
        // `run.json` says `running` and the runner dies by SIGKILL: the run
        // never reached a terminal state, so the unit must not read as alive.
        let launcher = workflow_launcher(dir.path(), "running", None, &[], "0", "kill -9 $$");
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
        let got = wait_terminal(&launcher, &id, dir.path());
        assert!(
            matches!(&got, UnitStatus::Failed(why) if why.contains("signal 9") && why.contains("running")),
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
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
        let pid = pid_of(&launcher, &id);

        // The copy lands a moment after spawn; poll until it has.
        let deadline = Instant::now() + Duration::from_secs(10);
        while launcher.poll(&id, dir.path()) != UnitStatus::Running {
            assert!(Instant::now() < deadline, "never became Running");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(pid_is_running(pid));

        launcher.terminate(&id);
        assert!(wait_terminal(&launcher, &id, dir.path()).is_terminal());
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
        let id = launcher.spawn(&workflow_spec(), dir.path()).unwrap().id;
        let grandchild = read_pid_file(&pidfile);
        let leader = pid_of(&launcher, &id);

        launcher.terminate(&id);

        // `run.json` no longer says `running`: the store cancelled the run...
        let rec = RunStore::new(dir.path().join("runs")).load(&id.0).unwrap();
        assert_eq!(rec.status, RunStatus::Cancelled);
        assert_eq!(rec.error_message.as_deref(), Some("fleet terminate"));
        // ...the unit reads as failed with that reason...
        assert_eq!(
            wait_terminal(&launcher, &id, dir.path()),
            UnitStatus::Failed("fleet terminate".into())
        );
        assert!(!pid_is_running(leader), "reaped, not a zombie");
        // ...and the step's grandchild died with the group.
        wait_gone(grandchild, "the workflow unit's step grandchild");
        // Terminating a finished unit is a harmless no-op.
        launcher.terminate(&id);
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

        let launcher = sh(&global, "sleep 5").with_workspace(&workspace);
        let id = launcher.spawn(&spec(&[]), tmp.path()).unwrap().id;
        assert_eq!(launcher.poll(&id, tmp.path()), UnitStatus::Pending);

        write_transcript_in(&transcripts, &id, Some("found it"));
        assert_eq!(
            launcher.poll(&id, tmp.path()),
            UnitStatus::Running,
            "the transcript under the ancestor project is started-evidence"
        );
        assert_eq!(launcher.final_text(&id.0).as_deref(), Some("found it"));
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

        let launcher = sh(&global, "sleep 5").with_workspace(&workspace);
        let id = launcher.spawn(&spec(&[]), tmp.path()).unwrap().id;
        write_transcript(&global, &id, Some("global one"));
        assert_eq!(launcher.poll(&id, tmp.path()), UnitStatus::Running);
        assert_eq!(launcher.final_text(&id.0).as_deref(), Some("global one"));
    }

    #[test]
    fn units_run_in_their_own_process_group_with_rupu_home_pointing_at_global() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("home.txt");
        let launcher = sh(
            dir.path(),
            &format!("echo \"$RUPU_HOME\" > {}; sleep 5", out.display()),
        );
        let spawned = launcher.spawn(&spec(&[]), dir.path()).unwrap();
        let id = spawned.id.clone();
        let pid = pid_of(&launcher, &id);

        let child = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        let pgid = rustix::process::getpgid(Some(child)).unwrap();
        assert_eq!(pgid, child, "the unit leads its own process group");
        // The group id spawn reports (and the supervisor persists) is that group.
        assert_eq!(spawned.pgid, Some(pgid.as_raw_nonzero().get() as u32));

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
