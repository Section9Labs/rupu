//! Discovery on resume: what to do with the agent attempts an interrupted run
//! left behind (spec `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md`
//! §3).
//!
//! `rupu workflow resume` used to re-dispatch every step and `for_each` unit
//! that had no recorded result, throwing away whatever the interrupted agent
//! had already done. [`discover`] looks at the **latest attempt** of each such
//! step / unit and asks [`prepare_continuation`] what its transcript says:
//!
//! | transcript                             | plan                          |
//! |----------------------------------------|-------------------------------|
//! | finished (`RunComplete: ok`, or the final answer landed) | [`AttemptPlan::Recovered`] — no model call |
//! | died / paused / killed mid-run         | [`AttemptPlan::Continue`]     |
//! | ended in failure, or can't be read     | [`AttemptPlan::Restart`]      |
//!
//! Attempts come from the per-run `attempts.jsonl` ledger, written as each
//! attempt *starts*. A run that predates the ledger has none, so the same
//! attempts are derived from its `events.jsonl` instead — the transcript
//! paths and run ids are in there too (`unit_started`, `agent_started`,
//! `step_working`).
//!
//! This module only *plans*; the runner consumes the plans (fan-out units and
//! linear steps). Everything else keeps today's restart behaviour: steps that
//! already finished, `parallel` / `panel` steps (continuing those is deferred),
//! members of a `loops:` subgraph (the "done" check would have to be
//! iteration-aware), and attempts placed on a remote host (a remote transcript
//! can't be continued from here — see [`classify`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rupu_agent::continuation::{prepare_continuation, Continuation};
use tracing::warn;

use crate::executor::Event;
use crate::runs::{AttemptRecord, RunStore};
use crate::workflow::{Step, Workflow};

/// What to do with one interrupted agent attempt on resume.
#[derive(Debug, Clone)]
pub enum AttemptPlan {
    /// The attempt can be picked up where it stopped: seed a new run from
    /// `transcript` (the interrupted attempt's) and record
    /// `from_agent_run_id` as the new attempt's `continued_from`.
    Continue {
        transcript: PathBuf,
        from_agent_run_id: String,
    },
    /// The attempt had in fact finished; only its record was lost. Use
    /// `output` as the result — no dispatch, no model call.
    Recovered {
        output: String,
        agent_run_id: String,
        transcript: PathBuf,
    },
    /// Nothing to continue (it failed, or its transcript is missing or
    /// unusable): dispatch a fresh attempt, as resume always did. `reason` is
    /// for the operator.
    Restart { reason: String },
}

/// The plans for one step: its single attempt (linear step) or one per
/// `for_each` unit index. A step has one or the other, never both.
#[derive(Debug, Clone, Default)]
pub struct StepPlan {
    pub linear: Option<AttemptPlan>,
    pub units: BTreeMap<usize, AttemptPlan>,
}

impl StepPlan {
    /// No attempt in this step has a plan.
    pub fn is_empty(&self) -> bool {
        self.linear.is_none() && self.units.is_empty()
    }

    /// How many of this step's plans are of each kind.
    pub fn counts(&self) -> PlanCounts {
        let mut counts = PlanCounts::default();
        for plan in self.linear.iter().chain(self.units.values()) {
            match plan {
                AttemptPlan::Continue { .. } => counts.continued += 1,
                AttemptPlan::Recovered { .. } => counts.recovered += 1,
                AttemptPlan::Restart { .. } => counts.restarted += 1,
            }
        }
        counts
    }
}

/// Per-kind plan counts for one step — what the CLI prints before dispatching
/// (`1 continued · 1 recovered · 1 restarted`; kinds with a zero count are
/// left out).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanCounts {
    pub continued: usize,
    pub recovered: usize,
    pub restarted: usize,
}

impl PlanCounts {
    pub fn total(&self) -> usize {
        self.continued + self.recovered + self.restarted
    }
}

impl fmt::Display for PlanCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = [
            (self.continued, "continued"),
            (self.recovered, "recovered"),
            (self.restarted, "restarted"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, label)| format!("{n} {label}"))
        .collect();
        f.write_str(&parts.join(" · "))
    }
}

/// Every step's plans, keyed by step id. Only steps with at least one plan
/// are present.
#[derive(Debug, Clone, Default)]
pub struct RecoveryPlans(pub BTreeMap<String, StepPlan>);

impl RecoveryPlans {
    /// No step has a plan (nothing was interrupted, or discovery was skipped).
    pub fn is_empty(&self) -> bool {
        self.0.values().all(StepPlan::is_empty)
    }

    /// The plans for `step_id`, if it has any.
    pub fn step(&self, step_id: &str) -> Option<&StepPlan> {
        self.0.get(step_id).filter(|p| !p.is_empty())
    }

