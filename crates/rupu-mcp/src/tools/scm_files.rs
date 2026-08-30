//! scm.files.read tool.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{AccountId, Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub path: String,
    /// Optional ref (branch / tag / sha). Defaults to repo's default branch.
    pub r#ref: Option<String>,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "scm.files.read",
        description: "Read a single file from a repository at an optional ref. Returns path, ref, content, and encoding.",
        input_schema: serde_json::to_value(schemars::schema_for!(ReadFileArgs)).unwrap(),
        kind: ToolKind::Read,
    }]
}

pub async fn dispatch_read(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ReadFileArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    let content = conn
        .read_file(&r, &parsed.path, parsed.r#ref.as_deref())
        .await?;
    Ok(serde_json::to_string(&content).unwrap())
}
