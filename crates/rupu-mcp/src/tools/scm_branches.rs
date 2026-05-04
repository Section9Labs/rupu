//! scm.branches.{list, create} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct ListBranchesArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateBranchArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub from_sha: String,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "scm.branches.list",
            description:
                "List branches on a repository. Returns name, sha, and protected flag for each.",
            input_schema: serde_json::to_value(schemars::schema_for!(ListBranchesArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.branches.create",
            description: "Create a new branch from a given SHA. Returns the new branch.",
            input_schema: serde_json::to_value(schemars::schema_for!(CreateBranchArgs)).unwrap(),
            kind: ToolKind::Write,
        },
    ]
}

pub async fn dispatch_list(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListBranchesArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.list_branches(&r).await?).unwrap())
}

pub async fn dispatch_create(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: CreateBranchArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(
        &conn
            .create_branch(&r, &parsed.name, &parsed.from_sha)
            .await?,
    )
    .unwrap())
}
