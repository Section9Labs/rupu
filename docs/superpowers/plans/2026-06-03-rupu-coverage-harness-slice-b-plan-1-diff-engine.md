# Coverage Harness Slice B — Plan 1: Run-Diff Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a run-to-run diff engine to `rupu-coverage` plus `rupu coverage diff` and `rupu coverage runs` CLI subcommands, so users can measure what changed between two runs against the same coverage target.

**Architecture:** Pure analysis over the three Slice A JSONL ledgers — no new instrumentation, no schema change. A `RunSelector` resolves (`latest` / `previous` / explicit `run_id`) to a run-id set; a `Contribution` reduces a run-id set to its `(concern, file) → status` cells, touched paths, and finding themes; `run_diff(base, compare)` set-differences two contributions into a `RunDiff`. Lives in a new `src/diff/` module mirroring the existing `src/audit/` module, re-exported at the crate root as `run_diff` / `list_runs` exactly like `run_audit`. CLI wiring is thin (arg parse + delegate + render), mirroring `run_audit_in`.

**Tech Stack:** Rust 2021, `thiserror` (library errors), `serde` / `serde_json`, `chrono`, `BTreeMap`/`BTreeSet` for deterministic ordering. Tests use `tempfile`.

**Spec:** `docs/superpowers/specs/2026-06-02-rupu-coverage-harness-slice-b-design.md` (Plan B-1 section).

---

## File Structure

**`rupu-coverage` crate:**
- `src/ledger/events.rs` *(modify)* — add `attribution()` + `at()` accessors to `FileTouchEvent` (the diff engine and runs-list both need run-id + timestamp per touch event; today only `path()` and `strength()` exist).
- `src/audit/generate.rs` *(modify)* — widen the existing private `theme_key` to `pub(crate)` so the diff engine reuses the exact same finding-theme primitive the audit's serendipitous clustering uses.
- `src/diff/mod.rs` *(create)* — module declarations + type re-exports (mirrors `src/audit/mod.rs`).
- `src/diff/types.rs` *(create)* — `RunDiff`, `CellRef`, `VerdictFlip`, `FindingThemeRef`, `RunListEntry`.
- `src/diff/generate.rs` *(create)* — `RunSelector`, `DiffError`, run ordering / selector resolution, `Contribution` builder, `run_diff`, `list_runs`.
- `src/lib.rs` *(modify)* — `pub mod diff;` + crate-root re-exports.

**`rupu-cli` crate:**
- `src/cmd/coverage.rs` *(modify)* — `Action::Diff` and `Action::Runs` variants, dispatch arms, `run_diff_in` + `run_runs_in` renderers (human + JSON).

---

