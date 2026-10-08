//! Permission gating for MCP tools — per-tool allowlist, then the one
//! [`PermissionPolicy`] decides from the tool's effect (W4 deletes this type
//! when MCP becomes a transport over the catalog's resolved grant).

use crate::error::McpError;
use rupu_tools::{AllowAlways, Decision, Effect, PermissionMode, PermissionPolicy};

#[derive(Clone)]
pub struct McpPermission {
    mode: PermissionMode,
    allowlist: Vec<String>,
}

impl McpPermission {
    pub fn new(mode: PermissionMode, allowlist: Vec<String>) -> Self {
        Self { mode, allowlist }
    }

    /// Same mode, but the allowlist narrowed to exactly one tool (ISSUES.md
    /// I-79).
    ///
    /// Used by `execute_action_step` so an `action:` step's dispatch is
    /// constrained *structurally* to the tool named in the workflow, rather
    /// than relying on the invariant that the run-scoped dispatcher is only
    /// ever handed one tool name. That invariant holds today, but it is
    /// enforced three modules away, so any future caller reusing the
    /// dispatcher would silently widen the surface with no local signal.
    pub fn narrowed_to(&self, tool: &str) -> Self {
        Self {
            mode: self.mode,
            allowlist: vec![tool.to_string()],
        }
    }

    /// Bypass mode + `*` allowlist — used by `rupu mcp serve` (where the
    /// upstream MCP client handles confirmation prompts) and by tests.
    pub fn allow_all() -> Self {
        Self {
            mode: PermissionMode::Bypass,
            allowlist: vec!["*".into()],
        }
    }

    /// Per-tool gating: allowlist match first, then [`PermissionPolicy`] on
    /// the tool's effect. There is no operator here: `ask` allows (the agent
    /// loop, which has one, has already decided the call by the time it
    /// reaches this dispatcher).
    pub fn check(&self, tool: &'static str, effect: Effect) -> Result<(), McpError> {
        if !self.tool_in_allowlist(tool) {
            return Err(McpError::PermissionDenied {
                tool: tool.to_string(),
                reason: format!(
                    "tool not in agent's `tools:` list (allowlist: {:?})",
                    self.allowlist
                ),
            });
        }
        let decision = PermissionPolicy::unattended(self.mode).decide_call(
            tool,
            effect,
            &serde_json::Value::Null,
            &mut AllowAlways::default(),
        );
        match decision {
            Decision::Deny { .. } | Decision::Stop => Err(McpError::PermissionDenied {
                tool: tool.to_string(),
                reason: format!("{} mode blocks {} tools", self.mode, effect.as_str()),
            }),
            Decision::Allow | Decision::AllowDegraded | Decision::Spawn { .. } => Ok(()),
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
        assert!(p.check("scm.repos.list", Effect::Read).is_ok());
        assert!(p.check("scm.prs.create", Effect::External).is_ok());
        assert!(p.check("issues.get", Effect::Read).is_err());
        assert!(p
            .check("github.workflows_dispatch", Effect::External)
            .is_err());
    }

    #[test]
    fn allowlist_exact_match() {
        let p = McpPermission::new(
            PermissionMode::Bypass,
            vec!["scm.repos.list".into(), "issues.get".into()],
        );
        assert!(p.check("scm.repos.list", Effect::Read).is_ok());
        assert!(p.check("issues.get", Effect::Read).is_ok());
        assert!(p.check("scm.repos.get", Effect::Read).is_err());
    }

    #[test]
    fn star_allows_all_namespaces() {
        let p = McpPermission::new(PermissionMode::Bypass, vec!["*".into()]);
        assert!(p.check("scm.repos.list", Effect::Read).is_ok());
        assert!(p.check("issues.create", Effect::External).is_ok());
        assert!(p
            .check("github.workflows_dispatch", Effect::External)
            .is_ok());
    }

    #[test]
    fn readonly_allows_record_tools() {
        // D4: readonly denies workspace writes and external actions, never
        // rupu's own bookkeeping.
        let p = McpPermission::new(PermissionMode::Readonly, vec!["*".into()]);
        assert!(p.check("findings.tag", Effect::Record).is_ok());
    }

    #[test]
    fn readonly_blocks_writes_even_when_allowlisted() {
        let p = McpPermission::new(PermissionMode::Readonly, vec!["*".into()]);
        assert!(p.check("scm.repos.list", Effect::Read).is_ok());
        let err = p.check("scm.prs.create", Effect::External).unwrap_err();
        assert!(matches!(err, McpError::PermissionDenied { .. }));
    }
}
