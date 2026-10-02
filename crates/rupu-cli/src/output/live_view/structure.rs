//! The structure pane: the workflow DAG as rails, with live run state painted
//! on top (spec 2026-10-01 §"The structure pane").
//!
//! `rupu-app-canvas` walks a [`Workflow`] into static [`GraphRow`]s — rails,
//! fork/merge glyphs, a bullet and label per node. It knows nothing about a
//! run. This module is the CLI's view layer over those rows:
//!
//! * [`node_status_of`] is the `status_lookup` that colours the rows from a
//!   [`RunView`];
//! * [`structure_rows`] maps every [`GraphCell`] to a styled [`Segment`] and
//!   overlays what the run has actually produced on each node row — the
//!   per-kind leading glyph, codename (role hue), agent, provider/model,
//!   duration and `@host` chip. A datum is shown only when the run has
//!   produced it: a node that has not started carries no overlay at all.
//!
//! A node row reads `<mark><rails…> <glyph> <label> <meta> · <codename> ·
//! <agent> · <provider/model> ···· <state> @host`. Everything after the
//! `LEADER` dots is the right-hand state, kept in its own trailing segments so
//! a width clip can protect it.
//!
//! [`structure_pane`] is the same DAG made to scale — bounded to the `w`×`h`
//! column the dashboard gives it:
//!
//! * a **fan-out** node (`for_each`, a unit-carrying `run:`; `parallel` gets
//!   its aggregate) shows a live density line + the top movers under its rail,
//!   and the filterable unit list when the operator drills into it;
//! * rows are grouped per step ([`Group`]: its connector, node row, lanes /
//!   members / fan-out block, loop frame) and the groups fold: a **settled
//!   split→join phase** to one `⟦ phase ⟧` row, contiguous settled steps to
//!   `✓ a … b (+N done)`, and — only if still needed — steps yet to start to
//!   `○ a … b (+N pending)`;
//! * the frontier and the selection never fold; every row is clipped to `w`
//!   with its right-hand state protected ([`clip_row`]).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use rupu_app_canvas::git_graph::{
    render_rows, BranchGlyph, GraphCell, GraphRow, LOOP_FOOTER_PREFIX, LOOP_HEADER_PREFIX,
};
use rupu_app_canvas::node_status::NodeStatus;
use rupu_orchestrator::workflow::loop_of_step;
use rupu_orchestrator::{is_approval_gate, workflow_edges, Step, Workflow};
use unicode_width::UnicodeWidthStr;

use crate::output::live_view::layout::printable;
use crate::output::live_view::nav::{Depth, NavState, UnitFilter};
use crate::output::live_view::row::{truncate_to, Line, Segment, Style};
use crate::output::palette::Status;
use crate::output::run_model::{fmt_hms, RunView, StepState, StepView, UnitStatus, UnitView};

/// Marker column of the operator-selected node.
const SELECT_MARK: &str = "▸ ";
/// Alignment filler of every unselected row.
const NO_MARK: &str = "  ";
/// A vertical rail.
const PIPE: &str = "│";
/// Dotted leader between a row's label part and its right-hand state
/// (`<label> ···· <state>`). Always its own segment, so a clip can find the
/// state that follows it.
const LEADER: &str = " ···· ";

// ---- status -----------------------------------------------------------------

/// The `status_lookup` closure body for `render_rows`: the node status of the
/// step `step_id` in `view`. A step the run has not reached yet (no
/// [`StepView`]) is `Waiting`.
///
/// A fan-out step that completed with failed units folds to `SoftFailed` — the
/// step finished, but not cleanly — so it never reads as a clean `Complete`.
pub fn node_status_of(view: &RunView, step_id: &str) -> NodeStatus {
    find_step(view, step_id).map_or(NodeStatus::Waiting, step_node_status)
}

fn find_step<'a>(view: &'a RunView, step_id: &str) -> Option<&'a StepView> {
    view.steps.iter().find(|s| s.step_id == step_id)
}

fn step_node_status(step: &StepView) -> NodeStatus {
    match step.state {
        // A paused step is parked, not progressing.
        StepState::Pending | StepState::Paused => NodeStatus::Waiting,
        StepState::Running => NodeStatus::Working,
        StepState::AwaitingApproval => NodeStatus::Awaiting,
        StepState::Complete if step.unit_counts().failed > 0 => NodeStatus::SoftFailed,
        StepState::Complete => NodeStatus::Complete,
        StepState::Failed => NodeStatus::Failed,
        StepState::Skipped => NodeStatus::Skipped,
    }
}

fn unit_node_status(status: UnitStatus) -> NodeStatus {
    match status {
        UnitStatus::Queued => NodeStatus::Waiting,
        UnitStatus::Running => NodeStatus::Working,
        UnitStatus::Done => NodeStatus::Complete,
        UnitStatus::Failed => NodeStatus::Failed,
    }
}

/// App-canvas status → the CLI palette status (glyph + colour). The two enums
/// carry the same nine states.
fn palette_status(ns: NodeStatus) -> Status {
    match ns {
        NodeStatus::Waiting => Status::Waiting,
        NodeStatus::Active => Status::Active,
        NodeStatus::Working => Status::Working,
        NodeStatus::Complete => Status::Complete,
        NodeStatus::Failed => Status::Failed,
        NodeStatus::SoftFailed => Status::SoftFailed,
        NodeStatus::Awaiting => Status::Awaiting,
        NodeStatus::Retrying => Status::Retrying,
        NodeStatus::Skipped => Status::Skipped,
    }
}

/// Style of a rail glyph. Only rails that mean something *now* take a colour
/// — working, awaiting, failed, … — so the live frontier lights up while
/// waiting, finished and skipped structure recedes to dim.
fn rail_style(ns: NodeStatus) -> Style {
    match ns {
        NodeStatus::Waiting | NodeStatus::Complete | NodeStatus::Skipped => Style::Dim,
        live => Style::Status(palette_status(live)),
    }
}

// ---- node kinds -------------------------------------------------------------

/// What a workflow step is, for its leading glyph. Derived from the static
/// [`Step`] — a node the run has not reached has no [`StepView`] to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    /// `agent:` / `run:` — the status bullet is the glyph.
    Agent,
    Gate,
    Action,
    Panel,
    Parallel,
    Split,
    Join,
    Branch,
    ForEach,
}

impl NodeKind {
    /// The kind's own leading glyph; `None` when the status bullet leads.
    fn glyph(self) -> Option<char> {
        match self {
            NodeKind::Agent => None,
            NodeKind::Gate => Some('⏸'),
            NodeKind::Action => Some('◆'),
            NodeKind::Panel => Some('⟲'),
            NodeKind::Parallel => Some('⇉'),
            NodeKind::Split | NodeKind::Join => Some('◈'),
            NodeKind::Branch => Some('◇'),
            NodeKind::ForEach => Some('⊞'),
        }
    }
}

/// The step's kind, with the same precedence `render_rows` dispatches in, so
/// the glyph always agrees with the shape app-canvas drew.
fn node_kind(step: &Step) -> NodeKind {
    [
        (is_approval_gate(step), NodeKind::Gate),
        // Before `for_each`: a `run:` + `for_each:` step is a command node
        // whose units fan out, not a `for_each` node.
        (step.run.is_some(), NodeKind::Agent),
        (step.action.is_some(), NodeKind::Action),
        (step.panel.is_some(), NodeKind::Panel),
        (step.parallel.is_some(), NodeKind::Parallel),
        (step.split.is_some(), NodeKind::Split),
        (step.join.is_some(), NodeKind::Join),
        (step.branch.is_some(), NodeKind::Branch),
        (step.for_each.is_some(), NodeKind::ForEach),
    ]
    .into_iter()
    .find_map(|(hit, kind)| hit.then_some(kind))
    .unwrap_or(NodeKind::Agent)
}

// ---- per-row facts ------------------------------------------------------------

/// What the run says about one row, resolved before the row is painted.
/// Every field is `None` / empty until the run has produced it.
#[derive(Default)]
struct Facts<'a> {
    /// The operator's selected node: its row gets the `▸` marker and a
    /// `Strong` label.
    selected: bool,
    /// The node kind's own leading glyph (replaces the status bullet).
    lead: Option<char>,
    /// Status shown by the node's own glyph cells when it is not the one
    /// app-canvas baked in: a panelist's comes from its unit, a loop frame's
    /// from the loop node.
    status: Option<NodeStatus>,
    /// With `status`: every rail cell of the row takes it (loop frame rows),
    /// not just the node's own glyph cells.
    tint_rails: bool,
    /// The row's label recedes (a loop's loop-back footer).
    dim_label: bool,
    codename: Option<&'a str>,
    agent: Option<&'a str>,
    provider: Option<&'a str>,
    model: Option<&'a str>,
    host: Option<&'a str>,
    /// Short note beside the identity, e.g. a panel's `round 1/3`.
    detail: Option<String>,
    /// The right-hand state: status plus its text (a duration, `running`, …).
    state: Option<(NodeStatus, String)>,
    /// How many `StepWarning`s the step has. Rides beside `state` as `⚠` /
    /// `⚠N` — a notice, never a status.
    warnings: usize,
}

fn state_of_step(step: &StepView) -> Option<(NodeStatus, String)> {
    let text = match step.state {
        StepState::Pending => return None,
        StepState::Paused => "paused".to_string(),
        StepState::Running => "running".to_string(),
        StepState::AwaitingApproval => "awaiting".to_string(),
        StepState::Complete => step.duration_ms.map_or_else(|| "done".to_string(), fmt_hms),
        StepState::Failed => "failed".to_string(),
        StepState::Skipped => "skipped".to_string(),
    };
    Some((step_node_status(step), text))
}

fn state_of_unit(unit: &UnitView) -> (NodeStatus, String) {
    let text = match unit.status {
        UnitStatus::Queued => "queued",
        UnitStatus::Running => "running",
        UnitStatus::Done => "done",
        UnitStatus::Failed => "failed",
    };
    (unit_node_status(unit.status), text.to_string())
}

/// `round r/max` of a panel mid-gate-loop; `None` before its first round.
fn round_label(step: &StepView) -> Option<String> {
    match (step.panel_round, step.panel_max) {
        (Some(r), Some(max)) => Some(format!("round {r}/{max}")),
        (Some(r), None) => Some(format!("round {r}")),
        (None, _) => None,
    }
}

/// Facts of a workflow step row, from its static kind and — once the run has
/// reached it — its [`StepView`].
fn facts_of_step<'a>(step: &Step, live: Option<&'a StepView>) -> Facts<'a> {
    let kind = node_kind(step);
    let mut facts = Facts {
        lead: kind.glyph(),
        ..Facts::default()
    };
    if let Some(sv) = live {
        facts.codename = sv.codename.as_deref();
        facts.agent = sv.agent.as_deref();
        facts.provider = sv.provider.as_deref();
        facts.model = sv.model.as_deref();
        facts.host = sv.host.as_deref();
        facts.detail = round_label(sv).filter(|_| kind == NodeKind::Panel);
        facts.state = state_of_step(sv);
        facts.warnings = sv.warnings.len();
    }
    facts
}

/// The panel / parallel block whose member rows are being walked.
/// app-canvas emits a block's members as the anchored rows right after the
/// block's header — one per panelist / sub-step, in declared order — and
/// anchors them on the member's agent / sub-step id, which is not a step id.
struct Block<'a> {
    live: Option<&'a StepView>,
    /// `(anchor id, agent)` of each member, in declared order.
    members: Vec<(&'a str, &'a str)>,
    /// Members are `parallel:` sub-steps, whose unit index is their declared
    /// position — not panelists, which are matched by agent.
    by_index: bool,
    taken: usize,
}

impl<'a> Block<'a> {
    /// A block for `step`, or `None` when `step` has no member rows.
    fn open(step: &'a Step, live: Option<&'a StepView>) -> Option<Self> {
        let (members, by_index): (Vec<(&str, &str)>, bool) = match (&step.panel, &step.parallel) {
            (Some(panel), _) => (
                panel
                    .panelists
                    .iter()
                    .map(|p| (p.as_str(), p.as_str()))
                    .collect(),
                false,
            ),
            (None, Some(subs)) => (
                subs.iter()
                    .map(|s| (s.id.as_str(), s.agent.as_str()))
                    .collect(),
                true,
            ),
            (None, None) => return None,
        };
        Some(Self {
            live,
            members,
            by_index,
            taken: 0,
        })
    }

    /// The declared position of the next member row, if the row anchored on
    /// `anchor` is that member. Anything else — the members exhausted, or a
    /// row that is not the one app-canvas should emit next — is not a member,
    /// so a drift in app-canvas's block shape degrades to plain rows, never
    /// to another node wearing a member's status.
    fn next_member(&mut self, anchor: &str) -> Option<usize> {
        let (id, _) = self.members.get(self.taken)?;
        if *id != anchor {
            return None;
        }
        self.taken += 1;
        Some(self.taken - 1)
    }

    /// Facts of the member row at declared position `pos`. Every member is a
    /// fan-out unit of its step, so its status and identity come from that
    /// unit: a `parallel:` sub-step's unit index is its declared position; a
    /// panelist is the newest unit of its agent (a later gate round has a
    /// higher index). A member with no unit yet keeps the status app-canvas
    /// baked in — except under a skipped parent, which skipped it too.
    fn member_facts(&self, pos: usize) -> Facts<'a> {
        let agent = self.members.get(pos).map_or("", |(_, agent)| *agent);
        let unit = self.live.and_then(|step| {
            if self.by_index {
                step.units.get(&pos)
            } else {
                step.units
                    .values()
                    .rev()
                    .find(|u| u.agent.as_deref() == Some(agent))
            }
        });
        if let Some(u) = unit {
            return Facts {
                status: Some(unit_node_status(u.status)),
                codename: u.codename.as_deref(),
                provider: u.provider.as_deref(),
                model: u.model.as_deref(),
                host: u.host.as_deref(),
                state: Some(state_of_unit(u)),
                ..Facts::default()
            };
        }
        let skipped = self.live.is_some_and(|s| s.state == StepState::Skipped);
        Facts {
            status: skipped.then_some(NodeStatus::Skipped),
            ..Facts::default()
        }
    }
}

/// One of a loop frame's two unanchored rows.
enum LoopFrame<'a> {
    Header(&'a str),
    Footer,
}

/// Recognise a loop frame row. A loop is not a step, so the frame rows carry
/// no anchor; they are told apart by their label's prefix.
fn loop_frame(row: &GraphRow) -> Option<LoopFrame<'_>> {
    if row.anchor.is_some() {
        return None;
    }
    row.cells.iter().find_map(|cell| match cell {
        GraphCell::Label(text) => text
            .strip_prefix(LOOP_HEADER_PREFIX)
            .map(LoopFrame::Header)
            .or_else(|| {
                text.starts_with(LOOP_FOOTER_PREFIX)
                    .then_some(LoopFrame::Footer)
            }),
        _ => None,
    })
}

