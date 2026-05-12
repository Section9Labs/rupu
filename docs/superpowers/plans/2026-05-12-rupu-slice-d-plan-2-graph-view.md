# Slice D — Plan 2: Graph View Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render any workflow YAML in `rupu.app`'s main area as a **vertical git-graph timeline** using the `●/│/├/╭/╰/◄` glyph vocabulary from the line-stream printer and the brainstorm mockups. Linear chains, panel fan-outs (3+ panelists branching off a parent rail and merging back), and the cross-pipe + corner-bend characters all draw correctly. All nodes render in the default `Waiting` status — D-3 brings them alive with executor events.

**Architecture:** New crate `rupu-app-canvas` holds the pure-Rust, GPUI-independent layer that walks a `rupu_orchestrator::Workflow` and emits a structured `Vec<GraphRow>`. Each row is a sequence of typed cells (pipe, branch glyph, bullet, label, meta) carrying its own status color. Snapshot-tested with insta — no GPUI dep, easy to lock the row sequence for representative workflow shapes. `rupu-app` gains `view/graph.rs`: a GPUI view that renders each `GraphRow` as a horizontal flex of monospace text spans, painting each cell with its color.

**Tech Stack:** Rust 2021, GPUI (already pinned), `rupu-orchestrator` (for `Workflow` / `WorkflowStep` / `Panel` types), `insta` for snapshot tests (already in workspace deps). No new external dependencies.

---

## Spec reference

- Spec: `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md` §7.1 (Graph view) + §10 (D-2 line)
- Brainstorm mockups: `.superpowers/brainstorm/35768-1778550913/content/workspace-shell-v1.html` and `views.html` (Graph view section). The canonical visual:

```
●  classify_input            ✓ done
│
│      ╭─●─ security_review   ✓ done · 1 finding
├──────┤
│      ├─●─ perf_review       ◐ running
│      ╰─●─ style_review      ⊘ skipped
│ ◄────╯
│
●  aggregate_findings        ⏸ awaiting approval
```

D-2 scope (spec §10): "Graph view widget — port Slice C TUI's canvas auto-layout to GPUI; render any workflow YAML; no live data yet."

What this plan ports vs. builds new:
- **Lift from rupu-tui:** the `NodeStatus` enum (verbatim) and the status-color mapping (translated from `owo_colors::Rgb` to RGB tuples for the GPUI layer to convert).
- **Build new:** the git-graph row emitter. Slice C's actual TUI canvas uses ratatui `Block` cards, NOT ASCII git-graph. The vertical-spine git-graph the spec describes is the **line-stream printer's** vocabulary (from `rupu-cli::output::printer::LineStreamPrinter`). We port that vocabulary — pipes, branch glyphs, bullets — to a structured row model that's render-target-agnostic.

