//! `RunView` — the canonical, correct projection of a workflow run.
//!
//! A pure state machine: `apply(&Event)` folds one `events.jsonl` line;
//! `from_run_dir` (Task 4) replays the whole log plus `run.json` /
//! `step_results.jsonl` and the token/cost fold. No I/O, no rendering here —
//! the live view (Plan 2) drives the same `apply`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rupu_config::PricingConfig;
use rupu_orchestrator::executor::{AttemptResumeMode, Event};
use rupu_orchestrator::runs::{RunStatus, StepKind};
use rupu_orchestrator::RunStore;

use crate::output::palette::Status;

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

/// How a resumed run picked an interrupted step / unit back up, for the live
/// view to mark it. Only the two modes that are *not* a fresh start have a
/// mark: a restarted attempt looks like any other start, so
/// [`AttemptResumeMode::Restarted`] has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumedMark {
    /// A new agent run carries on from the interrupted one's transcript.
    Continued,
    /// The interrupted attempt had finished; its answer was reused, no agent ran.
    Recovered,
}

impl ResumedMark {
    /// The mark for `mode`, if it earns one.
    pub fn of(mode: AttemptResumeMode) -> Option<Self> {
        match mode {
            AttemptResumeMode::Continued => Some(Self::Continued),
            AttemptResumeMode::Recovered => Some(Self::Recovered),
            AttemptResumeMode::Restarted => None,
        }
    }

