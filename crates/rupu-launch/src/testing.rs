//! A scripted [`ProcessBackend`] for tests: every spawned child walks its own
//! copy of a script of states, and a terminated one reads `Failed`.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::process::{ProcOutcome, ProcState, ProcessBackend, ProcessLaunch, SpawnedProcess, Stop};

type OnSpawn = Box<dyn Fn(&ProcessLaunch) + Send + Sync>;

/// See the module docs. Each `poll` returns the next scripted state and then
/// stays on the last one (an empty script reads `Pending` forever). An
/// optional `on_spawn` side effect runs inside `spawn`, so a test's child can
/// write a finding or post to a board the way a real one would.
pub struct ScriptedProcesses {
    script: Vec<ProcState>,
    on_spawn: Option<OnSpawn>,
    pid: Option<u32>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    polls: HashMap<String, usize>,
    terminated: Vec<(String, Stop)>,
    spawned: Vec<ProcessLaunch>,
}

impl ScriptedProcesses {
    pub fn scripted(script: Vec<ProcState>) -> Self {
        Self {
            script,
            on_spawn: None,
            pid: None,
            state: Mutex::new(State::default()),
        }
    }

    /// Every child answers `output` successfully at its first poll.
    pub fn done(output: &str) -> Self {
        Self::scripted(vec![ProcState::Done(ProcOutcome {
            output: output.into(),
            success: true,
        })])
    }

    /// Report `pid` as every child's pid (and so pgid), with its real start
    /// time.
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    pub fn with_on_spawn(mut self, f: impl Fn(&ProcessLaunch) + Send + Sync + 'static) -> Self {
        self.on_spawn = Some(Box::new(f));
        self
    }

    /// What `spawn` was handed, in call order.
    pub fn spawned(&self) -> Vec<ProcessLaunch> {
        self.lock().spawned.clone()
    }

    /// Run ids passed to `terminate`, in call order.
    pub fn terminated(&self) -> Vec<String> {
        self.lock()
            .terminated
            .iter()
            .map(|(r, _)| r.clone())
            .collect()
    }

    /// What `terminate` was handed, in call order.
    pub fn stops(&self) -> Vec<(String, Stop)> {
        self.lock().terminated.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ProcessBackend for ScriptedProcesses {
    fn spawn(&self, launch: &ProcessLaunch) -> Result<Option<SpawnedProcess>, String> {
        if let Some(f) = &self.on_spawn {
            f(launch);
        }
        let mut st = self.lock();
        st.polls.insert(launch.run_id.clone(), 0);
        st.spawned.push(launch.clone());
        Ok(self.pid.map(|pid| SpawnedProcess {
            pid,
            start: crate::proc::start_stamp(pid),
        }))
    }

    fn poll(&self, run_id: &str) -> ProcState {
        let mut st = self.lock();
        if st.terminated.iter().any(|(t, _)| t == run_id) {
            return ProcState::Failed("terminated".into());
        }
        let Some(n) = st.polls.get_mut(run_id) else {
            return ProcState::Failed(format!("unknown child {run_id}"));
        };
        let at = *n;
        *n = n.saturating_add(1);
        self.script
            .get(at)
            .or_else(|| self.script.last())
            .cloned()
            .unwrap_or(ProcState::Pending)
    }

    fn terminate(&self, run_id: &str, why: Stop) {
        self.lock().terminated.push((run_id.to_string(), why));
    }
}