Out of scope for this plan (deliberate — later sub-slices):
- Live data (D-3: `WorkflowExecutor` + `EventSink` traits → node statuses update in real time)
- Status pulse animations on active nodes (D-3 — needs live data first)
- Approve/reject UI (D-3)
- Click-to-focus + per-pane drill-down (D-3)
- ForEach and Parallel fan-out rendering (D-3 — Panel is the headline case for D-2 and the only fan-out kind the mockup showed; ForEach/Parallel render as a single linear-style row with their kind label until D-3)
- Other views: Canvas (D-6 — that's the Okesu-style boxy-card layout with horizontal flow), Transcript (D-8), YAML (D-5)
- View picker UI (deferred until there's more than one view to pick — D-5+)
- Pane splits, tab strip (D-5/D-6)
- Sidebar workflow click handlers (D-3 — D-2 auto-renders the first project workflow)

---

## File structure

**New crate `crates/rupu-app-canvas/`:**

```
crates/rupu-app-canvas/
  Cargo.toml                              # pure-Rust deps
  src/
    lib.rs                                # module hub + re-exports
    node_status.rs                        # NodeStatus enum + RGB color mapping (lifted from rupu-tui)
    git_graph.rs                          # GraphRow / GraphCell types + render_rows(&Workflow)
  tests/
    snapshots/                            # insta snapshot dir (committed)
    git_graph_snapshots.rs                # locks row output for 3 representative workflow shapes
```

Note: `layout_canvas` / `layout_tree` from Slice C are intentionally NOT ported in this plan. They produce a `BTreeMap<id, (col, row)>` grid which is the right primitive for D-6's Canvas view (Okesu-style boxy cards) but NOT for D-2's git-graph (which is a single-column tree-walk render). D-6 will port them when actually needed.

**Modified `crates/rupu-app/`:**

```
crates/rupu-app/
  Cargo.toml                              # add rupu-app-canvas + rupu-orchestrator deps
  src/
    lib.rs                                # add `pub mod view;`
    view/
      mod.rs                              # module hub
      graph.rs                            # GPUI Graph view (GraphRow → monospace text spans)
    window/mod.rs                         # main-area: render Graph for first project workflow
```

Total tasks below: 10 (1 scaffold + 4 data layer + 1 wiring + 2 GPUI + 2 closeout).

---

## Task 1: Scaffold `rupu-app-canvas` crate

**Files:**
- Modify: `Cargo.toml` (workspace root, add `rupu-app-canvas` to `[workspace] members`)
- Create: `crates/rupu-app-canvas/Cargo.toml`
- Create: `crates/rupu-app-canvas/src/lib.rs` (module hub stub)

- [ ] **Step 1: Confirm `insta` is in workspace deps**

```bash
grep '^insta' Cargo.toml | head -2
```

Expected: a line like `insta = { version = "1", features = ["yaml"] }`. It's already in workspace deps from Slice C. If missing for any reason, add it.

- [ ] **Step 2: Add `rupu-app-canvas` to workspace members**

In `Cargo.toml` (workspace root), the `[workspace] members = [...]` list — add `"crates/rupu-app-canvas"` in alphabetical position (immediately before `"crates/rupu-app"`).

- [ ] **Step 3: Create `crates/rupu-app-canvas/Cargo.toml`**

```toml
[package]
name = "rupu-app-canvas"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
publish = false

[dependencies]
rupu-orchestrator.workspace = true
serde = { workspace = true, features = ["derive"] }

[dev-dependencies]
insta.workspace = true

[lints]
workspace = true
```

- [ ] **Step 4: Create `crates/rupu-app-canvas/src/lib.rs` (module hub)**

```rust
//! Pure-Rust view layer for rupu.app's Graph view (D-2). The git-
//! graph renderer walks a `rupu_orchestrator::Workflow` and emits a
//! `Vec<GraphRow>` — a structured sequence of typed cells (pipe,
//! branch glyph, bullet, label, meta). GPUI views in `rupu-app`
//! consume the rows; this crate stays GPUI-free so the row layer
//! can be locked with insta snapshots at native test speed.
//!
//! D-6 (Canvas view) will add `layout_canvas` / `layout_tree`
//! grid primitives lifted from rupu-tui; D-2 doesn't need them.

pub mod git_graph;
pub mod node_status;

pub use git_graph::{render_rows, BranchGlyph, GraphCell, GraphRow};
pub use node_status::NodeStatus;
```

- [ ] **Step 5: Build to verify scaffold**

Empty placeholder files (`node_status.rs`, `git_graph.rs`) need to exist so the lib.rs `pub mod` declarations don't fail. Create them with a one-line comment:

```rust
// crates/rupu-app-canvas/src/node_status.rs
// Implemented in Task 2.
```

```rust
// crates/rupu-app-canvas/src/git_graph.rs
// Implemented in Task 3.
```

Then:

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app-canvas 2>&1 | tail -5
```

Expected: builds (will warn that lib.rs's `pub use` references missing items — but the placeholder files exist so `pub mod` succeeds; cargo's warning is non-fatal until Task 2). If it errors hard (unresolved imports in lib.rs), comment out the `pub use` lines temporarily and uncomment them after Task 3 finishes. Either approach is fine.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/rupu-app-canvas
git commit -m "feat(rupu-app-canvas): scaffold crate + module hub"
```

---

## Task 2: `NodeStatus` enum + RGB color mapping

**Files:**
- Modify: `crates/rupu-app-canvas/src/node_status.rs`

The `NodeStatus` enum lifts from `rupu-tui/src/state/node.rs`. The RGB mapping mirrors the Okesu palette already in `rupu-cli/src/output/palette.rs` and `rupu-app/src/palette.rs` so all three surfaces (CLI, app, canvas) draw the same green for "complete", red for "failed", etc.

- [ ] **Step 1: Replace placeholder with full module**

```rust
//! Status of one DAG node. Lifted from rupu-tui::state::NodeStatus
//! with one addition: this version returns RGB tuples for status
//! colors so the consuming GPUI layer (rupu-app) can convert to
//! `gpui::Rgba` without pulling GPUI into this crate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    #[default]
    Waiting,
    Active,
    Working,
    Complete,
    Failed,
    SoftFailed,
    Awaiting,
    Retrying,
    Skipped,
}

impl NodeStatus {
    /// Single-glyph identifier matching the line-stream printer's
    /// vocabulary: `○ ● ◐ ✓ ✗ ⏸ ⊘ ↻`.
    pub fn glyph(self) -> char {
        match self {
            Self::Waiting => '○',
            Self::Active => '●',
            Self::Working => '◐',
            Self::Complete => '✓',
            Self::Failed => '✗',
            Self::SoftFailed => '✗',
            Self::Awaiting => '⏸',
            Self::Retrying => '↻',
            Self::Skipped => '⊘',
        }
    }

    /// Foreground color as a 24-bit RGB tuple. Mirrors the Okesu
    /// palette in rupu-cli/output/palette.rs + rupu-app/src/palette.rs.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Waiting => (82, 82, 91),       // slate-500 (dim)
            Self::Active => (59, 130, 246),      // blue-500
            Self::Working => (59, 130, 246),     // blue-500
            Self::Complete => (34, 197, 94),     // green-500
            Self::Failed => (239, 68, 68),       // red-500
            Self::SoftFailed => (202, 138, 4),   // yellow-600
            Self::Awaiting => (251, 191, 36),    // amber-400
            Self::Retrying => (124, 58, 237),    // brand-500
            Self::Skipped => (203, 213, 225),    // slate-300
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_glyph_is_checkmark() {
        assert_eq!(NodeStatus::Complete.glyph(), '✓');
    }

    #[test]
    fn failed_color_is_red_500() {
        assert_eq!(NodeStatus::Failed.rgb(), (239, 68, 68));
    }

    #[test]
    fn default_is_waiting() {
        assert_eq!(NodeStatus::default(), NodeStatus::Waiting);
    }

    #[test]
    fn waiting_glyph_is_hollow_circle() {
        // The empty/pending state is `○` (hollow), so an
        // unstarted workflow doesn't look like every node is
        // running. Active/Working share `●`/`◐` once data flows.
        assert_eq!(NodeStatus::Waiting.glyph(), '○');
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-app-canvas node_status
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app-canvas/src/node_status.rs
git commit -m "feat(rupu-app-canvas): NodeStatus enum + RGB color mapping"
```

---

## Task 3: `git_graph` module — row emitter

**Files:**
- Modify: `crates/rupu-app-canvas/src/git_graph.rs`

This is the heart of the plan. Walks a `Workflow` in declaration order, emits a `Vec<GraphRow>` where each row is a sequence of typed cells in left-to-right order:

- `Pipe(NodeStatus)` — a `│` vertical bar (status drives color)
- `Branch(BranchGlyph, NodeStatus)` — `├─` / `╭─` / `╰─` / `◄─` / `─┤` (branch into / out of fan-out)
- `Bullet(NodeStatus)` — the `●` (or status-glyph) marker for a step
- `Label(String)` — the step id / agent name
- `Meta(String)` — dim meta text (agent name, "panel · N panelists")
- `Space(u16)` — a run of spaces (for column alignment)

Each row is rendered by the GPUI view in Task 6 as a horizontal flex of monospace text spans.

D-2 emits rows for two step kinds: **Linear** (agent + prompt) and **Panel** (N panelists). ForEach and Parallel render as a single linear-shaped row with their kind in the meta (e.g. `for_each · N items`) — D-3+ adds proper fan-out for them.

- [ ] **Step 1: Implement `git_graph.rs` with TDD**

```rust
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
/// variant maps to a 2-character string in the renderer:
/// - `Top` → `╭─` (top-left corner, opens a fan-out)
/// - `Mid` → `├─` (mid-T, branches off a continuing spine)
/// - `Bot` → `╰─` (bottom-left corner, closes a fan-out)
/// - `Merge` → `◄─` (merge-back arrow back to the parent spine)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BranchGlyph {
    Top,
    Mid,
    Bot,
    Merge,
}

impl BranchGlyph {
    /// Render as a 2-character monospace string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "╭─",
            Self::Mid => "├─",
            Self::Bot => "╰─",
            Self::Merge => "◄─",
        }
    }
}

/// Top-level entry point: render the workflow as rows. All node
/// statuses default to `Waiting` — D-3 will inject live statuses.
pub fn render_rows(wf: &Workflow) -> Vec<GraphRow> {
    let mut rows = Vec::new();
    let total = wf.steps.len();

    for (i, step) in wf.steps.iter().enumerate() {
        let is_last = i == total - 1;

        // Connector row (a `│` spine) BEFORE every step except the
        // first — keeps the vertical thread continuous between rows.
        if i > 0 {
            rows.push(spine_only());
        }

        if let Some(panel) = &step.panel {
            emit_panel_step(&mut rows, &step.id, &panel.panelists);
        } else if step.parallel.is_some() {
            // Parallel: rendered as a single linear-shaped row with
            // the kind in meta. Proper sub-step branching lands in D-3.
            let n = step.parallel.as_ref().map(|p| p.len()).unwrap_or(0);
            emit_linear_step(&mut rows, &step.id, format!("parallel · {n} sub-steps"));
        } else if step.for_each.is_some() {
            // ForEach: same — proper per-item branching lands in D-3.
            emit_linear_step(&mut rows, &step.id, "for_each · runtime fan-out".into());
        } else {
            // Plain linear step. agent may be None if the step uses
            // some other mode (dispatch agent in-prompt etc.); render
            // a blank meta in that case.
            let agent = step.agent.as_deref().unwrap_or("").to_string();
            emit_linear_step(&mut rows, &step.id, agent);
        }

        let _ = is_last; // currently unused — kept for D-3 close-row logic
    }

    rows
}

/// Emit a single linear-step row: `● <step_id>   <meta>`.
fn emit_linear_step(rows: &mut Vec<GraphRow>, step_id: &str, meta: String) {
    let mut cells = Vec::new();
    cells.push(GraphCell::Bullet(NodeStatus::Waiting));
    cells.push(GraphCell::Space(2));
    cells.push(GraphCell::Label(step_id.to_string()));
    if !meta.is_empty() {
        cells.push(GraphCell::Space(2));
        cells.push(GraphCell::Meta(meta));
    }
    rows.push(GraphRow { cells });
}

/// Emit a panel block: header row + one row per panelist + close row.
///
/// ```text
/// ├─╭─ <step_id>   panel · <N> panelists
/// │ │
/// │ ●─ <panelist[0]>
/// │ ●─ <panelist[1]>
/// │ ●─ <panelist[2]>
/// │ │
/// │ ◄─╯
/// ```
///
/// The leading `│` on each panelist row is the WORKFLOW spine
/// (continues through this panel). The `╭─/●─/●─/◄─╯` characters
/// indent in to mark panel membership.
fn emit_panel_step(rows: &mut Vec<GraphRow>, step_id: &str, panelists: &[String]) {
    let s = NodeStatus::Waiting;
    let n = panelists.len();

    // Header: ├─╭─ <step_id>   panel · N panelists
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Branch(BranchGlyph::Mid, s),
            GraphCell::Branch(BranchGlyph::Top, s),
            GraphCell::Space(1),
            GraphCell::Label(step_id.to_string()),
            GraphCell::Space(2),
            GraphCell::Meta(format!("panel · {n} panelist{}", if n == 1 { "" } else { "s" })),
        ],
    });

    // Spacer: │ │
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(s),
            GraphCell::Space(1),
            GraphCell::Pipe(s),
        ],
    });

    // One row per panelist: │ ●─ <agent>
    for agent in panelists {
        rows.push(GraphRow {
            cells: vec![
                GraphCell::Pipe(s),
                GraphCell::Space(1),
                GraphCell::Bullet(s),
                GraphCell::Branch(BranchGlyph::Mid, s),
                GraphCell::Space(1),
                GraphCell::Label(agent.clone()),
            ],
        });
    }

    // Spacer: │ │
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(s),
            GraphCell::Space(1),
            GraphCell::Pipe(s),
        ],
    });

    // Close row: │ ◄─╯
    // The merge arrow + bottom corner reconnect the panel sub-spine
    // back to the workflow spine.
    rows.push(GraphRow {
        cells: vec![
            GraphCell::Pipe(s),
            GraphCell::Space(1),
            GraphCell::Branch(BranchGlyph::Merge, s),
            GraphCell::Branch(BranchGlyph::Bot, s),
        ],
    });
}

