# CLI Output Polish — Plan 4: Report Readability

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `coverage` and `usage` scannable — severity colour, right-aligned numbers, compact-but-resolvable run ids, relative timestamps — without forcing 26 heterogeneous tables through one abstraction.

**Architecture:** Additive helpers on the existing `output/tables.rs`, applied per table where they earn their place. No new profile type.

**Tech Stack:** Rust 2021, `comfy-table` 7.2.2, `owo-colors`, `chrono`, `anyhow`.

**Spec:** `docs/superpowers/specs/2026-07-30-rupu-cli-output-polish-design.md`
**Builds on:** Plan 2 (PR #573) and Plan 1 (PR #570).

## Why this deviates from the spec

Spec §2 assigns coverage (18), usage (4), and auth (4) to a `ReportTable` profile, justified by "a zero in a coverage grid is data, not absence."

A survey of all 26 call sites found that rationale applies to **exactly one table**: `coverage audit`'s grid (`coverage.rs:567`), whose seven count columns are genuinely measured-and-zero. The rest:

- 15 identifier / path / classification lists with no zero-sensitive numerics
- 2 hybrids — identifier plus metric columns (`coverage runs`, `usage runs`)
- 1 numeric grid with a TOTAL footer (`usage` breakdown)
- 5 fixed KEY/VALUE detail cards (`usage` summary + backfill, `auth` backend state / paths / commands) — not grids at all
- 1 provider status list that already has colour (`auth status`)

Building a profile type to serve one table is abstraction for its own sake. **The user's directive is explicit: prioritise UI/UX over standardisation.** This plan does the readability work directly and leaves structurally-fine tables alone.

## Global Constraints

- Workspace dependency versions pinned in the root `Cargo.toml` only.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden. `rupu-cli` uses `anyhow`.
- Unit tests live in-module under `#[cfg(test)] mod tests`.
- **`--format json` / `--format csv` byte-identical** for every command touched. Note `coverage runs` has TWO frozen shapes: `RunListEntry` (table + JSON) and a separate `RunRow` (`coverage.rs:875`) for CSV. Both must stay compatible.
- **Never run `cargo fmt`** — reformats ~106 files here (rustfmt 1.97.1 vs the 1.95 pin). Use `rustfmt --edition 2021 <path>` and verify with `git diff --stat`.
- **Never `rustfmt` module-root files** (`lib.rs`, `output/mod.rs`) — they cascade into siblings (`lib.rs` measured hitting 12).
- **Never use `git stash` in any form.** The stash stack is shared across every worktree of this repo; a bare `stash pop` in an earlier session applied an unrelated entry and dirtied 104 files. Use `git checkout -- <paths>`.
- Never `git add -A`; always explicit paths.
- Baseline: `cargo clippy -p rupu-cli --lib` has ONE pre-existing error in `cmd/completers.rs` (`question_mark`). Not yours.
- Baselines: `cargo test -p rupu-cli --lib` → 648 / 0; `cargo build -p rupu-cli --lib` → **0 warnings**.
- **`crates/rupu-cli/tests/auth_status_table.rs` pins the literal strings `PROVIDER`, `API-KEY`, `SSO` by substring.** Do not change auth header casing.

## Scope

**In:** severity + assertion-status colour; numeric right-alignment; run-id resolution wired into `coverage rerun` / `coverage diff`; then compact ids + relative timestamps on `coverage runs`; colour + alignment on `coverage audit`; alignment + ids + relative time on `usage`.

**Out, deliberately:**
- The 5 fixed KEY/VALUE cards. Two-column cards with a stable row set; colour and alignment buy nothing.
- `auth status`. Already colours, nothing to compact or align, headers pinned by a test.
- Wholesale conversion of the 15 identifier lists. They get severity colour where Task 4 touches them; the rest is churn.
- Header-casing unification (coverage Title Case vs usage/auth SCREAMING_CASE). Cosmetic, and the auth test pins one side.

---

### Task 1: Severity and assertion-status colour vocabulary

**Files:** Modify `crates/rupu-cli/src/output/tables.rs`

**Interfaces:** Produces `severity_color(&str, &UiPrefs) -> Option<TableColor>` and `coverage_status_color(&str, &UiPrefs) -> Option<TableColor>`. Tasks 3-4 consume both.

**Why first.** `coverage.rs` contains **zero** `.fg()` calls and never touches `status_color`. Severity renders as bare `Critical` / `High` / `Low` text, in a tool whose purpose is surfacing findings. This is the largest readability win available.

`status_color`'s vocabulary covers lifecycle states and a few classification values — it knows nothing of `Severity` or `AssertionStatus`. Add separate functions rather than extending it; merging unrelated vocabularies repeats a mistake this project already documented (see `status_of`'s docstring on why glyph and colour mappings stay apart).

