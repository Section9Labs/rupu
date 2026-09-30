//! Zones of the redesigned live view, as pure functions from a [`RunView`]
//! (plus a [`NavState`]) to styled [`Line`]s — no terminal I/O.
//!
//! * [`dashboard`] — zone 1, the always-visible title / progress / meters.
//! * [`graph`] — zone 2, one row per step with codename, agent, provider/model
//!   and state.
//! * [`fanout_block`] — a `for_each`/`parallel` step as a status density bar +
//!   a few live movers (collapsed) or the filtered unit list (drilled); `graph`
//!   delegates every fan-out step to it.
//!
//! Every datum is shown only when the run has actually produced it (no mock
//! zeros): cost needs a priced run, findings need at least one finding,
//! provider/model need an `AgentStarted`, coverage is omitted entirely until
//! `RunView` carries it.

use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::{RunStatus, StepKind};

use crate::output::live_view::nav::{Depth, NavState, UnitFilter};
use crate::output::live_view::row::Line;
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
    let mut rows = Vec::new();
    for step in &view.steps {
        let marked = focus == Some(step.step_id.as_str());
        if is_fan_out(step) {
            rows.extend(fanout_block(step, nav, marked));
        } else {
            rows.push(step_row(step, marked));
            if marked {
                rows.extend(dispatch_rows(view, step));
            }
        }
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

/// Display width of a fan-out density bar, in columns.
const DENSITY_WIDTH: usize = 20;
/// How many live movers a collapsed fan-out block lists.
const MOVERS: usize = 4;
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
fn printable(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::nav::{NavKey, UnitFilter};
    use crate::output::live_view::row::{render_plain, Style};
    use crate::output::run_model::{RunView, UnitView};
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
        let statuses: Vec<UnitStatus> = (0..86)
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
            .collect();
        fanout_with("hunt", StepKind::ForEach, &statuses)
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
}
