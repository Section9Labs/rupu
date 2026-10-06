# Finding Tags — Plan 1 (core, agent + MCP tools, CLI) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Agents and operators can add and remove free-form tags on findings, and query and filter findings by tag. This works through `rupu-coverage`, the agent built-ins `query_findings` / `tag_findings`, the MCP tools `findings.query` / `findings.tag`, and `rupu findings list|tag|tags`.

**Architecture:**
- A finding's *declared* tags live on its `FindingRecord`, in a new `tags` field.
- Later changes are add/remove `TagEvent`s in one append-only log per workspace, `<workspace>/.rupu/coverage/finding_tags.jsonl`.
- Reads fold the events onto the declared tags, in file order.
- `ledger::tags::apply` is the only writer. It works under the log's own sidecar lock and mirrors its events into the run's coverage stream as a new `tags` line kind, which `ingest_unit_stream` merges, deduped by event id.
- `ledger::query` is the one filter: `select` returns every match (CLI, and the CP in Plan 2); `query` returns cursor pages (agent tool, MCP).

**Tech Stack:** Rust 2021 · serde / serde_json · chrono · ulid · thiserror · tokio · clap · assert_cmd (CLI tests).

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md` (read it first). The plan argues from it.

## Global Constraints

- **Tag syntax.** Trim, then lowercase. After that a tag is 1–64 characters from `[a-z0-9._:/-]`, and its first character is `[a-z0-9]`. Invalid tags are **rejected, never rewritten**.
- **At most 32 tags per finding** (`MAX_TAGS_PER_FINDING = 32`).
- **Fold rule.** Effective tags = declared tags, then events applied in **file order** (add inserts, remove deletes). Events for an unknown `finding_id` are ignored when folding. Lines that can't be parsed are skipped with a warning, never treated as errors.
- **One writer.** `ledger::tags::apply` is the ONLY function that writes tag events (`ingest_tag_events` is the ingest-side append, under the same lock). Finding lines are never rewritten for tags.
- **Workspace deps only.** Never add a version to a crate `Cargo.toml`. This plan adds no new dependencies.
- **Integration tests.** Each crate has ONE test binary: a new test file is a module under `crates/<c>/tests/it/`, listed in that directory's `main.rs`. Never add a top-level `tests/*.rs`.
- **Running tests.** Run only the touched test modules, e.g. `cargo test -p rupu-coverage --lib ledger::tags` or `cargo test -p rupu-cli --test it findings_tags::`. Never run a cold full-workspace `cargo test`.
- **Formatting.** Format per file only (`rustfmt --edition 2021 <file>…`). NEVER run `cargo fmt` package-wide or workspace-wide, because `main` is fmt-dirty.
- **Lints.** `#![deny(clippy::all)]` is on. Before each commit, run `cargo clippy -p <crate> --all-targets -- -D warnings` on the crates you touched.
- **Fixtures.** All test fixtures and examples are invented from scratch. Never adapt assessment data, even renamed.
- **Git.** Never use bare `git stash` / `git stash pop` (the stash is shared across worktrees). Stage only the files your task names. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## File map

| File | Responsibility |
|---|---|
| `crates/rupu-coverage/src/ledger/tags.rs` (new) | `Tag`, `TagEvent`, `TagActor`, `TagLog`, read, fold, `apply`, history, workspace reads, `TagChangeInput`, `tag_input_schema` |
| `crates/rupu-coverage/src/ledger/query.rs` (new) | `FindingQuery`, `select`, `query` (paged), `FindingRow`, `tags_in_use`, `query_response`, `query_input_schema` |
| `crates/rupu-coverage/src/ledger/events.rs` | `FindingRecord.tags` + lenient wire read |
| `crates/rupu-coverage/src/ledger/views.rs` | `read_declared_findings`; `read_findings` folds |
| `crates/rupu-coverage/src/ledger/paths.rs` | `CoveragePaths::tag_log()` |
| `crates/rupu-coverage/src/ledger/stream.rs` | `Ledger::Tags`, `StreamLine::Tags`, `stream_json_to`, generic `locked_at` |
| `crates/rupu-coverage/src/ledger/ingest.rs` | merge `tags` stream lines |
| `crates/rupu-coverage/src/tools/report_finding.rs` | `tags` input |
| `crates/rupu-coverage/src/catalog/types.rs` | `Severity::as_str` |
| `crates/rupu-agent/src/coverage_tools.rs` | `QueryFindingsTool`, `TagFindingsTool`, `tags` in the report schema |
| `crates/rupu-agent/src/runner.rs` | grant-gated registration |
| `crates/rupu-mcp/src/tools/findings.rs`, `src/dispatcher.rs` | `findings.query`, `findings.tag`, `tags` on `findings.record` |
| `crates/rupu-cp/src/api/findings.rs` | `tag_findings_across` (library function only; no route in this plan) |
| `crates/rupu-cli/src/cmd/findings.rs`, `src/lib.rs` | `rupu findings list|tag|tags` |
| `docs/coverage.md`, `CLAUDE.md` | docs |

---

### Task 1: `Tag` and the declared `tags` field on `FindingRecord`

**Files:**
- Create: `crates/rupu-coverage/src/ledger/tags.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs`, `crates/rupu-coverage/src/ledger/events.rs:237-330`
- Modify (add `tags: Vec::new(),` to each `FindingRecord { … }` literal; the compiler lists them all): `rupu-coverage/src/{ledger/events.rs,ledger/views.rs,ledger/ingest.rs,ledger/stream.rs,tools/report_finding.rs,diff/generate.rs}`, `rupu-agentiflow/src/{goal.rs,status_tools.rs,run.rs}`, `rupu-cli/src/cmd/{coverage.rs,node.rs}`, `rupu-cli/src/output/{printer.rs,run_model.rs}` (only where it is `rupu_coverage::FindingRecord`), `rupu-cli/tests/it/{findings_export.rs,findings_import.rs}`, `rupu-cp/src/api/{coverage.rs,findings.rs}`, `rupu-cp/tests/it/finding_artifacts.rs`, `rupu-findings-report/src/{number.rs,render.rs,select.rs}`, `rupu-findings-report/tests/it/common/mod.rs`, `rupu-orchestrator/tests/it/remote_coverage_ingest.rs`.
  - `rupu_orchestrator::FindingRecord` (in `rupu-orchestrator/src/runs.rs` and `rupu-cli/src/output/live_run.rs`) is a different type. Don't touch it.

**Interfaces:**
- Produces:
  - `rupu_coverage::ledger::tags::{Tag, TagParseError, parse_tags, MAX_TAGS_PER_FINDING}`
  - `Tag::parse(&str) -> Result<Tag, TagParseError>`, `Tag::as_str(&self) -> &str`
  - `parse_tags<S: AsRef<str>>(&[S]) -> Result<Vec<Tag>, TagParseError>`, which returns the tags deduped and sorted.
  - `FindingRecord.tags: Vec<Tag>`
  - All of these are re-exported from `rupu_coverage`.

- [ ] **Step 1: Write the failing tests.** Create `crates/rupu-coverage/src/ledger/tags.rs` containing only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_trimmed_and_lowercased() {
        assert_eq!(Tag::parse("  Needs-POC ").unwrap().as_str(), "needs-poc");
        assert_eq!(Tag::parse("class:SQLi").unwrap().as_str(), "class:sqli");
        assert_eq!(Tag::parse("team/payments_v2.1").unwrap().as_str(), "team/payments_v2.1");
    }

    #[test]
    fn an_invalid_tag_is_rejected_never_rewritten() {
        for (raw, why) in [
            ("", "empty"),
            ("   ", "empty"),
            ("needs poc", "only a-z"),
            ("naïve", "only a-z"),
            ("-leading", "must start"),
            (":leading", "must start"),
        ] {
            let e = Tag::parse(raw).unwrap_err();
            assert!(e.to_string().contains(why), "{raw:?}: {e}");
        }
        assert!(Tag::parse(&"a".repeat(64)).is_ok());
        assert!(Tag::parse(&"a".repeat(65))
            .unwrap_err()
            .to_string()
            .contains("longer than 64"));
    }

    #[test]
    fn parse_tags_dedupes_and_sorts() {
        let tags = parse_tags(&["b", "A", "a", "b"]).unwrap();
        let got: Vec<&str> = tags.iter().map(Tag::as_str).collect();
        assert_eq!(got, ["a", "b"]);
        assert!(parse_tags(&["ok", "not ok"]).is_err());
    }

    #[test]
    fn a_tag_serializes_as_a_plain_string_and_validates_on_read() {
        let t = Tag::parse("needs-poc").unwrap();
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"needs-poc\"");
        assert!(serde_json::from_str::<Tag>("\"bad tag\"").is_err());
    }
}
```

Add the `FindingRecord` tests to the end of the existing `mod tests` in `crates/rupu-coverage/src/ledger/events.rs`:

```rust
    /// A minimal finding line; `extra` is spliced in before the closing brace.
    fn finding_line(extra: &str) -> String {
        format!(
            "{{\"id\":\"fnd_1\",\"scope\":\"repo\",\"summary\":\"s\",\"severity\":\"low\",\
             \"evidence\":{{\"rationale\":\"r\"}},\
             \"declared_by\":{{\"run_id\":\"r\",\"model\":\"m\",\"surface\":\"workflow\"}},\
             \"declared_at\":\"2026-10-06T00:00:00Z\"{extra}}}"
        )
    }

    #[test]
    fn a_line_without_tags_reads_with_none_and_writes_none() {
        let rec: FindingRecord = serde_json::from_str(&finding_line("")).unwrap();
        assert!(rec.tags.is_empty());
        let back = serde_json::to_string(&rec).unwrap();
        assert!(!back.contains("\"tags\""), "{back}");
    }

    #[test]
    fn declared_tags_round_trip_deduped_and_sorted() {
        let rec: FindingRecord =
            serde_json::from_str(&finding_line(",\"tags\":[\"b\",\"a\",\"b\"]")).unwrap();
        let tags: Vec<&str> = rec.tags.iter().map(|t| t.as_str()).collect();
        assert_eq!(tags, ["a", "b"]);
        let again: FindingRecord =
            serde_json::from_str(&serde_json::to_string(&rec).unwrap()).unwrap();
        assert_eq!(again, rec);
    }

    #[test]
    fn an_unreadable_tag_drops_the_tag_not_the_finding() {
        let rec: FindingRecord =
            serde_json::from_str(&finding_line(",\"tags\":[\"ok\",\"not ok\"]")).unwrap();
        let tags: Vec<&str> = rec.tags.iter().map(|t| t.as_str()).collect();
        assert_eq!(tags, ["ok"]);
    }
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-coverage --lib ledger::`
  - Expected: a compile error. `tags` isn't a module yet, and `FindingRecord` has no field `tags`.

- [ ] **Step 3: Implement `Tag`.** Put this above the test module in `tags.rs`:

```rust
//! Finding tags (spec `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md`).
//!
//! A finding's tags are the ones it was declared with (`FindingRecord::tags`)
//! plus the add/remove events in its workspace's `finding_tags.jsonl`,
//! applied in file order.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// At most this many effective tags on one finding.
pub const MAX_TAGS_PER_FINDING: usize = 32;

/// A normalized tag. Constructing one is the only validation path, so a
/// `Tag` in hand is always valid.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tag(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tag `{raw}`: {reason}")]
pub struct TagParseError {
    pub raw: String,
    pub reason: &'static str,
}

impl Tag {
    pub const MAX_LEN: usize = 64;

    /// Trim and lowercase `raw`, then validate it. An invalid tag is an
    /// error naming it; it is never rewritten into a valid one.
    pub fn parse(raw: &str) -> Result<Self, TagParseError> {
        let err = |reason| TagParseError {
            raw: raw.to_string(),
            reason,
        };
        let t = raw.trim().to_lowercase();
        if t.is_empty() {
            return Err(err("empty"));
        }
        let allowed = |c: char| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | ':' | '/' | '-')
        };
        if !t.chars().all(allowed) {
            return Err(err("only a-z, 0-9 and . _ : / - are allowed"));
        }
        if !t.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(err("must start with a letter or digit"));
        }
        // ASCII only from here on, so bytes == characters.
        if t.len() > Self::MAX_LEN {
            return Err(err("longer than 64 characters"));
        }
        Ok(Tag(t))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Tag {
    type Error = TagParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Tag::parse(&s)
    }
}

impl From<Tag> for String {
    fn from(t: Tag) -> String {
        t.0
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Tag {
    type Err = TagParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Tag::parse(s)
    }
}

/// Validate every entry; the result is deduped and sorted. The first
/// invalid entry is the error.
pub fn parse_tags<S: AsRef<str>>(raw: &[S]) -> Result<Vec<Tag>, TagParseError> {
    let set: BTreeSet<Tag> = raw
        .iter()
        .map(|r| Tag::parse(r.as_ref()))
        .collect::<Result<_, _>>()?;
    Ok(set.into_iter().collect())
}
```

In `crates/rupu-coverage/src/ledger/mod.rs`, add `pub mod tags;` and `pub use tags::{parse_tags, Tag, TagParseError, MAX_TAGS_PER_FINDING};`. In `crates/rupu-coverage/src/lib.rs`, add `parse_tags, Tag, TagParseError, MAX_TAGS_PER_FINDING` to the `pub use ledger::{…}` list.

- [ ] **Step 4: Add the field.**
  - In `events.rs`, add the field as the **last** field of `FindingRecord` (after `report`):

```rust
    /// Tags the finding was declared with. Its effective tags also fold in
    /// its workspace's tag log (`ledger::tags`); `read_findings` returns
    /// them folded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<crate::ledger::tags::Tag>,
```

  - Add the matching field as the last field of `FindingRecordWire`:

```rust
    #[serde(default)]
    tags: Vec<String>,
```

  - In `impl From<FindingRecordWire> for FindingRecord`, add `tags: lenient_tags(&w.id, w.tags),` to the `Self { … }` literal.
    - Note: `w.id` is moved into `id: w.id`. Compute `let tags = lenient_tags(&w.id, w.tags);` **before** the `Self { … }` literal, then use `tags,` inside it.
  - Add this helper below the impl:

```rust
/// Declared tags as read from a ledger line: a tag this build cannot read
/// (a newer writer's syntax, a hand edit) is dropped with a warning naming
/// the finding, never the whole line. Deduped and sorted.
fn lenient_tags(finding_id: &str, raw: Vec<String>) -> Vec<crate::ledger::tags::Tag> {
    let mut set = std::collections::BTreeSet::new();
    for r in raw {
        match crate::ledger::tags::Tag::parse(&r) {
            Ok(t) => {
                set.insert(t);
            }
            Err(e) => tracing::warn!(
                finding_id = %finding_id,
                error = %e,
                "dropping a finding tag this rupu version cannot read"
            ),
        }
    }
    set.into_iter().collect()
}
```

- [ ] **Step 5: Fix every construction site.**
  - Run `cargo check --workspace --all-targets 2>&1 | grep -B2 -A6 "missing field \`tags\`"`.
  - Add `tags: Vec::new(),` as the last field of each `FindingRecord { … }` literal it reports (the files are listed above).
  - Repeat until `cargo check --workspace --all-targets` is clean.

- [ ] **Step 6: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-coverage --lib ledger::`
  - Expected: PASS, including the 4 new `tags::tests` and the 3 new `events::tests`.

- [ ] **Step 7: Lint, format and commit.**
  - Run `rustfmt --edition 2021` on every file you touched.
  - Run `cargo clippy -p rupu-coverage --all-targets -- -D warnings`.
  - Commit:

```bash
git add crates/rupu-coverage crates/rupu-agentiflow crates/rupu-cli crates/rupu-cp crates/rupu-findings-report crates/rupu-orchestrator
git commit -m "feat(coverage): Tag type and declared tags on FindingRecord

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Tag events, the workspace tag log, fold, and folded reads

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/tags.rs`, `crates/rupu-coverage/src/ledger/views.rs:45-59`, `crates/rupu-coverage/src/ledger/paths.rs`, `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs`

**Interfaces:**
- Consumes: `Tag`, and `FindingRecord.tags` (Task 1).
- Produces (all re-exported from `rupu_coverage`):
  - `TagOp { Add, Remove }`
  - `OperatorSurface { Cli, Cp }`
  - `OperatorAttribution { user: String, via: OperatorSurface }`
  - `TagActor::{Agent(Attribution), Operator(OperatorAttribution)}`, plus `TagActor::operator(OperatorSurface) -> TagActor`
  - `TagEvent { id, finding_id, op, tag, by, at }`
  - `TagLog { workspace, path, lock, run_stream }`, with `TagLog::for_workspace(&Path)` and `.with_run_stream(Option<RunStream>)`
  - `TAG_LOG_FILE`
  - `read_tag_events(&TagLog) -> io::Result<Vec<TagEvent>>`
  - `fold_tags(&mut [FindingRecord], &[TagEvent])`
  - `tag_history(&TagLog, &str) -> io::Result<Vec<TagEvent>>`
  - `read_declared_workspace_findings(&Path) -> io::Result<Vec<FindingRecord>>`, `read_workspace_findings(&Path) -> io::Result<Vec<FindingRecord>>`
  - `views::read_declared_findings(&CoveragePaths)`; `read_findings` now folds
  - `CoveragePaths::tag_log(&self) -> TagLog`

- [ ] **Step 1: Write the failing tests.** Append to `tags.rs`'s `mod tests`:

```rust
    use crate::ledger::events::{Attribution, FindingEvidence, FindingRecord, FindingScope, Surface};
    use crate::ledger::paths::CoveragePaths;
    use crate::ledger::stream::{append_record, Ledger};
    use chrono::Utc;

    pub(crate) fn attribution() -> Attribution {
        Attribution {
            run_id: "run_t".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: Some("tagger".into()),
            provider: None,
        }
    }

    pub(crate) fn finding(id: &str, tags: &[&str]) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: Some("src/lib.rs".into()),
            line_range: Some([1, 2]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: format!("summary of {id}"),
            severity: crate::catalog::types::Severity::High,
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
            tags: parse_tags(tags).unwrap(),
        }
    }

    fn ev(finding_id: &str, op: TagOp, tag: &str) -> TagEvent {
        TagEvent {
            id: format!("tge_{}", ulid::Ulid::new()),
            finding_id: finding_id.into(),
            op,
            tag: Tag::parse(tag).unwrap(),
            by: TagActor::Agent(attribution()),
            at: Utc::now(),
        }
    }

    fn write_events(log: &TagLog, events: &[TagEvent]) {
        std::fs::create_dir_all(log.path.parent().unwrap()).unwrap();
        let text: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(&log.path, text).unwrap();
    }

    fn names(r: &FindingRecord) -> Vec<&str> {
        r.tags.iter().map(Tag::as_str).collect()
    }

    #[test]
    fn fold_applies_events_in_file_order_last_one_wins() {
        let mut recs = vec![finding("fnd_a", &["keep", "drop"]), finding("fnd_b", &[])];
        fold_tags(
            &mut recs,
            &[
                ev("fnd_a", TagOp::Remove, "drop"),
                ev("fnd_a", TagOp::Add, "x"),
                ev("fnd_a", TagOp::Remove, "x"),
                ev("fnd_a", TagOp::Add, "x"),
                ev("fnd_b", TagOp::Add, "y"),
                ev("fnd_unknown", TagOp::Add, "z"),
            ],
        );
        assert_eq!(names(&recs[0]), ["keep", "x"]);
        assert_eq!(names(&recs[1]), ["y"]);
    }

    #[test]
    fn an_orphan_event_applies_once_its_finding_arrives() {
        let events = [ev("fnd_late", TagOp::Add, "triaged")];
        let mut none: Vec<FindingRecord> = vec![];
        fold_tags(&mut none, &events);
        let mut later = vec![finding("fnd_late", &[])];
        fold_tags(&mut later, &events);
        assert_eq!(names(&later[0]), ["triaged"]);
    }

    #[test]
    fn actors_serialize_with_a_kind() {
        let agent = serde_json::to_value(TagActor::Agent(attribution())).unwrap();
        assert_eq!(agent["kind"], "agent");
        assert_eq!(agent["run_id"], "run_t");
        let op = serde_json::to_value(TagActor::Operator(OperatorAttribution {
            user: "alice".into(),
            via: OperatorSurface::Cli,
        }))
        .unwrap();
        assert_eq!(op, serde_json::json!({"kind":"operator","user":"alice","via":"cli"}));
    }

    #[test]
    fn unreadable_log_lines_are_skipped_not_fatal() {
        let ws = tempfile::TempDir::new().unwrap();
        let log = TagLog::for_workspace(ws.path());
        let good = ev("fnd_a", TagOp::Add, "ok");
        write_events(&log, std::slice::from_ref(&good));
        let mut text = std::fs::read_to_string(&log.path).unwrap();
        text.push_str("not json\n");
        text.push_str(
            &serde_json::to_string(&good)
                .unwrap()
                .replace("\"add\"", "\"rename\""),
        );
        text.push_str("\n{\"id\":\"tge_x\",\"finding_id\":\"fnd_a\",\"op\":\"add\",\"tag\":\"BAD TAG\",\"by\":{\"kind\":\"operator\",\"user\":\"u\",\"via\":\"cli\"},\"at\":\"2026-10-06T00:00:00Z\"}\n");
        std::fs::write(&log.path, text).unwrap();
        assert_eq!(read_tag_events(&log).unwrap(), vec![good]);
    }

    #[test]
    fn a_missing_log_reads_as_no_events() {
        let ws = tempfile::TempDir::new().unwrap();
        assert!(read_tag_events(&TagLog::for_workspace(ws.path())).unwrap().is_empty());
    }

    #[test]
    fn read_findings_folds_the_workspace_log_across_targets() {
        let ws = tempfile::TempDir::new().unwrap();
        let t1 = CoveragePaths::new(ws.path(), "t1");
        let t2 = CoveragePaths::new(ws.path(), "t2");
        append_record(&t1, Ledger::Findings, &finding("fnd_a", &["declared"])).unwrap();
        append_record(&t2, Ledger::Findings, &finding("fnd_b", &[])).unwrap();
        write_events(
            &TagLog::for_workspace(ws.path()),
            &[ev("fnd_a", TagOp::Add, "x"), ev("fnd_b", TagOp::Add, "y")],
        );

        let a = crate::ledger::views::read_findings(&t1).unwrap();
        assert_eq!(names(&a[0]), ["declared", "x"]);
        let declared = crate::ledger::views::read_declared_findings(&t1).unwrap();
        assert_eq!(names(&declared[0]), ["declared"]);

        let all = read_workspace_findings(ws.path()).unwrap();
        assert_eq!(all.len(), 2);
        let b = all.iter().find(|r| r.id == "fnd_b").unwrap();
        assert_eq!(names(b), ["y"]);
        // The log file sits beside the target dirs and is not one.
        let targets = crate::ledger::discover::discover_targets(ws.path()).unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn history_is_one_findings_events_in_file_order() {
        let ws = tempfile::TempDir::new().unwrap();
        let log = TagLog::for_workspace(ws.path());
        let events = [
            ev("fnd_a", TagOp::Add, "x"),
            ev("fnd_b", TagOp::Add, "y"),
            ev("fnd_a", TagOp::Remove, "x"),
        ];
        write_events(&log, &events);
        let h = tag_history(&log, "fnd_a").unwrap();
        assert_eq!(h, vec![events[0].clone(), events[2].clone()]);
    }
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-coverage --lib ledger::tags`
  - Expected: a compile error (`TagEvent`, `TagLog`, `fold_tags` and the rest are undefined).

- [ ] **Step 3: Implement.** Add this to `tags.rs`, below `parse_tags`. Extend the `use` lines at the top with `use crate::ledger::events::{Attribution, FindingRecord}; use crate::ledger::stream::RunStream; use chrono::{DateTime, Utc}; use std::collections::HashMap; use std::path::{Path, PathBuf};`.

```rust
/// The tag log's file name, at `<workspace>/.rupu/coverage/`.
pub const TAG_LOG_FILE: &str = "finding_tags.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagOp {
    Add,
    Remove,
}

/// Where an operator's change came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperatorSurface {
    Cli,
    Cp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorAttribution {
    pub user: String,
    pub via: OperatorSurface,
}

/// Who changed a tag: an agent (the same attribution `report_finding`
/// stamps) or a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TagActor {
    Agent(Attribution),
    Operator(OperatorAttribution),
}

impl TagActor {
    /// The person running this process (`$USER`, else `unknown`).
    pub fn operator(via: OperatorSurface) -> Self {
        let user = std::env::var("USER")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        TagActor::Operator(OperatorAttribution { user, via })
    }
}

/// One line of `finding_tags.jsonl`. Not `deny_unknown_fields`: a newer
/// writer's extra fields are ignored. A line with an op, actor kind or tag
/// this build cannot read is skipped by [`read_tag_events`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEvent {
    pub id: String,
    pub finding_id: String,
    pub op: TagOp,
    pub tag: Tag,
    pub by: TagActor,
    pub at: DateTime<Utc>,
}

/// A workspace's tag log: one per workspace, shared by all its coverage
/// targets (a run stream names its writer's scope, never the target of the
/// finding it tags, so a per-target log could not route a remote event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagLog {
    pub workspace: PathBuf,
    pub path: PathBuf,
    /// Sidecar lock serializing every writer of `path`.
    pub lock: PathBuf,
    /// Where [`apply`]'s events are mirrored (`rupu run` only).
    pub run_stream: Option<RunStream>,
}

impl TagLog {
    pub fn for_workspace(workspace: &Path) -> Self {
        let dir = workspace.join(".rupu").join("coverage");
        Self {
            workspace: workspace.to_path_buf(),
            path: dir.join(TAG_LOG_FILE),
            lock: dir.join(format!("{TAG_LOG_FILE}.lock")),
            run_stream: None,
        }
    }

    pub fn with_run_stream(mut self, run_stream: Option<RunStream>) -> Self {
        self.run_stream = run_stream;
        self
    }
}

/// Every readable event, in file order. A missing log is no events; a line
/// that is not an event this build can read (torn, hand-edited, or from a
/// newer writer) is skipped with one warning, never an error.
pub fn read_tag_events(log: &TagLog) -> std::io::Result<Vec<TagEvent>> {
    let raw = match std::fs::read(&log.path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let mut skipped = 0usize;
    let events: Vec<TagEvent> = raw
        .split(|b| *b == b'\n')
        .filter_map(|l| std::str::from_utf8(l).ok())
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match serde_json::from_str::<TagEvent>(l) {
            Ok(e) => Some(e),
            Err(_) => {
                skipped += 1;
                None
            }
        })
        .collect();
    if skipped > 0 {
        tracing::warn!(
            path = %log.path.display(),
            skipped,
            "skipped finding-tag log lines this rupu version cannot read"
        );
    }
    Ok(events)
}

/// Fold `events` onto each record's declared tags, in file order: `add`
/// inserts, `remove` deletes, so the last event for a tag wins. An event for
/// a finding not in `records` is ignored here (it applies once that finding
/// is read alongside it).
pub fn fold_tags(records: &mut [FindingRecord], events: &[TagEvent]) {
    if events.is_empty() {
        return;
    }
    let mut sets: Vec<BTreeSet<Tag>> = records
        .iter()
        .map(|r| r.tags.iter().cloned().collect())
        .collect();
    {
        let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in records.iter().enumerate() {
            by_id.entry(r.id.as_str()).or_default().push(i);
        }
        for e in events {
            for &i in by_id.get(e.finding_id.as_str()).into_iter().flatten() {
                match e.op {
                    TagOp::Add => {
                        sets[i].insert(e.tag.clone());
                    }
                    TagOp::Remove => {
                        sets[i].remove(&e.tag);
                    }
                }
            }
        }
    }
    for (r, s) in records.iter_mut().zip(sets) {
        r.tags = s.into_iter().collect();
    }
}

/// One finding's events, in file order.
pub fn tag_history(log: &TagLog, finding_id: &str) -> std::io::Result<Vec<TagEvent>> {
    Ok(read_tag_events(log)?
        .into_iter()
        .filter(|e| e.finding_id == finding_id)
        .collect())
}

/// Every coverage target's findings in `workspace`, as declared (no events
/// folded).
pub fn read_declared_workspace_findings(workspace: &Path) -> std::io::Result<Vec<FindingRecord>> {
    let mut out = Vec::new();
    for t in crate::ledger::discover::discover_targets(workspace)? {
        let paths = crate::ledger::paths::CoveragePaths::new(workspace, &t.target_id);
        out.extend(crate::ledger::views::read_declared_findings(&paths)?);
    }
    Ok(out)
}

/// Every coverage target's findings in `workspace`, with effective tags.
pub fn read_workspace_findings(workspace: &Path) -> std::io::Result<Vec<FindingRecord>> {
    let mut records = read_declared_workspace_findings(workspace)?;
    fold_tags(&mut records, &read_tag_events(&TagLog::for_workspace(workspace))?);
    Ok(records)
}
```

In `views.rs`, rename the existing `read_findings` to `read_declared_findings`. Keep its body, and change the first doc line to `/// Every finding record in the ledger as declared: its own tags only, no tag-log events folded in.`. Then add:

```rust
/// Every finding record in the ledger with its effective tags: declared
/// tags plus the workspace's tag log, folded (`ledger::tags`). A tag log
/// that cannot be read is warned about and the findings are returned with
/// their declared tags: an unreadable log never hides findings.
pub fn read_findings(paths: &CoveragePaths) -> std::io::Result<Vec<FindingRecord>> {
    let mut records = read_declared_findings(paths)?;
    if records.is_empty() {
        return Ok(records);
    }
    let log = paths.tag_log();
    match crate::ledger::tags::read_tag_events(&log) {
        Ok(events) => crate::ledger::tags::fold_tags(&mut records, &events),
        Err(e) => tracing::warn!(
            error = %e,
            path = %log.path.display(),
            "cannot read the finding-tag log; showing declared tags only"
        ),
    }
    Ok(records)
}
```

In `paths.rs`, add this to `impl CoveragePaths`:

```rust
    /// The workspace's tag log, carrying this path's run stream.
    pub fn tag_log(&self) -> crate::ledger::tags::TagLog {
        crate::ledger::tags::TagLog::for_workspace(&self.workspace)
            .with_run_stream(self.run_stream.clone())
    }
```

Exports:
- Extend `ledger/mod.rs`'s `pub use tags::{…}` with `fold_tags, read_declared_workspace_findings, read_tag_events, read_workspace_findings, tag_history, OperatorAttribution, OperatorSurface, TagActor, TagEvent, TagLog, TagOp, TAG_LOG_FILE`.
- Extend `pub use views::{…}` with `read_declared_findings`.
- Mirror both lists in `lib.rs`'s `pub use ledger::{…}`.

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-coverage --lib ledger::`
  - Expected: PASS.

- [ ] **Step 5: Format, lint and commit.**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): workspace finding-tag log, fold, folded reads

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `apply` — the one tag writer — plus the stream mirror and the shared lock helper

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/stream.rs` (`Ledger`, `append_record`, `stream_json`, `findings_locked`, `open_lock_file`)
- Modify: `crates/rupu-coverage/src/ledger/tags.rs`
- Create: `crates/rupu-coverage/tests/it/finding_tags.rs`; modify `crates/rupu-coverage/tests/it/main.rs`

**Interfaces:**
- Consumes: Task 2's types.
- Produces:
  - `Ledger::Tags` (`as_str` gives `"tags"`)
  - `pub(crate) fn stream_json_to(Option<&RunStream>, Ledger, &str)`
  - `pub(crate) fn locked_at<T, E: From<io::Error>>(lock_path: &Path, ledger: &Path, lock: impl FnOnce(&File) -> io::Result<()>, f: impl FnOnce() -> Result<T, E>) -> Result<T, E>`
  - `TagChange { finding_ids: Vec<String>, add: Vec<Tag>, remove: Vec<Tag> }`, with `TagChange::check(&self) -> Result<(), TagError>`
  - `TagOutcome { finding_id, before: Vec<Tag>, after: Vec<Tag> }` (Serialize), with `TagOutcome::changed()`
  - `TagError`
  - `apply(&TagLog, &TagChange, &TagActor) -> Result<Vec<TagOutcome>, TagError>`
  - `ingest_tag_events(&TagLog, Vec<TagEvent>) -> Result<(usize, usize), TagError>`, which returns (appended, duplicates)

- [ ] **Step 1: Write the failing integration tests.** Create `crates/rupu-coverage/tests/it/finding_tags.rs` and add `mod finding_tags;` to `tests/it/main.rs`:

```rust
//! `ledger::tags::apply`: the one writer of a workspace's finding-tag log.

use chrono::Utc;
use rupu_coverage::ledger::tags::{apply, TagChange, TagError, TagOutcome};
use rupu_coverage::{
    append_record, parse_tags, read_tag_events, read_workspace_findings, Attribution,
    CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope, Ledger,
    OperatorSurface, RunStream, Severity, Surface, Tag, TagActor, TagLog,
};
use std::path::Path;

fn finding(id: &str) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: None,
        line_range: None,
        target_ref: None,
        scope: FindingScope::Repo,
        summary: format!("summary of {id}"),
        severity: Severity::Medium,
        concern_id: None,
        evidence: FindingEvidence {
            code_excerpt: None,
            rationale: "r".into(),
            references: vec![],
        },
        declared_by: Attribution {
            run_id: "run_seed".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: Utc::now(),
        profile: FindingProfile::Summary,
        report: None,
        tags: vec![],
    }
}

/// `fnd_a`, `fnd_b` in target `t1`; `fnd_c` in target `t2`.
fn seed(ws: &Path) {
    let t1 = CoveragePaths::new(ws, "t1");
    let t2 = CoveragePaths::new(ws, "t2");
    append_record(&t1, Ledger::Findings, &finding("fnd_a")).unwrap();
    append_record(&t1, Ledger::Findings, &finding("fnd_b")).unwrap();
    append_record(&t2, Ledger::Findings, &finding("fnd_c")).unwrap();
}

fn tags(raw: &[&str]) -> Vec<Tag> {
    parse_tags(raw).unwrap()
}

fn change(ids: &[&str], add: &[&str], remove: &[&str]) -> TagChange {
    TagChange {
        finding_ids: ids.iter().map(|s| s.to_string()).collect(),
        add: tags(add),
        remove: tags(remove),
    }
}

fn by() -> TagActor {
    TagActor::operator(OperatorSurface::Cli)
}

fn tags_of(ws: &Path, id: &str) -> Vec<String> {
    read_workspace_findings(ws)
        .unwrap()
        .into_iter()
        .find(|r| r.id == id)
        .unwrap()
        .tags
        .into_iter()
        .map(String::from)
        .collect()
}

#[test]
fn apply_tags_findings_across_targets_and_reports_before_and_after() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    let out = apply(&log, &change(&["fnd_c", "fnd_a"], &["class:sqli", "needs-poc"], &[]), &by())
        .unwrap();
    assert_eq!(
        out,
        vec![
            TagOutcome { finding_id: "fnd_a".into(), before: vec![], after: tags(&["class:sqli", "needs-poc"]) },
            TagOutcome { finding_id: "fnd_c".into(), before: vec![], after: tags(&["class:sqli", "needs-poc"]) },
        ]
    );
    assert_eq!(tags_of(ws.path(), "fnd_c"), ["class:sqli", "needs-poc"]);
    assert!(tags_of(ws.path(), "fnd_b").is_empty());
    let events = read_tag_events(&log).unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| matches!(e.by, TagActor::Operator(_))));
}

#[test]
fn a_change_that_changes_nothing_writes_nothing() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap();
    let before = std::fs::read(&log.path).unwrap();
    let out = apply(&log, &change(&["fnd_a"], &["x"], &["never-had"]), &by()).unwrap();
    assert!(!out[0].changed());
    assert_eq!(std::fs::read(&log.path).unwrap(), before);
}

#[test]
fn remove_then_readd_is_recorded_and_folds_in_order() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    apply(&log, &change(&["fnd_a"], &["x", "y"], &[]), &by()).unwrap();
    apply(&log, &change(&["fnd_a"], &[], &["x"]), &by()).unwrap();
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["y"]);
    apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap();
    assert_eq!(tags_of(ws.path(), "fnd_a"), ["x", "y"]);
}

#[test]
fn an_unknown_id_rejects_the_whole_batch_and_leaves_the_log_untouched() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    apply(&log, &change(&["fnd_a"], &["seed"], &[]), &by()).unwrap();
    let before = std::fs::read(&log.path).unwrap();
    let err = apply(&log, &change(&["fnd_a", "fnd_nope"], &["x"], &[]), &by()).unwrap_err();
    assert!(matches!(&err, TagError::UnknownFindings(ids) if ids == &["fnd_nope".to_string()]), "{err}");
    assert_eq!(std::fs::read(&log.path).unwrap(), before);
}

#[test]
fn invalid_changes_are_refused_before_anything_is_read() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    assert!(matches!(
        apply(&log, &change(&["fnd_a"], &[], &[]), &by()),
        Err(TagError::EmptyChange)
    ));
    assert!(matches!(
        apply(&log, &change(&[], &["x"], &[]), &by()),
        Err(TagError::NoFindings)
    ));
    assert!(matches!(
        apply(&log, &change(&["fnd_a"], &["x", "y"], &["y"]), &by()),
        Err(TagError::Conflict(t)) if t == tags(&["y"])
    ));
    assert!(!log.path.exists());
}

#[test]
fn the_tag_cap_is_enforced_per_finding() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let log = TagLog::for_workspace(ws.path());
    let many: Vec<String> = (0..32).map(|i| format!("t{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    apply(&log, &change(&["fnd_a"], &many, &[]), &by()).unwrap();
    let err = apply(&log, &change(&["fnd_a"], &["one-more"], &[]), &by()).unwrap_err();
    assert!(matches!(err, TagError::TooManyTags { count: 33, .. }), "{err}");
}

#[test]
fn a_workspace_without_coverage_has_no_findings_and_gets_no_files() {
    let ws = tempfile::TempDir::new().unwrap();
    let log = TagLog::for_workspace(ws.path());
    let err = apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap_err();
    assert!(matches!(err, TagError::UnknownFindings(_)));
    assert!(!ws.path().join(".rupu").exists());
}

#[test]
fn events_are_mirrored_to_the_run_stream() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let stream = ws.path().join("runs/run_1/coverage.jsonl");
    rupu_coverage::write_stream_begin(&stream, "run_1").unwrap();
    let log = TagLog::for_workspace(ws.path()).with_run_stream(Some(RunStream {
        path: stream.clone(),
        scope_name: "tagger".into(),
    }));
    apply(&log, &change(&["fnd_a"], &["x"], &[]), &by()).unwrap();
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&stream)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tag_line = lines.iter().find(|l| l["ledger"] == "tags").expect("a tags line");
    assert_eq!(tag_line["scope_name"], "tagger");
    assert_eq!(tag_line["record"]["finding_id"], "fnd_a");
    assert_eq!(tag_line["record"]["op"], "add");
}

#[test]
fn concurrent_writers_lose_no_events() {
    let ws = tempfile::TempDir::new().unwrap();
    seed(ws.path());
    let root = ws.path().to_path_buf();
    let writers: Vec<_> = (0..2)
        .map(|w| {
            let root = root.clone();
            std::thread::spawn(move || {
                let log = TagLog::for_workspace(&root);
                for i in 0..20 {
                    let tag = format!("w{w}-{i}");
                    // Stay under the per-finding cap: writer 0 tags fnd_a, writer 1 fnd_b.
                    let id = if w == 0 { "fnd_a" } else { "fnd_b" };
                    let ch = TagChange {
                        finding_ids: vec![id.to_string()],
                        add: vec![Tag::parse(&tag).unwrap()],
                        remove: if i > 0 { vec![Tag::parse(&format!("w{w}-{}", i - 1)).unwrap()] } else { vec![] },
                    };
                    apply(&log, &ch, &TagActor::operator(OperatorSurface::Cli)).unwrap();
                }
            })
        })
        .collect();
    let appender = {
        let root = root.clone();
        std::thread::spawn(move || {
            let t3 = CoveragePaths::new(&root, "t3");
            for i in 0..20 {
                append_record(&t3, Ledger::Findings, &finding(&format!("fnd_new{i}"))).unwrap();
            }
        })
    };
    for h in writers {
        h.join().unwrap();
    }
    appender.join().unwrap();
    let log = TagLog::for_workspace(&root);
    // 1 add for the first iteration, then 1 remove + 1 add for each of the other 19, per writer.
    assert_eq!(read_tag_events(&log).unwrap().len(), 2 * (1 + 19 * 2));
    let text = std::fs::read_to_string(&log.path).unwrap();
    assert!(text.lines().all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()));
    assert_eq!(tags_of(&root, "fnd_a"), ["w0-19"]);
    assert_eq!(tags_of(&root, "fnd_b"), ["w1-19"]);
}
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-coverage --test it finding_tags::`
  - Expected: a compile error (`apply`, `TagChange` and the rest are undefined).

- [ ] **Step 3: Generalize the lock and the stream helpers in `stream.rs`.**
  - Add `Tags` to `enum Ledger`, with the doc comment `/// Finding-tag events (the workspace's \`finding_tags.jsonl\`), written only by \`ledger::tags\`.`, and `Ledger::Tags => "tags"` in `as_str`.
  - In `append_record`'s `match ledger`, add:

```rust
        Ledger::Tags => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "finding-tag events are written by ledger::tags::apply, not append_record",
            ))
        }
```

  - Replace `stream_json` with:

```rust
pub(crate) fn stream_json(paths: &CoveragePaths, ledger: Ledger, record_json: &str) {
    stream_json_to(paths.run_stream.as_ref(), ledger, record_json)
}

/// [`stream_json`] for a writer that is not a `CoveragePaths` (the tag log).
pub(crate) fn stream_json_to(run_stream: Option<&RunStream>, ledger: Ledger, record_json: &str) {
    let Some(rs) = run_stream else {
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
```

  - Generalize the lock. Keep `findings_locked`'s doc comment, but change its body to delegate, and add `locked_at` and `open_lock_at`:

```rust
fn findings_locked(
    paths: &CoveragePaths,
    lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    append: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    locked_at(
        &paths.root.join("findings.jsonl.lock"),
        &paths.findings,
        lock,
        append,
    )
}

/// Run `f` holding the sidecar lock at `lock_path`, taken with `lock`, with
/// the fallback [`findings_locked`] documents: where the filesystem cannot
/// lock, or the lock file cannot be opened, `f` runs unlocked and a warning
/// names `ledger`; any other lock failure is an error and `f` does not run.
pub(crate) fn locked_at<T, E: From<std::io::Error>>(
    lock_path: &Path,
    ledger: &Path,
    lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    f: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let lock_file = match open_lock_at(lock_path) {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::warn!(
                error = %e,
                ledger = ?ledger,
                "cannot open the ledger's lock file; writing without the lock"
            );
            None
        }
    };
    if let Some((Err(e), read_only)) = lock_file.as_ref().map(|(f, ro)| (lock(f), *ro)) {
        let needs_write =
            read_only && e.raw_os_error() == Some(rustix::io::Errno::BADF.raw_os_error());
        if !(lock_unsupported(&e) || needs_write) {
            return Err(e.into());
        }
        tracing::warn!(
            error = %e,
            ledger = ?ledger,
            "this filesystem cannot lock the ledger; writing without the lock"
        );
    }
    let out = f();
    // Released after the write.
    drop(lock_file);
    out
}
```

  - Replace `open_lock_file` with `open_lock_at`, and keep `open_lock_file` delegating:

```rust
fn open_lock_file(paths: &CoveragePaths) -> std::io::Result<(std::fs::File, bool)> {
    open_lock_at(&paths.root.join("findings.jsonl.lock"))
}

/// The lock sidecar at `path` (its directory created if missing), and
/// whether it was opened read-only.
///
/// A lock needs only a handle on the file on most filesystems, so one this
/// user may not write (created by another user, say an import run with
/// `sudo`) is opened read-only instead.
fn open_lock_at(path: &Path) -> std::io::Result<(std::fs::File, bool)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    {
        Ok(f) => Ok((f, false)),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            std::fs::File::open(path).map(|f| (f, true)).map_err(|_| e)
        }
        Err(e) => Err(e),
    }
}
```

  - Run `cargo test -p rupu-coverage --lib ledger::stream`. Expected: PASS. Every existing lock test still holds.

- [ ] **Step 4: Implement `apply` and `ingest_tag_events` in `tags.rs`.**

```rust
/// One requested change: add and remove tags on a set of findings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagChange {
    pub finding_ids: Vec<String>,
    pub add: Vec<Tag>,
    pub remove: Vec<Tag>,
}

impl TagChange {
    /// The checks that need no I/O: something to change, some findings,
    /// and no tag both added and removed.
    pub fn check(&self) -> Result<(), TagError> {
        if self.add.is_empty() && self.remove.is_empty() {
            return Err(TagError::EmptyChange);
        }
        if self.finding_ids.iter().all(|id| id.trim().is_empty()) {
            return Err(TagError::NoFindings);
        }
        let add: BTreeSet<&Tag> = self.add.iter().collect();
        let conflict: Vec<Tag> = self
            .remove
            .iter()
            .filter(|t| add.contains(t))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !conflict.is_empty() {
            return Err(TagError::Conflict(conflict));
        }
        Ok(())
    }
}

/// One finding's tags before and after a change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagOutcome {
    pub finding_id: String,
    pub before: Vec<Tag>,
    pub after: Vec<Tag>,
}

impl TagOutcome {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("nothing to change: give at least one tag to add or remove")]
    EmptyChange,
    #[error("no finding ids given")]
    NoFindings,
    #[error("a tag cannot be both added and removed: {}", .0.iter().map(Tag::as_str).collect::<Vec<_>>().join(", "))]
    Conflict(Vec<Tag>),
    #[error("unknown finding id(s): {}", .0.join(", "))]
    UnknownFindings(Vec<String>),
    #[error("{finding_id} would have {count} tags; at most {MAX_TAGS_PER_FINDING} are allowed")]
    TooManyTags { finding_id: String, count: usize },
    #[error("finding-tag log I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("finding-tag log encode: {0}")]
    Encode(#[from] serde_json::Error),
}

/// Apply `change` to the findings of `log`'s workspace. The ONLY writer of
/// tag events (spec "Write path").
///
/// Under the log's sidecar lock: read every target's declared findings and
/// the log, fold, refuse the whole batch if any id is unknown or a finding
/// would exceed [`MAX_TAGS_PER_FINDING`], then append only the events that
/// change something (so a repeated request writes nothing) in one write,
/// fsynced, and mirror them to the log's run stream. Findings are read
/// without their own lock: their ledger is append-only and an import swaps
/// it by atomic rename, so a read always sees a whole file and an id never
/// disappears. Outcomes are sorted by finding id.
pub fn apply(log: &TagLog, change: &TagChange, by: &TagActor) -> Result<Vec<TagOutcome>, TagError> {
    change.check()?;
    let ids: Vec<String> = change
        .finding_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let add: BTreeSet<Tag> = change.add.iter().cloned().collect();
    let remove: BTreeSet<Tag> = change.remove.iter().cloned().collect();
    // No coverage directory: no findings here, and nothing is created.
    if !log.path.parent().is_some_and(Path::is_dir) {
        return Err(TagError::UnknownFindings(ids));
    }
    crate::ledger::stream::locked_at(&log.lock, &log.path, std::fs::File::lock, || {
        let mut records = read_declared_workspace_findings(&log.workspace)?;
        fold_tags(&mut records, &read_tag_events(log)?);
        let current: HashMap<&str, BTreeSet<Tag>> = records
            .iter()
            .map(|r| (r.id.as_str(), r.tags.iter().cloned().collect()))
            .collect();
        let unknown: Vec<String> = ids
            .iter()
            .filter(|id| !current.contains_key(id.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(TagError::UnknownFindings(unknown));
        }
        let at = Utc::now();
        let mut events = Vec::new();
        let mut outcomes = Vec::with_capacity(ids.len());
        for id in &ids {
            let before = current[id.as_str()].clone();
            let mut after = before.clone();
            for t in &remove {
                if after.remove(t) {
                    events.push(new_event(id, TagOp::Remove, t, by, at));
                }
            }
            for t in &add {
                if after.insert(t.clone()) {
                    events.push(new_event(id, TagOp::Add, t, by, at));
                }
            }
            if after.len() > MAX_TAGS_PER_FINDING {
                return Err(TagError::TooManyTags {
                    finding_id: id.clone(),
                    count: after.len(),
                });
            }
            outcomes.push(TagOutcome {
                finding_id: id.clone(),
                before: before.into_iter().collect(),
                after: after.into_iter().collect(),
            });
        }
        append_events(log, &events)?;
        Ok(outcomes)
    })
}

fn new_event(finding_id: &str, op: TagOp, tag: &Tag, by: &TagActor, at: DateTime<Utc>) -> TagEvent {
    TagEvent {
        id: format!("tge_{}", ulid::Ulid::new()),
        finding_id: finding_id.to_string(),
        op,
        tag: tag.clone(),
        by: by.clone(),
        at,
    }
}

/// Append `events` in ONE write (a concurrent unlocked appender could
/// otherwise land between two of them), fsync, then mirror each to the run
/// stream. The caller holds the log's lock.
fn append_events(log: &TagLog, events: &[TagEvent]) -> Result<(), TagError> {
    use std::io::Write;
    if events.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = events
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<_, _>>()?;
    let mut buf = lines.join("\n");
    buf.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log.path)?;
    f.write_all(buf.as_bytes())?;
    f.sync_data()?;
    for l in &lines {
        crate::ledger::stream::stream_json_to(
            log.run_stream.as_ref(),
            crate::ledger::stream::Ledger::Tags,
            l,
        );
    }
    Ok(())
}

/// Append a remote unit's events (`ingest_unit_stream`) that `log` does not
/// already hold, by event id, under the log's lock so two ingests cannot
/// both append one event. Never validated against findings: an orphan is
/// folded away until its finding arrives. Returns (appended, duplicates).
pub fn ingest_tag_events(log: &TagLog, events: Vec<TagEvent>) -> Result<(usize, usize), TagError> {
    if events.is_empty() {
        return Ok((0, 0));
    }
    crate::ledger::stream::locked_at(&log.lock, &log.path, std::fs::File::lock, || {
        let mut seen: std::collections::HashSet<String> =
            read_tag_events(log)?.into_iter().map(|e| e.id).collect();
        let total = events.len();
        let fresh: Vec<TagEvent> = events.into_iter().filter(|e| seen.insert(e.id.clone())).collect();
        let appended = fresh.len();
        append_events(log, &fresh)?;
        Ok((appended, total - appended))
    })
}
```

  - Export `apply, ingest_tag_events, TagChange, TagError, TagOutcome` from `ledger/mod.rs` and `lib.rs`.
  - `lib.rs` already re-exports `Ledger`, `RunStream` and `write_stream_begin`. Check that the test's `use` list compiles; if any name is missing, add it to the re-exports rather than changing the test.

- [ ] **Step 5: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-coverage --test it finding_tags::` and `cargo test -p rupu-coverage --lib ledger::`
  - Expected: PASS.

- [ ] **Step 6: Format, lint and commit.**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): ledger::tags::apply, the one finding-tag writer

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `tags` stream lines and remote-unit ingest

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/stream.rs` (`StreamLine`), `crates/rupu-coverage/src/ledger/ingest.rs`

**Interfaces:**
- Consumes: `TagEvent`, `TagLog`, `ingest_tag_events` (Task 3).
- Produces: `StreamLine::Tags { scope_name: String, record: TagEvent }`. `ingest_unit_stream` merges these lines into the workspace log, and `IngestReport.appended` / `duplicates` count them.

- [ ] **Step 1: Write the failing tests.** Append to the `mod tests` in `ingest.rs`:

```rust
    fn tag_event(id: &str, finding_id: &str, tag: &str) -> crate::ledger::tags::TagEvent {
        crate::ledger::tags::TagEvent {
            id: id.into(),
            finding_id: finding_id.into(),
            op: crate::ledger::tags::TagOp::Add,
            tag: crate::ledger::tags::Tag::parse(tag).unwrap(),
            by: crate::ledger::tags::TagActor::Operator(crate::ledger::tags::OperatorAttribution {
                user: "u".into(),
                via: crate::ledger::tags::OperatorSurface::Cli,
            }),
            at: Utc::now(),
        }
    }

    fn tagged_stream() -> Vec<u8> {
        [
            serde_json::to_string(&StreamLine::Begin { v: STREAM_VERSION, run_id: "r".into() }).unwrap(),
            serde_json::to_string(&StreamLine::Findings { scope_name: "unit".into(), record: finding("fnd_1", false) }).unwrap(),
            // Tagged under a different scope than its finding: tags route by workspace.
            serde_json::to_string(&StreamLine::Tags { scope_name: "other-agent".into(), record: tag_event("tge_1", "fnd_1", "needs-poc") }).unwrap(),
            serde_json::to_string(&StreamLine::Tags { scope_name: "other-agent".into(), record: tag_event("tge_2", "fnd_orphan", "x") }).unwrap(),
        ]
        .join("\n")
        .into_bytes()
    }

    #[test]
    fn tag_lines_merge_into_the_workspace_log_once() {
        let ws = tempfile::TempDir::new().unwrap();
        let r1 = ingest_unit_stream(ws.path(), &IngestSource::default(), &tagged_stream()).unwrap();
        assert_eq!(r1.appended, 3);
        assert_eq!(r1.malformed, 0);
        let all = crate::ledger::tags::read_workspace_findings(ws.path()).unwrap();
        let f = all.iter().find(|r| r.id == "fnd_1").unwrap();
        assert_eq!(f.tags, vec![crate::ledger::tags::Tag::parse("needs-poc").unwrap()]);

        let log = crate::ledger::tags::TagLog::for_workspace(ws.path());
        let before = std::fs::read(&log.path).unwrap();
        let r2 = ingest_unit_stream(ws.path(), &IngestSource::default(), &tagged_stream()).unwrap();
        assert_eq!(r2.appended, 0);
        assert_eq!(r2.duplicates, 3);
        assert_eq!(std::fs::read(&log.path).unwrap(), before);
        // The orphan is kept, waiting for its finding.
        assert_eq!(crate::ledger::tags::read_tag_events(&log).unwrap().len(), 2);
    }
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-coverage --lib ledger::ingest`
  - Expected: a compile error (no `StreamLine::Tags`).

- [ ] **Step 3: Add the variant.** In `StreamLine`, after `Assets`:

```rust
    /// A finding-tag event. Routed by workspace, not by `scope_name`: the
    /// tag log is workspace-wide (`ledger::tags`).
    Tags {
        scope_name: String,
        record: crate::ledger::tags::TagEvent,
    },
```

- [ ] **Step 4: Merge the lines in `ingest_unit_stream`.**
  - Add `#[error("coverage ingest tag log: {0}")] Tags(#[from] crate::ledger::tags::TagError),` to `IngestError`.
  - Make these changes in the loop:

```rust
    let mut tag_events = Vec::new();
    let mut targets: BTreeMap<String, (CoveragePaths, Seen)> = BTreeMap::new();
    for line in lines {
        let line = match line {
            StreamLine::Tags { record, .. } => {
                tag_events.push(record);
                continue;
            }
            other => other,
        };
        let scope = match &line {
            StreamLine::Begin { .. } | StreamLine::Tags { .. } => continue,
            // … existing arms unchanged …
        };
        // … unchanged …
        let fresh = match line {
            StreamLine::Begin { .. } | StreamLine::Tags { .. } => continue,
            // … existing arms unchanged …
        };
        // … unchanged …
    }
    let (appended, duplicates) = crate::ledger::tags::ingest_tag_events(
        &crate::ledger::tags::TagLog::for_workspace(workspace),
        tag_events,
    )?;
    report.appended += appended;
    report.duplicates += duplicates;
    Ok(report)
```

  - Update the module doc comment's dedup sentence to add: `tag events by event id, into the workspace-wide tag log whatever their scope`.

- [ ] **Step 5: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-coverage --lib ledger::`
  - Then: `cargo check --workspace --all-targets`. Any other exhaustive `match` on `StreamLine` must still compile; add a `StreamLine::Tags { .. }` arm wherever the compiler reports one.
  - Then: `cargo test -p rupu-orchestrator --test it remote_coverage_ingest::`
  - Expected: PASS.

- [ ] **Step 6: Format, lint and commit.**

```bash
git add crates/rupu-coverage crates/rupu-orchestrator crates/rupu-cli crates/rupu-cp
git commit -m "feat(coverage): stream finding-tag events and merge them on remote ingest

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `ledger::query` — select, paged query, tags in use

**Files:**
- Create: `crates/rupu-coverage/src/ledger/query.rs`
- Modify: `crates/rupu-coverage/src/ledger/mod.rs`, `crates/rupu-coverage/src/lib.rs`, `crates/rupu-coverage/src/catalog/types.rs` (`Severity::as_str`)

**Interfaces:**
- Consumes: `FindingRecord.tags` and `Tag`.
- Produces (all re-exported from `rupu_coverage`):
  - `TagMode { All, Any }`
  - `FindingQuery { tags, tag_mode, untagged, min_severity, concern_id, file_prefix, limit, cursor, run_ids }`. It derives `Deserialize` with `deny_unknown_fields`; `run_ids` is `#[serde(skip)]`.
  - `QueryError`
  - `select<'a, T>(&'a [T], impl Fn(&T) -> &FindingRecord, &FindingQuery) -> Result<Vec<&'a T>, QueryError>`
  - `query(&[FindingRecord], &FindingQuery) -> Result<Page, QueryError>`
  - `Page { rows: Vec<FindingRow>, next_cursor: Option<String>, total: usize }`
  - `FindingRow { id, title, severity, scope, location, concern_id, tags, declared_at, run_id }` (Serialize, and `From<&FindingRecord>`)
  - `TagCount { tag, count }`, and `tags_in_use<'a>(impl IntoIterator<Item = &'a FindingRecord>) -> Vec<TagCount>`
  - `query_response(&[FindingRecord], &FindingQuery) -> Result<serde_json::Value, QueryError>`
  - `query_input_schema() -> serde_json::Value`
  - `severity_rank(Severity) -> u8`, `DEFAULT_LIMIT = 50`, `MAX_LIMIT = 500`
  - `Severity::as_str(self) -> &'static str`

- [ ] **Step 1: Write the failing tests.** Create `query.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, FindingEvidence, Surface};
    use crate::ledger::tags::parse_tags;
    use chrono::TimeZone;

    fn rec(id: &str, sev: Severity, minute: u32, tags: &[&str]) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: Some(format!("src/{id}.rs")),
            line_range: Some([3, 4]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: format!("summary {id}"),
            severity: sev,
            concern_id: Some("injection".into()),
            evidence: FindingEvidence { code_excerpt: None, rationale: "r".into(), references: vec![] },
            declared_by: Attribution {
                run_id: format!("run_{id}"),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc.with_ymd_and_hms(2026, 10, 6, 12, minute, 0).unwrap(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
            tags: parse_tags(tags).unwrap(),
        }
    }

    fn fixture() -> Vec<FindingRecord> {
        vec![
            rec("fnd_low", Severity::Low, 1, &["class:sqli"]),
            rec("fnd_crit", Severity::Critical, 2, &["class:sqli", "needs-poc"]),
            rec("fnd_high_old", Severity::High, 3, &[]),
            rec("fnd_high_new", Severity::High, 9, &["needs-poc"]),
        ]
    }

    fn q() -> FindingQuery {
        FindingQuery::default()
    }

    fn ids(v: &[&FindingRecord]) -> Vec<String> {
        v.iter().map(|r| r.id.clone()).collect()
    }

    #[test]
    fn select_orders_by_severity_then_newest_first() {
        let f = fixture();
        let all = select(&f, |r| r, &q()).unwrap();
        assert_eq!(ids(&all), ["fnd_crit", "fnd_high_new", "fnd_high_old", "fnd_low"]);
    }

    #[test]
    fn tag_filters_all_any_and_untagged() {
        let f = fixture();
        let both = FindingQuery { tags: parse_tags(&["class:sqli", "needs-poc"]).unwrap(), ..q() };
        assert_eq!(ids(&select(&f, |r| r, &both).unwrap()), ["fnd_crit"]);
        let any = FindingQuery { tag_mode: TagMode::Any, ..both.clone() };
        assert_eq!(ids(&select(&f, |r| r, &any).unwrap()), ["fnd_crit", "fnd_high_new", "fnd_low"]);
        let untagged = FindingQuery { untagged: true, ..q() };
        assert_eq!(ids(&select(&f, |r| r, &untagged).unwrap()), ["fnd_high_old"]);
        let bad = FindingQuery { untagged: true, ..both };
        assert_eq!(select(&f, |r| r, &bad).unwrap_err(), QueryError::UntaggedWithTags);
    }

    #[test]
    fn other_filters_narrow() {
        let f = fixture();
        let high_up = FindingQuery { min_severity: Some(Severity::High), ..q() };
        assert_eq!(select(&f, |r| r, &high_up).unwrap().len(), 3);
        let file = FindingQuery { file_prefix: Some("src/fnd_low".into()), ..q() };
        assert_eq!(ids(&select(&f, |r| r, &file).unwrap()), ["fnd_low"]);
        let run = FindingQuery { run_ids: Some(["run_fnd_crit".to_string()].into()), ..q() };
        assert_eq!(ids(&select(&f, |r| r, &run).unwrap()), ["fnd_crit"]);
        let concern = FindingQuery { concern_id: Some("other".into()), ..q() };
        assert!(select(&f, |r| r, &concern).unwrap().is_empty());
    }

    #[test]
    fn pages_follow_the_cursor_and_stay_stable_under_appends() {
        let mut f = fixture();
        let p1 = query(&f, &FindingQuery { limit: Some(2), ..q() }).unwrap();
        assert_eq!(p1.total, 4);
        let p1_ids: Vec<&str> = p1.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(p1_ids, ["fnd_crit", "fnd_high_new"]);
        // A newer critical finding lands while paging: it sorts before the
        // cursor, so the next page neither repeats nor skips a row.
        f.push(rec("fnd_crit_new", Severity::Critical, 30, &[]));
        let p2 = query(&f, &FindingQuery { limit: Some(2), cursor: p1.next_cursor.clone(), ..q() }).unwrap();
        let p2_ids: Vec<&str> = p2.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(p2_ids, ["fnd_high_old", "fnd_low"]);
        assert_eq!(p2.next_cursor, None);
    }

    #[test]
    fn limits_and_cursors_are_validated() {
        let f = fixture();
        assert_eq!(query(&f, &FindingQuery { limit: Some(0), ..q() }).unwrap_err(), QueryError::Limit(0));
        assert_eq!(query(&f, &FindingQuery { limit: Some(501), ..q() }).unwrap_err(), QueryError::Limit(501));
        assert!(matches!(
            query(&f, &FindingQuery { cursor: Some("garbage".into()), ..q() }),
            Err(QueryError::Cursor(_))
        ));
        assert_eq!(query(&f, &q()).unwrap().rows.len(), 4);
    }

    #[test]
    fn rows_are_slim_and_located() {
        let f = fixture();
        let row = FindingRow::from(&f[0]);
        assert_eq!(row.title, "summary fnd_low");
        assert_eq!(row.location.as_deref(), Some("src/fnd_low.rs:3-4"));
        assert_eq!(row.run_id, "run_fnd_low");
    }

    #[test]
    fn tags_in_use_counts_by_count_then_name() {
        let f = fixture();
        let counts = tags_in_use(&f);
        let got: Vec<(&str, usize)> = counts.iter().map(|c| (c.tag.as_str(), c.count)).collect();
        assert_eq!(got, [("class:sqli", 2), ("needs-poc", 2)]);
    }

    #[test]
    fn query_input_rejects_unknown_fields() {
        let ok: FindingQuery = serde_json::from_value(serde_json::json!({"tags": ["Needs-POC"], "tag_mode": "any"})).unwrap();
        assert_eq!(ok.tags[0].as_str(), "needs-poc");
        assert!(serde_json::from_value::<FindingQuery>(serde_json::json!({"tag": ["x"]})).is_err());
        assert!(serde_json::from_value::<FindingQuery>(serde_json::json!({"run_ids": ["x"]})).is_err());
    }
}
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-coverage --lib ledger::query`
  - Expected: a compile error.

- [ ] **Step 3: Implement.**
  - In `catalog/types.rs`, add:

```rust
impl Severity {
    /// The wire name (`critical`, `high`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}
```

  - Then put this above the test module in `query.rs`:

```rust
//! The one finding filter (spec "Read path"): [`select`] returns every
//! match, for the CLI and CP; [`query`] pages them by cursor, for the agent
//! tool and MCP. Never a silent cap: a page says how many matched.

use crate::catalog::types::Severity;
use crate::ledger::events::{FindingRecord, FindingScope};
use crate::ledger::tags::Tag;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagMode {
    #[default]
    All,
    Any,
}

/// What to select. Deserialized from agent/MCP tool input, so unknown
/// fields are refused (a typo must not silently widen the selection).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingQuery {
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub tag_mode: TagMode,
    #[serde(default)]
    pub untagged: bool,
    #[serde(default)]
    pub min_severity: Option<Severity>,
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub file_prefix: Option<String>,
    /// [`query`] only.
    #[serde(default)]
    pub limit: Option<usize>,
    /// [`query`] only: a previous page's `next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Only findings declared by one of these runs. Set by the CLI/CP from a
    /// resolved run scope; never tool input.
    #[serde(skip)]
    pub run_ids: Option<HashSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("`untagged` cannot be combined with `tags`")]
    UntaggedWithTags,
    #[error("`limit` must be between 1 and {MAX_LIMIT}, got {0}")]
    Limit(usize),
    #[error("`cursor` is not one this query returned: {0}")]
    Cursor(String),
}

/// Sort rank: critical (4) first.
pub fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Critical => 4,
        Severity::High => 3,
        Severity::Medium => 2,
        Severity::Low => 1,
        Severity::Info => 0,
    }
}

fn matches(r: &FindingRecord, q: &FindingQuery) -> bool {
    if q.untagged && !r.tags.is_empty() {
        return false;
    }
    if !q.tags.is_empty() {
        let has = |t: &Tag| r.tags.contains(t);
        let ok = match q.tag_mode {
            TagMode::All => q.tags.iter().all(has),
            TagMode::Any => q.tags.iter().any(has),
        };
        if !ok {
            return false;
        }
    }
    if q.min_severity.is_some_and(|min| severity_rank(r.severity) < severity_rank(min)) {
        return false;
    }
    if q.concern_id.as_deref().is_some_and(|c| r.concern_id.as_deref() != Some(c)) {
        return false;
    }
    if let Some(prefix) = q.file_prefix.as_deref() {
        if !r.file_path.as_deref().is_some_and(|f| f.starts_with(prefix)) {
            return false;
        }
    }
    if let Some(runs) = &q.run_ids {
        if !runs.contains(&r.declared_by.run_id) {
            return false;
        }
    }
    true
}

type SortKey = (Reverse<u8>, Reverse<DateTime<Utc>>, String);

fn sort_key(r: &FindingRecord) -> SortKey {
    (Reverse(severity_rank(r.severity)), Reverse(r.declared_at), r.id.clone())
}

/// Every item whose record matches, ordered severity → newest → id.
pub fn select<'a, T>(
    items: &'a [T],
    record_of: impl Fn(&T) -> &FindingRecord,
    q: &FindingQuery,
) -> Result<Vec<&'a T>, QueryError> {
    if q.untagged && !q.tags.is_empty() {
        return Err(QueryError::UntaggedWithTags);
    }
    // `filter` and `sort_by_cached_key` hand the closures `&&T`: deref once.
    let mut out: Vec<&T> = items.iter().filter(|i| matches(record_of(*i), q)).collect();
    out.sort_by_cached_key(|i| sort_key(record_of(*i)));
    Ok(out)
}

/// A slim row: what an agent or a table needs, never the report body.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingRow {
    pub id: String,
    /// The report's title on a full-profile finding, else its summary.
    pub title: String,
    pub severity: Severity,
    pub scope: FindingScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concern_id: Option<String>,
    pub tags: Vec<Tag>,
    pub declared_at: DateTime<Utc>,
    pub run_id: String,
}

impl From<&FindingRecord> for FindingRow {
    fn from(r: &FindingRecord) -> Self {
        let location = match (&r.file_path, r.line_range) {
            (Some(f), Some([a, b])) => Some(format!("{f}:{a}-{b}")),
            (Some(f), None) => Some(f.clone()),
            (None, _) => r.target_ref.clone(),
        };
        FindingRow {
            id: r.id.clone(),
            title: r.report.as_ref().map(|rep| rep.title.clone()).unwrap_or_else(|| r.summary.clone()),
            severity: r.severity,
            scope: r.scope,
            location,
            concern_id: r.concern_id.clone(),
            tags: r.tags.clone(),
            declared_at: r.declared_at,
            run_id: r.declared_by.run_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page {
    pub rows: Vec<FindingRow>,
    /// Pass back as `cursor` for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
    /// Matches across all pages.
    pub total: usize,
}

fn cursor_of(r: &FindingRecord) -> String {
    format!(
        "{}|{}|{}",
        severity_rank(r.severity),
        r.declared_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        r.id
    )
}

fn parse_cursor(c: &str) -> Result<SortKey, QueryError> {
    let bad = || QueryError::Cursor(c.to_string());
    let mut parts = c.splitn(3, '|');
    let rank: u8 = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
    let at = parts
        .next()
        .and_then(|p| DateTime::parse_from_rfc3339(p).ok())
        .ok_or_else(bad)?
        .with_timezone(&Utc);
    let id = parts.next().filter(|p| !p.is_empty()).ok_or_else(bad)?;
    Ok((Reverse(rank), Reverse(at), id.to_string()))
}

/// One page of matches, after `q.cursor`. A cursor is a sort key, not an
/// offset, so findings appended while paging never shift or repeat rows.
pub fn query(records: &[FindingRecord], q: &FindingQuery) -> Result<Page, QueryError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(QueryError::Limit(limit));
    }
    let all = select(records, |r| r, q)?;
    let total = all.len();
    let start = match q.cursor.as_deref() {
        None => 0,
        Some(c) => {
            let key = parse_cursor(c)?;
            all.partition_point(|r| sort_key(r) <= key)
        }
    };
    let page: Vec<&FindingRecord> = all[start..].iter().take(limit).copied().collect();
    let next_cursor = if start + page.len() < total {
        page.last().map(|r| cursor_of(r))
    } else {
        None
    };
    Ok(Page {
        rows: page.into_iter().map(FindingRow::from).collect(),
        next_cursor,
        total,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagCount {
    pub tag: Tag,
    pub count: usize,
}

/// Each tag on `records` and how many carry it, most used first.
pub fn tags_in_use<'a>(records: impl IntoIterator<Item = &'a FindingRecord>) -> Vec<TagCount> {
    let mut counts: BTreeMap<&Tag, usize> = BTreeMap::new();
    for r in records {
        for t in &r.tags {
            *counts.entry(t).or_default() += 1;
        }
    }
    let mut v: Vec<TagCount> = counts
        .into_iter()
        .map(|(tag, count)| TagCount { tag: tag.clone(), count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    v
}

/// The agent tool's and MCP's answer: one page plus the vocabulary in use
/// across `records` (the whole workspace, not just the matches).
pub fn query_response(records: &[FindingRecord], q: &FindingQuery) -> Result<serde_json::Value, QueryError> {
    let page = query(records, q)?;
    Ok(serde_json::json!({
        "rows": page.rows,
        "next_cursor": page.next_cursor,
        "total": page.total,
        "tags_in_use": tags_in_use(records),
    }))
}

/// Input schema shared by `query_findings` and `findings.query`.
pub fn query_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tags": { "type": "array", "items": { "type": "string" }, "description": "Only findings carrying these tags." },
            "tag_mode": { "type": "string", "enum": ["all", "any"], "description": "With `tags`: findings carrying all of them (default) or any of them." },
            "untagged": { "type": "boolean", "description": "Only findings with no tags. Cannot be combined with `tags`." },
            "min_severity": { "type": "string", "enum": ["info", "low", "medium", "high", "critical"], "description": "Only this severity and worse." },
            "concern_id": { "type": "string", "description": "Only findings for this concern id." },
            "file_prefix": { "type": "string", "description": "Only findings whose file_path starts with this." },
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Rows per page (default 50)." },
            "cursor": { "type": "string", "description": "`next_cursor` from the previous page." }
        }
    })
}
```

  - Add `pub mod query;` and `pub use query::{query, query_input_schema, query_response, select, severity_rank, tags_in_use, FindingQuery, FindingRow, Page, QueryError, TagCount, TagMode, DEFAULT_LIMIT, MAX_LIMIT};` to `ledger/mod.rs`.
  - Re-export the same names from `lib.rs`.
  - Note: `rupu-cp/src/api/findings.rs` already imports `rupu_findings_report::select::select` by name. Every caller outside `rupu-coverage` uses the full path `rupu_coverage::ledger::query::select`, never the root re-export, so the two names never meet.

- [ ] **Step 4: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-coverage --lib ledger::query`
  - Expected: PASS.

- [ ] **Step 5: Format, lint and commit.**

```bash
git add crates/rupu-coverage
git commit -m "feat(coverage): ledger::query — tag-aware finding select and paged query

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Tags when a finding is reported (`report_finding`, `findings.record`)

**Files:**
- Modify: `crates/rupu-coverage/src/tools/report_finding.rs`, `crates/rupu-coverage/src/tools/attach_report.rs:494`, `crates/rupu-coverage/tests/it/attach_report.rs:31`
- Modify: `crates/rupu-agent/src/coverage_tools.rs` (`summary_schema`)
- Modify: `crates/rupu-mcp/src/tools/findings.rs` (`specs`, `RecordArgs`, `dispatch_record`); re-bless `crates/rupu-mcp/tests/snapshots/tools_list.json`
- Test: `report_finding.rs` unit tests, `crates/rupu-mcp/tests/it/findings_record.rs`

**Interfaces:**
- Consumes: `parse_tags`, `MAX_TAGS_PER_FINDING`.
- Produces:
  - `ReportFindingInput.tags: Vec<String>`
  - `ReportFindingError::{Tag(TagParseError), TooManyTags { count, max }}`
  - `RecordArgs.tags: Vec<String>`
  - `pub(crate) fn tags_property() -> serde_json::Value` in `rupu_coverage::ledger::tags`, re-exported as `rupu_coverage::tags_schema_property`

- [ ] **Step 1: Write the failing tests.**
  - Add `tags: Vec::new(),` to every `ReportFindingInput { … }` literal: the five in `report_finding.rs` tests, `attach_report.rs:494`, and `tests/it/attach_report.rs:31`.
  - Append to `report_finding.rs`'s `mod tests`:

```rust
    #[test]
    fn declared_tags_are_normalized_onto_the_record() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut inp = input(FindingScope::Repo);
        inp.tags = vec!["Needs-POC".into(), "class:sqli".into(), "needs-poc".into()];
        let opts = crate::report::FindingWriteOptions::default()
            .with_profile(crate::report::FindingProfile::Summary);
        report_finding(&paths, attribution(), inp, &opts).unwrap();
        let rec = &crate::ledger::views::read_findings(&paths).unwrap()[0];
        let tags: Vec<&str> = rec.tags.iter().map(|t| t.as_str()).collect();
        assert_eq!(tags, ["class:sqli", "needs-poc"]);
    }

    #[test]
    fn an_invalid_tag_fails_the_call_before_anything_is_written() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut inp = input(FindingScope::Repo);
        inp.tags = vec!["not ok".into()];
        let opts = crate::report::FindingWriteOptions::default()
            .with_profile(crate::report::FindingProfile::Summary);
        let err = report_finding(&paths, attribution(), inp, &opts).unwrap_err();
        assert!(err.to_string().contains("invalid tag `not ok`"), "{err}");
        assert!(!paths.findings.exists());
        assert!(!paths.assets.exists());
    }

    #[test]
    fn more_than_the_cap_is_refused() {
        let ws = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(ws.path(), "t");
        let mut inp = input(FindingScope::Repo);
        inp.tags = (0..33).map(|i| format!("t{i}")).collect();
        let opts = crate::report::FindingWriteOptions::default()
            .with_profile(crate::report::FindingProfile::Summary);
        let err = report_finding(&paths, attribution(), inp, &opts).unwrap_err();
        assert!(matches!(err, ReportFindingError::TooManyTags { count: 33, max: 32 }), "{err}");
    }
```

  - Append to `crates/rupu-mcp/tests/it/findings_record.rs`:

```rust
#[tokio::test]
async fn record_accepts_declared_tags() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dispatcher = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));
    let mut input = host_finding();
    input["tags"] = serde_json::json!(["Class:Authz", "needs-poc"]);
    dispatcher.call("findings.record", input).await.expect("record should succeed");
    let paths = rupu_coverage::CoveragePaths::new(
        tmp.path(),
        &rupu_coverage::target_id(tmp.path(), "chimera-campaign"),
    );
    let rec = &rupu_coverage::read_findings(&paths).unwrap()[0];
    let tags: Vec<&str> = rec.tags.iter().map(|t| t.as_str()).collect();
    assert_eq!(tags, ["class:authz", "needs-poc"]);
}
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-coverage --lib tools::report_finding` and `cargo test -p rupu-mcp --test it findings_record::`
  - Expected: a compile error (no field `tags` on `ReportFindingInput`).

- [ ] **Step 3: Implement.**
  - In `ReportFindingInput`, after `asset`:

```rust
    /// Tags to declare the finding with: free-form, normalized and validated
    /// (`ledger::tags::Tag`), at most 32. Changed later with `tag_findings`.
    #[serde(default)]
    pub tags: Vec<String>,
```

  - Add to `ReportFindingError`:

```rust
    #[error("{0}")]
    Tag(#[from] crate::ledger::tags::TagParseError),
    #[error("a finding can carry at most {max} tags; {count} were given")]
    TooManyTags { count: usize, max: usize },
```

  - At the top of `report_finding()`, **before** `validate_locator(&input)?;`, so a bad tag fails before any asset or finding write:

```rust
    let tags = crate::ledger::tags::parse_tags(&input.tags)?;
    if tags.len() > crate::ledger::tags::MAX_TAGS_PER_FINDING {
        return Err(ReportFindingError::TooManyTags {
            count: tags.len(),
            max: crate::ledger::tags::MAX_TAGS_PER_FINDING,
        });
    }
```

  - In the `FindingRecord { … }` literal, replace `tags: Vec::new(),` (from Task 1) with `tags,`.
  - In `ledger/tags.rs`, add the shared schema property and re-export it from `ledger/mod.rs` and `lib.rs` as `tags_schema_property`:

```rust
/// The `tags` property `report_finding` and `findings.record` advertise.
pub fn tags_schema_property() -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": { "type": "string" },
        "maxItems": MAX_TAGS_PER_FINDING,
        "description": "Free-form tags: lowercase a-z, 0-9 and . _ : / -, starting with a letter or digit, e.g. class:sqli, needs-poc. Reuse tags already in use where they fit (query_findings / findings.query list them)."
    })
}
```

  - In `rupu-agent/src/coverage_tools.rs` `summary_schema()`, add `"tags": rupu_coverage::tags_schema_property(),` to `"properties"`. `json!` accepts a function-call value expression in object position; if it doesn't, add it after construction the way `full_schema` adds `report`. `full_schema` derives from `summary_schema`, so it inherits the property.
  - In `rupu-mcp/src/tools/findings.rs`:
    - After `input_schema["properties"]["report"] = …;`, add `input_schema["properties"]["tags"] = rupu_coverage::tags_schema_property();`.
    - Add `#[serde(default)] pub tags: Vec<String>,` to `RecordArgs`.
    - In `dispatch_record`'s `ReportFindingInput { … }` literal, add `tags: args.tags,`.

- [ ] **Step 4: Re-bless the MCP snapshot and run the tests.**
  - Run `BLESS=1 cargo test -p rupu-mcp --test it schema_snapshot::`, then `git diff crates/rupu-mcp/tests/snapshots/tools_list.json`. The diff must show only the new `tags` property on `findings.record`.
  - Run `cargo test -p rupu-coverage --lib tools::`, `cargo test -p rupu-coverage --test it attach_report::`, `cargo test -p rupu-mcp --test it`, and `cargo test -p rupu-agent --test it findings_`.
  - Expected: PASS.

- [ ] **Step 5: Format, lint and commit.**

```bash
git add crates/rupu-coverage crates/rupu-agent crates/rupu-mcp
git commit -m "feat(findings): declare tags when reporting a finding

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Agent built-ins `query_findings` and `tag_findings`

**Files:**
- Modify: `crates/rupu-coverage/src/ledger/tags.rs` (`TagChangeInput`, `tag_input_schema`)
- Modify: `crates/rupu-agent/src/coverage_tools.rs` (two new tools), `crates/rupu-agent/src/runner.rs` (registration right after the `report_finding` block that ends near line 1907)
- Modify: `crates/rupu-agent/tests/it/findings_without_coverage.rs` (make `opts_for` `pub(crate)`)
- Create: `crates/rupu-agent/tests/it/findings_tags.rs`; modify `crates/rupu-agent/tests/it/main.rs`

**Interfaces:**
- Consumes: `apply`, `TagLog`, `TagActor`, `read_workspace_findings`, `query_response`, `query_input_schema`, `FindingQuery`.
- Produces:
  - `rupu_coverage::TagChangeInput { finding_ids, add, remove }` (`deny_unknown_fields`), with `into_change(self) -> Result<TagChange, TagParseError>`
  - `rupu_coverage::tag_input_schema()`
  - `rupu_agent::coverage_tools::{QueryFindingsTool::new(PathBuf), TagFindingsTool::new(TagLog)}`

- [ ] **Step 1: Write the failing tests.**
  - In `findings_without_coverage.rs`, change `fn opts_for(` to `pub(crate) fn opts_for(`.
  - Create `crates/rupu-agent/tests/it/findings_tags.rs` and add `mod findings_tags;` to `tests/it/main.rs`:

```rust
//! `query_findings` / `tag_findings`: explicit grants that read and tag the
//! workspace's findings.

use crate::findings_without_coverage::opts_for;
use chrono::Utc;
use rupu_agent::coverage_tools::{QueryFindingsTool, TagFindingsTool};
use rupu_agent::run_agent;
use rupu_agent::runner::ScriptedTurn;
use rupu_coverage::{
    append_record, read_tag_events, read_workspace_findings, Attribution, CoveragePaths,
    FindingEvidence, FindingProfile, FindingRecord, FindingScope, Ledger, Severity, Surface,
    TagActor, TagLog,
};
use rupu_providers::types::StopReason;
use rupu_tools::{Tool, ToolContext};

fn seed(ws: &std::path::Path, id: &str, severity: Severity) {
    let rec = FindingRecord {
        id: id.into(),
        file_path: Some("src/db.rs".into()),
        line_range: Some([10, 12]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: format!("{id}: string-built SQL"),
        severity,
        concern_id: None,
        evidence: FindingEvidence { code_excerpt: None, rationale: "r".into(), references: vec![] },
        declared_by: Attribution {
            run_id: "run_earlier".into(),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: Utc::now(),
        profile: FindingProfile::Summary,
        report: None,
        tags: vec![],
    };
    append_record(&CoveragePaths::new(ws, "earlier-scan"), Ledger::Findings, &rec).unwrap();
}

fn tag_then_stop(input: serde_json::Value) -> Vec<ScriptedTurn> {
    vec![
        ScriptedTurn::AssistantToolUse {
            text: None,
            tool_id: "t1".into(),
            tool_name: "tag_findings".into(),
            tool_input: input,
            stop: StopReason::ToolUse,
        },
        ScriptedTurn::AssistantText {
            text: "Tagged.".into(),
            stop: StopReason::EndTurn,
            input_tokens: 1,
            output_tokens: 1,
        },
    ]
}

#[tokio::test]
async fn a_granted_agent_tags_a_finding_another_scope_reported() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    run_agent(opts_for(
        &ws,
        Some(vec!["tag_findings".to_string()]),
        tag_then_stop(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["Class:SQLi"]})),
    ))
    .await
    .expect("agent run should succeed");

    let events = read_tag_events(&TagLog::for_workspace(&ws)).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0].by {
        TagActor::Agent(a) => {
            assert_eq!(a.run_id, "run_findings_test");
            assert_eq!(a.agent.as_deref(), Some("net-assessor"));
        }
        other => panic!("expected an agent actor, got {other:?}"),
    }
    let f = read_workspace_findings(&ws).unwrap();
    assert_eq!(f[0].tags[0].as_str(), "class:sqli");
}

#[tokio::test]
async fn tag_findings_is_absent_when_not_granted() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let _ = run_agent(opts_for(
        &ws,
        Some(vec!["read_file".to_string()]),
        tag_then_stop(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["x"]})),
    ))
    .await;
    assert!(!TagLog::for_workspace(&ws).path.exists());
}

#[tokio::test]
async fn tag_events_reach_the_run_stream() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let stream = tmp.path().join("runs/run_findings_test/coverage.jsonl");
    rupu_coverage::write_stream_begin(&stream, "run_findings_test").unwrap();
    let mut opts = opts_for(
        &ws,
        Some(vec!["tag_findings".to_string()]),
        tag_then_stop(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["x"]})),
    );
    opts.tool_context.coverage_stream = Some(stream.clone());
    run_agent(opts).await.expect("agent run should succeed");
    let found = std::fs::read_to_string(&stream).unwrap().lines().any(|l| {
        matches!(
            serde_json::from_str::<rupu_coverage::StreamLine>(l),
            Ok(rupu_coverage::StreamLine::Tags { scope_name, .. }) if scope_name == "net-assessor"
        )
    });
    assert!(found, "a tags line must reach the stream");
}

#[tokio::test]
async fn query_findings_pages_and_lists_the_vocabulary() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    seed(&ws, "fnd_b", Severity::Low);
    let tag = TagFindingsTool::new(TagLog::for_workspace(&ws));
    let ctx = ToolContext { workspace_path: ws.clone(), ..Default::default() };
    let out = tag
        .invoke(serde_json::json!({"finding_ids": ["fnd_b"], "add": ["needs-poc"]}), &ctx)
        .await
        .unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);

    let q = QueryFindingsTool::new(ws.clone());
    let out = q.invoke(serde_json::json!({"tags": ["needs-poc"]}), &ctx).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["rows"][0]["id"], "fnd_b");
    assert_eq!(v["tags_in_use"][0], serde_json::json!({"tag": "needs-poc", "count": 1}));

    let out = q.invoke(serde_json::json!({"limit": 1}), &ctx).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["rows"][0]["id"], "fnd_a");
    assert!(v["next_cursor"].is_string());
}

#[tokio::test]
async fn bad_input_is_an_error_the_agent_can_read() {
    let tmp = tempfile::TempDir::new().unwrap();
    let ws = tmp.path().to_path_buf();
    seed(&ws, "fnd_a", Severity::High);
    let ctx = ToolContext { workspace_path: ws.clone(), ..Default::default() };
    let tag = TagFindingsTool::new(TagLog::for_workspace(&ws));
    let out = tag
        .invoke(serde_json::json!({"finding_ids": ["fnd_nope"], "add": ["x"]}), &ctx)
        .await
        .unwrap();
    assert!(out.error.unwrap().contains("fnd_nope"));
    let out = tag
        .invoke(serde_json::json!({"finding_ids": ["fnd_a"], "add": ["not ok"]}), &ctx)
        .await
        .unwrap();
    assert!(out.error.unwrap().contains("invalid tag"));
    let q = QueryFindingsTool::new(ws);
    assert!(q.invoke(serde_json::json!({"tagz": ["x"]}), &ctx).await.is_err());
}
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-agent --test it findings_tags::`
  - Expected: a compile error (no `QueryFindingsTool`).

- [ ] **Step 3: Add the shared input type in `rupu-coverage` (`tags.rs`).** Export `TagChangeInput, tag_input_schema` from `ledger/mod.rs` and `lib.rs`.

```rust
/// `tag_findings` / `findings.tag` input. Unknown fields are refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagChangeInput {
    pub finding_ids: Vec<String>,
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
}

impl TagChangeInput {
    pub fn into_change(self) -> Result<TagChange, TagParseError> {
        Ok(TagChange {
            finding_ids: self.finding_ids,
            add: parse_tags(&self.add)?,
            remove: parse_tags(&self.remove)?,
        })
    }
}

/// Input schema shared by `tag_findings` and `findings.tag`.
pub fn tag_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["finding_ids"],
        "additionalProperties": false,
        "properties": {
            "finding_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1, "description": "Finding ids (fnd_…) from query_findings or report_finding." },
            "add": { "type": "array", "items": { "type": "string" }, "description": "Tags to add." },
            "remove": { "type": "array", "items": { "type": "string" }, "description": "Tags to remove." }
        }
    })
}
```

- [ ] **Step 4: Add the tools to `coverage_tools.rs`,** after `ReportFindingTool`'s schemas. Add `use std::path::PathBuf;`.

```rust
// ---------------------------------------------------------------------------
// query_findings / tag_findings
// ---------------------------------------------------------------------------

/// Read this workspace's findings, filtered and paged (`ledger::query`).
pub struct QueryFindingsTool {
    workspace: PathBuf,
}

impl QueryFindingsTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for QueryFindingsTool {
    fn name(&self) -> &'static str {
        "query_findings"
    }

    fn description(&self) -> &'static str {
        "List findings recorded in this project, filtered by tag, severity, concern or file. \
         Returns one page of slim rows (id, title, severity, location, tags), `next_cursor` \
         for the next page, `total` matches, and `tags_in_use` — the tags already used in \
         this project, with counts. Reuse an existing tag where it fits before inventing one."
    }

    fn input_schema(&self) -> Value {
        rupu_coverage::query_input_schema()
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let q: rupu_coverage::FindingQuery = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let workspace = self.workspace.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Value, String> {
            let records =
                rupu_coverage::read_workspace_findings(&workspace).map_err(|e| e.to_string())?;
            rupu_coverage::query_response(&records, &q).map_err(|e| e.to_string())
        })
        .await;
        match result {
            Ok(Ok(v)) => Ok(ok_output(
                serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into()),
                started,
            )),
            Ok(Err(e)) => Ok(err_output(e, started)),
            Err(join) => Ok(err_output(format!("query_findings did not complete: {join}"), started)),
        }
    }
}

/// Add or remove tags on this workspace's findings (`ledger::tags::apply`).
pub struct TagFindingsTool {
    log: rupu_coverage::TagLog,
}

impl TagFindingsTool {
    pub fn new(log: rupu_coverage::TagLog) -> Self {
        Self { log }
    }
}

#[async_trait]
impl Tool for TagFindingsTool {
    fn name(&self) -> &'static str {
        "tag_findings"
    }

    fn description(&self) -> &'static str {
        "Add or remove tags on findings in this project, one or many at once. Tags are \
         free-form: lowercase a-z, 0-9 and . _ : / -, starting with a letter or digit (e.g. \
         class:sqli, needs-poc, status:triaged). Prefer tags already in use (query_findings \
         lists them). An unknown finding id rejects the whole call; adding a tag a finding \
         already has changes nothing. Returns each finding's tags before and after."
    }

    fn input_schema(&self) -> Value {
        rupu_coverage::tag_input_schema()
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let parsed: rupu_coverage::TagChangeInput = serde_path_to_error::deserialize(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let change = match parsed.into_change() {
            Ok(c) => c,
            Err(e) => return Ok(err_output(e.to_string(), started)),
        };
        let by = rupu_coverage::TagActor::Agent(attribution_from_ctx(ctx));
        let log = self.log.clone();
        let result =
            tokio::task::spawn_blocking(move || rupu_coverage::apply(&log, &change, &by)).await;
        match result {
            Ok(Ok(outcomes)) => Ok(ok_output(
                serde_json::to_string_pretty(&serde_json::json!({ "outcomes": outcomes }))
                    .unwrap_or_else(|_| "{}".into()),
                started,
            )),
            Ok(Err(e)) => Ok(err_output(e.to_string(), started)),
            Err(join) => Ok(err_output(format!("tag_findings did not complete: {join}"), started)),
        }
    }
}
```

- [ ] **Step 5: Register the tools in `runner.rs`,** directly after the closing `}` of the `if coverage.is_none() && … "report_finding"` block:

```rust
    // Finding tags (spec 2026-10-06-rupu-finding-tags-design.md): explicit
    // `tools:` grants like `report_finding`, registered with or without the
    // coverage harness. Both act on this workspace's findings only — tags
    // live in one workspace-wide log — and `tag_findings` is allowed in
    // readonly mode, as `report_finding` is: it annotates the ledger and
    // never touches the workspace's files.
    let granted = |name: &str| {
        opts.agent_tools
            .as_ref()
            .is_some_and(|list| list.iter().any(|t| t == name))
    };
    let grant_query = granted("query_findings");
    let grant_tag = granted("tag_findings");
    if grant_query {
        registry.insert(
            "query_findings",
            std::sync::Arc::new(coverage_tools::QueryFindingsTool::new(
                opts.workspace_path.clone(),
            )),
        );
    }
    if grant_tag {
        let scope = opts.scope_name.as_deref().unwrap_or(&opts.agent_name);
        let log = rupu_coverage::TagLog::for_workspace(&opts.workspace_path)
            .with_run_stream(run_stream_for(&opts.tool_context, scope));
        registry.insert(
            "tag_findings",
            std::sync::Arc::new(coverage_tools::TagFindingsTool::new(log)),
        );
    }
```

If the borrow checker objects to `granted` borrowing `opts` while it's mutated later, the two `let grant_*` lines already end that borrow. Keep them before any `opts` mutation.

- [ ] **Step 6: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-agent --test it findings_tags::` and `cargo test -p rupu-agent --test it findings_without_coverage::`
  - Expected: PASS.

- [ ] **Step 7: Format, lint and commit.**

```bash
git add crates/rupu-coverage crates/rupu-agent
git commit -m "feat(agent): query_findings and tag_findings built-ins

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: MCP `findings.query` and `findings.tag`

**Files:**
- Modify: `crates/rupu-mcp/src/tools/findings.rs`, `crates/rupu-mcp/src/dispatcher.rs` (the `match name`, before `other =>`)
- Create: `crates/rupu-mcp/tests/it/findings_tags.rs`; modify `crates/rupu-mcp/tests/it/main.rs`; re-bless `tests/snapshots/tools_list.json`

**Interfaces:**
- Consumes: `FindingsContext`, `TagChangeInput`, `apply`, `query_response`, `read_workspace_findings`.
- Produces:
  - `tools::findings::dispatch_query(&FindingsContext, Value) -> Result<String, String>`
  - `tools::findings::dispatch_tag(&FindingsContext, Value) -> Result<String, String>`
  - Two `ToolSpec`s: `findings.query` (Read) and `findings.tag` (Write).

- [ ] **Step 1: Write the failing tests.** Create `crates/rupu-mcp/tests/it/findings_tags.rs` and add `mod findings_tags;` to `main.rs`:

```rust
//! `findings.query` / `findings.tag`: the MCP-side tag tools for `action:`
//! steps and `rupu mcp serve`.

use rupu_mcp::{FindingsContext, McpPermission, ToolDispatcher};
use rupu_scm::Registry;
use rupu_tools::PermissionMode;
use std::sync::Arc;

fn ctx(workspace: &std::path::Path) -> FindingsContext {
    FindingsContext {
        workspace_path: workspace.to_path_buf(),
        scope_name: "triage-flow".to_string(),
        run_id: "run_mcp_tags".to_string(),
        model: "m".to_string(),
        surface: rupu_coverage::Surface::Workflow,
        options: rupu_coverage::FindingWriteOptions::default()
            .with_profile(rupu_coverage::FindingProfile::Summary),
        codename: Some("jade-reef".to_string()),
        provider: None,
    }
}

async fn record(d: &ToolDispatcher, summary: &str) -> String {
    let out = d
        .call(
            "findings.record",
            serde_json::json!({"scope": "repo", "summary": summary, "severity": "high", "rationale": "r"}),
        )
        .await
        .unwrap();
    out.trim_start_matches("finding_id: ").to_string()
}

#[tokio::test]
async fn tag_then_query_by_tag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let d = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all())
        .with_findings(ctx(tmp.path()));
    let a = record(&d, "first").await;
    let _b = record(&d, "second").await;

    let out = d
        .call("findings.tag", serde_json::json!({"finding_ids": [a], "add": ["Needs-POC"]}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["outcomes"][0]["after"], serde_json::json!(["needs-poc"]));

    let out = d
        .call("findings.query", serde_json::json!({"tags": ["needs-poc"]}))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["rows"][0]["id"], a);

    let events = rupu_coverage::read_tag_events(&rupu_coverage::TagLog::for_workspace(tmp.path())).unwrap();
    match &events[0].by {
        rupu_coverage::TagActor::Agent(at) => {
            assert_eq!(at.run_id, "run_mcp_tags");
            assert_eq!(at.agent, None);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn readonly_mode_blocks_findings_tag_but_not_query() {
    let tmp = tempfile::TempDir::new().unwrap();
    let d = ToolDispatcher::new(
        Arc::new(Registry::default()),
        McpPermission::new(PermissionMode::Readonly, vec!["*".into()]),
    )
    .with_findings(ctx(tmp.path()));
    let err = d
        .call("findings.tag", serde_json::json!({"finding_ids": ["fnd_x"], "add": ["x"]}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("readonly"), "{err}");
    assert!(d.call("findings.query", serde_json::json!({})).await.is_ok());
}

#[tokio::test]
async fn without_run_context_the_tools_refuse() {
    let d = ToolDispatcher::new(Arc::new(Registry::default()), McpPermission::allow_all());
    let err = d.call("findings.query", serde_json::json!({})).await.unwrap_err();
    assert!(err.to_string().contains("unavailable"), "{err}");
}
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-mcp --test it findings_tags::`
  - Expected: FAIL with `UnknownTool("findings.tag")`.

- [ ] **Step 3: Implement in `tools/findings.rs`.**
  - Replace the record function's inline `Attribution` with a shared helper, and use it in `dispatch_record` too:

```rust
fn attribution(ctx: &FindingsContext) -> rupu_coverage::Attribution {
    rupu_coverage::Attribution {
        run_id: ctx.run_id.clone(),
        model: ctx.model.clone(),
        surface: ctx.surface,
        codename: ctx.codename.clone(),
        agent: None,
        provider: ctx.provider.clone(),
    }
}
```

  - Turn `specs()`'s `vec![ToolSpec { … }]` into `vec![ToolSpec { /* findings.record, unchanged */ }, query_spec(), tag_spec()]` with:

```rust
fn query_spec() -> ToolSpec {
    ToolSpec {
        name: "findings.query",
        description: "List this project's findings, filtered by tag, severity, concern or \
                      file: one page of slim rows, `next_cursor`, `total`, and `tags_in_use` \
                      (the project's tag vocabulary, with counts).",
        input_schema: rupu_coverage::query_input_schema(),
        kind: ToolKind::Read,
    }
}

