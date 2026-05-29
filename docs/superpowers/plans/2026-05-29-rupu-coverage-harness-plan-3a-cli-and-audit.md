# rupu coverage harness — Plan 3a: CLI + audit report + tool-mappings

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the coverage ledgers actionable: an `AuditReport` generator that joins the three JSONL ledgers + catalog snapshot into per-concern gap analysis / cross-model agreement / serendipitous-finding clusters, a thin `rupu coverage` CLI subcommand (`list` / `show` / `audit` / `gap` / `catalog` / `templates`) to surface it, and a `tool-mappings.yaml` so custom MCP tools can contribute file-touch events.

**Architecture:** The audit logic lives entirely in `rupu-coverage` (pure functions over the ledgers — no I/O beyond reading the JSONL/YAML files, fully unit-testable). `rupu-cli` adds a thin `coverage` subcommand that resolves a target, calls the rupu-coverage query/audit functions, and renders results via the existing `rupu-cli` output framework (JSON for `--format pretty`-off, human table otherwise). The `tool-mappings` feature adds a small config type in `rupu-coverage` and wires it into `rupu-tools`' unknown-tool emit path.

**Tech Stack:** Rust, `serde`/`serde_json`/`serde_yaml`, `glob`, `clap` (CLI), the existing `rupu-cli` output/printer/palette primitives. Tests use `tempfile`.

**Spec:** `docs/superpowers/specs/2026-05-23-rupu-coverage-harness-design.md` (the "Audit / report generation" + "Components → rupu-cli" sections).

**Prior plans:** Plan 1 (foundation, merged), Plan 2 (large catalogs + CWE, merged), follow-ups (merged).

**Out of scope for this plan** (deferred to a session-validated follow-up):
- Session-surface integration: the `coverage 12/15` footer indicator and the `/coverage` slash command in the interactive session UI. These need live terminal validation (`rupu session start`) per the project's UI-validation rule, so they land in a separate pass matt runs interactively.

---

## Background facts (verified against the codebase)

- `rupu-coverage` already exposes: `CoveragePaths::new(workspace, target_id)`, `target_id(workspace, scope_name)`, `read_file_events(&paths)`, `read_concern_assertions(&paths)`, `file_views(&events) -> Vec<FileView>`, `read_snapshot(&path) -> FlatCatalog`, and the `FlatCatalog`/`Concern`/`Severity`/`AssertionStatus`/`FindingRecord`/`FindingScope`/`ConcernAssertion`/`FileTouchEvent`/`Attribution` types. `FindingRecord` has a `read_findings`? — NO; only files + concerns have readers. **Task 1 adds `read_findings`.**
- `FileView` fields: `path`, `touch_modes: Vec<TouchStrength>`, `strongest: TouchStrength`, `read_lines`, `grep_matches`, `edits`, `first_at`, `last_at`, `touched_by: Vec<Attribution>`.
- `Concern` fields: `id`, `name`, `description`, `severity`, `applicable_globs: Vec<String>`, `min_strength`, `references`, `tags`.
- `ConcernAssertion`: `concern_id`, `file_path`, `status: AssertionStatus` (Clean/Finding/Examined/NotApplicable), `evidence`, `declared_by: Attribution { run_id, model, surface }`, `declared_at`.
- `FindingRecord`: `id`, `file_path: Option<String>`, `line_range`, `scope`, `summary`, `severity`, `concern_id: Option<String>`, `evidence`, `declared_by`, `declared_at`.
- `rupu-cli` clap enum is `Cmd` in `crates/rupu-cli/src/lib.rs`; dispatch arms call `cmd::<name>::handle(action, cli.format).await`. `cli.format` is an output-format enum. Subcommand modules live in `crates/rupu-cli/src/cmd/` and are registered in `crates/rupu-cli/src/cmd/mod.rs`.
- `rupu-cli` is a thin dispatcher per architecture rule #2: NO business logic in the CLI crate. All audit/query logic stays in `rupu-coverage`.
- `ToolContext` (in `crates/rupu-tools/src/tool.rs`) has `coverage_writer`, `surface_tag`, `run_id`, `model`, `workspace_path`. Unknown tools currently emit no path (Plan 1 design). The `coverage_emit` helper module is in `crates/rupu-tools/src/coverage_emit.rs`.

---

## File structure

```
crates/rupu-coverage/src/
├── audit/
│   ├── mod.rs                    (NEW) re-exports
│   ├── types.rs                  (NEW) AuditReport, ConcernCoverage, FileCoverage, CrossModelEntry, SerendipitousCluster
│   └── generate.rs               (NEW) audit() — joins ledgers + catalog into AuditReport
├── ledger/
│   ├── views.rs                  (MODIFY) add read_findings(&paths) -> io::Result<Vec<FindingRecord>>
│   └── mod.rs                    (MODIFY) re-export read_findings
├── tool_mappings.rs              (NEW) ToolMappings config type + load_tool_mappings
└── lib.rs                        (MODIFY) re-export audit + tool_mappings symbols

crates/rupu-cli/src/cmd/
├── coverage.rs                   (NEW) clap Action enum + handle(); thin — resolves target, calls rupu-coverage, renders
└── mod.rs                        (MODIFY) pub mod coverage;

crates/rupu-cli/src/lib.rs        (MODIFY) add Cmd::Coverage variant + dispatch arm

crates/rupu-tools/src/
├── coverage_emit.rs              (MODIFY) accept optional tool-mappings to resolve unknown-tool paths
└── tool.rs                       (MODIFY) ToolContext gains tool_mappings: Option<Arc<ToolMappings>>
```

---

## Task 1: `read_findings` ledger reader

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/views.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in `views.rs`

- [ ] **Step 1: Add `read_findings` to `views.rs`**

`views.rs` already has `read_file_events` and `read_concern_assertions` with an identical shape (slurp file, filter empty lines, filter_map serde_json). Add the third:

```rust
use crate::ledger::events::FindingRecord;

pub fn read_findings(paths: &CoveragePaths) -> std::io::Result<Vec<FindingRecord>> {
    if !paths.findings.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.findings)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<FindingRecord>(l).ok())
        .collect())
}
```

(Add `FindingRecord` to the existing `use crate::ledger::events::{...}` line rather than a second `use` if one already imports from that module.)

- [ ] **Step 2: Write the test**

Add to the `tests` module in `views.rs`:

