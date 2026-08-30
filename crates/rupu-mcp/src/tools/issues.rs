//! issues.{list, get, comments, comment, create, update_state} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{AccountId, CreateIssue, IssueFilter, IssueRef, IssueState, IssueTracker, Registry};

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

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "issues.list",
            description: "List issues on a tracker. Filters: state (open/closed), labels, author, limit.",
            input_schema: serde_json::to_value(schemars::schema_for!(ListIssuesArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "issues.get",
            description: "Fetch a single issue by number. Returns title, body, state, labels, author, timestamps.",
            input_schema: serde_json::to_value(schemars::schema_for!(GetIssueArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "issues.comments",
            description: "Read an issue's comment thread. Returns id, author, body, created_at, author_association per comment, oldest-first. author_association is GitHub's role for the commenter on this repo (e.g. OWNER, COLLABORATOR, CONTRIBUTOR, NONE) — use it to tell an authorized operator's comment apart from an arbitrary commenter's; it is null on trackers that don't expose an equivalent (GitLab, Linear, Jira). A `limit` truncates the NEWEST comments off the end, not the oldest — omitting `limit` returns only the oldest 100; hard ceiling 5000.",
            input_schema: serde_json::to_value(schemars::schema_for!(ListCommentsArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "issues.comment",
            description: "Post a comment on an issue.",
            input_schema: serde_json::to_value(schemars::schema_for!(CommentIssueArgs)).unwrap(),
            kind: ToolKind::Write,
        },
        ToolSpec {
            name: "issues.create",
            description: "Open a new issue with title, body, and optional labels.",
            input_schema: serde_json::to_value(schemars::schema_for!(CreateIssueArgs)).unwrap(),
            kind: ToolKind::Write,
        },
        ToolSpec {
            name: "issues.update_state",
            description: "Transition an issue's state to `open` or `closed`.",
            input_schema: serde_json::to_value(schemars::schema_for!(UpdateIssueStateArgs)).unwrap(),
            kind: ToolKind::Write,
        },
    ]
}

pub async fn dispatch_list(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListIssuesArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let state = match parsed.state.as_deref() {
        Some("open") => Some(IssueState::Open),
        Some("closed") => Some(IssueState::Closed),
        Some(other) => return Err(McpError::InvalidArgs(format!("unknown state: {other}"))),
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
    Ok(serde_json::to_string(&conn.list_issues(&parsed.project, filter).await?).unwrap())
}

pub async fn dispatch_get(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: GetIssueArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let repo = project_repo(tracker, &parsed.project);
    let r = IssueRef {
        tracker,
        project: parsed.project,
        number: parsed.number,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    Ok(serde_json::to_string(&conn.get_issue(&r).await?).unwrap())
}

pub async fn dispatch_comments(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListCommentsArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let r = IssueRef {
        tracker,
        project: parsed.project,
        number: parsed.number,
    };
    let conn = reg
        .issues(tracker)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {tracker}")))?;
    Ok(serde_json::to_string(&conn.list_comments(&r, parsed.limit).await?).unwrap())
}

pub async fn dispatch_comment(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: CommentIssueArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let repo = project_repo(tracker, &parsed.project);
    let r = IssueRef {
        tracker,
        project: parsed.project,
        number: parsed.number,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    Ok(serde_json::to_string(&conn.comment_issue(&r, &parsed.body).await?).unwrap())
}

pub async fn dispatch_create(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: CreateIssueArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let opts = CreateIssue {
        title: parsed.title,
        body: parsed.body,
        labels: parsed.labels.unwrap_or_default(),
    };
    let repo = project_repo(tracker, &parsed.project);
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    Ok(serde_json::to_string(&conn.create_issue(&parsed.project, opts).await?).unwrap())
}

pub async fn dispatch_update_state(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: UpdateIssueStateArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
    let repo = project_repo(tracker, &parsed.project);
    let r = IssueRef {
        tracker,
        project: parsed.project,
        number: parsed.number,
    };
    let new_state = match parsed.state.as_str() {
        "open" => IssueState::Open,
        "closed" => IssueState::Closed,
        other => return Err(McpError::InvalidArgs(format!("unknown state: {other}"))),
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.issues_for(tracker, repo.as_ref(), None, account.as_ref())?;
    conn.update_issue_state(&r, new_state).await?;
    Ok("{}".to_string())
}

/// Recover a `RepoRef` from an issue-tracker `project` string, when the
/// tracker is repo-backed (GitHub/GitLab use `"owner/repo"` project
/// identifiers) and the string actually parses that way. Linear/Jira
/// project keys (`"ENG"`) aren't owner/repo pairs — `issues_for` already
/// documents that those trackers only ever resolve via the explicit or
/// sole-account tiers, and this returning `None` for them is what makes
/// that true rather than a false owner/path match on a malformed value.
/// Mirrors `rupu-cli`'s `cmd/issues.rs::issue_ref_repo`.
fn project_repo(tracker: IssueTracker, project: &str) -> Option<rupu_scm::RepoRef> {
    let platform = match tracker {
        IssueTracker::Github => rupu_scm::Platform::Github,
        IssueTracker::Gitlab => rupu_scm::Platform::Gitlab,
        IssueTracker::Linear | IssueTracker::Jira => return None,
    };
    let (owner, repo) = project.split_once('/')?;
    Some(rupu_scm::RepoRef {
        platform,
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

fn resolve_tracker(arg: Option<&str>, reg: &Registry) -> Result<IssueTracker, McpError> {
    match arg {
        Some(s) => s.parse::<IssueTracker>().map_err(McpError::InvalidArgs),
        None => reg.default_tracker().ok_or_else(|| {
            McpError::InvalidArgs("no tracker arg and no [issues.default] configured".into())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_tool_is_in_catalog_and_is_read_kind() {
        let s = specs();
        let spec = s
            .iter()
            .find(|s| s.name == "issues.comments")
            .expect("issues.comments must be in the issues tool catalog");
        // Read-kind matters: a readonly permission mode must still be
        // able to read a comment thread, which is the whole point of the
        // operator control channel.
        assert_eq!(spec.kind, ToolKind::Read);
        let schema = &spec.input_schema;
        let props = &schema["properties"];
        assert!(props.get("project").is_some(), "project is required input");
        assert!(props.get("number").is_some(), "number is required input");
        assert!(props.get("limit").is_some(), "limit is an accepted input");
    }
}
