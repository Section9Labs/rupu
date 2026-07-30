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

    /// Build the underlying `comfy-table::Table`, shared by [`Self::render`]
    /// and (test-only) [`Self::render_at_width`].
    fn build(&self, now: DateTime<Utc>) -> Table {
        let mut t = tables::new_table();
        t.set_header(self.headers.clone());
        for row in &self.rows {
            t.add_row(row.iter().map(|c| self.cell(c, now)).collect::<Vec<_>>());
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
        for (idx, _) in self.headers.iter().enumerate() {
            if self.rows.iter().any(|r| matches!(r[idx], CellValue::Id(_))) {
                if let Some(col) = t.column_mut(idx) {
                    col.set_constraint(ColumnConstraint::ContentWidth);
                }
            }
        }
        t
    }

    /// Build the table. `now` is a parameter so tests are deterministic;
    /// callers pass `Utc::now()`.
    pub fn render(&self, now: DateTime<Utc>) -> String {
        self.build(now).to_string()
    }

    /// Test-only: render after forcing a narrow table width, to exercise
    /// the identifier no-wrap constraint under real squeeze pressure.
    /// Production callers never need to force a width — `render` always
    /// picks up the real terminal width via comfy-table's tty detection.
    #[cfg(test)]
    fn render_at_width(&self, now: DateTime<Utc>, width: u16) -> String {
        let mut t = self.build(now);
        t.set_width(width);
        t.to_string()
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
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["TARGET"])
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
        let p = prefs();
        let t = EntityTable::new(&p, RenderOpts::default(), vec!["A", "B"]);
        assert!(t.try_row(vec![CellValue::Text("only-one".into())]).is_err());
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
}
