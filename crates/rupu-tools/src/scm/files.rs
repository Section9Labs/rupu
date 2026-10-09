//! `scm.files.read`.

use crate::connector::{self, json, parse, resolve_platform, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountId, Registry, RepoRef};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

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

pub static READ: ToolDescriptor = ToolDescriptor {
    name: "scm.files.read",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "Read a single file from a repository at an optional ref. Returns path, ref, content, and encoding.",
    input_schema: schema::<ReadFileArgs>,
};

pub struct FilesReadTool;

#[async_trait]
impl Tool for FilesReadTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &READ
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| read(input, reg)).await
    }
}

async fn read(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: ReadFileArgs = parse(args)?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
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
    Ok(json(&content))
}
