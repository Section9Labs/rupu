# CLI Output Polish — Plan 2: Entity Profile

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the CLI's entity lists actually read well — compact identifiers, relative timestamps, status glyphs, no dead columns, and a summary line — by building an `EntityTable` renderer and applying it to `session` and `workflow`.

**Architecture:** A `CellValue` enum describes what each cell *is* (identifier, status, timestamp, plain text, missing) rather than how it looks. `EntityTable` owns the rendering policy for all of them in one place, so every entity list gets identical treatment and later plans adopt it by changing table construction only. Plan 1's helpers (`compact_id`, `relative_time`, `status_glyph`) are consumed here for the first time.

**Tech Stack:** Rust 2021, `comfy-table` 7.2.2 (`custom_styling`), `owo-colors`, `chrono`, `insta`, `anyhow`.

**Spec:** `docs/superpowers/specs/2026-07-30-rupu-cli-output-polish-design.md`
**Builds on:** `docs/superpowers/plans/2026-07-30-rupu-cli-polish-plan-1-foundation-and-resolution.md` (branch `worktree-cli-polish-spec`, PR #570)

## Global Constraints

- Workspace dependency versions are pinned in the root `Cargo.toml` only. Never add a version to a crate `Cargo.toml`.
- `#![deny(clippy::all)]` is workspace-wide. `unsafe_code` is forbidden.
- `rupu-cli` uses `anyhow`; libraries use `thiserror`.
- Unit tests live in-module under `#[cfg(test)] mod tests`.
- **`--format json` and `--format csv` output must remain byte-identical, and always carry full-length identifiers and full ISO timestamps.** Only the human table path changes. Every task that touches a command must assert this.
- **Never run `cargo fmt` in any form** — it reformats ~106 files in this worktree (rustfmt 1.97.1 vs the repo's 1.95 pin). Format only files you touched, with `rustfmt --edition 2021 <path>`, then confirm with `git diff --stat` that only your lines moved.
- **Never run `rustfmt` on `cmd/workflow.rs`, `cmd/autoflow.rs`, `cmd/transcript.rs`, or `rupu-orchestrator/src/runs.rs`** — these carry pre-existing drift and will sprout spurious hunks. Hand-write additions already correctly formatted.
- `crates/rupu-cli/src/cp_inventory.rs` is a pre-existing untracked orphan. Leave it. Never `git add -A`; always add explicit paths.
- Baseline: `cargo clippy -p rupu-cli --lib` has ONE pre-existing error in `cmd/completers.rs` (`question_mark`). Not yours — ignore it, do not fix it.
- Baselines to preserve: `cargo test -p rupu-cli --lib` → 607 passing / 0 failed; `cargo test -p rupu-orchestrator --lib` → 368 / 0.

## Scope of this plan

In: `CellValue`, `EntityTable`, empty-column suppression, summary lines, the `--absolute` and `--all-columns` global flags, `NoLineWrap` on identifier columns, and application to `session list`, `workflow runs`, and `workflow list` (including its enrichment).

Out (later plans):
- **Plan 3** — the remaining 18 entity tables: `cron` (2), `repos` (2), `issues` (1), `agent` (1), `models` (1), `autoflow` (7), `cleanup` (2), `transcript` (2).
- **Plan 4** — the report profile: `coverage` (18), `usage` (4), `auth` (4). Note `coverage.rs`'s `find_manifest` does exact id matching and must be wired through resolution *before* coverage ids are compacted.

Re-scoped from the spec, which folded all 26 entity tables into one plan. Splitting keeps each plan reviewable and gets the visible payoff onto `session`/`workflow` first.

## Carried forward from Plan 1's final review

- `idle` and `skipped` map to a glyph via `status_of` but have no arm in `status_color`, so they would render glyph-without-colour. **Decided in Task 1.**
- `stopped` (a real session status) is unmapped in both. **Decided in Task 1.**
- Identifier columns must not wrap — a compacted id split across two lines is unpasteable, breaking the governing rule by a different route. **Task 1.**
- `Resolution::Ambiguous(Vec<String>)` returns ids only, forcing callers to keep a parallel context map. Both existing callers already work around this (a `scope_of` HashMap for sessions, a `RunCandidate` struct for runs). Deferred to Plan 3 — it is a refactor, not a blocker, and the workaround pattern is established.

---

### Task 1: `CellValue` and the `EntityTable` renderer

**Files:**
- Create: `crates/rupu-cli/src/output/entity_table.rs`
- Modify: `crates/rupu-cli/src/output/mod.rs` (add `pub mod entity_table;`, alphabetical — between `diag` and `fmt`)
- Modify: `crates/rupu-cli/src/output/tables.rs` (extend `status_color` for `idle` / `skipped` / `stopped`)

**Interfaces:**
- Consumes: `output::ids::compact_id`, `output::fmt::relative_time`, `output::tables::{status_glyph, status_color}`, `cmd::ui::UiPrefs`.
- Produces: `output::entity_table::{CellValue, EntityTable}`. Tasks 2, 3, 5, 6, 7 consume both.

**Design note.** `CellValue` describes what a cell *is*; `EntityTable` owns how each kind renders. That is what makes one policy change apply everywhere, and it is why suppression (Task 2) can ask a cell whether it is empty without parsing rendered text.

**Decisions carried from Plan 1's review, settle them here:**
- `idle` → add to `status_color` mapped to `palette.dim` (it is a resting state, like `pending`). It already has the `○` glyph.
- `skipped` → add to `status_color` mapped to `palette.skipped`. Matches `Status::Skipped.color()`.
- `stopped` → add to BOTH: `status_of` mapped to `Status::Skipped` (`⊘` — a terminal non-failure), and `status_color` mapped to `palette.dim`.
- Do NOT collapse `status_of` and `status_color` into one mapping. Plan 1 documented why: `Status::Waiting.color()` is `palette.skipped` while `pending`/`eligible`/`released` render as `palette.dim`. The regression test `pending_keeps_its_dim_colour_not_the_waiting_colour` guards this — it must still pass.

- [ ] **Step 1: Write the failing tests**

Create `crates/rupu-cli/src/output/entity_table.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 30, 12, 0, 0).unwrap()
    }

    fn prefs() -> crate::cmd::ui::UiPrefs {
        let cfg = rupu_config::UiConfig::default();
        crate::cmd::ui::UiPrefs::resolve(&cfg, true, None, None, None)
    }

    #[test]
    fn identifiers_render_compacted() {
        let t = EntityTable::new(
            &prefs(),
            RenderOpts::default(),
            vec!["RUN"],
        )
        .row(vec![CellValue::Id(
            "run_01KRJDKSBE7X4J49094149WFJS".to_string(),
        )]);
        let out = t.render(now());
        assert!(out.contains("run_01KRJDKS…WFJS"), "got: {out}");
        assert!(
            !out.contains("run_01KRJDKSBE7X4J49094149WFJS"),
            "full id must not appear in a table cell: {out}"
        );
    }

    #[test]
    fn timestamps_render_relative_by_default() {
        let then = now() - chrono::Duration::days(14);
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["UPDATED"])
            .row(vec![CellValue::Timestamp(then)]);
        let out = t.render(now());
        assert!(out.contains("2w ago"), "got: {out}");
        assert!(!out.contains("2026-07-16"), "got: {out}");
    }

    #[test]
    fn absolute_opt_renders_iso_dates() {
        let then = now() - chrono::Duration::days(14);
        let opts = RenderOpts {
            absolute: true,
            ..RenderOpts::default()
        };
        let t = EntityTable::new(&prefs(), opts, vec!["UPDATED"])
            .row(vec![CellValue::Timestamp(then)]);
        let out = t.render(now());
        assert!(out.contains("2026-07-16"), "got: {out}");
        assert!(!out.contains("2w ago"), "got: {out}");
    }

    #[test]
    fn statuses_render_with_their_lifecycle_glyph() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["STATUS"])
            .row(vec![CellValue::Status("completed".to_string())])
            .row(vec![CellValue::Status("failed".to_string())])
            .row(vec![CellValue::Status("idle".to_string())]);
        let out = t.render(now());
        assert!(out.contains("✓ completed"), "got: {out}");
        assert!(out.contains("✗ failed"), "got: {out}");
        assert!(out.contains("○ idle"), "got: {out}");
    }

    #[test]
    fn classification_values_get_no_glyph() {
        // `project` is a scope, not a lifecycle position. A glyph would
        // imply progress it doesn't have.
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["SCOPE"])
            .row(vec![CellValue::Status("project".to_string())]);
        let out = t.render(now());
        assert!(out.contains("project"), "got: {out}");
        for glyph in ['✓', '✗', '○', '●', '⏸', '↺', '⊘'] {
            assert!(!out.contains(glyph), "unexpected glyph {glyph} in: {out}");
        }
    }

    #[test]
    fn missing_renders_as_em_dash() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["TARGET"])
            .row(vec![CellValue::Missing]);
        assert!(t.render(now()).contains('—'));
    }

    #[test]
    fn is_empty_distinguishes_missing_from_present() {
        assert!(CellValue::Missing.is_empty());
        assert!(CellValue::Text(String::new()).is_empty());
        assert!(CellValue::Text("—".to_string()).is_empty());
        assert!(!CellValue::Text("x".to_string()).is_empty());
        assert!(!CellValue::Id("run_1".to_string()).is_empty());
        assert!(!CellValue::Status("failed".to_string()).is_empty());
    }

    #[test]
    fn row_length_mismatch_is_rejected() {
        // A row with the wrong arity would silently misalign columns.
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["A", "B"]);
        assert!(t.try_row(vec![CellValue::Text("only-one".into())]).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::entity_table`
Expected: FAIL — module not registered, nothing defined.

- [ ] **Step 3: Register the module**

In `crates/rupu-cli/src/output/mod.rs`, insert alphabetically:

```rust
pub mod diag;
pub mod entity_table;
pub mod fmt;
```

- [ ] **Step 4: Extend the status mappings**

In `crates/rupu-cli/src/output/tables.rs`, add to `status_of`'s match (alongside the existing arms):

```rust
        "stopped" => Status::Skipped,
```

and add to `status_color`'s match:

```rust
        "idle" | "stopped" => palette.dim.into_table(),
        "skipped" => palette.skipped.into_table(),
```

Do not restructure either function. `pending`/`eligible`/`released` must keep `palette.dim`, and the existing regression test must still pass.

- [ ] **Step 5: Write the implementation**

Prepend to `crates/rupu-cli/src/output/entity_table.rs`:

```rust
//! The entity-list rendering profile.
//!
//! Entity lists — sessions, runs, workflows — are scanned by an operator
//! looking for one row to act on. They get compacted identifiers,
//! relative timestamps, lifecycle glyphs, and (via later tasks)
//! empty-column suppression and a summary line.
//!
//! Dense numeric reports (coverage, usage, auth) are a different
//! profile and deliberately do NOT use this — a zero in a coverage grid
//! is data, not absence, so suppression would destroy meaning.
//!
//! [`CellValue`] describes what a cell IS; this module owns how each
//! kind renders. That separation is what lets one policy change apply
//! to every entity list at once, and lets suppression ask a cell whether
//! it is empty without parsing rendered text.

use crate::cmd::ui::UiPrefs;
use crate::output::{fmt, ids, tables};
use chrono::{DateTime, Utc};
use comfy_table::{Cell, ColumnConstraint, Width};

/// What a cell contains, independent of how it is displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellValue {
    /// Free text, rendered verbatim.
    Text(String),
    /// An identifier. Compacted for display and never wrapped, because a
    /// wrapped identifier cannot be copy-pasted back — which would break
    /// the governing rule that every displayed id resolves.
    Id(String),
    /// A lifecycle status or classification value. Lifecycle states get a
    /// glyph; classification values get colour only.
    Status(String),
    /// A point in time. Rendered relative unless `RenderOpts::absolute`.
    Timestamp(DateTime<Utc>),
    /// No value. Renders as an em dash and counts as empty for
    /// column suppression.
    Missing,
}

impl CellValue {
    /// True when this cell carries no information. Drives empty-column
    /// suppression.
    pub fn is_empty(&self) -> bool {
        match self {
            CellValue::Missing => true,
            CellValue::Text(s) => s.is_empty() || s == "—",
            _ => false,
        }
    }
}

/// Rendering options, sourced from global CLI flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderOpts {
    /// Render timestamps as absolute ISO dates instead of relative ages.
    /// For correlating a run against external logs.
    pub absolute: bool,
    /// Keep every column even when it is entirely empty.
    pub all_columns: bool,
}

/// An entity list under construction.
pub struct EntityTable<'a> {
    prefs: &'a UiPrefs,
    opts: RenderOpts,
    headers: Vec<&'static str>,
    rows: Vec<Vec<CellValue>>,
}

impl<'a> EntityTable<'a> {
    pub fn new(prefs: &'a UiPrefs, opts: RenderOpts, headers: Vec<&'static str>) -> Self {
        Self {
            prefs,
            opts,
            headers,
            rows: Vec::new(),
        }
    }

    /// Append a row. Panics on arity mismatch — see [`Self::try_row`] for
    /// the checked form. Intended for call sites that build rows from a
    /// fixed literal, where a mismatch is a programming error.
    pub fn row(mut self, cells: Vec<CellValue>) -> Self {
        assert_eq!(
            cells.len(),
            self.headers.len(),
            "row arity {} does not match {} headers",
            cells.len(),
            self.headers.len()
        );
        self.rows.push(cells);
        self
    }

    /// Append a row, returning an error on arity mismatch rather than
    /// panicking. A mismatched row would silently misalign every column.
    pub fn try_row(&self, cells: Vec<CellValue>) -> anyhow::Result<()> {
        if cells.len() != self.headers.len() {
            anyhow::bail!(
                "row arity {} does not match {} headers",
                cells.len(),
                self.headers.len()
            );
        }
        Ok(())
    }

    /// Render one cell to a comfy-table `Cell`.
    fn cell(&self, value: &CellValue, now: DateTime<Utc>) -> Cell {
        match value {
            CellValue::Text(s) => Cell::new(s),
            CellValue::Id(id) => Cell::new(ids::compact_id(id)),
            CellValue::Missing => Cell::new("—"),
            CellValue::Timestamp(ts) => {
                if self.opts.absolute {
                    Cell::new(ts.to_rfc3339())
                } else {
                    Cell::new(fmt::relative_time(*ts, now))
                }
            }
            CellValue::Status(s) => {
                let text = match tables::status_glyph(s) {
                    Some(glyph) => format!("{glyph} {s}"),
                    None => s.clone(),
                };
                match tables::status_color(s, self.prefs) {
                    Some(c) => Cell::new(text).fg(c),
                    None => Cell::new(text),
                }
            }
        }
    }

    /// Build the table. `now` is a parameter so tests are deterministic;
    /// callers pass `Utc::now()`.
    pub fn render(&self, now: DateTime<Utc>) -> String {
        let mut t = tables::new_table();
        t.set_header(self.headers.clone());
        for row in &self.rows {
            t.add_row(row.iter().map(|c| self.cell(c, now)).collect::<Vec<_>>());
        }
        // Identifier columns must never wrap — a compacted id split
        // across two lines cannot be pasted back.
        for (idx, _) in self.headers.iter().enumerate() {
            if self.rows.iter().any(|r| matches!(r[idx], CellValue::Id(_))) {
                if let Some(col) = t.column_mut(idx) {
                    col.set_constraint(ColumnConstraint::LowerBoundary(Width::Fixed(17)));
                }
            }
        }
        t.to_string()
    }
}
```

Note on the `NoLineWrap` requirement: comfy-table expresses this through `ColumnConstraint`. If `LowerBoundary(Width::Fixed(17))` does not in practice prevent wrapping for a 17-character compact id, use whichever `ColumnConstraint` variant in comfy-table 7.2.2 does (check the enum's variants), and record what you used and why in your report. The requirement is behavioural: **a compact identifier must never be split across lines**. Add a test that renders an id in a narrow table and asserts the compact form appears intact on one line.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib output::entity_table` then `cargo test -p rupu-cli --lib output::tables`
Expected: PASS, including the pre-existing `pending_keeps_its_dim_colour_not_the_waiting_colour`.

- [ ] **Step 7: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/entity_table.rs \
                       crates/rupu-cli/src/output/mod.rs \
                       crates/rupu-cli/src/output/tables.rs
git diff --stat
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/output/entity_table.rs \
        crates/rupu-cli/src/output/mod.rs \
        crates/rupu-cli/src/output/tables.rs
git commit -m "feat(cli): EntityTable renderer and CellValue

CellValue says what a cell IS; EntityTable owns how each kind renders,
so one policy change reaches every entity list and suppression can ask
whether a cell is empty without parsing rendered text.

Settles two mappings Plan 1 deferred: idle/stopped take palette.dim,
skipped takes palette.skipped, and stopped gains the terminal
non-failure glyph. status_of and status_color stay separate — the
pending-colour regression test still guards that.

Identifier columns are constrained against wrapping: a compact id split
across lines cannot be pasted back."
```

---

### Task 2: Empty-column suppression

**Files:**
- Modify: `crates/rupu-cli/src/output/entity_table.rs`

**Interfaces:**
- Consumes: `CellValue::is_empty` from Task 1.
- Produces: suppression behaviour inside `EntityTable::render`, controlled by `RenderOpts::all_columns`.

Suppression is computed over the rows **actually displayed**, not the whole store. Two invocations of the same command with different filters can therefore show different column sets. That is intended — the table describes what is on screen — and Task 4 documents it in `--help`.

- [ ] **Step 1: Write the failing tests**

Append to `entity_table.rs`'s test module:

```rust
    #[test]
    fn all_empty_column_is_dropped() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["A", "TARGET", "B"])
            .row(vec![
                CellValue::Text("1".into()),
                CellValue::Missing,
                CellValue::Text("2".into()),
            ])
            .row(vec![
                CellValue::Text("3".into()),
                CellValue::Missing,
                CellValue::Text("4".into()),
            ]);
        let out = t.render(now());
        assert!(!out.contains("TARGET"), "empty column survived: {out}");
        assert!(out.contains('A') && out.contains('B'), "got: {out}");
    }

    #[test]
    fn partially_populated_column_is_kept() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["A", "TARGET"])
            .row(vec![CellValue::Text("1".into()), CellValue::Missing])
            .row(vec![
                CellValue::Text("2".into()),
                CellValue::Text("here".into()),
            ]);
        let out = t.render(now());
        assert!(out.contains("TARGET"), "populated column dropped: {out}");
        assert!(out.contains("here"), "got: {out}");
    }

    #[test]
    fn all_columns_opt_keeps_empty_columns() {
        let opts = RenderOpts {
            all_columns: true,
            ..RenderOpts::default()
        };
        let t = EntityTable::new(&prefs(), opts, vec!["A", "TARGET"]).row(vec![
            CellValue::Text("1".into()),
            CellValue::Missing,
        ]);
        assert!(t.render(now()).contains("TARGET"));
    }

    #[test]
    fn a_table_with_no_rows_keeps_all_headers() {
        // With zero rows every column is vacuously empty. Dropping them
        // all would render a headerless void instead of an empty list.
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["A", "B"]);
        let out = t.render(now());
        assert!(out.contains('A') && out.contains('B'), "got: {out}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::entity_table`
Expected: FAIL on `all_empty_column_is_dropped` — no suppression exists yet. `a_table_with_no_rows_keeps_all_headers` may pass already; it guards the edge case below.

- [ ] **Step 3: Implement suppression**

Add to `impl EntityTable`:

```rust
    /// Indices of columns worth rendering.
    ///
    /// A column is dropped when every displayed cell is empty. With no
    /// rows at all, every column is vacuously empty — keep them all, or
    /// an empty list renders as a headerless void.
    fn retained_columns(&self) -> Vec<usize> {
        if self.opts.all_columns || self.rows.is_empty() {
            return (0..self.headers.len()).collect();
        }
        (0..self.headers.len())
            .filter(|&i| self.rows.iter().any(|r| !r[i].is_empty()))
            .collect()
    }
```

Rewrite `render` to project through it:

```rust
    pub fn render(&self, now: DateTime<Utc>) -> String {
        let keep = self.retained_columns();
        let mut t = tables::new_table();
        t.set_header(keep.iter().map(|&i| self.headers[i]).collect::<Vec<_>>());
        for row in &self.rows {
            t.add_row(
                keep.iter()
                    .map(|&i| self.cell(&row[i], now))
                    .collect::<Vec<_>>(),
            );
        }
        for (pos, &src) in keep.iter().enumerate() {
            if self.rows.iter().any(|r| matches!(r[src], CellValue::Id(_))) {
                if let Some(col) = t.column_mut(pos) {
                    col.set_constraint(ColumnConstraint::LowerBoundary(Width::Fixed(17)));
                }
            }
        }
        t.to_string()
    }
```

Note the index discipline: `keep` maps display position → source column. The `Id` constraint must be applied at the DISPLAY index (`pos`), while the cell is read from the SOURCE index (`src`). Getting this backwards silently constrains the wrong column once any column is suppressed.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib output::entity_table`
Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/entity_table.rs
git diff --stat
git add crates/rupu-cli/src/output/entity_table.rs
git commit -m "feat(cli): drop entirely-empty columns from entity lists

session list rendered TARGET and RUN as an em dash on every row; those
columns cost width and carried nothing.

Computed over displayed rows, so a filtered invocation can show a
different column set — intended, and surfaced in --help. An empty list
keeps all headers rather than collapsing to a void."
```

---

### Task 3: Summary line

**Files:**
- Modify: `crates/rupu-cli/src/output/entity_table.rs`

**Interfaces:**
- Produces: `EntityTable::with_summary(noun: &'static str)` — builder method; `render` emits the line when set.

Counts by lifecycle status, using the first column whose cells are `CellValue::Status`. Emitted only in the human format; Tasks 5-7 must not emit it under `--format json` / `--format csv`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn summary_counts_by_status() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["ID", "STATUS"])
            .with_summary("session")
            .row(vec![
                CellValue::Id("a".into()),
                CellValue::Status("failed".into()),
            ])
            .row(vec![
                CellValue::Id("b".into()),
                CellValue::Status("idle".into()),
            ])
            .row(vec![
                CellValue::Id("c".into()),
                CellValue::Status("idle".into()),
            ]);
        let out = t.render(now());
        assert!(out.contains("3 sessions"), "got: {out}");
        assert!(out.contains("2 idle"), "got: {out}");
        assert!(out.contains("1 failed"), "got: {out}");
    }

    #[test]
    fn summary_singularises_one() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["STATUS"])
            .with_summary("run")
            .row(vec![CellValue::Status("failed".into())]);
        let out = t.render(now());
        assert!(out.contains("1 run"), "got: {out}");
        assert!(!out.contains("1 runs"), "got: {out}");
    }

    #[test]
    fn summary_absent_when_not_requested() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["STATUS"])
            .row(vec![CellValue::Status("failed".into())]);
        assert!(!t.render(now()).contains("1 "), "unexpected summary");
    }

    #[test]
    fn summary_on_empty_list_says_zero() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["STATUS"])
            .with_summary("session");
        assert!(t.render(now()).contains("0 sessions"));
    }

    #[test]
    fn summary_without_a_status_column_shows_only_the_count() {
        let t = EntityTable::new(&prefs(), RenderOpts::default(), vec!["NAME"])
            .with_summary("workflow")
            .row(vec![CellValue::Text("a".into())])
            .row(vec![CellValue::Text("b".into())]);
        let out = t.render(now());
        assert!(out.contains("2 workflows"), "got: {out}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib output::entity_table`
Expected: FAIL — `with_summary` does not exist.

- [ ] **Step 3: Implement**

Add `summary_noun: Option<&'static str>` to the struct (default `None` in `new`), plus:

```rust
    /// Emit a summary line above the table, e.g.
    /// `8 sessions · 2 failed · 5 idle · 1 stopped`.
    ///
    /// `noun` is the singular form; it is pluralised with a trailing `s`
    /// when the count is not 1. Human output only — callers must not use
    /// this under `--format json` / `--format csv`.
    pub fn with_summary(mut self, noun: &'static str) -> Self {
        self.summary_noun = Some(noun);
        self
    }

    /// `8 sessions · 2 failed · 5 idle`, or just the count when the table
    /// has no status column to break down by.
    fn summary_line(&self, noun: &str) -> String {
        let n = self.rows.len();
        let mut out = if n == 1 {
            format!("{n} {noun}")
        } else {
            format!("{n} {noun}s")
        };
        let status_col = (0..self.headers.len())
            .find(|&i| self.rows.iter().any(|r| matches!(r[i], CellValue::Status(_))));
        if let Some(col) = status_col {
            // BTreeMap keeps the breakdown in a stable order across runs.
            let mut counts: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for row in &self.rows {
                if let CellValue::Status(s) = &row[col] {
                    *counts.entry(s.as_str()).or_default() += 1;
                }
            }
            for (status, count) in counts {
                out.push_str(&format!(" · {count} {status}"));
            }
        }
        out
    }
```

In `render`, when `summary_noun` is set, prepend the line and a blank line before the table.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib output::entity_table`
Expected: PASS.

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/entity_table.rs
git add crates/rupu-cli/src/output/entity_table.rs
git commit -m "feat(cli): summary line for entity lists

The data was always present and never stated. Counts by lifecycle
status in a stable order, falls back to a bare count when the table has
no status column. Human format only."
```

---

### Task 4: `--absolute` and `--all-columns` global flags

**Files:**
- Modify: `crates/rupu-cli/src/lib.rs` (the `Cli` struct, around line 61)
- Modify: `crates/rupu-cli/src/cmd/ui.rs` (`UiPrefs`)

**Interfaces:**
- Produces: `Cli::absolute`, `Cli::all_columns`, and `UiPrefs::with_table_flags(absolute, all_columns) -> Self` plus `UiPrefs::render_opts() -> RenderOpts`. Tasks 5-7 consume `render_opts()`.

**Critical constraint — do NOT extend `UiPrefs::resolve`'s signature.** It has **75 call sites** across the crate. Adding parameters means editing all 75. Instead add the two fields with a `false` default in `resolve`, and a chained builder that only the entity-table commands call.

- [ ] **Step 1: Write the failing tests**

Append to `cmd/ui.rs`'s test module:

```rust
    #[test]
    fn table_flags_default_to_off() {
        let cfg = rupu_config::UiConfig::default();
        let prefs = UiPrefs::resolve(&cfg, true, None, None, None);
        let opts = prefs.render_opts();
        assert!(!opts.absolute);
        assert!(!opts.all_columns);
    }

    #[test]
    fn with_table_flags_sets_both() {
        let cfg = rupu_config::UiConfig::default();
        let prefs = UiPrefs::resolve(&cfg, true, None, None, None).with_table_flags(true, true);
        let opts = prefs.render_opts();
        assert!(opts.absolute);
        assert!(opts.all_columns);
    }
```

And to `lib.rs`'s test module (or wherever CLI parsing is tested — follow the existing pattern, e.g. `session_start_parses_view_mode` in `cmd/session.rs`):

```rust
    #[test]
    fn global_table_flags_parse() {
        let cli = crate::Cli::try_parse_from(["rupu", "--absolute", "session", "list"])
            .expect("cli parses");
        assert!(cli.absolute);
        let cli = crate::Cli::try_parse_from(["rupu", "--all-columns", "session", "list"])
            .expect("cli parses");
        assert!(cli.all_columns);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib ui:: ` and the CLI parse test.
Expected: FAIL — fields and methods do not exist.

- [ ] **Step 3: Add the flags**

In `crates/rupu-cli/src/lib.rs`'s `Cli`, alongside `format`:

```rust
    /// Show absolute ISO timestamps in tables instead of relative ages.
    /// Useful when correlating a run against external logs.
    #[arg(long, global = true)]
    pub absolute: bool,

    /// Keep every table column, including ones that are empty for every
    /// row shown. Columns are otherwise suppressed based on the rows
    /// actually displayed, so a filtered listing may show fewer columns.
    #[arg(long, global = true)]
    pub all_columns: bool,
```

The `--all-columns` help text carries the suppression caveat, per Task 2.

- [ ] **Step 4: Extend `UiPrefs`**

Add the two fields, defaulted to `false` inside `resolve` (so all 75 call sites keep compiling untouched), plus:

```rust
    /// Apply the global table-rendering flags. Chained by commands that
    /// render entity lists; every other caller keeps the defaults.
    ///
    /// Deliberately a builder rather than two more `resolve` parameters —
    /// `resolve` has 75 call sites and none of the others care.
    pub fn with_table_flags(mut self, absolute: bool, all_columns: bool) -> Self {
        self.absolute = absolute;
        self.all_columns = all_columns;
        self
    }

    /// The entity-table rendering options implied by these prefs.
    pub fn render_opts(&self) -> crate::output::entity_table::RenderOpts {
        crate::output::entity_table::RenderOpts {
            absolute: self.absolute,
            all_columns: self.all_columns,
        }
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib` — the full lib suite, since `UiPrefs` is constructed in many places.
Expected: PASS, ≥607 passing.

- [ ] **Step 6: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/ui.rs
git diff --stat
git add crates/rupu-cli/src/lib.rs crates/rupu-cli/src/cmd/ui.rs
git commit -m "feat(cli): --absolute and --all-columns global flags

Carried on UiPrefs via a chained builder rather than two more
resolve() parameters — resolve has 75 call sites and none of the
others care about table rendering."
```

---

### Task 5: Apply the profile to `session list`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/session.rs` (`render_table`, around line 599)

**Interfaces:**
- Consumes: `EntityTable`, `CellValue`, `RenderOpts`, `UiPrefs::render_opts`.

Current output — note `SCOPE` is `active` on every row, and `TARGET` / `RUN` are `—` on every row, so suppression should drop at least the latter two:

```
 SESSION                          AGENT             SCOPE    STATUS   TARGET  RUN  UPDATED
 ses_01KWA7HTYEDX0ACG93ZW26FG3M   oracle-assessor   active   failed   —       —    2026-07-16T22:23:19.165766+00:00
```

Target:

```
  8 sessions · 2 failed · 5 idle · 1 stopped

 SESSION             AGENT             SCOPE    STATUS     UPDATED
 ses_01KWA7HT…FG3M   oracle-assessor   active   ✗ failed   2w ago
```

**The timestamp problem.** `row.updated_at` is a `String` (an ISO timestamp), but `CellValue::Timestamp` needs a `DateTime<Utc>`. Parse it with `chrono::DateTime::parse_from_rfc3339` and convert to `Utc`. If parsing fails, fall back to `CellValue::Text(row.updated_at.clone())` rather than dropping the row — a malformed stored timestamp must not make a session invisible. Add a test for the malformed case.

- [ ] **Step 1: Write the failing test**

Append to `cmd/session.rs`'s test module:

```rust
    #[test]
    fn session_list_table_compacts_ids_and_drops_empty_columns() {
        let report = SessionListReport {
            rows: vec![session_list_row_for_test(
                "ses_01KWA7HTYEDX0ACG93ZW26FG3M",
                "oracle-assessor",
                "active",
                "failed",
                None,
                None,
                "2026-07-16T22:23:19.165766+00:00",
            )],
            ..Default::default()
        };
        let out = render_session_list_table(&report, test_prefs(), test_now());

        assert!(out.contains("ses_01KWA7HT…FG3M"), "got: {out}");
        assert!(
            !out.contains("ses_01KWA7HTYEDX0ACG93ZW26FG3M"),
            "full id leaked into a cell: {out}"
        );
        assert!(out.contains("✗ failed"), "got: {out}");
        assert!(!out.contains("TARGET"), "empty TARGET survived: {out}");
        assert!(!out.contains("RUN"), "empty RUN survived: {out}");
        assert!(out.contains("1 session"), "summary missing: {out}");
        assert!(
            !out.contains("2026-07-16T22:23:19"),
            "raw ISO timestamp in a cell: {out}"
        );
    }

    #[test]
    fn session_list_survives_an_unparseable_timestamp() {
        let report = SessionListReport {
            rows: vec![session_list_row_for_test(
                "ses_01KWA7HTYEDX0ACG93ZW26FG3M",
                "oracle-assessor",
                "active",
                "failed",
                None,
                None,
                "not-a-timestamp",
            )],
            ..Default::default()
        };
        let out = render_session_list_table(&report, test_prefs(), test_now());
        assert!(out.contains("not-a-timestamp"), "row was dropped: {out}");
    }
```

Extract the table construction into a testable `render_session_list_table(&SessionListReport, &UiPrefs, DateTime<Utc>) -> String` so it can be asserted without stdout capture; `render_table` becomes a thin `println!` wrapper. Write the `session_list_row_for_test` / `test_prefs` / `test_now` helpers to match the real `SessionListRow` field names — read the struct rather than guessing, and if `SessionListReport` has no `Default`, construct it fully.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cli --lib cmd::session::tests::session_list`
Expected: FAIL — the helper does not exist.

- [ ] **Step 3: Implement**

Replace the body of `render_table` with a call to the new function, which builds an `EntityTable` with headers `["SESSION", "AGENT", "SCOPE", "STATUS", "TARGET", "RUN", "UPDATED"]` and per row:

- `SESSION` → `CellValue::Id(row.session_id.clone())`
- `AGENT` → `CellValue::Text(row.agent.clone())`
- `SCOPE` → `CellValue::Status(row.scope.clone())` (classification — colour, no glyph)
- `STATUS` → `CellValue::Status(row.status.clone())`
- `TARGET` → `row.target` mapped to `CellValue::Text` / `CellValue::Missing`
- `RUN` → `row.active_run_id` mapped to `CellValue::Id` / `CellValue::Missing`
- `UPDATED` → parsed `CellValue::Timestamp`, falling back to `CellValue::Text`

with `.with_summary("session")`. Pass `Utc::now()` from `render_table`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib cmd::session`
Expected: PASS.

- [ ] **Step 5: Verify the JSON contract and the real binary**

```bash
cargo run -p rupu-cli -- session list --format json | head -5
cargo run -p rupu-cli -- session list | head -8
cargo run -p rupu-cli -- session list --absolute | head -4
cargo run -p rupu-cli -- session list --all-columns | head -4
```
The JSON must be unchanged from before this task and must still carry full-length ids and full ISO timestamps. Paste all four outputs into your report.

- [ ] **Step 6: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/session.rs
git diff --stat
git add crates/rupu-cli/src/cmd/session.rs
git commit -m "feat(cli): session list adopts the entity profile

Compact ids, status glyphs, relative timestamps, a summary line, and
TARGET/RUN dropped when empty for every row shown.

Table construction is extracted so it can be asserted directly instead
of through stdout. An unparseable stored timestamp falls back to
verbatim text rather than hiding the session. --format json is
unchanged and still carries full ids and full ISO timestamps."
```

---

### Task 6: Apply the profile to `workflow runs`

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (the `workflow runs` table — `set_header` around line 907)

**Interfaces:**
- Consumes: `EntityTable`, `CellValue`, `RenderOpts`.

Current columns: `RUN ID`, `STATUS`, `STARTED (UTC)`, `DURATION`, `EXPIRES`, `TOKENS`, `COST`, `WORKFLOW`. `EXPIRES` is blank for every row in real data, so suppression should drop it.

Target:

```
  20 runs · 14 completed · 6 failed

 RUN ID              STATUS        STARTED   DURATION   TOKENS   COST      WORKFLOW
 run_01KYSMDN…GKYJ   ✓ completed   4h ago    194s       1.82M    $5.5220   nightly-maintainability-security
```

**Reminder: do NOT run `rustfmt` on `cmd/workflow.rs`** — it has pre-existing drift. Hand-write your additions correctly formatted.

**The real row struct** (`workflow.rs:580`) — use these field names, they are verified:

```rust
struct WorkflowRunsRow {
    run_id: String,
    status: String,
    started_at: String,              // ISO string, parse it
    duration_seconds: Option<i64>,
    expires_in_seconds: Option<i64>, // a DURATION, not a timestamp
    total_tokens: u64,
    cost_usd: Option<f64>,
    workflow: String,
}
```

Note `expires_in_seconds` is seconds-until, **not** a point in time — do not map it to `CellValue::Timestamp`. `tables.rs` already has `format_seconds` and `relative_time_cell` for this shape; render it as `CellValue::Text(format_seconds(s))` when `Some`, `CellValue::Missing` when `None`.

- [ ] **Step 1: Write the failing tests**

Append to `cmd/workflow.rs`'s test module:

```rust
    fn runs_row_for_test(
        run_id: &str,
        status: &str,
        started_at: &str,
        expires_in_seconds: Option<i64>,
    ) -> WorkflowRunsRow {
        WorkflowRunsRow {
            run_id: run_id.to_string(),
            status: status.to_string(),
            started_at: started_at.to_string(),
            duration_seconds: Some(194),
            expires_in_seconds,
            total_tokens: 1_820_000,
            cost_usd: Some(5.522),
            workflow: "nightly-maintainability-security".to_string(),
        }
    }

    fn runs_test_now() -> chrono::DateTime<chrono::Utc> {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(2026, 7, 30, 17, 0, 0).unwrap()
    }

    fn runs_test_prefs() -> crate::cmd::ui::UiPrefs {
        let cfg = rupu_config::UiConfig::default();
        crate::cmd::ui::UiPrefs::resolve(&cfg, true, None, None, None)
    }

    #[test]
    fn workflow_runs_table_compacts_glyphs_and_drops_empty_expires() {
        let rows = vec![
            runs_row_for_test(
                "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
                "completed",
                "2026-07-30T13:46:31+00:00",
                None,
            ),
            runs_row_for_test(
                "run_01KYPASX18NYRER5NQPDWB2HZV",
                "failed",
                "2026-07-29T07:00:43+00:00",
                None,
            ),
        ];
        let out = render_workflow_runs_table(
            &rows,
            &runs_test_prefs(),
            crate::output::entity_table::RenderOpts::default(),
            runs_test_now(),
        );

        assert!(out.contains("run_01KYSMDN…GKYJ"), "got: {out}");
        assert!(
            !out.contains("run_01KYSMDNG84N9Z8XXHQZP3GKYJ"),
            "full id leaked into a cell: {out}"
        );
        assert!(out.contains("✓ completed"), "got: {out}");
        assert!(out.contains("✗ failed"), "got: {out}");
        assert!(out.contains("3h ago"), "relative start missing: {out}");
        assert!(!out.contains("EXPIRES"), "empty EXPIRES survived: {out}");
        assert!(out.contains("2 runs"), "summary missing: {out}");
    }

    #[test]
    fn workflow_runs_keeps_expires_when_a_run_has_one() {
        // Suppression must be driven by the data, not hardcoded.
        let rows = vec![
            runs_row_for_test("run_a1b2c3d4e5f6g7h8i9j0k1l2", "running", "2026-07-30T16:00:00+00:00", Some(300)),
            runs_row_for_test("run_z9y8x7w6v5u4t3s2r1q0p9o8", "completed", "2026-07-30T15:00:00+00:00", None),
        ];
        let out = render_workflow_runs_table(
            &rows,
            &runs_test_prefs(),
            crate::output::entity_table::RenderOpts::default(),
            runs_test_now(),
        );
        assert!(out.contains("EXPIRES"), "populated EXPIRES dropped: {out}");
    }

    #[test]
    fn workflow_runs_survives_an_unparseable_started_at() {
        let rows = vec![runs_row_for_test(
            "run_01KYSMDNG84N9Z8XXHQZP3GKYJ",
            "completed",
            "not-a-timestamp",
            None,
        )];
        let out = render_workflow_runs_table(
            &rows,
            &runs_test_prefs(),
            crate::output::entity_table::RenderOpts::default(),
            runs_test_now(),
        );
        assert!(out.contains("not-a-timestamp"), "row was dropped: {out}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib cmd::workflow::tests::workflow_runs`
Expected: FAIL — `render_workflow_runs_table` does not exist.

- [ ] **Step 3: Implement**

Extract the table construction out of `render_table` into:

```rust
/// Build the `workflow runs` table. Split out of `render_table` so it
/// can be asserted directly instead of through captured stdout.
///
/// `now` is a parameter for deterministic tests; the caller passes
/// `Utc::now()`.
fn render_workflow_runs_table(
    rows: &[WorkflowRunsRow],
    prefs: &crate::cmd::ui::UiPrefs,
    opts: crate::output::entity_table::RenderOpts,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    use crate::output::entity_table::{CellValue, EntityTable};

    let mut table = EntityTable::new(
        prefs,
        opts,
        vec![
            "RUN ID", "STATUS", "STARTED", "DURATION", "EXPIRES", "TOKENS", "COST", "WORKFLOW",
        ],
    )
    .with_summary("run");

    for row in rows {
        // A malformed stored timestamp must not hide the run.
        let started = match chrono::DateTime::parse_from_rfc3339(&row.started_at) {
            Ok(ts) => CellValue::Timestamp(ts.with_timezone(&chrono::Utc)),
            Err(_) => CellValue::Text(row.started_at.clone()),
        };
        table = table.row(vec![
            CellValue::Id(row.run_id.clone()),
            CellValue::Status(row.status.clone()),
            started,
            match row.duration_seconds {
                Some(s) => CellValue::Text(format!("{s}s")),
                None => CellValue::Missing,
            },
            // seconds-until, not a point in time
            match row.expires_in_seconds {
                Some(s) => CellValue::Text(crate::output::tables::format_seconds(s)),
                None => CellValue::Missing,
            },
            CellValue::Text(crate::output::fmt::format_token_compact(row.total_tokens)),
            match row.cost_usd {
                Some(c) => CellValue::Text(format!("${c:.4}")),
                None => CellValue::Missing,
            },
            CellValue::Text(row.workflow.clone()),
        ]);
    }
    table.render(now)
}
```

`render_table` becomes `println!("{}", render_workflow_runs_table(&self.report.rows, &self.prefs, self.prefs.render_opts(), chrono::Utc::now()));`.

`format_seconds` is currently private in `tables.rs` (around line 269) — make it `pub(crate)`. Keep the existing `$#.4` cost precision so the column matches today's output.

The header `STARTED (UTC)` becomes `STARTED`, since a relative age is not UTC-specific. Under `--absolute` it renders ISO with offset, which is self-describing.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib cmd::workflow`
Expected: PASS.

- [ ] **Step 5: Verify the JSON contract and the real binary**

```bash
cargo run -p rupu-cli -- workflow runs --format json | head -5
cargo run -p rupu-cli -- workflow runs | head -8
cargo run -p rupu-cli -- workflow runs --absolute | head -4
```
JSON unchanged, full ids, full ISO timestamps. Paste all three.

- [ ] **Step 6: Commit**

```bash
git diff --stat   # confirm ONLY your lines — no rustfmt drift
git add crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(cli): workflow runs adopts the entity profile

Compact ids, status glyphs, relative start times, a summary line, and
EXPIRES dropped when empty for every row shown — driven by the data,
so a run with a real expiry keeps the column.

--format json unchanged, still full ids and full ISO timestamps."
```

---

### Task 7: `workflow list` enrichment

**Files:**
- Modify: `crates/rupu-cli/src/cmd/workflow.rs` (the `workflow list` table — `set_header(vec!["NAME", "SCOPE"])` at line 641)

**Interfaces:**
- Consumes: `EntityTable`, `CellValue`, `RunStore::list`.

`workflow list` currently emits NAME and SCOPE only. Target adds STEPS, LAST RUN, and SCHEDULE:

```
  18 workflows

 NAME                               SCOPE     STEPS   LAST RUN        SCHEDULE
 nightly-maintainability-security   project   7       ✓ 4h ago        0 7 * * *
 pr-code-review                     project   3       ✗ 2d ago        —
 action-demo                        project   2       —               —
```

**⚠️ JSON-contract constraint — read before writing any code.** `WorkflowListRow` (`workflow.rs:574`) is `#[derive(Serialize)]` and feeds both `--format json` and `--format csv` (whose headers are the literal `["name", "scope"]`). **Do NOT add fields to it** — that would change the frozen JSON shape. The enrichment is human-table-only. Build a separate display struct and leave `WorkflowListRow` exactly as it is.

**What `workflow list` does today:** it does NOT parse workflows at all. `push_yaml_names` (`workflow.rs:983`) walks directories collecting `BTreeMap<name, scope>` from filenames. So STEPS and SCHEDULE require newly parsing each workflow file — roughly one read + YAML parse per workflow (about 18 today). That is acceptable, but it is new I/O on a command that previously did none, so it must degrade gracefully: a workflow that fails to parse still renders as a row with `Missing` in STEPS and SCHEDULE. One malformed file must never blank the listing.

`rupu_orchestrator::Workflow` (`crates/rupu-orchestrator/src/workflow.rs:1087`) carries `name`, `description`, `trigger: Trigger`, `inputs`, and the step list. Read that struct and the `Trigger` enum for the exact field names and the cron variant rather than guessing, and record what you used in your report.

**The performance constraint — the substantive part of this task.** A naive implementation reads the run store once per workflow row: O(workflows × runs), and `RunStore::list()` deserializes every `run.json`. Read the run list ONCE and fold it into a latest-run-per-workflow map. `RunRecord`'s identifier field is `id`, not `run_id`; it carries `workflow_name`, `status`, and `started_at`.

- [ ] **Step 1: Write the failing tests for the pure join**

The join is pure and is where the performance property lives, so it gets its own direct tests. Append to `cmd/workflow.rs`'s test module:

```rust
    fn run_for_test(
        id: &str,
        workflow_name: &str,
        status: rupu_orchestrator::RunStatus,
        started_at: chrono::DateTime<chrono::Utc>,
    ) -> LatestRun {
        LatestRun {
            workflow_name: workflow_name.to_string(),
            status: format!("{status:?}").to_lowercase(),
            started_at,
            run_id: id.to_string(),
        }
    }

    #[test]
    fn latest_run_keeps_only_the_most_recent_per_workflow() {
        use chrono::TimeZone;
        let older = chrono::Utc.with_ymd_and_hms(2026, 7, 28, 9, 0, 0).unwrap();
        let newer = chrono::Utc.with_ymd_and_hms(2026, 7, 30, 9, 0, 0).unwrap();
        let runs = vec![
            run_for_test("run_old", "nightly", rupu_orchestrator::RunStatus::Failed, older),
            run_for_test("run_new", "nightly", rupu_orchestrator::RunStatus::Completed, newer),
            run_for_test("run_other", "review", rupu_orchestrator::RunStatus::Completed, older),
        ];
        let map = latest_run_by_workflow(&runs);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("nightly").unwrap().run_id, "run_new");
        assert_eq!(map.get("review").unwrap().run_id, "run_other");
    }

    #[test]
    fn latest_run_on_an_empty_list_is_empty() {
        assert!(latest_run_by_workflow(&[]).is_empty());
    }
```

Define `LatestRun` as a small owned struct in `cmd/workflow.rs` (`workflow_name`, `status`, `started_at`, `run_id`) built from `RunRecord`, so the join is testable without constructing full `RunRecord`s — the same reason Plan 1 split `resolve_run_id` from `resolve_run_fragment`: `RunStore::create` needs a fully-populated `RunRecord` and `sample_record` is test-private to `rupu-orchestrator`.

- [ ] **Step 2: Write the failing tests for the table**

```rust
    #[test]
    fn workflow_list_table_shows_steps_last_run_and_schedule() {
        use chrono::TimeZone;
        let now = chrono::Utc.with_ymd_and_hms(2026, 7, 30, 17, 0, 0).unwrap();
        let ran = chrono::Utc.with_ymd_and_hms(2026, 7, 30, 13, 0, 0).unwrap();
        let rows = vec![
            WorkflowListDisplayRow {
                name: "nightly-health".to_string(),
                scope: "project".to_string(),
                steps: Some(7),
                schedule: Some("0 7 * * *".to_string()),
                last_run: Some(("completed".to_string(), ran)),
            },
            WorkflowListDisplayRow {
                name: "action-demo".to_string(),
                scope: "project".to_string(),
                steps: Some(2),
                schedule: None,
                last_run: None,
            },
        ];
        let out = render_workflow_list_table(
            &rows,
            &runs_test_prefs(),
            crate::output::entity_table::RenderOpts::default(),
            now,
        );

        assert!(out.contains("STEPS"), "got: {out}");
        assert!(out.contains("LAST RUN"), "got: {out}");
        assert!(out.contains("SCHEDULE"), "got: {out}");
        assert!(out.contains("✓ completed"), "got: {out}");
        assert!(out.contains("4h ago"), "got: {out}");
        assert!(out.contains("0 7 * * *"), "got: {out}");
        assert!(out.contains("2 workflows"), "summary missing: {out}");
    }

    #[test]
    fn workflow_list_renders_an_unparseable_workflow_as_a_row() {
        // One malformed file must not blank the listing.
        let now = chrono::Utc::now();
        let rows = vec![WorkflowListDisplayRow {
            name: "broken".to_string(),
            scope: "project".to_string(),
            steps: None,
            schedule: None,
            last_run: None,
        }];
        let out = render_workflow_list_table(
            &rows,
            &runs_test_prefs(),
            crate::output::entity_table::RenderOpts::default(),
            now,
        );
        assert!(out.contains("broken"), "row was dropped: {out}");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rupu-cli --lib cmd::workflow::tests::latest_run` then `... ::workflow_list`
Expected: FAIL — none of these types or functions exist.

- [ ] **Step 4: Implement**

```rust
/// One workflow's row in the HUMAN listing.
///
/// Deliberately separate from `WorkflowListRow`, which is `Serialize`
/// and feeds `--format json` / `--format csv`. That shape is frozen by
/// contract, so enrichment lives here and never touches it.
struct WorkflowListDisplayRow {
    name: String,
    scope: String,
    /// `None` when the workflow file could not be parsed.
    steps: Option<usize>,
    /// `None` when the workflow has no cron trigger.
    schedule: Option<String>,
    /// `(status, started_at)` of the most recent run, if any.
    last_run: Option<(String, chrono::DateTime<chrono::Utc>)>,
}

/// The fields of a run that the workflow listing needs.
///
/// An owned projection rather than a borrowed `RunRecord`, so the join
/// below is unit-testable without building full records — `RunStore`'s
/// only record builder is test-private to `rupu-orchestrator`.
struct LatestRun {
    workflow_name: String,
    status: String,
    started_at: chrono::DateTime<chrono::Utc>,
    run_id: String,
}

/// Most recent run per workflow name.
///
/// Pure, and the reason the listing stays O(workflows + runs): the run
/// store is read ONCE by the caller and folded here. Reading it per row
/// would be O(workflows x runs), and `RunStore::list` deserializes every
/// `run.json`.
fn latest_run_by_workflow(runs: &[LatestRun]) -> std::collections::HashMap<&str, &LatestRun> {
    let mut out: std::collections::HashMap<&str, &LatestRun> = std::collections::HashMap::new();
    for run in runs {
        out.entry(run.workflow_name.as_str())
            .and_modify(|existing| {
                if run.started_at > existing.started_at {
                    *existing = run;
                }
            })
            .or_insert(run);
    }
    out
}

fn render_workflow_list_table(
    rows: &[WorkflowListDisplayRow],
    prefs: &crate::cmd::ui::UiPrefs,
    opts: crate::output::entity_table::RenderOpts,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    use crate::output::entity_table::{CellValue, EntityTable};

    let mut table = EntityTable::new(
        prefs,
        opts,
        vec!["NAME", "SCOPE", "STEPS", "LAST RUN", "SCHEDULE"],
    )
    .with_summary("workflow");

    for row in rows {
        table = table.row(vec![
            CellValue::Text(row.name.clone()),
            CellValue::Status(row.scope.clone()),
            match row.steps {
                Some(n) => CellValue::Text(n.to_string()),
                None => CellValue::Missing,
            },
            match &row.last_run {
                // Glyph + relative age in one cell: the two facts an
                // operator scans this column for.
                Some((status, at)) => CellValue::Text(format!(
                    "{} {}",
                    crate::output::tables::status_glyph(status)
                        .map(|g| g.to_string())
                        .unwrap_or_default(),
                    crate::output::fmt::relative_time(*at, now),
                )),
                None => CellValue::Missing,
            },
            match &row.schedule {
                Some(s) => CellValue::Text(s.clone()),
                None => CellValue::Missing,
            },
        ]);
    }
    table.render(now)
}
```

Then wire the caller: after `by_name` is built (`workflow.rs:974`), read the run store once, project it into `Vec<LatestRun>`, fold with `latest_run_by_workflow`, parse each workflow for `steps`/`schedule` (tolerating parse failure as `None`), and build `Vec<WorkflowListDisplayRow>`. `render_table` calls `render_workflow_list_table`. `WorkflowListRow` and the JSON/CSV path stay untouched.

Note the `LAST RUN` cell is `Text`, not `Status` — it carries a glyph AND a relative time, so `EntityTable`'s status rendering would double up the glyph. Colour is therefore not applied to it; if you want the status colour there, add it explicitly and say so in your report.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rupu-cli --lib cmd::workflow`
Expected: PASS.

- [ ] **Step 5: Verify**

```bash
cargo run -p rupu-cli -- workflow list --format json | head -5
cargo run -p rupu-cli -- workflow list
time cargo run -p rupu-cli -- workflow list   # sanity: should not scale with run count
```
JSON unchanged. Paste all three, including the timing.

- [ ] **Step 6: Commit**

```bash
git diff --stat
git add crates/rupu-cli/src/cmd/workflow.rs
git commit -m "feat(cli): workflow list shows steps, last run, and schedule

NAME + SCOPE alone couldn't answer 'did this run, and did it work'.

The run store is read once and folded into a latest-run-per-workflow
map, then joined in memory — reading per row would be
O(workflows x runs) and RunStore::list deserializes every run.json. A
workflow that fails to parse still renders as a row."
```

---

## Verification

Before opening the PR:

- [ ] `cargo test -p rupu-cli` passes in full (≥607 lib, 0 failed).
- [ ] `cargo test -p rupu-orchestrator --lib` still 368 / 0.
- [ ] `cargo clippy -p rupu-cli --lib` introduces no new warnings beyond the pre-existing `completers.rs` error.
- [ ] `session list`, `workflow runs`, and `workflow list` all show compact ids, glyphs, relative times, and a summary line.
- [ ] Every compacted id shown is still resolvable — pick one from each listing and confirm `session show` / `workflow show-run` accept it. This is the governing rule and Plan 1's whole point; a regression here is Critical.
- [ ] `--absolute` restores ISO timestamps; `--all-columns` restores suppressed columns.
- [ ] `--format json` is byte-identical to pre-branch output for all three commands, with full-length ids and full ISO timestamps.
- [ ] No table wraps an identifier across lines at a narrow terminal width (try `COLUMNS=60`).

## Follow-on plans

- **Plan 3** — remaining entity tables (18 sites: cron, repos, issues, agent, models, autoflow, cleanup, transcript), plus making `Resolution::Ambiguous` carry context generically instead of via per-caller side maps.
- **Plan 4** — report profile (26 sites: coverage, usage, auth). `coverage.rs`'s `find_manifest` must be wired through resolution before coverage ids are compacted, or compacted coverage ids will not resolve.
