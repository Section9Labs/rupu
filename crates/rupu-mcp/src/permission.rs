//! Permission gating for MCP tools — per-tool allowlist + per-mode.

use crate::error::McpError;
use crate::tools::ToolKind;
use rupu_tools::PermissionMode;
use std::sync::Arc;

/// Callback type for `--mode ask` prompts. Returns `true` to allow the call.
pub type AskCb = Arc<dyn Fn(&str, &serde_json::Value) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct McpPermission {
    mode: PermissionMode,
    allowlist: Vec<String>,
    /// Optional callback for `--mode ask`. Returns Ok(true) to allow.
    /// `None` means deny all writes silently in --mode ask.
    ask_cb: Option<AskCb>,
}

impl McpPermission {
    pub fn new(mode: PermissionMode, allowlist: Vec<String>) -> Self {
        Self {
            mode,
            allowlist,
            ask_cb: None,
        }
    }

    /// Bypass mode + `*` allowlist — used by `rupu mcp serve` (where the
    /// upstream MCP client handles confirmation prompts) and by tests.
    pub fn allow_all() -> Self {
        Self {
            mode: PermissionMode::Bypass,
            allowlist: vec!["*".into()],
            ask_cb: None,
        }
    }

    pub fn with_ask_callback(mut self, cb: AskCb) -> Self {
        self.ask_cb = Some(cb);
        self
    }

    /// Per-tool gating. Allowlist match first, then mode check.
    pub fn check(&self, tool: &str, kind: ToolKind) -> Result<(), McpError> {
        if !self.tool_in_allowlist(tool) {
            return Err(McpError::PermissionDenied {
                tool: tool.to_string(),
                reason: format!(
                    "tool not in agent's `tools:` list (allowlist: {:?})",
                    self.allowlist
                ),
            });
        }
        match (self.mode, kind) {
            (PermissionMode::Readonly, ToolKind::Write) => Err(McpError::PermissionDenied {
                tool: tool.to_string(),
                reason: "readonly mode blocks write tools".into(),
            }),
            // Ask mode: allowed at the gate; runtime ask happens via ask_cb
            // (driven by the agent runtime in Task 18, not the MCP server).
            _ => Ok(()),
        }
    }

    fn tool_in_allowlist(&self, tool: &str) -> bool {
        self.allowlist.iter().any(|entry| {
            if entry == "*" || entry == tool {
                return true;
            }
            if let Some(prefix) = entry.strip_suffix('*') {
                tool.starts_with(prefix)
            } else {
                false
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_wildcard_matches_namespace() {
        let p = McpPermission::new(PermissionMode::Bypass, vec!["scm.*".into()]);
        assert!(p.check("scm.repos.list", ToolKind::Read).is_ok());
        assert!(p.check("scm.prs.create", ToolKind::Write).is_ok());
        assert!(p.check("issues.get", ToolKind::Read).is_err());
        assert!(p
            .check("github.workflows_dispatch", ToolKind::Write)
            .is_err());
    }

    #[test]
    fn allowlist_exact_match() {
        let p = McpPermission::new(
            PermissionMode::Bypass,
            vec!["scm.repos.list".into(), "issues.get".into()],
        );
        assert!(p.check("scm.repos.list", ToolKind::Read).is_ok());
        assert!(p.check("issues.get", ToolKind::Read).is_ok());
        assert!(p.check("scm.repos.get", ToolKind::Read).is_err());
    }

    #[test]
    fn star_allows_all_namespaces() {
        let p = McpPermission::new(PermissionMode::Bypass, vec!["*".into()]);
        assert!(p.check("scm.repos.list", ToolKind::Read).is_ok());
        assert!(p.check("issues.create", ToolKind::Write).is_ok());
        assert!(p
            .check("github.workflows_dispatch", ToolKind::Write)
            .is_ok());
    }

    #[test]
    fn readonly_blocks_writes_even_when_allowlisted() {
        let p = McpPermission::new(PermissionMode::Readonly, vec!["*".into()]);
        assert!(p.check("scm.repos.list", ToolKind::Read).is_ok());
        let err = p.check("scm.prs.create", ToolKind::Write).unwrap_err();
        assert!(matches!(err, McpError::PermissionDenied { .. }));
    }
}
