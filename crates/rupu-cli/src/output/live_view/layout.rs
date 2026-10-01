//! The always-visible dashboard header and the key legend of the live
//! `workflow run` view, as pure functions from a [`RunView`] (plus a
//! [`NavState`]) to styled [`Line`]s — no terminal I/O.
//!
//! * [`dashboard`] — the title / progress / meter rows at the top of every
//!   frame.
//! * [`footer_line`] — the one-row, pane- and gate-aware key legend at the
//!   bottom.
//! * [`printable`] — the shared control-character scrub for text that came off
//!   the wire.
//!
//! `panes::dashboard_frame` composes these with the structure, stream and
//! firehose panes into the whole frame.
//!
//! Every datum is shown only when the run has actually produced it (no mock
//! zeros): cost needs a priced run, findings need at least one finding,
//! provider/model need an `AgentStarted`, coverage is omitted entirely until
//! `RunView` carries it.

use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::RunStatus;

use crate::output::live_view::nav::{NavState, Pane};
use crate::output::live_view::row::{truncate_to, Line};
use crate::output::palette::Status;
use crate::output::run_model::{fmt_hms, step_status, RunView, StepState, StepView};

/// Display width of the `step N/M` progress bar, in columns.
const BAR_WIDTH: usize = 24;

/// The dashboard rows: title, `step N/M` progress, and (only when the run has
/// produced any meter) an adaptive meter row. Two or three lines.
pub fn dashboard(view: &RunView, now: DateTime<Utc>) -> Vec<Line> {
    let mut rows = vec![title_row(view, now), progress_row(view)];
    if let Some(meters) = meter_row(view) {
        rows.push(meters);
    }
    rows
}

/// Palette status (glyph + colour) for the run as a whole.
fn run_status(status: RunStatus) -> Status {
    match status {
        RunStatus::Completed => Status::Complete,
        RunStatus::Failed | RunStatus::Rejected | RunStatus::Cancelled => Status::Failed,
        RunStatus::AwaitingApproval => Status::Awaiting,
        RunStatus::Pending | RunStatus::Paused => Status::Waiting,
        RunStatus::Running => Status::Working,
    }
}

/// `<workflow>  ● <crew>  <glyph> <status> · <elapsed>`
fn title_row(view: &RunView, now: DateTime<Utc>) -> Line {
    let st = run_status(view.status);
    let elapsed = fmt_hms(view.elapsed_ms(now).unwrap_or(0));
    let mut line = Line::new().strong(&view.workflow_name);
    if let Some(crew) = &view.crew {
        line = line.plain("  ").crew(crew, format!("● {crew}"));
    }
    line.plain("  ")
        .status(st, format!("{} {}", st.glyph(), view.status.as_str()))
        .dim(" · ")
        .plain(elapsed)
}

/// `step N/M  ████░░░░…  <active step>` — N counts completed steps.
fn progress_row(view: &RunView) -> Line {
    let total = view.steps.len();
    let done = view
        .steps
        .iter()
        .filter(|s| s.state == StepState::Complete)
        .count();
    // Integer round-half-up of done/total * BAR_WIDTH.
    let filled = if total == 0 {
        0
    } else {
        ((done * BAR_WIDTH * 2 + total) / (total * 2)).min(BAR_WIDTH)
    };

    let mut line = Line::new()
        .dim("step ")
        .strong(format!("{done}/{total}"))
        .plain("  ")
        .status(run_status(view.status), "█".repeat(filled))
        .dim("░".repeat(BAR_WIDTH - filled));

    let active = active_steps(view);
    if let Some(first) = active.first() {
        let st = step_status(first.state);
        line = line
            .plain("  ")
            .status(st, format!("{} {}", st.glyph(), first.step_id));
        if active.len() > 1 {
            line = line.dim(format!(" +{}", active.len() - 1));
        }
    }
    line
}

/// The steps currently in flight: every running step, or — when none is
/// running — the steps parked at an approval gate.
fn active_steps(view: &RunView) -> Vec<&StepView> {
    let by_state = |state: StepState| {
        view.steps
            .iter()
            .filter(|s| s.state == state)
            .collect::<Vec<_>>()
    };
    let running = by_state(StepState::Running);
    if running.is_empty() {
        by_state(StepState::AwaitingApproval)
    } else {
        running
    }
}

/// `⇡in ⇣out $cost · ⚑findings` — each group only when its datum exists.
/// `None` when the run has produced none of them.
fn meter_row(view: &RunView) -> Option<Line> {
    let mut groups: Vec<Line> = Vec::new();

    if let Some(u) = &view.usage {
        let mut g = Line::new()
            .meter(format!("⇡{}", u.input_tokens))
            .plain(" ")
            .meter(format!("⇣{}", u.output_tokens));
        // A partial cost total can exist while `priced` is false; never show
        // it as if it were the real spend.
        if let Some(cost) = u.cost_usd.filter(|_| u.priced) {
            g = g.plain(" ").meter(format!("${cost:.2}"));
        }
        groups.push(g);
    }

    let findings: usize = view.findings_by_severity.values().sum();
    if findings > 0 {
        groups.push(Line::new().danger(format!("⚑{findings}")));
    }

    if groups.is_empty() {
        return None;
    }
    let mut line = Line::new();
    for (i, g) in groups.into_iter().enumerate() {
        if i > 0 {
            line = line.dim(" · ");
        }
        line.segments.extend(g.segments);
    }
    Some(line)
}

