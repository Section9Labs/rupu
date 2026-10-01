//! Walk a `Workflow` and emit a structured `Vec<GraphRow>` for the
//! git-graph view. Each row is a sequence of typed cells; the GPUI
//! renderer in `rupu-app::view::graph` paints them as monospace
//! text spans.
//!
//! Visual model (vertical spine, `●/│/├/╭/╰/◄` glyphs):
//!
//! ```text
//! ●  classify_input        waiting
//! │
//! ├─╭─ review_panel        panel · 3 panelists
//! │ │
//! │ ●─ security-reviewer   waiting
//! │ ●─ perf-reviewer       waiting
//! │ ●─ style-reviewer      waiting
//! │ │
//! │ ◄─╯
//! │
//! ●  post_to_issue         waiting
//! ```

use crate::node_status::NodeStatus;
use rupu_orchestrator::workflow::{loop_of_step, Branch, Join, JoinWait, JoinWaitKeyword, LoopDef};
use rupu_orchestrator::{is_approval_gate, Workflow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One row of the git-graph rendering. The GPUI renderer paints
/// cells left-to-right in monospace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRow {
    pub cells: Vec<GraphCell>,
    /// If this row represents a named step, this is its `(step_id, status)`.
    /// `None` for pure-connector rows (spine pipes, panel spacers, merge lines).
    pub anchor: Option<(String, NodeStatus)>,
}

impl GraphRow {
    /// Return the anchor step id, if any.
    pub fn anchor_step_id(&self) -> Option<&str> {
        self.anchor.as_ref().map(|(id, _)| id.as_str())
    }

    /// Return the anchor step status, if any.
    pub fn anchor_status(&self) -> Option<NodeStatus> {
        self.anchor.as_ref().map(|(_, status)| *status)
    }
}

/// One typed cell within a row. The renderer maps each variant to a
/// short monospace string (1-2 chars) + a foreground color.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphCell {
    /// A `│` vertical bar (parent rail). Color = status of the step
    /// whose lifetime this row falls within.
    Pipe(NodeStatus),
    /// A branch glyph (see `BranchGlyph` for the variants). Color =
    /// status of the step that owns this branch.
    Branch(BranchGlyph, NodeStatus),
    /// A `●` (or status-specific glyph) marking a step's row.
    Bullet(NodeStatus),
    /// Run of `n` literal space characters. Used for column-aligning
    /// the label after the bullet/branch.
    Space(u16),
    /// The step's identifier or panelist agent name.
    Label(String),
    /// Dim meta text following the label (kind label, panelist count,
    /// etc.). Renderer paints this in `palette::TEXT_DIMMEST`.
    Meta(String),
}

/// Branch glyph vocabulary, named for visual orientation. Each
/// variant maps to a 2-character string in the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchGlyph {
    Top,
    Mid,
    Bot,
    Merge,
}

impl BranchGlyph {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "╭─",
            Self::Mid => "├─",
            Self::Bot => "╰─",
            Self::Merge => "◄─",
        }
    }
}

/// Label prefix of a loop frame's header row (`↻ loop:<name>`). A loop is
/// not a step, so the header carries no anchor; a consumer that overlays
/// live state on it recognises the row by this prefix. Exported so the
/// emitter and its consumers share one spelling and cannot drift.
pub const LOOP_HEADER_PREFIX: &str = "↻ loop:";

/// Label prefix of a loop frame's loop-back footer row (`↺ loop:<name>`).
pub const LOOP_FOOTER_PREFIX: &str = "↺ loop:";

/// Render workflow as graph rows, using `status_lookup` to pick the
/// `NodeStatus` for each step. Pass `|_| NodeStatus::Waiting` for the
/// static (no live run) case.
///
/// Steps that belong to a `loops:` entry are framed: a `↻ loop:<name>`
/// header row before the first member, the members nested one gutter
/// (`│ `) deep, and a `↺` loop-back footer after the last. Framing only
/// adds rows around the members — each member still renders through its
/// normal arm. It is derived from `wf.steps` order and assumes a loop's
/// members are adjacent; the parser does not require that, so a loop
/// whose members are split by a non-member re-opens its frame, marked
/// `continued`, rather than re-ordering the steps.
pub fn render_rows<F>(wf: &Workflow, status_lookup: F) -> Vec<GraphRow>
where
    F: Fn(&str) -> NodeStatus,
{
    let mut rows = Vec::new();
    let total = wf.steps.len();
    // The loop whose frame is currently open, and every loop already
    // framed once (to mark a re-opened frame `continued`).
    let mut open_loop: Option<&str> = None;
    let mut framed_loops: BTreeSet<&str> = BTreeSet::new();

    for (i, step) in wf.steps.iter().enumerate() {
        let _is_last = i == total - 1;
        let step_loop = loop_of_step(wf, &step.id);

        // Leaving a loop: close its frame before the next connector.
        if step_loop != open_loop {
            if let Some(name) = open_loop {
                rows.push(loop_footer(name));
            }
        }

        // Connector row (a `│` spine) BEFORE every step except the
        // first — keeps the vertical thread continuous between rows.
        // Between two members of the same loop it sits inside the gutter.
        if i > 0 {
            let mut spine = spine_only();
            if step_loop.is_some() && step_loop == open_loop {
                nest_in_loop_gutter(&mut spine);
            }
            rows.push(spine);
        }

        // Entering a loop: open its frame after the connector.
        let entering = step_loop.filter(|name| open_loop != Some(*name));
        if let Some((name, def)) = entering.and_then(|name| wf.loops.get_key_value(name)) {
            let continued = !framed_loops.insert(name.as_str());
            rows.push(loop_header(name, def, continued));
        }
        open_loop = step_loop;

        let first_row = rows.len();
        if is_approval_gate(step) {
            emit_gate_step(&mut rows, step, &status_lookup);
        } else if step.run.is_some() {
            // Checked before `for_each` — a `for_each:` + `run:` step is
            // a Run node whose units fan out, matching how the runner
            // records its StepKind.
            emit_run_step(&mut rows, step, &status_lookup);
        } else if step.action.is_some() {
            emit_action_step(&mut rows, step, &status_lookup);
        } else if let Some(panel) = &step.panel {
            emit_panel_step(&mut rows, &step.id, &panel.panelists, &status_lookup);
        } else if let Some(subs) = &step.parallel {
            emit_parallel_step(&mut rows, &step.id, subs, &status_lookup);
        } else if let Some(targets) = &step.split {
            emit_split_step(&mut rows, &step.id, targets, &status_lookup);
        } else if let Some(join) = &step.join {
            emit_join_step(&mut rows, &step.id, join, &status_lookup);
        } else if let Some(branch) = &step.branch {
            emit_branch_step(&mut rows, &step.id, branch, &status_lookup);
        } else if step.for_each.is_some() {
            emit_for_each_step(&mut rows, &step.id, &status_lookup);
        } else {
            // Plain linear step. agent may be None if the step uses
            // some other mode (dispatch agent in-prompt etc.); render
            // a blank meta in that case.
            let agent = step.agent.as_deref().unwrap_or("").to_string();
            emit_linear_step(&mut rows, &step.id, agent, &status_lookup);
        }

        if step_loop.is_some() {
            for row in &mut rows[first_row..] {
                nest_in_loop_gutter(row);
            }
        }
    }

    // A loop that runs to the end of the workflow still closes.
    if let Some(name) = open_loop {
        rows.push(loop_footer(name));
    }

    rows
}