/// A `│` connector row used between steps.
fn spine_only() -> GraphRow {
    GraphRow {
        cells: vec![GraphCell::Pipe(NodeStatus::Waiting)],
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
            "name: t\nsteps:\n\
             \x20  - id: a\n    agent: aa\n    actions: []\n    prompt: hi\n\
             \x20  - id: b\n    agent: bb\n    actions: []\n    prompt: hi\n\
             \x20  - id: c\n    agent: cc\n    actions: []\n    prompt: hi\n",
        );
        let rows = render_rows(&wf);
        assert_eq!(rows.len(), 5, "expected 3 step + 2 connector = 5 rows, got: {rows:#?}");

        // Row 0: step a (bullet + label + meta)
        assert!(matches!(rows[0].cells[0], GraphCell::Bullet(NodeStatus::Waiting)));
        assert!(rows[0].cells.iter().any(|c| matches!(c, GraphCell::Label(s) if s == "a")));

        // Row 1: spine connector
        assert_eq!(rows[1].cells.len(), 1);
        assert!(matches!(rows[1].cells[0], GraphCell::Pipe(NodeStatus::Waiting)));
    }

    #[test]
    fn panel_step_emits_header_plus_panelists_plus_close() {
        let wf = parse(
            "name: r\nsteps:\n\
             \x20  - id: classify\n    agent: classifier\n    actions: []\n    prompt: hi\n\
             \x20  - id: review_panel\n    panel:\n      panelists:\n        - security-reviewer\n        - perf-reviewer\n        - style-reviewer\n",
        );
        let rows = render_rows(&wf);

        // 1 row (classify) + 1 connector + 6 rows (header + spacer + 3 panelists + spacer + close)
        // = 8 rows total
        assert_eq!(rows.len(), 9, "expected 8 rows; got {rows:#?}");

        // The panel header should be at index 2 (after classify + connector).
        // Header = Branch(Mid) + Branch(Top) + Space + Label(review_panel) + Space + Meta(panel · 3 panelists)
        let header = &rows[2];
        assert!(matches!(header.cells[0], GraphCell::Branch(BranchGlyph::Mid, _)));
        assert!(matches!(header.cells[1], GraphCell::Branch(BranchGlyph::Top, _)));
        assert!(header.cells.iter().any(|c| matches!(c, GraphCell::Label(s) if s == "review_panel")));
        assert!(header.cells.iter().any(|c| matches!(c, GraphCell::Meta(s) if s.contains("3 panelist"))));

        // The close row at the end of the panel must contain Merge + Bot
        let close = rows.iter().rev().find(|r| {
            r.cells.iter().any(|c| matches!(c, GraphCell::Branch(BranchGlyph::Merge, _)))
        }).expect("merge close row");
        assert!(close.cells.iter().any(|c| matches!(c, GraphCell::Branch(BranchGlyph::Bot, _))));
    }

    #[test]
    fn empty_workflow_emits_no_rows() {
        let wf = parse("name: e\nsteps: []\n");
        assert!(render_rows(&wf).is_empty());
    }
}
```

The test YAML uses `\x20  - id:` (the YAML continuation uses spaces, which `\x20` makes explicit so the source doesn't get hosed by an editor that strips leading whitespace). If the parser complains about the formatting, normal `  ` (4-space leading indent) in a raw string works too.

- [ ] **Step 2: Run tests**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-app-canvas git_graph
```