fn tag_spec() -> ToolSpec {
    ToolSpec {
        name: "findings.tag",
        description: "Add or remove free-form tags (lowercase a-z 0-9 . _ : / -) on one or \
                      more of this project's findings. An unknown id rejects the whole call; \
                      returns each finding's tags before and after.",
        input_schema: rupu_coverage::tag_input_schema(),
        kind: ToolKind::Write,
    }
}

/// `findings.query`: one page of this workspace's findings.
pub fn dispatch_query(ctx: &FindingsContext, args: serde_json::Value) -> Result<String, String> {
    let q: rupu_coverage::FindingQuery = serde_path_to_error::deserialize(args)
        .map_err(|e| format!("invalid findings.query input: {e}"))?;
    let records =
        rupu_coverage::read_workspace_findings(&ctx.workspace_path).map_err(|e| e.to_string())?;
    let v = rupu_coverage::query_response(&records, &q).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// `findings.tag`: apply one tag change to this workspace's findings.
pub fn dispatch_tag(ctx: &FindingsContext, args: serde_json::Value) -> Result<String, String> {
    let input: rupu_coverage::TagChangeInput = serde_path_to_error::deserialize(args)
        .map_err(|e| format!("invalid findings.tag input: {e}"))?;
    let change = input.into_change().map_err(|e| e.to_string())?;
    let by = rupu_coverage::TagActor::Agent(attribution(ctx));
    let outcomes = rupu_coverage::apply(
        &rupu_coverage::TagLog::for_workspace(&ctx.workspace_path),
        &change,
        &by,
    )
    .map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&serde_json::json!({ "outcomes": outcomes }))
        .map_err(|e| e.to_string())
}
```

  - Update the module doc comment's first line to: `` //! `findings.record`, `findings.query`, `findings.tag` — the findings ledger from a workflow `action:` step. ``
  - In `dispatcher.rs`, add before `other =>`:

```rust
            "findings.query" | "findings.tag" => {
                let ctx = self.findings.clone().ok_or_else(|| {
                    McpError::Tool(format!(
                        "{name} is unavailable: this MCP server was started without run \
                         context, so there is no workspace whose findings to use"
                    ))
                })?;
                let is_query = name == "findings.query";
                tokio::task::spawn_blocking(move || {
                    if is_query {
                        tools::findings::dispatch_query(&ctx, args)
                    } else {
                        tools::findings::dispatch_tag(&ctx, args)
                    }
                })
                .await
                .map_err(|e| McpError::Tool(format!("{name} did not complete: {e}")))?
                .map_err(McpError::Tool)
            }
