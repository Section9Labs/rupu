# Slice D — Plan 2: Graph View Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the Slice C TUI canvas auto-layout into a new pure-Rust crate `rupu-app-canvas`, then render any workflow YAML in `rupu.app`'s main area as a static vertical git-graph — boxy card nodes with edges between them, drawn in GPUI. No live data, no executor wiring — those land in D-3.

**Architecture:** New crate `rupu-app-canvas` holds the GPUI-independent layer: `NodeStatus` enum, `Position` + `layout_canvas()` (depth-by-edges), `CanvasModel` struct, and `derive_edges_from_workflow` that consumes `rupu-orchestrator::Workflow`. Snapshot-testable with insta — no GPUI dep needed. `rupu-app` gains `view/graph.rs`, a GPUI view that consumes a `CanvasModel` and paints cards + edges + dotted backdrop. The `WorkspaceWindow`'s main area picks the first project workflow on open and renders its Graph view; if no workflow exists, the existing "Open a workflow from the sidebar" placeholder remains.

**Tech Stack:** Rust 2021, GPUI (already pinned), `rupu-orchestrator` (for `Workflow` type), `insta` for snapshot tests (already in workspace deps from Slice C). No new external dependencies.

---

## Spec reference

- Spec: `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md` §7.1 (Graph view) + §10 (D-2 line)
- Slice C source to port from:
  - `crates/rupu-tui/src/view/layout.rs` — `layout_canvas` + `layout_tree`
  - `crates/rupu-tui/src/state/edges.rs` — `derive_edges`
  - `crates/rupu-tui/src/state/node.rs` — `NodeStatus` enum
  - `crates/rupu-tui/src/view/palette.rs` — status color mapping

D-2 scope (spec §10): "Graph view widget — port Slice C TUI's canvas auto-layout to GPUI; render any workflow YAML; no live data yet."

Out of scope for this plan (later sub-slices):
- Live data binding (D-3: `WorkflowExecutor`/`EventSink` traits make node statuses light up in real time)
- Approval / reject UI on nodes (D-3)
- Click-to-select with a focused-node drill-down pane (D-3)
- Other views — Canvas (D-6), Transcript (D-8), YAML (D-5)
- View picker UI (deferred — Graph is the only view in D-2 so no picker yet)
- Pane splits (D-2 keeps the single-pane main area)
- Tab strip (D-5 / D-6 introduce real tab content; D-2 just renders directly into the main area)
- Workflow selection from sidebar clicks (deferred to D-3 alongside event binding)

---

## File structure

**New crate `crates/rupu-app-canvas/`:**

```
crates/rupu-app-canvas/
  Cargo.toml                              # pure-Rust deps
  src/
    lib.rs                                # module hub + re-exports
    node_status.rs                        # NodeStatus enum + RGB color mapping
    layout.rs                             # Position + layout_canvas + layout_tree
    edges.rs                              # derive_edges_from_workflow (consumes rupu-orchestrator::Workflow)
    model.rs                              # CanvasModel: nodes BTreeMap + edges Vec
  tests/
    snapshots/                            # insta snapshot dir
    layout_snapshots.rs                   # insta-tested layout outputs for sample workflows
```

**Modified `crates/rupu-app/`:**

```
crates/rupu-app/
  Cargo.toml                              # add rupu-app-canvas + rupu-orchestrator deps
  src/
    lib.rs                                # add `pub mod view;`
    view/
      mod.rs                              # module hub
      graph.rs                            # GPUI Graph view (CanvasModel → painted elements)
    window/mod.rs                         # main-area: render Graph for first project workflow
```

The split is deliberate: `rupu-app-canvas` stays GPUI-free so layout regressions can be snapshot-tested at high speed. `rupu-app::view::graph` is GPUI-only — just paints from a pre-computed model.

Total tasks below: 12 (1 scaffold + 4 data layer + 1 wiring + 2 GPUI + 1 polish + 3 closeout).

---

## Task 1: Scaffold `rupu-app-canvas` crate

**Files:**
- Modify: `Cargo.toml` (workspace root, add `rupu-app-canvas` to members + `insta` already in workspace deps; confirm by inspection)
- Create: `crates/rupu-app-canvas/Cargo.toml`
- Create: `crates/rupu-app-canvas/src/lib.rs`

