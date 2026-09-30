# Workflow Live View Redesign — Plan 1: RunView model + correct completion summary

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a correct, unit-tested `RunView` projection of a workflow run and use it to replace the CLI's bare `completed · run_…` line with a truthful completion summary (outcome, hours-aware duration, per-step outcomes, unit counts, correct token/cost totals, failed units, `show-run` pointer).

**Architecture:** A new `run_model.rs` owns `RunView` — a pure state machine with an `apply(&Event)` transition and a `from_run_dir(...)` one-shot builder that replays `events.jsonl`, loads `run.json` (crew codename + `awaiting[]` gates) and `step_results.jsonl`, and folds token/cost totals through the existing `rupu_cp::usage::summarize_run`. A new `run_summary.rs` formats a `RunView` into the completion block. The summary is printed at the single convergence point in `execute_workflow_invocation`, so the live view (A), the retained view (B) and the line printer (C) all get it. This plan touches no rendering loop — that is Plan 2, which drives the same `apply(&Event)` live.

**Tech Stack:** Rust 2021, `rupu-cli` crate. `chrono`, `serde`, `insta` (snapshot tests, already a workspace dep). Reuses `rupu-orchestrator` (`executor::Event`, `runs::{RunStore, RunRecord, RunStatus, StepKind, AwaitingGate, StepResultRecord}`) and `rupu-cp` (`usage::summarize_run`, `usage::UsageSummary`).

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-workflow-live-view-redesign-design.md`

## Global Constraints

- Workspace deps only; never pin a version in a crate `Cargo.toml`. (`insta` is already `insta.workspace = true` in `crates/rupu-cli/Cargo.toml`.)
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- MSRV per `rust-toolchain.toml`; measure the baseline yourself, never assume red.
- `thiserror` for library errors; `anyhow` only in the CLI binary layer.
- **No mock features** (project rule): a summary field that cannot be computed truthfully is **omitted**, never shown as a silent zero. Specifically: the token/cost total is shown only when priced; the CP link is **not** emitted (no CP public-URL config exists yet — deferred, not faked); findings/coverage are handled per Task 6's omission rule.
- Do not run package-wide `cargo fmt`; format only the files you touch (main is fmt-dirty under the pinned toolchain).
- Every change lands on a feature branch via PR; never commit to `main`.
- Snapshot tests assert **structure only** — run them with color disabled (the house pattern: build the string with no ANSI, like `tables.rs`'s `insta::assert_snapshot!`).

---

### Task 1: `RunView` skeleton + run/step lifecycle `apply`

**Files:**
- Create: `crates/rupu-cli/src/output/run_model.rs`
- Modify: `crates/rupu-cli/src/output/mod.rs` (add `pub mod run_model;`)
- Test: inline `#[cfg(test)]` module in `run_model.rs`

