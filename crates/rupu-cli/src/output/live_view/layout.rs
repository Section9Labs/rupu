//! Zones of the redesigned live view, as pure functions from a [`RunView`]
//! (plus a [`NavState`]) to styled [`Line`]s — no terminal I/O.
//!
//! * [`dashboard`] — zone 1, the always-visible title / progress / meters.
//! * [`graph`] — zone 2, one row per step with codename, agent, provider/model
//!   and state.
//!
//! Every datum is shown only when the run has actually produced it (no mock
//! zeros): cost needs a priced run, findings need at least one finding,
//! provider/model need an `AgentStarted`, coverage is omitted entirely until
//! `RunView` carries it.

use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::{RunStatus, StepKind};

use crate::output::live_view::nav::{Depth, NavState};
use crate::output::live_view::row::Line;
use crate::output::palette::Status;
use crate::output::run_model::{
    fmt_hms, step_status, DispatchView, RunView, StepState, StepView, UnitStatus,
};

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

/// Leading marker column of a graph row (2 display columns).
const SELECT_MARK: &str = "▸ ";
const NO_MARK: &str = "  ";
/// Indent of a sub-agent row: its branch glyph sits under the step glyph.
const CHILD_INDENT: &str = "    ";
/// Dotted leader between a row's label and its right-hand state.
const LEADER: &str = " ···· ";

/// The graph rows: one per step in `view.steps` order —
/// `<▸|  ><glyph> <step_id> · <kind> · <codename> · <agent> · <provider/model> ···· <state>`.
/// Each part after the step id appears only when the run has produced it.
/// The operator-selected step is marked with `▸` and lists the sub-agents it
/// dispatched as indented `┣━`/`┗━` children.
pub fn graph(view: &RunView, nav: &NavState) -> Vec<Line> {
    let focus = focused_step_id(view, nav);
    let mut rows = Vec::new();
    for step in &view.steps {
        let marked = focus == Some(step.step_id.as_str());
        match step.kind {
            // Task 5: delegate ForEach/Parallel to fanout_block. Until then a
            // fan-out step is a single header row (kind + unit count).
            StepKind::ForEach | StepKind::Parallel => rows.push(step_row(step, marked)),
            _ => {
                rows.push(step_row(step, marked));
                if marked {
                    rows.extend(dispatch_rows(view, step));
                }
            }
        }
    }
    rows
}

/// The step the operator has *chosen*, if any. While the view is following
/// the newest activity at `Run` depth, `NavState`'s step cursor is just parked
/// on step 0 — that is not a selection, so nothing is marked. Once the
/// operator has moved the cursor or drilled in, the cursor step is the focus.
fn focused_step_id<'a>(view: &'a RunView, nav: &NavState) -> Option<&'a str> {
    let chosen = !nav.is_following() || nav.depth() != Depth::Run;
    nav.selected_step(view)
        .filter(|_| chosen)
        .map(|s| s.step_id.as_str())
}

fn step_row(step: &StepView, marked: bool) -> Line {
    let st = step_status(step.state);
    let mut line = if marked {
        Line::new().strong(SELECT_MARK)
    } else {
        Line::new().dim(NO_MARK)
    };
    line = line.status(st, st.glyph().to_string()).plain(" ");
    line = if marked {
        line.strong(&step.step_id)
    } else {
        line.plain(&step.step_id)
    };

    let mut parts: Vec<Line> = kind_parts(step)
        .into_iter()
        .map(|k| Line::new().dim(k))
        .collect();
    parts.extend(member_parts(
        step.codename.as_deref(),
        step.agent.as_deref(),
        step.provider.as_deref(),
        step.model.as_deref(),
    ));
    join_dot(line, parts, true)
        .dim(LEADER)
        .status(st, format!("{} {}", st.glyph(), state_text(step)))
}