## Task 1: `FileTouchEvent` accessors

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/events.rs` (impl block at lines 76-100)
- Test: same file, `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/rupu-coverage/src/ledger/events.rs`:

```rust
    #[test]
    fn file_touch_event_exposes_attribution_and_at() {
        let at = Utc::now();
        let ev = FileTouchEvent::Read {
            path: "src/a.rs".to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attribution(),
            at,
        };
        assert_eq!(ev.attribution().run_id, "run_01KS19A4MQXP");
        assert_eq!(ev.at(), at);

        // Unknown has no path but still carries attribution + timestamp.
        let unknown = FileTouchEvent::Unknown {
            tool: "mystery".to_string(),
            arg_hash: "deadbeef".to_string(),
            attribution: attribution(),
            at,
        };
        assert_eq!(unknown.attribution().model, "claude-sonnet-4-6");
        assert_eq!(unknown.at(), at);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage --lib ledger::events::tests::file_touch_event_exposes_attribution_and_at`
Expected: FAIL — `no method named 'attribution'` / `no method named 'at'`.

- [ ] **Step 3: Add the accessors**

In `crates/rupu-coverage/src/ledger/events.rs`, extend the existing `impl FileTouchEvent` block (after `path()`):

```rust
    pub fn attribution(&self) -> &Attribution {
        match self {
            FileTouchEvent::Read { attribution, .. }
            | FileTouchEvent::Grep { attribution, .. }
            | FileTouchEvent::Glob { attribution, .. }
            | FileTouchEvent::Edit { attribution, .. }
            | FileTouchEvent::Cmd { attribution, .. }
            | FileTouchEvent::Unknown { attribution, .. } => attribution,
        }
    }

    pub fn at(&self) -> DateTime<Utc> {
        match self {
            FileTouchEvent::Read { at, .. }
            | FileTouchEvent::Grep { at, .. }
            | FileTouchEvent::Glob { at, .. }
            | FileTouchEvent::Edit { at, .. }
            | FileTouchEvent::Cmd { at, .. }
            | FileTouchEvent::Unknown { at, .. } => *at,
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage --lib ledger::events::tests::file_touch_event_exposes_attribution_and_at`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/ledger/events.rs
git commit -m "feat(coverage): FileTouchEvent attribution() + at() accessors"
```

---

## Task 2: Diff types + module skeleton

**Files:**
- Create: `crates/rupu-coverage/src/diff/types.rs`
- Create: `crates/rupu-coverage/src/diff/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs` (add `pub mod diff;`)
- Test: in `crates/rupu-coverage/src/diff/types.rs`

- [ ] **Step 1: Create the module skeleton**

Create `crates/rupu-coverage/src/diff/mod.rs`:

```rust
//! Run-to-run diff over the Slice A coverage ledgers.
//!
//! Mirrors the `audit` module: pure analysis, re-exported at the crate
//! root (`run_diff`, `list_runs`). See the Slice B design spec.

pub mod generate;
pub mod types;

pub use types::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
```

Add to `crates/rupu-coverage/src/lib.rs` after `pub mod catalog;` (keep modules alphabetical-ish; place after `pub mod catalog;`):

```rust
pub mod diff;
```

Create `crates/rupu-coverage/src/diff/generate.rs` as an empty placeholder so the module compiles (filled in Task 3+):

```rust
//! Run ordering, selector resolution, contribution building, and the
//! `run_diff` / `list_runs` entry points.
```

- [ ] **Step 2: Write the failing test**

Create `crates/rupu-coverage/src/diff/types.rs`:

```rust
use crate::ledger::events::AssertionStatus;
use serde::{Deserialize, Serialize};

/// A `(concern_id, file_path)` cell with the status a run gave it. Used
/// for the cell-coverage delta (newly / no-longer asserted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellRef {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
}

/// A `(concern_id, file_path)` cell whose verdict differs between the two
/// runs. `high_signal` is set for the `clean -> finding` transition (a
/// later run found something an earlier run called clean).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictFlip {
    pub concern_id: String,
    pub file_path: String,
    pub base_status: AssertionStatus,
    pub compare_status: AssertionStatus,
    pub high_signal: bool,
}

/// A finding matched across runs by `(concern_id, theme)` — the same
/// best-effort theme primitive the audit's serendipitous clustering uses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingThemeRef {
    pub concern_id: Option<String>,
    pub theme: String,
}

/// The result of `run_diff(base, compare)`. All vectors are sorted
/// deterministically so output is stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDiff {
    pub base_runs: Vec<String>,
    pub compare_runs: Vec<String>,
    pub newly_asserted: Vec<CellRef>,
    pub no_longer_asserted: Vec<CellRef>,
    pub verdict_flips: Vec<VerdictFlip>,
    pub findings_appeared: Vec<FindingThemeRef>,
    pub findings_disappeared: Vec<FindingThemeRef>,
    pub newly_touched: Vec<String>,
    pub no_longer_touched: Vec<String>,
}

impl RunDiff {
    /// True when the two contributions are identical across all four
    /// dimensions (no changes to report).
    pub fn is_empty(&self) -> bool {
        self.newly_asserted.is_empty()
            && self.no_longer_asserted.is_empty()
            && self.verdict_flips.is_empty()
            && self.findings_appeared.is_empty()
            && self.findings_disappeared.is_empty()
            && self.newly_touched.is_empty()
            && self.no_longer_touched.is_empty()
    }
}

/// One row of `rupu coverage runs` — a run with its identity and
/// contribution counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunListEntry {
    pub run_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub model: String,
    pub surface: crate::ledger::events::Surface,
    pub cells_asserted: usize,
    pub findings: usize,
    pub files_touched: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_diff_is_empty_when_all_dimensions_empty() {
        let diff = RunDiff {
            base_runs: vec!["a".into()],
            compare_runs: vec!["b".into()],
            newly_asserted: vec![],
            no_longer_asserted: vec![],
            verdict_flips: vec![],
            findings_appeared: vec![],
            findings_disappeared: vec![],
            newly_touched: vec![],
            no_longer_touched: vec![],
        };
        assert!(diff.is_empty());
    }

    #[test]
    fn run_diff_round_trips_json() {
        let diff = RunDiff {
            base_runs: vec!["run_a".into()],
            compare_runs: vec!["run_b".into()],
            newly_asserted: vec![CellRef {
                concern_id: "stride:spoofing".into(),
                file_path: "src/a.rs".into(),
                status: AssertionStatus::Clean,
            }],
            no_longer_asserted: vec![],
            verdict_flips: vec![VerdictFlip {
                concern_id: "stride:tampering".into(),
                file_path: "src/b.rs".into(),
                base_status: AssertionStatus::Clean,
                compare_status: AssertionStatus::Finding,
                high_signal: true,
            }],
            findings_appeared: vec![FindingThemeRef {
                concern_id: None,
                theme: "missing csrf token on".into(),
            }],
            findings_disappeared: vec![],
            newly_touched: vec!["src/c.rs".into()],
            no_longer_touched: vec![],
        };
        let json = serde_json::to_string(&diff).unwrap();
        let back: RunDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(diff, back);
        assert!(!diff.is_empty());
    }
}
```

- [ ] **Step 3: Run test to verify it fails, then passes**

Run: `cargo test -p rupu-coverage --lib diff::types`
Expected: compiles and PASSES (this task is pure type definitions + their tests; the "failing" state is the pre-edit compile error from `pub mod diff;` referencing not-yet-created files, which Steps 1-2 resolve together). If it fails to compile, fix the module paths before moving on.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage/src/diff/ crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): diff result types + module skeleton"
```

---

## Task 3: Run selector + ordering + resolution

**Files:**
- Modify: `crates/rupu-coverage/src/diff/generate.rs`
- Test: same file

- [ ] **Step 1: Write the failing test**

Replace the placeholder body of `crates/rupu-coverage/src/diff/generate.rs` with the imports, the `RunSelector` / `DiffError` types, the ordering/resolution functions, and this test module:

```rust
//! Run ordering, selector resolution, contribution building, and the
//! `run_diff` / `list_runs` entry points.

use crate::ledger::events::{ConcernAssertion, FileTouchEvent, FindingRecord};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Selects which run(s) a diff side refers to. v1 selectors each resolve
/// to exactly one run; the return type is a `Vec` so future `model:` /
/// `through:` selectors (sets of runs) feed the same engine unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSelector {
    RunId(String),
    Latest,
    Previous,
}

impl FromStr for RunSelector {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "latest" => RunSelector::Latest,
            "previous" => RunSelector::Previous,
            other => RunSelector::RunId(other.to_string()),
        })
    }
}

