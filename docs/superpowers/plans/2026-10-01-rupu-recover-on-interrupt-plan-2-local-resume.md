# Recover on interrupt — Plan 2: local resume continues interrupted work

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rupu workflow resume` continues each interrupted **linear step** and **`for_each` unit** from its transcript (or recovers a finished-but-unrecorded one with no model call) instead of restarting it, on by default, with `--restart-interrupted` to opt out — covering runs interrupted before this ships via an `events.jsonl` fallback.

**Architecture:** A per-run `attempts.jsonl` ledger records every agent attempt as it starts. On resume, a new `rupu_orchestrator::recovery` module reads the ledger (or derives it from `events.jsonl`), and for each not-yet-settled unit asks `rupu_agent::continuation::prepare_continuation` what to do. The resulting plans ride on `ResumeState`; the fan-out and linear runners consume them — continuing via the existing post-`build_opts_for_step` override (extended to carry a `seed_source`), recovering by folding in a synthesized result, or restarting (flagged). A new `AttemptResumed` executor event and a one-line-per-step CLI summary report what happened.

**Tech Stack:** Rust 2021, tokio, serde_json, thiserror (libraries), anyhow (CLI), clap.

**Spec:** `docs/superpowers/specs/2026-10-01-rupu-recover-on-interrupt-design.md` (§2 ledger, §3 discovery, §4 resume integration, §6 events/CLI). This plan implements Rollout step 2 **narrowed to linear + `for_each`**; see Scope.

## Scope (what this plan does and defers)

**In:** the attempts ledger; discovery with the `events.jsonl` fallback; `ResumeState` plans; **linear** and **`for_each`** Continue/Recovered/Restart; `--restart-interrupted`; `AttemptResumed` + CLI summary; a shared `last_assistant_text` helper (deferred from PR 1's final review); and a resume-path orphan-reap so a crashed run is resumable without `cp serve`.

**Deferred (ruling, recorded below):**
- **`parallel` sub-steps and `panel` members** keep restarting (flagged). Parallel continue needs the same wiring as `for_each` but with no checkpoints; panel needs durable round state. Both go to a PR 2b.
- **Loop-member steps** (any step named in `workflow.loops`) keep restarting (flagged). The ledger/"settled" check would have to be loop-iteration aware; out of this plan.
- **Converging graceful pause off `paused_seeds.json`** onto the transcript path. `paused_seeds.json` keeps working unchanged; the new path handles crash/kill/terminal resumes.
- **The CP web "restart interrupted" option.** The record-field ripple (~67 `RunRecord` literals) and the `HostConnector::resume_run` signature change are batched into PR 3 with the rest of the CP display work. This plan ships the **CLI** flag only.
- **Remote `distribute:` units** (whole of §5) — PR 3.

## Global Constraints

- **Rulings this plan already made** (an executor records them; they bind every task):
  1. **Settled-vs-interrupted is decided from the transcript, not the checkpoint success flag.** A `for_each` unit with a `success: true` checkpoint is settled (replayed as today, no transcript read). A unit that is absent, or whose checkpoint is `success: false`, has its latest attempt classified by `prepare_continuation`: `Finished` → recovered, `Resume` → continue, `Failed`/`Err` → restart (flagged). This is why a SIGTERM-aborted unit (which writes a `success: false` checkpoint ending `aborted`) is continued, not treated as a genuine failure.
  2. **Loop members and `parallel`/`panel` are out of scope** (see Deferred). Discovery emits no plan for them; they restart as today.
  3. **CP web option deferred to PR 3** (see Deferred).
- `rupu-cli` is thin: discovery and classification live in `rupu-orchestrator`/`rupu-agent`; the CLI parses the flag and formats the summary.
- Workspace deps only (no versions in a crate `Cargo.toml`). Libraries use `thiserror`; the CLI uses `anyhow`. `unsafe_code` forbidden.
- `#![deny(clippy::all)]`: `cargo clippy -p <crate> --all-targets -- -D warnings` clean (allow the pre-existing `-A clippy::question_mark` in `rupu-cli/src/cmd/completers.rs`; don't touch that file).
- Integration tests: modules of the one `tests/it` binary per crate (listed in `tests/it/main.rs`); never a new top-level `tests/*.rs`. Env/cwd-mutating `rupu-cli` tests go in `tests/serial/` holding `ENV_LOCK`.
- `OrchestratorRunOpts` and `RunRecord` have **no `Default`** and ~160 / ~67 literal sites respectively — **do not add fields to them.** `ResumeState` derives `Default`; add fields there and use `..Default::default()` at literals (only `from_approval`/`from_rejection` spell every field).
- `opts` is wrapped in `Arc` for the DAG scheduler, so any new `ResumeState`/plan type must be `Clone`.
- Format only files you changed: `rustfmt --edition 2021 <file>`; never `cargo fmt` a package. Never bare `git stash`/`git stash pop`. Push over HTTPS with an explicit refspec; the controller opens the PR.
- Public repo: invented test data only.
- Commit messages end with: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`
- Builds in a fresh worktree are slow and the disk can be tight: use `CARGO_INCREMENTAL=0`, run focused tests while iterating, the full crate suite once before committing.

## File Structure

- `crates/rupu-transcript/src/lib.rs` (+ a small new fn, e.g. in `reader.rs`) — `last_assistant_text(&[Event]) -> String` (last non-blank `AssistantMessage`), the one shared "final answer" derivation.
- `crates/rupu-orchestrator/src/runs.rs` — `AttemptRecord` struct, `attempts_log` path, `append_attempt` (own static mutex), `read_attempts`; `create` touches `attempts.jsonl`.
- `crates/rupu-orchestrator/src/recovery.rs` (new) — `AttemptPlan`, `StepPlan`, `RecoveryPlans`, `discover(...)`, the `events.jsonl` fallback, loop/kind gating.
- `crates/rupu-orchestrator/src/runner.rs` — emit `AttemptRecord` at each attempt start (linear, `for_each`, retry); `ResumeState.recovery` field; consume plans in `run_fanout_step` and `run_linear_step`; widen `dispatch_one`'s `resume_seed` to carry an optional `seed_source`; emit `AttemptResumed`.
- `crates/rupu-orchestrator/src/executor/event.rs` — `AttemptResumed` variant + `run_id()` arm + round-trip test.
- `crates/rupu-orchestrator/src/lib.rs` — `pub mod recovery;`, re-exports.
- `crates/rupu-cli/src/cmd/workflow.rs` — `--restart-interrupted` on `Action::Resume`; call `recovery::discover` in `resume_run`; print the per-step summary; the resume-path orphan reap.
- `crates/rupu-cli/src/cmd/run.rs` — `--restart-interrupted` on the `RunAction::Resume` parser (symmetry).
- `crates/rupu-cli/src/output/run_model.rs` — `AttemptResumed` arm in `RunView::apply`; a `continued`/`recovered` mark on `UnitView`.
- `crates/rupu-cli/src/output/live_view/structure.rs` — render the mark.
- Tests: `crates/rupu-orchestrator/tests/it/{recovery.rs,attempts_ledger.rs}`, additions to `tests/it/pause_resume_e2e.rs`; `crates/rupu-cli/tests/serial/` resume tests.
- Docs: `docs/workflow-format.md` (or `docs/development-flows.md`), `CLAUDE.md`.

---

### Task 1: Shared `last_assistant_text` in `rupu-transcript`

Consolidates the three "final answer" derivations onto one (PR 1 final-review F6). Semantics: the last `AssistantMessage` event whose `content` is non-blank after `trim`, else `""`.

**Files:**
- Modify: `crates/rupu-transcript/src/reader.rs` (add the fn) and `crates/rupu-transcript/src/lib.rs` (re-export)
- Modify: `crates/rupu-agent/src/continuation.rs` (`final_assistant_text` → delegate), `crates/rupu-orchestrator/src/runner.rs:8996` (`read_final_assistant_text` → delegate), `crates/rupu-cli/src/cmd/dispatch.rs:623` (delegate)

**Interfaces:**
- Produces: `pub fn rupu_transcript::last_assistant_text(events: &[Event]) -> String`

- [ ] **Step 1: Failing test** — in `crates/rupu-transcript/src/reader.rs` tests (or a new `#[cfg(test)] mod`):

