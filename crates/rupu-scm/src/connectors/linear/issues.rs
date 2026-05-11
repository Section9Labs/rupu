use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

const DEFAULT_BASE_URL: &str = "https://api.linear.app/graphql";

pub struct LinearIssueConnector {
    http: reqwest::Client,
    token: String,
    base_url: String,
}

impl LinearIssueConnector {
    pub fn new(token: String, base_url: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .build()
                .expect("reqwest client build"),
            token,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
        }
    }

    async fn graphql<T>(&self, query: &str, variables: Value) -> Result<T, ScmError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("rupu/0"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.token)
                .map_err(|e| ScmError::Transient(anyhow::anyhow!("invalid token: {e}")))?,
        );

        let response = self
            .http
            .post(&self.base_url)
            .headers(headers)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|e| {
                ScmError::Network(anyhow::anyhow!(
                    "linear graphql POST {}: {e}",
                    self.base_url
                ))
            })?;

        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .text()
            .await
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("linear graphql body: {e}")))?;
        let envelope: LinearGraphqlEnvelope<T> = serde_json::from_str(&body)
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("linear graphql parse: {e}")))?;

        if let Some(err) = envelope
            .errors
            .as_ref()
            .and_then(|errors| classify_linear_graphql_error(errors))
        {
            return Err(err);
        }
        if !status.is_success() {
            return Err(classify_linear_http_error(status.as_u16(), &body, &headers));
        }
        envelope
            .data
            .ok_or_else(|| ScmError::Transient(anyhow::anyhow!("linear graphql missing data")))
    }

    async fn fetch_team_issues_page(
        &self,
        team_id: &str,
        after: Option<&str>,
    ) -> Result<LinearIssuePage, ScmError> {
        let query = r#"
            query TeamIssues($teamId: String!, $after: String) {
              team(id: $teamId) {
                id
                key
                issues(first: 100, after: $after, orderBy: updatedAt) {
                  nodes {
                    id
                    identifier
                    url
                    title
                    description
                    createdAt
                    updatedAt
                    creator {
                      name
                    }
                    team {
                      id
                      key
                      name
                    }
                    state {
                      id
                      name
                      type
                    }
                    labels(first: 50) {
                      nodes {
                        id
                        name
                        color
                      }
                    }
                  }
                  pageInfo {
                    hasNextPage
                    endCursor
                  }
                }
              }
            }
        "#;
        let data: TeamIssuesData = self
            .graphql(query, json!({ "teamId": team_id, "after": after }))
            .await?;
        let team = data.team.ok_or_else(|| ScmError::NotFound {
            what: format!("linear team `{team_id}`"),
        })?;
        Ok(team.issues)
    }

    async fn fetch_issue_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<LinearIssueNode, ScmError> {
        let query = r#"
            query Issue($id: String!) {
              issue(id: $id) {
                id
                identifier
                url
                title
                description
                createdAt
                updatedAt
                creator {
                  name
                }
                team {
                  id
                  key
                  name
                }
                state {
                  id
                  name
                  type
                }
                labels(first: 50) {
                  nodes {
                    id
                    name
                    color
                  }
                }
              }
            }
        "#;
        let data: IssueData = self.graphql(query, json!({ "id": identifier })).await?;
        data.issue.ok_or_else(|| ScmError::NotFound {
            what: format!("linear issue `{identifier}`"),
        })
    }

    async fn fetch_team_metadata(&self, team_id: &str) -> Result<LinearTeamMetadata, ScmError> {
        let query = r#"
            query TeamMetadata($teamId: String!) {
              team(id: $teamId) {
                id
                key
                states {
                  nodes {
                    id
                    name
                    type
                  }
                }
                labels(first: 100) {
                  nodes {
                    id
                    name
                    color
                  }
                }
              }
            }
        "#;
        let data: TeamMetadataData = self.graphql(query, json!({ "teamId": team_id })).await?;
        data.team.ok_or_else(|| ScmError::NotFound {
            what: format!("linear team `{team_id}`"),
        })
    }

    async fn resolve_identifier(&self, issue: &IssueRef) -> Result<String, ScmError> {
        let meta = self.fetch_team_metadata(&issue.project).await?;
        Ok(format!("{}-{}", meta.key, issue.number))
    }

    async fn resolve_issue_id(&self, issue: &IssueRef) -> Result<String, ScmError> {
        let identifier = self.resolve_identifier(issue).await?;
        let issue = self.fetch_issue_by_identifier(&identifier).await?;
        Ok(issue.id)
    }

    async fn resolve_label_ids(
        &self,
        team_id: &str,
        labels: &[String],
    ) -> Result<Vec<String>, ScmError> {
        if labels.is_empty() {
            return Ok(Vec::new());
        }
        let meta = self.fetch_team_metadata(team_id).await?;
        let mut resolved = Vec::with_capacity(labels.len());
        for wanted in labels {
            let Some(label) = meta.labels.nodes.iter().find(|label| label.name == *wanted) else {
                return Err(ScmError::BadRequest {
                    message: format!("linear label `{wanted}` not found in team `{team_id}`"),
                });
            };
            resolved.push(label.id.clone());
        }
        Ok(resolved)
    }
}

