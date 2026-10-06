# Agentiflows Plan 3c — Opt-in Verify Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a goal's `verified: true` / `verify_with: <agent>` actually satisfiable by supplying the missing finding-verification write path — a `finding.verify` agent tool that an *independent* verifier run uses to record a Confirmed verdict — and wiring the goal evaluator + load-time validation to gate on it fail-closed.

**Architecture:** The eval-time `verified` gate and the `Verification { status, by_run, notes }` model already exist; today they are dead because no code ever *writes* a verification (both `report_finding` and the import path refuse an agent-supplied one — a filer must not confirm its own work). 3c adds: (a) `by_agent` on `Verification`; (b) a `finding.verify` write tool that merges a verification block onto an existing-report finding line via the same locked-rewrite discipline `rupu findings import` uses, refusing self-verification; (c) evaluator gating that honors `verify_with` (specific independent agent) and an optional `verify_check` (broaden to require a PoC); (d) fail-closed load validation. The verifier agent itself is operator-composed (a pool agent whose frontmatter lists `finding.verify`); the lead dispatches it as a process-isolated unit (3b-2/3b-4 surface) whose different run id *is* the independence.

**Tech Stack:** Rust 2021, tokio, async-trait, thiserror (libs), serde/serde_json. Crates touched: `rupu-coverage`, `rupu-agent`, `rupu-agentiflow`.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` §14 (goals are predicates over VERIFIED profile-typed evidence; `verified` = report `verification_status`/`has_poc`/per-claim `evidence_status`; `verify_with: <agent>` = independent corroboration before a finding counts).

## Global Constraints

- **Fail-closed.** A goal that demands verification (`verified: true` or `verify_with: X`) with no way to satisfy it must be rejected at load, never silently unmeetable. `verify_with: X` ⇒ `X ∈ pool.agents` (hard error at `AgentiflowDef::validate`, mirroring the existing lead-in-pool check).
- **Independence is load-bearing and enforced at the evaluator, not just the tool.** A verification counts only when `status == Confirmed` AND `by_run` is set AND `by_run != finding.declared_by.run_id`. The `finding.verify` tool additionally refuses self-verification up front (friendly error), but the evaluator never trusts that — it re-checks independence itself.
- **Append-only ledger discipline.** The ONLY writer that rewrites a finding line is the locked-rewrite path in `crates/rupu-coverage/src/tools/attach_report.rs` (sidecar `findings.jsonl.lock`, byte-for-byte backup, atomic rename, length-guard, JSON-merge preserving unknown keys). `finding.verify` MUST reuse that exact discipline (shared helper), not a second bespoke rewrite. Every other findings write stays an append through `ledger::stream::append_record`.
- **Schema lockstep.** `Verification` gains a field → update the embedded draft-07 schema `crates/rupu-coverage/.../schema/finding_report.schema.json` and keep `tests/it/report_schema_lockstep.rs` green.
- **`#![deny(clippy::all)]`; no `unsafe`; `thiserror` in libs, no `anyhow` in a library crate; workspace-pinned deps; no new crate dep.**
- **Integration tests: one binary per crate** — modules under `crates/<c>/tests/it/` listed in `main.rs`; never a new top-level `tests/*.rs`. Env/cwd-mutating `rupu-cli`-style tests are not expected here.
- **No behavior change to `report_finding` / import.** Those must keep refusing an agent-supplied `verification`. `finding.verify` is the only writer of a verification block.

## Design rulings (read before Task 1)