```rust
#[test]
fn last_assistant_text_takes_the_last_non_blank_message() {
    use crate::Event;
    let ev = vec![
        Event::AssistantMessage { content: "first".into(), thinking: None },
        Event::AssistantMessage { content: "final answer".into(), thinking: None },
        Event::AssistantMessage { content: "  \n".into(), thinking: None },
    ];
    assert_eq!(crate::last_assistant_text(&ev), "final answer");
    assert_eq!(crate::last_assistant_text(&[]), "");
}
```

- [ ] **Step 2: Run, expect fail** — `cargo test -p rupu-transcript last_assistant_text` → fails (fn missing).
- [ ] **Step 3: Implement** in `reader.rs`:

```rust
/// The run's answer: the last `AssistantMessage` event whose text is
/// non-blank after trimming, or `""`. The one derivation shared by the
/// orchestrator, the continuation primitive and the CLI dispatch path.
pub fn last_assistant_text(events: &[Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::AssistantMessage { content, .. } if !content.trim().is_empty() => {
                Some(content.clone())
            }
            _ => None,
        })
        .unwrap_or_default()
}
```

Re-export from `lib.rs` (`pub use reader::{… , last_assistant_text};`).

- [ ] **Step 4: Delegate the three callers.**
  - `continuation.rs` `final_assistant_text(events)` → `rupu_transcript::last_assistant_text(events)` (keep the private wrapper or inline; keep tests green).
  - `runner.rs:8996` `read_final_assistant_text`: keep its signature (it also warns on a missing transcript when `success`), but compute the text via `rupu_transcript::last_assistant_text` over the read events instead of the "last even if blank" loop. (Behaviour change: a blank final block is now skipped — intended.)
  - `dispatch.rs:623` `read_final_assistant_text(&Path) -> Option<String>`: compute via the shared fn; return `None` only when there is no non-blank text (preserve its `Option` contract — `""` → `None`).

