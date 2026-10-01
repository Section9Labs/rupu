//! The dashboard frame (spec 2026-10-01): the whole `workflow run` live view
//! composed from the pane renderers, as a pure function from the run's
//! models to styled [`Line`]s — no terminal I/O.
//!
//! ```text
//! <header: layout::dashboard — title · progress · meters>
//! ─ structure ───────────┬─ stream · <selection> ───────────
//!  the workflow DAG      │  the selection's transcript
//!  (structure::…)        ├─ live · N active ────────────────
//!                        │  the run-wide firehose
//! <footer: layout::footer_line — gate-aware key legend>
//! ```
//!
//! * **Wide** (`w >= 76` and room for every region): a structure column of
//!   `clamp(w * 2 / 5, 28, 48)` columns, a `│` divider, and a right column
//!   split into the **stream** (top ~60% of the body) and the **firehose**
//!   (the rest, never fewer than 3 content rows under its rule). Every
//!   region has a title rule; the focused pane's title is `Strong`, the
//!   others `Dim`.
//! * **Narrow / short**: one stacked pane — the *focused* one
//!   ([`NavState::pane`], structure by default) fills the body, so `tab`
//!   (which only moves focus) cycles what is on screen.
//! * An empty stream or firehose is ONE dim `waiting for activity…` row,
//!   never a fabricated line and never a zero-height region.
//!
//! The stream and firehose are windowed **tail-anchored**: the newest line is
//! the region's last row, and [`NavState::scroll_offset`] holds it that many
//! lines above the tail. [`dashboard_frame`] only *reads* the offset (it takes
//! `&NavState`); the driver clamps it each frame against the real buffer
//! using [`scroll_rows`], which reports how many rows each pane really draws.

use chrono::{DateTime, Utc};
use rupu_orchestrator::Workflow;

use crate::output::live_view::layout::{dashboard, footer_line, printable};
use crate::output::live_view::nav::{NavState, Pane};
use crate::output::live_view::row::{truncate_to, Line};
use crate::output::live_view::structure::structure_pane;
use crate::output::run_model::{RunView, StepState, UnitStatus};

/// Below this width the three-column split is unreadable: fall back to one
/// stacked pane.
const WIDE_MIN_W: usize = 76;
/// The structure column's bounds (`w * 2 / 5` clamped into them).
const STRUCT_MIN_W: usize = 28;
const STRUCT_MAX_W: usize = 48;
/// Content rows the stream keeps (below its title row) in the wide layout.
const STREAM_MIN_ROWS: usize = 1;
/// Content rows the firehose keeps (below its rule) in the wide layout — it
/// is always visible, whatever the stream wants.
const FIREHOSE_MIN_ROWS: usize = 3;
/// The shortest body (title row + content rows of every region) the wide
/// layout fits in; a shorter terminal falls back to one stacked pane.
const WIDE_MIN_BODY: usize = (1 + STREAM_MIN_ROWS) + (1 + FIREHOSE_MIN_ROWS);
/// Blank columns between the divider and the content beside it (the
/// structure rows end this far short of it; the feeds start this far past).
const GUTTER: usize = 1;
/// The fallback hint on the single pane's title rule.
const TAB_HINT: &str = "tab cycles panes";
/// What an empty stream / firehose says.
const WAITING: &str = "waiting for activity…";

/// How a frame's rows are split, decided from the sizes alone.
#[derive(Debug, Clone, Copy)]
enum Mode {
    /// Structure │ stream over firehose.
    Wide {
        struct_w: usize,
        /// Rows of the stream region, its title row included.
        stream_h: usize,
    },
    /// One stacked pane.
    Single(Pane),
}

#[derive(Debug, Clone, Copy)]
struct Plan {
    header_h: usize,
    body_h: usize,
    footer_h: usize,
    mode: Mode,
}

impl Plan {
    /// Budget `h` rows: the footer takes one (when there is room for more
    /// than a header), the header as many of its rows as fit while leaving the
    /// body at least one, and the body the rest. Never over `h`.
    fn new(header_len: usize, nav: &NavState, w: usize, h: usize) -> Plan {
        let footer_h = usize::from(h >= 3);
        let header_h = header_len.min(h.saturating_sub(footer_h + 1)).max(1);
        let body_h = h.saturating_sub(header_h + footer_h);
        let mode = if w >= WIDE_MIN_W && body_h >= WIDE_MIN_BODY {
            Mode::Wide {
                struct_w: (w * 2 / 5).clamp(STRUCT_MIN_W, STRUCT_MAX_W),
                stream_h: (body_h * 3 / 5)
                    .clamp(1 + STREAM_MIN_ROWS, body_h - (1 + FIREHOSE_MIN_ROWS)),
            }
        } else {
            Mode::Single(nav.pane())
        };
        Plan {
            header_h,
            body_h,
            footer_h,
            mode,
        }
    }