Values arrive as Rust `Debug` reprs: `Critical`/`High`/`Medium`/`Low`/`Info`, and `Clean`/`Finding`/`Examined`/`NotApplicable`. Match case-insensitively so a call site that lowercases doesn't silently lose colour.

**Verify the palette field names** (`sev_critical`, `sev_high`, `sev_medium`, `sev_low`, `sev_info`) against `crates/rupu-cli/src/output/palette.rs` before writing. The survey reports them on `UiPaletteTheme`; confirm rather than trust.

- [ ] **Step 1: Write the failing tests**

Append to `tables.rs`'s existing `mod tests` (reuse the existing `prefs_color_always()` / `prefs_no_color()` helpers):

```rust
    #[test]
    fn severity_color_maps_the_five_levels() {
        let prefs = prefs_color_always();
        let p = crate::output::palette::active_palette();
        assert_eq!(severity_color("Critical", &prefs), Some(p.sev_critical.into_table()));
        assert_eq!(severity_color("High", &prefs), Some(p.sev_high.into_table()));
        assert_eq!(severity_color("Medium", &prefs), Some(p.sev_medium.into_table()));
        assert_eq!(severity_color("Low", &prefs), Some(p.sev_low.into_table()));
        assert_eq!(severity_color("Info", &prefs), Some(p.sev_info.into_table()));
    }

    #[test]
    fn severity_color_is_case_insensitive() {
        // Call sites may or may not lowercase the Debug repr; a casing
        // change must not silently drop the colour.
        let prefs = prefs_color_always();
        assert_eq!(severity_color("critical", &prefs), severity_color("Critical", &prefs));
        assert_eq!(severity_color("HIGH", &prefs), severity_color("High", &prefs));
    }

    #[test]
    fn severity_color_rejects_unknown_and_respects_no_color() {
        assert_eq!(severity_color("banana", &prefs_color_always()), None);
        assert_eq!(severity_color("Critical", &prefs_no_color()), None);
    }

    #[test]
    fn coverage_status_color_maps_assertion_states() {
        let prefs = prefs_color_always();
        let p = crate::output::palette::active_palette();
        assert_eq!(coverage_status_color("Finding", &prefs), Some(p.failed.into_table()));
        assert_eq!(coverage_status_color("Clean", &prefs), Some(p.complete.into_table()));
        assert_eq!(coverage_status_color("Examined", &prefs), Some(p.dim.into_table()));
        assert_eq!(coverage_status_color("NotApplicable", &prefs), Some(p.skipped.into_table()));
    }

    #[test]
    fn coverage_status_color_covers_the_audit_verdicts() {
        // `coverage audit` renders a literal "ok"/"GAP" Status column.
        let prefs = prefs_color_always();
        let p = crate::output::palette::active_palette();
        assert_eq!(coverage_status_color("GAP", &prefs), Some(p.failed.into_table()));
        assert_eq!(coverage_status_color("ok", &prefs), Some(p.complete.into_table()));
    }

    #[test]
    fn coverage_status_color_does_not_hijack_lifecycle_states() {
        // These belong to status_color. Overlap would make which
        // function a call site used silently significant.
        let prefs = prefs_color_always();
        assert_eq!(coverage_status_color("running", &prefs), None);
        assert_eq!(coverage_status_color("failed", &prefs), None);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: FAIL — neither function exists.

- [ ] **Step 3: Implement**

```rust
/// Colour for a `rupu_coverage` severity level.
///
/// Deliberately separate from [`status_color`], whose vocabulary is
/// lifecycle states and a few classification values and knows nothing
/// of `Severity`. Merging unrelated vocabularies is how a call site
/// ends up silently depending on which function it happened to call.
///
/// Values arrive as `Debug` reprs (`Critical`, `High`, …), so matching
/// is case-insensitive — a call site that lowercases must not lose the
/// colour.
pub fn severity_color(severity: &str, prefs: &UiPrefs) -> Option<TableColor> {
    if !prefs.use_color() {
        return None;
    }
    let palette = palette::active_palette();
    Some(match severity.to_ascii_lowercase().as_str() {
        "critical" => palette.sev_critical.into_table(),
        "high" => palette.sev_high.into_table(),
        "medium" => palette.sev_medium.into_table(),
        "low" => palette.sev_low.into_table(),
        "info" => palette.sev_info.into_table(),
        _ => return None,
    })
}

