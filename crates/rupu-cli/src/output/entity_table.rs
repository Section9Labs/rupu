//! The entity-list rendering profile.
//!
//! Entity lists — sessions, runs, workflows — are scanned by an operator
//! looking for one row to act on. They get compacted identifiers,
//! relative timestamps, lifecycle glyphs, empty-column suppression,
//! and (via later tasks) a summary line.
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
use comfy_table::{Cell, ColumnConstraint, Table};

/// What a cell contains, independent of how it is displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellValue {
    /// Free text, rendered verbatim.
    Text(String),
    /// An identifier. Compacted for display and never wrapped, because a
    /// wrapped identifier cannot be copy-pasted back — which would break
    /// the governing rule that every displayed id resolves.
    Id(String),
    /// A human-assigned name that the CLI also accepts back as an
    /// identifier (workflow names, agent names, cron names, model ids).
    /// Rendered verbatim — unlike `Id`, it is never compacted — but gets
    /// the same no-wrap constraint `Id` does: a name wrapped across two
    /// lines is exactly as unresolvable as a wrapped id, and this cell
    /// carries no glyph or hyphen scheme of its own for a reader to
    /// recognize the break.
    Name(String),
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
            CellValue::Name(s) => s.is_empty(),
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
    summary_noun: Option<&'static str>,
}

impl<'a> EntityTable<'a> {
    pub fn new(prefs: &'a UiPrefs, opts: RenderOpts, headers: Vec<&'static str>) -> Self {
        Self {
            prefs,
            opts,
            headers,
            rows: Vec::new(),
            summary_noun: None,
        }
    }

    /// Append a row. Panics on arity mismatch. Every call site builds a
    /// row from a fixed literal, so a mismatch is a programming error,
    /// not a data error — the panic is the correct response.
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

    /// Render one cell to a comfy-table `Cell`.
    fn cell(&self, value: &CellValue, now: DateTime<Utc>) -> Cell {
        match value {
            CellValue::Text(s) => Cell::new(s),
            CellValue::Id(id) => Cell::new(ids::compact_id(id)),
            CellValue::Name(s) => Cell::new(s),
            CellValue::Missing => {
                // M3: restore the pre-branch dim styling an absent value
                // used to get (`Cell::new("\x1b[2m—\x1b[0m")`) — plain
                // `Cell::new("—")` lost it. Gated on `use_color()` like
                // every other coloured cell in this match, so `--no-color`
                // / non-tty output stays a bare em dash.
                let cell = Cell::new("—");
                if self.prefs.use_color() {
                    cell.fg(crate::output::palette::active_palette().dim.into_table())
                } else {
                    cell
                }
            }
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

    /// Build the underlying `comfy-table::Table` with optional forced width.
    ///
    /// Projects columns through `keep` (display → source mapping), applies
    /// identifier constraints at display indices, and optionally forces a
    /// width for testing squeeze behavior. Both [`Self::render`] and
    /// (test-only) [`Self::render_at_width`] call this.
    fn build_table(&self, now: DateTime<Utc>, keep: &[usize], forced_width: Option<u16>) -> Table {
        let mut t = tables::new_table();
        t.set_header(keep.iter().map(|&i| self.headers[i]).collect::<Vec<_>>());
        for row in &self.rows {
            t.add_row(
                keep.iter()
                    .map(|&i| self.cell(&row[i], now))
                    .collect::<Vec<_>>(),
            );
        }
        // Identifier columns must never wrap — a compacted id split
        // across two lines cannot be pasted back.
        //
        // `ColumnConstraint::LowerBoundary` only sets a *floor*: under
        // real squeeze pressure (a table narrower than the sum of its
        // columns' natural widths), comfy-table 7.2.2's dynamic
        // arrangement algorithm (`utils::arrangement::dynamic`) applies
        // the boundary via `absolute_width_with_padding`, which
        // subtracts the column's padding from the boundary value to get
        // the *content* width. A `LowerBoundary(Fixed(17))` column with
        // 1-char padding on each side resolves to a 15-character content
        // width once the boundary is enforced — two characters short of
        // the 17-character compact id — so the id itself would wrap.
        // `ColumnConstraint::ContentWidth` is resolved once, up front,
        // in `constraint::evaluate` (before the dynamic squeeze
        // algorithm even runs) and fixes the column to its true max
        // content width for good; it is never revisited by the
        // squeeze/redistribute passes. See
        // `output::entity_table::tests::narrow_table_never_wraps_a_compact_id`.
        //
        // Critical: apply constraint at display index (pos), read cell from
        // source index (src). Swapping these silently constrains the wrong
        // column once any column is suppressed.
        for (pos, &src) in keep.iter().enumerate() {
            if self
                .rows
                .iter()
                .any(|r| matches!(r[src], CellValue::Id(_) | CellValue::Name(_)))
            {
                if let Some(col) = t.column_mut(pos) {
                    col.set_constraint(ColumnConstraint::ContentWidth);
                }
            }
        }
        if let Some(width) = forced_width {
            t.set_width(width);
        }
        t
    }

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

    /// `8 sessions · 2 failed · 5 idle`, or just the count when the table
    /// has no status column to break down by.
    fn summary_line(&self, noun: &str) -> String {
        let n = self.rows.len();
        let mut out = if n == 1 {
            format!("{n} {noun}")
        } else {
            format!("{n} {noun}s")
        };
        let status_col = self
            .headers
            .iter()
            .position(|&h| h.eq_ignore_ascii_case("STATUS"));
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

    /// Build the table. `now` is a parameter so tests are deterministic;
    /// callers pass `Utc::now()`.
    pub fn render(&self, now: DateTime<Utc>) -> String {
        let mut out = String::new();
        if let Some(noun) = self.summary_noun {
            out.push_str(&self.summary_line(noun));
            out.push_str("\n\n");
        }
        let keep = self.retained_columns();
        out.push_str(&self.build_table(now, &keep, None).to_string());
        out
    }

    /// Test-only: render after forcing a narrow table width, to exercise
    /// the identifier no-wrap constraint under real squeeze pressure.
    /// Production callers never need to force a width — `render` always
    /// picks up the real terminal width via comfy-table's tty detection.
    ///
    /// `pub(crate)` (not private) so other `cmd::*` modules' own test
    /// modules can force squeeze pressure on tables they build with this
    /// type — e.g. `cmd::workflow`'s narrow-width NAME no-wrap test.
    #[cfg(test)]
    pub(crate) fn render_at_width(&self, now: DateTime<Utc>, width: u16) -> String {
        let keep = self.retained_columns();
        self.build_table(now, &keep, Some(width)).to_string()
    }
}

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
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["RUN"]).row(vec![CellValue::Id(
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
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["UPDATED"])
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
        let p = prefs();
        let t = EntityTable::new(&p, opts, vec!["UPDATED"]).row(vec![CellValue::Timestamp(then)]);
        let out = t.render(now());
        assert!(out.contains("2026-07-16"), "got: {out}");
        assert!(!out.contains("2w ago"), "got: {out}");
    }

    #[test]
    fn statuses_render_with_their_lifecycle_glyph() {
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["STATUS"])
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
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["SCOPE"])
            .row(vec![CellValue::Status("project".to_string())]);
        let out = t.render(now());
        assert!(out.contains("project"), "got: {out}");
        for glyph in ['✓', '✗', '○', '●', '⏸', '↺', '⊘'] {
            assert!(!out.contains(glyph), "unexpected glyph {glyph} in: {out}");
        }
    }

