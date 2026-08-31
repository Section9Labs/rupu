//! github.workflows_dispatch tool.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{AccountId, Platform, Registry, RepoRef, WorkflowDispatch};

#[derive(Deserialize, JsonSchema)]
pub struct WorkflowsDispatchArgs {
    pub owner: String,
    pub repo: String,
    /// Workflow filename (e.g. `ci.yml`) or numeric ID.
    pub workflow: String,
    /// Branch / tag / sha to dispatch the workflow against.
    pub r#ref: String,
    /// Optional inputs map matching the workflow's `inputs:` schema.
    pub inputs: Option<Value>,
    /// Which configured GitHub account to use, when more than one is
    /// configured. Only needed when `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "github.workflows_dispatch",
        description: "Trigger a GitHub Actions workflow run via workflow_dispatch. Requires the workflow file to declare `on: workflow_dispatch:`.",
        input_schema: serde_json::to_value(schemars::schema_for!(WorkflowsDispatchArgs)).unwrap(),
        kind: ToolKind::Write,
    }]
}

pub async fn dispatch_workflows_dispatch(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: WorkflowsDispatchArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let r = RepoRef {
        platform: Platform::Github,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, extras) = reg.github_extras_for(&r, None, account.as_ref())?;
    extras
        .workflows_dispatch(
            &r,
            WorkflowDispatch {
                workflow: parsed.workflow,
                ref_: parsed.r#ref,
                inputs: parsed.inputs.unwrap_or(Value::Null),
            },
        )
        .await?;
    Ok("{}".to_string())
}