/// Colour for a coverage assertion status or audit verdict.
///
/// `Clean` / `Finding` / `Examined` / `NotApplicable` come from
/// `AssertionStatus`; `ok` / `GAP` are the literals `coverage audit`
/// renders in its Status column.
///
/// Returns `None` for lifecycle states — those belong to
/// [`status_color`], and the two vocabularies must not overlap.
pub fn coverage_status_color(status: &str, prefs: &UiPrefs) -> Option<TableColor> {
    if !prefs.use_color() {
        return None;
    }
    let palette = palette::active_palette();
    Some(match status.to_ascii_lowercase().as_str() {
        "finding" | "gap" => palette.failed.into_table(),
        "clean" | "ok" => palette.complete.into_table(),
        "examined" => palette.dim.into_table(),
        "notapplicable" | "n/a" => palette.skipped.into_table(),
        _ => return None,
    })
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p rupu-cli --lib output::tables`
Expected: PASS, including all pre-existing tests.

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/tables.rs
git diff --stat
cargo clippy -p rupu-cli --lib 2>&1 | tail -5
git add crates/rupu-cli/src/output/tables.rs
git commit -m "feat(cli): severity and coverage-status colour vocabulary

coverage.rs contains zero .fg() calls today — severity renders as bare
Critical/High/Low text in a tool whose job is surfacing findings.

Kept separate from status_color, whose vocabulary is lifecycle states
and knows nothing of Severity or AssertionStatus. Case-insensitive, so
a call site that lowercases the Debug repr doesn't lose the colour."
```

---

### Task 2: Right-aligned numeric columns

**Files:** Modify `crates/rupu-cli/src/output/tables.rs`

**Interfaces:** Produces `align_numeric(&mut Table, &[usize])`. Tasks 4-5 consume it.

Nothing in the CLI right-aligns anything today. `coverage audit` has seven adjacent count columns; `usage`'s breakdown has five. Comparing magnitudes down a column needs the digits to line up.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn align_numeric_right_aligns_the_named_columns() {
        let mut t = new_table();
        t.set_header(vec!["NAME", "COUNT"]);
        t.add_row(vec![Cell::new("a"), Cell::new("7")]);
        t.add_row(vec![Cell::new("b"), Cell::new("1234")]);
        align_numeric(&mut t, &[1]);
        let out = t.to_string();
        let row_a = out.lines().find(|l| l.contains(" a ")).expect("row a");
        assert!(row_a.trim_end().ends_with('7'), "not right-aligned: {row_a}");
    }

    #[test]
    fn align_numeric_ignores_out_of_range_indices() {
        // Call sites write these index sets by hand; a stale index
        // after a column change must not panic mid-render.
        let mut t = new_table();
        t.set_header(vec!["ONLY"]);
        t.add_row(vec![Cell::new("x")]);
        align_numeric(&mut t, &[0, 5, 99]);
        assert!(t.to_string().contains('x'));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rupu-cli --lib output::tables::tests::align_numeric`
Expected: FAIL — `align_numeric` not found.

- [ ] **Step 3: Implement**

```rust
/// Right-align the given column indices.
///
/// Counts and costs are read by comparing magnitudes down a column,
/// which only works when the digits line up. `coverage audit` has
/// seven adjacent count columns and `usage`'s breakdown has five.
///
/// Out-of-range indices are ignored rather than panicking: call sites
/// write these sets by hand, and a stale index must not take down a
/// render.
pub fn align_numeric(table: &mut Table, columns: &[usize]) {
    for &idx in columns {
        if let Some(col) = table.column_mut(idx) {
            col.set_cell_alignment(comfy_table::CellAlignment::Right);
        }
    }
}
```

Add `CellAlignment` to the `comfy_table` import if absent.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p rupu-cli --lib output::tables`

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/output/tables.rs
git diff --stat
git add crates/rupu-cli/src/output/tables.rs
git commit -m "feat(cli): right-align numeric table columns

Nothing in the CLI right-aligns anything today. coverage audit has
seven adjacent count columns and usage's breakdown five.

Out-of-range indices are ignored rather than panicking — call sites
write these index sets by hand."
```

---

### Task 3: Wire coverage run-id resolution BEFORE anything is compacted

**Files:** Modify `crates/rupu-cli/src/cmd/coverage.rs`

**Interfaces:** Consumes `crate::output::ids::{resolve, Resolution}`. Produces `resolve_coverage_run_id(&[String], &str) -> anyhow::Result<String>`.

**This task must land before Task 4.** It ships no visible change; it removes a trap.

**The hazard, verified.** `coverage runs` displays a full, uncompacted run id (`coverage.rs:936`, `Cell::new(&r.run_id)`). Two commands consume a pasted run id, both by exact string equality:

1. `coverage rerun <target_id> <run_id>` → `rupu_coverage::find_manifest(&paths, run_id)` at `coverage.rs:997`, which does `.find(|m| m.run_id == run_id)` (`rupu-coverage/src/ledger/manifest.rs:58-62`).
2. `coverage diff <target_id> [base] [compare]` → `RunSelector::from_str` → `resolve_selector`, which does `ordered.iter().any(|r| r == id)` (`rupu-coverage/src/diff/generate.rs:93`).

`coverage.rs` never imports `output::ids`. The instant Task 4 compacts that column, a pasted id stops resolving — shipping exactly the "displays an identifier it will not accept back" bug this project exists to prevent.

**Approach:** resolve in `rupu-cli`, not in `rupu-coverage`. Gather candidate run ids the way the listing does, run the user's argument through `output::ids::resolve`, and pass the resolved FULL id down. That keeps the library's exact-match contract intact and matches `cmd/workflow.rs:2503` and `cmd/session.rs:7373`. **Read both precedents and match their error phrasing** so the three feel like one feature.

- [ ] **Step 1: Survey**

```bash
grep -n "find_manifest\|RunSelector\|run_rerun_in\|run_diff_in" crates/rupu-cli/src/cmd/coverage.rs
```
Record where run ids enter from the user and how the candidate list is obtained — the `coverage runs` path already builds one; reuse that source rather than inventing a second.

- [ ] **Step 2: Write the failing tests**

Test the pure layer, not the store — following Plan 1's precedent of splitting `resolve_run_id` (pure) from `resolve_run_fragment` (store glue), because building library fixtures from the CLI crate is impractical.

```rust
    fn coverage_run_candidates() -> Vec<String> {
        vec![
            "run_01KRM1CVRC2A9XZ0CY33RN5R0S".to_string(),
            "run_01KRJDKSBE7X4J49094149WFJS".to_string(),
        ]
    }

    #[test]
    fn coverage_resolve_accepts_a_full_id() {
        let c = coverage_run_candidates();
        assert_eq!(resolve_coverage_run_id(&c, &c[0]).expect("resolves"), c[0]);
    }

    #[test]
    fn coverage_resolve_accepts_the_compact_form_the_table_prints() {
        let c = coverage_run_candidates();
        let shown = crate::output::ids::compact_id(&c[0]);
        assert_eq!(resolve_coverage_run_id(&c, &shown).expect("resolves"), c[0]);
    }

    #[test]
    fn coverage_resolve_accepts_a_bare_suffix() {
        let c = coverage_run_candidates();
        assert_eq!(resolve_coverage_run_id(&c, "5R0S").expect("resolves"), c[0]);
    }

    #[test]
    fn coverage_resolve_errors_on_ambiguity_listing_candidates() {
        let c = coverage_run_candidates();
        let err = resolve_coverage_run_id(&c, "run_01KR").expect_err("ambiguous");
        let msg = err.to_string();
        assert!(msg.contains("ambiguous"), "got: {msg}");
        assert!(msg.contains(&c[0]) && msg.contains(&c[1]), "got: {msg}");
    }

    #[test]
    fn coverage_resolve_reports_unknown() {
        let err = resolve_coverage_run_id(&coverage_run_candidates(), "zzzzzz")
            .expect_err("unknown");
        assert!(err.to_string().contains("unknown"));
    }
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p rupu-cli --lib cmd::coverage::tests::coverage_resolve`

- [ ] **Step 4: Implement**

```rust
/// Resolve a coverage run-id fragment against a candidate list.
///
/// Pure, so it is testable without a coverage store. `coverage rerun`
/// and `coverage diff` both match run ids by exact string equality
/// (`find_manifest`, `resolve_selector`), so this must run BEFORE the
/// id reaches them — and before `coverage runs` compacts what it
/// displays, or a pasted id would stop working.
fn resolve_coverage_run_id(candidates: &[String], fragment: &str) -> anyhow::Result<String> {
    use crate::output::ids::{resolve, Resolution};
    match resolve(candidates, fragment) {
        Resolution::Unique(id) => Ok(id),
        Resolution::NotFound => anyhow::bail!("unknown coverage run: {fragment}"),
        Resolution::Ambiguous(matches) => {
            let mut msg = format!(
                "ambiguous coverage run id — {} runs match `{fragment}`",
                matches.len()
            );
            for id in &matches {
                msg.push_str(&format!("\n  {id}"));
            }
            anyhow::bail!(msg)
        }
    }
}
```

Route both entry points through it, resolving once where the argument enters. **For `coverage diff`, resolve only when the selector is a literal run id** — `latest` and `previous` are keywords and must pass through untouched. Add a test proving both keywords still work.

- [ ] **Step 5: Run tests, then verify the real binary**

```bash
cargo test -p rupu-cli --lib cmd::coverage
cargo run -p rupu-cli -- coverage runs <target>          # take a run id
cargo run -p rupu-cli -- coverage diff <target> <last-6-chars> latest
```
If no coverage targets exist locally, say so and demonstrate via the tests instead.

- [ ] **Step 6: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/coverage.rs
git diff --stat
git add crates/rupu-cli/src/cmd/coverage.rs
git commit -m "feat(cli): resolve coverage run ids from fragments

coverage rerun and coverage diff match run ids by exact string equality
(find_manifest, resolve_selector). Wiring resolution first is what makes
it safe for the next commit to compact the id coverage runs displays —
otherwise a pasted id would silently stop working.

latest/previous are keywords and pass through unresolved."
```

---

### Task 4: `coverage runs` and `coverage audit` readability

**Files:** Modify `crates/rupu-cli/src/cmd/coverage.rs`

**Interfaces:** Consumes `severity_color`, `coverage_status_color`, `align_numeric`, `resolve_coverage_run_id`, `output::ids::compact_id`, `output::fmt::relative_time`.

**`coverage runs`** (`coverage.rs:936`), header `["Run", "Started", "Surface", "Model", "Cells", "Findings", "Files"]`:
- `Run` → `compact_id`. Safe only because Task 3 landed.
- `Started` → `relative_time`. It is a real `DateTime<Utc>` (`RunListEntry.started_at`), currently always `to_rfc3339()`. **No parsing needed** — unlike `workflow runs`, whose field was a pre-stringified `String`.
- `Cells`, `Findings`, `Files` → right-aligned (indices 4, 5, 6).

**`coverage audit`** (`coverage.rs:567`), header `["Concern", "Severity", "In-scope", "Asserted", "Gap", "Clean", "Finding", "Examined", "N/A", "Status"]`:
- `Severity` → `severity_color`.
- `Status` (the literal `ok` / `GAP` at `coverage.rs:583`) → `coverage_status_color`.
- The seven counts (indices 2..=8) → right-aligned.
- **Every count column keeps every value, including zeros.** This is the one table where "a zero is data, not absence" genuinely holds. Do **not** introduce `CellValue`, `EntityTable`, or any suppression here — this table renders with plain `comfy_table::Cell`s and stays that way.

**Also apply `severity_color`** to `coverage show`'s Findings table (`coverage.rs:500`) and `coverage templates show` / `coverage catalog` (`coverage.rs:302`/`393`, both `ConcernRow.severity`) — three one-line changes reusing Task 1 for free.

**JSON/CSV must not move.** `coverage runs` has two frozen shapes: `RunListEntry` (table + JSON) and `RunRow` (CSV, built at `coverage.rs:960`). You change only how cells render, never what is serialized. Capture both before, diff after.

- [ ] **Step 1: Write the failing tests**

Extract each table's construction into a testable function returning `String` (follow `render_session_list_table`'s shape from Plan 2), then assert: the run id renders compacted and the full id is absent; `Started` renders a relative age; count columns present including zeros; severity and status cells carry colour when colour is forced on.

