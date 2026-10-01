//! The dashboard frame (spec 2026-10-01): the whole `workflow run` live view
//! composed from the pane renderers, as a pure function from the run's
//! models to styled [`Line`]s — no terminal I/O.
//!
//! ```text
//! <header: layout::dashboard — title · progress · meters>
//! ─ structure ───────────┬─ stream · <selection> ───────────
//!  the workflow DAG      │  the selection's transcript
//!  (structure::…)        ├─ live · N active · H hosts ──────
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
//! * The firehose rule counts live agents (`N active`) and, only when the
//!   running work names any, the distinct hosts it runs on (`H hosts`).
//!
//! The stream and firehose are windowed **tail-anchored**: once a feed has at
//! least as many lines as the region has rows, its newest line is the region's
//! last row, and [`NavState::scroll_offset`] holds the window that many lines
//! above the tail. A shorter feed is top-aligned instead (oldest line first,
//! blank rows below). [`dashboard_frame`] only *reads* the offset (it takes
//! `&NavState`); the driver clamps it each frame against the real buffer
//! using [`scroll_rows`], which reports how many rows each pane really draws.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::StepKind;
use rupu_orchestrator::Workflow;

use crate::output::live_view::layout::{dashboard, footer_line, printable};
use crate::output::live_view::nav::{NavState, Pane};
use crate::output::live_view::row::{truncate_to, Line};
use crate::output::live_view::structure::structure_pane;
use crate::output::run_model::{RunView, StepState, StepView, UnitStatus};

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
        let hosts = match active_hosts(self.view) {
            0 => String::new(),
            1 => " · 1 host".to_string(),
            n => format!(" · {n} hosts"),
        };
        format!("live · {} active{hosts}", active_count(self.view))
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

/// Exactly `rows` rows of `lines` (oldest → newest), each clipped to `w`:
/// the window is anchored to the live tail and held `offset` lines above it,
/// so with at least `rows` lines the newest is the last row. A feed shorter
/// than `rows` fits whole: top-aligned, blank-padded below. An empty feed is
/// ONE dim [`WAITING`] row. An offset past the scrollback is bounded here too,
/// so the window is always full of the oldest lines rather than blank.
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

/// The running steps that are themselves work. A `loop:` step stays `Running`
/// for the whole loop but only frames its members (which are steps of their
/// own), so counting it too would report an agent that does not exist.
fn running_work(view: &RunView) -> impl Iterator<Item = &StepView> {
    view.steps
        .iter()
        .filter(|s| s.state == StepState::Running && s.kind != StepKind::Loop)
}