1. **`finding.verify` is an opt-in AGENT tool, not a lead/fleet tool, and it is NEVER auto-granted.** It registers beside `report_finding` but is gated on the agent's `tools:` frontmatter listing `finding.verify` — **always, even for a `concerns:` agent** (it is NOT added to the coverage-harness bundle, because verifying another unit's finding is a distinct, independence-sensitive role from filing one; auto-granting it to every concerns agent would let any hunter rubber-stamp a peer's finding to satisfy a plain `verified: true` goal). So a verifier runs it inside its own `rupu run`. The lead does not get it specially. "Operator-composed verifier independence" = the operator writes the verifier agent with `finding.verify` in its `tools:`; rupu provides the tool + the `verify_with` plumbing, not a shipped verifier agent.
2. **Pooled-scope write.** A dispatched verifier unit runs with the 3b-2 fleet attachment, so its coverage tools (incl. `finding.verify`) use the agentiflow's pooled `CoveragePaths` — the same `findings.jsonl` the evaluator reads and that the filing unit wrote to. The verify tool therefore rewrites the pooled line; the cross-process sidecar lock makes that safe.
3. **`verify_with: X` needs `by_agent`.** `Verification.by_run` (a run id) can't tell the evaluator *which agent* verified. Add `Verification.by_agent: Option<String>` (set from the verifier's `ctx.agent`). Evaluator: `verify_with: X` ⇒ the finding's verification is independent-Confirmed AND `by_agent == X`.
4. **`verify_check` (optional, per-goal) broadens the gate, off by default.** `GoalTarget.verify_check: Option<VerifyCheck>` with `confirmed` (default, = the independent-Confirmed gate) and `with_poc` (also require `!artifacts.is_empty()`, i.e. `has_poc`). This nudges toward spec §14's `has_poc` bar without pulling in CP-side per-claim `evidence_status` (which is a read-time staleness verdict, not persisted — explicitly out of v1 scope).
5. **Asset-target `verified` stays rejected (v1).** `def.rs` already refuses `verified: true` on an asset target ("the depth rung is the evidence"); the spec §14 sample puts `verified` on an asset target (`root-1122`). Reconciliation: v1 keeps `verified`/`verify_with` a **findings-target-only** concept (validator unchanged); asset-target verification (which needs an asset↔backing-finding link) is deferred to Plan 4. The plan documents this; `validate` gives a clear error.

## File Structure

- `crates/rupu-coverage/src/report/types.rs` — **modify.** `Verification` gains `by_agent: Option<String>`.
- `crates/rupu-coverage/.../schema/finding_report.schema.json` — **modify.** Add `by_agent` to the `verification` object (lockstep).
- `crates/rupu-coverage/src/tools/verify_finding.rs` — **create.** The `verify_finding` pure write fn (locked rewrite) + a shared lock/rewrite helper factored from `attach_report.rs`.
- `crates/rupu-coverage/src/tools/attach_report.rs` — **modify.** Extract the lock + `replace_ledger` + segment-rewrite into a shared helper the new path reuses (no behavior change to import).
- `crates/rupu-agent/src/coverage_tools.rs` — **modify.** `FindingVerifyTool` (the tool itself; NOT added to the concerns bundle in `register`).
- `crates/rupu-agent/src/runner.rs` — **modify.** Add `finding.verify` to the standalone opt-in `tools:`-gated registration beside `report_finding`.
- `crates/rupu-agentiflow/src/def.rs` — **modify.** `GoalTarget.verify_check`; `validate` fail-closed rules.
- `crates/rupu-agentiflow/src/goal.rs` — **modify.** `is_verified` independence + `verify_with`/`verify_check` gating; `count_matching_findings` signature.
- `crates/rupu-agentiflow/src/status_tools.rs` — **modify.** `goal.status` surfaces unverified count + the `verify_with` agent.
- Test modules: `crates/rupu-coverage/tests/it/`, `crates/rupu-agent/` unit tests, `crates/rupu-agentiflow/src/goal.rs` + `def.rs` unit tests + `src/run.rs` (or `tests/it/`) e2e.

---

### Task 1: `Verification.by_agent` + schema lockstep (`rupu-coverage`)

**Files:**
- Modify: `crates/rupu-coverage/src/report/types.rs` (`Verification`, ~:435-443)
- Modify: `crates/rupu-coverage/src/report/schema/finding_report.schema.json` (the `verification` object)
- Test: `crates/rupu-coverage/tests/it/report_schema_lockstep.rs` (must stay green) + a round-trip unit test in `types.rs`

