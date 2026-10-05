# Agentiflows Plan 2 — The Envelope (decision engine + round loop) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the deterministic envelope that drives an agentiflow: parse/validate the `AgentiflowDef`, evaluate the OR-of stop conditions (goals / coverage / budget / operator-stop / hard-ceiling) over the real `rupu-coverage` evidence stores, and run the round loop against a `LeadDriver` port — with the agentic lead, unit dispatch, collectors, and run-persistence left to Plan 3.

**Architecture:** A new leaf-ish crate `rupu-agentiflow` on top of `rupu-coverage`. The envelope owns the loop and the purse-and-kill-switch; it decides, code holds the authority. The lead is reached only through a `LeadDriver` trait (Plan 2 ships the port + a mock; Plan 3 ships the `run_agent`-backed real driver + the tools that let the lead orchestrate). Goal/coverage evaluation reads the shipped per-`(workspace, scope_name)` finding/asset ledgers; there are no aggregation helpers upstream, so the evaluators are hand-rolled over `read_findings`/`read_assets`. Budget is enforced over a `UsageSource` port (the real usage-ledger-backed impl wires in Plan 3).

**Tech Stack:** Rust 2021, `rupu-coverage` (evidence reads + profile `ActiveSet`), `serde`/`serde_yaml`/`serde_json`, `thiserror`, `chrono`, `tokio` (async loop), `tracing`. No new external deps beyond workspace-pinned ones.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md` — this plan implements §11 (envelope + round lifecycle), §12 (termination disjunction), §14 (goals), §15 (coverage), §16 (budget), §18 (definition file), and the Plan-2 scope in §23. The spec is the authority; where its §14/§15 examples assume APIs the shipped `rupu-coverage` doesn't have (`has_poc`, an asset↔finding join, a `verified` flag on the record), this plan maps them onto the real model and says so inline. Read the spec alongside this plan.

## Global Constraints

- **Hexagonal / ports (architecture rule #1).** The lead is reached only via the `LeadDriver` trait; budget reads only via the `UsageSource` trait. `rupu-agentiflow` depends on `rupu-coverage` (reads) and must NOT depend on `rupu-agent`, `rupu-cli`, or any provider crate in Plan 2 — the real `run_agent`-backed driver lands in Plan 3 in a crate that may depend on both.
- **Workspace deps only (rule #3):** crate `Cargo.toml` uses `foo.workspace = true`; never pin a version.
- **Lints:** `[lints] workspace = true`; no `unsafe`; `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings` must be clean.
- **Workspace floor:** `edition = "2021"`, `rust-version = "1.95"`, `version` via `.workspace = true`.
- **Errors:** `thiserror`.
- **Evidence model (the binding reality from #716):** findings/assets live in `rupu-coverage` ledgers keyed by `target_id(workspace, scope_name)`; the envelope reads ONE pooled store via a single `CoveragePaths`. "Verified" means `FindingRecord.report.as_ref().and_then(|r| r.verification.as_ref()).map(|v| v.status) == Some(VerificationStatus::Confirmed)` — there is no `has_poc` and no verified flag on the record; a `Summary`-profile finding (`report: None`) is treated as unverified. Findings and assets are separate ledgers with NO cross-reference, so a goal is EITHER a finding predicate OR an asset predicate — never a join.
- **Stop semantics (spec §12):** terminate on the first of — all `required` goals met / coverage reached / budget hard-cap / operator stop / hard ceiling. The budget soft threshold does NOT stop; it flags "converge" in the round digest.

## Scope boundary (stated so it is not mistaken for missing work)

Plan 2 delivers the deterministic decision engine and the loop, tested with a **mock `LeadDriver`** and **seeded evidence stores**. It deliberately does NOT include: the real `run_agent`-backed lead driver, unit dispatch, collectors wiring, the usage-ledger-backed `UsageSource`, or the on-disk agentiflow run record (`agentiflow.json` / `events.jsonl`). Those are Plan 3 (they need the lead's tools + the fleet to exist). The `verify` path that makes a `verified: true` goal reachable is also Plan 3 (the evaluator here reads the field correctly; nothing in Plan 2 sets it).

## File Structure

New crate `crates/rupu-agentiflow/`:
- `Cargo.toml`
- `src/lib.rs` — module decls + re-exports.
- `src/error.rs` — `AgentiflowError` (parse/validate), evaluator error types.
- `src/def.rs` — `AgentiflowDef` + nested types (`Goal`, `GoalTarget`, `FindingSelector`, `AssetSelector`, `CoverageTarget`, `Budget`, `Scope`, `ScopeRoot`, `Pool`, `RoundConfig`), `parse_str`, `validate`.
- `src/goal.rs` — `GoalEvaluator`, `GoalOutcome`; pure cores over `&[FindingRecord]` / `&[Asset]`.
- `src/coverage.rs` — `CoverageEvaluator`, `CoverageOutcome`.
- `src/budget.rs` — `Budget` enforcement: `UsageSource` trait, `BudgetStage`, `BudgetEnforcer`, `parse_duration`.
- `src/operator.rs` — `OperatorQueue` (file-backed steering channel: `enqueue`/`drain`), `OperatorMessage`.
- `src/envelope.rs` — `LeadDriver` trait, `RoundContext`/`Digest`/`RoundOutcome`, `StopReason`, `Envelope`, `EnvelopeOutcome`, wind-down.

Modified `crates/rupu-coverage/src/lib.rs` — add `VerificationStatus` (and `Verification`) to the `pub use report::{…}` line (export of an existing type; needed so the evaluator can match the enum).

Modified root `Cargo.toml` — add `crates/rupu-agentiflow` to `[workspace] members` and `rupu-agentiflow = { path = "crates/rupu-agentiflow" }` to `[workspace.dependencies]`.

---

## Task 1: Crate scaffold + `AgentiflowDef` model + YAML parse

**Files:**
- Create: `crates/rupu-agentiflow/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/def.rs`
- Modify: root `Cargo.toml`; `crates/rupu-coverage/src/lib.rs`

**Interfaces:**
- Produces: `AgentiflowDef` and nested types (below); `AgentiflowDef::parse_str(&str) -> Result<AgentiflowDef, AgentiflowError>`; `AgentiflowError`.

- [ ] **Step 1: Add the crate to the workspace + export `VerificationStatus`**

Root `Cargo.toml`: add `"crates/rupu-agentiflow",` to `[workspace] members`, and under `[workspace.dependencies]` add `rupu-agentiflow = { path = "crates/rupu-agentiflow" }`.

`crates/rupu-coverage/src/lib.rs`: change the report re-export from
```rust
pub use report::{
    Classification, DisasmLine, EvidenceBlock, FindingProfile, FindingReport, FindingWriteOptions,
};
```
to add the two verification types (they are defined `pub` in `report/types.rs:428,437` but not re-exported):
```rust
pub use report::{
    Classification, DisasmLine, EvidenceBlock, FindingProfile, FindingReport, FindingWriteOptions,
    Verification, VerificationStatus,
};
```
Run `cargo build -p rupu-coverage` to confirm the names resolve (if `report` does not already `pub use` them internally, add `pub use types::{Verification, VerificationStatus};` to `report/mod.rs`).

- [ ] **Step 2: Write the manifest and lib root**

`crates/rupu-agentiflow/Cargo.toml`:
```toml
[package]
name = "rupu-agentiflow"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
rupu-coverage = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_yaml = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
chrono = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```
(If `rupu-coverage`/`serde_yaml`/`chrono`/`tokio`/`tracing` are not yet `[workspace.dependencies]` entries, they already are — they are used across the workspace; confirm with `grep -nE '^(serde_yaml|chrono|tokio|tracing|rupu-coverage) ' Cargo.toml`.)

`crates/rupu-agentiflow/src/lib.rs`:
```rust
//! The agentiflow envelope: the deterministic supervisor that parses an
//! `AgentiflowDef`, evaluates goal / coverage / budget stop conditions over
//! `rupu-coverage` evidence, and runs the round loop against a `LeadDriver`
//! port. Spec: docs/superpowers/specs/2026-09-30-rupu-agentiflows-design.md.