```

  - If `serde_path_to_error` isn't a `rupu-mcp` dependency, the `findings.record` arm already uses it, so it is.

- [ ] **Step 4: Re-bless and run the tests.**
  - Run `BLESS=1 cargo test -p rupu-mcp --test it schema_snapshot::`. Check the diff adds exactly the two tools.
  - Run `cargo test -p rupu-mcp --test it`.
  - Expected: PASS.

- [ ] **Step 5: Format, lint and commit.**

```bash
git add crates/rupu-mcp
git commit -m "feat(mcp): findings.query and findings.tag

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Cross-workspace tagging and `rupu findings list|tag|tags`

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs` (add `tag_findings_across` + result types next to `finding_ledgers`)
- Modify: `crates/rupu-cli/src/cmd/findings.rs`, `crates/rupu-cli/src/lib.rs:438` (pass `cli.format`)
- Create: `crates/rupu-cli/tests/it/findings_tags.rs`; modify `crates/rupu-cli/tests/it/main.rs`

**Interfaces:**
- Consumes: `apply`, `TagChange`, `TagActor`, `select`, `FindingQuery`, `FindingRow`, `tags_in_use`, and the existing `collect_all_findings`, `resolve_project`, `resolve_run_scope`, `parse_min_severity`.
- Produces:
  - `rupu_cp::api::findings::{tag_findings_across(&Path, &TagChange, &TagActor) -> Result<TagAcrossResult, TagError>, TagAcrossResult { workspaces, unknown }, WorkspaceTagResult { ws_id, outcomes, error }}`, all `Serialize`. Plan 2's `POST /api/findings/tags` reuses them.
  - `cmd::findings::handle(action, Option<OutputFormat>)`

- [ ] **Step 1: Write the failing CLI tests.** Create `crates/rupu-cli/tests/it/findings_tags.rs` and add `mod findings_tags;` to `tests/it/main.rs`:

```rust
//! `rupu findings list|tag|tags` end to end, through the real binary, over a
//! temp `RUPU_HOME` with registered workspaces.