/// Kind annotation parts for non-linear kinds (`gate`, `panel iter 2/5`, …).
fn kind_parts(step: &StepView) -> Vec<String> {
    match step.kind {
        StepKind::Linear | StepKind::Unknown => Vec::new(),
        StepKind::Run => vec!["run".to_string()],
        StepKind::ForEach => fan_out_parts("for_each", step),
        StepKind::Parallel => fan_out_parts("parallel", step),
        StepKind::Panel => vec![iter_label("panel", step.panel_round, step.panel_max)],
        StepKind::Loop => vec![iter_label("loop", step.loop_iteration, None)],
        StepKind::Branch => vec!["branch".to_string()],
        StepKind::Split => vec!["split".to_string()],
        StepKind::Join => vec!["join".to_string()],
        StepKind::Action => vec!["action".to_string()],
        StepKind::ApprovalGate => vec!["gate".to_string()],
    }
}

/// `for_each` / `parallel`, plus `N units` once any unit is known.
fn fan_out_parts(kind: &str, step: &StepView) -> Vec<String> {
    let mut parts = vec![kind.to_string()];
    let n = step.units.len();
    if n > 0 {
        let plural = if n == 1 { "" } else { "s" };
        parts.push(format!("{n} unit{plural}"));
    }
    parts
}

/// `<kind> iter r/max`, `<kind> iter r`, or bare `<kind>` when no counter yet.
fn iter_label(kind: &str, round: Option<u32>, max: Option<u32>) -> String {
    match (round, max) {
        (Some(r), Some(m)) => format!("{kind} iter {r}/{m}"),
        (Some(r), None) => format!("{kind} iter {r}"),
        (None, _) => kind.to_string(),
    }
}

/// The right-hand state text: the duration of a completed step that has one,
/// otherwise the state word.
fn state_text(step: &StepView) -> String {
    match (step.state, step.duration_ms) {
        (StepState::Complete, Some(ms)) => fmt_hms(ms),
        (StepState::Pending, _) => "pending".to_string(),
        (StepState::Running, _) => "running".to_string(),
        (StepState::AwaitingApproval, _) => "awaiting approval".to_string(),
        (StepState::Complete, None) => "complete".to_string(),
        (StepState::Failed, _) => "failed".to_string(),
        (StepState::Skipped, _) => "skipped".to_string(),
        (StepState::Paused, _) => "paused".to_string(),
    }
}

/// Sub-agents dispatched while `step` was active, as indented tree children.
fn dispatch_rows(view: &RunView, step: &StepView) -> Vec<Line> {
    let children: Vec<&DispatchView> = view
        .dispatches
        .values()
        .filter(|d| d.parent_step_id.as_deref() == Some(step.step_id.as_str()))
        .collect();
    let last = children.len().saturating_sub(1);
    children
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let branch = if i == last { "┗━ " } else { "┣━ " };
            let st = unit_status(d.status);
            let line = Line::new()
                .dim(CHILD_INDENT)
                .dim(branch)
                .status(st, st.glyph().to_string())
                .plain(" ");
            let parts = member_parts(
                d.codename.as_deref(),
                d.agent.as_deref(),
                d.provider.as_deref(),
                d.model.as_deref(),
            );
            if parts.is_empty() {
                line.dim("sub-agent")
            } else {
                join_dot(line, parts, false)
            }
        })
        .collect()
}

/// Palette status (glyph + colour) for a fan-out unit / sub-agent.
fn unit_status(status: UnitStatus) -> Status {
    match status {
        UnitStatus::Queued => Status::Waiting,
        UnitStatus::Running => Status::Working,
        UnitStatus::Done => Status::Complete,
        UnitStatus::Failed => Status::Failed,
    }
}