```rust
    #[test]
    fn read_findings_parses_jsonl_and_handles_missing_file() {
        use crate::ledger::events::{Attribution, FindingEvidence, FindingScope, Surface};
        use crate::catalog::types::Severity;

        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t");

        // Missing file → empty.
        assert!(read_findings(&paths).unwrap().is_empty());

        // Write one finding, read it back.
        paths.ensure_dir().unwrap();
        let rec = FindingRecord {
            id: "fnd_1".to_string(),
            file_path: Some("src/a.rs".to_string()),
            line_range: Some([1, 5]),
            scope: FindingScope::Line,
            summary: "x".to_string(),
            severity: Severity::High,
            concern_id: Some("ssrf".to_string()),
            evidence: FindingEvidence { code_excerpt: None, rationale: "r".to_string(), references: vec![] },
            declared_by: Attribution { run_id: "r".to_string(), model: "m".to_string(), surface: Surface::Workflow },
            declared_at: chrono::Utc::now(),
        };
        std::fs::write(&paths.findings, serde_json::to_string(&rec).unwrap() + "\n").unwrap();
        let got = read_findings(&paths).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "fnd_1");
    }
```

- [ ] **Step 3: Re-export**

Edit `crates/rupu-coverage/src/ledger/mod.rs`: add `read_findings` to `pub use views::{...}`.
Edit `crates/rupu-coverage/src/lib.rs`: add `read_findings` to the `pub use ledger::{...}` line.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: prior tests + 1 new pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): add read_findings ledger reader"
```

---

## Task 2: Audit report types

**Files:**
- Create: `crates/rupu-coverage/src/audit/mod.rs`
- Create: `crates/rupu-coverage/src/audit/types.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in `types.rs`

- [ ] **Step 1: Create `crates/rupu-coverage/src/audit/types.rs`**

```rust
use crate::catalog::types::Severity;
use crate::ledger::events::AssertionStatus;
use serde::{Deserialize, Serialize};

/// Coverage outcome for a single concern across the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernCoverage {
    pub concern_id: String,
    pub name: String,
    pub severity: Severity,
    /// Files in scope = touched files whose path matches the concern's
    /// applicable_globs.
    pub in_scope_files: Vec<String>,
    /// Files with a non-NotApplicable assertion for this concern.
    pub asserted_files: Vec<String>,
    /// in_scope − asserted: files that should have been assessed but weren't.
    pub gap_files: Vec<String>,
    /// Count of assertions per status for this concern.
    pub clean: u32,
    pub findings: u32,
    pub examined: u32,
    pub not_applicable: u32,
}

impl ConcernCoverage {
    /// True when every in-scope file has been assessed (no gaps).
    pub fn is_complete(&self) -> bool {
        self.gap_files.is_empty()
    }
}

/// Per-file coverage summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCoverage {
    pub path: String,
    pub strongest_touch: String,
    /// Concern ids asserted (any status) against this file.
    pub asserted_concerns: Vec<String>,
    /// Catalog concern ids whose applicable_globs match this file but
    /// have no assertion — expected-but-missing.
    pub missing_concerns: Vec<String>,
}

/// A (concern, file) pair assessed by more than one model, with the set
/// of distinct statuses observed. Disagreement (len > 1) is a signal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrossModelEntry {
    pub concern_id: String,
    pub file_path: String,
    /// (model, status) pairs, one per model that asserted this cell.
    pub model_statuses: Vec<(String, AssertionStatus)>,
    /// True when models disagreed on the status.
    pub disagreement: bool,
}

/// Serendipitous findings (concern_id = None) grouped by a coarse key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerendipitousCluster {
    /// Coarse grouping key (lowercased first ~6 words of the summary).
    pub theme: String,
    pub finding_ids: Vec<String>,
    pub count: u32,
}

/// Full audit report for a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    pub target_id: String,
    pub concerns: Vec<ConcernCoverage>,
    pub files: Vec<FileCoverage>,
    pub cross_model: Vec<CrossModelEntry>,
    pub serendipitous: Vec<SerendipitousCluster>,
    /// Quick totals for the summary line.
    pub total_concerns: usize,
    pub complete_concerns: usize,
    pub total_gap_files: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concern_coverage_is_complete_when_no_gaps() {
        let cc = ConcernCoverage {
            concern_id: "ssrf".to_string(),
            name: "SSRF".to_string(),
            severity: Severity::High,
            in_scope_files: vec!["a.rs".to_string()],
            asserted_files: vec!["a.rs".to_string()],
            gap_files: vec![],
            clean: 1,
            findings: 0,
            examined: 0,
            not_applicable: 0,
        };
        assert!(cc.is_complete());
    }

    #[test]
    fn concern_coverage_incomplete_with_gaps() {
        let cc = ConcernCoverage {
            concern_id: "ssrf".to_string(),
            name: "SSRF".to_string(),
            severity: Severity::High,
            in_scope_files: vec!["a.rs".to_string(), "b.rs".to_string()],
            asserted_files: vec!["a.rs".to_string()],
            gap_files: vec!["b.rs".to_string()],
            clean: 1,
            findings: 0,
            examined: 0,
            not_applicable: 0,
        };
        assert!(!cc.is_complete());
    }
}
```

- [ ] **Step 2: Create `crates/rupu-coverage/src/audit/mod.rs`**

```rust
pub mod types;
pub use types::{
    AuditReport, ConcernCoverage, CrossModelEntry, FileCoverage, SerendipitousCluster,
};
```

- [ ] **Step 3: Re-export from lib.rs**

Edit `crates/rupu-coverage/src/lib.rs`: add `pub mod audit;` and a `pub use audit::{AuditReport, ConcernCoverage, CrossModelEntry, FileCoverage, SerendipitousCluster};` line.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: prior + 2 new pass; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): audit report data model"
```

---

## Task 3: Audit generator — per-concern coverage

**Files:**
- Create: `crates/rupu-coverage/src/audit/generate.rs`
- Modify: `crates/rupu-coverage/src/audit/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in `generate.rs`

- [ ] **Step 1: Create `generate.rs` with the per-concern pass + the top-level `audit` entry point**

