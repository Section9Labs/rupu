# Remote findings — Plan A: the coverage stream Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A remote unit's coverage (runs, file touches, concern assertions, findings) reaches the coordinator on every transport, whether the unit succeeds or fails. It's merged into the coordinator workspace under the right target, and its finding artifacts are recorded `stored: external` with `host` set.

**Architecture:**
1. `rupu run` copies every coverage ledger write into `$RUPU_HOME/runs/<run_id>/coverage.jsonl`, a "run stream".
2. Each host connector delivers that file through a new required `HostConnector::unit_coverage`. SSH, tunnel and bucket mirror it with the run's other files; HTTP fetches it from the remote CP; local reads it directly.
3. The fleet dispatcher attaches it to every post-launch outcome (`UnitOutcome` / the new `UnitFailure`).
4. The runner merges it with the pure `rupu_coverage::ingest_unit_stream`, and emits `Event::StepWarning` when it can't.

**Tech Stack:** Rust 2021, serde / serde_json, tokio, axum 0.7, reqwest 0.12, git2, object_store 0.14.

**Spec:** `docs/superpowers/specs/2026-09-30-rupu-remote-findings-transport-design.md` (Plan A = §A1–A4). Parent: `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md`.

**Branch:** `claude/remote-findings-transport`, stacked on Section9Labs/rupu#675 (`claude/findings-profile-remote-units`).

## Global Constraints

**Workspace and hygiene**
- Workspace deps only: versions live in the root `Cargo.toml`, and crates use `x.workspace = true`.
- `#![deny(clippy::all)]` applies workspace-wide, and `unsafe_code` is forbidden.
- Never run `cargo fmt`. The local Homebrew rustfmt disagrees with the committed style, so format only lines you wrote. To see rustfmt's opinion of a file without writing it: `rustfmt --edition 2021 --emit stdout --config skip_children=true <file>`. Hand-apply only the hunks that touch your own lines. Never rustfmt a `mod.rs` or crate root; it recurses into child modules.
- Never use bare `git stash` / `git stash pop`.

**Commands**
- Per-crate tests: `cargo test -p <crate>`.
- Clippy: `cargo clippy -p <crates> --all-targets -- -D warnings -A clippy::question_mark`. `crates/rupu-cli/src/cmd/completers.rs:127` has a pre-existing `question_mark` hit under the local clippy 1.97; the repo pins 1.95.

**Exact names**
- Stream file name: `coverage.jsonl` (`rupu_coverage::STREAM_FILE`).
- Stream line tags (`"ledger"`): `begin`, `runs`, `files`, `concerns`, `findings`, `catalog`.
- The begin line is `{"ledger":"begin","v":1,"run_id":"<id>"}`.
- Capability strings:
  - `mirror.coverage`: the CP advertises it in the tunnel `Welcome`.
  - `run.coverage_stream`: advertised in HTTP `/api/host/info` `features`.
- A remote SSH host's RUPU_HOME is `$HOME/.rupu`, the existing convention in `ssh.rs`.
- The local host id is the literal `"local"`. Every other host id is a `HostRegistry` id, which is exactly the `host: &str` passed to `UnitDispatcher::dispatch_unit`.

**Behaviour rules**
- A coverage warning never fails a unit.
- The coordinator's own ledgers never get a run stream. Only `rupu run` writes one.

## Deviations from the spec (flag in the PR)

1. **The stream path lives on `ToolContext.coverage_stream`,** not a new `AgentRunOpts` field. `AgentRunOpts` has 58 struct literals and no `Default`; `ToolContext` has 7 exhaustive literals, and already carries run-scoped coverage state (`coverage_writer`, `run_id`, `findings`).
2. **`.rupu/coverage/` is excluded when the delta is collected, not ignored on apply.** Collection runs on the host (`rupu __workspace collect`), so an older host's delta still carries its coverage. Applying it keeps that host's findings arriving exactly as they do today, instead of dropping them.
3. **`Event::StepWarning` is deliberately left out of the macOS fixture.** The app is deprecated, and it decodes unknown tags as `.unknown`. Adding it to the fixture would force Swift work, because the Swift fixture test rejects `.unknown`.
4. **`dispatch_agent` children on a host stream into the unit's file.** `CliAgentDispatcher` carries the stream path, so their findings reach the coordinator too; the spec didn't mention this.

## File map

| File | Responsibility |
|---|---|
| `crates/rupu-coverage/src/ledger/stream.rs` (new) | `RunStream`, `Ledger`, `StreamLine`, `append_record`, `stream_catalog`, `write_stream_begin`, `stream_path`, `STREAM_FILE` |
| `crates/rupu-coverage/src/ledger/ingest.rs` (new) | `IngestSource`, `IngestReport`, `IngestError`, `ingest_unit_stream` |
| `crates/rupu-coverage/src/ledger/{paths,manifest,writer,mod}.rs`, `tools/{coverage_mark,report_finding}.rs`, `lib.rs` | `CoveragePaths.run_stream`; every ledger write through `append_record` |
| `crates/rupu-tools/src/tool.rs` | `ToolContext.coverage_stream` |
| `crates/rupu-agent/src/runner.rs` | build `CoveragePaths` with the stream, stream the catalog |
| `crates/rupu-cli/src/cmd/{run,dispatch,session,workflow}.rs`, `resume.rs`, `crates/rupu-orchestrator/src/step_factory.rs` | begin line + stream for `rupu run`; `CliAgentDispatcher` passes it to children |
| `crates/rupu-cp/src/node/{protocol,mirror,server}.rs` | `ArtifactFile::Coverage`, `Welcome.capabilities`, capability constants, `replace_coverage` |
| `crates/rupu-cp/src/host/{connector,local,ssh,tunnel,http}.rs`, `host/bucket/{connector,poller}.rs` | `HostConnector::unit_coverage`; SSH tail + terminal catch-up; bucket key classification |
| `crates/rupu-cp/src/api/{runs,host_info}.rs`, `embed.rs` | `GET /api/runs/:id/coverage`, `run.coverage_stream` feature, JSON 404 for unmatched `/api/*` |
| `crates/rupu-cli/src/cmd/node.rs` | tunnel node + bucket worker ship the stream |
| `crates/rupu-orchestrator/src/runner.rs`, `executor/event.rs` | `UnitCoverage`, `UnitFailure`, `UnitOutcome.coverage`, `Event::StepWarning`, merge on every remote outcome |
| `crates/rupu-cli/src/fleet_unit_dispatcher.rs` | collect coverage on every post-launch outcome |
| `crates/rupu-cli/src/output/live_run.rs` | show `StepWarning` |
| `crates/rupu-workspace/src/workspace_sync.rs` | exclude `.rupu/coverage/` from collected deltas |
| `crates/rupu-orchestrator/tests/remote_coverage_ingest.rs` (new) | end to end |
| `docs/coverage.md`, `docs/workflow-format.md`, `CLAUDE.md` | docs |

---

### Task 1: The run stream and a single ledger write path (`rupu-coverage`)

**Files:**
- Create: `crates/rupu-coverage/src/ledger/stream.rs`
- Modify: `crates/rupu-coverage/src/ledger/paths.rs`, `ledger/manifest.rs:30-41`, `ledger/writer.rs:55-84`, `ledger/mod.rs`, `tools/coverage_mark.rs:84-93`, `tools/report_finding.rs:191-204`, `lib.rs:39-46`

**Interfaces:**
- Produces:
  - `pub struct RunStream { pub path: PathBuf, pub scope_name: String }`
  - `pub enum Ledger { Runs, Files, Concerns, Findings, Catalog }`
  - `pub enum StreamLine { Begin{v,run_id}, Runs{scope_name,record: RunManifest}, Files{scope_name,record: FileTouchEvent}, Concerns{scope_name,record: ConcernAssertion}, Findings{scope_name,record: FindingRecord}, Catalog{scope_name,record: FlatCatalog} }`
  - `pub fn append_record(paths: &CoveragePaths, ledger: Ledger, record: &impl Serialize) -> std::io::Result<()>`
  - `pub fn stream_catalog(paths: &CoveragePaths, catalog: &FlatCatalog) -> std::io::Result<()>`
  - `pub fn write_stream_begin(path: &Path, run_id: &str) -> std::io::Result<()>`
  - `pub fn stream_path(runs_root: &Path, run_id: &str) -> PathBuf`
  - `pub const STREAM_FILE: &str = "coverage.jsonl"`
  - `CoveragePaths.run_stream: Option<RunStream>` and `CoveragePaths::with_run_stream(self, Option<RunStream>) -> Self`
  - All of the above are re-exported at the crate root.

- [ ] **Step 1: Write the failing tests.** Create `crates/rupu-coverage/src/ledger/stream.rs` with only the test module below, and add `pub mod stream;` to `ledger/mod.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{
        AssertionStatus, Attribution, ConcernAssertion, Evidence, FindingEvidence, FindingRecord,
        FindingScope, Surface,
    };
    use crate::ledger::paths::CoveragePaths;
    use chrono::Utc;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_S1".into(),
            model: "m".into(),
            surface: Surface::Agent,
        }
    }

    pub(crate) fn finding(id: &str) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::File,
            summary: "s".into(),
            severity: crate::Severity::Medium,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
        }
    }

    fn lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn append_record_writes_the_ledger_and_the_stream() {
        let tmp = tempfile::tempdir().unwrap();
        let stream = tmp.path().join("runs/run_S1/coverage.jsonl");
        let paths = CoveragePaths::new(tmp.path(), "t1").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        paths.ensure_dir().unwrap();

        append_record(&paths, Ledger::Findings, &finding("f1")).unwrap();

        let ledger = lines(&paths.findings);
        assert_eq!(ledger.len(), 1);
        let back: FindingRecord = serde_json::from_str(&ledger[0]).unwrap();
        assert_eq!(back.id, "f1");

        let streamed = lines(&stream);
        assert_eq!(streamed.len(), 1);
        match serde_json::from_str::<StreamLine>(&streamed[0]).unwrap() {
            StreamLine::Findings { scope_name, record } => {
                assert_eq!(scope_name, "sec");
                assert_eq!(record.id, "f1");
            }
            other => panic!("expected a findings line, got {other:?}"),
        }
    }

    #[test]
    fn without_a_run_stream_only_the_ledger_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t1");
        paths.ensure_dir().unwrap();
        let a = ConcernAssertion {
            concern_id: "c1".into(),
            file_path: "a.rs".into(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "ok".into(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        append_record(&paths, Ledger::Concerns, &a).unwrap();
        assert_eq!(lines(&paths.concerns).len(), 1);
        assert!(paths.run_stream.is_none());
    }

    #[test]
    fn catalog_is_a_snapshot_not_a_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t1");
        let err = append_record(&paths, Ledger::Catalog, &"x").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn stream_catalog_and_begin_lines_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let stream = stream_path(&tmp.path().join("runs"), "run_S1");
        assert!(stream.ends_with("runs/run_S1/coverage.jsonl"));
        write_stream_begin(&stream, "run_S1").unwrap();
        let paths = CoveragePaths::new(tmp.path(), "t1").with_run_stream(Some(RunStream {
            path: stream.clone(),
            scope_name: "sec".into(),
        }));
        let catalog = crate::FlatCatalog::default();
        stream_catalog(&paths, &catalog).unwrap();

        let got: Vec<StreamLine> = lines(&stream)
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            got,
            vec![
                StreamLine::Begin {
                    v: STREAM_VERSION,
                    run_id: "run_S1".into()
                },
                StreamLine::Catalog {
                    scope_name: "sec".into(),
                    record: catalog
                },
            ]
        );
        assert_eq!(
            lines(&stream)[0],
            r#"{"ledger":"begin","v":1,"run_id":"run_S1"}"#
        );
    }
}
```

If `FlatCatalog` does not implement `Default`, build it as `FlatCatalog { concerns: vec![], sources: Default::default(), render_modes: Default::default() }`. Its fields are `concerns: Vec<Concern>`, `sources: BTreeMap<String, String>` and `render_modes: BTreeMap<String, CatalogMode>`.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-coverage --lib ledger::stream`
Expected: compile errors, because `RunStream`, `append_record` and the other items don't exist yet.

- [ ] **Step 3: Implement `stream.rs`.** Put this above the test module:

```rust
//! A run's coverage, copied line by line into one run-scoped file so a
//! coordinator can collect a remote unit's coverage (spec
//! 2026-09-30-rupu-remote-findings-transport-design.md §A1).
//!
//! Every ledger write goes through [`append_record`]: the ledger line first,
//! then — when the paths carry a [`RunStream`] — the same record wrapped in a
//! [`StreamLine`] envelope. The stream carries `scope_name`, never
//! `target_id`: the target id hashes the host's workspace path, so the
//! coordinator recomputes it for its own workspace.

use crate::catalog::types::FlatCatalog;
use crate::ledger::events::{ConcernAssertion, FileTouchEvent, FindingRecord};
use crate::ledger::manifest::RunManifest;
use crate::ledger::paths::CoveragePaths;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The stream's file name inside `runs/<run_id>/`.
pub const STREAM_FILE: &str = "coverage.jsonl";
/// Written into the begin line; bump on an incompatible envelope change.
pub const STREAM_VERSION: u32 = 1;

/// Where a run's coverage is streamed, and the scope its records belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStream {
    pub path: PathBuf,
    pub scope_name: String,
}

/// Which coverage file a record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ledger {
    Runs,
    Files,
    Concerns,
    Findings,
    /// The catalog snapshot (`catalog.yaml`); streamed, never appended.
    Catalog,
}

impl Ledger {
    pub fn as_str(self) -> &'static str {
        match self {
            Ledger::Runs => "runs",
            Ledger::Files => "files",
            Ledger::Concerns => "concerns",
            Ledger::Findings => "findings",
            Ledger::Catalog => "catalog",
        }
    }
}