/// Facts of the `↻ loop:<name>` header: the loop node's own state, and the
/// iteration the members have reached (`iter n/max`). The loop node is the
/// synthetic `loop:<name>` step the runner emits events for.
fn facts_of_loop<'a>(view: &'a RunView, wf: &Workflow, name: &str) -> Facts<'a> {
    let node = view
        .steps
        .iter()
        .find(|s| s.step_id.strip_prefix("loop:") == Some(name));
    let iteration = wf.loops.get(name).and_then(|def| {
        let reached = def
            .nodes
            .iter()
            .filter_map(|id| find_step(view, id)?.loop_iteration)
            .max()?;
        Some(format!("iter {reached}/{}", def.max_iterations))
    });
    // While the loop runs, its iteration is the news; once settled, its state.
    let state = match (node, iteration) {
        (Some(n), Some(iter)) if n.state == StepState::Running => Some((NodeStatus::Working, iter)),
        (Some(n), _) => state_of_step(n),
        (None, Some(iter)) => Some((NodeStatus::Working, iter)),
        (None, None) => None,
    };
    Facts {
        status: Some(state.as_ref().map_or(NodeStatus::Waiting, |(ns, _)| *ns)),
        tint_rails: true,
        host: node.and_then(|n| n.host.as_deref()),
        state,
        ..Facts::default()
    }
}

// ---- painting -----------------------------------------------------------------

/// Append `text` in `style`. `Line`'s builders cover the named styles; a
/// computed one (rails, tails) goes through here.
fn styled(mut line: Line, style: Style, text: impl Into<String>) -> Line {
    line.segments.push(Segment {
        text: text.into(),
        style,
    });
    line
}

/// A member codename as a row part: the crew prefix is dropped (the header
/// carries the crew) and each `>`-chained segment's role word is emitted as
/// `Style::Role`, with its `#n` / `.attempt` suffix plain. `None` for a value
/// that is not codename-shaped — it comes off the wire, so anything outside
/// `[A-Za-z0-9#.>]` (control characters included) is never echoed.
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

/// The identity parts that follow a row's label, each its own [`Line`]:
/// `detail`, `codename`, `agent`, `provider/model`. `shown` holds the row's
/// meta texts so an agent app-canvas already printed there is not repeated.
fn identity_parts(facts: &Facts<'_>, shown: &[&str]) -> Vec<Line> {
    // An empty string is "not produced" (it would print as a bare ` · `).
    let clean = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(printable);
    let mut parts = Vec::new();
    if let Some(detail) = &facts.detail {
        parts.push(Line::new().dim(printable(detail)));
    }
    if let Some(c) = facts.codename.and_then(codename_line) {
        parts.push(c);
    }
    if let Some(agent) = clean(facts.agent).filter(|a| !shown.contains(&a.as_str())) {
        parts.push(Line::new().plain(agent));
    }
    match (clean(facts.provider), clean(facts.model)) {
        (Some(p), Some(m)) => parts.push(Line::new().dim(format!("{p}/{m}"))),
        (Some(x), None) | (None, Some(x)) => parts.push(Line::new().dim(x)),
        (None, None) => {}
    }
    parts
}

/// Paint one app-canvas row. `gutter` is the live status of the loop frame
/// the row sits inside, if any: member rows lead with the frame's `│` gutter.
fn paint(row: &GraphRow, facts: &Facts<'_>, gutter: Option<NodeStatus>) -> Line {
    let mut line = if facts.selected {
        Line::new().strong(SELECT_MARK)
    } else {
        Line::new().dim(NO_MARK)
    };
    // The row's own status: what colours the node's glyph.
    let row_status = facts
        .status
        .or_else(|| row.anchor_status())
        .unwrap_or(NodeStatus::Waiting);
    let mut seen_bullet = false;
    let mut shown: Vec<&str> = Vec::new();

    for (i, cell) in row.cells.iter().enumerate() {
        match cell {
            GraphCell::Pipe(ns) => {
                let ns = match (facts.status.filter(|_| facts.tint_rails), gutter) {
                    (Some(tint), _) => tint,
                    (None, Some(frame)) if i == 0 => frame,
                    (None, _) => *ns,
                };
                line = styled(line, rail_style(ns), PIPE);
            }
            GraphCell::Branch(glyph, ns) => {
                // Cells after the bullet belong to the node itself (a
                // panelist's `●─`); a loop frame row tints them all.
                let own = seen_bullet || facts.tint_rails;
                let ns = facts.status.filter(|_| own).unwrap_or(*ns);
                line = styled(line, rail_style(ns), glyph.as_str());
            }
            GraphCell::Bullet(ns) => {
                seen_bullet = true;
                let ns = facts.status.unwrap_or(*ns);
                let st = palette_status(ns);
                line = line.status(st, facts.lead.unwrap_or_else(|| st.glyph()).to_string());
            }
            GraphCell::Space(n) => line = line.plain(" ".repeat(usize::from(*n))),
            GraphCell::Label(text) => {
                // A header row with no bullet to replace (panel, parallel,
                // for_each) takes its kind glyph just before the label.
                if let Some(glyph) = facts.lead.filter(|_| !seen_bullet) {
                    let st = palette_status(row_status);
                    line = line.status(st, format!("{glyph} "));
                }
                let style = match (facts.dim_label, facts.selected) {
                    (true, _) => Style::Dim,
                    (false, true) => Style::Strong,
                    (false, false) => Style::Plain,
                };
                line = styled(line, style, printable(text));
            }
            GraphCell::Meta(text) => {
                shown.push(text);
                line = line.dim(printable(text));
            }
        }
    }

    for (i, part) in identity_parts(facts, &shown).into_iter().enumerate() {
        line = match (i, shown.is_empty()) {
            (0, true) => line.plain("  "),
            _ => line.dim(" · "),
        };
        line.segments.extend(part.segments);
    }
    right_hand_state(line, facts)
}

/// `···· <glyph> <state> ⚠N @host` — the protected tail of a node row; absent
/// when the run has nothing to say about the node yet. The `⚠` (with its count
/// past the first) sits beside the state, in the warning tone: the step's own
/// state is untouched.
fn right_hand_state(mut line: Line, facts: &Facts<'_>) -> Line {
    let host = facts.host.filter(|h| !h.is_empty());
    if facts.state.is_none() && host.is_none() && facts.warnings == 0 {
        return line;
    }
    line = line.dim(LEADER);
    if let Some((ns, text)) = &facts.state {
        let st = palette_status(*ns);
        line = line.status(st, format!("{} {}", st.glyph(), printable(text)));
    }
    if facts.warnings > 0 {
        let gap = if facts.state.is_some() { " " } else { "" };
        let marker = match facts.warnings {
            1 => "⚠".to_string(),
            n => format!("⚠{n}"),
        };
        line = line.status(Status::SoftFailed, format!("{gap}{marker}"));
    }
    if let Some(host) = host {
        let gap = if facts.state.is_some() || facts.warnings > 0 {
            " "
        } else {
            ""
        };
        line = line.dim(format!("{gap}@{}", printable(host)));
    }
    line
}

/// What a painted row is, for grouping (see [`build_groups`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role<'a> {
    /// A top-level step's own row.
    Head(&'a str),
    /// A panelist / sub-step row of the block the preceding head opened.
    Member,
    /// A loop frame's `↻` header.
    LoopHeader,
    /// Every other unanchored row: connectors, lanes, spacers, closers,
    /// placeholders, the loop-back footer.
    Other,
}

/// One painted app-canvas row plus what grouping needs to know about it.
struct Painted<'a> {
    line: Line,
    role: Role<'a>,
    /// Live status of the loop frame the row sits inside, if any.
    gutter: Option<NodeStatus>,
}

/// Paint every app-canvas row of `wf` from `view`, one [`Painted`] per row.
/// `expand` lets a fan-out header that the operator has drilled into carry
/// its unit filter (the pane then lists the filtered units under it).
fn paint_rows<'a>(
    view: &RunView,
    wf: &Workflow,
    nav: &NavState,
    rows: &'a [GraphRow],
    expand: bool,
) -> Vec<Painted<'a>> {
    let chosen = nav.chosen_step(view).map(|s| s.step_id.as_str());

    let mut block: Option<Block<'_>> = None;
    let mut gutter: Option<NodeStatus> = None;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let frame = loop_frame(row);
        let (facts, role) = match (&frame, row.anchor_step_id()) {
            (Some(LoopFrame::Header(name)), _) => {
                let mut facts = facts_of_loop(view, wf, name);
                // The loop is a step of its own (`loop:<name>`) to the
                // navigator, but has no anchored row: its header is it.
                facts.selected = chosen.and_then(|c| c.strip_prefix("loop:")) == Some(*name);
                (facts, Role::LoopHeader)
            }
            (Some(LoopFrame::Footer), _) => (
                Facts {
                    status: Some(gutter.unwrap_or(NodeStatus::Waiting)),
                    tint_rails: true,
                    dim_label: true,
                    ..Facts::default()
                },
                Role::Other,
            ),
            (None, None) => (Facts::default(), Role::Other),
            (None, Some(id)) => match block.as_mut().and_then(|b| b.next_member(id)) {
                Some(pos) => (
                    block
                        .as_ref()
                        .map_or_else(Facts::default, |b| b.member_facts(pos)),
                    Role::Member,
                ),
                None => {
                    let step = wf.steps.iter().find(|s| s.id == id);
                    let live = find_step(view, id);
                    block = step.and_then(|s| Block::open(s, live));
                    let mut facts = step.map_or_else(Facts::default, |s| facts_of_step(s, live));
                    facts.selected = chosen == Some(id);
                    let lists_units = step.is_some_and(|s| {
                        matches!(
                            fan_shape(s, live),
                            Some(FanShape::ForEach | FanShape::AfterRow)
                        )
                    });
                    if expand && facts.selected && nav.depth() != Depth::Run && lists_units {
                        facts.detail = filter_label(nav.filter()).map(|f| format!("filter: {f}"));
                    }
                    (facts, Role::Head(id))
                }
            },
        };
        out.push(Painted {
            line: paint(row, &facts, gutter),
            role,
            gutter,
        });
        match frame {
            Some(LoopFrame::Header(_)) => gutter = facts.status,
            Some(LoopFrame::Footer) => gutter = None,
            None => {}
        }
    }
    out
}

/// The structure pane's rows: every app-canvas row of `wf`, coloured and
/// overlaid from `view`. `nav`'s chosen step gets the `▸` marker. One line
/// per row, unclipped, nothing folded — [`structure_pane`] is the bounded,
/// collapsing view of the same rows.
pub fn structure_rows(view: &RunView, wf: &Workflow, nav: &NavState) -> Vec<Line> {
    let rows = render_rows(wf, |id| node_status_of(view, id));
    paint_rows(view, wf, nav, &rows, false)
        .into_iter()
        .map(|p| p.line)
        .collect()
}

// ---- fan-out density ------------------------------------------------------------

/// Display width of a fan-out density bar, in columns.
const DENSITY_WIDTH: usize = 20;
/// How many live movers a collapsed fan-out lists.
const MOVERS: usize = 4;

/// How a fan-out node's live block sits among app-canvas's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FanShape {
    /// `for_each:` — app-canvas frames a static `○ runtime items`
    /// placeholder in rails; the live block replaces it.
    ForEach,
    /// `parallel:` — its sub-steps are already member rows, so only the
    /// aggregate density line is added (in place of the first spacer).
    Parallel,
    /// A single-row node that has produced units (a `run:` + `for_each:`
    /// command): the block hangs under its row.
    AfterRow,
}