    #[test]
    fn missing_renders_as_em_dash() {
        let p = prefs();
        // With an entirely-empty column, suppression drops it. To test that
        // Missing renders as em dash, use a column that won't be suppressed.
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["TARGET", "DATA"])
            .row(vec![CellValue::Missing, CellValue::Text("x".into())])
            .row(vec![
                CellValue::Text("y".into()),
                CellValue::Text("z".into()),
            ]);
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
        assert!(CellValue::Name(String::new()).is_empty());
        assert!(!CellValue::Name("nightly".to_string()).is_empty());
    }

    #[test]
    #[should_panic(expected = "row arity")]
    fn row_length_mismatch_panics() {
        // A row with the wrong arity would silently misalign columns.
        // `row()` is the only append method; a mismatch is a
        // programming error at a call site with a fixed literal, so it
        // panics rather than returning a recoverable error.
        let p = prefs();
        let _ = EntityTable::new(&p, RenderOpts::default(), vec!["A", "B"])
            .row(vec![CellValue::Text("only-one".into())]);
    }

    #[test]
    fn a_successfully_added_row_appears_in_render_output() {
        // Regression guard: a prior version of `try_row` took `&self`
        // and silently discarded the row on the success path — the
        // only test on it exercised the error path, so 9/9 tests
        // passed with the row vanishing in production. This asserts
        // the row we add is actually the row we get back.
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["A", "B"])
            .row(vec![
                CellValue::Text("alpha".to_string()),
                CellValue::Text("beta".to_string()),
            ])
            .row(vec![
                CellValue::Text("gamma".to_string()),
                CellValue::Text("delta".to_string()),
            ]);
        let out = t.render(now());
        assert!(out.contains("alpha"), "got: {out}");
        assert!(out.contains("beta"), "got: {out}");
        assert!(out.contains("gamma"), "got: {out}");
        assert!(out.contains("delta"), "got: {out}");
    }

    #[test]
    fn narrow_table_never_wraps_a_compact_id() {
        // Force real squeeze pressure: a wide sibling column competing
        // for space in a narrow table. If the id column's constraint
        // were a mere *lower bound* (comfy-table's `LowerBoundary`),
        // padding accounting can still push its assigned content width
        // below the compact form's 17 characters under pressure, and
        // the id would be split across two lines — unpasteable, which
        // breaks the rule that every displayed identifier resolves.
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["RUN", "NOTES"]).row(vec![
            CellValue::Id("run_01KRJDKSBE7X4J49094149WFJS".to_string()),
            CellValue::Text("x".repeat(120)),
        ]);
        let out = t.render_at_width(now(), 30);
        assert!(
            out.contains("run_01KRJDKS…WFJS"),
            "compact id must survive a narrow, squeezed table intact: {out}"
        );
    }

    #[test]
    fn narrow_table_never_wraps_a_name() {
        // Same governing rule as the compact-id case, for `CellValue::Name`:
        // a workflow (or agent/cron/model) name is an identifier the CLI
        // accepts back verbatim, so it must never be allowed to wrap
        // across two lines under squeeze pressure either.
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["NAME", "NOTES"]).row(vec![
            CellValue::Name("nightly-maintainability-security".to_string()),
            CellValue::Text("x".repeat(120)),
        ]);
        let out = t.render_at_width(now(), 30);
        assert!(
            out.contains("nightly-maintainability-security"),
            "name must survive a narrow, squeezed table intact on one line: {out}"
        );
    }

    #[test]
    fn suppressed_column_with_narrow_squeeze_never_wraps_id() {
        // Critical regression guard: production render() and test-only
        // render_at_width() now share the same builder path. This combines
        // both conditions: a column that gets suppressed (entirely Missing),
        // plus an id column under narrow squeeze. The display/source index
        // discipline (applying constraint at pos, reading cell from src) is
        // only exposed when suppression is active — if those indices are
        // swapped, the id column gets no constraint and wraps, but the test
        // sees no wrapping because constraint was (wrongly) applied elsewhere.
        let p = prefs();
        let t =
            EntityTable::new(&p, RenderOpts::default(), vec!["TARGET", "RUN", "NOTES"]).row(vec![
                CellValue::Missing,
                CellValue::Id("run_01KRJDKSBE7X4J49094149WFJS".to_string()),
                CellValue::Text("x".repeat(120)),
            ]);
        let out = t.render_at_width(now(), 30);
        assert!(
            out.contains("run_01KRJDKS…WFJS"),
            "compact id must survive suppression + narrow squeeze intact: {out}"
        );
    }

    #[test]
    fn all_empty_column_is_dropped() {
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["A", "TARGET", "B"])
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
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["A", "TARGET"])
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
        let p = prefs();
        let opts = RenderOpts {
            all_columns: true,
            ..RenderOpts::default()
        };
        let t = EntityTable::new(&p, opts, vec!["A", "TARGET"])
            .row(vec![CellValue::Text("1".into()), CellValue::Missing]);
        assert!(t.render(now()).contains("TARGET"));
    }

    #[test]
    fn a_table_with_no_rows_keeps_all_headers() {
        // With zero rows every column is vacuously empty. Dropping them
        // all would render a headerless void instead of an empty list.
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["A", "B"]);
        let out = t.render(now());
        assert!(out.contains('A') && out.contains('B'), "got: {out}");
    }

    #[test]
    fn summary_counts_by_status() {
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["ID", "STATUS"])
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
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["STATUS"])
            .with_summary("run")
            .row(vec![CellValue::Status("failed".into())]);
        let out = t.render(now());
        assert!(out.contains("1 run"), "got: {out}");
        assert!(!out.contains("1 runs"), "got: {out}");
    }

    #[test]
    fn summary_absent_when_not_requested() {
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["STATUS"])
            .row(vec![CellValue::Status("failed".into())]);
        assert!(!t.render(now()).contains("1 "), "unexpected summary");
    }

    #[test]
    fn summary_on_empty_list_says_zero() {
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["STATUS"]).with_summary("session");
        assert!(t.render(now()).contains("0 sessions"));
    }

    #[test]
    fn summary_without_a_status_column_shows_only_the_count() {
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["NAME"])
            .with_summary("workflow")
            .row(vec![CellValue::Text("a".into())])
            .row(vec![CellValue::Text("b".into())]);
        let out = t.render(now());
        assert!(out.contains("2 workflows"), "got: {out}");
    }

    #[test]
    fn summary_with_two_status_columns_uses_the_status_header() {
        // Regression guard: session list has both SCOPE and STATUS as Status cells.
        // Must break down by STATUS (lifecycle), not SCOPE (classification).
        let p = prefs();
        let t = EntityTable::new(
            &p,
            RenderOpts::default(),
            vec!["SESSION", "SCOPE", "STATUS"],
        )
        .with_summary("session")
        .row(vec![
            CellValue::Id("s1".into()),
            CellValue::Status("project".into()),
            CellValue::Status("failed".into()),
        ])
        .row(vec![
            CellValue::Id("s2".into()),
            CellValue::Status("project".into()),
            CellValue::Status("idle".into()),
        ])
        .row(vec![
            CellValue::Id("s3".into()),
            CellValue::Status("project".into()),
            CellValue::Status("idle".into()),
        ]);
        let out = t.render(now());
        assert!(out.contains("3 sessions"), "got: {out}");
        assert!(out.contains("2 idle"), "got: {out}");
        assert!(out.contains("1 failed"), "got: {out}");
        assert!(
            !out.contains("3 project"),
            "should not summarize by SCOPE: {out}"
        );
    }
}