/// Errors from the diff / runs surface.
#[derive(Debug, thiserror::Error)]
pub enum DiffError {
    #[error("io error reading ledgers: {0}")]
    Io(#[from] std::io::Error),
    #[error("no run with id '{0}' on this target")]
    UnknownRun(String),
    #[error("no run matches '{0}'")]
    NoRunMatches(String),
}

/// Run ids ordered most-recent-first. "Recency" is the maximum timestamp
/// observed for a run across all three ledgers; ties break by run id
/// ascending for stability.
pub(crate) fn ordered_runs(
    files: &[FileTouchEvent],
    assertions: &[ConcernAssertion],
    findings: &[FindingRecord],
) -> Vec<String> {
    let mut max_at: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
    let mut bump = |run_id: &str, at: DateTime<Utc>| {
        max_at
            .entry(run_id.to_string())
            .and_modify(|cur| {
                if at > *cur {
                    *cur = at;
                }
            })
            .or_insert(at);
    };
    for f in files {
        bump(&f.attribution().run_id, f.at());
    }
    for a in assertions {
        bump(&a.declared_by.run_id, a.declared_at);
    }
    for f in findings {
        bump(&f.declared_by.run_id, f.declared_at);
    }
    let mut runs: Vec<(String, DateTime<Utc>)> = max_at.into_iter().collect();
    // Most-recent-first; ties broken by run id ascending.
    runs.sort_by(|(a_id, a_at), (b_id, b_at)| b_at.cmp(a_at).then(a_id.cmp(b_id)));
    runs.into_iter().map(|(id, _)| id).collect()
}

/// Resolve a selector against the recency-ordered run list. v1 returns a
/// single-element Vec.
pub(crate) fn resolve_selector(
    selector: &RunSelector,
    ordered: &[String],
) -> Result<Vec<String>, DiffError> {
    match selector {
        RunSelector::RunId(id) => {
            if ordered.iter().any(|r| r == id) {
                Ok(vec![id.clone()])
            } else {
                Err(DiffError::UnknownRun(id.clone()))
            }
        }
        RunSelector::Latest => ordered
            .first()
            .map(|r| vec![r.clone()])
            .ok_or_else(|| DiffError::NoRunMatches("latest".to_string())),
        RunSelector::Previous => ordered
            .get(1)
            .map(|r| vec![r.clone()])
            .ok_or_else(|| DiffError::NoRunMatches("previous".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, Surface};

    fn attribution(run_id: &str) -> Attribution {
        Attribution {
            run_id: run_id.to_string(),
            model: "m1".to_string(),
            surface: Surface::Session,
        }
    }

    fn read_event(run_id: &str, secs: i64) -> FileTouchEvent {
        FileTouchEvent::Read {
            path: "src/a.rs".to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attribution(run_id),
            at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        }
    }

    #[test]
    fn ordered_runs_is_most_recent_first() {
        let files = vec![read_event("run_old", 100), read_event("run_new", 200)];
        let ordered = ordered_runs(&files, &[], &[]);
        assert_eq!(ordered, vec!["run_new", "run_old"]);
    }

    #[test]
    fn ordered_runs_breaks_ties_by_run_id() {
        let files = vec![read_event("run_b", 100), read_event("run_a", 100)];
        let ordered = ordered_runs(&files, &[], &[]);
        assert_eq!(ordered, vec!["run_a", "run_b"]);
    }

    #[test]
    fn resolve_latest_and_previous() {
        let ordered = vec!["run_new".to_string(), "run_old".to_string()];
        assert_eq!(
            resolve_selector(&RunSelector::Latest, &ordered).unwrap(),
            vec!["run_new"]
        );
        assert_eq!(
            resolve_selector(&RunSelector::Previous, &ordered).unwrap(),
            vec!["run_old"]
        );
    }

    #[test]
    fn resolve_explicit_run_id() {
        let ordered = vec!["run_new".to_string(), "run_old".to_string()];
        assert_eq!(
            resolve_selector(&RunSelector::RunId("run_old".into()), &ordered).unwrap(),
            vec!["run_old"]
        );
    }

    #[test]
    fn resolve_unknown_run_id_errors() {
        let ordered = vec!["run_new".to_string()];
        let err = resolve_selector(&RunSelector::RunId("nope".into()), &ordered).unwrap_err();
        assert!(matches!(err, DiffError::UnknownRun(id) if id == "nope"));
    }

    #[test]
    fn resolve_previous_with_single_run_errors() {
        let ordered = vec!["only".to_string()];
        let err = resolve_selector(&RunSelector::Previous, &ordered).unwrap_err();
        assert!(matches!(err, DiffError::NoRunMatches(s) if s == "previous"));
    }

    #[test]
    fn resolve_latest_with_no_runs_errors() {
        let err = resolve_selector(&RunSelector::Latest, &[]).unwrap_err();
        assert!(matches!(err, DiffError::NoRunMatches(s) if s == "latest"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage --lib diff::generate::tests`
Expected: PASS after the implementation in Step 1 is in place (this task writes test + impl together because they share the file). If `DateTime::from_timestamp` is flagged, confirm `chrono` is in scope (it is, via the `use chrono::...` line).

- [ ] **Step 3: Verify the whole crate still builds**

Run: `cargo build -p rupu-coverage`
Expected: clean build (no unused-import warnings escalated to errors under `#![deny(clippy::all)]`). If `FindingRecord` / `ConcernAssertion` imports are unused at this stage, they ARE used by `ordered_runs` — keep them.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage/src/diff/generate.rs
git commit -m "feat(coverage): run selector parsing, ordering, and resolution"
```

---

## Task 4: Contribution builder

**Files:**
- Modify: `crates/rupu-coverage/src/audit/generate.rs` (widen `theme_key` visibility)
- Modify: `crates/rupu-coverage/src/diff/generate.rs`
- Test: `crates/rupu-coverage/src/diff/generate.rs`

- [ ] **Step 1: Widen `theme_key` to `pub(crate)`**

In `crates/rupu-coverage/src/audit/generate.rs`, change the existing signature:

```rust
fn theme_key(summary: &str) -> String {
```

to:

```rust
pub(crate) fn theme_key(summary: &str) -> String {
```

(Body unchanged — it takes the first six whitespace-split words, lowercased.)

- [ ] **Step 2: Write the failing test**

Add to the `tests` module in `crates/rupu-coverage/src/diff/generate.rs` (extend the existing imports line with `AssertionStatus`, `ConcernAssertion`, `Evidence`, `FindingRecord`, `FindingEvidence`, `FindingScope`, and `Severity` as shown):

```rust
    use crate::catalog::types::Severity;
    use crate::ledger::events::{
        AssertionStatus, ConcernAssertion, Evidence, FindingEvidence, FindingRecord, FindingScope,
    };

    fn assertion(run: &str, concern: &str, file: &str, status: AssertionStatus, secs: i64) -> ConcernAssertion {
        ConcernAssertion {
            concern_id: concern.to_string(),
            file_path: file.to_string(),
            status,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attribution(run),
            declared_at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        }
    }

    fn finding(run: &str, concern: Option<&str>, summary: &str) -> FindingRecord {
        FindingRecord {
            id: format!("find_{summary}"),
            file_path: None,
            line_range: None,
            scope: FindingScope::File,
            summary: summary.to_string(),
            severity: Severity::Medium,
            concern_id: concern.map(|c| c.to_string()),
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".to_string(),
                references: vec![],
            },
            declared_by: attribution(run),
            declared_at: Utc::now(),
        }
    }

    #[test]
    fn contribution_collects_cells_touched_and_themes_for_run_set() {
        let runs: std::collections::BTreeSet<String> = ["run_a".to_string()].into_iter().collect();
        let files = vec![read_event("run_a", 100), read_event("run_b", 100)];
        // run_b's file event must NOT appear in run_a's contribution.
        let mut files = files;
        files.push(FileTouchEvent::Read {
            path: "src/only_a.rs".to_string(),
            line_range: [1, 5],
            tool: "read_file".to_string(),
            attribution: attribution("run_a"),
            at: DateTime::<Utc>::from_timestamp(101, 0).unwrap(),
        });
        let assertions = vec![
            assertion("run_a", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            // later assertion in the same run supersedes the earlier one
            assertion("run_a", "c1", "src/a.rs", AssertionStatus::Finding, 200),
            assertion("run_b", "c2", "src/b.rs", AssertionStatus::Clean, 100),
        ];
        let findings = vec![
            finding("run_a", Some("c1"), "sql injection in login handler path"),
            finding("run_b", None, "unrelated finding from other run here"),
        ];
        let c = contribution(&runs, &files, &assertions, &findings);

        // Cell supersession: last status within the run wins.
        assert_eq!(
            c.cells.get(&("c1".to_string(), "src/a.rs".to_string())),
            Some(&AssertionStatus::Finding)
        );
        // run_b's cell is excluded.
        assert!(!c.cells.contains_key(&("c2".to_string(), "src/b.rs".to_string())));
        // Touched paths come only from run_a.
        assert!(c.touched.contains("src/a.rs"));
        assert!(c.touched.contains("src/only_a.rs"));
        // Finding themes are (concern_id, theme_key); run_b's is excluded.
        assert!(c
            .finding_themes
            .contains(&(Some("c1".to_string()), theme_key("sql injection in login handler path"))));
        assert_eq!(c.finding_themes.len(), 1);
    }
```

- [ ] **Step 3: Implement `Contribution` + `contribution`**

Add to `crates/rupu-coverage/src/diff/generate.rs` (above the `#[cfg(test)]` module), and add `use crate::audit::generate::theme_key;` and `use crate::ledger::events::AssertionStatus;` and `use std::collections::BTreeSet;` to the imports at the top:

```rust
use crate::audit::generate::theme_key;
use crate::ledger::events::AssertionStatus;
use std::collections::BTreeSet;

/// One run set's contribution to a target, reduced for diffing.
pub(crate) struct Contribution {
    /// `(concern_id, file_path) -> last status` for assertions by these runs.
    pub cells: BTreeMap<(String, String), AssertionStatus>,
    /// File paths touched by these runs.
    pub touched: BTreeSet<String>,
    /// `(concern_id, theme_key(summary))` for findings by these runs.
    pub finding_themes: BTreeSet<(Option<String>, String)>,
}

/// Build a contribution from the ledgers, restricted to `runs`. Cell
/// supersession matches the audit: assertions are applied in timestamp
/// order so the last write within the run set wins.
pub(crate) fn contribution(
    runs: &BTreeSet<String>,
    files: &[FileTouchEvent],
    assertions: &[ConcernAssertion],
    findings: &[FindingRecord],
) -> Contribution {
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for f in files.iter().filter(|f| runs.contains(&f.attribution().run_id)) {
        if let Some(path) = f.path() {
            touched.insert(path.to_string());
        }
    }

    let mut sorted: Vec<&ConcernAssertion> = assertions
        .iter()
        .filter(|a| runs.contains(&a.declared_by.run_id))
        .collect();
    sorted.sort_by(|a, b| a.declared_at.cmp(&b.declared_at));
    let mut cells: BTreeMap<(String, String), AssertionStatus> = BTreeMap::new();
    for a in sorted {
        cells.insert((a.concern_id.clone(), a.file_path.clone()), a.status);
    }

    let mut finding_themes: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    for f in findings.iter().filter(|f| runs.contains(&f.declared_by.run_id)) {
        finding_themes.insert((f.concern_id.clone(), theme_key(&f.summary)));
    }

    Contribution {
        cells,
        touched,
        finding_themes,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rupu-coverage --lib diff::generate::tests::contribution_collects_cells_touched_and_themes_for_run_set`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/audit/generate.rs crates/rupu-coverage/src/diff/generate.rs
git commit -m "feat(coverage): per-run contribution builder (cells, touches, finding themes)"
```

---

## Task 5: `run_diff`

**Files:**
- Modify: `crates/rupu-coverage/src/diff/generate.rs`
- Modify: `crates/rupu-coverage/src/lib.rs` (re-export)
- Test: `crates/rupu-coverage/src/diff/generate.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/rupu-coverage/src/diff/generate.rs`. This builds two runs in a temp ledger and diffs them:

```rust
    use crate::ledger::paths::CoveragePaths;

    fn write_ledgers(
        paths: &CoveragePaths,
        files: &[FileTouchEvent],
        assertions: &[ConcernAssertion],
        findings: &[FindingRecord],
    ) {
        paths.ensure_dir().unwrap();
        let f: String = files
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.files, f).unwrap();
        let a: String = assertions
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.concerns, a).unwrap();
        let fi: String = findings
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&paths.findings, fi).unwrap();
    }

    #[test]
    fn run_diff_reports_all_four_dimensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");

        // run_old (older): asserts (c1, a.rs)=clean and (c2, b.rs)=clean,
        //   touches a.rs + b.rs, finding theme "alpha".
        // run_new (newer): asserts (c1, a.rs)=finding [FLIP, high-signal],
        //   asserts (c3, c.rs)=clean [NEW], drops (c2, b.rs) [NO LONGER],
        //   touches a.rs + c.rs (drops b.rs), finding theme "beta" [APPEARED],
        //   loses theme "alpha" [DISAPPEARED].
        let files = vec![
            read_event("run_old", 100), // a.rs
            FileTouchEvent::Read {
                path: "src/b.rs".to_string(),
                line_range: [1, 5],
                tool: "read_file".to_string(),
                attribution: attribution("run_old"),
                at: DateTime::<Utc>::from_timestamp(101, 0).unwrap(),
            },
            read_event("run_new", 200), // a.rs
            FileTouchEvent::Read {
                path: "src/c.rs".to_string(),
                line_range: [1, 5],
                tool: "read_file".to_string(),
                attribution: attribution("run_new"),
                at: DateTime::<Utc>::from_timestamp(201, 0).unwrap(),
            },
        ];
        let assertions = vec![
            assertion("run_old", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            assertion("run_old", "c2", "src/b.rs", AssertionStatus::Clean, 101),
            assertion("run_new", "c1", "src/a.rs", AssertionStatus::Finding, 200),
            assertion("run_new", "c3", "src/c.rs", AssertionStatus::Clean, 201),
        ];
        let findings = vec![
            finding("run_old", Some("c1"), "alpha alpha alpha alpha alpha alpha"),
            finding("run_new", Some("c1"), "beta beta beta beta beta beta"),
        ];
        write_ledgers(&paths, &files, &assertions, &findings);

        let diff = run_diff(&paths, &RunSelector::Previous, &RunSelector::Latest).unwrap();

        assert_eq!(diff.base_runs, vec!["run_old"]);
        assert_eq!(diff.compare_runs, vec!["run_new"]);

        // Cell-coverage delta.
        assert!(diff.newly_asserted.iter().any(|c| c.concern_id == "c3" && c.file_path == "src/c.rs"));
        assert!(diff.no_longer_asserted.iter().any(|c| c.concern_id == "c2" && c.file_path == "src/b.rs"));

        // Verdict flip, high-signal.
        let flip = diff.verdict_flips.iter().find(|f| f.concern_id == "c1").unwrap();
        assert_eq!(flip.base_status, AssertionStatus::Clean);
        assert_eq!(flip.compare_status, AssertionStatus::Finding);
        assert!(flip.high_signal);

        // Findings appeared / disappeared.
        assert!(diff.findings_appeared.iter().any(|f| f.theme.starts_with("beta")));
        assert!(diff.findings_disappeared.iter().any(|f| f.theme.starts_with("alpha")));

        // File-touch delta.
        assert!(diff.newly_touched.contains(&"src/c.rs".to_string()));
        assert!(diff.no_longer_touched.contains(&"src/b.rs".to_string()));

        assert!(!diff.is_empty());
    }

    #[test]
    fn run_diff_identical_runs_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        let files = vec![read_event("run_old", 100), read_event("run_new", 200)];
        let assertions = vec![
            assertion("run_old", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            assertion("run_new", "c1", "src/a.rs", AssertionStatus::Clean, 200),
        ];
        write_ledgers(&paths, &files, &assertions, &[]);
        let diff = run_diff(&paths, &RunSelector::Previous, &RunSelector::Latest).unwrap();
        assert!(diff.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage --lib diff::generate::tests::run_diff_reports_all_four_dimensions`
Expected: FAIL — `cannot find function 'run_diff'`.

- [ ] **Step 3: Implement `run_diff`**

Add to `crates/rupu-coverage/src/diff/generate.rs` (above the test module). Add `use crate::diff::types::{CellRef, FindingThemeRef, RunDiff, VerdictFlip};` and `use crate::ledger::paths::CoveragePaths;` and `use crate::ledger::views::{read_concern_assertions, read_file_events, read_findings};` to the top imports:

```rust
use crate::diff::types::{CellRef, FindingThemeRef, RunDiff, VerdictFlip};
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{read_concern_assertions, read_file_events, read_findings};

/// Diff two run selectors against a target's ledgers. `base` is the
/// earlier reference; `compare` is the run under inspection.
pub fn run_diff(
    paths: &CoveragePaths,
    base: &RunSelector,
    compare: &RunSelector,
) -> Result<RunDiff, DiffError> {
    let files = read_file_events(paths)?;
    let assertions = read_concern_assertions(paths)?;
    let findings = read_findings(paths)?;

    let ordered = ordered_runs(&files, &assertions, &findings);
    let base_runs = resolve_selector(base, &ordered)?;
    let compare_runs = resolve_selector(compare, &ordered)?;

    let base_set: BTreeSet<String> = base_runs.iter().cloned().collect();
    let compare_set: BTreeSet<String> = compare_runs.iter().cloned().collect();
    let b = contribution(&base_set, &files, &assertions, &findings);
    let c = contribution(&compare_set, &files, &assertions, &findings);

    // Cell-coverage delta.
    let mut newly_asserted: Vec<CellRef> = c
        .cells
        .iter()
        .filter(|(k, _)| !b.cells.contains_key(*k))
        .map(|((concern_id, file_path), status)| CellRef {
            concern_id: concern_id.clone(),
            file_path: file_path.clone(),
            status: *status,
        })
        .collect();
    let mut no_longer_asserted: Vec<CellRef> = b
        .cells
        .iter()
        .filter(|(k, _)| !c.cells.contains_key(*k))
        .map(|((concern_id, file_path), status)| CellRef {
            concern_id: concern_id.clone(),
            file_path: file_path.clone(),
            status: *status,
        })
        .collect();

    // Verdict flips: cells in both with a changed status.
    let mut verdict_flips: Vec<VerdictFlip> = b
        .cells
        .iter()
        .filter_map(|(k, base_status)| {
            c.cells.get(k).and_then(|compare_status| {
                if base_status != compare_status {
                    Some(VerdictFlip {
                        concern_id: k.0.clone(),
                        file_path: k.1.clone(),
                        base_status: *base_status,
                        compare_status: *compare_status,
                        high_signal: *base_status == AssertionStatus::Clean
                            && *compare_status == AssertionStatus::Finding,
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    // Finding themes appeared / disappeared.
    let mut findings_appeared: Vec<FindingThemeRef> = c
        .finding_themes
        .difference(&b.finding_themes)
        .map(|(concern_id, theme)| FindingThemeRef {
            concern_id: concern_id.clone(),
            theme: theme.clone(),
        })
        .collect();
    let mut findings_disappeared: Vec<FindingThemeRef> = b
        .finding_themes
        .difference(&c.finding_themes)
        .map(|(concern_id, theme)| FindingThemeRef {
            concern_id: concern_id.clone(),
            theme: theme.clone(),
        })
        .collect();

    // File-touch delta.
    let mut newly_touched: Vec<String> =
        c.touched.difference(&b.touched).cloned().collect();
    let mut no_longer_touched: Vec<String> =
        b.touched.difference(&c.touched).cloned().collect();

    // Deterministic ordering for stable output.
    let cell_key = |r: &CellRef| (r.concern_id.clone(), r.file_path.clone());
    newly_asserted.sort_by_key(cell_key);
    no_longer_asserted.sort_by_key(cell_key);
    verdict_flips.sort_by_key(|f| (f.concern_id.clone(), f.file_path.clone()));
    let theme_key_sort = |r: &FindingThemeRef| (r.concern_id.clone(), r.theme.clone());
    findings_appeared.sort_by_key(theme_key_sort);
    findings_disappeared.sort_by_key(theme_key_sort);
    newly_touched.sort();
    no_longer_touched.sort();

    Ok(RunDiff {
        base_runs,
        compare_runs,
        newly_asserted,
        no_longer_asserted,
        verdict_flips,
        findings_appeared,
        findings_disappeared,
        newly_touched,
        no_longer_touched,
    })
}
```

- [ ] **Step 4: Re-export from the crate root**

In `crates/rupu-coverage/src/lib.rs`, add after the `pub use audit::generate::audit as run_audit;` line:

```rust
pub use diff::generate::{list_runs, run_diff, DiffError, RunSelector};
pub use diff::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
```

(`list_runs` lands in Task 6; if this task's build fails on the missing `list_runs`, temporarily drop it from the re-export and add it back in Task 6. Cleaner: defer the whole `pub use diff::generate::{...}` line to include `list_runs` only after Task 6 — for now export `pub use diff::generate::{run_diff, DiffError, RunSelector};`.)

Use this for now:

```rust
pub use diff::generate::{run_diff, DiffError, RunSelector};
pub use diff::{CellRef, FindingThemeRef, RunDiff, RunListEntry, VerdictFlip};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-coverage --lib diff::`
Expected: PASS (all diff tests, including the two new `run_diff` tests).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-coverage/src/diff/generate.rs crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): run_diff — four-dimension run-to-run comparison"
```

---

## Task 6: `list_runs`

**Files:**
- Modify: `crates/rupu-coverage/src/diff/generate.rs`
- Modify: `crates/rupu-coverage/src/lib.rs` (add `list_runs` to re-export)
- Test: `crates/rupu-coverage/src/diff/generate.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/rupu-coverage/src/diff/generate.rs`:

```rust
    #[test]
    fn list_runs_reports_counts_most_recent_first() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        let files = vec![read_event("run_old", 100), read_event("run_new", 200)];
        let assertions = vec![
            assertion("run_old", "c1", "src/a.rs", AssertionStatus::Clean, 100),
            assertion("run_new", "c1", "src/a.rs", AssertionStatus::Finding, 200),
            assertion("run_new", "c2", "src/b.rs", AssertionStatus::Clean, 201),
        ];
        let findings = vec![finding("run_new", Some("c1"), "something something here now ok yes")];
        write_ledgers(&paths, &files, &assertions, &findings);

        let runs = list_runs(&paths).unwrap();
        assert_eq!(runs.len(), 2);
        // Most-recent-first.
        assert_eq!(runs[0].run_id, "run_new");
        assert_eq!(runs[1].run_id, "run_old");
        // Counts for run_new: 2 cells, 1 finding, 1 file touched.
        assert_eq!(runs[0].cells_asserted, 2);
        assert_eq!(runs[0].findings, 1);
        assert_eq!(runs[0].files_touched, 1);
        // started_at is the earliest activity timestamp for the run.
        assert_eq!(runs[0].started_at, DateTime::<Utc>::from_timestamp(200, 0).unwrap());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-coverage --lib diff::generate::tests::list_runs_reports_counts_most_recent_first`
Expected: FAIL — `cannot find function 'list_runs'`.

- [ ] **Step 3: Implement `list_runs`**

Add to `crates/rupu-coverage/src/diff/generate.rs`. Add `use crate::diff::types::RunListEntry;` and `use crate::ledger::events::Surface;` to the top imports:

```rust
use crate::diff::types::RunListEntry;
use crate::ledger::events::Surface;

/// List every run on a target with its identity and contribution counts,
/// most-recent-first.
pub fn list_runs(paths: &CoveragePaths) -> Result<Vec<RunListEntry>, DiffError> {
    let files = read_file_events(paths)?;
    let assertions = read_concern_assertions(paths)?;
    let findings = read_findings(paths)?;
    let ordered = ordered_runs(&files, &assertions, &findings);

    let mut out = Vec::with_capacity(ordered.len());
    for run_id in ordered {
        let single: BTreeSet<String> = [run_id.clone()].into_iter().collect();
        let c = contribution(&single, &files, &assertions, &findings);

        // Identity (model, surface) and earliest timestamp from any of the
        // run's ledger rows. Every row for a run carries the same model +
        // surface, so the first match is representative.
        let mut model = String::new();
        let mut surface = Surface::Session;
        let mut started_at: Option<DateTime<Utc>> = None;
        let mut consider = |attr_run: &str, m: &str, s: Surface, at: DateTime<Utc>| {
            if attr_run == run_id {
                if model.is_empty() {
                    model = m.to_string();
                    surface = s;
                }
                started_at = Some(match started_at {
                    Some(cur) if cur <= at => cur,
                    _ => at,
                });
            }
        };
        for f in &files {
            let a = f.attribution();
            consider(&a.run_id, &a.model, a.surface, f.at());
        }
        for a in &assertions {
            consider(
                &a.declared_by.run_id,
                &a.declared_by.model,
                a.declared_by.surface,
                a.declared_at,
            );
        }
        for f in &findings {
            consider(
                &f.declared_by.run_id,
                &f.declared_by.model,
                f.declared_by.surface,
                f.declared_at,
            );
        }

        out.push(RunListEntry {
            run_id,
            started_at: started_at.unwrap_or_else(Utc::now),
            model,
            surface,
            cells_asserted: c.cells.len(),
            findings: c.finding_themes.len(),
            files_touched: c.touched.len(),
        });
    }
    Ok(out)
}
```

- [ ] **Step 4: Add `list_runs` to the crate re-export**

In `crates/rupu-coverage/src/lib.rs`, update the diff re-export line to:

```rust
pub use diff::generate::{list_runs, run_diff, DiffError, RunSelector};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-coverage --lib diff::`
Expected: PASS (all diff tests).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-coverage/src/diff/generate.rs crates/rupu-coverage/src/lib.rs
git commit -m "feat(coverage): list_runs — per-run identity + contribution counts"
```

---

## Task 7: CLI `coverage diff`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/coverage.rs` (Action enum ~lines 9-40, dispatch ~lines 55-67, new renderer fn)
- Test: `crates/rupu-cli/src/cmd/coverage.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Add the `Diff` subcommand variant**

In `crates/rupu-cli/src/cmd/coverage.rs`, add to the `Action` enum (after the `Gap` variant):

```rust
    /// Diff two runs against a target (defaults to `previous latest`).
    Diff {
        /// Target id (from `coverage list`).
        target_id: String,
        /// Base run selector: a run id, `latest`, or `previous`.
        base: Option<String>,
        /// Compare run selector: a run id, `latest`, or `previous`.
        compare: Option<String>,
        /// Emit machine-readable JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },
```

Add the dispatch arm in `handle` (after the `Gap` arm):

```rust
        Action::Diff {
            target_id,
            base,
            compare,
            json,
        } => workspace().and_then(|ws| run_diff_in(&ws, &target_id, base, compare, json)),
```

- [ ] **Step 2: Write the failing test**

Add to the `tests` module in `crates/rupu-cli/src/cmd/coverage.rs`:

```rust
    #[test]
    fn diff_on_two_run_target_json_and_human() {
        use rupu_coverage::{
            AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, Evidence,
            FileTouchEvent, Surface,
        };
        use chrono::{DateTime, Utc};

        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();

        let attr = |run: &str| Attribution {
            run_id: run.to_string(),
            model: "m".to_string(),
            surface: Surface::Session,
        };
        let read = |run: &str, path: &str, secs: i64| FileTouchEvent::Read {
            path: path.to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attr(run),
            at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        };
        let files = format!(
            "{}\n{}\n",
            serde_json::to_string(&read("run_old", "src/a.rs", 100)).unwrap(),
            serde_json::to_string(&read("run_new", "src/a.rs", 200)).unwrap(),
        );
        std::fs::write(&paths.files, files).unwrap();

        let mark = |run: &str, status: AssertionStatus, secs: i64| ConcernAssertion {
            concern_id: "c1".to_string(),
            file_path: "src/a.rs".to_string(),
            status,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attr(run),
            declared_at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        };
        let concerns = format!(
            "{}\n{}\n",
            serde_json::to_string(&mark("run_old", AssertionStatus::Clean, 100)).unwrap(),
            serde_json::to_string(&mark("run_new", AssertionStatus::Finding, 200)).unwrap(),
        );
        std::fs::write(&paths.concerns, concerns).unwrap();

        // Default selectors (previous latest), both output modes.
        assert!(run_diff_in(tmp.path(), "tgt", None, None, true).is_ok());
        assert!(run_diff_in(tmp.path(), "tgt", None, None, false).is_ok());
        // Explicit selectors.
        assert!(run_diff_in(
            tmp.path(),
            "tgt",
            Some("run_old".to_string()),
            Some("run_new".to_string()),
            false
        )
        .is_ok());
        // Only one selector supplied → error.
        assert!(run_diff_in(tmp.path(), "tgt", Some("run_old".to_string()), None, false).is_err());
        // Unknown selector → error.
        assert!(run_diff_in(
            tmp.path(),
            "tgt",
            Some("nope".to_string()),
            Some("run_new".to_string()),
            false
        )
        .is_err());
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rupu-cli --lib cmd::coverage::tests::diff_on_two_run_target_json_and_human`
Expected: FAIL — `cannot find function 'run_diff_in'`.

- [ ] **Step 4: Implement `run_diff_in`**

Add to `crates/rupu-cli/src/cmd/coverage.rs` (after `run_gap_in`):

```rust
fn run_diff_in(
    workspace: &Path,
    target_id: &str,
    base: Option<String>,
    compare: Option<String>,
    json: bool,
) -> Result<()> {
    let (base, compare) = match (base, compare) {
        (None, None) => ("previous".to_string(), "latest".to_string()),
        (Some(b), Some(c)) => (b, c),
        _ => anyhow::bail!("provide both base and compare run selectors, or neither"),
    };
    // RunSelector::from_str is infallible (any non-keyword is a run id).
    let base_sel: rupu_coverage::RunSelector = base.parse().unwrap();
    let compare_sel: rupu_coverage::RunSelector = compare.parse().unwrap();

    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let diff = rupu_coverage::run_diff(&paths, &base_sel, &compare_sel)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&diff)?);
        return Ok(());
    }

    println!(
        "coverage diff · target {} · base {} → compare {}",
        target_id,
        diff.base_runs.join(","),
        diff.compare_runs.join(","),
    );
    if diff.is_empty() {
        println!();
        println!("no changes between the two runs");
        return Ok(());
    }

    println!();
    println!("== cell-coverage delta ==");
    println!("  + newly asserted: {}", diff.newly_asserted.len());
    for c in &diff.newly_asserted {
        println!("      {} · {}  [{:?}]", c.concern_id, c.file_path, c.status);
    }
    println!("  - no longer asserted: {}", diff.no_longer_asserted.len());
    for c in &diff.no_longer_asserted {
        println!("      {} · {}  [{:?}]", c.concern_id, c.file_path, c.status);
    }

    if !diff.verdict_flips.is_empty() {
        println!();
        println!("== verdict flips ==");
        for f in &diff.verdict_flips {
            let mark = if f.high_signal { "!" } else { " " };
            println!(
                "  [{}] {} · {}  {:?} → {:?}",
                mark, f.concern_id, f.file_path, f.base_status, f.compare_status
            );
        }
    }

    if !diff.findings_appeared.is_empty() || !diff.findings_disappeared.is_empty() {
        println!();
        println!("== findings (theme-based, best-effort) ==");
        println!("  + appeared: {}", diff.findings_appeared.len());
        for f in &diff.findings_appeared {
            println!(
                "      ({}) {}",
                f.concern_id.as_deref().unwrap_or("-"),
                f.theme
            );
        }
        println!("  - disappeared: {}", diff.findings_disappeared.len());
        for f in &diff.findings_disappeared {
            println!(
                "      ({}) {}",
                f.concern_id.as_deref().unwrap_or("-"),
                f.theme
            );
        }
    }

    if !diff.newly_touched.is_empty() || !diff.no_longer_touched.is_empty() {
        println!();
        println!("== file-touch delta ==");
        println!("  + newly touched: {}", diff.newly_touched.len());
        for p in &diff.newly_touched {
            println!("      {p}");
        }
        println!("  - no longer touched: {}", diff.no_longer_touched.len());
        for p in &diff.no_longer_touched {
            println!("      {p}");
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rupu-cli --lib cmd::coverage::tests::diff_on_two_run_target_json_and_human`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/coverage.rs
git commit -m "feat(cli): rupu coverage diff — run-to-run comparison (human + json)"
```

---

## Task 8: CLI `coverage runs`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/coverage.rs`
- Test: `crates/rupu-cli/src/cmd/coverage.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Add the `Runs` subcommand variant**

In `crates/rupu-cli/src/cmd/coverage.rs`, add to the `Action` enum (after `Diff`):

```rust
    /// List the runs recorded against a target (to find ids to diff).
    Runs {
        /// Target id (from `coverage list`).
        target_id: String,
        /// Emit machine-readable JSON instead of the human table.
        #[arg(long)]
        json: bool,
    },
```

Add the dispatch arm in `handle` (after the `Diff` arm):

```rust
        Action::Runs { target_id, json } => {
            workspace().and_then(|ws| run_runs_in(&ws, &target_id, json))
        }
```

- [ ] **Step 2: Write the failing test**

Add to the `tests` module in `crates/rupu-cli/src/cmd/coverage.rs`:

```rust
    #[test]
    fn runs_list_json_and_human() {
        use rupu_coverage::{
            AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, Evidence, Surface,
        };
        use chrono::{DateTime, Utc};

        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();

        let a = ConcernAssertion {
            concern_id: "c1".to_string(),
            file_path: "src/a.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: Attribution {
                run_id: "run_one".to_string(),
                model: "m".to_string(),
                surface: Surface::Session,
            },
            declared_at: DateTime::<Utc>::from_timestamp(100, 0).unwrap(),
        };
        std::fs::write(&paths.concerns, serde_json::to_string(&a).unwrap() + "\n").unwrap();

        assert!(run_runs_in(tmp.path(), "tgt", true).is_ok()); // json
        assert!(run_runs_in(tmp.path(), "tgt", false).is_ok()); // human

        // Empty target lists no runs without error.
        let empty = CoveragePaths::new(tmp.path(), "empty");
        empty.ensure_dir().unwrap();
        assert!(run_runs_in(tmp.path(), "empty", false).is_ok());
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rupu-cli --lib cmd::coverage::tests::runs_list_json_and_human`
Expected: FAIL — `cannot find function 'run_runs_in'`.

- [ ] **Step 4: Implement `run_runs_in`**

Add to `crates/rupu-cli/src/cmd/coverage.rs` (after `run_diff_in`):

```rust
fn run_runs_in(workspace: &Path, target_id: &str, json: bool) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let runs = rupu_coverage::list_runs(&paths)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
        return Ok(());
    }

    println!("coverage runs · target {} · {} run(s)", target_id, runs.len());
    if runs.is_empty() {
        return Ok(());
    }
    println!();
    for r in &runs {
        println!(
            "  {} · {} · {:?} · {}  (cells {} / findings {} / files {})",
            r.run_id,
            r.started_at.to_rfc3339(),
            r.surface,
            r.model,
            r.cells_asserted,
            r.findings,
            r.files_touched,
        );
    }
    Ok(())
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rupu-cli --lib cmd::coverage::tests::runs_list_json_and_human`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/coverage.rs
git commit -m "feat(cli): rupu coverage runs — list runs with contribution counts"
```

---

## Task 9: Full verification + format/lint pass

**Files:** none (verification only)

- [ ] **Step 1: Build the whole workspace including tests**

Run: `cargo build --workspace --tests`
Expected: clean build.

- [ ] **Step 2: Run the coverage + CLI test suites**

Run: `cargo test -p rupu-coverage --lib diff:: && cargo test -p rupu-cli --lib cmd::coverage`
Expected: all PASS.

- [ ] **Step 3: Clippy on the touched crates**

Run: `cargo clippy -p rupu-coverage --lib --tests`
Expected: no new warnings from `src/diff/` or the `events.rs` / `audit/generate.rs` edits. (Pre-existing warnings elsewhere in the workspace are out of scope; only fix ones in files this plan touched.)

- [ ] **Step 4: Format check on touched files**

Run: `cargo fmt -p rupu-coverage -- --check` and `cargo fmt -p rupu-cli -- --check`
Expected: the diff/runs additions are clean. If only the new code shows diffs, run `cargo fmt -p rupu-coverage` / `cargo fmt -p rupu-cli` and re-stage. (Pre-existing fmt drift in untouched files on `main` is out of scope — do not reformat files this plan didn't change.)

- [ ] **Step 5: Final commit if fmt changed anything**

```bash
git add -A
git commit -m "style(coverage): rustfmt diff engine + CLI additions" || echo "nothing to format"
```

---

## Self-Review (completed by plan author)

**1. Spec coverage (B-1 section of the Slice B spec):**
- Run selectors (`<run_id>` / `latest` / `previous`, set-returning, zero-match error) → Task 3. ✅
- Derived verdict map / within-set supersession → Task 4 (`contribution`, timestamp-ordered last-wins). ✅
- Four diff dimensions (cell-coverage delta, verdict flips with `clean→finding` flagged, findings appeared/disappeared via theme primitive, file-touch delta) → Task 5 + `RunDiff` (Task 2). ✅
- `RunDiff` struct shape → Task 2 matches the spec's field list. ✅
- Deterministic sorted output → Task 5 sort block. ✅
- `rupu coverage runs` with id/timestamp/model/surface/counts → Tasks 6 + 8. ✅
- `rupu coverage diff` defaulting to `previous latest`, `--format`/json, human table → Task 7. ✅ (Note: follows the existing `--json` bool convention used by `coverage audit`, not the global `--format`; consistent with the sibling subcommand.)
- Error handling: unknown run id, zero-match selector → Task 3 (`DiffError`) surfaced through CLI in Tasks 7-8. ✅
- Theme-based finding matching reuses `audit::theme_key` → Task 4. ✅
- Testing matrix (synthetic two-run ledgers, selector resolution incl. errors, CLI json+human smoke) → Tasks 3, 5, 7, 8. ✅

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every command has an expected result. ✅

**3. Type consistency:** `RunDiff` / `CellRef` / `VerdictFlip` / `FindingThemeRef` / `RunListEntry` defined in Task 2 are used with identical field names in Tasks 5-8. `RunSelector` / `DiffError` (Task 3) consumed unchanged in Tasks 5-7. `Contribution` (Task 4) consumed in Tasks 5-6. `run_diff` / `list_runs` signatures match between definition (Tasks 5/6) and CLI call sites (Tasks 7/8) and the crate re-exports. ✅

**Out of scope for this plan (later B-plans):** manifest capture, `rerun`, determinism contract tests — Plans B-2 and B-3.
