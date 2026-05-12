//! Linear `EventConnector` impl.
//!
//! Polls the Linear GraphQL API for one team's issues ordered by
//! `updatedAt`, diffs a persisted local snapshot, and emits:
//!
//! - `linear.issue.opened` for newly-created issues after the cursor
//! - `linear.issue.updated` when workflow-relevant fields changed
//!
//! Updated events carry the same normalized top-level payload shape as
//! webhook-delivered Linear events, so the native tracker alias layer
//! can derive issue-state / project / cycle / priority / blocked
//! aliases without additional connector-specific logic.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::ScmError;
use crate::event_connector::{EventConnector, EventPollResult, PolledEvent};
use crate::platform::IssueTracker;
use crate::types::{EventSourceRef, EventSubjectRef, IssueRef};

const DEFAULT_BASE_URL: &str = "https://api.linear.app/graphql";

pub struct LinearEventConnector {
    http: reqwest::Client,
    token: String,
    base_url: String,
    snapshot_root: PathBuf,
}

impl LinearEventConnector {
    pub fn new(token: String, base_url: Option<String>, snapshot_root: Option<PathBuf>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .build()
                .expect("reqwest client build"),
            token,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            snapshot_root: snapshot_root.unwrap_or_else(default_snapshot_root),
        }
    }

    fn snapshot_path(&self, team_id: &str) -> PathBuf {
        self.snapshot_root
            .join(format!("{}.json", slug_segment(team_id)))
    }

    fn load_store(&self, team_id: &str) -> Result<SnapshotStore, ScmError> {
        let path = self.snapshot_path(team_id);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                ScmError::Transient(anyhow::anyhow!(
                    "linear snapshot parse {}: {e}",
                    path.display()
                ))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SnapshotStore::default()),
            Err(err) => Err(ScmError::Transient(anyhow::anyhow!(
                "linear snapshot read {}: {err}",
                path.display()
            ))),
        }
    }

    fn save_store(&self, team_id: &str, store: &SnapshotStore) -> Result<(), ScmError> {
        let path = self.snapshot_path(team_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                ScmError::Transient(anyhow::anyhow!(
                    "linear snapshot mkdir {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let body = serde_json::to_vec_pretty(store).map_err(|e| {
            ScmError::Transient(anyhow::anyhow!(
                "linear snapshot serialize {}: {e}",
                path.display()
            ))
        })?;
        std::fs::write(&path, body).map_err(|err| {
            ScmError::Transient(anyhow::anyhow!(
                "linear snapshot write {}: {err}",
                path.display()
            ))
        })
    }

    async fn fetch_team_issues(
        &self,
        team_id: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<LinearIssueNode>, ScmError> {
        let mut after: Option<String> = None;
        let mut out = Vec::new();
        let mut descending: Option<bool> = None;

        loop {
            let page = self
                .fetch_team_issue_page(team_id, after.as_deref())
                .await?;
            if page.nodes.is_empty() {
                break;
            }
            if descending.is_none() {
                descending = Some(page_is_descending(&page.nodes));
            }

            let mut reached_older = false;
            for issue in &page.nodes {
                if let Some(since) = since {
                    if issue.updated_at <= since {
                        reached_older = true;
                        if descending == Some(true) {
                            continue;
                        }
                    }
                }
                out.push(issue.clone());
            }

            if !page.page_info.has_next_page {
                break;
            }
            if reached_older && descending == Some(true) {
                break;
            }
            after = page.page_info.end_cursor;
        }

        Ok(out)
    }

    async fn fetch_team_issue_page(
        &self,
        team_id: &str,
        after: Option<&str>,
    ) -> Result<LinearIssuePage, ScmError> {
        let query = r#"
            query TeamIssues($teamId: String!, $after: String) {
              team(id: $teamId) {
                id
                key
                name
                issues(first: 100, after: $after, orderBy: updatedAt) {
                  nodes {
                    id
                    identifier
                    url
                    createdAt
                    updatedAt
                    priority
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
                    project {
                      id
                      name
                    }
                    cycle {
                      id
                      name
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
        let data: LinearIssuesData = self
            .graphql(
                query,
                json!({
                    "teamId": team_id,
                    "after": after,
                }),
            )
            .await?;
        let team = data.team.ok_or_else(|| ScmError::NotFound {
            what: format!("linear team `{team_id}`"),
        })?;
        Ok(team.issues)
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
            .post(self.base_url.trim_end_matches('/'))
            .headers(headers)
            .json(&json!({
                "query": query,
                "variables": variables,
            }))
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
        let body = response.text().await.unwrap_or_default();
        let parsed: LinearGraphqlEnvelope<T> = serde_json::from_str(&body)
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("linear graphql parse: {e}")))?;

        if let Some(error) = parsed
            .errors
            .as_ref()
            .and_then(|errors| classify_linear_graphql_error(errors))
        {
            return Err(error);
        }
        if !status.is_success() {
            return Err(classify_linear_http_error(status.as_u16(), &body, &headers));
        }
        parsed
            .data
            .ok_or_else(|| ScmError::Transient(anyhow::anyhow!("linear graphql missing data")))
    }
}

#[async_trait]
impl EventConnector for LinearEventConnector {
    async fn poll_events(
        &self,
        source: &EventSourceRef,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventPollResult, ScmError> {
        let team_id = match source {
            EventSourceRef::TrackerProject { tracker, project }
                if *tracker == IssueTracker::Linear =>
            {
                project.as_str()
            }
            _ => {
                return Err(ScmError::BadRequest {
                    message: "linear events polling only supports linear tracker_project sources"
                        .to_string(),
                });
            }
        };
        let cursor = parse_cursor(cursor);
        let mut store = self.load_store(team_id)?;

        if cursor.since.is_none() {
            for issue in self.fetch_team_issues(team_id, None).await? {
                store
                    .issues
                    .insert(issue.id.clone(), IssueSnapshot::from(&issue));
            }
            self.save_store(team_id, &store)?;
            return Ok(EventPollResult {
                events: Vec::new(),
                next_cursor: encode_cursor(&Cursor {
                    since: Some(Utc::now()),
                }),
            });
        }

        let since = cursor.since.expect("checked above");
        let mut issues = self.fetch_team_issues(team_id, Some(since)).await?;
        if issues.is_empty() {
            return Ok(EventPollResult {
                events: Vec::new(),
                next_cursor: encode_cursor(&cursor),
            });
        }
        issues.sort_by_key(|issue| issue.updated_at);

        let mut candidates = Vec::new();
        let mut refreshed_without_event = Vec::new();
        let mut max_seen = since;

        for issue in issues {
            max_seen = max_seen.max(issue.updated_at);
            let current = IssueSnapshot::from(&issue);
            let Some(previous) = store.issues.get(&issue.id) else {
                if issue.created_at > since {
                    candidates.push(candidate_opened_event(team_id, source, &issue, current));
                } else {
                    refreshed_without_event.push(current);
                }
                continue;
            };

            if let Some(payload) = build_updated_payload(previous, &current, &issue) {
                candidates.push(CandidateEvent {
                    delivery: format!(
                        "linear:{}:{}:{}",
                        team_id,
                        issue.id,
                        issue.updated_at.timestamp_millis()
                    ),
                    event_id: "linear.issue.updated".to_string(),
                    subject: issue_subject(team_id, &issue),
                    current,
                    payload,
                });
            } else {
                refreshed_without_event.push(current);
            }
        }

        let limit = limit as usize;
        let emitted_count = candidates.len().min(limit);
        for snapshot in refreshed_without_event {
            store.issues.insert(snapshot.id.clone(), snapshot);
        }
        for candidate in candidates.iter().take(emitted_count) {
            store
                .issues
                .insert(candidate.current.id.clone(), candidate.current.clone());
        }
        self.save_store(team_id, &store)?;

        let next_since = if candidates.len() > emitted_count {
            candidates
                .get(emitted_count.saturating_sub(1))
                .map(|candidate| candidate.current.updated_at - Duration::nanoseconds(1))
                .unwrap_or(since)
        } else {
            max_seen
        };

        Ok(EventPollResult {
            events: candidates
                .into_iter()
                .take(emitted_count)
                .map(|candidate| PolledEvent {
                    id: candidate.event_id,
                    delivery: candidate.delivery,
                    source: source.clone(),
                    subject: candidate.subject,
                    payload: candidate.payload,
                })
                .collect(),
            next_cursor: encode_cursor(&Cursor {
                since: Some(next_since),
            }),
        })
    }
}

#[derive(Debug, Clone)]
struct CandidateEvent {
    delivery: String,
    event_id: String,
    subject: Option<EventSubjectRef>,
    current: IssueSnapshot,
    payload: Value,
}

fn candidate_opened_event(
    team_id: &str,
    source: &EventSourceRef,
    issue: &LinearIssueNode,
    current: IssueSnapshot,
) -> CandidateEvent {
    CandidateEvent {
        delivery: format!(
            "linear:{}:{}:{}",
            source.source_ref_text(),
            issue.id,
            issue.updated_at.timestamp_millis()
        ),
        event_id: "linear.issue.opened".to_string(),
        subject: issue_subject(team_id, issue),
        current,
        payload: normalized_issue_base_payload(issue),
    }
}

fn issue_subject(team_id: &str, issue: &LinearIssueNode) -> Option<EventSubjectRef> {
    let number = issue
        .identifier
        .rsplit_once('-')
        .and_then(|(_, number)| number.parse::<u64>().ok())?;
    Some(EventSubjectRef::Issue {
        issue: IssueRef {
            tracker: IssueTracker::Linear,
            project: team_id.to_string(),
            number,
        },
    })
}

fn build_updated_payload(
    previous: &IssueSnapshot,
    current: &IssueSnapshot,
    issue: &LinearIssueNode,
) -> Option<Value> {
    let mut payload = normalized_issue_base_payload(issue);
    let mut changed = false;

    if previous.state != current.state {
        payload["state"] = transition_object(
            snapshot_state_value(previous.state.as_ref()),
            snapshot_state_value(current.state.as_ref()),
            Some("workflow_state"),
        )?;
        changed = true;
    }
    if previous.project != current.project {
        payload["project"] = transition_object(
            snapshot_named_value(previous.project.as_ref()),
            snapshot_named_value(current.project.as_ref()),
            None,
        )?;
        changed = true;
    }
    if previous.cycle != current.cycle {
        payload["cycle"] = transition_object(
            snapshot_named_value(previous.cycle.as_ref()),
            snapshot_named_value(current.cycle.as_ref()),
            None,
        )?;
        changed = true;
    }
    if previous.priority != current.priority {
        payload["priority"] = transition_object(
            priority_value(previous.priority),
            priority_value(current.priority),
            None,
        )?;
        changed = true;
    }
    changed.then_some(payload)
}

fn normalized_issue_base_payload(issue: &LinearIssueNode) -> Value {
    json!({
        "vendor": "linear",
        "subject": {
            "kind": "issue",
            "id": issue.id,
            "ref": issue.identifier,
            "url": issue.url,
        },
        "team": {
            "id": issue.team.id,
            "key": issue.team.key,
            "name": issue.team.name,
        },
        "payload": serde_json::to_value(issue).unwrap_or(Value::Null),
    })
}

fn transition_object(before: Value, after: Value, category: Option<&str>) -> Option<Value> {
    if before == after {
        return None;
    }
    let mut value = json!({
        "before": before,
        "after": after,
    });
    if let Some(category) = category {
        value["category"] = Value::String(category.to_string());
    }
    Some(value)
}

fn snapshot_named_value(value: Option<&NamedEntitySnapshot>) -> Value {
    match value {
        Some(value) => json!({
            "id": value.id,
            "name": value.name,
        }),
        None => Value::Null,
    }
}

fn snapshot_state_value(value: Option<&StateSnapshot>) -> Value {
    match value {
        Some(value) => json!({
            "id": value.id,
            "name": value.name,
            "type": value.kind,
        }),
        None => Value::Null,
    }
}

fn priority_value(priority: i64) -> Value {
    let (id, name) = match priority {
        1 => ("1", "Urgent"),
        2 => ("2", "High"),
        3 => ("3", "Medium"),
        4 => ("4", "Low"),
        _ => ("0", "No priority"),
    };
    json!({
        "id": id,
        "name": name,
    })
}

fn page_is_descending(nodes: &[LinearIssueNode]) -> bool {
    nodes
        .windows(2)
        .all(|pair| pair[0].updated_at >= pair[1].updated_at)
}

fn default_snapshot_root() -> PathBuf {
    rupu_home_dir()
        .unwrap_or_else(|| PathBuf::from(".rupu"))
        .join("cron-state")
        .join("linear-snapshots")
}

fn rupu_home_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RUPU_HOME") {
        return Some(PathBuf::from(path));
    }
    dirs::home_dir().map(|home| home.join(".rupu"))
}

fn slug_segment(raw: &str) -> String {
    let mut out = String::new();
    let mut last_sep = false;
    for ch in raw.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch.to_ascii_lowercase()
        } else {
            '_'
        };
        if mapped == '_' {
            if out.is_empty() || last_sep {
                continue;
            }
            out.push('_');
            last_sep = true;
        } else {
            out.push(mapped);
            last_sep = false;
        }
    }
    out.trim_matches('_').to_string()
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
                .map(StdDuration::from_secs),
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

/// Discovery helper.
pub async fn try_build(
    resolver: &dyn rupu_auth::CredentialResolver,
    cfg: &rupu_config::Config,
) -> anyhow::Result<Option<Arc<dyn EventConnector>>> {
    let creds = match resolver
        .get("linear", Some(rupu_providers::AuthMode::ApiKey))
        .await
    {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let token = match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => key,
        rupu_providers::auth::AuthCredentials::OAuth { access, .. } => access,
    };
    let base_url = cfg
        .scm
        .platforms
        .get("linear")
        .and_then(|platform| platform.base_url.clone());
    Ok(Some(Arc::new(LinearEventConnector::new(
        token, base_url, None,
    ))))
}

#[derive(Debug, Clone, Default)]
struct Cursor {
    since: Option<DateTime<Utc>>,
}

fn parse_cursor(raw: Option<&str>) -> Cursor {
    let Some(raw) = raw else {
        return Cursor::default();
    };
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for part in raw.split('|') {
        if let Some((key, value)) = part.split_once(':') {
            fields.insert(key, value);
        }
    }
    let since = fields
        .get("since")
        .and_then(|_| {
            raw.find("since:")
                .and_then(|idx| raw.get(idx + "since:".len()..))
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        })
        .map(|dt| dt.with_timezone(&Utc));
    Cursor { since }
}

fn encode_cursor(cursor: &Cursor) -> String {
    match cursor.since {
        Some(since) => format!("since:{}", since.to_rfc3339()),
        None => String::new(),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SnapshotStore {
    issues: BTreeMap<String, IssueSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct IssueSnapshot {
    id: String,
    identifier: String,
    url: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    priority: i64,
    team: NamedEntitySnapshot,
    state: Option<StateSnapshot>,
    project: Option<NamedEntitySnapshot>,
    cycle: Option<NamedEntitySnapshot>,
}

impl From<&LinearIssueNode> for IssueSnapshot {
    fn from(value: &LinearIssueNode) -> Self {
        Self {
            id: value.id.clone(),
            identifier: value.identifier.clone(),
            url: value.url.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            priority: value.priority,
            team: NamedEntitySnapshot {
                id: value.team.id.clone(),
                name: value.team.name.clone(),
            },
            state: value.state.as_ref().map(|state| StateSnapshot {
                id: state.id.clone(),
                name: state.name.clone(),
                kind: state.kind.clone(),
            }),
            project: value.project.as_ref().map(|project| NamedEntitySnapshot {
                id: project.id.clone(),
                name: project.name.clone(),
            }),
            cycle: value.cycle.as_ref().map(|cycle| NamedEntitySnapshot {
                id: cycle.id.clone(),
                name: cycle.name.clone(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NamedEntitySnapshot {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StateSnapshot {
    id: String,
    name: String,
    kind: String,
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
struct LinearIssuesData {
    team: Option<LinearTeamNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct LinearTeamNode {
    issues: LinearIssuePage,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LinearIssueNode {
    id: String,
    identifier: String,
    url: String,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    updated_at: DateTime<Utc>,
    #[serde(default)]
    priority: i64,
    team: LinearNamedNodeWithKey,
    state: Option<LinearStateNode>,
    project: Option<LinearNamedNode>,
    cycle: Option<LinearNamedNode>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LinearNamedNode {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LinearNamedNodeWithKey {
    id: String,
    key: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LinearStateNode {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::POST;
    use httpmock::MockServer;

    fn linear_source(team_id: &str) -> EventSourceRef {
        EventSourceRef::TrackerProject {
            tracker: IssueTracker::Linear,
            project: team_id.to_string(),
        }
    }

    #[test]
    fn cursor_round_trip() {
        let cursor = Cursor {
            since: Some(Utc::now()),
        };
        let encoded = encode_cursor(&cursor);
        let parsed = parse_cursor(Some(&encoded));
        assert_eq!(
            parsed.since.expect("since").timestamp(),
            cursor.since.expect("since").timestamp()
        );
    }

    #[tokio::test]
    async fn first_poll_warms_snapshot_and_emits_no_events() {
        let server = MockServer::start_async().await;
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(200).json_body(json!({
                "data": {
                    "team": {
                        "issues": {
                            "nodes": [
                                {
                                    "id": "issue-1",
                                    "identifier": "ENG-1",
                                    "url": "https://linear.app/acme/issue/ENG-1",
                                    "createdAt": "2026-05-10T00:00:00Z",
                                    "updatedAt": "2026-05-10T00:00:00Z",
                                    "priority": 0,
                                    "team": { "id": "team-123", "key": "ENG", "name": "Engineering" },
                                    "state": { "id": "todo", "name": "Todo", "type": "unstarted" },
                                    "project": null,
                                    "cycle": null,
                                    "blockedByIssues": { "nodes": [] }
                                }
                            ],
                            "pageInfo": { "hasNextPage": false, "endCursor": null }
                        }
                    }
                }
            }));
        });
        let temp = tempfile::tempdir().unwrap();
        let connector = LinearEventConnector::new(
            "lin_api_test".into(),
            Some(server.url("/")),
            Some(temp.path().to_path_buf()),
        );

        let result = connector
            .poll_events(&linear_source("team-123"), None, 50)
            .await
            .unwrap();
        assert!(result.events.is_empty());
        assert!(result.next_cursor.contains("since:"));
        assert!(temp.path().join("team_123.json").exists());
    }

    #[tokio::test]
    async fn update_poll_emits_normalized_state_transition() {
        let server = MockServer::start_async().await;
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(200).json_body(json!({
                "data": {
                    "team": {
                        "issues": {
                            "nodes": [
                                {
                                    "id": "issue-1",
                                    "identifier": "ENG-1",
                                    "url": "https://linear.app/acme/issue/ENG-1",
                                    "createdAt": "2026-05-10T00:00:00Z",
                                    "updatedAt": "2026-05-10T01:00:00Z",
                                    "priority": 0,
                                    "team": { "id": "team-123", "key": "ENG", "name": "Engineering" },
                                    "state": { "id": "in_progress", "name": "In Progress", "type": "started" },
                                    "project": null,
                                    "cycle": null,
                                    "blockedByIssues": { "nodes": [] }
                                }
                            ],
                            "pageInfo": { "hasNextPage": false, "endCursor": null }
                        }
                    }
                }
            }));
        });
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("team_123.json"),
            serde_json::to_vec_pretty(&SnapshotStore {
                issues: BTreeMap::from([(
                    "issue-1".to_string(),
                    IssueSnapshot {
                        id: "issue-1".into(),
                        identifier: "ENG-1".into(),
                        url: "https://linear.app/acme/issue/ENG-1".into(),
                        created_at: DateTime::parse_from_rfc3339("2026-05-10T00:00:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        updated_at: DateTime::parse_from_rfc3339("2026-05-10T00:10:00Z")
                            .unwrap()
                            .with_timezone(&Utc),
                        priority: 0,
                        team: NamedEntitySnapshot {
                            id: "team-123".into(),
                            name: "Engineering".into(),
                        },
                        state: Some(StateSnapshot {
                            id: "todo".into(),
                            name: "Todo".into(),
                            kind: "unstarted".into(),
                        }),
                        project: None,
                        cycle: None,
                    },
                )]),
            })
            .unwrap(),
        )
        .unwrap();
        let connector = LinearEventConnector::new(
            "lin_api_test".into(),
            Some(server.url("/")),
            Some(temp.path().to_path_buf()),
        );

        let result = connector
            .poll_events(
                &linear_source("team-123"),
                Some("since:2026-05-10T00:30:00+00:00"),
                50,
            )
            .await
            .unwrap();
        assert_eq!(result.events.len(), 1);
        let event = &result.events[0];
        assert_eq!(event.id, "linear.issue.updated");
        assert_eq!(event.payload["subject"]["ref"], "ENG-1");
        assert_eq!(event.payload["state"]["before"]["name"], "Todo");
        assert_eq!(event.payload["state"]["after"]["name"], "In Progress");
        assert_eq!(
            event.subject.as_ref().unwrap(),
            &EventSubjectRef::Issue {
                issue: IssueRef {
                    tracker: IssueTracker::Linear,
                    project: "team-123".into(),
                    number: 1,
                }
            }
        );
    }
}