Build fixtures from the real types — read `RunListEntry` at `rupu-coverage/src/diff/types.rs:64` and `ConcernCoverage` at `rupu-coverage/src/audit/types.rs:7` for exact field names.

**Include a test that a `ConcernCoverage` with all-zero counts still renders all seven columns with `0` in each** — the explicit guard for the property that motivated the profile split.

- [ ] **Step 2: Run to verify they fail**

- [ ] **Step 3: Implement**

- [ ] **Step 4: Run tests and verify the real binary**

```bash
cargo test -p rupu-cli --lib cmd::coverage
cargo run -p rupu-cli -- coverage runs <target>
cargo run -p rupu-cli -- coverage audit <target>
cargo run -p rupu-cli -- coverage runs <target> --format json > /tmp/after.json
cargo run -p rupu-cli -- coverage runs <target> --format csv  > /tmp/after.csv
```
Then paste a compacted run id from the listing back into `coverage rerun` / `coverage diff`. **If a displayed id does not resolve, that is Critical — stop and report.**

- [ ] **Step 5: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/coverage.rs
git diff --stat
git add crates/rupu-cli/src/cmd/coverage.rs
git commit -m "feat(cli): readable coverage runs and audit tables

Compact run ids (resolvable since the previous commit), relative start
times, right-aligned counts, and severity/verdict colour. coverage.rs
had zero .fg() calls before this.

