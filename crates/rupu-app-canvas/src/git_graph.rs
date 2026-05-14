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
use rupu_orchestrator::Workflow;
use serde::{Deserialize, Serialize};

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

/// Render workflow as graph rows, using `status_lookup` to pick the
/// `NodeStatus` for each step. Pass `|_| NodeStatus::Waiting` for the
/// static (no live run) case.
pub fn render_rows<F>(wf: &Workflow, status_lookup: F) -> Vec<GraphRow>
where
    F: Fn(&str) -> NodeStatus,
{
    let mut rows = Vec::new();
    let total = wf.steps.len();

    for (i, step) in wf.steps.iter().enumerate() {
        let _is_last = i == total - 1;

        // Connector row (a `│` spine) BEFORE every step except the
        // first — keeps the vertical thread continuous between rows.
        if i > 0 {
            rows.push(spine_only());
        }

        if let Some(panel) = &step.panel {
            emit_panel_step(&mut rows, &step.id, &panel.panelists, &status_lookup);
        } else if let Some(subs) = &step.parallel {
            emit_parallel_step(&mut rows, &step.id, subs, &status_lookup);
        } else if step.for_each.is_some() {
            emit_for_each_step(&mut rows, &step.id, &status_lookup);
        } else {
            // Plain linear step. agent may be None if the step uses
            // some other mode (dispatch agent in-prompt etc.); render
            // a blank meta in that case.
            let agent = step.agent.as_deref().unwrap_or("").to_string();
            emit_linear_step(&mut rows, &step.id, agent, &status_lookup);
        }
    }

    rows
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
        assert!(rows
            .iter()
            .any(|row| row
                .cells
                .iter()
                .any(|cell| matches!(cell, GraphCell::Label(label) if label == "spec"))));
        assert!(rows
            .iter()
            .any(|row| row
                .cells
                .iter()
                .any(|cell| matches!(cell, GraphCell::Label(label) if label == "verify"))));
        assert!(rows.iter().any(|row| row
            .cells
            .iter()
            .any(|cell| matches!(cell, GraphCell::Branch(BranchGlyph::Merge, _)))));
    }
}