    /// The single pane's title row exists only when the body has a row to
    /// spare for it.
    fn single_title_h(&self) -> usize {
        usize::from(self.body_h >= 2)
    }

    /// Content rows `pane` draws (0 when it is not on screen).
    fn rows(&self, pane: Pane) -> usize {
        match (self.mode, pane) {
            (Mode::Wide { .. }, Pane::Structure) => self.body_h - 1,
            (Mode::Wide { stream_h, .. }, Pane::Stream) => stream_h - 1,
            (Mode::Wide { stream_h, .. }, Pane::Firehose) => self.body_h - stream_h - 1,
            (Mode::Single(shown), pane) if shown == pane => self.body_h - self.single_title_h(),
            (Mode::Single(_), _) => 0,
        }
    }
}

/// How many content rows the stream and firehose panes draw at `w`×`h` for
/// the pane focus in `nav` (`0` for a pane the narrow fallback hides). The
/// driver clamps each pane's scroll to `buffer_len - rows` with this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRows {
    pub stream: usize,
    pub firehose: usize,
}

/// The visible content rows of the scrolling panes — see [`ScrollRows`]. It
/// runs the same budgeting as [`dashboard_frame`], so the two cannot drift.
pub fn scroll_rows(
    view: &RunView,
    nav: &NavState,
    now: DateTime<Utc>,
    w: usize,
    h: usize,
) -> ScrollRows {
    if w == 0 || h == 0 {
        return ScrollRows {
            stream: 0,
            firehose: 0,
        };
    }
    let plan = Plan::new(dashboard(view, now).len(), nav, w, h);
    ScrollRows {
        stream: plan.rows(Pane::Stream),
        firehose: plan.rows(Pane::Firehose),
    }
}

/// The whole live view at `w`×`h`: header, the panes, footer. At most `h`
/// rows of at most `w` columns each (nothing for a zero-sized frame).
///
/// `stream` is the selection's own transcript and `firehose` the merged
/// run-wide feed, both oldest → newest (the mux's `pinned_lines` /
/// `firehose_lines`). They are windowed by `nav`'s per-pane scroll offset
/// but never clamped here.
#[allow(clippy::too_many_arguments)]
pub fn dashboard_frame(
    view: &RunView,
    wf: &Workflow,
    nav: &NavState,
    stream: &[Line],
    firehose: &[Line],
    now: DateTime<Utc>,
    w: usize,
    h: usize,
) -> Vec<Line> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let header = dashboard(view, now);
    let plan = Plan::new(header.len(), nav, w, h);
    let panes = Panes {
        view,
        wf,
        nav,
        stream,
        firehose,
    };

    let mut out: Vec<Line> = header
        .into_iter()
        .take(plan.header_h)
        .map(|l| truncate_to(l, w))
        .collect();
    match plan.mode {
        Mode::Wide { struct_w, .. } => out.extend(panes.wide_body(&plan, struct_w, w)),
        Mode::Single(pane) => out.extend(panes.single_body(&plan, pane, w)),
    }
    if plan.footer_h > 0 {
        out.push(footer_line(view, nav, w));
    }
    out
}

/// The inputs every pane draws from.
struct Panes<'a> {
    view: &'a RunView,
    wf: &'a Workflow,
    nav: &'a NavState,
    stream: &'a [Line],
    firehose: &'a [Line],
}

