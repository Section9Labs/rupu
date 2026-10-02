# Finding Reports Plan 5: Backfill Importer

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rupu findings import <PATH>…` reads finding reports written as Markdown in the report layout and attaches each one to the summary finding it cites. The finding becomes a full-profile finding.

**Architecture:** Three layers, each in the crate that owns it:
- A pure, best-effort parser (`rupu_findings_report::import`) turns one Markdown report into a `FindingReport` and lists the finding ids it cites.
- `rupu_coverage::tools::attach_report::attach_reports` rewrites the matching ledger lines under a sidecar lock, keeps a backup, and runs the same validation and artifact path `report_finding` uses.
- A read-only locator (`rupu_cp::api::findings::finding_ledgers`) finds which ledger holds each id.
- The CLI glues these together and prints one line per file.

**Tech Stack:** Rust 2021 (toolchain 1.95: `std::fs::File::lock` is available), serde_json, chrono, thiserror, clap, assert_cmd.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md`, section "Backfill (last, optional)".

## Global Constraints

- **Public repo.** Every fixture, test string and doc example is invented. Use the toy "Notebin" notes app and `example.com` URLs. Never adapt real assessment content. Never use org-specific names ("TVM", real vendor trackers).
- **Migration aid, not an input path** (spec). A report either attaches whole or is listed as not imported. Nothing is partially written.
- **Nothing is invented** for a missing field:
  - A required section or field with no sentinel that is missing fails that report.
  - A missing one that allows a sentinel gets it: `Unknown`, `None`, or `Not Provided — section missing from the imported report`.
  - A missing *part* of a section that is present is `Not stated in the imported report.`
- **Text with no typed home is kept**, never dropped.
- **`rupu-cli` stays thin.** Parsing lives in `rupu-findings-report` and ledger rewriting in `rupu-coverage`. `rupu-cp` stays read-only; it only gains a lookup.
- Workspace dependency versions only. No new third-party crates.
- `#![deny(clippy::all)]`, `unsafe_code` forbidden. Gate: `cargo clippy --workspace --all-targets -- -D warnings -A clippy::question_mark`.
- **rustfmt only the leaf files you touch** (`rustfmt --edition 2021 <file>`). Never `cargo fmt`, and never rustfmt a `lib.rs` or `mod.rs`: rustfmt follows `mod` declarations and reformats the whole crate.
- **Git:** never run bare `git stash` or `git stash pop`, and stage only your own files.
- The macOS app is out of scope. No CP serde type changes, so `make macos-fixtures` shows no drift. Run it once in Task 3 to confirm.

## File Structure

| File | Responsibility |
|---|---|
| `crates/rupu-coverage/src/tools/report_finding.rs` (modify) | Extract `check_full_report` / `prepare_full_report` / `derived_fields` / `lock_findings`. The append takes the sidecar lock. |
| `crates/rupu-coverage/src/tools/attach_report.rs` (create) | `attach_reports`: lock, index, validate/prepare, rewrite atomically, backup. |
| `crates/rupu-coverage/src/tools/mod.rs` (modify) | `pub mod attach_report;` + re-exports. |
| `crates/rupu-coverage/tests/attach_report.rs` (create) | Behaviour tests for attach. |
| `crates/rupu-findings-report/src/import.rs` (create) | `parse_report`, `fnd_ids`, `retain_known_cross_references`, `NOT_STATED`. |
| `crates/rupu-findings-report/src/lib.rs` (modify) | `pub mod import;` (edit by hand; never rustfmt it). |
| `crates/rupu-findings-report/tests/fixtures/import/{notebin_plain.md,notebin_markdown.md}` (create) | Invented reports in the two layout spellings. |
| `crates/rupu-findings-report/tests/import.rs` (create) | Parser tests, including a round trip through the Markdown exporter. |
| `crates/rupu-cp/src/api/findings.rs` (modify) | `finding_ledgers`, sharing one workspace/target walk with `collect_all_findings`. |
| `crates/rupu-cli/src/cmd/findings.rs` (modify) | `Import(ImportArgs)` action. |
| `crates/rupu-cli/tests/findings_import.rs` (create) | End-to-end through the binary. |
| `docs/coverage.md`, `CLAUDE.md`, the spec (modify) | Document the importer. |

---

### Task 1: Attach a report to an existing finding (rupu-coverage)

**Files:**
- Modify: `crates/rupu-coverage/src/tools/report_finding.rs`
- Create: `crates/rupu-coverage/src/tools/attach_report.rs`
- Modify: `crates/rupu-coverage/src/tools/mod.rs` (hand edit: add `pub mod attach_report;` and `pub use attach_report::{attach_reports, AttachBatch, AttachItem, AttachOutcome};`)
- Test: `crates/rupu-coverage/tests/attach_report.rs`

**Interfaces:**
- Produces:
  - `rupu_coverage::tools::attach_report::{attach_reports, AttachItem, AttachOutcome, AttachBatch}`, re-exported from `rupu_coverage::tools`.
  - `attach_reports(paths: &CoveragePaths, items: Vec<AttachItem>, opts: &FindingWriteOptions, dry_run: bool) -> std::io::Result<AttachBatch>`
  - `AttachItem { finding_id: String, report: FindingReport }`
  - `AttachOutcome::{Attached, NotFound, AlreadyHasReport, Duplicate, Rejected(ReportFindingError)}`
  - `AttachBatch { outcomes: Vec<AttachOutcome> /* input order */, backup: Option<PathBuf> }`

- [ ] **Step 1: Refactor `report_finding` (no behaviour change)**

In `report_finding.rs`, move the full-profile validation and preparation out of the `FindingProfile::Full` arm into three crate-visible functions. Add a lock helper. The `Full` arm becomes:

```rust
        FindingProfile::Full => {
            if input.summary.is_some() || input.severity.is_some() || input.evidence.is_some() {
                return Err(ReportFindingError::DerivedFieldsSupplied);
            }
            let report = input.report.ok_or(ReportFindingError::ReportRequired)?;
            let known: Vec<String> = crate::ledger::read_findings(paths)?
                .into_iter()
                .map(|f| f.id)
                .collect();
            let report = prepare_full_report(paths, report, &known, opts)?;
            let (summary, severity, evidence) = derived_fields(&report);
            (summary, severity, evidence, Some(report))
        }
```

The new functions go below `report_finding`. Their bodies are the existing code, moved:

```rust
/// Everything `validate_report` checks, plus the rule that a report may not
/// carry its own `verification`: all problems at once, no I/O. The dry-run
/// half of [`prepare_full_report`].
pub(crate) fn check_full_report(
    report: &crate::report::FindingReport,
    known: &[String],
    opts: &crate::report::FindingWriteOptions,
) -> Result<(), ReportFindingError> {
    // `verification` is part of a stored record, but it is the
    // verdict of a later verification run: the agent that wrote the
    // finding cannot confirm its own work. Reported alongside every
    // other problem so the agent still fixes them all in one retry.
    let mut problems = Vec::new();
    if report.verification.is_some() {
        problems.push(crate::report::FieldError {
            path: "report.verification".into(),
            message: "set by verification runs, not by the reporting agent; omit it".into(),
        });
    }
    if let Err(crate::report::ReportValidationError(errors)) = crate::report::validate_report(
        report,
        &crate::report::ValidateCtx {
            known_finding_ids: known,
            max_bytes: opts.report_max_bytes,
        },
    ) {
        problems.extend(errors);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ReportFindingError::Report(crate::report::ReportValidationError(problems)))
    }
}

/// Validate a full-profile report and turn it into what the ledger stores:
/// claim hashes recomputed, artifacts ingested into the store, the size
/// budget re-checked. Shared by `report_finding` (a new finding) and
/// `attach_reports` (a report imported onto an existing finding).
pub(crate) fn prepare_full_report(
    paths: &CoveragePaths,
    mut report: crate::report::FindingReport,
    known: &[String],
    opts: &crate::report::FindingWriteOptions,
) -> Result<crate::report::FindingReport, ReportFindingError> {
    check_full_report(&report, known, opts)?;
    // A claim's hash is rupu's record of the file at write time, not
    // something the agent can assert: drop whatever it sent, then
    // hash what is actually there (or leave it unset).
    for claim in &mut report.evidence {
        claim.sha256 = None;
    }
    if !report.artifacts.is_empty() {
        let store = opts
            .artifact_root
            .as_ref()
            .map(crate::report::ArtifactStore::new)
            .ok_or(crate::report::ArtifactError::NoStore)?;
        report.artifacts = store.ingest(&paths.workspace, &report.artifacts, opts.ingest_limits())?;
    }
    hash_claim_files(&paths.workspace, &mut report, opts.artifact_max_bytes);
    // Directory artifacts expand to one entry per file, so the report
    // can grow far past the budget `validate_report` checked. Re-check
    // before anything reaches the ledger.
    let size = serde_json::to_vec(&report)?.len();
    if size > opts.report_max_bytes {
        return Err(ReportFindingError::Report(
            crate::report::ReportValidationError(vec![crate::report::FieldError {
                path: "report.artifacts".into(),
                message: format!(
                    "after expanding directories the report is {size} bytes, over the {} byte budget ({} artifact files); list specific files instead of large directories",
                    opts.report_max_bytes,
                    report.artifacts.len()
                ),
            }]),
        ));
    }
    Ok(report)
}

/// The record fields a full-profile finding derives from its report.
pub(crate) fn derived_fields(
    report: &crate::report::FindingReport,
) -> (String, Severity, FindingEvidence) {
    (
        report.title.clone(),
        Severity::from(report.rating.risk_rating),
        FindingEvidence {
            code_excerpt: report.evidence.iter().find_map(|c| c.excerpt.clone()),
            rationale: report.root_cause.clone(),
            references: Vec::new(),
        },
    )
}

/// Serializes the writers of one target's findings ledger. The lock is a
/// sidecar file, not the ledger itself: `attach_reports` replaces the ledger
/// by rename, and a lock held on the replaced file would not exclude the
/// next writer. Released when the returned handle drops.
pub(crate) fn lock_findings(paths: &CoveragePaths) -> std::io::Result<std::fs::File> {
    paths.ensure_dir()?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.root.join("findings.jsonl.lock"))?;
    f.lock()?;
    Ok(f)
}
```

In `report_finding`, take the lock just before the append handle is opened. Replace `paths.ensure_dir()?;` with:

```rust
    // Held across the append so it cannot interleave with an import's
    // rewrite of this ledger.
    let _lock = lock_findings(paths)?;
```

Run: `cargo test -p rupu-coverage`
Expected: PASS. The refactor changes no behaviour, so the existing tests cover it.

- [ ] **Step 2: Write the failing attach tests**

Create `crates/rupu-coverage/tests/attach_report.rs`. The summary record is seeded through the real `report_finding` under the summary profile. Build its `ReportFindingInput` the way the existing `report_finding` unit tests do (`scope: FindingScope::Line` with `file_path` + `line_range`, `summary`, `severity`, `evidence`), and adapt the helper below to their field names.