    /// The word the live view shows beside the row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Continued => "continued",
            Self::Recovered => "recovered",
        }
    }
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
    /// Set when this unit's current attempt resumes an interrupted one
    /// (`AttemptResumed`); cleared by the next `UnitStarted`.
    pub resumed: Option<ResumedMark>,
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
    /// Provider / model of a singleton agent step, from its `AgentStarted`.
    /// Always `None` for fan-out / parallel / panel steps: those run many
    /// agent instances under one step id (units carry their own).
    pub provider: Option<String>,
    pub model: Option<String>,
    pub host: Option<String>,
    pub duration_ms: Option<u64>,
    /// Fan-out units keyed by their own `index` (stable per unit within a
    /// step). Keying by index — not a shared `Vec` slot — is the fix for the
    /// sub-agent overwrite bug.
    pub units: BTreeMap<usize, UnitView>,
    pub panel_round: Option<u32>,
    pub panel_max: Option<u32>,
    pub loop_iteration: Option<u32>,
    /// `StepWarning` messages for this step (step-level and per-unit), in
    /// arrival order. Information only — never part of `state`.
    pub warnings: Vec<String>,
    /// Set when a linear step's current attempt resumes an interrupted one
    /// (`AttemptResumed` with no `unit_index`); cleared by the next
    /// `StepStarted`. A fan-out step's units carry their own.
    pub resumed: Option<ResumedMark>,
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

    /// The live frontier unit of a fan-out: the newest *running* unit (units
    /// start in index order, so the highest index is the most recent), else —
    /// when none is running — the newest unit that has started at all. A queued
    /// unit has no transcript yet, so it is never the frontier. `None` for a
    /// step with no started units.
    ///
    /// The live view auto-follows this unit's transcript when a fan-out step is
    /// selected but no single unit is drilled (the stream pane would otherwise
    /// sit idle while 69 units stream in the firehose). Shared by the stream
    /// pin ([`crate::output::live_run`]) and the stream title
    /// ([`crate::output::live_view::panes`]) so the two stay in lockstep.
    pub fn frontier_unit(&self) -> Option<&UnitView> {
        self.units
            .values()
            .filter(|u| u.status == UnitStatus::Running)
            .max_by_key(|u| u.index)
            .or_else(|| {
                self.units
                    .values()
                    .filter(|u| u.status != UnitStatus::Queued)
                    .max_by_key(|u| u.index)
            })
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
    /// Panel findings by severity (lowercased), counting findings EMITTED per
    /// step-result record (a finding that persists across bounded-loop
    /// iterations is counted once per iteration). Empty when none.
    pub findings_by_severity: BTreeMap<String, usize>,
    /// `StepWarning` messages, in arrival order, as `"<step>: <message>"` —
    /// `"<step>[<unit index>]: <message>"` when the warning is about one
    /// fan-out unit (e.g. a remote unit whose coverage could not be
    /// collected).
    pub warnings: Vec<String>,
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
            provider: None,
            model: None,
            host: None,
            duration_ms: None,
            units: BTreeMap::new(),
            panel_round: None,
            panel_max: None,
            loop_iteration: None,
            warnings: Vec::new(),
            resumed: None,
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
                // `events.jsonl` spans every generation. Keep the FIRST start
                // so the header elapsed is cumulative since the run began and
                // never visibly "resets" on resume, and clear any finish the
                // prior generation recorded — a new generation is live, so
                // `elapsed_ms` must measure against `now`, not a stale
                // `finished_at` (which read earlier than the resume's start
                // and clamped elapsed to `0s`).
                if self.started_at.is_none() {
                    self.started_at = Some(*started_at);
                }
                self.finished_at = None;
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
                // A new attempt: any earlier resumption mark described the
                // last one (`events.jsonl` spans every resume).
                s.resumed = None;
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
            Event::StepWarning {
                step_id,
                index,
                message,
                ..
            } => {
                // Information only: never a step or run failure. Kept flat
                // (for the completion summary / `show-run`) as
                // `<step>[<unit>]: <message>` — the unit named when there is
                // one, so units of one host don't read as the same line — and
                // on the step (for the live view's marker), but only on a step
                // the run has reached: a warning must not conjure a pending
                // step into the view.
                self.warnings.push(match index {
                    Some(i) => format!("{step_id}[{i}]: {message}"),
                    None => format!("{step_id}: {message}"),
                });
                if let Some(s) = self.steps.iter_mut().find(|s| s.step_id == *step_id) {
                    s.warnings.push(message.clone());
                }
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
                    resumed: None,
                });
                u.unit_key = unit_key.clone();
                u.status = UnitStatus::Running;
                // Every start is a new attempt; `AttemptResumed` follows it
                // when that attempt is a resumption.
                u.resumed = None;
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
            // A singleton agent step (linear / run / loop / …) runs exactly one
            // agent, so its provider/model describe the step. Fan-out,
            // parallel and panel steps run many agent instances under one
            // step id — no single provider/model describes them — so they
            // stay unset rather than showing whichever instance started last.
            Event::AgentStarted {
                step_id,
                unit_index: None,
                provider,
                model,
                ..
            } => {
                let s = self.step_mut(step_id);
                let many = matches!(
                    s.kind,
                    StepKind::ForEach | StepKind::Parallel | StepKind::Panel
                );
                if !many {
                    if provider.is_some() {
                        s.provider = provider.clone();
                    }
                    if model.is_some() {
                        s.model = model.clone();
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
            // A resumed attempt is announced right after its `StepStarted` /
            // `UnitStarted`, so its row exists to take the mark. `Restarted`
            // is a plain fresh start and earns none; a unit whose start was
            // never seen is not conjured up for a mark.
            Event::AttemptResumed {
                step_id,
                unit_index,
                mode,
                ..
            } => {
                if let Some(mark) = ResumedMark::of(*mode) {
                    let s = self.step_mut(step_id);
                    match unit_index {
                        Some(i) => {
                            if let Some(u) = s.units.get_mut(i) {
                                u.resumed = Some(mark);
                            }
                        }
                        None => s.resumed = Some(mark),
                    }
                }
            }
            // Written by a newer rupu: nothing this view can show.
            Event::Unknown => {}
        }
    }
}