```rust
use crate::audit::types::{
    AuditReport, ConcernCoverage, CrossModelEntry, FileCoverage, SerendipitousCluster,
};
use crate::catalog::types::FlatCatalog;
use crate::ledger::events::{AssertionStatus, ConcernAssertion, FindingRecord};
use crate::ledger::paths::CoveragePaths;
use crate::ledger::views::{file_views, read_concern_assertions, read_file_events, read_findings};
use std::collections::{BTreeMap, BTreeSet};

/// Build a full audit report for a target by joining the three ledgers
/// with the effective-catalog snapshot.
pub fn audit(paths: &CoveragePaths) -> std::io::Result<AuditReport> {
    let catalog = crate::catalog::snapshot::read_snapshot(&paths.catalog)
        .unwrap_or(FlatCatalog {
            concerns: vec![],
            sources: BTreeMap::new(),
            render_modes: BTreeMap::new(),
        });
    let events = read_file_events(paths)?;
    let views = file_views(&events);
    let assertions = read_concern_assertions(paths)?;
    let findings = read_findings(paths)?;

    let target_id = paths
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let concerns = concern_coverage(&catalog, &views, &assertions);
    let files = file_coverage(&catalog, &views, &assertions);
    let cross_model = cross_model(&assertions);
    let serendipitous = serendipitous(&findings);

    let total_concerns = concerns.len();
    let complete_concerns = concerns.iter().filter(|c| c.is_complete()).count();
    let total_gap_files = concerns.iter().map(|c| c.gap_files.len()).sum();

    Ok(AuditReport {
        target_id,
        concerns,
        files,
        cross_model,
        serendipitous,
        total_concerns,
        complete_concerns,
        total_gap_files,
    })
}

fn glob_match(globs: &[String], path: &str) -> bool {
    if globs.is_empty() {
        return true;
    }
    globs.iter().any(|g| {
        glob::Pattern::new(g)
            .map(|p| p.matches(path))
            .unwrap_or(false)
    })
}

fn concern_coverage(
    catalog: &FlatCatalog,
    views: &[crate::ledger::views::FileView],
    assertions: &[ConcernAssertion],
) -> Vec<ConcernCoverage> {
    catalog
        .concerns
        .iter()
        .map(|concern| {
            let in_scope: Vec<String> = views
                .iter()
                .filter(|v| glob_match(&concern.applicable_globs, &v.path))
                .map(|v| v.path.clone())
                .collect();

            let mut asserted: BTreeSet<String> = BTreeSet::new();
            let (mut clean, mut findings, mut examined, mut not_applicable) = (0u32, 0u32, 0u32, 0u32);
            for a in assertions.iter().filter(|a| a.concern_id == concern.id) {
                match a.status {
                    AssertionStatus::Clean => clean += 1,
                    AssertionStatus::Finding => findings += 1,
                    AssertionStatus::Examined => examined += 1,
                    AssertionStatus::NotApplicable => not_applicable += 1,
                }
                if a.status != AssertionStatus::NotApplicable {
                    asserted.insert(a.file_path.clone());
                }
            }

            let asserted_files: Vec<String> = asserted.iter().cloned().collect();
            let gap_files: Vec<String> = in_scope
                .iter()
                .filter(|f| !asserted.contains(*f))
                .cloned()
                .collect();

            ConcernCoverage {
                concern_id: concern.id.clone(),
                name: concern.name.clone(),
                severity: concern.severity,
                in_scope_files: in_scope,
                asserted_files,
                gap_files,
                clean,
                findings,
                examined,
                not_applicable,
            }
        })
        .collect()
}

fn file_coverage(
    catalog: &FlatCatalog,
    views: &[crate::ledger::views::FileView],
    assertions: &[ConcernAssertion],
) -> Vec<FileCoverage> {
    views
        .iter()
        .map(|v| {
            let asserted: Vec<String> = assertions
                .iter()
                .filter(|a| a.file_path == v.path)
                .map(|a| a.concern_id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let asserted_set: BTreeSet<&String> = asserted.iter().collect();
            let missing: Vec<String> = catalog
                .concerns
                .iter()
                .filter(|c| glob_match(&c.applicable_globs, &v.path))
                .map(|c| c.id.clone())
                .filter(|id| !asserted_set.contains(id))
                .collect();
            FileCoverage {
                path: v.path.clone(),
                strongest_touch: format!("{:?}", v.strongest).to_lowercase(),
                asserted_concerns: asserted,
                missing_concerns: missing,
            }
        })
        .collect()
}

fn cross_model(assertions: &[ConcernAssertion]) -> Vec<CrossModelEntry> {
    // Group by (concern_id, file_path) → map model → latest status.
    let mut cells: BTreeMap<(String, String), BTreeMap<String, AssertionStatus>> = BTreeMap::new();
    for a in assertions {
        cells
            .entry((a.concern_id.clone(), a.file_path.clone()))
            .or_default()
            .insert(a.declared_by.model.clone(), a.status);
    }
    cells
        .into_iter()
        .filter(|(_, models)| models.len() > 1)
        .map(|((concern_id, file_path), models)| {
            let distinct: BTreeSet<AssertionStatus> = models.values().copied().collect();
            let model_statuses: Vec<(String, AssertionStatus)> = models.into_iter().collect();
            CrossModelEntry {
                concern_id,
                file_path,
                disagreement: distinct.len() > 1,
                model_statuses,
            }
        })
        .collect()
}

fn serendipitous(findings: &[FindingRecord]) -> Vec<SerendipitousCluster> {
    let mut by_theme: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in findings.iter().filter(|f| f.concern_id.is_none()) {
        let theme = theme_key(&f.summary);
        by_theme.entry(theme).or_default().push(f.id.clone());
    }
    by_theme
        .into_iter()
        .map(|(theme, ids)| SerendipitousCluster {
            theme,
            count: ids.len() as u32,
            finding_ids: ids,
        })
        .collect()
}

fn theme_key(summary: &str) -> String {
    summary
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::flatten::flatten;
    use crate::catalog::types::{CatalogMode, ConcernsBlock, ConcernsEntry, IncludeDirective};
    use crate::ledger::events::{Attribution, Evidence, FileTouchEvent, Surface};
    use chrono::Utc;

    fn attribution(model: &str) -> Attribution {
        Attribution {
            run_id: "r".to_string(),
            model: model.to_string(),
            surface: Surface::Workflow,
        }
    }

    fn read_event(path: &str) -> FileTouchEvent {
        FileTouchEvent::Read {
            path: path.to_string(),
            line_range: [1, 50],
            tool: "read_file".to_string(),
            attribution: attribution("m1"),
            at: Utc::now(),
        }
    }

    fn assertion(concern: &str, file: &str, status: AssertionStatus, model: &str) -> ConcernAssertion {
        ConcernAssertion {
            concern_id: concern.to_string(),
            file_path: file.to_string(),
            status,
            evidence: Evidence { summary: "s".to_string(), line_ranges: vec![], finding_ids: vec![] },
            declared_by: attribution(model),
            declared_at: Utc::now(),
        }
    }

    fn stride_catalog() -> FlatCatalog {
        flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            })],
        })
        .unwrap()
    }

    #[test]
    fn concern_coverage_computes_gaps() {
        let catalog = stride_catalog();
        // stride concerns apply to ** (most) so two touched files are in-scope.
        let views = file_views(&[read_event("src/a.rs"), read_event("src/b.rs")]);
        // Assert spoofing clean on a.rs only → b.rs is a gap for spoofing.
        let assertions = vec![assertion(
            "stride:spoofing",
            "src/a.rs",
            AssertionStatus::Clean,
            "m1",
        )];
        let cov = concern_coverage(&catalog, &views, &assertions);
        let spoofing = cov.iter().find(|c| c.concern_id == "stride:spoofing").unwrap();
        // spoofing applies to **/auth/** etc — verify against the actual
        // stride.yaml globs: spoofing has applicable_globs, so src/a.rs may
        // or may not be in scope. Assert on the relationship instead:
        assert_eq!(spoofing.clean, 1);
        // Any in-scope file without an assertion is a gap.
        for f in &spoofing.in_scope_files {
            if f != "src/a.rs" {
                assert!(spoofing.gap_files.contains(f));
            }
        }
    }

    #[test]
    fn cross_model_flags_disagreement() {
        let assertions = vec![
            assertion("stride:spoofing", "src/a.rs", AssertionStatus::Clean, "m1"),
            assertion("stride:spoofing", "src/a.rs", AssertionStatus::Finding, "m2"),
        ];
        let xm = cross_model(&assertions);
        assert_eq!(xm.len(), 1);
        assert!(xm[0].disagreement);
        assert_eq!(xm[0].model_statuses.len(), 2);
    }

    #[test]
    fn cross_model_agreement_not_flagged_as_disagreement() {
        let assertions = vec![
            assertion("stride:spoofing", "src/a.rs", AssertionStatus::Clean, "m1"),
            assertion("stride:spoofing", "src/a.rs", AssertionStatus::Clean, "m2"),
        ];
        let xm = cross_model(&assertions);
        assert_eq!(xm.len(), 1);
        assert!(!xm[0].disagreement);
    }

    #[test]
    fn single_model_cell_not_in_cross_model() {
        let assertions = vec![assertion("stride:spoofing", "src/a.rs", AssertionStatus::Clean, "m1")];
        assert!(cross_model(&assertions).is_empty());
    }
}
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/audit/mod.rs`: add `pub mod generate;` and `pub use generate::audit;`.
Edit `crates/rupu-coverage/src/lib.rs`: add `audit` to the `pub use audit::{...}` re-export (note the module is `audit` and the fn is `audit` — re-export as `pub use audit::generate::audit;` or alias; to avoid the name clash with the module, in lib.rs write `pub use audit::audit as audit_report;` OR just reference `rupu_coverage::audit::audit` from the CLI. CLEANEST: in lib.rs add `pub use audit::generate::audit as run_audit;` and have the CLI call `rupu_coverage::run_audit(&paths)`. Decide one and use it consistently in Task 6/7.)

