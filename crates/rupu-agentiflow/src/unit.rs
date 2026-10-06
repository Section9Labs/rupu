//! The unit port: how the fleet supervisor fires, observes and stops one
//! agent unit, without knowing whether it is a real `rupu run` subprocess
//! (Task 5's launcher) or a scripted test double ([`MockUnitLauncher`]).
//!
//! The shape is deliberately fire-then-poll: `spawn` returns the unit's handle
//! immediately and never blocks on the unit; `poll` is cheap and idempotent, so
//! the supervisor (and the lead's dispatch tools) can watch a unit without
//! holding a thread per unit.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// What kind of thing a unit runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitKind {
    /// One agent (`rupu run <agent>`).
    #[default]
    Agent,
    /// One workflow (`rupu workflow run <name>`).
    Workflow,
}

impl UnitKind {
    /// The stable lowercase name (`unit.json`'s `kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            UnitKind::Agent => "agent",
            UnitKind::Workflow => "workflow",
        }
    }
}

/// What to run as a unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSpec {
    /// The thing to run: the agent file for an [`UnitKind::Agent`] unit
    /// (`rupu run <agent>`), or the workflow name for a [`UnitKind::Workflow`]
    /// unit (`rupu workflow run <name>`). The field keeps its 3b-2 name so the
    /// agent path is untouched; `kind` says how to read it.
    pub agent: String,
    /// The unit's task prompt. Agent units only: a workflow takes `inputs`
    /// instead and ignores this (conventionally empty).
    pub prompt: String,
    /// Engagement-scope roots the unit is bound to (may be empty).
    pub engagement: Vec<String>,
    /// The unit's participant name on the board / mailboxes (`recon#1`).
    pub participant: String,
    /// Whether this unit runs an agent or a workflow.
    pub kind: UnitKind,
    /// `KEY=VALUE` workflow inputs (`--input k=v`, in order). Empty for an
    /// agent unit.
    pub inputs: Vec<(String, String)>,
}

/// A unit's handle: the pre-minted run id (`run_<ULID>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnitId(pub String);

/// A unit's final answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitOutcome {
    pub output: String,
    pub success: bool,
}

/// Where a unit is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitStatus {
    /// Spawned, no evidence yet that it has started.
    Pending,
    /// Started and alive.
    Running,
    /// Ended with an answer (which may itself report `success: false`).
    Done(UnitOutcome),
    /// Ended without an answer (crashed, killed, vanished); carries why.
    Failed(String),
}