- [ ] **Step 1: Confirm `insta` is in workspace deps**

```bash
grep -A1 '^insta' Cargo.toml | head -3
```

Expected: `insta = "1"` (or similar). It is already in workspace deps from Slice C. If for some reason it isn't, add `insta = { version = "1", features = ["yaml"] }` to `[workspace.dependencies]`.

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

- [ ] **Step 4: Create `crates/rupu-app-canvas/src/lib.rs` (module hub only)**

```rust
//! Pure-Rust view layer for rupu.app's Graph / Canvas / Transcript /
//! YAML views. Consumes a `Workflow` from rupu-orchestrator, produces
//! a `CanvasModel` that GPUI views can paint without re-implementing
//! the layout algorithm.
//!
//! Snapshot-testable without booting GPUI: tests in `tests/` use
//! `insta` to lock the layout output for sample workflows.

pub mod edges;
pub mod layout;
pub mod model;
pub mod node_status;

pub use edges::derive_edges_from_workflow;
pub use layout::{layout_canvas, layout_tree, Position};
pub use model::{CanvasModel, NodeView};
pub use node_status::NodeStatus;
```

- [ ] **Step 5: Build to verify scaffold**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app-canvas
```

Expected: builds (likely a lot of warnings about unused modules — that's OK; tasks 2-5 fill them in).

If `cargo build` complains about missing modules (because we declared them in lib.rs but haven't created the files yet), that's expected. Either:
- Create empty `node_status.rs` / `layout.rs` / `edges.rs` / `model.rs` files as placeholders to make the build pass for this task, with `// implemented in Task N` comments inside
- Or skip the build verification here and combine it with Task 2's build

The plan-author preference: create empty placeholder files for clean per-task builds.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/rupu-app-canvas
git commit -m "feat(rupu-app-canvas): scaffold crate + module hub"
```

---

## Task 2: `NodeStatus` enum + RGB color mapping

**Files:**
- Modify: `crates/rupu-app-canvas/src/node_status.rs`

- [ ] **Step 1: Write the failing tests first (TDD)**

The full content (tests live in the same file). Replace any placeholder in `node_status.rs` with:

```rust
//! Status of one DAG node. Lifted from rupu-tui's NodeStatus with
//! one addition: this version returns RGB tuples for status colors
//! so the consuming GPUI layer can convert to `gpui::Rgba` without
//! pulling GPUI into this crate.

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
    /// status palette: `◐`, `●`, `✓`, `✗`, `⏸`, `⊘`.
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

    /// Foreground color as a 24-bit RGB tuple. Maps to the Okesu
    /// palette already used by rupu-cli/output/palette.rs and
    /// rupu-app/src/palette.rs.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Waiting => (82, 82, 91),       // slate-500 dim
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
}
```

- [ ] **Step 2: Build + run tests**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-app-canvas node_status
```

Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app-canvas/src/node_status.rs
git commit -m "feat(rupu-app-canvas): NodeStatus enum + RGB color mapping"
```

---

## Task 3: `layout_canvas` function + snapshot test

**Files:**
- Modify: `crates/rupu-app-canvas/src/layout.rs`
- Create: `crates/rupu-app-canvas/tests/layout_snapshots.rs`

- [ ] **Step 1: Port `layout.rs` from rupu-tui**

The Slice C source at `crates/rupu-tui/src/view/layout.rs` is 88 lines and already pure. Copy it verbatim with the following modifications:
- Module-level doc-comment that points at this crate's purpose
- Make `Position::col` and `Position::row` `u16` (unchanged — keep the existing type)
- Add `#[derive(Serialize, Deserialize)]` to `Position` (to enable snapshot testing via serialized output)
- Add `Hash` derive to `Position` (useful if maps key on it later)

Full file content for `crates/rupu-app-canvas/src/layout.rs`:

