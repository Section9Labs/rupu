# Finding reports — Plan 1: the structured report contract

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Agents record a finding once, as a typed and validated `report` on the finding record. Whether a report is required is governed by a `full` / `summary` profile, and proof-of-concept files are stored with the finding.

**Architecture:** A new `report` module in `rupu-coverage` owns the types, the embedded JSON Schema, the validator, the artifact store, and the prompt guidance. `report_finding()` (the one write path shared by the agent builtin and the MCP `findings.record` tool) takes a `FindingWriteOptions` and enforces the profile. The profile travels to tools on `ToolContext.findings`. It is resolved step → workflow `defaults` → agent frontmatter → `full` in `DefaultStepFactory`, and from the agent spec in the standalone/dispatch/session CLI paths.

**Tech Stack:** Rust 2021, serde / serde_json, sha2, ulid, thiserror, jsonschema 0.18 (dev-dependency, already a workspace dep), tokio tests.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md`

## Global Constraints

- Workspace deps only. Versions live in the root `Cargo.toml`; crates use `x.workspace = true`.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- `rupu-coverage` must not depend on `rupu-config` or `rupu-cli`. Config → options conversion lives in `rupu-cli`.
- Never run package-wide `cargo fmt`. Format only the files you touched: `rustfmt --edition 2021 <file>`.
- Per-crate test commands: `cargo test -p <crate>`. Measure the baseline yourself before assuming anything is red.
- Never use bare `git stash` / `git stash pop`.
- Profile names are exactly `full` and `summary`. The built-in default is `full`. Legacy ledger lines without a `profile` key deserialize as `summary`.
- Sentinels are exact strings: `Unknown`, `Not Applicable`, `None Provided`, `None`, and the prefix `Not Provided — ` (with an em dash U+2014 and a space on each side) followed by a non-empty justification.
- Artifact cap default: 500 MB per file (`524_288_000` bytes). Report size budget default: 256 KiB (`262_144` bytes) serialized.
- No organisation- or standard-specific names (no "TVM", no vendor ticket URLs) in code, schema, or shipped prompts.
- Agent frontmatter keys are camelCase (`findingsProfile`); workflow YAML keys are snake_case (`findings_profile`), matching each file format's existing convention.

## Deviations from the spec (deliberate, flag in the PR)

1. **`parallel:` sub-steps** inherit the parent step's `findings_profile`. The step factory resolves by the parent's step id and cannot tell sub-steps that share an agent apart. `SubStep` gets no field.
2. **`action:` steps** use the workflow `defaults.findings_profile` (the MCP dispatcher is built once per run). A `findings_profile:` on an `action:` step is a parse error, so it can't be silently ignored.
3. **Agent frontmatter key** is `findingsProfile` (camelCase, like `maxTokens`), not `findings_profile`.

## File map

| File | Responsibility |
|---|---|
| `crates/rupu-coverage/src/report/mod.rs` | module root + re-exports |
| `crates/rupu-coverage/src/report/profile.rs` | `FindingProfile` + precedence resolve |
| `crates/rupu-coverage/src/report/types.rs` | `FindingReport` and nested types |
| `crates/rupu-coverage/src/report/validate.rs` | validator → `ReportValidationError` |
| `crates/rupu-coverage/src/report/schema.rs` | embedded canonical schema + advertised (provider-safe) schema |
| `crates/rupu-coverage/schema/finding_report.schema.json` | the canonical JSON Schema (draft-07) |
| `crates/rupu-coverage/src/report/artifacts.rs` | content-addressed artifact store |
| `crates/rupu-coverage/src/report/options.rs` | `FindingWriteOptions` |
| `crates/rupu-coverage/src/report/guidance.rs` | system-prompt guidance for the full profile |
| `crates/rupu-coverage/src/tools/report_finding.rs` | write path (modified) |
| `crates/rupu-coverage/src/ledger/{events,paths}.rs` | `FindingRecord` fields; `CoveragePaths.workspace` |
| `crates/rupu-coverage/tests/report_schema_lockstep.rs` + `tests/fixtures/finding_report/valid_full.json` | schema ↔ validator agreement |
| `crates/rupu-config/src/findings_config.rs` | `[findings]` config section |
| `crates/rupu-tools/src/tool.rs` | `ToolContext.findings` |
| `crates/rupu-agent/src/{coverage_tools,runner,spec}.rs` | profile-aware tool, guidance injection, `findingsProfile` |
| `crates/rupu-mcp/src/tools/findings.rs` | `findings.record` profile + report |
| `crates/rupu-orchestrator/src/{workflow,step_factory}.rs` | `findings_profile` fields, validation, resolution |
| `crates/rupu-cli/src/findings_opts.rs`, `cmd/{run,dispatch,session,workflow,findings}.rs`, `resume.rs`, `lib.rs` | wiring + `rupu findings schema` |

---

### Task 1: Report types, profile, and the extended FindingRecord

**Files:**
- Create: `crates/rupu-coverage/src/report/mod.rs`, `report/profile.rs`, `report/types.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`, `crates/rupu-coverage/src/ledger/events.rs:213-231` (`FindingRecord`)

**Interfaces:**
- Produces:
  - `rupu_coverage::FindingProfile { Full, Summary }` (Default = `Full`; `FindingProfile::legacy() -> Summary`; `FindingProfile::resolve(step: Option<Self>, workflow: Option<Self>, agent: Option<Self>) -> Self`).
  - `rupu_coverage::report::*` types: `FindingReport`, `OrSentinel<T>`, `RiskLevel`, `Likelihood`, `Ownership`, `Ticket`, `Rating`, `ReportLocation`, `ChainHop`, `HopRole`, `EvidenceClaim`, `Patch`, `CiDetection`, `RegressionTest`, `CrossRef`, `Relation`, `ArtifactRef`, `ArtifactKind`, `ArtifactStorage`, `Verification`, `VerificationStatus`, const `NOT_PROVIDED_PREFIX`.
  - `FindingRecord.profile: FindingProfile`, `FindingRecord.report: Option<FindingReport>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-coverage/src/report/profile.rs`:

```rust
//! Which findings contract a run records under.

use serde::{Deserialize, Serialize};

/// `full` requires a complete [`crate::report::FindingReport`]; `summary` is
/// the lightweight record (summary + severity + evidence) rupu has always
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingProfile {
    /// The built-in default: a finding is a full report.
    #[default]
    Full,
    Summary,
}

impl FindingProfile {
    /// The profile a ledger line gets when it predates profiles. Every such
    /// line was written as a summary record.
    pub fn legacy() -> Self {
        Self::Summary
    }

    /// Precedence, most specific first: step → workflow `defaults` → agent
    /// frontmatter → built-in default ([`FindingProfile::Full`]).
    pub fn resolve(step: Option<Self>, workflow: Option<Self>, agent: Option<Self>) -> Self {
        step.or(workflow).or(agent).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::FindingProfile::{self, Full, Summary};

    #[test]
    fn resolve_prefers_the_most_specific_setting() {
        assert_eq!(FindingProfile::resolve(Some(Summary), Some(Full), Some(Full)), Summary);
        assert_eq!(FindingProfile::resolve(None, Some(Summary), Some(Full)), Summary);
        assert_eq!(FindingProfile::resolve(None, None, Some(Summary)), Summary);
        assert_eq!(FindingProfile::resolve(None, None, None), Full);
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Full).unwrap(), "\"full\"");
        assert_eq!(serde_json::from_str::<FindingProfile>("\"summary\"").unwrap(), Summary);
    }
}
```

Add these tests at the bottom of `crates/rupu-coverage/src/ledger/events.rs`'s existing `mod tests`:

```rust
    #[test]
    fn legacy_finding_line_deserializes_as_summary_without_report() {
        let line = r#"{"id":"fnd_1","scope":"repo","summary":"s","severity":"high",
            "evidence":{"rationale":"r"},
            "declared_by":{"run_id":"run_1","model":"m","surface":"workflow"},
            "declared_at":"2026-09-01T00:00:00Z"}"#;
        let rec: FindingRecord = serde_json::from_str(line).unwrap();
        assert_eq!(rec.profile, crate::report::FindingProfile::Summary);
        assert!(rec.report.is_none());
    }

    #[test]
    fn full_record_round_trips_with_its_report() {
        let fixture = include_str!("../../tests/fixtures/finding_report/valid_full.json");
        let report: crate::report::FindingReport = serde_json::from_str(fixture).unwrap();
        let rec = FindingRecord {
            id: "fnd_2".into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: report.title.clone(),
            severity: crate::catalog::types::Severity::Critical,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: report.root_cause.clone(),
                references: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
            profile: crate::report::FindingProfile::Full,
            report: Some(report),
        };
        let back: FindingRecord =
            serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        assert_eq!(back, rec);
    }
```

Create the fixture `crates/rupu-coverage/tests/fixtures/finding_report/valid_full.json` (a generic, fictional finding, which later tasks also use):

```json
{
  "title": "Notes API returns another user's note by id",
  "ownership": {
    "owner": "Unknown",
    "product": "Notebin (sample app)",
    "affected_component": "GET /api/notes/{id} handler",
    "source_repository": "example/notebin"
  },
  "tickets": "None Provided",
  "rating": {
    "impact": "High",
    "likelihood": "High",
    "risk_rating": "Critical",
    "risk_factor": "High",
    "cvss_v3": "Unknown"
  },
  "category": "Authorization Bypass Through User-Controlled Key",
  "attack_vector": "Authenticated HTTP request with another user's note id",
  "cwe": ["CWE-639", "CWE-862"],
  "description": "The `get_note` handler loads a note by the `id` path parameter and returns it without checking that the note belongs to the signed-in user.",
  "impact": "Any signed-in user can read every other user's notes by iterating ids.",
  "location": {
    "input": "`GET /api/notes/{id}` path parameter `id`.",
    "output": "The JSON body of the note, including `title` and `body`."
  },
  "root_cause": "`NoteStore::find_by_id` is called with only the note id; the owner id from the session is never part of the lookup.",
  "call_chain": [
    { "label": "router: GET /api/notes/{id}", "file": "src/app.rs", "lines": [12, 30], "gate": "session cookie", "passes_because": "any signed-in user has one", "role": "source" },
    { "label": "get_note()", "file": "src/routes/notes.rs", "lines": [40, 58], "role": "hop" },
    { "label": "NoteStore::find_by_id()", "file": "src/store/notes.rs", "lines": [88, 97], "role": "sink" }
  ],
  "evidence": [
    { "claim": "The handler looks the note up by id alone.", "file": "src/routes/notes.rs", "lines": [40, 58], "excerpt": "let note = store.find_by_id(id).await?;\nOk(Json(note))", "lang": "rust" }
  ],
  "remediation": "Scope the lookup to the session user and return 404 when the note belongs to someone else.",
  "recommended_patch": { "diff": "--- a/src/routes/notes.rs\n+++ b/src/routes/notes.rs\n@@\n-    let note = store.find_by_id(id).await?;\n+    let note = store.find_by_id_for_owner(id, session.user_id).await?;\n" },
  "ci_cd_detection": { "stage": "pre-merge", "body": "Integration test: user B requests user A's note and must get 404.", "command": "cargo test --test notes_access", "expect": "test fails when the response is 200" },
  "regression_test": { "body": "`notes_access::other_users_note_is_404` creates two users and one note.", "command": "cargo test --test notes_access other_users_note_is_404", "expect_vulnerable": "fails: got 200", "expect_patched": "passes: got 404" },
  "replication_steps": [
    "Sign up as user A and create a note; record its id.",
    "Sign up as user B.",
    "As user B, request `GET /api/notes/{id}` with user A's note id."
  ],
  "cross_references": "None",
  "references": "CWE-639; CWE-862; OWASP A01:2021 Broken Access Control"
}
```

- [ ] **Step 2: Run the tests and check that they fail**

Run: `cargo test -p rupu-coverage legacy_finding_line full_record_round_trips resolve_prefers serializes_lowercase`
Expected: compile errors. `crate::report` does not exist, and `FindingRecord` has no `profile`/`report`.

- [ ] **Step 3: Write the types**

Create `crates/rupu-coverage/src/report/types.rs`:

```rust
//! Typed finding report. Field names follow the reporting standard the spec
//! was drawn from; where that standard used prose for something with obvious
//! structure (call chain, evidence, patch, tests) this uses the structure.
//!
//! Every struct is `deny_unknown_fields`: a misspelled field is a loud parse
//! error back to the agent, never silently dropped content.

use crate::catalog::types::Severity;
use serde::{Deserialize, Serialize};

/// Prefix of the one sentinel allowed on mandatory-but-sometimes-impossible
/// sections. Must be followed by a non-empty justification.
pub const NOT_PROVIDED_PREFIX: &str = "Not Provided — ";

