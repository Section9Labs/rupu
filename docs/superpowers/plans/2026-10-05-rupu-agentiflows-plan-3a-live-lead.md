# Agentiflows Plan 3a — "It's Alive": real lead + persistence — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the agentiflow envelope actually run: a real `run_agent`-backed `LeadDriver` whose context persists across rounds, operator steering delivered into each round, run persistence (a new `RunTriggerSource::Agentiflow`, the `<global>/agentiflows/<id>/` run dir with `agentiflow.json`, the lead transcript, and `events.jsonl`), and the four Plan-2 pre-req cleanups. End state: `run_agentiflow(def, …)` boots a lead, runs rounds, persists, and stops on the envelope's existing stop disjunction — the lead has introspection only (no spawn/verify tools yet; those are 3b/3c).

**Architecture:** Plan 2's `rupu-agentiflow` crate gains its first dependency on `rupu-agent` (acyclic — agent depends on neither fleet nor agentiflow). The real driver mirrors the proven persistent-session pattern in `rupu-cli`'s `session.rs run_turn` (feed `final_messages` back as `initial_messages`, carry `turn_index_offset`), bridging the sync `LeadDriver::run_round` to async `run_agent_full` via a driver-owned Tokio runtime. A provider **factory** is injected by the caller (tests pass a `MockProvider` factory; the Plan-4 CLI will pass `provider_factory`), so `rupu-agentiflow` stays off the provider/runtime build path. Steering reaches the lead by rendering the round `Digest` into the round's `user_message` (the fleet `MailboxCollector` for mid-round delivery is 3b).

**Tech Stack:** Rust 2021, `rupu-agent` (run_agent + AgentRunOpts + MockProvider), `rupu-coverage` (evidence + ActiveSet, already a dep), `rupu-runtime` (RunTriggerSource), `rupu-orchestrator` executor `JsonlSink`/`Event` for `events.jsonl`, `serde`/`serde_json`/`chrono`/`tracing`, `tokio` (re-added — the driver owns a runtime).

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — §10/§11 (lead-as-session, round lifecycle), §17 (operator steering), §19 (run model / run dir). This plan also discharges the Plan-2-review pre-reqs recorded in session memory (`project_agentiflows_design` → "Plan 3 PRE-REQS") items 1–3. Read the spec alongside.

## Global Constraints

