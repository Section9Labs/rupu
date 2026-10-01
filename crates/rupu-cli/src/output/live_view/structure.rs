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

use rupu_app_canvas::git_graph::{
    render_rows, GraphCell, GraphRow, LOOP_FOOTER_PREFIX, LOOP_HEADER_PREFIX,
};
use rupu_app_canvas::node_status::NodeStatus;
use rupu_orchestrator::{is_approval_gate, Step, Workflow};

use crate::output::live_view::layout::printable;
use crate::output::live_view::nav::NavState;
use crate::output::live_view::row::{Line, Segment, Style};
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

/// `···· <glyph> <state> @host` — the protected tail of a node row; absent
/// when the run has nothing to say about the node yet.
fn right_hand_state(mut line: Line, facts: &Facts<'_>) -> Line {
    let host = facts.host.filter(|h| !h.is_empty());
    if facts.state.is_none() && host.is_none() {
        return line;
    }
    line = line.dim(LEADER);
    if let Some((ns, text)) = &facts.state {
        let st = palette_status(*ns);
        line = line.status(st, format!("{} {}", st.glyph(), printable(text)));
    }
    if let Some(host) = host {
        let gap = if facts.state.is_some() { " " } else { "" };
        line = line.dim(format!("{gap}@{}", printable(host)));
    }
    line
}

/// The structure pane's rows: every app-canvas row of `wf`, coloured and
/// overlaid from `view`. `nav`'s chosen step gets the `▸` marker. One line
/// per row, unclipped.
pub fn structure_rows(view: &RunView, wf: &Workflow, nav: &NavState) -> Vec<Line> {
    let chosen = nav.chosen_step(view).map(|s| s.step_id.as_str());
    let rows = render_rows(wf, |id| node_status_of(view, id));

    let mut block: Option<Block<'_>> = None;
    let mut gutter: Option<NodeStatus> = None;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let frame = loop_frame(row);
        let facts = match (&frame, row.anchor_step_id()) {
            (Some(LoopFrame::Header(name)), _) => {
                let mut facts = facts_of_loop(view, wf, name);
                // The loop is a step of its own (`loop:<name>`) to the
                // navigator, but has no anchored row: its header is it.
                facts.selected = chosen.and_then(|c| c.strip_prefix("loop:")) == Some(*name);
                facts
            }
            (Some(LoopFrame::Footer), _) => Facts {
                status: Some(gutter.unwrap_or(NodeStatus::Waiting)),
                tint_rails: true,
                dim_label: true,
                ..Facts::default()
            },
            (None, None) => Facts::default(),
            (None, Some(id)) => match block.as_mut().and_then(|b| b.next_member(id)) {
                Some(pos) => block
                    .as_ref()
                    .map_or_else(Facts::default, |b| b.member_facts(pos)),
                None => {
                    let step = wf.steps.iter().find(|s| s.id == id);
                    let live = find_step(view, id);
                    block = step.and_then(|s| Block::open(s, live));
                    let mut facts = step.map_or_else(Facts::default, |s| facts_of_step(s, live));
                    facts.selected = chosen == Some(id);
                    facts
                }
            },
        };
        out.push(paint(row, &facts, gutter));
        match frame {
            Some(LoopFrame::Header(_)) => gutter = facts.status,
            Some(LoopFrame::Footer) => gutter = None,
            None => {}
        }
    }
    out
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
}