/// Wire text for a single-line row: control characters (ESC, newlines, …)
/// become U+FFFD so they can neither reach the terminal nor break the row.
pub(crate) fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

/// The one-row key legend for the focused pane. A parked, focused approval
/// gate is *modal*: its approve / reject / findings keys replace everything
/// else, from any pane (the same `focused_gate` predicate the key dispatch
/// uses, so the legend never advertises a key that would do nothing). Away
/// from a gate the structure pane shows the move / drill / filter set and the
/// stream / firehose panes the scroll set — keys that are no-ops in the other
/// focus are never shown. Both non-gate sets carry a `tab pane` hint.
///
/// Hints carry a rank; a terminal too narrow for them all drops the
/// highest-ranked (least important) first, so `tab pane`, `Esc pause` and
/// `q quit` outlast the mode-specific keys.
pub(crate) fn footer_line(view: &RunView, nav: &NavState, w: usize) -> Line {
    const AT_GATE: &[(&str, u8)] = &[
        ("a approve", 1),
        ("r reject", 2),
        ("v findings", 3),
        ("Esc pause", 4),
        ("q quit", 0),
    ];
    const STRUCTURE: &[(&str, u8)] = &[
        ("↑↓ move", 3),
        ("enter drill", 4),
        ("← back", 5),
        ("/ filter", 6),
        ("tab pane", 2),
        ("Esc pause", 1),
        ("q quit", 0),
    ];
    const SCROLL: &[(&str, u8)] = &[
        ("↑↓ scroll", 4),
        ("PgUp/PgDn page", 5),
        ("tab pane", 2),
        ("Esc pause", 1),
        ("q quit", 0),
    ];
    // The gate predicate takes precedence over the pane: a parked gate's keys
    // act from any focus, so its legend shows from any focus.
    let hints: &[(&str, u8)] = if nav.focused_gate(view).is_some() {
        AT_GATE
    } else if nav.pane() == Pane::Structure {
        STRUCTURE
    } else {
        SCROLL
    };
    let mut hints = hints.to_vec();
    while legend(&hints).width() > w && hints.len() > 1 {
        let Some(least) = hints
            .iter()
            .enumerate()
            .max_by_key(|(_, (_, rank))| *rank)
            .map(|(i, _)| i)
        else {
            break;
        };
        hints.remove(least);
    }
    truncate_to(legend(&hints), w)
}