To keep it unambiguous: in `lib.rs`, re-export the function as `run_audit`:

```rust
pub use audit::generate::audit as run_audit;
```

and keep the types re-exported as in Task 2.

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: prior + 4 new pass; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): audit generator (per-concern gaps, cross-model, serendipitous)"
```

---

## Task 4: Target discovery

**Files:**
- Create: `crates/rupu-coverage/src/ledger/discover.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline in `discover.rs`

- [ ] **Step 1: Create `discover.rs`**

```rust
use std::path::Path;

/// A coverage target found under `.rupu/coverage/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTarget {
    pub target_id: String,
    /// Number of concern assertions on disk (cheap signal of activity).
    pub assertion_lines: usize,
    pub has_catalog: bool,
}

/// List all coverage targets under `<workspace>/.rupu/coverage/`.
/// Returns an empty vec if the directory doesn't exist.
pub fn discover_targets(workspace: &Path) -> std::io::Result<Vec<DiscoveredTarget>> {
    let root = workspace.join(".rupu").join("coverage");
    if !root.is_dir() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let target_id = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        let concerns = dir.join("concerns.jsonl");
        let assertion_lines = std::fs::read_to_string(&concerns)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        let has_catalog = dir.join("catalog.yaml").exists();
        out.push(DiscoveredTarget {
            target_id,
            assertion_lines,
            has_catalog,
        });
    }
    out.sort_by(|a, b| a.target_id.cmp(&b.target_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::paths::CoveragePaths;

    #[test]
    fn discover_empty_when_no_coverage_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(discover_targets(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn discover_lists_target_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "abc123");
        paths.ensure_dir().unwrap();
        std::fs::write(&paths.concerns, "{}\n{}\n").unwrap();
        std::fs::write(&paths.catalog, "name: x\n").unwrap();
        let targets = discover_targets(tmp.path()).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target_id, "abc123");
        assert_eq!(targets[0].assertion_lines, 2);
        assert!(targets[0].has_catalog);
    }
}
```

- [ ] **Step 2: Wire into module + lib**

Edit `crates/rupu-coverage/src/ledger/mod.rs`: add `pub mod discover;` and `pub use discover::{discover_targets, DiscoveredTarget};`.
Edit `crates/rupu-coverage/src/lib.rs`: add `discover_targets, DiscoveredTarget` to the `pub use ledger::{...}` line.

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: prior + 2 new pass; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): discover coverage targets under .rupu/coverage"
```

---

## Task 5: `rupu coverage` subcommand skeleton + `list` + `templates`

**Files:**
- Create: `crates/rupu-cli/src/cmd/coverage.rs`
- Modify: `crates/rupu-cli/src/cmd/mod.rs`
- Modify: `crates/rupu-cli/src/lib.rs`
- Modify: `crates/rupu-cli/Cargo.toml` (ensure `rupu-coverage` dep)
- Test: inline in `coverage.rs`

- [ ] **Step 1: Confirm/add the `rupu-coverage` dependency**

Check `crates/rupu-cli/Cargo.toml` for `rupu-coverage`. If absent, add under `[dependencies]`:

```toml
rupu-coverage = { path = "../rupu-coverage" }
```

- [ ] **Step 2: Create `crates/rupu-cli/src/cmd/coverage.rs`**