```rust
//! `attach_reports`: a report imported onto an existing summary finding.

use rupu_coverage::report::{FindingProfile, FindingReport, FindingWriteOptions};
use rupu_coverage::tools::report_finding::{report_finding, ReportFindingInput};
use rupu_coverage::tools::{attach_reports, AttachItem, AttachOutcome};
use rupu_coverage::{
    read_findings, Attribution, CoveragePaths, FindingEvidence, FindingScope, Severity, Surface,
};

fn report() -> FindingReport {
    serde_json::from_str(include_str!("fixtures/finding_report/valid_full.json")).unwrap()
}

fn attribution() -> Attribution {
    Attribution {
        run_id: "run_old".into(),
        model: "m".into(),
        surface: Surface::Workflow,
    }
}

/// A summary finding written the way pre-full-profile agents wrote them.
fn seed_summary(paths: &CoveragePaths) -> String {
    let input = ReportFindingInput {
        file_path: Some("src/routes/notes.rs".into()),
        line_range: Some([40, 58]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: Some("Note lookup ignores the owner".into()),
        severity: Some(Severity::High),
        concern_id: Some("authz-idor".into()),
        evidence: Some(FindingEvidence {
            code_excerpt: Some("store.find_by_id(id)".into()),
            rationale: "The id is the only key.".into(),
            references: vec![],
        }),
        report: None,
    };
    let opts = FindingWriteOptions::default().with_profile(FindingProfile::Summary);
    report_finding(paths, attribution(), input, &opts).unwrap().id
}

fn setup() -> (tempfile::TempDir, CoveragePaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(dir.path(), "tgt1");
    (dir, paths)
}

fn full_opts() -> FindingWriteOptions {
    FindingWriteOptions::default().with_profile(FindingProfile::Full)
}

#[test]
fn attaches_onto_a_summary_finding_and_keeps_everything_else() {
    let (_d, paths) = setup();
    let a = seed_summary(&paths);
    let b = seed_summary(&paths);
    let before = std::fs::read_to_string(&paths.findings).unwrap();

    let batch = attach_reports(
        &paths,
        vec![AttachItem { finding_id: b.clone(), report: report() }],
        &full_opts(),
        false,
    )
    .unwrap();

    assert!(matches!(batch.outcomes.as_slice(), [AttachOutcome::Attached]));
    let backup = batch.backup.expect("a backup is kept");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), before);
    let after = std::fs::read_to_string(&paths.findings).unwrap();
    // The untouched line is byte-identical.
    assert_eq!(after.lines().next(), before.lines().next());

    let records = read_findings(&paths).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, a);
    assert!(records[0].report.is_none());
    let r = &records[1];
    assert_eq!(r.id, b);
    assert_eq!(r.profile, FindingProfile::Full);
    assert_eq!(r.report.as_ref().unwrap().title, report().title);
    // Derived exactly as `report_finding` derives them for a full finding.
    assert_eq!(r.summary, report().title);
    assert_eq!(r.severity, Severity::Critical);
    assert_eq!(r.evidence.rationale, report().root_cause);
    // Identity and provenance are the original record's.
    assert_eq!(r.declared_by.run_id, "run_old");
    assert_eq!(r.concern_id.as_deref(), Some("authz-idor"));
    assert_eq!(r.file_path.as_deref(), Some("src/routes/notes.rs"));
}

#[test]
fn a_dry_run_validates_and_writes_nothing() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let before = std::fs::read_to_string(&paths.findings).unwrap();
    let batch = attach_reports(
        &paths,
        vec![AttachItem { finding_id: id, report: report() }],
        &full_opts(),
        true,
    )
    .unwrap();
    assert!(matches!(batch.outcomes.as_slice(), [AttachOutcome::Attached]));
    assert!(batch.backup.is_none());
    assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), before);
    let entries: Vec<_> = std::fs::read_dir(&paths.root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.contains("pre-import"))
        .collect();
    assert!(entries.is_empty(), "{entries:?}");
}

#[test]
fn outcomes_for_unknown_full_invalid_and_repeated_ids() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let full = attach_reports(
        &paths,
        vec![AttachItem { finding_id: id.clone(), report: report() }],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(full.outcomes.as_slice(), [AttachOutcome::Attached]));

    let other = seed_summary(&paths);
    let mut bad = report();
    bad.title = "   ".into();
    let batch = attach_reports(
        &paths,
        vec![
            AttachItem { finding_id: "fnd_missing".into(), report: report() },
            AttachItem { finding_id: id, report: report() },
            AttachItem { finding_id: other.clone(), report: bad },
            AttachItem { finding_id: other.clone(), report: report() },
            AttachItem { finding_id: other, report: report() },
        ],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(batch.outcomes[0], AttachOutcome::NotFound));
    assert!(matches!(batch.outcomes[1], AttachOutcome::AlreadyHasReport));
    match &batch.outcomes[2] {
        AttachOutcome::Rejected(e) => assert!(e.to_string().contains("report.title"), "{e}"),
        o => panic!("expected Rejected, got {o:?}"),
    }
    // The first valid item for an id attaches; a second one for the same
    // id in the same batch is refused rather than silently overwriting.
    assert!(matches!(batch.outcomes[3], AttachOutcome::Attached));
    assert!(matches!(batch.outcomes[4], AttachOutcome::Duplicate));
}

#[test]
fn a_full_finding_whose_report_no_longer_parses_is_never_rewritten() {
    let (_d, paths) = setup();
    // A full-profile line whose report fails to parse loads with
    // `report: None`. Rewriting it would destroy the stored report.
    let id = seed_summary(&paths);
    let mut v: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&paths.findings).unwrap().trim()).unwrap();
    v["profile"] = serde_json::json!("full");
    v["report"] = serde_json::json!({ "title": "only a title" });
    let line = format!("{}\n", serde_json::to_string(&v).unwrap());
    std::fs::write(&paths.findings, &line).unwrap();
    assert!(read_findings(&paths).unwrap()[0].report.is_none(), "loads leniently");
    let batch = attach_reports(
        &paths,
        vec![AttachItem { finding_id: id, report: report() }],
        &full_opts(),
        false,
    )
    .unwrap();
    assert!(matches!(batch.outcomes.as_slice(), [AttachOutcome::AlreadyHasReport]));
    assert_eq!(std::fs::read_to_string(&paths.findings).unwrap(), line);
}

#[test]
fn unparseable_lines_and_unknown_keys_survive_a_rewrite() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    let mut ledger = std::fs::read_to_string(&paths.findings).unwrap();
    // A future key on the record, and a line nothing can parse.
    ledger = ledger.replacen("{\"id\"", "{\"future_key\":7,\"id\"", 1);
    ledger.push_str("not json at all\n");
    std::fs::write(&paths.findings, &ledger).unwrap();

    attach_reports(
        &paths,
        vec![AttachItem { finding_id: id, report: report() }],
        &full_opts(),
        false,
    )
    .unwrap();
    let after = std::fs::read_to_string(&paths.findings).unwrap();
    assert!(after.contains("\"future_key\":7"), "{after}");
    assert!(after.ends_with("not json at all\n"), "{after}");
}

#[test]
fn a_new_finding_can_still_be_appended_after_an_attach() {
    let (_d, paths) = setup();
    let id = seed_summary(&paths);
    attach_reports(
        &paths,
        vec![AttachItem { finding_id: id, report: report() }],
        &full_opts(),
        false,
    )
    .unwrap();
    seed_summary(&paths);
    assert_eq!(read_findings(&paths).unwrap().len(), 2);
    assert!(paths.root.join("findings.jsonl.lock").exists());
}
```

If a `FindingRecord` with an unknown key does not deserialize (the wire type denies unknown fields), drop the `future_key` half of `unparseable_lines_and_unknown_keys_survive_a_rewrite` and keep the unparseable-line half; say so in your report. If `rupu_coverage` does not re-export one of these names at the crate root (for example `read_findings`, `Surface`, `FindingScope`), import it from its module path (`rupu_coverage::ledger::…`) instead. Do not add new re-exports for the tests' sake.

Run: `cargo test -p rupu-coverage --test attach_report`
Expected: FAIL to compile (`attach_reports` does not exist).

- [ ] **Step 3: Implement `attach_reports`**

Create `crates/rupu-coverage/src/tools/attach_report.rs`:

```rust
//! Attach a report to an existing summary finding: the one place a finding
//! line is rewritten rather than appended. Used by `rupu findings import`,
//! the one-time migration of reports that were written beside summary
//! findings before the full profile existed (spec: "Backfill").
//!
//! A report attaches whole or not at all: it goes through the same
//! validation, claim hashing and artifact store as `report_finding`, and a
//! finding that already has a report is never touched. The ledger is
//! replaced atomically under the ledger lock, after a byte-for-byte backup,
//! and every other line (including ones nothing can parse, and keys this
//! version does not know) is written back unchanged.

use crate::ledger::events::FindingRecord;
use crate::ledger::paths::CoveragePaths;
use crate::report::{FindingProfile, FindingReport, FindingWriteOptions};
use crate::tools::report_finding::{
    check_full_report, derived_fields, lock_findings, prepare_full_report, ReportFindingError,
};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

/// A report to attach to the finding `finding_id`.
#[derive(Debug, Clone)]
pub struct AttachItem {
    pub finding_id: String,
    pub report: FindingReport,
}

#[derive(Debug)]
pub enum AttachOutcome {
    /// Written (on a dry run: would be; the report passed validation).
    Attached,
    /// No finding with this id in this ledger.
    NotFound,
    /// The finding already has a report (or is a full-profile finding whose
    /// report no longer parses); left unchanged.
    AlreadyHasReport,
    /// The id is on more than one ledger line, or an earlier item in this
    /// batch already targets it; left unchanged.
    Duplicate,
    /// The report failed validation, or its artifacts could not be stored.
    Rejected(ReportFindingError),
}

#[derive(Debug)]
pub struct AttachBatch {
    /// One outcome per item, in input order.
    pub outcomes: Vec<AttachOutcome>,
    /// A copy of the ledger as it was before the rewrite. `None` when
    /// nothing was written (a dry run, or no item attached).
    pub backup: Option<PathBuf>,
}

pub fn attach_reports(
    paths: &CoveragePaths,
    items: Vec<AttachItem>,
    opts: &FindingWriteOptions,
    dry_run: bool,
) -> std::io::Result<AttachBatch> {
    let _lock = lock_findings(paths)?;
    let raw = match std::fs::read_to_string(&paths.findings) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<String> = raw.lines().map(str::to_owned).collect();
    let mut at: HashMap<String, Vec<usize>> = HashMap::new();
    let mut records: HashMap<usize, FindingRecord> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if let Ok(r) = serde_json::from_str::<FindingRecord>(line) {
            at.entry(r.id.clone()).or_default().push(i);
            records.insert(i, r);
        }
    }
    let known: Vec<String> = at.keys().cloned().collect();

    let mut claimed: HashSet<usize> = HashSet::new();
    let mut changed = false;
    let mut outcomes = Vec::with_capacity(items.len());
    for item in items {
        let outcome = match at.get(&item.finding_id).map(Vec::as_slice) {
            None => AttachOutcome::NotFound,
            Some([i]) if claimed.contains(i) => AttachOutcome::Duplicate,
            Some([i]) => {
                let record = &records[i];
                if record.report.is_some() || record.profile == FindingProfile::Full {
                    AttachOutcome::AlreadyHasReport
                } else if dry_run {
                    match check_full_report(&item.report, &known, opts) {
                        Ok(()) => {
                            claimed.insert(*i);
                            AttachOutcome::Attached
                        }
                        Err(e) => AttachOutcome::Rejected(e),
                    }
                } else {
                    // Only a report that attaches claims the finding: a
                    // rejected one leaves it free for a later item.
                    match prepare_full_report(paths, item.report, &known, opts)
                        .and_then(|report| Ok(upgraded_line(&lines[*i], &report)?))
                    {
                        Ok(line) => {
                            claimed.insert(*i);
                            lines[*i] = line;
                            changed = true;
                            AttachOutcome::Attached
                        }
                        Err(e) => AttachOutcome::Rejected(e),
                    }
                }
            }
            Some(_) => AttachOutcome::Duplicate,
        };
        outcomes.push(outcome);
    }

    let backup = if changed {
        Some(replace_ledger(paths, &lines)?)
    } else {
        None
    };
    Ok(AttachBatch { outcomes, backup })
}

/// `line` with the report attached and the fields a full finding derives
/// from it. Edited as JSON, not re-serialized from `FindingRecord`, so keys
/// this version does not know survive.
fn upgraded_line(line: &str, report: &FindingReport) -> Result<String, serde_json::Error> {
    let mut v: serde_json::Value = serde_json::from_str(line)?;
    let (summary, severity, evidence) = derived_fields(report);
    let Some(obj) = v.as_object_mut() else {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "a finding line is not a JSON object",
        ));
    };
    obj.insert("summary".into(), summary.into());
    obj.insert("severity".into(), serde_json::to_value(severity)?);
    obj.insert("evidence".into(), serde_json::to_value(evidence)?);
    obj.insert("profile".into(), serde_json::to_value(FindingProfile::Full)?);
    obj.insert("report".into(), serde_json::to_value(report)?);
    serde_json::to_string(&v)
}

/// Back the ledger up byte for byte, then replace it with `lines` via a
/// temp file and a rename, so a reader sees the old ledger or the new one,
/// never a mix. Returns the backup's path.
fn replace_ledger(paths: &CoveragePaths, lines: &[String]) -> std::io::Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let base = paths.root.join(format!("findings.jsonl.pre-import-{stamp}"));
    let mut backup = base.clone();
    let mut n = 1;
    while backup.exists() {
        n += 1;
        backup = PathBuf::from(format!("{}-{n}", base.display()));
    }
    std::fs::copy(&paths.findings, &backup)?;

    let tmp = paths.root.join("findings.jsonl.import-tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        for line in lines {
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
        }
        f.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::metadata(&paths.findings)?.permissions())?;
    std::fs::rename(&tmp, &paths.findings)?;
    Ok(backup)
}
```

`ReportFindingError` already has `#[from] serde_json::Error`, so the `?` inside the `and_then` closure converts. If the compiler cannot infer the closure's error type, write `.and_then(|report| upgraded_line(&lines[*i], &report).map_err(ReportFindingError::from))`.

Register the module in `tools/mod.rs` by hand, as described under **Files**.

Run: `cargo test -p rupu-coverage`
Expected: PASS (new and existing tests).

- [ ] **Step 4: Lint and commit**

Run:
- `rustfmt --edition 2021 crates/rupu-coverage/src/tools/report_finding.rs crates/rupu-coverage/src/tools/attach_report.rs crates/rupu-coverage/tests/attach_report.rs`
- `cargo clippy -p rupu-coverage --all-targets -- -D warnings -A clippy::question_mark`

Both clean.

```bash
git add crates/rupu-coverage/src/tools/report_finding.rs crates/rupu-coverage/src/tools/attach_report.rs crates/rupu-coverage/src/tools/mod.rs crates/rupu-coverage/tests/attach_report.rs
git commit -m "feat(coverage): attach_reports — attach a report to an existing summary finding under a ledger lock"
```

---

### Task 2: Parse a Markdown report (rupu-findings-report)

**Files:**
- Create: `crates/rupu-findings-report/src/import.rs`
- Modify: `crates/rupu-findings-report/src/lib.rs` (hand edit: `pub mod import;` after `pub mod html;`; never rustfmt this file)
- Create: `crates/rupu-findings-report/tests/fixtures/import/notebin_plain.md`
- Create: `crates/rupu-findings-report/tests/fixtures/import/notebin_markdown.md`
- Test: `crates/rupu-findings-report/tests/import.rs`