**Interfaces:**
- Consumes: `VerificationStatus` (`types.rs:426`).
- Produces: `Verification { status, by_run: Option<String>, by_agent: Option<String>, notes: Option<String> }` — `by_agent` is `#[serde(default, skip_serializing_if = "Option::is_none")]`, matching `by_run`.

- [ ] **Step 1: Write the failing test** — a round-trip asserting `by_agent` serializes/deserializes and is omitted when `None`:

```rust
#[test]
fn verification_round_trips_by_agent() {
    let v = Verification { status: VerificationStatus::Confirmed,
        by_run: Some("run_B".into()), by_agent: Some("exploit-verifier".into()), notes: None };
    let j = serde_json::to_string(&v).unwrap();
    assert!(j.contains("\"by_agent\":\"exploit-verifier\""));
    let back: Verification = serde_json::from_str(&j).unwrap();
    assert_eq!(back.by_agent.as_deref(), Some("exploit-verifier"));
    // omitted when None
    let v2 = Verification { status: VerificationStatus::Confirmed, by_run: None, by_agent: None, notes: None };
    assert!(!serde_json::to_string(&v2).unwrap().contains("by_agent"));
}
```

- [ ] **Step 2: Run it, confirm it fails** — `cargo test -p rupu-coverage --lib verification_round_trips_by_agent` → fails (no such field).
- [ ] **Step 3: Add the field** to `Verification` (after `by_run`), and add `"by_agent": { "type": "string" }` to the `verification` object's `properties` in `finding_report.schema.json` (do NOT add it to `required`).
- [ ] **Step 4: Run tests** — `cargo test -p rupu-coverage --lib report::` AND `cargo test -p rupu-coverage --test it report_schema_lockstep` — both green (the lockstep test proves the struct and schema agree).
- [ ] **Step 5: Commit** — `git commit -m "feat(coverage): add Verification.by_agent (+ schema lockstep)"`

---

### Task 2: `verify_finding` locked-rewrite write fn (`rupu-coverage`)

**Files:**
- Modify: `crates/rupu-coverage/src/tools/attach_report.rs` — extract the shared lock + ledger-rewrite helper (no behavior change).
- Create: `crates/rupu-coverage/src/tools/verify_finding.rs`
- Modify: `crates/rupu-coverage/src/tools/mod.rs` (export `verify_finding`)
- Test: `crates/rupu-coverage/tests/it/` (new `verify_finding.rs` module, listed in `main.rs`)

**Interfaces:**
- Consumes: the `CoveragePaths`, the `findings.jsonl.lock` sidecar lock (`lock_findings`), `replace_ledger` + the segment-rewrite loop (currently private in `attach_report.rs`), `FindingRecord`/`Verification`/`VerificationStatus`, `Attribution`.
- Produces:
  ```rust
  pub struct VerifyInput {
      pub finding_id: String,
      pub status: VerificationStatus,     // Confirmed | Disputed | Inconclusive (Unverified rejected)
      pub by_run: String,                 // the verifier's run id (required)
      pub by_agent: Option<String>,       // the verifier's agent name
      pub notes: Option<String>,
  }
  pub enum VerifyError { NotFound, NoReport, SelfVerification, BadStatus, Io(..), /* thiserror */ }
  /// Merge a `verification` block onto the finding's existing-report JSON line,
  /// under the findings lock, preserving unknown keys. Refuses: unknown id,
  /// a finding with no report, `status == Unverified`, and self-verification
  /// (`by_run == declared_by.run_id`).
  pub fn verify_finding(paths: &CoveragePaths, input: &VerifyInput) -> Result<(), VerifyError>;
  ```

- [ ] **Step 1: Write failing tests** — seed a `findings.jsonl` (append a full-report finding via the existing write path, with `declared_by.run_id = "run_A"`), then:
  - `verify_finding` with `by_run = "run_B"`, `status = Confirmed` → Ok; re-reading the ledger shows that finding's `verification.status == Confirmed`, `by_run == "run_B"`, `by_agent` set; every OTHER line is byte-identical.
  - self-verification: `by_run = "run_A"` → `Err(SelfVerification)`, ledger unchanged.
  - unknown id → `Err(NotFound)`, ledger unchanged.
  - a finding with no report (Summary-profile append) → `Err(NoReport)`.
  - `status = Unverified` → `Err(BadStatus)`.
  - concurrency: a second thread appending a new finding while `verify_finding` holds the lock does not corrupt the ledger (both land; the length-guard/lock hold).