/// The shape of `step`'s live fan-out block; `None` while it has produced no
/// units — nothing is invented, the static rows stand — and for nodes whose
/// units are shown some other way (a panel's panelists).
fn fan_shape(step: &Step, live: Option<&StepView>) -> Option<FanShape> {
    let has_units = live.is_some_and(|s| !s.units.is_empty());
    match node_kind(step) {
        NodeKind::ForEach if has_units => Some(FanShape::ForEach),
        NodeKind::Parallel if has_units => Some(FanShape::Parallel),
        NodeKind::Agent if has_units => Some(FanShape::AfterRow),
        _ => None,
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

/// `<bar> <done>/<total> ✓<done> ◐<running> ✗<failed> ○<queued>` — a status
/// count appears only when non-zero; every number from `unit_counts()`.
/// Units carry no per-unit spend, so the row has no `⇡tokens $cost` tail
/// rather than a made-up one.
fn density_tail(step: &StepView) -> Line {
    let c = step.unit_counts();
    let filled = density_fill(c.done, c.total);
    let mut line = Line::new();
    // Only non-empty runs: a zero-width styled segment would still wrap
    // nothing in colour codes.
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

/// Up to [`MOVERS`] units to surface while the node is collapsed: the newest
/// running units (highest indices — units start in index order), in index
/// order, then — when fewer than that are running — the highest-index of the
/// rest. `UnitView` carries no timestamps, so index order stands in for
/// recency.
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

/// Where a fan-out's rows hang: the cells that lead each kind of row, so the
/// block nests under the node's rails (and its loop's gutter).
struct Scaffold {
    /// Leads a unit row, up to its status bullet.
    lead: Vec<GraphCell>,
    /// Leads the density / `more` rows: `lead`'s columns, then the inner rail.
    bar: Vec<GraphCell>,
    /// Live status of the loop frame the node sits inside, if any.
    gutter: Option<NodeStatus>,
}

/// The leading `│` / space cells of a row: the loop gutter and outer rails.
fn leading_rails(cells: &[GraphCell]) -> Vec<GraphCell> {
    cells
        .iter()
        .take_while(|c| matches!(c, GraphCell::Pipe(_) | GraphCell::Space(_)))
        .cloned()
        .collect()
}

/// An unanchored row of nothing but rails and spaces (a spacer).
fn is_rails_only(row: &GraphRow) -> bool {
    row.anchor.is_none()
        && row
            .cells
            .iter()
            .all(|c| matches!(c, GraphCell::Pipe(_) | GraphCell::Space(_)))
}

impl Scaffold {
    /// A row that is only the scaffold's `bar` cells followed by `tail`.
    fn bar_row(&self, tail: Line) -> Line {
        let row = GraphRow {
            cells: self.bar.clone(),
            anchor: None,
        };
        let mut line = paint(&row, &Facts::default(), self.gutter);
        line.segments.extend(tail.segments);
        line
    }

    /// One unit as a member row — `◐├─ <key>  <codename> · <provider/model>
    /// ···· ◐ running` — painted like every other node row, so its status
    /// glyph, identity and protected tail follow the same rules.
    fn unit_row(&self, unit: &UnitView, marked: bool) -> Line {
        let ns = unit_node_status(unit.status);
        let key = if unit.unit_key.is_empty() {
            format!("unit {}", unit.index)
        } else {
            unit.unit_key.clone()
        };
        let mut cells = self.lead.clone();
        cells.extend([
            GraphCell::Bullet(ns),
            GraphCell::Branch(BranchGlyph::Mid, ns),
            GraphCell::Space(1),
            GraphCell::Label(key),
        ]);
        let facts = Facts {
            selected: marked,
            status: Some(ns),
            codename: unit.codename.as_deref(),
            provider: unit.provider.as_deref(),
            model: unit.model.as_deref(),
            host: unit.host.as_deref(),
            state: Some(state_of_unit(unit)),
            ..Facts::default()
        };
        paint(
            &GraphRow {
                cells,
                anchor: None,
            },
            &facts,
            self.gutter,
        )
    }
}

/// A fan-out's live rows, ready to splice into its group.
struct FanBlock {
    /// The density line — always shown.
    density: Line,
    /// The mover / unit rows (and the `more` note) beneath it: what a tight
    /// pane may cut.
    body: Vec<Line>,
    /// Index into `body` of the unit the operator's cursor is on (expanded).
    cursor: Option<usize>,
}

/// The rows of `step`'s fan-out. *Collapsed*: the density line, up to
/// [`MOVERS`] live movers and `… +K more`. *Drilled*: the density line —
/// still the whole step's counts — and every unit `nav`'s filter admits, the
/// cursor unit marked. All numbers come from [`StepView::unit_counts`].
fn fan_block(scaffold: &Scaffold, step: &StepView, nav: &NavState, drilled: bool) -> FanBlock {
    let density = scaffold.bar_row(density_tail(step));
    let note = |text: String| scaffold.bar_row(Line::new().dim(text));
    if !drilled {
        let movers = live_movers(step);
        let hidden = step.units.len().saturating_sub(movers.len());
        let mut body: Vec<Line> = movers.iter().map(|u| scaffold.unit_row(u, false)).collect();
        if hidden > 0 {
            body.push(note(format!("… +{hidden} more · [enter] expand")));
        }
        return FanBlock {
            density,
            body,
            cursor: None,
        };
    }
    let units = nav.filtered_units(step);
    if units.is_empty() {
        let text = match filter_label(nav.filter()) {
            Some(f) => format!("no {f} units"),
            None => "no units".to_string(),
        };
        return FanBlock {
            density,
            body: vec![note(text)],
            cursor: None,
        };
    }
    let at = nav.selected_unit_in(step).map(|u| u.index);
    FanBlock {
        density,
        body: units
            .iter()
            .map(|u| scaffold.unit_row(u, at == Some(u.index)))
            .collect(),
        cursor: units.iter().position(|u| Some(u.index) == at),
    }
}

// ---- groups -----------------------------------------------------------------------

/// How a group is treated when the pane must shrink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Running or awaiting approval — the live frontier. Never collapsed.
    Frontier,
    /// Failed, paused, warned, selected, or part of a loop still in play.
    /// Never collapsed.
    Keep,
    /// Complete or skipped, and nothing above. Folds first.
    Settled,
    /// Not started. Folds only once every settled step has.
    Pending,
}

/// The trimmable part of a fan-out group, and what survives around it.
struct Fan {
    /// The mover / unit rows trimming may cut. Everything before it (the
    /// leading rows, the node row, the density line) and after it (a
    /// `for_each` frame's closing rail) survives.
    body: Range<usize>,
    /// Row (in the group) of the unit cursor, when the list is drilled.
    cursor: Option<usize>,
    /// The unit list is drilled (not the collapsed movers).
    drilled: bool,
    /// Leads the `⋮ +N above · +M below` row of a windowed list.
    marker: Line,
}

/// One step's rows — an anchored node row plus everything that belongs to it
/// — and what the fitter needs to know about them.
///
/// A group is `[leading rows] [node row] [trailing rows]`: the leading rows
/// are the connector `│` that precedes the node (absent for the first) and a
/// loop frame's `↻` header when the node opens one; the trailing rows are its
/// lanes, spacers, members, fan-out block and a loop-back footer. A group is
/// the unit that folds, so a fold never strands a half of a construct.
struct Group {
    rows: Vec<Line>,
    /// Leading rows before the node row.
    lead: usize,
    /// The first row is a connector `│` that survives folding the group.
    spine: bool,
    id: String,
    state: Option<StepState>,
    /// Complete, but some of its fan-out units failed.
    soft_failed: bool,
    class: Class,
    loop_name: Option<String>,
    fan: Option<Fan>,
}

impl Group {
    /// Rows no trimming may remove.
    fn min_rows(&self) -> usize {
        self.rows.len() - self.fan.as_ref().map_or(0, |f| f.body.len())
    }
}

fn class_of(view: &RunView, id: &str, live: Option<&StepView>, selected: bool) -> Class {
    let warned = live.is_some_and(|s| !s.warnings.is_empty());
    let below_running =
        live.is_some_and(|s| s.units.values().any(|u| u.status == UnitStatus::Running))
            || view.dispatches.values().any(|d| {
                d.status == UnitStatus::Running && d.parent_step_id.as_deref() == Some(id)
            });
    match live.map(|s| s.state) {
        Some(StepState::Running | StepState::AwaitingApproval) => Class::Frontier,
        _ if below_running => Class::Frontier,
        _ if selected => Class::Keep,
        // A warning is only visible as its row's `⚠`: folding the step into a
        // `✓ a … b (+N done)` summary would hide it.
        Some(StepState::Complete | StepState::Skipped) if warned => Class::Keep,
        Some(StepState::Complete | StepState::Skipped) => Class::Settled,
        None | Some(StepState::Pending) => Class::Pending,
        Some(StepState::Failed | StepState::Paused) => Class::Keep,
    }
}

/// A group's app-canvas rows, as the fan-out splice needs to see them.
#[derive(Clone, Copy)]
struct GroupRows<'a> {
    /// The group's rows; `lines` (the painted ones) are parallel to them.
    rows: &'a [GraphRow],
    /// Leading rows (connector, loop header) before the node row.
    lead: usize,
    /// Live status of the loop frame the group sits inside, if any.
    gutter: Option<NodeStatus>,
}

/// Splice `shape`'s live block into the group's painted `lines`. `None` when
/// the rows do not have the expected frame — they then stay as painted, never
/// half-replaced — and for a block that has nothing trimmable.
fn splice_fan_out(
    shape: FanShape,
    live: &StepView,
    nav: &NavState,
    drilled: bool,
    group: &GroupRows<'_>,
    lines: &mut Vec<Line>,
) -> Option<Fan> {
    let GroupRows { rows, lead, gutter } = *group;
    let node = rows.get(lead)?;
    let node_status = node.anchor_status().unwrap_or(NodeStatus::Waiting);
    match shape {
        FanShape::ForEach => {
            let at = rows.iter().position(|r| {
                r.anchor.is_none() && r.cells.iter().any(|c| matches!(c, GraphCell::Bullet(_)))
            })?;
            let lead_cells = leading_rails(&rows[at].cells);
            let mut bar = lead_cells.clone();
            bar.extend([GraphCell::Pipe(node_status), GraphCell::Space(1)]);
            let scaffold = Scaffold {
                lead: lead_cells,
                bar,
                gutter,
            };
            let block = fan_block(&scaffold, live, nav, drilled);
            // The placeholder and the spacers around it give way to the block.
            let from = if at > 0 && is_rails_only(&rows[at - 1]) {
                at - 1
            } else {
                at
            };
            let to = if rows.get(at + 1).is_some_and(is_rails_only) {
                at + 2
            } else {
                at + 1
            };
            let body = from + 1..from + 1 + block.body.len();
            let marker = scaffold.bar_row(Line::new());
            let cursor = block.cursor.map(|c| body.start + c);
            lines.splice(from..to, std::iter::once(block.density).chain(block.body));
            Some(Fan {
                body,
                cursor,
                drilled,
                marker,
            })
        }
        FanShape::Parallel => {
            // Its sub-steps are the units; only the aggregate is new. It
            // takes the place of the first spacer under the header.
            let spacer = rows.get(lead + 1).filter(|r| is_rails_only(r))?;
            let mut bar = spacer.cells.clone();
            bar.push(GraphCell::Space(1));
            let scaffold = Scaffold {
                lead: Vec::new(),
                bar,
                gutter,
            };
            lines[lead + 1] = scaffold.bar_row(density_tail(live));
            None
        }
        FanShape::AfterRow => {
            let mut lead_cells = leading_rails(&node.cells);
            lead_cells.push(GraphCell::Space(2));
            let mut bar = lead_cells.clone();
            bar.push(GraphCell::Space(2));
            let scaffold = Scaffold {
                lead: lead_cells,
                bar,
                gutter,
            };
            let block = fan_block(&scaffold, live, nav, drilled);
            let body = lead + 2..lead + 2 + block.body.len();
            let marker = scaffold.bar_row(Line::new());
            let cursor = block.cursor.map(|c| body.start + c);
            lines.splice(
                lead + 1..lead + 1,
                std::iter::once(block.density).chain(block.body),
            );
            Some(Fan {
                body,
                cursor,
                drilled,
                marker,
            })
        }
    }
}

/// Fold the painted rows into per-step [`Group`]s (see [`Group`] for the
/// anatomy), with each fan-out node's live block spliced in.
fn build_groups(
    view: &RunView,
    wf: &Workflow,
    nav: &NavState,
    rows: &[GraphRow],
    painted: Vec<Painted<'_>>,
) -> Vec<Group> {
    let chosen = nav.chosen_step(view).map(|s| s.step_id.as_str());
    let chosen_loop = chosen.and_then(|c| c.strip_prefix("loop:"));
    let heads: Vec<usize> = painted
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.role, Role::Head(_)))
        .map(|(i, _)| i)
        .collect();
    // A group opens with the connector that precedes its node (and a loop
    // header after it); the first group also owns whatever precedes it.
    let starts: Vec<usize> = heads
        .iter()
        .enumerate()
        .map(|(k, &head)| {
            let lead = match k {
                0 => head,
                _ => {
                    let gap = head - heads[k - 1] - 1;
                    let header = head > 0 && painted[head - 1].role == Role::LoopHeader;
                    gap.min(1 + usize::from(header))
                }
            };
            head - lead
        })
        .collect();

    let mut painted = painted.into_iter();
    let mut groups = Vec::with_capacity(heads.len());
    for (k, &head) in heads.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(rows.len());
        let span = starts[k]..end;
        let lead = head - starts[k];
        let taken: Vec<Painted<'_>> = painted.by_ref().take(span.len()).collect();
        let Some(&Painted {
            role: Role::Head(id),
            gutter,
            ..
        }) = taken.get(lead)
        else {
            continue;
        };
        let mut lines: Vec<Line> = taken.into_iter().map(|p| p.line).collect();

        let step = wf.steps.iter().find(|s| s.id == id);
        let live = find_step(view, id);
        let loop_name = loop_of_step(wf, id).map(str::to_string);
        let selected =
            chosen == Some(id) || (chosen_loop.is_some() && chosen_loop == loop_name.as_deref());
        let drilled = chosen == Some(id) && nav.depth() != Depth::Run;
        let fan = step
            .and_then(|s| fan_shape(s, live))
            .zip(live)
            .and_then(|(shape, sv)| {
                let group = GroupRows {
                    rows: &rows[span.clone()],
                    lead,
                    gutter,
                };
                splice_fan_out(shape, sv, nav, drilled, &group, &mut lines)
            });
        groups.push(Group {
            rows: lines,
            lead,
            spine: k > 0 && lead >= 1,
            id: id.to_string(),
            state: live.map(|s| s.state),
            soft_failed: live
                .is_some_and(|s| s.state == StepState::Complete && s.unit_counts().failed > 0),
            class: class_of(view, id, live, selected),
            loop_name,
            fan,
        });
    }
    settle_loops(view, &mut groups);
    groups
}

/// A loop frame folds whole or not at all: while any member is still in play,
/// or the loop itself has not settled (between iterations every member can
/// read `Complete` while the loop is the running thing), its settled members
/// stay visible.
fn settle_loops(view: &RunView, groups: &mut [Group]) {
    let names: BTreeSet<String> = groups.iter().filter_map(|g| g.loop_name.clone()).collect();
    for name in names {
        let node_open = view.steps.iter().any(|s| {
            s.step_id.strip_prefix("loop:") == Some(name.as_str())
                && !matches!(s.state, StepState::Complete | StepState::Skipped)
        });
        let mut members: Vec<&mut Group> = groups
            .iter_mut()
            .filter(|g| g.loop_name.as_deref() == Some(name.as_str()))
            .collect();
        if node_open || members.iter().any(|g| g.class != Class::Settled) {
            for g in &mut members {
                if g.class == Class::Settled {
                    g.class = Class::Keep;
                }
            }
        }
    }
}

// ---- phases -----------------------------------------------------------------------

/// Every id reachable from `from` over `adj` (`from` itself only on a cycle).
fn reach<'a>(adj: &BTreeMap<&'a str, Vec<&'a str>>, from: &'a str) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(id) = stack.pop() {
        for &next in adj.get(id).into_iter().flatten() {
            if seen.insert(next) {
                stack.push(next);
            }
        }
    }
    seen
}

/// No loop is split by the span: every loop that has a member inside has all
/// of them inside, so folding the span never cuts a loop frame in two.
fn loops_inside(wf: &Workflow, span: &[Step]) -> bool {
    let ids: BTreeSet<&str> = span.iter().map(|s| s.id.as_str()).collect();
    wf.loops.values().all(|def| {
        let inside = def
            .nodes
            .iter()
            .filter(|n| ids.contains(n.as_str()))
            .count();
        inside == 0 || inside == def.nodes.len()
    })
}

/// The `(split, join)` step indices of every clean split→join sub-DAG of
/// `wf`, in step order.
///
/// A split `S` and the first later join `J` it reaches form a phase only when
/// the sub-DAG is exactly the steps declared between them — everything
/// reachable from `S` that can reach `J` lies between the two in step order,
/// and everything between them is such a step. Rows are emitted in step
/// order, so that is what makes the span contiguous and foldable; anything
/// less tidy (an unrelated step declared inside, a branch escaping to the
/// outside, a loop the span cuts through) is not a phase and renders as
/// ordinary steps. A nested split pairs with the nearer join; the enclosing
/// split then finds the next one that closes it.
fn phase_spans(wf: &Workflow, groups: &[Group]) -> Vec<(usize, usize)> {
    // Groups map 1:1 onto steps in order, or there is nothing safe to say.
    let aligned =
        groups.len() == wf.steps.len() && groups.iter().zip(&wf.steps).all(|(g, s)| g.id == s.id);
    if !aligned || wf.steps.iter().all(|s| s.split.is_none()) {
        return Vec::new();
    }
    let edges = workflow_edges(wf);
    let mut succ: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut pred: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (a, b) in &edges {
        succ.entry(a.as_str()).or_default().push(b.as_str());
        pred.entry(b.as_str()).or_default().push(a.as_str());
    }
    let mut spans = Vec::new();
    for (i, step) in wf.steps.iter().enumerate() {
        if step.split.is_none() {
            continue;
        }
        let forward = reach(&succ, &step.id);
        let close = (i + 1..wf.steps.len()).find(|&j| {
            let join = &wf.steps[j];
            if join.join.is_none() || !forward.contains(join.id.as_str()) {
                return false;
            }
            let backward = reach(&pred, &join.id);
            let region: BTreeSet<&str> = forward.intersection(&backward).copied().collect();
            let between: BTreeSet<&str> =
                wf.steps[i + 1..j].iter().map(|s| s.id.as_str()).collect();
            region == between && loops_inside(wf, &wf.steps[i..=j])
        });
        if let Some(j) = close {
            spans.push((i, j));
        }
    }
    spans
}

/// The group ranges that fold into a single `⟦ phase ⟧` row: phases every one
/// of whose steps is settled. A phase holding the frontier, a failure, the
/// operator's selection or a step yet to start stays open — which is also how
/// a collapsed phase opens (the cursor entering it selects a step inside). An
/// open outer phase leaves its inner ones to fold on their own; a folded one
/// swallows them.
fn folded_phases(wf: &Workflow, groups: &[Group]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut covered = 0;
    for (i, j) in phase_spans(wf, groups) {
        if i >= covered && groups[i..=j].iter().all(|g| g.class == Class::Settled) {
            out.push(i..j + 1);
            covered = j + 1;
        }
    }
    out
}

