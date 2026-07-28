//! Routes MCP tool name → per-tool dispatch fn. Permission check
//! happens BEFORE dispatch so a denied write tool never reaches the
//! connector.

use crate::error::McpError;
use crate::permission::McpPermission;
use crate::tools::{self, ToolKind};
use rupu_scm::Registry;
use serde_json::Value;
use std::sync::Arc;

pub struct ToolDispatcher {
    registry: Arc<Registry>,
    permission: McpPermission,
}

impl ToolDispatcher {
    pub fn new(registry: Arc<Registry>, permission: McpPermission) -> Self {
        Self {
            registry,
            permission,
        }
    }

    /// A dispatcher over the same registry whose permission allows exactly
    /// `tool` and nothing else (ISSUES.md I-79).
    ///
    /// The run-scoped `action_dispatcher` is built once per run with a `["*"]`
    /// allowlist, because the tool it may call is per-step and unknown at
    /// construction. Narrowing at the call site makes the single-tool
    /// guarantee structural instead of an invariant a reader has to go find.
    pub fn narrowed_to(&self, tool: &str) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
            permission: self.permission.narrowed_to(tool),
        }
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<String, McpError> {
        let kind = self.kind_for(name)?;
        self.permission.check(name, kind)?;
        match name {
            "scm.repos.list" => tools::scm_repos::dispatch_list(args, &self.registry).await,
            "scm.repos.get" => tools::scm_repos::dispatch_get(args, &self.registry).await,
            "scm.branches.list" => tools::scm_branches::dispatch_list(args, &self.registry).await,
            "scm.branches.create" => {
                tools::scm_branches::dispatch_create(args, &self.registry).await
            }
            "scm.files.read" => tools::scm_files::dispatch_read(args, &self.registry).await,
            "scm.prs.list" => tools::scm_prs::dispatch_list(args, &self.registry).await,
            "scm.prs.get" => tools::scm_prs::dispatch_get(args, &self.registry).await,
            "scm.prs.diff" => tools::scm_prs::dispatch_diff(args, &self.registry).await,
            "scm.prs.comment" => tools::scm_prs::dispatch_comment(args, &self.registry).await,
            "scm.prs.create" => tools::scm_prs::dispatch_create(args, &self.registry).await,
            "issues.list" => tools::issues::dispatch_list(args, &self.registry).await,
            "issues.get" => tools::issues::dispatch_get(args, &self.registry).await,
            "issues.comment" => tools::issues::dispatch_comment(args, &self.registry).await,
            "issues.create" => tools::issues::dispatch_create(args, &self.registry).await,
            "issues.update_state" => {
                tools::issues::dispatch_update_state(args, &self.registry).await
            }
            "github.workflows_dispatch" => {
                tools::github_extras::dispatch_workflows_dispatch(args, &self.registry).await
            }
            "gitlab.pipeline_trigger" => {
                tools::gitlab_extras::dispatch_pipeline_trigger(args, &self.registry).await
            }
            other => Err(McpError::UnknownTool(other.to_string())),
        }
    }

    fn kind_for(&self, name: &str) -> Result<ToolKind, McpError> {
        for spec in tools::tool_catalog() {
            if spec.name == name {
                return Ok(spec.kind);
            }
        }
        Err(McpError::UnknownTool(name.to_string()))
    }

    /// Whether a `call` result represents a DENIAL (blocked by the
    /// permission layer) rather than an execution failure. Shared by
    /// every caller that needs to distinguish "denied" from "errored"
    /// for audit purposes (`rupu-orchestrator`'s `execute_action_step`,
    /// which builds the `action:`-node `tool_audit` transcript line)
    /// without duplicating the match against
    /// `McpError::PermissionDenied`. Does not change `call`'s own
    /// error/permission behavior — purely a read-only classifier over
    /// its result.
    pub fn is_blocked(result: &Result<String, McpError>) -> bool {
        matches!(result, Err(McpError::PermissionDenied { .. }))
    }
}

#[cfg(test)]
mod is_blocked_tests {
    use super::*;

    #[test]
    fn permission_denied_is_blocked() {
        let result: Result<String, McpError> = Err(McpError::PermissionDenied {
            tool: "issues.create".into(),
            reason: "readonly mode".into(),
        });
        assert!(ToolDispatcher::is_blocked(&result));
    }

    #[test]
    fn success_is_not_blocked() {
        let result: Result<String, McpError> = Ok("{}".to_string());
        assert!(!ToolDispatcher::is_blocked(&result));
    }

    #[test]
    fn a_non_permission_error_is_not_blocked() {
        // A connector failure (bad request, network error, etc.) is a
        // real failure, but NOT a denial — `blocked` must stay false so
        // the audit trail doesn't conflate "the call was refused" with
        // "the call reached the connector and errored".
        let result: Result<String, McpError> = Err(McpError::UnknownTool("bogus".into()));
        assert!(!ToolDispatcher::is_blocked(&result));
    }

    // ── I-79: narrowing is structural, not invariant-dependent ───────

    #[test]
    fn narrowed_to_allows_only_the_named_tool() {
        // A wildcard dispatcher narrowed to one tool must refuse every other
        // tool, even though its parent allowed everything. This is what makes
        // an action step's single-tool guarantee structural rather than a
        // property of who happens to call it.
        let wide = McpPermission::new(rupu_tools::PermissionMode::Bypass, vec!["*".into()]);
        let narrow = wide.narrowed_to("issues.comment");

        assert!(narrow.check("issues.comment", crate::tools::ToolKind::Write).is_ok());
        assert!(
            narrow.check("scm.prs.create", crate::tools::ToolKind::Write).is_err(),
            "a narrowed permission must refuse a tool outside its single-entry allowlist"
        );
    }

    #[test]
    fn narrowing_preserves_the_mode() {
        // Narrowing must not accidentally widen or tighten the MODE -- a
        // readonly run stays readonly, so Write tools are still refused even
        // when they are the named tool.
        let ro = McpPermission::new(rupu_tools::PermissionMode::Readonly, vec!["*".into()]);
        let narrow = ro.narrowed_to("issues.comment");
        assert!(
            narrow.check("issues.comment", crate::tools::ToolKind::Write).is_err(),
            "narrowing must preserve readonly's refusal of Write tools"
        );
    }
}