/// One line of a run stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ledger", rename_all = "lowercase")]
pub enum StreamLine {
    /// First line of every stream: proves the host streams at all.
    Begin { v: u32, run_id: String },
    Runs { scope_name: String, record: RunManifest },
    Files { scope_name: String, record: FileTouchEvent },
    Concerns { scope_name: String, record: ConcernAssertion },
    Findings { scope_name: String, record: FindingRecord },
    Catalog { scope_name: String, record: FlatCatalog },
}

/// `<runs_root>/<run_id>/coverage.jsonl`.
pub fn stream_path(runs_root: &Path, run_id: &str) -> PathBuf {
    runs_root.join(run_id).join(STREAM_FILE)
}

/// Append `line` and its newline in ONE write: with two writes, a concurrent
/// appender (a parallel `dispatch_agent` child sharing the stream, or a second
/// writer on the same ledger) could land its line between ours and our
/// newline, fusing two records into one unparseable line.
pub(crate) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    f.write_all(buf.as_bytes())?;
    f.flush()
}

/// `{"ledger":"<ledger>","scope_name":<json>,"record":<record_json>}` —
/// built around the already-serialized record so it is encoded once.
pub(crate) fn envelope(
    ledger: Ledger,
    scope_name: &str,
    record_json: &str,
) -> std::io::Result<String> {
    Ok(format!(
        "{{\"ledger\":\"{}\",\"scope_name\":{},\"record\":{}}}",
        ledger.as_str(),
        serde_json::to_string(scope_name)?,
        record_json
    ))
}

/// Mirror an already-serialized record into the run stream, if any. The
/// ledger line is the primary record and has already landed, so a stream
/// failure is logged loudly and not returned: returning it would make a
/// caller retry — and duplicate — a record that was written.
pub(crate) fn stream_json(paths: &CoveragePaths, ledger: Ledger, record_json: &str) {
    let Some(rs) = &paths.run_stream else {
        return;
    };
    if let Err(e) =
        envelope(ledger, &rs.scope_name, record_json).and_then(|l| append_line(&rs.path, &l))
    {
        tracing::error!(
            error = %e,
            path = %rs.path.display(),
            ledger = ledger.as_str(),
            "coverage stream write failed; a coordinator will not see this record"
        );
    }
}

/// Write `record` to its ledger under `paths`, then to the run stream.
pub fn append_record(
    paths: &CoveragePaths,
    ledger: Ledger,
    record: &impl Serialize,
) -> std::io::Result<()> {
    let file = match ledger {
        Ledger::Runs => &paths.runs,
        Ledger::Files => &paths.files,
        Ledger::Concerns => &paths.concerns,
        Ledger::Findings => &paths.findings,
        Ledger::Catalog => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the catalog is a snapshot, not a ledger; use stream_catalog",
            ))
        }
    };
    let json = serde_json::to_string(record)?;
    append_line(file, &json)?;
    stream_json(paths, ledger, &json);
    Ok(())
}

/// Stream the catalog snapshot a run writes at start. No-op without a stream.
pub fn stream_catalog(paths: &CoveragePaths, catalog: &FlatCatalog) -> std::io::Result<()> {
    let json = serde_json::to_string(catalog)?;
    stream_json(paths, Ledger::Catalog, &json);
    Ok(())
}

/// Write a stream's first line (creating the file and its directory).
pub fn write_stream_begin(path: &Path, run_id: &str) -> std::io::Result<()> {
    let line = serde_json::to_string(&StreamLine::Begin {
        v: STREAM_VERSION,
        run_id: run_id.to_string(),
    })?;
    append_line(path, &line)
}
```

- [ ] **Step 4: Add the stream to `CoveragePaths`.** In `ledger/paths.rs`, add the field and the builder:

```rust
    pub runs: PathBuf,
    /// Where this run's coverage is also streamed (`rupu run` only). `None`
    /// for every other writer — the coordinator's own ledgers never stream.
    pub run_stream: Option<crate::ledger::stream::RunStream>,
}
```

In `new`, add `run_stream: None,` to the `Self { … }` literal. Then add this after `ensure_dir`:

```rust
    /// Attach (or clear) the run stream every append through
    /// [`crate::ledger::stream::append_record`] mirrors into.
    pub fn with_run_stream(mut self, stream: Option<crate::ledger::stream::RunStream>) -> Self {
        self.run_stream = stream;
        self
    }
```

- [ ] **Step 5: Route the three synchronous write sites through `append_record`.**

`ledger/manifest.rs`: replace the body of `append_manifest` (lines 31-41) with:

```rust
pub fn append_manifest(paths: &CoveragePaths, manifest: &RunManifest) -> std::io::Result<()> {
    crate::ledger::stream::append_record(paths, crate::ledger::stream::Ledger::Runs, manifest)
}
```

Then remove `use std::io::Write;` from `manifest.rs` if nothing else in the file uses it.

`tools/coverage_mark.rs`: replace lines 84-93 (from `paths.ensure_dir()?;` through `f.flush()?;`) with:

```rust
    paths.ensure_dir()?;
    crate::ledger::stream::append_record(paths, crate::ledger::stream::Ledger::Concerns, &assertion)?;
```

`tools/report_finding.rs`: replace lines 191-204 (from `paths.ensure_dir()?;` through `f.flush()?;`) with:

```rust
    paths.ensure_dir()?;
    // One write per line, then the run stream — see `ledger::stream::append_line`.
    crate::ledger::stream::append_record(paths, crate::ledger::stream::Ledger::Findings, &record)?;
    Ok(ReportFindingOutput { id })