**Interfaces:**
- Consumes: `rupu_orchestrator::executor::Event`; `rupu_orchestrator::runs::{RunStatus, StepKind}`.
- Produces (relied on by every later task and by Plan 2):
  - `RunView` with fields exactly as defined below; `RunView::default()`.
  - `RunView::apply(&mut self, ev: &Event)`.
  - `StepView`, `StepState`, `UnitView`, `UnitStatus`, `UnitCounts`, `DispatchView`, `GateView`.
  - `RunView::step_mut(&mut self, step_id: &str) -> &mut StepView` (find-or-insert, preserves first-seen order via `order`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::{RunStatus, StepKind};
    use chrono::Utc;

    fn started(step: &str, kind: StepKind) -> Event {
        Event::StepStarted { run_id: "r".into(), step_id: step.into(), kind,
            agent: None, host: None, codename: None }
    }

    #[test]
    fn linear_lifecycle_sets_states_and_status() {
        let mut v = RunView::default();
        v.apply(&Event::RunStarted { event_version: 1, run_id: "r".into(),
            workflow_path: "wf".into(), started_at: Utc::now() });
        v.apply(&started("a", StepKind::Run));
        v.apply(&Event::StepCompleted { run_id: "r".into(), step_id: "a".into(),
            success: true, duration_ms: 18_000, host: None });
        v.apply(&started("b", StepKind::Run));
        v.apply(&Event::RunCompleted { run_id: "r".into(),
            status: RunStatus::Completed, finished_at: Utc::now() });

        assert_eq!(v.status, RunStatus::Completed);
        assert_eq!(v.generation, 1);
        assert_eq!(v.steps.len(), 2);
        assert_eq!(v.steps[0].state, StepState::Complete);
        assert_eq!(v.steps[0].duration_ms, Some(18_000));
        assert_eq!(v.steps[1].state, StepState::Running);
        assert!(matches!(v.steps[0].kind, StepKind::Run));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: FAIL — `RunView` not found / does not compile.

- [ ] **Step 3: Write minimal implementation**

```rust
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
pub enum UnitStatus { Queued, Running, Done, Failed }

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
pub enum StepState { Pending, Running, AwaitingApproval, Complete, Failed, Skipped, Paused }

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
            Event::RunStarted { run_id, started_at, .. } => {
                self.run_id = run_id.clone();
                self.started_at = Some(*started_at);
                self.status = RunStatus::Running;
                self.error = None;
                self.generation += 1;
            }
            Event::StepStarted { step_id, kind, agent, host, codename, .. } => {
                let s = self.step_mut(step_id);
                s.kind = kind.clone();
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
            Event::StepCompleted { step_id, success, duration_ms, host, .. } => {
                let s = self.step_mut(step_id);
                s.state = if *success { StepState::Complete } else { StepState::Failed };
                s.duration_ms = Some(*duration_ms);
                if host.is_some() { s.host = host.clone(); }
            }
            Event::StepFailed { step_id, error, .. } => {
                let s = self.step_mut(step_id);
                s.state = StepState::Failed;
                if self.error.is_none() { self.error = Some(error.clone()); }
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
            Event::RunCompleted { status, finished_at, .. } => {
                self.status = status.clone();
                self.finished_at = Some(*finished_at);
            }
            Event::RunFailed { error, finished_at, .. } => {
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
```

Add to `crates/rupu-cli/src/output/mod.rs` next to the other `pub mod` lines:

```rust
pub mod run_model;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/run_model.rs crates/rupu-cli/src/output/mod.rs
git commit -m "feat(cli): RunView model skeleton + run/step lifecycle apply"
```

---

### Task 2: fan-out units + the slot-overwrite regression guard

**Files:**
- Modify: `crates/rupu-cli/src/output/run_model.rs` (extend `apply`, add unit arms)
- Test: same inline module

**Interfaces:**
- Consumes: `RunView::apply`, `StepView.units`, `UnitView`, `UnitStatus`, `UnitCounts` (Task 1).
- Produces: `apply` handling of `Event::{UnitStarted, UnitCompleted, AgentStarted}`; units keyed by `index`; provider/model captured from `AgentStarted` onto the matching unit.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn fanout_units_count_and_survive_a_dispatch() {
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::StepKind;
    let mut v = RunView::default();
    v.apply(&Event::StepStarted { run_id: "r".into(), step_id: "hunt".into(),
        kind: StepKind::ForEach, agent: None, host: None, codename: None });
    for i in 0..3usize {
        v.apply(&Event::UnitStarted { run_id: "r".into(), step_id: "hunt".into(),
            index: i, unit_key: format!("svc-{i}"), agent: Some("breaker".into()),
            transcript_path: format!("t{i}").into(), host: None,
            codename: Some(format!("otter#{}", i + 1)) });
    }
    // A dispatch must NOT clobber unit slot 3 (== units.len()); it lives in
    // its own map (Task 3), so units stay intact.
    v.apply(&Event::DispatchStarted { run_id: "r".into(), sub_run_id: "sub1".into(),
        agent: Some("scout".into()), transcript_path: "ts".into(),
        codename: Some("wren#1".into()), provider: None, model: None });
    v.apply(&Event::UnitCompleted { run_id: "r".into(), step_id: "hunt".into(),
        index: 0, unit_key: "svc-0".into(), success: true,
        tokens_in: 0, tokens_out: 0, host: None });
    v.apply(&Event::UnitCompleted { run_id: "r".into(), step_id: "hunt".into(),
        index: 1, unit_key: "svc-1".into(), success: false,
        tokens_in: 0, tokens_out: 0, host: None });

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
    v.apply(&Event::StepStarted { run_id: "r".into(), step_id: "hunt".into(),
        kind: StepKind::ForEach, agent: None, host: None, codename: None });
    v.apply(&Event::UnitStarted { run_id: "r".into(), step_id: "hunt".into(),
        index: 0, unit_key: "svc-0".into(), agent: Some("breaker".into()),
        transcript_path: "t0".into(), host: None, codename: None });
    v.apply(&Event::AgentStarted { run_id: "r".into(), step_id: "hunt".into(),
        unit_index: Some(0), codename: None, agent: "breaker".into(),
        provider: Some("openai".into()), model: Some("gpt-5".into()),
        agent_run_id: "ar".into(), transcript_path: "t0".into() });
    assert_eq!(v.steps[0].units[&0].provider.as_deref(), Some("openai"));
    assert_eq!(v.steps[0].units[&0].model.as_deref(), Some("gpt-5"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: FAIL — units empty (arms are `_ => {}`).

- [ ] **Step 3: Write minimal implementation**

Replace the `// Fan-out … handled in Tasks 2-3.` comment and the `_ => {}` arm with the unit arms (keep a trailing `_ => {}` for the still-unhandled dispatch/panel arms until Task 3):

```rust
            Event::UnitStarted { step_id, index, unit_key, agent, host, codename, .. } => {
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
                if u.codename.is_none() { u.codename = codename.clone(); }
                if u.host.is_none() { u.host = host.clone(); }
            }
            Event::UnitCompleted { step_id, index, success, .. } => {
                let s = self.step_mut(step_id);
                if let Some(u) = s.units.get_mut(index) {
                    u.status = if *success { UnitStatus::Done } else { UnitStatus::Failed };
                }
            }
            Event::AgentStarted { step_id, unit_index: Some(i), provider, model, codename, .. } => {
                let s = self.step_mut(step_id);
                if let Some(u) = s.units.get_mut(i) {
                    if provider.is_some() { u.provider = provider.clone(); }
                    if model.is_some() { u.model = model.clone(); }
                    if u.codename.is_none() { u.codename = codename.clone(); }
                }
            }
            _ => {}
```

Note the `UnitCompleted` arm ignores `tokens_in`/`tokens_out` deliberately — they are always `0` (see the event doc comment); real totals come from the usage fold in Task 4.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/run_model.rs
git commit -m "feat(cli): RunView fan-out units keyed by index (fixes dispatch slot overwrite)"
```

---

### Task 3: sub-agent dispatches (own map) + panel rounds

**Files:**
- Modify: `crates/rupu-cli/src/output/run_model.rs`
- Test: same inline module

**Interfaces:**
- Consumes: `RunView`, `DispatchView`, `RunView.last_active_step` (Task 1).
- Produces: `apply` handling of `Event::{DispatchStarted, DispatchCompleted, PanelRound}`; `RunView.dispatches` keyed by `sub_run_id`; `StepView.{panel_round, panel_max}`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dispatch_lives_in_its_own_map_attributed_to_active_step() {
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::StepKind;
    let mut v = RunView::default();
    v.apply(&Event::StepStarted { run_id: "r".into(), step_id: "assess".into(),
        kind: StepKind::Run, agent: None, host: None, codename: None });
    v.apply(&Event::DispatchStarted { run_id: "r".into(), sub_run_id: "sub1".into(),
        agent: Some("scout".into()), transcript_path: "ts".into(),
        codename: Some("wren#1".into()), provider: Some("anthropic".into()),
        model: Some("opus".into()) });
    v.apply(&Event::DispatchCompleted { run_id: "r".into(), sub_run_id: "sub1".into(),
        success: true, tokens_in: 1000, tokens_out: 200 });

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
    v.apply(&Event::StepStarted { run_id: "r".into(), step_id: "triage".into(),
        kind: StepKind::Panel, agent: None, host: None, codename: None });
    v.apply(&Event::PanelRound { run_id: "r".into(), step_id: "triage".into(),
        round: 2, max_iterations: 5, max_severity_remaining: Some("high".into()) });
    assert_eq!(v.steps[0].panel_round, Some(2));
    assert_eq!(v.steps[0].panel_max, Some(5));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: FAIL — `dispatches` empty, `panel_round` `None`.

- [ ] **Step 3: Write minimal implementation**

Add these arms before the final `_ => {}`:

```rust
            Event::DispatchStarted { sub_run_id, agent, codename, provider, model, .. } => {
                self.dispatches.insert(sub_run_id.clone(), DispatchView {
                    sub_run_id: sub_run_id.clone(),
                    parent_step_id: self.last_active_step.clone(),
                    agent: agent.clone(),
                    codename: codename.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                    status: UnitStatus::Running,
                    tokens_in: 0,
                    tokens_out: 0,
                });
            }
            Event::DispatchCompleted { sub_run_id, success, tokens_in, tokens_out, .. } => {
                if let Some(d) = self.dispatches.get_mut(sub_run_id) {
                    d.status = if *success { UnitStatus::Done } else { UnitStatus::Failed };
                    d.tokens_in = *tokens_in;
                    d.tokens_out = *tokens_out;
                }
            }
            Event::PanelRound { step_id, round, max_iterations, .. } => {
                let s = self.step_mut(step_id);
                s.panel_round = Some(*round);
                s.panel_max = Some(*max_iterations);
            }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/run_model.rs
git commit -m "feat(cli): RunView sub-agent dispatch map + panel round counters"
```

---

### Task 4: `from_run_dir` — replay + usage fold + gates + panel findings

**Files:**
- Modify: `crates/rupu-cli/src/output/run_model.rs`
- Test: same inline module (writes a tiny fixture run dir to a `tempfile::TempDir`)

**Interfaces:**
- Consumes: `RunView::apply`; `rupu_orchestrator::runs::{RunStore, RunRecord, AwaitingGate, StepResultRecord}`; `rupu_cp::usage::summarize_run`; `rupu_config::PricingConfig`; `rupu_cli::output::jsonl_reader::WfEventTailer` (drains `events.jsonl`).
- Produces: `RunView::from_run_dir(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> RunView`.

Confirm `tempfile` is a dev-dep of `rupu-cli` before writing the test: `grep -n 'tempfile' crates/rupu-cli/Cargo.toml`. If absent, add `tempfile.workspace = true` under `[dev-dependencies]` (it is already a workspace dep) in the same commit.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn from_run_dir_builds_totals_gates_and_crew() {
    use std::io::Write;
    use rupu_orchestrator::RunStore;
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let run_dir = runs.join("run_TEST");
    std::fs::create_dir_all(&run_dir).unwrap();

    // Minimal run.json: workflow_name, codename crew, awaiting gate.
    std::fs::write(run_dir.join("run.json"), serde_json::json!({
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
    }).to_string()).unwrap();

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
    // No transcripts on disk → usage folds to a zero summary, not a panic.
    assert!(v.usage.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::run_model::tests::from_run_dir 2>&1 | tail -20`
Expected: FAIL — `from_run_dir` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
use rupu_config::PricingConfig;
use rupu_orchestrator::RunStore;

impl RunView {
    /// Build the full projection of a run from its on-disk artifacts.
    /// Replays `events.jsonl`, overlays `run.json` (crew + gates + status)
    /// and `step_results.jsonl` (loop iteration, host, panel findings), and
    /// folds token/cost totals through the shared `rupu_cp::usage` path
    /// (which already resolves fan-out + remote-mirror transcripts).
    pub fn from_run_dir(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> RunView {
        let mut v = RunView::default();

        // 1. Replay the event log.
        let events_path = store.runs_dir().join(run_id).join("events.jsonl");
        let mut tailer = crate::output::jsonl_reader::WfEventTailer::new(events_path);
        for ev in tailer.drain_events() {
            v.apply(&ev);
        }

        // 2. Overlay run.json.
        if let Ok(rec) = store.load(run_id) {
            v.run_id = rec.id.clone();
            v.workflow_name = rec.workflow_name.clone();
            v.crew = rec.codename.clone();
            v.status = rec.status.clone();
            v.gates = rec.awaiting_gates().into_iter().map(|g| GateView {
                step_id: g.step_id.clone(),
                prompt: g.prompt.clone(),
                since: g.since,
                expires_at: g.expires_at,
            }).collect();
        }

        // 3. step_results: loop iteration, host, panel findings by severity.
        if let Ok(records) = store.read_step_results(run_id) {
            for r in &records {
                let s = v.step_mut(&r.step_id);
                if r.loop_iteration.is_some() { s.loop_iteration = r.loop_iteration; }
                if r.host.is_some() { s.host = r.host.clone(); }
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
}
```

If `RunStore::runs_dir()` does not exist, use the public accessor that does (check with `grep -n 'pub fn runs_dir\|pub fn root\|pub fn dir' crates/rupu-orchestrator/src/runs.rs`) — the run directory is `<runs_dir>/<run_id>/`. If no accessor exists, add a one-line `pub fn runs_dir(&self) -> &Path` to `RunStore` in the same commit (it already stores this path). Confirm `RunRecord` field names (`id`, `workflow_name`, `codename`, `status`) and `awaiting_gates()` with `grep -n` before relying on them.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/run_model.rs crates/rupu-cli/Cargo.toml
git commit -m "feat(cli): RunView::from_run_dir — replay + usage fold + gates + findings"
```

---

### Task 5: hours-aware duration + status mapping helpers

**Files:**
- Modify: `crates/rupu-cli/src/output/run_model.rs`
- Test: same inline module

**Interfaces:**
- Consumes: `StepState`, `UnitStatus`; `rupu_cli::output::palette::Status`.
- Produces:
  - `pub fn fmt_hms(ms: u64) -> String` — `"18s"`, `"2m 03s"`, `"1h 04m"`, `"3h 07m"`.
  - `pub fn step_status(state: StepState) -> rupu_cli::output::palette::Status`.
  - `RunView::elapsed_ms(&self, now: DateTime<Utc>) -> Option<u64>` (finished_at − started_at, else now − started_at).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn fmt_hms_is_hours_aware() {
    assert_eq!(fmt_hms(18_000), "18s");
    assert_eq!(fmt_hms(123_000), "2m 03s");
    assert_eq!(fmt_hms(3_840_000), "1h 04m");
    assert_eq!(fmt_hms(11_220_000), "3h 07m");
}

#[test]
fn step_status_maps_to_palette() {
    use crate::output::palette::Status;
    assert!(matches!(step_status(StepState::Complete), Status::Complete));
    assert!(matches!(step_status(StepState::Running), Status::Working));
    assert!(matches!(step_status(StepState::AwaitingApproval), Status::Awaiting));
    assert!(matches!(step_status(StepState::Skipped), Status::Skipped));
    assert!(matches!(step_status(StepState::Pending), Status::Waiting));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: FAIL — `fmt_hms` / `step_status` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
use crate::output::palette::Status;

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

impl RunView {
    pub fn elapsed_ms(&self, now: DateTime<Utc>) -> Option<u64> {
        let start = self.started_at?;
        let end = self.finished_at.unwrap_or(now);
        Some((end - start).num_milliseconds().max(0) as u64)
    }
}
```

Confirm the `palette::Status` variant names with `grep -n` (Task 1 of the spec listed them: `Waiting, Active, Working, Complete, Failed, SoftFailed, Awaiting, Retrying, Skipped`).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-cli output::run_model 2>&1 | tail -20`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/run_model.rs
git commit -m "feat(cli): hours-aware fmt_hms + StepState→palette::Status mapping"
```

---

### Task 6: `run_summary.rs` — the completion summary formatter

**Files:**
- Create: `crates/rupu-cli/src/output/run_summary.rs`
- Modify: `crates/rupu-cli/src/output/mod.rs` (add `pub mod run_summary;`)
- Test: inline `#[cfg(test)]` snapshot tests

**Interfaces:**
- Consumes: `RunView`, `StepView`, `UnitCounts`, `fmt_hms`, `step_status`, `RunView::elapsed_ms` (Tasks 1-5); `rupu_cp::usage::UsageSummary`.
- Produces: `pub fn render_completion_summary(v: &RunView, now: DateTime<Utc>) -> String`.

Layout rule (matches spec frame 4, "no mock features"):
- Header line: status glyph + `workflow_name` · crew · `completed`/`failed`/… + `fmt_hms(elapsed)`.
- `steps` line: each step glyph + id; a fan-out step annotates `(done/total)`.
- `units` line: only when any step has units — `N ok · M failed`.
- totals line: `⇡in ⇣out  $cost` — the `$cost` segment omitted when `usage.priced == false` or `cost_usd` is `None` (show tokens, not a fake price).
- findings line: only when `findings_by_severity` is non-empty — `N · H HIGH · M MED · …`.
- failed-units line: only when any failed — codenames + keys + `→ resume to retry`.
- `view  rupu workflow show-run <run_id>` always. **No CP URL line** (deferred; see Global Constraints).
- If `status == AwaitingApproval` (gates present): instead of the completion header, render the gate block (reason + since + expires + `run workflow approve/reject`), because the run did not finish.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::run_model::*;
    use chrono::{TimeZone, Utc};
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    fn now() -> chrono::DateTime<Utc> { Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap() }

    fn completed_view() -> RunView {
        let mut v = RunView::default();
        v.run_id = "run_01M1SSABCDEFG".into();
        v.workflow_name = "assess-services".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Completed;
        v.started_at = Some(Utc.with_ymd_and_hms(2026, 9, 30, 10, 12, 40).unwrap());
        v.finished_at = Some(now());
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 6_100_000, output_tokens: 420_000, total_tokens: 6_520_000,
            cost_usd: Some(18.40), priced: true, ..Default::default()
        });
        // one completed linear step + one fan-out with 2 failed
        v.apply(&rupu_orchestrator::executor::Event::StepStarted {
            run_id: "r".into(), step_id: "preflight".into(), kind: StepKind::Run,
            agent: None, host: None, codename: None });
        v.apply(&rupu_orchestrator::executor::Event::StepCompleted {
            run_id: "r".into(), step_id: "preflight".into(), success: true,
            duration_ms: 18000, host: None });
        v
    }

    #[test]
    fn completed_summary_snapshot() {
        let s = render_completion_summary(&completed_view(), now());
        insta::assert_snapshot!(s);
    }

    #[test]
    fn cost_omitted_when_unpriced() {
        let mut v = completed_view();
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 100, output_tokens: 50, total_tokens: 150,
            cost_usd: None, priced: false, ..Default::default()
        });
        let s = render_completion_summary(&v, now());
        assert!(!s.contains('$'), "unpriced run must not print a fake cost:\n{s}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli output::run_summary 2>&1 | tail -20`
Expected: FAIL — `render_completion_summary` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
//! Completion-summary formatter: a `RunView` → the block printed when a
//! `workflow run` / `resume` finishes (or parks at a gate). Shared by the
//! live view (A), retained view (B), and line printer (C).

use chrono::{DateTime, Utc};

use crate::output::palette::Status;
use crate::output::run_model::{fmt_hms, step_status, RunView, StepState, UnitStatus};
use rupu_orchestrator::runs::RunStatus;

fn compact_run_id(id: &str) -> String {
    // Reuse the house compaction if available; fall back to a head…tail.
    crate::output::ids::compact_id(id)
}

pub fn render_completion_summary(v: &RunView, now: DateTime<Utc>) -> String {
    let mut out = String::new();
    let elapsed = v.elapsed_ms(now).map(fmt_hms).unwrap_or_default();

    if v.status == RunStatus::AwaitingApproval && !v.gates.is_empty() {
        out.push_str(&format!(
            "{} {} · {} awaiting approval · {}\n",
            Status::Awaiting.glyph(),
            v.workflow_name,
            v.crew.as_deref().unwrap_or("-"),
            elapsed,
        ));
        for g in &v.gates {
            out.push_str(&format!(
                "  gate · {} — {}\n",
                g.step_id,
                g.prompt.as_deref().unwrap_or("approve to continue"),
            ));
        }
        out.push_str(&format!(
            "  approve  rupu workflow approve {}\n  reject   rupu workflow reject {}\n",
            compact_run_id(&v.run_id), compact_run_id(&v.run_id),
        ));
        return out;
    }

    let status_glyph = match v.status {
        RunStatus::Completed => Status::Complete,
        RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled => Status::Failed,
        RunStatus::Paused => Status::Waiting,
        _ => Status::Working,
    }.glyph();
    out.push_str(&format!(
        "{} {} · {}    {:?} · {}\n",
        status_glyph, v.workflow_name, v.crew.as_deref().unwrap_or("-"),
        v.status, elapsed,
    ));

    // steps line
    let mut steps = String::from("steps   ");
    for s in &v.steps {
        let g = step_status(s.state).glyph();
        let c = s.unit_counts();
        if c.total > 0 {
            steps.push_str(&format!("{} {} ({}/{})  ", g, s.step_id, c.done, c.total));
        } else {
            steps.push_str(&format!("{} {}  ", g, s.step_id));
        }
    }
    out.push_str(steps.trim_end());
    out.push('\n');

    // units line (only when there are units anywhere)
    let (ok, failed): (usize, usize) = v.steps.iter().flat_map(|s| s.units.values()).fold(
        (0, 0),
        |(ok, bad), u| match u.status {
            UnitStatus::Done => (ok + 1, bad),
            UnitStatus::Failed => (ok, bad + 1),
            _ => (ok, bad),
        },
    );
    if ok + failed > 0 {
        out.push_str(&format!("units   {ok} ok · {failed} failed\n"));
    }

    // totals line (cost only when priced)
    if let Some(u) = &v.usage {
        let mut line = format!("tokens  ⇡{} ⇣{}", u.input_tokens, u.output_tokens);
        if let Some(cost) = u.cost_usd {
            if u.priced {
                line.push_str(&format!("  ${cost:.2}"));
            }
        }
        out.push_str(&line);
        out.push('\n');
    }

    // findings (only when present)
    if !v.findings_by_severity.is_empty() {
        let total: usize = v.findings_by_severity.values().sum();
        let mut parts = vec![format!("findings {total}")];
        for (sev, n) in &v.findings_by_severity {
            parts.push(format!("{n} {}", sev.to_uppercase()));
        }
        out.push_str(&parts.join(" · "));
        out.push('\n');
    }

    // failed units
    let failed_units: Vec<String> = v.steps.iter().flat_map(|s| s.units.values())
        .filter(|u| u.status == UnitStatus::Failed)
        .map(|u| match &u.codename {
            Some(c) => format!("{c} ({})", u.unit_key),
            None => u.unit_key.clone(),
        })
        .collect();
    if !failed_units.is_empty() {
        out.push_str(&format!("failed  {}  → resume to retry\n", failed_units.join("  ")));
    }

    out.push_str(&format!("view    rupu workflow show-run {}\n", compact_run_id(&v.run_id)));
    out
}
```

Verify `palette::Status::glyph()` exists and `ids::compact_id` exists (both were confirmed in the spec's prior art). If `compact_id` takes a different name, use whatever `crates/rupu-cli/src/output/ids.rs` exposes.

- [ ] **Step 4: Run test to verify it passes + review the snapshot**

Run: `cargo test -p rupu-cli output::run_summary 2>&1 | tail -20`
Expected: the snapshot test writes a `.snap.new`; run `cargo insta review` (or inspect `crates/rupu-cli/src/output/snapshots/`) and accept it once the block reads correctly. `cost_omitted_when_unpriced` must PASS outright.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli/src/output/run_summary.rs crates/rupu-cli/src/output/mod.rs crates/rupu-cli/src/output/snapshots/
git commit -m "feat(cli): completion-summary formatter from RunView (truthful cost/findings)"
```

---

### Task 7: wire the summary into `workflow run` / `resume` completion

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (the convergence point after `workflow_result` in `execute_workflow_invocation`, ~`workflow.rs:4929`+; and the `run`/`resume` completion prints at `~4028` and `~3429`)
- Test: `crates/rupu-cli/tests/cli_workflow_summary.rs` (new) — builds a fixture run dir and asserts the printed block; OR an inline test on a small helper.

**Interfaces:**
- Consumes: `RunView::from_run_dir`, `render_completion_summary` (Tasks 4, 6); `runs_dir`, `run_id`, `cfg.pricing` already in scope at the convergence point.
- Produces: the completion summary printed once per run, replacing the bare `completed · run_…` line; the gate block printed when the run parked.

- [ ] **Step 1: Write the failing test**

```rust
// crates/rupu-cli/tests/cli_workflow_summary.rs
use std::io::Write;

#[test]
fn completion_summary_is_rendered_from_a_run_dir() {
    // Exercises the public path Task 6 formats: build a RunView from a
    // fixture run dir and assert the block. (The CLI wiring calls exactly
    // this.) Uses the crate's own test entry point for from_run_dir via a
    // thin re-export, or duplicates the fixture write from Task 4's test.
    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join("runs");
    let run_dir = runs.join("run_TEST");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("run.json"), serde_json::json!({
        "id": "run_TEST", "workflow_name": "assess-services", "status": "completed",
        "inputs": {}, "workspace_id": "ws", "workspace_path": tmp.path(),
        "transcript_dir": tmp.path(), "started_at": "2026-09-30T10:00:00Z",
        "finished_at": "2026-09-30T10:00:18Z", "codename": "mint-tundra"
    }).to_string()).unwrap();
    let mut f = std::fs::File::create(run_dir.join("events.jsonl")).unwrap();
    writeln!(f, r#"{{"type":"run_started","event_version":1,"run_id":"run_TEST","workflow_path":"wf","started_at":"2026-09-30T10:00:00Z"}}"#).unwrap();
    writeln!(f, r#"{{"type":"step_started","run_id":"run_TEST","step_id":"preflight","kind":"run","agent":null}}"#).unwrap();
    writeln!(f, r#"{{"type":"step_completed","run_id":"run_TEST","step_id":"preflight","success":true,"duration_ms":18000}}"#).unwrap();
    writeln!(f, r#"{{"type":"run_completed","run_id":"run_TEST","status":"completed","finished_at":"2026-09-30T10:00:18Z"}}"#).unwrap();

    let store = rupu_orchestrator::RunStore::new(runs);
    let v = rupu_cli::output::run_model::RunView::from_run_dir(
        &store, "run_TEST", &rupu_config::PricingConfig::default());
    let block = rupu_cli::output::run_summary::render_completion_summary(&v, chrono::Utc::now());
    assert!(block.contains("assess-services"));
    assert!(block.contains("mint-tundra"));
    assert!(block.contains("preflight"));
    assert!(block.contains("show-run"));
}
```

Confirm `rupu-cli` exposes `output` publicly (`grep -n 'pub mod output' crates/rupu-cli/src/lib.rs`); if `output` or the two modules are private, make them `pub` (or add a thin `pub use`) in the same commit — a CLI integration test can only reach public items.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli --test cli_workflow_summary 2>&1 | tail -20`
Expected: FAIL — module path not public / not found.

- [ ] **Step 3: Write minimal implementation**

At the convergence point in `execute_workflow_invocation`, after `workflow_result` is obtained and before the function returns, print the summary once. Locate the existing terminal print (the bare `completed · run_…`; in the live path it is emitted inside `live_run.rs:1862`, and the `run` path discards the outcome at `workflow.rs:4028`). Replace/augment with:

```rust
// Shared completion summary — one truthful block for A/B/C.
{
    let store = rupu_orchestrator::RunStore::new(runs_dir.clone());
    let view = crate::output::run_model::RunView::from_run_dir(
        &store, &run_id, &cfg.pricing);
    let block = crate::output::run_summary::render_completion_summary(
        &view, chrono::Utc::now());
    // Print via the same sink the rest of the command uses (respect --no-color,
    // shared_printer). If a shared_printer is set, route through it; else stdout.
    print!("{block}");
}
```

Remove the now-redundant bare `completed · run_…` line from `live_run.rs:1862` (the summary supersedes it). For the `resume` path (`workflow.rs:3429`+), the per-step `rupu: step … -> path` lines stay; append the same summary block after them. Ensure the block prints for the awaiting-gate case too (the formatter already branches on `status == AwaitingApproval`).

Guard: `from_run_dir` must not panic if `run.json` is missing (e.g. a run that failed before persisting) — it already degrades to an empty `RunView`; in that case skip printing an empty block (`if !view.run_id.is_empty()`).

- [ ] **Step 4: Run tests + clippy**

Run: `cargo test -p rupu-cli --test cli_workflow_summary 2>&1 | tail -20`
Expected: PASS.
Run: `cargo clippy -p rupu-cli --all-targets 2>&1 | tail -20`
Expected: no new warnings.

- [ ] **Step 5: Manual smoke (non-CI, record result)**

Run a real short workflow and confirm the summary prints once and reads correctly in a real terminal (per the project's GUI/real-run rule — `make macos-test` green ≠ rendering green applies in spirit to terminal output too):

```bash
cargo run -p rupu-cli -- workflow run <some-short-workflow>
```

Expected: a completion block with status, duration, steps, tokens (cost only if `[pricing]` is configured), and a `show-run` line. Note the outcome in the PR description.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/workflow.rs crates/rupu-cli/src/output/live_run.rs crates/rupu-cli/src/lib.rs crates/rupu-cli/tests/cli_workflow_summary.rs
git commit -m "feat(cli): print a truthful completion summary for workflow run/resume"
```

---

## Self-Review (completed against the spec)

- **Spec coverage (Plan 1 scope):** correctness fixes #1 (usage via shared fold, no double-count) ✓ Task 4; #3 (frontier is a set / no single-active assumption — the model holds all steps, no reset) ✓ Tasks 1-3; #6 (panel round populated; findings from step_results; the "no mock features" omission rule for cost/findings/CP-link) ✓ Tasks 3,4,6; #7 (sub-agent slot overwrite) ✓ Task 2; #8 (hours-aware + `fmt_hms`) ✓ Task 5; completion summary ✓ Tasks 6-7; gate-park text in the terminal summary ✓ Task 6. Fixes #2 (frontier never scrolls off), #4 (density bar), #5 (firehose/feed liveness), #9 (role-badge tint), #10 (live gate-park interaction), and the diff renderer are **Plan 2** (the render loop) and out of this plan's scope by design.
- **Placeholder scan:** no TBD/TODO; every code step has real code; test bodies are complete; `grep` confirmations are called out where a signature must be verified before use (`runs_dir`, `RunRecord` fields, `awaiting_gates`, `palette::Status`, `ids::compact_id`, `pub mod output`, `tempfile` dev-dep).
- **Type consistency:** `RunView`, `StepView`, `StepState`, `UnitView`, `UnitStatus`, `UnitCounts`, `DispatchView`, `GateView`, `fmt_hms`, `step_status`, `from_run_dir`, `render_completion_summary` are named identically across all tasks. Units keyed by `index: usize` throughout; dispatches keyed by `sub_run_id: String` throughout.

## Plan 2 preview (not part of this plan)

Plan 2 consumes the now-trusted `RunView` + `apply(&Event)` and builds the render loop: `transcript_mux` (bounded multi-tail firehose within the fd budget), `live_layout` (adaptive rows — collapse settled regions, density bar + movers, frontier + feed always visible; snapshot-tested), `live_render` (diff redraw → flicker gone), `NavState` (run→step→unit→sub-agent drill with the breadcrumb + full key scheme), in-view gate approve/reject, and the resume-generation terminal-event guard. It rewrites `live_run.rs` to drive these, leaving the four modules independently testable.