/// `key label · key label …`: the key plain, its label dim.
fn legend(hints: &[(&str, u8)]) -> Line {
    let mut line = Line::new();
    for (i, (hint, _)) in hints.iter().enumerate() {
        if i > 0 {
            line = line.dim(" · ");
        }
        match hint.split_once(' ') {
            Some((key, label)) => line = line.plain(key).dim(format!(" {label}")),
            None => line = line.plain(*hint),
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::row::render_plain;
    use crate::output::run_model::GateView;
    use chrono::{Duration, TimeZone, Utc};
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
    }

    fn start(v: &mut RunView, step: &str) {
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind: StepKind::Run,
            agent: None,
            host: None,
            codename: None,
        });
    }

    fn complete(v: &mut RunView, step: &str) {
        start(v, step);
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: step.into(),
            success: true,
            duration_ms: 18_000,
            host: None,
        });
    }

    /// Two completed steps + one running, priced usage, findings, 4m 12s in.
    fn live_view() -> RunView {
        let mut v = RunView::default();
        v.workflow_name = "assess-services".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        v.started_at = Some(now() - Duration::seconds(4 * 60 + 12));
        complete(&mut v, "preflight");
        complete(&mut v, "inventory");
        start(&mut v, "scan");
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 6_100_000,
            output_tokens: 420_000,
            total_tokens: 6_520_000,
            cost_usd: Some(18.40),
            priced: true,
            ..Default::default()
        });
        v.findings_by_severity.insert("high".into(), 2);
        v.findings_by_severity.insert("medium".into(), 3);
        v
    }

    #[test]
    fn dashboard_snapshot() {
        let rows = dashboard(&live_view(), now());
        insta::assert_snapshot!(render_plain(&rows));
    }

    #[test]
    fn cost_absent_when_unpriced_and_findings_absent_when_empty() {
        let mut v = live_view();
        // A partial cost total may be present while `priced` is false; it
        // must not be shown as if it were the real spend.
        v.usage = Some(rupu_cp::usage::UsageSummary {
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            cost_usd: Some(0.5),
            priced: false,
            ..Default::default()
        });
        v.findings_by_severity.clear();
        let s = render_plain(&dashboard(&v, now()));
        assert!(!s.contains('$'), "unpriced run must not print a cost:\n{s}");
        assert!(!s.contains('⚑'), "no findings => no findings meter:\n{s}");
        assert!(s.contains("⇡100") && s.contains("⇣50"), "tokens stay:\n{s}");
        assert!(!s.contains("coverage"), "coverage is never invented:\n{s}");
    }

    #[test]
    fn meter_row_omitted_when_run_has_produced_nothing() {
        let mut v = RunView::default();
        v.workflow_name = "fresh".into();
        v.status = RunStatus::Pending;
        let rows = dashboard(&v, now());
        // Title + progress only; no blank/zeroed meter row.
        assert_eq!(rows.len(), 2);
        let s = render_plain(&rows);
        assert!(s.contains("step 0/0"), "{s}");
        assert!(!s.contains('█'), "empty run has an empty bar:\n{s}");
        assert!(!s.contains('⇡') && !s.contains('⚑'), "{s}");
    }

    #[test]
    fn crew_segment_absent_until_known() {
        let mut v = live_view();
        v.crew = None;
        let s = render_plain(&dashboard(&v, now()));
        assert!(!s.contains("mint-tundra"), "{s}");
    }

    #[test]
    fn progress_bar_is_fixed_width_and_rounds() {
        // 1 of 3 complete => round(24/3) = 8 filled of 24.
        let mut v = RunView::default();
        v.status = RunStatus::Running;
        complete(&mut v, "a");
        start(&mut v, "b");
        start(&mut v, "c");
        let s = render_plain(&dashboard(&v, now()));
        let bar: String = s.chars().filter(|c| *c == '█' || *c == '░').collect();
        assert_eq!(bar.chars().count(), 24, "{s}");
        assert_eq!(bar.chars().filter(|c| *c == '█').count(), 8, "{s}");
        // Two running steps: first named, the rest summarised.
        assert!(s.contains("b +1"), "{s}");
    }

    #[test]
    fn completed_run_bar_is_full() {
        let mut v = RunView::default();
        v.status = RunStatus::Completed;
        complete(&mut v, "a");
        complete(&mut v, "b");
        let s = render_plain(&dashboard(&v, now()));
        assert_eq!(s.matches('█').count(), 24, "{s}");
        assert!(!s.contains('░'), "{s}");
    }

    // ---- footer legend ---------------------------------------------------

    /// A run parked at an approval gate `step`, with that gate in the awaiting
    /// set so it is actionable.
    fn gate_parked(step: &str) -> RunView {
        let mut v = RunView::default();
        v.status = RunStatus::AwaitingApproval;
        let g = v.step_mut(step);
        g.kind = StepKind::ApprovalGate;
        g.state = StepState::AwaitingApproval;
        v.gates = vec![GateView {
            step_id: step.into(),
            prompt: None,
            since: now(),
            expires_at: None,
        }];
        v
    }

    #[test]
    fn footer_is_pane_aware() {
        let v = RunView::default();
        let footer = |nav: &NavState, w: usize| render_plain(&[footer_line(&v, nav, w)]);

        // Structure focus: the move / drill / back / filter set, plus the
        // always-present tab-pane hint.
        let mut nav = NavState::default();
        assert_eq!(nav.pane(), Pane::Structure);
        assert_eq!(
            footer(&nav, 80),
            "↑↓ move · enter drill · ← back · / filter · tab pane · Esc pause · q quit"
        );

        // Stream / firehose focus: the move/drill/filter keys are no-ops
        // there, so the legend shows the scroll set instead.
        nav.cycle_pane(true);
        assert_eq!(nav.pane(), Pane::Stream);
        assert_eq!(
            footer(&nav, 80),
            "↑↓ scroll · PgUp/PgDn page · tab pane · Esc pause · q quit"
        );
        nav.cycle_pane(true);
        assert_eq!(nav.pane(), Pane::Firehose);
        assert_eq!(
            footer(&nav, 80),
            "↑↓ scroll · PgUp/PgDn page · tab pane · Esc pause · q quit"
        );
    }

    #[test]
    fn footer_narrows_mode_keys_first_keeping_tab_pause_quit() {
        let v = RunView::default();
        let footer = |w: usize| render_plain(&[footer_line(&v, &NavState::default(), w)]);
        // Wide: everything. Narrower: the mode-specific keys fold away before
        // tab / Esc / q, which survive down to the last column.
        assert_eq!(footer(30), "tab pane · Esc pause · q quit");
        assert_eq!(footer(20), "Esc pause · q quit");
        assert_eq!(footer(6), "q quit");
    }

    #[test]
    fn footer_at_a_gate_is_the_modal_legend_from_any_pane() {
        let v = gate_parked("approve");
        // Following a parked run auto-focuses its gate, so the keys show with
        // no navigation — and from whichever pane has focus.
        let mut nav = NavState::default();
        let at_gate = "a approve · r reject · v findings · Esc pause · q quit";
        for _ in 0..3 {
            assert_eq!(render_plain(&[footer_line(&v, &nav, 80)]), at_gate);
            nav.cycle_pane(true);
        }
        // A narrow gate footer drops the least important first; q quit stays.
        assert_eq!(
            render_plain(&[footer_line(&v, &NavState::default(), 24)]),
            "a approve · q quit"
        );
    }
}