**Interfaces:**
- Consumes: `rupu_coverage::report::*` types.
- Produces (all in `rupu_findings_report::import`):
  - `parse_report(md: &str) -> Result<Parsed, ImportError>`
  - `Parsed::{Report { report: FindingReport, cited_ids: Vec<String> }, NotAReport}`
  - `ImportError::{MissingSection(&'static str), MissingField(&'static str), BadValue { field, value, allowed }, SeveralFindings(usize)}`
  - `fnd_ids(text: &str) -> Vec<String>`
  - `retain_known_cross_references(report: &mut FindingReport, known: &HashSet<String>)`
  - `NOT_STATED: &str`

- [ ] **Step 1: Write the fixtures**

These are invented; do not change the facts in them. `notebin_plain.md` uses the plain-text spelling of the layout, with bare heading lines and unbolded fields. Use exactly this content:

````markdown
Filename: NB-001 - Notes API returns another user's note by id.pdf

Notes API returns another user's note by id

Identifier: NB-001
Finding ID: fnd_01J00000000000000000000001
Owner: Unknown
Product: Notebin (sample app)
Affected Component: GET /api/notes/{id} handler
Source Repository: example/notebin
Existing Ticket References: None Provided
Impact: High
Category: Authorization Bypass Through User-Controlled Key
Attack Vector: Authenticated HTTP request with another user's note id
Likelihood: High
Risk Rating: Critical

Description
The get_note handler loads a note by the id path parameter and returns it without checking that the note belongs to the signed-in user.

Impact
Any signed-in user can read every other user's notes by iterating ids.

Location
Input:
GET /api/notes/{id} path parameter id.

Output:
The JSON body of the note, including title and body.

Root Cause
NoteStore::find_by_id is called with only the note id; the owner id from the session is never part of the lookup.

Call Chain / Attack Flow
router GET /api/notes/{id} (src/app.rs:12-30) → get_note() (src/routes/notes.rs:40-58) → NoteStore::find_by_id() (src/store/notes.rs:88-97)

Evidence
The handler looks the note up by id alone (src/routes/notes.rs:40-58):

```rust
let note = store.find_by_id(id).await?;
Ok(Json(note))
```

Remediation
Scope the lookup to the session user and return 404 when the note belongs to someone else.

Recommended Patch
```diff
--- a/src/routes/notes.rs
+++ b/src/routes/notes.rs
@@
-    let note = store.find_by_id(id).await?;
+    let note = store.find_by_id_for_owner(id, session.user_id).await?;
```

CI/CD Detection
Integration test: user B requests user A's note and must get 404. Run it on every merge request.

Regression Test
notes_access::other_users_note_is_404 creates two users and one note.
Command: cargo test --test notes_access other_users_note_is_404
Vulnerable build: fails: got 200
Patched build: passes: got 404

Cross-References
None

References
CWE-639: Authorization Bypass Through User-Controlled Key; CWE-862: Missing Authorization; OWASP A01:2021 Broken Access Control

CVSS v3 Base Score: 8.1
Risk Factor: High

Replication Steps
Step 1: Sign up as user A and create a note; record its id.
Step 2: Sign up as user B.
Step 3: As user B, request GET /api/notes/{id} with user A's note id.
````

`notebin_markdown.md` uses the Markdown spelling: `##` headings, bold fields, a structured ticket list, cross-references, and a code block inside the call chain. Use exactly this content:

````markdown
Filename: NB-002 - Share links never expire.pdf

# Share links never expire

**Identifier:** NB-002
**Finding ID:** fnd_01J00000000000000000000002
**Owner:** Notebin core team
**Product:** Notebin (sample app)
**Affected Component:** Share link service
**Source Repository:** example/notebin
**Existing Ticket References:**
- Type: Example Tracker
  Identifier: NB-42
  URL: https://tracker.example.com/NB-42
  Notes: Product team tracking remediation
**Impact:** Medium
**Category:** Insufficient Session Expiration
**CWE:** CWE-613
**Attack Vector:** Replaying an old share link
**Likelihood:** Medium
**Risk Rating:** Medium

## Description

Share links are signed tokens with no expiry claim, so a link keeps working after the owner stops sharing the note.

## Impact

Anyone who ever received a link can read the note for as long as it exists.

## Location

**Input:** `GET /s/{token}`

**Output:** The shared note page.

## Root Cause

`ShareToken::verify` checks the signature but the token format has no `exp` field to check.

## Call Chain / Attack Flow

Step 1: `GET /s/{token}` route (`src/app.rs:44-46`)
Step 2: `open_shared()` (`src/routes/share.rs:10-22`)

```rust
let claims = ShareToken::verify(&token)?;
```

Step 3: `ShareToken::verify()` (`src/share/token.rs:30-41`)

## Evidence

- The token claims have no expiry (`src/share/token.rs:5-9`).
- Verification never compares a time (`src/share/token.rs:30-41`).

```rust
pub fn verify(t: &str) -> Result<Claims> { check_sig(t) }
```

## Remediation

Add an `exp` claim when a link is minted and reject expired tokens in `verify`.

## Recommended Patch

```diff
--- a/src/share/token.rs
+++ b/src/share/token.rs
@@
-pub fn verify(t: &str) -> Result<Claims> { check_sig(t) }
+pub fn verify(t: &str) -> Result<Claims> { check_sig(t).and_then(check_exp) }
```

Existing links minted without `exp` should be treated as expired.

## CI/CD Detection

**Stage:** nightly

A test mints a token with a past `exp` and requires `verify` to reject it.

**Command:** cargo test --test share_expiry

**Fails when:** an expired token verifies

## Regression Test

`share_expiry::expired_link_is_rejected` mints a token that expired an hour ago.

```sh
cargo test --test share_expiry expired_link_is_rejected
```

**Vulnerable build:** fails: token accepted

**Patched build:** passes: token rejected

## Cross-References

- fnd_01J00000000000000000000001 is a prerequisite: share pages load notes through the same store.
- NB-007 covers link revocation.

## References

OWASP A07:2021 Identification and Authentication Failures

**CVSS v3 Base Score:** 6.5
**Risk Factor:** Medium

## Replication Steps

Step 1: As user A, share a note and copy the link.
Step 2: Stop sharing the note.
Step 3: Open the copied link in a private window; the note still loads.
````

- [ ] **Step 2: Write the failing tests**

Create `crates/rupu-findings-report/tests/import.rs`:

```rust
//! Best-effort import of Markdown finding reports back into `FindingReport`.

mod common;

use common::*;
use rupu_coverage::report::{
    validate_report, HopRole, Likelihood, OrSentinel, Relation, RiskLevel, ValidateCtx,
    NOT_PROVIDED_PREFIX,
};
use rupu_coverage::Severity;
use rupu_findings_report::import::{
    fnd_ids, parse_report, retain_known_cross_references, ImportError, Parsed, NOT_STATED,
};
use rupu_findings_report::number::number_map;
use rupu_findings_report::{render_finding, Format};
use std::collections::HashSet;

const PLAIN: &str = include_str!("fixtures/import/notebin_plain.md");
const MARKDOWN: &str = include_str!("fixtures/import/notebin_markdown.md");
const ID1: &str = "fnd_01J00000000000000000000001";
const ID2: &str = "fnd_01J00000000000000000000002";

fn report_of(md: &str) -> (rupu_coverage::FindingReport, Vec<String>) {
    match parse_report(md).expect("parses") {
        Parsed::Report { report, cited_ids } => (report, cited_ids),
        Parsed::NotAReport => panic!("expected a report"),
    }
}

fn assert_valid(r: &rupu_coverage::FindingReport, known: &[&str]) {
    let known: Vec<String> = known.iter().map(|s| s.to_string()).collect();
    validate_report(r, &ValidateCtx { known_finding_ids: &known, max_bytes: 256 * 1024 })
        .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_plain_layout_parses_into_a_valid_report() {
    let (r, cited) = report_of(PLAIN);
    assert_eq!(cited, vec![ID1.to_string()]);
    assert_eq!(r.title, "Notes API returns another user's note by id");
    assert_eq!(r.ownership.owner, "Unknown");
    assert_eq!(r.ownership.product, "Notebin (sample app)");
    assert_eq!(r.tickets, OrSentinel::Sentinel("None Provided".into()));
    assert_eq!(r.rating.impact, RiskLevel::High);
    assert_eq!(r.rating.likelihood, Likelihood::High);
    assert_eq!(r.rating.risk_rating, RiskLevel::Critical);
    assert_eq!(r.rating.risk_factor, RiskLevel::High);
    assert_eq!(r.rating.cvss_v3, "8.1");
    assert_eq!(r.cwe, vec!["CWE-639".to_string(), "CWE-862".to_string()]);
    assert_eq!(r.location.input, "GET /api/notes/{id} path parameter id.");
    assert_eq!(r.location.output, "The JSON body of the note, including title and body.");
    let OrSentinel::Value(hops) = &r.call_chain else { panic!("{:?}", r.call_chain) };
    assert_eq!(hops.len(), 3);
    assert_eq!(hops[0].role, HopRole::Source);
    assert_eq!(hops[1].role, HopRole::Hop);
    assert_eq!(hops[2].role, HopRole::Sink);
    assert_eq!(hops[1].file.as_deref(), Some("src/routes/notes.rs"));
    assert_eq!(hops[1].lines, Some([40, 58]));
    assert_eq!(r.evidence.len(), 1);
    assert_eq!(r.evidence[0].file.as_deref(), Some("src/routes/notes.rs"));
    assert_eq!(r.evidence[0].lang.as_deref(), Some("rust"));
    assert!(r.evidence[0].excerpt.as_deref().unwrap().contains("find_by_id(id)"));
    let OrSentinel::Value(p) = &r.recommended_patch else { panic!() };
    assert!(p.diff.contains("find_by_id_for_owner"));
    assert_eq!(p.notes, None);
    let OrSentinel::Value(ci) = &r.ci_cd_detection else { panic!() };
    assert_eq!(ci.stage, NOT_STATED);
    assert_eq!(ci.expect, NOT_STATED);
    assert_eq!(ci.command, None);
    assert!(ci.body.starts_with("Integration test: user B"));
    let OrSentinel::Value(rt) = &r.regression_test else { panic!() };
    assert_eq!(rt.command, "cargo test --test notes_access other_users_note_is_404");
    assert_eq!(rt.expect_vulnerable, "fails: got 200");
    assert_eq!(rt.expect_patched, "passes: got 404");
    assert_eq!(rt.body, "notes_access::other_users_note_is_404 creates two users and one note.");
    assert_eq!(r.cross_references, OrSentinel::Sentinel("None".into()));
    assert!(!r.references.contains("CVSS"), "trailing rating lines are fields, not prose");
    assert_eq!(r.replication_steps.len(), 3);
    assert_eq!(r.replication_steps[1], "Sign up as user B.");
    assert!(r.artifacts.is_empty());
    assert!(r.verification.is_none());
    assert_valid(&r, &[]);
}

#[test]
fn the_markdown_layout_keeps_tickets_cross_references_and_chain_code() {
    let (r, cited) = report_of(MARKDOWN);
    // The cross-reference's id is not the report's own.
    assert_eq!(cited, vec![ID2.to_string()]);
    let OrSentinel::Value(t) = &r.tickets else { panic!("{:?}", r.tickets) };
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].kind, "Example Tracker");
    assert_eq!(t[0].identifier, "NB-42");
    assert_eq!(t[0].url.as_deref(), Some("https://tracker.example.com/NB-42"));
    assert_eq!(t[0].notes.as_deref(), Some("Product team tracking remediation"));
    assert_eq!(r.cwe, vec!["CWE-613".to_string()]);
    assert_eq!(r.rating.cvss_v3, "6.5");
    assert_eq!(r.location.input, "`GET /s/{token}`");
    let OrSentinel::Value(hops) = &r.call_chain else { panic!() };
    assert_eq!(hops.len(), 3);
    assert_eq!(hops[2].file.as_deref(), Some("src/share/token.rs"));
    // Two bullet claims (the second takes the code block) and the call
    // chain's code block as a claim of its own.
    assert_eq!(r.evidence.len(), 3);
    assert_eq!(r.evidence[0].lines, Some([5, 9]));
    assert!(r.evidence[0].excerpt.is_none());
    assert!(r.evidence[1].excerpt.as_deref().unwrap().contains("check_sig"));
    assert!(r.evidence[2].claim.starts_with("Call chain"));
    assert!(r.evidence[2].excerpt.as_deref().unwrap().contains("ShareToken::verify"));
    let OrSentinel::Value(p) = &r.recommended_patch else { panic!() };
    assert_eq!(
        p.notes.as_deref(),
        Some("Existing links minted without `exp` should be treated as expired.")
    );
    let OrSentinel::Value(ci) = &r.ci_cd_detection else { panic!() };
    assert_eq!(ci.stage, "nightly");
    assert_eq!(ci.command.as_deref(), Some("cargo test --test share_expiry"));
    assert_eq!(ci.expect, "an expired token verifies");
    assert_eq!(ci.body, "A test mints a token with a past `exp` and requires `verify` to reject it.");
    let OrSentinel::Value(rt) = &r.regression_test else { panic!() };
    assert_eq!(rt.command, "cargo test --test share_expiry expired_link_is_rejected");
    assert!(!rt.body.contains("```"), "the command's code block moved to `command`");
    let OrSentinel::Value(x) = &r.cross_references else { panic!("{:?}", r.cross_references) };
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].finding_id, ID1);
    assert_eq!(x[0].relation, Relation::Prerequisite);
    // Cross-reference text is also kept verbatim, so nothing is lost when an
    // id cannot be linked.
    assert!(r.references.starts_with("OWASP A07:2021"));
    assert!(r.references.contains("Cross-references (imported):"));
    assert!(r.references.contains("NB-007 covers link revocation."));
    assert_valid(&r, &[ID1]);
}