/// Collapse a template/expression to a single line so a multi-line YAML
/// block (`until: |`) can't break the one-row-per-node layout.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Prefix a row with the loop gutter (`│ `), nesting it one level deeper.
fn nest_in_loop_gutter(row: &mut GraphRow) {
    row.cells.splice(
        0..0,
        [GraphCell::Pipe(NodeStatus::Waiting), GraphCell::Space(1)],
    );
}

/// The opening row of a loop frame: `├─╭─ ↻ loop:<name>  until <expr> · max <n>`.
///
/// A loop is not a step, so the row carries no anchor. `continued` marks
/// a frame re-opened for a loop whose members are not adjacent in step
/// order.
fn loop_header(name: &str, def: &LoopDef, continued: bool) -> GraphRow {
    let mut meta = format!(
        "until {} · max {}",
        one_line(&def.until),
        def.max_iterations
    );
    if continued {
        meta.push_str(" · continued");
    }
    GraphRow {
        cells: vec![
            GraphCell::Branch(BranchGlyph::Mid, NodeStatus::Waiting),
            GraphCell::Branch(BranchGlyph::Top, NodeStatus::Waiting),
            GraphCell::Space(1),
            GraphCell::Label(format!("{LOOP_HEADER_PREFIX}{name}")),
            GraphCell::Space(2),
            GraphCell::Meta(meta),
        ],
        anchor: None,
    }
}

/// The closing row of a loop frame: `│ ◄─ ↺ loop:<name>  loop-back`.
fn loop_footer(name: &str) -> GraphRow {
    GraphRow {
        cells: vec![
            GraphCell::Pipe(NodeStatus::Waiting),
            GraphCell::Space(1),
            GraphCell::Branch(BranchGlyph::Merge, NodeStatus::Waiting),
            GraphCell::Space(1),
            GraphCell::Label(format!("{LOOP_FOOTER_PREFIX}{name}")),
            GraphCell::Space(2),
            GraphCell::Meta("loop-back".into()),
        ],
        anchor: None,
    }
}

fn emit_parallel_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    sub_steps: &[rupu_orchestrator::SubStep],
    status_lookup: &F,
) {
    let step_status = status_lookup(step_id);
    let n = sub_steps.len();
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Branch(BranchGlyph::Mid, step_status),
            GraphCell::Branch(BranchGlyph::Top, step_status),
            GraphCell::Space(1),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta(format!(
                "parallel · {n} sub-step{}",
                if n == 1 { "" } else { "s" }
            )),
        ],
        anchor: Some((step_id.to_string(), step_status)),
    });
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Pipe(step_status),
        ],
        anchor: None,
    });
    for sub in sub_steps {
        let sub_status = status_lookup(&sub.id);
        let mut cells = vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Bullet(sub_status),
            GraphCell::Branch(BranchGlyph::Mid, sub_status),
            GraphCell::Space(1),
            GraphCell::Label(sub.id.clone()),
        ];
        if !sub.agent.is_empty() {
            cells.push(GraphCell::Space(2));
            cells.push(GraphCell::Meta(sub.agent.clone()));
        }
        rows.push(GraphRow {
            cells,
            anchor: Some((sub.id.clone(), sub_status)),
        });
    }
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Pipe(step_status),
        ],
        anchor: None,
    });
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Branch(BranchGlyph::Merge, step_status),
            GraphCell::Branch(BranchGlyph::Bot, step_status),
        ],
        anchor: None,
    });
}

/// Emit a `split:` orchestration node: a bulleted node row carrying a
/// `split` meta, then one fork lane per target id.
///
/// The first lane opens with `╭─`, the last closes with `╰─`, and any
/// in between use `├─` (a lone target gets the closing `╰─`). Lanes are
/// coloured by the *target's* status. They deliberately carry no anchor:
/// every target is itself a top-level step that renders — and anchors —
/// on its own row, so anchoring the lane too would register the same
/// step id twice.
fn emit_split_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    targets: &[String],
    status_lookup: &F,
) {
    let status = status_lookup(step_id);
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Bullet(status),
            GraphCell::Space(2),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta("split".into()),
        ],
        anchor: Some((step_id.to_string(), status)),
    });
    for (i, target) in targets.iter().enumerate() {
        rows.push(GraphRow {
            cells: vec![
                GraphCell::Branch(lane_glyph(i, targets.len()), status_lookup(target)),
                GraphCell::Space(1),
                GraphCell::Label(target.clone()),
            ],
            anchor: None,
        });
    }
}