Expected: 3 tests pass.

If the panel YAML parse fails because the orchestrator's `Workflow::parse` requires extra fields on panel steps (e.g. `prompt:` or `actions:` at the step level), adjust the test YAML — match `rupu-orchestrator`'s own parse tests for panel steps.

If the test counts are off because the orchestrator adds implicit metadata or your panel rendering decision differs slightly (e.g. spacer rows vs. no spacers), recount and update the assertions to match the implementation — the *shape* is what's load-bearing, not specific row counts.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app-canvas/src/git_graph.rs
git commit -m "feat(rupu-app-canvas): git_graph row emitter (linear + panel)"
```

---

## Task 4: Snapshot tests for representative workflow shapes

**Files:**
- Create: `crates/rupu-app-canvas/tests/git_graph_snapshots.rs`

Insta snapshots lock the exact row output for representative workflows. If anyone tweaks the row-emission logic, snapshot diffs immediately surface visual drift.

- [ ] **Step 1: Create the integration test file**

```rust
//! Snapshot tests for rupu-app-canvas's git-graph row emitter.
//!
//! Each test parses a representative workflow YAML, runs
//! `render_rows`, and snapshots the result via insta. The .snap
//! files are committed alongside the test so visual changes show
//! up as PR diffs.

use rupu_app_canvas::render_rows;
use rupu_orchestrator::Workflow;