- **Hexagonal / ports.** The lead is still reached only via `LeadDriver`; the provider is injected via a factory closure, so `rupu-agentiflow` does NOT depend on `rupu-runtime::provider_factory` or any provider crate. It MAY depend on `rupu-agent` (for `run_agent_full`, `AgentRunOpts`, `Message`, `MockProvider` in tests) and `rupu-runtime` (only for the `RunTriggerSource` enum).
- **Workspace deps only;** `[lints] workspace = true`; no `unsafe`; `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings` clean; `edition`/`rust-version`/`version` via `.workspace = true`; `thiserror`.
- **Don't break Plan 2.** Every existing `rupu-agentiflow` test stays green; the `Envelope`/`LeadDriver`/evaluator public API changes only as the cleanups below require, and those changes are additive or explicitly noted.
- **No new restrictive gates (operator's directive).** Tool access stays the existing per-agent `tools:` opt-in; this plan adds NO capability gate and NO `capabilities` frontmatter field. The lead's granted tools come from the lead agent's own `.md`.
- **Fail-closed stays fail-closed** (budget, validate) — the cleanups only tighten.

## Scope boundary

3a does NOT include: the lead's orchestration/coordination tools, the fleet-backed collectors, process-isolated unit dispatch, `generate_workflow`, or the verify path (all 3b/3c); nor the `rupu agentiflow` CLI or CP surface (Plan 4). 3a is exercised through a library entry point `run_agentiflow(…)` and integration tests using `MockProvider`. The lead in 3a can think and persist but cannot spawn work — that is intended; it proves the envelope + driver + persistence end-to-end.

## File Structure

In `crates/rupu-agentiflow/`:
- `Cargo.toml` — add `rupu-agent`, `rupu-runtime`, `tokio` (workspace) deps.
- `src/budget.rs` — modify: NaN-spend → `Hard`.
- `src/envelope.rs` — modify: `Digest.warnings` field; `Envelope::new` takes one `started` and builds the `BudgetEnforcer` from it (collapse the dual instant).
- `src/lead.rs` — **new**: `RunAgentLeadDriver`, the provider-factory type, `render_round_prompt`.
- `src/run.rs` — **new**: `AgentiflowRecord` (agentiflow.json), the run-dir layout, `run_agentiflow(…)` entry fn, `events.jsonl` emission.
- `src/lib.rs` — modify: `mod lead; mod run;` + re-exports.

In `crates/rupu-runtime/src/run_envelope.rs` — add the `Agentiflow` variant to `RunTriggerSource`.

---

## Task 1: Plan-2 pre-req cleanups (NaN-spend, dual-`started`, `Digest.warnings`)

**Files:** Modify `crates/rupu-agentiflow/src/budget.rs`, `src/envelope.rs`.

**Interfaces:**
- Produces: `BudgetEnforcer::stage` treats a non-finite `spent_usd()` as `Hard`; `Envelope::new(paths, active, cfg, budget: Budget, operator, started)` (takes `Budget` + `started`, builds the enforcer internally — one instant); `Digest { …, warnings: Vec<String> }`.

- [ ] **Step 1: NaN-spend fails closed — failing test**

In `budget.rs` tests:
```rust
#[test]
fn nan_spend_is_hard_not_ok() {
    struct NanUsage;
    impl UsageSource for NanUsage {
        fn spent_usd(&self) -> f64 { f64::NAN }
        fn spent_tokens(&self) -> u64 { 0 }
    }
    let e = BudgetEnforcer::new(budget(), chrono::Utc::now());
    // a NaN spend must trip Hard on the usd dimension, never Ok (NaN >= 1.0 is false).
    assert!(matches!(e.stage(&NanUsage, 0, chrono::Utc::now()), BudgetStage::Hard { .. }));
}
```
Run: `cargo test -p rupu-agentiflow budget::tests::nan_spend_is_hard_not_ok` → FAIL (currently returns `Ok`).

- [ ] **Step 2: Implement** — in `stage`, when computing the usd fraction, treat a non-finite spent value as exhausted: `let usd_frac = if usage.spent_usd().is_finite() { usage.spent_usd() / cap } else { f64::INFINITY };` (apply the same guard wherever a spend feeds a fraction). Run the test → PASS.

- [ ] **Step 3: Collapse the dual `started` — change `Envelope::new`**

`Envelope::new` currently takes both a `started: DateTime<Utc>` and a pre-built `BudgetEnforcer` (which carries its own `started`), so a caller can pass mismatched instants (the wall-clock *budget* dimension and the wall-clock *ceiling* would measure from different zeros). Change the signature to take the `Budget` (not the enforcer) plus one `started`, and build the enforcer inside:
```rust
pub fn new(paths: CoveragePaths, active: ActiveSet, cfg: EnvelopeConfig, budget: Budget, operator: OperatorQueue, started: DateTime<Utc>) -> Self {
    let enforcer = BudgetEnforcer::new(budget, started);
    Self { paths, active, cfg, budget: enforcer, operator, started }
}
```
Update the existing envelope tests' `Envelope::new(...)` call sites to pass `Budget` + one `started` (they currently construct a `BudgetEnforcer` separately — replace with the budget value + the same instant). Run the envelope tests → PASS.

- [ ] **Step 4: Add `Digest.warnings` — test + implement**

Add `pub warnings: Vec<String>` to `Digest`. In the loop's `assess`, collect coverage/goal evaluation-error strings into `warnings` and set them on the `Digest` (today they only reach the final summary). Add/extend a test: a goal whose evaluation errors (seed a bogus state) yields a `Digest` with a non-empty `warnings` on the round the lead sees. Keep the summary behavior. Update the `MockLead` digest-capturing test if it pattern-matches `Digest` fields exhaustively.

- [ ] **Step 5: clippy + commit**
```bash
cargo test -p rupu-agentiflow && cargo clippy -p rupu-agentiflow --all-targets -- -D warnings
git add crates/rupu-agentiflow/src/budget.rs crates/rupu-agentiflow/src/envelope.rs
git commit -m "fix(agentiflow): NaN-spend fails closed, single started instant, Digest.warnings"
```

## Task 2: `RunTriggerSource::Agentiflow`

**Files:** Modify `crates/rupu-runtime/src/run_envelope.rs`.

**Interfaces:** Produces the `RunTriggerSource::Agentiflow` variant.

- [ ] **Step 1:** Add `Agentiflow` to `pub enum RunTriggerSource` (`run_envelope.rs:30`), after `Autoflow`. It derives the same serde/Debug/Clone as its siblings (match the existing derives + any `#[serde(rename_all=…)]`). If any `match` on `RunTriggerSource` is non-exhaustive elsewhere, add the arm (grep `RunTriggerSource::` across `crates`). Run `cargo build -p rupu-runtime` and `cargo build --workspace` to catch non-exhaustive matches.

- [ ] **Step 2:** Commit.
```bash
git add crates/rupu-runtime/src/run_envelope.rs  # + any match-arm sites
git commit -m "feat(runtime): RunTriggerSource::Agentiflow variant"
```

## Task 3: `render_round_prompt` — turn a `Digest` into the lead's round message

**Files:** Create `crates/rupu-agentiflow/src/lead.rs` (this part); modify `src/lib.rs`.

**Interfaces:**
- Consumes: `crate::envelope::{Digest, RoundContext}`, `crate::goal::GoalOutcome`, `crate::def::Goal`.
- Produces: `pub fn render_round_prompt(ctx: &RoundContext, goals: &[Goal], objective: &str) -> String` (pure) — the per-round user message.

- [ ] **Step 1: Write the failing test**

The prompt must: on round 0 include the mission objective; every round include each goal's id + met/unmet + `current/target` (from `GoalOutcome`), the budget stage, the converge flag when set, any `warnings`, and each steering message body. It is attributed, plain text.
```rust
#[test]
fn round_prompt_includes_objective_goals_budget_and_steering() {
    let ctx = /* RoundContext { round: 0, digest: Digest { goals: [GoalOutcome{id:"rce",met:false,current:3,target:10,detail:"3/10 verified"}], coverage:None, budget: BudgetStage::Soft, converge:true, steering: [OperatorMessage{ts:"t",body:"focus auth",stop:false}], warnings: vec![] } } */;
    let goals = /* the def Goals */;
    let p = render_round_prompt(&ctx, &goals, "Find 10 verified RCE issues.");
    assert!(p.contains("Find 10 verified RCE issues.")); // objective on round 0
    assert!(p.contains("rce") && p.contains("3/10"));     // goal progress
    assert!(p.contains("converge"));                      // soft-budget converge hint
    assert!(p.contains("focus auth"));                    // steering delivered
}
```

- [ ] **Step 2: Run → fail; Step 3: implement** a straightforward formatter (round 0 prepends the objective + "you are the lead orchestrator" framing; later rounds a "progress since last round" header). Keep steering clearly attributed as operator instructions (this is the operator's own channel, so unlike injected data it IS authoritative guidance — but still render it as "Operator steering:" lines, not raw). Run → PASS.

- [ ] **Step 4: Commit** (fold into Task 4's commit if preferred, or standalone).
```bash
git add crates/rupu-agentiflow/src/lead.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): render_round_prompt — Digest -> lead round message"
```

## Task 4: `RunAgentLeadDriver` — the real `run_agent`-backed lead

**Files:** Modify `crates/rupu-agentiflow/src/lead.rs`, `src/lib.rs`, `Cargo.toml`.

**Interfaces:**
- Consumes: `rupu_agent::{run_agent_full, AgentRunOpts, RunExit, MockProvider (tests), runner::… }`, `rupu_providers::types::Message`, `render_round_prompt`, `LeadDriver`/`RoundContext`/`RoundOutcome`.
- Produces:
  - `pub type ProviderFactory = Box<dyn FnMut() -> Box<dyn rupu_providers::LlmProvider> + Send>;` (a fresh provider per round — `AgentRunOpts.provider` is `Box<dyn …>`, not `Clone`).
  - `pub struct LeadConfig { pub agent_name: String, pub system_prompt: String, pub provider_name: String, pub model: String, pub per_round_max_turns: u32, pub run_id: String, pub transcript_path: PathBuf, pub objective: String, pub goals: Vec<Goal> }`.
  - `pub struct RunAgentLeadDriver { cfg: LeadConfig, make_provider: ProviderFactory, rt: tokio::runtime::Runtime, history: Vec<Message>, total_turns: u32 }` + `RunAgentLeadDriver::new(cfg, make_provider) -> std::io::Result<Self>` (builds a `tokio::runtime::Builder::new_current_thread().enable_all().build()`).
  - `impl LeadDriver for RunAgentLeadDriver`.

- [ ] **Step 1: Cargo.toml** — add `rupu-agent = { workspace = true }`, `rupu-runtime = { workspace = true }`, `tokio = { workspace = true }`, and (if not present) `rupu-providers = { workspace = true }` for the `LlmProvider`/`Message` types. Confirm each is a `[workspace.dependencies]` entry (they are — used across the workspace).

- [ ] **Step 2: Write the failing test (MockProvider-backed round)**
```rust
#[test]
fn run_round_runs_the_agent_and_persists_history() {
    use rupu_agent::runner::{MockProvider, ScriptedTurn};
    let cfg = /* LeadConfig { per_round_max_turns: 4, run_id, transcript_path: tempdir file, objective, goals: vec![], .. } */;
    let make = Box::new(|| -> Box<dyn rupu_providers::LlmProvider> {
        Box::new(MockProvider::new(vec![ScriptedTurn::AssistantText { text: "Understood; planning.".into() }]))
    });
    let mut d = RunAgentLeadDriver::new(cfg, make).unwrap();
    let ctx = /* RoundContext { round: 0, digest: Digest { goals: vec![], coverage: None, budget: BudgetStage::Ok, converge: false, steering: vec![], warnings: vec![] } } */;
    let out = d.run_round(&ctx);
    assert!(matches!(out, RoundOutcome::Yielded));
    assert!(!d.history_is_empty());          // final_messages persisted
    assert!(d.last_turn_count() >= 1);
}
```
(Add tiny test-only accessors `history_is_empty`/`last_turn_count`, or assert via a second round seeing the first round's history as `initial_messages` — e.g. a MockProvider that records the request it received.)

- [ ] **Step 3: Implement `run_round`**
```rust
impl LeadDriver for RunAgentLeadDriver {
    fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome {
        let user_message = render_round_prompt(ctx, &self.cfg.goals, &self.cfg.objective);
        let opts = AgentRunOpts {
            agent_name: self.cfg.agent_name.clone(),
            agent_system_prompt: self.cfg.system_prompt.clone(),
            provider: (self.make_provider)(),
            provider_name: self.cfg.provider_name.clone(),
            model: self.cfg.model.clone(),
            run_id: self.cfg.run_id.clone(),
            transcript_path: self.cfg.transcript_path.clone(),
            max_turns: self.cfg.per_round_max_turns,
            user_message,
            initial_messages: self.history.clone(),
            turn_index_offset: self.total_turns,
            collectors: Vec::new(), // 3b wires the fleet-backed collectors
            // … all remaining AgentRunOpts fields: mirror session.rs run_turn's construction,
            //    with no tools (agent_tools from the lead spec), BypassDecider, depth 0.
            ../* see session.rs run_turn for the full field set */
        };
        let exit = self.rt.block_on(rupu_agent::run_agent_full(opts));
        // RunExit destructures to a RunResult-like; on success:
        self.history = exit.result.final_messages.clone();
        self.total_turns += exit.result.turns;
        match &exit.result.error {
            Some(e) => RoundOutcome::Error(e.clone()),
            None if exit.result.turns >= self.cfg.per_round_max_turns => RoundOutcome::TurnBudgetHit,
            None => RoundOutcome::Yielded,
        }
    }
}
```
(Match the real `RunExit`/`RunResult` shape from `rupu-agent/src/runner.rs:1677,1509` — the implementer reads it and adapts field names. Document: the driver owns a `current_thread` runtime and `block_on`s it; the envelope must therefore be driven from a blocking context in the Plan-4 daemon — note this on the struct.)

- [ ] **Step 4: Run → pass; clippy; commit**
```bash
git add crates/rupu-agentiflow/src/lead.rs crates/rupu-agentiflow/src/lib.rs crates/rupu-agentiflow/Cargo.toml
git commit -m "feat(agentiflow): RunAgentLeadDriver — run_agent-backed lead with persistent history"
```

## Task 5: Run persistence + `run_agentiflow` entry point

**Files:** Create `crates/rupu-agentiflow/src/run.rs`; modify `src/lib.rs`.

**Interfaces:**
- Consumes: `AgentiflowDef`, `Envelope`, `RunAgentLeadDriver`, `CoveragePaths`, `ActiveSet`, `rupu_runtime::RunTriggerSource`, the executor `JsonlSink`/`Event` (for `events.jsonl`) OR a small local append — see Step 3.
- Produces:
  - `pub struct AgentiflowRecord { id, name, engagement_profiles: Vec<String>, trigger: RunTriggerSource, status: String, stop_reason: Option<String>, rounds: u32, goals: Vec<GoalStatus>, started_at, ended_at, codename }` (serde) + its on-disk write (`agentiflow.json`, atomic tmp+rename).
  - `pub struct RunAgentiflowOpts { def: AgentiflowDef, workspace: PathBuf, global: PathBuf, active: ActiveSet, budget_started: DateTime<Utc>, now: Box<dyn Fn() -> DateTime<Utc>>, make_provider: ProviderFactory, lead: LeadConfigInputs }` (the caller supplies the resolved `ActiveSet`, the pooled scope, and the provider factory — tests pass MockProvider; Plan 4 passes the real resolver + provider_factory).
  - `pub fn run_agentiflow(opts: RunAgentiflowOpts) -> Result<EnvelopeOutcome, AgentiflowError>`.

- [ ] **Step 1: Run-dir layout + `AgentiflowRecord`** — `<global>/agentiflows/<id>/` holding `agentiflow.json`, `lead/transcript.jsonl` (the lead's `transcript_path`), `events.jsonl`, and the pooled evidence under the workspace coverage store keyed by the agentiflow's scope. Write `agentiflow.json` atomically. Test: construct a record, write, read back.

- [ ] **Step 2: `run_agentiflow`** — wire it:
  1. `def.validate(&active)?` (fail-closed).
  2. **No-automatic-terminator warning (pre-req item 1, as a warn not a gate — operator's "explicit + optional" preference):** if the def has no `required` goal AND no `coverage` AND no `budget` cap AND no `round.ceiling`, `tracing::warn!(id, "agentiflow has no automatic terminator; it will run until an operator stop")`. Do NOT reject — operator-only is a valid explicit mode.
  3. Build `CoveragePaths` for the pooled scope = the agentiflow id (so this run's evidence pools under one target; §7 of the spec).
  4. Build the `EnvelopeConfig` (goals, coverage, ceiling from `def.round.ceiling`, parsing `wall_clock` via `budget::parse_duration`), the `OperatorQueue` (rooted in the run dir), the `Envelope::new(paths, active, cfg, def.budget.unwrap_or_default(), operator, budget_started)`.
  5. Build the `RunAgentLeadDriver` (`LeadConfig` from the def's `lead` agent + the caller's `make_provider`; `transcript_path` = `<run dir>/lead/transcript.jsonl`; `objective` = a digest of the goals' objectives; `goals` = def goals).
  6. Emit `events.jsonl` round/stop events (reuse `rupu_orchestrator::executor::JsonlSink` + `Event` if a fitting variant exists; otherwise append a tiny local `{ts, kind, …}` JSON line — keep it minimal and documented).
  7. `let outcome = envelope.run(&mut lead, &usage, &now);` — **3a note:** `usage` is a trivial `UsageSource` returning the lead transcript's token totals (or zero for the MockProvider path; the usage-ledger-backed source is a Plan-3 pre-req deferred to when real dispatch exists). Persist the final `AgentiflowRecord` (status/stop_reason/rounds/goal statuses) and return the outcome.

- [ ] **Step 3: Integration test (end-to-end, MockProvider)** — seed a tempdir `global`/`workspace`, write a goal's evidence into the pooled `CoveragePaths` so a required goal is already met (or set `round.ceiling.rounds = 2` for the ceiling path), run `run_agentiflow` with a `MockProvider` factory, and assert: `agentiflow.json` exists with the right `stop_reason` (`GoalsMet` or `Ceiling`), `events.jsonl` has round + stop lines, the lead transcript exists, and the returned `EnvelopeOutcome.stop` matches. A second test asserts an operator `stop` message (enqueued into the run dir's operator queue before the run) yields `OperatorStop`.

- [ ] **Step 4: Full-crate check + commit**
```bash
cargo test -p rupu-agentiflow && cargo clippy -p rupu-agentiflow --all-targets -- -D warnings && cargo build --workspace
git add crates/rupu-agentiflow/src/run.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): run_agentiflow entry + run-dir persistence (agentiflow.json, events.jsonl)"
```

---

## Self-Review

**Spec coverage:** §10/§11 lead-as-session + round loop → the `RunAgentLeadDriver` mirroring `session.rs run_turn` (Tasks 3–4); §17 operator steering → rendered into the round prompt (Task 3; mid-round `MailboxCollector` is 3b); §19 run model (RunTriggerSource, run dir, agentiflow.json, events.jsonl) → Tasks 2, 5. Plan-2 pre-reqs 1–3 → Task 1 (NaN, dual-started, Digest.warnings) + Task 5 Step 2 (no-terminator warn). Pre-req "collapse dual started" → Task 1 Step 3.

**Deliberate deferrals (stated):** the lead has no tools in 3a (no record_finding/dispatch/verify) — it thinks + persists only; the real usage-ledger `UsageSource`, the fleet collectors, process-isolated dispatch, `generate_workflow`, the verify path, and the CLI/CP are 3b/3c/Plan 4. The no-terminator case is a WARN, not a reject (operator's "don't gate unless explicit+optional" — operator-only is a valid mode).

**Placeholder scan:** Task 3/4 test bodies use `/* … */` sketches for the `RoundContext`/`AgentRunOpts` literals because their full field sets must be read from the real structs (`envelope.rs` `Digest`, `runner.rs` `AgentRunOpts`/`RunExit`) — the implementer fills them from the cited line numbers; the step text names exactly which fields to set and where to copy the rest from (`session.rs run_turn`). This is a read-then-fill instruction, not a vague placeholder.

**Type consistency:** `ProviderFactory`/`LeadConfig`/`RunAgentLeadDriver` used consistently across Tasks 4–5; `Envelope::new`'s new signature (Task 1 Step 3) is the one Task 5 Step 2 calls; `Digest.warnings` (Task 1 Step 4) is what `render_round_prompt` (Task 3) reads; `RunTriggerSource::Agentiflow` (Task 2) is what `AgentiflowRecord.trigger` (Task 5) uses.

**Ordering note for the executor:** Task 1 (envelope/budget signature changes) must land before Tasks 3–5 (which call the new `Envelope::new` + `Digest.warnings`). Task 2 before Task 5. Otherwise linear 1→5.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-10-05-rupu-agentiflows-plan-3a-live-lead.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — fresh subagent per task, spec+quality review between, broad review at the end.

**2. Inline Execution** — here with checkpoints.

**Which approach?**
