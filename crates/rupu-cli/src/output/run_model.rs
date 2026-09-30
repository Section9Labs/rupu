//! `RunView` — the canonical, correct projection of a workflow run.
//!
//! A pure state machine: `apply(&Event)` folds one `events.jsonl` line;
//! `from_run_dir` (Task 4) replays the whole log plus `run.json` /
//! `step_results.jsonl` and the token/cost fold. No I/O, no rendering here —
//! the live view (Plan 2) drives the same `apply`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStatus, StepKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitStatus {
    Queued,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnitCounts {
    pub queued: usize,
    pub running: usize,
    pub done: usize,
    pub failed: usize,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct UnitView {
    pub index: usize,
    pub unit_key: String,
    pub agent: Option<String>,
    pub codename: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub status: UnitStatus,
}

#[derive(Debug, Clone)]
pub struct DispatchView {
    pub sub_run_id: String,
    /// Step that was active when the dispatch began (dispatch events carry
    /// no `step_id`; see the event doc comment). Never a unit slot — this is
    /// what fixes the slot-overwrite bug: dispatches live in their own map.
    pub parent_step_id: Option<String>,
    pub agent: Option<String>,
    pub codename: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub status: UnitStatus,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    AwaitingApproval,
    Complete,
    Failed,
    Skipped,
    Paused,
}

#[derive(Debug, Clone)]
pub struct StepView {
    pub step_id: String,
    pub kind: StepKind,
    pub state: StepState,
    pub agent: Option<String>,
    pub codename: Option<String>,
    pub host: Option<String>,
    pub duration_ms: Option<u64>,
    /// Fan-out units keyed by their own `index` (stable per unit within a
    /// step). Keying by index — not a shared `Vec` slot — is the fix for the
    /// sub-agent overwrite bug.
    pub units: BTreeMap<usize, UnitView>,
    pub panel_round: Option<u32>,
    pub panel_max: Option<u32>,
    pub loop_iteration: Option<u32>,
    /// First-seen order, for stable rendering.
    pub order: usize,
}

impl StepView {
    pub fn unit_counts(&self) -> UnitCounts {
        let mut c = UnitCounts::default();
        for u in self.units.values() {
            match u.status {
                UnitStatus::Queued => c.queued += 1,
                UnitStatus::Running => c.running += 1,
                UnitStatus::Done => c.done += 1,
                UnitStatus::Failed => c.failed += 1,
            }
            c.total += 1;
        }
        c
    }
}

#[derive(Debug, Clone)]
pub struct GateView {
    pub step_id: String,
    pub prompt: Option<String>,
    pub since: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct RunView {
    pub run_id: String,
    pub workflow_name: String,
    /// Crew word of the run codename (run-level tint). `None` until known.
    pub crew: Option<String>,
    pub status: RunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Bumped on each `RunStarted`. Lets Plan 2's live view ignore a prior
    /// run generation's terminal event during replay.
    pub generation: u64,
    pub steps: Vec<StepView>,
    pub dispatches: BTreeMap<String, DispatchView>,
    pub gates: Vec<GateView>,
    pub error: Option<String>,
    pub usage: Option<rupu_cp::usage::UsageSummary>,
    /// Panel findings by severity string (lowercased). Empty when none.
    pub findings_by_severity: BTreeMap<String, usize>,
    /// Step that was active most recently — the attribution target for a
    /// dispatch (which carries no `step_id`).
    last_active_step: Option<String>,
}

impl RunView {
    pub fn step_mut(&mut self, step_id: &str) -> &mut StepView {
        if let Some(i) = self.steps.iter().position(|s| s.step_id == step_id) {
            return &mut self.steps[i];
        }
        let order = self.steps.len();
        self.steps.push(StepView {
            step_id: step_id.to_string(),
            kind: StepKind::Linear,
            state: StepState::Pending,
            agent: None,
            codename: None,
            host: None,
            duration_ms: None,
            units: BTreeMap::new(),
            panel_round: None,
            panel_max: None,
            loop_iteration: None,
            order,
        });
        self.steps.last_mut().unwrap()
    }

    pub fn apply(&mut self, ev: &Event) {
        match ev {
            Event::RunStarted {
                run_id, started_at, ..
            } => {
                self.run_id = run_id.clone();
                self.started_at = Some(*started_at);
                self.status = RunStatus::Running;
                self.error = None;
                self.generation += 1;
            }
            Event::StepStarted {
                step_id,
                kind,
                agent,
                host,
                codename,
                ..
            } => {
                let s = self.step_mut(step_id);
                s.kind = *kind;
                s.state = StepState::Running;
                s.agent = agent.clone();
                s.host = host.clone();
                s.codename = codename.clone();
                self.last_active_step = Some(step_id.clone());
            }
            Event::StepWorking { step_id, .. } => {
                let s = self.step_mut(step_id);
                if s.state == StepState::Pending {
                    s.state = StepState::Running;
                }
                self.last_active_step = Some(step_id.clone());
            }
            Event::StepAwaitingApproval { step_id, .. } => {
                self.step_mut(step_id).state = StepState::AwaitingApproval;
            }
            Event::StepCompleted {
                step_id,
                success,
                duration_ms,
                host,
                ..
            } => {
                let s = self.step_mut(step_id);
                s.state = if *success {
                    StepState::Complete
                } else {
                    StepState::Failed
                };
                s.duration_ms = Some(*duration_ms);
                if host.is_some() {
                    s.host = host.clone();
                }
            }
            Event::StepFailed { step_id, error, .. } => {
                self.step_mut(step_id).state = StepState::Failed;
                if self.error.is_none() {
                    self.error = Some(error.clone());
                }
            }
            Event::StepSkipped { step_id, .. } => {
                self.step_mut(step_id).state = StepState::Skipped;
            }
            Event::StepPaused { step_id, .. } => {
                self.step_mut(step_id).state = StepState::Paused;
            }
            Event::StepResumed { step_id, .. } => {
                self.step_mut(step_id).state = StepState::Running;
            }
            Event::RunCompleted {
                status,
                finished_at,
                ..
            } => {
                self.status = *status;
                self.finished_at = Some(*finished_at);
            }
            Event::RunFailed {
                error, finished_at, ..
            } => {
                self.status = RunStatus::Failed;
                self.error = Some(error.clone());
                self.finished_at = Some(*finished_at);
            }
            Event::RunPaused { .. } => self.status = RunStatus::Paused,
            Event::RunResumed { .. } => self.status = RunStatus::Running,
            // Fan-out, dispatch, panel handled in Tasks 2-3.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    fn started(step: &str, kind: StepKind) -> Event {
        Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind,
            agent: None,
            host: None,
            codename: None,
        }
    }

    #[test]
    fn linear_lifecycle_sets_states_and_status() {
        let mut v = RunView::default();
        v.apply(&Event::RunStarted {
            event_version: 1,
            run_id: "r".into(),
            workflow_path: "wf".into(),
            started_at: Utc::now(),
        });
        v.apply(&started("a", StepKind::Run));
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: "a".into(),
            success: true,
            duration_ms: 18_000,
            host: None,
        });
        v.apply(&started("b", StepKind::Run));
        v.apply(&Event::RunCompleted {
            run_id: "r".into(),
            status: RunStatus::Completed,
            finished_at: Utc::now(),
        });

        assert_eq!(v.status, RunStatus::Completed);
        assert_eq!(v.generation, 1);
        assert_eq!(v.steps.len(), 2);
        assert_eq!(v.steps[0].state, StepState::Complete);
        assert_eq!(v.steps[0].duration_ms, Some(18_000));
        assert_eq!(v.steps[1].state, StepState::Running);
        assert!(matches!(v.steps[0].kind, StepKind::Run));
    }
}
