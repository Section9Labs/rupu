//! issues.{list, get, comment, create, update_state} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{CreateIssue, IssueFilter, IssueRef, IssueState, IssueTracker, Registry};

#[derive(Deserialize, JsonSchema)]
pub struct ListIssuesArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub state: Option<String>,
    pub labels: Option<Vec<String>>,
    pub author: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetIssueArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct CommentIssueArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    pub body: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateIssueArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub title: String,
    pub body: String,
    pub labels: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct UpdateIssueStateArgs {
    pub tracker: Option<String>,
    pub project: String,
    pub number: u64,
    /// `open` | `closed`.
    pub state: String,
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
    let conn = reg
        .issues(tracker)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {tracker}")))?;
    Ok(serde_json::to_string(&conn.list_issues(&parsed.project, filter).await?).unwrap())
}

pub async fn dispatch_get(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: GetIssueArgs =
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
    Ok(serde_json::to_string(&conn.get_issue(&r).await?).unwrap())
}

pub async fn dispatch_comment(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: CommentIssueArgs =
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
    let conn = reg
        .issues(tracker)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {tracker}")))?;
    Ok(serde_json::to_string(&conn.create_issue(&parsed.project, opts).await?).unwrap())
}

pub async fn dispatch_update_state(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: UpdateIssueStateArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let tracker = resolve_tracker(parsed.tracker.as_deref(), reg)?;
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
    let conn = reg
        .issues(tracker)
        .ok_or_else(|| McpError::NotWiredInV0(format!("no connector for {tracker}")))?;
    conn.update_issue_state(&r, new_state).await?;
    Ok("{}".to_string())
}

fn resolve_tracker(arg: Option<&str>, reg: &Registry) -> Result<IssueTracker, McpError> {
    match arg {
        Some(s) => s.parse::<IssueTracker>().map_err(McpError::InvalidArgs),
        None => reg.default_tracker().ok_or_else(|| {
            McpError::InvalidArgs("no tracker arg and no [issues.default] configured".into())
        }),
    }
}