#[async_trait]
impl IssueConnector for LinearIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Linear
    }

    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let mut after: Option<String> = None;
        let mut out = Vec::new();
        let max = filter.limit.unwrap_or(50) as usize;

        loop {
            let page = self
                .fetch_team_issues_page(project, after.as_deref())
                .await?;
            if page.nodes.is_empty() {
                break;
            }
            for node in page.nodes {
                let issue = linear_issue_from_node(project.to_string(), node);
                if issue_matches_filter(&issue, &filter) {
                    out.push(issue);
                    if out.len() >= max {
                        return Ok(out);
                    }
                }
            }
            if !page.page_info.has_next_page {
                break;
            }
            after = page.page_info.end_cursor;
        }

        Ok(out)
    }

    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
        let identifier = self.resolve_identifier(i).await?;
        let node = self.fetch_issue_by_identifier(&identifier).await?;
        Ok(linear_issue_from_node(i.project.clone(), node))
    }

    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError> {
        let issue_id = self.resolve_issue_id(i).await?;
        let query = r#"
            mutation CommentCreate($issueId: String!, $body: String!) {
              commentCreate(input: { issueId: $issueId, body: $body }) {
                success
                comment {
                  id
                  body
                  createdAt
                  user {
                    name
                  }
                }
              }
            }
        "#;
        let data: CommentCreateData = self
            .graphql(query, json!({ "issueId": issue_id, "body": body }))
            .await?;
        let payload = data.comment_create.ok_or_else(|| ScmError::BadRequest {
            message: "linear commentCreate returned no payload".into(),
        })?;
        if !payload.success {
            return Err(ScmError::BadRequest {
                message: "linear commentCreate returned success=false".into(),
            });
        }
        let comment = payload.comment.ok_or_else(|| ScmError::BadRequest {
            message: "linear commentCreate returned no comment".into(),
        })?;
        Ok(Comment {
            id: comment.id,
            author: comment.user.map(|user| user.name).unwrap_or_default(),
            body: comment.body,
            created_at: comment.created_at,
        })
    }

    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError> {
        let label_ids = self.resolve_label_ids(project, &opts.labels).await?;
        let query = r#"
            mutation IssueCreate($teamId: String!, $title: String!, $description: String!, $labelIds: [String!]) {
              issueCreate(input: {
                teamId: $teamId,
                title: $title,
                description: $description,
                labelIds: $labelIds
              }) {
                success
                issue {
                  id
                  identifier
                  url
                  title
                  description
                  createdAt
                  updatedAt
                  creator {
                    name
                  }
                  team {
                    id
                    key
                    name
                  }
                  state {
                    id
                    name
                    type
                  }
                  labels(first: 50) {
                    nodes {
                      id
                      name
                      color
                    }
                  }
                }
              }
            }
        "#;
        let data: IssueCreateData = self
            .graphql(
                query,
                json!({
                    "teamId": project,
                    "title": opts.title,
                    "description": opts.body,
                    "labelIds": label_ids,
                }),
            )
            .await?;
        let payload = data.issue_create.ok_or_else(|| ScmError::BadRequest {
            message: "linear issueCreate returned no payload".into(),
        })?;
        if !payload.success {
            return Err(ScmError::BadRequest {
                message: "linear issueCreate returned success=false".into(),
            });
        }
        let issue = payload.issue.ok_or_else(|| ScmError::BadRequest {
            message: "linear issueCreate returned no issue".into(),
        })?;
        Ok(linear_issue_from_node(project.to_string(), issue))
    }

    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError> {
        let meta = self.fetch_team_metadata(&i.project).await?;
        let identifier = format!("{}-{}", meta.key, i.number);
        let state_id = pick_linear_state_id(&meta, &i.project, state)?;
        let query = r#"
            mutation IssueUpdate($id: String!, $stateId: String!) {
              issueUpdate(id: $id, input: { stateId: $stateId }) {
                success
                issue {
                  id
                }
              }
            }
        "#;
        let data: IssueUpdateData = self
            .graphql(query, json!({ "id": identifier, "stateId": state_id }))
            .await?;
        let payload = data.issue_update.ok_or_else(|| ScmError::BadRequest {
            message: "linear issueUpdate returned no payload".into(),
        })?;
        if !payload.success {
            return Err(ScmError::BadRequest {
                message: "linear issueUpdate returned success=false".into(),
            });
        }
        Ok(())
    }
}

