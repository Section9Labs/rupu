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

// re-enabled by Task 3 once the items exist.
// pub use git_graph::{render_rows, BranchGlyph, GraphCell, GraphRow};
// pub use node_status::NodeStatus;