This is a THIN dispatcher (architecture rule #2). `list` and `templates` only here; `show`/`audit`/`gap`/`catalog` land in Tasks 6-7.

```rust
use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::output::OutputFormat;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List coverage targets recorded under .rupu/coverage/.
    List,
    /// List or show bundled concern templates.
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplatesAction {
    /// List bundled template names.
    List,
    /// Print a bundled template's concerns.
    Show { name: String },
}

fn workspace() -> Result<PathBuf> {
    Ok(std::env::current_dir()?)
}

pub async fn handle(action: Action, _format: OutputFormat) -> ExitCode {
    let result = match action {
        Action::List => run_list(),
        Action::Templates { action } => run_templates(action),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("coverage error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_list() -> Result<()> {
    let ws = workspace()?;
    let targets = rupu_coverage::discover_targets(&ws)?;
    if targets.is_empty() {
        println!("no coverage targets under .rupu/coverage/");
        return Ok(());
    }
    for t in targets {
        println!(
            "{}  ·  {} assertions  ·  catalog: {}",
            t.target_id,
            t.assertion_lines,
            if t.has_catalog { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn run_templates(action: TemplatesAction) -> Result<()> {
    match action {
        TemplatesAction::List => {
            for name in rupu_coverage::builtin_names() {
                println!("{name}");
            }
            Ok(())
        }
        TemplatesAction::Show { name } => {
            let template = rupu_coverage::resolve_builtin(&name)
                .ok_or_else(|| anyhow::anyhow!("unknown template `{name}`"))?
                .map_err(|e| anyhow::anyhow!("template parse error: {e}"))?;
            for concern in &template.concerns {
                println!("{}  [{:?}]  {}", concern.id, concern.severity, concern.name);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_handles_no_targets_gracefully() {
        // Run in a temp cwd with no .rupu/coverage; should succeed.
        let tmp = tempfile::TempDir::new().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let code = handle(Action::List, OutputFormat::default()).await;
        std::env::set_current_dir(prev).unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
    }
}
```

IMPORTANT — verify before writing: the actual `OutputFormat` type name + import path (grep `cli.format` usage in `crates/rupu-cli/src/lib.rs` and how other `handle(action, cli.format)` signatures type it). Match the real type. Also confirm `builtin_names` returns `impl Iterator<Item = &'static str>` and `resolve_builtin(name) -> Option<Result<Template, _>>` (from Plan 1) — adapt the calls to the real signatures. The `set_current_dir` test is process-global; if the crate's tests run in parallel and this is flaky, mark it `#[ignore]` with a note or use a more targeted approach (e.g. add an internal `run_list_in(workspace)` that takes the dir, test that directly, and have `run_list` call it with `current_dir()`). PREFER the `run_list_in(&Path)` seam to avoid cwd-mutation flakiness.

- [ ] **Step 3: Register the subcommand**

Edit `crates/rupu-cli/src/cmd/mod.rs`: add `pub mod coverage;` (alphabetical).

Edit `crates/rupu-cli/src/lib.rs`:
- Add a `Cmd` variant:
  ```rust
      /// Inspect agentic coverage ledgers and concern catalogs.
      Coverage {
          #[command(subcommand)]
          action: cmd::coverage::Action,
      },
  ```
  (Place it near the other subcommands.)
- Add the dispatch arm:
  ```rust
          Cmd::Coverage { action } => cmd::coverage::handle(action, cli.format).await,
  ```

- [ ] **Step 4: Run tests + build + clippy**

```bash
cargo build -p rupu-cli
cargo test -p rupu-cli --lib coverage
cargo clippy -p rupu-cli --lib -- -D warnings 2>&1 | tail -5
```

Expected: builds; the coverage list test passes; clippy on the new module clean. (Note: rupu-cli has pre-existing clippy issues in OTHER modules under a newer toolchain — only verify the coverage module added no NEW issues.)

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cli Cargo.lock
git commit -m "feat(cli): rupu coverage subcommand skeleton (list + templates)"
```

---

## Task 6: `coverage catalog` + `coverage show`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/coverage.rs`
- Test: inline

- [ ] **Step 1: Add `Catalog` and `Show` actions**

Extend the `Action` enum:

```rust
    /// Print the effective catalog snapshot for a target.
    Catalog {
        /// Target id (from `coverage list`).
        target_id: String,
    },
    /// Show the derived ledger view (touched files + assertions) for a target.
    Show {
        /// Target id (from `coverage list`).
        target_id: String,
    },
```

Add handler arms in `handle`:

```rust
        Action::Catalog { target_id } => run_catalog(&target_id),
        Action::Show { target_id } => run_show(&target_id),
```

Implement:

```rust
fn paths_for(target_id: &str) -> Result<rupu_coverage::CoveragePaths> {
    let ws = workspace()?;
    Ok(rupu_coverage::CoveragePaths::new(&ws, target_id))
}

fn run_catalog(target_id: &str) -> Result<()> {
    let paths = paths_for(target_id)?;
    if !paths.catalog.exists() {
        anyhow::bail!("no catalog snapshot for target `{target_id}`");
    }
    let catalog = rupu_coverage::read_snapshot(&paths.catalog)?;
    println!("{} concerns in effective catalog", catalog.concerns.len());
    for c in &catalog.concerns {
        println!("  {}  [{:?}]  {}", c.id, c.severity, c.name);
    }
    Ok(())
}

fn run_show(target_id: &str) -> Result<()> {
    let paths = paths_for(target_id)?;
    let events = rupu_coverage::read_file_events(&paths)?;
    let views = rupu_coverage::file_views(&events);
    let assertions = rupu_coverage::read_concern_assertions(&paths)?;
    let findings = rupu_coverage::read_findings(&paths)?;

    println!("== files touched ({}) ==", views.len());
    for v in &views {
        println!("  {}  [{}]", v.path, format!("{:?}", v.strongest).to_lowercase());
    }
    println!("== concern assertions ({}) ==", assertions.len());
    for a in &assertions {
        println!(
            "  {} · {} · {:?} · {}",
            a.concern_id, a.file_path, a.status, a.declared_by.model
        );
    }
    println!("== findings ({}) ==", findings.len());
    for f in &findings {
        println!(
            "  {} · {:?} · {} · {}",
            f.id,
            f.severity,
            f.file_path.as_deref().unwrap_or("(repo)"),
            f.summary
        );
    }
    Ok(())
}
```

Verify `read_snapshot`, `read_file_events`, `file_views`, `read_concern_assertions`, `read_findings`, `CoveragePaths` are all re-exported at the `rupu_coverage` crate root (Tasks 1 + prior plans). If `read_snapshot` is exported as `read_snapshot`, use it; adapt names to reality.

- [ ] **Step 2: Add a test**

```rust
    #[tokio::test]
    async fn show_errors_clearly_for_missing_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        // No ledger files → show should still succeed (empty sections), and
        // catalog should fail clearly.
        let show = handle(Action::Show { target_id: "missing".into() }, OutputFormat::default()).await;
        let cat = handle(Action::Catalog { target_id: "missing".into() }, OutputFormat::default()).await;
        std::env::set_current_dir(prev).unwrap();
        assert_eq!(show, ExitCode::SUCCESS); // empty sections, no panic
        assert_eq!(cat, ExitCode::FAILURE);  // no catalog → error
    }
```

(If you used the `run_*_in(&Path)` seam to avoid cwd mutation in Task 5, mirror it here.)

- [ ] **Step 3: Build + test + clippy**

```bash
cargo build -p rupu-cli
cargo test -p rupu-cli --lib coverage
cargo clippy -p rupu-cli --lib -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli
git commit -m "feat(cli): coverage catalog + show subcommands"
```

---

## Task 7: `coverage audit` + `coverage gap`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/coverage.rs`
- Test: inline

- [ ] **Step 1: Add `Audit` and `Gap` actions + rendering**

Extend `Action`:

```rust
    /// Generate the coverage audit report for a target.
    Audit {
        target_id: String,
        /// Emit machine-readable JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// Show only the coverage gaps (in-scope files lacking an assertion).
    Gap { target_id: String },
```

Handler arms:

```rust
        Action::Audit { target_id, json } => run_audit(&target_id, json),
        Action::Gap { target_id } => run_gap(&target_id),
```

Implement using `rupu_coverage::run_audit` (the re-exported `audit::generate::audit` from Task 3):

```rust
fn run_audit(target_id: &str, json: bool) -> Result<()> {
    let paths = paths_for(target_id)?;
    let report = rupu_coverage::run_audit(&paths)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    // Human summary.
    println!(
        "coverage audit · target {} · {}/{} concerns complete · {} gap files",
        report.target_id, report.complete_concerns, report.total_concerns, report.total_gap_files
    );
    println!();
    println!("== per-concern ==");
    for c in &report.concerns {
        let mark = if c.is_complete() { "ok" } else { "GAP" };
        println!(
            "  [{}] {}  [{:?}]  in_scope={} asserted={} gap={}  (clean {} / finding {} / examined {} / n/a {})",
            mark, c.concern_id, c.severity,
            c.in_scope_files.len(), c.asserted_files.len(), c.gap_files.len(),
            c.clean, c.findings, c.examined, c.not_applicable
        );
    }
    if !report.cross_model.is_empty() {
        println!();
        println!("== cross-model ==");
        for x in &report.cross_model {
            let tag = if x.disagreement { "DISAGREE" } else { "agree" };
            println!("  [{}] {} · {} · {:?}", tag, x.concern_id, x.file_path, x.model_statuses);
        }
    }
    if !report.serendipitous.is_empty() {
        println!();
        println!("== serendipitous findings ==");
        for s in &report.serendipitous {
            println!("  ({}) {}  {:?}", s.count, s.theme, s.finding_ids);
        }
    }
    Ok(())
}

fn run_gap(target_id: &str) -> Result<()> {
    let paths = paths_for(target_id)?;
    let report = rupu_coverage::run_audit(&paths)?;
    let mut any = false;
    for c in &report.concerns {
        if c.gap_files.is_empty() {
            continue;
        }
        any = true;
        println!("{} ({} gap files):", c.concern_id, c.gap_files.len());
        for f in &c.gap_files {
            println!("  {f}");
        }
    }
    if !any {
        println!("no coverage gaps");
    }
    Ok(())
}
```

- [ ] **Step 2: Add tests (use the `run_*_in` seam or a populated tempdir)**

```rust
    #[tokio::test]
    async fn audit_json_on_populated_target() {
        use rupu_coverage::{
            flatten, write_snapshot, CoveragePaths, ConcernsBlock, ConcernsEntry, IncludeDirective,
        };
        use rupu_coverage::CatalogMode;

        let tmp = tempfile::TempDir::new().unwrap();
        // Build a target dir with a snapshot + one touch + one assertion.
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();
        let cat = flatten(&ConcernsBlock {
            entries: vec![ConcernsEntry::Include(IncludeDirective {
                include: "stride".to_string(),
                overrides: vec![],
                mode: CatalogMode::Auto,
                filter: None,
            })],
        })
        .unwrap();
        write_snapshot(&cat, &paths.catalog).unwrap();
        // A read touch.
        std::fs::write(
            &paths.files,
            serde_json::json!({
                "kind":"read","path":"src/auth/login.rs","line_range":[1,50],
                "tool":"read_file","run_id":"r","model":"m","surface":"workflow",
                "at":"2026-05-29T00:00:00Z"
            }).to_string() + "\n",
        ).unwrap();

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let code = handle(Action::Audit { target_id: "tgt".into(), json: true }, OutputFormat::default()).await;
        std::env::set_current_dir(prev).unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
    }
```

(Adapt the `FileTouchEvent::Read` JSON to the exact serde representation — the `kind` tag + flattened attribution fields. If hand-writing the JSON is fragile, instead build a `FileTouchEvent::Read` value and `serde_json::to_string` it, matching the events.rs shape.)

- [ ] **Step 3: Build + test + clippy**

```bash
cargo build -p rupu-cli
cargo test -p rupu-cli --lib coverage
cargo clippy -p rupu-cli --lib -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli
git commit -m "feat(cli): coverage audit + gap subcommands (json + human render)"
```

---

## Task 8: `ToolMappings` config type + loader

**Files:**
- Create: `crates/rupu-coverage/src/tool_mappings.rs`
- Modify: `crates/rupu-coverage/src/lib.rs`
- Test: inline

- [ ] **Step 1: Create `tool_mappings.rs`**

```rust
//! User-declared mappings that teach the coverage harness how to extract
//! a file path from an otherwise-unrecognized (e.g. MCP-provided) tool's
//! input, so it can emit FileTouchEvents. Loaded from
//! `.rupu/coverage/tool-mappings.yaml`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One tool's path-extraction rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMapping {
    /// JSON key in the tool's input object that holds the file path.
    pub path_arg: String,
    /// Touch kind to record (defaults to "read").
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "read".to_string()
}

/// Map of tool name → extraction rule.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolMappings {
    pub tools: BTreeMap<String, ToolMapping>,
}

impl ToolMappings {
    pub fn get(&self, tool: &str) -> Option<&ToolMapping> {
        self.tools.get(tool)
    }
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// Load `.rupu/coverage/tool-mappings.yaml` from a workspace. Returns an
/// empty mapping (not an error) when the file is absent.
pub fn load_tool_mappings(workspace: &Path) -> Result<ToolMappings, serde_yaml::Error> {
    let path = workspace
        .join(".rupu")
        .join("coverage")
        .join("tool-mappings.yaml");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Ok(ToolMappings::default());
    };
    serde_yaml::from_str(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_yields_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let m = load_tool_mappings(tmp.path()).unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn parses_mappings_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join(".rupu/coverage");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("tool-mappings.yaml"),
            "cat_file:\n  path_arg: path\nread_doc:\n  path_arg: file\n  kind: read\n",
        )
        .unwrap();
        let m = load_tool_mappings(tmp.path()).unwrap();
        assert_eq!(m.get("cat_file").unwrap().path_arg, "path");
        assert_eq!(m.get("cat_file").unwrap().kind, "read"); // default
        assert_eq!(m.get("read_doc").unwrap().path_arg, "file");
    }
}
```

- [ ] **Step 2: Re-export**

Edit `crates/rupu-coverage/src/lib.rs`: add `pub mod tool_mappings;` and `pub use tool_mappings::{load_tool_mappings, ToolMapping, ToolMappings};`.

- [ ] **Step 3: Run tests + clippy**

```bash
cargo test -p rupu-coverage
cargo clippy -p rupu-coverage --tests -- -D warnings
```

Expected: prior + 2 new pass; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): tool-mappings config type + loader"
```

---

## Task 9: Wire tool-mappings into `rupu-tools` unknown-tool emit

**Files:**
- Modify: `crates/rupu-tools/src/tool.rs` (add `tool_mappings` to `ToolContext`)
- Modify: `crates/rupu-tools/src/coverage_emit.rs` (resolve path for mapped unknown tools)
- Test: `crates/rupu-tools/tests/coverage_instrumentation.rs`

- [ ] **Step 1: Read the current unknown-tool emit path**

```bash
cat crates/rupu-tools/src/coverage_emit.rs
grep -n "Unknown\|unknown\|tool_call_observed" crates/rupu-tools/src/*.rs
```

Understand how/where an unrecognized tool currently emits (Plan 1 design: a `tool_call_observed`-style event with no path, OR nothing). Find where the dispatcher invokes tools and whether there's a central place that knows the tool name + input for tools NOT in the built-in instrumented set (read_file/grep/glob/edit_file/bash).

If there is NO central unknown-tool hook today (the built-in tools self-instrument; MCP/other tools simply don't emit), then this task adds one: at the tool-dispatch site (or wherever ToolContext is available alongside the tool name + JSON input for an arbitrary tool), consult `ctx.tool_mappings`; if the tool name has a mapping and the input JSON has the `path_arg` key with a string value, emit a `FileTouchEvent::Read` (or the mapping's kind) for that path.

**If this requires touching code outside rupu-tools** (e.g. the agent runner dispatches MCP tools), STOP and report NEEDS_CONTEXT describing where unknown tools are actually invoked — we may need to scope the wiring there. Do NOT guess a hook location.

- [ ] **Step 2: Add `tool_mappings` to `ToolContext`**

In `crates/rupu-tools/src/tool.rs`, add (mirroring the existing `coverage_writer` optional field):

```rust
/// Optional user-declared tool→path-arg mappings, so unrecognized tools
/// can still contribute FileTouchEvents. None = no mappings.
pub tool_mappings: Option<std::sync::Arc<rupu_coverage::ToolMappings>>,
```

Default to `None` in the `Default` impl and update all `ToolContext { ... }` construction sites (grep for them) to add `tool_mappings: None`.

- [ ] **Step 3: Add a resolver in `coverage_emit.rs`**

```rust
use rupu_coverage::ToolMappings;

/// If `tool_name` has a mapping and `input` carries the mapped path arg,
/// return a FileTouchEvent for it. Used for unrecognized tools.
pub fn mapped_touch(
    ctx: &ToolContext,
    tool_name: &str,
    input: &serde_json::Value,
) -> Option<rupu_coverage::FileTouchEvent> {
    let mappings: &ToolMappings = ctx.tool_mappings.as_deref()?;
    let mapping = mappings.get(tool_name)?;
    let path = input.get(&mapping.path_arg)?.as_str()?.to_string();
    let attribution = attribution_from(ctx);
    // Map the configured kind string to a Read event (the only path-bearing
    // mapped kind for v1; extend later if needed).
    Some(rupu_coverage::FileTouchEvent::Read {
        path,
        line_range: [0, 0],
        tool: tool_name.to_string(),
        attribution,
        at: chrono::Utc::now(),
    })
}
```

(`attribution_from(ctx)` already exists in this module. Adapt field names to the real `FileTouchEvent::Read` shape. `line_range: [0,0]` denotes "whole/unknown range" for a mapped tool; document that in a comment.)

- [ ] **Step 4: Call it from wherever unknown tools are dispatched**

Based on Step 1's finding, insert the emit at the right place (after a non-built-in tool succeeds). If the only viable hook is in the agent runner, implement it there instead and note the deviation. The call:

```rust
if let Some(event) = crate::coverage_emit::mapped_touch(ctx, tool_name, &input) {
    crate::coverage_emit::emit(ctx, event).await;
}
```

- [ ] **Step 5: Integration test**

Add to `crates/rupu-tools/tests/coverage_instrumentation.rs`:

```rust
#[tokio::test]
async fn mapped_unknown_tool_emits_read_event() {
    use rupu_coverage::{ToolMapping, ToolMappings};
    use std::collections::BTreeMap;

    let tmp = tempfile::TempDir::new().unwrap();
    let paths = rupu_coverage::CoveragePaths::new(tmp.path(), "t");
    let handle = rupu_coverage::CoverageWriterHandle::spawn(paths.clone()).unwrap();

    let mut tools = BTreeMap::new();
    tools.insert("cat_file".to_string(), ToolMapping { path_arg: "path".to_string(), kind: "read".to_string() });
    let mappings = std::sync::Arc::new(ToolMappings { tools });

    // Build a ToolContext with the writer + mappings, call mapped_touch + emit.
    // (Use the same ToolContext construction the other tests in this file use.)
    {
        let mut ctx = rupu_tools::tool::ToolContext::default();
        ctx.workspace_path = tmp.path().to_path_buf();
        ctx.coverage_writer = Some(handle.writer.clone());
        ctx.surface_tag = Some("workflow".to_string());
        ctx.run_id = Some("r".to_string());
        ctx.model = Some("m".to_string());
        ctx.tool_mappings = Some(mappings);
        let input = serde_json::json!({ "path": "src/x.rs" });
        if let Some(ev) = rupu_tools::coverage_emit::mapped_touch(&ctx, "cat_file", &input) {
            rupu_tools::coverage_emit::emit(&ctx, ev).await;
        }
    } // ctx dropped (releases its writer Arc clone) before shutdown
    handle.shutdown().await;

    let body = std::fs::read_to_string(&paths.files).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    let ev: rupu_coverage::FileTouchEvent = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(ev.path(), Some("src/x.rs"));
}
```

(Adapt visibility: if `coverage_emit::mapped_touch` / `emit` aren't `pub` at the crate root, make them reachable for the integration test, or move this to an inline `#[cfg(test)]` test in coverage_emit.rs where they're in scope.)

- [ ] **Step 6: Build + test + clippy + workspace**

```bash
cargo test -p rupu-tools
cargo build --workspace --tests
cargo clippy -p rupu-tools --tests -- -D warnings
```

Expected: new test passes; workspace test-build clean; clippy clean.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-tools Cargo.lock
git commit -m "feat(tools): emit FileTouchEvents for mapped unknown tools via tool-mappings"
```

---

## Task 10: End-to-end CLI audit test

**Files:**
- Create: `crates/rupu-cli/tests/coverage_audit_cli.rs`

- [ ] **Step 1: Write an integration test driving the CLI entrypoint**

`rupu-cli` exposes `pub async fn run(args: Vec<String>) -> ExitCode` (the testable entrypoint). Drive `rupu coverage audit` against a hand-built target dir.

```rust
//! End-to-end: populate a coverage target on disk, then run
//! `rupu coverage audit <id> --json` through the CLI entrypoint.

use rupu_coverage::{
    flatten, write_snapshot, AssertionStatus, Attribution, ConcernAssertion, ConcernsBlock,
    ConcernsEntry, CatalogMode, CoveragePaths, Evidence, FileTouchEvent, IncludeDirective, Surface,
};
use chrono::Utc;

#[tokio::test]
async fn coverage_audit_cli_runs_on_populated_target() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(tmp.path(), "tgt");
    paths.ensure_dir().unwrap();

    let cat = flatten(&ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    })
    .unwrap();
    write_snapshot(&cat, &paths.catalog).unwrap();

    let attribution = Attribution { run_id: "r".into(), model: "m".into(), surface: Surface::Workflow };
    let touch = FileTouchEvent::Read {
        path: "src/auth/login.rs".into(),
        line_range: [1, 80],
        tool: "read_file".into(),
        attribution: attribution.clone(),
        at: Utc::now(),
    };
    std::fs::write(&paths.files, serde_json::to_string(&touch).unwrap() + "\n").unwrap();

    let assertion = ConcernAssertion {
        concern_id: "stride:spoofing".into(),
        file_path: "src/auth/login.rs".into(),
        status: AssertionStatus::Clean,
        evidence: Evidence { summary: "ok".into(), line_ranges: vec![], finding_ids: vec![] },
        declared_by: attribution,
        declared_at: Utc::now(),
    };
    std::fs::write(&paths.concerns, serde_json::to_string(&assertion).unwrap() + "\n").unwrap();

    // Run from the workspace dir so the CLI resolves .rupu/coverage/tgt.
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();
    let code = rupu_cli::run(vec![
        "rupu".into(),
        "coverage".into(),
        "audit".into(),
        "tgt".into(),
        "--json".into(),
    ])
    .await;
    std::env::set_current_dir(prev).unwrap();

    assert_eq!(code, std::process::ExitCode::SUCCESS);
}
```

Verify `rupu_cli::run` is the actual public entrypoint signature (from `crates/rupu-cli/src/lib.rs`). If `ExitCode` equality isn't derivable for assertion, assert via `format!("{code:?}")` or check the function returns without panic. Adapt as needed.

Note the cwd-mutation caveat — if rupu-cli integration tests run in parallel and clash on `set_current_dir`, this test may be flaky. If the crate already has integration tests that set cwd, follow their pattern; otherwise, consider gating with a serial-test mechanism if one exists, or accept it as the single cwd-mutating integration test.

- [ ] **Step 2: Run**

```bash
cargo test -p rupu-cli --test coverage_audit_cli 2>&1 | tail -10
```

Expected: pass.

- [ ] **Step 3: Final workspace verification**

```bash
cargo build --workspace --tests 2>&1 | tail -3
cargo test -p rupu-coverage 2>&1 | grep "test result" | tail -3
cargo clippy -p rupu-coverage -p rupu-tools -- -D warnings 2>&1 | tail -5
```

Expected: workspace test-build clean; rupu-coverage tests pass; clippy clean on the coverage crates. (rupu-cli has pre-existing clippy noise under the newer toolchain — confirm no NEW issues in the coverage module specifically.)

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/tests/coverage_audit_cli.rs
git commit -m "test(cli): end-to-end coverage audit via CLI entrypoint"
```

---

## Self-review

**1. Spec coverage:**

| Spec requirement (Plan 3 scope) | Task |
| --- | --- |
| Audit: per-concern in-scope/asserted/gap | Task 3 |
| Audit: per-file expected-but-missing | Task 3 (file_coverage) |
| Audit: cross-model agreement/disagreement | Task 3 (cross_model) |
| Audit: serendipitous findings clustering | Task 3 (serendipitous) |
| `read_findings` ledger reader | Task 1 |
| Target discovery | Task 4 |
| `rupu coverage list` | Task 5 |
| `rupu coverage templates list/show` | Task 5 |
| `rupu coverage catalog` | Task 6 |
| `rupu coverage show` | Task 6 |
| `rupu coverage audit` (json + human) | Task 7 |
| `rupu coverage gap` | Task 7 |
| tool-mappings.yaml config + loader | Task 8 |
| tool-mappings wired into emit | Task 9 |
| e2e CLI audit | Task 10 |
| session footer + /coverage | **DEFERRED** (needs live validation) |

**2. Placeholder scan:** No "TBD"/"handle edge cases". Tasks 5/7/9/10 flag real adaptation points (exact `OutputFormat` type, exact unknown-tool hook location, cwd-mutation flakiness) with explicit "verify against reality / report NEEDS_CONTEXT if different" guidance rather than guessing — that's deliberate because those are genuine codebase-shape unknowns, not hand-waving. Task 9 explicitly says STOP+report if the unknown-tool hook doesn't exist where assumed.

**3. Type consistency:** `AuditReport`/`ConcernCoverage`/`FileCoverage`/`CrossModelEntry`/`SerendipitousCluster` defined in Task 2, consumed in Tasks 3/6/7/10. `run_audit` (the lib.rs alias for `audit::generate::audit`) named consistently in Tasks 3, 7, 10. `read_findings` from Task 1 used in Tasks 3, 6. `ToolMappings`/`ToolMapping`/`load_tool_mappings` from Task 8 used in Task 9. `CoveragePaths`, `file_views`, `read_file_events`, `read_concern_assertions`, `read_snapshot` are pre-existing rupu-coverage exports.

**4. Risk note:** The biggest uncertainty is Task 9 (where unknown/MCP tools are dispatched — the hook may live in rupu-agent, not rupu-tools). The task is written to STOP and report rather than guess, so it won't produce wrong wiring. If it reports NEEDS_CONTEXT, the controller scopes the hook with the implementer.

---

## Execution

Plan complete and saved to `docs/superpowers/plans/2026-05-29-rupu-coverage-harness-plan-3a-cli-and-audit.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, two-stage review, fast iteration.

**2. Inline Execution** — execute in this session with checkpoints.

Which approach?
