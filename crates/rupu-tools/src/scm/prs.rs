//! `scm.prs.{list, get, diff, comment, create}`.

use crate::connector::{self, json, parse, resolve_platform, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountId, CreatePr, PrFilter, PrRef, PrState, Registry, RepoRef};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
pub struct ListPrsArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    /// `open` | `closed` | `merged`. Omit to list all states.
    pub state: Option<String>,
    pub author: Option<String>,
    pub limit: Option<u32>,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetPrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiffPrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CommentPrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
    pub body: String,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreatePrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: Option<bool>,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub static LIST: ToolDescriptor = ToolDescriptor {
    name: "scm.prs.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "List pull/merge requests on a repository. Optional filters: state (open/closed/merged), author, limit.",
    input_schema: schema::<ListPrsArgs>,
};

pub static GET: ToolDescriptor = ToolDescriptor {
    name: "scm.prs.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "Fetch a single pull/merge request by number. Returns title, body, state, head/base branches, author, timestamps.",
    input_schema: schema::<GetPrArgs>,
};

pub static DIFF: ToolDescriptor = ToolDescriptor {
    name: "scm.prs.diff",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "Fetch the unified-diff patch for a pull/merge request. Returns patch text + per-file change counts.",
    input_schema: schema::<DiffPrArgs>,
};

pub static COMMENT: ToolDescriptor = ToolDescriptor {
    name: "scm.prs.comment",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Post a top-level comment on a pull/merge request.",
    input_schema: schema::<CommentPrArgs>,
};

pub static CREATE: ToolDescriptor = ToolDescriptor {
    name: "scm.prs.create",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Open a pull/merge request from `head` branch into `base`. Set draft=true to open as a draft.",
    input_schema: schema::<CreatePrArgs>,
};

pub struct PrsListTool;

#[async_trait]
impl Tool for PrsListTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &LIST
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| list(input, reg)).await
    }
}

pub struct PrsGetTool;

#[async_trait]
impl Tool for PrsGetTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &GET
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| get(input, reg)).await
    }
}

pub struct PrsDiffTool;

#[async_trait]
impl Tool for PrsDiffTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &DIFF
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| diff(input, reg)).await
    }
}

pub struct PrsCommentTool;

#[async_trait]
impl Tool for PrsCommentTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &COMMENT
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| comment(input, reg)).await
    }
}

pub struct PrsCreateTool;

#[async_trait]
impl Tool for PrsCreateTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CREATE
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| create(input, reg)).await
    }
}

fn pr_ref(
    platform: Option<&str>,
    owner: String,
    repo: String,
    number: u32,
    reg: &Registry,
) -> Result<PrRef, ConnectorError> {
    Ok(PrRef {
        repo: RepoRef {
            platform: resolve_platform(platform, reg)?,
            owner,
            repo,
        },
        number,
    })
}

async fn list(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: ListPrsArgs = parse(args)?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let state = match parsed.state.as_deref() {
        Some("open") => Some(PrState::Open),
        Some("closed") => Some(PrState::Closed),
        Some("merged") => Some(PrState::Merged),
        Some(other) => {
            return Err(ConnectorError::InvalidArgs(format!(
                "unknown state: {other}"
            )))
        }
        None => None,
    };
    let filter = PrFilter {
        state,
        author: parsed.author,
        limit: parsed.limit,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    Ok(json(&conn.list_prs(&r, filter).await?))
}

async fn get(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: GetPrArgs = parse(args)?;
    let p = pr_ref(
        parsed.platform.as_deref(),
        parsed.owner,
        parsed.repo,
        parsed.number,
        reg,
    )?;
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&p.repo, None, account.as_ref())?;
    Ok(json(&conn.get_pr(&p).await?))
}

async fn diff(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: DiffPrArgs = parse(args)?;
    let p = pr_ref(
        parsed.platform.as_deref(),
        parsed.owner,
        parsed.repo,
        parsed.number,
        reg,
    )?;
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&p.repo, None, account.as_ref())?;
    Ok(json(&conn.diff_pr(&p).await?))
}

async fn comment(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: CommentPrArgs = parse(args)?;
    let p = pr_ref(
        parsed.platform.as_deref(),
        parsed.owner,
        parsed.repo,
        parsed.number,
        reg,
    )?;
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&p.repo, None, account.as_ref())?;
    Ok(json(&conn.comment_pr(&p, &parsed.body).await?))
}

async fn create(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: CreatePrArgs = parse(args)?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let opts = CreatePr {
        title: parsed.title,
        body: parsed.body,
        head: parsed.head,
        base: parsed.base,
        draft: parsed.draft.unwrap_or(false),
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    Ok(json(&conn.create_pr(&r, opts).await?))
}