/// `codename · agent · provider/model`, each present part as its own [`Line`].
fn member_parts(
    codename: Option<&str>,
    agent: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Vec<Line> {
    let mut parts = Vec::new();
    if let Some(c) = codename.and_then(codename_line) {
        parts.push(c);
    }
    if let Some(a) = agent {
        parts.push(Line::new().plain(a));
    }
    match (provider, model) {
        (Some(p), Some(m)) => parts.push(Line::new().dim(format!("{p}/{m}"))),
        (Some(x), None) | (None, Some(x)) => parts.push(Line::new().dim(x)),
        (None, None) => {}
    }
    parts
}

/// Append `parts` to `line`, separated by a dim ` · ` (also before the first
/// part when `lead` is set).
fn join_dot(mut line: Line, parts: Vec<Line>, lead: bool) -> Line {
    for (i, part) in parts.into_iter().enumerate() {
        if lead || i > 0 {
            line = line.dim(" · ");
        }
        line.segments.extend(part.segments);
    }
    line
}

/// A member codename as a row part: the crew prefix is dropped (the dashboard
/// carries the crew) and each `>`-chained segment's role word is emitted as
/// `Style::Role`, with its `#n` / `.attempt` suffix plain. `None` for a value
/// that is not codename-shaped — this text comes off the wire, so anything
/// outside `[A-Za-z0-9#.>]` (control characters included) is never echoed.
fn codename_line(codename: &str) -> Option<Line> {
    let leaf = codename.split_once('/').map_or(codename, |(_, l)| l);
    let shaped = !leaf.is_empty()
        && leaf
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '#' | '.' | '>'));
    if !shaped {
        return None;
    }
    let mut line = Line::new();
    for (i, seg) in leaf.split('>').enumerate() {
        let (role, suffix) = seg.split_at(seg.find(['#', '.']).unwrap_or(seg.len()));
        if role.is_empty() {
            return None;
        }
        if i > 0 {
            line = line.dim(">");
        }
        line = line.role(role, role);
        if !suffix.is_empty() {
            line = line.plain(suffix);
        }
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::nav::NavKey;
    use crate::output::live_view::row::{render_plain, Style};
    use crate::output::run_model::RunView;
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

    // ---- graph zone ------------------------------------------------------

    fn start_as(
        v: &mut RunView,
        step: &str,
        kind: StepKind,
        agent: Option<&str>,
        codename: Option<&str>,
    ) {
        v.apply(&Event::StepStarted {
            run_id: "r".into(),
            step_id: step.into(),
            kind,
            agent: agent.map(Into::into),
            host: None,
            codename: codename.map(Into::into),
        });
    }

    fn agent_started(v: &mut RunView, step: &str, agent: &str, provider: &str, model: &str) {
        v.apply(&Event::AgentStarted {
            run_id: "r".into(),
            step_id: step.into(),
            unit_index: None,
            codename: None,
            agent: agent.into(),
            provider: Some(provider.into()),
            model: Some(model.into()),
            agent_run_id: "ar".into(),
            transcript_path: "t".into(),
        });
    }

    fn dispatch(v: &mut RunView, id: &str, agent: &str, codename: &str, model: &str, done: bool) {
        v.apply(&Event::DispatchStarted {
            run_id: "r".into(),
            sub_run_id: id.into(),
            agent: Some(agent.into()),
            transcript_path: "t".into(),
            codename: Some(codename.into()),
            provider: Some("anthropic".into()),
            model: Some(model.into()),
        });
        if done {
            v.apply(&Event::DispatchCompleted {
                run_id: "r".into(),
                sub_run_id: id.into(),
                success: true,
                tokens_in: 10,
                tokens_out: 5,
            });
        }
    }

    /// (a) completed linear step with codename + provider/model + duration,
    /// (b) a running approval gate, (c) a panel on round 2/5, (d) a running
    /// step that has dispatched two sub-agents (one done, one running).
    fn graph_view() -> RunView {
        let mut v = RunView::default();
        v.status = RunStatus::Running;
        start_as(
            &mut v,
            "report",
            StepKind::Linear,
            Some("reporter"),
            Some("heron#1"),
        );
        agent_started(&mut v, "report", "reporter", "anthropic", "claude-opus-5-5");
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: "report".into(),
            success: true,
            duration_ms: 18_000,
            host: None,
        });
        start_as(&mut v, "approve", StepKind::ApprovalGate, None, None);
        start_as(&mut v, "review", StepKind::Panel, None, None);
        v.apply(&Event::PanelRound {
            run_id: "r".into(),
            step_id: "review".into(),
            round: 2,
            max_iterations: 5,
            max_severity_remaining: None,
        });
        start_as(
            &mut v,
            "scan",
            StepKind::Linear,
            Some("scanner"),
            Some("otter#2"),
        );
        agent_started(&mut v, "scan", "scanner", "openai", "gpt-5");
        dispatch(
            &mut v,
            "sub_1",
            "triage",
            "wren#1",
            "claude-haiku-4-5",
            true,
        );
        dispatch(
            &mut v,
            "sub_2",
            "prober",
            "lynx#3",
            "claude-sonnet-5-5",
            false,
        );
        v
    }

    /// A nav that has manually selected step `idx` (Down x idx).
    fn nav_at(view: &RunView, idx: usize) -> NavState {
        let mut nav = NavState::default();
        for _ in 0..idx {
            nav.apply(NavKey::Down, view);
        }
        nav
    }

    #[test]
    fn graph_snapshot() {
        let v = graph_view();
        let rows = graph(&v, &nav_at(&v, 3));
        insta::assert_snapshot!(render_plain(&rows));
    }

    #[test]
    fn selected_step_is_marked_and_only_it() {
        let v = graph_view();
        let rows = graph(&v, &nav_at(&v, 3));
        let plain: Vec<String> = rows
            .iter()
            .map(|l| render_plain(std::slice::from_ref(l)))
            .collect();
        let marked: Vec<&String> = plain.iter().filter(|l| l.starts_with('▸')).collect();
        assert_eq!(marked.len(), 1, "{plain:#?}");
        assert!(marked[0].starts_with("▸ ◐ scan"), "{plain:#?}");
        // The marker is Strong; the unmarked leader is Dim.
        let lead_of = |i: usize| rows[i].segments[0].style.clone();
        assert_eq!(lead_of(0), Style::Dim);
        assert_eq!(
            rows.iter()
                .position(|l| render_plain(std::slice::from_ref(l)).starts_with('▸'))
                .map(lead_of),
            Some(Style::Strong)
        );
    }

    #[test]
    fn following_at_run_depth_marks_nothing_and_hides_sub_agents() {
        // A following nav's cursor sits on step 0 only as an artifact; it is
        // not a selection, so no marker and no children are drawn.
        let v = graph_view();
        let s = render_plain(&graph(&v, &NavState::default()));
        assert!(!s.contains('▸'), "{s}");
        assert!(!s.contains("┣━") && !s.contains("┗━"), "{s}");
    }

    #[test]
    fn sub_agent_rows_only_under_the_selected_step() {
        let v = graph_view();
        // Select `report` (Down x3 then Up x3): it owns no dispatches.
        let mut nav = nav_at(&v, 3);
        for _ in 0..3 {
            nav.apply(NavKey::Up, &v);
        }
        let s = render_plain(&graph(&v, &nav));
        assert!(s.starts_with("▸ ✓ report"), "{s}");
        assert!(!s.contains("wren#1"), "dispatches belong to `scan`:\n{s}");

        // Selecting `scan` shows both children, the last with the closing glyph.
        let s = render_plain(&graph(&v, &nav_at(&v, 3)));
        assert!(
            s.contains("┣━ ✓ wren#1 · triage · anthropic/claude-haiku-4-5"),
            "{s}"
        );
        assert!(
            s.contains("┗━ ◐ lynx#3 · prober · anthropic/claude-sonnet-5-5"),
            "{s}"
        );
    }

    #[test]
    fn kind_annotations_and_state_words() {
        let mut v = RunView::default();
        let mut add = |id: &str, kind: StepKind, state: StepState| {
            let s = v.step_mut(id);
            s.kind = kind;
            s.state = state;
        };
        add("lp", StepKind::Loop, StepState::Running);
        add("act", StepKind::Action, StepState::Complete);
        add("br", StepKind::Branch, StepState::Skipped);
        add("sp", StepKind::Split, StepState::Pending);
        add("jn", StepKind::Join, StepState::Failed);
        add("gate", StepKind::ApprovalGate, StepState::AwaitingApproval);
        add("cmd", StepKind::Run, StepState::Paused);
        add("plain", StepKind::Linear, StepState::Running);
        v.step_mut("lp").loop_iteration = Some(3);

        let s = render_plain(&graph(&v, &NavState::default()));
        let expect = [
            "◐ lp · loop iter 3 ···· ◐ running",
            "✓ act · action ···· ✓ complete",
            "⊘ br · branch ···· ⊘ skipped",
            "○ sp · split ···· ○ pending",
            "✗ jn · join ···· ✗ failed",
            "⏸ gate · gate ···· ⏸ awaiting approval",
            "○ cmd · run ···· ○ paused",
            "◐ plain ···· ◐ running",
        ];
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), expect.len(), "{s}");
        for (line, want) in lines.iter().zip(expect) {
            assert_eq!(line.trim_start(), want, "{s}");
        }
    }

    #[test]
    fn provider_model_and_duration_only_when_known() {
        let mut v = RunView::default();
        start_as(
            &mut v,
            "a",
            StepKind::Linear,
            Some("reporter"),
            Some("heron#1"),
        );
        // No AgentStarted (no provider/model) and no StepCompleted (no duration).
        let s = render_plain(&graph(&v, &NavState::default()));
        assert_eq!(
            s.trim_start(),
            "◐ a · heron#1 · reporter ···· ◐ running",
            "{s}"
        );
        assert!(!s.contains('/'), "no invented provider/model:\n{s}");
    }

    #[test]
    fn codename_is_leaf_with_role_style_and_hostile_values_are_dropped() {
        let mut v = RunView::default();
        start_as(
            &mut v,
            "a",
            StepKind::Linear,
            None,
            Some("mint-tundra/heron#4>lynx#3"),
        );
        start_as(
            &mut v,
            "b",
            StepKind::Linear,
            None,
            Some("x\u{1b}[31mheron#1"),
        );
        let rows = graph(&v, &NavState::default());
        let a = render_plain(&rows[..1]);
        assert!(
            a.contains("· heron#4>lynx#3"),
            "crew prefix is dropped:\n{a}"
        );
        assert!(!a.contains("mint-tundra"), "{a}");
        let roles: Vec<&Style> = rows[0]
            .segments
            .iter()
            .map(|s| &s.style)
            .filter(|s| matches!(s, Style::Role(_)))
            .collect();
        assert_eq!(
            roles,
            vec![&Style::Role("heron".into()), &Style::Role("lynx".into())]
        );
        // A codename carrying control characters never reaches the terminal.
        let b = render_plain(&rows[1..]);
        assert!(!b.contains('\u{1b}') && !b.contains("heron"), "{b:?}");
    }

    #[test]
    fn fan_out_steps_render_a_single_header_line() {
        // Until `fanout_block` lands, for_each / parallel are one header row
        // (with the unit count when known), even when selected.
        let mut v = RunView::default();
        start_as(&mut v, "hunt", StepKind::ForEach, None, None);
        for i in 0..3 {
            v.apply(&Event::UnitStarted {
                run_id: "r".into(),
                step_id: "hunt".into(),
                index: i,
                unit_key: format!("svc-{i}"),
                agent: None,
                transcript_path: "t".into(),
                host: None,
                codename: None,
            });
        }
        start_as(&mut v, "fan", StepKind::Parallel, None, None);
        dispatch(
            &mut v,
            "sub_1",
            "triage",
            "wren#1",
            "claude-haiku-4-5",
            false,
        );
        let rows = graph(&v, &nav_at(&v, 1));
        let s = render_plain(&rows);
        assert_eq!(rows.len(), 2, "{s}");
        assert!(s.contains("◐ hunt · for_each · 3 units"), "{s}");
        assert!(s.contains("▸ ◐ fan · parallel"), "{s}");
        assert!(!s.contains("wren#1"), "{s}");
    }
}
