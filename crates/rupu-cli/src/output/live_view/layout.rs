//! Zones of the redesigned live view, as pure functions from a [`RunView`]
//! (plus a [`NavState`]) to styled [`Line`]s — no terminal I/O.
//!
//! * [`dashboard`] — zone 1, the always-visible title / progress / meters.
//! * [`graph`] — zone 2, one row per step with codename, agent, provider/model
//!   and state.
//! * [`fanout_block`] — a `for_each`/`parallel` step as a status density bar +
//!   a few live movers (collapsed) or the filtered unit list (drilled); `graph`
//!   delegates every fan-out step to it.
//! * [`live_layout`] — the whole frame: those zones plus a breadcrumb, a feed
//!   and a key legend, with the graph adaptively collapsed so the active
//!   frontier and the feed never scroll off.
//!
//! Every datum is shown only when the run has actually produced it (no mock
//! zeros): cost needs a priced run, findings need at least one finding,
//! provider/model need an `AgentStarted`, coverage is omitted entirely until
//! `RunView` carries it.

use std::ops::Range;

use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::{RunStatus, StepKind};
use unicode_width::UnicodeWidthStr;

use crate::output::live_view::nav::{Depth, NavState, UnitFilter};
use crate::output::live_view::row::{truncate_to, Line};
use crate::output::palette::Status;
use crate::output::run_model::{
    fmt_hms, step_status, DispatchView, RunView, StepState, StepView, UnitStatus, UnitView,
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
    view.steps
        .iter()
        .flat_map(|step| step_block(view, step, nav, focus == Some(step.step_id.as_str())))
        .collect()
}

/// One step's rows: a [`fanout_block`] for a fan-out step, otherwise its row
/// (plus, when `marked`, the sub-agents it dispatched).
fn step_block(view: &RunView, step: &StepView, nav: &NavState, marked: bool) -> Vec<Line> {
    if is_fan_out(step) {
        return fanout_block(step, nav, marked);
    }
    let mut rows = vec![step_row(step, marked)];
    if marked {
        rows.extend(dispatch_rows(view, step));
    }
    rows
}

/// Whether `step` renders as a [`fanout_block`]: a `for_each` / `parallel`
/// step, or a step that has actually produced fan-out units. The units test
/// matters because a `run:` step that also carries `for_each:` stays
/// `StepKind::Run` yet fans out. It is limited to the kinds that have no row
/// label of their own: a panel's panelists (and a loop's body) also arrive as
/// units, and those steps keep their `panel iter r/max` / `loop iter n` row.
fn is_fan_out(step: &StepView) -> bool {
    match step.kind {
        StepKind::ForEach | StepKind::Parallel => true,
        StepKind::Run | StepKind::Linear | StepKind::Unknown => !step.units.is_empty(),
        StepKind::Panel
        | StepKind::Loop
        | StepKind::Branch
        | StepKind::Split
        | StepKind::Join
        | StepKind::Action
        | StepKind::ApprovalGate => false,
    }
}

/// The step the operator has *chosen*, if any. While the view is following
/// the newest activity at `Run` depth, `NavState`'s step cursor is just parked
/// on step 0 — that is not a selection, so nothing is marked. Once the
/// operator has moved the cursor or drilled in, the cursor step is the focus.
fn focused_step_id<'a>(view: &'a RunView, nav: &NavState) -> Option<&'a str> {
    chosen_step(view, nav).map(|s| s.step_id.as_str())
}

/// The step the operator has chosen (see [`focused_step_id`]) — what the
/// graph marks `▸` and what the layout never collapses. While following, a
/// parked run's first gate stands in for a choice ([`NavState::gate_step`]),
/// so the marker shows which gate `a` / `r` would act on.
fn chosen_step<'a>(view: &'a RunView, nav: &NavState) -> Option<&'a StepView> {
    let chosen = !nav.is_following() || nav.depth() != Depth::Run;
    nav.selected_step(view)
        .filter(|_| chosen)
        .or_else(|| nav.gate_step(view))
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

/// Display width of a fan-out density bar, in columns.
const DENSITY_WIDTH: usize = 20;
/// How many live movers a collapsed fan-out block lists.
const MOVERS: usize = 4;
/// Rows a fan-out block always opens with: its header and density row.
const FANOUT_HEAD: usize = 2;
/// Tree branch glyphs of a fan-out unit row.
const BRANCH_MID: &str = "┣━ ";
const BRANCH_LAST: &str = "┗━ ";

/// A fan-out step (`for_each` / `parallel`, or a `run:` step with
/// `for_each:`) as rows:
///
/// ```text
/// ▸ ◐ hunt · for_each · 86 units
///     ▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░ 52/86 ✓52 ◐6 ✗2 ○26
///     ┣━ ◐ otter#57 · svc-56 · anthropic/claude-opus-5-5 ···· ◐ running
///     ┗━ … +82 more · [enter] expand
/// ```
///
/// *Collapsed* (the step is not `selected`, or the view is still at `Run`
/// depth): the header, the density row, and up to [`MOVERS`] live movers.
/// *Expanded* (`selected` and drilled to `Step` or deeper): the header (tagged
/// with the active unit filter), the density row — always the whole step's
/// counts — and every unit admitted by `nav.filter()`, the cursor unit
/// marked `▸`. Every number comes from [`StepView::unit_counts`].
pub fn fanout_block(step: &StepView, nav: &NavState, selected: bool) -> Vec<Line> {
    let expanded = selected && nav.depth() != Depth::Run;
    let mut rows = vec![
        fanout_header(step, nav, selected, expanded),
        density_row(step),
    ];
    debug_assert_eq!(rows.len(), FANOUT_HEAD);
    if expanded {
        rows.extend(expanded_rows(step, nav));
    } else {
        rows.extend(mover_rows(step));
    }
    rows
}

/// `<▸|  ><glyph> <step_id> · <for_each|parallel> · <N units> [· <filter>]`,
/// plus the duration of a completed step.
fn fanout_header(step: &StepView, nav: &NavState, selected: bool, expanded: bool) -> Line {
    let st = step_status(step.state);
    let mut line = if selected {
        Line::new().strong(SELECT_MARK)
    } else {
        Line::new().dim(NO_MARK)
    };
    line = line.status(st, st.glyph().to_string()).plain(" ");
    line = if selected {
        line.strong(&step.step_id)
    } else {
        line.plain(&step.step_id)
    };

    // A `run:` step with `for_each:` (and any other unit-carrying step that
    // is not a `parallel`) is a `for_each` for display purposes.
    let label = if step.kind == StepKind::Parallel {
        "parallel"
    } else {
        "for_each"
    };
    let mut parts: Vec<Line> = fan_out_parts(label, step)
        .into_iter()
        .map(|k| Line::new().dim(k))
        .collect();
    let filter = filter_label(nav.filter()).filter(|_| expanded);
    if let Some(f) = filter {
        parts.push(Line::new().dim(f));
    }
    let line = join_dot(line, parts, true);

    match (step.state, step.duration_ms) {
        (StepState::Complete, Some(ms)) => line
            .dim(LEADER)
            .status(st, format!("{} {}", st.glyph(), fmt_hms(ms))),
        _ => line,
    }
}

/// The active unit filter as header text; `None` for `All`.
fn filter_label(filter: UnitFilter) -> Option<&'static str> {
    match filter {
        UnitFilter::All => None,
        UnitFilter::Running => Some("running"),
        UnitFilter::Failed => Some("failed"),
        UnitFilter::Done => Some("done"),
    }
}

/// `<bar> <done>/<total> ✓<done> ◐<running> ✗<failed> ○<queued>` — a status
/// count appears only when non-zero; all of it from `unit_counts()`.
///
/// Plan 3: per-unit spend. Units carry no per-unit tokens or cost yet, so the
/// row has no `⇡tokens $cost` tail rather than a made-up one.
fn density_row(step: &StepView) -> Line {
    let c = step.unit_counts();
    let filled = density_fill(c.done, c.total);
    let mut line = Line::new().dim(CHILD_INDENT);
    // Only non-empty runs: a zero-width styled segment would still wrap
    // nothing in colour codes once Plan 3 renders it.
    if filled > 0 {
        line = line.good("▓".repeat(filled));
    }
    if filled < DENSITY_WIDTH {
        line = line.dim("░".repeat(DENSITY_WIDTH - filled));
    }
    line = line.plain(" ").strong(format!("{}/{}", c.done, c.total));
    for (st, n) in [
        (Status::Complete, c.done),
        (Status::Working, c.running),
        (Status::Failed, c.failed),
        (Status::Waiting, c.queued),
    ] {
        if n > 0 {
            line = line.plain(" ").status(st, format!("{}{n}", st.glyph()));
        }
    }
    line
}

/// Filled cells of the density bar: `done / total` rounded half-up, but never
/// a full bar until every unit is done nor an empty one once any is.
/// `total == 0` is an empty bar (no division).
fn density_fill(done: usize, total: usize) -> usize {
    if total == 0 || done == 0 {
        0
    } else if done >= total {
        DENSITY_WIDTH
    } else {
        ((done * DENSITY_WIDTH * 2 + total) / (total * 2)).clamp(1, DENSITY_WIDTH - 1)
    }
}

