//! Tool catalog for the unified MCP surface.
//!
//! Each module under `tools/` exposes:
//!   - `specs()` returning Vec<ToolSpec> for tools/list registration
//!   - per-tool `dispatch_*` async fns invoked by ToolDispatcher (Task 14)
//!
//! Conventions:
//!   - Tool names use dot-namespacing: "<namespace>.<resource>.<verb>".
//!   - `platform?` / `tracker?` parameters fall back to [scm.default]
//!     / [issues.default] from rupu-config when omitted.
//!   - All Args structs derive `JsonSchema` so input_schema is auto-generated.

pub mod github_extras;
pub mod gitlab_extras;
pub mod issues;
pub mod scm_branches;
pub mod scm_files;
pub mod scm_prs;
pub mod scm_repos;

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(skip)]
    pub kind: ToolKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolKind {
    #[default]
    Read,
    Write,
}

/// Returns the full tool catalog. Stable order — used by snapshot test.
pub fn tool_catalog() -> Vec<ToolSpec> {
    let mut v = Vec::new();
    v.extend(scm_repos::specs());
    v.extend(scm_branches::specs());
    v.extend(scm_files::specs());
    v.extend(scm_prs::specs());
    v.extend(issues::specs());
    v.extend(github_extras::specs());
    v.extend(gitlab_extras::specs());
    v
}