// ---- folded rows ------------------------------------------------------------------

/// How many of a folded run's steps are done, skipped, and done with failed
/// units — the real settled counts a summary reports.
struct Tally {
    done: usize,
    skipped: usize,
    soft: usize,
}

fn tally(groups: &[Group]) -> Tally {
    let count = |f: &dyn Fn(&Group) -> bool| groups.iter().filter(|g| f(g)).count();
    Tally {
        done: count(&|g| g.state == Some(StepState::Complete)),
        skipped: count(&|g| g.state == Some(StepState::Skipped)),
        soft: count(&|g| g.soft_failed),
    }
}

impl Tally {
    /// `18 done · 2 skipped · 1 with failed units` — each part only when
    /// non-zero.
    fn text(&self) -> String {
        [
            (self.done, "done"),
            (self.skipped, "skipped"),
            (self.soft, "with failed units"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, what)| format!("{n} {what}"))
        .collect::<Vec<_>>()
        .join(" · ")
    }

    /// A fold never reads as a clean `✓` when a step in it finished with
    /// failures, nor as done when everything in it was skipped.
    fn status(&self) -> Status {
        match (self.soft, self.done) {
            (0, 0) => Status::Skipped,
            (0, _) => Status::Complete,
            _ => Status::SoftFailed,
        }
    }
}

/// `✓ first … last (+N done)` — a run of settled steps folded to one row, or
/// `○ first … last (+N pending)` for steps not yet started; runs of up to
/// [`SUMMARY_NAMES`] list every step.
fn summary_row(run: &[Group]) -> Line {
    let names: Vec<String> = run.iter().map(|g| printable(&g.id)).collect();
    let label = match names.as_slice() {
        [first, .., last] if names.len() > SUMMARY_NAMES => format!("{first} … {last}"),
        _ => names.join(" · "),
    };
    let (st, counts) = if run.iter().all(|g| g.class == Class::Pending) {
        (Status::Waiting, format!("{} pending", run.len()))
    } else {
        let t = tally(run);
        (t.status(), t.text())
    };
    Line::new()
        .dim(NO_MARK)
        .status(st, st.glyph().to_string())
        .plain(" ")
        .dim(label)
        .dim(format!(" (+{counts})"))
}

/// `⟦ split … join ⟧ phase · N steps ···· ✓ N done` — a settled split→join
/// sub-DAG folded to one node. The right-hand state is its own segments so a
/// width clip protects it.
fn phase_row(span: &[Group]) -> Line {
    let (first, last) = match (span.first(), span.last()) {
        (Some(f), Some(l)) => (printable(&f.id), printable(&l.id)),
        _ => (String::new(), String::new()),
    };
    let t = tally(span);
    let st = t.status();
    Line::new()
        .dim(NO_MARK)
        .status(st, "⟦ ")
        .plain(format!("{first} … {last}"))
        .status(st, " ⟧")
        .dim(format!("  phase · {} steps", span.len()))
        .dim(LEADER)
        .status(st, format!("{} {}", st.glyph(), t.text()))
}

// ---- fitting ----------------------------------------------------------------------

/// A folded run names its steps up to this many; longer runs read
/// `first … last`.
const SUMMARY_NAMES: usize = 3;
/// Columns of label a clipped row keeps before it gives up protecting its
/// right-hand state (so a sliver of label is never traded for a state word).
const MIN_LABEL: usize = 8;

/// What one stretch of the fitted pane is.
enum Piece {
    /// One group, shown whole (or trimmed).
    Group(usize),
    /// A run of settled (or of pending) groups folded to one summary row.
    Run(Range<usize>),
    /// A settled split→join phase folded to one `⟦ phase ⟧` row.
    Phase(Range<usize>),
}

impl Piece {
    /// Rows the piece takes: a fold keeps the connector that led its first
    /// group, plus its one row.
    fn height(&self, groups: &[Group], trim: &[usize]) -> usize {
        match self {
            Piece::Group(i) => trim[*i],
            Piece::Run(r) | Piece::Phase(r) => usize::from(groups[r.start].spine) + 1,
        }
    }
}

/// Partition the groups into pieces: each folded phase on its own, each
/// maximal run of folded groups of one class (settled, or pending) as one
/// summary — unless the summary would take as many rows as the run does (a
/// lone one-row step), in which case the steps stay as they are.
fn pieces(
    groups: &[Group],
    phase_end: &[Option<usize>],
    fold: &[bool],
    trim: &[usize],
) -> Vec<Piece> {
    let n = groups.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if let Some(end) = phase_end[i] {
            out.push(Piece::Phase(i..end));
            i = end;
        } else if !fold[i] {
            out.push(Piece::Group(i));
            i += 1;
        } else {
            let mut j = i + 1;
            while j < n && fold[j] && groups[j].class == groups[i].class && phase_end[j].is_none() {
                j += 1;
            }
            let shown: usize = trim[i..j].iter().sum();
            if usize::from(groups[i].spine) + 1 >= shown {
                out.extend((i..j).map(Piece::Group));
            } else {
                out.push(Piece::Run(i..j));
            }
            i = j;
        }
    }
    out
}

/// `group`'s rows cut to `target`: the head rows, a window of the fan-out
/// body centred on the unit cursor with a `⋮ +N above · +M below` row for
/// what was dropped, then the rows that follow the body. A target with no room
/// for a window plus that marker keeps no body at all; a *collapsed* fan-out
/// is only ever cut whole — its density line already says exactly how many
/// units are in each state, where a `⋮ +N below` over its movers would count
/// rows and read as a unit count.
fn trim_group(group: &Group, target: usize) -> Vec<Line> {
    let rows = &group.rows;
    let Some(fan) = group.fan.as_ref().filter(|_| target < rows.len()) else {
        return rows.clone();
    };
    let body = &fan.body;
    let room = target.saturating_sub(group.min_rows());
    let mut out = rows[..body.start].to_vec();
    if fan.drilled && room >= 2 {
        let slots = room - 1;
        let len = body.len();
        let cursor = fan
            .cursor
            .map_or(0, |c| c.saturating_sub(body.start))
            .min(len.saturating_sub(1));
        let start = cursor.saturating_sub(slots / 2).min(len - slots);
        out.extend_from_slice(&rows[body.start + start..body.start + start + slots]);
        let mut marker = fan.marker.clone();
        marker.segments.push(Segment {
            text: hidden_text(start, len - start - slots),
            style: Style::Dim,
        });
        out.push(marker);
    }
    out.extend_from_slice(&rows[body.end..]);
    out
}

/// `⋮ +N above · +M below` — what a windowed list left out.
fn hidden_text(above: usize, below: usize) -> String {
    let parts: Vec<String> = [(above, "above"), (below, "below")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, side)| format!("+{n} {side}"))
        .collect();
    format!("⋮ {}", parts.join(" · "))
}

/// Keep `avail` of `rows` with `anchor` inside the window (one row of context
/// above it when there is room), marking what was cut with `⋮` rows. Only
/// reached when the frontier itself outgrows the pane.
fn hard_clip(mut rows: Vec<Line>, anchor: usize, avail: usize) -> Vec<Line> {
    if rows.len() <= avail {
        return rows;
    }
    let lead = usize::from(avail >= 3);
    let start = anchor.saturating_sub(lead).min(rows.len() - avail);
    let above = start;
    let below = rows.len() - start - avail;
    let mut out: Vec<Line> = rows.drain(start..start + avail).collect();
    let marker = |n: usize, side: &str| Line::new().dim(NO_MARK).dim(format!("⋮ +{n} {side}"));
    if above > 0 && avail >= 3 {
        out[0] = marker(above + 1, "above");
    }
    if below > 0 && avail >= 2 {
        let last = out.len() - 1;
        out[last] = marker(below + 1, "below");
    }
    out
}

/// Width-clip a pane row while protecting its right-hand state. A row with a
/// [`LEADER`] (`<label> ···· <state>`) loses its *label* middle to an
/// ellipsis — the leading glyph and the trailing state stay; when even that is
/// too tight the leader shrinks to one space, and below that the row is
/// clipped plain (its leading glyph still says the state). Rows without a
/// leader (summaries, markers, density lines) are clipped plain.
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

/// The groups that fold together with group `i`: a loop's members, which share
/// one frame (`↻` header on the first, `↺` footer on the last) and so fold
/// whole or not at all; any other group is alone.
fn frame_of(groups: &[Group], i: usize) -> Range<usize> {
    let Some(name) = groups[i].loop_name.as_deref() else {
        return i..i + 1;
    };
    let same = |k: usize| groups[k].loop_name.as_deref() == Some(name);
    let start = (0..i).rev().take_while(|&k| same(k)).last().unwrap_or(i);
    let end = (i + 1..groups.len())
        .take_while(|&k| same(k))
        .last()
        .unwrap_or(i)
        + 1;
    start..end
}

/// Fold group `i` (with its frame) if everything in the frame is of `class`.
fn fold_group(groups: &[Group], fold: &mut [bool], i: usize, class: Class) {
    let frame = frame_of(groups, i);
    if groups[frame.clone()].iter().all(|g| g.class == class) {
        fold[frame].fill(true);
    }
}

/// The connector row that leads a folded piece, if its first group had one.
fn spine_of(group: &Group) -> Option<Line> {
    group.spine.then(|| group.rows[0].clone())
}

/// The groups bounded to `h` rows, shrinking in this order — and **the
/// frontier and the operator's selection are never folded**:
///
/// 1. settled phases are already folded to `⟦ phase ⟧` rows;
/// 2. settled steps fold into `✓ first … last (+N done)` rows, oldest first,
///    only as far as needed;
/// 3. steps not yet started fold into `○ first … last (+N pending)` rows,
///    latest first, only as far as needed;
/// 4. fan-out blocks are trimmed, largest first: a drilled list keeps its
///    density line plus a window around the unit cursor; a collapsed block
///    keeps its density line only;
/// 5. as a last resort the rows are windowed around the first frontier row.
fn fit(groups: &[Group], phases: &[Range<usize>], h: usize) -> Vec<Line> {
    let n = groups.len();
    let mut phase_end: Vec<Option<usize>> = vec![None; n];
    let mut in_phase = vec![false; n];
    for r in phases {
        phase_end[r.start] = Some(r.end);
        for flag in &mut in_phase[r.clone()] {
            *flag = true;
        }
    }
    let mut fold = vec![false; n];
    let mut trim: Vec<usize> = groups.iter().map(|g| g.rows.len()).collect();
    let height = |fold: &[bool], trim: &[usize]| -> usize {
        pieces(groups, &phase_end, fold, trim)
            .iter()
            .map(|p| p.height(groups, trim))
            .sum()
    };

    // 2. Settled steps fold oldest first, only until the pane fits.
    for i in 0..n {
        if height(&fold, &trim) <= h {
            break;
        }
        if groups[i].class == Class::Settled && !in_phase[i] {
            fold_group(groups, &mut fold, i, Class::Settled);
        }
    }
    // 3. Then steps not yet started fold the same way, furthest from the
    //    frontier first — the next step to start is the last to go.
    for i in (0..n).rev() {
        if height(&fold, &trim) <= h {
            break;
        }
        if groups[i].class == Class::Pending {
            fold_group(groups, &mut fold, i, Class::Pending);
        }
    }
    // 4. Then the biggest fan-out gives up rows, one at a time. A target of
    //    `min + 1` has no room for a unit and the marker, so it goes straight
    //    to `min`; a collapsed block has no window to keep.
    while height(&fold, &trim) > h {
        let biggest = (0..n)
            .filter(|&i| trim[i] > groups[i].min_rows() && !fold[i] && !in_phase[i])
            .max_by_key(|&i| trim[i] - groups[i].min_rows());
        let Some(i) = biggest else { break };
        let min = groups[i].min_rows();
        let collapsed = groups[i].fan.as_ref().is_some_and(|f| !f.drilled);
        trim[i] = if collapsed || trim[i] == min + 2 {
            min
        } else {
            trim[i] - 1
        };
    }

    let mut rows: Vec<Line> = Vec::new();
    let (mut frontier_at, mut keep_at) = (None, None);
    for piece in pieces(groups, &phase_end, &fold, &trim) {
        match piece {
            Piece::Group(i) => {
                let g = &groups[i];
                let node = rows.len() + g.lead;
                match g.class {
                    Class::Frontier => frontier_at = frontier_at.or(Some(node)),
                    Class::Keep => keep_at = keep_at.or(Some(node)),
                    Class::Settled | Class::Pending => {}
                }
                rows.extend(trim_group(g, trim[i]));
            }
            Piece::Run(r) => {
                rows.extend(spine_of(&groups[r.start]));
                rows.push(summary_row(&groups[r]));
            }
            Piece::Phase(r) => {
                rows.extend(spine_of(&groups[r.start]));
                rows.push(phase_row(&groups[r]));
            }
        }
    }
    // 5. Only reachable when frontier + kept rows alone exceed `h`.
    hard_clip(rows, frontier_at.or(keep_at).unwrap_or(0), h)
}

