//! `issues.{list, get, comments, comment, create, update_state}`.

use crate::connector::{self, json, parse, resolve_tracker, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountId, CreateIssue, IssueFilter, IssueRef, IssueState, IssueTracker, Registry};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
pub struct ListIssuesArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub state: Option<String>,
    pub labels: Option<Vec<String>>,
    pub author: Option<String>,
    pub limit: Option<u32>,
    /// Which configured account to use, when more than one is configured
    /// for this tracker (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetIssueArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    /// Which configured account to use, when more than one is configured
    /// for this tracker (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CommentIssueArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    pub body: String,
    /// Which configured account to use, when more than one is configured
    /// for this tracker (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListCommentsArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    /// Maximum comments to return. The thread is walked and returned oldest-first, so a `limit` below the thread's true length drops the NEWEST comments, not the oldest. Omitting `limit` returns at most the oldest 100 comments. Hard ceiling: 5000 comments, regardless of `limit`.
    pub limit: Option<u32>,
    /// Which configured account to use, when more than one is configured
    /// for this tracker (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateIssueArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub title: String,
    pub body: String,
    pub labels: Option<Vec<String>>,
    /// Which configured account to use, when more than one is configured
    /// for this tracker (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateIssueStateArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    /// `open` | `closed`.
    pub state: String,
    /// Which configured account to use, when more than one is configured
    /// for this tracker (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub static LIST: ToolDescriptor = ToolDescriptor {
    name: "issues.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "List issues on a tracker. Filters: state (open/closed), labels, author, limit.",
    input_schema: schema::<ListIssuesArgs>,
};

pub static GET: ToolDescriptor = ToolDescriptor {
    name: "issues.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description:
        "Fetch a single issue by number. Returns title, body, state, labels, author, timestamps.",
    input_schema: schema::<GetIssueArgs>,
};

pub static COMMENTS: ToolDescriptor = ToolDescriptor {
    name: "issues.comments",
    aliases: &[],
    // Read: a readonly run must still be able to read a comment thread,
    // which is the whole point of the operator control channel.
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "Read an issue's comment thread. Returns id, author, body, created_at, author_association per comment, oldest-first. author_association is GitHub's role for the commenter on this repo (e.g. OWNER, COLLABORATOR, CONTRIBUTOR, NONE) — use it to tell an authorized operator's comment apart from an arbitrary commenter's; it is null on trackers that don't expose an equivalent (GitLab, Linear, Jira). A `limit` truncates the NEWEST comments off the end, not the oldest — omitting `limit` returns only the oldest 100; hard ceiling 5000.",
    input_schema: schema::<ListCommentsArgs>,
};

pub static COMMENT: ToolDescriptor = ToolDescriptor {
    name: "issues.comment",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Post a comment on an issue.",
    input_schema: schema::<CommentIssueArgs>,
};

pub static CREATE: ToolDescriptor = ToolDescriptor {
    name: "issues.create",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Open a new issue with title, body, and optional labels.",
    input_schema: schema::<CreateIssueArgs>,
};

pub static UPDATE_STATE: ToolDescriptor = ToolDescriptor {
    name: "issues.update_state",
    aliases: &[],
    effect: Effect::External,
    needs: &[Service::Scm],
    uses: &[],
    description: "Transition an issue's state to `open` or `closed`.",
    input_schema: schema::<UpdateIssueStateArgs>,
};

pub struct IssuesListTool;

#[async_trait]
impl Tool for IssuesListTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &LIST
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| list(input, reg)).await
    }
}

pub struct IssuesGetTool;

#[async_trait]
impl Tool for IssuesGetTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &GET
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| get(input, reg)).await
    }
}

pub struct IssuesCommentsTool;

#[async_trait]
impl Tool for IssuesCommentsTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &COMMENTS
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| comments(input, reg)).await
    }
}

pub struct IssuesCommentTool;

#[async_trait]
impl Tool for IssuesCommentTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &COMMENT
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| comment(input, reg)).await
    }
}

pub struct IssuesCreateTool;