- [ ] **Step 5: Run** `cargo test -p rupu-transcript -p rupu-agent -p rupu-cli --lib` focused on the touched tests; then the three crates' suites once. Expected: green (watch for an orchestrator test that asserted a blank-final output).
- [ ] **Step 6: Format + commit** (`feat(transcript): one shared last_assistant_text derivation`).

---

### Task 2: Attempts ledger — `AttemptRecord` + `RunStore` I/O

**Files:**
- Modify: `crates/rupu-orchestrator/src/runs.rs`

**Interfaces:**
- Produces:
  - `pub struct AttemptRecord { pub v: u32, pub step_id: String, pub unit_index: Option<usize>, pub sub_id: Option<String>, pub agent_run_id: String, pub transcript_path: PathBuf, pub host: Option<String>, pub continued_from: Option<String>, pub started_at: DateTime<Utc> }` (derive `Debug, Clone, Serialize, Deserialize`; `#[serde(default, skip_serializing_if = "Option::is_none")]` on every `Option`)
  - `pub fn RunStore::append_attempt(&self, run_id: &str, a: &AttemptRecord) -> Result<(), RunStoreError>` (own function-local `static APPEND: Mutex<()>`, mirror `append_unit_checkpoint`)
  - `pub fn RunStore::read_attempts(&self, run_id: &str) -> Result<Vec<AttemptRecord>, RunStoreError>` (missing file → empty; skip malformed lines)
  - `fn attempts_log(&self, run_id) -> PathBuf` (private, `run_dir.join("attempts.jsonl")`)

- [ ] **Step 1: Failing test** — in `runs.rs` tests:

```rust
#[test]
fn attempts_round_trip_in_append_order() {
    let tmp = TempDir::new().unwrap();
    let store = RunStore::new(tmp.path().join("runs"));
    let rec = sample_record("run_att");
    store.create(rec.clone(), "x").unwrap();
    for (i, idx) in [Some(0usize), Some(1), None].into_iter().enumerate() {
        store.append_attempt(&rec.id, &AttemptRecord {
            v: 1, step_id: "s".into(), unit_index: idx, sub_id: None,
            agent_run_id: format!("run_a{i}"), transcript_path: format!("/t/{i}.jsonl").into(),
            host: None, continued_from: None, started_at: Utc::now(),
        }).unwrap();
    }
    let got = store.read_attempts(&rec.id).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got[2].unit_index, None);
    assert!(store.read_attempts("run_missing").unwrap().is_empty());
}
```