fn fixture(name: &str) -> Workflow {
    Workflow::parse(name).expect("fixture workflow parses")
}

#[test]
fn snapshot_linear_3_steps() {
    let wf = fixture(
        "name: linear3\nsteps:\n\
         \x20  - id: classify\n    agent: classifier\n    actions: []\n    prompt: hi\n\
         \x20  - id: review\n    agent: reviewer\n    actions: []\n    prompt: hi\n\
         \x20  - id: publish\n    agent: publisher\n    actions: []\n    prompt: hi\n",
    );
    let rows = render_rows(&wf);
    insta::assert_yaml_snapshot!("linear_3_steps", rows);
}

#[test]
fn snapshot_panel_with_3_panelists() {
    let wf = fixture(
        "name: review\nsteps:\n\
         \x20  - id: classify\n    agent: classifier\n    actions: []\n    prompt: hi\n\
         \x20  - id: review_panel\n    panel:\n      panelists:\n        - security-reviewer\n        - perf-reviewer\n        - style-reviewer\n\
         \x20  - id: aggregate\n    agent: findings-aggregator\n    actions: []\n    prompt: hi\n",
    );
    let rows = render_rows(&wf);
    insta::assert_yaml_snapshot!("panel_with_3_panelists", rows);
}

#[test]
fn snapshot_single_linear_step() {
    let wf = fixture(
        "name: single\nsteps:\n\
         \x20  - id: hello\n    agent: greeter\n    actions: []\n    prompt: hi\n",
    );
    let rows = render_rows(&wf);
    insta::assert_yaml_snapshot!("single_linear_step", rows);
}
```

- [ ] **Step 2: Seed the snapshots**

```bash
cd /Users/matt/Code/Oracle/rupu
INSTA_UPDATE=auto cargo test -p rupu-app-canvas --test git_graph_snapshots
```

Expected: 3 tests pass; insta creates `crates/rupu-app-canvas/tests/snapshots/git_graph_snapshots__linear_3_steps.snap` etc.

- [ ] **Step 3: Eyeball the snapshots**

```bash
cat crates/rupu-app-canvas/tests/snapshots/git_graph_snapshots__panel_with_3_panelists.snap
```

Confirm the row sequence reads like the canonical visual at the top of this plan:
- classify → spine → panel header (Mid+Top branch) → spacer → 3 panelist rows → spacer → close (Merge+Bot) → spine → aggregate

If the snapshot looks wrong, fix the implementation in Task 3 and re-seed.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app-canvas/tests
git commit -m "test(rupu-app-canvas): insta snapshots for linear + panel + single workflows"
```