coverage audit keeps every count column including zeros — a measured
zero is data. No suppression is introduced here."
```

---

### Task 5: `usage` breakdown and runs readability

**Files:** Modify `crates/rupu-cli/src/cmd/usage.rs`

**Interfaces:** Consumes `align_numeric`, `output::ids::compact_id`, `output::fmt::relative_time`.

**`print_breakdown_table`** (`usage.rs:1105`) — the densest numeric table in the CLI, with INPUT / OUTPUT / CACHED / RUNS / COST and a **TOTAL footer row** (`usage.rs:1191`). Right-align the numeric columns; the footer must stay aligned with them.

**`print_run_table`** (`usage.rs:1234`) — compact `RUN_ID`, right-align token/cost columns, render `STARTED_AT` relative (currently `format_timestamp` → `"%Y-%m-%d %H:%MZ"` at `usage.rs:1401`). Its STATUS column already uses `status_cell`; leave that.

**Before compacting `RUN_ID`, apply the Task 3 lesson.** Determine whether any `usage` subcommand accepts a run id as an argument. If one does, wire resolution first exactly as Task 3 did. If none does, state that explicitly in your report — the id is display-only and compaction is safe. **Do not assume; check.**

**Leave alone:** `print_backfill_table` (`usage.rs:1279`) and `print_summary_table` (`usage.rs:1317`). Both are fixed KEY/VALUE cards with a stable row set.

**A real decision to make and report, not guess.** `usage.rs` has its own `format_count` (comma-grouped: `1,234,567`) while the rest of the CLI uses `fmt::format_token_compact` (`1.2M`). **Recommendation: keep `format_count`.** `usage` is the billing view — someone reconciling a bill needs `1,234,567`, not `1.2M`. Consistency is the weaker argument here. If you disagree, say why rather than switching silently.

- [ ] **Step 1: Survey**

```bash
grep -n "run_id" crates/rupu-cli/src/cmd/usage.rs | head -20
```
Determine whether any `usage` subcommand takes a user-supplied run id. Report before proceeding.

- [ ] **Step 2: Write the failing tests**

Extract both tables into testable functions returning `String`. Assert: numeric columns right-aligned; the TOTAL footer present and aligned; `RUN_ID` compacted with the full id absent; `STARTED_AT` relative. Build fixtures from the real row types.

- [ ] **Step 3: Run to verify they fail**

- [ ] **Step 4: Implement**

- [ ] **Step 5: Verify the real binary**

```bash
cargo run -p rupu-cli -- usage
cargo run -p rupu-cli -- usage runs
cargo run -p rupu-cli -- usage --format json > /tmp/after.json
```

- [ ] **Step 6: Format, lint, commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/usage.rs
git diff --stat
git add crates/rupu-cli/src/cmd/usage.rs
git commit -m "feat(cli): readable usage breakdown and run tables

Right-aligned token/cost columns so magnitudes compare down the column,
with the TOTAL footer still aligned. Compact run ids and relative start
times on usage runs.

Kept usage's comma-grouped format_count rather than converging on
format_token_compact: this is the billing view, and someone reconciling
a bill needs 1,234,567 rather than 1.2M."
```

