//! `github.workflows_dispatch`.

use crate::connector::{self, parse, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountId, Platform, Registry, RepoRef, WorkflowDispatch};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

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

pub static WORKFLOWS_DISPATCH: ToolDescriptor = ToolDescriptor {
    name: "github.workflows_dispatch",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Trigger a GitHub Actions workflow run via workflow_dispatch. Requires the workflow file to declare `on: workflow_dispatch:`.",
    input_schema: schema::<WorkflowsDispatchArgs>,
};

pub struct WorkflowsDispatchTool;

#[async_trait]
impl Tool for WorkflowsDispatchTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &WORKFLOWS_DISPATCH
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| workflows_dispatch(input, reg)).await
    }
}

async fn workflows_dispatch(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: WorkflowsDispatchArgs = parse(args)?;
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