impl Panes<'_> {
    /// Structure │ stream over firehose: `plan.body_h` rows.
    fn wide_body(&self, plan: &Plan, struct_w: usize, w: usize) -> Vec<Line> {
        let right_w = w - struct_w - 1;
        let focus = self.nav.pane();
        let rows = |pane| plan.rows(pane);

        // Left: the structure title, then its rows (a gutter short of the
        // divider), every row padded out to the divider column.
        let mut left = vec![title_rule(
            "structure",
            "",
            focus == Pane::Structure,
            struct_w,
        )];
        left.extend(structure_pane(
            self.view,
            self.wf,
            self.nav,
            struct_w - GUTTER,
            rows(Pane::Structure),
        ));
        left.resize(plan.body_h, Line::new());

        // Right: stream title + rows, firehose rule + rows — each row with the
        // divider glyph that joins it to the left column.
        let content_w = right_w - GUTTER;
        let mut right: Vec<(char, Line)> = Vec::with_capacity(plan.body_h);
        right.push((
            '┬',
            title_rule(&self.stream_title(), "", focus == Pane::Stream, right_w),
        ));
        right.extend(
            indented(self.stream_rows(rows(Pane::Stream), content_w))
                .into_iter()
                .map(|l| ('│', l)),
        );
        right.push((
            '├',
            title_rule(&self.firehose_title(), "", focus == Pane::Firehose, right_w),
        ));
        right.extend(
            indented(self.firehose_rows(rows(Pane::Firehose), content_w))
                .into_iter()
                .map(|l| ('│', l)),
        );
        debug_assert_eq!(right.len(), plan.body_h);

        left.into_iter()
            .zip(right)
            .map(|(l, (divider, r))| {
                let mut row = pad_to(l, struct_w).dim(divider.to_string());
                row.segments.extend(r.segments);
                truncate_to(row, w)
            })
            .collect()
    }

    /// One stacked pane: a title rule (when there is a row for it) and the
    /// focused pane's rows, full width.
    fn single_body(&self, plan: &Plan, pane: Pane, w: usize) -> Vec<Line> {
        let rows = plan.rows(pane);
        let mut out = Vec::with_capacity(plan.body_h);
        if plan.single_title_h() > 0 {
            let title = match pane {
                Pane::Structure => "structure".to_string(),
                Pane::Stream => self.stream_title(),
                Pane::Firehose => self.firehose_title(),
            };
            out.push(title_rule(&title, TAB_HINT, true, w));
        }
        out.extend(match pane {
            Pane::Structure => structure_pane(self.view, self.wf, self.nav, w, rows),
            Pane::Stream => self.stream_rows(rows, w),
            Pane::Firehose => self.firehose_rows(rows, w),
        });
        // Keep the `≤ body_h` guarantee local here rather than relying on
        // `structure_pane` to clip itself to `rows`.
        out.truncate(plan.body_h);
        out
    }

    fn stream_title(&self) -> String {
        format!("stream · {}", selection_label(self.view, self.nav))
    }

    fn firehose_title(&self) -> String {
        format!("live · {} active", active_count(self.view))
    }

    /// Exactly `rows` rows (blank-padded) of the stream, windowed.
    fn stream_rows(&self, rows: usize, w: usize) -> Vec<Line> {
        feed_rows(self.stream, self.nav.scroll_offset(Pane::Stream), rows, w)
    }

    /// Exactly `rows` rows (blank-padded) of the firehose, windowed.
    fn firehose_rows(&self, rows: usize, w: usize) -> Vec<Line> {
        feed_rows(
            self.firehose,
            self.nav.scroll_offset(Pane::Firehose),
            rows,
            w,
        )
    }
}

/// `─ <title> · <hint> ────…` filling `w` columns. The focused pane's title
/// is `Strong`, an unfocused one `Dim`; the rule itself is always `Dim`.
fn title_rule(title: &str, hint: &str, focused: bool, w: usize) -> Line {
    let mut line = Line::new().dim("─ ");
    line = if focused {
        line.strong(title)
    } else {
        line.dim(title)
    };
    if !hint.is_empty() {
        line = line.dim(format!(" · {hint}"));
    }
    line = line.dim(" ");
    let fill = w.saturating_sub(line.width());
    if fill > 0 {
        line = line.dim("─".repeat(fill));
    }
    truncate_to(line, w)
}

/// Each non-blank row preceded by the [`GUTTER`] that keeps feed text off the
/// divider.
fn indented(rows: Vec<Line>) -> Vec<Line> {
    rows.into_iter()
        .map(|l| {
            if l.segments.is_empty() {
                return l;
            }
            let mut row = Line::new().plain(" ".repeat(GUTTER));
            row.segments.extend(l.segments);
            row
        })
        .collect()
}

/// `line` clipped to `w` and blank-padded out to exactly `w` columns, so a
/// divider glyph after it lands in one column on every row.
fn pad_to(line: Line, w: usize) -> Line {
    let line = truncate_to(line, w);
    match w.saturating_sub(line.width()) {
        0 => line,
        gap => line.plain(" ".repeat(gap)),
    }
}