- [ ] **Step 2: Run, expect fail.**
- [ ] **Step 3: Implement** the struct, `attempts_log`, `append_attempt` (own static mutex; `OpenOptions::new().create(true).append(true)`), `read_attempts` (mirror `read_unit_checkpoints` at runs.rs:2011). In `create` (runs.rs:1222-1223) also touch `attempts.jsonl` for symmetry (optional; appends create it anyway).
- [ ] **Step 4: Run** `cargo test -p rupu-orchestrator --lib attempts_round_trip` → pass.
- [ ] **Step 5: Format + commit** (`feat(orchestrator): per-run attempts.jsonl ledger`).

---

### Task 3: Write an `AttemptRecord` at every linear / for_each attempt start

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs`

**Interfaces:**
- Consumes: `RunStore::append_attempt`, `AttemptRecord` (Task 2).

- [ ] **Step 1: Failing test** — append to `crates/rupu-orchestrator/tests/it/attempts_ledger.rs` (new module; add `mod attempts_ledger;` to `tests/it/main.rs`). A `for_each` workflow with 2 units and a linear step, run to completion with a real `RunStore`, then assert `store.read_attempts(run_id)` has one line per unit (correct `step_id`, `unit_index`, matching `transcript_path` and `agent_run_id`) and one for the linear step (`unit_index: None`). Use the `pause_resume_e2e.rs` harness patterns (`FastOrHangFactory`/`MockProvider`, `run_id_override`).

```rust
// core assertion
let atts = store.read_attempts(&res.run_id).unwrap();
let fe: Vec<_> = atts.iter().filter(|a| a.step_id == "each").collect();
assert_eq!(fe.len(), 2);
assert_eq!(fe.iter().filter_map(|a| a.unit_index).collect::<std::collections::BTreeSet<_>>(),
           [0usize, 1].into_iter().collect());
assert!(atts.iter().any(|a| a.step_id == "lin" && a.unit_index.is_none()));
for a in &atts { assert!(a.transcript_path.ends_with(format!("{}.jsonl", a.agent_run_id))); }
```

- [ ] **Step 2: Run, expect fail** (no attempts written yet → empty vec).
- [ ] **Step 3: Implement.** Append an `AttemptRecord` from `opts.run_store` (when `Some` and `workflow_run_id` non-empty), at:
  - **for_each**, inside the spawned task right where `UnitStarted` is emitted (runner.rs ~8056). The task already clones `store` (PR #705). Set `unit_index: Some(idx)`, `host: placement_host.clone()`, `continued_from: <the Continue plan's from id, else None>` (Task 6 sets this; for now `None`). `started_at: Utc::now()`.
  - **for_each retry** (runner.rs ~8217): a second `AttemptRecord` with the retry run id/host.
  - **linear**, right where `StepWorking { transcript_path: Some(..) }` is emitted (runner.rs ~7378), via `opts.run_store`. `unit_index: None`, `host` = the placed host or `None`.
  Use the agent run id and transcript path already in scope at each site.
- [ ] **Step 4: Run** the new test → pass; run `cargo test -p rupu-orchestrator` once.
- [ ] **Step 5: Format + commit** (`feat(orchestrator): record each linear/for_each attempt in the ledger`).

---

### Task 4: `recovery` module — plans from the ledger, with an events fallback

**Files:**
- Create: `crates/rupu-orchestrator/src/recovery.rs`
- Modify: `crates/rupu-orchestrator/src/lib.rs` (`pub mod recovery;` + re-exports)