/// Collapsed body: the live movers, then `… +K more` for the rest.
fn mover_rows(step: &StepView) -> Vec<Line> {
    let movers = live_movers(step);
    let hidden = step.units.len().saturating_sub(movers.len());
    let last = movers.len().saturating_sub(1);
    let mut rows: Vec<Line> = movers
        .iter()
        .enumerate()
        .map(|(i, u)| unit_row(u, hidden == 0 && i == last, false))
        .collect();
    if hidden > 0 {
        rows.push(
            Line::new()
                .dim(NO_MARK)
                .dim(NO_MARK)
                .dim(BRANCH_LAST)
                .dim(format!("… +{hidden} more · [enter] expand")),
        );
    }
    rows
}

/// Up to [`MOVERS`] units to surface while the step is collapsed: the newest
/// running units (highest indices — units start in index order), in index
/// order, then — when fewer than that are running — the highest-index of the
/// rest.
///
/// Plan 3: most-recently-*active*. `UnitView` carries no timestamps, so index
/// order stands in for recency.
fn live_movers(step: &StepView) -> Vec<&UnitView> {
    let running: Vec<&UnitView> = step
        .units
        .values()
        .filter(|u| u.status == UnitStatus::Running)
        .collect();
    let skip = running.len().saturating_sub(MOVERS);
    let mut movers: Vec<&UnitView> = running.into_iter().skip(skip).collect();
    let need = MOVERS.saturating_sub(movers.len());
    let mut fill: Vec<&UnitView> = step
        .units
        .values()
        .rev()
        .filter(|u| u.status != UnitStatus::Running)
        .take(need)
        .collect();
    fill.reverse();
    movers.extend(fill);
    movers
}

/// Expanded body: every unit the nav filter admits (or a note that none do),
/// the cursor unit marked.
fn expanded_rows(step: &StepView, nav: &NavState) -> Vec<Line> {
    let units = nav.filtered_units(step);
    if units.is_empty() {
        let note = match filter_label(nav.filter()) {
            Some(f) => format!("no {f} units"),
            None => "no units".to_string(),
        };
        return vec![Line::new()
            .dim(NO_MARK)
            .dim(NO_MARK)
            .dim(BRANCH_LAST)
            .dim(note)];
    }
    let cursor = nav.selected_unit_in(step).map(|u| u.index);
    let last = units.len() - 1;
    units
        .iter()
        .enumerate()
        .map(|(i, u)| unit_row(u, i == last, cursor == Some(u.index)))
        .collect()
}

/// `<mark><branch><glyph> <codename> · <unit_key> · <provider/model> ···· <state>`.
/// The mark column lines up with the step glyph; absent parts are omitted
/// (no codename before `AgentStarted`, no provider/model until it has run).
fn unit_row(unit: &UnitView, closes: bool, marked: bool) -> Line {
    let st = unit_status(unit.status);
    let mut line = Line::new().dim(NO_MARK);
    line = if marked {
        line.strong(SELECT_MARK)
    } else {
        line.dim(NO_MARK)
    };
    let branch = if closes { BRANCH_LAST } else { BRANCH_MID };
    line = line
        .dim(branch)
        .status(st, st.glyph().to_string())
        .plain(" ");
    let key = if unit.unit_key.is_empty() {
        format!("unit {}", unit.index)
    } else {
        unit.unit_key.clone()
    };
    // The unit key takes the agent slot: codename · key · provider/model.
    let parts = member_parts(
        unit.codename.as_deref(),
        Some(&key),
        unit.provider.as_deref(),
        unit.model.as_deref(),
    );
    join_dot(line, parts, false).dim(LEADER).status(
        st,
        format!("{} {}", st.glyph(), unit_state_text(unit.status)),
    )
}

fn unit_state_text(status: UnitStatus) -> &'static str {
    match status {
        UnitStatus::Queued => "queued",
        UnitStatus::Running => "running",
        UnitStatus::Done => "done",
        UnitStatus::Failed => "failed",
    }
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
            let branch = if i == last { BRANCH_LAST } else { BRANCH_MID };
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
    // An empty string is "not produced" (it would print as a bare ` · `), and
    // all three come off the wire, so control characters never get echoed.
    let clean = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(printable);
    let (agent, provider, model) = (clean(agent), clean(provider), clean(model));
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

/// Wire text for a single-line row: control characters (ESC, newlines, …)
/// become U+FFFD so they can neither reach the terminal nor break the row.
pub(crate) fn printable(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
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

// ---- live_layout: compose + adaptive bounding ---------------------------------

/// Rows the feed is guaranteed, when it has that many lines. The graph yields
/// to this floor — never the reverse.
const FEED_MIN: usize = 4;
/// A collapsed run names its steps up to this many; longer runs read
/// `first … last`.
const SUMMARY_NAMES: usize = 3;
/// Columns of label a clipped row keeps before it gives up protecting its
/// right-hand state (so a sliver of label is never traded for a state word).
const MIN_LABEL: usize = 8;

/// The whole live-view frame, top to bottom: dashboard, a breadcrumb (when
/// drilled), the graph, the feed, and a one-row key legend. At most `h` rows,
/// every row at most `w` columns, no terminal I/O.
///
/// The graph is bounded to whatever the other zones leave, in this order, and
/// **the active frontier (every running / awaiting-approval step) and the
/// feed's floor are never sacrificed to it**:
///
/// 1. settled steps (complete / skipped, not the operator's selection) fold
///    into `✓ first … last  (+N done)` summary rows, oldest first, only as
///    far as needed;
/// 2. pending steps then fold the same way, latest first;
/// 3. fan-out blocks are trimmed largest first: a drilled block keeps its
///    header + density row plus a window around the unit cursor; a collapsed
///    block trims to header + density row only (the density row already
///    carries the exact counts, so no misleading hidden-row marker is shown);
/// 4. as a last resort the rows are windowed around the first frontier row.
///
/// The feed gets `min(feed.len(), FEED_MIN)` rows guaranteed and every row
/// the graph leaves spare (its newest lines). The breadcrumb yields when it
/// would starve the graph. Below the floor (the dashboard, the footer, the
/// feed's floor and one graph row) the frame degrades by priority — footer,
/// title, frontier row, feed, rest of the dashboard; that range is a cramped
/// terminal, not a layout contract.
///
/// `nav` should have been [`NavState::sync`]ed against `view` this tick; a
/// stale one is clamped, never a panic.
pub fn live_layout(
    view: &RunView,
    nav: &NavState,
    feed: &[Line],
    now: DateTime<Utc>,
    w: usize,
    h: usize,
) -> Vec<Line> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let dash = dashboard(view, now);
    let feed_min = feed.len().min(FEED_MIN);
    let floor = dash.len() + 1 + feed_min + 1;
    if h < floor {
        return cramped(view, nav, &dash, feed, w, h);
    }

    let crumb = (nav.depth() != Depth::Run && h > floor)
        .then(|| crumb_line(&nav.breadcrumb(view), w))
        .flatten();
    let fixed = dash.len() + usize::from(crumb.is_some()) + 1;
    // `h >= floor (+1 with a crumb)`, so both subtractions are in range and
    // the graph has at least one row.
    let graph_rows = fit_graph(view, nav, h - fixed - feed_min);
    let feed_room = h - fixed - graph_rows.len();
    let skip = feed.len().saturating_sub(feed_room);

    let mut out = Vec::with_capacity(h);
    out.extend(dash.into_iter().map(|l| truncate_to(l, w)));
    out.extend(crumb);
    out.extend(graph_rows.into_iter().map(|l| clip_row(l, w)));
    out.extend(feed[skip..].iter().cloned().map(|l| truncate_to(l, w)));
    out.push(footer_line(view, nav, w));
    out
}

/// A frame for a terminal below the layout floor: rows are granted by
/// priority — footer, dashboard title, one frontier row, the feed's floor,
/// the rest of the dashboard — and emitted in screen order.
fn cramped(
    view: &RunView,
    nav: &NavState,
    dash: &[Line],
    feed: &[Line],
    w: usize,
    h: usize,
) -> Vec<Line> {
    let mut room = h;
    let mut take = |want: usize| {
        let n = want.min(room);
        room -= n;
        n
    };
    let footer_n = take(1);
    let title_n = take(1);
    let graph_n = take(1);
    let feed_n = take(feed.len().min(FEED_MIN));
    let dash_n = (title_n + take(dash.len().saturating_sub(1))).min(dash.len());

    let mut out: Vec<Line> = dash[..dash_n]
        .iter()
        .cloned()
        .map(|l| truncate_to(l, w))
        .collect();
    out.extend(
        fit_graph(view, nav, graph_n)
            .into_iter()
            .map(|l| clip_row(l, w)),
    );
    out.extend(
        feed[feed.len() - feed_n..]
            .iter()
            .cloned()
            .map(|l| truncate_to(l, w)),
    );
    if footer_n > 0 {
        out.push(footer_line(view, nav, w));
    }
    out
}

/// How a step is treated when the graph must shrink to fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Running or awaiting approval — the active frontier. Never collapsed.
    Frontier,
    /// Failed, paused, or chosen by the operator. Never collapsed.
    Keep,
    /// Complete or skipped, and not chosen. The first thing to fold.
    Settled,
    /// Not started, and not chosen. Folds once the settled steps have.
    Pending,
}