/// Exactly `rows` rows of `lines` (oldest → newest), tail-anchored and held
/// `offset` lines above the tail, each clipped to `w`, blank-padded below.
/// An empty feed is ONE dim [`WAITING`] row. An offset past the scrollback is
/// bounded here too, so the window is always full of the oldest lines rather
/// than blank.
fn feed_rows(lines: &[Line], offset: usize, rows: usize, w: usize) -> Vec<Line> {
    let mut out: Vec<Line> = if lines.is_empty() {
        vec![Line::new().dim(WAITING)]
    } else {
        let end = lines.len() - offset.min(lines.len().saturating_sub(rows));
        let start = end.saturating_sub(rows);
        lines[start..end].to_vec()
    };
    out.truncate(rows);
    let mut out: Vec<Line> = out.into_iter().map(|l| truncate_to(l, w)).collect();
    out.resize(rows, Line::new());
    out
}

/// What the stream is showing, for its title: the drilled path below the run
/// (`hunt › otter#1`), else the step the cursor is on, else the neutral
/// `run`. Control characters are scrubbed (the text comes off the wire).
fn selection_label(view: &RunView, nav: &NavState) -> String {
    // `breadcrumb` leads with the crew crumb once the run's codename is known.
    let skip = usize::from(view.crew.is_some());
    let path: Vec<String> = nav
        .breadcrumb(view)
        .into_iter()
        .skip(skip)
        .map(|c| printable(&c))
        .collect();
    if !path.is_empty() {
        return path.join(" › ");
    }
    nav.chosen_step(view)
        .map_or_else(|| "run".to_string(), |s| printable(&s.step_id))
}

/// Agents running right now: a running leaf step counts as one, a running
/// fan-out contributes its running units (never itself on top of them), and
/// each running sub-agent dispatch is one more live transcript. A real count
/// of live work — `0` for a finished or parked run.
fn active_count(view: &RunView) -> usize {
    let steps: usize = view
        .steps
        .iter()
        .filter(|s| s.state == StepState::Running)
        .map(|s| {
            if s.units.is_empty() {
                1
            } else {
                s.unit_counts().running
            }
        })
        .sum();
    let dispatches = view
        .dispatches
        .values()
        .filter(|d| d.status == UnitStatus::Running)
        .count();
    steps + dispatches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::nav::{NavKey, Pane};
    use crate::output::live_view::row::{render_plain, Style};
    use crate::output::run_model::GateView;
    use chrono::{Duration, TimeZone};
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap()
    }

    const WF: &str = r#"
name: assess-services
steps:
  - id: preflight
    agent: scanner
    prompt: go
  - id: sweep
    agent: sweeper
    prompt: p
  - id: report
    agent: reporter
    prompt: p
"#;

    const GATE_WF: &str = r#"
name: release-train
steps:
  - id: build
    agent: builder
    prompt: go
  - id: approve-deploy
    approval:
      prompt: "Ship it?"
  - id: deploy
    agent: deployer
    prompt: p
"#;

    const FANOUT_WF: &str = r#"
name: fanout
steps:
  - id: hunt
    for_each: '["a", "b", "c", "d"]'
    agent: worker
    prompt: p
