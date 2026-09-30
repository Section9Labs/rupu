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
            Event::UnitStarted {
                step_id,
                index,
                unit_key,
                agent,
                host,
                codename,
                ..
            } => {
                let s = self.step_mut(step_id);
                let u = s.units.entry(*index).or_insert_with(|| UnitView {
                    index: *index,
                    unit_key: unit_key.clone(),
                    agent: agent.clone(),
                    codename: codename.clone(),
                    provider: None,
                    model: None,
                    host: host.clone(),
                    status: UnitStatus::Queued,
                });
                u.unit_key = unit_key.clone();
                u.status = UnitStatus::Running;
                // The runner re-emits `UnitStarted` for the same (step, index)
                // when a unit is retried onto a fallback host, carrying the new
                // host/codename. Overwrite (latest wins) rather than back-fill,
                // so a retried unit doesn't keep the stale primary host. A
                // re-emit that carries no value leaves the known one intact.
                if host.is_some() {
                    u.host = host.clone();
                }
                if codename.is_some() {
                    u.codename = codename.clone();
                }
            }
            // `tokens_in`/`tokens_out` are deliberately ignored: they are
            // always 0 on this event (see its doc comment). Real totals come
            // from the usage fold (Task 4).
            Event::UnitCompleted {
                step_id,
                index,
                success,
                ..
            } => {
                let s = self.step_mut(step_id);
                if let Some(u) = s.units.get_mut(index) {
                    u.status = if *success {
                        UnitStatus::Done
                    } else {
                        UnitStatus::Failed
                    };
                }
            }
            Event::AgentStarted {
                step_id,
                unit_index: Some(i),
                provider,
                model,
                codename,
                ..
            } => {
                let s = self.step_mut(step_id);
                if let Some(u) = s.units.get_mut(i) {
                    if provider.is_some() {
                        u.provider = provider.clone();
                    }
                    if model.is_some() {
                        u.model = model.clone();
                    }
                    if u.codename.is_none() {
                        u.codename = codename.clone();
                    }
                }
            }
            // Sub-agent dispatches live in their own map, keyed by
            // `sub_run_id`, and never touch any step's `units` — this is the
            // fix for the dispatch-overwrites-unit-slot bug. The event carries
            // no `step_id`, so attribute to the most recently active step.
            Event::DispatchStarted {
                sub_run_id,
                agent,
                codename,
                provider,
                model,
                ..
            } => {
                self.dispatches.insert(
                    sub_run_id.clone(),
                    DispatchView {
                        sub_run_id: sub_run_id.clone(),
                        parent_step_id: self.last_active_step.clone(),
                        agent: agent.clone(),
                        codename: codename.clone(),
                        provider: provider.clone(),
                        model: model.clone(),
                        status: UnitStatus::Running,
                        tokens_in: 0,
                        tokens_out: 0,
                    },
                );
            }
            // Unlike `UnitCompleted`, these token totals are the real child
            // run totals. A completion for an unknown `sub_run_id` is a no-op.
            Event::DispatchCompleted {
                sub_run_id,
                success,
                tokens_in,
                tokens_out,
                ..
            } => {
                if let Some(d) = self.dispatches.get_mut(sub_run_id) {
                    d.status = if *success {
                        UnitStatus::Done
                    } else {
                        UnitStatus::Failed
                    };
                    d.tokens_in = *tokens_in;
                    d.tokens_out = *tokens_out;
                }
            }
            Event::PanelRound {
                step_id,
                round,
                max_iterations,
                ..
            } => {
                let s = self.step_mut(step_id);
                s.panel_round = Some(*round);
                s.panel_max = Some(*max_iterations);
            }
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

    #[test]
    fn fanout_units_count_and_survive_a_dispatch() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            kind: StepKind::ForEach,
            agent: None,
            host: None,
            codename: None,
        });
        for i in 0..3usize {
            v.apply(&Event::UnitStarted {
                run_id: "r".into(),
                step_id: "hunt".into(),
                index: i,
                unit_key: format!("svc-{i}"),
                agent: Some("breaker".into()),
                transcript_path: format!("t{i}").into(),
                host: None,
                codename: Some(format!("otter#{}", i + 1)),
            });
        }
        // A dispatch must NOT clobber unit slot 3 (== units.len()); it lives in
        // its own map (Task 3), so units stay intact.
        v.apply(&Event::DispatchStarted {
            run_id: "r".into(),
            sub_run_id: "sub1".into(),
            agent: Some("scout".into()),
            transcript_path: "ts".into(),
            codename: Some("wren#1".into()),
            provider: None,
            model: None,
        });
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 0,
            unit_key: "svc-0".into(),
            success: true,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        });
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 1,
            unit_key: "svc-1".into(),
            success: false,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        });

        let step = &v.steps[0];
        assert_eq!(step.units.len(), 3);
        assert_eq!(step.units[&0].status, UnitStatus::Done);
        assert_eq!(step.units[&1].status, UnitStatus::Failed);
        assert_eq!(step.units[&2].status, UnitStatus::Running);
        assert_eq!(step.units[&2].unit_key, "svc-2");
        let c = step.unit_counts();
        assert_eq!((c.done, c.failed, c.running, c.total), (1, 1, 1, 3));
    }

    #[test]
    fn agent_started_attaches_provider_model_to_unit() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            kind: StepKind::ForEach,
            agent: None,
            host: None,
            codename: None,
        });
        v.apply(&Event::UnitStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 0,
            unit_key: "svc-0".into(),
            agent: Some("breaker".into()),
            transcript_path: "t0".into(),
            host: None,
            codename: None,
        });
        v.apply(&Event::AgentStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            unit_index: Some(0),
            codename: None,
            agent: "breaker".into(),
            provider: Some("openai".into()),
            model: Some("gpt-5".into()),
            agent_run_id: "ar".into(),
            transcript_path: "t0".into(),
        });
        assert_eq!(v.steps[0].units[&0].provider.as_deref(), Some("openai"));
        assert_eq!(v.steps[0].units[&0].model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn unit_started_reemit_overwrites_host_and_codename_on_fallback_retry() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            kind: StepKind::ForEach,
            agent: None,
            host: None,
            codename: None,
        });
        let unit_started = |host: &str, codename: &str| Event::UnitStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 0,
            unit_key: "svc-0".into(),
            agent: Some("breaker".into()),
            transcript_path: "t0".into(),
            host: Some(host.into()),
            codename: Some(codename.into()),
        };
        // Primary host, then the runner re-emits UnitStarted for the same
        // (step, index) when the unit is retried onto a fallback host.
        v.apply(&unit_started("host-a", "otter#1"));
        v.apply(&unit_started("host-b", "otter#2"));

        let step = &v.steps[0];
        assert_eq!(step.units.len(), 1, "retry must reuse the same slot");
        let u = &step.units[&0];
        assert_eq!(u.host.as_deref(), Some("host-b"), "latest host wins");
        assert_eq!(
            u.codename.as_deref(),
            Some("otter#2"),
            "latest codename wins"
        );
        assert_eq!(u.status, UnitStatus::Running);

        // A re-emit that carries no host/codename must not erase what we know.
        v.apply(&Event::UnitStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 0,
            unit_key: "svc-0".into(),
            agent: Some("breaker".into()),
            transcript_path: "t0".into(),
            host: None,
            codename: None,
        });
        let u = &v.steps[0].units[&0];
        assert_eq!(u.host.as_deref(), Some("host-b"));
        assert_eq!(u.codename.as_deref(), Some("otter#2"));
    }

    #[test]
    fn sparse_out_of_order_unit_indices_are_keyed_by_their_own_index() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            kind: StepKind::ForEach,
            agent: None,
            host: None,
            codename: None,
        });
        for i in [5usize, 2usize] {
            v.apply(&Event::UnitStarted {
                run_id: "r".into(),
                step_id: "hunt".into(),
                index: i,
                unit_key: format!("svc-{i}"),
                agent: Some("breaker".into()),
                transcript_path: format!("t{i}").into(),
                host: None,
                codename: None,
            });
        }
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 5,
            unit_key: "svc-5".into(),
            success: true,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        });

        assert_eq!(v.steps.len(), 1, "no phantom step");
        let step = &v.steps[0];
        let keys: Vec<usize> = step.units.keys().copied().collect();
        assert_eq!(keys, vec![2, 5]);
        assert_eq!(step.units[&5].status, UnitStatus::Done);
        assert_eq!(step.units[&5].unit_key, "svc-5");
        assert_eq!(step.units[&2].status, UnitStatus::Running);
        assert_eq!(step.units[&2].unit_key, "svc-2");
    }

    #[test]
    fn dispatch_lives_in_its_own_map_attributed_to_active_step() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "assess".into(),
            kind: StepKind::Run,
            agent: None,
            host: None,
            codename: None,
        });
        v.apply(&Event::DispatchStarted {
            run_id: "r".into(),
            sub_run_id: "sub1".into(),
            agent: Some("scout".into()),
            transcript_path: "ts".into(),
            codename: Some("wren#1".into()),
            provider: Some("anthropic".into()),
            model: Some("opus".into()),
        });
        v.apply(&Event::DispatchCompleted {
            run_id: "r".into(),
            sub_run_id: "sub1".into(),
            success: true,
            tokens_in: 1000,
            tokens_out: 200,
        });

        let d = &v.dispatches["sub1"];
        assert_eq!(d.parent_step_id.as_deref(), Some("assess"));
        assert_eq!(d.status, UnitStatus::Done);
        assert_eq!((d.tokens_in, d.tokens_out), (1000, 200));
        assert_eq!(d.codename.as_deref(), Some("wren#1"));
    }

    #[test]
    fn panel_round_sets_counter() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "triage".into(),
            kind: StepKind::Panel,
            agent: None,
            host: None,
            codename: None,
        });
        v.apply(&Event::PanelRound {
            run_id: "r".into(),
            step_id: "triage".into(),
            round: 2,
            max_iterations: 5,
            max_severity_remaining: Some("high".into()),
        });
        assert_eq!(v.steps[0].panel_round, Some(2));
        assert_eq!(v.steps[0].panel_max, Some(5));
    }
}