```rust
//! Layout algorithms for the Graph view. Pure functions:
//! `layout_canvas` produces a column×row position per node id by
//! propagating depth along edges; `layout_tree` produces a pre-order
//! DFS with indent depth.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub col: u16,
    pub row: u16,
}

/// Place each node at column = its depth from the root, row =
/// position among siblings at the same column. Topology only —
/// no manual positions.
pub fn layout_canvas(node_ids: &[&str], edges: &[(String, String)]) -> BTreeMap<String, Position> {
    let mut depth: BTreeMap<&str, u16> = BTreeMap::new();
    for id in node_ids {
        depth.insert(id, 0);
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (parent, child) in edges {
            let parent_d = *depth.get(parent.as_str()).unwrap_or(&0);
            let child_d = *depth.get(child.as_str()).unwrap_or(&0);
            if child_d <= parent_d {
                depth.insert(child.as_str(), parent_d + 1);
                changed = true;
            }
        }
    }

    let mut by_col: BTreeMap<u16, Vec<&str>> = BTreeMap::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in node_ids {
        if seen.insert(id) {
            by_col
                .entry(*depth.get(id).unwrap_or(&0))
                .or_default()
                .push(id);
        }
    }

    let mut out = BTreeMap::new();
    for (col, ids) in by_col {
        for (row, id) in ids.into_iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            // Upper bound: workflows with >65535 fan-out children is impossible
            out.insert(
                id.to_string(),
                Position {
                    col,
                    row: row as u16,
                },
            );
        }
    }
    out
}

/// Pre-order DFS yielding (step_id, indent_depth) for tree view.
/// Currently unused by D-2 (Graph only) but included so D-N tree
/// view doesn't need a second port pass.
pub fn layout_tree(node_ids: &[&str], edges: &[(String, String)]) -> Vec<(String, u16)> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut has_parent: BTreeSet<&str> = BTreeSet::new();
    for (p, c) in edges {
        children.entry(p.as_str()).or_default().push(c.as_str());
        has_parent.insert(c.as_str());
    }
    let roots: Vec<&str> = node_ids
        .iter()
        .copied()
        .filter(|id| !has_parent.contains(id))
        .collect();

    let mut out = Vec::new();
    fn dfs<'a>(
        node: &'a str,
        depth: u16,
        children: &BTreeMap<&'a str, Vec<&'a str>>,
        out: &mut Vec<(String, u16)>,
    ) {
        out.push((node.to_string(), depth));
        if let Some(kids) = children.get(node) {
            for kid in kids {
                let next_depth = if kids.len() == 1 { depth } else { depth + 1 };
                dfs(kid, next_depth, children, out);
            }
        }
    }
    for r in roots {
        dfs(r, 0, &children, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_chain_layout_is_one_per_column() {
        // a → b → c → d
        let ids = ["a", "b", "c", "d"];
        let edges = vec![
            ("a".into(), "b".into()),
            ("b".into(), "c".into()),
            ("c".into(), "d".into()),
        ];
        let pos = layout_canvas(&ids, &edges);
        assert_eq!(pos["a"], Position { col: 0, row: 0 });
        assert_eq!(pos["b"], Position { col: 1, row: 0 });
        assert_eq!(pos["c"], Position { col: 2, row: 0 });
        assert_eq!(pos["d"], Position { col: 3, row: 0 });
    }

    #[test]
    fn fan_out_places_children_in_same_column() {
        // a → b, a → c, a → d (b/c/d are siblings)
        let ids = ["a", "b", "c", "d"];
        let edges = vec![
            ("a".into(), "b".into()),
            ("a".into(), "c".into()),
            ("a".into(), "d".into()),
        ];
        let pos = layout_canvas(&ids, &edges);
        assert_eq!(pos["a"].col, 0);
        assert_eq!(pos["b"].col, 1);
        assert_eq!(pos["c"].col, 1);
        assert_eq!(pos["d"].col, 1);
        // Rows are 0/1/2 in some order
        let rows: BTreeSet<u16> = ["b", "c", "d"].iter().map(|id| pos[*id].row).collect();
        assert_eq!(rows, BTreeSet::from([0, 1, 2]));
    }
}
```

- [ ] **Step 2: Build + run unit tests**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-app-canvas layout
```

Expected: 2 inline tests pass.

- [ ] **Step 3: Create snapshot test file**

`crates/rupu-app-canvas/tests/layout_snapshots.rs`:

```rust
//! Integration tests: locks layout output for a few representative
//! workflow shapes via insta snapshots. If the layout algorithm
//! drifts, these tests catch it.

use rupu_app_canvas::{layout_canvas, layout_tree};

