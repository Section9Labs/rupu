//! Permission gating for tool calls. Three modes; all logic here is
//! synchronous and pure. Interactive prompt UX (for `Ask` mode) lives
//! in `rupu-cli` (Plan 2) and consumes this gate as a pure decision API.
//!
//! Mode resolution (CLI flag > agent frontmatter > project config >
//! global config > default `Ask`) happens upstream of this gate; by
//! the time a tool dispatch reaches `PermissionGate`, the mode is
//! already chosen.

use serde::{Deserialize, Serialize};

/// Three permission modes the agent runtime can run in.
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

/// Pure decision API for whether a tool call is allowed under a given
/// permission mode. The interactive `Ask`-mode prompt UX lives in the
/// CLI (Plan 2); this struct only answers yes/no/needs-decision.
#[derive(Debug, Clone, Copy)]
pub struct PermissionGate {
    mode: PermissionMode,
}

const KNOWN_READ_TOOLS: &[&str] = &["read_file", "grep", "glob"];
const KNOWN_WRITE_TOOLS: &[&str] = &["bash", "write_file", "edit_file"];

impl PermissionGate {
    /// Construct a gate for the given mode.
    pub fn for_mode(mode: PermissionMode) -> Self {
        Self { mode }
    }

    /// The mode this gate was constructed with.
    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// True if `tool` can run with no operator decision under this
    /// mode. False either because the tool is denied outright
    /// (readonly + writer, unknown tool) OR because it needs a
    /// decision (ask + writer — see [`Self::requires_decision`]).
    pub fn allow_unconditionally(&self, tool: &str) -> bool {
        let is_read = KNOWN_READ_TOOLS.contains(&tool);
        let is_write = KNOWN_WRITE_TOOLS.contains(&tool);
        if !is_read && !is_write {
            // Unknown tool: never allow without explicit thought, even
            // under bypass. The runtime will refuse to dispatch it
            // anyway, but the gate also says no.
            return false;
        }
        match self.mode {
            PermissionMode::Bypass => true,
            PermissionMode::Readonly => is_read,
            PermissionMode::Ask => is_read,
        }
    }

    /// True if `tool` needs an operator decision before running. The
    /// CLI's interactive prompt only fires when this returns true.
    pub fn requires_decision(&self, tool: &str) -> bool {
        let is_write = KNOWN_WRITE_TOOLS.contains(&tool);
        matches!(self.mode, PermissionMode::Ask) && is_write
    }

    /// True if the tool is denied outright — no decision will help.
    /// Used by the CLI to short-circuit before the prompt UX.
    pub fn denied_outright(&self, tool: &str) -> bool {
        let is_write = KNOWN_WRITE_TOOLS.contains(&tool);
        let is_read = KNOWN_READ_TOOLS.contains(&tool);
        if !is_read && !is_write {
            return true;
        }
        matches!(self.mode, PermissionMode::Readonly) && is_write
    }
}