    /// Per-step counts, in step-id order — one entry per step with a plan.
    pub fn summary(&self) -> BTreeMap<String, PlanCounts> {
        self.0
            .iter()
            .filter(|(_, plan)| !plan.is_empty())
            .map(|(step_id, plan)| (step_id.clone(), plan.counts()))
            .collect()
    }
}

/// One agent attempt, from the ledger or derived from events.
#[derive(Debug, Clone)]
struct Attempt {
    step_id: String,
    unit_index: Option<usize>,
    agent_run_id: String,
    transcript_path: PathBuf,
    host: Option<String>,
}

/// Work out a plan for every interrupted attempt of `run_id` that the resume
/// would otherwise redo.
///
/// - `done_step_ids`: steps that already have a recorded result — never
///   planned.
/// - `settled_units`: the `for_each` units covered by a *successful*
///   checkpoint (the resume's `completed_units` key set) — replayed as done,
///   never planned. A unit absent from it, or whose checkpoint failed, is
///   planned from its latest attempt's transcript (so a unit killed by
///   SIGTERM, which checkpoints `success: false` but ends `aborted`, is
///   continued rather than treated as a genuine failure).
///
/// Only linear steps and `for_each` steps get plans. `loops:` members,
/// `parallel`/`panel` steps and non-agent nodes are skipped (they restart as
/// before). Reads transcripts synchronously.
pub fn discover(
    store: &RunStore,
    run_id: &str,
    wf: &Workflow,
    done_step_ids: &BTreeSet<String>,
    settled_units: &BTreeMap<String, BTreeMap<usize, ()>>,
) -> RecoveryPlans {
    let mut plans = RecoveryPlans::default();
    for ((step_id, unit_index), attempt) in latest_attempts(load_attempts(store, run_id)) {
        if done_step_ids.contains(&step_id) {
            continue;
        }
        let Some(step) = wf.steps.iter().find(|s| s.id == step_id) else {
            continue;
        };
        if is_loop_member(wf, &step_id) {
            continue;
        }
        match (shape(step), unit_index) {
            (Shape::Linear, None) => {
                plans.0.entry(step_id).or_default().linear = Some(classify(step, &attempt));
            }
            (Shape::ForEach, Some(idx)) => {
                if settled_units
                    .get(&step_id)
                    .is_some_and(|units| units.contains_key(&idx))
                {
                    continue;
                }
                plans
                    .0
                    .entry(step_id)
                    .or_default()
                    .units
                    .insert(idx, classify(step, &attempt));
            }
            // A linear attempt on a `for_each` step (or the reverse) isn't an
            // attempt of that step's own work; anything else is out of scope.
            _ => {}
        }
    }
    plans
}

/// Decide the plan for one attempt from its transcript.
fn classify(step: &Step, attempt: &Attempt) -> AttemptPlan {
    // A placed attempt ran on another host, so its transcript lives there:
    // continuing it locally would silently run it on the wrong host, and the
    // dispatcher has no way to continue it remotely yet (spec §5, a later
    // release). Say so rather than plan something the runner can't honour.
    if let Some(host) = placed_on(step, attempt) {
        return AttemptPlan::Restart {
            reason: format!("placed on {host}; continuing a remote attempt isn't supported yet"),
        };
    }
    match prepare_continuation(&attempt.transcript_path) {
        Ok(Continuation::Finished { output }) => AttemptPlan::Recovered {
            output,
            agent_run_id: attempt.agent_run_id.clone(),
            transcript: attempt.transcript_path.clone(),
        },
        Ok(Continuation::Resume { .. }) => AttemptPlan::Continue {
            transcript: attempt.transcript_path.clone(),
            from_agent_run_id: attempt.agent_run_id.clone(),
        },
        Ok(Continuation::Failed { .. }) => AttemptPlan::Restart {
            reason: "previous attempt failed".to_string(),
        },
        // A ledger row can name a transcript that was never written (a
        // placement or pack failure before any agent started), so an
        // unreadable transcript is an ordinary "start over", not an error.
        Err(e) => AttemptPlan::Restart {
            reason: e.to_string(),
        },
    }
}

/// The host an attempt was placed on, if it was placed at all.
fn placed_on(step: &Step, attempt: &Attempt) -> Option<String> {
    attempt
        .host
        .clone()
        .or_else(|| step.host.clone())
        .or_else(|| step.distribute.as_ref().map(|_| "a fleet host".to_string()))
}

/// What kind of work a step's agent attempts are.
enum Shape {
    /// One agent attempt (`agent` + `prompt`).
    Linear,
    /// One attempt per unit of a `for_each` fan-out.
    ForEach,
    /// `parallel` / `panel` / branch / action / run / split / join / gate:
    /// no plan.
    Other,
}