fn classify(state: StepState, chosen: bool) -> Class {
    match state {
        StepState::Running | StepState::AwaitingApproval => Class::Frontier,
        _ if chosen => Class::Keep,
        StepState::Complete | StepState::Skipped => Class::Settled,
        StepState::Pending => Class::Pending,
        StepState::Failed | StepState::Paused => Class::Keep,
    }
}

/// One step's rows plus what the fitter needs to know about them.
struct Block<'a> {
    step: &'a StepView,
    rows: Vec<Line>,
    class: Class,
    /// Leading rows that survive any trimming: the step row, or a fan-out's
    /// header + density row.
    head: usize,
    /// Row the unit cursor sits on (an expanded fan-out only); trimming keeps
    /// it in view.
    cursor: Option<usize>,
    /// A fan-out shown collapsed (header + density + movers + hint). Its
    /// density row already carries the exact unit counts, so trimming it
    /// drops everything past the head rather than leave a `⋮ +N below` whose
    /// N counts hidden rows, not units.
    collapsed_fan_out: bool,
}

fn make_block<'a>(
    view: &RunView,
    step: &'a StepView,
    nav: &NavState,
    focus: Option<&str>,
) -> Block<'a> {
    let chosen = focus == Some(step.step_id.as_str());
    let fan_out = is_fan_out(step);
    let expanded = fan_out && chosen && nav.depth() != Depth::Run;
    let cursor = expanded
        .then(|| {
            let cur = nav.selected_unit_in(step)?;
            let pos = nav
                .filtered_units(step)
                .iter()
                .position(|u| u.index == cur.index)?;
            Some(FANOUT_HEAD + pos)
        })
        .flatten();
    Block {
        step,
        rows: step_block(view, step, nav, chosen),
        class: classify(step.state, chosen),
        head: if fan_out { FANOUT_HEAD } else { 1 },
        cursor,
        collapsed_fan_out: fan_out && !expanded,
    }
}

/// A display piece of the fitted graph: a visible block, or one summary row
/// standing for a run of collapsed blocks.
enum Piece {
    Block(usize),
    Run(Range<usize>),
}

/// Partition the blocks into pieces: each visible block on its own, each
/// maximal run of same-class collapsed blocks as one summary. A lone collapsed
/// block that is a single row already stays as it is — a summary of one row
/// would save nothing.
fn pieces(blocks: &[Block], collapsed: &[bool]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        if !collapsed[i] {
            out.push(Piece::Block(i));
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < blocks.len() && collapsed[j] && blocks[j].class == blocks[i].class {
            j += 1;
        }
        if j - i == 1 && blocks[i].rows.len() <= 1 {
            out.push(Piece::Block(i));
        } else {
            out.push(Piece::Run(i..j));
        }
        i = j;
    }
    out
}

/// Rows the graph takes with these collapse flags and per-block trim targets.
fn graph_height(blocks: &[Block], collapsed: &[bool], trim: &[usize]) -> usize {
    pieces(blocks, collapsed)
        .iter()
        .map(|p| match p {
            Piece::Block(i) => trim[*i],
            Piece::Run(_) => 1,
        })
        .sum()
}

/// The graph bounded to `avail` rows (see [`live_layout`] for the order in
/// which it shrinks). `avail == 0` is an empty graph.
fn fit_graph(view: &RunView, nav: &NavState, avail: usize) -> Vec<Line> {
    if avail == 0 {
        return Vec::new();
    }
    let focus = focused_step_id(view, nav);
    let blocks: Vec<Block> = view
        .steps
        .iter()
        .map(|s| make_block(view, s, nav, focus))
        .collect();
    let n = blocks.len();
    let mut collapsed = vec![false; n];
    let mut trim: Vec<usize> = blocks.iter().map(|b| b.rows.len()).collect();
    let fits = |collapsed: &[bool], trim: &[usize]| graph_height(&blocks, collapsed, trim) <= avail;

    // 1. Settled steps fold oldest first, only until the graph fits.
    let settled = (0..n).filter(|&i| blocks[i].class == Class::Settled);
    for i in settled {
        if fits(&collapsed, &trim) {
            break;
        }
        collapsed[i] = true;
    }
    // 2. Then pending steps fold, furthest from the frontier first.
    let pending = (0..n).rev().filter(|&i| blocks[i].class == Class::Pending);
    for i in pending {
        if fits(&collapsed, &trim) {
            break;
        }
        collapsed[i] = true;
    }
    // 3. Then the biggest surviving block gives up rows, one at a time. A
    //    target of `head + 1` has no room for a unit and the marker, so it
    //    goes straight to `head`. A collapsed fan-out has no window to keep: it
    //    goes straight to its head (header + exact-count density row).
    while !fits(&collapsed, &trim) {
        let biggest = (0..n)
            .filter(|&i| !collapsed[i] && trim[i] > blocks[i].head)
            .max_by_key(|&i| trim[i] - blocks[i].head);
        let Some(i) = biggest else { break };
        trim[i] = if blocks[i].collapsed_fan_out || trim[i] == blocks[i].head + 2 {
            blocks[i].head
        } else {
            trim[i] - 1
        };
    }

    let mut rows: Vec<Line> = Vec::new();
    let (mut frontier_at, mut keep_at) = (None, None);
    for piece in pieces(&blocks, &collapsed) {
        match piece {
            Piece::Block(i) => {
                let b = &blocks[i];
                if frontier_at.is_none() && b.class == Class::Frontier {
                    frontier_at = Some(rows.len());
                }
                if keep_at.is_none() && b.class == Class::Keep {
                    keep_at = Some(rows.len());
                }
                rows.extend(trim_block(b, trim[i]));
            }
            Piece::Run(range) => rows.push(summary_line(&blocks[range])),
        }
    }
    // 4. Only reachable when frontier + kept rows alone exceed `avail`.
    hard_clip(rows, frontier_at.or(keep_at).unwrap_or(0), avail)
}

/// `block`'s rows cut to `target`: the head rows, a window of the rest centred
/// on the unit cursor, and a last `⋮ +N above · +M below` row for what was
/// dropped. A target with no room for a window plus that marker keeps just the
/// head. A *collapsed* fan-out keeps just the head whenever it is cut at all:
/// its density row says exactly how many units are in each state, whereas a
/// `⋮ +N below` over its movers would count rows and read as a unit count.
fn trim_block(block: &Block, target: usize) -> Vec<Line> {
    let rows = &block.rows;
    let head = block.head.min(rows.len());
    if target >= rows.len() {
        return rows.clone();
    }
    if block.collapsed_fan_out || target < head + 2 {
        return rows[..head].to_vec();
    }
    let body = &rows[head..];
    let slots = target - head - 1;
    let cursor = block
        .cursor
        .map_or(0, |c| c.saturating_sub(head))
        .min(body.len() - 1);
    let start = cursor.saturating_sub(slots / 2).min(body.len() - slots);
    let mut out = rows[..head].to_vec();
    out.extend_from_slice(&body[start..start + slots]);
    out.push(hidden_row(start, body.len() - start - slots));
    out
}

/// `⋮ +N above · +M below` — what a trimmed window left out.
fn hidden_row(above: usize, below: usize) -> Line {
    let parts: Vec<String> = [(above, "above"), (below, "below")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, side)| format!("+{n} {side}"))
        .collect();
    Line::new()
        .dim(CHILD_INDENT)
        .dim(format!("⋮ {}", parts.join(" · ")))
}

/// `✓ first … last  (+N done · M skipped)` / `○ first … last  (+N pending)`
/// for a run of collapsed steps; runs of up to [`SUMMARY_NAMES`] list every
/// step.
fn summary_line(run: &[Block]) -> Line {
    let names: Vec<String> = run.iter().map(|b| printable(&b.step.step_id)).collect();
    let label = if names.len() <= SUMMARY_NAMES {
        names.join(" · ")
    } else {
        format!("{} … {}", names[0], names[names.len() - 1])
    };
    let pending = run.iter().all(|b| b.class == Class::Pending);
    let (st, count) = if pending {
        (Status::Waiting, format!("{} pending", run.len()))
    } else {
        let skipped = run
            .iter()
            .filter(|b| b.step.state == StepState::Skipped)
            .count();
        let done = run.len() - skipped;
        let mut parts = Vec::new();
        if done > 0 {
            parts.push(format!("{done} done"));
        }
        if skipped > 0 {
            parts.push(format!("{skipped} skipped"));
        }
        let st = if done > 0 {
            Status::Complete
        } else {
            Status::Skipped
        };
        (st, parts.join(" · "))
    };
    Line::new()
        .dim(NO_MARK)
        .status(st, st.glyph().to_string())
        .plain(" ")
        .dim(label)
        .dim(format!("  (+{count})"))
}

/// Keep `avail` of `rows` with `anchor` inside the window (one row of context
/// above it when there is room), marking what was cut with `⋮` rows. Only
/// reached when the frontier itself outgrows the graph's budget.
fn hard_clip(mut rows: Vec<Line>, anchor: usize, avail: usize) -> Vec<Line> {
    if rows.len() <= avail {
        return rows;
    }
    let lead = usize::from(avail >= 3);
    let start = anchor.saturating_sub(lead).min(rows.len() - avail);
    let above = start;
    let below = rows.len() - start - avail;
    let mut out: Vec<Line> = rows.drain(start..start + avail).collect();
    if above > 0 && avail >= 3 {
        out[0] = hidden_row(above + 1, 0);
    }
    if below > 0 && avail >= 2 {
        let last = out.len() - 1;
        out[last] = hidden_row(0, below + 1);
    }
    out
}

