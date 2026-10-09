//! `gitlab.pipeline_trigger`.

use crate::connector::{self, parse, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountId, PipelineTrigger, Platform, Registry, RepoRef};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize, JsonSchema)]
pub struct PipelineTriggerArgs {
    pub owner: String,
    pub repo: String,
    /// Branch or tag to run the pipeline against.
    pub r#ref: String,
    /// Map of pipeline variables (key → value).
    pub variables: Option<BTreeMap<String, String>>,
    /// Which configured GitLab account to use, when more than one is
    /// configured. Only needed when `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub static PIPELINE_TRIGGER: ToolDescriptor = ToolDescriptor {
    name: "gitlab.pipeline_trigger",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Trigger a GitLab CI pipeline against a branch/tag with optional variables.",
    input_schema: schema::<PipelineTriggerArgs>,
};

pub struct PipelineTriggerTool;

#[async_trait]
impl Tool for PipelineTriggerTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &PIPELINE_TRIGGER
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| pipeline_trigger(input, reg)).await
    }
}

async fn pipeline_trigger(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: PipelineTriggerArgs = parse(args)?;
    let r = RepoRef {
        platform: Platform::Gitlab,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, extras) = reg.gitlab_extras_for(&r, None, account.as_ref())?;
    extras
        .pipeline_trigger(
            &r,
            PipelineTrigger {
                ref_: parsed.r#ref,
                variables: parsed.variables.unwrap_or_default(),
            },
        )
        .await?;
    Ok("{}".to_string())
}