---

## Verification

- [ ] `cargo test -p rupu-cli` passes in full; `cargo build -p rupu-cli --lib` emits 0 warnings.
- [ ] `cargo clippy -p rupu-cli --lib` introduces nothing beyond the pre-existing `completers.rs` error.
- [ ] **Every compacted identifier resolves.** Take a run id from `coverage runs` and from `usage runs` and paste each back into a command that accepts it. This is the governing rule; a regression is Critical.
- [ ] `--format json` and `--format csv` byte-identical for `coverage runs`, `coverage audit`, `usage`, `usage runs`. Remember `coverage runs` has two frozen shapes.
- [ ] `coverage audit` still shows every count column, including all-zero ones.
- [ ] `cargo test -p rupu-cli --test auth_status_table` still passes — it pins `PROVIDER` / `API-KEY` / `SSO` by substring.
- [ ] Severity colour visible in `coverage audit`, `coverage show`, `coverage catalog`; gone under `NO_COLOR=1` and when piped.
- [ ] No identifier wraps at a narrow width (try `COLUMNS=60`).

## Deliberately not done

- No `ReportTable` type. One table of 26 has the property that would justify it.
- The 5 fixed KEY/VALUE detail cards are untouched.
- `auth status` is untouched — already colours, nothing to compact or align, headers pinned by a test.
- Header-casing unification is left alone.

## Follow-on candidates

- `--absolute` is not wired into `Cmd::Coverage` / `Cmd::Usage` / `Cmd::Auth` (`lib.rs:308-321`), so the relative timestamps this plan adds cannot be toggled back to ISO. Worth doing once across all three rather than piecemeal.
- `coverage diff`'s cross-model and serendipitous tables render raw Rust `Debug` output (`format!("{:?}", Vec<(String, AssertionStatus)>)`). Genuinely unreadable; deserves its own pass.
- `coverage diff`'s findings tables use a `-` sentinel where the rest of the CLI uses an em dash.