fn issue_matches_filter(issue: &Issue, filter: &IssueFilter) -> bool {
    if let Some(state) = filter.state {
        if issue.state != state {
            return false;
        }
    }
    if let Some(author) = filter.author.as_deref() {
        if issue.author != author {
            return false;
        }
    }
    if !filter.labels.is_empty()
        && !filter
            .labels
            .iter()
            .all(|label| issue.labels.iter().any(|present| present == label))
    {
        return false;
    }
    true
}

fn linear_issue_from_node(project: String, node: LinearIssueNode) -> Issue {
    let mut label_colors = std::collections::BTreeMap::new();
    let labels = node
        .labels
        .nodes
        .into_iter()
        .map(|label| {
            if let Some(color) = label.color.as_deref() {
                label_colors.insert(
                    label.name.clone(),
                    color.trim_start_matches('#').to_string(),
                );
            }
            label.name
        })
        .collect();

    Issue {
        r: IssueRef {
            tracker: IssueTracker::Linear,
            project,
            number: parse_issue_number(&node.identifier).unwrap_or_default(),
        },
        title: node.title,
        body: node.description.unwrap_or_default(),
        state: normalize_linear_state(node.state.as_ref()),
        labels,
        label_colors,
        author: node.creator.map(|creator| creator.name).unwrap_or_default(),
        created_at: node.created_at,
        updated_at: node.updated_at,
    }
}

fn parse_issue_number(identifier: &str) -> Option<u64> {
    identifier.rsplit_once('-')?.1.parse().ok()
}

fn normalize_linear_state(state: Option<&LinearStateNode>) -> IssueState {
    match state.map(|state| state.kind.as_str()) {
        Some(kind) if is_terminal_state(kind) => IssueState::Closed,
        _ => IssueState::Open,
    }
}

fn is_terminal_state(kind: &str) -> bool {
    matches!(kind, "completed" | "canceled" | "cancelled")
}

fn pick_linear_state_id(
    meta: &LinearTeamMetadata,
    team_id: &str,
    desired: IssueState,
) -> Result<String, ScmError> {
    let pick = match desired {
        IssueState::Closed => meta
            .states
            .nodes
            .iter()
            .find(|state| is_terminal_state(&state.kind))
            .or_else(|| meta.states.nodes.first()),
        IssueState::Open => meta
            .states
            .nodes
            .iter()
            .find(|state| {
                matches!(
                    state.kind.as_str(),
                    "unstarted" | "triage" | "backlog" | "started"
                )
            })
            .or_else(|| {
                meta.states
                    .nodes
                    .iter()
                    .find(|state| !is_terminal_state(&state.kind))
            })
            .or_else(|| meta.states.nodes.first()),
    };
    pick.map(|state| state.id.clone())
        .ok_or_else(|| ScmError::BadRequest {
            message: format!("linear team `{team_id}` has no workflow states"),
        })
}