- [ ] **Step 2: Run them, confirm they fail** — module/fn don't exist yet.
- [ ] **Step 3: Implement.**
  1. In `attach_report.rs`, extract (a) the lock acquisition and (b) `replace_ledger` + the read→segment→rewrite-one-line→write-back loop into `pub(crate)` helpers (e.g. `with_findings_rewrite(paths, |lines| -> Result<(), E>)`), and refactor `attach_reports` to call them. Run the existing import tests to prove no behavior change.
  2. `verify_finding`: take the lock; read the ledger; find the line whose parsed `FindingRecord.id == finding_id` (JSON parse per line, skip non-JSON as the readers do); if none → `NotFound`; if that record has no `report` → `NoReport`; parse `declared_by.run_id`; if `== input.by_run` → `SelfVerification`; if `input.status == Unverified` → `BadStatus`; otherwise JSON-merge a `verification` object `{status, by_run, by_agent?, notes?}` into the line's `report` object (operate on the parsed `serde_json::Value` of the line, overwriting any existing `verification`, preserving all other keys — the `attach_report` `upgraded_line` discipline), and write every other line back byte-for-byte via the extracted rewrite helper.

- [ ] **Step 4: Run tests** — `cargo test -p rupu-coverage --test it verify_finding` + `cargo test -p rupu-coverage --test it attach_report` (import unchanged) + `cargo test -p rupu-coverage` green; `cargo clippy -p rupu-coverage --all-targets -- -D warnings`.
- [ ] **Step 5: Commit** — `git commit -m "feat(coverage): verify_finding — locked-rewrite of a finding's verification block"`

---

### Task 3: `finding.verify` agent tool + registration (`rupu-agent`)

**Files:**
- Modify: `crates/rupu-agent/src/coverage_tools.rs` — `FindingVerifyTool` + add to `register` and the standalone opt-in path.
- Modify: `crates/rupu-agent/src/runner.rs` — the `tools:`-gated standalone registration beside `report_finding` (~:1875-1907).
- Test: `crates/rupu-agent/src/coverage_tools.rs` unit tests.

**Interfaces:**
- Consumes: `rupu_coverage::tools::verify_finding::{verify_finding, VerifyInput, VerifyError}`; `ToolContext` (`run_id`, `agent`); `CoveragePaths` (same handle `report_finding` uses); `attribution`/ctx helpers.
- Produces: `FindingVerifyTool` (name `"finding.verify"`) — `input_schema`: required `finding_id` (string), required `status` (enum `confirmed|disputed|inconclusive`), optional `notes`. `invoke` builds `VerifyInput { finding_id, status, by_run: ctx.run_id, by_agent: ctx.agent, notes }` and runs `verify_finding` on `spawn_blocking`; maps `VerifyError::{NotFound,NoReport,SelfVerification,BadStatus}` to `ToolOutput.error` (a refusal the agent sees), `Io` to `ToolError`.

