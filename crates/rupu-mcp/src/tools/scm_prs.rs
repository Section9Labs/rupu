//! scm.prs.{list, get, diff, comment, create} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{CreatePr, PrFilter, PrRef, PrState, Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct ListPrsArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    /// `open` | `closed` | `merged`. Omit to list all states.
    pub state: Option<String>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetPrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiffPrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct CommentPrArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    pub number: u32,
    pub body: String,
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
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "scm.prs.list",
            description: "List pull/merge requests on a repository. Optional filters: state (open/closed/merged), author, limit.",
            input_schema: serde_json::to_value(schemars::schema_for!(ListPrsArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.prs.get",
            description: "Fetch a single pull/merge request by number. Returns title, body, state, head/base branches, author, timestamps.",
            input_schema: serde_json::to_value(schemars::schema_for!(GetPrArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.prs.diff",
            description: "Fetch the unified-diff patch for a pull/merge request. Returns patch text + per-file change counts.",
            input_schema: serde_json::to_value(schemars::schema_for!(DiffPrArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.prs.comment",
            description: "Post a top-level comment on a pull/merge request.",
            input_schema: serde_json::to_value(schemars::schema_for!(CommentPrArgs)).unwrap(),
            kind: ToolKind::Write,
        },
        ToolSpec {
            name: "scm.prs.create",
            description: "Open a pull/merge request from `head` branch into `base`. Set draft=true to open as a draft.",
            input_schema: serde_json::to_value(schemars::schema_for!(CreatePrArgs)).unwrap(),
            kind: ToolKind::Write,
        },
    ]
}

pub async fn dispatch_list(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListPrsArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let state = match parsed.state.as_deref() {
        Some("open") => Some(PrState::Open),
        Some("closed") => Some(PrState::Closed),
        Some("merged") => Some(PrState::Merged),
        Some(other) => return Err(McpError::InvalidArgs(format!("unknown state: {other}"))),
        None => None,
    };
    let filter = PrFilter {
        state,
        author: parsed.author,
        limit: parsed.limit,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.list_prs(&r, filter).await?).unwrap())
}

pub async fn dispatch_get(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: GetPrArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let p = PrRef {
        repo: RepoRef {
            platform,
            owner: parsed.owner,
            repo: parsed.repo,
        },
        number: parsed.number,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.get_pr(&p).await?).unwrap())
}

pub async fn dispatch_diff(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: DiffPrArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let p = PrRef {
        repo: RepoRef {
            platform,
            owner: parsed.owner,
            repo: parsed.repo,
        },
        number: parsed.number,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.diff_pr(&p).await?).unwrap())
}

pub async fn dispatch_comment(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: CommentPrArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
    let p = PrRef {
        repo: RepoRef {
            platform,
            owner: parsed.owner,
            repo: parsed.repo,
        },
        number: parsed.number,
    };
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.comment_pr(&p, &parsed.body).await?).unwrap())
}

pub async fn dispatch_create(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: CreatePrArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = super::scm_repos::resolve_platform(parsed.platform.as_deref(), reg)?;
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
    let conn = reg
        .repo(platform)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {platform}")))?;
    Ok(serde_json::to_string(&conn.create_pr(&r, opts).await?).unwrap())
}
