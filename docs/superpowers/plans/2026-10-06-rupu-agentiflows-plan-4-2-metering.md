# Agentiflows Plan 4-2 — Metering + Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `budget.usd` and `budget.tokens` actually enforce. Today the envelope runs against a stub `UnmeteredUsage` (0/0), so only `wall_clock`/`rounds` can stop a run. 4-2 wires a ledger-backed `UsageSource` that folds the lead's + units' usage, writes the agentiflow's `usage.jsonl`, adds the `budget.status` tool, and surfaces live spend in the record/events and `rupu agentiflow status`.

**Architecture:** Reuse `rupu_orchestrator::usage_ledger` (`UsageLedger`/`LedgerRow`; already a dep). The lead's `run_agent_full` gets an `on_usage` hook writing the agentiflow run dir's `usage.jsonl`; a fleet-attached `rupu run` (a unit) likewise writes its own `usage.jsonl`; a fold sums tokens across the lead's + units' ledgers each round; pricing (`rupu-config`) converts tokens→usd. The real `UsageSource` replaces the stub at `run.rs:706`.

**Tech Stack:** Rust 2021, tokio, serde, thiserror (libs)/anyhow (cli). Crates: `rupu-cli`, `rupu-agentiflow` (+ a new `rupu-config` dep), `rupu-orchestrator` (reused).

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` §16 (budget — `usd` is the primary dimension, "every call is already priced"), §13 (`budget.status` Tier-1 read tool), §19 (`usage.jsonl` "reuse the usage ledger, folded across all units").

## Global Constraints

- **`#![deny(clippy::all)]`; no `unsafe`; thiserror in libs / anyhow only in rupu-cli; workspace-pinned deps.** `rupu-agentiflow` MAY gain a direct `rupu-config` dep (pricing data; acyclic — rupu-config is foundational). No other new crate dep (UsageLedger/LedgerRow come from rupu-orchestrator, already a dep).
- **Metering never fails a run.** The ledger append is best-effort (opens lazily, `write_all`, never errors a run — mirror `UsageLedger`'s contract). A missing/corrupt usage.jsonl folds as 0 for that source, never a hard error.
- **`UsageSource` is queried once per round** (`envelope.rs:302`), so a per-round re-fold from disk is acceptable; keep it O(rows), tolerant of a unit dir that doesn't exist yet.
- **usd-without-pricing is a WARN, not a fail-closed stop.** If `budget.usd` is set but no pricing resolves for the lead/active models, warn at launch that usd can't enforce (tokens/wall_clock/rounds still do) and treat usd spend as 0/unenforced for that run — never Hard-stop a run just because pricing is missing. (`BudgetEnforcer::stage` treats a non-finite/zero cap as Hard; the launch must only pass a usd cap into the enforcer when it is actually priceable.)
- **Fleet-scoped only.** A `rupu run` writes `usage.jsonl` ONLY when fleet-attached (3b-2's `--fleet-run-dir`/`--fleet-participant`); a plain standalone `rupu run` is UNCHANGED (no new artifact). Do not alter non-fleet run behavior.
- **Back-compat records.** New `AgentiflowRecord`/event fields are `#[serde(default)]` so existing records/tests still parse.
- **Integration tests: one binary per crate.** rupu-cli serial e2e holds `ENV_LOCK`, runs `< /dev/null`.

## Design rulings (read before Task 1)

1. **Lead metering = write the agentiflow's own `usage.jsonl`.** Wire `AgentRunOpts.on_usage` (currently `None` at `lead.rs:407`) to `UsageLedger::for_run`-style append into `<agentiflow_run_dir>/usage.jsonl`. The `LeadConfig` gains a `usage_ledger: Option<UsageLedger>` the driver threads into each round's opts.
2. **Unit metering = a fleet-attached `rupu run` writes `<runs>/<unit_id>/usage.jsonl`.** In `cmd/run.rs`, when the run is fleet-attached, set `on_usage` to a `UsageLedger` for the run's own `<runs>/<id>/` dir. Units share `RUPU_HOME=<global>`, so the agentiflow folds `<global>/runs/<unit_id>/usage.jsonl`.
3. **The fold lives in `rupu-agentiflow`** (reuses `LedgerRow`): given the agentiflow run dir + the set of launched unit run dirs, read every `usage.jsonl`, dedup by `LedgerRow.id`, sum tokens per `(provider, model)`. The launched unit ids are surfaced from `FleetSupervisor` (it already tracks them in the launcher's `live` map — expose a `launched_unit_run_dirs()` / `launched_unit_ids()`).
4. **Pricing in `rupu-agentiflow` via an injected `PricingConfig`.** `RunAgentiflowOpts` gains `pricing: rupu_config::PricingConfig` (the resolved pricing from config, supplied by the CLI). The `UsageSource` prices the folded tokens → usd via `rupu_config::pricing::lookup` + `ModelPricing::cost_usd`. Unpriced rows contribute tokens but 0 usd; a run whose `budget.usd` can't be priced warns at launch (ruling above).
5. **`budget.status` is a 4th always-on Tier-1 tool** in `status_tools.rs`, mirroring `goal.status`/`coverage.status`.

## File Structure

- `crates/rupu-cli/src/cmd/run.rs` — **modify.** Fleet-attached runs set `on_usage` to a `UsageLedger` for `<runs>/<id>/usage.jsonl`.
- `crates/rupu-orchestrator/src/usage_ledger.rs` — **read/reuse** (maybe a small `for_dir` ctor if `for_run` is RunStore-coupled).
- `crates/rupu-agentiflow/Cargo.toml` — **modify.** Add `rupu-config` (path dep).
- `crates/rupu-agentiflow/src/usage.rs` — **create.** The token fold + the `LedgerUsageSource` (folds lead+unit ledgers, prices via PricingConfig).
- `crates/rupu-agentiflow/src/supervisor.rs` + `subprocess.rs` — **modify.** Surface launched unit run dirs/ids.
- `crates/rupu-agentiflow/src/lead.rs` — **modify.** `LeadConfig.usage_ledger`; wire `on_usage`.
- `crates/rupu-agentiflow/src/run.rs` — **modify.** Build the lead's `UsageLedger`; build the `LedgerUsageSource`; replace `&UnmeteredUsage` at `:706`; update the usd/tokens launch warning (`:500`) + `has_automatic_terminator` (`:245`); add spend to `AgentiflowRecord` + the `round`/`run_stopped` events (+ module doc + round-trip test).
- `crates/rupu-agentiflow/src/status_tools.rs` — **modify.** `budget.status` tool.
- `crates/rupu-cli/src/cmd/agentiflow.rs` — **modify.** Resolve + pass `pricing`; warn if `budget.usd` unpriceable; `status` shows live spend.

---

### Task 1: A fleet-attached `rupu run` writes `usage.jsonl` (`rupu-cli`)

**Files:** Modify `crates/rupu-cli/src/cmd/run.rs` (the `on_usage: None` site ~:1219 + the fleet-attachment detection); Test: `crates/rupu-cli/tests/serial/` or an existing run test.

**Interfaces:**
- Consumes: `rupu_orchestrator::usage_ledger::UsageLedger` + its `hook()`; the run's `<runs>/<id>/` dir (`RunStore::new(global.join("runs"))`); the fleet-attachment detection 3b-2 added (`fleet_attachment`/the `--fleet-run-dir` flag).
- Produces: when (and only when) the run is fleet-attached, `AgentRunOpts.on_usage = Some(UsageLedger::for_run(&store, &run_id).hook(...))` so `<runs>/<run_id>/usage.jsonl` accrues the turn rows. Non-fleet runs keep `on_usage: None`.

- [ ] **Step 1: failing test** — a fleet-attached `rupu run` (set the hidden `--fleet-run-dir`/`--fleet-participant`, mock provider) writes `<runs>/<id>/usage.jsonl` with ≥1 `LedgerRow` (input/output tokens, model); a NON-fleet run writes none. (Reuse the serial run-test harness; if `for_run` needs a RunStore, construct it as the run path does.)
- [ ] **Step 2** run, confirm fail.
- [ ] **Step 3** implement: detect fleet-attachment; build the ledger + hook; set `on_usage`. Confirm the UsageLedger ctor works for a standalone run's dir (add a `UsageLedger::for_dir(path)` in rupu-orchestrator if `for_run` is RunStore-only — minimal, behavior-preserving).
- [ ] **Step 4** `cargo test -p rupu-cli -- <the test> < /dev/null`; `cargo build -p rupu-orchestrator -p rupu-cli`; clippy.
- [ ] **Step 5** commit `feat(cli): fleet-attached runs write usage.jsonl`.

---

### Task 2: Usage fold + unit-dir surfacing (`rupu-agentiflow`)

**Files:** Create `crates/rupu-agentiflow/src/usage.rs`; modify `supervisor.rs` + `subprocess.rs` (surface launched unit run dirs); `lib.rs` (module); Test: `usage.rs` unit tests.

**Interfaces:**
- Consumes: `rupu_orchestrator::usage_ledger::LedgerRow` (parse each usage.jsonl line); the agentiflow run dir + `<global>`; the launcher's live unit ids.
- Produces:
  - `FleetSupervisor::launched_unit_run_dirs(&self, global: &Path) -> Vec<PathBuf>` (each `<global>/runs/<unit_id>`), backed by the launcher tracking ids it minted (expose from `SubprocessUnitLauncher.live` / wherever ids live).
  - `pub fn fold_tokens(usage_jsonl_paths: &[PathBuf]) -> TokenTotals` — read each file line-by-line, skip non-JSON/non-UTF-8, dedup by `LedgerRow.id`, sum per `(provider, model)` into `TokenTotals { by_model: BTreeMap<(String,String), Tokens>, total: Tokens }` (`Tokens { input, output, cached, cache_write }`).

- [ ] **Step 1: failing test** — seed two usage.jsonl (lead + one unit) with known `LedgerRow`s (incl. a duplicate id across a re-read) → `fold_tokens` returns the correct per-model + total token sums, dedup honored, a missing path folds as 0.
- [ ] **Step 2** run, confirm fail.
- [ ] **Step 3** implement `fold_tokens` + `TokenTotals`/`Tokens`; surface `launched_unit_run_dirs` from the supervisor/launcher (the ids are minted in `SubprocessUnitLauncher`; thread a getter up through `FleetSupervisor`).
- [ ] **Step 4** `cargo test -p rupu-agentiflow --lib -- usage::`; clippy.
- [ ] **Step 5** commit `feat(agentiflow): usage fold + launched-unit-dir surfacing`.

---

### Task 3: `LedgerUsageSource` + pricing, replace the stub, enforce usd/tokens (`rupu-agentiflow`)

**Files:** `Cargo.toml` (add rupu-config); `usage.rs` (the source); `lead.rs` (LeadConfig.usage_ledger + wire on_usage); `run.rs` (build the lead ledger + the source; replace `&UnmeteredUsage`; update warning + terminator + tests); Test: `usage.rs` + `run.rs`.

**Interfaces:**
- Consumes: `fold_tokens` (Task 2); `rupu_config::{PricingConfig, pricing::lookup}` + `ModelPricing::cost_usd`; `UsageSource` trait (`budget.rs:45`); `UsageLedger`.
- Produces:
  - `LeadConfig.usage_ledger: Option<UsageLedger>` (lead.rs); in `run_round`, `AgentRunOpts.on_usage = self.cfg.usage_ledger.as_ref().map(|l| l.hook(..))` (replacing `None` at `lead.rs:407`).
  - `struct LedgerUsageSource { agentiflow_usage: PathBuf, supervisor: Arc<FleetSupervisor>, global: PathBuf, pricing: PricingConfig }` impl `UsageSource`: `spent_tokens` = `fold_tokens(lead + unit dirs).total.billable()`; `spent_usd` = price those rows via `pricing` (0 for unpriced). Built in `run_agentiflow` and passed to `envelope.run(&mut lead, &source, &*now)` (replacing `&UnmeteredUsage` at `run.rs:706`).
  - `RunAgentiflowOpts.pricing: rupu_config::PricingConfig` (supplied by the CLI, Task 5).
  - `run.rs`: `has_automatic_terminator` (`:245`) now counts `budget.usd` (only when priceable) + `budget.tokens` as effective terminators; the `unenforced usd/tokens are warned` block (`:500`) becomes: tokens always enforce; usd enforces iff priceable, else warn. Update the two tests (`:997`, `:1242`).

- [ ] **Step 1: failing tests** — a `LedgerUsageSource` over seeded lead+unit ledgers reports the summed tokens, and usd via a test `PricingConfig` (and 0 usd when unpriced); `BudgetEnforcer::stage` with a tokens cap below the folded total → `Hard{tokens}`; the terminator-predicate test now treats a tokens cap (and a priceable usd cap) as effective.
- [ ] **Step 2** run, confirm fail.
- [ ] **Step 3** implement: the source; the lead `on_usage` wiring; the stub replacement; the warning + terminator updates. Keep `UnmeteredUsage` only for the tests that still want it (or delete if unused).
- [ ] **Step 4** `cargo test -p rupu-agentiflow`; clippy; `cargo build -p rupu-cli` (opts field added).
- [ ] **Step 5** commit `feat(agentiflow): ledger-backed UsageSource — budget.usd/tokens now enforce`.

---

### Task 4: `budget.status` tool (`rupu-agentiflow`)

**Files:** `status_tools.rs`; Test: `status_tools.rs`.

**Interfaces:**
- Consumes: the `Budget` caps, a `UsageSource` handle (or the fold + pricing), `BudgetEnforcer::stage`; the `StatusCtx`/`Tool` pattern.
- Produces: a `budget.status` tool (added to `status_tools(...)` + `StatusCtx`) reporting per-dimension spend vs cap (usd, tokens, rounds elapsed, wall-clock elapsed) + the `BudgetStage` (ok/soft/hard). Off-runtime via `spawn_blocking` if it re-folds from disk. Remove the "budget.status is a later plan" caveat (`status_tools.rs:8-9, 150-151, 210-211`).

- [ ] **Step 1: failing test** — seed ledgers + a Budget; `budget.status` returns the spend/cap per dimension + the right stage (ok/soft/hard) for a seeded over/under-budget case.
- [ ] **Step 2** run, fail. **Step 3** implement. **Step 4** `cargo test -p rupu-agentiflow --lib -- status_tools::`; clippy. **Step 5** commit `feat(agentiflow): budget.status tool`.

---

### Task 5: Record/events spend + `status` live spend + CLI pricing wiring

**Files:** `run.rs` (record + events + module doc + round-trip test); `cmd/agentiflow.rs` (pricing resolve + pass; usd-unpriceable warn; `status` shows spend); Test: `run.rs` + the agentiflow status test.

**Interfaces:**
- Produces:
  - `AgentiflowRecord` gains `spent_usd: Option<f64>` + `spent_tokens: u64` (`#[serde(default)]`); written at the final rewrite (`run.rs:727`) and (for live visibility) refreshed per round in `RecordingLead::run_round` (give it a handle to the usage source or thread spend via the Digest). The `round` + `run_stopped` events carry `spent_usd`/`spent_tokens`; module doc (`run.rs:36-53`) + `sample_record`/round-trip test (`run.rs:903`) updated.
  - `cmd/agentiflow.rs`: resolve `PricingConfig` from layered config (mirror how the CP/pricing is loaded) and pass it as `opts.pricing`; if `def.budget.usd` is set and the lead/gen model has no price, `tracing::warn!` that usd won't enforce. `rupu agentiflow status` prints live `spent_usd`/`spent_tokens` (from the record, falling back to a fold of `usage.jsonl` if the record lacks them).

- [ ] **Step 1: failing tests** — record round-trips with the new spend fields (and an OLD record without them still parses → defaults); `status` shows spend for a seeded record. **Step 2** fail. **Step 3** implement. **Step 4** `cargo test -p rupu-agentiflow`; `cargo test -p rupu-cli -- agentiflow < /dev/null`; clippy. **Step 5** commit `feat(agentiflow): persist + surface budget spend (record/events/status)`.

---

### Task 6: End-to-end — budget enforcement (`rupu-cli`)

**Files:** `crates/rupu-cli/tests/serial/agentiflow_budget.rs` (+ main.rs).

**Interfaces:** the whole path; mock provider scripted to emit token usage; a def with a tiny `budget.tokens` cap.

- [ ] **Step 1** write the e2e: a def with `budget.tokens` low enough that the mock lead's first round's usage exceeds it; run `rupu agentiflow run`; assert the stop reason is `budget_exhausted` on `tokens` (NOT ceiling), `usage.jsonl` exists with rows, and `agentiflow status` / `budget.status`-equivalent reports the spend. (If scripting precise token counts through the mock is hard, assert tokens accrue to `usage.jsonl` and `status` shows non-zero spent_tokens + that a tokens cap below that total yields `budget_exhausted` — adjust the cap to the observed usage.) Hold `ENV_LOCK`, run `< /dev/null`.
- [ ] **Step 2** fail. **Step 3** pass. **Step 4** `cargo test -p rupu-cli --test serial agentiflow_budget < /dev/null`; `cargo test -p rupu-cli < /dev/null`; `cargo clippy -p rupu-cli -p rupu-agentiflow -p rupu-orchestrator --all-targets -- -D warnings -A clippy::question_mark`. **Step 5** commit `test(cli): agentiflow budget.tokens enforcement e2e`.

---

## Self-Review

**Spec coverage:** §16 `usd`/`tokens` enforcement → Tasks 2/3 (fold + source + enforcer wiring); §19 `usage.jsonl` folded across units → Tasks 1/2/3; §13 `budget.status` → Task 4; live spend surfacing → Task 5; proof → Task 6.

**Placeholder scan:** none — interfaces are concrete; Task 1 carries a `for_dir` fallback note; Task 3 is explicit about the warning/terminator updates + their existing tests.

**Type consistency:** `fold_tokens`/`TokenTotals` (Task 2) feed `LedgerUsageSource` (Task 3) and `budget.status` (Task 4); `RunAgentiflowOpts.pricing` (Task 3) is supplied by the CLI (Task 5); the `on_usage`→`usage.jsonl` writer (Tasks 1 lead-side via LeadConfig, unit-side via cmd/run.rs) is what `fold_tokens` reads.

**Verify-before-commit:** each task green on its crate; Task 6 is the cross-crate enforcement proof. Full suite is the release gate on `main`.