- [ ] **Step 1: Write failing tests** — construct the tool over a seeded `CoveragePaths` + a `ToolContext { run_id: Some("run_B"), agent: Some("exploit-verifier"), .. }`; invoking with a real finding id + `status: "confirmed"` records the verification (re-read shows Confirmed/by_run/by_agent); invoking on a finding filed by `run_B` itself returns `ToolOutput.error` (self-verification) and writes nothing; a missing `finding_id` arg → `ToolError::InvalidInput`.
- [ ] **Step 2: Run them, confirm they fail.**
- [ ] **Step 3: Implement** `FindingVerifyTool` modeled on `ReportFindingTool` (`coverage_tools.rs:258-324`): hold `CoveragePaths`; `spawn_blocking` the write; map errors to `done`/`failed`-equivalent `ToolOutput`. Register it ONLY from `runner.rs`, gated on the agent's `tools:` containing `"finding.verify"` (exact match; `None`/`["*"]` do not grant it), registered after the `filter_to` pass — for BOTH a concerns agent and a no-concerns agent (reuse the concerns bundle's `CoveragePaths` when present, else build the standalone one). It is NOT added to `coverage_tools::register`'s concerns bundle, so it is never auto-granted (ruling 1).
- [ ] **Step 4: Run tests** — `cargo test -p rupu-agent --lib coverage_tools::` + `cargo test -p rupu-agent` green; clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "feat(agent): finding.verify tool (opt-in, beside report_finding)"`

---

### Task 4: Evaluator gating — independence + `verify_with` + `verify_check` (`rupu-agentiflow`)

**Files:**
- Modify: `crates/rupu-agentiflow/src/def.rs` — `GoalTarget.verify_check` + `VerifyCheck` enum.
- Modify: `crates/rupu-agentiflow/src/goal.rs` — `is_verified`, `count_matching_findings`, `evaluate` findings branch.
- Test: `crates/rupu-agentiflow/src/goal.rs` unit tests.

**Interfaces:**
- Consumes: `FindingRecord` (`report.verification` + `declared_by.run_id`), `VerificationStatus::Confirmed`, `read_findings`.
- Produces:
  - `def.rs`: `pub verify_check: Option<VerifyCheck>` on `GoalTarget` (`#[serde(default)]`); `#[serde(rename_all="snake_case")] pub enum VerifyCheck { Confirmed, WithPoc }` (default-equivalent to `Confirmed` when `None`).
  - `goal.rs`: `fn is_verified(rec, verify_with: Option<&str>, check: VerifyCheck) -> bool` requiring: `v.status == Confirmed` AND `v.by_run.is_some()` AND `v.by_run != Some(rec.declared_by.run_id)` AND (`verify_with` ⇒ `v.by_agent.as_deref() == verify_with`) AND (`check == WithPoc` ⇒ `!rec.artifacts.is_empty()`). `count_matching_findings(records, sel, verified, verify_with, check)` threads them; `evaluate` passes `goal.verify_with.as_deref()` and `goal.target.verify_check`.