#[test]
fn an_exported_report_round_trips() {
    let original = full_report();
    let f = numbered(vec![input(
        "notebin",
        Some("audit"),
        full_record(ID1, Severity::Critical, original.clone()),
    )])
    .remove(0);
    let md = String::from_utf8(
        render_finding(&f, &number_map(std::slice::from_ref(&f)), Format::Markdown).unwrap(),
    )
    .unwrap();
    let (r, cited) = report_of(&md);
    assert_eq!(cited, vec![ID1.to_string()]);
    assert_eq!(r.title, original.title);
    assert_eq!(r.ownership, original.ownership);
    assert_eq!(r.tickets, original.tickets);
    assert_eq!(r.rating, original.rating);
    assert_eq!(r.category, original.category);
    assert_eq!(r.attack_vector, original.attack_vector);
    assert_eq!(r.cwe, original.cwe);
    assert_eq!(r.description, original.description);
    assert_eq!(r.impact, original.impact);
    assert_eq!(r.location, original.location);
    assert_eq!(r.root_cause, original.root_cause);
    assert_eq!(r.call_chain, original.call_chain);
    assert_eq!(r.evidence, original.evidence);
    assert_eq!(r.remediation, original.remediation);
    let (OrSentinel::Value(a), OrSentinel::Value(b)) = (&r.recommended_patch, &original.recommended_patch) else {
        panic!()
    };
    assert_eq!(a.diff.trim_end(), b.diff.trim_end());
    assert_eq!(a.notes, b.notes);
    assert_eq!(r.ci_cd_detection, original.ci_cd_detection);
    assert_eq!(r.regression_test, original.regression_test);
    assert_eq!(r.replication_steps, original.replication_steps);
    assert_eq!(r.cross_references, original.cross_references);
    assert_eq!(r.references, original.references);
    assert_valid(&r, &[]);
}

#[test]
fn a_file_that_is_not_a_report_is_skipped() {
    let readme = "# Notebin\n\nA toy notes app.\n\n## Description\n\nNotes, shared.\n";
    assert_eq!(parse_report(readme).unwrap(), Parsed::NotAReport);
}

#[test]
fn a_missing_required_section_fails() {
    let md = PLAIN.replace("Root Cause\n", "Root cause notes\n");
    assert_eq!(parse_report(&md).unwrap_err(), ImportError::MissingSection("Root Cause"));
}

#[test]
fn a_rating_outside_the_scale_fails() {
    let md = PLAIN.replace("Likelihood: High", "Likelihood: Very High");
    assert!(matches!(
        parse_report(&md).unwrap_err(),
        ImportError::BadValue { field: "Likelihood", .. }
    ));
}

#[test]
fn several_findings_in_one_file_fail() {
    let md = format!("{PLAIN}\n---\n\n{MARKDOWN}");
    assert_eq!(parse_report(&md).unwrap_err(), ImportError::SeveralFindings(2));
}

#[test]
fn missing_sentinel_sections_say_so() {
    let md = PLAIN
        .split("CI/CD Detection\n")
        .next()
        .unwrap()
        .to_string()
        + "References\nCWE-639\n\nRisk Factor: High\n\nReplication Steps\nStep 1: Request another user's note.\n";
    let (r, _) = report_of(&md);
    let missing = format!("{NOT_PROVIDED_PREFIX}section missing from the imported report");
    assert_eq!(r.ci_cd_detection, OrSentinel::Sentinel(missing.clone()));
    assert_eq!(r.regression_test, OrSentinel::Sentinel(missing));
    assert_eq!(r.cross_references, OrSentinel::Sentinel("None".into()));
    assert_valid(&r, &[]);
}

#[test]
fn a_heading_inside_a_code_block_is_not_a_heading() {
    let md = PLAIN.replace(
        "Ok(Json(note))\n",
        "Ok(Json(note))\n// Remediation\nRemediation\n",
    );
    let (r, _) = report_of(&md);
    assert!(r.evidence[0].excerpt.as_deref().unwrap().contains("\nRemediation"));
    assert!(r.remediation.starts_with("Scope the lookup"));
}

#[test]
fn a_deeper_heading_inside_prose_is_not_a_section() {
    let md = MARKDOWN.replace(
        "no expiry claim, so a link keeps working after the owner stops sharing the note.",
        "no expiry claim.\n\n#### Remediation\n\nA link keeps working after sharing stops.",
    );
    let (r, _) = report_of(&md);
    assert!(r.description.contains("#### Remediation"), "{}", r.description);
    assert!(r.remediation.starts_with("Add an `exp` claim"), "{}", r.remediation);
}

#[test]
fn a_lone_carriage_return_ends_a_line() {
    let (r, _) = report_of(&PLAIN.replace('\n', "\r"));
    assert_eq!(r.replication_steps.len(), 3);
}

#[test]
fn a_patch_section_without_a_diff_keeps_its_text_in_the_sentinel() {
    let start = PLAIN.find("Recommended Patch\n").unwrap();
    let end = PLAIN.find("CI/CD Detection\n").unwrap();
    let md = format!(
        "{}Recommended Patch\nCall find_by_id_for_owner instead of find_by_id.\n\n{}",
        &PLAIN[..start],
        &PLAIN[end..]
    );
    let (r, _) = report_of(&md);
    let OrSentinel::Sentinel(s) = &r.recommended_patch else { panic!() };
    assert!(s.starts_with(NOT_PROVIDED_PREFIX), "{s}");
    assert!(s.contains("Call find_by_id_for_owner instead of find_by_id."), "{s}");
}

#[test]
fn fnd_ids_finds_whole_ulid_ids_only() {
    let text = format!("{ID1}, again {ID1}; x{ID2} fnd_short fnd_{}", "A".repeat(27));
    assert_eq!(fnd_ids(&text), vec![ID1.to_string()]);
}

#[test]
fn unknown_cross_references_are_dropped_to_none() {
    let (mut r, _) = report_of(MARKDOWN);
    retain_known_cross_references(&mut r, &HashSet::new());
    assert_eq!(r.cross_references, OrSentinel::Sentinel("None".into()));
    assert!(r.references.contains("fnd_01J00000000000000000000001 is a prerequisite"));
}
```

`common` is the existing `tests/common/mod.rs`, which provides `full_report`, `full_record`, `input` and `numbered`.

Run: `cargo test -p rupu-findings-report --test import`
Expected: FAIL to compile (`rupu_findings_report::import` does not exist).

- [ ] **Step 3: Implement the parser**

Create `crates/rupu-findings-report/src/import.rs`:

```rust
//! Best-effort parse of a finding report written as Markdown in the report
//! layout, back into a [`FindingReport`]. This is the migration aid behind
//! `rupu findings import` (spec: "Backfill"), not a supported input path.
//!
//! Two spellings of the layout are read:
//! - the one `markdown.rs` emits (`## Heading`, `**Label:** value`);
//! - the plain-text one used by reports written before the full profile
//!   existed (a heading is a line of its own; fields are `Label: value`).
//!
//! Nothing is invented:
//! - A section or field the schema has no sentinel for fails the parse
//!   when it is missing.
//! - A missing section or field that allows a sentinel gets it (`Unknown`,
//!   `None`, or `Not Provided — section missing from the imported report`).
//! - A missing *part* of a section that is present (a CI check with no
//!   stated stage) is [`NOT_STATED`].
//! - Text with no typed home is kept as text. Cross-references are also
//!   appended to `references` verbatim, and code blocks in a call chain
//!   become evidence claims.
//!
//! The result must still pass `validate_report`; the caller runs it.

use rupu_coverage::report::{
    ArtifactRef, ChainHop, CiDetection, CrossRef, EvidenceClaim, FindingReport, HopRole,
    Likelihood, OrSentinel, Ownership, Patch, Rating, RegressionTest, Relation, ReportLocation,
    RiskLevel, Ticket, NOT_PROVIDED_PREFIX,
};
use std::collections::HashSet;

