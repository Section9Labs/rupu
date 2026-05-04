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
}