- [ ] **Step 1: Write failing tests** over seeded `FindingRecord`s (built in-test): a Confirmed verification by a different run counts under `verified: true`; a self-verification (same run) does NOT; `verify_with: "x"` counts only when `by_agent == "x"` (wrong/absent agent doesn't); `verify_check: with_poc` additionally requires a non-empty `artifacts`; `verified: false` ignores verification entirely (today's behavior).
- [ ] **Step 2: Run them, confirm they fail.**
- [ ] **Step 3: Implement** the richer `is_verified` + the threaded signature + the `VerifyCheck` field (remember `deny_unknown_fields` on `GoalTarget`). Keep `verified: false` a pure no-op (independence/verify_with/verify_check only apply when `verified` is true OR `verify_with` is set — define that a `verify_with` set with `verified:false` still gates, since naming a verifier implies requiring verification; document this).
- [ ] **Step 4: Run tests** — `cargo test -p rupu-agentiflow --lib goal::` + crate green; clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "feat(agentiflow): goal verify gating — independence + verify_with + verify_check"`

---

### Task 5: Fail-closed load validation (`rupu-agentiflow`)

**Files:**
- Modify: `crates/rupu-agentiflow/src/def.rs` — `AgentiflowDef::validate` (~:163-284)
- Test: `crates/rupu-agentiflow/src/def.rs` unit tests.

**Interfaces:**
- Consumes: `self.pool.agents`, each goal's `verify_with`/`target.verified`/`target.verify_check`/`target.asset`.
- Produces: new validation errors (reuse the crate's `thiserror` `AgentiflowError`/validation error shape): `verify_with: X` where `X ∉ pool.agents` → hard error; `verified: true`/`verify_with`/`verify_check` on an **asset** target → hard error (keep the existing asset-verified rejection and extend its message to name `verify_with`/`verify_check`).

- [ ] **Step 1: Write failing tests** — a def with a goal `verify_with: "ghost"` not in the pool fails `validate` with a message naming `ghost`; `verify_with` naming a real pool agent passes; `verify_with`/`verify_check` on an asset target fails; a findings goal with `verified: true` and no `verify_with` still passes (the `finding.verify` tool is the mechanism, always available to a verifier the operator composes).
- [ ] **Step 2: Run them, confirm they fail.**
- [ ] **Step 3: Implement** the checks in `validate`, next to the existing lead-in-pool and asset-verified checks.
- [ ] **Step 4: Run tests** — `cargo test -p rupu-agentiflow --lib def::` + crate green; clippy clean.
- [ ] **Step 5: Commit** — `git commit -m "feat(agentiflow): fail-closed validation for verify_with / verified targets"`

---

### Task 6: `goal.status` surfacing + e2e (`rupu-agentiflow`)

**Files:**
- Modify: `crates/rupu-agentiflow/src/status_tools.rs` — `goal.status` detail.
- Test: `crates/rupu-agentiflow/src/run.rs` (or `tests/it/`) e2e + `status_tools.rs` unit test.

**Interfaces:**
- Consumes: the Task-4 evaluator; the 3b dispatch surface (`dispatch`/`join`, `MockUnitLauncher`); `finding.verify` (exercised indirectly through a mock unit that writes a verification, OR directly via `verify_finding` to keep the e2e hermetic).
- Produces: `goal.status` detail that, for a findings goal with `verify_with`/`verified`, reports matched-but-unverified count and the required verifier agent (so the lead knows to dispatch it).

- [ ] **Step 1: Write failing tests.**
  - `status_tools` unit: a seeded pooled ledger with 3 matching findings, 1 independently Confirmed → `goal.status` detail shows the verified/needs-verification split and names `verify_with` when set.
  - e2e (reuse the `run_agentiflow` harness + `MockUnitLauncher`): a finding filed under the pooled scope with `declared_by.run_id = "run_A"` is NOT counted by a `verified: true` goal; after an independent Confirmed verification is recorded (by a different run id — simplest: have the mock verifier unit's recorded outcome drive a `verify_finding` write, or seed it), the goal flips to met; a self-verification (same run id) never counts. Assert `StopReason::GoalsMet` only after independent verification.
- [ ] **Step 2: Run them, confirm they fail.**
- [ ] **Step 3: Implement** the `goal.status` detail; ensure the e2e exercises the whole gate (file → independent verify → met; self-verify → not met).
- [ ] **Step 4: Run tests** — `cargo test -p rupu-agentiflow` green; `cargo clippy -p rupu-coverage -p rupu-agent -p rupu-agentiflow --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit** — `git commit -m "feat(agentiflow): goal.status verify surfacing + verify-path e2e"`

---

## Self-Review

**Spec coverage:** §14 `verified` → Tasks 1-4 make it satisfiable (write path + independence); `verify_with: <agent>` independent corroboration → Tasks 1/3/4 (`by_agent` + the evaluator gate) + Task 5 (fail-closed pool check); `has_poc` facet of "verified" → Task 4 `verify_check: with_poc`. Per-claim `evidence_status` (CP read-time staleness) is explicitly out of v1 scope (ruling 4). Asset-target `verified` deferred (ruling 5), with a clear validator error.

**Placeholder scan:** none — every task has concrete signatures + test bodies. The one factoring step (Task 2 extracting `attach_report`'s lock/rewrite) is guarded by re-running the import tests for no-behavior-change.

**Type consistency:** `Verification.by_agent` (Task 1) is set by `finding.verify` (Task 3) from `ctx.agent`, and read by `is_verified` (Task 4). `VerifyInput { finding_id, status, by_run, by_agent, notes }` (Task 2) is the exact shape Task 3 builds. `GoalTarget.verify_check: Option<VerifyCheck>` (Task 4) is validated in Task 5. `is_verified`'s independence rule (`by_run != declared_by.run_id`) is the same seam Task 2's `SelfVerification` enforces at write time.

**Verify-before-commit:** each task ends green on its crate scope; Task 6 ends on the cross-crate clippy gate. Full workspace suite is the release gate on `main`.