/// The structure pane: the workflow DAG ([`structure_rows`]) scaled to the
/// `w`×`h` column it is given.
///
/// * a `for_each` (or any unit-carrying node) shows its live **density bar +
///   movers** nested under its rail, and — when the operator has drilled into
///   it — the filtered unit list in their place;
/// * a settled split→join **phase** folds to one `⟦ phase ⟧` row; the phase
///   that holds the frontier stays open, and so does one the operator enters:
///   `nav` selects steps, and a phase holding the selected step never folds,
///   so moving the cursor onto any step inside a collapsed phase (or drilling
///   into it) opens it, and leaving it folds it again;
/// * contiguous **settled** steps fold to `✓ a … b (+N done)` rows, oldest
///   first, only as far as `h` demands — then, only if still needed, steps
///   not yet started fold to `○ a … b (+N pending)` rows, latest first;
/// * the frontier and the selected node are never folded;
/// * every row is clipped to `w` with its right-hand state protected.
///
/// At most `h` rows (none for a zero-sized column).
pub fn structure_pane(
    view: &RunView,
    wf: &Workflow,
    nav: &NavState,
    w: usize,
    h: usize,
) -> Vec<Line> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let rows = render_rows(wf, |id| node_status_of(view, id));
    let painted = paint_rows(view, wf, nav, &rows, true);
    let groups = build_groups(view, wf, nav, &rows, painted);
    let phases = folded_phases(wf, &groups);
    fit(&groups, &phases, h)
        .into_iter()
        .map(|l| clip_row(l, w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::live_view::nav::NavKey;
    use crate::output::live_view::row::render_plain;
    use crate::output::run_model::GateView;
    use rupu_orchestrator::executor::Event;
    use rupu_orchestrator::runs::{RunStatus, StepKind};

    /// preflight -> recon (split) -> {sweep, probe} -> gather (join); a gate;
    /// a bounded loop (`gen` + `critique`); a two-panelist panel.
    const WF: &str = r#"
name: assess
steps:
  - id: preflight
    agent: scanner
    prompt: go
    next: [recon]
  - id: recon
    split: [sweep, probe]
  - id: sweep
    agent: sweeper
    prompt: p
    next: [gather]
  - id: probe
    agent: prober
    prompt: p
    next: [gather]
  - id: gather
    join: { wait: all }
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

    /// One of every other node kind: command, connector action, `for_each`,
    /// `parallel`, and a `branch` with a then and an else arm.
    const KINDS_WF: &str = r#"
name: kinds
steps:
  - id: build
    run: { cmd: make, args: [all] }
  - id: comment
    action: scm.prs.comment
    with:
      platform: github
      owner: acme
      repo: widget
      number: 7
      body: done
  - id: hunt
    for_each: '["a", "b", "c"]'
    agent: worker
    prompt: p
  - id: lanes
    parallel:
      - id: spec
        agent: writer
        prompt: hi
      - id: verify
        agent: reviewer
        prompt: hi
  - id: pick
    branch:
      condition: "{{ steps.build.output }}"
      then: [ship]
      else: [hold]
  - id: ship
    agent: shipper
    prompt: p
  - id: hold
    agent: holder
    prompt: p
"#;

    /// A `parallel:` step whose first two sub-steps run the SAME agent.
    const PARALLEL_WF: &str = r#"
name: par
steps:
  - id: lanes
    parallel:
      - id: spec
        agent: writer
        prompt: a
      - id: draft
        agent: writer
        prompt: b
      - id: verify
        agent: reviewer
        prompt: c
"#;

    fn wf() -> Workflow {
        Workflow::parse(WF).expect("test workflow parses")
    }

    fn kinds_wf() -> Workflow {
        Workflow::parse(KINDS_WF).expect("test workflow parses")
    }

    fn start(
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

    fn agent_started(
        v: &mut RunView,
        step: &str,
        unit: Option<usize>,
        provider: &str,
        model: &str,
    ) {
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

    fn complete(v: &mut RunView, step: &str, ms: u64, host: Option<&str>) {
        v.apply(&Event::StepCompleted {
            run_id: "r".into(),
            step_id: step.into(),
            success: true,
            duration_ms: ms,
            host: host.map(Into::into),
        });
    }

    fn unit_started(v: &mut RunView, step: &str, index: usize, agent: &str, codename: &str) {
        v.apply(&Event::UnitStarted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("u{index}"),
            agent: Some(agent.into()),
            transcript_path: "t".into(),
            host: None,
            codename: Some(codename.into()),
        });
    }

    fn unit_completed(v: &mut RunView, step: &str, index: usize, success: bool) {
        v.apply(&Event::UnitCompleted {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            unit_key: format!("u{index}"),
            success,
            tokens_in: 0,
            tokens_out: 0,
            host: None,
        });
    }

    /// A mid-run view touching every construct in [`WF`]: preflight done
    /// (codename, model, duration, host), the split done, `sweep` running on
    /// a host, `probe` done, the join not reached, the gate parked, the loop
    /// on iteration 2 of 5, and the panel on round 1 of 3 with one panelist
    /// running and one done.
    fn live() -> RunView {
        let mut v = RunView::default();
        start(
            &mut v,
            "preflight",
            StepKind::Linear,
            Some("scanner"),
            Some("heron#1"),
            None,
        );
        agent_started(&mut v, "preflight", None, "anthropic", "claude-opus-5-5");
        complete(&mut v, "preflight", 18_000, Some("kuki"));
        start(&mut v, "recon", StepKind::Split, None, None, None);
        complete(&mut v, "recon", 2_000, None);
        start(
            &mut v,
            "sweep",
            StepKind::Linear,
            Some("sweeper"),
            Some("otter#2"),
            Some("mini"),
        );
        agent_started(&mut v, "sweep", None, "openai", "gpt-5");
        start(
            &mut v,
            "probe",
            StepKind::Linear,
            Some("prober"),
            Some("lynx#3"),
            None,
        );
        complete(&mut v, "probe", 95_000, None);
        start(&mut v, "approve", StepKind::ApprovalGate, None, None, None);
        v.apply(&Event::StepAwaitingApproval {
            run_id: "r".into(),
            step_id: "approve".into(),
            reason: "needs approval".into(),
        });
        start(&mut v, "loop:refine", StepKind::Loop, None, None, None);
        start(
            &mut v,
            "gen",
            StepKind::Linear,
            Some("generator"),
            None,
            None,
        );
        complete(&mut v, "gen", 7_000, None);
        v.step_mut("gen").loop_iteration = Some(2);
        start(
            &mut v,
            "critique",
            StepKind::Linear,
            Some("critic"),
            None,
            None,
        );
        v.step_mut("critique").loop_iteration = Some(2);
        start(&mut v, "review", StepKind::Panel, None, None, None);
        v.apply(&Event::PanelRound {
            run_id: "r".into(),
            step_id: "review".into(),
            round: 1,
            max_iterations: 3,
            max_severity_remaining: None,
        });
        unit_started(&mut v, "review", 0, "security-reviewer", "wren#1");
        agent_started(&mut v, "review", Some(0), "anthropic", "claude-sonnet-5-5");
        unit_started(&mut v, "review", 1, "perf-reviewer", "egret#1");
        unit_completed(&mut v, "review", 1, true);
        v
    }

    /// A nav that has manually selected the step at `idx` in `view.steps`.
    fn nav_at(view: &RunView, idx: usize) -> NavState {
        let mut nav = NavState::default();
        for _ in 0..idx {
            nav.apply(NavKey::Down, view);
        }
        nav
    }

    fn nav_on(view: &RunView, step: &str) -> NavState {
        nav_at(
            view,
            view.steps.iter().position(|s| s.step_id == step).unwrap(),
        )
    }

    fn plain(line: &Line) -> String {
        render_plain(std::slice::from_ref(line))
    }

    /// The first row whose plain text contains `needle`.
    fn row<'a>(rows: &'a [Line], needle: &str) -> &'a Line {
        rows.iter()
            .find(|l| plain(l).contains(needle))
            .unwrap_or_else(|| panic!("no row contains {needle:?}:\n{}", render_plain(rows)))
    }

    /// The style of the first segment of `line` whose text is exactly `text`.
    fn style_of<'a>(line: &'a Line, text: &str) -> &'a Style {
        &line
            .segments
            .iter()
            .find(|s| s.text == text)
            .unwrap_or_else(|| panic!("no segment {text:?} in {line:?}"))
            .style
    }

    fn marked(rows: &[Line]) -> Vec<String> {
        rows.iter()
            .map(plain)
            .filter(|l| l.starts_with('▸'))
            .collect()
    }

    #[test]
    fn structure_snapshot() {
        let v = live();
        let rows = structure_rows(&v, &wf(), &nav_on(&v, "sweep"));
        insta::assert_snapshot!(render_plain(&rows));
    }

    /// The remaining node kinds, in the states a run leaves them in: a failed
    /// command, a done action, a running `for_each` with a failed unit, a
    /// skipped `parallel`, a decided branch with its taken and skipped arms.
    #[test]
    fn kinds_snapshot() {
        let mut v = RunView::default();
        start(&mut v, "build", StepKind::Run, None, None, None);
        v.apply(&Event::StepFailed {
            run_id: "r".into(),
            step_id: "build".into(),
            error: "make failed".into(),
        });
        start(
            &mut v,
            "comment",
            StepKind::Action,
            None,
            None,
            Some("kuki"),
        );
        complete(&mut v, "comment", 1_000, Some("kuki"));
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        unit_started(&mut v, "hunt", 0, "worker", "otter#1");
        unit_completed(&mut v, "hunt", 0, false);
        unit_started(&mut v, "hunt", 1, "worker", "otter#2");
        v.apply(&Event::StepSkipped {
            run_id: "r".into(),
            step_id: "lanes".into(),
            reason: "when: false".into(),
        });
        start(&mut v, "pick", StepKind::Branch, None, None, None);
        complete(&mut v, "pick", 2_000, None);
        start(
            &mut v,
            "ship",
            StepKind::Linear,
            Some("shipper"),
            None,
            None,
        );
        v.apply(&Event::StepSkipped {
            run_id: "r".into(),
            step_id: "hold".into(),
            reason: "not taken".into(),
        });
        let rows = structure_rows(&v, &kinds_wf(), &NavState::default());
        insta::assert_snapshot!(render_plain(&rows));
    }

    fn warn(v: &mut RunView, step: &str, index: Option<usize>, message: &str) {
        v.apply(&Event::StepWarning {
            run_id: "r".into(),
            step_id: step.into(),
            index,
            message: message.into(),
        });
    }

    /// A step with warnings wears a `⚠` in its right-hand state — beside its
    /// real state, never in place of it; several warnings carry their count.
    #[test]
    fn a_warned_step_wears_a_marker_beside_its_state() {
        let mut v = live();
        warn(&mut v, "probe", None, "no coverage from the host");
        warn(&mut v, "sweep", Some(0), "host went away");
        warn(&mut v, "sweep", Some(1), "host went away again");
        let rows = structure_rows(&v, &wf(), &nav_on(&v, "sweep"));
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);

        // The state glyph and word are exactly what the lifecycle said.
        assert!(
            plain(row(&rows, "probe  prober")).ends_with("···· ✓ 1m 35s ⚠"),
            "{s}"
        );
        assert!(
            plain(row(&rows, "sweep  sweeper")).ends_with("···· ◐ running ⚠2 @mini"),
            "{s}"
        );
        assert_eq!(node_status_of(&v, "probe"), NodeStatus::Complete);
        assert_eq!(node_status_of(&v, "sweep"), NodeStatus::Working);
        // The marker is the warning tone, not a failure red.
        assert_eq!(
            style_of(row(&rows, "probe  prober"), " ⚠"),
            &Style::Status(Status::SoftFailed)
        );
        // Unwarned rows carry none.
        assert!(!plain(row(&rows, "preflight")).contains('⚠'), "{s}");
    }

    /// A settled step that warned must not fold away into `✓ a … b (+N done)`:
    /// the marker is the only place the warning lives while the run is live.
    #[test]
    fn a_warned_settled_step_is_never_folded_away() {
        let (mut v, wf) = (scale_view(20), scale_wf(20));
        let quiet = pane(&v, &wf, &NavState::default(), W, 16);
        assert!(!quiet.contains("s10"), "unwarned s10 folds:\n{quiet}");

        warn(&mut v, "s10", None, "no coverage from the host");
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 16);
        let s = render_plain(&rows);
        assert!(rows.len() <= 16, "{} rows:\n{s}", rows.len());
        let line = plain(row(&rows, "s10"));
        assert!(line.contains('⚠'), "{s}");
        assert!(line.contains("✓"), "its own state still reads done:\n{s}");
    }

    /// The marker rides in the protected right-hand state, so a tight column
    /// clips the label, not the warning.
    #[test]
    fn the_warning_marker_survives_a_tight_clip() {
        let mut v = live();
        warn(&mut v, "probe", None, "no coverage");
        let rows = structure_pane(&v, &wf(), &NavState::default(), 30, 40);
        let line = plain(row(&rows, "1m 35s"));
        assert!(line.ends_with("✓ 1m 35s ⚠"), "{line}");
        assert!(
            line.contains('…'),
            "the label gave way, not the state: {line}"
        );
        assert!(line.chars().count() <= 30, "{line}");
    }

    #[test]
    fn node_status_follows_the_step_state() {
        let mut v = RunView::default();
        for (id, state, want) in [
            ("a", StepState::Pending, NodeStatus::Waiting),
            ("b", StepState::Running, NodeStatus::Working),
            ("c", StepState::AwaitingApproval, NodeStatus::Awaiting),
            ("d", StepState::Complete, NodeStatus::Complete),
            ("e", StepState::Failed, NodeStatus::Failed),
            ("f", StepState::Skipped, NodeStatus::Skipped),
            ("g", StepState::Paused, NodeStatus::Waiting),
        ] {
            v.step_mut(id).state = state;
            assert_eq!(node_status_of(&v, id), want, "{id}: {state:?}");
        }
        // A step the run has not reached has no view: waiting, not an error.
        assert_eq!(node_status_of(&v, "unknown"), NodeStatus::Waiting);
    }

    #[test]
    fn a_fan_out_that_completed_with_failed_units_is_soft_failed() {
        let mut v = RunView::default();
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        unit_started(&mut v, "hunt", 0, "worker", "otter#1");
        unit_completed(&mut v, "hunt", 0, false);
        unit_started(&mut v, "hunt", 1, "worker", "otter#2");
        unit_completed(&mut v, "hunt", 1, true);
        // Still running: failed units do not decide the step yet.
        assert_eq!(node_status_of(&v, "hunt"), NodeStatus::Working);
        complete(&mut v, "hunt", 9_000, None);
        assert_eq!(node_status_of(&v, "hunt"), NodeStatus::SoftFailed);

        // All units clean: a plain complete.
        let mut clean = RunView::default();
        start(&mut clean, "hunt", StepKind::ForEach, None, None, None);
        unit_started(&mut clean, "hunt", 0, "worker", "otter#1");
        unit_completed(&mut clean, "hunt", 0, true);
        complete(&mut clean, "hunt", 9_000, None);
        assert_eq!(node_status_of(&clean, "hunt"), NodeStatus::Complete);
    }

    #[test]
    fn only_the_chosen_step_is_marked() {
        let v = live();
        // Following, nothing parked: the cursor idles on step 0 — no choice.
        assert!(marked(&structure_rows(&v, &wf(), &NavState::default())).is_empty());

        // A manual choice marks exactly its own row — not its fork lane.
        let rows = structure_rows(&v, &wf(), &nav_on(&v, "sweep"));
        let marks = marked(&rows);
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(marks[0].starts_with("▸ ◐  sweep"), "{marks:?}");
        assert!(!plain(row(&rows, "╭─ sweep")).starts_with('▸'));

        // The loop is a step of its own to the navigator; its header is it.
        let rows = structure_rows(&v, &wf(), &nav_on(&v, "loop:refine"));
        let marks = marked(&rows);
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(marks[0].contains("↻ loop:refine"), "{marks:?}");
    }

    #[test]
    fn a_parked_gate_is_marked_while_following() {
        let mut v = live();
        v.status = RunStatus::AwaitingApproval;
        v.gates = vec![GateView {
            step_id: "approve".into(),
            prompt: Some("Ship it?".into()),
            since: chrono::Utc::now(),
            expires_at: None,
        }];
        let marks = marked(&structure_rows(&v, &wf(), &NavState::default()));
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(marks[0].starts_with("▸ ⏸  approve"), "{marks:?}");
    }

    #[test]
    fn unstarted_nodes_carry_no_overlay() {
        // A run that has produced nothing: every node is the static row —
        // no state, no leader, no host, no identity parts.
        let rows = structure_rows(&RunView::default(), &wf(), &NavState::default());
        let text = render_plain(&rows);
        assert!(!text.contains("····"), "{text}");
        assert!(!text.contains('@'), "{text}");
        assert_eq!(plain(row(&rows, "preflight")), "  ○  preflight  scanner");
        assert_eq!(plain(row(&rows, "recon  ")), "  ◈  recon  split");
        assert_eq!(plain(row(&rows, "approve")), "  ⏸  approve  gate");
        assert_eq!(plain(row(&rows, "gather")), "  ◈◄─ gather  join · wait:all");
    }

    #[test]
    fn a_field_the_run_has_not_produced_is_not_shown() {
        // Complete, but no duration, host, codename or model were ever seen.
        let mut v = RunView::default();
        start(&mut v, "preflight", StepKind::Linear, None, None, None);
        v.step_mut("preflight").state = StepState::Complete;
        let rows = structure_rows(&v, &wf(), &NavState::default());
        assert_eq!(
            plain(row(&rows, "preflight")),
            "  ✓  preflight  scanner ···· ✓ done"
        );
    }

    #[test]
    fn identity_overlays_the_row_in_order() {
        let rows = structure_rows(&live(), &wf(), &NavState::default());
        assert_eq!(
            plain(row(&rows, "preflight")),
            "  ✓  preflight  scanner · heron#1 · anthropic/claude-opus-5-5 ···· ✓ 18s @kuki"
        );
        // A running step on a host: state, then the host chip.
        assert_eq!(
            plain(row(&rows, "sweeper")),
            "  ◐  sweep  sweeper · otter#2 · openai/gpt-5 ···· ◐ running @mini"
        );
    }

    #[test]
    fn the_agent_is_named_once_unless_it_differs_from_the_meta() {
        let mut v = live();
        let rows = structure_rows(&v, &wf(), &NavState::default());
        // app-canvas's meta already says `scanner`; the overlay must not.
        assert_eq!(plain(row(&rows, "preflight")).matches("scanner").count(), 1);

        // The agent that actually ran differs from the declared one: both show.
        v.step_mut("preflight").agent = Some("fallback-scanner".into());
        let rows = structure_rows(&v, &wf(), &NavState::default());
        let line = plain(row(&rows, "preflight"));
        assert!(
            line.contains("scanner · heron#1 · fallback-scanner"),
            "{line}"
        );
    }

    #[test]
    fn the_codename_takes_its_role_hue() {
        let rows = structure_rows(&live(), &wf(), &NavState::default());
        let line = row(&rows, "preflight");
        assert_eq!(*style_of(line, "heron"), Style::Role("heron".into()));
        assert_eq!(*style_of(line, "#1"), Style::Plain);
    }

    #[test]
    fn node_glyphs_lead_by_kind_and_take_the_status_colour() {
        let rows = structure_rows(&live(), &wf(), &NavState::default());
        // Agent step: the status bullet leads, coloured by state.
        assert_eq!(
            *style_of(row(&rows, "preflight"), "✓"),
            Style::Status(Status::Complete)
        );
        // Gate: its own glyph, amber while parked.
        assert_eq!(
            *style_of(row(&rows, "approve"), "⏸"),
            Style::Status(Status::Awaiting)
        );
        // Join not reached: its glyph recedes with the waiting status.
        assert_eq!(
            *style_of(row(&rows, "gather"), "◈"),
            Style::Status(Status::Waiting)
        );
        // A header with no bullet takes the kind glyph before its label.
        let panel = plain(row(&rows, "review"));
        assert!(panel.contains("├─╭─ ⟲ review"), "{panel}");
    }

    #[test]
    fn panelists_take_status_and_identity_from_their_units() {
        let rows = structure_rows(&live(), &wf(), &NavState::default());
        let running = row(&rows, "security-reviewer");
        assert_eq!(*style_of(running, "◐"), Style::Status(Status::Working));
        assert_eq!(*style_of(running, "wren"), Style::Role("wren".into()));
        assert!(plain(running).contains("anthropic/claude-sonnet-5-5"));
        let done = row(&rows, "perf-reviewer");
        assert_eq!(*style_of(done, "✓"), Style::Status(Status::Complete));
        // The panelist's `●├─` branch cell follows its own status too.
        assert_eq!(*style_of(done, "├─"), Style::Dim);
        assert_eq!(*style_of(running, "├─"), Style::Status(Status::Working));
    }

    #[test]
    fn a_later_round_reads_the_newest_unit_of_the_panelist() {
        let mut v = live();
        // Round 2 re-runs the same panelist as a new unit, higher index.
        unit_started(&mut v, "review", 2, "perf-reviewer", "egret#1");
        let rows = structure_rows(&v, &wf(), &NavState::default());
        assert_eq!(
            *style_of(row(&rows, "perf-reviewer"), "◐"),
            Style::Status(Status::Working)
        );
    }

    #[test]
    fn unit_less_members_wait_unless_the_parent_was_skipped() {
        let panel_only = Workflow::parse(
            r#"
name: p
steps:
  - id: review
    panel:
      panelists: [a-reviewer, b-reviewer]
      subject: x
"#,
        )
        .unwrap();
        let mut v = RunView::default();
        start(&mut v, "review", StepKind::Panel, None, None, None);
        let rows = structure_rows(&v, &panel_only, &NavState::default());
        assert!(plain(row(&rows, "a-reviewer")).contains("○"));

        v.apply(&Event::StepSkipped {
            run_id: "r".into(),
            step_id: "review".into(),
            reason: "skipped".into(),
        });
        let rows = structure_rows(&v, &panel_only, &NavState::default());
        assert!(plain(row(&rows, "a-reviewer")).contains("⊘"));
        assert!(plain(row(&rows, "b-reviewer")).contains("⊘"));
    }

    #[test]
    fn the_loop_header_reports_the_iteration_the_members_reached() {
        let mut v = live();
        let rows = structure_rows(&v, &wf(), &NavState::default());
        let header = row(&rows, "↻ loop:refine");
        assert!(
            plain(header).ends_with("···· ◐ iter 2/5"),
            "{}",
            plain(header)
        );

        // No member has reported an iteration yet: no invented counter.
        v.step_mut("gen").loop_iteration = None;
        v.step_mut("critique").loop_iteration = None;
        let rows = structure_rows(&v, &wf(), &NavState::default());
        assert!(
            !render_plain(&rows).contains("iter "),
            "{}",
            render_plain(&rows)
        );
        assert!(plain(row(&rows, "↻ loop:refine")).ends_with("···· ◐ running"));

        // Settled: the loop's own state and duration.
        complete(&mut v, "loop:refine", 61_000, None);
        let rows = structure_rows(&v, &wf(), &NavState::default());
        assert!(plain(row(&rows, "↻ loop:refine")).ends_with("···· ✓ 1m 01s"));
    }

    #[test]
    fn live_rails_light_up_and_settled_rails_recede() {
        let mut v = live();
        let rows = structure_rows(&v, &wf(), &NavState::default());
        // Fork lanes follow their target: sweep runs, probe is done.
        assert_eq!(
            *style_of(row(&rows, "╭─ sweep"), "╭─"),
            Style::Status(Status::Working)
        );
        assert_eq!(*style_of(row(&rows, "╰─ probe"), "╰─"), Style::Dim);
        // The running panel's rails, and the running loop's gutter.
        assert_eq!(
            *style_of(row(&rows, "security-reviewer"), "│"),
            Style::Status(Status::Working)
        );
        let gutter = row(&rows, "generator");
        assert_eq!(*style_of(gutter, "│"), Style::Status(Status::Working));
        assert_eq!(
            *style_of(row(&rows, "↻ loop:refine"), "├─"),
            Style::Status(Status::Working)
        );
        // Between-step connectors are plain structure.
        assert!(rows
            .iter()
            .filter(|l| plain(l).trim() == "│")
            .all(|l| *style_of(l, "│") == Style::Dim));

        // Once the loop completes its frame recedes.
        complete(&mut v, "loop:refine", 9_000, None);
        let rows = structure_rows(&v, &wf(), &NavState::default());
        assert_eq!(*style_of(row(&rows, "generator"), "│"), Style::Dim);
        assert_eq!(*style_of(row(&rows, "↺ loop:refine"), "◄─"), Style::Dim);
    }

    /// A `parallel:` step as the runner reports it: every declared sub-step
    /// is a unit of it (`index` = declared position), announced with its
    /// agent's identity.
    fn parallel_view(subs: &[(&str, Option<bool>)]) -> RunView {
        let mut v = RunView::default();
        start(&mut v, "lanes", StepKind::Parallel, None, None, None);
        for (i, (agent, outcome)) in subs.iter().enumerate() {
            unit_started(&mut v, "lanes", i, agent, &format!("otter#{}", i + 1));
            agent_started(&mut v, "lanes", Some(i), "anthropic", "claude-sonnet-5-5");
            if let Some(success) = outcome {
                unit_completed(&mut v, "lanes", i, *success);
            }
        }
        v
    }

    #[test]
    fn a_completed_parallel_step_shows_its_sub_steps_done_not_waiting() {
        let mut v = parallel_view(&[
            ("writer", Some(true)),
            ("writer", Some(true)),
            ("reviewer", Some(true)),
        ]);
        complete(&mut v, "lanes", 9_000, None);
        let par = Workflow::parse(PARALLEL_WF).unwrap();
        let rows = structure_rows(&v, &par, &NavState::default());

        // The parent and every sub-step read finished — none un-started.
        assert_eq!(
            *style_of(row(&rows, "⇉ lanes"), "⇉ "),
            Style::Status(Status::Complete)
        );
        for sub in ["├─ spec", "├─ draft", "├─ verify"] {
            let line = row(&rows, sub);
            assert_eq!(
                *style_of(line, "✓"),
                Style::Status(Status::Complete),
                "{sub}"
            );
            assert!(plain(line).ends_with("···· ✓ done"), "{}", plain(line));
        }
        let text = render_plain(&rows);
        assert!(
            !text.contains('○'),
            "no sub-step may read un-started:\n{text}"
        );
        // Identity comes from the sub-step's unit.
        let spec = row(&rows, "├─ spec");
        assert_eq!(*style_of(spec, "otter"), Style::Role("otter".into()));
        assert!(plain(spec).contains("anthropic/claude-sonnet-5-5"));
    }

    #[test]
    fn parallel_sub_steps_sharing_an_agent_are_told_apart_by_position() {
        // Two sub-steps run `writer`: one done, one failed; `verify` runs.
        let v = parallel_view(&[
            ("writer", Some(true)),
            ("writer", Some(false)),
            ("reviewer", None),
        ]);
        let par = Workflow::parse(PARALLEL_WF).unwrap();
        let rows = structure_rows(&v, &par, &NavState::default());
        assert_eq!(
            *style_of(row(&rows, "├─ spec"), "✓"),
            Style::Status(Status::Complete)
        );
        assert_eq!(
            *style_of(row(&rows, "├─ draft"), "✗"),
            Style::Status(Status::Failed)
        );
        assert_eq!(
            *style_of(row(&rows, "├─ verify"), "◐"),
            Style::Status(Status::Working)
        );
        // Each sub-step's own rail follows its status (running lights up).
        assert_eq!(
            *style_of(row(&rows, "├─ verify"), "├─"),
            Style::Status(Status::Working)
        );
        assert_eq!(*style_of(row(&rows, "├─ spec"), "├─"), Style::Dim);
    }

    #[test]
    fn wire_text_never_reaches_the_terminal() {
        let mut v = RunView::default();
        start(
            &mut v,
            "sweep",
            StepKind::Linear,
            Some("ag\u{1b}[31ment"),
            Some("he\u{1b}[31mron#1"),
            Some("ho\u{1b}st"),
        );
        agent_started(&mut v, "sweep", None, "pro\u{7}vider", "mo\ndel");
        let rows = structure_rows(&v, &wf(), &NavState::default());
        for seg in rows.iter().flat_map(|l| &l.segments) {
            assert!(
                !seg.text.chars().any(char::is_control),
                "control character in {seg:?}"
            );
        }
        // A codename that is not codename-shaped is dropped, not echoed.
        assert!(!plain(row(&rows, "sweep")).contains("heron"));
    }

    // ---- structure_pane: scale ----------------------------------------------------

    /// Terminal width of the pane snapshots.
    const W: usize = 100;

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

    /// Give step `id` one unit per entry of `statuses`.
    fn give_units(v: &mut RunView, id: &str, statuses: &[UnitStatus]) {
        let step = v.step_mut(id);
        step.units.clear();
        for (i, st) in statuses.iter().enumerate() {
            step.units.insert(i, unit(i, *st));
        }
    }

    /// 86 unit statuses: 52 done, 6 running (54..60), 2 failed (7, 31),
    /// 26 queued (60..86).
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

    /// `s01`..`s{settled}` chained, then a `recon` split into a `for_each`
    /// `hunt` and a `probe`, a `gather` join and a `report` that follows it.
    fn scale_wf(settled: usize) -> Workflow {
        let mut yaml = String::from("name: scale\nsteps:\n");
        for i in 1..=settled {
            let next = if i == settled {
                "recon".to_string()
            } else {
                format!("s{:02}", i + 1)
            };
            yaml.push_str(&format!(
                "  - id: s{i:02}\n    agent: scanner\n    prompt: p\n    next: [{next}]\n"
            ));
        }
        yaml.push_str(
            r#"  - id: recon
    split: [hunt, probe]
  - id: hunt
    for_each: '["a", "b"]'
    agent: worker
    prompt: p
    next: [gather]
  - id: probe
    agent: prober
    prompt: p
    next: [gather]
  - id: gather
    join: { wait: all }
  - id: report
    agent: reporter
    prompt: p
    depends_on: [gather]
"#,
        );
        Workflow::parse(&yaml).expect("scale workflow parses")
    }

    /// The run of [`scale_wf`] mid-fan-out: every `s*` step and the split
    /// done, `probe` done, `hunt` running over 86 units, the join and the
    /// report not reached.
    fn scale_view(settled: usize) -> RunView {
        let mut v = RunView::default();
        for i in 1..=settled {
            let id = format!("s{i:02}");
            start(&mut v, &id, StepKind::Linear, Some("scanner"), None, None);
            complete(&mut v, &id, 1_000 + i as u64, None);
        }
        start(&mut v, "recon", StepKind::Split, None, None, None);
        complete(&mut v, "recon", 2_000, None);
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        give_units(&mut v, "hunt", &statuses_86());
        start(
            &mut v,
            "probe",
            StepKind::Linear,
            Some("prober"),
            None,
            None,
        );
        complete(&mut v, "probe", 5_000, None);
        v
    }

    fn pane(view: &RunView, wf: &Workflow, nav: &NavState, w: usize, h: usize) -> String {
        render_plain(&structure_pane(view, wf, nav, w, h))
    }

    #[test]
    fn scale_snapshot_86_units_under_a_split_with_20_settled_steps() {
        let (v, wf) = (scale_view(20), scale_wf(20));
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 16);
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);

        // Bounded: the whole workflow is 70+ rows, the pane is 16.
        assert!(rows.len() <= 16, "{} rows:\n{s}", rows.len());
        // The 20 settled steps (and the done split) fold to one summary with
        // the real settled count; none of them survives as its own row.
        assert!(s.contains("✓ s01 … recon (+21 done)"), "{s}");
        assert!(!s.contains("s10"), "{s}");
        // The frontier is intact: the for_each header, running.
        assert!(s.contains("⊞ hunt"), "{s}");
        assert!(
            plain(row(&rows, "⊞ hunt")).ends_with("···· ◐ running"),
            "{s}"
        );
        // Density line + movers nest under the node's rail; the static
        // placeholder is gone.
        assert!(!s.contains("runtime items"), "{s}");
        let density = row(&rows, "52/86");
        assert!(plain(density).starts_with("  │ │ "), "{}", plain(density));
        assert!(plain(density).ends_with("52/86 ✓52 ◐6 ✗2 ○26"), "{s}");
        for running in [56, 57, 58, 59] {
            let line = plain(row(&rows, &format!("svc-{running}")));
            assert!(line.starts_with("  │ ◐├─ svc-"), "{line}");
        }
        assert!(plain(row(&rows, "… +82 more")).starts_with("  │ │ "), "{s}");
        // Everything past the frontier still shows what is coming.
        assert!(s.contains("gather"), "{s}");
    }

    #[test]
    fn the_frontier_and_selection_are_never_folded() {
        let (v, wf) = (scale_view(20), scale_wf(20));
        // Operator parks on a settled step mid-run: it stays, with its fold
        // split around it.
        let nav = nav_on(&v, "s10");
        let rows = structure_pane(&v, &wf, &nav, W, 14);
        let s = render_plain(&rows);
        assert!(rows.len() <= 14, "{s}");
        assert!(s.contains("▸ ✓  s10"), "{s}");
        assert!(s.contains("s01 … s09 (+9 done)"), "{s}");
        assert!(s.contains("(+11 done)"), "{s}");
        // The frontier (running hunt) is there regardless.
        assert!(s.contains("◐ running"), "{s}");
        assert_eq!(
            s.matches('▸').count(),
            1,
            "exactly one selection marker:\n{s}"
        );
    }

    #[test]
    fn settled_steps_do_not_fold_while_the_pane_has_room() {
        let (v, wf) = (scale_view(3), scale_wf(3));
        let s = pane(&v, &wf, &NavState::default(), W, 60);
        assert!(!s.contains("(+"), "{s}");
        for id in ["s01", "s02", "s03", "recon", "hunt", "probe", "gather"] {
            assert!(s.contains(id), "{id}:\n{s}");
        }
    }

    #[test]
    fn a_summary_counts_skipped_and_flags_soft_failures() {
        let wf = scale_wf(6);
        let mut v = scale_view(6);
        v.apply(&Event::StepSkipped {
            run_id: "r".into(),
            step_id: "s02".into(),
            reason: "when: false".into(),
        });
        let s = pane(&v, &wf, &NavState::default(), W, 14);
        // 5 done + recon = 6 done, 1 skipped — the real counts.
        assert!(s.contains("(+6 done · 1 skipped)"), "{s}");

        // A completed fan-out that lost units must not hide behind a clean ✓.
        let mut v = scale_view(6);
        let finished = [UnitStatus::Done, UnitStatus::Done, UnitStatus::Failed];
        give_units(&mut v, "hunt", &finished);
        complete(&mut v, "hunt", 9_000, None);
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 4);
        let s = render_plain(&rows);
        assert!(
            s.contains("s01 … probe (+9 done · 1 with failed units)"),
            "{s}"
        );
        let summary = row(&rows, "(+9");
        assert_eq!(*style_of(summary, "!"), Style::Status(Status::SoftFailed));
        // What has not started folds last and says so.
        assert!(s.contains("○ gather · report (+2 pending)"), "{s}");
    }

    #[test]
    fn the_pane_never_exceeds_its_column() {
        let (v, wf) = (scale_view(20), scale_wf(20));
        for nav in [NavState::default(), nav_on(&v, "hunt"), nav_on(&v, "s03")] {
            for h in 0..=40 {
                for w in [0, 1, 8, 24, 40, 80, 100] {
                    let rows = structure_pane(&v, &wf, &nav, w, h);
                    assert!(rows.len() <= h, "h={h} w={w}: {} rows", rows.len());
                    for r in &rows {
                        assert!(r.width() <= w, "h={h} w={w}: {r:?}");
                    }
                    if w >= 24 && h >= 3 {
                        // Frontier is sacred — even a cramped pane keeps it
                        // (its label may give way, its running state does not).
                        assert!(
                            render_plain(&rows).contains("running"),
                            "h={h} w={w}:\n{}",
                            render_plain(&rows)
                        );
                    }
                }
            }
        }
        assert!(structure_pane(&v, &wf, &NavState::default(), 0, 10).is_empty());
        assert!(structure_pane(&v, &wf, &NavState::default(), 10, 0).is_empty());
    }

    #[test]
    fn a_clipped_row_keeps_its_right_hand_state() {
        let wf = wf();
        let v = live();
        let full = pane(&v, &wf, &NavState::default(), 200, 60);
        let long = full.lines().find(|l| l.contains("preflight")).unwrap();
        assert!(long.ends_with("···· ✓ 18s @kuki"), "{long}");
        for w in [60, 48, 40] {
            let rows = structure_pane(&v, &wf, &NavState::default(), w, 60);
            let line = plain(row(&rows, "preflight"));
            assert!(line.ends_with("✓ 18s @kuki"), "w={w}: {line}");
            assert!(line.contains('…'), "the label gave way: {line}");
            assert!(rows.iter().all(|r| r.width() <= w));
            // The state keeps its colour through the clip.
            assert_eq!(
                *style_of(row(&rows, "preflight"), "✓ 18s"),
                Style::Status(Status::Complete)
            );
        }
        // Too tight to protect anything: plain clip, still bounded.
        let rows = structure_pane(&v, &wf, &NavState::default(), 12, 60);
        assert!(rows.iter().all(|r| r.width() <= 12));
    }

    // ---- structure_pane: fan-out ----------------------------------------------------

    #[test]
    fn a_for_each_with_no_units_yet_keeps_its_static_rows() {
        let wf = scale_wf(2);
        let mut v = RunView::default();
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        let s = pane(&v, &wf, &NavState::default(), W, 80);
        assert!(s.contains("runtime items"), "{s}");
        assert!(!s.contains("░"), "no invented density bar:\n{s}");
    }

    #[test]
    fn the_density_block_hangs_under_the_rail_and_lights_with_the_node() {
        let (v, wf) = (scale_view(2), scale_wf(2));
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 80);
        let density = row(&rows, "52/86");
        // Filled run is Good, the rest dim, counts coloured by status.
        assert_eq!(
            *style_of(density, &"▓".repeat(density_fill(52, 86))),
            Style::Good
        );
        assert_eq!(*style_of(density, "◐6"), Style::Status(Status::Working));
        assert_eq!(*style_of(density, "✗2"), Style::Status(Status::Failed));
        // The running node's rails light the block; the unit's own bullet
        // and branch follow the unit's status.
        let mover = row(&rows, "svc-59");
        assert_eq!(*style_of(mover, "│"), Style::Status(Status::Working));
        assert_eq!(*style_of(mover, "◐"), Style::Status(Status::Working));
        assert_eq!(*style_of(mover, "├─"), Style::Status(Status::Working));
        assert!(plain(mover).ends_with("···· ◐ running"), "{}", plain(mover));
        // Identity from the unit; no unit codename → none shown.
        assert!(plain(mover).contains("anthropic/claude-opus-5-5"));
        // No per-unit spend exists, so none is invented.
        let s = render_plain(&rows);
        assert!(!s.contains('⇡') && !s.contains('$'), "{s}");
    }

    #[test]
    fn density_fill_is_never_full_until_done_nor_empty_once_any_is() {
        assert_eq!(density_fill(0, 0), 0);
        assert_eq!(density_fill(0, 86), 0);
        assert_eq!(density_fill(85, 86), DENSITY_WIDTH - 1);
        assert_eq!(density_fill(86, 86), DENSITY_WIDTH);
        assert_eq!(density_fill(1, 200), 1);
        // Never an empty styled segment.
        for statuses in [
            vec![UnitStatus::Queued; 3],
            vec![UnitStatus::Done; 86],
            Vec::new(),
        ] {
            let mut v = RunView::default();
            start(&mut v, "hunt", StepKind::ForEach, None, None, None);
            give_units(&mut v, "hunt", &statuses);
            let tail = density_tail(&v.steps[0]);
            assert!(tail.segments.iter().all(|s| !s.text.is_empty()), "{tail:?}");
        }
    }

    #[test]
    fn unit_rows_drop_empty_parts_and_control_characters() {
        let wf = scale_wf(2);
        let mut v = RunView::default();
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        give_units(&mut v, "hunt", &[UnitStatus::Running]);
        let u = v.step_mut("hunt").units.get_mut(&0).unwrap();
        u.codename = None;
        u.provider = Some(String::new());
        u.model = Some("opus".into());
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 40);
        assert_eq!(
            plain(row(&rows, "svc-0")),
            "  │ ◐├─ svc-0  opus ···· ◐ running"
        );

        // Empty provider AND model and a hostile, then empty, unit key: no
        // dangling separators, no control character on the terminal.
        let u = v.step_mut("hunt").units.get_mut(&0).unwrap();
        u.provider = Some(String::new());
        u.model = Some(String::new());
        u.unit_key = "hostile\u{1b}[31m\nkey".into();
        u.host = Some("ho\u{7}st".into());
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 40);
        for seg in rows.iter().flat_map(|l| &l.segments) {
            assert!(!seg.text.chars().any(char::is_control), "{seg:?}");
        }
        let line = plain(row(&rows, "hostile"));
        assert!(
            !line.contains(" ·  ·") && !line.contains("· ····"),
            "{line}"
        );
        assert!(line.ends_with("···· ◐ running @ho\u{FFFD}st"), "{line}");
        let u = v.step_mut("hunt").units.get_mut(&0).unwrap();
        u.unit_key = String::new();
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 40);
        assert!(plain(row(&rows, "unit 0")).contains("◐├─ unit 0"));
    }

    #[test]
    fn movers_are_the_newest_running_units_padded_by_the_rest() {
        let mut v = RunView::default();
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        let s = [
            UnitStatus::Done,
            UnitStatus::Done,
            UnitStatus::Running,
            UnitStatus::Queued,
            UnitStatus::Queued,
            UnitStatus::Queued,
        ];
        give_units(&mut v, "hunt", &s);
        let keys: Vec<usize> = live_movers(&v.steps[0]).iter().map(|u| u.index).collect();
        assert_eq!(keys, vec![2, 3, 4, 5]);
        // Many running: the last four, in index order.
        give_units(&mut v, "hunt", &statuses_86());
        let keys: Vec<usize> = live_movers(&v.steps[0]).iter().map(|u| u.index).collect();
        assert_eq!(keys, vec![56, 57, 58, 59]);
    }

    #[test]
    fn a_drilled_for_each_lists_its_filtered_units_and_marks_the_cursor() {
        let (v, wf) = (scale_view(2), scale_wf(2));
        let mut nav = nav_on(&v, "hunt");
        nav.apply(NavKey::In, &v); // Step depth: the unit list
        nav.apply(NavKey::Filter, &v); // running
        nav.apply(NavKey::Filter, &v); // failed
        assert_eq!(nav.filter(), UnitFilter::Failed);
        let rows = structure_pane(&v, &wf, &nav, W, 80);
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);

        let header = plain(row(&rows, "⊞ hunt"));
        assert!(header.contains("filter: failed"), "{header}");
        assert!(header.starts_with("▸ "), "{header}");
        // The density line still describes the whole step.
        assert!(s.contains("52/86 ✓52 ◐6 ✗2 ○26"), "{s}");
        // Only the two failed units are listed — no movers, no `more`.
        assert!(!s.contains("more"), "{s}");
        assert_eq!(s.matches("svc-").count(), 2, "{s}");
        assert!(
            plain(row(&rows, "svc-7 ")).starts_with("▸ │ ✗├─ svc-7"),
            "{s}"
        );
        assert!(
            plain(row(&rows, "svc-31")).starts_with("  │ ✗├─ svc-31"),
            "{s}"
        );
        // Down moves the unit cursor, not the step.
        nav.apply(NavKey::Down, &v);
        let rows = structure_pane(&v, &wf, &nav, W, 80);
        assert!(plain(row(&rows, "svc-31")).starts_with("▸ │ ✗├─ svc-31"));
        assert!(plain(row(&rows, "svc-7 ")).starts_with("  │ ✗├─ svc-7"));
        // A filter that admits nothing says so.
        nav.apply(NavKey::Filter, &v); // done
        nav.apply(NavKey::Filter, &v); // all
        nav.apply(NavKey::Filter, &v); // running
        nav.apply(NavKey::Filter, &v); // failed again
        assert_eq!(nav.filter(), UnitFilter::Failed);
        let mut empty = RunView::default();
        start(&mut empty, "hunt", StepKind::ForEach, None, None, None);
        give_units(&mut empty, "hunt", &[UnitStatus::Done, UnitStatus::Running]);
        let mut nav = nav_on(&empty, "hunt");
        nav.apply(NavKey::In, &empty);
        nav.apply(NavKey::Filter, &empty);
        nav.apply(NavKey::Filter, &empty);
        assert!(pane(&empty, &wf, &nav, W, 80).contains("no failed units"));
    }

    #[test]
    fn a_drilled_list_is_windowed_around_the_cursor_in_a_tight_pane() {
        let (v, wf) = (scale_view(2), scale_wf(2));
        let mut nav = nav_on(&v, "hunt");
        nav.apply(NavKey::In, &v); // All units, cursor on unit 0
        for _ in 0..40 {
            nav.apply(NavKey::Down, &v);
        }
        for h in [12, 16, 24] {
            let rows = structure_pane(&v, &wf, &nav, W, h);
            let s = render_plain(&rows);
            assert!(rows.len() <= h, "h={h}:\n{s}");
            // The cursor unit (40) stays in view and is the marked row.
            assert!(
                plain(row(&rows, "svc-40 ")).starts_with("▸ "),
                "h={h}:\n{s}"
            );
            // What was cut is counted, in rows of the list (units).
            let marker = plain(row(&rows, "⋮"));
            assert!(
                marker.contains("above") && marker.contains("below"),
                "{marker}"
            );
            // The density line (exact counts) and the frame's closer survive.
            assert!(s.contains("52/86 ✓52 ◐6 ✗2 ○26"), "h={h}:\n{s}");
            assert!(s.contains("◄─╰─"), "h={h}:\n{s}");
        }
        // Unit counts reconcile: shown + above + below == 86.
        let rows = structure_pane(&v, &wf, &nav, W, 16);
        let shown = rows.iter().filter(|r| plain(r).contains("svc-")).count();
        let marker = plain(row(&rows, "⋮"));
        let n = |what: &str| -> usize {
            marker
                .split(what)
                .next()
                .and_then(|l| l.rsplit('+').next())
                .and_then(|d| d.trim().parse().ok())
                .unwrap_or(0)
        };
        assert_eq!(shown + n(" above") + n(" below"), 86, "{marker}");
    }

    #[test]
    fn a_collapsed_fan_out_trims_to_its_density_line_in_a_cramped_pane() {
        let (v, wf) = (scale_view(2), scale_wf(2));
        let roomy = pane(&v, &wf, &NavState::default(), W, 80);
        assert!(
            roomy.contains("svc-59") && roomy.contains("+82 more"),
            "{roomy}"
        );
        let s = pane(&v, &wf, &NavState::default(), W, 12);
        // No misleading `⋮ +N` over the movers: the density line carries the
        // exact counts, so the whole mover list goes at once.
        assert!(s.contains("52/86 ✓52 ◐6 ✗2 ○26"), "{s}");
        assert!(!s.contains('⋮'), "{s}");
        assert!(!s.contains("svc-"), "{s}");
        assert!(s.lines().count() <= 12, "{s}");
    }

    #[test]
    fn a_unit_carrying_command_node_gets_its_block_under_its_row() {
        let wf = Workflow::parse(
            r#"
name: cmd
steps:
  - id: sweep
    run: { cmd: scan, args: [all] }
    for_each: '["a", "b", "c"]'
"#,
        )
        .unwrap();
        let mut v = RunView::default();
        start(&mut v, "sweep", StepKind::Run, None, None, None);
        give_units(
            &mut v,
            "sweep",
            &[UnitStatus::Done, UnitStatus::Running, UnitStatus::Failed],
        );
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 40);
        insta::assert_snapshot!(render_plain(&rows));
        let s = render_plain(&rows);
        assert!(plain(row(&rows, "sweep")).contains("run · scan · for_each"));
        assert!(s.contains("1/3 ✓1 ◐1 ✗1"), "{s}");
        assert!(!s.contains("more"), "all three units fit: {s}");
        assert_eq!(s.matches("svc-").count(), 3, "{s}");
        // A `run:` node that has produced no units is a plain row.
        let mut none = RunView::default();
        start(&mut none, "sweep", StepKind::Run, None, None, None);
        let s = pane(&none, &wf, &NavState::default(), W, 40);
        assert_eq!(s.lines().count(), 1, "{s}");
    }

    #[test]
    fn a_parallel_step_gets_its_aggregate_in_place_of_a_spacer() {
        let par = Workflow::parse(PARALLEL_WF).unwrap();
        let v = parallel_view(&[("writer", Some(true)), ("writer", None), ("reviewer", None)]);
        let plain_rows = render_plain(&structure_rows(&v, &par, &NavState::default()));
        let s = pane(&v, &par, &NavState::default(), W, 40);
        // Same height as the DAG: the density line took the first spacer's
        // row, and the sub-steps stay the units.
        assert_eq!(s.lines().count(), plain_rows.lines().count(), "{s}");
        assert!(s.contains("1/3 ✓1 ◐2"), "{s}");
        assert!(plain(row(
            &structure_pane(&v, &par, &NavState::default(), W, 40),
            "1/3"
        ))
        .starts_with("  │ │ "));
        assert_eq!(s.matches("├─ spec").count(), 1, "{s}");
    }

    // ---- structure_pane: phases -----------------------------------------------------

    /// Two split→join phases in a row, `pre` before and `post` after.
    const PHASES_WF: &str = r#"