fn shape(step: &Step) -> Shape {
    if step.for_each.is_some() {
        return Shape::ForEach;
    }
    if step.parallel.is_some()
        || step.panel.is_some()
        || step.branch.is_some()
        || step.action.is_some()
        || step.run.is_some()
        || step.split.is_some()
        || step.join.is_some()
    {
        return Shape::Other;
    }
    if step.agent.is_some() && step.prompt.is_some() {
        Shape::Linear
    } else {
        Shape::Other
    }
}

fn is_loop_member(wf: &Workflow, step_id: &str) -> bool {
    wf.loops
        .values()
        .any(|def| def.nodes.iter().any(|n| n == step_id))
}

/// The latest attempt (by file order) per `(step, unit)`.
fn latest_attempts(attempts: Vec<Attempt>) -> BTreeMap<(String, Option<usize>), Attempt> {
    let mut latest = BTreeMap::new();
    for a in attempts {
        latest.insert((a.step_id.clone(), a.unit_index), a);
    }
    latest
}

/// The run's attempts in file order: the ledger, or — when there is none —
/// the attempts derivable from `events.jsonl`. Ledger rows for a `parallel` /
/// `panel` sub-step (`sub_id`) are dropped: they never belong to a linear or
/// `for_each` attempt.
fn load_attempts(store: &RunStore, run_id: &str) -> Vec<Attempt> {
    match store.read_attempts(run_id) {
        Ok(rows) if !rows.is_empty() => {
            return rows
                .into_iter()
                .filter(|r: &AttemptRecord| r.sub_id.is_none())
                .map(|r| Attempt {
                    step_id: r.step_id,
                    unit_index: r.unit_index,
                    agent_run_id: r.agent_run_id,
                    transcript_path: r.transcript_path,
                    host: r.host,
                })
                .collect();
        }
        Ok(_) => {}
        Err(e) => {
            warn!(run_id, error = %e, "can't read attempts.jsonl; falling back to events.jsonl")
        }
    }
    attempts_from_events(&store.events_path(run_id))
}

/// Derive attempts from an `events.jsonl`: `unit_started` (a `for_each` unit's
/// transcript + host), `step_working` carrying a transcript path (a linear
/// step's), and `agent_started` (the agent run id, for either). A transcript
/// path is one attempt; the events that name the same path refine it.
/// Unparseable lines and a missing file are skipped.
fn attempts_from_events(events_path: &Path) -> Vec<Attempt> {
    let Ok(file) = std::fs::File::open(events_path) else {
        return Vec::new();
    };
    let mut found = EventAttempts::default();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue;
        };
        match event {
            Event::UnitStarted {
                step_id,
                index,
                transcript_path,
                host,
                ..
            } => found.observe(&step_id, Some(index), None, &transcript_path, host),
            Event::StepWorking {
                step_id,
                transcript_path: Some(path),
                ..
            } => found.observe(&step_id, None, None, &path, None),
            Event::AgentStarted {
                step_id,
                unit_index,
                agent_run_id,
                transcript_path,
                ..
            } => found.observe(
                &step_id,
                unit_index,
                Some(agent_run_id),
                &transcript_path,
                None,
            ),
            _ => {}
        }
    }
    found.attempts
}

/// Attempts accumulated from a scan of the events, in first-seen order.
#[derive(Default)]
struct EventAttempts {
    attempts: Vec<Attempt>,
    /// The latest attempt per (step, unit), as an index into `attempts`.
    latest: HashMap<(String, Option<usize>), usize>,
}

impl EventAttempts {
    /// An event named `path` as the transcript of `(step_id, unit_index)`'s
    /// attempt. The same path as that unit's latest attempt refines it (the
    /// run id, the host); a different path is a new attempt. `run_id` is
    /// absent on events that don't carry one — the transcript's file name
    /// stands in.
    fn observe(
        &mut self,
        step_id: &str,
        unit_index: Option<usize>,
        run_id: Option<String>,
        path: &Path,
        host: Option<String>,
    ) {
        let key = (step_id.to_string(), unit_index);
        if let Some(&i) = self.latest.get(&key) {
            let attempt = &mut self.attempts[i];
            if attempt.transcript_path == path {
                if let Some(run_id) = run_id {
                    attempt.agent_run_id = run_id;
                }
                if attempt.host.is_none() {
                    attempt.host = host;
                }
                return;
            }
        }
        self.latest.insert(key, self.attempts.len());
        self.attempts.push(Attempt {
            step_id: step_id.to_string(),
            unit_index,
            agent_run_id: run_id.unwrap_or_else(|| run_id_of(path)),
            transcript_path: path.to_path_buf(),
            host,
        });
    }
}

/// An agent run's id, from its transcript's file name (`<run_id>.jsonl`).
fn run_id_of(transcript: &Path) -> String {
    transcript
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| transcript.display().to_string())
}