---

## Task 5: Wire `rupu-app-canvas` into `rupu-app`

**Files:**
- Modify: `crates/rupu-app/Cargo.toml` (add `rupu-app-canvas` + `rupu-orchestrator` deps)
- Modify: `crates/rupu-app/src/lib.rs` (add `pub mod view;`)
- Create: `crates/rupu-app/src/view/mod.rs`
- Create: `crates/rupu-app/src/view/graph.rs` (stub — populated in Task 6)

- [ ] **Step 1: Add deps to `crates/rupu-app/Cargo.toml`**

In `[dependencies]`, add (alphabetical position):

```toml
rupu-app-canvas = { path = "../rupu-app-canvas" }
rupu-orchestrator.workspace = true
```

`rupu-app-canvas` uses a path reference (not workspace = true) because it's a private internal crate, not in `[workspace.dependencies]`.

- [ ] **Step 2: Update `crates/rupu-app/src/lib.rs`**

```rust
//! rupu.app library — exposed so integration tests can reach the
//! pure-data modules (workspace, palette, view).

pub mod menu;
pub mod palette;
pub mod view;
pub mod window;
pub mod workspace;
```

- [ ] **Step 3: Create `crates/rupu-app/src/view/mod.rs`**

```rust
//! GPUI views for the rupu.app main area.
//!
//! Each view is a thin GPUI wrapper around a `rupu-app-canvas`
//! data structure. D-2 ships `graph` (vertical git-graph). D-5 / D-6
//! / D-8 add YAML / Canvas / Transcript.

pub mod graph;
```

- [ ] **Step 4: Stub `crates/rupu-app/src/view/graph.rs`**

```rust
//! Graph view — populated in Task 6.

// Stub. Task 6 replaces this with the GPUI git-graph renderer.
```

- [ ] **Step 5: Build**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app
```

Expected: builds cleanly.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/Cargo.toml crates/rupu-app/src/lib.rs crates/rupu-app/src/view
git commit -m "feat(rupu-app): wire rupu-app-canvas as a dep + view module hub"
```

---

## Task 6: GPUI git-graph renderer

**Files:**
- Modify: `crates/rupu-app/src/view/graph.rs`

Renders a `Vec<GraphRow>` as a column of monospace rows. Each row is a flex of styled text spans — one span per `GraphCell`. Cells have their own foreground color (status-driven for Pipe/Branch/Bullet, TEXT_PRIMARY for Label, TEXT_DIMMEST for Meta).

- [ ] **Step 1: Replace the stub with the GPUI renderer**

```rust
//! Graph view — GPUI renderer for `Vec<GraphRow>` from
//! `rupu-app-canvas::render_rows`. Each row becomes a horizontal
//! flex of styled monospace text spans. Layout is pure tree-walk
//! (no col×row grid for D-2 — that lives in D-6's Canvas view).

use crate::palette;
use gpui::{div, prelude::*, px, AnyElement, IntoElement, Rgba};
use rupu_app_canvas::{BranchGlyph, GraphCell, GraphRow, NodeStatus};
use rupu_orchestrator::Workflow;

/// Top-level entry point: render a parsed `Workflow` as the git-
/// graph view.
pub fn render(workflow: &Workflow) -> impl IntoElement {
    let rows = rupu_app_canvas::render_rows(workflow);

    let mut container = div()
        .size_full()
        .bg(palette::BG_PRIMARY)
        .px(px(24.0))
        .py(px(20.0))
        .flex()
        .flex_col()
        .gap(px(2.0));

    for row in &rows {
        container = container.child(render_row(row));
    }

    container
}

fn render_row(row: &GraphRow) -> AnyElement {
    let mut hbox = div()
        .flex()
        .flex_row()
        .items_center()
        .text_sm()
        // Monospace font so the glyphs line up vertically across rows.
        // GPUI honors the font_family setter — the exact name varies
        // by host. "Menlo" is a macOS system mono that ships ubiquitously.
        .font_family("Menlo");

    for cell in &row.cells {
        hbox = hbox.child(render_cell(cell));
    }

    hbox.into_any_element()
}

fn render_cell(cell: &GraphCell) -> AnyElement {
    match cell {
        GraphCell::Pipe(status) => {
            div().text_color(status_rgba(*status)).child("│").into_any_element()
        }
        GraphCell::Branch(glyph, status) => {
            div()
                .text_color(status_rgba(*status))
                .child(glyph.as_str().to_string())
                .into_any_element()
        }
        GraphCell::Bullet(status) => {
            // Use the status glyph (●, ◐, ✓, ✗, ⏸, ⊘, ↻, ○) rather than a
            // fixed `●` — that way Waiting renders as ○ (hollow) so an
            // unstarted workflow doesn't look like it's mid-run.
            div()
                .text_color(status_rgba(*status))
                .child(status.glyph().to_string())
                .into_any_element()
        }
        GraphCell::Space(n) => div().child(" ".repeat(*n as usize)).into_any_element(),
        GraphCell::Label(s) => div()
            .text_color(palette::TEXT_PRIMARY)
            .child(s.clone())
            .into_any_element(),
        GraphCell::Meta(s) => div()
            .text_color(palette::TEXT_DIMMEST)
            .child(s.clone())
            .into_any_element(),
    }
}

fn status_rgba(status: NodeStatus) -> Rgba {
    let (r, g, b) = status.rgb();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}
```

