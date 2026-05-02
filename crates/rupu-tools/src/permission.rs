//! Permission gating for tool calls. Real impl lands in Task 18.

use serde::{Deserialize, Serialize};

/// Three permission modes the agent runtime can run in. Resolved
/// from CLI flag > agent frontmatter > project config > global config
/// > default (`Ask`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Prompt the operator before each write-class tool call. Default.
    Ask,
    /// Allow all tool calls without prompting. Use for unattended runs.
    Bypass,
    /// Allow only read-class tools; deny writers outright.
    Readonly,
}

/// Decision API for whether a tool call is allowed under a given
/// permission mode. Implementation lands in Task 18.
pub struct PermissionGate;