/// The glyph for lane `index` of a group of `len` lanes: the first opens
/// with `╭─`, the last closes with `╰─`, any in between use `├─`, and a
/// lone lane gets the closing `╰─`.
fn lane_glyph(index: usize, len: usize) -> BranchGlyph {
    if index + 1 == len {
        BranchGlyph::Bot
    } else if index == 0 {
        BranchGlyph::Top
    } else {
        BranchGlyph::Mid
    }
}

/// Emit a `join:` (barrier) orchestration node: `●◄─ <step_id>  join · wait:<policy>`.
///
/// The merge glyph marks the convergence point; `wait:` is `all`, `any`,
/// or the required inbound count.
fn emit_join_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    join: &Join,
    status_lookup: &F,
) {
    let status = status_lookup(step_id);
    let wait = match &join.wait {
        JoinWait::Keyword(JoinWaitKeyword::All) => "all".to_string(),
        JoinWait::Keyword(JoinWaitKeyword::Any) => "any".to_string(),
        JoinWait::Count { count } => count.to_string(),
    };
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Bullet(status),
            GraphCell::Branch(BranchGlyph::Merge, status),
            GraphCell::Space(1),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta(format!("join · wait:{wait}")),
        ],
        anchor: Some((step_id.to_string(), status)),
    });
}

/// Emit a `branch:` orchestration node: a bulleted node row carrying
/// `branch · when: <condition>`, then the `then` targets as one lane group
/// and the `else` targets as another.
///
/// The arms are told apart by a leading marker (`▶ then →` / `⊘ else →`,
/// a `Meta` cell) before the target id (a `Label`, as in `split` lanes).
/// An empty arm emits no lanes. Like split lanes they carry no anchor —
/// every target is a top-level step that anchors on its own row — and are
/// coloured by the *target's* status, so the arm a live run did not take
/// shows as skipped.
fn emit_branch_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    branch: &Branch,
    status_lookup: &F,
) {
    let status = status_lookup(step_id);
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Bullet(status),
            GraphCell::Space(2),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta(format!("branch · when: {}", one_line(&branch.condition))),
        ],
        anchor: Some((step_id.to_string(), status)),
    });
    emit_branch_arm(rows, "▶ then →", &branch.then, status_lookup);
    emit_branch_arm(rows, "⊘ else →", &branch.r#else, status_lookup);
}

/// One arm of a branch: a lane per target, each led by the arm `marker`.
fn emit_branch_arm<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    marker: &str,
    targets: &[String],
    status_lookup: &F,
) {
    for (i, target) in targets.iter().enumerate() {
        rows.push(GraphRow {
            cells: vec![
                GraphCell::Branch(lane_glyph(i, targets.len()), status_lookup(target)),
                GraphCell::Space(1),
                GraphCell::Meta(marker.to_string()),
                GraphCell::Space(1),
                GraphCell::Label(target.clone()),
            ],
            anchor: None,
        });
    }
}

fn emit_for_each_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    status_lookup: &F,
) {
    let step_status = status_lookup(step_id);
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Branch(BranchGlyph::Mid, step_status),
            GraphCell::Branch(BranchGlyph::Top, step_status),
            GraphCell::Space(1),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta("for_each · runtime fan-out".into()),
        ],
        anchor: Some((step_id.to_string(), step_status)),
    });
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Pipe(step_status),
        ],
        anchor: None,
    });
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Bullet(NodeStatus::Waiting),
            GraphCell::Branch(BranchGlyph::Mid, NodeStatus::Waiting),
            GraphCell::Space(1),
            GraphCell::Label("runtime items".into()),
        ],
        anchor: None,
    });
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Pipe(step_status),
        ],
        anchor: None,
    });
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(step_status),
            GraphCell::Space(1),
            GraphCell::Branch(BranchGlyph::Merge, step_status),
            GraphCell::Branch(BranchGlyph::Bot, step_status),
        ],
        anchor: None,
    });
}

/// Emit a single linear-step row: `● <step_id>   <meta>`.
fn emit_linear_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    meta: String,
    status_lookup: &F,
) {
    let status = status_lookup(step_id);
    let mut cells = Vec::new();
    cells.push(GraphCell::Bullet(status));
    cells.push(GraphCell::Space(2));
    cells.push(GraphCell::Label(step_id.to_string()));
    if !meta.is_empty() {
        cells.push(GraphCell::Space(2));
        cells.push(GraphCell::Meta(meta));
    }
    rows.push(GraphRow {
        cells,
        anchor: Some((step_id.to_string(), status)),
    });
}

/// Emit a single standalone-approval-gate row: `● <step_id>   gate[ · auto]`.
fn emit_gate_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step: &rupu_orchestrator::Step,
    status_lookup: &F,
) {
    let step_id = &step.id;
    let status = status_lookup(step_id);
    let mut meta = String::from("gate");
    let is_auto = step
        .approval
        .as_ref()
        .and_then(|a| a.auto_approve.as_ref())
        .is_some();
    if is_auto {
        meta.push_str(" · auto");
    }
    let cells = vec![
        GraphCell::Bullet(status),
        GraphCell::Space(2),
        GraphCell::Label(step_id.to_string()),
        GraphCell::Space(2),
        GraphCell::Meta(meta),
    ];
    rows.push(GraphRow {
        cells,
        anchor: Some((step_id.to_string(), status)),
    });
}

/// Emit a single action-step row: `● <step_id>   action · <tool>`.
/// Emit a `run:` (deterministic command) node.
///
/// The meta shows the executable and, when the step fans out, that it
/// does — an operator scanning the graph should be able to tell a
/// command node from an agent node without opening it.
fn emit_run_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step: &rupu_orchestrator::Step,
    status_lookup: &F,
) {
    let step_id = &step.id;
    let status = status_lookup(step_id);
    let cmd = step.run.as_ref().map(|r| r.cmd.as_str()).unwrap_or("?");
    let meta = if step.for_each.is_some() {
        format!("run · {cmd} · for_each")
    } else {
        format!("run · {cmd}")
    };
    let cells = vec![
        GraphCell::Bullet(status),
        GraphCell::Space(2),
        GraphCell::Label(step_id.to_string()),
        GraphCell::Space(2),
        GraphCell::Meta(meta),
    ];
    rows.push(GraphRow {
        cells,
        anchor: Some((step_id.to_string(), status)),
    });
}