```

Keep the existing `Ok(ReportFindingOutput { id })` only once.

Both error enums already convert from `std::io::Error`, since they used `?` on `ensure_dir`. If the compiler says `CoverageMarkError` or `ReportFindingError` lacks `From<std::io::Error>`, add an `Io(#[from] std::io::Error)` variant, following the enum's existing variant style.

- [ ] **Step 6: Stream the async `files.jsonl` writer.** In `ledger/writer.rs`, replace the `WriteRequest::File(ev)` arm body (lines 71-76) with the code below. It writes the line and newline once, then streams the line:

```rust
            WriteRequest::File(ev) => {
                if let Ok(mut line) = serde_json::to_string(&ev) {
                    line.push('\n');
                    let _ = files_f.write_all(line.as_bytes()).await;
                    line.pop();
                    crate::ledger::stream::stream_json(
                        &paths,
                        crate::ledger::stream::Ledger::Files,
                        &line,
                    );
                }
            }
```

`stream_json` is a small synchronous append. The writer task already owns `paths`, so no extra state is needed.

- [ ] **Step 7: Export.** In `ledger/mod.rs`, next to the other `pub use` lines:

```rust
pub use stream::{
    append_record, stream_catalog, stream_path, write_stream_begin, Ledger, RunStream, StreamLine,
    STREAM_FILE, STREAM_VERSION,
};
```

In `lib.rs`, extend the `pub use ledger::{ … }` list with `append_record, stream_catalog, stream_path, write_stream_begin, Ledger, RunStream, StreamLine, STREAM_FILE, STREAM_VERSION`.

- [ ] **Step 8: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-coverage`
Expected: PASS. The new `ledger::stream` tests pass, and the existing `manifest` / `coverage_mark` / `report_finding` / `writer` tests still pass.

- [ ] **Step 9: Commit.**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): run stream — every ledger write through append_record, mirrored to runs/<id>/coverage.jsonl"
```

---

### Task 2: Merging a unit's stream into a workspace (`rupu-coverage`)

**Files:**
- Create: `crates/rupu-coverage/src/ledger/ingest.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs`

**Interfaces:**
- Consumes: Task 1's `StreamLine`, `append_record`, `Ledger`; the existing `target_id`, `read_findings`, `read_file_events`, `read_concern_assertions`, `read_manifests`, `write_snapshot`.
- Produces:
  - `pub struct IngestSource { pub host: Option<String> }`. `None` means the same machine, so the artifact store is shared.
  - `pub struct IngestReport { pub begin_seen: bool, pub appended: usize, pub duplicates: usize, pub malformed: usize, pub targets: BTreeSet<String> }`
  - `pub enum IngestError { Io, Encode, Catalog }`
  - `pub fn ingest_unit_stream(workspace: &Path, source: &IngestSource, stream: &[u8]) -> Result<IngestReport, IngestError>`
  - All re-exported at the crate root.

- [ ] **Step 1: Write the failing tests.** Create `crates/rupu-coverage/src/ledger/ingest.rs` with only the test module below, and add `pub mod ingest;` to `ledger/mod.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{
        Attribution, FindingEvidence, FindingRecord, FindingScope, Surface,
    };
    use crate::ledger::stream::{StreamLine, STREAM_VERSION};
    use crate::report::{ArtifactKind, ArtifactRef, ArtifactStorage, FindingReport};
    use chrono::Utc;

    fn finding(id: &str, with_artifact: bool) -> FindingRecord {
        let report = with_artifact.then(|| {
            let mut r: FindingReport = serde_json::from_str(include_str!(
                "../../tests/fixtures/finding_report/valid_full.json"
            ))
            .unwrap();
            r.artifacts = vec![ArtifactRef {
                path: "poc/exploit.py".into(),
                sha256: "ab".repeat(32),
                size: 12,
                kind: Some(ArtifactKind::Text),
                stored: Some(ArtifactStorage::Copied),
                host: None,
            }];
            r
        });
        FindingRecord {
            id: id.into(),
            file_path: None,
            line_range: None,
            target_ref: None,
            scope: FindingScope::File,
            summary: "s".into(),
            severity: crate::Severity::High,
            concern_id: None,
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: "run_U1".into(),
                model: "m".into(),
                surface: Surface::Agent,
            },
            declared_at: Utc::now(),
            profile: if with_artifact {
                crate::report::FindingProfile::Full
            } else {
                crate::report::FindingProfile::Summary
            },
            report,
        }
    }

    fn stream(lines: &[StreamLine]) -> Vec<u8> {
        let mut out = String::new();
        for l in lines {
            out.push_str(&serde_json::to_string(l).unwrap());
            out.push('\n');
        }
        out.into_bytes()
    }

    fn begin() -> StreamLine {
        StreamLine::Begin {
            v: STREAM_VERSION,
            run_id: "run_U1".into(),
        }
    }

    fn findings_line(scope: &str, f: FindingRecord) -> StreamLine {
        StreamLine::Findings {
            scope_name: scope.into(),
            record: f,
        }
    }

    fn remote() -> IngestSource {
        IngestSource {
            host: Some("host_01REMOTE".into()),
        }
    }

    #[test]
    fn rekeys_by_scope_into_the_coordinator_workspace() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", false))]);
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert!(rep.begin_seen);
        assert_eq!(rep.appended, 1);
        let tid = target_id(ws.path(), "sec");
        assert!(rep.targets.contains(&tid));
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "f1");
    }

    #[test]
    fn a_remote_hosts_artifacts_become_external_with_host() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", true))]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        let a = &got[0].report.as_ref().unwrap().artifacts[0];
        assert_eq!(a.stored, Some(ArtifactStorage::External));
        assert_eq!(a.host.as_deref(), Some("host_01REMOTE"));
        assert_eq!(a.sha256, "ab".repeat(32), "sha256 is kept for the pull's verification");
        assert_eq!(a.size, 12);
        assert_eq!(a.path, "poc/exploit.py");
    }

    #[test]
    fn a_local_hosts_artifacts_stay_copied() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", true))]);
        ingest_unit_stream(ws.path(), &IngestSource { host: None }, &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap();
        let a = &got[0].report.as_ref().unwrap().artifacts[0];
        assert_eq!(a.stored, Some(ArtifactStorage::Copied));
        assert_eq!(a.host, None);
    }

    #[test]
    fn reingesting_the_same_stream_appends_nothing() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[begin(), findings_line("sec", finding("f1", false))]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!(rep.appended, 0);
        assert_eq!(rep.duplicates, 1);
        let tid = target_id(ws.path(), "sec");
        assert_eq!(read_findings(&CoveragePaths::new(ws.path(), &tid)).unwrap().len(), 1);
    }

    #[test]
    fn without_a_begin_line_nothing_is_written() {
        let ws = tempfile::tempdir().unwrap();
        let s = stream(&[findings_line("sec", finding("f1", false))]);
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert!(!rep.begin_seen);
        assert_eq!(rep.appended, 0);
        assert!(rep.targets.is_empty());
        assert!(!ws.path().join(".rupu").exists());
    }

    #[test]
    fn malformed_lines_are_skipped_and_counted() {
        let ws = tempfile::tempdir().unwrap();
        let mut s = stream(&[begin(), findings_line("sec", finding("f1", false))]);
        s.extend_from_slice(b"{not json\n{\"ledger\":\"nope\"}\n");
        let rep = ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        assert_eq!(rep.appended, 1);
        assert_eq!(rep.malformed, 2);
    }

    #[test]
    fn an_empty_stream_is_not_begun() {
        let ws = tempfile::tempdir().unwrap();
        let rep = ingest_unit_stream(ws.path(), &remote(), b"").unwrap();
        assert!(!rep.begin_seen);
    }

    #[test]
    fn catalog_lines_overwrite_the_targets_snapshot() {
        let ws = tempfile::tempdir().unwrap();
        let catalog = crate::FlatCatalog::default();
        let s = stream(&[
            begin(),
            StreamLine::Catalog {
                scope_name: "sec".into(),
                record: catalog.clone(),
            },
        ]);
        ingest_unit_stream(ws.path(), &remote(), &s).unwrap();
        let tid = target_id(ws.path(), "sec");
        let got = crate::read_snapshot(&CoveragePaths::new(ws.path(), &tid).catalog).unwrap();
        assert_eq!(got, catalog);
    }
}
```

As in Task 1, build `FlatCatalog` field by field if it doesn't implement `Default`.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-coverage --lib ledger::ingest`
Expected: compile errors, because `ingest_unit_stream`, `IngestSource` and the other items don't exist yet.

- [ ] **Step 3: Implement `ingest.rs`.** Put this above the test module:

```rust
//! Merge a remote unit's run stream into a workspace's coverage ledgers
//! (spec 2026-09-30-rupu-remote-findings-transport-design.md §A4).
//!
//! Pure file I/O over one workspace: each line is re-keyed to
//! `target_id(workspace, scope_name)`, de-duplicated (findings by id, every
//! other ledger by exact record equality), and — when the unit ran on another
//! machine — its finding artifacts are recorded `stored: external` with that
//! host, since their blobs live in the host's store, not this one.

use crate::catalog::snapshot::write_snapshot;
use crate::ledger::events::FindingRecord;
use crate::ledger::manifest::read_manifests;
use crate::ledger::paths::CoveragePaths;
use crate::ledger::stream::{append_record, Ledger, StreamLine};
use crate::ledger::target_id::target_id;
use crate::ledger::views::{read_concern_assertions, read_file_events, read_findings};
use crate::report::ArtifactStorage;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

/// Where the stream came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestSource {
    /// The registry id of the host the unit ran on; `None` for this machine
    /// (the artifact store is shared, so artifacts stay `copied`).
    pub host: Option<String>,
}

/// What a merge did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// The stream had its `begin` line — i.e. the host streams at all. When
    /// `false` nothing was written.
    pub begin_seen: bool,
    pub appended: usize,
    pub duplicates: usize,
    pub malformed: usize,
    /// Target ids written to (or checked for duplicates).
    pub targets: BTreeSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("coverage ingest I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("coverage ingest encode: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("coverage ingest catalog snapshot: {0}")]
    Catalog(String),
}

/// Keys already present in one target's ledgers.
#[derive(Default)]
struct Seen {
    findings: HashSet<String>,
    runs: HashSet<String>,
    files: HashSet<String>,
    concerns: HashSet<String>,
}

fn load_seen(paths: &CoveragePaths) -> Result<Seen, IngestError> {
    let mut s = Seen::default();
    for f in read_findings(paths)? {
        s.findings.insert(f.id);
    }
    for m in read_manifests(paths)? {
        s.runs.insert(serde_json::to_string(&m)?);
    }
    for e in read_file_events(paths)? {
        s.files.insert(serde_json::to_string(&e)?);
    }
    for a in read_concern_assertions(paths)? {
        s.concerns.insert(serde_json::to_string(&a)?);
    }
    Ok(s)
}

fn mark_external(record: &mut FindingRecord, host: &str) {
    if let Some(report) = record.report.as_mut() {
        for a in &mut report.artifacts {
            a.stored = Some(ArtifactStorage::External);
            a.host = Some(host.to_string());
        }
    }
}

/// Merge `stream` (a unit's `coverage.jsonl`) into `workspace`'s ledgers.
pub fn ingest_unit_stream(
    workspace: &Path,
    source: &IngestSource,
    stream: &[u8],
) -> Result<IngestReport, IngestError> {
    let mut report = IngestReport::default();
    let text = String::from_utf8_lossy(stream);
    let mut lines = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<StreamLine>(raw) {
            Ok(l) => lines.push(l),
            Err(_) => report.malformed += 1,
        }
    }
    report.begin_seen = lines.iter().any(|l| matches!(l, StreamLine::Begin { .. }));
    if !report.begin_seen {
        return Ok(report);
    }

    let mut targets: BTreeMap<String, (CoveragePaths, Seen)> = BTreeMap::new();
    for line in lines {
        let scope = match &line {
            StreamLine::Begin { .. } => continue,
            StreamLine::Runs { scope_name, .. }
            | StreamLine::Files { scope_name, .. }
            | StreamLine::Concerns { scope_name, .. }
            | StreamLine::Findings { scope_name, .. }
            | StreamLine::Catalog { scope_name, .. } => scope_name.clone(),
        };
        if !targets.contains_key(&scope) {
            let tid = target_id(workspace, &scope);
            let paths = CoveragePaths::new(workspace, &tid);
            paths.ensure_dir()?;
            let seen = load_seen(&paths)?;
            report.targets.insert(tid);
            targets.insert(scope.clone(), (paths, seen));
        }
        let (paths, seen) = targets.get_mut(&scope).expect("inserted above");
        let fresh = match line {
            StreamLine::Begin { .. } => continue,
            StreamLine::Runs { record, .. } => {
                let fresh = seen.runs.insert(serde_json::to_string(&record)?);
                if fresh {
                    append_record(paths, Ledger::Runs, &record)?;
                }
                fresh
            }
            StreamLine::Files { record, .. } => {
                let fresh = seen.files.insert(serde_json::to_string(&record)?);
                if fresh {
                    append_record(paths, Ledger::Files, &record)?;
                }
                fresh
            }
            StreamLine::Concerns { record, .. } => {
                let fresh = seen.concerns.insert(serde_json::to_string(&record)?);
                if fresh {
                    append_record(paths, Ledger::Concerns, &record)?;
                }
                fresh
            }
            StreamLine::Findings { mut record, .. } => {
                let fresh = seen.findings.insert(record.id.clone());
                if fresh {
                    if let Some(host) = source.host.as_deref() {
                        mark_external(&mut record, host);
                    }
                    append_record(paths, Ledger::Findings, &record)?;
                }
                fresh
            }
            StreamLine::Catalog { record, .. } => {
                // Latest run wins — the same rule the agent runner applies at
                // run start. Not counted as appended or duplicate.
                write_snapshot(&record, &paths.catalog)
                    .map_err(|e| IngestError::Catalog(e.to_string()))?;
                continue;
            }
        };
        if fresh {
            report.appended += 1;
        } else {
            report.duplicates += 1;
        }
    }
    Ok(report)
}
```

Check that `rupu-coverage/Cargo.toml` already has `thiserror`. It does if any existing module uses `#[derive(thiserror::Error)]` (`grep -rn "thiserror" crates/rupu-coverage/Cargo.toml`). If it doesn't, add `thiserror.workspace = true`.

- [ ] **Step 4: Export.** In `ledger/mod.rs`: `pub use ingest::{ingest_unit_stream, IngestError, IngestReport, IngestSource};`. Add the same four names to the `pub use ledger::{ … }` list in `lib.rs`.

- [ ] **Step 5: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-coverage`
Expected: PASS, including the 8 new `ledger::ingest` tests.

- [ ] **Step 6: Commit.**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): ingest_unit_stream re-keys, de-duplicates and marks remote artifacts external+host"
```

---

### Task 3: `rupu run` streams its coverage, and `dispatch_agent` children share the stream

**Files:**
- Modify: `crates/rupu-tools/src/tool.rs` (the `ToolContext` struct and its `Default` impl at `:108`)
- Modify: `crates/rupu-agent/src/runner.rs:859-866`, `:995-997`
- Modify: `crates/rupu-cli/src/cmd/run.rs` (after the run id at `:564-570`; the `ToolContext` at `:827`; `CliAgentDispatcher::new` at `:808`)
- Modify: `crates/rupu-cli/src/cmd/dispatch.rs` (the struct, `new` at `:95`, `child_tool_ctx` at `:311`, and the 7 test call sites at `:638, :737, :805, :883, :968, :1055, :1133`)
- Modify: `crates/rupu-cli/src/cmd/workflow.rs:3214`, `crates/rupu-cli/src/resume.rs:298`, `crates/rupu-cli/src/cmd/session.rs:7483`, `crates/rupu-orchestrator/src/step_factory.rs:432`, `crates/rupu-orchestrator/tests/dispatch_agent.rs:168,:339`, `crates/rupu-orchestrator/tests/dispatch_agents_parallel.rs:183`
- Test: `crates/rupu-agent/tests/findings_without_coverage.rs`

**Interfaces:**
- Consumes: Task 1's `RunStream`, `stream_catalog`, `write_stream_begin`, `stream_path`.
- Produces:
  - `ToolContext.coverage_stream: Option<PathBuf>`
  - `CliAgentDispatcher::new(…, coverage_stream: Option<PathBuf>)`, a new last parameter
  - `$RUPU_HOME/runs/<run_id>/coverage.jsonl` on every `rupu run`

- [ ] **Step 1: Write the failing test.** Append this to `crates/rupu-agent/tests/findings_without_coverage.rs`. It reuses that file's `opts_for` and `call_then_stop`.

```rust
#[tokio::test]
async fn a_run_stream_receives_the_finding_with_its_scope() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let stream = tmp.path().join("runs/run_findings_test/coverage.jsonl");
    rupu_coverage::write_stream_begin(&stream, "run_findings_test").unwrap();

    let mut opts = opts_for(
        &workspace,
        Some(vec!["report_finding".to_string()]),
        call_then_stop(),
    );
    opts.tool_context.coverage_stream = Some(stream.clone());
    run_agent(opts).await.expect("agent run should succeed");

    let lines: Vec<rupu_coverage::StreamLine> = std::fs::read_to_string(&stream)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(matches!(lines[0], rupu_coverage::StreamLine::Begin { .. }));
    let found = lines.iter().any(|l| matches!(
        l,
        rupu_coverage::StreamLine::Findings { scope_name, record }
            if scope_name == "net-assessor" && record.severity == rupu_coverage::Severity::Medium
    ));
    assert!(found, "the finding must reach the stream: {lines:?}");
}
```

If `rupu_coverage::Severity` isn't re-exported at the crate root, compare `serde_json::to_value(&record.severity).unwrap() == "medium"` instead.

- [ ] **Step 2: Run the test to confirm it fails.**

Run: `cargo test -p rupu-agent --test findings_without_coverage a_run_stream_receives`
Expected: compile error, because `ToolContext` has no `coverage_stream` field.

- [ ] **Step 3: Add `ToolContext.coverage_stream`.** In `crates/rupu-tools/src/tool.rs`, add this after the `findings` field:

```rust
    /// Where this run's coverage is streamed for a coordinator to collect
    /// (`$RUPU_HOME/runs/<run_id>/coverage.jsonl`). Set by `rupu run` — the
    /// command every host connector launches — and shared with its
    /// `dispatch_agent` children; `None` for in-process workflow steps and
    /// sessions, which already write the coordinator's ledgers directly.
    #[serde(skip)]
    pub coverage_stream: Option<std::path::PathBuf>,
```

Add `coverage_stream: None,` to the manual `impl Default` (`tool.rs:108`).

Then add `coverage_stream: None,` to every exhaustive `ToolContext { … }` literal:
- `crates/rupu-cli/src/cmd/session.rs:7483`
- `crates/rupu-orchestrator/src/step_factory.rs:432`
- `crates/rupu-orchestrator/tests/dispatch_agent.rs:168`, `:339`
- `crates/rupu-orchestrator/tests/dispatch_agents_parallel.rs:183`

`run.rs` and `dispatch.rs` are handled in Steps 5 and 6. Find any remaining ones with `cargo check --workspace --tests`.

- [ ] **Step 4: Stream from the agent runner.** In `crates/rupu-agent/src/runner.rs`, add this private helper near `CoverageBundle` (`:34`):

```rust
/// The run stream for records written under `scope`, when `rupu run` asked
/// for one (see `ToolContext::coverage_stream`).
fn run_stream_for(ctx: &ToolContext, scope: &str) -> Option<rupu_coverage::RunStream> {
    ctx.coverage_stream.clone().map(|path| rupu_coverage::RunStream {
        path,
        scope_name: scope.to_string(),
    })
}
```

Replace lines 861-866 (the `let paths = …` through the `write_snapshot` call) with:

```rust
            let paths = CoveragePaths::new(&opts.workspace_path, &target)
                .with_run_stream(run_stream_for(&opts.tool_context, resolved_scope));
            paths
                .ensure_dir()
                .map_err(|e| RunError::Coverage(format!("ensure coverage dir: {e}")))?;
            write_snapshot(&catalog, &paths.catalog)
                .map_err(|e| RunError::Coverage(format!("write catalog snapshot: {e}")))?;
            rupu_coverage::stream_catalog(&paths, &catalog)
                .map_err(|e| RunError::Coverage(format!("stream catalog snapshot: {e}")))?;
```

Replace line 997 (`let paths = CoveragePaths::new(&opts.workspace_path, &target);` inside the report-finding-only block) with:

```rust
        let paths = CoveragePaths::new(&opts.workspace_path, &target)
            .with_run_stream(run_stream_for(&opts.tool_context, scope));
```

If `ToolContext` isn't already imported in `runner.rs`, spell it `rupu_tools::ToolContext` in the helper's signature.

- [ ] **Step 5: `rupu run` starts the stream.** In `crates/rupu-cli/src/cmd/run.rs`, add this right after the transcript path is computed (`:570`):

```rust
    // Every `rupu run` streams its coverage to a run-scoped file so a
    // coordinator can collect a placed unit's findings (spec
    // 2026-09-30-rupu-remote-findings-transport-design.md §A1). The begin line
    // goes first, even if the run records nothing: its absence is how the
    // coordinator tells "this host can't stream" from "nothing recorded".
    let coverage_stream = rupu_coverage::stream_path(&global.join("runs"), &run_id);
    if let Err(e) = rupu_coverage::write_stream_begin(&coverage_stream, &run_id) {
        warn!(
            error = %e,
            path = %coverage_stream.display(),
            "could not start this run's coverage stream; a coordinator will report its coverage as not collected"
        );
    }
```

In the `ToolContext { … }` literal (`:827`), add `coverage_stream: Some(coverage_stream.clone()),`. In the `CliAgentDispatcher::new(…)` call (`:808`), append `Some(coverage_stream.clone())` as the last argument.

`RunStore::create` only refuses when `run.json` exists (`crates/rupu-orchestrator/src/runs.rs:906`). So creating `runs/<run_id>/` early doesn't clash with the `run.json` write at the end of the run.

- [ ] **Step 6: `dispatch_agent` children share the stream.** In `crates/rupu-cli/src/cmd/dispatch.rs`:
- Add a field to `CliAgentDispatcher`: `coverage_stream: Option<PathBuf>,` with the doc `/// The parent \`rupu run\`'s coverage stream; children append to it.`
- Add `coverage_stream: Option<PathBuf>,` as the last parameter of `new` (after `findings_base`), and set it in the `Self { … }` literal.
- In `child_tool_ctx` (`:311`), add `coverage_stream: self.coverage_stream.clone(),`.
- Pass `None` as the new last argument at the 7 test call sites in `dispatch.rs` (`:638, :737, :805, :883, :968, :1055, :1133`), at `crates/rupu-cli/src/cmd/workflow.rs:3214`, and at `crates/rupu-cli/src/resume.rs:298`.

- [ ] **Step 7: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-agent --test findings_without_coverage && cargo test -p rupu-tools && cargo check --workspace --tests`
Expected: PASS, and the workspace compiles.

- [ ] **Step 8: Commit.**

```bash
git add crates/rupu-tools crates/rupu-agent crates/rupu-cli crates/rupu-orchestrator
git commit -m "feat(run): rupu run streams its coverage to runs/<id>/coverage.jsonl; dispatch children share it"
```

---

### Task 4: Protocol and mirror for the stream (`rupu-cp`)

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs` (`Frame::Welcome`; `ArtifactFile`; constants near `:76`)
- Modify: `crates/rupu-cp/src/node/mirror.rs` (`append` match at `:180`; new `coverage_path` and `replace_coverage`)
- Modify: `crates/rupu-cp/src/node/server.rs:175` (send `Welcome` with capabilities)
- Modify: `crates/rupu-cli/src/cmd/node.rs:427-438` (read the `Welcome` capabilities) and `:596` (the exhaustive `match frame` arm)
- Modify: `crates/rupu-cp/tests/node_tunnel.rs`, the six `matches!(…, Frame::Welcome {})` at `:857, :904, :959, :1783, :2125, :2386`

**Interfaces:**
- Produces:
  - `ArtifactFile::Coverage`
  - `Frame::Welcome { capabilities: Vec<String> }` (`#[serde(default)]`)
  - `pub const CAP_MIRROR_COVERAGE: &str = "mirror.coverage"`
  - `pub const CAP_RUN_COVERAGE_STREAM: &str = "run.coverage_stream"`
  - `pub fn cp_capabilities() -> Vec<String>`
  - `NodeMirror::coverage_path(&self, run_id) -> PathBuf`
  - `NodeMirror::replace_coverage(&self, run_id, node_id, body: &str) -> Result<(), MirrorError>`
  - `connect_and_run` gains a `mirror_coverage: bool` local, used by Task 8.

- [ ] **Step 1: Write the failing tests.** Add these to `crates/rupu-cp/src/node/protocol.rs`'s test module:

```rust
    #[test]
    fn welcome_carries_cp_capabilities_and_old_welcomes_still_parse() {
        let w = Frame::Welcome {
            capabilities: cp_capabilities(),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(CAP_MIRROR_COVERAGE), "{json}");
        // An older CP sends `{"type":"welcome"}`.
        let old: Frame = serde_json::from_str(r#"{"type":"welcome"}"#).unwrap();
        assert_eq!(
            old,
            Frame::Welcome {
                capabilities: vec![]
            }
        );
    }

    #[test]
    fn coverage_artifact_frame_round_trips() {
        let f = Frame::Artifact {
            run_id: "run_1".into(),
            file: ArtifactFile::Coverage,
            line: r#"{"ledger":"begin","v":1,"run_id":"run_1"}"#.into(),
        };
        let back: Frame = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
```

Add these to the test module of `crates/rupu-cp/src/node/mirror.rs`. It has tests using a `NodeMirror` over a temp `RunStore`; reuse whatever helper it has for creating a mirrored run owned by a node. If there's none, build one as the existing tests do, with `NodeMirror::new(Arc::new(RunStore::new(tmp.join("runs"))))` plus `create_run(run_id, node_id, &spec)`.

```rust
    #[test]
    fn coverage_lines_append_and_replace_is_authoritative() {
        let tmp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(rupu_orchestrator::runs::RunStore::new(tmp.path().join("runs")));
        let mirror = NodeMirror::new(std::sync::Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Agent,
            name: "a".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
        };
        mirror.create_run("run_C1", "node-1", &spec).unwrap();
        mirror
            .append("run_C1", "node-1", ArtifactFile::Coverage, "line-1")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path("run_C1")).unwrap(),
            "line-1\n"
        );
        mirror
            .replace_coverage("run_C1", "node-1", "a\nb\n")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(mirror.coverage_path("run_C1")).unwrap(),
            "a\nb\n"
        );
        assert!(matches!(
            mirror.replace_coverage("run_C1", "node-2", "x"),
            Err(MirrorError::WrongNode(_))
        ));
    }
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --lib node::`
Expected: compile errors, because `ArtifactFile::Coverage`, `cp_capabilities` and the other items don't exist yet.

- [ ] **Step 3: Update the protocol.** In `protocol.rs`:

```rust
    /// CP→node after a valid `Hello`. `capabilities` lists what this CP
    /// accepts from a node (e.g. [`CAP_MIRROR_COVERAGE`]); an older CP sends
    /// none, so the field defaults empty and the node sends only what every
    /// CP understands.
    Welcome {
        #[serde(default)]
        capabilities: Vec<String>,
    },
```

Add a variant to `ArtifactFile`, after `Transcript`:

```rust
    /// The run's coverage stream (`runs/<run_id>/coverage.jsonl`). Sent only
    /// to a CP that advertised [`CAP_MIRROR_COVERAGE`] — an older CP fails to
    /// parse an unknown variant.
    Coverage,
```

Add the constants below `CAP_AGENT_FINDINGS_PROFILE`:

```rust
/// `Welcome.capabilities` entry: this CP mirrors [`ArtifactFile::Coverage`].
pub const CAP_MIRROR_COVERAGE: &str = "mirror.coverage";

/// HTTP `/api/host/info` `features` entry: this CP serves
/// `GET /api/runs/:id/coverage`.
pub const CAP_RUN_COVERAGE_STREAM: &str = "run.coverage_stream";

/// What this CP advertises to a node in `Welcome`.
pub fn cp_capabilities() -> Vec<String> {
    vec![CAP_MIRROR_COVERAGE.to_string()]
}
```

Update the existing `protocol.rs` tests that build `Frame::Welcome {}` to `Frame::Welcome { capabilities: vec![] }`.

- [ ] **Step 4: Update the mirror.** In `mirror.rs`, add an arm to the exhaustive `match file` in `append`:

```rust
            ArtifactFile::Coverage => {
                let path = self.coverage_path(run_id);
                let mut f = OpenOptions::new().create(true).append(true).open(path)?;
                writeln!(f, "{line}")?;
            }
```

Add these methods to `impl NodeMirror`:

```rust
    /// `<global>/runs/<run_id>/coverage.jsonl` — the mirrored run stream.
    pub fn coverage_path(&self, run_id: &str) -> PathBuf {
        rupu_coverage::stream_path(&self.run_store.root, run_id)
    }

    /// Replace the mirrored stream with `body` — the host's complete file,
    /// read once the run is terminal (the tail may still have been behind).
    /// Same ownership check as [`Self::append`].
    pub fn replace_coverage(
        &self,
        run_id: &str,
        node_id: &str,
        body: &str,
    ) -> Result<(), MirrorError> {
        validate_run_id(run_id)?;
        let existing = self.run_store.load(run_id)?;
        if existing.worker_id.as_deref() != Some(node_id) {
            return Err(MirrorError::WrongNode(run_id.to_string()));
        }
        let path = self.coverage_path(run_id);
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
```

- [ ] **Step 5: The CP advertises, and the node reads.** In `server.rs:175`, change `&Frame::Welcome {}` to `&Frame::Welcome { capabilities: crate::node::protocol::cp_capabilities() }`.

In `crates/rupu-cli/src/cmd/node.rs`, replace the Welcome check (`:433-438`) with:

```rust
    let welcome_frame = parse_frame(&welcome_msg)?;
    let cp_capabilities = match welcome_frame {
        Frame::Welcome { capabilities } => capabilities,
        other => anyhow::bail!(
            "expected Welcome from server, got: {}",
            serde_json::to_string(&other).unwrap_or_else(|_| "?".into())
        ),
    };
    // Only a CP that advertised it can parse `ArtifactFile::Coverage` frames.
    let mirror_coverage = cp_capabilities
        .iter()
        .any(|c| c == rupu_cp::node::protocol::CAP_MIRROR_COVERAGE);
```

Task 8 uses `mirror_coverage`. Until then, bind it as `let _mirror_coverage = …` so the build stays warning-free, and rename it in Task 8.

In the exhaustive `match frame` (`:596`), change `| Frame::Welcome {}` to `| Frame::Welcome { .. }`.

In `crates/rupu-cp/tests/node_tunnel.rs`, change the six `matches!(…, Frame::Welcome {})` to `matches!(…, Frame::Welcome { .. })`.

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --lib node:: && cargo test -p rupu-cp --test node_tunnel && cargo test -p rupu-cli --lib cmd::node`
Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-cp crates/rupu-cli/src/cmd/node.rs
git commit -m "feat(cp): ArtifactFile::Coverage, Welcome capabilities (mirror.coverage), mirror replace_coverage"
```

---

### Task 5: `HostConnector::unit_coverage` on local, SSH, tunnel and bucket

**Files:**
- Modify: `crates/rupu-cp/src/host/connector.rs` (the trait; a new `mirror_unit_coverage` helper; test doubles `StubConnector` at `:938` and `Bare` at `:1034`)
- Modify: `crates/rupu-cp/src/host/{local,ssh,tunnel}.rs`, `crates/rupu-cp/src/host/bucket/connector.rs`
- Modify: the test doubles `crates/rupu-cp/src/api/graph.rs:620`, `api/usage.rs:1032`, `api/runs.rs:2654, :2796`, `crates/rupu-cp/tests/host_registry.rs:31`, and `crates/rupu-cli/src/fleet_unit_dispatcher.rs:764, :885, :1211, :1525, :1655`
- HTTP is Task 6. It gets a temporary `Err(Unsupported)` body here, which Task 6 replaces.

**Interfaces:**
- Produces:
  - `async fn unit_coverage(&self, run_id: &str) -> Result<Vec<u8>, HostConnectorError>`, a required trait method with no default. An empty vector means no stream arrived.
  - `pub fn mirror_unit_coverage(run_store: &RunStore, run_id: &str) -> Result<Vec<u8>, HostConnectorError>`

- [ ] **Step 1: Write the failing tests.** Add these to `crates/rupu-cp/src/host/connector.rs`'s test module:

```rust
    #[test]
    fn mirror_unit_coverage_reads_the_run_stream_or_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        assert_eq!(mirror_unit_coverage(&store, "run_X1").unwrap(), Vec::<u8>::new());
        let p = rupu_coverage::stream_path(&store.root, "run_X1");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"line\n").unwrap();
        assert_eq!(mirror_unit_coverage(&store, "run_X1").unwrap(), b"line\n");
        assert!(matches!(
            mirror_unit_coverage(&store, "../etc"),
            Err(HostConnectorError::Invalid(_))
        ));
    }
```

Add this to `crates/rupu-cp/src/host/bucket/connector.rs`'s tests. It uses `make_conn` and the mirror, and follows the pattern of `launch_agent_puts_agent_kind_job`:

```rust
    #[tokio::test]
    async fn unit_coverage_reads_the_mirrored_stream() {
        let (conn, run_store, _bucket, _tmp) = make_conn();
        let run_id = conn.launch_agent(profile_req(None)).await.unwrap();
        let p = rupu_coverage::stream_path(&run_store.root, &run_id);
        std::fs::write(&p, b"{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"x\"}\n").unwrap();
        assert!(conn.unit_coverage(&run_id).await.unwrap().starts_with(b"{\"ledger\":\"begin\""));
    }
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --lib host::`
Expected: compile errors, because `mirror_unit_coverage` and `unit_coverage` don't exist yet.

- [ ] **Step 3: Add the trait method and the helper.** In `connector.rs`, inside `pub trait HostConnector`, add this after `pull_transcript`:

```rust
    /// The coverage stream (`runs/<run_id>/coverage.jsonl`) the executing
    /// host wrote for `run_id`, for the coordinator to merge (spec
    /// 2026-09-30-rupu-remote-findings-transport-design.md §A2). `Ok(empty)`
    /// ⇒ no stream arrived. Deliberately no default: every transport must say
    /// how it delivers this, or refuse.
    async fn unit_coverage(&self, run_id: &str) -> Result<Vec<u8>, HostConnectorError>;
```

Add this as a free function, next to the other `mirror_*` helpers:

```rust
/// A run's coverage stream read from the coordinator's own run store: the
/// local host's file, or a mirror-backed transport's mirrored copy.
pub fn mirror_unit_coverage(
    run_store: &RunStore,
    run_id: &str,
) -> Result<Vec<u8>, HostConnectorError> {
    let valid = run_id.starts_with("run_")
        && run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(HostConnectorError::Invalid(format!(
            "{run_id:?} is not a valid run id"
        )));
    }
    match std::fs::read(rupu_coverage::stream_path(&run_store.root, run_id)) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(HostConnectorError::Invalid(format!(
            "read coverage stream for {run_id}: {e}"
        ))),
    }
}
```

- [ ] **Step 4: Implement it for the five production connectors.**
- `local.rs`, `ssh.rs`, `tunnel.rs` and `bucket/connector.rs`: each owns `run_store: Arc<RunStore>`. Add:

```rust
    async fn unit_coverage(&self, run_id: &str) -> Result<Vec<u8>, HostConnectorError> {
        crate::host::connector::mirror_unit_coverage(&self.run_store, run_id)
    }
```

- `http.rs`, temporary until Task 6:

```rust
    async fn unit_coverage(&self, _run_id: &str) -> Result<Vec<u8>, HostConnectorError> {
        Err(HostConnectorError::Unsupported(
            "coverage collection over HTTP is not implemented yet".into(),
        ))
    }
```

- [ ] **Step 5: Implement it for every test double.** Add this to each of the 11 test impls listed under **Files**:

```rust
        async fn unit_coverage(&self, _run_id: &str) -> Result<Vec<u8>, HostConnectorError> {
            Ok(Vec::new())
        }
```

Match the double's indentation, and use whatever path the file already uses for `HostConnectorError`. Task 11 makes `FakeConnector`'s answer configurable.

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --lib host:: && cargo check --workspace --tests`
Expected: PASS, and the workspace compiles.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-cp crates/rupu-cli/src/fleet_unit_dispatcher.rs
git commit -m "feat(cp): HostConnector::unit_coverage (required) — local/ssh/tunnel/bucket read the run stream"
```

---

### Task 6: HTTP hosts serve and fetch the stream; unmatched `/api/*` returns a JSON 404

**Files:**
- Modify: `crates/rupu-cp/src/api/runs.rs:21-37` (route) plus a new handler
- Modify: `crates/rupu-cp/src/api/host_info.rs` (`host_features`)
- Modify: `crates/rupu-cp/src/host/http.rs` (`unit_coverage`)
- Modify: `crates/rupu-cp/src/embed.rs:65-77`
- Regenerate: `apps/rupu-macos/Fixtures/host_info.json`
- Test: `crates/rupu-cp/tests/host_http.rs`

**Interfaces:**
- Consumes: `mirror_unit_coverage` (Task 5), `CAP_RUN_COVERAGE_STREAM` (Task 4), and `HttpHostConnector::require_feature` (from #675).
- Produces:
  - `GET /api/runs/:id/coverage`: 200 with `application/x-ndjson`, and an empty body when the run wrote none; 400 for a malformed id.
  - `features` gains `run.coverage_stream`.

- [ ] **Step 1: Write the failing tests.** Add these to `crates/rupu-cp/tests/host_http.rs`:

```rust
/// A real remote CP serves a run's stream; the connector fetches it.
#[tokio::test]
async fn unit_coverage_fetches_the_remote_runs_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let state = rupu_cp::state::AppState::new(
        tmp.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let p = rupu_coverage::stream_path(&state.run_store.root, "run_H1");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"run_H1\"}\n").unwrap();
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let bytes = c.unit_coverage("run_H1").await.unwrap();
    assert!(bytes.starts_with(b"{\"ledger\":\"begin\""));
    assert!(c.unit_coverage("run_NONE").await.unwrap().is_empty());
}

/// An older remote answers unknown /api paths with the SPA (200 HTML), so the
/// connector must check the feature first rather than trust the status.
#[tokio::test]
async fn unit_coverage_refuses_a_remote_without_the_feature() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(serde_json::json!({"version": "0.70.0", "features": []}));
    });
    let spa = server.mock(|when, then| {
        when.method("GET").path("/api/runs/run_H1/coverage");
        then.status(200).body("<!doctype html>");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.unit_coverage("run_H1").await.unwrap_err();
    assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err:?}");
    spa.assert_hits(0);
}

#[tokio::test]
async fn unmatched_api_paths_are_a_json_404_not_the_spa() {
    let tmp = tempfile::tempdir().unwrap();
    let state = rupu_cp::state::AppState::new(
        tmp.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    #[allow(clippy::disallowed_methods)]
    let resp = reqwest::get(format!("http://{addr}/api/definitely/not/a/route"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("/api/definitely"));
}
```

If `host_http.rs` lacks an `#![allow(clippy::disallowed_methods)]` for test-only reqwest use, keep the per-statement `#[allow]` shown above. `crates/rupu-cp/tests/host_launch_control.rs:12` does the same at file level.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --test host_http unit_coverage unmatched_api`
Expected: FAIL. The HTTP `unit_coverage` is still the Task 5 stub, and `/api/definitely…` returns the SPA.

- [ ] **Step 3: Add the remote endpoint.** In `api/runs.rs` `routes()`, add `.route("/api/runs/:id/coverage", get(get_run_coverage))`. Then add the handler:

```rust
/// `GET /api/runs/:id/coverage` — the run's coverage stream as this host
/// wrote it, for a coordinator merging a placed unit (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §A2). 200 with an
/// empty body when the run wrote none.
async fn get_run_coverage(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let bytes = crate::host::connector::mirror_unit_coverage(&s.run_store, &id).map_err(
        |e| match e {
            crate::host::connector::HostConnectorError::Invalid(m) => ApiError::bad_request(m),
            other => ApiError::internal(other.to_string()),
        },
    )?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
        bytes,
    )
        .into_response())
}
```

Use `runs.rs`'s existing imports for `State`, `Path`, `ApiError` and `ApiResult`, and add any that are missing.

- [ ] **Step 4: Advertise the feature and fetch it.** In `api/host_info.rs`, change `host_features()` to:

```rust
fn host_features() -> Vec<String> {
    vec![
        crate::node::protocol::CAP_AGENT_FINDINGS_PROFILE.to_string(),
        crate::node::protocol::CAP_RUN_COVERAGE_STREAM.to_string(),
    ]
}
```

In `host/http.rs`, replace the Task 5 stub with:

```rust
    async fn unit_coverage(&self, run_id: &str) -> Result<Vec<u8>, HostConnectorError> {
        let valid = run_id.starts_with("run_")
            && run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(HostConnectorError::Invalid(format!(
                "{run_id:?} is not a valid run id"
            )));
        }
        // An older remote answers an unknown /api path with the SPA and 200,
        // so the feature — not the status — says whether this is a stream.
        self.require_feature(
            crate::node::protocol::CAP_RUN_COVERAGE_STREAM,
            "this unit's coverage cannot be collected",
        )
        .await?;
        let resp = self
            .send(
                self.client
                    .get(self.url(&format!("/api/runs/{run_id}/coverage"))),
            )
            .await?;
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| HostConnectorError::Unreachable(e.to_string()))?;
        Ok(bytes.to_vec())
    }