/// Filler for a required part of a present section that the imported
/// report does not state.
pub const NOT_STATED: &str = "Not stated in the imported report.";
const SECTION_MISSING: &str = "section missing from the imported report";
const EXCERPT_ONLY: &str = "Code excerpt; the imported report states no claim for it.";

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// A finding report, and the finding ids it cites outside its
    /// cross-references. Its own id is expected to be the only one.
    Report {
        report: FindingReport,
        cited_ids: Vec<String>,
    },
    /// Fewer than three of the layout's section headings: not a finding
    /// report (an index, a README). Skipped, not an error.
    NotAReport,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    #[error("missing section `{0}`")]
    MissingSection(&'static str),
    #[error("missing field `{0}`")]
    MissingField(&'static str),
    #[error("`{field}` is `{value}`, not one of {allowed}")]
    BadValue {
        field: &'static str,
        value: String,
        allowed: &'static str,
    },
    #[error("the file holds {0} findings; split it into one file per finding first")]
    SeveralFindings(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Sec {
    Description,
    Impact,
    Location,
    RootCause,
    CallChain,
    Evidence,
    Remediation,
    Patch,
    CiCd,
    Regression,
    CrossRefs,
    References,
    Replication,
    Artifacts,
    Provenance,
}

impl Sec {
    fn name(self) -> &'static str {
        match self {
            Sec::Description => "Description",
            Sec::Impact => "Impact",
            Sec::Location => "Location",
            Sec::RootCause => "Root Cause",
            Sec::CallChain => "Call Chain / Attack Flow",
            Sec::Evidence => "Evidence",
            Sec::Remediation => "Remediation",
            Sec::Patch => "Recommended Patch",
            Sec::CiCd => "CI/CD Detection",
            Sec::Regression => "Regression Test",
            Sec::CrossRefs => "Cross-References",
            Sec::References => "References",
            Sec::Replication => "Replication Steps",
            Sec::Artifacts => "Artifacts",
            Sec::Provenance => "Provenance",
        }
    }
}

/// Heading text (lower-case, `#`/`**`/trailing `:` removed) → section.
const SECTIONS: &[(&str, Sec)] = &[
    ("description", Sec::Description),
    ("impact", Sec::Impact),
    ("location", Sec::Location),
    ("root cause", Sec::RootCause),
    ("call chain / attack flow", Sec::CallChain),
    ("call chain/attack flow", Sec::CallChain),
    ("call chain", Sec::CallChain),
    ("attack flow", Sec::CallChain),
    ("evidence", Sec::Evidence),
    ("remediation", Sec::Remediation),
    ("recommended patch", Sec::Patch),
    ("ci/cd detection", Sec::CiCd),
    ("ci cd detection", Sec::CiCd),
    ("regression test", Sec::Regression),
    ("cross-references", Sec::CrossRefs),
    ("cross references", Sec::CrossRefs),
    ("references", Sec::References),
    ("replication steps", Sec::Replication),
    ("artifacts", Sec::Artifacts),
    ("provenance", Sec::Provenance),
];

/// Field labels of the layout's header block (lower-case).
const FIELDS: &[&str] = &[
    "filename",
    "identifier",
    "finding id",
    "owner",
    "product",
    "affected component",
    "source repository",
    "existing ticket references",
    "impact",
    "category",
    "cwe",
    "attack vector",
    "likelihood",
    "risk rating",
    "cvss v3 base score",
    "cvss v3",
    "cvss",
    "risk factor",
    "severity",
];

/// The rating fields the layout places after References, outside any
/// heading of their own.
const TRAILING_FIELDS: &[&str] = &["cvss v3 base score", "cvss v3", "cvss", "risk factor"];

pub fn parse_report(md: &str) -> Result<Parsed, ImportError> {
    // CommonMark also ends a line at a lone `\r`.
    let md = md.replace("\r\n", "\n").replace('\r', "\n");
    let mut doc = split(&md);
    let kinds: HashSet<Sec> = doc.sections.iter().map(|(s, _)| *s).collect();
    if kinds.len() < 3 {
        return Ok(Parsed::NotAReport);
    }
    let descriptions = doc.sections.iter().filter(|(s, _)| *s == Sec::Description).count();
    let findings = doc.filename_lines.max(descriptions);
    if findings > 1 {
        return Err(ImportError::SeveralFindings(findings));
    }

    let mut header = header(&doc.header);
    take_trailing_fields(&mut doc, &mut header.fields);

    let cited_ids = {
        let mut text = doc.header.join("\n");
        for (s, body) in &doc.sections {
            if *s != Sec::CrossRefs {
                text.push('\n');
                text.push_str(&body.join("\n"));
            }
        }
        fnd_ids(&text)
    };

    let title = header.title.clone().ok_or(ImportError::MissingField("title"))?;
    let or_unknown = |keys: &[&str]| header.get(keys).unwrap_or_else(|| "Unknown".to_string());
    let ownership = Ownership {
        owner: or_unknown(&["owner"]),
        product: or_unknown(&["product"]),
        affected_component: or_unknown(&["affected component"]),
        source_repository: or_unknown(&["source repository"]),
    };
    let rating = Rating {
        impact: risk("Impact", header.get(&["impact"]))?,
        likelihood: likelihood(header.get(&["likelihood"]))?,
        risk_rating: risk("Risk Rating", header.get(&["risk rating"]))?,
        risk_factor: risk("Risk Factor", header.get(&["risk factor"]))?,
        cvss_v3: or_unknown(&["cvss v3 base score", "cvss v3", "cvss"]),
    };
    let category = header
        .get(&["category"])
        .ok_or(ImportError::MissingField("Category"))?;
    let attack_vector = header
        .get(&["attack vector"])
        .ok_or(ImportError::MissingField("Attack Vector"))?;

    let required = |s: Sec| section(&doc, s).ok_or(ImportError::MissingSection(s.name()));
    let description = required(Sec::Description)?.text();
    let impact = required(Sec::Impact)?.text();
    let location = location(&required(Sec::Location)?);
    let root_cause = required(Sec::RootCause)?.text();
    let (call_chain, chain_claims) = call_chain(section(&doc, Sec::CallChain).as_ref());
    let mut evidence = evidence(&required(Sec::Evidence)?);
    if evidence.is_empty() {
        return Err(ImportError::MissingSection(Sec::Evidence.name()));
    }
    evidence.extend(chain_claims);
    let remediation = required(Sec::Remediation)?.text();
    let recommended_patch = patch(section(&doc, Sec::Patch).as_ref());
    let ci_cd_detection = ci(section(&doc, Sec::CiCd).as_ref());
    let regression_test = regression(section(&doc, Sec::Regression).as_ref());
    let replication_steps = steps(&required(Sec::Replication)?);
    if replication_steps.is_empty() {
        return Err(ImportError::MissingSection(Sec::Replication.name()));
    }
    let mut references = required(Sec::References)?.text();
    let cwe = match header.get(&["cwe"]) {
        Some(v) => cwe_ids(&v),
        None => cwe_ids(&format!("{category}\n{references}")),
    };
    let (cross_references, verbatim) =
        cross_refs(section(&doc, Sec::CrossRefs).as_ref(), &cited_ids);
    if let Some(text) = verbatim {
        references = format!("{references}\n\nCross-references (imported):\n\n{text}");
    }
    let artifacts = section(&doc, Sec::Artifacts)
        .map(|b| artifacts(&b))
        .unwrap_or_default();

    Ok(Parsed::Report {
        report: FindingReport {
            title,
            ownership,
            tickets: tickets(header.get(&["existing ticket references"])),
            rating,
            category,
            attack_vector,
            cwe,
            description,
            impact,
            location,
            root_cause,
            call_chain,
            evidence,
            remediation,
            recommended_patch,
            ci_cd_detection,
            regression_test,
            replication_steps,
            cross_references,
            references,
            artifacts,
            verification: None,
        },
        cited_ids,
    })
}

/// Distinct `fnd_<ULID>` ids in `text`, in first-seen order. An id must be
/// exactly 26 upper-case alphanumerics after `fnd_`, not glued to a longer
/// word on either side.
pub fn fnd_ids(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(off) = text[from..].find("fnd_") {
        let start = from + off;
        let tail = &text[start + 4..];
        let n = tail.chars().take_while(|c| c.is_ascii_alphanumeric()).count();
        let glued = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        if n == 26
            && !glued
            && tail[..26].chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        {
            let id = format!("fnd_{}", &tail[..26]);
            if !out.contains(&id) {
                out.push(id);
            }
        }
        from = start + 4;
    }
    out
}

/// Drop cross-references to findings not in `known`: the ledger accepts
/// references to its own findings only. Their text is already in
/// `references`. An emptied list becomes `None`.
pub fn retain_known_cross_references(report: &mut FindingReport, known: &HashSet<String>) {
    if let OrSentinel::Value(refs) = &mut report.cross_references {
        refs.retain(|r| known.contains(&r.finding_id));
        if refs.is_empty() {
            report.cross_references = OrSentinel::Sentinel("None".into());
        }
    }
}

// ---- document structure ----------------------------------------------------

#[derive(Debug, Default)]
struct Doc {
    header: Vec<String>,
    sections: Vec<(Sec, Vec<String>)>,
    filename_lines: usize,
}

/// Number of leading `#`s (0 for a bare or bold heading line).
fn heading_level(line: &str) -> usize {
    line.trim_start().chars().take_while(|&c| c == '#').count()
}

/// Split into the header block and the sections, by heading lines outside
/// code blocks. A section may appear more than once; `section` merges them.
///
/// The layout's section headings share one level, and the most common level
/// among lines that name a section is taken as that level (a tie goes to
/// the shallower one). So a deeper heading inside prose (the exporter
/// shifts those to h3 and below) and a stray bare line that happens to say
/// "Evidence" are not sections.
fn split(md: &str) -> Doc {
    let lines: Vec<&str> = md.lines().collect();
    let mut in_code = vec![false; lines.len()];
    let mut candidates: Vec<(usize, Sec, usize)> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(open) = fence {
            in_code[i] = true;
            if closes(line, open) {
                fence = None;
            }
        } else if let Some(open) = opens(line) {
            in_code[i] = true;
            fence = Some(open);
        } else if let Some(sec) = heading(line) {
            candidates.push((i, sec, heading_level(line)));
        }
    }
    let mut counts: std::collections::BTreeMap<usize, usize> = Default::default();
    for (_, _, level) in &candidates {
        *counts.entry(*level).or_default() += 1;
    }
    let level = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(l, _)| *l);
    let headings: std::collections::HashMap<usize, Sec> = candidates
        .into_iter()
        .filter(|(_, _, l)| Some(*l) == level)
        .map(|(i, s, _)| (i, s))
        .collect();

    let mut doc = Doc::default();
    for (i, line) in lines.iter().enumerate() {
        if let Some(sec) = headings.get(&i) {
            doc.sections.push((*sec, Vec::new()));
            continue;
        }
        if !in_code[i] && matches!(field(line), Some((k, _)) if k == "filename") {
            doc.filename_lines += 1;
        }
        match doc.sections.last_mut() {
            Some((_, body)) => body.push(line.to_string()),
            None => doc.header.push(line.to_string()),
        }
    }
    doc
}

/// Move CVSS / Risk Factor lines out of section bodies into the header
/// fields, fence-aware.
fn take_trailing_fields(doc: &mut Doc, into: &mut Vec<(String, String)>) {
    for (_, body) in &mut doc.sections {
        let mut fence: Option<(char, usize)> = None;
        body.retain(|line| {
            if let Some(open) = fence {
                if closes(line, open) {
                    fence = None;
                }
                return true;
            }
            if let Some(open) = opens(line) {
                fence = Some(open);
                return true;
            }
            match field(line) {
                Some((k, v)) if TRAILING_FIELDS.contains(&k.as_str()) => {
                    into.push((k, v));
                    false
                }
                _ => true,
            }
        });
    }
}

fn section(doc: &Doc, s: Sec) -> Option<Body> {
    let lines: Vec<String> = doc
        .sections
        .iter()
        .filter(|(k, _)| *k == s)
        .flat_map(|(_, b)| b.iter().cloned().chain(std::iter::once(String::new())))
        .collect();
    let body = Body::parse(&lines);
    (!body.text().is_empty()).then_some(body)
}

fn opens(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    let c = t.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = t.chars().take_while(|&x| x == c).count();
    (n >= 3).then_some((c, n))
}

fn closes(line: &str, (c, n): (char, usize)) -> bool {
    let t = line.trim();
    t.len() >= n && t.chars().all(|x| x == c)
}

fn heading(line: &str) -> Option<Sec> {
    let mut t = line.trim();
    t = t.trim_start_matches('#').trim();
    // "## 3. Root Cause"
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        if let Some(r) = t[digits..].strip_prefix(['.', ')']) {
            t = r.trim();
        }
    }
    let t = strip_pair(strip_pair(t, "**"), "__");
    let key = t.trim().trim_end_matches(':').trim().to_ascii_lowercase();
    SECTIONS.iter().find(|(n, _)| *n == key).map(|(_, s)| *s)
}

fn strip_pair<'a>(s: &'a str, p: &str) -> &'a str {
    s.strip_prefix(p)
        .and_then(|r| r.strip_suffix(p))
        .unwrap_or(s)
}

/// `**Label:** value`, `**Label**: value` or `Label: value`, as
/// (lower-case label, value without surrounding emphasis).
fn labelled(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    let (label, rest) = if let Some(r) = t.strip_prefix("**") {
        let end = r.find("**")?;
        let (inner, after) = (&r[..end], &r[end + 2..]);
        match inner.strip_suffix(':') {
            Some(l) => (l, after),
            None => (inner, after.strip_prefix(':')?),
        }
    } else {
        let colon = t.find(':')?;
        (&t[..colon], &t[colon + 1..])
    };
    let label = label.trim();
    if label.is_empty() || label.len() > 40 || label.contains('`') {
        return None;
    }
    Some((
        label.to_ascii_lowercase(),
        unwrap_emphasis(rest.trim()).to_string(),
    ))
}

fn field(line: &str) -> Option<(String, String)> {
    labelled(line).filter(|(k, _)| FIELDS.contains(&k.as_str()))
}

/// `_text_` / `*text*` → `text` (the exporter's notes).
fn unwrap_emphasis(s: &str) -> &str {
    if s.len() > 2 && !s.starts_with("**") && !s.starts_with("__") {
        for p in ["_", "*"] {
            if let Some(inner) = s.strip_prefix(p).and_then(|r| r.strip_suffix(p)) {
                return inner.trim();
            }
        }
    }
    s
}

fn is_thematic_break(t: &str) -> bool {
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

/// A leading `Step N:`, `- `, `* `, `+ `, `N. ` or `N) ` removed.
fn strip_marker(line: &str) -> &str {
    let t = line.trim();
    if t.len() > 5 && t[..5].eq_ignore_ascii_case("step ") {
        let r = &t[5..];
        let d = r.chars().take_while(|c| c.is_ascii_digit()).count();
        if d > 0 {
            if let Some(rest) = r[d..].strip_prefix([':', '.']) {
                return rest.trim();
            }
        }
    }
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(m) {
            return r.trim();
        }
    }
    let d = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if d > 0 {
        if let Some(r) = t[d..].strip_prefix(". ").or_else(|| t[d..].strip_prefix(") ")) {
            return r.trim();
        }
    }
    t
}

fn is_list_item(line: &str) -> bool {
    strip_marker(line).len() != line.trim().len()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn missing<T>() -> OrSentinel<T> {
    OrSentinel::Sentinel(format!("{NOT_PROVIDED_PREFIX}{SECTION_MISSING}"))
}

/// A whole section that says "Not provided …", normalized to the sentinel.
fn not_provided(text: &str) -> Option<String> {
    let t = unwrap_emphasis(text.trim());
    if t.len() < 12 || !t[..12].eq_ignore_ascii_case("not provided") {
        return None;
    }
    let why = t[12..].trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '—' | '–' | '-'));
    let why = one_line(why);
    Some(format!(
        "{NOT_PROVIDED_PREFIX}{}",
        if why.is_empty() { "not given in the imported report" } else { why.as_str() }
    ))
}

// ---- header ----------------------------------------------------------------

#[derive(Debug, Default)]
struct Header {
    title: Option<String>,
    fields: Vec<(String, String)>,
}

impl Header {
    /// The first non-blank value for any of `keys`.
    fn get(&self, keys: &[&str]) -> Option<String> {
        self.fields
            .iter()
            .find(|(k, v)| keys.contains(&k.as_str()) && !v.trim().is_empty())
            .map(|(_, v)| v.trim().to_string())
    }
}