#[async_trait]
impl Tool for IssuesCreateTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &CREATE
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| create(input, reg)).await
    }
}

pub struct IssuesUpdateStateTool;

#[async_trait]
impl Tool for IssuesUpdateStateTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &UPDATE_STATE
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| update_state(input, reg)).await
    }
}

/// The issue `number` of `project` on `tracker`, the connector that
/// serves it, and its repo when the tracker is repo-backed. The `RepoRef` is
/// derived before `project` moves into the `IssueRef`: for a repo-backed
/// tracker it is what lets the owner rule tier fire.
fn issue_target(
    reg: &Registry,
    tracker: Option<&str>,
    project: String,
    number: u64,
    account: Option<&str>,
) -> Result<(IssueRef, std::sync::Arc<dyn rupu_scm::IssueConnector>), ConnectorError> {
    let tracker = resolve_tracker(tracker, reg)?;
    let repo = project_repo(tracker, &project);
    let r = IssueRef {
        tracker,
        project,
        number,
    };
    let account = account.map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    Ok((r, conn))
}

async fn list(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: ListIssuesArgs = parse(args)?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let state = match parsed.state.as_deref() {
        Some("open") => Some(IssueState::Open),
        Some("closed") => Some(IssueState::Closed),
        Some(other) => {
            return Err(ConnectorError::InvalidArgs(format!(
                "unknown state: {other}"
            )))
        }
        None => None,
    };
    let filter = IssueFilter {
        state,
        labels: parsed.labels.unwrap_or_default(),
        author: parsed.author,
        limit: parsed.limit,
    };
    let repo = project_repo(tracker, &parsed.project);
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    Ok(json(&conn.list_issues(&parsed.project, filter).await?))
}

async fn get(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: GetIssueArgs = parse(args)?;
    let (r, conn) = issue_target(
        reg,
        parsed.tracker.as_deref(),
        parsed.project,
        parsed.number,
        parsed.account.as_deref(),
    )?;
    Ok(json(&conn.get_issue(&r).await?))
}

async fn comments(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: ListCommentsArgs = parse(args)?;
    let (r, conn) = issue_target(
        reg,
        parsed.tracker.as_deref(),
        parsed.project,
        parsed.number,
        parsed.account.as_deref(),
    )?;
    Ok(json(&conn.list_comments(&r, parsed.limit).await?))
}

async fn comment(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: CommentIssueArgs = parse(args)?;
    let (r, conn) = issue_target(
        reg,
        parsed.tracker.as_deref(),
        parsed.project,
        parsed.number,
        parsed.account.as_deref(),
    )?;
    Ok(json(&conn.comment_issue(&r, &parsed.body).await?))
}

async fn create(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: CreateIssueArgs = parse(args)?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let opts = CreateIssue {
        title: parsed.title,
        body: parsed.body,
        labels: parsed.labels.unwrap_or_default(),
    };
    let repo = project_repo(tracker, &parsed.project);
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    Ok(json(&conn.create_issue(&parsed.project, opts).await?))
}

async fn update_state(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: UpdateIssueStateArgs = parse(args)?;
    let new_state = match parsed.state.as_str() {
        "open" => IssueState::Open,
        "closed" => IssueState::Closed,
        other => {
            return Err(ConnectorError::InvalidArgs(format!(
                "unknown state: {other}"
            )))
        }
    };
    let (r, conn) = issue_target(
        reg,
        parsed.tracker.as_deref(),
        parsed.project,
        parsed.number,
        parsed.account.as_deref(),
    )?;
    conn.update_issue_state(&r, new_state).await?;
    Ok("{}".to_string())
}

/// Recover a `RepoRef` from an issue-tracker `project` string, when the
/// tracker is repo-backed (GitHub/GitLab use `"owner/repo"` project
/// identifiers) and the string actually parses that way. Linear/Jira
/// project keys (`"ENG"`) aren't owner/repo pairs: those trackers only ever
/// resolve via the explicit or sole-account tiers.
fn project_repo(tracker: IssueTracker, project: &str) -> Option<rupu_scm::RepoRef> {
    rupu_scm::tracker_project_repo(tracker, project)
}