use assert_cmd::Command;
use rupu_coverage::{
    Attribution, CoveragePaths, FindingEvidence, FindingProfile, FindingRecord, FindingScope,
    Severity, Surface,
};
use std::path::{Path, PathBuf};

fn record(id: &str, severity: Severity) -> FindingRecord {
    FindingRecord {
        id: id.into(),
        file_path: Some("src/app.rs".into()),
        line_range: Some([5, 9]),
        target_ref: None,
        scope: FindingScope::Line,
        summary: format!("{id} summary"),
        severity,
        concern_id: None,
        evidence: FindingEvidence { code_excerpt: None, rationale: "why".into(), references: vec![] },
        declared_by: Attribution {
            run_id: format!("run_{id}"),
            model: "m".into(),
            surface: Surface::Workflow,
            codename: None,
            agent: None,
            provider: None,
        },
        declared_at: "2026-10-01T00:00:00Z".parse().unwrap(),
        profile: FindingProfile::Summary,
        report: None,
        tags: vec![],
    }
}

/// Register workspace `ws_id` at `<home>/<dir>` with `records` in one target.
fn seed(home: &Path, ws_id: &str, dir: &str, records: &[FindingRecord]) -> PathBuf {
    let repo = home.join(dir);
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.canonicalize().unwrap();
    let ws = rupu_workspace::Workspace {
        id: ws_id.to_string(),
        path: repo.to_str().unwrap().to_string(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        last_run_at: None,
    };
    let wsdir = home.join("workspaces");
    std::fs::create_dir_all(&wsdir).unwrap();
    std::fs::write(wsdir.join(format!("{ws_id}.toml")), toml::to_string(&ws).unwrap()).unwrap();
    let paths = CoveragePaths::new(&repo, "tgt1");
    paths.ensure_dir().unwrap();
    let jsonl: String = records.iter().map(|r| serde_json::to_string(r).unwrap() + "\n").collect();
    std::fs::write(&paths.findings, jsonl).unwrap();
    repo
}

fn seed_one(home: &Path) -> PathBuf {
    seed(home, "ws1", "repo", &[record("fnd_a", Severity::High), record("fnd_b", Severity::Low)])
}

fn rupu(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env("RUPU_HOME", home).env("NO_COLOR", "1").current_dir(home).write_stdin("");
    cmd
}

fn ok(out: &std::process::Output) -> String {
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn tag_then_filter_by_tag_and_untagged() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "--add", "Class:SQLi", "--add", "needs-poc"])
        .output()
        .unwrap();
    assert!(ok(&out).contains("fnd_a: (none) → class:sqli, needs-poc"));

    let out = rupu(tmp.path())
        .args(["findings", "list", "--tag", "class:sqli", "--ids-only"])
        .output()
        .unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
    let out = rupu(tmp.path()).args(["findings", "list", "--untagged", "--ids-only"]).output().unwrap();
    assert_eq!(ok(&out), "fnd_b\n");
}