impl RunView {
    /// Build the full projection of a run from its on-disk artifacts.
    /// Replays `events.jsonl`, overlays `run.json` (crew + gates + status)
    /// and `step_results.jsonl` (loop iteration, host, panel findings), and
    /// folds token/cost totals through the shared `rupu_cp::usage` path
    /// (which already resolves fan-out + remote-mirror transcripts).
    pub fn from_run_dir(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> RunView {
        // Carry the id even if run.json is missing/unreadable (degraded view).
        let mut v = RunView {
            run_id: run_id.to_string(),
            ..RunView::default()
        };

        // 1. Replay the event log.
        let mut tailer = crate::output::jsonl_reader::WfEventTailer::new(store.events_path(run_id));
        for ev in tailer.drain_events() {
            v.apply(&ev);
        }

        // 2. Overlay run.json.
        if let Ok(rec) = store.load(run_id) {
            v.run_id = rec.id.clone();
            v.workflow_name = rec.workflow_name.clone();
            v.crew = rec.codename.clone();
            v.status = rec.status;
            v.gates = rec
                .awaiting_gates()
                .into_iter()
                .map(|g| GateView {
                    step_id: g.step_id.clone(),
                    prompt: g.prompt.clone(),
                    since: g.since,
                    expires_at: g.expires_at,
                })
                .collect();
        }

        // 3. step_results: loop iteration, host, panel findings by severity.
        if let Ok(records) = store.read_step_results(run_id) {
            for r in &records {
                let s = v.step_mut(&r.step_id);
                if r.loop_iteration.is_some() {
                    s.loop_iteration = r.loop_iteration;
                }
                if r.host.is_some() {
                    s.host = r.host.clone();
                }
                for fnd in &r.findings {
                    *v.findings_by_severity
                        .entry(fnd.severity.to_lowercase())
                        .or_insert(0) += 1;
                }
            }
        }

        // 4. Token/cost totals via the proven fold (one pass, no double-count).
        v.usage = Some(rupu_cp::usage::summarize_run(store, run_id, pricing));

        v
    }

    /// The `"<step>[<unit>]: <message>"` lines of every `StepWarning` in a run's
    /// event log, in order — what `rupu workflow show-run` prints. Folds just
    /// the warnings through [`RunView::apply`] (so the format is the one the
    /// completion summary uses) without the `run.json` / step-results /
    /// usage work of [`RunView::from_run_dir`]. Empty for a run with no
    /// warnings or no readable event log.
    pub fn warnings_from_run_dir(store: &RunStore, run_id: &str) -> Vec<String> {
        let mut v = RunView::default();
        let mut tailer = crate::output::jsonl_reader::WfEventTailer::new(store.events_path(run_id));
        for ev in tailer.drain_events() {
            if matches!(ev, Event::StepWarning { .. }) {
                v.apply(&ev);
            }
        }
        v.warnings
    }

    /// Wall-clock elapsed time: `finished_at − started_at`, or
    /// `now − started_at` while the run is still live. `None` until the run
    /// has started; a clock that runs backwards clamps to 0.
    pub fn elapsed_ms(&self, now: DateTime<Utc>) -> Option<u64> {
        let start = self.started_at?;
        let end = self.finished_at.unwrap_or(now);
        Some((end - start).num_milliseconds().max(0) as u64)
    }
}

/// Hours-aware duration. Under a minute → `"Ns"`; under an hour →
/// `"Mm SSs"`; an hour or more → `"Hh MMm"` (seconds dropped past the hour).
pub fn fmt_hms(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Map a step's lifecycle state onto the shared palette status (glyph +
/// colour). `Paused` reads as `Waiting`: it is parked, not progressing.
pub fn step_status(state: StepState) -> Status {
    match state {
        StepState::Pending => Status::Waiting,
        StepState::Running => Status::Working,
        StepState::AwaitingApproval => Status::Awaiting,
        StepState::Complete => Status::Complete,
        StepState::Failed => Status::Failed,
        StepState::Skipped => Status::Skipped,
        StepState::Paused => Status::Waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::executor::{AttemptResumeMode, Event};
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
    fn step_warnings_are_kept_per_step_and_never_change_a_state() {
        let mut v = RunView::default();
        v.apply(&started("a", StepKind::Run));
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: "a".into(),
            success: true,
            duration_ms: 1_000,
            host: None,
        });
        v.apply(&started("b", StepKind::ForEach));
        let warn = |step: &str, index: Option<usize>, message: &str| Event::StepWarning {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            message: message.into(),
        };
        v.apply(&warn("a", None, "no coverage"));
        v.apply(&warn("b", Some(2), "host went away"));
        v.apply(&warn("b", Some(5), "host went away again"));
        // A warning for a step the run never reached must not conjure one.
        v.apply(&warn("ghost", None, "orphan"));

        assert_eq!(v.steps.len(), 2, "no phantom step for a warned unknown id");
        assert_eq!(v.steps[0].warnings, vec!["no coverage"]);
        assert_eq!(
            v.steps[1].warnings,
            vec!["host went away", "host went away again"]
        );
        // Information only: the step states and the run status are untouched.
        assert_eq!(v.steps[0].state, StepState::Complete);
        assert_eq!(v.steps[1].state, StepState::Running);
        assert_eq!(v.status, RunStatus::default());
        // The flat, summary-facing list carries every warning in order, naming
        // the fan-out unit when there is one (units on one host would
        // otherwise read as the same line).
        assert_eq!(
            v.warnings,
            vec![
                "a: no coverage",
                "b[2]: host went away",
                "b[5]: host went away again",
                "ghost: orphan"
            ]
        );
    }

    #[test]
    fn warnings_from_run_dir_reads_only_the_warnings_of_the_event_log() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        std::fs::create_dir_all(tmp.path().join("runs/run_W")).unwrap();
        let log = [
            r#"{"type":"run_started","event_version":1,"run_id":"run_W","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}"#,
            r#"{"type":"step_started","run_id":"run_W","step_id":"sweep","kind":"for_each","agent":null}"#,
            r#"{"type":"step_warning","run_id":"run_W","step_id":"sweep","index":1,"message":"host gpu-9 sent no coverage"}"#,
            r#"{"type":"step_warning","run_id":"run_W","step_id":"triage","message":"merge skipped"}"#,
        ]
        .join("\n");
        std::fs::write(
            tmp.path().join("runs/run_W/events.jsonl"),
            format!("{log}\n"),
        )
        .unwrap();

        assert_eq!(
            RunView::warnings_from_run_dir(&store, "run_W"),
            vec![
                "sweep[1]: host gpu-9 sent no coverage",
                "triage: merge skipped"
            ]
        );
        assert!(RunView::warnings_from_run_dir(&store, "run_NOPE").is_empty());
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
            cause: None,
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
            cause: None,
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
    fn agent_started_attaches_provider_model_to_singleton_step_only() {
        use rupu_orchestrator::executor::Event;
        use rupu_orchestrator::runs::StepKind;
        let agent_started = |step: &str, model: &str| Event::AgentStarted {
            run_id: "r".into(),
            step_id: step.into(),
            unit_index: None,
            codename: None,
            agent: "a".into(),
            provider: Some("anthropic".into()),
            model: Some(model.into()),
            agent_run_id: "ar".into(),
            transcript_path: "t".into(),
        };
        let mut v = RunView::default();
        v.apply(&started("solo", StepKind::Linear));
        v.apply(&started("crowd", StepKind::Panel));
        v.apply(&agent_started("solo", "claude-opus-5-5"));
        v.apply(&agent_started("crowd", "claude-haiku-4-5"));
        v.apply(&agent_started("crowd", "claude-opus-5-5"));

        let solo = &v.steps[0];
        assert_eq!(solo.provider.as_deref(), Some("anthropic"));
        assert_eq!(solo.model.as_deref(), Some("claude-opus-5-5"));
        // A panel runs several agents under one step id: no single
        // provider/model describes it, so none is recorded.
        let crowd = &v.steps[1];
        assert_eq!(
            (crowd.provider.as_deref(), crowd.model.as_deref()),
            (None, None)
        );
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
            cause: None,
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
            cause: None,
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

    #[test]
    fn from_run_dir_builds_totals_gates_and_crew() {
        use rupu_orchestrator::RunStore;
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs");
        let run_dir = runs.join("run_TEST");
        std::fs::create_dir_all(&run_dir).unwrap();

        // Minimal run.json: workflow_name, codename crew, awaiting gate.
        std::fs::write(
            run_dir.join("run.json"),
            serde_json::json!({
                "id": "run_TEST",
                "workflow_name": "assess-services",
                "status": "awaiting_approval",
                "inputs": {},
                "workspace_id": "ws",
                "workspace_path": tmp.path(),
                "transcript_dir": tmp.path(),
                "started_at": "2026-09-30T10:00:00Z",
                "codename": "mint-tundra",
                "awaiting": [{
                    "step_id": "triage",
                    "prompt": "approve report publish?",
                    "since": "2026-09-30T11:00:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        // events.jsonl: one completed linear step.
        let mut f = std::fs::File::create(run_dir.join("events.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"run_started","event_version":1,"run_id":"run_TEST","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}}"#).unwrap();
        writeln!(f, r#"{{"type":"step_started","run_id":"run_TEST","step_id":"preflight","kind":"run","agent":null}}"#).unwrap();
        writeln!(f, r#"{{"type":"step_completed","run_id":"run_TEST","step_id":"preflight","success":true,"duration_ms":18000}}"#).unwrap();

        let store = RunStore::new(runs);
        let pricing = rupu_config::PricingConfig::default();
        let v = RunView::from_run_dir(&store, "run_TEST", &pricing);

        assert_eq!(v.workflow_name, "assess-services");
        assert_eq!(v.crew.as_deref(), Some("mint-tundra"));
        assert_eq!(v.steps.len(), 1);
        assert_eq!(v.steps[0].state, StepState::Complete);
        assert_eq!(v.gates.len(), 1);
        assert_eq!(v.gates[0].step_id, "triage");
        // No transcripts on disk -> usage folds to a zero summary, not a panic.
        assert_eq!(v.usage.unwrap().total_tokens, 0);
    }

    #[test]
    fn from_run_dir_overlays_step_results_findings_host_and_loop_iteration() {
        use rupu_orchestrator::runs::{FindingRecord, StepResultRecord};
        use rupu_orchestrator::RunStore;
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs");
        let run_dir = runs.join("run_OVL");
        std::fs::create_dir_all(&run_dir).unwrap();

        std::fs::write(
            run_dir.join("run.json"),
            serde_json::json!({
                "id": "run_OVL",
                "workflow_name": "overlay-wf",
                "status": "awaiting_approval",
                "inputs": {},
                "workspace_id": "ws",
                "workspace_path": tmp.path(),
                "transcript_dir": tmp.path(),
                "started_at": "2026-09-30T10:00:00Z",
                "awaiting": [{
                    "step_id": "triage",
                    "prompt": "approve?",
                    "since": "2026-09-30T11:00:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        // Replay creates the "triage" and "impl" steps.
        let mut f = std::fs::File::create(run_dir.join("events.jsonl")).unwrap();
        writeln!(f, r#"{{"type":"run_started","event_version":1,"run_id":"run_OVL","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}}"#).unwrap();
        writeln!(f, r#"{{"type":"step_started","run_id":"run_OVL","step_id":"triage","kind":"panel","agent":null}}"#).unwrap();
        writeln!(f, r#"{{"type":"step_started","run_id":"run_OVL","step_id":"impl","kind":"linear","agent":null}}"#).unwrap();
        drop(f);

        let store = RunStore::new(runs);
        let finding = |severity: &str| FindingRecord {
            source: "panel".into(),
            severity: severity.into(),
            title: "t".into(),
            body: "b".into(),
            codename: None,
        };
        let record = |step_id: &str, kind: StepKind| StepResultRecord {
            run_outcome: None,
            step_id: step_id.into(),
            run_id: "run_OVL".into(),
            transcript_path: tmp.path().join("t.jsonl"),
            output: String::new(),
            success: true,
            skipped: false,
            rendered_prompt: String::new(),
            kind,
            items: vec![],
            findings: vec![],
            iterations: 1,
            resolved: true,
            finished_at: Utc::now(),
            loop_iteration: None,
            host: None,
            codename: None,
            cause: None,
            error: None,
        };
        let mut triage = record("triage", StepKind::Panel);
        triage.findings = vec![finding("High"), finding("high"), finding("low")];
        let mut imp = record("impl", StepKind::Linear);
        imp.host = Some("kuki".into());
        imp.loop_iteration = Some(2);
        store.append_step_result("run_OVL", &triage).unwrap();
        store.append_step_result("run_OVL", &imp).unwrap();

        let pricing = rupu_config::PricingConfig::default();
        let v = RunView::from_run_dir(&store, "run_OVL", &pricing);

        let mut want = BTreeMap::new();
        want.insert("high".to_string(), 2usize);
        want.insert("low".to_string(), 1usize);
        assert_eq!(v.findings_by_severity, want);
        let imp_view = v.steps.iter().find(|s| s.step_id == "impl").unwrap();
        assert_eq!(imp_view.host, Some("kuki".to_string()));
        assert_eq!(imp_view.loop_iteration, Some(2));
        assert_eq!(v.status, RunStatus::AwaitingApproval);
    }

    #[test]
    fn from_run_dir_degrades_when_run_json_missing() {
        use rupu_orchestrator::RunStore;
        let tmp = tempfile::tempdir().unwrap();
        let runs = tmp.path().join("runs");
        // Only the run dir exists: no run.json, no events.jsonl.
        std::fs::create_dir_all(runs.join("run_GONE")).unwrap();

        let store = RunStore::new(runs);
        let pricing = rupu_config::PricingConfig::default();
        let v = RunView::from_run_dir(&store, "run_GONE", &pricing);

        assert_eq!(v.run_id, "run_GONE");
        assert!(v.steps.is_empty());
        assert_eq!(v.usage.unwrap().total_tokens, 0);
    }

    fn unit_started(step: &str, index: usize) -> Event {
        Event::UnitStarted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("svc-{index}"),
            agent: Some("breaker".into()),
            transcript_path: format!("t{index}").into(),
            host: None,
            codename: None,
        }
    }

    fn unit_completed(step: &str, index: usize) -> Event {
        Event::UnitCompleted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("svc-{index}"),
            success: true,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        }
    }

    fn resumed(step: &str, unit_index: Option<usize>, mode: AttemptResumeMode) -> Event {
        Event::AttemptResumed {
            run_id: "r".into(),
            step_id: step.into(),
            unit_index,
            mode,
            from_agent_run_id: Some("run_old".into()),
            reason: None,
        }
    }

    #[test]
    fn a_resumed_unit_keeps_its_mark_through_its_lifecycle() {
        let mut v = RunView::default();
        v.apply(&started("hunt", StepKind::ForEach));
        for i in 0..3 {
            v.apply(&unit_started("hunt", i));
        }
        // Unit 0 is continued, unit 2 recovered (its answer was reused);
        // unit 1 is a plain fresh start.
        v.apply(&resumed("hunt", Some(0), AttemptResumeMode::Continued));
        v.apply(&resumed("hunt", Some(2), AttemptResumeMode::Recovered));
        v.apply(&unit_completed("hunt", 0));
        v.apply(&unit_completed("hunt", 2));

        let units = &v.steps[0].units;
        assert_eq!(units[&0].resumed, Some(ResumedMark::Continued));
        assert_eq!(
            units[&0].status,
            UnitStatus::Done,
            "the mark is not a status"
        );
        assert_eq!(units[&1].resumed, None);
        assert_eq!(units[&2].resumed, Some(ResumedMark::Recovered));
        assert_eq!(
            v.steps[0].resumed, None,
            "the marks are the units', not the step's"
        );
    }

    #[test]
    fn a_resumed_linear_step_is_marked_and_its_units_are_not() {
        let mut v = RunView::default();
        v.apply(&started("only", StepKind::Linear));
        v.apply(&resumed("only", None, AttemptResumeMode::Continued));
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: "only".into(),
            success: true,
            duration_ms: 1_000,
            host: None,
        });

        let step = &v.steps[0];
        assert_eq!(step.resumed, Some(ResumedMark::Continued));
        assert_eq!(step.state, StepState::Complete);
        assert!(step.units.is_empty());
    }

    #[test]
    fn a_restarted_attempt_is_not_marked() {
        let mut v = RunView::default();
        v.apply(&started("only", StepKind::Linear));
        v.apply(&resumed("only", None, AttemptResumeMode::Restarted));
        v.apply(&started("hunt", StepKind::ForEach));
        v.apply(&unit_started("hunt", 0));
        v.apply(&resumed("hunt", Some(0), AttemptResumeMode::Restarted));

        assert_eq!(v.steps[0].resumed, None);
        assert_eq!(v.steps[1].units[&0].resumed, None);
    }

    #[test]
    fn a_unit_start_ends_the_previous_attempts_mark() {
        // events.jsonl spans every resume: the same unit is started again by
        // a later one (or retried onto a fallback host), and that new attempt
        // is not the continuation the earlier mark described.
        let mut v = RunView::default();
        v.apply(&started("hunt", StepKind::ForEach));
        v.apply(&unit_started("hunt", 0));
        v.apply(&resumed("hunt", Some(0), AttemptResumeMode::Continued));
        assert_eq!(v.steps[0].units[&0].resumed, Some(ResumedMark::Continued));

        v.apply(&unit_started("hunt", 0));
        assert_eq!(v.steps[0].units[&0].resumed, None);

        // Same for a linear step started again.
        v.apply(&started("only", StepKind::Linear));
        v.apply(&resumed("only", None, AttemptResumeMode::Recovered));
        assert_eq!(v.steps[1].resumed, Some(ResumedMark::Recovered));
        v.apply(&started("only", StepKind::Linear));
        assert_eq!(v.steps[1].resumed, None);
    }

    #[test]
    fn a_resumption_of_a_unit_never_seen_start_makes_no_phantom_unit() {
        let mut v = RunView::default();
        v.apply(&started("hunt", StepKind::ForEach));
        v.apply(&resumed("hunt", Some(4), AttemptResumeMode::Continued));
        assert!(v.steps[0].units.is_empty());
    }

    #[test]
    fn fmt_hms_is_hours_aware() {
        assert_eq!(fmt_hms(18_000), "18s");
        assert_eq!(fmt_hms(123_000), "2m 03s");
        assert_eq!(fmt_hms(3_840_000), "1h 04m");
        assert_eq!(fmt_hms(11_220_000), "3h 07m");
    }

    #[test]
    fn elapsed_ms_handles_live_finished_and_backwards_clock() {
        use chrono::{Duration, TimeZone};
        let t0 = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();

        // Not started: nothing to measure.
        assert_eq!(RunView::default().elapsed_ms(t0), None);

        // Live: measured against `now`.
        let mut v = RunView {
            started_at: Some(t0),
            ..Default::default()
        };
        assert_eq!(v.elapsed_ms(t0 + Duration::seconds(5)), Some(5_000));

        // Clock running backwards clamps to 0 rather than wrapping to a huge u64.
        assert_eq!(v.elapsed_ms(t0 - Duration::seconds(1)), Some(0));

        // Finished wins: `finished_at`, not `now`.
        v.finished_at = Some(t0 + Duration::seconds(3));
        assert_eq!(v.elapsed_ms(t0 + Duration::seconds(100)), Some(3_000));
    }

    /// A resumed run must not show `0s`. `events.jsonl` spans every
    /// generation, so on attach the view replays the prior generation's
    /// `RunFailed` (which set `finished_at`) and then the resume's
    /// `RunStarted`. Keeping the ORIGINAL start (cumulative, so the timer
    /// never visibly "resets") and clearing the stale `finished_at` (a new
    /// generation is live again) is what stops `elapsed_ms` going
    /// negative-then-clamped-to-0.
    #[test]
    fn a_resume_keeps_the_original_start_and_clears_the_stale_finish() {
        use chrono::{Duration, TimeZone};
        let t0 = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();
        let start = |at: DateTime<Utc>| Event::RunStarted {
            event_version: 1,
            run_id: "r".into(),
            workflow_path: "wf".into(),
            started_at: at,
        };
        let mut v = RunView::default();
        v.apply(&start(t0));
        v.apply(&Event::RunFailed {
            run_id: "r".into(),
            error: "interrupted".into(),
            finished_at: t0 + Duration::hours(1),
        });
        // Prior generation's outcome is frozen at its failure (1h).
        assert_eq!(v.elapsed_ms(t0 + Duration::hours(2)), Some(3_600_000));

        // Resumed three hours after the original start.
        v.apply(&start(t0 + Duration::hours(3)));
        assert_eq!(v.started_at, Some(t0), "keeps the original start");
        assert_eq!(v.finished_at, None, "clears the stale finish");
        assert_eq!(v.generation, 2, "still a new generation");
        // Elapsed now advances against `now` cumulatively — never 0, never
        // negative.
        assert_eq!(
            v.elapsed_ms(t0 + Duration::hours(3) + Duration::minutes(5)),
            Some((3 * 3600 + 5 * 60) * 1000)
        );
    }

    #[test]
    fn frontier_unit_is_the_newest_running_then_the_newest_started() {
        let mk = |index: usize, status: UnitStatus| UnitView {
            index,
            unit_key: format!("u{index}"),
            agent: None,
            codename: None,
            provider: None,
            model: None,
            host: None,
            status,
            resumed: None,
        };
        let mut v = RunView::default();
        // No units: no frontier.
        assert!(v.step_mut("hunt").frontier_unit().is_none());
        // Running wins, highest index among running (newest-started).
        let s = v.step_mut("hunt");
        for (i, st) in [
            (0, UnitStatus::Done),
            (1, UnitStatus::Running),
            (2, UnitStatus::Running),
            (3, UnitStatus::Queued),
        ] {
            s.units.insert(i, mk(i, st));
        }
        assert_eq!(s.frontier_unit().map(|u| u.index), Some(2));
        // None running: the newest unit that has *started* (queued excluded —
        // it has no transcript to stream yet).
        let mut v = RunView::default();
        let s = v.step_mut("hunt");
        s.units.insert(0, mk(0, UnitStatus::Done));
        s.units.insert(1, mk(1, UnitStatus::Failed));
        s.units.insert(9, mk(9, UnitStatus::Queued));
        assert_eq!(s.frontier_unit().map(|u| u.index), Some(1));
        // Only queued units: nothing to follow yet.
        let mut v = RunView::default();
        let s = v.step_mut("hunt");
        s.units.insert(0, mk(0, UnitStatus::Queued));
        assert!(s.frontier_unit().is_none());
    }

    #[test]
    fn step_status_maps_to_palette() {
        use crate::output::palette::Status;
        assert!(matches!(step_status(StepState::Complete), Status::Complete));
        assert!(matches!(step_status(StepState::Running), Status::Working));
        assert!(matches!(
            step_status(StepState::AwaitingApproval),
            Status::Awaiting
        ));
        assert!(matches!(step_status(StepState::Skipped), Status::Skipped));
        assert!(matches!(step_status(StepState::Pending), Status::Waiting));
    }
}
