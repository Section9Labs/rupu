use std::collections::BTreeMap;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as Base64;
use base64::Engine;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

const DEFAULT_SITE_SCHEME: &str = "https";

pub struct JiraIssueConnector {
    http: reqwest::Client,
    auth: JiraAuth,
    base_url: Option<String>,
}

impl JiraIssueConnector {
    pub fn new(
        creds: rupu_providers::auth::AuthCredentials,
        base_url: Option<String>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .build()
                .expect("reqwest client build"),
            auth: JiraAuth::from_credentials(creds)?,
            base_url: base_url.map(|url| normalize_site_base_url(&url)),
        })
    }

    fn resolve_target(&self, project: &str) -> Result<JiraProjectTarget, ScmError> {
        if let Some((site, project_key)) = project.rsplit_once('/') {
            if site.is_empty() || project_key.is_empty() {
                return Err(ScmError::BadRequest {
                    message: format!(
                        "invalid jira project `{project}`; expected <site>/<project> or <project>"
                    ),
                });
            }
            let base_url = normalize_site_base_url(site);
            let site_key = site_key_from_base_url(&base_url)?;
            return Ok(JiraProjectTarget {
                base_url,
                site_key: site_key.clone(),
                project_key: project_key.to_string(),
                project_ref: format!("{site_key}/{project_key}"),
            });
        }

        let Some(base_url) = self.base_url.clone() else {
            return Err(ScmError::BadRequest {
                message: format!(
                    "jira project `{project}` requires <site>/<project> unless [scm.jira].base_url is configured"
                ),
            });
        };
        let site_key = site_key_from_base_url(&base_url)?;
        Ok(JiraProjectTarget {
            base_url,
            site_key: site_key.clone(),
            project_key: project.to_string(),
            project_ref: format!("{site_key}/{project}"),
        })
    }

    fn request_headers(&self) -> Result<HeaderMap, ScmError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("rupu-jira-issue-connector"),
        );
        let auth = HeaderValue::from_str(&self.auth.authorization_header()).map_err(|err| {
            ScmError::BadRequest {
                message: format!("invalid jira auth header: {err}"),
            }
        })?;
        headers.insert(AUTHORIZATION, auth);
        Ok(headers)
    }

    async fn request_json<T>(
        &self,
        method: Method,
        url: String,
        body: Option<Value>,
    ) -> Result<T, ScmError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut request = self
            .http
            .request(method, &url)
            .headers(self.request_headers()?);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|err| ScmError::Network(anyhow::anyhow!("jira request {url}: {err}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify_jira_http_error(status.as_u16(), &body, &headers));
        }
        serde_json::from_str(&body)
            .map_err(|err| ScmError::Transient(anyhow::anyhow!("jira parse {url}: {err}")))
    }

    async fn fetch_issue_page(
        &self,
        target: &JiraProjectTarget,
        filter: &IssueFilter,
        next_page_token: Option<&str>,
    ) -> Result<JiraSearchResponse, ScmError> {
        let mut body = json!({
            "jql": build_jql(&target.project_key, filter.state),
            "fields": ["summary", "description", "status", "labels", "reporter", "created", "updated"],
            "maxResults": 100,
        });
        if let Some(next_page_token) = next_page_token {
            body["nextPageToken"] = Value::String(next_page_token.to_string());
        }
        let url = format!(
            "{}/rest/api/3/search/jql",
            target.base_url.trim_end_matches('/')
        );
        self.request_json(Method::POST, url, Some(body)).await
    }

    async fn fetch_issue(
        &self,
        target: &JiraProjectTarget,
        key: &str,
    ) -> Result<JiraIssueNode, ScmError> {
        let url = format!(
            "{}/rest/api/3/issue/{key}?fields=summary,description,status,labels,reporter,created,updated",
            target.base_url.trim_end_matches('/')
        );
        self.request_json(Method::GET, url, None).await
    }

    async fn fetch_issue_types(
        &self,
        target: &JiraProjectTarget,
    ) -> Result<Vec<JiraIssueType>, ScmError> {
        let url = format!(
            "{}/rest/api/3/issue/createmeta/{}/issuetypes",
            target.base_url.trim_end_matches('/'),
            target.project_key
        );
        let value: Value = self.request_json(Method::GET, url, None).await?;
        extract_issue_types(value)
    }

    async fn fetch_transitions(
        &self,
        target: &JiraProjectTarget,
        issue_key: &str,
    ) -> Result<Vec<JiraTransition>, ScmError> {
        let url = format!(
            "{}/rest/api/3/issue/{issue_key}/transitions",
            target.base_url.trim_end_matches('/'),
        );
        let response: JiraTransitionsResponse = self.request_json(Method::GET, url, None).await?;
        Ok(response.transitions)
    }
}