fn emit_action_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step: &rupu_orchestrator::Step,
    status_lookup: &F,
) {
    let step_id = &step.id;
    let status = status_lookup(step_id);
    let meta = format!("action · {}", step.action.as_deref().unwrap_or("?"));
    let cells = vec![
        GraphCell::Bullet(status),
        GraphCell::Space(2),
        GraphCell::Label(step_id.to_string()),
        GraphCell::Space(2),
        GraphCell::Meta(meta),
    ];
    rows.push(GraphRow {
        cells,
        anchor: Some((step_id.to_string(), status)),
    });
}

/// Emit a panel block: header row + spacer + one row per panelist + spacer + close row.
///
/// The panel step's own status drives the header/spacer/close glyphs.
/// Each panelist row uses its agent name as the status-lookup key so
/// live runs can colour individual panelist nodes independently.
fn emit_panel_step<F: Fn(&str) -> NodeStatus>(
    rows: &mut Vec<GraphRow>,
    step_id: &str,
    panelists: &[String],
    status_lookup: &F,
) {
    let panel_status = status_lookup(step_id);
    let n = panelists.len();

    // Header: ├─╭─ <step_id>   panel · N panelists
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Branch(BranchGlyph::Mid, panel_status),
            GraphCell::Branch(BranchGlyph::Top, panel_status),
            GraphCell::Space(1),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta(format!(
                "panel · {n} panelist{}",
                if n == 1 { "" } else { "s" }
            )),
        ],
        anchor: Some((step_id.to_string(), panel_status)),
    });

    // Spacer: │ │
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(panel_status),
            GraphCell::Space(1),
            GraphCell::Pipe(panel_status),
        ],
        anchor: None,
    });

    // One row per panelist: │ ●─ <agent>
    // The panelist agent name is used as the lookup key.
    for agent in panelists {
        let panelist_status = status_lookup(agent.as_str());
        rows.push(GraphRow {
            cells: vec![
                GraphCell::Pipe(panel_status),
                GraphCell::Space(1),
                GraphCell::Bullet(panelist_status),
                GraphCell::Branch(BranchGlyph::Mid, panelist_status),
                GraphCell::Space(1),
                GraphCell::Label(agent.clone()),
            ],
            anchor: Some((agent.clone(), panelist_status)),
        });
    }

    // Spacer: │ │
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(panel_status),
            GraphCell::Space(1),
            GraphCell::Pipe(panel_status),
        ],
        anchor: None,
    });

    // Close row: │ ◄─╯
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(panel_status),
            GraphCell::Space(1),
            GraphCell::Branch(BranchGlyph::Merge, panel_status),
            GraphCell::Branch(BranchGlyph::Bot, panel_status),
        ],
        anchor: None,
    });
}