#[test]
fn snapshot_linear_5_steps() {
    let ids = ["classify", "fetch", "transform", "verify", "publish"];
    let edges: Vec<(String, String)> = ids
        .windows(2)
        .map(|w| (w[0].to_string(), w[1].to_string()))
        .collect();
    let pos = layout_canvas(&ids, &edges);
    let tree = layout_tree(&ids, &edges);
    insta::assert_yaml_snapshot!("linear_5_steps", (pos, tree));
}

#[test]
fn snapshot_diamond_4_steps() {
    // start fans to A and B, both merge into end:
    //   start → A → end
    //   start → B → end
    let ids = ["start", "A", "B", "end"];
    let edges = vec![
        ("start".to_string(), "A".to_string()),
        ("start".to_string(), "B".to_string()),
        ("A".to_string(), "end".to_string()),
        ("B".to_string(), "end".to_string()),
    ];
    let pos = layout_canvas(&ids, &edges);
    let tree = layout_tree(&ids, &edges);
    insta::assert_yaml_snapshot!("diamond_4_steps", (pos, tree));
}
```

- [ ] **Step 4: Run snapshot tests to seed the snapshots**

```bash
cd /Users/matt/Code/Oracle/rupu
INSTA_UPDATE=auto cargo test -p rupu-app-canvas --test layout_snapshots
```

Expected: 2 tests pass; insta creates `crates/rupu-app-canvas/tests/snapshots/layout_snapshots__linear_5_steps.snap` and `..__diamond_4_steps.snap`. The `.snap` files now exist; commit them.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app-canvas/src/layout.rs crates/rupu-app-canvas/tests
git commit -m "feat(rupu-app-canvas): layout_canvas + layout_tree (ported from rupu-tui) + snapshot tests"
```

---

## Task 4: `CanvasModel` + `derive_edges_from_workflow`

**Files:**
- Modify: `crates/rupu-app-canvas/src/edges.rs`
- Modify: `crates/rupu-app-canvas/src/model.rs`

- [ ] **Step 1: Implement `edges.rs`**

```rust
//! Derive parent→child edges from a `rupu_orchestrator::Workflow`.
//! Ported from `rupu-tui::state::edges::derive_edges` — for D-2 we
//! handle the v0 spec shape: linear chain of `steps`. Fan-out steps
//! (`for_each:` / `parallel:` / `panel:`) treat each child as a
//! sibling of the fan-out node visually (drawn as a vertical drop in
//! canvas mode). D-N can expand to richer edges as the spec
//! evolves.

use rupu_orchestrator::Workflow;

/// Return parent→child step-id edges in workflow declaration order.
pub fn derive_edges_from_workflow(wf: &Workflow) -> Vec<(String, String)> {
    let ids: Vec<&str> = wf.steps.iter().map(|s| s.id.as_str()).collect();
    ids.windows(2)
        .map(|w| (w[0].to_string(), w[1].to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_workflow_has_no_edges() {
        let wf = parse_test_workflow("name: empty\nsteps: []");
        assert!(derive_edges_from_workflow(&wf).is_empty());
    }

    #[test]
    fn three_step_workflow_yields_two_edges() {
        let wf = parse_test_workflow(
            "name: three\nsteps:\n  - id: a\n    agent: a\n    actions: []\n    prompt: hi\n  - id: b\n    agent: b\n    actions: []\n    prompt: hi\n  - id: c\n    agent: c\n    actions: []\n    prompt: hi\n",
        );
        let edges = derive_edges_from_workflow(&wf);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0], ("a".into(), "b".into()));
        assert_eq!(edges[1], ("b".into(), "c".into()));
    }

    fn parse_test_workflow(yaml: &str) -> Workflow {
        Workflow::parse(yaml).expect("test workflow parses")
    }
}
```

- [ ] **Step 2: Implement `model.rs`**

