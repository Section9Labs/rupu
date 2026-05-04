//! gitlab.pipeline_trigger tool.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{PipelineTrigger, Platform, Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct PipelineTriggerArgs {
    pub owner: String,
    pub repo: String,
    /// Branch or tag to run the pipeline against.
    pub r#ref: String,
    /// Map of pipeline variables (key → value).
    pub variables: Option<BTreeMap<String, String>>,
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
    let extras = reg.gitlab_extras().ok_or_else(|| {
        McpError::NotWiredInV0("gitlab extras require a gitlab credential".into())
    })?;
    let r = RepoRef {
        platform: Platform::Gitlab,
        owner: parsed.owner,
        repo: parsed.repo,
    };
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