**Interfaces:**
- Consumes: `RunStore::{read_attempts, read_unit_checkpoints, events_path}`, `AttemptRecord`, `UnitCheckpoint`, `rupu_agent::continuation::{prepare_continuation, Continuation}`, `Workflow` (for step kind + loop membership).
- Produces:
  - `pub enum AttemptPlan { Continue { transcript: PathBuf, from_agent_run_id: String }, Recovered { output: String, agent_run_id: String, transcript: PathBuf }, Restart { reason: String } }` (derive `Debug, Clone`)
  - `pub struct StepPlan { pub linear: Option<AttemptPlan>, pub units: BTreeMap<usize, AttemptPlan> }` (derive `Debug, Clone, Default`)
  - `pub struct RecoveryPlans(pub BTreeMap<String, StepPlan>)` (derive `Debug, Clone, Default`) with `pub fn is_empty(&self) -> bool` and a `summary()` returning per-step counts for the CLI.
  - `pub fn discover(store: &RunStore, run_id: &str, wf: &Workflow, done_step_ids: &BTreeSet<String>, settled_units: &BTreeMap<String, BTreeMap<usize, ()>>) -> RecoveryPlans`
    - `settled_units` = the units already covered by successful checkpoints (the caller's `completed_units` keyset), so discovery only plans the rest.

**Logic:**
1. Load attempts: `store.read_attempts`; if empty, derive from `events_path` by scanning JSONL for `unit_started`/`agent_started` (for_each: `step_id`,`index`,`transcript_path`; linear: `step_working` with a `transcript_path`, `unit_index: None`). Keep the **latest** attempt per `(step_id, unit_index)` by file order.
2. For each attempt whose `step_id` is **not** in `done_step_ids`, **not** a loop member (`wf.loops` values), and whose step kind is linear or `for_each` (skip `parallel`/`panel`):
   - for_each unit already in `settled_units` → skip (replayed as done).
   - else classify `prepare_continuation(&attempt.transcript_path)`:
     - `Ok(Finished { output })` → `Recovered`
     - `Ok(Resume { .. })` → `Continue { transcript, from_agent_run_id: attempt.agent_run_id }`
     - `Ok(Failed { .. })` → `Restart { reason: "previous attempt failed" }`
     - `Err(e)` → `Restart { reason: e.to_string() }`
   - place into `StepPlan.linear` (unit_index None) or `.units[idx]`.

- [ ] **Step 1: Failing tests** — `crates/rupu-orchestrator/tests/it/recovery.rs` (new; add to `main.rs`). Build real transcripts with `run_agent`+`MockProvider`, write an `attempts.jsonl` and `events.jsonl` by hand, and assert plans. Cases:
  - a for_each unit whose transcript ends mid-turn (no `RunComplete`) → `Continue`;
  - a unit whose transcript is a finished run → `Recovered` with the right output;
  - a unit whose transcript ended `error` → `Restart`;
  - a unit already in `settled_units` → no plan;
  - a linear step attempt → `StepPlan.linear` populated;
  - **fallback parity:** with `attempts.jsonl` absent, deriving from `events.jsonl` yields the same plans;
  - a loop-member step and a `parallel` step → no plan.

- [ ] **Step 2: Run, expect fail** (module missing).
- [ ] **Step 3: Implement** per the logic above; add `pub mod recovery;` and re-exports to `lib.rs`.
- [ ] **Step 4: Run** `cargo test -p rupu-orchestrator --test it recovery::` → pass.
- [ ] **Step 5: Format + commit** (`feat(orchestrator): recovery discovery with an events.jsonl fallback`).

---

### Task 5: `AttemptResumed` executor event

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/event.rs`, `crates/rupu-cli/src/output/run_model.rs`

**Interfaces:**
- Produces: `Event::AttemptResumed { run_id: String, step_id: String, #[serde(default, skip_serializing_if = "Option::is_none")] unit_index: Option<usize>, mode: AttemptResumeMode, #[serde(default, skip_serializing_if = "Option::is_none")] from_agent_run_id: Option<String>, #[serde(default, skip_serializing_if = "Option::is_none")] reason: Option<String> }` and `pub enum AttemptResumeMode { Continued, Recovered, Restarted }` (`#[serde(rename_all = "snake_case")]`).

- [ ] **Step 1: Failing test** — in `event.rs` tests, a serde round-trip asserting the tag `"attempt_resumed"` and `mode` serializing as `"continued"`:

```rust
#[test]
fn attempt_resumed_round_trips() {
    let e = Event::AttemptResumed { run_id: "r".into(), step_id: "s".into(),
        unit_index: Some(3), mode: AttemptResumeMode::Continued,
        from_agent_run_id: Some("run_prev".into()), reason: None };
    let j = serde_json::to_value(&e).unwrap();
    assert_eq!(j["type"], "attempt_resumed");
    assert_eq!(j["mode"], "continued");
    assert_eq!(serde_json::from_value::<Event>(j).unwrap(), e);
}
```

- [ ] **Step 2: Run, expect fail** (variant missing → won't compile; that is the RED).
- [ ] **Step 3: Implement** the variant + enum; add the `Event::run_id()` arm (event.rs:196) returning `run_id`; add the `RunView::apply` arm in `run_model.rs` (the exhaustive match, last arm `PanelRound`) — for now set a new `UnitView`/step mark (Task 8 renders it); make it compile with a minimal body.
- [ ] **Step 4: Run** `cargo test -p rupu-orchestrator --lib attempt_resumed_round_trips` and `cargo build -p rupu-cli`.
- [ ] **Step 5: Format + commit** (`feat(orchestrator): AttemptResumed executor event`).

---

### Task 6: Consume plans in the fan-out runner (Continue / Recovered / Restart)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`ResumeState` field; `dispatch_one` seed param; `run_fanout_step`)

**Interfaces:**
- Consumes: `recovery::{RecoveryPlans, AttemptPlan}`, `rupu_agent::continuation::apply_continuation`, `AttemptResumeMode`.
- Produces: `ResumeState.recovery: RecoveryPlans` (new field; `Default`). Widen `dispatch_one`'s `resume_seed: Option<(Vec<Message>, String)>` → `resume_seed: Option<ContinuationSeed>` where `pub struct ContinuationSeed { messages: Vec<Message>, user_message: String, seed_source: Option<PathBuf> }`, and at the override site (runner.rs:8943) set `initial_messages`, `user_message`, and `seed_source` when present. The linear pause path builds `ContinuationSeed { messages, user_message: String::new(), seed_source: None }` (unchanged behaviour).

**Fan-out consumption** (in `run_fanout_step`, per unit idx, in the spawned task or the pre-dispatch `prepared` loop):
- `RecoveryPlans` for this step → look up `units[idx]`:
  - `Continue { transcript, from }` → `apply_continuation(&mut agent_opts, prepare_continuation(transcript)?.messages, transcript)`; set the attempt's `continued_from = Some(from)`; emit `AttemptResumed { mode: Continued, from_agent_run_id: Some(from) }` before dispatch. Dispatch normally (counts as an attempt, checkpointed on finish).
  - `Recovered { output, agent_run_id, transcript }` → **no dispatch**: synthesize an `ItemResult { index: idx, item, sub_id: "", rendered_prompt: "", run_id: agent_run_id, transcript_path: transcript, output, success: true, is_fixer: false, codename }`, append its unit checkpoint (`append_unit_checkpoint`), emit `UnitCompleted { success: true, .. }` and `AttemptResumed { mode: Recovered }`, and fold it into the step result (like a replayed unit).
  - `Restart { reason }` → dispatch fresh; emit `AttemptResumed { mode: Restarted, reason: Some(reason) }`.
  - none → today's behaviour.

- [ ] **Step 1: Failing e2e test** — add to `tests/it/pause_resume_e2e.rs`: a `for_each` run with 3 units where unit 0 finishes, unit 1 is killed mid-turn (its transcript ends mid-turn), unit 2 never starts; abort the runner (as `fanout_unit_checkpoint_is_durable_while_siblings_still_run` does). Then build `ResumeState` the way `resume_run` does **plus** `recovery = discover(...)`, resume, and assert: unit 0 is **not** re-dispatched (replayed), unit 1's resumed provider request contains the rebuilt conversation + the continuation note (capture via a `CapturingMockProvider` keyed by prompt), unit 2 runs fresh, and the run completes. Also assert an `AttemptResumed { mode: Continued }` for unit 1 via an `EventRecorder`.
- [ ] **Step 2: Run, expect fail.**
- [ ] **Step 3: Implement** the `ResumeState.recovery` field, the `ContinuationSeed` widening of `dispatch_one` (update all six callers), and the fan-out consumption.
- [ ] **Step 4: Run** the new test and `cargo test -p rupu-orchestrator`.
- [ ] **Step 5: Format + commit** (`feat(orchestrator): continue/recover interrupted for_each units on resume`).

---

### Task 7: Consume plans in the linear runner; thread discovery through `resume_run`; `--restart-interrupted`; reaper

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs` (`run_linear_step`), `crates/rupu-cli/src/cmd/workflow.rs`, `crates/rupu-cli/src/cmd/run.rs`

**Interfaces:**
- Consumes: `ResumeState.recovery`, `recovery::discover`, `RunStore::reap_if_orphaned`.

**Linear consumption** (`run_linear_step`): before minting a fresh attempt, check `resume_from.recovery.0[step_id].linear`:
- `Continue { transcript, from }` → build the `ContinuationSeed` from `prepare_continuation(transcript)` and pass it to `dispatch_one`; add the step id to the `resume_paused_step_ids` set (so its gate is suppressed and `StepResumed` fires) — or emit `AttemptResumed { Continued }` directly; set `continued_from`.
- `Recovered { output, agent_run_id, transcript }` → return `LinearStepOutcome::Completed(StepResult { .. })` with that output, no dispatch; emit `AttemptResumed { Recovered }`.
- `Restart` / none → today.

**`resume_run` (workflow.rs:3268):**
- Add `restart_interrupted: bool` param (thread from a new `--restart-interrupted` clap flag on `Action::Resume` and the `rupu run resume` parser).
- **Reaper:** before the status guard (3306), if the record is `Running`/`Pending` with a dead `runner_pid`, call `store.reap_if_orphaned(&mut record, now)` and reload — so a crashed run becomes `Failed` and is resumable without `cp serve`. (Keep the live-pid refusal for a still-alive runner.)
- After building `completed_units` (3407) and before the `ResumeState` literal (3618): when `!restart_interrupted`, compute `settled_units` from `completed_units`, call `recovery::discover(&store, run_id, &workflow, &done_step_ids, &settled_units)`, and set `resume.recovery`. When `--restart-interrupted`, leave it `Default` (empty).
- **Summary:** print one line per step with a plan before dispatch (near workflow.rs:3666), e.g. `each: 1 continued · 1 recovered · 1 restarted`.

- [ ] **Step 1: Failing tests.**
  - orchestrator e2e: a two-step linear workflow where step 2's agent is killed mid-turn; resume with a `recovery` plan continues step 2 from its transcript (provider sees the note) rather than re-running its prompt; `--restart-interrupted` equivalent (empty recovery) re-runs it fresh.
  - CLI serial test (`tests/serial/`): a crashed run (status `Running`, dead `runner_pid`, from `sample_run_record`) is reaped and resumed by `resume_run(id, None, false)` without `cp serve`; and `resume_run(id, None, /*restart=*/true)` skips discovery (assert no `attempt_resumed` continued events, e.g. via the run's `events.jsonl`).
- [ ] **Step 2: Run, expect fail.**
- [ ] **Step 3: Implement** the linear consumption, the `resume_run` signature + flag + reaper + discovery + summary, and the `rupu run resume` parser flag.
- [ ] **Step 4: Run** `cargo test -p rupu-orchestrator -p rupu-cli`.
- [ ] **Step 5: Format + commit** (`feat(cli,orchestrator): resume continues interrupted linear steps; --restart-interrupted; reap crashed runs`).

---

### Task 8: CLI live-view mark + docs + full verification

**Files:**
- Modify: `crates/rupu-cli/src/output/run_model.rs` (`UnitView` mark from Task 5's arm), `crates/rupu-cli/src/output/live_view/structure.rs` (render it), `docs/workflow-format.md`, `CLAUDE.md`

- [ ] **Step 1: Failing test** — a `run_model.rs` unit test: feeding `AttemptResumed { mode: Continued, unit_index: Some(0) }` then that unit's `UnitStarted`/`UnitCompleted` sets a `continued`/`recovered` flag on the `UnitView` (assert the flag).
- [ ] **Step 2: Run, expect fail.**
- [ ] **Step 3: Implement** the `UnitView` field + the `apply` arm (fill in Task 5's stub) + a small glyph/label in `structure.rs`'s `unit_row`/`state_of_unit`.
- [ ] **Step 4: Docs.** `docs/workflow-format.md` (or development-flows): resume now continues interrupted linear steps and `for_each` units from their transcripts by default; `--restart-interrupted` restores restart-from-scratch; runs from before this feature are covered via the event log; `parallel`/`panel`/loop members and remote units still restart (noted as follow-ups). `CLAUDE.md`: a `recovery` note on the `rupu-orchestrator` entry and the Read-first plan bullet.
- [ ] **Step 5: Full verification** (rebased on latest `main` first):
  - `cargo test -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-cli`
  - `cargo check -p rupu-cp`
  - `cargo clippy -p rupu-transcript -p rupu-agent -p rupu-orchestrator -p rupu-cli --all-targets -- -D warnings -A clippy::question_mark`
- [ ] **Step 6: Commit** (`docs: resume continues interrupted work; live-view mark`).

The controller pushes (HTTPS, explicit refspec) and opens the PR after the final whole-branch review.