/// Agents running right now: a running leaf step counts as one, a running
/// fan-out contributes its running units (never itself on top of them), and
/// each running sub-agent dispatch is one more live transcript. A real count
/// of live work — `0` for a finished or parked run.
fn active_count(view: &RunView) -> usize {
    let steps: usize = running_work(view)
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

/// Distinct hosts the live work (the same work [`active_count`] counts) runs
/// on: a running leaf step's own host, a running fan-out's running units'
/// hosts. Work the run has not placed on a named host (`host: None` = the
/// orchestrator's own machine) names no host, so a purely local run is `0` —
/// the title then omits the figure instead of inventing one.
fn active_hosts(view: &RunView) -> usize {
    let mut hosts: BTreeSet<&str> = BTreeSet::new();
    for step in running_work(view) {
        if step.units.is_empty() {
            hosts.extend(step.host.as_deref());
        } else {
            hosts.extend(
                step.units
                    .values()
                    .filter(|u| u.status == UnitStatus::Running)
                    .filter_map(|u| u.host.as_deref()),
            );
        }
    }
    hosts.retain(|h| !h.is_empty());
    hosts.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::mux::project_event;
    use crate::output::live_view::nav::{Depth, NavKey, Pane};
    use crate::output::live_view::row::{render_plain, Style};
    use crate::output::run_model::{GateView, UnitView};
    use chrono::{Duration, TimeZone};
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::RunStatus;
    use rupu_transcript::Event as Tx;
    use serde_json::json;

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

    /// One of every construct the structure pane draws, in one workflow: a
    /// split into a `for_each`, a `parallel` and a plain step, a join, a
    /// `branch` (then/else), an approval gate, a bounded loop, and a panel.
    const COMPOSITE_WF: &str = r#"
name: assess-fleet
steps:
  - id: preflight
    agent: scanner
    prompt: go
    next: [recon]
  - id: recon
    split: [sweep, hunt, lanes]
  - id: sweep
    agent: sweeper
    prompt: p
    next: [gather]
  - id: hunt
    for_each: '["a", "b", "c", "d"]'
    agent: worker
    prompt: p
    next: [gather]
  - id: lanes
    parallel:
      - id: spec
        agent: writer
        prompt: a
      - id: verify
        agent: reviewer
        prompt: b
    next: [gather]
  - id: gather
    join: { wait: all }
  - id: pick
    branch:
      condition: "{{ steps.sweep.output }}"
      then: [ship]
      else: [hold]
  - id: ship
    agent: shipper
    prompt: p
  - id: hold
    agent: holder
    prompt: p
  - id: approve
    approval:
      prompt: "Ship it?"
  - id: gen
    agent: generator
    prompt: p
  - id: critique
    agent: critic
    prompt: p
    depends_on: [gen]
  - id: review
    panel:
      panelists: [security-reviewer, perf-reviewer]
      subject: "{{ steps.critique.output }}"
loops:
  refine:
    nodes: [gen, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 5
"#;

    /// A preflight, a `for_each` over a batch of services, and the report.
    const DRILL_WF: &str = r#"
name: breakers
steps:
  - id: preflight
    agent: scanner
    prompt: go
    next: [hunt]
  - id: hunt
    for_each: '["auth", "billing", "search"]'
    agent: breaker
    prompt: p
    next: [report]
  - id: report
    agent: reporter
    prompt: p
    depends_on: [hunt]
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

    // ---- whole-frame fixtures ---------------------------------------------------

    /// `s01..s12` chained, then two split→join phases (`recon1`, then
    /// `recon2` fanning a `for_each` `hunt` beside a `probe`) and a `report`.
    fn large_wf() -> Workflow {
        let mut yaml = String::from("name: sweep-fleet\nsteps:\n");
        for i in 1..=12 {
            let next = if i == 12 {
                "recon1".to_string()
            } else {
                format!("s{:02}", i + 1)
            };
            yaml.push_str(&format!(
                "  - id: s{i:02}\n    agent: scanner\n    prompt: p\n    next: [{next}]\n"
            ));
        }
        yaml.push_str(
            r#"  - id: recon1
    split: [a, b]
  - id: a
    agent: ax
    prompt: p
    next: [join1]
  - id: b
    agent: bx
    prompt: p
    next: [join1]
  - id: join1
    join: { wait: all }
    next: [recon2]
  - id: recon2
    split: [hunt, probe]
  - id: hunt
    for_each: '["a", "b"]'
    agent: breaker
    prompt: p
    next: [join2]
  - id: probe
    agent: prober
    prompt: p
    next: [join2]
  - id: join2
    join: { wait: all }
    next: [report]
  - id: report
    agent: reporter
    prompt: p
"#,
        );
        Workflow::parse(&yaml).expect("large workflow parses")
    }

    const ROLES: [&str; 5] = ["otter", "heron", "wren", "egret", "lynx"];
    /// Started fan-out units alternate between these two placed hosts.
    const HOSTS: [&str; 2] = ["mini", "kuki"];

    /// A fan-out unit with the identity a started unit carries; queued units
    /// know neither a codename nor a provider/model.
    fn unit_of(i: usize, status: UnitStatus) -> UnitView {
        let started = status != UnitStatus::Queued;
        UnitView {
            index: i,
            unit_key: format!("svc-{i}"),
            agent: Some("breaker".into()),
            codename: started.then(|| format!("{}#{}", ROLES[i % ROLES.len()], i + 1)),
            provider: started.then(|| "anthropic".to_string()),
            model: started.then(|| "claude-opus-5-5".to_string()),
            host: started.then(|| HOSTS[i % HOSTS.len()].to_string()),
            status,
        }
    }

    /// The large run: every `s*` step and the first phase settled, the second
    /// phase open with `hunt` fanned over 86 units (52 done, 2 failed, 6
    /// running, 26 queued) and `probe` done; the join and report to come.
    fn large_view() -> RunView {
        let mut v = RunView::default();
        v.workflow_name = "sweep-fleet".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        v.started_at = Some(now() - Duration::seconds(38 * 60 + 5));
        for i in 1..=12 {
            let id = format!("s{i:02}");
            begin(&mut v, &id, StepKind::Linear, Some("scanner"), None, None);
            complete(&mut v, &id, 1_000 + i as u64);
        }
        begin(&mut v, "recon1", StepKind::Split, None, None, None);
        complete(&mut v, "recon1", 1_000);
        begin(&mut v, "a", StepKind::Linear, Some("ax"), None, None);
        complete(&mut v, "a", 4_000);
        begin(&mut v, "b", StepKind::Linear, Some("bx"), None, None);
        complete(&mut v, "b", 6_000);
        begin(&mut v, "join1", StepKind::Join, None, None, None);
        complete(&mut v, "join1", 100);
        begin(&mut v, "recon2", StepKind::Split, None, None, None);
        complete(&mut v, "recon2", 1_000);
        begin(&mut v, "hunt", StepKind::ForEach, None, None, None);
        for i in 0..86 {
            let status = match i {
                7 | 31 => UnitStatus::Failed,
                0..=53 => UnitStatus::Done,
                54..=59 => UnitStatus::Running,
                _ => UnitStatus::Queued,
            };
            v.step_mut("hunt").units.insert(i, unit_of(i, status));
        }
        begin(
            &mut v,
            "probe",
            StepKind::Linear,
            Some("prober"),
            None,
            None,
        );
        complete(&mut v, "probe", 5_000);
        v.step_mut("join2");
        v.step_mut("report");
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 41_000_000,
            output_tokens: 2_300_000,
            total_tokens: 43_300_000,
            cost_usd: Some(112.60),
            priced: true,
            ..Default::default()
        });
        v.findings_by_severity.insert("high".into(), 4);
        v
    }

    /// The pinned/merged activity of the six running units, as the mux
    /// projects it.
    fn running_units_firehose() -> Vec<Line> {
        let at = |i: usize| format!("{}#{}", ROLES[i % ROLES.len()], i + 1);
        let (a, b, c, d, e, f) = (at(54), at(55), at(56), at(57), at(58), at(59));
        projected(&[
            (
                &a,
                call("read_file", json!({"path": "svc/auth/handler.rs"})),
            ),
            (&b, call("grep", json!({"pattern": "unwrap()"}))),
            (
                &c,
                call("read_file", json!({"path": "svc/billing/ledger.rs"})),
            ),
            (&a, finding("high", "token accepted after revoke")),
            (
                &d,
                call("read_file", json!({"path": "svc/search/index.rs"})),
            ),
            (&e, call("grep", json!({"pattern": "TODO"}))),
            (&f, call("read_file", json!({"path": "svc/export/csv.rs"}))),
            (
                &b,
                call("read_file", json!({"path": "svc/auth/session.rs"})),
            ),
        ])
    }

    fn begin(
        v: &mut RunView,
        step: &str,
        kind: StepKind,
        agent: Option<&str>,
        codename: Option<&str>,
        host: Option<&str>,
    ) {
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind,
            agent: agent.map(Into::into),
            host: host.map(Into::into),
            codename: codename.map(Into::into),
        });
    }

    fn agent_up(v: &mut RunView, step: &str, unit: Option<usize>, provider: &str, model: &str) {
        v.apply(&Event::AgentStarted {
            run_id: "r".into(),
            step_id: step.into(),
            unit_index: unit,
            codename: None,
            agent: "a".into(),
            provider: Some(provider.into()),
            model: Some(model.into()),
            agent_run_id: "ar".into(),
            transcript_path: "t".into(),
        });
    }

    fn unit_up(
        v: &mut RunView,
        step: &str,
        index: usize,
        agent: &str,
        codename: &str,
        host: Option<&str>,
    ) {
        v.apply(&Event::UnitStarted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("svc-{index}"),
            agent: Some(agent.into()),
            transcript_path: "t".into(),
            host: host.map(Into::into),
            codename: Some(codename.into()),
        });
    }

    fn unit_down(v: &mut RunView, step: &str, index: usize, success: bool) {
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("svc-{index}"),
            success,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        });
    }

    /// The nav a cursor on `step` gives (steps in first-seen order).
    fn nav_on(view: &RunView, step: &str) -> NavState {
        let at = view.steps.iter().position(|s| s.step_id == step).unwrap();
        let mut nav = NavState::default();
        for _ in 0..at {
            nav.apply(NavKey::Down, view);
        }
        nav
    }

    fn call(tool: &str, input: serde_json::Value) -> Tx {
        Tx::ToolCall {
            call_id: "c".into(),
            tool: tool.into(),
            input,
        }
    }

    /// Feed rows exactly as the mux would project them: each `(codename,
    /// event)` through the real [`project_event`], in arrival order.
    fn projected(events: &[(&str, Tx)]) -> Vec<Line> {
        events
            .iter()
            .filter_map(|(codename, ev)| project_event(ev, Some(codename)))
            .map(|f| f.line)
            .collect()
    }

    /// A transcript from one agent (`codename`): what the stream pane shows
    /// when it is pinned to it.
    fn transcript(codename: &str, events: Vec<Tx>) -> Vec<Line> {
        let tagged: Vec<(&str, Tx)> = events.into_iter().map(|e| (codename, e)).collect();
        projected(&tagged)
    }

    fn finding(severity: &str, title: &str) -> Tx {
        Tx::ActionEmitted {
            kind: "finding".into(),
            payload: json!({ "severity": severity, "title": title }),
            allowed: true,
            applied: true,
            reason: None,
        }
    }

    /// The composite workflow mid-run, inside its fan-out phase: the split
    /// has fanned out, `sweep` is done, the `for_each` `hunt` is running over
    /// four items (one done, two running, one queued) and the `parallel`
    /// lanes have one sub-step done and one running — on two hosts. The join
    /// and everything after it (branch, gate, loop, panel) have not started.
    fn composite_view() -> RunView {
        let mut v = RunView::default();
        v.workflow_name = "assess-fleet".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        v.started_at = Some(now() - Duration::seconds(3 * 60 + 40));

        begin(
            &mut v,
            "preflight",
            StepKind::Linear,
            Some("scanner"),
            Some("heron#1"),
            Some("kuki"),
        );
        agent_up(&mut v, "preflight", None, "anthropic", "claude-opus-5-5");
        complete(&mut v, "preflight", 18_000);
        begin(&mut v, "recon", StepKind::Split, None, None, None);
        complete(&mut v, "recon", 2_000);
        begin(
            &mut v,
            "sweep",
            StepKind::Linear,
            Some("sweeper"),
            Some("otter#2"),
            Some("mini"),
        );
        agent_up(&mut v, "sweep", None, "openai", "gpt-5");
        complete(&mut v, "sweep", 41_000);

        begin(&mut v, "hunt", StepKind::ForEach, None, None, None);
        unit_up(&mut v, "hunt", 0, "worker", "lynx#1", Some("mini"));
        unit_down(&mut v, "hunt", 0, true);
        unit_up(&mut v, "hunt", 1, "worker", "wren#1", Some("kuki"));
        unit_up(&mut v, "hunt", 2, "worker", "egret#1", Some("mini"));
        v.step_mut("hunt")
            .units
            .insert(3, unit_of(3, UnitStatus::Queued));

        begin(&mut v, "lanes", StepKind::Parallel, None, None, None);
        unit_up(&mut v, "lanes", 0, "writer", "otter#3", None);
        unit_down(&mut v, "lanes", 0, true);
        unit_up(&mut v, "lanes", 1, "reviewer", "heron#2", Some("kuki"));

        // Not started: registered in workflow order, as the live seed does.
        for id in [
            "gather", "pick", "ship", "hold", "approve", "gen", "critique", "review",
        ] {
            v.step_mut(id);
        }

        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 3_400_000,
            output_tokens: 210_000,
            total_tokens: 3_610_000,
            cost_usd: Some(9.85),
            priced: true,
            ..Default::default()
        });
        v.findings_by_severity.insert("high".into(), 1);
        v.findings_by_severity.insert("medium".into(), 2);
        v
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
        // The parked gate has no transcript of its own (the stream follows the
        // gate step, so it idles) and nothing is running (the firehose has
        // nothing live to merge): both panes say so rather than showing
        // activity that cannot exist.
        let out = dashboard_frame(&v, &gate_wf, &NavState::default(), &[], &[], now(), 100, 28);
        let s = render_plain(&out);
        insta::assert_snapshot!(s);
        assert_eq!(
            rows_of(&out).last().unwrap(),
            "a approve · r reject · v findings · Esc pause · q quit",
            "{s}"
        );
        // …and the gate is on screen in the structure column.
        assert!(s.contains("approve-deploy"), "{s}");
        // The honest placeholders: one per feed pane, a real zero active.
        assert_eq!(s.matches("waiting for activity…").count(), 2, "{s}");
        assert!(s.contains("├─ live · 0 active ─"), "{s}");
        assert!(!s.contains("read_file"), "{s}");
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
    fn composite_dag_frame_shows_every_construct() {
        let wf = Workflow::parse(COMPOSITE_WF).expect("parses");
        let v = composite_view();
        // The cursor is on the finished `sweep`, so the stream pane shows that
        // step's own transcript; the firehose is the merged live activity.
        let nav = nav_on(&v, "sweep");
        let stream = transcript(
            "otter#2",
            vec![
                call("list_files", json!({})),
                call("read_file", json!({"path": "deploy/ingress.yaml"})),
                Tx::Thinking {
                    text: Some("the ingress exposes the admin port".into()),
                    provider: "openai".into(),
                    model: "gpt-5".into(),
                    raw: json!({}),
                },
                finding("medium", "admin port reachable from the edge"),
                Tx::AssistantMessage {
                    content: "Swept 14 services; one exposure noted.".into(),
                    thinking: None,
                },
            ],
        );
        let firehose = projected(&[
            (
                "lynx#1",
                call("read_file", json!({"path": "svc/auth/handler.rs"})),
            ),
            ("heron#2", call("grep", json!({"pattern": "TODO"}))),
            (
                "wren#1",
                call("read_file", json!({"path": "svc/billing/ledger.rs"})),
            ),
            (
                "egret#1",
                call("read_file", json!({"path": "svc/search/index.rs"})),
            ),
            ("wren#1", finding("high", "ledger totals lose cents")),
            (
                "heron#2",
                call("read_file", json!({"path": "docs/runbook.md"})),
            ),
        ]);
        let out = dashboard_frame(&v, &wf, &nav, &stream, &firehose, now(), 120, 53);
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        assert!(out.len() <= 53, "{s}");
        assert!(out.iter().all(|l| l.width() <= 120), "{s}");
        // Every construct of the workflow is on screen, each with its glyph.
        for construct in [
            "◈  recon",      // split
            "⊞ hunt",        // for_each
            "⇉ lanes",       // parallel
            "◈◄─ gather",    // join
            "◇  pick",       // branch
            "▶ then → ship", // …its arms
            "⊘ else → hold",
            "⏸  approve",    // gate
            "↻ loop:refine", // loop frame
            "↺ loop:refine",
            "⟲ review", // panel
        ] {
            assert!(s.contains(construct), "{construct}:\n{s}");
        }
        // Nothing is folded: the frontier phase is open, the pane has room.
        assert!(!s.contains('⋮') && !s.contains("(+"), "{s}");
        assert!(!s.contains("⟦"), "{s}");
        // The live fan-outs carry their density lines under their rails.
        assert!(s.contains("1/4 ✓1 ◐2 ○1"), "{s}");
        assert!(s.contains("1/2 ✓1 ◐1"), "{s}");
        // Stream follows the cursor; the firehose rule counts live agents
        // and the hosts they run on.
        assert!(s.contains("─ stream · sweep ─"), "{s}");
        assert!(s.contains("otter#2 ▸ read_file deploy/ingress.yaml"), "{s}");
        assert!(s.contains("├─ live · 3 active · 2 hosts ─"), "{s}");
        assert!(s.contains("wren#1 ⚑ HIGH ledger totals lose cents"), "{s}");
    }

    #[test]
    fn large_run_frame_collapses_and_phases_at_a_tight_height() {
        let (wf, v) = (large_wf(), large_view());
        let firehose = running_units_firehose();
        let out = dashboard_frame(
            &v,
            &wf,
            &NavState::default(),
            &[],
            &firehose,
            now(),
            120,
            30,
        );
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        // Bounded: the whole run is 70+ structure rows, the frame is 30.
        assert_eq!(out.len(), 30, "{s}");
        assert!(out.iter().all(|l| l.width() <= 120), "{s}");
        // Settled steps fold to one summary row; the settled phase folds to
        // one phase row; the open phase keeps the frontier.
        assert!(s.contains("✓ s01 … s11 (+11 done)"), "{s}");
        assert!(s.contains("⟦ recon1 … join1 ⟧"), "{s}");
        assert!(!s.contains("s05") && !s.contains("├─ a"), "{s}");
        assert!(
            s.contains("⊞ hunt") && s.contains("52/86 ✓52 ◐6 ✗2 ○26"),
            "{s}"
        );
        assert!(s.contains("… +82 more"), "{s}");
        // What is still to come stays visible.
        assert!(s.contains("join2") && s.contains("report"), "{s}");
        // Following with nothing chosen: the stream idles, the firehose is
        // live and counts the six running units over two hosts.
        assert!(s.contains("─ stream · run ─"), "{s}");
        assert_eq!(s.matches("waiting for activity…").count(), 1, "{s}");
        assert!(s.contains("├─ live · 6 active · 2 hosts ─"), "{s}");
        assert!(
            s.contains("lynx#55 ⚑ HIGH token accepted after revoke"),
            "{s}"
        );
        assert!(rows_of(&out).last().unwrap().contains("Esc pause"), "{s}");
    }

    #[test]
    fn drilled_unit_frame_fills_the_stream_from_the_pinned_feed() {
        let wf = Workflow::parse(DRILL_WF).expect("parses");
        let mut v = RunView::default();
        v.workflow_name = "breakers".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        v.started_at = Some(now() - Duration::seconds(6 * 60 + 2));
        begin(
            &mut v,
            "preflight",
            StepKind::Linear,
            Some("scanner"),
            Some("heron#1"),
            None,
        );
        complete(&mut v, "preflight", 21_000);
        begin(&mut v, "hunt", StepKind::ForEach, None, None, None);
        for i in 0..8 {
            let status = match i {
                0..=3 => UnitStatus::Done,
                4 => UnitStatus::Failed,
                _ => UnitStatus::Running,
            };
            v.step_mut("hunt").units.insert(i, unit_of(i, status));
        }
        for i in 8..10 {
            v.step_mut("hunt")
                .units
                .insert(i, unit_of(i, UnitStatus::Queued));
        }
        v.step_mut("report");

        // hunt → its unit list → walk to unit 5 (otter#6, running) → drill.
        let mut nav = nav_on(&v, "hunt");
        nav.apply(NavKey::In, &v);
        for _ in 0..5 {
            nav.apply(NavKey::Down, &v);
        }
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Unit);

        let stream = transcript(
            "otter#6",
            vec![
                call("read_file", json!({"path": "svc/billing/ledger.rs"})),
                Tx::Thinking {
                    text: Some("amounts are summed as f64 before rounding".into()),
                    provider: "anthropic".into(),
                    model: "claude-opus-5-5".into(),
                    raw: json!({}),
                },
                call("grep", json!({"pattern": "as f64"})),
                finding("medium", "ledger totals lose cents"),
                call("read_file", json!({"path": "svc/billing/rounding.rs"})),
                Tx::AssistantMessage {
                    content: "Two call sites convert cents to f64 before summing.".into(),
                    thinking: None,
                },
            ],
        );
        let firehose = projected(&[
            ("heron#7", call("grep", json!({"pattern": "unwrap()"}))),
            (
                "otter#6",
                call("read_file", json!({"path": "svc/billing/ledger.rs"})),
            ),
            (
                "wren#8",
                call("read_file", json!({"path": "svc/search/index.rs"})),
            ),
            ("otter#6", call("grep", json!({"pattern": "as f64"}))),
            (
                "heron#7",
                call("read_file", json!({"path": "svc/auth/session.rs"})),
            ),
            ("otter#6", finding("medium", "ledger totals lose cents")),
        ]);
        let out = dashboard_frame(&v, &wf, &nav, &stream, &firehose, now(), 120, 34);
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        assert!(out.len() <= 34, "{s}");
        assert!(out.iter().all(|l| l.width() <= 120), "{s}");
        assert_eq!(selection_label(&v, &nav), "hunt › otter#6");
        // The stream names the drilled unit and shows ITS transcript, all of
        // it, oldest first (the whole feed fits the pane).
        assert!(s.contains("─ stream · hunt › otter#6 ─"), "{s}");
        let rows = rows_of(&out);
        let top = rows.iter().position(|r| r.contains("─ stream")).unwrap();
        for (i, want) in [
            "otter#6 ▸ read_file svc/billing/ledger.rs",
            "otter#6 ◇ thinking amounts are summed as f64 before rounding",
            "otter#6 ▸ grep as f64",
            "otter#6 ⚑ MEDIUM ledger totals lose cents",
            "otter#6 ▸ read_file svc/billing/rounding.rs",
            "otter#6 ▪ Two call sites convert cents to f64 before summing.",
        ]
        .into_iter()
        .enumerate()
        {
            assert!(rows[top + 1 + i].contains(want), "row {i}:\n{s}");
        }
        // The structure pane marks the drilled unit under its step.
        assert!(
            plain(row_with(&out, "svc-5 ")).starts_with("▸ │ ◐├─ svc-5"),
            "{s}"
        );
        // The firehose is the other units' merged feed, with its live count.
        assert!(s.contains("├─ live · 3 active · 2 hosts ─"), "{s}");
        assert!(s.contains("heron#7 ▸ grep unwrap()"), "{s}");
    }

    #[test]
    fn the_stream_title_names_the_drilled_path() {
        // hunt → unit 0 (otter#1): the breadcrumb below the run, joined.
        let mut v = RunView::default();
        v.crew = Some("mint-tundra".into());
        begin(&mut v, "hunt", StepKind::ForEach, None, None, None);
        for i in 0..3 {
            v.step_mut("hunt")
                .units
                .insert(i, unit_of(i, UnitStatus::Running));
        }
        let mut nav = NavState::default();
        nav.apply(NavKey::In, &v);
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Unit);
        assert_eq!(selection_label(&v, &nav), "hunt › otter#1");
        // Without a crew crumb the path still starts at the step, not the unit.
        v.crew = None;
        assert_eq!(selection_label(&v, &nav), "hunt › otter#1");
        // Wire text cannot reach the title.
        v.step_mut("hunt").units.get_mut(&0).unwrap().codename = Some("ot\x1b[2Jter#1".into());
        assert_eq!(selection_label(&v, &nav), "hunt › ot\u{FFFD}[2Jter#1");
    }

    #[test]
    fn the_firehose_rule_counts_the_distinct_hosts_of_the_live_work() {
        let rule = |v: &RunView, wf: &Workflow| {
            let out = dashboard_frame(v, wf, &NavState::default(), &[], &[], now(), 100, 28);
            plain(row_with(&out, "├─ live"))
                .trim_start_matches(|c| c != '├')
                .to_string()
        };
        let wf = wf();

        // A local run (no step names a host): the figure is omitted, not 0.
        let mut v = live();
        assert_eq!(active_hosts(&v), 0);
        assert!(
            rule(&v, &wf).starts_with("├─ live · 1 active ─"),
            "{}",
            rule(&v, &wf)
        );

        // One placed step: singular.
        v.step_mut("sweep").host = Some("mini".into());
        assert_eq!(active_hosts(&v), 1);
        assert!(
            rule(&v, &wf).starts_with("├─ live · 1 active · 1 host ─"),
            "{}",
            rule(&v, &wf)
        );

        // A blank host names nothing.
        v.step_mut("sweep").host = Some(String::new());
        assert_eq!(active_hosts(&v), 0);

        // A running fan-out counts the hosts of its RUNNING units only, each
        // host once however many units run on it.
        let fan = Workflow::parse(FANOUT_WF).expect("parses");
        let mut v = RunView::default();
        begin(
            &mut v,
            "hunt",
            StepKind::ForEach,
            None,
            None,
            Some("ignored"),
        );
        for (i, (status, host)) in [
            (UnitStatus::Running, Some("mini")),
            (UnitStatus::Running, Some("mini")),
            (UnitStatus::Running, Some("kuki")),
            (UnitStatus::Running, None),
            (UnitStatus::Done, Some("far")),
            (UnitStatus::Queued, Some("away")),
        ]
        .into_iter()
        .enumerate()
        {
            let mut unit = unit_of(i, status);
            unit.host = host.map(Into::into);
            v.step_mut("hunt").units.insert(i, unit);
        }
        assert_eq!(active_count(&v), 4);
        assert_eq!(active_hosts(&v), 2);
        assert!(
            rule(&v, &fan).starts_with("├─ live · 4 active · 2 hosts ─"),
            "{}",
            rule(&v, &fan)
        );

        // Settled work names no live host.
        complete(&mut v, "hunt", 1_000);
        assert_eq!((active_count(&v), active_hosts(&v)), (0, 0));
    }

    #[test]
    fn a_running_loop_frame_is_not_an_extra_active_agent() {
        // `loop:refine` stays Running for the whole loop but only frames its
        // members: the one running member is the one active agent.
        let mut v = RunView::default();
        begin(&mut v, "loop:refine", StepKind::Loop, None, None, None);
        begin(
            &mut v,
            "gen",
            StepKind::Linear,
            Some("generator"),
            None,
            Some("mini"),
        );
        complete(&mut v, "gen", 1_000);
        begin(
            &mut v,
            "critique",
            StepKind::Linear,
            Some("critic"),
            None,
            Some("kuki"),
        );
        assert_eq!(active_count(&v), 1);
        assert_eq!(active_hosts(&v), 1);
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