#[test]
fn ids_can_be_piped_on_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "tag", "-", "--add", "triaged"])
        .write_stdin("fnd_a\n\nfnd_b\n")
        .output()
        .unwrap();
    ok(&out);
    let out = rupu(tmp.path()).args(["findings", "list", "--tag", "triaged", "--ids-only"]).output().unwrap();
    assert_eq!(ok(&out), "fnd_a\nfnd_b\n");
}

#[test]
fn an_unknown_id_fails_but_known_ones_are_tagged() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path())
        .args(["findings", "tag", "fnd_a", "fnd_nope", "--add", "x"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("fnd_nope"));
    let out = rupu(tmp.path()).args(["findings", "list", "--tag", "x", "--ids-only"]).output().unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
}

#[test]
fn one_call_tags_findings_in_two_workspaces() {
    let tmp = tempfile::tempdir().unwrap();
    let repo1 = seed_one(tmp.path());
    let repo2 = seed(tmp.path(), "ws2", "other", &[record("fnd_c", Severity::Medium)]);
    ok(&rupu(tmp.path()).args(["findings", "tag", "fnd_a", "fnd_c", "--add", "x"]).output().unwrap());
    for repo in [repo1, repo2] {
        let log = rupu_coverage::TagLog::for_workspace(&repo);
        assert_eq!(rupu_coverage::read_tag_events(&log).unwrap().len(), 1);
    }
}