#[async_trait]
impl IssueConnector for JiraIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Jira
    }

    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let target = self.resolve_target(project)?;
        let mut out = Vec::new();
        let mut next_page_token: Option<String> = None;
        let max = filter.limit.unwrap_or(100) as usize;

        loop {
            let page = self
                .fetch_issue_page(&target, &filter, next_page_token.as_deref())
                .await?;
            if page.issues.is_empty() {
                break;
            }
            for node in page.issues {
                let issue = jira_issue_from_node(&target.project_ref, node);
                if issue_matches_filter(&issue, &filter) {
                    out.push(issue);
                    if out.len() >= max {
                        return Ok(out);
                    }
                }
            }
            let Some(token) = page.next_page_token else {
                break;
            };
            next_page_token = Some(token);
        }

        Ok(out)
    }

    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
        let target = self.resolve_target(&i.project)?;
        let issue_key = format!("{}-{}", target.project_key, i.number);
        let node = self.fetch_issue(&target, &issue_key).await?;
        Ok(jira_issue_from_node(&target.project_ref, node))
    }

    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError> {
        let target = self.resolve_target(&i.project)?;
        let issue_key = format!("{}-{}", target.project_key, i.number);
        let url = format!(
            "{}/rest/api/3/issue/{issue_key}/comment",
            target.base_url.trim_end_matches('/'),
        );
        let payload = json!({ "body": adf_text(body) });
        let comment: JiraComment = self.request_json(Method::POST, url, Some(payload)).await?;
        Ok(Comment {
            id: comment.id,
            author: comment
                .author
                .and_then(|author| author.display_name)
                .unwrap_or_default(),
            body: render_adf_text(comment.body.as_ref()),
            created_at: comment.created,
        })
    }

    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError> {
        let target = self.resolve_target(project)?;
        let issue_types = self.fetch_issue_types(&target).await?;
        let issue_type = pick_issue_type(&issue_types).ok_or_else(|| ScmError::BadRequest {
            message: format!(
                "jira project `{}` has no standard issue types available for create",
                target.project_key
            ),
        })?;
        let url = format!("{}/rest/api/3/issue", target.base_url.trim_end_matches('/'));
        let payload = json!({
            "fields": {
                "project": { "key": target.project_key },
                "summary": opts.title,
                "description": adf_text(&opts.body),
                "labels": opts.labels,
                "issuetype": { "id": issue_type.id }
            }
        });
        let created: JiraCreatedIssue = self.request_json(Method::POST, url, Some(payload)).await?;
        let node = self.fetch_issue(&target, &created.key).await?;
        Ok(jira_issue_from_node(&target.project_ref, node))
    }

    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError> {
        let target = self.resolve_target(&i.project)?;
        let issue_key = format!("{}-{}", target.project_key, i.number);
        let transitions = self.fetch_transitions(&target, &issue_key).await?;
        let transition =
            pick_transition(&transitions, state).ok_or_else(|| ScmError::BadRequest {
                message: format!(
                    "jira issue `{issue_key}` has no available {:?} transition",
                    state
                ),
            })?;
        let url = format!(
            "{}/rest/api/3/issue/{issue_key}/transitions",
            target.base_url.trim_end_matches('/'),
        );
        let payload = json!({ "transition": { "id": transition.id } });
        let response = self
            .http
            .request(Method::POST, &url)
            .headers(self.request_headers()?)
            .json(&payload)
            .send()
            .await
            .map_err(|err| ScmError::Network(anyhow::anyhow!("jira request {url}: {err}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify_jira_http_error(status.as_u16(), &body, &headers));
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

fn jira_issue_from_node(project_ref: &str, node: JiraIssueNode) -> Issue {
    Issue {
        r: IssueRef {
            tracker: IssueTracker::Jira,
            project: project_ref.to_string(),
            number: parse_issue_number(&node.key).unwrap_or_default(),
        },
        title: node.fields.summary,
        body: render_adf_text(node.fields.description.as_ref()),
        state: normalize_jira_state(node.fields.status.as_ref()),
        labels: node.fields.labels,
        label_colors: BTreeMap::new(),
        author: node
            .fields
            .reporter
            .and_then(|reporter| reporter.display_name)
            .unwrap_or_default(),
        created_at: node.fields.created,
        updated_at: node.fields.updated,
    }
}

fn parse_issue_number(key: &str) -> Option<u64> {
    key.rsplit_once('-')?.1.parse().ok()
}

fn normalize_jira_state(status: Option<&JiraIssueStatus>) -> IssueState {
    match status.and_then(|status| status.status_category.as_ref()) {
        Some(category) if is_done_category(category) => IssueState::Closed,
        _ => IssueState::Open,
    }
}

fn is_done_category(category: &JiraStatusCategory) -> bool {
    matches!(category.key.as_deref(), Some("done"))
        || matches!(category.name.as_deref(), Some("Done"))
}

fn pick_transition(transitions: &[JiraTransition], desired: IssueState) -> Option<&JiraTransition> {
    match desired {
        IssueState::Closed => transitions.iter().find(|transition| {
            transition
                .to
                .as_ref()
                .and_then(|to| to.status_category.as_ref())
                .is_some_and(is_done_category)
        }),
        IssueState::Open => transitions
            .iter()
            .find(|transition| {
                let name = transition.name.to_ascii_lowercase();
                !transition
                    .to
                    .as_ref()
                    .and_then(|to| to.status_category.as_ref())
                    .is_some_and(is_done_category)
                    && (name.contains("reopen")
                        || name.contains("open")
                        || name.contains("start")
                        || name.contains("progress")
                        || name.contains("todo"))
            })
            .or_else(|| {
                transitions.iter().find(|transition| {
                    !transition
                        .to
                        .as_ref()
                        .and_then(|to| to.status_category.as_ref())
                        .is_some_and(is_done_category)
                })
            }),
    }
}

fn pick_issue_type(issue_types: &[JiraIssueType]) -> Option<&JiraIssueType> {
    for preferred in ["Task", "Story", "Bug"] {
        if let Some(issue_type) = issue_types
            .iter()
            .find(|issue_type| !issue_type.subtask && issue_type.name == preferred)
        {
            return Some(issue_type);
        }
    }
    issue_types
        .iter()
        .find(|issue_type| !issue_type.subtask)
        .or_else(|| issue_types.first())
}

fn extract_issue_types(value: Value) -> Result<Vec<JiraIssueType>, ScmError> {
    if value.is_array() {
        return serde_json::from_value(value)
            .map_err(|err| ScmError::Transient(anyhow::anyhow!("jira issue types parse: {err}")));
    }
    if let Some(values) = value.get("values") {
        return serde_json::from_value(values.clone())
            .map_err(|err| ScmError::Transient(anyhow::anyhow!("jira issue types parse: {err}")));
    }
    if let Some(values) = value.get("issueTypes") {
        return serde_json::from_value(values.clone())
            .map_err(|err| ScmError::Transient(anyhow::anyhow!("jira issue types parse: {err}")));
    }
    Err(ScmError::Transient(anyhow::anyhow!(
        "jira issue types parse: unexpected response shape"
    )))
}

fn render_adf_text(value: Option<&Value>) -> String {
    fn walk(value: &Value, out: &mut String) {
        match value {
            Value::String(text) => out.push_str(text),
            Value::Object(map) => {
                if let Some(text) = map.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                }
                if let Some(content) = map.get("content").and_then(Value::as_array) {
                    let is_paragraph = map
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind == "paragraph");
                    let start_len = out.len();
                    for child in content {
                        walk(child, out);
                    }
                    if is_paragraph && out.len() > start_len && !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
            }
            Value::Array(array) => {
                for entry in array {
                    walk(entry, out);
                }
            }
            _ => {}
        }
    }

    let Some(value) = value else {
        return String::new();
    };
    let mut out = String::new();
    walk(value, &mut out);
    out.trim_end().to_string()
}

fn adf_text(text: &str) -> Value {
    json!({
        "type": "doc",
        "version": 1,
        "content": [{
            "type": "paragraph",
            "content": [{
                "type": "text",
                "text": text,
            }]
        }]
    })
}

fn normalize_site_base_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("{DEFAULT_SITE_SCHEME}://{trimmed}")
    };
    with_scheme
        .split("/rest/api/")
        .next()
        .unwrap_or(&with_scheme)
        .trim_end_matches('/')
        .to_string()
}

fn site_key_from_base_url(base_url: &str) -> Result<String, ScmError> {
    let parsed = Url::parse(base_url).map_err(|err| ScmError::BadRequest {
        message: format!("invalid jira base url `{base_url}`: {err}"),
    })?;
    parsed
        .host_str()
        .map(|host| host.to_string())
        .ok_or_else(|| ScmError::BadRequest {
            message: format!("invalid jira base url `{base_url}`: missing host"),
        })
}

fn build_jql(project_key: &str, state: Option<IssueState>) -> String {
    let escaped = project_key.replace('"', "\\\"");
    match state {
        Some(IssueState::Open) => {
            format!("project = \"{escaped}\" AND statusCategory != Done ORDER BY updated DESC")
        }
        Some(IssueState::Closed) => {
            format!("project = \"{escaped}\" AND statusCategory = Done ORDER BY updated DESC")
        }
        None => format!("project = \"{escaped}\" ORDER BY updated DESC"),
    }
}

fn classify_jira_http_error(status: u16, body: &str, headers: &HeaderMap) -> ScmError {
    match status {
        401 => ScmError::Unauthorized {
            platform: "jira".to_string(),
            hint: "run: rupu auth login --provider jira --mode api-key (store <email>:<api_token>)"
                .to_string(),
        },
        403 => ScmError::Forbidden {
            platform: "jira".to_string(),
            message: truncate_message(body),
        },
        404 => ScmError::NotFound {
            what: "jira resource".to_string(),
        },
        429 => ScmError::RateLimited {
            retry_after: headers
                .get("Retry-After")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(StdDuration::from_secs),
        },
        500..=599 => {
            ScmError::Transient(anyhow::anyhow!("jira {status}: {}", truncate_message(body)))
        }
        _ => ScmError::BadRequest {
            message: truncate_message(body),
        },
    }
}

fn truncate_message(body: &str) -> String {
    if body.len() <= 200 {
        body.to_string()
    } else {
        format!("{}…", &body[..200])
    }
}

#[derive(Debug, Clone)]
enum JiraAuth {
    Basic { email: String, token: String },
    Bearer(String),
}

impl JiraAuth {
    fn from_credentials(
        creds: rupu_providers::auth::AuthCredentials,
    ) -> Result<Self, anyhow::Error> {
        match creds {
            rupu_providers::auth::AuthCredentials::ApiKey { key } => {
                let (email, token) = key.split_once(':').ok_or_else(|| {
                    anyhow::anyhow!("jira api-key credential must be stored as <email>:<api_token>")
                })?;
                if email.is_empty() || token.is_empty() {
                    anyhow::bail!("jira api-key credential must be stored as <email>:<api_token>");
                }
                Ok(Self::Basic {
                    email: email.to_string(),
                    token: token.to_string(),
                })
            }
            rupu_providers::auth::AuthCredentials::OAuth { access, .. } => Ok(Self::Bearer(access)),
        }
    }

    fn authorization_header(&self) -> String {
        match self {
            Self::Basic { email, token } => {
                let joined = format!("{email}:{token}");
                format!("Basic {}", Base64.encode(joined.as_bytes()))
            }
            Self::Bearer(token) => format!("Bearer {token}"),
        }
    }
}

#[derive(Debug, Clone)]
struct JiraProjectTarget {
    base_url: String,
    #[allow(dead_code)]
    site_key: String,
    project_key: String,
    project_ref: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraSearchResponse {
    issues: Vec<JiraIssueNode>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraIssueNode {
    key: String,
    #[serde(rename = "self")]
    #[allow(dead_code)]
    self_url: String,
    fields: JiraIssueFields,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraIssueFields {
    summary: String,
    description: Option<Value>,
    #[serde(default)]
    labels: Vec<String>,
    status: Option<JiraIssueStatus>,
    reporter: Option<JiraUser>,
    created: DateTime<Utc>,
    updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraIssueStatus {
    #[serde(rename = "statusCategory")]
    status_category: Option<JiraStatusCategory>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraStatusCategory {
    key: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraUser {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraComment {
    id: String,
    body: Option<Value>,
    created: DateTime<Utc>,
    author: Option<JiraUser>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraCreatedIssue {
    #[allow(dead_code)]
    id: String,
    key: String,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraIssueType {
    id: String,
    name: String,
    #[serde(default)]
    subtask: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraTransitionsResponse {
    transitions: Vec<JiraTransition>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraTransition {
    id: String,
    name: String,
    to: Option<JiraIssueStatus>,
}
