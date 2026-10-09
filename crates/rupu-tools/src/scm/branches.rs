//! `scm.branches.{list, create}`.

use crate::connector::{self, json, parse, resolve_platform, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountId, Registry, RepoRef};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
pub struct ListBranchesArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateBranchArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub from_sha: String,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub static LIST: ToolDescriptor = ToolDescriptor {
    name: "scm.branches.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "List branches on a repository. Returns name, sha, and protected flag for each.",
    input_schema: schema::<ListBranchesArgs>,
};

pub static CREATE: ToolDescriptor = ToolDescriptor {
    name: "scm.branches.create",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Create a new branch from a given SHA. Returns the new branch.",
    input_schema: schema::<CreateBranchArgs>,
};

pub struct BranchesListTool;

#[async_trait]
impl Tool for BranchesListTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &LIST
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| list(input, reg)).await
    }
}

pub struct BranchesCreateTool;

#[async_trait]
impl Tool for BranchesCreateTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CREATE
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| create(input, reg)).await
    }
}

async fn list(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: ListBranchesArgs = parse(args)?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    Ok(json(&conn.list_branches(&r).await?))
}

async fn create(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: CreateBranchArgs = parse(args)?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    Ok(json(
        &conn
            .create_branch(&r, &parsed.name, &parsed.from_sha)
            .await?,
    ))
}