/// A field that is either real content or a sentinel string. Which sentinel
/// strings are acceptable is per field and enforced by
/// [`crate::report::validate_report`], not by the type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OrSentinel<T> {
    Value(T),
    Sentinel(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl From<RiskLevel> for Severity {
    fn from(r: RiskLevel) -> Self {
        match r {
            RiskLevel::Low => Severity::Low,
            RiskLevel::Medium => Severity::Medium,
            RiskLevel::High => Severity::High,
            RiskLevel::Critical => Severity::Critical,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Likelihood {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ownership {
    pub owner: String,
    pub product: String,
    pub affected_component: String,
    pub source_repository: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    #[serde(rename = "type")]
    pub kind: String,
    pub identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    pub impact: RiskLevel,
    pub likelihood: Likelihood,
    pub risk_rating: RiskLevel,
    pub risk_factor: RiskLevel,
    /// Base score, optionally with the vector string, or `Unknown`.
    pub cvss_v3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportLocation {
    pub input: String,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HopRole {
    Source,
    Hop,
    Sink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainHop {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_va: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passes_because: Option<String>,
    pub role: HopRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceClaim {
    pub claim: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<[u32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_va: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// SHA-256 of `file` at write time. Set by rupu, not the agent: it is
    /// what lets a viewer flag a claim whose code has since changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Path of an entry in `artifacts` this claim is proven by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    /// Unified diff, or a `binary@VA` pseudo-diff for binary-only targets.
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiDetection {
    /// Where in the pipeline it runs (pre-merge, nightly, release gate).
    pub stage: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub expect: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionTest {
    pub body: String,
    pub command: String,
    pub expect_vulnerable: String,
    pub expect_patched: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Relation {
    Duplicate,
    Sibling,
    Prerequisite,
    Supersedes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossRef {
    pub finding_id: String,
    pub relation: Relation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Text,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactStorage {
    /// Copied into the content-addressed store; available forever.
    Copied,
    /// Over the size cap (or on a remote host): recorded by path + hash only.
    External,
}

/// An artifact. The agent supplies only `path`; rupu fills in the rest at
/// write time and overwrites anything the agent put there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArtifactKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<ArtifactStorage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerificationStatus {
    Unverified,
    Confirmed,
    Disputed,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    pub status: VerificationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingReport {
    pub title: String,
    pub ownership: Ownership,
    /// `None Provided` / `Unknown`, or the known tickets.
    pub tickets: OrSentinel<Vec<Ticket>>,
    pub rating: Rating,
    pub category: String,
    pub attack_vector: String,
    #[serde(default)]
    pub cwe: Vec<String>,
    pub description: String,
    pub impact: String,
    pub location: ReportLocation,
    pub root_cause: String,
    pub call_chain: OrSentinel<Vec<ChainHop>>,
    pub evidence: Vec<EvidenceClaim>,
    pub remediation: String,
    pub recommended_patch: OrSentinel<Patch>,
    pub ci_cd_detection: OrSentinel<CiDetection>,
    pub regression_test: OrSentinel<RegressionTest>,
    pub replication_steps: Vec<String>,
    /// `None`, or related findings by `fnd_` id.
    pub cross_references: OrSentinel<Vec<CrossRef>>,
    pub references: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
}
```

Create `crates/rupu-coverage/src/report/mod.rs`:

```rust
//! Structured finding reports.
//!
//! A finding under the `full` profile carries a [`FindingReport`]: the whole
//! assessment write-up as typed data. Every presentation (UI sections,
//! Markdown/HTML/PDF exports) is generated from it, so the agent writes it
//! exactly once. Design: docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md

pub mod profile;
pub mod types;

pub use profile::FindingProfile;
pub use types::*;
```

In `crates/rupu-coverage/src/lib.rs`, add `pub mod report;` after `pub mod rerun;`, and add this re-export line after the `pub use rerun::...` line:

```rust
pub use report::{FindingProfile, FindingReport};
```

In `crates/rupu-coverage/src/ledger/events.rs`, add two fields at the end of `FindingRecord` (after `declared_at`):

```rust
    /// Contract this finding was recorded under. Absent on ledger lines that
    /// predate profiles, which were all summary records.
    #[serde(default = "crate::report::FindingProfile::legacy")]
    pub profile: crate::report::FindingProfile,
    /// The full report. `Some` exactly when `profile` is `full`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<crate::report::FindingReport>,
```

Then fix every `FindingRecord { .. }` literal that no longer compiles:

Run: `cargo build -p rupu-coverage --tests 2>&1 | grep -B2 "missing fields \`profile\`"`

Add `profile: crate::report::FindingProfile::Summary, report: None,` to each one (or `rupu_coverage::FindingProfile::Summary` outside the crate). Then:

Run: `cargo build --workspace --tests 2>&1 | grep -A3 "missing fields \`profile\`"`

Fix those the same way (the CP's findings tests and `rupu-cli` coverage tests construct records).

- [ ] **Step 4: Run the tests and check that they pass**

Run: `cargo test -p rupu-coverage legacy_finding_line full_record_round_trips resolve_prefers serializes_lowercase`
Expected: PASS (4 tests).

Run: `cargo build --workspace --tests`
Expected: builds.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage crates/rupu-cp crates/rupu-cli
git commit -m "feat(coverage): typed FindingReport + findings profile on FindingRecord

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The report validator

**Files:**
- Create: `crates/rupu-coverage/src/report/validate.rs`
- Modify: `crates/rupu-coverage/src/report/mod.rs`

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  - `pub struct FieldError { pub path: String, pub message: String }`
  - `pub struct ReportValidationError(pub Vec<FieldError>)` (implements `std::error::Error`; `Display` lists every problem)
  - `pub struct ValidateCtx<'a> { pub known_finding_ids: &'a [String], pub max_bytes: usize }`
  - `pub fn validate_report(r: &FindingReport, ctx: &ValidateCtx) -> Result<(), ReportValidationError>`
  - `pub(crate) fn rel_path_problem(p: &str) -> Option<&'static str>` (reused by the artifact store)

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-coverage/src/report/validate.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::*;

    fn valid() -> FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn ctx() -> ValidateCtx<'static> {
        ValidateCtx { known_finding_ids: &[], max_bytes: 262_144 }
    }

    fn problems(r: &FindingReport) -> Vec<String> {
        match validate_report(r, &ctx()) {
            Ok(()) => vec![],
            Err(e) => e.0.into_iter().map(|f| f.path).collect(),
        }
    }

    #[test]
    fn fixture_is_valid() {
        assert_eq!(problems(&valid()), Vec::<String>::new());
    }

    #[test]
    fn empty_required_strings_are_rejected_and_all_reported_at_once() {
        let mut r = valid();
        r.root_cause = "  ".into();
        r.description = String::new();
        r.ownership.owner = String::new();
        let p = problems(&r);
        assert!(p.contains(&"report.root_cause".to_string()), "{p:?}");
        assert!(p.contains(&"report.description".to_string()), "{p:?}");
        assert!(p.contains(&"report.ownership.owner".to_string()), "{p:?}");
    }

    #[test]
    fn unknown_is_not_accepted_for_patch_ci_or_regression() {
        let mut r = valid();
        r.recommended_patch = OrSentinel::Sentinel("Unknown".into());
        r.ci_cd_detection = OrSentinel::Sentinel("Unknown".into());
        r.regression_test = OrSentinel::Sentinel("Unknown".into());
        let p = problems(&r);
        for f in ["report.recommended_patch", "report.ci_cd_detection", "report.regression_test"] {
            assert!(p.contains(&f.to_string()), "{f} missing from {p:?}");
        }
    }

    #[test]
    fn not_provided_needs_a_justification() {
        let mut r = valid();
        r.regression_test = OrSentinel::Sentinel("Not Provided — ".into());
        assert_eq!(problems(&r), vec!["report.regression_test".to_string()]);
        r.regression_test =
            OrSentinel::Sentinel("Not Provided — requires hardware we do not have".into());
        assert_eq!(problems(&r), Vec::<String>::new());
    }

    #[test]
    fn ticket_sentinels_are_exact() {
        let mut r = valid();
        r.tickets = OrSentinel::Sentinel("none provided".into());
        assert_eq!(problems(&r), vec!["report.tickets".to_string()]);
        r.tickets = OrSentinel::Sentinel("Unknown".into());
        assert_eq!(problems(&r), Vec::<String>::new());
        r.tickets = OrSentinel::Value(vec![]);
        assert_eq!(problems(&r), vec!["report.tickets".to_string()]);
    }

    #[test]
    fn cross_references_accept_only_none_or_known_ids() {
        let mut r = valid();
        r.cross_references = OrSentinel::Sentinel("none".into());
        assert_eq!(problems(&r), vec!["report.cross_references".to_string()]);

        r.cross_references = OrSentinel::Value(vec![CrossRef {
            finding_id: "fnd_UNKNOWN".into(),
            relation: Relation::Sibling,
            note: None,
        }]);
        assert_eq!(problems(&r), vec!["report.cross_references[0].finding_id".to_string()]);

        let known = vec!["fnd_UNKNOWN".to_string()];
        let ok = validate_report(&r, &ValidateCtx { known_finding_ids: &known, max_bytes: 262_144 });
        assert!(ok.is_ok());
    }

    #[test]
    fn cwe_entries_must_be_cwe_n() {
        let mut r = valid();
        r.cwe = vec!["306".into()];
        assert_eq!(problems(&r), vec!["report.cwe[0]".to_string()]);
    }

    #[test]
    fn paths_must_be_workspace_relative_and_lines_ordered() {
        let mut r = valid();
        r.evidence[0].file = Some("/etc/passwd".into());
        r.evidence[0].lines = Some([10, 2]);
        if let OrSentinel::Value(hops) = &mut r.call_chain {
            hops[0].file = Some("../outside.c".into());
        }
        let p = problems(&r);
        assert!(p.contains(&"report.evidence[0].file".to_string()), "{p:?}");
        assert!(p.contains(&"report.evidence[0].lines".to_string()), "{p:?}");
        assert!(p.contains(&"report.call_chain[0].file".to_string()), "{p:?}");
    }

    #[test]
    fn lists_that_must_be_non_empty() {
        let mut r = valid();
        r.evidence.clear();
        r.replication_steps.clear();
        r.call_chain = OrSentinel::Value(vec![]);
        let p = problems(&r);
        for f in ["report.evidence", "report.replication_steps", "report.call_chain"] {
            assert!(p.contains(&f.to_string()), "{f} missing from {p:?}");
        }
    }

    #[test]
    fn size_budget_is_enforced() {
        let mut r = valid();
        r.description = "x".repeat(300_000);
        assert_eq!(problems(&r), vec!["report".to_string()]);
    }

    #[test]
    fn display_lists_every_problem() {
        let mut r = valid();
        r.root_cause = String::new();
        r.remediation = String::new();
        let msg = validate_report(&r, &ctx()).unwrap_err().to_string();
        assert!(msg.contains("2 problem"), "{msg}");
        assert!(msg.contains("report.root_cause"), "{msg}");
        assert!(msg.contains("report.remediation"), "{msg}");
    }
}
```

Add `pub mod validate;` and `pub use validate::{validate_report, FieldError, ReportValidationError, ValidateCtx};` to `report/mod.rs`.

- [ ] **Step 2: Run the tests and check that they fail**

Run: `cargo test -p rupu-coverage report::validate`
Expected: compile error, because `validate_report` / `ValidateCtx` are undefined.

- [ ] **Step 3: Implement the validator**

Put this above the test module in `validate.rs`:

```rust
//! Strict validation of a [`FindingReport`] under the `full` profile.
//!
//! Collects EVERY problem before returning, so the agent can fix them all in
//! one retry instead of discovering them one call at a time.

use crate::report::types::*;
use std::fmt;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    /// JSON path, e.g. `report.evidence[0].file`.
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportValidationError(pub Vec<FieldError>);

impl fmt::Display for ReportValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "finding report has {} problem(s); fix all of them and call again:",
            self.0.len()
        )?;
        for e in &self.0 {
            writeln!(f, "- {}: {}", e.path, e.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ReportValidationError {}

pub struct ValidateCtx<'a> {
    /// `fnd_` ids already in this project's ledger; cross-references must
    /// point at one of them.
    pub known_finding_ids: &'a [String],
    /// Serialized-size budget for the whole report.
    pub max_bytes: usize,
}

/// Why `p` is not an acceptable workspace-relative path, if it isn't.
pub(crate) fn rel_path_problem(p: &str) -> Option<&'static str> {
    if p.trim().is_empty() {
        return Some("must not be empty");
    }
    let path = Path::new(p);
    if path.is_absolute() || p.starts_with('/') || p.starts_with('\\') {
        return Some("must be workspace-relative, not absolute");
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Some("must not contain `..`");
    }
    None
}

fn not_provided_ok(s: &str) -> bool {
    s.strip_prefix(NOT_PROVIDED_PREFIX)
        .is_some_and(|rest| !rest.trim().is_empty())
}

const NOT_PROVIDED_HINT: &str =
    "provide the content, or exactly `Not Provided — <one-line justification>`; `Unknown` is not accepted here";

struct Check {
    errors: Vec<FieldError>,
}

impl Check {
    fn err(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(FieldError { path: path.into(), message: message.into() });
    }

    fn text(&mut self, path: &str, v: &str) {
        if v.trim().is_empty() {
            self.err(path, "must not be empty (use the field's sentinel if it is genuinely unknown)");
        }
    }

    fn opt_text(&mut self, path: &str, v: &Option<String>) {
        if let Some(s) = v {
            self.text(path, s);
        }
    }

    fn rel_path(&mut self, path: &str, v: &Option<String>) {
        if let Some(p) = v {
            if let Some(why) = rel_path_problem(p) {
                self.err(path, why);
            }
        }
    }

    fn lines(&mut self, path: &str, v: &Option<[u32; 2]>) {
        if let Some([a, b]) = v {
            if a > b || *a == 0 {
                self.err(path, "must be [start, end] with 1 <= start <= end");
            }
        }
    }
}

pub fn validate_report(r: &FindingReport, ctx: &ValidateCtx) -> Result<(), ReportValidationError> {
    let mut c = Check { errors: Vec::new() };

    c.text("report.title", &r.title);
    c.text("report.ownership.owner", &r.ownership.owner);
    c.text("report.ownership.product", &r.ownership.product);
    c.text("report.ownership.affected_component", &r.ownership.affected_component);
    c.text("report.ownership.source_repository", &r.ownership.source_repository);

    match &r.tickets {
        OrSentinel::Sentinel(s) if s == "None Provided" || s == "Unknown" => {}
        OrSentinel::Sentinel(_) => {
            c.err("report.tickets", "must be `None Provided`, `Unknown`, or a list of tickets")
        }
        OrSentinel::Value(list) if list.is_empty() => {
            c.err("report.tickets", "an empty list is not allowed; use `None Provided`")
        }
        OrSentinel::Value(list) => {
            for (i, t) in list.iter().enumerate() {
                c.text(&format!("report.tickets[{i}].type"), &t.kind);
                c.text(&format!("report.tickets[{i}].identifier"), &t.identifier);
            }
        }
    }

    c.text("report.rating.cvss_v3", &r.rating.cvss_v3);
    c.text("report.category", &r.category);
    c.text("report.attack_vector", &r.attack_vector);
    for (i, cwe) in r.cwe.iter().enumerate() {
        let ok = cwe
            .strip_prefix("CWE-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            c.err(format!("report.cwe[{i}]"), "must look like `CWE-306`");
        }
    }
    c.text("report.description", &r.description);
    c.text("report.impact", &r.impact);
    c.text("report.location.input", &r.location.input);
    c.text("report.location.output", &r.location.output);
    c.text("report.root_cause", &r.root_cause);

    match &r.call_chain {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.call_chain", NOT_PROVIDED_HINT),
        OrSentinel::Value(hops) if hops.is_empty() => {
            c.err("report.call_chain", "must list at least one hop")
        }
        OrSentinel::Value(hops) => {
            for (i, h) in hops.iter().enumerate() {
                c.text(&format!("report.call_chain[{i}].label"), &h.label);
                c.rel_path(&format!("report.call_chain[{i}].file"), &h.file);
                c.lines(&format!("report.call_chain[{i}].lines"), &h.lines);
            }
        }
    }

    if r.evidence.is_empty() {
        c.err("report.evidence", "must contain at least one claim");
    }
    for (i, e) in r.evidence.iter().enumerate() {
        c.text(&format!("report.evidence[{i}].claim"), &e.claim);
        c.rel_path(&format!("report.evidence[{i}].file"), &e.file);
        c.lines(&format!("report.evidence[{i}].lines"), &e.lines);
    }

    c.text("report.remediation", &r.remediation);

    match &r.recommended_patch {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.recommended_patch", NOT_PROVIDED_HINT),
        OrSentinel::Value(p) => c.text("report.recommended_patch.diff", &p.diff),
    }
    match &r.ci_cd_detection {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.ci_cd_detection", NOT_PROVIDED_HINT),
        OrSentinel::Value(d) => {
            c.text("report.ci_cd_detection.stage", &d.stage);
            c.text("report.ci_cd_detection.body", &d.body);
            c.opt_text("report.ci_cd_detection.command", &d.command);
            c.text("report.ci_cd_detection.expect", &d.expect);
        }
    }
    match &r.regression_test {
        OrSentinel::Sentinel(s) if not_provided_ok(s) => {}
        OrSentinel::Sentinel(_) => c.err("report.regression_test", NOT_PROVIDED_HINT),
        OrSentinel::Value(t) => {
            c.text("report.regression_test.body", &t.body);
            c.text("report.regression_test.command", &t.command);
            c.text("report.regression_test.expect_vulnerable", &t.expect_vulnerable);
            c.text("report.regression_test.expect_patched", &t.expect_patched);
        }
    }

    if r.replication_steps.is_empty() {
        c.err("report.replication_steps", "must contain at least one step");
    }
    for (i, s) in r.replication_steps.iter().enumerate() {
        c.text(&format!("report.replication_steps[{i}]"), s);
    }

    match &r.cross_references {
        OrSentinel::Sentinel(s) if s == "None" => {}
        OrSentinel::Sentinel(_) => {
            c.err("report.cross_references", "must be `None` or a list of related findings")
        }
        OrSentinel::Value(list) if list.is_empty() => {
            c.err("report.cross_references", "an empty list is not allowed; use `None`")
        }
        OrSentinel::Value(list) => {
            for (i, x) in list.iter().enumerate() {
                if !ctx.known_finding_ids.iter().any(|k| k == &x.finding_id) {
                    c.err(
                        format!("report.cross_references[{i}].finding_id"),
                        format!(
                            "`{}` is not a finding in this project; use the id a previous report_finding call returned",
                            x.finding_id
                        ),
                    );
                }
            }
        }
    }

    c.text("report.references", &r.references);

    for (i, a) in r.artifacts.iter().enumerate() {
        if let Some(why) = rel_path_problem(&a.path) {
            c.err(format!("report.artifacts[{i}].path"), why);
        }
    }

    // Size last, and only when nothing else is wrong: a flood of field
    // errors plus "too big" buries the actionable ones.
    if c.errors.is_empty() {
        let size = serde_json::to_vec(r).map(|v| v.len()).unwrap_or(usize::MAX);
        if size > ctx.max_bytes {
            c.err(
                "report",
                format!(
                    "serialized report is {size} bytes, over the {} byte budget; trim excerpts and move long output into `artifacts`",
                    ctx.max_bytes
                ),
            );
        }
    }

    if c.errors.is_empty() {
        Ok(())
    } else {
        Err(ReportValidationError(c.errors))
    }
}
```

- [ ] **Step 4: Run the tests and check that they pass**

Run: `cargo test -p rupu-coverage report::validate`
Expected: PASS (11 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/report
git commit -m "feat(coverage): finding report validator with per-field sentinels

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Embedded JSON Schema, provider-safe advertised schema, lockstep test

**Files:**
- Create: `crates/rupu-coverage/schema/finding_report.schema.json`, `crates/rupu-coverage/src/report/schema.rs`, `crates/rupu-coverage/tests/report_schema_lockstep.rs`
- Modify: `crates/rupu-coverage/src/report/mod.rs`, `crates/rupu-coverage/Cargo.toml` (dev-dep)

**Interfaces:**
- Produces: `rupu_coverage::report::schema::{FINDING_REPORT_SCHEMA: &str, canonical_schema() -> serde_json::Value, advertised_schema() -> serde_json::Value}`.

- [ ] **Step 1: Write the canonical schema**

Create `crates/rupu-coverage/schema/finding_report.schema.json`. It is draft-07, so the jsonschema 0.18 validator is fully supported, and it has no `$ref` because some providers don't resolve refs in tool schemas:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "$id": "https://rupu.dev/schema/finding_report.schema.json",
  "title": "rupu finding report",
  "description": "A complete security finding report, recorded once by the agent. Every presentation (UI, Markdown, HTML, PDF) is generated from it. When information genuinely cannot be determined use the field's sentinel instead of omitting it: 'Unknown', 'Not Applicable', 'None Provided', 'None', or 'Not Provided — <justification>' as each field describes.",
  "type": "object",
  "additionalProperties": false,
  "required": ["title", "ownership", "tickets", "rating", "category", "attack_vector", "cwe", "description", "impact", "location", "root_cause", "call_chain", "evidence", "remediation", "recommended_patch", "ci_cd_detection", "regression_test", "replication_steps", "cross_references", "references"],
  "properties": {
    "title": { "type": "string", "minLength": 1, "description": "Short title specific to the vulnerable feature and target." },
    "ownership": {
      "type": "object",
      "additionalProperties": false,
      "required": ["owner", "product", "affected_component", "source_repository"],
      "properties": {
        "owner": { "type": "string", "minLength": 1, "description": "Accountable team/service owner, or 'Unknown'. Never invent one." },
        "product": { "type": "string", "minLength": 1, "description": "Named application/platform/service, or 'Unknown'." },
        "affected_component": { "type": "string", "minLength": 1, "description": "Exact feature/endpoint/module, or 'Unknown'." },
        "source_repository": { "type": "string", "minLength": 1, "description": "Canonical repository (optionally with a path), 'Not Applicable' when no source-controlled code is involved, or 'Unknown'." }
      }
    },
    "tickets": {
      "description": "'None Provided' when no existing ticket is mentioned, 'Unknown' when one exists but its id is not given, or the tickets present in the evidence. Never invent a ticket.",
      "oneOf": [
        { "type": "string", "enum": ["None Provided", "Unknown"] },
        {
          "type": "array",
          "minItems": 1,
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["type", "identifier"],
            "properties": {
              "type": { "type": "string", "minLength": 1, "description": "Jira / Bug / Remediation Tracker / Vulnerability Management / Other." },
              "identifier": { "type": "string", "minLength": 1 },
              "url": { "type": "string" },
              "notes": { "type": "string" }
            }
          }
        }
      ]
    },
    "rating": {
      "type": "object",
      "additionalProperties": false,
      "required": ["impact", "likelihood", "risk_rating", "risk_factor", "cvss_v3"],
      "properties": {
        "impact": { "type": "string", "enum": ["Low", "Medium", "High", "Critical"] },
        "likelihood": { "type": "string", "enum": ["Low", "Medium", "High"] },
        "risk_rating": { "type": "string", "enum": ["Low", "Medium", "High", "Critical"], "description": "Drives the finding's severity." },
        "risk_factor": { "type": "string", "enum": ["Low", "Medium", "High", "Critical"] },
        "cvss_v3": { "type": "string", "minLength": 1, "description": "Base score, optionally with the vector string, or 'Unknown'. Never invent a score." }
      }
    },
    "category": { "type": "string", "minLength": 1 },
    "attack_vector": { "type": "string", "minLength": 1, "description": "How the surface is reached, including authentication or user interaction required, or 'Unknown'." },
    "cwe": { "type": "array", "items": { "type": "string", "pattern": "^CWE-[0-9]+$" }, "description": "CWE ids, e.g. ['CWE-306']. May be empty." },
    "description": { "type": "string", "minLength": 1, "description": "Markdown. What the issue is, where it occurs, why it is a weakness." },
    "impact": { "type": "string", "minLength": 1, "description": "Markdown. What an attacker could achieve and why it matters." },
    "location": {
      "type": "object",
      "additionalProperties": false,
      "required": ["input", "output"],
      "properties": {
        "input": { "type": "string", "minLength": 1, "description": "Markdown. Input surface, endpoint, parameter, or feature." },
        "output": { "type": "string", "minLength": 1, "description": "Markdown. Affected area, response, or downstream system." }
      }
    },
    "root_cause": { "type": "string", "minLength": 1, "description": "Markdown, one or two sentences. The single underlying defect, naming the exact variable, check, or assumption that is wrong — not the symptom." },
    "call_chain": {
      "description": "Ordered path from the externally reachable entry point to the sink, or 'Not Provided — <justification>'.",
      "oneOf": [
        {
          "type": "array",
          "minItems": 1,
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["label", "role"],
            "properties": {
              "label": { "type": "string", "minLength": 1, "description": "Function or surface name." },
              "file": { "type": "string", "description": "Workspace-relative path." },
              "lines": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 },
              "binary_va": { "type": "string", "description": "binary@VA for binary-only targets." },
              "gate": { "type": "string", "description": "Precondition crossed at this hop." },
              "passes_because": { "type": "string", "description": "Why the gate passes." },
              "role": { "type": "string", "enum": ["source", "hop", "sink"] }
            }
          }
        },
        { "type": "string", "pattern": "^Not Provided — \\S" }
      ]
    },
    "evidence": {
      "type": "array",
      "minItems": 1,
      "description": "Minimal annotated proof, one claim per entry.",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["claim"],
        "properties": {
          "claim": { "type": "string", "minLength": 1 },
          "file": { "type": "string", "description": "Workspace-relative path." },
          "lines": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 },
          "binary_va": { "type": "string" },
          "excerpt": { "type": "string", "description": "Only the lines that matter." },
          "lang": { "type": "string", "description": "Language for highlighting, e.g. 'c', 'sh'." },
          "sha256": { "type": "string", "description": "Set by rupu; omit." },
          "artifact": { "type": "string", "description": "Path of an entry in artifacts that proves this claim." }
        }
      }
    },
    "remediation": { "type": "string", "minLength": 1, "description": "Markdown. Specific corrective actions." },
    "recommended_patch": {
      "description": "Minimal unified diff against the source repository (or a binary@VA pseudo-diff), or 'Not Provided — <justification>'.",
      "oneOf": [
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["diff"],
          "properties": {
            "diff": { "type": "string", "minLength": 1 },
            "notes": { "type": "string", "description": "Markdown. Alternatives or the equivalent source-level change." }
          }
        },
        { "type": "string", "pattern": "^Not Provided — \\S" }
      ]
    },
    "ci_cd_detection": {
      "description": "A pipeline check that fails the build when this class of issue is present, or 'Not Provided — <justification>'.",
      "oneOf": [
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["stage", "body", "expect"],
          "properties": {
            "stage": { "type": "string", "minLength": 1, "description": "pre-merge / nightly / release gate." },
            "body": { "type": "string", "minLength": 1, "description": "Markdown. The rule or test, verbatim." },
            "command": { "type": "string", "minLength": 1 },
            "expect": { "type": "string", "minLength": 1, "description": "The failing signal." }
          }
        },
        { "type": "string", "pattern": "^Not Provided — \\S" }
      ]
    },
    "regression_test": {
      "description": "A deterministic test that fails on the vulnerable build and passes on the patched one, or 'Not Provided — <justification>'.",
      "oneOf": [
        {
          "type": "object",
          "additionalProperties": false,
          "required": ["body", "command", "expect_vulnerable", "expect_patched"],
          "properties": {
            "body": { "type": "string", "minLength": 1 },
            "command": { "type": "string", "minLength": 1 },
            "expect_vulnerable": { "type": "string", "minLength": 1 },
            "expect_patched": { "type": "string", "minLength": 1 }
          }
        },
        { "type": "string", "pattern": "^Not Provided — \\S" }
      ]
    },
    "replication_steps": { "type": "array", "minItems": 1, "items": { "type": "string", "minLength": 1 }, "description": "Sequential, reproducible steps with exact payloads and commands." },
    "cross_references": {
      "description": "'None', or related findings by the fnd_ id a previous report_finding call returned.",
      "oneOf": [
        { "type": "string", "enum": ["None"] },
        {
          "type": "array",
          "minItems": 1,
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["finding_id", "relation"],
            "properties": {
              "finding_id": { "type": "string", "minLength": 1 },
              "relation": { "type": "string", "enum": ["duplicate", "sibling", "prerequisite", "supersedes"] },
              "note": { "type": "string" }
            }
          }
        }
      ]
    },
    "references": { "type": "string", "minLength": 1, "description": "Markdown. CWE, OWASP category, control references, related tickets." },
    "artifacts": {
      "type": "array",
      "description": "Proof-of-concept files (scripts, outputs, harnesses) as workspace-relative paths; rupu stores them with the finding. Supply only 'path'.",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["path"],
        "properties": {
          "path": { "type": "string", "minLength": 1 },
          "sha256": { "type": "string" },
          "size": { "type": "integer" },
          "kind": { "type": "string", "enum": ["text", "binary"] },
          "stored": { "type": "string", "enum": ["copied", "external"] },
          "host": { "type": "string" }
        }
      }
    },
    "verification": {
      "type": "object",
      "additionalProperties": false,
      "required": ["status"],
      "properties": {
        "status": { "type": "string", "enum": ["unverified", "confirmed", "disputed", "inconclusive"] },
        "by_run": { "type": "string" },
        "notes": { "type": "string" }
      }
    }
  }
}
```

- [ ] **Step 2: Write the failing tests**

Create `crates/rupu-coverage/src/report/schema.rs` with only its tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn walk_keys(v: &serde_json::Value, found: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, child) in m {
                    found.push(k.clone());
                    walk_keys(child, found);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|c| walk_keys(c, found)),
            _ => {}
        }
    }

    #[test]
    fn canonical_schema_parses() {
        let s = canonical_schema();
        assert_eq!(s["type"], "object");
        assert!(s["required"].as_array().unwrap().len() >= 20);
    }

    #[test]
    fn advertised_schema_drops_provider_unsafe_keywords() {
        let mut keys = Vec::new();
        walk_keys(&advertised_schema(), &mut keys);
        for banned in ["oneOf", "pattern", "minLength", "additionalProperties", "$schema", "$id"] {
            assert!(!keys.iter().any(|k| k == banned), "{banned} survived");
        }
        assert!(keys.iter().any(|k| k == "anyOf"), "oneOf must become anyOf");
    }

    #[test]
    fn advertised_schema_keeps_property_names() {
        // `title` is both a JSON Schema keyword and a report property; the
        // property must survive.
        let s = advertised_schema();
        assert!(s["properties"]["title"].is_object());
        assert!(s["properties"]["regression_test"]["anyOf"].is_array());
    }
}
```

Create `crates/rupu-coverage/tests/report_schema_lockstep.rs`:

```rust
//! The embedded JSON Schema and the Rust validator must agree.
//!
//! Tool definitions are generated from the schema; stored records are
//! checked by serde + `validate_report`. If they drift, an agent is told one
//! contract and held to another. Every case here must be accepted by both or
//! rejected by both. Checks only the validator can express (workspace-relative
//! paths, line order, cross-reference existence, size budget) are unit-tested
//! in `report::validate`.

use rupu_coverage::report::{schema::canonical_schema, validate_report, FindingReport, ValidateCtx};
use serde_json::{json, Value};

fn valid() -> Value {
    serde_json::from_str(include_str!("fixtures/finding_report/valid_full.json")).unwrap()
}

fn rust_accepts(v: &Value) -> bool {
    match serde_json::from_value::<FindingReport>(v.clone()) {
        Ok(r) => validate_report(&r, &ValidateCtx { known_finding_ids: &[], max_bytes: 262_144 }).is_ok(),
        Err(_) => false,
    }
}

fn schema_accepts(v: &Value) -> bool {
    let schema = canonical_schema();
    let compiled = jsonschema::JSONSchema::compile(&schema).expect("schema compiles");
    compiled.is_valid(v)
}

fn mutated(f: impl FnOnce(&mut Value)) -> Value {
    let mut v = valid();
    f(&mut v);
    v
}

#[test]
fn valid_fixture_accepted_by_both() {
    let v = valid();
    assert!(rust_accepts(&v), "rust rejected the valid fixture");
    assert!(schema_accepts(&v), "schema rejected the valid fixture");
}

#[test]
fn not_provided_with_justification_accepted_by_both() {
    let v = mutated(|v| v["regression_test"] = json!("Not Provided — needs the physical board"));
    assert!(rust_accepts(&v));
    assert!(schema_accepts(&v));
}

#[test]
fn invalid_cases_rejected_by_both() {
    let cases: Vec<(&str, Value)> = vec![
        ("missing root_cause", mutated(|v| { v.as_object_mut().unwrap().remove("root_cause"); })),
        ("Unknown regression test", mutated(|v| v["regression_test"] = json!("Unknown"))),
        ("empty justification", mutated(|v| v["regression_test"] = json!("Not Provided — "))),
        ("bad ticket sentinel", mutated(|v| v["tickets"] = json!("TBD"))),
        ("likelihood Critical", mutated(|v| v["rating"]["likelihood"] = json!("Critical"))),
        ("no replication steps", mutated(|v| v["replication_steps"] = json!([]))),
        ("bare cwe number", mutated(|v| v["cwe"] = json!(["306"]))),
        ("empty description", mutated(|v| v["description"] = json!(""))),
        ("lowercase none", mutated(|v| v["cross_references"] = json!("none"))),
        ("no evidence", mutated(|v| v["evidence"] = json!([]))),
        ("empty call chain", mutated(|v| v["call_chain"] = json!([]))),
        ("unknown field", mutated(|v| v["extra"] = json!(1))),
        ("unknown nested field", mutated(|v| v["rating"]["severity"] = json!("High"))),
    ];
    for (name, v) in cases {
        assert!(!rust_accepts(&v), "rust accepted invalid case: {name}");
        assert!(!schema_accepts(&v), "schema accepted invalid case: {name}");
    }
}
```

Add `jsonschema.workspace = true` under `[dev-dependencies]` in `crates/rupu-coverage/Cargo.toml`. Add `pub mod schema;` to `report/mod.rs`.

- [ ] **Step 3: Run the tests and check that they fail**

Run: `cargo test -p rupu-coverage --test report_schema_lockstep && cargo test -p rupu-coverage report::schema`
Expected: compile error, because `canonical_schema` / `advertised_schema` are undefined.

- [ ] **Step 4: Implement schema.rs**

Put this above the tests in `crates/rupu-coverage/src/report/schema.rs`:

```rust
//! The finding-report JSON Schema, embedded in the binary.
//!
//! `canonical_schema()` is the contract (what `rupu findings schema` prints
//! and what the lockstep test checks the validator against).
//! `advertised_schema()` is the copy put in a tool definition: providers
//! differ in which JSON Schema keywords their tool-calling accepts, so it
//! keeps only the widely supported subset. Dropping keywords there loses
//! nothing — `validate_report` enforces the full contract on every write.

use serde_json::Value;

pub const FINDING_REPORT_SCHEMA: &str = include_str!("../../schema/finding_report.schema.json");

pub fn canonical_schema() -> Value {
    serde_json::from_str(FINDING_REPORT_SCHEMA).expect("embedded finding report schema is valid JSON")
}

pub fn advertised_schema() -> Value {
    let mut v = canonical_schema();
    simplify_schema(&mut v);
    v
}

/// Keywords removed from the advertised copy.
const DROP: &[&str] = &["$schema", "$id", "pattern", "minLength", "additionalProperties"];

/// Walk a schema node. Only schema *keywords* are rewritten; the keys of a
/// `properties` map are property names and are left alone.
fn simplify_schema(node: &mut Value) {
    let Value::Object(map) = node else { return };
    for k in DROP {
        map.remove(*k);
    }
    if let Some(one_of) = map.remove("oneOf") {
        map.insert("anyOf".to_string(), one_of);
    }
    if let Some(Value::Object(props)) = map.get_mut("properties") {
        for child in props.values_mut() {
            simplify_schema(child);
        }
    }
    if let Some(items) = map.get_mut("items") {
        simplify_schema(items);
    }
    if let Some(Value::Array(alts)) = map.get_mut("anyOf") {
        for alt in alts.iter_mut() {
            simplify_schema(alt);
        }
    }
}

```

- [ ] **Step 5: Run the tests and check that they pass**

Run: `cargo test -p rupu-coverage --test report_schema_lockstep && cargo test -p rupu-coverage report::schema`
Expected: PASS (3 + 3 tests). If jsonschema 0.18 treats the em-dash `pattern` differently from the Rust check on some case, the lockstep test fails. Fix the schema, not the test.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): embed finding report schema + provider-safe advertised copy + lockstep test

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Content-addressed artifact store

**Files:**
- Create: `crates/rupu-coverage/src/report/artifacts.rs`
- Modify: `crates/rupu-coverage/src/report/mod.rs`

**Interfaces:**
- Consumes: `ArtifactRef`, `ArtifactKind`, `ArtifactStorage` (Task 1); `rel_path_problem` (Task 2).
- Produces:
  - `pub struct ArtifactStore` with `new(root: impl Into<PathBuf>) -> Self`, `blob_path(&self, sha256: &str) -> PathBuf`, and `ingest(&self, workspace: &Path, requested: &[ArtifactRef], max_bytes: u64) -> Result<Vec<ArtifactRef>, ArtifactError>`.
  - `pub enum ArtifactError { Path{path,reason}, Missing{path}, Escapes{path}, Io{path,source}, NoStore }`.
  - `pub(crate) fn sha256_file(p: &Path) -> std::io::Result<String>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-coverage/src/report/artifacts.rs` with only its tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::types::{ArtifactKind, ArtifactRef, ArtifactStorage};
    use std::fs;

    fn req(p: &str) -> ArtifactRef {
        ArtifactRef { path: p.into(), sha256: String::new(), size: 0, kind: None, stored: None, host: None }
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir) {
        (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap())
    }

    #[test]
    fn copies_small_file_and_fills_metadata() {
        let (ws, store) = setup();
        fs::create_dir_all(ws.path().join("pocs")).unwrap();
        fs::write(ws.path().join("pocs/result.txt"), "rc=0\n").unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s.ingest(ws.path(), &[req("pocs/result.txt")], 1024).unwrap();
        assert_eq!(out.len(), 1);
        let a = &out[0];
        assert_eq!(a.path, "pocs/result.txt");
        assert_eq!(a.size, 5);
        assert_eq!(a.kind, Some(ArtifactKind::Text));
        assert_eq!(a.stored, Some(ArtifactStorage::Copied));
        assert_eq!(a.sha256.len(), 64);
        assert_eq!(fs::read(s.blob_path(&a.sha256)).unwrap(), b"rc=0\n");
    }

    #[test]
    fn identical_content_is_stored_once() {
        let (ws, store) = setup();
        fs::write(ws.path().join("a.bin"), [0u8, 1, 2, 3]).unwrap();
        fs::write(ws.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s.ingest(ws.path(), &[req("a.bin"), req("b.bin")], 1024).unwrap();
        assert_eq!(out[0].sha256, out[1].sha256);
        assert_eq!(out[0].kind, Some(ArtifactKind::Binary));
        let blobs: Vec<_> = walk(store.path()).into_iter().filter(|p| !p.ends_with(".tmp")).collect();
        assert_eq!(blobs.len(), 1, "{blobs:?}");
    }

    #[test]
    fn over_cap_file_is_recorded_external_and_not_copied() {
        let (ws, store) = setup();
        fs::write(ws.path().join("big.img"), vec![7u8; 2048]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s.ingest(ws.path(), &[req("big.img")], 1024).unwrap();
        assert_eq!(out[0].stored, Some(ArtifactStorage::External));
        assert_eq!(out[0].size, 2048);
        assert_eq!(out[0].sha256.len(), 64);
        assert!(!s.blob_path(&out[0].sha256).exists());
    }

    #[test]
    fn directory_expands_to_its_files_in_sorted_order() {
        let (ws, store) = setup();
        fs::create_dir_all(ws.path().join("pocs/sub")).unwrap();
        fs::write(ws.path().join("pocs/b.txt"), "b").unwrap();
        fs::write(ws.path().join("pocs/sub/a.txt"), "a").unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s.ingest(ws.path(), &[req("pocs")], 1024).unwrap();
        let paths: Vec<_> = out.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(paths, vec!["pocs/b.txt", "pocs/sub/a.txt"]);
    }

    #[test]
    fn missing_file_is_an_error() {
        let (ws, store) = setup();
        let err = ArtifactStore::new(store.path()).ingest(ws.path(), &[req("nope.txt")], 1024).unwrap_err();
        assert!(matches!(err, ArtifactError::Missing { .. }), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_workspace_is_refused() {
        let (ws, store) = setup();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), ws.path().join("link")).unwrap();
        let err = ArtifactStore::new(store.path()).ingest(ws.path(), &[req("link")], 1024).unwrap_err();
        assert!(matches!(err, ArtifactError::Escapes { .. }), "{err}");
    }

    #[test]
    fn absolute_path_is_refused() {
        let (ws, store) = setup();
        let err = ArtifactStore::new(store.path()).ingest(ws.path(), &[req("/etc/hosts")], 1024).unwrap_err();
        assert!(matches!(err, ArtifactError::Path { .. }), "{err}");
    }

    fn walk(dir: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.to_string_lossy().into_owned());
            }
        }
        out
    }
}
```

Add `pub mod artifacts;` and `pub use artifacts::{ArtifactError, ArtifactStore};` to `report/mod.rs`.

- [ ] **Step 2: Run the tests and check that they fail**

Run: `cargo test -p rupu-coverage report::artifacts`
Expected: compile error, because `ArtifactStore` is undefined.

- [ ] **Step 3: Implement the store**

Put this above the tests in `artifacts.rs`:

```rust
//! Content-addressed store for finding artifacts (PoC scripts, outputs,
//! harnesses, binaries).
//!
//! Layout: `<root>/<first two hex chars>/<sha256>`. Identical content is
//! stored once across runs and projects. Files over the size cap are hashed
//! but not copied, and recorded as `external`.

use crate::report::types::{ArtifactKind, ArtifactRef, ArtifactStorage};
use crate::report::validate::rel_path_problem;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact `{path}`: {reason}")]
    Path { path: String, reason: &'static str },
    #[error("artifact `{path}` does not exist in the workspace")]
    Missing { path: String },
    #[error("artifact `{path}` resolves outside the workspace")]
    Escapes { path: String },
    #[error("artifact `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the report lists artifacts but no artifact store is configured for this run")]
    NoStore,
}

pub struct ArtifactStore {
    root: PathBuf,
}

const CHUNK: usize = 64 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file's contents, streamed.
pub(crate) fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn sniff_kind(p: &Path) -> std::io::Result<ArtifactKind> {
    let mut f = File::open(p)?;
    let mut buf = vec![0u8; 8192];
    let n = f.read(&mut buf)?;
    let head = &buf[..n];
    let text = !head.contains(&0) && std::str::from_utf8(head).is_ok();
    Ok(if text { ArtifactKind::Text } else { ArtifactKind::Binary })
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn blob_path(&self, sha256: &str) -> PathBuf {
        self.root.join(&sha256[..2.min(sha256.len())]).join(sha256)
    }

    /// Resolve every requested path (expanding directories), then hash and
    /// either copy (≤ `max_bytes`) or record as external. Agent-supplied
    /// metadata on `requested` is ignored; only `path` is read.
    pub fn ingest(
        &self,
        workspace: &Path,
        requested: &[ArtifactRef],
        max_bytes: u64,
    ) -> Result<Vec<ArtifactRef>, ArtifactError> {
        let ws_canon = fs::canonicalize(workspace).map_err(|source| ArtifactError::Io {
            path: workspace.display().to_string(),
            source,
        })?;
        let mut out = Vec::new();
        for r in requested {
            if let Some(reason) = rel_path_problem(&r.path) {
                return Err(ArtifactError::Path { path: r.path.clone(), reason });
            }
            let abs = workspace.join(&r.path);
            if fs::symlink_metadata(&abs).is_err() {
                return Err(ArtifactError::Missing { path: r.path.clone() });
            }
            let canon = fs::canonicalize(&abs).map_err(|source| ArtifactError::Io {
                path: r.path.clone(),
                source,
            })?;
            if !canon.starts_with(&ws_canon) {
                return Err(ArtifactError::Escapes { path: r.path.clone() });
            }
            if canon.is_dir() {
                let mut files = Vec::new();
                collect_files(&canon, &mut files).map_err(|source| ArtifactError::Io {
                    path: r.path.clone(),
                    source,
                })?;
                files.sort();
                for f in files {
                    let rel = f
                        .strip_prefix(&ws_canon)
                        .expect("collected under the workspace")
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push(self.ingest_file(&rel, &f, max_bytes)?);
                }
            } else {
                out.push(self.ingest_file(&r.path, &canon, max_bytes)?);
            }
        }
        Ok(out)
    }

    fn ingest_file(&self, rel: &str, abs: &Path, max_bytes: u64) -> Result<ArtifactRef, ArtifactError> {
        let io = |source| ArtifactError::Io { path: rel.to_string(), source };
        let size = fs::metadata(abs).map_err(io)?.len();
        let kind = sniff_kind(abs).map_err(io)?;
        let (sha256, stored) = if size > max_bytes {
            (sha256_file(abs).map_err(io)?, ArtifactStorage::External)
        } else {
            (self.copy_hashing(abs).map_err(io)?, ArtifactStorage::Copied)
        };
        Ok(ArtifactRef {
            path: rel.to_string(),
            sha256,
            size,
            kind: Some(kind),
            stored: Some(stored),
            host: None,
        })
    }

    /// Stream `src` into a temp file in the store while hashing, then move it
    /// to its content address (or drop it when that blob already exists).
    fn copy_hashing(&self, src: &Path) -> std::io::Result<String> {
        fs::create_dir_all(&self.root)?;
        let tmp = self.root.join(format!("{}.tmp", ulid::Ulid::new()));
        let mut input = File::open(src)?;
        let mut output = File::create(&tmp)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            output.write_all(&buf[..n])?;
        }
        output.flush()?;
        drop(output);
        let sha = hex(&h.finalize());
        let dest = self.blob_path(&sha);
        if dest.exists() {
            fs::remove_file(&tmp)?;
        } else {
            fs::create_dir_all(dest.parent().expect("blob path has a parent"))?;
            fs::rename(&tmp, &dest)?;
        }
        Ok(sha)
    }
}

/// Every regular file under `dir`. Symlinks are skipped, not followed: a link
/// inside a PoC directory must not pull in files from elsewhere.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_files(&entry.path(), out)?;
        } else if ft.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests and check that they pass**

Run: `cargo test -p rupu-coverage report::artifacts`
Expected: PASS (7 tests on unix).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/report
git commit -m "feat(coverage): content-addressed finding artifact store with size cap

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Write options, guidance, and the profile-enforcing write path

**Files:**
- Create: `crates/rupu-coverage/src/report/options.rs`, `crates/rupu-coverage/src/report/guidance.rs`
- Modify: `crates/rupu-coverage/src/report/mod.rs`, `crates/rupu-coverage/src/ledger/paths.rs`, `crates/rupu-coverage/src/tools/report_finding.rs`, `crates/rupu-coverage/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces:
  - `pub struct FindingWriteOptions { pub profile: FindingProfile, pub artifact_root: Option<PathBuf>, pub artifact_max_bytes: u64, pub report_max_bytes: usize, pub ticket_patterns: Vec<String> }`. `Default` gives `Full`, no store, `524_288_000`, `262_144`, no patterns. Consts `DEFAULT_ARTIFACT_MAX_BYTES`, `DEFAULT_REPORT_MAX_BYTES`.
  - `pub fn guidance(opts: &FindingWriteOptions) -> Option<String>` (Some only for `Full`).
  - `CoveragePaths.workspace: PathBuf`.
  - `ReportFindingInput { .., summary: Option<String>, severity: Option<Severity>, evidence: Option<FindingEvidence>, report: Option<FindingReport> }`.
  - `report_finding(paths: &CoveragePaths, attribution: Attribution, input: ReportFindingInput, opts: &FindingWriteOptions) -> Result<ReportFindingOutput, ReportFindingError>`.
  - New `ReportFindingError` variants: `MissingField(&'static str)`, `ReportRequired`, `ReportInSummaryMode`, `DerivedFieldsSupplied`, `Report(ReportValidationError)`, `Artifact(ArtifactError)`.
  - Re-exported from the crate root: `FindingWriteOptions`.

- [ ] **Step 1: Write the options and guidance modules**

Create `crates/rupu-coverage/src/report/options.rs`:

```rust
//! Per-run settings for recording findings, carried to the write path on
//! `ToolContext.findings` (agent builtin) or `FindingsContext` (MCP tool).

use crate::report::FindingProfile;
use std::path::PathBuf;

pub const DEFAULT_ARTIFACT_MAX_BYTES: u64 = 500 * 1024 * 1024;
pub const DEFAULT_REPORT_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingWriteOptions {
    pub profile: FindingProfile,
    /// Root of the artifact store (`<RUPU_HOME>/findings/artifacts`). `None`
    /// makes a report that lists artifacts fail loudly rather than drop them.
    pub artifact_root: Option<PathBuf>,
    pub artifact_max_bytes: u64,
    pub report_max_bytes: usize,
    /// Organisation-specific ticket patterns (from `[findings].ticket_patterns`),
    /// appended to the prompt guidance. Nothing org-specific ships in rupu.
    pub ticket_patterns: Vec<String>,
}

impl Default for FindingWriteOptions {
    fn default() -> Self {
        Self {
            profile: FindingProfile::default(),
            artifact_root: None,
            artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
            report_max_bytes: DEFAULT_REPORT_MAX_BYTES,
            ticket_patterns: Vec::new(),
        }
    }
}

impl FindingWriteOptions {
    pub fn with_profile(mut self, profile: FindingProfile) -> Self {
        self.profile = profile;
        self
    }
}
```

Create `crates/rupu-coverage/src/report/guidance.rs`:

```rust
//! System-prompt guidance appended to agents recording under the full
//! profile, so an agent needs no external reporting-standard file.

use crate::report::{FindingProfile, FindingWriteOptions};

const FULL_GUIDANCE: &str = "\
## Recording findings

Findings in this run use the full report profile. Record each finding with one \
`report_finding` call whose `report` object is complete. rupu generates the \
Markdown, HTML and PDF reports from it, so do not also write a report file.

- Write in a concise, formal, factual tone. Do not speculate.
- Every field is required. When information genuinely cannot be determined, \
use the field's sentinel instead of omitting or guessing it: `Unknown` for owner, \
product, affected_component, attack_vector and cvss_v3; `Not Applicable` for \
source_repository when no source-controlled code is involved; `None Provided` for \
tickets when none are mentioned; `None` for cross_references when no related \
finding exists.
- Never invent owners, products, repositories, ticket identifiers or CVSS scores.
- `root_cause` names the single underlying defect — the exact variable, check or \
assumption that is wrong — not the symptom.
- `call_chain` runs from the externally reachable entry point to the sink, one hop \
per entry, with file and lines (or binary_va), and every gate crossed and why it passes.
- `evidence` is a list of claims, each citing file and lines (or binary_va) with \
only the lines that matter.
- `recommended_patch`, `ci_cd_detection` and `regression_test` are mandatory: a \
minimal unified diff; a pipeline check that fails the build on this class of issue; \
a deterministic test with its exact command and its result on the vulnerable and the \
patched build. If one genuinely cannot be produced, write exactly \
`Not Provided — <one-line justification>`.
- List proof-of-concept files (scripts, outputs, harnesses) as workspace-relative \
paths in `artifacts`; rupu stores them with the finding.
- Cross-reference related findings by the `fnd_` id a previous `report_finding` \
call returned.
- A rejected call lists every problem at once. Fix all of them and call again.";

pub fn guidance(opts: &FindingWriteOptions) -> Option<String> {
    if opts.profile != FindingProfile::Full {
        return None;
    }
    let mut s = FULL_GUIDANCE.to_string();
    if !opts.ticket_patterns.is_empty() {
        s.push_str(
            "\n- Treat references matching these patterns as existing tickets (record them in `tickets`): ",
        );
        s.push_str(&opts.ticket_patterns.join(", "));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_full_gets_guidance() {
        assert!(guidance(&FindingWriteOptions::default()).is_some());
        let summary = FindingWriteOptions::default().with_profile(FindingProfile::Summary);
        assert!(guidance(&summary).is_none());
    }

    #[test]
    fn ticket_patterns_are_appended() {
        let o = FindingWriteOptions {
            ticket_patterns: vec!["ABC-[0-9]+".into()],
            ..Default::default()
        };
        assert!(guidance(&o).unwrap().contains("ABC-[0-9]+"));
    }
}
```

Update `report/mod.rs`:

```rust
pub mod artifacts;
pub mod guidance;
pub mod options;
pub mod profile;
pub mod schema;
pub mod types;
pub mod validate;

pub use artifacts::{ArtifactError, ArtifactStore};
pub use guidance::guidance;
pub use options::{FindingWriteOptions, DEFAULT_ARTIFACT_MAX_BYTES, DEFAULT_REPORT_MAX_BYTES};
pub use profile::FindingProfile;
pub use types::*;
pub use validate::{validate_report, FieldError, ReportValidationError, ValidateCtx};
```

Change the crate-root re-export in `lib.rs` to `pub use report::{FindingProfile, FindingReport, FindingWriteOptions};`.

In `crates/rupu-coverage/src/ledger/paths.rs`, add `pub workspace: PathBuf,` as the first field of `CoveragePaths`, and set `workspace: workspace.to_path_buf(),` in `new`.

Run: `cargo build --workspace --tests 2>&1 | grep -B2 "missing field \`workspace\`"`. There should be no `CoveragePaths { .. }` literals outside `new`. If any turn up, add `workspace` to them.

- [ ] **Step 2: Write the failing write-path tests**

In `crates/rupu-coverage/src/tools/report_finding.rs`'s existing `mod tests`:

1. Replace every call `report_finding(&paths, attribution(), X)` with `report_finding(&paths, attribution(), X, &summary_opts())`.
2. In the existing `input(...)` helper and any `ReportFindingInput { .. }` literal, wrap `summary`, `severity`, and `evidence` in `Some(..)` and add `report: None`.
3. Add this helper plus the new tests:

```rust
    fn summary_opts() -> crate::report::FindingWriteOptions {
        crate::report::FindingWriteOptions::default()
            .with_profile(crate::report::FindingProfile::Summary)
    }

    fn full_opts(store: &std::path::Path) -> crate::report::FindingWriteOptions {
        crate::report::FindingWriteOptions {
            artifact_root: Some(store.to_path_buf()),
            ..Default::default()
        }
    }

    fn fixture_report() -> crate::report::FindingReport {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap()
    }

    fn full_input(report: crate::report::FindingReport) -> ReportFindingInput {
        ReportFindingInput {
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::Repo,
            summary: None,
            severity: None,
            concern_id: None,
            evidence: None,
            report: Some(report),
        }
    }

    fn only_record(paths: &CoveragePaths) -> FindingRecord {
        let text = std::fs::read_to_string(&paths.findings).unwrap();
        serde_json::from_str(text.lines().next().unwrap()).unwrap()
    }

    #[test]
    fn full_profile_records_report_and_derives_top_level_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        report_finding(&paths, attribution(), full_input(fixture_report()), &full_opts(store.path()))
            .expect("valid full report records");
        let rec = only_record(&paths);
        assert_eq!(rec.profile, crate::report::FindingProfile::Full);
        let report = rec.report.as_ref().unwrap();
        assert_eq!(rec.summary, report.title);
        assert_eq!(rec.severity, Severity::Critical);
        assert_eq!(rec.evidence.rationale, report.root_cause);
        assert_eq!(
            rec.evidence.code_excerpt.as_deref(),
            report.evidence[0].excerpt.as_deref()
        );
    }

    #[test]
    fn full_profile_without_report_is_rejected() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = full_input(fixture_report());
        i.report = None;
        let err = report_finding(&paths, attribution(), i, &full_opts(ws.path())).unwrap_err();
        assert!(matches!(err, ReportFindingError::ReportRequired), "{err}");
        assert!(!paths.findings.exists(), "nothing written on rejection");
    }

    #[test]
    fn full_profile_rejects_derived_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = full_input(fixture_report());
        i.summary = Some("x".into());
        let err = report_finding(&paths, attribution(), i, &full_opts(ws.path())).unwrap_err();
        assert!(matches!(err, ReportFindingError::DerivedFieldsSupplied), "{err}");
    }

    #[test]
    fn full_profile_surfaces_every_validation_problem() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut r = fixture_report();
        r.root_cause = String::new();
        r.regression_test = crate::report::OrSentinel::Sentinel("Unknown".into());
        let err = report_finding(&paths, attribution(), full_input(r), &full_opts(ws.path()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("report.root_cause"), "{err}");
        assert!(err.contains("report.regression_test"), "{err}");
        assert!(!paths.findings.exists());
    }

    #[test]
    fn summary_profile_rejects_a_report() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let err = report_finding(&paths, attribution(), full_input(fixture_report()), &summary_opts())
            .unwrap_err();
        assert!(matches!(err, ReportFindingError::ReportInSummaryMode), "{err}");
    }

    #[test]
    fn full_profile_ingests_artifacts_and_hashes_claim_files() {
        let ws = tempfile::TempDir::new().unwrap();
        let store = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("src/routes")).unwrap();
        std::fs::write(ws.path().join("src/routes/notes.rs"), "async fn get_note() {}\n").unwrap();
        std::fs::create_dir_all(ws.path().join("pocs")).unwrap();
        std::fs::write(ws.path().join("pocs/out.txt"), "GET /api/notes/42 as user B: 200\n").unwrap();
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "pocs/out.txt".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        report_finding(&paths, attribution(), full_input(r), &full_opts(store.path())).unwrap();
        let rec = only_record(&paths);
        let rep = rec.report.unwrap();
        assert_eq!(rep.artifacts[0].stored, Some(crate::report::ArtifactStorage::Copied));
        assert_eq!(rep.evidence[0].sha256.as_ref().map(String::len), Some(64));
    }

    #[test]
    fn artifacts_without_a_store_fail_loudly() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::write(ws.path().join("out.txt"), "x").unwrap();
        let mut r = fixture_report();
        r.artifacts = vec![crate::report::ArtifactRef {
            path: "out.txt".into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }];
        let paths = CoveragePaths::new(ws.path(), "t");
        let opts = crate::report::FindingWriteOptions::default(); // no artifact_root
        let err = report_finding(&paths, attribution(), full_input(r), &opts).unwrap_err();
        assert!(matches!(err, ReportFindingError::Artifact(crate::report::ArtifactError::NoStore)), "{err}");
    }

    #[test]
    fn summary_profile_requires_its_three_fields() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut i = input(FindingScope::Repo);
        i.severity = None;
        let err = report_finding(&paths, attribution(), i, &summary_opts()).unwrap_err();
        assert!(matches!(err, ReportFindingError::MissingField("severity")), "{err}");
    }
```

(`input(FindingScope::Repo)` is the existing helper after step 2 of this list. Check its name in the file; if it differs, use that name.)

- [ ] **Step 3: Run the tests and check that they fail**

Run: `cargo test -p rupu-coverage tools::report_finding`
Expected: compile errors (the 4-argument `report_finding` and the new variants don't exist yet).

- [ ] **Step 4: Implement the write path**

In `crates/rupu-coverage/src/tools/report_finding.rs`:

Replace `ReportFindingInput`'s `summary`, `severity`, and `evidence` fields and add `report`:

```rust
    /// Required under the summary profile; derived from `report` under full.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub evidence: Option<FindingEvidence>,
    /// The full report. Required under the full profile; refused under summary.
    #[serde(default)]
    pub report: Option<crate::report::FindingReport>,
```

(Keep the field order `file_path, line_range, target_ref, scope, summary, severity, concern_id, evidence, report`.)

Add the error variants to `ReportFindingError`:

```rust
    #[error("`{0}` is required under the summary findings profile")]
    MissingField(&'static str),
    #[error("this step records findings under the full profile: `report` is required (see the report_finding tool schema)")]
    ReportRequired,
    #[error("this step records findings under the summary profile: omit `report`, or have the workflow author set findings_profile: full")]
    ReportInSummaryMode,
    #[error("under the full findings profile `summary`, `severity` and `evidence` are derived from `report`; omit them")]
    DerivedFieldsSupplied,
    #[error("{0}")]
    Report(#[from] crate::report::ReportValidationError),
    #[error("{0}")]
    Artifact(#[from] crate::report::ArtifactError),
```

Replace the body of `report_finding` with:

```rust
pub fn report_finding(
    paths: &CoveragePaths,
    attribution: Attribution,
    input: ReportFindingInput,
    opts: &crate::report::FindingWriteOptions,
) -> Result<ReportFindingOutput, ReportFindingError> {
    use crate::report::{FindingProfile, ValidateCtx};

    validate_locator(&input)?;
    let (summary, severity, evidence, report) = match opts.profile {
        FindingProfile::Summary => {
            if input.report.is_some() {
                return Err(ReportFindingError::ReportInSummaryMode);
            }
            (
                input.summary.ok_or(ReportFindingError::MissingField("summary"))?,
                input.severity.ok_or(ReportFindingError::MissingField("severity"))?,
                input.evidence.ok_or(ReportFindingError::MissingField("evidence"))?,
                None,
            )
        }
        FindingProfile::Full => {
            if input.summary.is_some() || input.severity.is_some() || input.evidence.is_some() {
                return Err(ReportFindingError::DerivedFieldsSupplied);
            }
            let mut report = input.report.ok_or(ReportFindingError::ReportRequired)?;
            let known: Vec<String> = crate::ledger::read_findings(paths)?
                .into_iter()
                .map(|f| f.id)
                .collect();
            crate::report::validate_report(
                &report,
                &ValidateCtx { known_finding_ids: &known, max_bytes: opts.report_max_bytes },
            )?;
            if !report.artifacts.is_empty() {
                let store = opts
                    .artifact_root
                    .as_ref()
                    .map(crate::report::ArtifactStore::new)
                    .ok_or(crate::report::ArtifactError::NoStore)?;
                report.artifacts =
                    store.ingest(&paths.workspace, &report.artifacts, opts.artifact_max_bytes)?;
            }
            hash_claim_files(&paths.workspace, &mut report);
            let evidence = FindingEvidence {
                code_excerpt: report.evidence.iter().find_map(|c| c.excerpt.clone()),
                rationale: report.root_cause.clone(),
                references: Vec::new(),
            };
            (
                report.title.clone(),
                Severity::from(report.rating.risk_rating),
                evidence,
                Some(report),
            )
        }
    };

    let id = format!("fnd_{}", Ulid::new());
    let record = FindingRecord {
        id: id.clone(),
        file_path: input.file_path,
        line_range: input.line_range,
        target_ref: input.target_ref,
        scope: input.scope,
        summary,
        severity,
        concern_id: input.concern_id,
        evidence,
        declared_by: attribution,
        declared_at: Utc::now(),
        profile: opts.profile,
        report,
    };
    paths.ensure_dir()?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.findings)?;
    let line = serde_json::to_string(&record)?;
    f.write_all(line.as_bytes())?;
    f.write_all(b"\n")?;
    f.flush()?;
    Ok(ReportFindingOutput { id })
}

/// Record the SHA-256 of each evidence claim's file as it is right now, so a
/// viewer can later flag a claim whose code has changed. Claims whose file
/// is not in the workspace (binary targets, other trees) keep whatever the
/// agent supplied.
fn hash_claim_files(workspace: &std::path::Path, report: &mut crate::report::FindingReport) {
    for claim in &mut report.evidence {
        if let Some(file) = &claim.file {
            let abs = workspace.join(file);
            if abs.is_file() {
                if let Ok(h) = crate::report::artifacts::sha256_file(&abs) {
                    claim.sha256 = Some(h);
                }
            }
        }
    }
}
```

`validate_locator` must still work. Update it if it reads `input.summary` (it shouldn't; it checks locators only). `crate::ledger::read_findings` returns `std::io::Result`, which converts through the existing `Io(#[from])` variant.

- [ ] **Step 5: Run the tests and check that they pass**

Run: `cargo test -p rupu-coverage`
Expected: PASS, the whole crate (existing tests with `summary_opts()` plus 8 new ones).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): report_finding enforces the findings profile, ingests artifacts

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

(Downstream crates don't compile until Tasks 7–8. Tasks 5–8 land in the same PR.)

---

### Task 6: `[findings]` config section

**Files:**
- Create: `crates/rupu-config/src/findings_config.rs`
- Modify: `crates/rupu-config/src/lib.rs`, `crates/rupu-config/src/config.rs:40-72` (the `Config` struct)

**Interfaces:**
- Produces: `rupu_config::FindingsConfig { artifact_max_bytes: Option<u64>, report_max_bytes: Option<u64>, ticket_patterns: Vec<String> }`; `Config.findings: FindingsConfig`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-config/src/findings_config.rs`:

```rust
//! `[findings]` section — limits and org-specific hints for recorded findings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FindingsConfig {
    /// Per-file cap for copying finding artifacts into the store. Larger
    /// files are recorded by hash only. Default 500 MB.
    pub artifact_max_bytes: Option<u64>,
    /// Serialized-size budget for one finding report. Default 256 KiB.
    pub report_max_bytes: Option<u64>,
    /// Patterns (regexes or URL prefixes) that identify existing tickets in
    /// this organisation; added to the agent guidance. Empty by default.
    pub ticket_patterns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn parses_findings_section() {
        let cfg: Config = toml::from_str(
            r#"
            [findings]
            artifact_max_bytes = 1048576
            ticket_patterns = ["ABC-[0-9]+"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.findings.artifact_max_bytes, Some(1_048_576));
        assert_eq!(cfg.findings.report_max_bytes, None);
        assert_eq!(cfg.findings.ticket_patterns, vec!["ABC-[0-9]+".to_string()]);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[findings]\nbogus = 1\n").is_err());
    }
}
```

In `lib.rs`: add `pub mod findings_config;` and `pub use findings_config::FindingsConfig;`.

- [ ] **Step 2: Run the test and check that it fails**

Run: `cargo test -p rupu-config findings_config`
Expected: compile error (`no field findings on Config`).

- [ ] **Step 3: Add the field**

In `config.rs`, add to `Config` after `workflow`:

```rust
    #[serde(default)]
    pub findings: crate::findings_config::FindingsConfig,
```

Fix any `Config { .. }` literals the compiler reports by adding `findings: Default::default(),`:

Run: `cargo build --workspace --tests 2>&1 | grep -B2 "missing field \`findings\`"`

- [ ] **Step 4: Run the tests and check that they pass**

Run: `cargo test -p rupu-config`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-config
git commit -m "feat(config): [findings] section (artifact cap, report budget, ticket patterns)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Agent side — ToolContext.findings, profile-aware report_finding, guidance, `findingsProfile`

**Files:**
- Modify: `crates/rupu-tools/src/tool.rs:38-119` (`ToolContext` + `Default`)
- Modify: `crates/rupu-agent/src/coverage_tools.rs` (`ReportFindingTool`, `register`)
- Modify: `crates/rupu-agent/src/runner.rs:906-985` (guidance + registration)
- Modify: `crates/rupu-agent/src/spec.rs` (`Frontmatter`, `AgentSpec`, `parse`)
- Modify: every `ToolContext { .. }` and `AgentSpec { .. }` literal the compiler flags
- Modify: `crates/rupu-agent/tests/findings_without_coverage.rs`, `crates/rupu-agent/tests/coverage_integration.rs`
- Create: `crates/rupu-agent/tests/findings_full_profile.rs`

**Interfaces:**
- Consumes: `FindingWriteOptions`, `FindingProfile`, `report::guidance`, `report::schema::advertised_schema`, 4-arg `report_finding`.
- Produces:
  - `ToolContext.findings: Option<rupu_coverage::FindingWriteOptions>` (`None` ⇒ `FindingWriteOptions::default()`, i.e. `full`).
  - `ReportFindingTool::new(paths: CoveragePaths, options: FindingWriteOptions)`.
  - `coverage_tools::register(registry, catalog, paths, options: FindingWriteOptions)`.
  - `AgentSpec.findings_profile: Option<FindingProfile>` (frontmatter `findingsProfile`).

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-agent/tests/findings_full_profile.rs`:

```rust
//! Under the default (full) profile an agent must send a complete report.
//! An incomplete one is rejected with every problem listed; the agent fixes
//! it and the retry records exactly one finding.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{target_id, CoveragePaths, FindingWriteOptions};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

fn report() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap()
}

fn call(id: &str, report: serde_json::Value) -> ScriptedTurn {
    ScriptedTurn::AssistantToolUse {
        text: None,
        tool_id: id.into(),
        tool_name: "report_finding".into(),
        tool_input: serde_json::json!({ "scope": "repo", "report": report }),
        stop: StopReason::ToolUse,
    }
}

fn opts(workspace: &std::path::Path, store: &std::path::Path, turns: Vec<ScriptedTurn>) -> AgentRunOpts {
    AgentRunOpts {
        seed_source: None,
        agent_name: "assessor".into(),
        agent_system_prompt: "You assess code.".into(),
        agent_tools: Some(vec!["report_finding".into()]),
        provider: Box::new(CapturingMockProvider::new(turns)),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_full".into(),
        workspace_id: "ws_full".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 6,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.to_path_buf(),
            findings: Some(FindingWriteOptions {
                artifact_root: Some(store.to_path_buf()),
                ..Default::default()
            }),
            ..Default::default()
        },
        user_message: "Assess.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        output_schema: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns: None,
        scope_name: None,
        max_tokens: rupu_agent::runner::DEFAULT_MAX_TOKENS,
        surface_tag: Some("agent".into()),
        context_window_tokens: None,
        compact_at_percent: None,
        pause: None,
    }
}

#[tokio::test]
async fn incomplete_report_is_rejected_then_retry_records_one_finding() {
    let ws = tempfile::TempDir::new().unwrap();
    let store = tempfile::TempDir::new().unwrap();
    let mut bad = report();
    bad["root_cause"] = serde_json::json!("");
    let turns = vec![
        call("t1", bad),
        call("t2", report()),
        ScriptedTurn::AssistantText {
            text: "Done.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ];
    run_agent(opts(ws.path(), store.path(), turns)).await.expect("run succeeds");

    let paths = CoveragePaths::new(ws.path(), &target_id(ws.path(), "assessor"));
    let text = std::fs::read_to_string(&paths.findings).unwrap();
    let lines: Vec<_> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly the retry is recorded");
    let rec: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(rec["profile"], "full");
    assert_eq!(rec["severity"], "critical");

    let transcript = std::fs::read_to_string(ws.path().join("run.jsonl")).unwrap();
    assert!(transcript.contains("report.root_cause"), "rejection names the field");
    assert!(transcript.contains("## Recording findings"), "full-profile guidance in the system prompt");
}
```

Add a unit test at the bottom of `crates/rupu-agent/src/coverage_tools.rs` (create a `#[cfg(test)] mod tests` if there isn't one):

```rust
#[cfg(test)]
mod findings_profile_tests {
    use super::*;
    use rupu_coverage::{FindingProfile, FindingWriteOptions};

    fn tool(profile: FindingProfile) -> ReportFindingTool {
        let tmp = std::env::temp_dir();
        ReportFindingTool::new(
            CoveragePaths::new(&tmp, "t"),
            FindingWriteOptions::default().with_profile(profile),
        )
    }

    #[test]
    fn full_profile_advertises_report_as_required() {
        let s = tool(FindingProfile::Full).input_schema();
        let req: Vec<&str> = s["required"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(req.contains(&"report"), "{req:?}");
        assert!(!req.contains(&"summary"));
        assert!(s["properties"]["report"]["properties"]["root_cause"].is_object());
        assert!(s["properties"].get("summary").is_none());
    }

    #[test]
    fn summary_profile_advertises_the_lightweight_fields() {
        let s = tool(FindingProfile::Summary).input_schema();
        let req: Vec<&str> = s["required"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert!(req.contains(&"summary") && req.contains(&"severity") && req.contains(&"evidence"));
        assert!(s["properties"].get("report").is_none());
    }
}
```

In `crates/rupu-agent/src/spec.rs`'s test module, add:

```rust
    #[test]
    fn parses_findings_profile() {
        let s = "---\nname: a\nfindingsProfile: summary\n---\nbody\n";
        let spec = AgentSpec::parse(s).unwrap();
        assert_eq!(spec.findings_profile, Some(rupu_coverage::FindingProfile::Summary));
        let s = "---\nname: a\n---\nbody\n";
        assert_eq!(AgentSpec::parse(s).unwrap().findings_profile, None);
    }
```

- [ ] **Step 2: Run the tests and check that they fail**

Run: `cargo test -p rupu-agent --test findings_full_profile; cargo test -p rupu-agent findings_profile_tests parses_findings_profile`
Expected: compile errors (`findings` not on `ToolContext`, 2-arg `ReportFindingTool::new`, no `findings_profile`).

- [ ] **Step 3: Add `ToolContext.findings`**

In `crates/rupu-tools/src/tool.rs`, add to `ToolContext` after `tool_mappings`:

```rust
    /// How findings are recorded in this run: the resolved profile, the
    /// artifact store, and size limits. `None` means the caller did not
    /// configure it, and tools use `FindingWriteOptions::default()`, which
    /// is the full profile with no artifact store.
    #[serde(skip)]
    pub findings: Option<rupu_coverage::FindingWriteOptions>,
```

Add `findings: None,` to `impl Default for ToolContext`. Then fix every literal:

Run: `cargo build --workspace --tests 2>&1 | grep -A2 "missing field \`findings\` in initializer of \`.*ToolContext\`" | grep -- "-->"`

Add `findings: None,` to each flagged `ToolContext { .. }` literal. The production sites (`step_factory.rs`, `cmd/run.rs`, `cmd/dispatch.rs`, `cmd/session.rs`) get real values in Task 9. `None` is correct for now.

- [ ] **Step 4: Profile-aware ReportFindingTool**

In `crates/rupu-agent/src/coverage_tools.rs`:

Change the struct and constructor:

```rust
pub struct ReportFindingTool {
    paths: CoveragePaths,
    options: rupu_coverage::FindingWriteOptions,
}

impl ReportFindingTool {
    /// Build the tool against an explicit ledger location and the run's
    /// findings options (profile, artifact store, limits).
    ///
    /// `register` below wires this up as part of the coverage harness. This
    /// constructor exists for the other caller: an agent that records
    /// findings WITHOUT a `concerns:` block (see `runner`'s
    /// findings-without-coverage registration).
    pub fn new(paths: CoveragePaths, options: rupu_coverage::FindingWriteOptions) -> Self {
        Self { paths, options }
    }
}
```

Replace `input_schema` with a profile switch. The summary branch is today's JSON literal, unchanged. Move it into `fn summary_schema() -> Value` and add a full branch:

```rust
    fn input_schema(&self) -> Value {
        match self.options.profile {
            rupu_coverage::FindingProfile::Summary => summary_schema(),
            rupu_coverage::FindingProfile::Full => full_schema(),
        }
    }
```

```rust
/// The lightweight record: today's schema, verbatim.
fn summary_schema() -> Value {
    // ← move the existing `serde_json::json!({ ... })` body of
    //   `input_schema` here unchanged.
}

/// The full profile: locators + a complete `report`. `summary`, `severity`
/// and `evidence` are derived from the report, so they are not offered.
fn full_schema() -> Value {
    let mut s = summary_schema();
    let props = s["properties"].as_object_mut().expect("object schema");
    props.remove("summary");
    props.remove("severity");
    props.remove("evidence");
    props.insert(
        "report".to_string(),
        rupu_coverage::report::schema::advertised_schema(),
    );
    s["required"] = serde_json::json!(["scope", "report"]);
    s
}
```

Change `description()` to mention both profiles:

```rust
    fn description(&self) -> &'static str {
        "Record a security or quality finding in this project's ledger. Returns the \
         generated finding id (use it in coverage_mark calls and in another finding's \
         cross_references). Under the full profile send a complete `report`; a rejected \
         call lists every problem to fix."
    }
```

In `invoke`, pass the options:

```rust
        match report_finding(&self.paths, attribution, parsed, &self.options) {
```

Change `register` to take the options and pass them through:

```rust
pub fn register(
    registry: &mut ToolRegistry,
    catalog: Arc<FlatCatalog>,
    paths: CoveragePaths,
    findings: rupu_coverage::FindingWriteOptions,
) {
```

In `register`, change the insert line to:

```rust
    registry.insert("report_finding", Arc::new(ReportFindingTool::new(paths, findings)));
```

(Keep `register`'s real registry type name: whatever its first parameter's type is today.)

- [ ] **Step 5: Runner — guidance + registration**

In `crates/rupu-agent/src/runner.rs`, replace the block

```rust
    // Append catalog prompt section to system prompt when coverage is active.
    if let Some(bundle) = &coverage {
        opts.agent_system_prompt.push_str("\n\n");
        opts.agent_system_prompt.push_str(&bundle.prompt_section);
    }
```

with:

```rust
    // Append catalog prompt section to system prompt when coverage is active.
    if let Some(bundle) = &coverage {
        opts.agent_system_prompt.push_str("\n\n");
        opts.agent_system_prompt.push_str(&bundle.prompt_section);
    }

    // Findings contract for this run. Resolved before `RunStart` is written
    // so the guidance is part of the recorded system prompt.
    let findings_opts = opts.tool_context.findings.clone().unwrap_or_default();
    let records_findings = coverage.is_some()
        || opts
            .agent_tools
            .as_ref()
            .is_some_and(|list| list.iter().any(|t| t == "report_finding"));
    if records_findings {
        if let Some(g) = rupu_coverage::report::guidance(&findings_opts) {
            opts.agent_system_prompt.push_str("\n\n");
            opts.agent_system_prompt.push_str(&g);
        }
    }
```

Change the two registration sites:

```rust
        coverage_tools::register(
            &mut registry,
            bundle.catalog.clone(),
            bundle.paths.clone(),
            findings_opts.clone(),
        );
```

```rust
            std::sync::Arc::new(coverage_tools::ReportFindingTool::new(paths, findings_opts.clone())),
```

- [ ] **Step 6: `findingsProfile` frontmatter**

In `crates/rupu-agent/src/spec.rs`, add to `Frontmatter` after `compact_at_percent`:

```rust
    /// Findings contract when no workflow step or workflow default overrides
    /// it: `full` (a complete report) or `summary`. Absent ⇒ `full`.
    #[serde(default, rename = "findingsProfile")]
    findings_profile: Option<rupu_coverage::FindingProfile>,
```

Add `pub findings_profile: Option<rupu_coverage::FindingProfile>,` to `AgentSpec` after `compact_at_percent`, and `findings_profile: fm.findings_profile,` in `parse`. Fix `AgentSpec { .. }` literals (for example `step_factory.rs`'s error-stub spec at ~line 200) by adding `findings_profile: None,`:

Run: `cargo build --workspace --tests 2>&1 | grep -A2 "missing field \`findings_profile\`" | grep -- "-->"`

- [ ] **Step 7: Keep the existing summary-shaped tests on the summary profile**

In `crates/rupu-agent/tests/findings_without_coverage.rs`, change the `tool_context` in `opts_for` to:

```rust
        tool_context: ToolContext {
            workspace_path: workspace.to_path_buf(),
            // These tests exercise the lightweight record.
            findings: Some(
                rupu_coverage::FindingWriteOptions::default()
                    .with_profile(rupu_coverage::FindingProfile::Summary),
            ),
            ..Default::default()
        },
```

Do the same in every `AgentRunOpts` in `crates/rupu-agent/tests/coverage_integration.rs` whose script calls `report_finding` with `summary`/`severity`/`evidence`. Find them with `grep -n '"report_finding"' crates/rupu-agent/tests/coverage_integration.rs`.

- [ ] **Step 8: Run the tests and check that they pass**

Run: `cargo test -p rupu-tools && cargo test -p rupu-agent`
Expected: PASS, including `findings_full_profile`, `findings_profile_tests`, `parses_findings_profile`, and the pre-existing findings/coverage tests.

- [ ] **Step 9: Commit**

```bash
git add crates/rupu-tools crates/rupu-agent crates/rupu-orchestrator crates/rupu-cli
git commit -m "feat(agent): profile-aware report_finding, full-profile guidance, findingsProfile frontmatter

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: MCP `findings.record` honours the profile

**Files:**
- Modify: `crates/rupu-mcp/src/tools/findings.rs`
- Modify: `crates/rupu-mcp/tests/findings_record.rs`
- Modify: `FindingsContext { .. }` literals in `crates/rupu-cli/src/cmd/workflow.rs:3234,4784`, `crates/rupu-cli/src/resume.rs:317`

**Interfaces:**
- Consumes: 4-arg `report_finding`, `FindingWriteOptions`, `advertised_schema`.
- Produces: `FindingsContext.options: rupu_coverage::FindingWriteOptions`; `RecordArgs` with optional `summary`/`severity`/`rationale` and `report: Option<FindingReport>`.

- [ ] **Step 1: Write the failing tests**

In `crates/rupu-mcp/tests/findings_record.rs`, change `ctx` to take a profile:

```rust
fn ctx(workspace: &std::path::Path) -> FindingsContext {
    ctx_with(workspace, rupu_coverage::FindingProfile::Summary)
}

fn ctx_with(workspace: &std::path::Path, profile: rupu_coverage::FindingProfile) -> FindingsContext {
    FindingsContext {
        workspace_path: workspace.to_path_buf(),
        scope_name: "chimera-campaign".to_string(),
        run_id: "run_mcp_test".to_string(),
        model: "gpt-5.6-cyber".to_string(),
        surface: rupu_coverage::Surface::Workflow,
        options: rupu_coverage::FindingWriteOptions::default().with_profile(profile),
    }
}
```

Add:

```rust
#[tokio::test]
async fn full_profile_records_a_report() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let report: serde_json::Value = serde_json::from_str(include_str!(
        "../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
    ))
    .unwrap();
    let out = dispatcher
        .call("findings.record", serde_json::json!({ "scope": "repo", "report": report }))
        .await
        .expect("full report records");
    assert!(out.starts_with("finding_id: fnd_"), "got {out}");
}

#[tokio::test]
async fn full_profile_refuses_a_summary_only_call() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx_with(tmp.path(), rupu_coverage::FindingProfile::Full));
    let err = dispatcher
        .call("findings.record", host_finding())
        .await
        .expect_err("summary-shaped call must be refused under full");
    assert!(err.to_string().contains("derived from `report`") || err.to_string().contains("`report` is required"), "{err}");
}
```

(If `dispatcher.call` returns `Ok` with an error string instead of `Err` on tool failure, mirror what `refuses_when_the_server_has_no_run_context` asserts in this same file.)

- [ ] **Step 2: Run the tests and check that they fail**

Run: `cargo test -p rupu-mcp --test findings_record`
Expected: compile error (`no field options on FindingsContext`).

- [ ] **Step 3: Implement**

In `crates/rupu-mcp/src/tools/findings.rs`:

Add to `FindingsContext`:

```rust
    /// Profile + artifact store + limits. For an `action:` step this is the
    /// workflow `defaults.findings_profile` (the dispatcher is built once per
    /// run); a step-level `findings_profile` on an action step is a parse error.
    pub options: rupu_coverage::FindingWriteOptions,
```

Change `input_schema` in `specs()`:
- `"required": ["scope"]`
- add `"report": rupu_coverage::report::schema::advertised_schema()` to `properties`. `json!` can't embed a function call inside the literal key map, so build it: `let mut schema = json!({ ... }); schema["properties"]["report"] = rupu_coverage::report::schema::advertised_schema();`
- extend the `description` with: `" Under the run's full findings profile send `report` (a complete finding report) and omit summary/severity/rationale; under the summary profile send summary, severity and rationale."`

Change `RecordArgs`:

```rust
#[derive(Debug, Deserialize)]
pub struct RecordArgs {
    pub scope: rupu_coverage::FindingScope,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub severity: Option<rupu_coverage::Severity>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    #[serde(default)]
    pub target_ref: Option<String>,
    #[serde(default)]
    pub code_excerpt: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub report: Option<rupu_coverage::FindingReport>,
}
```

In `dispatch_record`, build the input like this:

```rust
    let evidence = args.rationale.map(|rationale| rupu_coverage::FindingEvidence {
        code_excerpt: args.code_excerpt,
        rationale,
        references: args.references,
    });
    let input = rupu_coverage::ReportFindingInput {
        file_path: args.file_path,
        line_range: args.line_range,
        target_ref: args.target_ref,
        scope: args.scope,
        summary: args.summary,
        severity: args.severity,
        concern_id: args.concern_id,
        evidence,
        report: args.report,
    };
    rupu_coverage::report_finding(&paths, attribution, input, &ctx.options)
        .map(|out| out.id)
        .map_err(|e| e.to_string())
```

(Under the summary profile, a missing `rationale` gives `evidence: None` → `MissingField("evidence")`. Map that message for this tool: `.map_err(|e| e.to_string().replace("`evidence`", "`rationale`"))`.)

Fix the three `FindingsContext { .. }` literals in `rupu-cli` by adding `options: rupu_coverage::FindingWriteOptions::default(),` for now. Task 9 replaces them with real values.

- [ ] **Step 4: Run the tests and check that they pass**

Run: `cargo test -p rupu-mcp`
Expected: PASS. Also run `cargo test -p rupu-mcp --test schema_snapshot`: the `findings.record` input schema changed, so review the insta snapshot diff, confirm it is the intended change, and accept it with `cargo insta accept -p rupu-mcp` (or `INSTA_UPDATE=always`).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-mcp crates/rupu-cli
git commit -m "feat(mcp): findings.record accepts a full report and enforces the run's profile

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Workflow `findings_profile`, resolution in the step factory, CLI wiring

**Files:**
- Modify: `crates/rupu-orchestrator/src/workflow.rs` (`WorkflowDefaults`, `Step`, `WorkflowParseError`, `validate_step_shape`)
- Modify: `crates/rupu-orchestrator/src/step_factory.rs` (`DefaultStepFactory.findings_base`, resolution, `ToolContext.findings`)
- Create: `crates/rupu-cli/src/findings_opts.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (module), `cmd/run.rs`, `cmd/dispatch.rs`, `cmd/session.rs`, `cmd/workflow.rs`, `resume.rs`, plus every `DefaultStepFactory { .. }` literal

**Interfaces:**
- Consumes: `FindingProfile::resolve`, `FindingWriteOptions`, `rupu_config::FindingsConfig`, `AgentSpec.findings_profile`.
- Produces:
  - `WorkflowDefaults.findings_profile: Option<FindingProfile>`, `Step.findings_profile: Option<FindingProfile>`.
  - `WorkflowParseError::FindingsProfileOnActionStep { step }`.
  - `DefaultStepFactory.findings_base: FindingWriteOptions`.
  - `rupu_cli::findings_opts::base_options(global: &Path, cfg: &rupu_config::FindingsConfig) -> FindingWriteOptions` (profile `Full`; callers set the resolved profile).

- [ ] **Step 1: Write the failing tests**

Add to the test module of `crates/rupu-orchestrator/src/workflow.rs` (next to the existing parse tests):

```rust
    #[test]
    fn parses_findings_profile_on_defaults_and_step() {
        let wf = Workflow::parse(
            "name: w\ndefaults:\n  findings_profile: summary\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n    findings_profile: full\n",
        )
        .unwrap();
        assert_eq!(wf.defaults.findings_profile, Some(rupu_coverage::FindingProfile::Summary));
        assert_eq!(wf.steps[0].findings_profile, Some(rupu_coverage::FindingProfile::Full));
    }

    #[test]
    fn findings_profile_on_an_action_step_is_rejected() {
        let err = Workflow::parse(
            "name: w\nsteps:\n  - id: a\n    action: findings.record\n    with: {}\n    findings_profile: summary\n",
        )
        .unwrap_err();
        assert!(matches!(err, WorkflowParseError::FindingsProfileOnActionStep { .. }), "{err}");
    }
```

(If `Workflow::parse` validates `action:` tools against a catalog that rejects `findings.record` in a unit-test context, pick any tool name the existing action-step parse tests in this file use.)

Add to `crates/rupu-orchestrator/src/step_factory.rs`'s test module (it already has `factory(global)`, `write_agent(global)`, and the `WF` workflow used by `bash_config_reaches_the_step_opts`). First add `findings_base: rupu_coverage::FindingWriteOptions::default(),` to the existing `factory()` helper's literal. Then add:

```rust
    const WF_FINDINGS: &str = r#"
name: findings-wf
defaults:
  findings_profile: full
steps:
  - id: overridden
    agent: fp
    prompt: p
    findings_profile: summary
  - id: inherits
    agent: fp
    prompt: p
"#;

    const WF_NO_DEFAULT: &str = r#"
name: findings-wf-2
steps:
  - id: agent_decides
    agent: fp
    prompt: p
"#;

    fn write_summary_agent(global: &std::path::Path) {
        let agents_dir = global.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("fp.md"),
            "---\nname: fp\ntools: [report_finding]\nfindingsProfile: summary\n---\nAssess.\n",
        )
        .unwrap();
    }

    async fn profile_for(wf: &str, step: &str, global: &std::path::Path) -> rupu_coverage::FindingProfile {
        let mut f = factory(global.to_path_buf());
        f.workflow = Workflow::parse(wf).expect("workflow must parse");
        let opts = f
            .build_opts_for_step(
                step,
                "fp",
                "prompt".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                global.to_path_buf(),
                global.join(format!("{step}.jsonl")),
                None,
            )
            .await;
        opts.tool_context
            .findings
            .expect("step factory must always set findings options")
            .profile
    }

    #[tokio::test]
    async fn findings_profile_resolves_step_then_defaults_then_agent() {
        use rupu_coverage::FindingProfile::{Full, Summary};
        let tmp = assert_fs::TempDir::new().unwrap();
        write_summary_agent(tmp.path());

        // Step override beats the workflow default.
        assert_eq!(profile_for(WF_FINDINGS, "overridden", tmp.path()).await, Summary);
        // Workflow default beats the agent's `findingsProfile: summary`.
        assert_eq!(profile_for(WF_FINDINGS, "inherits", tmp.path()).await, Full);
        // With neither, the agent's frontmatter decides.
        assert_eq!(profile_for(WF_NO_DEFAULT, "agent_decides", tmp.path()).await, Summary);
    }

    #[tokio::test]
    async fn findings_base_limits_reach_the_step() {
        let tmp = assert_fs::TempDir::new().unwrap();
        write_summary_agent(tmp.path());
        let mut f = factory(tmp.path().to_path_buf());
        f.workflow = Workflow::parse(WF_NO_DEFAULT).unwrap();
        f.findings_base = rupu_coverage::FindingWriteOptions {
            artifact_root: Some(tmp.path().join("store")),
            artifact_max_bytes: 7,
            ..Default::default()
        };
        let opts = f
            .build_opts_for_step(
                "agent_decides",
                "fp",
                "p".to_string(),
                "run1".to_string(),
                "ws1".to_string(),
                tmp.path().to_path_buf(),
                tmp.path().join("t.jsonl"),
                None,
            )
            .await;
        let fo = opts.tool_context.findings.unwrap();
        assert_eq!(fo.artifact_max_bytes, 7);
        assert_eq!(fo.artifact_root, Some(tmp.path().join("store")));
    }
```


- [ ] **Step 2: Run the tests and check that they fail**

Run: `cargo test -p rupu-orchestrator findings_profile`
Expected: compile errors.

- [ ] **Step 3: Workflow fields + parse check**

In `workflow.rs`:

Add to `WorkflowDefaults` after `workspace`:

```rust
    /// Findings contract for every step unless a step overrides it:
    /// `full` (complete report) or `summary`. Absent ⇒ the agent's
    /// `findingsProfile`, else `full`. Action steps always use this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
```

Add to `Step` after `workspace`:

```rust
    /// Per-step findings contract override (step → workflow defaults →
    /// agent `findingsProfile` → `full`). Not allowed on an `action:` step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
```

Fix `Step { .. }` literals flagged by the compiler (`findings_profile: None,`).

Add to `WorkflowParseError`:

```rust
    #[error("step `{step}`: `findings_profile` is not supported on an `action:` step; set `defaults.findings_profile` instead")]
    FindingsProfileOnActionStep { step: String },
```

At the end of `validate_step_shape`, before the final `Ok(())`:

```rust
    if step.action.is_some() && step.findings_profile.is_some() {
        return Err(WorkflowParseError::FindingsProfileOnActionStep {
            step: step.id.clone(),
        });
    }
```

- [ ] **Step 4: Resolve in the step factory**

In `step_factory.rs`, add to `DefaultStepFactory`:

```rust
    /// Artifact store + limits for recording findings; the per-step profile
    /// is resolved in `build_opts_for_step` and set on a clone of this.
    pub findings_base: rupu_coverage::FindingWriteOptions,
```

In `build_opts_for_step`, right after `let (spec, load_err) = resolve_step_agent_spec(...)`, add:

```rust
        let findings = rupu_coverage::FindingWriteOptions {
            profile: rupu_coverage::FindingProfile::resolve(
                step.findings_profile,
                self.workflow.defaults.findings_profile,
                spec.findings_profile,
            ),
            ..self.findings_base.clone()
        };
```

In the `ToolContext { .. }` literal inside `AgentRunOpts`, replace `findings: None,` with `findings: Some(findings),`.

Fix every `DefaultStepFactory { .. }` literal:

Run: `cargo build --workspace --tests 2>&1 | grep -A2 "missing field \`findings_base\`" | grep -- "-->"`

Tests get `findings_base: rupu_coverage::FindingWriteOptions::default(),`. Production literals (in `rupu-cli`) are set in step 5.

- [ ] **Step 5: CLI wiring**

Create `crates/rupu-cli/src/findings_opts.rs`:

```rust
//! Build the findings write options every CLI entry point hands to tools.

use rupu_coverage::FindingWriteOptions;
use std::path::Path;

/// Artifact store under the global rupu dir plus the `[findings]` limits.
/// Profile is `full`; callers set the resolved profile.
pub fn base_options(global: &Path, cfg: &rupu_config::FindingsConfig) -> FindingWriteOptions {
    let d = FindingWriteOptions::default();
    FindingWriteOptions {
        artifact_root: Some(global.join("findings").join("artifacts")),
        artifact_max_bytes: cfg.artifact_max_bytes.unwrap_or(d.artifact_max_bytes),
        report_max_bytes: cfg
            .report_max_bytes
            .map(|b| b as usize)
            .unwrap_or(d.report_max_bytes),
        ticket_patterns: cfg.ticket_patterns.clone(),
        ..d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let g = Path::new("/tmp/rupu-home");
        let o = base_options(g, &rupu_config::FindingsConfig::default());
        assert_eq!(o.artifact_root.as_deref(), Some(Path::new("/tmp/rupu-home/findings/artifacts")));
        assert_eq!(o.artifact_max_bytes, rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES);
        let cfg = rupu_config::FindingsConfig { artifact_max_bytes: Some(10), ..Default::default() };
        assert_eq!(base_options(g, &cfg).artifact_max_bytes, 10);
    }
}
```

Register it in `crates/rupu-cli/src/lib.rs` (`pub mod findings_opts;` next to the other top-level modules).

Then set real values at each production site. `global` is whatever global-dir path the site already has (`paths::global_dir()?` or a local `global`), and `cfg` is the resolved `rupu_config::Config` it already loads:

- `cmd/run.rs` (standalone agent): where `tool_context` is built, set
  `findings: Some(crate::findings_opts::base_options(&global, &cfg.findings).with_profile(rupu_coverage::FindingProfile::resolve(None, None, spec.findings_profile))),`
- `cmd/dispatch.rs` (sub-agent dispatch): same, using the **child** agent's `spec.findings_profile`.
- `cmd/session.rs:~7539`: same, using the session agent's spec.
- `cmd/workflow.rs` (two `DefaultStepFactory { .. }` / `FindingsContext { .. }` sites around 3234 and 4784) and `resume.rs:317`:
  - `DefaultStepFactory { .., findings_base: crate::findings_opts::base_options(&global, &cfg.findings) }`
  - `FindingsContext { .., options: crate::findings_opts::base_options(&global, &cfg.findings).with_profile(rupu_coverage::FindingProfile::resolve(None, workflow.defaults.findings_profile, None)) }`

Confirm no production `findings: None` remains in an agent-run path:

Run: `grep -rn "findings: None" crates/rupu-cli/src crates/rupu-orchestrator/src | grep -v test`
Expected: no output. Any hit is a caller silently falling back to `full` with no artifact store.

- [ ] **Step 6: Run the tests and check that they pass**

Run: `cargo test -p rupu-orchestrator && cargo test -p rupu-cli findings_opts && cargo build --workspace`
Expected: PASS / builds.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-orchestrator crates/rupu-cli
git commit -m "feat(orchestrator): findings_profile on workflow defaults/steps, resolved per step

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: `rupu findings schema`

**Files:**
- Create: `crates/rupu-cli/src/cmd/findings.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs`, `crates/rupu-cli/src/lib.rs` (`Cmd` enum, dispatch, `ensure_output_format`)
- Create: `crates/rupu-cli/tests/findings_schema.rs`

**Interfaces:**
- Produces: `rupu findings schema [--advertised]`, which prints the canonical schema (or the provider-safe copy) as pretty JSON to stdout. Plan 3 adds `export` to this same subcommand.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/findings_schema.rs`, following how the other `crates/rupu-cli/tests/*.rs` invoke the binary. Check one, e.g. `grep -ln "assert_cmd\|Command::cargo_bin" crates/rupu-cli/tests | head -1`, and use the same helper:

```rust
use assert_cmd::Command;

#[test]
fn prints_the_embedded_schema() {
    let out = Command::cargo_bin("rupu").unwrap().args(["findings", "schema"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["title"], "rupu finding report");
    assert!(v["properties"]["regression_test"]["oneOf"].is_array());
}

#[test]
fn advertised_flag_prints_the_provider_safe_copy() {
    let out = Command::cargo_bin("rupu").unwrap().args(["findings", "schema", "--advertised"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("\"oneOf\""));
    assert!(text.contains("\"anyOf\""));
}
```

(If the crate's tests don't use `assert_cmd`, use the harness they do use. Don't add a dependency.)

- [ ] **Step 2: Run the test and check that it fails**

Run: `cargo test -p rupu-cli --test findings_schema`
Expected: FAIL (unrecognized subcommand `findings`).

- [ ] **Step 3: Implement**

Create `crates/rupu-cli/src/cmd/findings.rs`:

```rust
//! `rupu findings` — the finding report contract (and, in a later plan,
//! report exports). Thin: delegates to `rupu_coverage::report`.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Print the finding report JSON Schema embedded in this build.
    Schema {
        /// Print the simplified copy used in tool definitions instead.
        #[arg(long)]
        advertised: bool,
    },
}

pub async fn handle(action: Action) -> anyhow::Result<()> {
    match action {
        Action::Schema { advertised } => {
            let v = if advertised {
                rupu_coverage::report::schema::advertised_schema()
            } else {
                rupu_coverage::report::schema::canonical_schema()
            };
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
    }
}
```

In `cmd/mod.rs`: `pub mod findings;` (alphabetical position).

In `lib.rs`:
- in `enum Cmd`, after `Coverage { .. }`:

```rust
    /// Finding reports: the embedded report schema.
    Findings {
        #[command(subcommand)]
        action: cmd::findings::Action,
    },
```

- in the dispatch `match`, next to `Cmd::Coverage { action } => ...`: `Cmd::Findings { action } => cmd::findings::handle(action).await,`
- in `ensure_output_format`'s `match`, next to the `Coverage` arm:

```rust
        Cmd::Findings { .. } => output::formats::ensure_supported(
            "findings",
            format,
            &[output::formats::OutputFormat::Table],
        ),
```

(Match whatever return type the neighbouring arms use. If `handle` returns a different error type in this crate, mirror `cmd::webhook::handle`'s signature.)

- [ ] **Step 4: Run the tests and check that they pass**

Run: `cargo test -p rupu-cli --test findings_schema`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli
git commit -m "feat(cli): rupu findings schema prints the embedded finding report schema

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Docs, sample agent, fixtures, and the full gate

**Files:**
- Modify: `docs/agent-format.md` (document `findingsProfile`)
- Modify: `docs/workflow-format.md` (document `defaults.findings_profile` / step `findings_profile`, and the action-step rule)
- Modify: `docs/coverage.md` (report_finding profiles, artifacts, `rupu findings schema`)
- Modify: `examples/agents/security-assessor.md` (add `findingsProfile: full` with a one-line comment; remove any instruction telling it to write a separate report file)
- Modify: `CLAUDE.md` (a `rupu-coverage` line in the Crates list, and the Read-first entries for the spec + plans)
- Regenerate: `apps/rupu-macos/Fixtures/*.json`

- [ ] **Step 1: Document**

`docs/agent-format.md`: add a `findingsProfile` row/section with the text: "`full` (default) — findings must include a complete `report` (see `rupu findings schema`); `summary` — the lightweight summary/severity/evidence record. A workflow step's `findings_profile` or the workflow's `defaults.findings_profile` overrides this."

`docs/workflow-format.md`: document both keys, the precedence (step → defaults → agent → `full`), that `parallel:` sub-steps inherit their parent step's value, and that `action:` steps use `defaults.findings_profile` and reject a step-level value.

`docs/coverage.md`: a "Finding reports" section covering what `full` requires, the sentinels, artifacts (copied ≤ `[findings].artifact_max_bytes`, default 500 MB, content-addressed under `<RUPU_HOME>/findings/artifacts`; larger files recorded by hash), and `rupu findings schema`.

`CLAUDE.md`: under Crates add

```
- **`rupu-coverage`** — coverage ledgers + concern catalogs + the findings write path. `report/` owns the structured finding report (spec `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md`): typed `FindingReport`, embedded draft-07 schema (`schema/finding_report.schema.json`, `rupu findings schema`) kept in lockstep with `validate_report` by `tests/report_schema_lockstep.rs`, `full`/`summary` `FindingProfile` (step → workflow defaults → agent `findingsProfile` → `full`), and the content-addressed artifact store (`<RUPU_HOME>/findings/artifacts`).
```

and add the spec and the Plan 0 / Plan 1 paths to "Read first".

- [ ] **Step 2: Regenerate the macOS fixtures**

Run: `make macos-fixtures`
Expected: `apps/rupu-macos/Fixtures/findings*.json` gain a `"profile": "summary"` key (fixture records are legacy-shaped). Then run `cargo test -p rupu-cp`. Expected: PASS (fixture drift test green).

- [ ] **Step 3: The full gate**

Run each separately and check the output:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

```bash
make macos-test
```

Expected: all green. `macos-test` should be unaffected, because Swift decoding ignores the new keys, but run it: the fixture JSON changed.

- [ ] **Step 4: Commit and open the PR**

```bash
git add docs examples CLAUDE.md apps/rupu-macos/Fixtures
git commit -m "docs: finding report profiles, schema, artifacts; regenerate macOS fixtures

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

Push with an explicit refspec (bare `git push` pushes every matching branch in this repo), then open a PR. The PR body must call out:
- **Breaking behaviour:** the built-in default profile is `full`, so any existing agent that records thin findings (a `concerns:` block or `report_finding` in `tools:`) is now rejected unless it declares `findingsProfile: summary` or its workflow sets `findings_profile: summary`. Agents outside the repo (for example in `~/.rupu/agents/`) need that one-line change, or a prompt update to send `report`.
- The three spec deviations listed at the top of this plan.
- `findings.record`'s MCP schema snapshot changed intentionally.