fn classify_linear_http_error(status: u16, body: &str, headers: &HeaderMap) -> ScmError {
    match status {
        401 => ScmError::Unauthorized {
            platform: "linear".to_string(),
            hint: "run: rupu auth login --provider linear --mode api-key".to_string(),
        },
        403 => ScmError::Forbidden {
            platform: "linear".to_string(),
            message: truncate_message(body),
        },
        404 => ScmError::NotFound {
            what: "linear resource".to_string(),
        },
        429 => ScmError::RateLimited {
            retry_after: headers
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(std::time::Duration::from_secs),
        },
        500..=599 => ScmError::Transient(anyhow::anyhow!(
            "linear {status}: {}",
            truncate_message(body)
        )),
        _ => ScmError::BadRequest {
            message: truncate_message(body),
        },
    }
}

fn classify_linear_graphql_error(errors: &[LinearGraphqlError]) -> Option<ScmError> {
    let first = errors.first()?;
    match first
        .extensions
        .as_ref()
        .and_then(|extensions| extensions.code.as_deref())
        .unwrap_or_default()
    {
        "RATELIMITED" => Some(ScmError::RateLimited { retry_after: None }),
        "AUTHENTICATION_ERROR" => Some(ScmError::Unauthorized {
            platform: "linear".to_string(),
            hint: "run: rupu auth login --provider linear --mode api-key".to_string(),
        }),
        _ => Some(ScmError::BadRequest {
            message: first.message.clone(),
        }),
    }
}

fn truncate_message(body: &str) -> String {
    if body.len() <= 200 {
        body.to_string()
    } else {
        format!("{}…", &body[..200])
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LinearGraphqlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<LinearGraphqlError>>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearGraphqlError {
    message: String,
    #[serde(default)]
    extensions: Option<LinearGraphqlErrorExtensions>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearGraphqlErrorExtensions {
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TeamIssuesData {
    team: Option<LinearTeamIssueList>,
}

#[derive(Debug, Clone, Deserialize)]
struct TeamMetadataData {
    team: Option<LinearTeamMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
struct IssueData {
    issue: Option<LinearIssueNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct IssueCreateData {
    #[serde(rename = "issueCreate")]
    issue_create: Option<LinearIssuePayload>,
}

#[derive(Debug, Clone, Deserialize)]
struct IssueUpdateData {
    #[serde(rename = "issueUpdate")]
    issue_update: Option<LinearMutationSuccess>,
}

#[derive(Debug, Clone, Deserialize)]
struct CommentCreateData {
    #[serde(rename = "commentCreate")]
    comment_create: Option<LinearCommentPayload>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearTeamIssueList {
    issues: LinearIssuePage,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearTeamMetadata {
    key: String,
    states: LinearStateConnection,
    labels: LinearLabelConnection,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearIssuePage {
    nodes: Vec<LinearIssueNode>,
    #[serde(rename = "pageInfo")]
    page_info: LinearPageInfo,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearIssueNode {
    id: String,
    identifier: String,
    title: String,
    description: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    updated_at: DateTime<Utc>,
    creator: Option<LinearUserNode>,
    state: Option<LinearStateNode>,
    labels: LinearLabelConnection,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearUserNode {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearStateNode {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearStateConnection {
    nodes: Vec<LinearStateNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearLabelConnection {
    nodes: Vec<LinearLabelNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearLabelNode {
    id: String,
    name: String,
    color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearIssuePayload {
    success: bool,
    issue: Option<LinearIssueNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearMutationSuccess {
    success: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearCommentPayload {
    success: bool,
    comment: Option<LinearCommentNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearCommentNode {
    id: String,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    user: Option<LinearUserNode>,
}