#[test]
fn remove_and_tags_in_use_as_json() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    ok(&rupu(tmp.path()).args(["findings", "tag", "fnd_a", "fnd_b", "--add", "x", "--add", "y"]).output().unwrap());
    ok(&rupu(tmp.path()).args(["findings", "tag", "fnd_b", "--remove", "y"]).output().unwrap());
    let out = rupu(tmp.path()).args(["--format", "json", "findings", "tags"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_str(&ok(&out)).unwrap();
    assert_eq!(v, serde_json::json!([{"tag": "x", "count": 2}, {"tag": "y", "count": 1}]));
    let events = rupu_coverage::read_tag_events(&rupu_coverage::TagLog::for_workspace(
        &tmp.path().join("repo").canonicalize().unwrap(),
    ))
    .unwrap();
    assert!(events.iter().all(|e| matches!(
        &e.by,
        rupu_coverage::TagActor::Operator(o) if o.via == rupu_coverage::OperatorSurface::Cli
    )));
}

#[test]
fn list_as_json_carries_tags_and_project_and_limit_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    ok(&rupu(tmp.path()).args(["findings", "tag", "fnd_a", "--add", "x"]).output().unwrap());
    let out = rupu(tmp.path()).args(["--format", "json", "findings", "list"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_str(&ok(&out)).unwrap();
    assert_eq!(v[0]["id"], "fnd_a");
    assert_eq!(v[0]["tags"], serde_json::json!(["x"]));
    assert_eq!(v[0]["project"], "repo");
    let out = rupu(tmp.path()).args(["findings", "list", "--limit", "1", "--ids-only"]).output().unwrap();
    assert_eq!(ok(&out), "fnd_a\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("showing 1 of 2"));
}

#[test]
fn an_invalid_tag_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    seed_one(tmp.path());
    let out = rupu(tmp.path()).args(["findings", "tag", "fnd_a", "--add", "bad tag"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("invalid tag"));
}
```

- [ ] **Step 2: Run the tests and watch them fail.**
  - Run: `cargo test -p rupu-cli --test it findings_tags::`
  - Expected: FAIL. `findings tag` isn't a subcommand yet, so clap exits 2.

- [ ] **Step 3: Add `tag_findings_across` in `rupu-cp/src/api/findings.rs`,** after `finding_ledgers`:

```rust
/// One workspace's part of a cross-workspace tag change.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceTagResult {
    pub ws_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<rupu_coverage::TagOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a tag change did across the registered workspaces. Each workspace's
/// batch is atomic; the whole is not.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TagAcrossResult {
    pub workspaces: Vec<WorkspaceTagResult>,
    /// Ids no registered workspace holds.
    pub unknown: Vec<String>,
}

/// Apply `change` to findings wherever they live: ids are grouped by the
/// registered workspace holding them and `rupu_coverage::apply` runs once
/// per workspace (spec "Batches that span workspaces"). An id found in two
/// distinct workspace paths is tagged in both. Used by `rupu findings tag`
/// and, in Plan 2, `POST /api/findings/tags`.
pub fn tag_findings_across(
    global_dir: &std::path::Path,
    change: &rupu_coverage::TagChange,
    by: &rupu_coverage::TagActor,
) -> Result<TagAcrossResult, rupu_coverage::TagError> {
    change.check()?;
    // finding id → each (ws_id, workspace path) holding it, once per path.
    let mut homes: HashMap<String, Vec<(String, std::path::PathBuf)>> = HashMap::new();
    each_ledger(global_dir, |w, _, _, records| {
        let path = std::path::PathBuf::from(&w.path);
        for r in records {
            let entry = homes.entry(r.id).or_default();
            if !entry.iter().any(|(_, p)| *p == path) {
                entry.push((w.id.clone(), path.clone()));
            }
        }
    });
    let mut out = TagAcrossResult::default();
    let mut by_ws: std::collections::BTreeMap<(String, std::path::PathBuf), Vec<String>> =
        std::collections::BTreeMap::new();
    for id in &change.finding_ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        match homes.get(id) {
            Some(hs) => {
                for h in hs {
                    by_ws.entry(h.clone()).or_default().push(id.to_string());
                }
            }
            None if !out.unknown.iter().any(|u| u == id) => out.unknown.push(id.to_string()),
            None => {}
        }
    }
    for ((ws_id, path), ids) in by_ws {
        let one = rupu_coverage::TagChange {
            finding_ids: ids,
            add: change.add.clone(),
            remove: change.remove.clone(),
        };
        let log = rupu_coverage::TagLog::for_workspace(&path);
        out.workspaces.push(match rupu_coverage::apply(&log, &one, by) {
            Ok(outcomes) => WorkspaceTagResult { ws_id, outcomes, error: None },
            Err(e) => WorkspaceTagResult { ws_id, outcomes: vec![], error: Some(e.to_string()) },
        });
    }
    Ok(out)
}
```

- [ ] **Step 4: Add the CLI subcommands in `cmd/findings.rs`.**
  - Update the module doc comment's first line to: `` //! `rupu findings` — the finding report contract, report exports, the one-time import of reports written before the full profile, and finding tags (list/tag/tags). ``
  - Add the `Action` variants:

```rust
    /// List findings, filtered by tag, severity, project or run.
    List(ListArgs),
    /// Add or remove tags on findings.
    ///
    /// Tags are free-form: lowercase a-z, 0-9 and `. _ : / -`, starting with
    /// a letter or digit, at most 64 characters (`Needs-POC` is stored as
    /// `needs-poc`). Give `-` as the only id to read ids from stdin, one per
    /// line: `rupu findings list --tag class:sqli --ids-only | rupu findings
    /// tag - --add needs-poc`. Findings in different projects are changed
    /// project by project; an unknown id fails the command after the rest
    /// are changed.
    Tag(TagArgs),
    /// List the tags in use, with how many findings carry each.
    Tags(TagsArgs),
```

  - Add the argument structs:

```rust
#[derive(Debug, clap::Args)]
pub struct ScopeArgs {
    /// Only this project: a workspace id, or the path of its checkout.
    #[arg(long, value_name = "WS_ID|PATH", value_parser = non_blank)]
    project: Option<String>,
    /// Only the findings a run (and its sub-runs) declared.
    #[arg(long, value_name = "RUN_ID", value_parser = non_blank)]
    run: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[command(flatten)]
    scope: ScopeArgs,
    /// Only findings carrying this tag (repeatable: all of them, or any with
    /// --any-tag).
    #[arg(long = "tag", value_name = "TAG", value_parser = tag_arg)]
    tags: Vec<rupu_coverage::Tag>,
    /// With several --tag: findings carrying any of them.
    #[arg(long, requires = "tags")]
    any_tag: bool,
    /// Only findings with no tags.
    #[arg(long, conflicts_with = "tags")]
    untagged: bool,
    /// Only this severity and worse.
    #[arg(long, value_name = "SEVERITY", value_parser = ["critical", "high", "medium", "low", "info"])]
    severity: Option<String>,
    /// Show at most N findings; how many were left out goes to stderr.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    limit: Option<u64>,
    /// Print only the finding ids, one per line.
    #[arg(long)]
    ids_only: bool,
}

#[derive(Debug, clap::Args)]
pub struct TagArgs {
    /// Finding ids, or `-` alone to read them from stdin.
    #[arg(required = true, value_name = "FINDING_ID")]
    ids: Vec<String>,
    /// Tag to add (repeatable).
    #[arg(long = "add", value_name = "TAG", value_parser = tag_arg)]
    add: Vec<rupu_coverage::Tag>,
    /// Tag to remove (repeatable).
    #[arg(long = "remove", value_name = "TAG", value_parser = tag_arg)]
    remove: Vec<rupu_coverage::Tag>,
}

#[derive(Debug, clap::Args)]
pub struct TagsArgs {
    #[command(flatten)]
    scope: ScopeArgs,
}

fn tag_arg(raw: &str) -> Result<rupu_coverage::Tag, String> {
    rupu_coverage::Tag::parse(raw).map_err(|e| e.to_string())
}
```

  - In `ensure_output_format`, add `Action::List(_) => "findings list"`, `Action::Tag(_) => "findings tag"` and `Action::Tags(_) => "findings tags"`. Use `report::TABLE_JSON` for those three, and keep `TABLE_ONLY` for the rest:

```rust
    let supported = match action {
        Action::List(_) | Action::Tag(_) | Action::Tags(_) => report::TABLE_JSON,
        _ => report::TABLE_ONLY,
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
```

  - Change `handle` to take the format, and update `src/lib.rs:438` to `cmd::findings::handle(action, cli.format).await`:

```rust
pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    let json = matches!(format, Some(OutputFormat::Json));
    let result = match action {
        Action::Schema { advertised } => schema_cmd(advertised),
        Action::Export(args) => export_cmd(&args),
        Action::Import(args) => import_cmd(&args),
        Action::List(args) => list_cmd(&args, json),
        Action::Tag(args) => tag_cmd(&args, json),
        Action::Tags(args) => tags_cmd(&args, json),
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}
```

  - Add the commands:

```rust
/// Every finding the scope selects (tags folded), and the run-id set a
/// `--run` resolves to (the run plus its sub-runs).
fn scoped_findings(
    scope: &ScopeArgs,
) -> anyhow::Result<(Vec<cp_findings::FindingOut>, Option<std::collections::HashSet<String>>)> {
    let global = crate::paths::global_dir()?;
    let mut all = cp_findings::collect_all_findings(&global);
    if let Some(project) = &scope.project {
        let ws = cp_findings::resolve_project(&global, project).map_err(anyhow::Error::msg)?;
        all.retain(|f| f.ws_id == ws);
    }
    let run_ids = scope
        .run
        .as_deref()
        .map(|r| cp_findings::resolve_run_scope(&RunStore::new(global.join("runs")), r));
    Ok((all, run_ids))
}

#[derive(serde::Serialize)]
struct ListRow {
    #[serde(flatten)]
    row: rupu_coverage::FindingRow,
    ws_id: String,
    project: String,
}

fn tag_list(tags: &[rupu_coverage::Tag]) -> String {
    if tags.is_empty() {
        "(none)".to_string()
    } else {
        tags.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(", ")
    }
}

fn list_cmd(args: &ListArgs, json: bool) -> anyhow::Result<()> {
    use rupu_coverage::{FindingQuery, TagMode};
    let (all, run_ids) = scoped_findings(&args.scope)?;
    let q = FindingQuery {
        tags: args.tags.clone(),
        tag_mode: if args.any_tag { TagMode::Any } else { TagMode::All },
        untagged: args.untagged,
        min_severity: args.severity.as_deref().and_then(cp_findings::parse_min_severity),
        run_ids,
        ..Default::default()
    };
    let selected = rupu_coverage::ledger::query::select(&all, |f| &f.record, &q)?;
    let total = selected.len();
    let shown: Vec<&cp_findings::FindingOut> = match args.limit {
        Some(n) => selected.into_iter().take(n as usize).collect(),
        None => selected,
    };
    if shown.len() < total {
        eprintln!("showing {} of {total} findings", shown.len());
    }
    if args.ids_only {
        for f in &shown {
            println!("{}", f.record.id);
        }
        return Ok(());
    }
    let rows: Vec<ListRow> = shown
        .iter()
        .map(|f| ListRow {
            row: rupu_coverage::FindingRow::from(&f.record),
            ws_id: f.ws_id.clone(),
            project: f.project.clone(),
        })
        .collect();
    if json {
        return crate::output::formats::print_json(&rows);
    }
    if rows.is_empty() {
        println!("no findings match");
        return Ok(());
    }
    let mut t = crate::output::tables::new_table();
    t.set_header(vec!["ID", "SEVERITY", "TITLE", "LOCATION", "TAGS", "PROJECT"]);
    for r in &rows {
        let mut title = r.row.title.clone();
        if title.chars().count() > 60 {
            title = title.chars().take(59).collect::<String>() + "…";
        }
        t.add_row(vec![
            r.row.id.clone(),
            r.row.severity.as_str().to_string(),
            title,
            r.row.location.clone().unwrap_or_default(),
            if r.row.tags.is_empty() { String::new() } else { tag_list(&r.row.tags) },
            r.project.clone(),
        ]);
    }
    println!("{t}");
    Ok(())
}

/// The ids to change: the arguments, or stdin's lines when `-` is the only one.
fn finding_ids(args: &[String], stdin: impl std::io::BufRead) -> anyhow::Result<Vec<String>> {
    if args.len() == 1 && args[0] == "-" {
        let mut ids = Vec::new();
        for line in stdin.lines() {
            let line = line?;
            let id = line.trim();
            if !id.is_empty() {
                ids.push(id.to_string());
            }
        }
        if ids.is_empty() {
            anyhow::bail!("no finding ids on stdin");
        }
        return Ok(ids);
    }
    if args.iter().any(|a| a == "-") {
        anyhow::bail!("`-` (read ids from stdin) must be the only id");
    }
    Ok(args.to_vec())
}

fn tag_cmd(args: &TagArgs, json: bool) -> anyhow::Result<()> {
    let change = rupu_coverage::TagChange {
        finding_ids: finding_ids(&args.ids, std::io::stdin().lock())?,
        add: args.add.clone(),
        remove: args.remove.clone(),
    };
    let global = crate::paths::global_dir()?;
    let by = rupu_coverage::TagActor::operator(rupu_coverage::OperatorSurface::Cli);
    let result = cp_findings::tag_findings_across(&global, &change, &by)?;
    if json {
        crate::output::formats::print_json(&result)?;
    } else {
        for w in &result.workspaces {
            if let Some(e) = &w.error {
                println!("{}: not changed: {e}", w.ws_id);
                continue;
            }
            for o in &w.outcomes {
                if o.changed() {
                    println!("{}: {} → {}", o.finding_id, tag_list(&o.before), tag_list(&o.after));
                } else {
                    println!("{}: {} (unchanged)", o.finding_id, tag_list(&o.after));
                }
            }
        }
    }
    let failed: Vec<&str> = result
        .workspaces
        .iter()
        .filter(|w| w.error.is_some())
        .map(|w| w.ws_id.as_str())
        .collect();
    if result.unknown.is_empty() && failed.is_empty() {
        return Ok(());
    }
    let mut why = Vec::new();
    if !result.unknown.is_empty() {
        why.push(format!("unknown finding id(s): {}", result.unknown.join(", ")));
    }
    if !failed.is_empty() {
        why.push(format!("nothing changed in {}", failed.join(", ")));
    }
    let partial = result.workspaces.iter().any(|w| w.error.is_none());
    anyhow::bail!(
        "{}{}",
        why.join("; "),
        if partial { " (the other findings were changed)" } else { "" }
    )
}

fn tags_cmd(args: &TagsArgs, json: bool) -> anyhow::Result<()> {
    let (all, run_ids) = scoped_findings(&args.scope)?;
    let q = rupu_coverage::FindingQuery { run_ids, ..Default::default() };
    let selected = rupu_coverage::ledger::query::select(&all, |f| &f.record, &q)?;
    let counts = rupu_coverage::tags_in_use(selected.iter().map(|f| &f.record));
    if json {
        return crate::output::formats::print_json(&counts);
    }
    if counts.is_empty() {
        println!("no tags in use");
        return Ok(());
    }
    let mut t = crate::output::tables::new_table();
    t.set_header(vec!["TAG", "FINDINGS"]);
    for c in &counts {
        t.add_row(vec![c.tag.as_str().to_string(), c.count.to_string()]);
    }
    println!("{t}");
    Ok(())
}
```

Notes:
- If `crate::output::tables::new_table()` returns a type whose header and row methods differ, match how other `cmd/*.rs` files build tables (`grep -rn "new_table()" crates/rupu-cli/src/cmd | head`).
- `print_json`'s signature is `fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()>`.

- [ ] **Step 5: Run the tests and watch them pass.**
  - Run: `cargo test -p rupu-cli --test it findings_tags::`, then `cargo test -p rupu-cli --test it findings_export::` and `cargo test -p rupu-cli --test it findings_import::` (both share `cmd/findings.rs`).
  - Expected: PASS.

- [ ] **Step 6: Format, lint and commit.**
  - Run `cargo clippy -p rupu-cp -p rupu-cli --all-targets -- -D warnings`.

```bash
git add crates/rupu-cp/src/api/findings.rs crates/rupu-cli
git commit -m "feat(cli): rupu findings list|tag|tags, cross-workspace tagging

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Docs

**Files:**
- Modify: `docs/coverage.md` (a new `## Tagging findings` section, placed after the findings sections, before the export or import sections)
- Modify: `CLAUDE.md` (the `rupu-coverage` crate entry)
- Modify: `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md` (mark Plan 1 complete)

- [ ] **Step 1: Write the `docs/coverage.md` section.** Cover each of these with short paragraphs and one example per surface:
  - **Syntax:** the normalization and rejection rules, the 32 cap, and `namespace:value` as a convention.
  - **Who can tag:** agents (`report_finding`'s `tags` at report time; `query_findings` / `tag_findings` as explicit `tools:` grants, shown in a frontmatter snippet), workflow `action:` steps (`findings.query` / `findings.tag`), and operators through the CLI.
  - **Where tags live:** declared tags on the finding record; later changes in `.rupu/coverage/finding_tags.jsonl` (workspace-wide, append-only, folded on read in file order); who changed what, with the agent/operator actor.
  - **CLI examples:** `rupu findings tag fnd_… --add needs-poc`, the `list --tag … --ids-only | tag - --add …` pipe, `rupu findings tags`, `--untagged`, `--any-tag`, and `--format json`.
  - **Remote units:** tag changes reach the coordinator through the run's coverage stream.
  - **Limits:** tags belong to a finding id, so a re-reported vulnerability starts untagged; the CP UI arrives in Plan 2.

  An example agent frontmatter grant:

```yaml
tools: [read_file, query_findings, tag_findings]
```

- [ ] **Step 2: Add one sentence to CLAUDE.md,** at the end of the `rupu-coverage` crate entry:

```markdown
Finding tags (spec `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md`): declared tags ride `FindingRecord.tags`; later add/remove events live in ONE workspace-wide `.rupu/coverage/finding_tags.jsonl` (its own `.lock`), folded on read in file order by `read_findings` / `read_workspace_findings`; `ledger::tags::apply` is the only writer (`ingest_tag_events` for remote `tags` stream lines); `ledger::query` (`select` unpaged, `query` cursor-paged) is the one filter behind `query_findings` / `findings.query` / `rupu findings list`.
```

- [ ] **Step 3: Mark Plan 1 complete.** In the spec's "Plans" section, change "1. **Plan 1: …**" to "1. **Plan 1 (complete): …**" and add the plan path `docs/superpowers/plans/2026-10-06-rupu-finding-tags-plan-1-core-cli-agent.md`.

- [ ] **Step 4: Commit.**

```bash
git add docs/coverage.md CLAUDE.md docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md
git commit -m "docs: finding tags (coverage.md, CLAUDE.md)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Final verification (after Task 10)

- [ ] `cargo check --workspace --all-targets` is clean.
- [ ] `cargo clippy -p rupu-coverage -p rupu-agent -p rupu-mcp -p rupu-cp -p rupu-cli --all-targets -- -D warnings` is clean.
- [ ] The targeted suites pass:
  - `cargo test -p rupu-coverage --lib ledger::`
  - `cargo test -p rupu-coverage --test it`
  - `cargo test -p rupu-agent --test it findings_`
  - `cargo test -p rupu-mcp --test it`
  - `cargo test -p rupu-cli --test it findings_`
  - `cargo test -p rupu-orchestrator --test it remote_coverage_ingest::`
  - `cargo test -p rupu-cp --test it finding_artifacts::`
  - `cargo test -p rupu-findings-report --test it`
- [ ] A manual smoke test against a scratch `RUPU_HOME` with one seeded finding: `rupu findings tag <id> --add needs-poc && rupu findings list --tag needs-poc && rupu findings tags`.