impl UnitStatus {
    /// Whether this status is final (no further transitions).
    pub fn is_terminal(&self) -> bool {
        matches!(self, UnitStatus::Done(_) | UnitStatus::Failed(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UnitError {
    /// The launcher could not start the unit.
    #[error("unit spawn failed: {0}")]
    Spawn(String),
    /// A unit id the launcher / supervisor does not know.
    #[error("unknown unit")]
    Unknown,
}

/// The port the supervisor drives. Implementations must be cheap to `poll` and
/// must never block `spawn` on the unit's execution.
pub trait UnitLauncher: Send + Sync {
    /// Fire a unit; return its handle immediately (non-blocking).
    fn spawn(&self, spec: &UnitSpec, run_dir: &Path) -> Result<UnitId, UnitError>;
    /// Current status (started-evidence + liveness + terminal outcome). Cheap,
    /// pollable.
    fn poll(&self, id: &UnitId, run_dir: &Path) -> UnitStatus;
    /// SIGTERM the unit; best-effort.
    fn terminate(&self, id: &UnitId);
}

/// A scripted [`UnitLauncher`] for tests.
///
/// Every spawned unit gets its own copy of the script. Each `poll` returns the
/// next scripted status and then stays on the last one (an empty script reads
/// as `Pending` forever). An optional `on_spawn` side effect runs inside
/// `spawn`, so a test's mock unit can write a finding into the pooled scope or
/// post to the board the way a real unit would.
pub struct MockUnitLauncher {
    script: Vec<UnitStatus>,
    on_spawn: Option<OnSpawn>,
    state: Mutex<MockState>,
}

type OnSpawn = Box<dyn Fn(&UnitSpec, &Path) + Send + Sync>;

#[derive(Default)]
struct MockState {
    /// id -> number of polls served so far.
    polls: HashMap<String, usize>,
    /// Ids passed to `terminate`, in call order.
    terminated: Vec<String>,
    /// Specs passed to `spawn`, in call order.
    spawned: Vec<UnitSpec>,
}

impl MockUnitLauncher {
    /// A launcher whose every unit walks `script` (see the type docs).
    pub fn scripted(script: Vec<UnitStatus>) -> Self {
        Self {
            script,
            on_spawn: None,
            state: Mutex::new(MockState::default()),
        }
    }

    /// Run `f(spec, run_dir)` on every `spawn`, before the id is returned.
    pub fn with_on_spawn(mut self, f: impl Fn(&UnitSpec, &Path) + Send + Sync + 'static) -> Self {
        self.on_spawn = Some(Box::new(f));
        self
    }

    /// Ids passed to `terminate` so far, in call order.
    pub fn terminated(&self) -> Vec<String> {
        self.lock().terminated.clone()
    }

    /// Specs passed to `spawn` so far, in call order.
    pub fn spawned(&self) -> Vec<UnitSpec> {
        self.lock().spawned.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MockState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl UnitLauncher for MockUnitLauncher {
    fn spawn(&self, spec: &UnitSpec, run_dir: &Path) -> Result<UnitId, UnitError> {
        let id = format!("run_{}", ulid::Ulid::new());
        if let Some(f) = &self.on_spawn {
            f(spec, run_dir);
        }
        let mut st = self.lock();
        st.polls.insert(id.clone(), 0);
        st.spawned.push(spec.clone());
        Ok(UnitId(id))
    }

    fn poll(&self, id: &UnitId, _run_dir: &Path) -> UnitStatus {
        let mut st = self.lock();
        let Some(n) = st.polls.get_mut(&id.0) else {
            return UnitStatus::Failed(format!("unknown unit {}", id.0));
        };
        let at = *n;
        *n = n.saturating_add(1);
        match self.script.get(at).or_else(|| self.script.last()) {
            Some(s) => s.clone(),
            None => UnitStatus::Pending,
        }
    }

    fn terminate(&self, id: &UnitId) {
        self.lock().terminated.push(id.0.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn spec(participant: &str) -> UnitSpec {
        UnitSpec {
            agent: "recon".into(),
            prompt: "scan".into(),
            engagement: vec![],
            participant: participant.into(),
            kind: UnitKind::Agent,
            inputs: vec![],
        }
    }

    #[test]
    fn mock_walks_its_script_then_stays_on_the_last_status() {
        let done = UnitStatus::Done(UnitOutcome {
            output: "ok".into(),
            success: true,
        });
        let m = MockUnitLauncher::scripted(vec![
            UnitStatus::Pending,
            UnitStatus::Running,
            done.clone(),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let id = m.spawn(&spec("recon#1"), dir.path()).unwrap();
        assert!(
            id.0.starts_with("run_"),
            "id is a pre-minted run id: {id:?}"
        );
        assert_eq!(m.poll(&id, dir.path()), UnitStatus::Pending);
        assert_eq!(m.poll(&id, dir.path()), UnitStatus::Running);
        assert_eq!(m.poll(&id, dir.path()), done);
        assert_eq!(m.poll(&id, dir.path()), done, "stays on the last status");
    }

    #[test]
    fn mock_scripts_each_unit_independently_and_ids_are_unique() {
        let m =
            MockUnitLauncher::scripted(vec![UnitStatus::Running, UnitStatus::Failed("x".into())]);
        let dir = tempfile::tempdir().unwrap();
        let a = m.spawn(&spec("a#1"), dir.path()).unwrap();
        let b = m.spawn(&spec("b#1"), dir.path()).unwrap();
        assert_ne!(a, b);
        assert_eq!(m.poll(&a, dir.path()), UnitStatus::Running);
        // `b` has not been polled yet, so it starts at the head of the script.
        assert_eq!(m.poll(&b, dir.path()), UnitStatus::Running);
        assert_eq!(m.poll(&a, dir.path()), UnitStatus::Failed("x".into()));
    }

    #[test]
    fn mock_empty_script_is_pending_and_unknown_id_fails() {
        let m = MockUnitLauncher::scripted(vec![]);
        let dir = tempfile::tempdir().unwrap();
        let id = m.spawn(&spec("a#1"), dir.path()).unwrap();
        assert_eq!(m.poll(&id, dir.path()), UnitStatus::Pending);
        assert!(matches!(
            m.poll(&UnitId("run_nope".into()), dir.path()),
            UnitStatus::Failed(_)
        ));
    }

    #[test]
    fn mock_runs_the_on_spawn_side_effect_and_records_terminate() {
        let seen: Arc<Mutex<Vec<(String, std::path::PathBuf)>>> = Arc::default();
        let sink = seen.clone();
        let m = MockUnitLauncher::scripted(vec![UnitStatus::Running]).with_on_spawn(move |s, d| {
            sink.lock()
                .unwrap()
                .push((s.participant.clone(), d.to_path_buf()));
        });
        let dir = tempfile::tempdir().unwrap();
        let id = m.spawn(&spec("recon#1"), dir.path()).unwrap();
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![("recon#1".to_string(), dir.path().to_path_buf())]
        );
        assert_eq!(m.spawned(), vec![spec("recon#1")]);
        m.terminate(&id);
        assert_eq!(m.terminated(), vec![id.0]);
    }

    #[test]
    fn terminal_statuses_are_terminal() {
        assert!(!UnitStatus::Pending.is_terminal());
        assert!(!UnitStatus::Running.is_terminal());
        assert!(UnitStatus::Failed("x".into()).is_terminal());
        assert!(UnitStatus::Done(UnitOutcome {
            output: String::new(),
            success: false
        })
        .is_terminal());
    }
}