fn header(lines: &[String]) -> Header {
    let mut h = Header::default();
    let mut in_tickets = false;
    let mut fence: Option<(char, usize)> = None;
    for line in lines {
        if let Some(open) = fence {
            if closes(line, open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = opens(line) {
            fence = Some(open);
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        // The structured ticket list continues the field on indented or
        // bulleted lines.
        if in_tickets && (line.starts_with(char::is_whitespace) || is_list_item(line)) {
            if let Some((_, v)) = h.fields.last_mut() {
                if !v.is_empty() {
                    v.push('\n');
                }
                v.push_str(line.trim());
            }
            continue;
        }
        in_tickets = false;
        if let Some((k, v)) = field(line) {
            in_tickets = k == "existing ticket references";
            h.fields.push((k, v));
        } else if h.title.is_none() {
            let t = line.trim().trim_start_matches('#').trim();
            let t = strip_pair(strip_pair(t, "**"), "__").trim();
            if !t.is_empty() && !is_thematic_break(t) {
                h.title = Some(t.to_string());
            }
        }
    }
    h
}

fn first_word(v: &str) -> String {
    v.trim()
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn risk(field: &'static str, v: Option<String>) -> Result<RiskLevel, ImportError> {
    let v = v.ok_or(ImportError::MissingField(field))?;
    match first_word(&v).as_str() {
        "low" => Ok(RiskLevel::Low),
        "medium" => Ok(RiskLevel::Medium),
        "high" => Ok(RiskLevel::High),
        "critical" => Ok(RiskLevel::Critical),
        _ => Err(ImportError::BadValue {
            field,
            value: v,
            allowed: "Low, Medium, High, Critical",
        }),
    }
}

fn likelihood(v: Option<String>) -> Result<Likelihood, ImportError> {
    let v = v.ok_or(ImportError::MissingField("Likelihood"))?;
    match first_word(&v).as_str() {
        "low" => Ok(Likelihood::Low),
        "medium" => Ok(Likelihood::Medium),
        "high" => Ok(Likelihood::High),
        _ => Err(ImportError::BadValue {
            field: "Likelihood",
            value: v,
            allowed: "Low, Medium, High",
        }),
    }
}

fn cwe_ids(text: &str) -> Vec<String> {
    let upper = text.to_ascii_uppercase();
    let mut out: Vec<String> = Vec::new();
    let mut rest = upper.as_str();
    while let Some(i) = rest.find("CWE-") {
        let tail = &rest[i + 4..];
        let n = tail.chars().take_while(|c| c.is_ascii_digit()).count();
        if n > 0 {
            let id = format!("CWE-{}", &tail[..n]);
            if !out.contains(&id) {
                out.push(id);
            }
        }
        rest = tail;
    }
    out
}

fn tickets(v: Option<String>) -> OrSentinel<Vec<Ticket>> {
    let unknown = || OrSentinel::Sentinel("Unknown".to_string());
    let Some(v) = v else { return unknown() };
    let bare = unwrap_emphasis(v.trim()).trim_end_matches('.');
    if bare.eq_ignore_ascii_case("none provided") || bare.eq_ignore_ascii_case("none") {
        return OrSentinel::Sentinel("None Provided".into());
    }
    if bare.eq_ignore_ascii_case("unknown") {
        return unknown();
    }
    let stated = |s: &str| {
        !(s.is_empty()
            || s.eq_ignore_ascii_case("unknown")
            || s.eq_ignore_ascii_case("not provided")
            || s.eq_ignore_ascii_case("none"))
    };
    let structured = v
        .lines()
        .any(|l| matches!(labelled(strip_marker(l)), Some((k, _)) if k == "type"));
    let mut out: Vec<Ticket> = Vec::new();
    if structured {
        for l in v.lines() {
            let Some((k, val)) = labelled(strip_marker(l)) else { continue };
            match (k.as_str(), out.last_mut()) {
                ("type", _) => out.push(Ticket {
                    kind: if stated(&val) { val } else { "Other".into() },
                    identifier: "Unknown".into(),
                    url: None,
                    notes: None,
                }),
                ("identifier", Some(t)) if stated(&val) => t.identifier = val,
                ("url", Some(t)) => t.url = stated(&val).then_some(val),
                ("notes", Some(t)) => t.notes = stated(&val).then_some(val),
                _ => {}
            }
        }
    } else {
        for item in v.split(['\n', ';']).map(strip_marker).filter(|s| !s.is_empty()) {
            let url = item
                .split_whitespace()
                .map(|w| w.trim_matches(|c| matches!(c, '(' | ')' | '<' | '>' | ',')))
                .find(|w| w.starts_with("http://") || w.starts_with("https://"))
                .map(str::to_string);
            out.push(Ticket {
                kind: "Other".into(),
                identifier: item.to_string(),
                url,
                notes: None,
            });
        }
    }
    if out.is_empty() {
        unknown()
    } else {
        OrSentinel::Value(out)
    }
}

// ---- section bodies --------------------------------------------------------

#[derive(Debug, Clone)]
enum Block {
    Text(Vec<String>),
    Fence {
        info: String,
        content: String,
        raw: String,
    },
}

#[derive(Debug, Clone, Default)]
struct Body(Vec<Block>);

impl Body {
    fn parse(lines: &[String]) -> Body {
        let mut blocks = Vec::new();
        let mut text: Vec<String> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let line = &lines[i];
            if let Some(open) = opens(line) {
                if !text.is_empty() {
                    blocks.push(Block::Text(std::mem::take(&mut text)));
                }
                let info = line.trim_start().trim_start_matches(open.0).trim().to_string();
                let mut raw = vec![line.clone()];
                let mut content = Vec::new();
                i += 1;
                while i < lines.len() {
                    raw.push(lines[i].clone());
                    if closes(&lines[i], open) {
                        break;
                    }
                    content.push(lines[i].clone());
                    i += 1;
                }
                i += 1;
                blocks.push(Block::Fence {
                    info,
                    content: content.join("\n"),
                    raw: raw.join("\n"),
                });
            } else {
                let t = line.trim_end();
                if !is_thematic_break(t.trim()) {
                    text.push(t.to_string());
                }
                i += 1;
            }
        }
        if !text.is_empty() {
            blocks.push(Block::Text(text));
        }
        Body(blocks)
    }

    fn text(&self) -> String {
        render(&self.0)
    }
}

fn render(blocks: &[Block]) -> String {
    blocks
        .iter()
        .map(|b| match b {
            Block::Text(lines) => lines.join("\n"),
            Block::Fence { raw, .. } => raw.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn without(blocks: &[Block], skip: usize) -> Vec<Block> {
    blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != skip)
        .map(|(_, b)| b.clone())
        .collect()
}

fn non_blank_or_not_stated(s: String) -> String {
    if s.trim().is_empty() {
        NOT_STATED.to_string()
    } else {
        s
    }
}

fn claim(text: String) -> EvidenceClaim {
    EvidenceClaim {
        claim: text,
        file: None,
        lines: None,
        binary_va: None,
        excerpt: None,
        lang: None,
        sha256: None,
        artifact: None,
    }
}

fn fence_lang(info: &str) -> Option<String> {
    let w = info.split_whitespace().next()?;
    w.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '#' | '.'))
        .then(|| w.to_string())
}

/// The first `path:12` / `path:12-30` token whose path is workspace-relative,
/// with backticks and punctuation around it ignored.
fn location_token(text: &str) -> Option<(String, [u32; 2])> {
    text.split_whitespace().find_map(|raw| {
        let tok = raw
            .trim_start_matches(|c: char| matches!(c, '`' | '*' | '(' | '[' | '"' | '\''))
            .trim_end_matches(|c: char| {
                matches!(c, '`' | '*' | ')' | ']' | ',' | ';' | '"' | '\'' | '.' | ':')
            });
        let (path, range) = tok.rsplit_once(':')?;
        let (a, b) = range.split_once(['-', '–']).unwrap_or((range, range));
        let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
        let relative = !path.is_empty()
            && !path.starts_with('/')
            && !path.contains("..")
            && !path.contains(':')
            && !path.contains('\\')
            && (path.contains('.') || path.contains('/'));
        (relative && a >= 1 && b >= a).then(|| (path.to_string(), [a, b]))
    })
}

fn binary_va(text: &str) -> Option<String> {
    let i = text.find('@')?;
    let r = text[i + 1..].trim_start();
    let r = r.strip_prefix("0x").or_else(|| r.strip_prefix("0X"))?;
    let n = r.chars().take_while(|c| c.is_ascii_hexdigit()).count();
    (n > 0).then(|| format!("0x{}", &r[..n]))
}

fn location(body: &Body) -> ReportLocation {
    let mut input: Vec<String> = Vec::new();
    let mut output: Vec<String> = Vec::new();
    let mut labelled_any = false;
    let mut current = 0u8; // 1 input, 2 output
    for line in body.text().lines() {
        match labelled(line) {
            Some((k, v)) if k == "input" => {
                labelled_any = true;
                current = 1;
                input.push(v);
            }
            Some((k, v)) if k == "output" => {
                labelled_any = true;
                current = 2;
                output.push(v);
            }
            _ if current == 2 => output.push(line.to_string()),
            _ => input.push(line.to_string()),
        }
    }
    let join = |v: Vec<String>| non_blank_or_not_stated(v.join("\n").trim().to_string());
    if !labelled_any {
        return ReportLocation {
            input: body.text(),
            output: NOT_STATED.into(),
        };
    }
    ReportLocation {
        input: join(input),
        output: join(output),
    }
}

fn role(i: usize, n: usize) -> HopRole {
    if n == 1 || i + 1 == n {
        HopRole::Sink
    } else if i == 0 {
        HopRole::Source
    } else {
        HopRole::Hop
    }
}

fn hop(line: &str, role: HopRole) -> ChainHop {
    let mut h = ChainHop {
        label: line.to_string(),
        file: None,
        lines: None,
        binary_va: None,
        gate: None,
        passes_because: None,
        role,
    };
    // The exporter's spelling:
    // **label** — `file:a-b` [@ va] — gate: g (passes because p) — role: r
    if let Some(r) = line.strip_prefix("**") {
        if let Some(end) = r.find("**") {
            h.label = r[..end].trim().to_string();
            for seg in r[end + 2..].split(" — ").map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(g) = seg.strip_prefix("gate: ") {
                    match g.rsplit_once(" (passes because ") {
                        Some((gate, why)) => {
                            h.gate = Some(gate.trim().to_string());
                            h.passes_because = Some(why.trim_end_matches(')').trim().to_string());
                        }
                        None => h.gate = Some(g.trim().to_string()),
                    }
                } else if let Some(r) = seg.strip_prefix("role: ") {
                    h.role = match r.trim() {
                        "source" => HopRole::Source,
                        "hop" => HopRole::Hop,
                        "sink" => HopRole::Sink,
                        _ => h.role,
                    };
                } else {
                    if let Some((f, l)) = location_token(seg) {
                        h.file = Some(f);
                        h.lines = Some(l);
                    }
                    h.binary_va = h.binary_va.or_else(|| binary_va(seg));
                }
            }
            return h;
        }
    }
    if let Some((f, l)) = location_token(line) {
        h.file = Some(f);
        h.lines = Some(l);
    }
    h.binary_va = binary_va(line);
    h
}

fn call_chain(body: Option<&Body>) -> (OrSentinel<Vec<ChainHop>>, Vec<EvidenceClaim>) {
    let Some(body) = body else { return (missing(), Vec::new()) };
    if let Some(s) = not_provided(&body.text()) {
        return (OrSentinel::Sentinel(s), Vec::new());
    }
    let mut lines: Vec<String> = Vec::new();
    let mut claims = Vec::new();
    for b in &body.0 {
        match b {
            Block::Text(ls) => lines.extend(
                ls.iter()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty()),
            ),
            Block::Fence { info, content, .. } => {
                let mut c = claim(match lines.last() {
                    Some(h) => format!("Call chain, at: {}", strip_marker(h)),
                    None => "Call chain excerpt".to_string(),
                });
                c.excerpt = Some(content.clone());
                c.lang = fence_lang(info);
                claims.push(c);
            }
        }
    }
    if lines.len() == 1 && (lines[0].contains('→') || lines[0].contains("->")) {
        let one = lines.remove(0);
        lines = one
            .split('→')
            .flat_map(|p| p.split("->"))
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
    }
    if lines.is_empty() {
        return (
            OrSentinel::Sentinel(format!(
                "{NOT_PROVIDED_PREFIX}the imported call chain is code only; see the evidence claims"
            )),
            claims,
        );
    }
    let n = lines.len();
    let hops = lines
        .iter()
        .enumerate()
        .map(|(i, l)| hop(strip_marker(l), role(i, n)))
        .collect();
    (OrSentinel::Value(hops), claims)
}

fn paragraphs(lines: &[String]) -> Vec<String> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for l in lines {
        let t = l.trim();
        if t.is_empty() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if is_list_item(t) && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(t);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|p| p.join("\n")).collect()
}

fn claim_from(para: &str) -> EvidenceClaim {
    let text = strip_marker(para);
    // The exporter's spelling: **`file:a-b`** — claim
    let (loc, text) = match text.strip_prefix("**`").and_then(|r| r.split_once("`** — ")) {
        Some((loc, rest)) => (Some(loc), rest),
        None => (None, text),
    };
    let mut c = claim(text.trim().to_string());
    let at = loc.unwrap_or(text);
    if let Some((f, l)) = location_token(at) {
        c.file = Some(f);
        c.lines = Some(l);
    }
    c.binary_va = binary_va(at);
    c
}

fn evidence(body: &Body) -> Vec<EvidenceClaim> {
    let mut claims: Vec<EvidenceClaim> = Vec::new();
    for b in &body.0 {
        match b {
            Block::Text(lines) => claims.extend(paragraphs(lines).iter().map(|p| claim_from(p))),
            Block::Fence { info, content, .. } => match claims.last_mut() {
                Some(c) if c.excerpt.is_none() => {
                    c.excerpt = Some(content.clone());
                    c.lang = fence_lang(info);
                }
                _ => {
                    let mut c = claim(EXCERPT_ONLY.to_string());
                    c.excerpt = Some(content.clone());
                    c.lang = fence_lang(info);
                    claims.push(c);
                }
            },
        }
    }
    claims
}

fn is_diff(info: &str, content: &str) -> bool {
    let lang = info.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    lang == "diff"
        || lang == "patch"
        || content.starts_with("--- ")
        || content.starts_with("diff ")
        || content.starts_with("@@")
}

fn patch(body: Option<&Body>) -> OrSentinel<Patch> {
    let Some(body) = body else { return missing() };
    let text = body.text();
    if let Some(s) = not_provided(&text) {
        return OrSentinel::Sentinel(s);
    }
    let found = body.0.iter().enumerate().find_map(|(i, b)| match b {
        Block::Fence { info, content, .. } if is_diff(info, content) => Some((i, content.clone())),
        _ => None,
    });
    match found {
        Some((i, diff)) => {
            let rest = render(&without(&body.0, i));
            OrSentinel::Value(Patch {
                diff,
                notes: (!rest.is_empty()).then_some(rest),
            })
        }
        None => OrSentinel::Sentinel(format!(
            "{NOT_PROVIDED_PREFIX}the imported report gives no unified diff: {}",
            one_line(&text)
        )),
    }
}