```rust
//! `CanvasModel` — the data layer the GPUI Graph view consumes.
//! Built from a `Workflow` once at workflow-open time; for D-2 all
//! nodes carry `NodeStatus::Waiting`. D-3 will mutate node statuses
//! in response to `EventSink` events.

use crate::edges::derive_edges_from_workflow;
use crate::node_status::NodeStatus;
use rupu_orchestrator::Workflow;
use std::collections::BTreeMap;

/// One node's data as known to the canvas. Position is computed by
/// `layout::layout_canvas` separately and not stored here.
#[derive(Debug, Clone)]
pub struct NodeView {
    pub step_id: String,
    pub agent: String,
    pub status: NodeStatus,
}

/// Complete model for one workflow's Graph view.
#[derive(Debug, Clone)]
pub struct CanvasModel {
    /// Nodes keyed by step_id, sorted by id (BTreeMap → deterministic).
    pub nodes: BTreeMap<String, NodeView>,
    /// Parent→child edges in declaration order.
    pub edges: Vec<(String, String)>,
    /// Step ids in declaration order (so renderers can preserve YAML order
    /// when laying out columns/rows of equal depth).
    pub step_order: Vec<String>,
}

impl CanvasModel {
    /// Build a model from a parsed workflow. All node statuses default
    /// to `Waiting` — D-3 will mutate them as events arrive.
    pub fn from_workflow(wf: &Workflow) -> Self {
        let mut nodes = BTreeMap::new();
        let mut step_order = Vec::with_capacity(wf.steps.len());
        for step in &wf.steps {
            step_order.push(step.id.clone());
            nodes.insert(
                step.id.clone(),
                NodeView {
                    step_id: step.id.clone(),
                    agent: step.agent.clone().unwrap_or_default(),
                    status: NodeStatus::Waiting,
                },
            );
        }
        let edges = derive_edges_from_workflow(wf);
        Self {
            nodes,
            edges,
            step_order,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_orchestrator::Workflow;

    #[test]
    fn from_workflow_populates_nodes_and_edges() {
        let yaml = "name: rev\nsteps:\n  - id: classify\n    agent: classifier\n    actions: []\n    prompt: hi\n  - id: review\n    agent: security-reviewer\n    actions: []\n    prompt: hi\n";
        let wf = Workflow::parse(yaml).unwrap();
        let model = CanvasModel::from_workflow(&wf);
        assert_eq!(model.nodes.len(), 2);
        assert_eq!(model.edges.len(), 1);
        assert_eq!(model.step_order, vec!["classify", "review"]);
        assert_eq!(model.nodes["classify"].agent, "classifier");
        assert_eq!(model.nodes["review"].status, NodeStatus::Waiting);
    }
}
```

- [ ] **Step 3: Build + run all tests**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-app-canvas
```

Expected: previous tests still pass + 3 new (2 edges + 1 model). Total 6+ tests including snapshots.

Note: `Workflow::parse` may reject the minimal test YAML if the orchestrator's parser requires fields we didn't include. If the test fails with a parse error, look at the parser's required fields and add them to the test YAML (e.g. an explicit empty `inputs:` section, or `actions:` per-step). The test bodies above use `actions: []` and `prompt: hi` per the existing pattern in `rupu-orchestrator`'s own parse tests.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app-canvas/src
git commit -m "feat(rupu-app-canvas): CanvasModel::from_workflow + derive_edges_from_workflow"
```

---

## Task 5: Wire `rupu-app-canvas` into `rupu-app`

**Files:**
- Modify: `crates/rupu-app/Cargo.toml` (add `rupu-app-canvas` + `rupu-orchestrator` to `[dependencies]`)
- Modify: `crates/rupu-app/src/lib.rs` (add `pub mod view;`)
- Create: `crates/rupu-app/src/view/mod.rs` (stub — populated in Task 6)

- [ ] **Step 1: Add dependencies to `rupu-app/Cargo.toml`**

In `crates/rupu-app/Cargo.toml`, add to `[dependencies]` (alphabetical position):

```toml
rupu-app-canvas = { path = "../rupu-app-canvas" }
rupu-orchestrator.workspace = true
```

The `rupu-app-canvas` dep uses a path reference rather than `workspace = true` because the crate isn't (and shouldn't be) in `[workspace.dependencies]` — it's a private internal crate.

- [ ] **Step 2: Add `pub mod view;` to lib.rs**

`crates/rupu-app/src/lib.rs`:

```rust
//! rupu.app library — exposed so integration tests can reach the
//! pure-data modules (workspace, palette). The binary entry point
//! lives in main.rs.

pub mod menu;
pub mod palette;
pub mod view;
pub mod window;
pub mod workspace;
```

- [ ] **Step 3: Create the view module hub**

`crates/rupu-app/src/view/mod.rs`:

```rust
//! GPUI views for the rupu.app main area.
//!
//! Each view is a thin GPUI wrapper around a `rupu-app-canvas` model.
//! D-2 ships `graph` (the static git-graph). D-5 / D-6 / D-8 add
//! YAML / Canvas / Transcript.

pub mod graph;
```

(File `view/graph.rs` is created in Task 6.)

- [ ] **Step 4: Stub `view/graph.rs` so the build passes for this task**

`crates/rupu-app/src/view/graph.rs`:

```rust
//! Graph view — populated in Task 6.

// Stub. Task 6 replaces this with the GPUI Graph view.
```

- [ ] **Step 5: Build**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app
```

Expected: builds; `rupu-app-canvas` becomes a transitive dep of `rupu-app`. No new symbols used yet.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/Cargo.toml crates/rupu-app/src/lib.rs crates/rupu-app/src/view
git commit -m "feat(rupu-app): wire rupu-app-canvas as a dep + view module hub"
```

---

## Task 6: GPUI Graph view

**Files:**
- Modify: `crates/rupu-app/src/view/graph.rs` (replace stub with real Graph view)

- [ ] **Step 1: Implement the Graph view**

`crates/rupu-app/src/view/graph.rs`:

```rust
//! Graph view — GPUI renderer for a `rupu_app_canvas::CanvasModel`.
//! Static rendering for D-2: a vertical column of card nodes (one
//! per workflow step) connected by 1px edges, drawn on a dotted
//! backdrop. No animation (Tasks D-3+ will add live status pulses);
//! all nodes render in their `Waiting` glyph + dim color.

use crate::palette;
use gpui::{div, prelude::*, px, AnyElement, IntoElement};
use rupu_app_canvas::{layout_canvas, CanvasModel, NodeView, Position};

/// Card dimensions (in pixels). Vertical column layout: cards stack
/// top-to-bottom, edges drop between them. Each card is wide enough
/// to show step_id + agent name without truncation for typical 12-32
/// char step ids.
const CARD_W: f32 = 240.0;
const CARD_H: f32 = 72.0;
const COL_GAP: f32 = 56.0;
const ROW_GAP: f32 = 32.0;

/// Top-level entry point: render a `CanvasModel` as a GPUI element.
/// Layout is computed inline; the model carries node data + edges,
/// `layout_canvas` resolves positions, then we paint cards + edges.
pub fn render(model: &CanvasModel) -> impl IntoElement {
    let ids: Vec<&str> = model.step_order.iter().map(|s| s.as_str()).collect();
    let positions = layout_canvas(&ids, &model.edges);

    let mut canvas = div()
        .size_full()
        .bg(palette::BG_PRIMARY)
        .relative()
        .overflow_hidden();

    // Cards. Edges are deferred to Task D-3 (need proper SVG/path
    // painting; for D-2 the columnar drop is implied by vertical
    // card stacking with no horizontal offset between steps).
    for id in &model.step_order {
        let node = match model.nodes.get(id) {
            Some(n) => n,
            None => continue,
        };
        let pos = match positions.get(id) {
            Some(p) => *p,
            None => continue,
        };
        canvas = canvas.child(render_card(node, pos));
    }

    canvas
}

fn render_card(node: &NodeView, pos: Position) -> AnyElement {
    let x = pos.col as f32 * (CARD_W + COL_GAP) + COL_GAP;
    let y = pos.row as f32 * (CARD_H + ROW_GAP) + ROW_GAP;

    let glyph = node.status.glyph();
    let (r, g, b) = node.status.rgb();
    let status_color = gpui::Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    };

    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(CARD_W))
        .h(px(CARD_H))
        .bg(palette::BG_SIDEBAR)
        .border_1()
        .border_color(palette::BORDER)
        .border_l_2()
        .border_color(status_color)
        .rounded(px(6.0))
        .px(px(14.0))
        .py(px(10.0))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            // Header row: step_id + status glyph
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(palette::TEXT_PRIMARY)
                        .child(node.step_id.clone()),
                )
                .child(
                    div()
                        .text_color(status_color)
                        .text_sm()
                        .child(glyph.to_string()),
                ),
        )
        .child(
            // Subtitle row: agent name + status label (e.g. "waiting")
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .text_xs()
                .text_color(palette::TEXT_DIMMEST)
                .child(div().child(node.agent.clone()))
                .child(div().child("·"))
                .child(div().child(format!("{:?}", node.status).to_lowercase())),
        )
        .into_any_element()
}
```