### GPUI API adaptations to expect

GPUI is pre-1.0; the methods called above (`.font_family()`, `.gap()`, `.items_center()`) work at our pinned commit but the names may shift. Same caveat as prior tasks. If something doesn't compile, look at the actual gpui::styled trait for the correct method name. Common alternatives:
- `.font_family(name)` — may be `.font(name)` or `.text_family(name)`
- `.gap(px(N))` — confirmed to exist (used in Task 11 of Plan 1)
- `.items_center()` — confirmed

If `Rgba` isn't constructible by field literal at this commit (it was in Task 3 of Plan 1, so should still work), use whatever constructor pattern Task 3 of Plan 1 settled on.

- [ ] **Step 2: Build**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app 2>&1 | tail -10
```

Expected: builds. Iterate on any GPUI API drift.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/view/graph.rs
git commit -m "feat(rupu-app): Graph view — render GraphRow vec as monospace text spans"
```

---

## Task 7: Wire Graph view into the main area

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs`

Replace the centered "Open a workflow from the sidebar." placeholder with: if there's at least one project workflow, parse it and render the Graph view; otherwise keep the placeholder.

- [ ] **Step 1: Update `window/mod.rs`**

Find the current `Render for WorkspaceWindow` impl. Replace the main-area child block:

```rust
impl Render for WorkspaceWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let main_area = match self.workspace.project_assets.workflows.first() {
            Some(asset) => render_main_for_workflow(asset),
            None => render_main_placeholder(),
        };

        div()
            .size_full()
            .bg(palette::BG_PRIMARY)
            .text_color(palette::TEXT_PRIMARY)
            .flex()
            .flex_col()
            .child(titlebar::render(&self.workspace))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(sidebar::render(&self.workspace))
                    .child(main_area),
            )
    }
}

fn render_main_placeholder() -> gpui::AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::TEXT_DIMMEST)
        .child("Open a workflow from the sidebar.")
        .into_any_element()
}

fn render_main_for_workflow(asset: &crate::workspace::Asset) -> gpui::AnyElement {
    use rupu_orchestrator::Workflow;

    let body = match std::fs::read_to_string(&asset.path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(path = ?asset.path, %e, "read workflow");
            return render_main_error(format!("failed to read {}: {e}", asset.path.display()));
        }
    };
    let wf = match Workflow::parse(&body) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(path = ?asset.path, %e, "parse workflow");
            return render_main_error(format!("failed to parse {}: {e}", asset.path.display()));
        }
    };

    div()
        .flex_1()
        .child(crate::view::graph::render(&wf))
        .into_any_element()
}

fn render_main_error(msg: String) -> gpui::AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(palette::FAILED)
        .child(msg)
        .into_any_element()
}
```

Add necessary imports at the top if missing — likely `IntoElement` and `Render` from gpui::prelude.

- [ ] **Step 2: Build + smoke run**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app
timeout 5 cargo run -p rupu-app -- /Users/matt/Code/Oracle/rupu || true
```

Expected: window opens; main area shows the git-graph of the first project workflow from `<repo>/.rupu/workflows/*.yaml` (whichever sorts first alphabetically). Vertical spine of ○/│ pipes, with the step ids + agent meta along the right. If the rupu repo has the `review-and-file-issues` panel workflow checked in, eyeball that to confirm the panel header (├─╭─) + 3 panelists + close (◄─╯) all render.

If the workflow is linear, you should see something like:

```
○  classify_input        classifier
│
○  panel_review          review-panel
│
○  aggregate_findings    findings-aggregator
```

