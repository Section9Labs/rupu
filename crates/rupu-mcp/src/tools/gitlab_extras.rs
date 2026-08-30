//! gitlab.pipeline_trigger tool.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{AccountId, PipelineTrigger, Platform, Registry, RepoRef};

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

pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "gitlab.pipeline_trigger",
        description: "Trigger a GitLab CI pipeline against a branch/tag with optional variables.",
        input_schema: serde_json::to_value(schemars::schema_for!(PipelineTriggerArgs)).unwrap(),
        kind: ToolKind::Write,
    }]
}

pub async fn dispatch_pipeline_trigger(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: PipelineTriggerArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
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