/// Label lines (`Stage:`, `Command:` …) taken out of a body's text blocks.
struct Labels {
    found: Vec<(String, String)>,
    rest: Vec<Block>,
}

impl Labels {
    fn get(&self, keys: &[&str]) -> Option<String> {
        self.found
            .iter()
            .find(|(k, v)| keys.contains(&k.as_str()) && !v.is_empty())
            .map(|(_, v)| v.clone())
    }
}

fn take_labels(body: &Body, keys: &[&str]) -> Labels {
    let mut found = Vec::new();
    let rest = body
        .0
        .iter()
        .map(|b| match b {
            Block::Text(lines) => Block::Text(
                lines
                    .iter()
                    .filter(|l| match labelled(l) {
                        Some((k, v)) if keys.contains(&k.as_str()) => {
                            found.push((k, v));
                            false
                        }
                        _ => true,
                    })
                    .cloned()
                    .collect(),
            ),
            fence => fence.clone(),
        })
        .collect();
    Labels { found, rest }
}

/// The first one-line shell code block, taken out as a command.
fn take_command_fence(blocks: &[Block]) -> (Vec<Block>, Option<String>) {
    let found = blocks.iter().enumerate().find_map(|(i, b)| match b {
        Block::Fence { info, content, .. }
            if matches!(
                info.split_whitespace().next().unwrap_or(""),
                "" | "sh" | "bash" | "shell" | "console" | "zsh"
            ) && content.trim().lines().count() == 1 =>
        {
            Some((i, content.trim().to_string()))
        }
        _ => None,
    });
    match found {
        Some((i, cmd)) => (without(blocks, i), Some(cmd)),
        None => (blocks.to_vec(), None),
    }
}

const EXPECT: &[&str] = &["fails when", "expect", "expected", "expected result"];
const VULNERABLE: &[&str] = &["vulnerable build", "on the vulnerable build", "expect vulnerable"];
const PATCHED: &[&str] = &["patched build", "on the patched build", "expect patched"];

fn ci(body: Option<&Body>) -> OrSentinel<CiDetection> {
    let Some(body) = body else { return missing() };
    if let Some(s) = not_provided(&body.text()) {
        return OrSentinel::Sentinel(s);
    }
    let keys: Vec<&str> = ["stage", "command"].iter().chain(EXPECT).copied().collect();
    let labels = take_labels(body, &keys);
    let (rest, command) = match labels.get(&["command"]) {
        Some(c) => (labels.rest.clone(), Some(c)),
        None => take_command_fence(&labels.rest),
    };
    OrSentinel::Value(CiDetection {
        stage: labels.get(&["stage"]).unwrap_or_else(|| NOT_STATED.into()),
        body: non_blank_or_not_stated(render(&rest)),
        command,
        expect: labels.get(EXPECT).unwrap_or_else(|| NOT_STATED.into()),
    })
}

fn regression(body: Option<&Body>) -> OrSentinel<RegressionTest> {
    let Some(body) = body else { return missing() };
    if let Some(s) = not_provided(&body.text()) {
        return OrSentinel::Sentinel(s);
    }
    let keys: Vec<&str> = ["command"].iter().chain(VULNERABLE).chain(PATCHED).copied().collect();
    let labels = take_labels(body, &keys);
    let (rest, command) = match labels.get(&["command"]) {
        Some(c) => (labels.rest.clone(), Some(c)),
        None => take_command_fence(&labels.rest),
    };
    OrSentinel::Value(RegressionTest {
        body: non_blank_or_not_stated(render(&rest)),
        command: command.unwrap_or_else(|| NOT_STATED.into()),
        expect_vulnerable: labels.get(VULNERABLE).unwrap_or_else(|| NOT_STATED.into()),
        expect_patched: labels.get(PATCHED).unwrap_or_else(|| NOT_STATED.into()),
    })
}

fn steps(body: &Body) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in &body.0 {
        match b {
            Block::Text(lines) => {
                for l in lines {
                    let t = l.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let s = strip_marker(t);
                    match out.last_mut() {
                        Some(last) if s.len() == t.len() => {
                            last.push('\n');
                            last.push_str(t);
                        }
                        _ => out.push(s.to_string()),
                    }
                }
            }
            Block::Fence { raw, .. } => match out.last_mut() {
                Some(last) => {
                    last.push_str("\n\n");
                    last.push_str(raw);
                }
                None => out.push(raw.clone()),
            },
        }
    }
    out
}

fn relation(line: &str) -> Relation {
    let l = line.to_ascii_lowercase();
    if l.contains("duplicate") {
        Relation::Duplicate
    } else if l.contains("prerequisite") {
        Relation::Prerequisite
    } else if l.contains("supersede") {
        Relation::Supersedes
    } else {
        Relation::Sibling
    }
}

/// The cross-references (links for the `fnd_` ids it names, other than the
/// report's own), and the section's verbatim text for `references`.
fn cross_refs(body: Option<&Body>, own: &[String]) -> (OrSentinel<Vec<CrossRef>>, Option<String>) {
    let none = || OrSentinel::Sentinel("None".to_string());
    let Some(body) = body else { return (none(), None) };
    let text = body.text();
    let bare = unwrap_emphasis(text.trim()).trim_end_matches('.');
    if ["none", "none provided", "n/a", "not applicable"]
        .iter()
        .any(|n| bare.eq_ignore_ascii_case(n))
    {
        return (none(), None);
    }
    let mut refs: Vec<CrossRef> = Vec::new();
    for line in text.lines() {
        for id in fnd_ids(line) {
            if own.contains(&id) || refs.iter().any(|r| r.finding_id == id) {
                continue;
            }
            refs.push(CrossRef {
                finding_id: id,
                relation: relation(line),
                note: Some(one_line(strip_marker(line))),
            });
        }
    }
    let refs = if refs.is_empty() { none() } else { OrSentinel::Value(refs) };
    (refs, Some(text))
}

fn artifacts(body: &Body) -> Vec<ArtifactRef> {
    let mut out = Vec::new();
    let mut seen_header = false;
    for b in &body.0 {
        let Block::Text(lines) = b else { continue };
        for l in lines {
            let t = l.trim();
            let path = if let Some(row) = t.strip_prefix('|') {
                let first = row.split('|').next().unwrap_or("").trim();
                if first.chars().all(|c| matches!(c, '-' | ':')) {
                    continue;
                }
                if !seen_header {
                    seen_header = true;
                    continue;
                }
                first
            } else if is_list_item(t) {
                strip_marker(t)
            } else {
                continue;
            };
            let path = path.trim_matches('`').trim();
            if !path.is_empty() {
                out.push(ArtifactRef {
                    path: path.to_string(),
                    sha256: String::new(),
                    size: 0,
                    kind: None,
                    stored: None,
                    host: None,
                });
            }
        }
    }
    out
}
```

Add `pub mod import;` to `crates/rupu-findings-report/src/lib.rs` after `pub mod html;`, by hand. Add one line to the crate doc comment: `//! Also: a best-effort Markdown → FindingReport parser for \`rupu findings import\` ([`import`]).` If `thiserror` is not a dependency of the crate, it already is (see its `Cargo.toml`).

Run: `cargo test -p rupu-findings-report --test import`
Expected: PASS.

The exporter (Plan 3's final fixes) ends each hop with ` — role: <source|hop|sink>`, separates steps with blank lines, escapes a line-leading `<` in agent prose as `\<`, and shifts prose headings to h3 and below; the parser above reads all of these. If the round-trip test fails on a specific field, check how `blocks.rs` / `markdown.rs` render that field and fix the **parser** to read it back. Change a test expectation only when the exporter output genuinely cannot carry the value, for example a value the exporter drops. In that case, write in the report which field it is and why, and narrow only that assertion.

- [ ] **Step 4: Feature-off build, lint and commit**

Run each of these; all must be clean:
- `cargo test -p rupu-findings-report` (all targets)
- `cargo test -p rupu-findings-report --no-default-features`
- `rustfmt --edition 2021 crates/rupu-findings-report/src/import.rs crates/rupu-findings-report/tests/import.rs`
- `cargo clippy -p rupu-findings-report --all-targets -- -D warnings -A clippy::question_mark`

```bash
git add crates/rupu-findings-report/src/import.rs crates/rupu-findings-report/src/lib.rs crates/rupu-findings-report/tests/import.rs crates/rupu-findings-report/tests/fixtures/import
git commit -m "feat(findings-report): best-effort Markdown report parser for import"
```

---

### Task 3: `rupu findings import` (rupu-cp lookup + CLI + docs)

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs`
- Modify: `crates/rupu-cli/src/cmd/findings.rs`
- Create: `crates/rupu-cli/tests/findings_import.rs`
- Modify: `docs/coverage.md`, `CLAUDE.md`, `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md`

**Interfaces:**
- Consumes:
  - Task 1: `rupu_coverage::tools::{attach_reports, AttachItem, AttachOutcome, AttachBatch}`, `rupu_coverage::tools::report_finding::ReportFindingError`
  - Task 2: `rupu_findings_report::import::{parse_report, Parsed, retain_known_cross_references}`
- Produces:
  - `rupu_cp::api::findings::finding_ledgers(global_dir: &Path) -> HashMap<String, Vec<CoveragePaths>>`
  - CLI `rupu findings import <PATH>... [--id <FINDING_ID>] [--dry-run]`

- [ ] **Step 1: `finding_ledgers` in rupu-cp**

`collect_all_findings` already walks registered workspaces, then `discover_targets`, then `read_findings`. Extract that walk into one private function that both use, so there is still a single walker. For example:

```rust
/// Every registered workspace's coverage targets, with their ledgers read.
/// Workspaces or targets that cannot be read are skipped with a warning.
fn each_ledger(global_dir: &std::path::Path, mut f: impl FnMut(&Workspace, CoveragePaths, Vec<FindingRecord>)) {
    // body = the existing loop in `collect_all_findings`, calling `f` per target
}
```

Use the real workspace type name from `store_for(...).list()`. Then add:

```rust
/// Which ledger holds each finding: id → the coverage paths of every ledger
/// it appears in (normally exactly one). Read-only; `rupu findings import`
/// uses it to find the finding a report belongs to.
pub fn finding_ledgers(global_dir: &std::path::Path) -> HashMap<String, Vec<CoveragePaths>> {
    let mut out: HashMap<String, Vec<CoveragePaths>> = HashMap::new();
    each_ledger(global_dir, |_, paths, records| {
        for r in records {
            out.entry(r.id).or_default().push(paths.clone());
        }
    });
    out
}
```

Add a unit test next to the existing ones in that file:
- Seed a temp global dir with one workspace and one target holding two findings, the way the existing `collect_all_findings` tests seed.
- Assert `finding_ledgers` maps both ids to one `CoveragePaths` whose `findings` path is the seeded ledger.

Run: `cargo test -p rupu-cp --lib findings`
Expected: PASS, and the existing `collect_all_findings` tests still pass.

- [ ] **Step 2: Write the failing end-to-end tests**

Create `crates/rupu-cli/tests/findings_import.rs`:
- Copy `full_record`, `seed` and `rupu`/`rupu_in` from `tests/findings_export.rs` verbatim. Copy them rather than sharing them: integration test files are separate crates here.
- Add a `summary_record(id)` helper that builds a `FindingRecord` with `profile: FindingProfile::Summary`, `report: None`, `scope: FindingScope::Line`, `file_path: Some("src/routes/notes.rs")` and `line_range: Some([40, 58])`.

Then write these tests:

```rust
const PLAIN: &str =
    include_str!("../../rupu-findings-report/tests/fixtures/import/notebin_plain.md");
const ID1: &str = "fnd_01J00000000000000000000001";

fn ledger(repo: &Path) -> String {
    std::fs::read_to_string(CoveragePaths::new(repo, "tgt1").findings).unwrap()
}

#[test]
fn imports_a_directory_and_attaches_by_cited_id() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1), summary_record("fnd_01J00000000000000000000009")]);
    let reports = home.path().join("reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(reports.join("NB-001.md"), PLAIN).unwrap();
    std::fs::write(reports.join("README.md"), "# Reports\n\nOne per finding.\n").unwrap();
    let before = ledger(&repo);

    let out = rupu(home.path())
        .args(["findings", "import"])
        .arg(&reports)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("attached"), "{out}");
    assert!(out.contains(ID1), "{out}");
    assert!(out.contains("README.md: not a finding report"), "{out}");
    assert!(out.contains("1 attached, 1 skipped, 0 failed"), "{out}");

    let after = ledger(&repo);
    let first: serde_json::Value = serde_json::from_str(after.lines().next().unwrap()).unwrap();
    assert_eq!(first["profile"], "full");
    assert_eq!(first["report"]["title"], "Notes API returns another user's note by id");
    // The other finding's line is untouched.
    assert_eq!(after.lines().nth(1), before.lines().nth(1));
    let backups: Vec<_> = std::fs::read_dir(CoveragePaths::new(&repo, "tgt1").root)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("pre-import"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), before);
}

#[test]
fn a_dry_run_changes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let repo = seed(home.path(), &[summary_record(ID1)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let before = ledger(&repo);
    let out = rupu(home.path())
        .args(["findings", "import", "--dry-run"])
        .arg(&file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("would attach"), "{out}");
    assert_eq!(ledger(&repo), before);
}

#[test]
fn a_report_for_an_unknown_finding_fails_the_command() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record("fnd_01J00000000000000000000009")]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    let assert = rupu(home.path())
        .args(["findings", "import"])
        .arg(&file)
        .assert()
        .failure();
    let out = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(out.contains("no finding"), "{out}");
}