(All `○` because nothing's running.)

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): render Graph view of first project workflow in main area"
```

---

## Task 8: CLAUDE.md update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add `rupu-app-canvas` to Crates section**

Find the Crates list. Insert after `rupu-app`:

```markdown
- **`rupu-app-canvas`** — pure-Rust view layer for rupu.app (Slice D). Walks a `rupu_orchestrator::Workflow` and emits a `Vec<GraphRow>` of structured cells (pipe / branch glyph / bullet / label / meta) for the git-graph view. Snapshot-tested with insta; no GPUI dep. rupu-app's `view/graph.rs` consumes the rows and paints with GPUI text spans. D-6 will add `layout_canvas`/`layout_tree` here for the Canvas view's col×row grid.
```

- [ ] **Step 2: Append Plan 2 pointer in Read first**

```markdown
- Slice D Plan 2 (Graph view, complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-2-graph-view.md`
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md — rupu-app-canvas crate + Slice D Plan 2 pointer"
```

---

## Task 9: Smoke test extension

**Files:**
- Modify: `Makefile` (extend `app-smoke` to assert workspace-open log line)

- [ ] **Step 1: Tighten the smoke target**

In `Makefile`, find `app-smoke` and update its body:

```makefile
app-smoke:
	@cargo build --release -p rupu-app
	@FIXTURE="$$(pwd)/crates/rupu-app/tests/fixtures/sample-workspace"; \
	OUTPUT=$$(timeout 4 ./target/release/rupu-app "$$FIXTURE" 2>&1 || true); \
	if echo "$$OUTPUT" | grep -qE 'panic|panicked'; then \
		echo "app-smoke FAIL — panic in output:"; \
		echo "$$OUTPUT"; \
		exit 1; \
	fi; \
	if ! echo "$$OUTPUT" | grep -q 'opened workspace'; then \
		echo "app-smoke FAIL — expected 'opened workspace' log line missing:"; \
		echo "$$OUTPUT"; \
		exit 1; \
	fi
	@echo "app-smoke OK"
```

The fixture workspace from Plan 1's Task 16 has one workflow at `tests/fixtures/sample-workspace/.rupu/workflows/example.yaml`. The Graph view will render that during the 4-second window. Any GPUI panic in the new render path manifests here.

- [ ] **Step 2: Run**

```bash
make app-smoke
```

Expected: `app-smoke OK`.

- [ ] **Step 3: Commit**

```bash
git add Makefile
git commit -m "test(rupu-app): tighten app-smoke to assert workspace-open log line"
```

---

## Task 10: Workspace gates

**Files:**
- (none — runs existing tooling; may need minor fixups)

- [ ] **Step 1: fmt**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo fmt --all -- --check
```

If diffs, `cargo fmt --all` + commit.

- [ ] **Step 2: clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

If warnings, fix narrowly + commit.

- [ ] **Step 3: tests**

```bash
cargo test --workspace 2>&1 | tail -30
```

Expected: all pass. rupu-app-canvas reports 4 unit (NodeStatus) + 3 unit (git_graph) + 3 snapshot = 10 tests. rupu-app's prior 14 tests still pass.

- [ ] **Step 4: app-smoke**

```bash
make app-smoke
```

Expected: `app-smoke OK`.

- [ ] **Step 5: Final state**

Any fixups committed in Steps 1-3. Plan 2 complete.

---

## Self-review notes

**Spec coverage** (against spec §7.1 + §10 D-2 line):

| D-2 deliverable | Covered by |
|---|---|
| Vertical git-graph spine with `●/│/├/╭/╰/◄` glyphs | Task 3 (`git_graph` emitter) + Task 6 (GPUI renderer paints each glyph) |
| Status colors from the existing palette | Task 2 (`NodeStatus::rgb()` matches `rupu-cli/output/palette.rs`) + Task 6 (renderer maps to gpui::Rgba) |
| Linear chains render | Task 3 (`emit_linear_step`) + insta snapshot in Task 4 (`linear_3_steps`) |
| Panel fan-outs render (header + N panelists + close) | Task 3 (`emit_panel_step`) + insta snapshot in Task 4 (`panel_with_3_panelists`) |
| Monospace code-editor typeface | Task 6 (`.font_family("Menlo")`) |
| All nodes Waiting (no live data) | Task 3 (every emitted bullet has `NodeStatus::Waiting`) |
| Render any workflow YAML | Task 7 (parses + renders the first project workflow) |

**Spec sections deferred to later sub-slices:**
- ForEach + Parallel fan-out rendering (D-3 — D-2 renders these as single linear-shaped rows with kind label in meta)
- Live status updates (D-3 — `EventSink` subscription)
- Status pulse animations (D-3 — needs live data first)
- Click-to-focus / drill-down (D-3)
- Pane splits + tab strip (D-5 / D-6)
- View picker (D-5+)

**Placeholder scan:** zero `TODO` markers. The "D-3 lands ForEach/Parallel fan-out" notes are deferral documentation, not unfinished work.

**Type consistency:**
- `NodeStatus` (Task 2) → field of every `GraphCell::Pipe/Branch/Bullet` (Task 3) → consumed by `status_rgba` in renderer (Task 6).
- `BranchGlyph` (Task 3) → consumed by `render_cell` via `as_str()` (Task 6).
- `GraphRow` / `GraphCell` (Task 3) → produced by `render_rows` (Task 3) → consumed by `view::graph::render` (Tasks 6, 7).
- `Workflow` (rupu-orchestrator) → consumed by `render_rows` (Task 3) and `view::graph::render` (Task 6).

**No-placeholder verification:** every code step has full executable Rust; every command has expected output; every snapshot test is concrete.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-2-graph-view.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.

**2. Inline Execution** — `superpowers:executing-plans` with batch checkpoints.

Which approach?