name: phases
steps:
  - id: pre
    agent: scanner
    prompt: p
    next: [recon1]
  - id: recon1
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
    split: [c, d]
  - id: c
    agent: cx
    prompt: p
    next: [join2]
  - id: d
    agent: dx
    prompt: p
    next: [join2]
  - id: join2
    join: { wait: all }
    next: [post]
  - id: post
    agent: poster
    prompt: p
"#;

    /// Phase 1 settled (`a` done, `b` skipped); phase 2 open: `recon2` done,
    /// `c` running, `d` and the join not reached.
    fn phases_view() -> RunView {
        let mut v = RunView::default();
        start(&mut v, "pre", StepKind::Linear, Some("scanner"), None, None);
        complete(&mut v, "pre", 3_000, None);
        start(&mut v, "recon1", StepKind::Split, None, None, None);
        complete(&mut v, "recon1", 1_000, None);
        start(&mut v, "a", StepKind::Linear, Some("ax"), None, None);
        complete(&mut v, "a", 4_000, None);
        v.apply(&Event::StepSkipped {
            run_id: "r".into(),
            step_id: "b".into(),
            reason: "not needed".into(),
        });
        start(&mut v, "join1", StepKind::Join, None, None, None);
        complete(&mut v, "join1", 100, None);
        start(&mut v, "recon2", StepKind::Split, None, None, None);
        complete(&mut v, "recon2", 1_000, None);
        start(&mut v, "c", StepKind::Linear, Some("cx"), None, None);
        v
    }

    fn phases_wf() -> Workflow {
        Workflow::parse(PHASES_WF).expect("phases workflow parses")
    }

    #[test]
    fn phase_spans_pair_each_split_with_its_join() {
        let wf = phases_wf();
        let v = phases_view();
        let rows = render_rows(&wf, |id| node_status_of(&v, id));
        let painted = paint_rows(&v, &wf, &NavState::default(), &rows, true);
        let groups = build_groups(&v, &wf, &NavState::default(), &rows, painted);
        assert_eq!(phase_spans(&wf, &groups), vec![(1, 4), (5, 8)]);
    }

    #[test]
    fn a_settled_phase_folds_and_the_frontier_phase_stays_open() {
        let (v, wf) = (phases_view(), phases_wf());
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 60);
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);

        // Phase 1 is one row: aggregate status, step count, the real counts.
        let phase = plain(row(&rows, "⟦"));
        assert_eq!(
            phase, "  ⟦ recon1 … join1 ⟧  phase · 4 steps ···· ✓ 3 done · 1 skipped",
            "{s}"
        );
        assert_eq!(
            *style_of(row(&rows, "⟦"), "⟦ "),
            Style::Status(Status::Complete)
        );
        // Its insides are not rows of their own.
        for hidden in ["recon1  split", "agent", "ax", "bx"] {
            assert!(!s.contains(hidden), "{hidden}:\n{s}");
        }
        assert!(!s.contains("├─ b"), "{s}");
        // Phase 2 holds the frontier: open, its lanes and join all there.
        assert_eq!(s.matches('⟦').count(), 1, "{s}");
        for open in ["recon2", "╭─ c", "╰─ d", "◐  c", "join2"] {
            assert!(s.contains(open), "{open}:\n{s}");
        }
        // The phase row keeps its connector to what precedes it.
        let at = rows.iter().position(|r| plain(r).contains('⟦')).unwrap();
        assert_eq!(plain(&rows[at - 1]), "  │");
    }

    #[test]
    fn the_cursor_entering_a_collapsed_phase_opens_it() {
        let (v, wf) = (phases_view(), phases_wf());
        // Parked on `a`, a step inside phase 1: that is the drill.
        let mut nav = nav_on(&v, "a");
        let rows = structure_pane(&v, &wf, &nav, W, 60);
        let s = render_plain(&rows);
        assert!(!s.contains('⟦'), "{s}");
        assert!(s.contains("▸ ✓  a"), "{s}");
        assert!(s.contains("recon1"), "{s}");
        // Drilling into the step (`enter`) keeps its phase open.
        nav.apply(NavKey::In, &v);
        assert!(!pane(&v, &wf, &nav, W, 60).contains('⟦'));
        // Walking the cursor down from `pre` into the phase opens it, and out
        // of it folds it again.
        let mut nav = nav_on(&v, "pre");
        assert!(pane(&v, &wf, &nav, W, 60).contains('⟦'));
        nav.apply(NavKey::Down, &v); // recon1
        assert!(!pane(&v, &wf, &nav, W, 60).contains('⟦'));
        // Back to following: it folds again.
        let rows = structure_pane(&v, &wf, &NavState::default(), W, 60);
        assert_eq!(render_plain(&rows).matches('⟦').count(), 1);
    }

    #[test]
    fn a_phase_that_is_not_fully_settled_stays_open() {
        let wf = phases_wf();
        // `b` not yet done: nothing running, but not entirely settled.
        let mut v = RunView::default();
        for (id, kind) in [
            ("pre", StepKind::Linear),
            ("recon1", StepKind::Split),
            ("a", StepKind::Linear),
        ] {
            start(&mut v, id, kind, None, None, None);
            complete(&mut v, id, 1_000, None);
        }
        assert!(!pane(&v, &wf, &NavState::default(), W, 60).contains('⟦'));
        // A failure inside an otherwise settled phase stays visible.
        let mut v = phases_view();
        v.apply(&Event::StepFailed {
            run_id: "r".into(),
            step_id: "join1".into(),
            error: "x".into(),
        });
        assert!(!pane(&v, &wf, &NavState::default(), W, 60).contains('⟦'));
    }

    #[test]
    fn a_split_that_is_not_a_clean_span_is_not_a_phase() {
        // `stray` is declared inside the span but is not part of the sub-DAG.
        let wf = Workflow::parse(
            r#"
name: messy
steps:
  - id: recon
    split: [a]
  - id: stray
    agent: x
    prompt: p
  - id: a
    agent: ax
    prompt: p
    next: [gather]
  - id: gather
    join: { wait: all }
"#,
        )
        .unwrap();
        let mut v = RunView::default();
        for id in ["recon", "stray", "a", "gather"] {
            start(&mut v, id, StepKind::Linear, None, None, None);
            complete(&mut v, id, 1_000, None);
        }
        let s = pane(&v, &wf, &NavState::default(), W, 60);
        assert!(!s.contains('⟦'), "{s}");
        for id in ["recon", "stray", "gather"] {
            assert!(s.contains(id), "{id}:\n{s}");
        }
    }

    #[test]
    fn nested_phases_fold_as_one_and_open_to_the_inner_frontier() {
        let wf = Workflow::parse(
            r#"
name: nested
steps:
  - id: outer
    split: [a, inner]
  - id: a
    agent: ax
    prompt: p
    next: [done]
  - id: inner
    split: [b, c]
  - id: b
    agent: bx
    prompt: p
    next: [ij]
  - id: c
    agent: cx
    prompt: p
    next: [ij]
  - id: ij
    join: { wait: all }
    next: [done]
  - id: done
    join: { wait: all }
"#,
        )
        .unwrap();
        let ids = ["outer", "a", "inner", "b", "c", "ij", "done"];
        let mut v = RunView::default();
        for id in ids {
            start(&mut v, id, StepKind::Linear, None, None, None);
            complete(&mut v, id, 1_000, None);
        }
        // All settled: the outer phase swallows the inner one — one row.
        let s = pane(&v, &wf, &NavState::default(), W, 60);
        assert_eq!(s.matches('⟦').count(), 1, "{s}");
        assert!(s.contains("⟦ outer … done ⟧  phase · 7 steps"), "{s}");
        // `b` running (inside the inner phase): both stay open.
        let mut v = RunView::default();
        for id in ["outer", "a", "inner"] {
            start(&mut v, id, StepKind::Linear, None, None, None);
            complete(&mut v, id, 1_000, None);
        }
        start(&mut v, "b", StepKind::Linear, None, None, None);
        let s = pane(&v, &wf, &NavState::default(), W, 60);
        assert!(!s.contains('⟦'), "{s}");
        // `a` running, the inner phase wholly settled: only the inner folds.
        let mut v = RunView::default();
        start(&mut v, "outer", StepKind::Split, None, None, None);
        complete(&mut v, "outer", 1_000, None);
        start(&mut v, "a", StepKind::Linear, None, None, None);
        for id in ["inner", "b", "c", "ij"] {
            start(&mut v, id, StepKind::Linear, None, None, None);
            complete(&mut v, id, 1_000, None);
        }
        let s = pane(&v, &wf, &NavState::default(), W, 60);
        assert_eq!(s.matches('⟦').count(), 1, "{s}");
        assert!(s.contains("⟦ inner … ij ⟧  phase · 4 steps"), "{s}");
        assert!(s.contains("◐  a"), "{s}");
    }

    // ---- structure_pane: loops ----------------------------------------------------

    #[test]
    fn a_loop_folds_whole_or_not_at_all() {
        let wf = wf();
        let mut v = live();
        // `refine` is running (critique mid-iteration): its settled member
        // `gen` keeps its frame — nothing in a loop folds out from under it.
        for h in 20..=40 {
            let s = pane(&v, &wf, &NavState::default(), W, h);
            assert!(s.contains("↻ loop:refine"), "h={h}:\n{s}");
            assert!(s.contains("↺ loop:refine"), "h={h}:\n{s}");
            assert!(
                s.contains("generator") && s.contains("critic"),
                "h={h}:\n{s}"
            );
        }
        // Once the loop settles, a pane that must shrink folds it whole —
        // frame and members together, header and footer never apart.
        complete(&mut v, "critique", 3_000, None);
        complete(&mut v, "loop:refine", 10_000, None);
        let mut folded_somewhere = false;
        for h in 12..=40 {
            let s = pane(&v, &wf, &NavState::default(), W, h);
            if s.contains('⋮') {
                continue; // windowed by the last resort: not a fold decision
            }
            let (header, footer) = (s.contains("↻ loop:refine"), s.contains("↺ loop:refine"));
            assert_eq!(header, footer, "h={h}:\n{s}");
            let (gen, crit) = (s.contains("generator"), s.contains("critic"));
            assert_eq!(gen, crit, "h={h}:\n{s}");
            assert_eq!(gen, header, "h={h}:\n{s}");
            folded_somewhere |= !header;
        }
        assert!(folded_somewhere, "the settled loop folds when pressed");
        // The operator parked on the loop keeps it open.
        let nav = nav_on(&v, "loop:refine");
        let s = pane(&v, &wf, &nav, W, 22);
        assert!(!s.contains('⋮'), "{s}");
        assert!(
            s.contains("↻ loop:refine") && s.contains("↺ loop:refine"),
            "{s}"
        );
        assert!(s.contains("generator") && s.contains("critic"), "{s}");
    }

    #[test]
    fn a_pending_loop_folds_whole_and_a_fan_out_in_a_loop_keeps_the_gutter() {
        // Not started at all: under pressure the pending frame folds whole.
        let wf = wf();
        let mut folded = false;
        for h in 3..=40 {
            let s = pane(&RunView::default(), &wf, &NavState::default(), W, h);
            if s.contains('⋮') {
                continue;
            }
            let (header, footer) = (s.contains("↻ loop:refine"), s.contains("↺ loop:refine"));
            assert_eq!(header, footer, "h={h}:\n{s}");
            assert_eq!(header, s.contains("generator"), "h={h}:\n{s}");
            folded |= !header;
        }
        assert!(folded, "the pending loop folds when pressed");

        // A `for_each` inside a loop: the block nests under the loop gutter
        // and the node's own rails.
        let looped = Workflow::parse(
            r#"
name: loopfan
steps:
  - id: hunt
    for_each: '["a", "b"]'
    agent: worker
    prompt: p
  - id: judge
    agent: judge
    prompt: p
    depends_on: [hunt]
loops:
  again:
    nodes: [hunt, judge]
    until: "{{ steps.judge.output }}"
    max_iterations: 3
"#,
        )
        .unwrap();
        let mut v = RunView::default();
        start(&mut v, "loop:again", StepKind::Loop, None, None, None);
        start(&mut v, "hunt", StepKind::ForEach, None, None, None);
        give_units(&mut v, "hunt", &statuses_86());
        let rows = structure_pane(&v, &looped, &NavState::default(), W, 40);
        let s = render_plain(&rows);
        insta::assert_snapshot!(s);
        assert!(plain(row(&rows, "52/86")).starts_with("  │ │ │ "), "{s}");
        assert!(
            plain(row(&rows, "svc-59")).starts_with("  │ │ ◐├─ svc-59"),
            "{s}"
        );
        // The loop's live gutter lights the block's outermost rail.
        assert_eq!(
            *style_of(row(&rows, "svc-59"), "│"),
            Style::Status(Status::Working)
        );
        assert!(!s.contains("runtime items"), "{s}");
        assert!(s.contains("↺ loop:again"), "{s}");
    }

    // ---- the existing DAG is untouched ----------------------------------------------

    #[test]
    fn structure_rows_still_passes_the_placeholder_through() {
        // `structure_rows` is the unfolded DAG; only the pane swaps in the
        // live block.
        let wf = scale_wf(2);
        let v = scale_view(2);
        let s = render_plain(&structure_rows(&v, &wf, &NavState::default()));
        assert!(s.contains("runtime items"), "{s}");
        assert!(!s.contains("52/86"), "{s}");
    }
}