mod budget;
mod coverage;
mod def;
mod envelope;
mod error;
mod goal;
mod operator;

pub use budget::{Budget, BudgetEnforcer, BudgetStage, UsageSource};
pub use coverage::{CoverageEvaluator, CoverageOutcome, CoverageTarget};
pub use def::{
    AgentiflowDef, AssetSelector, FindingSelector, Goal, GoalTarget, Pool, RoundConfig, Scope,
    ScopeRoot,
};
pub use envelope::{Digest, Envelope, EnvelopeOutcome, LeadDriver, RoundContext, RoundOutcome, StopReason};
pub use error::AgentiflowError;
pub use goal::{GoalEvaluator, GoalOutcome};
pub use operator::{OperatorMessage, OperatorQueue};
```

- [ ] **Step 3: Write `error.rs`**
```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentiflowError {
    #[error("agentiflow YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid agentiflow definition: {0}")]
    Invalid(String),
    #[error("unknown engagement profile(s): {0}")]
    UnknownProfile(String),
}
```

- [ ] **Step 4: Write the `AgentiflowDef` model in `def.rs`**

Mirror spec §18. Use `deny_unknown_fields` on every struct (the workflow parser does; it catches typos).
```rust
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentiflowDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub lead: String,
    pub engagement_profiles: Vec<String>,
    #[serde(default)]
    pub goals: Vec<Goal>,
    #[serde(default)]
    pub coverage: Option<crate::coverage::CoverageTarget>,
    #[serde(default)]
    pub budget: Option<crate::budget::Budget>,
    pub scope: Scope,
    pub pool: Pool,
    #[serde(default)]
    pub round: Option<RoundConfig>,
    #[serde(default)]
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub id: String,
    pub objective: String,
    pub target: GoalTarget,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub verify_with: Option<String>,
}
fn default_true() -> bool { true }

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTarget {
    #[serde(default)]
    pub findings: Option<FindingSelector>,
    #[serde(default)]
    pub asset: Option<AssetSelector>,
    #[serde(default)]
    pub count_gte: Option<u64>,
    #[serde(default)]
    pub depth_at_least: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingSelector {
    /// Matches a classification id, e.g. "CWE-94" (via FindingReport::all_classifications()).
    #[serde(default)]
    pub classification: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetSelector {
    /// Profile-namespaced kind, e.g. "network:host".
    pub kind: String,
    /// coordinate-tag -> value, e.g. {host: "1.1.2.2"}. v1 supports string coords.
    #[serde(default)]
    pub locator: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub authorized: bool,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub roots: Vec<ScopeRoot>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScopeRoot {
    /// Profile-namespaced root kind, e.g. "network:scope" / "web:target".
    pub kind: String,
    /// Remaining keys are the root kind's coordinates/attributes, captured opaquely.
    #[serde(flatten)]
    pub fields: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub workflows: WorkflowsSpec,
}

/// `workflows: all` or `workflows: [a, b]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WorkflowsSpec {
    All(AllKeyword),
    List(Vec<String>),
}
impl Default for WorkflowsSpec { fn default() -> Self { WorkflowsSpec::List(Vec::new()) } }
#[derive(Debug, Clone, Deserialize)]
pub enum AllKeyword { #[serde(rename = "all")] All }

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundConfig {
    #[serde(default)]
    pub lead_max_turns: Option<u32>,
    #[serde(default)]
    pub ceiling: Option<Ceiling>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ceiling {
    #[serde(default)]
    pub rounds: Option<u32>,
    #[serde(default)]
    pub wall_clock: Option<String>,
}

impl AgentiflowDef {
    pub fn parse_str(s: &str) -> Result<Self, crate::error::AgentiflowError> {
        Ok(serde_yaml::from_str(s)?)
    }
}
```

- [ ] **Step 5: Write the failing parse test**

In `def.rs` `#[cfg(test)] mod tests`:
```rust
use super::*;

const SAMPLE: &str = r#"
name: acme-pentest
description: Assess the in-scope services.
lead: orchestrator-lead
engagement_profiles: [network, web]
goals:
  - id: rce
    objective: "Find 10 verified RCE issues."
    target: { findings: { classification: "CWE-94" }, count_gte: 10, verified: true }
    required: true
coverage:
  reach: 0.9
  depth: tested
budget:
  usd: 50
  wall_clock: "6h"
  rounds: 40
  soft_at: 0.8
scope:
  authorized: true
  mode: bypass
  roots:
    - kind: network:scope
      cidrs: ["10.0.0.0/24"]
      hosts: ["1.1.2.2"]
    - kind: web:target
      url: "https://app.acme.test"
pool:
  agents: [recon, exploit-verifier, orchestrator-lead]
  workflows: [network-assessment]
round:
  lead_max_turns: 200
  ceiling: { rounds: 40, wall_clock: "8h" }
trigger: manual
"#;

#[test]
fn parses_a_full_definition() {
    let def = AgentiflowDef::parse_str(SAMPLE).unwrap();
    assert_eq!(def.name, "acme-pentest");
    assert_eq!(def.engagement_profiles, ["network", "web"]);
    assert_eq!(def.goals.len(), 1);
    assert_eq!(def.goals[0].target.count_gte, Some(10));
    assert!(def.goals[0].target.verified);
    assert_eq!(def.scope.roots.len(), 2);
    assert_eq!(def.scope.roots[0].kind, "network:scope");
    assert_eq!(def.pool.agents.len(), 3);
}

#[test]
fn rejects_unknown_top_level_field() {
    let bad = "name: x\nlead: y\nengagement_profiles: [code]\nscope: {authorized: true}\npool: {}\nbogus: 1\n";
    assert!(AgentiflowDef::parse_str(bad).is_err());
}
```

- [ ] **Step 6: Run → fail → make it compile/pass**

Run: `cargo test -p rupu-agentiflow def::tests`
Expected: compiles and passes once `def.rs` is complete. Then `cargo clippy -p rupu-agentiflow --all-targets -- -D warnings`.

- [ ] **Step 7: Commit**
```bash
git add Cargo.toml crates/rupu-coverage/src/lib.rs crates/rupu-agentiflow
git commit -m "feat(agentiflow): new crate + AgentiflowDef model + YAML parse"
```

## Task 2: `AgentiflowDef::validate` (fail-closed, against the resolved profile set)

**Files:**
- Modify: `crates/rupu-agentiflow/src/def.rs`

**Interfaces:**
- Consumes: `rupu_coverage::{ProfileRegistry, ActiveSet, registry_with_overlay, builtin_registry, EngagementProfile}`.
- Produces: `AgentiflowDef::resolve_profiles(&self, registry: &ProfileRegistry) -> Result<ActiveSet, AgentiflowError>`; `AgentiflowDef::validate(&self, active: &ActiveSet) -> Result<(), AgentiflowError>`.

- [ ] **Step 1: Write the failing tests**

Build the active set from the builtin registry (the pilot ships `network`/`web` etc. as data). In `def.rs` tests:
```rust
fn active() -> rupu_coverage::ActiveSet {
    rupu_coverage::builtin_registry().unwrap().active_set(&["network".into(), "web".into()]).unwrap()
}

#[test]
fn validate_accepts_the_sample() {
    let def = AgentiflowDef::parse_str(SAMPLE).unwrap();
    def.validate(&active()).unwrap();
}

#[test]
fn validate_rejects_unauthorized_scope() {
    let mut def = AgentiflowDef::parse_str(SAMPLE).unwrap();
    def.scope.authorized = false;
    assert!(def.validate(&active()).is_err());
}

#[test]
fn validate_rejects_lead_not_in_pool() {
    let mut def = AgentiflowDef::parse_str(SAMPLE).unwrap();
    def.lead = "ghost".into();
    assert!(def.validate(&active()).is_err());
}

#[test]
fn validate_rejects_goal_with_both_findings_and_asset() {
    let yaml = SAMPLE.replace(
        "target: { findings: { classification: \"CWE-94\" }, count_gte: 10, verified: true }",
        "target: { findings: { classification: \"CWE-94\" }, asset: { kind: \"network:host\" }, count_gte: 1 }",
    );
    let def = AgentiflowDef::parse_str(&yaml).unwrap();
    assert!(def.validate(&active()).is_err());
}

#[test]
fn validate_rejects_scope_root_kind_not_a_profile_root() {
    let mut def = AgentiflowDef::parse_str(SAMPLE).unwrap();
    def.scope.roots[0].kind = "network:host".into(); // host has a parent → not a root
    assert!(def.validate(&active()).is_err());
}
```

- [ ] **Step 2: Run → fail (no `validate`)**

Run: `cargo test -p rupu-agentiflow def::tests::validate_accepts_the_sample`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `resolve_profiles` + `validate`**

A profile's root kinds are the `AssetKindDef`s with `parent == None`, namespaced `"<profile.id>:<kind.id>"`. `EngagementProfile` exposes `asset_kinds: Vec<AssetKindDef>` and `AssetKindDef { id, parent, coordinates, label }` (from `rupu_coverage`). Add to `impl AgentiflowDef`:
```rust
pub fn resolve_profiles(
    &self,
    registry: &rupu_coverage::ProfileRegistry,
) -> Result<rupu_coverage::ActiveSet, crate::error::AgentiflowError> {
    registry
        .active_set(&self.engagement_profiles)
        .map_err(|e| crate::error::AgentiflowError::UnknownProfile(e.to_string()))
}

pub fn validate(&self, active: &rupu_coverage::ActiveSet) -> Result<(), crate::error::AgentiflowError> {
    use crate::error::AgentiflowError::Invalid;
    if !self.scope.authorized {
        return Err(Invalid("scope.authorized must be true".into()));
    }
    if self.goals.is_empty() && self.coverage.is_none() {
        return Err(Invalid("at least one of goals/coverage is required".into()));
    }
    if !self.pool.agents.iter().any(|a| a == &self.lead) {
        return Err(Invalid(format!("lead `{}` is not in pool.agents", self.lead)));
    }
    // goal predicates
    for g in &self.goals {
        match (&g.target.findings, &g.target.asset) {
            (Some(_), Some(_)) => return Err(Invalid(format!("goal `{}`: target has both findings and asset", g.id))),
            (None, None) => return Err(Invalid(format!("goal `{}`: target has neither findings nor asset", g.id))),
            (Some(_), None) => {
                if g.target.count_gte.is_none() {
                    return Err(Invalid(format!("goal `{}`: findings target requires count_gte", g.id)));
                }
            }
            (None, Some(a)) => {
                if g.target.depth_at_least.is_none() {
                    return Err(Invalid(format!("goal `{}`: asset target requires depth_at_least", g.id)));
                }
                if g.target.verified {
                    return Err(Invalid(format!("goal `{}`: `verified` is not valid on an asset target (the depth rung is the evidence)", g.id)));
                }
                // kind must be owned by an active profile; depth a rung of its ladder
                let profile = active.profile_for_kind(&a.kind)
                    .ok_or_else(|| Invalid(format!("goal `{}`: asset kind `{}` not owned by any active profile", g.id, a.kind)))?;
                let depth = g.target.depth_at_least.as_deref().unwrap();
                if !profile.coverage.depth_ladder.iter().any(|d| d == depth) {
                    return Err(Invalid(format!("goal `{}`: depth `{}` is not a rung of `{}`'s ladder", g.id, depth, profile.id)));
                }
            }
        }
    }
    // scope roots must name a ROOT kind (parent == None) of an active profile
    for root in &self.scope.roots {
        let profile = active.profile_for_kind(&root.kind)
            .ok_or_else(|| Invalid(format!("scope root kind `{}` not owned by any active profile", root.kind)))?;
        let bare = rupu_coverage::profile_of(&root.kind); // "network"
        let local = root.kind.strip_prefix(&format!("{bare}:")).unwrap_or(&root.kind);
        let is_root = profile.asset_kinds.iter().any(|k| k.id == local && k.parent.is_none());
        if !is_root {
            return Err(Invalid(format!("scope root `{}` is not a root asset kind of profile `{}`", root.kind, profile.id)));
        }
    }
    // budget sanity
    if let Some(b) = &self.budget {
        b.validate().map_err(Invalid)?;
    }
    Ok(())
}
```
(`Budget::validate(&self) -> Result<(), String>` is added in Task 6; stub it to `Ok(())` now and fill it there, or land Task 6 first. Pick an order in the ledger.)

- [ ] **Step 4: Run → pass; clippy; commit**

Run: `cargo test -p rupu-agentiflow def::tests && cargo clippy -p rupu-agentiflow --all-targets -- -D warnings`
```bash
git add crates/rupu-agentiflow/src/def.rs
git commit -m "feat(agentiflow): fail-closed AgentiflowDef validation against the active profile set"
```

## Task 3: `GoalEvaluator` — finding predicate

**Files:**
- Create: `crates/rupu-agentiflow/src/goal.rs`

**Interfaces:**
- Consumes: `rupu_coverage::{read_findings, FindingRecord, CoveragePaths, Classification, VerificationStatus, ActiveSet}`; `crate::def::{Goal, GoalTarget, FindingSelector}`.
- Produces:
  - `GoalOutcome { pub id: String, pub met: bool, pub current: u64, pub target: u64, pub detail: String }`.
  - `fn count_matching_findings(records: &[FindingRecord], sel: &FindingSelector, verified: bool) -> u64` (pure).
  - `GoalEvaluator::evaluate(goal: &Goal, paths: &CoveragePaths, active: &ActiveSet) -> Result<GoalOutcome, GoalEvalError>` (reads the ledgers; dispatches finding vs asset — asset arm added in Task 4).

- [ ] **Step 1: Confirm `FindingRecord` construction for tests**

Open `crates/rupu-coverage/src/ledger/events.rs:237` and note `FindingRecord`'s fields + whether they are `pub` and whether a constructor exists. The pure `count_matching_findings` takes `&[FindingRecord]`, so tests must build records. If all fields are `pub`, write a local `mk_finding(classification: Option<&str>, verified: Option<VerificationStatus>) -> FindingRecord` helper that fills the required fields and attaches a minimal `FindingReport` (set `cwe`/`classifications` and `verification`). If fields are private, build the record via `serde_json::from_value(json!({...}))` matching its serde shape, or seed `findings.jsonl` and use the `evaluate` path. Record which approach you used in the report.

- [ ] **Step 2: Write the failing test (pure core)**
```rust
use super::*;
use rupu_coverage::VerificationStatus;

#[test]
fn counts_only_verified_findings_of_the_classification() {
    let recs = vec![
        mk_finding(Some("CWE-94"), Some(VerificationStatus::Confirmed)),
        mk_finding(Some("CWE-94"), Some(VerificationStatus::Unverified)),
        mk_finding(Some("CWE-94"), None),              // Summary profile, report: None
        mk_finding(Some("CWE-79"), Some(VerificationStatus::Confirmed)),
    ];
    let sel = crate::def::FindingSelector { classification: Some("CWE-94".into()) };
    assert_eq!(count_matching_findings(&recs, &sel, true), 1);   // only the confirmed CWE-94
    assert_eq!(count_matching_findings(&recs, &sel, false), 3);  // all CWE-94 regardless of verification
}
```

- [ ] **Step 3: Run → fail**

Run: `cargo test -p rupu-agentiflow goal::tests::counts_only_verified_findings_of_the_classification`
Expected: FAIL to compile.

- [ ] **Step 4: Implement the pure core + the `evaluate` finding arm**
```rust
use crate::def::{FindingSelector, Goal};
use rupu_coverage::{read_findings, ActiveSet, CoveragePaths, FindingRecord, VerificationStatus};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GoalEvalError {
    #[error("reading evidence: {0}")]
    Io(String),
    #[error("goal `{0}`: {1}")]
    Bad(String, String),
}

#[derive(Debug, Clone)]
pub struct GoalOutcome {
    pub id: String,
    pub met: bool,
    pub current: u64,
    pub target: u64,
    pub detail: String,
}

fn is_verified(rec: &FindingRecord) -> bool {
    rec.report
        .as_ref()
        .and_then(|r| r.verification.as_ref())
        .map(|v| v.status == VerificationStatus::Confirmed)
        .unwrap_or(false)
}

fn finding_has_classification(rec: &FindingRecord, class_id: &str) -> bool {
    match rec.report.as_ref() {
        Some(report) => report.all_classifications().iter().any(|c| c.id == class_id),
        None => false,
    }
}

pub fn count_matching_findings(records: &[FindingRecord], sel: &FindingSelector, verified: bool) -> u64 {
    records
        .iter()
        .filter(|r| match &sel.classification {
            Some(id) => finding_has_classification(r, id),
            None => true,
        })
        .filter(|r| !verified || is_verified(r))
        .count() as u64
}

pub struct GoalEvaluator;

impl GoalEvaluator {
    pub fn evaluate(goal: &Goal, paths: &CoveragePaths, active: &ActiveSet) -> Result<GoalOutcome, GoalEvalError> {
        if let Some(sel) = &goal.target.findings {
            let recs = read_findings(paths).map_err(|e| GoalEvalError::Io(e.to_string()))?;
            let target = goal.target.count_gte.unwrap_or(1);
            let current = count_matching_findings(&recs, sel, goal.target.verified);
            return Ok(GoalOutcome {
                id: goal.id.clone(),
                met: current >= target,
                current,
                target,
                detail: format!(
                    "{}/{} {}findings{}",
                    current, target,
                    if goal.target.verified { "verified " } else { "" },
                    sel.classification.as_deref().map(|c| format!(" [{c}]")).unwrap_or_default(),
                ),
            });
        }
        // asset arm: Task 4
        Self::evaluate_asset(goal, paths, active)
    }
}
```
Add a temporary `fn evaluate_asset(...) -> Result<GoalOutcome, GoalEvalError> { Err(GoalEvalError::Bad(goal.id.clone(), "asset goals land in Task 4".into())) }` placeholder, replaced in Task 4.

- [ ] **Step 5: Run → pass; clippy; commit**
```bash
git add crates/rupu-agentiflow/src/goal.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): goal evaluator — finding predicate (classification + verified)"
```

## Task 4: `GoalEvaluator` — asset predicate (locator + depth ladder)

**Files:**
- Modify: `crates/rupu-agentiflow/src/goal.rs`

**Interfaces:**
- Consumes: `rupu_coverage::{read_assets, Asset, Coordinate, Locator, ActiveSet}`; `crate::def::AssetSelector`.
- Produces: `fn asset_matches(a: &Asset, sel: &AssetSelector, min_depth_idx: usize, ladder: &[String]) -> bool` (pure); the real `GoalEvaluator::evaluate_asset`.

- [ ] **Step 1: Write the failing test (pure core)**

`Asset::new(kind, locator, label, parent)` builds an asset (`asset/mod.rs`); set `depth` after. `Coordinate::Host(String)` is the host coord. In `goal.rs` tests:
```rust
use rupu_coverage::{Asset, Coordinate, Locator};

fn host_asset(host: &str, depth: Option<&str>) -> Asset {
    let mut a = Asset::new("network:host", Locator(vec![Coordinate::Host(host.into())]), host, None);
    a.depth = depth.map(|d| d.to_string());
    a
}

#[test]
fn asset_matches_kind_locator_and_min_depth() {
    let ladder = ["discovered", "enumerated", "tested", "exploited"].map(String::from);
    let sel = crate::def::AssetSelector {
        kind: "network:host".into(),
        locator: std::collections::BTreeMap::from([("host".into(), "1.1.2.2".into())]),
    };
    let min = 3; // "exploited"
    assert!(asset_matches(&host_asset("1.1.2.2", Some("exploited")), &sel, min, &ladder));
    assert!(!asset_matches(&host_asset("1.1.2.2", Some("tested")), &sel, min, &ladder)); // below
    assert!(!asset_matches(&host_asset("9.9.9.9", Some("exploited")), &sel, min, &ladder)); // wrong host
    assert!(!asset_matches(&host_asset("1.1.2.2", None), &sel, min, &ladder)); // no depth yet
}
```

- [ ] **Step 2: Run → fail**

Run: `cargo test -p rupu-agentiflow goal::tests::asset_matches_kind_locator_and_min_depth`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the locator match + depth check + real `evaluate_asset`**

Map the supported string coordinate tags to `Coordinate` and compare. v1 supports the string-valued coords; an unsupported tag in the selector is a miss (document it).
```rust
fn coord_value_matches(loc: &Locator, tag: &str, want: &str) -> bool {
    loc.0.iter().any(|c| match (tag, c) {
        ("host", Coordinate::Host(v)) => v == want,
        ("url", Coordinate::Url(v)) => v == want,
        ("path", Coordinate::Path(v)) => v == want,
        ("symbol", Coordinate::Symbol(v)) => v == want,
        ("sha256", Coordinate::Sha256(v)) => v == want,
        ("commit", Coordinate::Commit(v)) => v == want,
        ("param", Coordinate::Param(v)) => v == want,
        _ => false, // structured coords (port/line_range/offset/address/http_route/resource_id) unsupported in v1 locator match
    })
}

fn asset_matches(a: &Asset, sel: &AssetSelector, min_depth_idx: usize, ladder: &[String]) -> bool {
    if a.kind != sel.kind {
        return false;
    }
    if !sel.locator.iter().all(|(tag, want)| coord_value_matches(&a.locator, tag, want)) {
        return false;
    }
    match a.depth.as_ref().and_then(|d| ladder.iter().position(|r| r == d)) {
        Some(idx) => idx >= min_depth_idx,
        None => false, // no depth recorded (or unknown rung) → not at/above
    }
}
```
Replace the `evaluate_asset` placeholder:
```rust
impl GoalEvaluator {
    fn evaluate_asset(goal: &Goal, paths: &CoveragePaths, active: &ActiveSet) -> Result<GoalOutcome, GoalEvalError> {
        let sel = goal.target.asset.as_ref().ok_or_else(|| GoalEvalError::Bad(goal.id.clone(), "no asset selector".into()))?;
        let depth = goal.target.depth_at_least.as_deref().ok_or_else(|| GoalEvalError::Bad(goal.id.clone(), "no depth_at_least".into()))?;
        let profile = active.profile_for_kind(&sel.kind).ok_or_else(|| GoalEvalError::Bad(goal.id.clone(), format!("kind `{}` unowned", sel.kind)))?;
        let ladder = &profile.coverage.depth_ladder;
        let min = ladder.iter().position(|r| r == depth).ok_or_else(|| GoalEvalError::Bad(goal.id.clone(), format!("depth `{depth}` not in ladder")))?;
        let assets = read_assets(&paths.assets).map_err(|e| GoalEvalError::Io(e.to_string()))?;
        let current = assets.iter().filter(|a| asset_matches(a, sel, min, ladder)).count() as u64;
        let target = goal.target.count_gte.unwrap_or(1); // asset goals default to "exists" (>=1)
        Ok(GoalOutcome {
            id: goal.id.clone(),
            met: current >= target,
            current,
            target,
            detail: format!("{}/{} assets {} @ depth>={}", current, target, sel.kind, depth),
        })
    }
}
```
Use `paths.assets` (the `CoveragePaths` field holding the assets.jsonl path; confirm the field name in `ledger/paths.rs` — it is the `assets` path).

- [ ] **Step 4: Run → pass; clippy; commit**
```bash
git add crates/rupu-agentiflow/src/goal.rs
git commit -m "feat(agentiflow): goal evaluator — asset predicate (locator + depth ladder)"
```

## Task 5: `CoverageEvaluator` — fraction of enumerated kinds at depth

**Files:**
- Create: `crates/rupu-agentiflow/src/coverage.rs`

**Interfaces:**
- Consumes: `rupu_coverage::{read_assets, Asset, ActiveSet, CoveragePaths}`.
- Produces:
  - `CoverageTarget { pub reach: f64, pub depth: Option<String>, pub kinds: Option<Vec<String>> }` (`Deserialize`, `deny_unknown_fields`).
  - `CoverageOutcome { pub met: bool, pub fraction: f64, pub per_kind: Vec<(String, f64)> }`.
  - `CoverageEvaluator::evaluate(target: &CoverageTarget, paths: &CoveragePaths, active: &ActiveSet) -> Result<CoverageOutcome, CoverageEvalError>`.

- [ ] **Step 1: Write the failing test**

Reuse the `host_asset`-style helper. Seed assets and assert the fraction. In `coverage.rs` tests, build assets in-memory and test a pure `fraction_at_depth(assets, "network", &["host"], "tested", ladder) -> (covered, total)` helper, then the `evaluate` wrapper over a seeded `read_assets` file is optional (prefer the pure helper for determinism):
```rust
#[test]
fn fraction_counts_enumerated_kind_at_or_above_depth() {
    let ladder: Vec<String> = ["discovered","enumerated","tested","exploited"].iter().map(|s| s.to_string()).collect();
    let assets = vec![
        host_asset("a", Some("tested")),
        host_asset("b", Some("exploited")),
        host_asset("c", Some("discovered")), // below "tested"
    ];
    let (covered, total) = fraction_at_depth(&assets, "network:host", "tested", &ladder);
    assert_eq!((covered, total), (2, 3));
}
```

- [ ] **Step 2: Run → fail; Step 3: implement**
```rust
use rupu_coverage::{read_assets, ActiveSet, Asset, CoveragePaths};
use thiserror::Error;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageTarget {
    pub reach: f64,
    #[serde(default)]
    pub depth: Option<String>,
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
}

#[derive(Debug, Error)]
pub enum CoverageEvalError {
    #[error("reading assets: {0}")]
    Io(String),
    #[error("{0}")]
    Bad(String),
}

#[derive(Debug, Clone)]
pub struct CoverageOutcome {
    pub met: bool,
    pub fraction: f64,
    pub per_kind: Vec<(String, f64)>,
}

/// (covered, total) assets of `namespaced_kind` whose depth rung index >= index(min_depth).
fn fraction_at_depth(assets: &[Asset], namespaced_kind: &str, min_depth: &str, ladder: &[String]) -> (u64, u64) {
    let min = ladder.iter().position(|r| r == min_depth);
    let mut covered = 0u64;
    let mut total = 0u64;
    for a in assets.iter().filter(|a| a.kind == namespaced_kind) {
        total += 1;
        let idx = a.depth.as_ref().and_then(|d| ladder.iter().position(|r| r == d));
        if let (Some(i), Some(m)) = (idx, min) {
            if i >= m {
                covered += 1;
            }
        }
    }
    (covered, total)
}

pub struct CoverageEvaluator;

impl CoverageEvaluator {
    pub fn evaluate(target: &CoverageTarget, paths: &CoveragePaths, active: &ActiveSet) -> Result<CoverageOutcome, CoverageEvalError> {
        let assets = read_assets(&paths.assets).map_err(|e| CoverageEvalError::Io(e.to_string()))?;
        let mut covered_total = 0u64;
        let mut all_total = 0u64;
        let mut per_kind = Vec::new();
        for profile in active.profiles() {
            // `enumerates` holds BARE kinds; namespace them with the profile id.
            let kinds: Vec<String> = match &target.kinds {
                Some(explicit) => explicit.clone(), // may already be namespaced; accept as given
                None => profile.coverage.enumerates.iter().map(|k| format!("{}:{}", profile.id, k)).collect(),
            };
            let depth = target.depth.clone().or_else(|| profile.coverage.depth_ladder.last().cloned());
            let Some(depth) = depth else { continue };
            for k in kinds {
                let nk = if k.contains(':') { k.clone() } else { format!("{}:{}", profile.id, k) };
                let (c, t) = fraction_at_depth(&assets, &nk, &depth, &profile.coverage.depth_ladder);
                if t > 0 {
                    per_kind.push((nk, c as f64 / t as f64));
                }
                covered_total += c;
                all_total += t;
            }
        }
        // 0/0 => 0.0 (nothing discovered yet is "not covered", so the flow keeps going).
        let fraction = if all_total == 0 { 0.0 } else { covered_total as f64 / all_total as f64 };
        Ok(CoverageOutcome { met: fraction >= target.reach, fraction, per_kind })
    }
}
```

- [ ] **Step 4: Run → pass; clippy; commit**
```bash
git add crates/rupu-agentiflow/src/coverage.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): coverage evaluator — fraction of enumerated kinds at depth"
```

## Task 6: `Budget` + `BudgetEnforcer` over a `UsageSource` port

**Files:**
- Create: `crates/rupu-agentiflow/src/budget.rs`

**Interfaces:**
- Produces:
  - `Budget { usd: Option<f64>, tokens: Option<u64>, wall_clock: Option<String>, rounds: Option<u32>, soft_at: Option<f64> }` (`Deserialize`, `deny_unknown_fields`) + `Budget::validate(&self) -> Result<(), String>`.
  - `trait UsageSource: Send + Sync { fn spent_usd(&self) -> f64; fn spent_tokens(&self) -> u64; }`.
  - `enum BudgetStage { Ok, Soft, Hard { dimension: String } }`.
  - `BudgetEnforcer { budget: Budget, started: chrono::DateTime<chrono::Utc>, soft_at: f64 }` + `new(budget, started)` + `stage(&self, usage: &dyn UsageSource, round: u32, now: chrono::DateTime<chrono::Utc>) -> BudgetStage`.
  - `fn parse_duration(s: &str) -> Result<chrono::Duration, String>` (supports `Ns`/`Nm`/`Nh`/`Nd`).

- [ ] **Step 1: Write the failing tests**
```rust
use super::*;
use chrono::{Duration, Utc};

struct FixedUsage { usd: f64, tokens: u64 }
impl UsageSource for FixedUsage {
    fn spent_usd(&self) -> f64 { self.usd }
    fn spent_tokens(&self) -> u64 { self.tokens }
}

fn budget() -> Budget {
    Budget { usd: Some(50.0), tokens: Some(20_000_000), wall_clock: Some("6h".into()), rounds: Some(40), soft_at: Some(0.8) }
}

#[test]
fn hard_when_any_dimension_hits_cap() {
    let start = Utc::now();
    let e = BudgetEnforcer::new(budget(), start);
    let now = start;
    assert!(matches!(e.stage(&FixedUsage { usd: 50.0, tokens: 0 }, 0, now), BudgetStage::Hard { .. }));
    assert!(matches!(e.stage(&FixedUsage { usd: 0.0, tokens: 0 }, 40, now), BudgetStage::Hard { .. }));
    let late = start + Duration::hours(6);
    assert!(matches!(e.stage(&FixedUsage { usd: 0.0, tokens: 0 }, 0, late), BudgetStage::Hard { .. }));
}

#[test]
fn soft_at_threshold_then_ok_below() {
    let start = Utc::now();
    let e = BudgetEnforcer::new(budget(), start);
    assert!(matches!(e.stage(&FixedUsage { usd: 40.0, tokens: 0 }, 0, start), BudgetStage::Soft)); // 80% USD
    assert!(matches!(e.stage(&FixedUsage { usd: 10.0, tokens: 0 }, 0, start), BudgetStage::Ok));
}

#[test]
fn parse_duration_units() {
    assert_eq!(parse_duration("6h").unwrap(), Duration::hours(6));
    assert_eq!(parse_duration("30m").unwrap(), Duration::minutes(30));
    assert!(parse_duration("banana").is_err());
}
```

- [ ] **Step 2: Run → fail; Step 3: implement**
```rust
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    #[serde(default)]
    pub usd: Option<f64>,
    #[serde(default)]
    pub tokens: Option<u64>,
    #[serde(default)]
    pub wall_clock: Option<String>,
    #[serde(default)]
    pub rounds: Option<u32>,
    #[serde(default)]
    pub soft_at: Option<f64>,
}

impl Budget {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(u) = self.usd { if u < 0.0 { return Err("budget.usd must be >= 0".into()); } }
        if let Some(s) = self.soft_at { if !(0.0..=1.0).contains(&s) { return Err("budget.soft_at must be in [0,1]".into()); } }
        if let Some(w) = &self.wall_clock { parse_duration(w)?; }
        Ok(())
    }
}

pub trait UsageSource: Send + Sync {
    fn spent_usd(&self) -> f64;
    fn spent_tokens(&self) -> u64;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetStage {
    Ok,
    Soft,
    Hard { dimension: String },
}

pub struct BudgetEnforcer {
    budget: Budget,
    started: DateTime<Utc>,
    soft_at: f64,
}

impl BudgetEnforcer {
    pub fn new(budget: Budget, started: DateTime<Utc>) -> Self {
        let soft_at = budget.soft_at.unwrap_or(0.8);
        Self { budget, started, soft_at }
    }

    pub fn stage(&self, usage: &dyn UsageSource, round: u32, now: DateTime<Utc>) -> BudgetStage {
        // (fraction, dimension-name) for each SET dimension.
        let mut fracs: Vec<(f64, &str)> = Vec::new();
        if let Some(cap) = self.budget.usd { if cap > 0.0 { fracs.push((usage.spent_usd() / cap, "usd")); } }
        if let Some(cap) = self.budget.tokens { if cap > 0 { fracs.push((usage.spent_tokens() as f64 / cap as f64, "tokens")); } }
        if let Some(cap) = self.budget.rounds { if cap > 0 { fracs.push((round as f64 / cap as f64, "rounds")); } }
        if let Some(w) = &self.budget.wall_clock {
            if let Ok(d) = parse_duration(w) {
                let secs = d.num_seconds().max(1) as f64;
                let elapsed = (now - self.started).num_seconds().max(0) as f64;
                fracs.push((elapsed / secs, "wall_clock"));
            }
        }
        if let Some((_, dim)) = fracs.iter().find(|(f, _)| *f >= 1.0) {
            return BudgetStage::Hard { dimension: (*dim).to_string() };
        }
        if fracs.iter().any(|(f, _)| *f >= self.soft_at) {
            return BudgetStage::Soft;
        }
        BudgetStage::Ok
    }
}

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).ok_or_else(|| format!("no unit in `{s}`"))?);
    let n: i64 = num.parse().map_err(|_| format!("bad number in `{s}`"))?;
    match unit {
        "s" => Ok(Duration::seconds(n)),
        "m" => Ok(Duration::minutes(n)),
        "h" => Ok(Duration::hours(n)),
        "d" => Ok(Duration::days(n)),
        other => Err(format!("unknown duration unit `{other}`")),
    }
}
```

- [ ] **Step 4: Run → pass; clippy; commit**
```bash
git add crates/rupu-agentiflow/src/budget.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): budget enforcer (4 dims, soft/hard) over a UsageSource port"
```

## Task 7: `OperatorQueue` — file-backed steering channel

**Files:**
- Create: `crates/rupu-agentiflow/src/operator.rs`

**Interfaces:**
- Produces:
  - `OperatorMessage { pub ts: String, pub body: String, pub stop: bool }` (serde).
  - `OperatorQueue { root: PathBuf }` + `new(root)`, `enqueue(&self, msg: &OperatorMessage) -> io::Result<()>` (atomic tmp+rename into a `steering/` dir), `drain(&self) -> io::Result<Vec<OperatorMessage>>` (read all, oldest-first by filename, delete).

This is the §17 operator→lead channel realized without the session worker: `rupu agentiflow send` (Plan 4) calls `enqueue`; the envelope calls `drain` each round. Mirror `rupu-fleet`'s mailbox atomic-rename discipline (one JSON file per message; drain reads then removes).

- [ ] **Step 1: Write the failing test**
```rust
#[test]
fn enqueue_then_drain_once_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let q = OperatorQueue::new(tmp.path());
    q.enqueue(&OperatorMessage { ts: "t1".into(), body: "focus auth".into(), stop: false }).unwrap();
    q.enqueue(&OperatorMessage { ts: "t2".into(), body: "wrap up".into(), stop: true }).unwrap();
    let msgs = q.drain().unwrap();
    assert_eq!(msgs.len(), 2);
    assert!(msgs.iter().any(|m| m.stop));
    assert!(q.drain().unwrap().is_empty());
}
```

- [ ] **Step 2: Run → fail; Step 3: implement** (one JSON file per message under `<root>/steering/`, filename = a monotonically sortable id e.g. `<unix_nanos>-<rand>.json`; `drain` lists, sorts, reads, deletes; a torn/parse-failing file is skipped). Use `serde_json`. Keep it ~40 lines.

- [ ] **Step 4: Run → pass; clippy; commit**
```bash
git add crates/rupu-agentiflow/src/operator.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): file-backed operator steering queue"
```

## Task 8: The envelope round loop + `LeadDriver` port + stop disjunction + wind-down

**Files:**
- Create: `crates/rupu-agentiflow/src/envelope.rs`

**Interfaces:**
- Consumes: everything above + `rupu_coverage::{CoveragePaths, ActiveSet}`.
- Produces:
  - `#[async_trait::async_trait] pub trait LeadDriver: Send { async fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome; }` — **or**, to avoid an `async-trait` dep, a sync `trait LeadDriver { fn run_round(&mut self, ctx: &RoundContext) -> RoundOutcome; }` and make `Envelope::run` call it directly (the real Plan-3 driver will do its async work inside via a handle). Prefer the **sync** trait for Plan 2 (no new dep); document that the Plan-3 real driver bridges async internally.
  - `RoundContext { pub round: u32, pub digest: Digest }`; `Digest { pub goals: Vec<GoalOutcome>, pub coverage: Option<CoverageOutcome>, pub budget: BudgetStage, pub converge: bool, pub steering: Vec<OperatorMessage> }`.
  - `enum RoundOutcome { Yielded, TurnBudgetHit, Error(String) }`.
  - `enum StopReason { GoalsMet, CoverageReached, BudgetExhausted { dimension: String }, OperatorStop, Ceiling }`.
  - `struct EnvelopeConfig { pub goals: Vec<Goal>, pub coverage: Option<CoverageTarget>, pub ceiling_rounds: Option<u32>, pub ceiling_wall_clock: Option<chrono::Duration> }` (distilled from the def by the caller).
  - `struct Envelope { paths: CoveragePaths, active: ActiveSet, cfg: EnvelopeConfig, budget: BudgetEnforcer, operator: OperatorQueue, started: DateTime<Utc> }` + `new(...)`.
  - `EnvelopeOutcome { pub stop: StopReason, pub goals: Vec<GoalOutcome>, pub coverage: Option<CoverageOutcome>, pub rounds: u32, pub summary: String }`.
  - `Envelope::evaluate_stop(&self, usage: &dyn UsageSource, round: u32, now: DateTime<Utc>) -> Result<Option<StopReason>, ...>` and `Envelope::run(&mut self, lead: &mut dyn LeadDriver, usage: &dyn UsageSource) -> EnvelopeOutcome` (the loop; a `now`/clock is injected for testability, e.g. a `Fn() -> DateTime<Utc>`).

- [ ] **Step 1: Write the failing tests (mock lead + seeded evidence)**

A mock lead that records how many rounds it ran and always `Yielded`. Seed a `CoveragePaths` in a tempdir (write a findings/asset ledger directly, or reuse the Task 3/4 `mk_*` + a tiny JSONL writer) and a mock `UsageSource`. Assert each stop:
```rust
struct MockLead { rounds: u32 }
impl LeadDriver for MockLead {
    fn run_round(&mut self, _ctx: &RoundContext) -> RoundOutcome { self.rounds += 1; RoundOutcome::Yielded }
}

#[test]
fn stops_when_all_required_goals_met() { /* seed evidence so the goal is already met; run; assert StopReason::GoalsMet and lead ran 0 rounds (stop checked first) */ }

#[test]
fn stops_on_budget_hard_cap() { /* UsageSource over cap; assert BudgetExhausted */ }

#[test]
fn stops_on_operator_stop_message() { /* enqueue an OperatorMessage{stop:true}; assert OperatorStop */ }

#[test]
fn stops_on_round_ceiling() { /* no goals ever met, ceiling_rounds=3; assert Ceiling and lead ran exactly 3 rounds */ }

#[test]
fn soft_budget_sets_converge_in_digest_without_stopping() { /* UsageSource at soft; lead captures ctx.digest.converge == true; still runs; stops on ceiling */ }
```

- [ ] **Step 2: Run → fail; Step 3: implement the loop**

Order of the stop checks each round (spec §12 — first match wins): (1) all `required` goals met → `GoalsMet`; (2) coverage set and reached → `CoverageReached`; (3) budget `Hard` → `BudgetExhausted`; (4) any drained operator message has `stop` → `OperatorStop`; (5) `round >= ceiling_rounds` or `elapsed >= ceiling_wall_clock` → `Ceiling`. The loop:
```
round = 0
loop:
    drain operator queue -> steering (retain for the digest; note any stop)
    evaluate goals (GoalEvaluator::evaluate per goal), coverage, budget.stage(usage, round, now)
    if stop condition (above) -> break with that StopReason
    digest = Digest { goals, coverage, budget, converge: budget == Soft, steering }
    lead.run_round(&RoundContext { round, digest })   // Yielded / TurnBudgetHit / Error
    round += 1
wind_down: build summary (stop reason + per-goal met/unmet + rounds)
```
Operator `stop` must win even if also at a ceiling; check in the documented order. A `RoundOutcome::Error` is recorded in the summary but does not itself stop the loop (the budget/ceiling will); document this.

The `now` clock and the "wait between rounds" are injected: Plan 2's `run` takes a `now: &dyn Fn() -> DateTime<Utc>` (tests pass a controllable clock) and performs NO real sleeping (the real daemon's wait-on-signal is layered in Plan 3). State this in the function doc.

- [ ] **Step 4: Run → pass; full-crate check; commit**

Run: `cargo test -p rupu-agentiflow && cargo clippy -p rupu-agentiflow --all-targets -- -D warnings`
```bash
git add crates/rupu-agentiflow/src/envelope.rs crates/rupu-agentiflow/src/lib.rs
git commit -m "feat(agentiflow): envelope round loop + LeadDriver port + stop disjunction + wind-down"
```

---

## Self-Review

**Spec coverage:**
- §18 definition file + parse/validate → Tasks 1–2 (fail-closed: profiles resolvable, goal predicates well-formed against the active set, scope roots are real root kinds, lead ∈ pool, scope.authorized, budget sane). ✓
- §14 goals as objective predicates over verified evidence → Tasks 3–4 (finding predicate with `verified` = `verification.status == Confirmed`; asset predicate with locator + depth ladder). The spec's `has_poc` is dropped (not in the shipped model) and the asset/finding join is avoided (separate ledgers) — stated in Global Constraints. ✓
- §15 coverage stop as asset-tree depth fraction → Task 5. ✓
- §16 budget, 4 dims, soft/hard → Task 6. ✓
- §17 operator steering channel → Task 7 (file queue; `send` CLI is Plan 4). ✓
- §11 round loop + §12 termination disjunction + wind-down → Task 8. ✓
- §23 Plan-2 scope: the "session-worker lift" is realized as the `LeadDriver` port (the real `run_agent`-backed driver + operator `send` CLI + run persistence are Plan 3, stated in the scope boundary). ✓

**Deliberate deviations from the spec, for the reviewer:**
1. `LeadDriver` is a **sync** trait (no `async-trait` dep); the real Plan-3 driver bridges async internally. 2. `verified` on goals = `verification.status == Confirmed`; `has_poc` dropped. 3. `verified: true` is forbidden on asset targets (the depth rung is the evidence). 4. The session-worker is realized as a port + operator file-queue, not a lift (per the Decision-2 discussion; the spec's design intent — envelope drives a session-like lead, operator steers by messages — is preserved).

**Placeholder scan:** the only intentional temporary is Task 3's `evaluate_asset` stub, replaced in Task 4 (noted in both). Task 1 Step 1 adds a one-line `rupu-coverage` re-export. Task 2's `Budget::validate` depends on Task 6 — the ledger must order Task 6 before Task 2's final green, or stub `Budget::validate` to `Ok(())` in Task 2 and fill in Task 6 (noted).

**Type consistency:** `GoalOutcome`/`CoverageOutcome`/`BudgetStage`/`StopReason`/`Digest`/`RoundOutcome` are defined once and consumed consistently in Task 8. `CoveragePaths.assets` is the assets-path field (confirm exact field name in `ledger/paths.rs` during Task 4). `read_findings(&CoveragePaths)` vs `read_assets(&Path)` — the signatures differ (findings take `&CoveragePaths`, assets take `&paths.assets`); honored in Tasks 3/4/5.

**Task ordering note for the executor:** land Task 6 (`Budget::validate`/`parse_duration`) before Task 2's final assertion, or stub `Budget::validate` in Task 2. Everything else is linear 1→8.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-10-04-rupu-agentiflows-plan-2-envelope.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — fresh subagent per task, spec+quality review between, broad review at the end (how Plan 1 was built).

**2. Inline Execution** — execute here with checkpoints.

**Which approach?**