/// Width-clip a graph row while protecting its right-hand state. A row with a
/// [`LEADER`] (`<label> ···· <state>`) loses its *label* middle to an ellipsis
/// — the leading glyph and the trailing state stay; when even that is too
/// tight the leader shrinks to one space, and below that the row is clipped
/// plain (its leading glyph still says the state). Rows without a leader
/// (summaries, markers) are clipped plain.
fn clip_row(line: Line, w: usize) -> Line {
    if line.width() <= w {
        return line;
    }
    let Some(at) = line.segments.iter().rposition(|s| s.text == LEADER) else {
        return truncate_to(line, w);
    };
    let state_w: usize = line.segments[at + 1..].iter().map(|s| s.text.width()).sum();
    let Some(gap) = [LEADER, " "]
        .into_iter()
        .find(|g| w >= g.width() + state_w + MIN_LABEL)
    else {
        return truncate_to(line, w);
    };
    let mut label = line;
    let state = label.segments.split_off(at + 1);
    label.segments.pop(); // the leader itself
    let mut out = truncate_to(label, w - gap.width() - state_w).dim(gap);
    out.segments.extend(state);
    out
}

/// The breadcrumb row, ` › `-joined. A path too wide for `w` loses crumbs
/// from the *left* (`… › otter#41 › wren#1`): the deepest crumb is the one
/// that says where you are. `None` for an empty path.
fn crumb_line(crumbs: &[String], w: usize) -> Option<Line> {
    if crumbs.is_empty() {
        return None;
    }
    let build = |from: usize| {
        let mut line = Line::new();
        if from > 0 {
            line = line.dim("… › ");
        }
        for (i, crumb) in crumbs[from..].iter().enumerate() {
            if i > 0 {
                line = line.dim(" › ");
            }
            line = line.dim(printable(crumb));
        }
        line
    };
    let fitting = (0..crumbs.len()).map(build).find(|line| line.width() <= w);
    Some(fitting.unwrap_or_else(|| truncate_to(build(crumbs.len() - 1), w)))
}