#[test]
fn id_overrides_the_cited_id_for_a_single_file() {
    let home = tempfile::tempdir().unwrap();
    let other = "fnd_01J00000000000000000000009";
    let repo = seed(home.path(), &[summary_record(other)]);
    let file = home.path().join("NB-001.md");
    std::fs::write(&file, PLAIN).unwrap();
    rupu(home.path())
        .args(["findings", "import", "--id", other])
        .arg(&file)
        .assert()
        .success();
    let first: serde_json::Value = serde_json::from_str(ledger(&repo).lines().next().unwrap()).unwrap();
    assert_eq!(first["id"], other);
    assert_eq!(first["profile"], "full");
}

#[test]
fn id_needs_exactly_one_file() {
    let home = tempfile::tempdir().unwrap();
    seed(home.path(), &[summary_record(ID1)]);
    let dir = home.path().join("reports");
    std::fs::create_dir_all(&dir).unwrap();
    rupu(home.path())
        .args(["findings", "import", "--id", ID1])
        .arg(&dir)
        .assert()
        .failure();
}
```

Run: `cargo test -p rupu-cli --test findings_import`
Expected: FAIL (`import` is not a `findings` subcommand).

- [ ] **Step 3: The CLI action**

In `crates/rupu-cli/src/cmd/findings.rs`, add the variant and its args. Doc comments become `--help`.

```rust
    /// Attach reports written before the full profile to their findings.
    ///
    /// A one-time migration aid: reads Markdown finding reports in the report
    /// layout, and attaches each to the summary finding whose `fnd_` id it
    /// cites. The finding becomes a full-profile finding. A report attaches
    /// whole or not at all; a finding that already has a report is left
    /// alone; each changed ledger is backed up first
    /// (`findings.jsonl.pre-import-<time>`).
    Import(ImportArgs),
```

```rust
#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Report files, or directories to search for `*.md` files.
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// The finding a single report belongs to, when it does not cite exactly
    /// one finding id itself. Only with one file.
    #[arg(long = "id", value_name = "FINDING_ID", value_parser = non_blank)]
    id: Option<String>,
    /// Parse and validate every report; change nothing.
    #[arg(long)]
    dry_run: bool,
}
```

Add `Action::Import(_) => "findings import"` to `ensure_output_format`, and `Action::Import(args) => import_cmd(&args)` to `handle`.

Then implement `import_cmd`:

```rust
/// Files larger than this are not finding reports.
const IMPORT_MAX_BYTES: u64 = 4 * 1024 * 1024;

enum Line {
    Attached(String),
    Skipped(String),
    Failed(String, Vec<String>),
}

fn import_cmd(args: &ImportArgs) -> anyhow::Result<()> {
    use rupu_coverage::tools::{attach_reports, AttachItem, AttachOutcome};
    use rupu_findings_report::import::{parse_report, retain_known_cross_references, Parsed};

    let files = report_files(&args.paths)?;
    if args.id.is_some() && (files.len() != 1 || !args.paths[0].is_file()) {
        anyhow::bail!("--id needs exactly one report file");
    }
    let global = crate::paths::global_dir()?;
    // Limits come from the global config, as for the export prefix; an
    // unreadable config falls back to the defaults with a warning.
    let cfg_path = global.join("config.toml");
    let cfg = match rupu_config::layer_files_locked(Some(&cfg_path), None) {
        Ok(c) => c.findings,
        Err(e) => {
            crate::output::diag::warn(
                &crate::output::diag::prefs_for_diag(false),
                format!("cannot read {}: {e}; using the default findings limits", cfg_path.display()),
            );
            Default::default()
        }
    };
    let opts = crate::findings_opts::base_options(&global, &cfg);
    let ledgers = cp_findings::finding_ledgers(&global);

    let mut lines: std::collections::BTreeMap<PathBuf, Line> = Default::default();
    // finding id → (file, report)
    let mut wanted: std::collections::BTreeMap<String, Vec<(PathBuf, rupu_coverage::FindingReport)>> =
        Default::default();
    for file in &files {
        let parsed = std::fs::metadata(file)
            .map_err(anyhow::Error::from)
            .and_then(|m| {
                if m.len() > IMPORT_MAX_BYTES {
                    anyhow::bail!("larger than 4 MiB");
                }
                Ok(std::fs::read_to_string(file)?)
            })
            .and_then(|md| Ok(parse_report(&md)?));
        match parsed {
            Err(e) => {
                lines.insert(file.clone(), Line::Failed(e.to_string(), vec![]));
            }
            Ok(Parsed::NotAReport) => {
                lines.insert(file.clone(), Line::Skipped("not a finding report".into()));
            }
            Ok(Parsed::Report { report, cited_ids }) => {
                let id = match (&args.id, cited_ids.as_slice()) {
                    (Some(id), _) => id.clone(),
                    (None, [one]) => one.clone(),
                    (None, []) => {
                        lines.insert(file.clone(), Line::Failed("cites no finding id; import it alone with --id".into(), vec![]));
                        continue;
                    }
                    (None, many) => {
                        lines.insert(file.clone(), Line::Failed(format!("cites several finding ids ({}); import it alone with --id", many.join(", ")), vec![]));
                        continue;
                    }
                };
                wanted.entry(id).or_default().push((file.clone(), report));
            }
        }
    }

    // Group by ledger; refuse ids claimed twice or not found exactly once.
    let mut by_ledger: std::collections::BTreeMap<PathBuf, (rupu_coverage::CoveragePaths, Vec<(PathBuf, AttachItem)>)> =
        Default::default();
    for (id, reports) in wanted {
        if reports.len() > 1 {
            let names: Vec<String> = reports.iter().map(|(f, _)| f.display().to_string()).collect();
            for (f, _) in reports {
                lines.insert(f, Line::Failed(format!("{} reports cite {id}: {}", names.len(), names.join(", ")), vec![]));
            }
            continue;
        }
        let (file, mut report) = reports.into_iter().next().expect("one report");
        let paths = match ledgers.get(&id).map(Vec::as_slice) {
            Some([p]) => p.clone(),
            Some(_) => {
                lines.insert(file, Line::Failed(format!("{id} is in more than one ledger"), vec![]));
                continue;
            }
            None => {
                lines.insert(file, Line::Failed(format!("no finding {id} in any registered project"), vec![]));
                continue;
            }
        };
        // A ledger only accepts cross-references to its own findings.
        let same_ledger: std::collections::HashSet<String> = ledgers
            .iter()
            .filter(|(_, ps)| ps.iter().any(|p| p.findings == paths.findings))
            .map(|(i, _)| i.clone())
            .collect();
        retain_known_cross_references(&mut report, &same_ledger);
        by_ledger
            .entry(paths.findings.clone())
            .or_insert_with(|| (paths.clone(), Vec::new()))
            .1
            .push((file, AttachItem { finding_id: id, report }));
    }

    let mut backups = Vec::new();
    for (_, (paths, items)) in by_ledger {
        let (files, items): (Vec<PathBuf>, Vec<AttachItem>) = items.into_iter().unzip();
        let ids: Vec<String> = items.iter().map(|i| i.finding_id.clone()).collect();
        let batch = attach_reports(&paths, items, &opts, args.dry_run)
            .with_context(|| format!("cannot update {}", paths.findings.display()))?;
        backups.extend(batch.backup);
        for ((file, id), outcome) in files.into_iter().zip(ids).zip(batch.outcomes) {
            let line = match outcome {
                AttachOutcome::Attached => Line::Attached(id),
                AttachOutcome::AlreadyHasReport => Line::Skipped(format!("{id} already has a report")),
                AttachOutcome::NotFound => Line::Failed(format!("no finding {id} in its ledger any more"), vec![]),
                AttachOutcome::Duplicate => Line::Failed(format!("{id} appears more than once in its ledger"), vec![]),
                AttachOutcome::Rejected(rupu_coverage::tools::report_finding::ReportFindingError::Report(v)) => Line::Failed(
                    format!("{} problem(s) in the report", v.0.len()),
                    v.0.iter().map(|e| format!("{}: {}", e.path, e.message)).collect(),
                ),
                AttachOutcome::Rejected(e) => Line::Failed(e.to_string(), vec![]),
            };
            lines.insert(file, line);
        }
    }

    let (mut attached, mut skipped, mut failed) = (0, 0, 0);
    let verb = if args.dry_run { "would attach" } else { "attached" };
    for (file, line) in &lines {
        match line {
            Line::Attached(id) => {
                attached += 1;
                println!("{verb:<12} {} → {id}", file.display());
            }
            Line::Skipped(why) => {
                skipped += 1;
                println!("{:<12} {}: {why}", "skipped", file.display());
            }
            Line::Failed(why, details) => {
                failed += 1;
                println!("{:<12} {}: {why}", "failed", file.display());
                for d in details {
                    println!("{:<12}   {d}", "");
                }
            }
        }
    }
    for b in &backups {
        println!("{:<12} {}", "backup", b.display());
    }
    println!("{attached} {verb}, {skipped} skipped, {failed} failed");
    if failed > 0 {
        anyhow::bail!("{failed} of {} report(s) were not imported", lines.len());
    }
    Ok(())
}

/// The files named, plus every `*.md` under the directories named, in path
/// order. Hidden directories and files are skipped.
fn report_files(paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&path, out)?;
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for p in paths {
        let meta = std::fs::metadata(p).with_context(|| format!("cannot read {}", p.display()))?;
        if meta.is_dir() {
            walk(p, &mut out).with_context(|| format!("cannot read {}", p.display()))?;
        } else {
            out.push(p.clone());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}
```

Adapt these names to the real API where they differ, and keep the behaviour:
- `crate::paths::global_dir()` may return a `PathBuf` rather than a `Result`.
- `rupu_config::layer_files_locked` returns the merged config. If reading it fails, fall back to defaults with a warning, the way `export_prefix` does. Do not fail silently.
- `ReportValidationError`'s field may not be `.0`.
- The `ReportFindingError` path may differ.

Keep `import_cmd` as glue. If it grows a helper that is domain logic rather than I/O or printing, that helper belongs in a library.

Run: `cargo test -p rupu-cli --test findings_import`, then `cargo test -p rupu-cli`
Expected: PASS.

- [ ] **Step 4: Docs**

**`docs/coverage.md`:** after the "Exporting reports" section, add `### Importing reports written before the full profile`. It must say:
- `rupu findings import <PATH>... [--id FINDING_ID] [--dry-run]` is a one-time migration aid, not an input path.
- Which layouts it reads: the exporter's, and the plain-text one with bare headings and `Label: value` fields.
- How the finding is chosen: the one `fnd_` id the report cites outside Cross-References, or `--id` for a single file.
- What happens to the finding. It becomes full-profile. Summary, severity and evidence are re-derived from the report as for any full finding. Id and provenance are kept. A finding that already has a report is never changed. The ledger is backed up as `findings.jsonl.pre-import-<UTC time>` before it is rewritten. Every other line is written back unchanged.
- The rules for missing content, quoting `Not stated in the imported report.` and the `Not Provided — section missing from the imported report` sentinel.
- Cross-reference text is kept in References, and links are kept only to findings in the same ledger.
- Code blocks in a call chain become evidence claims.
- Artifacts listed in the report must exist in the workspace.
- A file holding several findings is refused.
- The exit status is non-zero when any report fails.
- Known limit: a placed or remote run that syncs its ledger back while an import runs is not covered by the ledger lock. Import when no placed runs are active.

**`CLAUDE.md`:**
- In the `rupu-findings-report` entry, add: `import` (best-effort Markdown → `FindingReport` parser behind `rupu findings import`).
- In the `rupu-coverage` entry, add: `tools::attach_report::attach_reports`, the only path that rewrites a finding line (sidecar `findings.jsonl.lock`, byte-for-byte backup, atomic rename).
- In "Read first", add: `- Finding reports Plan 5 (backfill importer: \`rupu findings import\`): \`docs/superpowers/plans/2026-09-30-rupu-finding-reports-plan-5-import.md\``.

**The spec:** in its "Deviations as built" section, add a Plan 4 line and a Plan 5 paragraph.
- Plan 4 (macOS parity) was dropped: the macOS app is deprecated.
- Plan 5: the importer matches by the `fnd_` id cited outside Cross-References, with `--id` as the override. The rewrite is a single locked, backed-up, atomic replacement. Missing content follows the no-invention rules above.

Run `make macos-fixtures`, then `git status --short apps/rupu-macos/Fixtures`. Expected: no changes.

- [ ] **Step 5: Gate and commit**

Run each separately and record the results:
- `cargo test --workspace < /dev/null`
- `cargo clippy --workspace --all-targets -- -D warnings -A clippy::question_mark`
- `cargo build -p rupu-cli --no-default-features`
- `rustfmt --edition 2021` on each Rust file you touched, never a `lib.rs`/`mod.rs`

```bash
git add crates/rupu-cp/src/api/findings.rs crates/rupu-cli/src/cmd/findings.rs crates/rupu-cli/tests/findings_import.rs docs/coverage.md CLAUDE.md docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md
git commit -m "feat(cli): rupu findings import — attach pre-full-profile reports to their findings"
```