"#;

    fn wf() -> Workflow {
        Workflow::parse(WF).expect("test workflow parses")
    }

    fn start(v: &mut RunView, step: &str, agent: &str, codename: &str) {
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind: StepKind::Linear,
            agent: Some(agent.into()),
            host: None,
            codename: Some(codename.into()),
        });
    }

    fn complete(v: &mut RunView, step: &str, ms: u64) {
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: step.into(),
            success: true,
            duration_ms: ms,
            host: None,
        });
    }

    /// preflight done, sweep running, report not started; 4m 12s in, priced
    /// usage and two findings so the header has its full meter row.
    fn live() -> RunView {
        let mut v = RunView::default();
        v.workflow_name = "assess-services".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        v.started_at = Some(now() - Duration::seconds(4 * 60 + 12));
        start(&mut v, "preflight", "scanner", "heron#1");
        complete(&mut v, "preflight", 18_000);
        start(&mut v, "sweep", "sweeper", "otter#2");
        v.step_mut("report");
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 6_100_000,
            output_tokens: 420_000,
            total_tokens: 6_520_000,
            cost_usd: Some(18.40),
            priced: true,
            ..Default::default()
        });
        v.findings_by_severity.insert("high".into(), 2);
        v
    }

    /// `build` done, parked at `approve-deploy`, `deploy` still to come.
    fn gate_parked() -> RunView {
        let mut v = RunView::default();
        v.workflow_name = "release-train".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::AwaitingApproval;
        v.started_at = Some(now() - Duration::seconds(7 * 60 + 41));
        start(&mut v, "build", "builder", "heron#1");
        complete(&mut v, "build", 96_000);
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "approve-deploy".into(),
            kind: StepKind::ApprovalGate,
            agent: None,
            host: None,
            codename: None,
        });
        v.apply(&Event::StepAwaitingApproval {
            run_id: "r".into(),
            step_id: "approve-deploy".into(),
            reason: "awaiting approval".into(),
        });
        v.step_mut("deploy");
        v.gates = vec![GateView {
            step_id: "approve-deploy".into(),
            prompt: Some("Ship it?".into()),
            since: now() - Duration::seconds(95),
            expires_at: None,
        }];
        v
    }

    fn stream_fixture() -> Vec<Line> {
        vec![
            Line::new().dim("11:42:31 ").plain("▸ read_file src/lib.rs"),
            Line::new()
                .dim("11:42:33 ")
                .plain("◇ thinking \"check the auth path\""),
            Line::new()
                .dim("11:42:39 ")
                .danger("⚑ HIGH unchecked unwrap"),
            Line::new()
                .dim("11:42:44 ")
                .plain("▪ assistant wrote the report"),
        ]
    }

    fn firehose_fixture() -> Vec<Line> {
        vec![
            Line::new().plain("otter#2  ").dim("▸ grep unwrap"),
            Line::new().plain("heron#1  ").dim("✓ done"),
            Line::new().plain("lynx#3   ").dim("▸ read_file Cargo.toml"),
        ]
    }

    fn frame(
        view: &RunView,
        nav: &NavState,
        stream: &[Line],
        firehose: &[Line],
        w: usize,
        h: usize,
    ) -> Vec<Line> {
        dashboard_frame(view, &wf(), nav, stream, firehose, now(), w, h)
    }

    fn plain(l: &Line) -> String {
        render_plain(std::slice::from_ref(l))
    }

    fn rows_of(lines: &[Line]) -> Vec<String> {
        lines.iter().map(plain).collect()
    }

    /// The style of the first segment whose text starts with `prefix`.
    fn style_of<'a>(line: &'a Line, prefix: &str) -> &'a Style {
        &line
            .segments
            .iter()
            .find(|s| s.text.starts_with(prefix))
            .unwrap_or_else(|| panic!("no segment starting {prefix:?} in {line:?}"))
            .style
    }

    fn row_with<'a>(lines: &'a [Line], needle: &str) -> &'a Line {
        lines
            .iter()
            .find(|l| plain(l).contains(needle))
            .unwrap_or_else(|| panic!("no row contains {needle:?}:\n{}", render_plain(lines)))
    }

    fn char_at(s: &str, col: usize) -> Option<char> {
        s.chars().nth(col)
    }

    #[test]
    fn wide_frame_composes_header_structure_stream_firehose_footer() {
        let out = frame(
            &live(),
            &NavState::default(),
            &stream_fixture(),
            &firehose_fixture(),
            100,
            28,
        );
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        let rows = rows_of(&out);
        // Fills the frame: 3 header rows + 24 body rows + footer.
        assert_eq!(out.len(), 28, "{s}");
        assert!(out.iter().all(|l| l.width() <= 100), "{s}");

        // Header block on top, footer legend at the bottom.
        assert!(rows[0].starts_with("assess-services"), "{s}");
        assert!(rows.last().unwrap().contains("q quit"), "{s}");

        // All three regions are present.
        let top = rows.iter().position(|r| r.contains("─ structure")).unwrap();
        assert_eq!(top, 3, "the body starts right under the 3-row header:\n{s}");
        assert!(rows[top].contains("─ stream · "), "{s}");
        let rule = rows
            .iter()
            .position(|r| r.contains("├─ live · 1 active"))
            .unwrap_or_else(|| panic!("no firehose rule:\n{s}"));
        assert!(rule > top, "{s}");
        assert!(
            rows.iter().any(|r| r.contains("read_file src/lib.rs")),
            "{s}"
        );
        assert!(rows.iter().any(|r| r.contains("grep unwrap")), "{s}");
        assert!(rows.iter().any(|r| r.contains("preflight")), "{s}");

        // The divider sits in ONE column (struct_w = 100*2/5 = 40) on every
        // body row: `┬` on the title row, `├` on the firehose rule, `│` else.
        let footer = rows.len() - 1;
        for (i, r) in rows.iter().enumerate().take(footer).skip(top) {
            let want = if i == top {
                '┬'
            } else if i == rule {
                '├'
            } else {
                '│'
            };
            assert_eq!(char_at(r, 40), Some(want), "row {i}:\n{s}");
        }

        // The firehose region is the bottom ~40% of the body: its rule plus
        // rows down to the footer, at least the 3-row floor.
        assert!(footer - rule > 3, "{s}");
        // The stream gets the larger share (~60%).
        assert!(rule - top > footer - rule, "{s}");
    }

    #[test]
    fn firehose_floor_holds_when_the_body_is_short() {
        // h=11 → body 7 rows: 60% would leave the firehose 3 (rule + 2);
        // the floor takes it back to rule + 3 rows.
        let out = frame(
            &live(),
            &NavState::default(),
            &stream_fixture(),
            &firehose_fixture(),
            100,
            11,
        );
        let s = render_plain(&out);
        assert_eq!(out.len(), 11, "{s}");
        let rows = rows_of(&out);
        let rule = rows.iter().position(|r| r.contains("├─ live")).unwrap();
        assert_eq!(rows.len() - 1 - rule - 1, 3, "{s}");
        // And the stream keeps its title + at least one row.
        let top = rows.iter().position(|r| r.contains("─ stream")).unwrap();
        assert!(rule - top >= 2, "{s}");
    }

    #[test]
    fn narrow_frame_is_a_single_stacked_pane() {
        let nav = NavState::default();
        let out = frame(
            &live(),
            &nav,
            &stream_fixture(),
            &firehose_fixture(),
            60,
            24,
        );
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        let rows = rows_of(&out);
        assert!(out.len() <= 24, "{s}");
        assert!(out.iter().all(|l| l.width() <= 60), "{s}");
        // Header, ONE title (the focused structure pane), the structure
        // rows, the footer — and none of the other panes.
        assert!(rows[0].starts_with("assess-services"), "{s}");
        assert_eq!(
            rows.iter().filter(|r| r.contains("─ structure")).count(),
            1,
            "{s}"
        );
        assert!(rows.iter().any(|r| r.contains("tab")), "tab hint:\n{s}");
        assert!(!rows.iter().any(|r| r.contains("─ stream")), "{s}");
        assert!(!rows.iter().any(|r| r.contains("live ·")), "{s}");
        assert!(
            !rows.iter().any(|r| r.contains("read_file src/lib.rs")),
            "{s}"
        );
        assert!(rows.iter().any(|r| r.contains("preflight")), "{s}");
        assert!(rows.last().unwrap().contains("q quit"), "{s}");
    }

    #[test]
    fn narrow_frame_shows_whichever_pane_has_focus() {
        let mut nav = NavState::default();
        nav.cycle_pane(true);
        assert_eq!(nav.pane(), Pane::Stream);
        let out = frame(
            &live(),
            &nav,
            &stream_fixture(),
            &firehose_fixture(),
            60,
            24,
        );
        let s = render_plain(&out);
        assert!(s.contains("─ stream · "), "{s}");
        assert!(s.contains("read_file src/lib.rs"), "{s}");
        assert!(!s.contains("─ structure"), "{s}");
        assert!(!s.contains("grep unwrap"), "{s}");

        nav.cycle_pane(true);
        assert_eq!(nav.pane(), Pane::Firehose);
        let out = frame(
            &live(),
            &nav,
            &stream_fixture(),
            &firehose_fixture(),
            60,
            24,
        );
        let s = render_plain(&out);
        assert!(s.contains("─ live · 1 active"), "{s}");
        assert!(s.contains("grep unwrap"), "{s}");
        assert!(!s.contains("read_file src/lib.rs"), "{s}");
    }

    #[test]
    fn a_short_terminal_falls_back_even_when_wide() {
        // h=9 → body 5 rows: no room for stream + the firehose floor.
        let out = frame(
            &live(),
            &NavState::default(),
            &stream_fixture(),
            &firehose_fixture(),
            100,
            9,
        );
        let s = render_plain(&out);
        assert!(out.len() <= 9, "{s}");
        assert!(s.contains("─ structure"), "{s}");
        assert!(!s.contains("─ stream"), "{s}");
        assert!(!s.contains("live ·"), "{s}");
    }

    #[test]
    fn gate_parked_footer_shows_the_gate_legend() {
        let gate_wf = Workflow::parse(GATE_WF).expect("parses");
        let v = gate_parked();
        let out = dashboard_frame(
            &v,
            &gate_wf,
            &NavState::default(),
            &stream_fixture(),
            &firehose_fixture(),
            now(),
            100,
            28,
        );
        let s = render_plain(&out);
        insta::assert_snapshot!(s);
        assert_eq!(
            rows_of(&out).last().unwrap(),
            "a approve · r reject · v findings · Esc pause · q quit",
            "{s}"
        );
        // …and the gate is on screen in the structure column.
        assert!(s.contains("approve-deploy"), "{s}");
    }

    #[test]
    fn empty_panes_show_one_waiting_row_each() {
        let out = frame(&live(), &NavState::default(), &[], &[], 100, 28);
        let s = render_plain(&out);
        let rows = rows_of(&out);
        assert_eq!(s.matches("waiting for activity…").count(), 2, "{s}");
        // Directly under each region's title row, never a zero-height pane.
        let top = rows.iter().position(|r| r.contains("─ stream")).unwrap();
        assert!(rows[top + 1].contains("waiting for activity…"), "{s}");
        let rule = rows.iter().position(|r| r.contains("├─ live")).unwrap();
        assert!(rows[rule + 1].contains("waiting for activity…"), "{s}");
        // The placeholder is dim, not content.
        assert_eq!(
            *style_of(row_with(&out, "waiting for activity…"), "waiting"),
            Style::Dim
        );

        // The narrow fallback gets exactly one, in the focused pane.
        let mut nav = NavState::default();
        nav.cycle_pane(true);
        let out = frame(&live(), &nav, &[], &[], 60, 24);
        let s = render_plain(&out);
        assert_eq!(s.matches("waiting for activity…").count(), 1, "{s}");
        nav.cycle_pane(true);
        let out = frame(&live(), &nav, &[], &[], 60, 24);
        assert_eq!(
            render_plain(&out).matches("waiting for activity…").count(),
            1
        );
    }

    #[test]
    fn the_focused_pane_title_is_strong_the_rest_dim() {
        let mut nav = NavState::default();
        let strong_dim = |nav: &NavState| {
            let out = frame(
                &live(),
                nav,
                &stream_fixture(),
                &firehose_fixture(),
                100,
                28,
            );
            let title = row_with(&out, "─ structure").clone();
            let rule = row_with(&out, "├─ live").clone();
            (
                style_of(&title, "structure").clone(),
                style_of(&title, "stream").clone(),
                style_of(&rule, "live").clone(),
            )
        };
        assert_eq!(strong_dim(&nav), (Style::Strong, Style::Dim, Style::Dim));
        nav.cycle_pane(true);
        assert_eq!(strong_dim(&nav), (Style::Dim, Style::Strong, Style::Dim));
        nav.cycle_pane(true);
        assert_eq!(strong_dim(&nav), (Style::Dim, Style::Dim, Style::Strong));
    }

    #[test]
    fn stream_and_firehose_are_tail_anchored_and_scrollable() {
        let numbered = |tag: &str, n: usize| -> Vec<Line> {
            (1..=n)
                .map(|i| Line::new().plain(format!("{tag}-{i:03}")))
                .collect()
        };
        let stream = numbered("sline", 60);
        let firehose = numbered("fline", 60);
        let view = live();

        // Tail-anchored: the newest line is the last row of each region.
        let nav = NavState::default();
        let out = frame(&view, &nav, &stream, &firehose, 100, 28);
        let rows = rows_of(&out);
        let rule = rows.iter().position(|r| r.contains("├─ live")).unwrap();
        assert!(rows[rule - 1].contains("sline-060"), "{}", rows.join("\n"));
        assert!(
            rows[rows.len() - 2].contains("fline-060"),
            "{}",
            rows.join("\n")
        );
        // Older lines are windowed out, not drawn.
        assert!(!rows.iter().any(|r| r.contains("sline-001")));

        // Scrolling the focused stream pane up a page slides its window.
        let mut nav = NavState::default();
        nav.cycle_pane(true);
        nav.apply(NavKey::ScrollUp, &view);
        assert_eq!(nav.scroll_offset(Pane::Stream), 10);
        let out = frame(&view, &nav, &stream, &firehose, 100, 28);
        let rows = rows_of(&out);
        let rule = rows.iter().position(|r| r.contains("├─ live")).unwrap();
        assert!(rows[rule - 1].contains("sline-050"), "{}", rows.join("\n"));
        // The firehose was not scrolled.
        assert!(rows[rows.len() - 2].contains("fline-060"));

        // An offset past the real scrollback (frame never clamps nav) still
        // draws a FULL window of the oldest lines, never a blank pane.
        for _ in 0..20 {
            nav.apply(NavKey::ScrollUp, &view);
        }
        let out = frame(&view, &nav, &stream, &firehose, 100, 28);
        let rows = rows_of(&out);
        let top = rows.iter().position(|r| r.contains("─ stream")).unwrap();
        let rule = rows.iter().position(|r| r.contains("├─ live")).unwrap();
        assert!(rows[top + 1].contains("sline-001"), "{}", rows.join("\n"));
        assert!(rows[rule - 1].contains("sline-"), "{}", rows.join("\n"));
        assert_eq!(rule - top - 1, 13, "a full stream window");
        assert!(rows[rule - 1].contains("sline-013"), "{}", rows.join("\n"));
    }

    #[test]
    fn scroll_rows_matches_what_the_frame_draws() {
        let stream: Vec<Line> = (0..500)
            .map(|i| Line::new().plain(format!("sline-{i:03}")))
            .collect();
        let firehose: Vec<Line> = (0..500)
            .map(|i| Line::new().plain(format!("fline-{i:03}")))
            .collect();
        let view = live();
        let drawn = |out: &[Line], tag: &str| out.iter().filter(|l| plain(l).contains(tag)).count();
        for (w, h) in [(100, 28), (100, 11), (60, 24), (120, 50), (100, 9)] {
            let mut nav = NavState::default();
            for _ in 0..3 {
                let out = frame(&view, &nav, &stream, &firehose, w, h);
                let rows = scroll_rows(&view, &nav, now(), w, h);
                assert_eq!(
                    drawn(&out, "sline-"),
                    rows.stream,
                    "stream {w}x{h} {:?}",
                    nav.pane()
                );
                assert_eq!(
                    drawn(&out, "fline-"),
                    rows.firehose,
                    "firehose {w}x{h} {:?}",
                    nav.pane()
                );
                nav.cycle_pane(true);
            }
        }
    }

    #[test]
    fn stream_title_names_the_actual_selection() {
        let view = live();
        let title = |nav: &NavState| {
            plain(row_with(
                &frame(&view, nav, &stream_fixture(), &firehose_fixture(), 100, 28),
                "─ stream",
            ))
        };
        // Following, nothing chosen: neutral.
        let nav = NavState::default();
        assert!(title(&nav).contains("─ stream · run"), "{}", title(&nav));
        // Move the cursor: that step is the selection.
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &view);
        assert!(title(&nav).contains("─ stream · sweep"), "{}", title(&nav));
    }

    #[test]
    fn active_count_is_real() {
        // One running leaf step → 1.
        assert_eq!(active_count(&live()), 1);
        // Finished run → 0 (a real zero, not hidden).
        let mut done = live();
        complete(&mut done, "sweep", 1_000);
        assert_eq!(active_count(&done), 0);
        // A fan-out counts its RUNNING units, not itself and not queued/done.
        let mut v = RunView::default();
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            kind: StepKind::ForEach,
            agent: None,
            host: None,
            codename: None,
        });
        for i in 0..4usize {
            v.apply(&Event::UnitStarted {
                run_id: "r".into(),
                step_id: "hunt".into(),
                index: i,
                unit_key: format!("u{i}"),
                agent: Some("worker".into()),
                transcript_path: "t".into(),
                host: None,
                codename: None,
            });
        }
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: "hunt".into(),
            index: 0,
            unit_key: "u0".into(),
            success: true,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        });
        assert_eq!(active_count(&v), 3);
        let fan = Workflow::parse(FANOUT_WF).expect("parses");
        let out = dashboard_frame(&v, &fan, &NavState::default(), &[], &[], now(), 100, 28);
        assert!(render_plain(&out).contains("├─ live · 3 active"));
    }

    #[test]
    fn zero_size_is_empty_and_no_size_panics_or_overflows() {
        let view = live();
        let nav = NavState::default();
        assert!(frame(&view, &nav, &stream_fixture(), &firehose_fixture(), 0, 28).is_empty());
        assert!(frame(&view, &nav, &stream_fixture(), &firehose_fixture(), 100, 0).is_empty());
        for pane in 0..3 {
            let mut nav = NavState::default();
            for _ in 0..pane {
                nav.cycle_pane(true);
            }
            for w in 1..130 {
                for h in 1..45 {
                    let out = frame(&view, &nav, &stream_fixture(), &firehose_fixture(), w, h);
                    assert!(out.len() <= h, "{w}x{h}: {} rows", out.len());
                    assert!(
                        out.iter().all(|l| l.width() <= w),
                        "{w}x{h}:\n{}",
                        render_plain(&out)
                    );
                }
            }
        }
    }
}