```

- [ ] **Step 5: Return a JSON 404 for unmatched `/api/*`.** In `embed.rs` `static_handler`, add this right after the `let path = …` line:

```rust
    // An unmatched API path is a client error, not a page: serving the SPA
    // with 200 made every newer endpoint look present on an older CP.
    if path == "api" || path.starts_with("api/") {
        return crate::error::ApiError::not_found(format!("no API route for /{path}"))
            .into_response();
    }
```

Add `use axum::response::IntoResponse;` to `embed.rs` if it's not already imported.

- [ ] **Step 6: Regenerate the fixture.**

Run: `REGEN_FIXTURES=1 cargo test -p rupu-cp fixture_is_current`
Then run `git diff apps/rupu-macos/Fixtures/host_info.json`. The only change should be `"run.coverage_stream"` added to `features`.

- [ ] **Step 7: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp`
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git add crates/rupu-cp apps/rupu-macos/Fixtures/host_info.json
git commit -m "feat(cp): GET /api/runs/:id/coverage + run.coverage_stream feature; HTTP unit_coverage; JSON 404 for unmatched /api"
```

---

### Task 7: The SSH tail pump mirrors the stream and catches up when the run finishes

**Files:**
- Modify: `crates/rupu-cp/src/host/ssh.rs`:
  - the tail command (`:1369-1383`)
  - the marker routing (`:1434-1475`)
  - `pump_finalize_if_terminal` (`:940-977`) and the fallback path (`:1588-1635`)
  - a new `pump_catch_up_coverage`
  - `FakeExec` (`:3081`): add a field and route it in `run()` at `:3227-3246`

**Interfaces:**
- Consumes: `ArtifactFile::Coverage` and `NodeMirror::replace_coverage` (Task 4).
- Produces: after an SSH unit is terminal and `await_run_mirror` returns, the mirror's `runs/<id>/coverage.jsonl` equals the host's file.

- [ ] **Step 1: Write the failing test.** Add this to `ssh.rs`'s tests, modelled on `tail_pump_routes_events_and_finishes_run` (`:5170`):

```rust
    #[test]
    fn tail_pump_mirrors_the_coverage_stream_and_replaces_it_at_terminal() {
        let run_id = "run_01COVPUMP";
        let mut fake = FakeExec::with_cat_stdout(
            vec![
                format!("==> $HOME/.rupu/runs/{run_id}/coverage.jsonl <=="),
                r#"{"ledger":"begin","v":1,"run_id":"run_01COVPUMP"}"#.to_string(),
            ],
            r#"{"status":"completed","final_output":"done."}"#,
        );
        fake.cat_coverage_stdout = Some(
            "{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"run_01COVPUMP\"}\n{\"late\":true}\n"
                .to_string(),
        );
        let fake = std::sync::Arc::new(fake);
        let (conn, _store, _tmp) = make_conn(std::sync::Arc::clone(&fake));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            conn.launch_agent(crate::agent_launcher::AgentLaunchRequest {
                agent: "sec".into(),
                prompt: None,
                mode: None,
                target: None,
                working_dir: None,
                run_id: Some(run_id.into()),
                findings_profile: None,
            })
            .await
            .unwrap();
            conn.await_run_mirror(run_id).await;
            let body = conn.unit_coverage(run_id).await.unwrap();
            assert_eq!(
                String::from_utf8(body).unwrap(),
                "{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"run_01COVPUMP\"}\n{\"late\":true}\n",
                "terminal catch-up must replace the tailed copy with the host's file"
            );
        });
        let cmds = fake.commands.lock().unwrap();
        assert!(cmds.iter().any(|c| c.starts_with("tail ")
            && c.contains(&format!("$HOME/.rupu/runs/{run_id}/coverage.jsonl"))));
    }
```

If the existing pump tests use a different way to drive the pump to terminal (for example, which `FakeExec` constructor makes `cat run.json` return a terminal status), follow `tail_pump_routes_events_and_finishes_run` exactly. The assertions above are what matter.

- [ ] **Step 2: Run the test to confirm it fails.**

Run: `cargo test -p rupu-cp --lib host::ssh::tests::tail_pump_mirrors_the_coverage`
Expected: compile error, because `FakeExec` has no `cat_coverage_stdout`.

- [ ] **Step 3: Teach `FakeExec` the coverage `cat`.**
- Add `cat_coverage_stdout: Option<String>,` to the struct.
- Add `cat_coverage_stdout: None,` to all five constructors (`ok`, `offline`, `with_cat_stdout`, `with_bytes_ok`, `with_bytes_err`).
- In `run()`, before the generic `"cat "` arm, add an arm matching `cmd.starts_with("cat ") && cmd.contains("/coverage.jsonl")`. When `Some(s)`, it returns `RemoteOutput { stdout: s.clone(), stderr: String::new(), success: true }`. When `None`, it returns `RemoteOutput { stdout: String::new(), stderr: "No such file".into(), success: false }`.

- [ ] **Step 4: Tail and route the file.** In the tail command (`:1369`), add `$HOME/.rupu/runs/{run_id}/coverage.jsonl \` after the `unit_checkpoints.jsonl` line. In the marker routing, add this before the `transcript_suffix` branch:

```rust
                                            } else if path.ends_with("coverage.jsonl") {
                                                Some(ArtifactFile::Coverage)
```

- [ ] **Step 5: Catch up when the run finishes.** Add this next to `pump_catch_up_transcript` (`:783`), with the same `exec` and `mirror` parameter types that function uses:

```rust
/// The tail can still be behind when the run turns terminal. Replace the
/// mirrored stream with the host's file, which is complete by then. An older
/// host has no file: keep whatever the tail delivered (nothing), and the
/// coordinator reports that unit's coverage as not collected.
async fn pump_catch_up_coverage(
    exec: &Arc<dyn RemoteExec>,
    mirror: &NodeMirror,
    run_id: &str,
    host_id: &str,
) {
    let cmd = format!("cat $HOME/.rupu/runs/{run_id}/coverage.jsonl");
    if let Ok(out) = exec.run(&cmd).await {
        if out.success {
            let _ = mirror.replace_coverage(run_id, host_id, &out.stdout);
        }
    }
}
```

If `pump_catch_up_transcript` takes `exec` as some type other than `&Arc<dyn RemoteExec>`, use that same type. Call it right after `pump_catch_up_transcript(…)` in both places: `pump_finalize_if_terminal` and the stream-ended fallback path.

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --lib host::ssh`
Expected: PASS. That includes the existing pump tests, whose tail assertions check `contains`, so the extra file doesn't break them.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-cp/src/host/ssh.rs
git commit -m "feat(ssh): tail pump mirrors runs/<id>/coverage.jsonl and replaces it at terminal"
```

---

### Task 8: The tunnel node and bucket worker ship the stream

**Files:**
- Modify: `crates/rupu-cli/src/cmd/node.rs`:
  - `FileOffsets` / `BucketRunState` (`:135-152`) and their literals at `:517-521` and `:944-948`
  - tunnel drain (`:462-501`), cancel (`:548-559`)
  - bucket drain (`:964-1020`) and terminal (`:1108-1115`)
- Modify: `crates/rupu-cp/src/host/bucket/poller.rs:109-125` (`classify_key`) and its tests

**Interfaces:**
- Consumes: `mirror_coverage: bool` (Task 4), `ArtifactFile::Coverage`, `rupu_coverage::STREAM_FILE`.
- Produces:
  - Tunnel: `Frame::Artifact { file: Coverage }` lines, sent only if `mirror_coverage`, with a final drain before `RunJson` + `RunFinished` and before a cancel's `RunFinished`.
  - Bucket: `coverage.NNNN.jsonl` result objects, with a final drain before `put_finished`.

- [ ] **Step 1: Write the failing tests.**

In `crates/rupu-cp/src/host/bucket/poller.rs` tests (`classify_key_maps_known_suffixes`), add:

```rust
        assert!(matches!(
            classify_key("coverage.0001.jsonl"),
            Some(ArtifactFile::Coverage)
        ));
```

In `crates/rupu-cli/src/cmd/node.rs` tests, add a unit test for the new drain helper:

```rust
    #[test]
    fn drain_coverage_reads_the_run_stream_incrementally() {
        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path();
        std::fs::write(run_dir.join(rupu_coverage::STREAM_FILE), "a\nb\n").unwrap();
        let mut off = 0u64;
        assert_eq!(drain_coverage(run_dir, &mut off), vec!["a", "b"]);
        std::fs::write(run_dir.join(rupu_coverage::STREAM_FILE), "a\nb\nc\n").unwrap();
        assert_eq!(drain_coverage(run_dir, &mut off), vec!["c"]);
    }
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cp --lib host::bucket::poller && cargo test -p rupu-cli --lib cmd::node::tests::drain_coverage`
Expected: FAIL. `classify_key` returns `None`, and `drain_coverage` doesn't exist.

- [ ] **Step 3: Poller.** In `classify_key`, inside the `.jsonl` branch, add:

```rust
        if key.starts_with("coverage") {
            return Some(ArtifactFile::Coverage);
        }
```

- [ ] **Step 4: Node offsets and helper.** In `node.rs`:
- Add `coverage: u64,` to `FileOffsets`, and `coverage: 0,` to both literals (`:517-521` and `:944-948`).
- Add `coverage_seq: u64,` to `BucketRunState`, and `coverage_seq: 0,` to its literal.
- Add the helper next to `drain_new_lines`:

```rust
/// New lines of a run's coverage stream (`runs/<id>/coverage.jsonl`).
fn drain_coverage(run_dir: &Path, offset: &mut u64) -> Vec<String> {
    drain_new_lines(&run_dir.join(rupu_coverage::STREAM_FILE), offset)
}
```

- [ ] **Step 5: The tunnel drains the stream, with a final drain.** In `connect_and_run`, rename Task 4's `_mirror_coverage` to `mirror_coverage`. In the per-run drain loop, add this after the `unit_checkpoints.jsonl` drain:

```rust
            // coverage.jsonl — only a CP that advertised it parses these frames.
            if mirror_coverage {
                for line in drain_coverage(&run_dir, &mut state.offsets.coverage) {
                    send_artifact(&mut sink, rid, ArtifactFile::Coverage, line).await;
                }
            }
```

In the terminal block, add a final drain as the first statement inside `if let Some((status, body)) = read_terminal_status(…) {`. The run may have written its last lines after the drain above and before `run.json` turned terminal.

```rust
                if mirror_coverage {
                    for line in drain_coverage(&run_dir, &mut state.offsets.coverage) {
                        send_artifact(&mut sink, rid, ArtifactFile::Coverage, line).await;
                    }
                }
```

In the `Frame::Cancel` arm, add this before sending the cancelled `RunFinished`. The run's directory is `runs_root.join(&run_id)`.

```rust
                    if mirror_coverage {
                        let run_dir = runs_root.join(&run_id);
                        for line in drain_coverage(&run_dir, &mut state.offsets.coverage) {
                            send_artifact(&mut sink, &run_id, ArtifactFile::Coverage, line).await;
                        }
                    }
```

- [ ] **Step 6: The bucket worker uploads the stream, with a final drain.** Add this helper next to `result_key`:

```rust
/// Upload `lines` as one `<kind>.<seq>.jsonl` result object, advancing `seq`
/// only when the upload landed.
async fn put_lines(
    bucket: &ObjectStoreBucket,
    rid: &str,
    kind: &str,
    lines: Vec<String>,
    seq: &mut u64,
) {
    if lines.is_empty() {
        return;
    }
    let body = lines.join("\n") + "\n";
    let key = result_key(kind, *seq);
    match bucket.put_result(rid, &key, body.as_bytes()).await {
        Ok(()) => *seq += 1,
        Err(e) => warn!(run_id = %rid, key = %key, error = %e, "node pull: put {kind} result failed"),
    }
}
```

In `pull()`'s drain, after the `unit_checkpoints` block, add:

```rust
            // coverage.jsonl — an older CP's poller skips unknown keys.
            let lines = drain_coverage(&run_dir, &mut state.offsets.coverage);
            put_lines(&bucket, rid, "coverage", lines, &mut state.coverage_seq).await;
```

In the terminal block, add this before `bucket.put_finished(rid, &status)`:

```rust
                let lines = drain_coverage(&run_dir, &mut state.offsets.coverage);
                put_lines(&bucket, rid, "coverage", lines, &mut state.coverage_seq).await;
```

If `bucket` is borrowed differently in `pull()` (for example an `Arc`), adjust the `put_lines` parameter type to match.

- [ ] **Step 7: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cp --lib host::bucket && cargo test -p rupu-cli --lib cmd::node && cargo test -p rupu-cp --test bucket_e2e`
Expected: PASS.

- [ ] **Step 8: Commit.**

```bash
git add crates/rupu-cli/src/cmd/node.rs crates/rupu-cp/src/host/bucket/poller.rs
git commit -m "feat(node): tunnel node and bucket worker ship the coverage stream, with a final drain at finish/cancel"
```

---

### Task 9: `UnitCoverage`, `UnitFailure` and `UnitOutcome.coverage` (types only, no behaviour change)

**Files:**
- Modify: `crates/rupu-orchestrator/src/runner.rs`:
  - new types near `UnitOutcome` (`:162`)
  - the trait (`:183`)
  - `dispatch_placed_step`'s match (`:6085-6111`)
  - the fan-out `Err(first_err)` / `Err(second_err)` arms (`:6848-6984`)
  - the test impls at `:8518, :8707, :9175, :9319, :9969, :15924, :15952`, and the `UnitOutcome` literals at `:8531, :8717, :9181, :9341, :9983, :15932, :15959`
- Modify: `crates/rupu-orchestrator/src/lib.rs`, if the runner items are re-exported there.
- Modify: `crates/rupu-orchestrator/tests/{distributed_fanout_e2e,remote_findings_profile,pause_resume_e2e,placed_step_e2e,workspace_sync_e2e}.rs`
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs` (`dispatch_unit` signature, its `Err` returns, and the `UnitOutcome` literal at `:459`)

**Interfaces:**
- Produces:
  - `pub enum UnitCoverage { Stream(Vec<u8>), Unavailable(String), NotLaunched }`
  - `pub struct UnitFailure { pub error: RunError, pub coverage: UnitCoverage }`, with `impl From<RunError> for UnitFailure` and `impl Display`
  - `UnitOutcome.coverage: UnitCoverage`
  - `UnitDispatcher::dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, UnitFailure>`

- [ ] **Step 1: Write the failing test.** Add this to runner.rs's test module:

```rust
    #[test]
    fn a_plain_run_error_is_a_failure_that_never_launched() {
        let f: UnitFailure = RunError::Provider("boom".into()).into();
        assert_eq!(f.coverage, UnitCoverage::NotLaunched);
        assert_eq!(f.to_string(), RunError::Provider("boom".into()).to_string());
    }
```

- [ ] **Step 2: Run the test to confirm it fails.**

Run: `cargo test -p rupu-orchestrator --lib a_plain_run_error_is_a_failure`
Expected: compile error, because `UnitFailure` doesn't exist.

- [ ] **Step 3: Add the types.** In `runner.rs`, after `UnitOutcome`:

```rust
/// What a remote unit recorded (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §A4), carried on every
/// post-launch outcome — success or failure — so the runner can merge it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitCoverage {
    /// The unit's coverage stream as its host connector delivered it.
    Stream(Vec<u8>),
    /// The connector could not deliver a stream; the reason is surfaced as a
    /// `StepWarning`.
    Unavailable(String),
    /// The unit never launched: nothing to collect.
    NotLaunched,
}

/// A remote unit that failed, with whatever coverage it recorded. `Err` keeps
/// its meaning for the fan-out's fallback-host retry.
#[derive(Debug)]
pub struct UnitFailure {
    pub error: RunError,
    pub coverage: UnitCoverage,
}

impl From<RunError> for UnitFailure {
    fn from(error: RunError) -> Self {
        Self {
            error,
            coverage: UnitCoverage::NotLaunched,
        }
    }
}

impl std::fmt::Display for UnitFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}
```

Add a field to `UnitOutcome`:

```rust
    /// What the unit recorded; see [`UnitCoverage`].
    pub coverage: UnitCoverage,
```

Change the trait method to `async fn dispatch_unit(&self, unit: UnitDispatch, host: &str) -> Result<UnitOutcome, UnitFailure>;`.

If `lib.rs` re-exports runner items by name (`grep -n "UnitOutcome" crates/rupu-orchestrator/src/lib.rs`), add `UnitCoverage, UnitFailure` to that list.

- [ ] **Step 4: Update the runner's call sites without changing behaviour.**
- In `dispatch_placed_step`, change the last arm to:

```rust
        Err(failure) => {
            let source = failure.error;
            let output = source.to_string();
            placed_failure(step, host, output, source, continue_on_error)
        }
```

- In the fan-out, `Err(first_err)` still works for `warn!(error = %first_err, …)` because `UnitFailure: Display`.
- Change the `Err(second_err)` arm to:

```rust
                                            Err(second_err) => {
                                                let msg = second_err.to_string();
                                                (
                                                    msg.clone(),
                                                    false,
                                                    Some(msg),
                                                    Some(second_err.error),
                                                    None,
                                                    false,
                                                )
                                            }
```

- [ ] **Step 5: Update every `UnitDispatcher` impl and `UnitOutcome` literal.**
- Change each impl's signature to `-> Result<UnitOutcome, UnitFailure>`, and add `coverage: UnitCoverage::NotLaunched,` to each `UnitOutcome { … }` literal.
- Where a double returns `Err(RunError::…)`, write `Err(RunError::…(…).into())`. `ProfileRecorder` in `tests/remote_findings_profile.rs` is one example.
- Import `UnitCoverage` / `UnitFailure` from `rupu_orchestrator::runner` in the test files.
- The full list of impls and literals is under **Files** above; `cargo check --workspace --tests` finds any stragglers.

In `crates/rupu-cli/src/fleet_unit_dispatcher.rs`:
- Change the `dispatch_unit` signature to `-> Result<UnitOutcome, UnitFailure>`.
- `?` on a `RunError` now converts through `From`.
- Change each `return Err(host_err_to_run_err(e));` to `return Err(host_err_to_run_err(e).into());`.
- Wrap the final poll-timeout `Err(RunError::Provider(…))` as `Err(RunError::Provider(…).into())`.
- Add `coverage: UnitCoverage::NotLaunched,` to the `UnitOutcome` literal (Task 11 replaces it).
- Fix the test module's assertions that match on `Err(e)` by comparing `e.error`, or `e.to_string()`, where a test inspects the error.

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-orchestrator && cargo test -p rupu-cli --lib fleet_unit_dispatcher`
Expected: PASS, with no behaviour change.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-orchestrator crates/rupu-cli/src/fleet_unit_dispatcher.rs
git commit -m "refactor(orchestrator): UnitCoverage + UnitFailure; dispatch_unit carries coverage on Ok and Err"
```

---

### Task 10: The runner merges remote coverage on every outcome, and `Event::StepWarning`

**Files:**
- Modify: `crates/rupu-orchestrator/src/executor/event.rs` (the variant; `run_id()` at `:166-188`)
- Modify: `crates/rupu-orchestrator/src/runner.rs`:
  - a new `ingest_remote_unit_coverage` and `take_coverage`
  - `dispatch_placed_step` gains a `workflow_run_id: &str` parameter, and its call at `:6194` passes it
  - the fan-out primary and retry dispatches
- Modify: `crates/rupu-cli/src/output/live_run.rs` (a `warnings` field on `LiveRunState`, the new arm, rendering)
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs:185-207` (the arm, plus a comment on why it's not in the fixture)
- Test: `crates/rupu-orchestrator/tests/remote_coverage_ingest.rs` (new)

**Interfaces:**
- Consumes: `rupu_coverage::{ingest_unit_stream, IngestSource}` (Task 2), `UnitCoverage` (Task 9).
- Produces: `Event::StepWarning { run_id, step_id, index: Option<usize>, message }`.

- [ ] **Step 1: Write the failing end-to-end test.** Create `crates/rupu-orchestrator/tests/remote_coverage_ingest.rs`:

```rust
//! A remote unit's coverage stream is merged into the coordinator workspace on
//! every outcome — success and failure — and a missing stream is a warning,
//! never a failed unit (spec 2026-09-30-rupu-remote-findings-transport-design.md §A4).

use async_trait::async_trait;
use rupu_agent::{AgentRunOpts, RunError};
use rupu_coverage::{target_id, CoveragePaths, StreamLine, STREAM_VERSION};
use rupu_orchestrator::executor::{Event, EventSink};
use rupu_orchestrator::runner::{
    run_workflow, OrchestratorRunOpts, StepFactory, UnitCoverage, UnitDispatch, UnitDispatcher,
    UnitFailure, UnitOutcome,
};
use rupu_orchestrator::{RunStore, Workflow};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

struct PanicFactory;

#[async_trait]
impl StepFactory for PanicFactory {
    async fn build_opts_for_step(
        &self,
        _step_id: &str,
        _agent_name: &str,
        _rendered_prompt: String,
        _run_id: String,
        _workspace_id: String,
        _workspace_path: std::path::PathBuf,
        _transcript_path: std::path::PathBuf,
        _on_tool_call: Option<rupu_agent::OnToolCallCallback>,
    ) -> AgentRunOpts {
        panic!("remote units must not be built locally");
    }
}

fn stream_with_finding(id: &str) -> Vec<u8> {
    let record = rupu_coverage::FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: rupu_coverage::FindingScope::File,
        summary: "s".into(),
        severity: rupu_coverage::Severity::High,
        concern_id: None,
        evidence: rupu_coverage::FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: rupu_coverage::Attribution {
            run_id: "run_U".into(),
            model: "m".into(),
            surface: rupu_coverage::Surface::Agent,
        },
        declared_at: chrono::Utc::now(),
        profile: rupu_coverage::FindingProfile::Summary,
        report: None,
    };
    let mut out = String::new();
    for line in [
        StreamLine::Begin {
            v: STREAM_VERSION,
            run_id: "run_U".into(),
        },
        StreamLine::Findings {
            scope_name: "sec".into(),
            record,
        },
    ] {
        out.push_str(&serde_json::to_string(&line).unwrap());
        out.push('\n');
    }
    out.into_bytes()
}

/// Returns a scripted result per dispatch, in order.
struct Scripted {
    results: Mutex<Vec<Result<UnitOutcome, UnitFailure>>>,
}

#[async_trait]
impl UnitDispatcher for Scripted {
    async fn dispatch_unit(
        &self,
        _unit: UnitDispatch,
        _host: &str,
    ) -> Result<UnitOutcome, UnitFailure> {
        self.results.lock().unwrap().remove(0)
    }
}

#[derive(Default)]
struct Collect(Mutex<Vec<Event>>);

impl EventSink for Collect {
    fn emit(&self, _run_id: &str, ev: &Event) {
        self.0.lock().unwrap().push(ev.clone());
    }
}

async fn run(
    yaml: &str,
    results: Vec<Result<UnitOutcome, UnitFailure>>,
) -> (tempfile::TempDir, Arc<Collect>) {
    let tmp = tempfile::tempdir().unwrap();
    let sink = Arc::new(Collect::default());
    let opts = OrchestratorRunOpts {
        run_step: Default::default(),
        workflow: Workflow::parse(yaml).unwrap(),
        inputs: BTreeMap::new(),
        workspace_id: "ws_cov".into(),
        workspace_path: tmp.path().to_path_buf(),
        transcript_dir: tmp.path().join("transcripts"),
        factory: Arc::new(PanicFactory),
        event: None,
        issue: None,
        issue_ref: None,
        run_store: Some(Arc::new(RunStore::new(tmp.path().join("runs")))),
        workflow_yaml: Some(yaml.to_string()),
        resume_from: None,
        run_id_override: None,
        strict_templates: false,
        event_sink: Some(sink.clone()),
        unit_dispatcher: Some(Arc::new(Scripted {
            results: Mutex::new(results),
        })),
        action_dispatcher: None,
        pause: None,
    };
    let _ = run_workflow(opts).await;
    (tmp, sink)
}

const PLACED: &str =
    "name: w\nsteps:\n  - id: s\n    agent: sec\n    prompt: p\n    host: host_01R\n    continue_on_error: true\n";

fn findings_in(ws: &std::path::Path) -> Vec<rupu_coverage::FindingRecord> {
    rupu_coverage::read_findings(&CoveragePaths::new(ws, &target_id(ws, "sec"))).unwrap()
}

#[tokio::test]
async fn a_successful_placed_units_findings_land_under_the_coordinators_target() {
    let (tmp, _sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(stream_with_finding("f_ok")),
        })],
    )
    .await;
    let got = findings_in(tmp.path());
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, "f_ok");
}

#[tokio::test]
async fn a_failed_units_findings_still_land() {
    let (tmp, _sink) = run(
        PLACED,
        vec![Err(UnitFailure {
            error: RunError::Provider("unit died".into()),
            coverage: UnitCoverage::Stream(stream_with_finding("f_failed")),
        })],
    )
    .await;
    assert_eq!(findings_in(tmp.path())[0].id, "f_failed");
}

#[tokio::test]
async fn unavailable_coverage_is_a_step_warning_not_a_failure() {
    let (tmp, sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Unavailable("host predates it".into()),
        })],
    )
    .await;
    assert!(findings_in(tmp.path()).is_empty());
    let events = sink.0.lock().unwrap();
    let warned = events.iter().any(|e| matches!(
        e,
        Event::StepWarning { step_id, message, .. }
            if step_id == "s" && message.contains("host_01R") && message.contains("host predates it")
    ));
    assert!(warned, "{events:?}");
    assert!(
        events.iter().any(|e| matches!(e, Event::StepCompleted { success: true, .. })),
        "a coverage warning must never fail the unit: {events:?}"
    );
}

#[tokio::test]
async fn a_stream_without_a_begin_line_is_a_warning() {
    let (_tmp, sink) = run(
        PLACED,
        vec![Ok(UnitOutcome {
            output: "ok".into(),
            success: true,
            error: None,
            workspace_delta: None,
            coverage: UnitCoverage::Stream(Vec::new()),
        })],
    )
    .await;
    let events = sink.0.lock().unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepWarning { message, .. } if message.contains("no coverage stream")
    )));
}

#[tokio::test]
async fn a_fan_out_primary_failure_and_its_retry_both_merge() {
    let yaml = "name: w\nsteps:\n  - id: fan\n    agent: sec\n    actions: []\n    for_each: \"only\"\n    prompt: \"p {{ item }}\"\n    distribute:\n      hosts: [host_01A, host_01B]\n";
    let (tmp, _sink) = run(
        yaml,
        vec![
            Err(UnitFailure {
                error: RunError::Provider("primary died".into()),
                coverage: UnitCoverage::Stream(stream_with_finding("f_primary")),
            }),
            Ok(UnitOutcome {
                output: "ok".into(),
                success: true,
                error: None,
                workspace_delta: None,
                coverage: UnitCoverage::Stream(stream_with_finding("f_retry")),
            }),
        ],
    )
    .await;
    let mut ids: Vec<String> = findings_in(tmp.path()).into_iter().map(|f| f.id).collect();
    ids.sort();
    assert_eq!(ids, vec!["f_primary", "f_retry"]);
}
```

If `executor::{Event, EventSink}` aren't re-exported at that path, use the path `rupu_orchestrator::runner` uses (for example `rupu_orchestrator::executor::sink::EventSink`).

- [ ] **Step 2: Run the test to confirm it fails.**

Run: `cargo test -p rupu-orchestrator --test remote_coverage_ingest`
Expected: compile error, because there's no `Event::StepWarning`.

- [ ] **Step 3: Add the event.** In `event.rs`, add this after `StepSkipped`:

```rust
    /// Something about a step the operator should see that did not fail it —
    /// e.g. a remote unit whose coverage could not be collected.
    StepWarning {
        run_id: String,
        step_id: String,
        /// The fan-out unit, when the warning is about one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        message: String,
    },
```

Add `| Event::StepWarning { run_id, .. }` to the or-pattern in `run_id()`.

In `crates/rupu-cp/tests/macos_fixtures.rs`'s `assert_events_cover_every_variant`, add:

```rust
            // Deliberately NOT in the events fixture: the macOS app is
            // deprecated (no new Swift work), it decodes an unknown tag as
            // `.unknown`, and `FixtureDecodingTests.swift` rejects `.unknown`.
            Event::StepWarning { .. } => {}
```

- [ ] **Step 4: Show it in the CLI live view.** In `crates/rupu-cli/src/output/live_run.rs`, add this field to `LiveRunState`:

```rust
    /// `StepWarning` messages, in arrival order, as `"<step>: <message>"`.
    pub warnings: Vec<String>,
```

Initialise it as `warnings: Vec::new()` in `from_workflow`, and in any other `LiveRunState { … }` literal the compiler flags. Add this arm to `apply`:

```rust
            WfEvent::StepWarning {
                step_id, message, ..
            } => {
                self.warnings.push(format!("{step_id}: {message}"));
            }
```

Then find where the finished-run footer is rendered: `grep -n "findings_count" crates/rupu-cli/src/output/*.rs` shows the footer. After the findings/coverage line there, print each warning on its own line as `⚠ {w}`, using the same output mechanism (`writeln!` / `println!`) as the neighbouring lines.

- [ ] **Step 5: Merge in the runner.** Add these to `runner.rs`, near `dispatch_placed_step`:

```rust
/// Take the coverage out of a dispatch result, leaving `NotLaunched`.
fn take_coverage(r: &mut Result<UnitOutcome, UnitFailure>) -> UnitCoverage {
    let slot = match r {
        Ok(o) => &mut o.coverage,
        Err(f) => &mut f.coverage,
    };
    std::mem::replace(slot, UnitCoverage::NotLaunched)
}

/// Merge a remote unit's coverage into the coordinator workspace (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §A4) and surface
/// anything that went wrong as a `StepWarning`. Never fails the unit.
#[allow(clippy::too_many_arguments)]
async fn ingest_remote_unit_coverage(
    workspace: &Path,
    host: &str,
    coverage: UnitCoverage,
    sink: Option<&Arc<dyn crate::executor::EventSink>>,
    workflow_run_id: &str,
    step_id: &str,
    index: Option<usize>,
) {
    let warn = |message: String| {
        warn!(step = %step_id, host, %message, "remote unit coverage");
        if let Some(sink) = sink {
            sink.emit(
                workflow_run_id,
                &crate::executor::Event::StepWarning {
                    run_id: workflow_run_id.to_string(),
                    step_id: step_id.to_string(),
                    index,
                    message,
                },
            );
        }
    };
    let bytes = match coverage {
        UnitCoverage::NotLaunched => return,
        UnitCoverage::Unavailable(reason) => {
            warn(format!(
                "coverage from host {host} was not collected: {reason}"
            ));
            return;
        }
        UnitCoverage::Stream(bytes) => bytes,
    };
    let source = rupu_coverage::IngestSource {
        host: (host != "local").then(|| host.to_string()),
    };
    let ws = workspace.to_path_buf();
    let merged = tokio::task::spawn_blocking(move || {
        rupu_coverage::ingest_unit_stream(&ws, &source, &bytes)
    })
    .await;
    match merged {
        Ok(Ok(r)) if !r.begin_seen => warn(format!(
            "host {host} sent no coverage stream (it may predate coverage streaming — upgrade \
             rupu there); this unit's findings and coverage were not collected"
        )),
        Ok(Ok(r)) if r.malformed > 0 => warn(format!(
            "{} malformed coverage line(s) from host {host} were skipped ({} merged)",
            r.malformed, r.appended
        )),
        Ok(Ok(_)) => {}
        Ok(Err(e)) => warn(format!("merging coverage from host {host} failed: {e}")),
        Err(e) => warn(format!("merging coverage from host {host} panicked: {e}")),
    }
}
```

Use whatever `Arc` / `Path` imports `runner.rs` already has.

**Placed step.** Add a `workflow_run_id: &str` parameter to `dispatch_placed_step` (first position), and pass `workflow_run_id` at its call (`:6194`). Then replace `match dispatcher.dispatch_unit(unit, host).await {` with:

```rust
    let mut result = dispatcher.dispatch_unit(unit, host).await;
    ingest_remote_unit_coverage(
        &opts.workspace_path,
        host,
        take_coverage(&mut result),
        opts.event_sink.as_ref(),
        workflow_run_id,
        &step.id,
        None,
    )
    .await;
    match result {
```

**Fan-out.** Inside the spawned task, the coordinator workspace is the task-local `workspace_path` (a clone of `opts.workspace_path`). Replace `match dispatcher.dispatch_unit(unit, &host).await {` with:

```rust
                                let mut result = dispatcher.dispatch_unit(unit, &host).await;
                                ingest_remote_unit_coverage(
                                    &workspace_path,
                                    &host,
                                    take_coverage(&mut result),
                                    event_sink.as_ref(),
                                    &workflow_run_id,
                                    &step_id,
                                    Some(idx),
                                )
                                .await;
                                match result {
```

Replace `match dispatcher.dispatch_unit(retry_unit, retry_host).await` the same way, with `retry_host` in place of `&host`.

The local branch moves `workspace_path` into `dispatch_one`, but the two branches are exclusive, so borrowing it here compiles. If the borrow checker disagrees, clone it into `let coordinator_ws = workspace_path.clone();` before the `if let Some(host) = placement` split, and use that.

- [ ] **Step 6: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-orchestrator && cargo test -p rupu-cli --lib output && cargo test -p rupu-cp --test macos_fixtures`
Expected: PASS, including the 5 new `remote_coverage_ingest` tests.

- [ ] **Step 7: Commit.**

```bash
git add crates/rupu-orchestrator crates/rupu-cli/src/output/live_run.rs crates/rupu-cp/tests/macos_fixtures.rs
git commit -m "feat(orchestrator): merge remote unit coverage on every outcome; Event::StepWarning when it can't"
```

---

### Task 11: The fleet dispatcher collects coverage on every post-launch outcome

**Files:**
- Modify: `crates/rupu-cli/src/fleet_unit_dispatcher.rs`:
  - `dispatch_unit` (`:270-551`)
  - `FakeConnector` (`:661-760`) gains `coverage: Vec<u8>` and `coverage_fails: bool`, and its `unit_coverage` from Task 5 answers from them

**Interfaces:**
- Consumes: `HostConnector::unit_coverage` (Task 5), `UnitCoverage` / `UnitFailure` (Task 9).
- Produces: outcomes carry coverage as follows.

  | Situation | Coverage carried |
  |---|---|
  | Terminal (success or failure) | `Stream` / `Unavailable` |
  | Poll error after the run was observed | `Stream` / `Unavailable` |
  | Wall-clock timeout on an observed run | `Stream` / `Unavailable` |
  | Launch or stage failure | `NotLaunched` |
  | Never started | `NotLaunched` |

- [ ] **Step 1: Write the failing tests.**
- Add `coverage: Vec<u8>, coverage_fails: bool,` to `FakeConnector`. Set `coverage: Vec::new(), coverage_fails: false,` in `completed()` and `failed()`; the other constructors derive from those.
- Make its `unit_coverage` push `"unit_coverage"` to `calls`, then return `Err(HostConnectorError::Unreachable("down".into()))` if `coverage_fails`, else `Ok(self.coverage.clone())`.

Then add these tests:

```rust
    #[tokio::test]
    async fn a_completed_unit_carries_its_coverage_stream() {
        let mut conn = FakeConnector::completed();
        conn.coverage = b"stream".to_vec();
        let conn = Arc::new(conn);
        let d = FleetUnitDispatcher::from_connector(
            Arc::clone(&conn) as Arc<dyn HostConnector>,
            PathBuf::from("/g"),
        );
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert_eq!(out.coverage, UnitCoverage::Stream(b"stream".to_vec()));
    }

    #[tokio::test]
    async fn a_failed_unit_still_carries_its_coverage() {
        let mut conn = FakeConnector::failed();
        conn.coverage = b"stream".to_vec();
        let d = FleetUnitDispatcher::from_connector(Arc::new(conn), PathBuf::from("/g"));
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(!out.success);
        assert_eq!(out.coverage, UnitCoverage::Stream(b"stream".to_vec()));
    }

    #[tokio::test]
    async fn a_connector_that_cannot_deliver_is_unavailable() {
        let mut conn = FakeConnector::completed();
        conn.coverage_fails = true;
        let d = FleetUnitDispatcher::from_connector(Arc::new(conn), PathBuf::from("/g"));
        let out = d.dispatch_unit(make_unit(), "h1").await.unwrap();
        assert!(
            matches!(&out.coverage, UnitCoverage::Unavailable(m) if m.contains("h1")),
            "{:?}",
            out.coverage
        );
    }

    #[tokio::test]
    async fn a_launch_failure_carries_nothing() {
        let d = FleetUnitDispatcher::from_connector(Arc::new(UnreachableConnector), PathBuf::from("/g"));
        let err = d.dispatch_unit(make_unit(), "h1").await.unwrap_err();
        assert_eq!(err.coverage, UnitCoverage::NotLaunched);
    }
```

Import `UnitCoverage` in the test module from `rupu_orchestrator::runner`.

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-cli --lib fleet_unit_dispatcher`
Expected: FAIL. The dispatcher still returns `NotLaunched` from Task 9.

- [ ] **Step 3: Collect coverage in `dispatch_unit`.** Add this helper to `fleet_unit_dispatcher.rs`:

```rust
/// The unit's coverage stream, or why it could not be collected.
async fn collect_coverage(conn: &Arc<dyn HostConnector>, run_id: &str, host: &str) -> UnitCoverage {
    match conn.unit_coverage(run_id).await {
        Ok(bytes) => UnitCoverage::Stream(bytes),
        Err(e) => UnitCoverage::Unavailable(format!("host {host}: {e}")),
    }
}
```

Make these changes in `dispatch_unit`:

1. **Poll error after the run was observed.** Replace `Err(e) => return Err(host_err_to_run_err(e).into()),` with:

```rust
                Err(e) => {
                    let coverage = collect_coverage(&conn, &run_id, host).await;
                    return Err(UnitFailure {
                        error: host_err_to_run_err(e),
                        coverage,
                    });
                }
```

2. **Terminal.** Right after `conn.await_run_mirror(&run_id).await;`, add `let coverage = collect_coverage(&conn, &run_id, host).await;`. In the `(Some(dir), true)` delta arm, carry `coverage` on the error paths instead of using `?`:

```rust
                    (Some(dir), true) => {
                        let bytes = match conn.collect_workspace_delta(dir).await {
                            Ok(b) => b,
                            Err(e) => {
                                return Err(UnitFailure {
                                    error: host_err_to_run_err(e),
                                    coverage,
                                })
                            }
                        };
                        let delta = match decode_delta(&bytes) {
                            Ok(d) => d,
                            Err(e) => {
                                return Err(UnitFailure {
                                    error: host_err_to_run_err(e),
                                    coverage,
                                })
                            }
                        };
                        Some(to_orchestrator_delta(&delta))
                    }
```

Set `coverage,` in the returned `UnitOutcome` in place of Task 9's `NotLaunched`.

3. **Timeout.** Before the final `Err(RunError::Provider(match …))`, compute:

```rust
        // A run the host never showed has no stream to collect — and saying
        // "no coverage stream" on top of "never started" would mislead.
        let coverage = if never_started {
            UnitCoverage::NotLaunched
        } else {
            collect_coverage(&conn, &run_id, host).await
        };
```

Then build `Err(UnitFailure { error: RunError::Provider(match … { … }), coverage })`, keeping the existing message-building `match` unchanged inside `RunError::Provider(…)`.

Launch and stage failures keep `.into()` (`NotLaunched`).

- [ ] **Step 4: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-cli --lib fleet_unit_dispatcher`
Expected: PASS, including the existing startup/timeout tests.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-cli/src/fleet_unit_dispatcher.rs
git commit -m "feat(fleet): collect unit coverage on every post-launch outcome"
```

---

### Task 12: Collected workspace deltas no longer carry `.rupu/coverage/`

**Files:**
- Modify: `crates/rupu-workspace/src/workspace_sync.rs`: `collect_delta_tar` (`:142`), `collect_delta_git` (`:355`), and tests (`:500+`)

**Interfaces:**
- Produces: `pub(crate) fn excluded_from_delta(rel: &str) -> bool`, plus deltas without `.rupu/coverage/` paths in both modes. Pack and apply are unchanged, so `.rupu/coverage/tool-mappings.yaml` still ships to hosts, and an older host's delta still applies.

- [ ] **Step 1: Write the failing tests.** Add these to the `tests` module in `workspace_sync.rs`. They use its existing `write` and `git_init` helpers.

```rust
    #[test]
    fn tar_delta_excludes_rupu_coverage() {
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), "a.txt", "a");
        let payload = pack_tar(ws.path()).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let baseline = stage_tar(&payload, scratch.path()).unwrap();
        write(scratch.path(), "b.txt", "b");
        write(scratch.path(), ".rupu/coverage/t1/findings.jsonl", "{}\n");
        let delta = collect_delta_tar(scratch.path(), &baseline).unwrap();
        assert_eq!(delta.changed, vec!["b.txt".to_string()]);
        apply_deltas_tar(ws.path(), &[delta]).unwrap();
        assert!(ws.path().join("b.txt").exists());
        assert!(!ws.path().join(".rupu/coverage").exists());
    }

    #[test]
    fn git_delta_excludes_rupu_coverage_from_paths_and_patch() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path());
        let payload = pack(ws.path()).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let baseline = stage(&payload, scratch.path()).unwrap();
        write(scratch.path(), "b.txt", "b\n");
        write(scratch.path(), ".rupu/coverage/t1/findings.jsonl", "{\"x\":1}\n");
        let delta = collect_delta(scratch.path(), &baseline).unwrap();
        assert!(delta.changed.contains(&"b.txt".to_string()));
        assert!(!delta.changed.iter().any(|p| p.starts_with(".rupu/coverage/")));
        let patch = String::from_utf8_lossy(&delta.bytes);
        assert!(!patch.contains(".rupu/coverage"), "{patch}");
        apply_deltas(ws.path(), &[delta]).unwrap();
        assert_eq!(fs::read_to_string(ws.path().join("b.txt")).unwrap(), "b\n");
        assert!(!ws.path().join(".rupu/coverage").exists());
    }
```

- [ ] **Step 2: Run the tests to confirm they fail.**

Run: `cargo test -p rupu-workspace --lib delta_excludes_rupu_coverage`
Expected: FAIL, because the coverage path is in `changed` and in the patch.

- [ ] **Step 3: Implement the exclusion.** Add this above `collect_delta_tar`:

```rust
/// Coverage ledgers travel through the run's coverage stream (spec
/// 2026-09-30-rupu-remote-findings-transport-design.md §A4), not the
/// workspace delta: carried here they would land under a target id derived
/// from the scratch path. Filtered at COLLECTION only — collection runs on
/// the host, so an older host's delta still carries (and still applies) them.
const DELTA_EXCLUDED_PREFIX: &str = ".rupu/coverage/";

pub(crate) fn excluded_from_delta(rel: &str) -> bool {
    rel.starts_with(DELTA_EXCLUDED_PREFIX)
}
```

In `collect_delta_tar`, skip excluded paths in both loops:

```rust
    for (path, hash) in &after {
        if excluded_from_delta(path) {
            continue;
        }
        match baseline.tar_manifest.get(path) {
            Some(old) if old == hash => {}
            _ => changed.push(path.clone()),
        }
    }
    for path in baseline.tar_manifest.keys() {
        if !excluded_from_delta(path) && !after.contains_key(path) {
            deleted.push(path.clone());
        }
    }
```

In `collect_delta_git`, skip excluded deltas in the `for d in diff.deltas()` loop by adding `if excluded_from_delta(&p) { continue; }` as the first statement inside `if let Some(p) = path {`. In the patch printer, skip every line that belongs to an excluded file:

```rust
    diff.print(git2::DiffFormat::Patch, |d, _h, line| {
        let excluded = d
            .new_file()
            .path()
            .or_else(|| d.old_file().path())
            .is_some_and(|p| excluded_from_delta(&p.to_string_lossy().replace('\\', "/")));
        if excluded {
            return true;
        }
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin() as u8);
        }
        patch.extend_from_slice(line.content());
        true
    })
```

- [ ] **Step 4: Run the tests to confirm they pass.**

Run: `cargo test -p rupu-workspace`
Expected: PASS, including the existing round-trip tests.

- [ ] **Step 5: Commit.**

```bash
git add crates/rupu-workspace/src/workspace_sync.rs
git commit -m "fix(workspace): collected deltas no longer carry .rupu/coverage/ (it travels in the coverage stream)"
```

---

### Task 13: Docs, and the full gate

**Files:**
- Modify: `docs/coverage.md` (the remote-unit artifacts bullet added in #675)
- Modify: `docs/workflow-format.md` (the remote-steps bullets under `findings_profile`, and the workspace-sync section)
- Modify: `CLAUDE.md` (the `rupu-coverage` and `rupu-cp` crate bullets)

- [ ] **Step 1: `docs/coverage.md`.** Replace the bullet starting "A remote workflow unit (`host:` / `distribute:`) runs `report_finding` on the host…" with:

```markdown
- A remote workflow unit (`host:` / `distribute:`) records findings on its
  host. Every `rupu run` also streams its coverage (runs, file touches,
  concern assertions, findings) to `$RUPU_HOME/runs/<run_id>/coverage.jsonl`,
  which the host's connector delivers to the coordinator when the unit ends —
  success or failure — on every transport. The coordinator merges it under its
  own workspace's targets and records the unit's artifacts `stored: external`
  with `host` set (their blobs stay in the host's store). A host too old to
  stream shows a `StepWarning` on the step; the unit is not failed.
```

- [ ] **Step 2: `docs/workflow-format.md`.** Under `findings_profile` → Rules, add this after the remote-steps bullets:

```markdown
- A remote unit's findings and coverage reach the coordinator through the
  run's coverage stream, not the workspace delta: `.rupu/coverage/` is not
  carried back by `workspace: sync`.
```

In the workspace-sync section (`grep -n "workspace: sync" docs/workflow-format.md`), add one sentence: "Coverage ledgers under `.rupu/coverage/` are not part of the returned delta; they travel in the run's coverage stream."

- [ ] **Step 3: `CLAUDE.md`.**
- Append this to the `rupu-coverage` bullet: "`ledger::stream` is the single ledger write path (`append_record`), mirrored to a run's `runs/<id>/coverage.jsonl` when `ToolContext.coverage_stream` is set (`rupu run` only). `ledger::ingest::ingest_unit_stream` merges a remote unit's stream into the coordinator workspace: re-keyed by scope, deduplicated, with artifacts rewritten to `external` + `host`."
- Append this to the `rupu-cp` bullet: "`HostConnector::unit_coverage` (required) delivers a unit's coverage stream: mirror-backed transports read the mirror (SSH tails it plus a terminal catch-up; tunnel via `ArtifactFile::Coverage` when `Welcome` advertises `mirror.coverage`; bucket via `coverage.NNNN.jsonl` results), and HTTP GETs `/api/runs/:id/coverage` behind the `run.coverage_stream` feature."
- Add the new spec and Plan A to "Read first".

- [ ] **Step 4: Full gate.**

Run:

```bash
cargo test -p rupu-coverage
cargo test -p rupu-tools
cargo test -p rupu-agent
cargo test -p rupu-workspace
cargo test -p rupu-orchestrator
cargo test -p rupu-cp
cargo test -p rupu-cli
cargo clippy -p rupu-coverage -p rupu-tools -p rupu-agent -p rupu-workspace -p rupu-orchestrator -p rupu-cp -p rupu-cli --all-targets -- -D warnings -A clippy::question_mark
```

Expected: all PASS, and clippy is clean.

- [ ] **Step 5: Commit.**

```bash
git add docs/coverage.md docs/workflow-format.md CLAUDE.md
git commit -m "docs: remote units' coverage travels in the run stream; artifacts recorded external+host"
```