/// A `│` connector row used between steps.
fn spine_only() -> GraphRow {
    GraphRow {
        cells: vec![GraphCell::Pipe(NodeStatus::Waiting)],
        anchor: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::Workflow;

    fn parse(yaml: &str) -> Workflow {
        Workflow::parse(yaml).expect("test workflow parses")
    }

    #[test]
    fn linear_3_step_workflow_emits_5_rows() {
        // 3 step rows + 2 connector rows = 5 total
        let wf = parse(
            r#"
name: t
steps:
  - id: a
    agent: aa
    actions: []
    prompt: hi
  - id: b
    agent: bb
    actions: []
    prompt: hi
  - id: c
    agent: cc
    actions: []
    prompt: hi
"#,
        );
        let rows = render_rows(&wf, |_| NodeStatus::Waiting);
        assert_eq!(
            rows.len(),
            5,
            "expected 3 step + 2 connector = 5 rows, got: {rows:#?}"
        );

        // Row 0: step a (bullet + space + label + space + meta)
        assert!(matches!(
            rows[0].cells[0],
            GraphCell::Bullet(NodeStatus::Waiting)
        ));
        assert!(rows[0]
            .cells
            .iter()
            .any(|c| matches!(c, GraphCell::Label(s) if s == "a")));

        // Row 1: spine connector
        assert_eq!(rows[1].cells.len(), 1);
        assert!(matches!(
            rows[1].cells[0],
            GraphCell::Pipe(NodeStatus::Waiting)
        ));
    }

    #[test]
    fn panel_step_emits_header_plus_panelists_plus_close() {
        let wf = parse(
            r#"
name: r
steps:
  - id: classify
    agent: classifier
    actions: []
    prompt: hi
  - id: review_panel
    actions: []
    panel:
      panelists:
        - security-reviewer
        - perf-reviewer
        - style-reviewer
      subject: review
"#,
        );
        let rows = render_rows(&wf, |_| NodeStatus::Waiting);

        // 1 row (classify) + 1 connector + 6 rows panel (header + spacer + 3 panelists + spacer + close)
        // = 8 rows total
        // Wait actually let me recount:
        //   classify: 1 row
        //   spine connector: 1 row
        //   panel header: 1 row
        //   panel spacer: 1 row
        //   3 panelists: 3 rows
        //   panel spacer: 1 row
        //   panel close: 1 row
        // = 1 + 1 + 1 + 1 + 3 + 1 + 1 = 9 rows
        assert_eq!(rows.len(), 9, "expected 9 rows; got {rows:#?}");

        // The panel header should be at index 2 (after classify + connector).
        let header = &rows[2];
        assert!(matches!(
            header.cells[0],
            GraphCell::Branch(BranchGlyph::Mid, _)
        ));
        assert!(matches!(
            header.cells[1],
            GraphCell::Branch(BranchGlyph::Top, _)
        ));
        assert!(header
            .cells
            .iter()
            .any(|c| matches!(c, GraphCell::Label(s) if s == "review_panel")));
        assert!(header
            .cells
            .iter()
            .any(|c| matches!(c, GraphCell::Meta(s) if s.contains("3 panelist"))));

        // The close row at the end of the panel must contain Merge + Bot
        let close = rows
            .iter()
            .rev()
            .find(|r| {
                r.cells
                    .iter()
                    .any(|c| matches!(c, GraphCell::Branch(BranchGlyph::Merge, _)))
            })
            .expect("merge close row");
        assert!(close
            .cells
            .iter()
            .any(|c| matches!(c, GraphCell::Branch(BranchGlyph::Bot, _))));
    }

    #[test]
    fn single_step_workflow_emits_one_row() {
        let wf = parse(
            r#"
name: e
steps:
  - id: x
    agent: xa
    actions: []
    prompt: hi
"#,
        );
        let rows = render_rows(&wf, |_| NodeStatus::Waiting);
        assert_eq!(rows.len(), 1);
        assert!(matches!(
            rows[0].cells[0],
            GraphCell::Bullet(NodeStatus::Waiting)
        ));
        assert!(rows[0]
            .cells
            .iter()
            .any(|c| matches!(c, GraphCell::Label(s) if s == "x")));
    }

    #[test]
    fn parallel_step_emits_nested_substeps() {
        let wf = parse(
            r#"
name: p
steps:
  - id: gather
    parallel:
      - id: spec
        agent: writer
        prompt: hi
      - id: verify
        agent: reviewer
        prompt: hi
    actions: []
"#,
        );
        let rows = render_rows(&wf, |_| NodeStatus::Waiting);
        assert!(rows.iter().any(|row| row
            .cells
            .iter()
            .any(|cell| matches!(cell, GraphCell::Label(label) if label == "spec"))));
        assert!(rows.iter().any(|row| row
            .cells
            .iter()
            .any(|cell| matches!(cell, GraphCell::Label(label) if label == "verify"))));
        assert!(rows.iter().any(|row| row
            .cells
            .iter()
            .any(|cell| matches!(cell, GraphCell::Branch(BranchGlyph::Merge, _)))));
    }
}

#[cfg(test)]
mod run_step_rows_tests {
    use super::*;
    use rupu_orchestrator::Workflow;

    fn rows_for(yaml: &str) -> Vec<GraphRow> {
        let wf = Workflow::parse(yaml).expect("workflow parses");
        render_rows(&wf, |_| NodeStatus::Waiting)
    }

    fn metas(rows: &[GraphRow]) -> Vec<String> {
        rows.iter()
            .flat_map(|r| r.cells.iter())
            .filter_map(|c| match c {
                GraphCell::Meta(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn renders_a_linear_run_node_with_its_command() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: score
    run: { cmd: python3, args: ["score.py"] }
"#,
        );
        assert_eq!(metas(&rows), vec!["run · python3"]);
    }

    #[test]
    fn renders_a_fanout_run_node_distinctly() {
        // An operator scanning the graph must be able to tell a fan-out
        // command node from a single one without opening it.
        let rows = rows_for(
            r#"
name: t
steps:
  - id: score
    for_each: '["a"]'
    run: { cmd: python3 }
"#,
        );
        assert_eq!(metas(&rows), vec!["run · python3 · for_each"]);
    }

    #[test]
    fn a_run_node_is_not_rendered_as_a_plain_for_each() {
        // Regression guard: `for_each` is checked AFTER `run` in
        // render_rows, mirroring the runner's StepKind precedence.
        let rows = rows_for(
            r#"
name: t
steps:
  - id: score
    for_each: '["a"]'
    run: { cmd: echo }
"#,
        );
        assert!(
            !metas(&rows).iter().any(|m| m == "for_each"),
            "must not fall through to the agent for_each renderer"
        );
    }

    #[test]
    fn run_node_anchors_for_status_painting() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: score
    run: { cmd: echo }
"#,
        );
        assert!(
            rows.iter()
                .any(|r| r.anchor.as_ref().map(|(id, _)| id.as_str()) == Some("score")),
            "the run node must anchor so live status can paint it"
        );
    }
}

#[cfg(test)]
mod split_join_rows_tests {
    use super::*;
    use rupu_orchestrator::Workflow;

    /// a -> fan (split [b, c, d]) -> gather (join) with the given wait
    /// policy YAML (`all`, `any`, or `{ count: 2 }`).
    fn fanout_yaml(wait: &str) -> String {
        format!(
            r#"
name: t
steps:
  - id: a
    agent: x
    prompt: p
    next: [fan]
  - id: fan
    split: [b, c, d]
  - id: b
    agent: x
    prompt: p
    next: [gather]
  - id: c
    agent: x
    prompt: p
    next: [gather]
  - id: d
    agent: x
    prompt: p
    next: [gather]
  - id: gather
    join: {{ wait: {wait} }}
"#
        )
    }

    fn rows_with<F: Fn(&str) -> NodeStatus>(yaml: &str, lookup: F) -> Vec<GraphRow> {
        let wf = Workflow::parse(yaml).expect("workflow parses");
        render_rows(&wf, lookup)
    }

    fn rows_for(yaml: &str) -> Vec<GraphRow> {
        rows_with(yaml, |_| NodeStatus::Waiting)
    }

    fn metas(rows: &[GraphRow]) -> Vec<String> {
        rows.iter()
            .flat_map(|r| r.cells.iter())
            .filter_map(|c| match c {
                GraphCell::Meta(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    fn label_of(row: &GraphRow) -> Option<&str> {
        row.cells.iter().find_map(|c| match c {
            GraphCell::Label(l) => Some(l.as_str()),
            _ => None,
        })
    }

    /// The fork-lane rows: a leading `Branch` cell and no anchor (the
    /// targets are real top-level steps that anchor on their own rows).
    fn lane_rows(rows: &[GraphRow]) -> Vec<(BranchGlyph, NodeStatus, String)> {
        rows.iter()
            .filter(|r| r.anchor.is_none())
            .filter_map(|r| match (r.cells.first(), label_of(r)) {
                (Some(GraphCell::Branch(g, s)), Some(l)) => Some((*g, *s, l.to_string())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn split_renders_one_fork_lane_per_target() {
        let rows = rows_for(&fanout_yaml("all"));
        let lanes = lane_rows(&rows);
        assert_eq!(
            lanes,
            vec![
                (BranchGlyph::Top, NodeStatus::Waiting, "b".to_string()),
                (BranchGlyph::Mid, NodeStatus::Waiting, "c".to_string()),
                (BranchGlyph::Bot, NodeStatus::Waiting, "d".to_string()),
            ],
            "split targets must render as Top/Mid/Bot lanes in declared order: {rows:#?}"
        );
    }

    #[test]
    fn split_node_row_carries_split_meta_and_anchors() {
        let rows = rows_for(&fanout_yaml("all"));
        let node = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("fan"))
            .expect("the split node must anchor so the CLI can select + paint it");
        assert_eq!(label_of(node), Some("fan"));
        assert!(
            node.cells
                .iter()
                .any(|c| matches!(c, GraphCell::Meta(m) if m == "split")),
            "split node row must carry a `split` meta: {node:#?}"
        );
        assert!(
            matches!(node.cells.first(), Some(GraphCell::Bullet(_))),
            "split node is a bulleted node row"
        );
    }

    #[test]
    fn split_lanes_are_coloured_by_their_target_status() {
        let rows = rows_with(&fanout_yaml("all"), |id| match id {
            "fan" => NodeStatus::Active,
            "b" => NodeStatus::Complete,
            _ => NodeStatus::Waiting,
        });
        let lanes = lane_rows(&rows);
        assert_eq!(lanes[0].1, NodeStatus::Complete, "lane b");
        assert_eq!(lanes[1].1, NodeStatus::Waiting, "lane c");
        let node = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("fan"))
            .unwrap();
        assert_eq!(node.anchor_status(), Some(NodeStatus::Active));
    }

    #[test]
    fn a_single_target_split_ends_its_lane_instead_of_opening_it() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: fan
    split: [b]
  - id: b
    agent: x
    prompt: p
"#,
        );
        let lanes = lane_rows(&rows);
        assert_eq!(
            lanes,
            vec![(BranchGlyph::Bot, NodeStatus::Waiting, "b".to_string())]
        );
    }

    #[test]
    fn join_renders_a_merge_node_with_its_wait_policy() {
        for (wait, expected) in [
            ("all", "join · wait:all"),
            ("any", "join · wait:any"),
            ("{ count: 2 }", "join · wait:2"),
        ] {
            let rows = rows_for(&fanout_yaml(wait));
            assert!(
                metas(&rows).iter().any(|m| m == expected),
                "wait `{wait}` must render `{expected}`, got {:?}",
                metas(&rows)
            );
            let node = rows
                .iter()
                .find(|r| r.anchor_step_id() == Some("gather"))
                .expect("the join node must anchor so the CLI can select + paint it");
            assert!(
                node.cells
                    .iter()
                    .any(|c| matches!(c, GraphCell::Branch(BranchGlyph::Merge, _))),
                "join node row must carry the merge glyph: {node:#?}"
            );
            assert_eq!(label_of(node), Some("gather"));
        }
    }

    #[test]
    fn join_with_no_wait_key_defaults_to_all() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: a
    agent: x
    prompt: p
    next: [j]
  - id: j
    join: {}
"#,
        );
        assert!(metas(&rows).iter().any(|m| m == "join · wait:all"));
    }

    #[test]
    fn join_merge_glyph_takes_the_join_status() {
        let rows = rows_with(&fanout_yaml("all"), |id| match id {
            "gather" => NodeStatus::Failed,
            _ => NodeStatus::Waiting,
        });
        let node = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("gather"))
            .unwrap();
        assert_eq!(node.anchor_status(), Some(NodeStatus::Failed));
        assert!(node
            .cells
            .iter()
            .any(|c| matches!(c, GraphCell::Branch(BranchGlyph::Merge, NodeStatus::Failed))));
    }

    #[test]
    fn split_and_join_do_not_fall_through_to_the_linear_renderer() {
        // Regression guard: before the split/join arms existed these
        // steps rendered as a bare `● id` row with no meta at all.
        let rows = rows_for(&fanout_yaml("all"));
        let ms = metas(&rows);
        assert!(ms.iter().any(|m| m == "split"), "{ms:?}");
        assert!(ms.iter().any(|m| m.starts_with("join")), "{ms:?}");
    }
}

#[cfg(test)]
mod branch_and_loop_rows_tests {
    use super::*;
    use rupu_orchestrator::Workflow;

    /// a -> g (branch) -> then {t1, t2} / else {e1}.
    const BRANCH_YAML: &str = r#"
name: t
steps:
  - id: a
    agent: x
    prompt: p
    next: [g]
  - id: g
    branch:
      condition: "{{ steps.a.output }}"
      then: [t1, t2]
      else: [e1]
  - id: t1
    agent: x
    prompt: p
  - id: t2
    agent: x
    prompt: p
  - id: e1
    agent: x
    prompt: p
"#;

    /// seed -> (loop refine: gen -> test -> critique) -> ship.
    const LOOP_YAML: &str = r#"
name: t
steps:
  - id: seed
    agent: x
    prompt: p
    next: [gen]
  - id: gen
    agent: x
    prompt: p
  - id: test
    agent: x
    prompt: p
    depends_on: [gen]
  - id: critique
    agent: x
    prompt: p
    depends_on: [test]
  - id: ship
    agent: x
    prompt: p
    depends_on: [critique]
loops:
  refine:
    nodes: [gen, test, critique]
    until: "{{ steps.critique.output }}"
    max_iterations: 5
"#;

    fn rows_with<F: Fn(&str) -> NodeStatus>(yaml: &str, lookup: F) -> Vec<GraphRow> {
        let wf = Workflow::parse(yaml).expect("workflow parses");
        render_rows(&wf, lookup)
    }

    fn rows_for(yaml: &str) -> Vec<GraphRow> {
        rows_with(yaml, |_| NodeStatus::Waiting)
    }

    fn metas(rows: &[GraphRow]) -> Vec<String> {
        rows.iter()
            .flat_map(|r| r.cells.iter())
            .filter_map(|c| match c {
                GraphCell::Meta(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    fn label_of(row: &GraphRow) -> Option<&str> {
        row.cells.iter().find_map(|c| match c {
            GraphCell::Label(l) => Some(l.as_str()),
            _ => None,
        })
    }

    /// One row as the plain text an operator would read.
    fn plain(row: &GraphRow) -> String {
        row.cells
            .iter()
            .map(|c| match c {
                GraphCell::Pipe(_) => "│".to_string(),
                GraphCell::Branch(g, _) => g.as_str().to_string(),
                GraphCell::Bullet(s) => s.glyph().to_string(),
                GraphCell::Space(n) => " ".repeat(usize::from(*n)),
                GraphCell::Label(t) | GraphCell::Meta(t) => t.clone(),
            })
            .collect()
    }

    fn plain_lines(rows: &[GraphRow]) -> Vec<String> {
        rows.iter().map(plain).collect()
    }

    /// Branch arm lanes: `(glyph, target status, arm marker, target id)`.
    /// A lane is an unanchored row with a leading `Branch` cell, an arm
    /// marker `Meta`, and the target id as its `Label`.
    fn arm_lanes(rows: &[GraphRow]) -> Vec<(BranchGlyph, NodeStatus, String, String)> {
        rows.iter()
            .filter(|r| r.anchor.is_none())
            .filter_map(|r| {
                let marker = r.cells.iter().find_map(|c| match c {
                    GraphCell::Meta(m) => Some(m.clone()),
                    _ => None,
                })?;
                match (r.cells.first(), label_of(r)) {
                    (Some(GraphCell::Branch(g, s)), Some(l)) => {
                        Some((*g, *s, marker, l.to_string()))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    // ---- branch ---------------------------------------------------

    #[test]
    fn branch_node_row_carries_the_when_condition_and_anchors() {
        let rows = rows_for(BRANCH_YAML);
        let node = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("g"))
            .expect("the branch node must anchor so the CLI can select + paint it");
        assert_eq!(label_of(node), Some("g"));
        assert!(
            matches!(node.cells.first(), Some(GraphCell::Bullet(_))),
            "branch node is a bulleted node row: {node:#?}"
        );
        assert!(
            node.cells.iter().any(
                |c| matches!(c, GraphCell::Meta(m) if m == "branch · when: {{ steps.a.output }}")
            ),
            "branch node row must carry `branch · when: <condition>`: {node:#?}"
        );
    }

    #[test]
    fn branch_renders_then_and_else_arms_as_distinguishable_lane_groups() {
        let rows = rows_for(BRANCH_YAML);
        assert_eq!(
            arm_lanes(&rows),
            vec![
                (
                    BranchGlyph::Top,
                    NodeStatus::Waiting,
                    "▶ then →".to_string(),
                    "t1".to_string()
                ),
                (
                    BranchGlyph::Bot,
                    NodeStatus::Waiting,
                    "▶ then →".to_string(),
                    "t2".to_string()
                ),
                (
                    BranchGlyph::Bot,
                    NodeStatus::Waiting,
                    "⊘ else →".to_string(),
                    "e1".to_string()
                ),
            ],
            "then arm = Top/Bot lanes, else arm = a lone (closing) lane: {rows:#?}"
        );
    }

    #[test]
    fn branch_full_frame_reads_as_then_and_else_arms_under_the_node() {
        let rows = rows_for(BRANCH_YAML);
        let lines = plain_lines(&rows);
        let at = lines
            .iter()
            .position(|l| l.contains("branch · when:"))
            .expect("branch node row present");
        assert_eq!(
            lines[at..at + 4],
            [
                "○  g  branch · when: {{ steps.a.output }}",
                "╭─ ▶ then → t1",
                "╰─ ▶ then → t2",
                "╰─ ⊘ else → e1",
            ],
            "{lines:#?}"
        );
    }

    #[test]
    fn branch_with_an_empty_arm_emits_no_lanes_for_that_arm() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: a
    agent: x
    prompt: p
    next: [g]
  - id: g
    branch:
      condition: "{{ steps.a.output }}"
      then: [t1]
  - id: t1
    agent: x
    prompt: p
"#,
        );
        let lanes = arm_lanes(&rows);
        assert_eq!(lanes.len(), 1, "{lanes:?}");
        assert_eq!(lanes[0].2, "▶ then →");
        assert!(
            !metas(&rows).iter().any(|m| m.contains("else")),
            "an empty else arm must emit nothing: {:?}",
            metas(&rows)
        );
    }

    #[test]
    fn branch_lanes_are_coloured_by_their_target_status() {
        // The not-taken arm is Skipped in a live run — the lane must
        // carry that so the operator sees taken vs skipped.
        let rows = rows_with(BRANCH_YAML, |id| match id {
            "t1" | "t2" => NodeStatus::Complete,
            "e1" => NodeStatus::Skipped,
            "g" => NodeStatus::Active,
            _ => NodeStatus::Waiting,
        });
        let lanes = arm_lanes(&rows);
        assert_eq!(lanes[0].1, NodeStatus::Complete);
        assert_eq!(lanes[2].1, NodeStatus::Skipped);
        let node = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("g"))
            .unwrap();
        assert_eq!(node.anchor_status(), Some(NodeStatus::Active));
    }

    #[test]
    fn branch_does_not_fall_through_to_the_linear_renderer() {
        // Regression guard: before the branch arm existed this step
        // rendered as a bare `● g` row with no meta at all.
        let ms = metas(&rows_for(BRANCH_YAML));
        assert!(ms.iter().any(|m| m.starts_with("branch · when:")), "{ms:?}");
    }

    // ---- loops ----------------------------------------------------

    #[test]
    fn loop_members_are_framed_by_a_header_and_a_loop_back_footer() {
        let rows = rows_for(LOOP_YAML);
        assert_eq!(
            plain_lines(&rows),
            [
                "○  seed  x",
                "│",
                "├─╭─ ↻ loop:refine  until {{ steps.critique.output }} · max 5",
                "│ ○  gen  x",
                "│ │",
                "│ ○  test  x",
                "│ │",
                "│ ○  critique  x",
                "│ ◄─ ↺ loop:refine  loop-back",
                "│",
                "○  ship  x",
            ],
            "{rows:#?}"
        );
    }

    #[test]
    fn loop_header_carries_until_and_max_meta() {
        let rows = rows_for(LOOP_YAML);
        let header = rows
            .iter()
            .find(|r| label_of(r) == Some("↻ loop:refine"))
            .expect("loop header row");
        assert!(header.cells.iter().any(
            |c| matches!(c, GraphCell::Meta(m) if m == "until {{ steps.critique.output }} · max 5")
        ));
        assert_eq!(
            header.anchor, None,
            "a loop is not a step — its header must not anchor a step id"
        );
    }

    #[test]
    fn loop_members_still_anchor_and_render_through_their_normal_arms() {
        let rows = rows_for(LOOP_YAML);
        for id in ["seed", "gen", "test", "critique", "ship"] {
            assert_eq!(
                rows.iter()
                    .filter(|r| r.anchor_step_id() == Some(id))
                    .count(),
                1,
                "step `{id}` must anchor exactly once"
            );
        }
        // Members are nested one gutter deep: Pipe + Space(1) before the node.
        let gen = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("gen"))
            .unwrap();
        assert!(matches!(gen.cells[0], GraphCell::Pipe(_)));
        assert_eq!(gen.cells[1], GraphCell::Space(1));
        assert!(matches!(gen.cells[2], GraphCell::Bullet(_)));
        // Non-members are NOT nested.
        let seed = rows
            .iter()
            .find(|r| r.anchor_step_id() == Some("seed"))
            .unwrap();
        assert!(matches!(seed.cells[0], GraphCell::Bullet(_)));
    }

    #[test]
    fn a_workflow_without_loops_renders_no_loop_framing() {
        let ms = metas(&rows_for(BRANCH_YAML));
        assert!(
            !ms.iter().any(|m| m.contains("loop") || m.contains("until")),
            "{ms:?}"
        );
    }

    #[test]
    fn a_loop_at_the_very_end_of_the_workflow_still_closes() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: gen
    agent: x
    prompt: p
  - id: test
    agent: x
    prompt: p
    depends_on: [gen]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 2
"#,
        );
        let lines = plain_lines(&rows);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("│ ◄─ ↺ loop:refine  loop-back"),
            "{lines:#?}"
        );
    }

    #[test]
    fn adjacent_loops_each_get_their_own_frame() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: a1
    agent: x
    prompt: p
  - id: a2
    agent: x
    prompt: p
    depends_on: [a1]
  - id: b1
    agent: x
    prompt: p
    depends_on: [a2]
  - id: b2
    agent: x
    prompt: p
    depends_on: [b1]
loops:
  one:
    nodes: [a1, a2]
    until: "{{ steps.a2.output }}"
    max_iterations: 2
  two:
    nodes: [b1, b2]
    until: "{{ steps.b2.output }}"
    max_iterations: 3
"#,
        );
        let lines = plain_lines(&rows);
        let close_one = lines
            .iter()
            .position(|l| l.contains("↺ loop:one"))
            .expect("loop one footer");
        let open_two = lines
            .iter()
            .position(|l| l.contains("↻ loop:two"))
            .expect("loop two header");
        assert!(close_one < open_two, "{lines:#?}");
        assert!(lines[open_two].ends_with("until {{ steps.b2.output }} · max 3"));
    }

    #[test]
    fn a_multiline_until_expression_collapses_to_one_line() {
        let rows = rows_for(
            r#"
name: t
steps:
  - id: gen
    agent: x
    prompt: p
  - id: test
    agent: x
    prompt: p
    depends_on: [gen]
loops:
  refine:
    nodes: [gen, test]
    until: |
      {{ steps.test.output
         and steps.gen.output }}
    max_iterations: 2
"#,
        );
        assert!(
            metas(&rows)
                .iter()
                .any(|m| m == "until {{ steps.test.output and steps.gen.output }} · max 2"),
            "{:?}",
            metas(&rows)
        );
    }

    #[test]
    fn a_loop_member_with_a_nested_construct_keeps_its_own_rows_inside_the_gutter() {
        // A split inside a loop: its fork lanes are nested under the
        // loop gutter like any other member row.
        let rows = rows_for(
            r#"
name: t
steps:
  - id: gen
    agent: x
    prompt: p
    next: [fan]
  - id: fan
    split: [w1, w2]
  - id: w1
    agent: x
    prompt: p
  - id: w2
    agent: x
    prompt: p
loops:
  refine:
    nodes: [gen, fan, w1, w2]
    until: "{{ steps.w1.output }}"
    max_iterations: 2
"#,
        );
        let lines = plain_lines(&rows);
        assert!(
            lines.iter().any(|l| l == "│ ╭─ w1"),
            "split lane must be nested under the loop gutter: {lines:#?}"
        );
    }

    #[test]
    fn non_contiguous_loop_members_reopen_the_frame_marked_continued() {
        // The parser does not require a loop's members to be adjacent in
        // step order. Rather than re-group, a re-opened frame says so.
        let rows = rows_for(
            r#"
name: t
steps:
  - id: gen
    agent: x
    prompt: p
  - id: mid
    agent: x
    prompt: p
  - id: test
    agent: x
    prompt: p
    depends_on: [gen]
loops:
  refine:
    nodes: [gen, test]
    until: "{{ steps.test.output }}"
    max_iterations: 2
"#,
        );
        let ms = metas(&rows);
        let headers: Vec<&String> = ms.iter().filter(|m| m.starts_with("until")).collect();
        assert_eq!(headers.len(), 2, "{ms:?}");
        assert!(!headers[0].contains("continued"), "{headers:?}");
        assert!(headers[1].ends_with("· continued"), "{headers:?}");
    }
}