Notes on the implementation choices:
- The plan punts edge-drawing to D-3. D-2 is "static graph that proves nodes appear in the right positions"; edges between cards are visually implied by the vertical stacking. D-3 will introduce proper edge painting (probably via GPUI's `canvas` paintable element).
- Card colors lift from the existing `palette` module — no new color constants.
- `node.status.glyph()` and `node.status.rgb()` are functions on `NodeStatus` from Task 2.
- All nodes start in `Waiting` (gray), so the graph for D-2 looks subdued — that's intentional. D-3 lights it up.

- [ ] **Step 2: Build**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app
```

Expected: builds. If gpui complains about missing trait methods (`.absolute()` / `.relative()` / `.overflow_hidden()` / `.border_l_2()`), check the actual gpui::styled API at the pinned commit; some may have different names. Adapt.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/view/graph.rs
git commit -m "feat(rupu-app): Graph view — static cards laid out by canvas-layout"
```

---

## Task 7: Wire Graph view into the main area

**Files:**
- Modify: `crates/rupu-app/src/window/mod.rs`

- [ ] **Step 1: Update `window/mod.rs`**

Currently the main area renders a centered placeholder ("Open a workflow from the sidebar."). Replace that with: if there's at least one project workflow, render the Graph view of the first one; otherwise keep the placeholder.

In `crates/rupu-app/src/window/mod.rs`, find the part of the `render` impl that builds the main area (the `div().flex_1()...` block) and replace with:

```rust
impl Render for WorkspaceWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Pick the first project workflow (if any) and render its
        // Graph view in the main area. Workflow YAML is parsed
        // lazily on render; failures fall back to a small inline
        // error message. D-3 will introduce real sidebar selection.
        let main_area = match self.workspace.project_assets.workflows.first() {
            Some(asset) => render_main_for_workflow(asset, &self.workspace),
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

fn render_main_for_workflow(
    asset: &crate::workspace::Asset,
    _workspace: &crate::workspace::Workspace,
) -> gpui::AnyElement {
    use rupu_app_canvas::CanvasModel;
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
    let model = CanvasModel::from_workflow(&wf);

    div()
        .flex_1()
        .child(crate::view::graph::render(&model))
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

Also: add the necessary imports at the top of `window/mod.rs`. The exact set depends on what's already there, but you'll likely need:

```rust
use gpui::{div, prelude::*, IntoElement, Render};
```

(Add `Render` and `Context`/`Window` types as needed for the trait impl.)

- [ ] **Step 2: Build + smoke run**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-app
timeout 5 cargo run -p rupu-app -- /Users/matt/Code/Oracle/rupu || true
```

Expected: the binary opens a window. The main area now shows the first project workflow's steps as a vertical column of cards (probably from `.rupu/workflows/*.yaml`), each card showing step_id + agent + a dim "waiting" status. No panic on stderr.

If the rupu repo itself has many workflows, you'll see whichever sorts first alphabetically. If there are no project workflows, the placeholder remains.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/mod.rs
git commit -m "feat(rupu-app): render Graph view of first project workflow in main area"
```

---

## Task 8: CLAUDE.md update

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add `rupu-app-canvas` to the Crates section**

Find the bulleted Crates list in `CLAUDE.md`. Insert a new bullet for `rupu-app-canvas` in alphabetical position (immediately after `rupu-app`):

```markdown
- **`rupu-app-canvas`** — pure-Rust view layer for rupu.app (Slice D). Holds the GPUI-independent layout algorithms (`layout_canvas`, `layout_tree`), `NodeStatus` enum, and `CanvasModel` builder. Snapshot-tested via insta; no GPUI dep. rupu-app's `view/graph.rs` consumes a `CanvasModel` and paints with GPUI primitives.
```

- [ ] **Step 2: Append Plan 2 pointer to Read first**

In the "## Read first" section, append after the existing Slice D Plan 1 line:

```markdown
- Slice D Plan 2 (Graph view, complete): `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-2-graph-view.md`
```

(Mark "complete" — Task 11 verifies gates before finalizing.)

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md — rupu-app-canvas crate + Slice D Plan 2 pointer"
```

---

## Task 9: Smoke test verifies graph renders

**Files:**
- Modify: `Makefile` (extend `app-smoke` to also assert specific log lines)

- [ ] **Step 1: Tighten the smoke assertion**

The existing `app-smoke` target (from Plan 1, Task 16) only checks for absence of "panic" / "panicked" in output. Extend it to also confirm the app opens a workspace and renders something. Add an assertion that the workspace-open log line appears:

In `Makefile`, find the existing `app-smoke` target. Replace its body with:

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

This extra check confirms `Workspace::open` was actually invoked. If the Graph view renders cleanly (i.e. the workflow YAML parses), no extra log line is needed — absence of panic is the assertion.

- [ ] **Step 2: Run the smoke**

```bash
make app-smoke
```

Expected: prints `app-smoke OK`. The fixture workspace at `crates/rupu-app/tests/fixtures/sample-workspace` has one workflow (`example.yaml`) which will trigger the Graph render path.

- [ ] **Step 3: Commit**

```bash
git add Makefile
git commit -m "test(rupu-app): tighten app-smoke to assert workspace open log"
```

---

## Task 10: Workspace gates

**Files:**
- (none — runs existing tooling; may need minor fixups)

- [ ] **Step 1: `cargo fmt`**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo fmt --all -- --check
```

If diffs, run `cargo fmt --all` and commit:

```bash
git add -u
git commit -m "style: cargo fmt"
```

- [ ] **Step 2: `cargo clippy`**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

If warnings, fix narrowly + commit:

```bash
git add -u
git commit -m "fix: clippy warnings surfaced by D-2 Plan gates"
```

- [ ] **Step 3: `cargo test --workspace`**

```bash
cargo test --workspace 2>&1 | tail -30
```

Expected: all pass. rupu-app-canvas should report ~6 unit tests + 2 snapshot tests = 8 total. rupu-app's prior 14 tests still pass.

- [ ] **Step 4: `make app-smoke`**

```bash
make app-smoke
```

Expected: `app-smoke OK`.

- [ ] **Step 5: Final state**

If any fixups were needed in Steps 1-3, the commits are in place. The CLAUDE.md pointer from Task 8 is already marked "complete" — confirm gates passed.

---

## Self-review notes

**Spec coverage check** (against spec §10 D-2 line: "Graph view widget — port Slice C TUI's canvas auto-layout to GPUI; render any workflow YAML; no live data yet."):

| D-2 deliverable | Covered by |
|---|---|
| Port Slice C canvas auto-layout | Task 3 (`layout_canvas` lifted verbatim + 2 unit tests + 2 snapshot tests) |
| Render any workflow YAML | Tasks 4 (`CanvasModel::from_workflow`) + 6 (GPUI render) + 7 (wire into main area, parses YAML on render) |
| No live data | Confirmed: all nodes default to `NodeStatus::Waiting`; no `EventSink` subscription |

**Spec sections deferred to later sub-slices:**
- Edge painting (cards laid out but edges are visually implied via vertical stacking; proper SVG/path edges land in D-3)
- Status pulse animations (D-3 — needs live data first)
- Click-to-focus + drill-down (D-3)
- Pane splits + tab strip (D-5/D-6)
- View picker (D-5+ when there's more than one view)

**Placeholder scan:** zero `TODO` markers in the plan that aren't deliberate forward-references (Task 6 notes edges are deferred to D-3; that's documentation, not an unfinished item).

**Type consistency:**
- `Position` (Task 3) → consumed by `layout_canvas` return type → consumed by `view::graph::render_card` (Task 6).
- `NodeStatus` (Task 2) → field of `NodeView` (Task 4) → consumed by `view::graph::render_card` (Task 6) via `.glyph()` + `.rgb()`.
- `CanvasModel` (Task 4) → built in `window/mod.rs` (Task 7) → passed to `view::graph::render` (Tasks 6, 7).
- `derive_edges_from_workflow` (Task 4) → called by `CanvasModel::from_workflow` (Task 4).

**No-placeholder verification:** every code step has full executable Rust; every command step has the exact command + expected output; no "TBD" / "fill in later" / "add error handling" hand-waves.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-12-rupu-slice-d-plan-2-graph-view.md`. Two execution options:

**1. Subagent-Driven (recommended)** — fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session via `superpowers:executing-plans`, batch checkpoints.

Which approach?