/// The one-row key legend for what is selected. At an approval gate the
/// modal approve / reject keys replace the navigation set. Hints carry a rank;
/// a terminal too narrow for them all drops the highest-ranked (least
/// important) first, `q quit` (rank 0) last.
fn footer_line(view: &RunView, nav: &NavState, w: usize) -> Line {
    const NAVIGATE: &[(&str, u8)] = &[
        ("↑↓ move", 1),
        ("enter drill", 2),
        ("← back", 3),
        ("/ filter", 4),
        ("q quit", 0),
    ];
    const AT_GATE: &[(&str, u8)] = &[
        ("a approve", 1),
        ("r reject", 2),
        ("v findings", 3),
        ("q quit", 0),
    ];
    // The same predicate the key dispatch uses, so the legend never
    // advertises a key that would do nothing (or hides one that would act).
    let at_gate = nav.focused_gate(view).is_some();
    let mut hints = if at_gate { AT_GATE } else { NAVIGATE }.to_vec();
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
    use crate::output::live_view::nav::{NavKey, UnitFilter};
    use crate::output::live_view::row::{render_plain, Style};
    use crate::output::run_model::{GateView, RunView, UnitView};
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

    // ---- fan-out block ---------------------------------------------------

    /// A fan-out unit with the wire-derived fields a live run would have:
    /// every third unit has no codename; only started (non-queued) units know
    /// their provider/model.
    fn unit(i: usize, status: UnitStatus) -> UnitView {
        let started = status != UnitStatus::Queued;
        UnitView {
            index: i,
            unit_key: format!("svc-{i}"),
            agent: Some("breaker".into()),
            codename: matches!(i % 3, 1 | 2).then(|| format!("otter#{}", i + 1)),
            provider: started.then(|| "anthropic".to_string()),
            model: started.then(|| "claude-opus-5-5".to_string()),
            host: None,
            status,
        }
    }

    /// Step `id` of `kind` with one unit per entry of `statuses`.
    fn fanout_with(id: &str, kind: StepKind, statuses: &[UnitStatus]) -> RunView {
        let mut v = RunView::default();
        v.status = RunStatus::Running;
        start_as(&mut v, id, kind, None, None);
        let step = v.step_mut(id);
        for (i, st) in statuses.iter().enumerate() {
            step.units.insert(i, unit(i, *st));
        }
        v
    }

    /// `hunt`: 86 units — 52 done, 6 running (54..60), 2 failed (7, 31),
    /// 26 queued (60..86).
    fn fanout_view_86() -> RunView {
        fanout_with("hunt", StepKind::ForEach, &statuses_86())
    }

    /// The 86 unit statuses of [`fanout_view_86`].
    fn statuses_86() -> Vec<UnitStatus> {
        (0..86)
            .map(|i| {
                if i == 7 || i == 31 {
                    UnitStatus::Failed
                } else if i < 54 {
                    UnitStatus::Done
                } else if i < 60 {
                    UnitStatus::Running
                } else {
                    UnitStatus::Queued
                }
            })
            .collect()
    }

    /// A nav drilled into step 0 with the filter cycled `presses` times
    /// (All -> Running -> Failed -> Done).
    fn drilled(view: &RunView, presses: usize) -> NavState {
        let mut nav = NavState::default();
        nav.apply(NavKey::In, view);
        for _ in 0..presses {
            nav.apply(NavKey::Filter, view);
        }
        nav
    }

    fn lines_of(rows: &[Line]) -> Vec<String> {
        rows.iter()
            .map(|l| render_plain(std::slice::from_ref(l)))
            .collect()
    }

    #[test]
    fn fanout_collapsed_shows_density_movers_and_more_at_86_units() {
        let v = fanout_view_86();
        let step = &v.steps[0];
        let rows = fanout_block(step, &NavState::default(), false);
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);

        // The density row reconciles exactly with `unit_counts()`.
        let c = step.unit_counts();
        assert_eq!(
            (c.done, c.running, c.failed, c.queued, c.total),
            (52, 6, 2, 26, 86)
        );
        let lines = lines_of(&rows);
        assert!(
            lines[0].starts_with("  ◐ hunt · for_each · 86 units"),
            "{s}"
        );
        assert!(
            lines[1].ends_with(&format!(
                " {}/{} ✓{} ◐{} ✗{} ○{}",
                c.done, c.total, c.done, c.running, c.failed, c.queued
            )),
            "{s}"
        );
        // Four live movers — the last four running units — then `+K more`
        // with K = total - shown.
        assert_eq!(rows.len(), 2 + 4 + 1, "{s}");
        for running in [56, 57, 58, 59] {
            assert!(s.contains(&format!("svc-{running}")), "{s}");
        }
        for not_shown in [0, 7, 54, 55, 60, 85] {
            assert!(
                !s.contains(&format!("svc-{not_shown} ")),
                "svc-{not_shown}:\n{s}"
            );
        }
        assert!(lines[6].contains("… +82 more · [enter] expand"), "{s}");
        // No per-unit spend exists yet, so none is invented.
        assert!(!s.contains('⇡') && !s.contains('$'), "{s}");
    }

    #[test]
    fn fanout_selected_at_run_depth_stays_collapsed_via_graph() {
        let v = fanout_view_86();
        let mut nav = NavState::default();
        nav.apply(NavKey::Down, &v); // operator-selected, still Run depth
        let rows = graph(&v, &nav);
        let s = render_plain(&rows);
        assert!(s.starts_with("▸ ◐ hunt · for_each · 86 units"), "{s}");
        assert!(s.contains("+82 more · [enter] expand"), "{s}");
        assert_eq!(rows.len(), 7, "{s}");
    }

    #[test]
    fn fanout_expanded_failed_filter_lists_only_failed_units() {
        let v = fanout_view_86();
        let nav = drilled(&v, 2);
        assert_eq!(nav.filter(), UnitFilter::Failed);
        let rows = fanout_block(&v.steps[0], &nav, true);
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);

        let lines = lines_of(&rows);
        assert_eq!(lines.len(), 2 + 2, "{s}");
        assert!(
            lines[0].starts_with("▸ ◐ hunt · for_each · 86 units · failed"),
            "{s}"
        );
        // The density row is unfiltered: counts still describe the whole step.
        assert!(lines[1].ends_with("52/86 ✓52 ◐6 ✗2 ○26"), "{s}");
        assert!(lines[2].contains("✗ otter#8 · svc-7 "), "{s}");
        assert!(lines[3].contains("✗ otter#32 · svc-31 "), "{s}");
        assert!(!s.contains("more") && !s.contains("expand"), "{s}");
        // Below the (unfiltered) density row nothing but failed units renders.
        for row in &lines[2..] {
            assert!(
                row.contains('✗') && !row.contains('✓') && !row.contains('○'),
                "{s}"
            );
        }
        assert_eq!(s.matches("svc-").count(), 2, "{s}");
    }

    #[test]
    fn fanout_expanded_all_lists_every_unit_with_no_more_row() {
        let v = fanout_view_86();
        let nav = drilled(&v, 0);
        let rows = fanout_block(&v.steps[0], &nav, true);
        let s = render_plain(&rows);
        assert_eq!(rows.len(), 2 + 86, "{s}");
        assert!(!s.contains("more"), "{s}");
        // `All` shows no filter tag.
        assert!(lines_of(&rows)[0].ends_with("86 units"), "{s}");
        // Tree closes on the last unit only.
        assert_eq!(s.matches("┗━").count(), 1, "{s}");
        let last = s.lines().last().unwrap();
        assert!(last.contains("┗━ ○") && last.contains("svc-85"), "{s}");

        let running = fanout_block(&v.steps[0], &drilled(&v, 1), true);
        let s = render_plain(&running);
        assert_eq!(running.len(), 2 + 6, "{s}");
        assert!(lines_of(&running)[0].ends_with("86 units · running"), "{s}");
        let done = fanout_block(&v.steps[0], &drilled(&v, 3), true);
        assert_eq!(done.len(), 2 + 52, "done filter");
        assert!(lines_of(&done)[0].ends_with("86 units · done"));
    }

    #[test]
    fn fanout_marks_only_the_selected_unit_when_expanded() {
        let v = fanout_view_86();
        let mut nav = drilled(&v, 2); // Failed filter: svc-7, svc-31
        let marked = |nav: &NavState| -> Vec<String> {
            lines_of(&fanout_block(&v.steps[0], nav, true))
                .into_iter()
                .skip(2)
                .filter(|l| l.trim_start().starts_with('▸'))
                .collect()
        };
        let m = marked(&nav);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("svc-7"), "{m:?}");
        nav.apply(NavKey::Down, &v);
        let m = marked(&nav);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].contains("svc-31"), "{m:?}");
    }

    #[test]
    fn fanout_expanded_with_no_matching_units_says_so() {
        let v = fanout_with(
            "hunt",
            StepKind::ForEach,
            &[UnitStatus::Done, UnitStatus::Running],
        );
        let nav = drilled(&v, 2); // Failed
        let s = render_plain(&fanout_block(&v.steps[0], &nav, true));
        assert!(s.contains("no failed units"), "{s}");
        assert!(!s.contains("svc-"), "{s}");
    }

    #[test]
    fn run_step_carrying_units_routes_to_fanout_block() {
        // `run:` + `for_each:` is StepKind::Run but carries units.
        let v = fanout_with(
            "sweep",
            StepKind::Run,
            &[UnitStatus::Done, UnitStatus::Running, UnitStatus::Failed],
        );
        let s = render_plain(&graph(&v, &NavState::default()));
        assert!(s.contains("◐ sweep · for_each · 3 units"), "{s}");
        assert!(s.contains(" 1/3 "), "{s}");
        assert!(s.contains("✓1 ◐1 ✗1"), "{s}");
        assert!(!s.contains("· run"), "{s}");
        // A `run:` step with no units is still a plain `run` row.
        let mut plain = RunView::default();
        start_as(&mut plain, "cmd", StepKind::Run, None, None);
        let s = render_plain(&graph(&plain, &NavState::default()));
        assert_eq!(s.trim_start(), "◐ cmd · run ···· ◐ running", "{s}");
    }

    #[test]
    fn panel_step_with_units_keeps_its_panel_row() {
        // Panelists arrive as units; the panel row (round counter) must win.
        let mut v = fanout_with(
            "review",
            StepKind::Panel,
            &[UnitStatus::Done, UnitStatus::Running],
        );
        v.step_mut("review").panel_round = Some(2);
        v.step_mut("review").panel_max = Some(5);
        let s = render_plain(&graph(&v, &NavState::default()));
        assert_eq!(
            s.trim_start(),
            "◐ review · panel iter 2/5 ···· ◐ running",
            "{s}"
        );
    }

    #[test]
    fn fanout_with_no_units_yet_renders_an_empty_bar_without_panicking() {
        let v = fanout_with("hunt", StepKind::ForEach, &[]);
        let rows = fanout_block(&v.steps[0], &NavState::default(), false);
        let lines = lines_of(&rows);
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert_eq!(lines[0].trim_start(), "◐ hunt · for_each");
        assert_eq!(lines[1].trim_start(), format!("{} 0/0", "░".repeat(20)));
        // Expanding an empty step does not panic either.
        let nav = drilled(&v, 0);
        let s = render_plain(&fanout_block(&v.steps[0], &nav, true));
        assert!(s.contains("no units"), "{s}");
    }

    #[test]
    fn density_bar_is_full_width_and_only_full_when_every_unit_is_done() {
        let bar = |statuses: &[UnitStatus]| -> (usize, usize) {
            let v = fanout_with("h", StepKind::ForEach, statuses);
            let s = render_plain(&fanout_block(&v.steps[0], &NavState::default(), false));
            (s.matches('▓').count(), s.matches('░').count())
        };
        let mut u = vec![UnitStatus::Done; 85];
        u.push(UnitStatus::Running);
        // 85/86 rounds to a full bar, but the step is not finished.
        assert_eq!(bar(&u), (19, 1));
        assert_eq!(bar(&[UnitStatus::Done; 86]), (20, 0));
        // One done of many still shows progress.
        let mut u = vec![UnitStatus::Queued; 200];
        u[0] = UnitStatus::Done;
        assert_eq!(bar(&u), (1, 19));
        assert_eq!(bar(&[UnitStatus::Queued; 3]), (0, 20));
    }

    #[test]
    fn small_fanout_shows_every_unit_and_closes_the_tree_without_more() {
        let v = fanout_with(
            "hunt",
            StepKind::ForEach,
            &[UnitStatus::Done, UnitStatus::Running, UnitStatus::Queued],
        );
        let s = render_plain(&fanout_block(&v.steps[0], &NavState::default(), false));
        assert!(!s.contains("more") && !s.contains("expand"), "{s}");
        assert_eq!(s.matches("┣━").count(), 2, "{s}");
        assert_eq!(s.matches("┗━").count(), 1, "{s}");
        let last = s.lines().last().unwrap();
        assert!(last.contains("┗━ ○") && last.contains("svc-2"), "{s}");
    }

    #[test]
    fn unit_rows_drop_empty_parts_and_control_characters() {
        let mut v = fanout_with("hunt", StepKind::ForEach, &[UnitStatus::Running]);
        let u = v.step_mut("hunt").units.get_mut(&0).unwrap();
        u.codename = None;
        u.provider = Some(String::new());
        u.model = Some("opus".into());
        let row = render_plain(&fanout_block(&v.steps[0], &NavState::default(), false)[2..]);
        assert_eq!(
            row.trim_start(),
            "┗━ ◐ svc-0 · opus ···· ◐ running",
            "{row}"
        );

        // Empty provider AND model, empty unit key: no dangling separators.
        let u = v.step_mut("hunt").units.get_mut(&0).unwrap();
        u.provider = Some(String::new());
        u.model = Some(String::new());
        u.unit_key = "hostile\u{1b}[31m\nkey".into();
        let rows = fanout_block(&v.steps[0], &NavState::default(), false);
        let row = render_plain(&rows[2..]);
        assert!(!row.contains('\u{1b}') && !row.contains('\n'), "{row:?}");
        assert!(!row.contains(" ·  ·") && !row.contains("· ····"), "{row}");

        // The shared member formatter filters empty parts for step rows too.
        let mut v = RunView::default();
        start_as(&mut v, "a", StepKind::Linear, Some(""), None);
        v.step_mut("a").provider = Some(String::new());
        v.step_mut("a").model = Some(String::new());
        let s = render_plain(&graph(&v, &NavState::default()));
        assert_eq!(s.trim_start(), "◐ a ···· ◐ running", "{s}");
    }

    #[test]
    fn movers_fill_from_non_running_units_when_fewer_than_four_run() {
        // 1 running (index 2) + 5 others: the running unit leads, padded by
        // the highest-index remaining units.
        let v = fanout_with(
            "hunt",
            StepKind::ForEach,
            &[
                UnitStatus::Done,
                UnitStatus::Done,
                UnitStatus::Running,
                UnitStatus::Queued,
                UnitStatus::Queued,
                UnitStatus::Queued,
            ],
        );
        let s = render_plain(&fanout_block(&v.steps[0], &NavState::default(), false));
        let keys: Vec<&str> = s
            .lines()
            .filter_map(|l| l.split("svc-").nth(1))
            .map(|r| r.split(' ').next().unwrap())
            .collect();
        assert_eq!(keys, vec!["2", "3", "4", "5"], "{s}");
        assert!(s.contains("+2 more · [enter] expand"), "{s}");
    }

    #[test]
    fn parallel_fanout_selected_shows_no_dispatch_children_and_parallel_label() {
        let mut v = fanout_with(
            "fan",
            StepKind::Parallel,
            &[UnitStatus::Done, UnitStatus::Running, UnitStatus::Queued],
        );
        // A sub-agent dispatched while `fan` was active (parent_step_id = fan).
        dispatch(
            &mut v,
            "sub_1",
            "triage",
            "wren#1",
            "claude-haiku-4-5",
            false,
        );
        assert_eq!(
            v.dispatches
                .values()
                .next()
                .unwrap()
                .parent_step_id
                .as_deref(),
            Some("fan"),
            "fixture: the dispatch belongs to the parallel step"
        );

        // Operator-selected at Run depth, then drilled to Step and Unit depth:
        // the `parallel` label shows and no dispatch child is ever listed —
        // only a singleton `step_row` renders sub-agents.
        let mut run_depth = NavState::default();
        run_depth.apply(NavKey::Down, &v);
        let drilled_step = drilled(&v, 0);
        let mut drilled_unit = drilled(&v, 0);
        drilled_unit.apply(NavKey::In, &v);
        assert_eq!(
            drilled_step.depth(),
            crate::output::live_view::nav::Depth::Step
        );
        assert_eq!(
            drilled_unit.depth(),
            crate::output::live_view::nav::Depth::Unit
        );
        for nav in [run_depth, drilled_step, drilled_unit] {
            let rows = graph(&v, &nav);
            let s = render_plain(&rows);
            assert!(s.contains("fan · parallel · 3 units"), "{s}");
            assert!(!s.contains("for_each"), "{s}");
            assert!(!s.contains("wren#1") && !s.contains("triage"), "{s}");
        }
    }

    #[test]
    fn density_bar_never_emits_an_empty_segment() {
        // A zero-width segment would still wrap a styled run (stray ANSI
        // resets) in the Plan 3 renderer.
        let no_empty = |statuses: &[UnitStatus]| {
            let v = fanout_with("h", StepKind::ForEach, statuses);
            let density = &fanout_block(&v.steps[0], &NavState::default(), false)[1];
            assert!(
                density.segments.iter().all(|seg| !seg.text.is_empty()),
                "{:?}",
                density.segments
            );
        };
        no_empty(&[UnitStatus::Queued; 3]); // 0/3: no filled run
        no_empty(&[UnitStatus::Done; 86]); // 86/86: no empty run
        no_empty(&[]); // 0/0
        no_empty(&[UnitStatus::Done, UnitStatus::Queued]); // both runs present
    }

    // ---- live_layout: compose + adaptive bounding ----------------------------

    /// Terminal width of the frame snapshots.
    const W: usize = 100;

    /// 40 steps / 86 units: `stage-00`..`stage-29` settled (05 and 06
    /// skipped), the `hunt` for_each fan-out running, nine steps pending.
    fn big_run() -> RunView {
        let mut v = RunView::default();
        v.workflow_name = "assess-services".into();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        v.started_at = Some(now() - Duration::seconds(4 * 60 + 12));
        for i in 0..30 {
            complete(&mut v, &format!("stage-{i:02}"));
        }
        for skipped in ["stage-05", "stage-06"] {
            v.step_mut(skipped).state = StepState::Skipped;
        }
        start_as(&mut v, "hunt", StepKind::ForEach, None, None);
        for (i, st) in statuses_86().into_iter().enumerate() {
            v.step_mut("hunt").units.insert(i, unit(i, st));
        }
        for id in [
            "verify", "triage", "report", "notify", "ticket", "archive", "digest", "publish",
            "cleanup",
        ] {
            v.step_mut(id);
        }
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

    /// `n` feed lines, oldest first; every one starts `feed `.
    fn feed(n: usize) -> Vec<Line> {
        (0..n)
            .map(|i| {
                Line::new()
                    .dim("feed ")
                    .plain(format!("{i:02} otter#57 read_file src/lib.rs"))
            })
            .collect()
    }

    fn feed_rows(out: &[Line]) -> Vec<String> {
        lines_of(out)
            .into_iter()
            .filter(|l| l.starts_with("feed "))
            .collect()
    }

    /// Rows of the running `hunt` fan-out's block (its header names it; the
    /// dashboard's progress row does not carry `for_each`).
    fn hunt_headers(out: &[Line]) -> usize {
        lines_of(out)
            .iter()
            .filter(|l| l.contains("hunt · for_each"))
            .count()
    }

    /// A nav that selected step `idx`, drilled in once, with the unit filter
    /// cycled `presses` times.
    fn drilled_at(view: &RunView, idx: usize, presses: usize) -> NavState {
        let mut nav = nav_at(view, idx);
        nav.apply(NavKey::In, view);
        for _ in 0..presses {
            nav.apply(NavKey::Filter, view);
        }
        nav
    }

    #[test]
    fn live_layout_24_rows_collapses_settled_and_keeps_frontier_and_feed() {
        let v = big_run();
        assert_eq!(v.steps.len(), 40);
        assert_eq!(v.steps.iter().map(|s| s.units.len()).sum::<usize>(), 86);
        let out = live_layout(&v, &NavState::default(), &feed(12), now(), W, 24);
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        assert_eq!(out.len(), 24, "{s}");
        // The running frontier and its density block are on screen.
        assert!(s.contains("◐ hunt · for_each · 86 units"), "{s}");
        assert!(s.contains("52/86 ✓52 ◐6 ✗2 ○26"), "{s}");
        assert!(s.contains("+82 more · [enter] expand"), "{s}");
        // Every settled step folded into one summary row.
        assert!(
            s.contains("✓ stage-00 … stage-29  (+28 done · 2 skipped)"),
            "{s}"
        );
        assert_eq!(s.matches("stage-").count(), 2, "summary names 2 ends:\n{s}");
        // The feed keeps its minimum — and it is the newest lines.
        let feed = feed_rows(&out);
        assert_eq!(feed.len(), 4, "{s}");
        assert_eq!(
            feed.last().unwrap(),
            "feed 11 otter#57 read_file src/lib.rs"
        );
        assert!(lines_of(&out).last().unwrap().starts_with("↑↓ move"), "{s}");
    }

    #[test]
    fn live_layout_60_rows_shows_every_step_and_more_feed() {
        let v = big_run();
        let out = live_layout(&v, &NavState::default(), &feed(12), now(), W, 60);
        let s = render_plain(&out);
        insta::assert_snapshot!(s);

        assert!(out.len() <= 60, "{s}");
        // Room for the whole graph: nothing is collapsed.
        assert!(!s.contains("(+"), "no summary rows:\n{s}");
        assert_eq!(s.matches("stage-").count(), 30, "{s}");
        assert!(s.contains("○ cleanup"), "{s}");
        assert!(s.contains("◐ hunt · for_each · 86 units"), "{s}");
        // The 46-row graph leaves 10 rows: the feed shows its newest 10.
        let feed = feed_rows(&out);
        assert_eq!(feed.len(), 10, "{s}");
        assert!(
            feed[0].starts_with("feed 02") && feed[9].starts_with("feed 11"),
            "{s}"
        );
        assert_eq!(out.len(), 60, "{s}");
    }

    #[test]
    fn collapse_is_progressive_oldest_settled_first() {
        // 40 rows: 32 for the graph; 46 needed, so the oldest 15 settled
        // steps fold (saving 14 rows) and the newest 15 stay.
        let v = big_run();
        let s = render_plain(&live_layout(
            &v,
            &NavState::default(),
            &feed(12),
            now(),
            W,
            40,
        ));
        assert!(
            s.contains("✓ stage-00 … stage-14  (+13 done · 2 skipped)"),
            "{s}"
        );
        assert!(!s.contains("stage-14 ·"), "{s}");
        assert!(s.contains("✓ stage-15 ·"), "stage-15 stays expanded:\n{s}");
        assert!(s.contains("✓ stage-29 ·"), "{s}");
        assert!(s.contains("○ cleanup"), "pending is not folded:\n{s}");
    }

    #[test]
    fn breadcrumb_row_appears_only_when_drilled() {
        let v = big_run();
        let run_depth = lines_of(&live_layout(
            &v,
            &NavState::default(),
            &feed(12),
            now(),
            W,
            30,
        ));
        assert!(run_depth.iter().all(|l| !l.contains('›')), "{run_depth:#?}");

        let nav = drilled_at(&v, 30, 0);
        assert_eq!(nav.depth(), Depth::Step);
        let out = live_layout(&v, &nav, &feed(12), now(), W, 24);
        let s = render_plain(&out);
        insta::assert_snapshot!(s);
        let lines = lines_of(&out);
        // Dashboard (3 rows), then the breadcrumb.
        assert_eq!(lines[3], "mint-tundra › hunt", "{s}");
        // The expanded fan-out is bounded to the terminal, keeps its header +
        // density, the cursor unit, and says what was dropped.
        assert!(out.len() <= 24, "{s}");
        assert!(lines[4].starts_with("  ✓ stage-00 … stage-29"), "{s}");
        assert!(
            lines[5].starts_with("▸ ◐ hunt · for_each · 86 units"),
            "{s}"
        );
        assert!(lines[6].contains("52/86 ✓52 ◐6 ✗2 ○26"), "{s}");
        assert!(
            lines[7].starts_with("  ▸ ┣━ ✓") && lines[7].contains("svc-0 "),
            "{s}"
        );
        assert!(s.contains("⋮ +"), "{s}");
        assert_eq!(feed_rows(&out).len(), 4, "{s}");

        // One level deeper the path grows a unit crumb.
        let mut unit_nav = nav;
        unit_nav.apply(NavKey::In, &v);
        assert_eq!(unit_nav.depth(), Depth::Unit);
        let out = live_layout(&v, &unit_nav, &feed(12), now(), W, 24);
        assert_eq!(lines_of(&out)[3], "mint-tundra › hunt › svc-0");
    }

    #[test]
    fn breadcrumb_clips_from_the_left_and_yields_before_the_frontier() {
        let crumbs: Vec<String> = ["mint-tundra", "hunt", "otter#41", "wren#1"]
            .map(String::from)
            .to_vec();
        let plain = |w: usize| render_plain(&[crumb_line(&crumbs, w).unwrap()]);
        assert_eq!(plain(80), "mint-tundra › hunt › otter#41 › wren#1");
        // Too narrow: the root crumbs go first, the deepest one stays.
        assert_eq!(plain(28), "… › hunt › otter#41 › wren#1");
        assert_eq!(plain(27), "… › otter#41 › wren#1");
        assert_eq!(plain(21), "… › otter#41 › wren#1");
        assert_eq!(plain(20), "… › wren#1");
        assert_eq!(plain(10), "… › wren#1");
        assert_eq!(plain(6), "… › w…");
        assert!(crumb_line(&[], 80).is_none());

        // At the very floor there is no spare row for the breadcrumb: it
        // yields so the frontier still fits.
        let v = big_run();
        let floor = dashboard(&v, now()).len() + 1 + 4 + 1;
        let nav = drilled_at(&v, 30, 0);
        let out = live_layout(&v, &nav, &feed(12), now(), W, floor);
        let s = render_plain(&out);
        assert_eq!(out.len(), floor, "{s}");
        assert!(!s.contains("mint-tundra ›"), "{s}");
        assert_eq!(hunt_headers(&out), 1, "{s}");
    }

    #[test]
    fn footer_is_a_context_key_legend() {
        let mut v = RunView::default();
        v.status = RunStatus::AwaitingApproval;
        complete(&mut v, "build");
        let gate = v.step_mut("gate");
        gate.kind = StepKind::ApprovalGate;
        gate.state = StepState::AwaitingApproval;
        v.step_mut("deploy");
        // A step that merely LOOKS parked (no entry in run.json's awaiting
        // set) is not actionable: the footer must not advertise a/r on it.
        let footer_of = |v: &RunView, nav: &NavState, w: usize| {
            let out = live_layout(v, nav, &feed(6), now(), w, 24);
            render_plain(&out[out.len() - 1..])
        };
        let normal = "↑↓ move · enter drill · ← back · / filter · q quit";
        let at_gate = "a approve · r reject · v findings · q quit";
        assert_eq!(footer_of(&v, &NavState::default(), W), normal);
        assert_eq!(footer_of(&v, &nav_at(&v, 1), W), normal);

        v.gates = vec![GateView {
            step_id: "gate".into(),
            prompt: None,
            since: now(),
            expires_at: None,
        }];
        let footer = |nav: &NavState, w: usize| footer_of(&v, nav, w);
        // Following with the run parked: the gate is auto-focused, so its
        // keys show without any navigation (Plan 2's I1 gap).
        assert_eq!(footer(&NavState::default(), W), at_gate);
        // The operator selects another step: navigation keys. (`nav_at(_, 0)`
        // is just the default, still following; stepping down then back up
        // selects step 0 by hand.)
        let mut at_build = nav_at(&v, 1);
        at_build.apply(NavKey::Up, &v);
        assert!(!at_build.is_following());
        assert_eq!(footer(&at_build, W), normal);
        assert_eq!(footer(&nav_at(&v, 2), W), normal);
        // Selecting the parked gate: the modal approve/reject set.
        assert_eq!(footer(&nav_at(&v, 1), W), at_gate);
        assert_eq!(footer(&drilled_at(&v, 1, 0), W), at_gate);

        // A narrow terminal drops the least important hints; `q quit` stays.
        assert_eq!(footer(&nav_at(&v, 2), 30), "↑↓ move · enter drill · q quit");
        assert_eq!(footer(&nav_at(&v, 1), 30), "a approve · r reject · q quit");
        assert_eq!(footer(&nav_at(&v, 1), 24), "a approve · q quit");
        assert_eq!(footer(&nav_at(&v, 2), 6), "q quit");
    }

    #[test]
    fn the_auto_focused_gate_is_marked_so_a_and_r_have_a_visible_target() {
        let mut v = RunView::default();
        v.status = RunStatus::AwaitingApproval;
        complete(&mut v, "build");
        for id in ["gate_a", "gate_b"] {
            let g = v.step_mut(id);
            g.kind = StepKind::ApprovalGate;
            g.state = StepState::AwaitingApproval;
            v.gates.push(GateView {
                step_id: id.into(),
                prompt: None,
                since: now(),
                expires_at: None,
            });
        }
        let marked = |v: &RunView, nav: &NavState| -> Vec<String> {
            lines_of(&graph(v, nav))
                .into_iter()
                .filter(|l| l.starts_with('▸'))
                .collect()
        };
        // Following: exactly the first parked gate carries the marker.
        let auto = marked(&v, &NavState::default());
        assert_eq!(auto.len(), 1, "{auto:?}");
        assert!(auto[0].contains("gate_a"), "{auto:?}");
        // Choosing the second gate moves it there.
        let second = marked(&v, &nav_at(&v, 2));
        assert_eq!(second.len(), 1, "{second:?}");
        assert!(second[0].contains("gate_b"), "{second:?}");
        // A run that is not parked marks nothing while following.
        v.status = RunStatus::Running;
        assert!(marked(&v, &NavState::default()).is_empty());
    }

    #[test]
    fn selected_and_failed_steps_are_never_collapsed() {
        let mut v = big_run();
        v.step_mut("stage-20").state = StepState::Failed;
        // Select a settled step in the middle of the settled region.
        let s = render_plain(&live_layout(&v, &nav_at(&v, 10), &feed(12), now(), W, 24));
        assert!(s.contains("▸ ✓ stage-10 "), "selected stays:\n{s}");
        assert!(s.contains("✗ stage-20 · "), "failed stays:\n{s}");
        // Runs either side of the kept rows fold independently.
        assert!(
            s.contains("✓ stage-00 … stage-09  (+8 done · 2 skipped)"),
            "{s}"
        );
        assert!(s.contains("✓ stage-11 … stage-19  (+9 done)"), "{s}");
        assert!(s.contains("✓ stage-21 … stage-29  (+9 done)"), "{s}");
        assert!(s.contains("◐ hunt · for_each"), "{s}");
    }

    #[test]
    fn state_survives_a_narrow_terminal() {
        // `scan` is selected: its label is long, its state is what matters.
        let v = graph_view();
        let out = live_layout(&v, &nav_at(&v, 3), &feed(4), now(), 44, 24);
        let rows = lines_of(&out);
        let scan = rows.iter().find(|l| l.contains("scan")).unwrap();
        assert!(scan.ends_with(" ···· ◐ running"), "{scan}");
        assert!(scan.contains('…') && scan.starts_with("▸ ◐ scan"), "{scan}");
        assert!(out.iter().all(|l| l.width() <= 44), "{rows:#?}");

        // A done row keeps its duration, a fan-out unit row its state.
        let s = render_plain(&live_layout(&v, &nav_at(&v, 3), &feed(4), now(), 26, 24));
        assert!(s.contains("✓ 18s"), "{s}");
        let fan = big_run();
        let s = render_plain(&live_layout(
            &fan,
            &drilled_at(&fan, 30, 1),
            &feed(4),
            now(),
            40,
            30,
        ));
        let unit = s
            .lines()
            .find(|l| l.contains("┣━") || l.contains("┗━"))
            .unwrap();
        assert!(unit.ends_with("◐ running"), "{s}");
        assert!(
            unit.contains('…') && unicode_width::UnicodeWidthStr::width(unit) <= 40,
            "{s}"
        );
    }

    #[test]
    fn frontier_and_feed_are_always_visible_above_the_floor() {
        let v = big_run();
        let feed = feed(12);
        let floor = dashboard(&v, now()).len() + 1 + 4 + 1;
        let navs = [
            ("following", NavState::default()),
            ("select hunt", nav_at(&v, 30)),
            ("select a settled step", nav_at(&v, 3)),
            ("select the last pending step", nav_at(&v, 39)),
            ("drill hunt", drilled_at(&v, 30, 0)),
            ("drill hunt, failed filter", drilled_at(&v, 30, 2)),
            ("drill a settled step", drilled_at(&v, 3, 0)),
            ("drill a pending step", drilled_at(&v, 35, 0)),
            ("drill a unit", {
                let mut n = drilled_at(&v, 30, 0);
                n.apply(NavKey::In, &v);
                n
            }),
        ];
        for (name, nav) in &navs {
            for w in [100usize, 60, 30] {
                for h in floor..=70 {
                    let out = live_layout(&v, nav, &feed, now(), w, h);
                    let s = render_plain(&out);
                    let ctx = format!("{name} w={w} h={h}\n{s}");
                    assert!(out.len() <= h, "{ctx}");
                    assert!(out.iter().all(|l| l.width() <= w), "{ctx}");
                    assert!(hunt_headers(&out) >= 1, "frontier row missing:\n{ctx}");
                    let fr = feed_rows(&out);
                    assert!(fr.len() >= 4, "feed shrank below its floor:\n{ctx}");
                    assert!(
                        fr[fr.len() - 1].starts_with("feed 11"),
                        "newest feed:\n{ctx}"
                    );
                    assert!(
                        lines_of(&out).last().unwrap().ends_with("q quit"),
                        "footer is always the last row:\n{ctx}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_drilled_non_frontier_fan_out_does_not_push_the_frontier_off() {
        // `sweep` (done, 30 units) is drilled; `hunt` is what is running.
        let mut v = RunView::default();
        v.crew = Some("mint-tundra".into());
        v.status = RunStatus::Running;
        start_as(&mut v, "sweep", StepKind::ForEach, None, None);
        for i in 0..30 {
            v.step_mut("sweep")
                .units
                .insert(i, unit(i, UnitStatus::Done));
        }
        v.step_mut("sweep").state = StepState::Complete;
        start_as(&mut v, "hunt", StepKind::ForEach, None, None);
        for (i, st) in statuses_86().into_iter().take(10).enumerate() {
            v.step_mut("hunt").units.insert(i, unit(i, st));
        }
        let nav = drilled_at(&v, 0, 0);
        assert_eq!(nav.selected_step(&v).unwrap().step_id, "sweep");
        // dashboard 2 + breadcrumb 1 + footer 1 + feed 4 = 8 fixed rows.
        for h in 9..=30 {
            let out = live_layout(&v, &nav, &feed(8), now(), W, h);
            let s = render_plain(&out);
            assert!(out.len() <= h, "h={h}\n{s}");
            // The frontier is on screen at every height…
            assert_eq!(hunt_headers(&out), 1, "h={h}\n{s}");
            assert!(feed_rows(&out).len() >= 4, "h={h}\n{s}");
            // …and once the graph has room for both headers + densities, so
            // is the drilled step the operator asked for.
            if h >= 8 + 4 {
                assert!(s.contains("▸ ✓ sweep · for_each · 30 units"), "h={h}\n{s}");
                assert!(s.contains("30/30 ✓30"), "h={h}\n{s}");
            }
        }
        // With room, the drilled list shows its cursor unit as well.
        let s = render_plain(&live_layout(&v, &nav, &feed(8), now(), W, 26));
        assert!(s.contains("▸ ┣━ ✓") && s.contains("svc-0 "), "{s}");
    }

    #[test]
    fn collapsed_fanout_trim_keeps_density_not_misleading_row_count() {
        // `big_run`'s 86-unit `hunt` is the running frontier, collapsed (Run
        // depth, no unit cursor). At h=17 its whole block (header, density, 4
        // movers, hint) fits; below that it must trim. A trimmed collapsed
        // block keeps only its header + density row — the density row already
        // carries the exact unit counts — and must NOT emit a `⋮ +N below`
        // whose N counts hidden ROWS (a handful) while ~85 units are hidden.
        let v = big_run();
        let nav = NavState::default();
        let full = render_plain(&live_layout(&v, &nav, &feed(12), now(), W, 17));
        assert!(full.contains("+82 more · [enter] expand"), "{full}");
        assert!(!full.contains('⋮'), "{full}");

        let floor = dashboard(&v, now()).len() + 1 + 4 + 1;
        // 12 is the least that holds summary + header + density + summary.
        for h in 12..17 {
            let out = live_layout(&v, &nav, &feed(12), now(), W, h);
            let s = render_plain(&out);
            assert!(h >= floor && out.len() <= h, "h={h}\n{s}");
            // Frontier header + the truthful density row stay on screen.
            assert_eq!(hunt_headers(&out), 1, "h={h}\n{s}");
            assert!(s.contains("52/86 ✓52 ◐6 ✗2 ○26"), "h={h}\n{s}");
            // No row-count marker, and no half-shown mover list.
            assert!(!s.contains('⋮'), "h={h}\n{s}");
            assert!(!s.contains("┣━") && !s.contains("┗━"), "h={h}\n{s}");
            // The frontier-and-feed invariant still holds.
            assert!(feed_rows(&out).len() >= 4, "h={h}\n{s}");
            assert!(lines_of(&out).last().unwrap().starts_with("↑↓ move"), "{s}");
        }
    }

    #[test]
    fn many_running_steps_are_clipped_around_the_first_with_a_marker() {
        // 12 parallel running steps cannot all fit: the graph keeps the
        // frontier's start, never a window that starts past it.
        let mut v = RunView::default();
        v.status = RunStatus::Running;
        complete(&mut v, "prep");
        for i in 0..12 {
            start(&mut v, &format!("lane-{i:02}"));
        }
        let floor = dashboard(&v, now()).len() + 1 + 4 + 1;
        for h in floor..=floor + 6 {
            let out = live_layout(&v, &NavState::default(), &feed(10), now(), W, h);
            let s = render_plain(&out);
            assert_eq!(out.len(), h, "{s}");
            assert!(s.contains("◐ lane-00"), "first frontier row kept:\n{s}");
            assert!(feed_rows(&out).len() >= 4, "{s}");
        }
        let s = render_plain(&live_layout(
            &v,
            &NavState::default(),
            &feed(10),
            now(),
            W,
            floor + 4,
        ));
        assert!(s.contains("⋮ +"), "says rows were cut:\n{s}");
    }

    #[test]
    fn feed_shorter_than_its_floor_gives_the_rest_to_the_graph() {
        let v = big_run();
        // No feed at all: the graph gets every row it can use.
        let out = live_layout(&v, &NavState::default(), &[], now(), W, 24);
        assert_eq!(out.len(), 24);
        assert!(feed_rows(&out).is_empty());
        let with_feed = live_layout(&v, &NavState::default(), &feed(2), now(), W, 24);
        assert_eq!(feed_rows(&with_feed).len(), 2);
        // Each feed row costs the graph one: the 46-row graph folds its oldest
        // settled steps only as far as it must (20 rows free, then 18).
        let shows = |o: &[Line], step: &str| render_plain(o).contains(&format!("✓ {step} ·"));
        assert!(shows(&out, "stage-27") && shows(&out, "stage-29"));
        assert!(!shows(&with_feed, "stage-27") && shows(&with_feed, "stage-29"));
    }

    #[test]
    fn cramped_terminals_still_get_a_bounded_useful_frame() {
        let v = big_run();
        let floor = dashboard(&v, now()).len() + 1 + 4 + 1;
        for h in 0..floor {
            let out = live_layout(&v, &NavState::default(), &feed(12), now(), W, h);
            let s = render_plain(&out);
            assert!(out.len() <= h, "h={h}\n{s}");
            if h > 0 {
                assert!(s.lines().last().unwrap().ends_with("q quit"), "h={h}\n{s}");
            }
        }
        assert!(live_layout(&v, &NavState::default(), &feed(12), now(), W, 0).is_empty());
        assert!(live_layout(&v, &NavState::default(), &feed(12), now(), 0, 24).is_empty());
        // h=3: title, the frontier row, the footer.
        let s = render_plain(&live_layout(
            &v,
            &NavState::default(),
            &feed(12),
            now(),
            W,
            3,
        ));
        let rows: Vec<&str> = s.lines().collect();
        assert!(rows[0].starts_with("assess-services"), "{s}");
        assert!(rows[1].contains("hunt · for_each"), "{s}");
    }

    // ---- NavState edges through live_layout ------------------------------------

    #[test]
    fn drilling_into_a_step_with_no_units_renders_without_panicking() {
        let v = graph_view(); // `scan` (index 3) dispatches sub-agents, has no units
        let mut nav = nav_at(&v, 3);
        nav.apply(NavKey::In, &v);
        assert_eq!(nav.depth(), Depth::Step);
        nav.apply(NavKey::In, &v); // nothing to descend into: stays put
        assert_eq!(nav.depth(), Depth::Step);

        let out = live_layout(&v, &nav, &feed(6), now(), W, 24);
        let lines = lines_of(&out);
        let s = render_plain(&out);
        assert!(out.len() <= 24, "{s}");
        // Dashboard (2 rows: no usage yet), then the breadcrumb.
        assert_eq!(lines[2], "scan", "{s}");
        assert!(s.contains("▸ ◐ scan"), "{s}");
        assert!(s.contains("┗━ ◐ lynx#3"), "sub-agents still listed:\n{s}");
        assert_eq!(feed_rows(&out).len(), 6, "{s}");
    }

    #[test]
    fn nav_clamps_when_the_view_shrinks_or_grows_between_calls() {
        let v = graph_view(); // 4 steps
        let mut nav = nav_at(&v, 3); // on the last
        nav.apply(NavKey::In, &v);

        // The next tick's view is shorter. Stale nav must not panic…
        let mut shorter = RunView::default();
        shorter.status = RunStatus::Running;
        start(&mut shorter, "only");
        let stale = live_layout(&shorter, &nav, &feed(6), now(), W, 24);
        assert!(render_plain(&stale).contains("▸ ◐ only"));
        // …and after `sync` the cursor is re-clamped to the real last step.
        nav.sync(&shorter);
        assert_eq!(nav.selected_step(&shorter).unwrap().step_id, "only");
        let out = live_layout(&shorter, &nav, &feed(6), now(), W, 24);
        assert_eq!(lines_of(&out)[2], "only", "{}", render_plain(&out));

        // A view that emptied out entirely is just a dashboard + feed + footer.
        let empty = RunView::default();
        nav.sync(&empty);
        assert_eq!(nav.depth(), Depth::Run);
        let out = live_layout(&empty, &nav, &feed(6), now(), W, 24);
        assert!(out.len() <= 24 && feed_rows(&out).len() == 6);

        // Growth keeps the operator on the same index; the new steps append.
        let mut nav = nav_at(&v, 3);
        let grown = big_run();
        nav.sync(&grown);
        assert_eq!(nav.selected_step(&grown).unwrap().step_id, "stage-03");
        let s = render_plain(&live_layout(&grown, &nav, &feed(6), now(), W, 24));
        assert!(s.contains("▸ ✓ stage-03 "), "{s}");
    }
}
