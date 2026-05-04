//! scm.repos.{list, get} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{Platform, Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct ListReposArgs {
    /// Platform to query (`github` | `gitlab`). Omit to use [scm.default].
    pub platform: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetRepoArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "scm.repos.list",
            description: "List repositories the authenticated user can access on the given platform. Omit `platform` to use [scm.default].",
            input_schema: serde_json::to_value(schemars::schema_for!(ListReposArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.repos.get",
            description: "Fetch a single repository (default branch, clone URLs, visibility, description).",
            input_schema: serde_json::to_value(schemars::schema_for!(GetRepoArgs)).unwrap(),
            kind: ToolKind::Read,
        },
    ]
}

pub async fn dispatch_list(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListReposArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    let repos = conn.list_repos().await?;
    Ok(serde_json::to_string(&repos).unwrap())
}

pub async fn dispatch_get(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: GetRepoArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.get_repo(&r).await?).unwrap())
}

/// Resolve an optional platform argument. If `arg` is `Some`, parse it;
/// otherwise fall back to the first registered platform in the registry.
pub(crate) fn resolve_platform(arg: Option<&str>, reg: &Registry) -> Result<Platform, McpError> {
    match arg {
        Some(s) => s.parse::<Platform>().map_err(McpError::InvalidArgs),
        None => reg.default_platform().ok_or_else(|| {
            McpError::InvalidArgs("no platform arg and no [scm.default] configured".into())
        }),
    }
}
