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

pub mod findings;
pub mod github_extras;
pub mod gitlab_extras;
pub mod issues;
pub mod scm_branches;
pub mod scm_files;
pub mod scm_prs;
pub mod scm_repos;

use rupu_tools::Effect;
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

impl ToolSpec {
    /// The tool's [`Effect`] — the ONE place a `ToolKind` maps to an effect
    /// (W1; W4 replaces `ToolKind` with catalog descriptors). Reads observe;
    /// writes act on a system outside this machine (`External`), except the
    /// findings writes, which only touch rupu's own ledgers (`Record`).
    pub fn effect(&self) -> Effect {
        match self.kind {
            ToolKind::Read => Effect::Read,
            ToolKind::Write if matches!(self.name, "findings.record" | "findings.tag") => {
                Effect::Record
            }
            ToolKind::Write => Effect::External,
        }
    }
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
    v.extend(findings::specs());
    v
}
