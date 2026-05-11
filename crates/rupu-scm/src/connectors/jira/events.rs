//! Jira `EventConnector` impl.
//!
//! Polls Jira Cloud issue search for one project, diffs a persisted
//! local snapshot, and emits:
//!
//! - `jira.issue.opened` for newly-created issues after the cursor
//! - `jira.issue.updated` when workflow-relevant fields changed
//!
//! Updated events carry the same normalized top-level payload shape as
//! webhook-delivered Jira events, so the native tracker alias layer
//! can derive issue-state / sprint / priority / project aliases without
//! extra connector-specific logic.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as Base64;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::error::ScmError;
use crate::event_connector::{EventConnector, EventPollResult, PolledEvent};
use crate::platform::IssueTracker;
use crate::types::{EventSourceRef, EventSubjectRef, IssueRef};

const DEFAULT_SITE_SCHEME: &str = "https";

pub struct JiraEventConnector {
    http: reqwest::Client,
    auth: JiraAuth,
    base_url: Option<String>,
    snapshot_root: PathBuf,
    sprint_field_ids: Mutex<HashMap<String, Option<String>>>,
}

impl JiraEventConnector {
    pub fn new(auth: JiraAuth, base_url: Option<String>, snapshot_root: Option<PathBuf>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .build()
                .expect("reqwest client build"),
            auth,
            base_url: base_url.map(|url| normalize_site_base_url(&url)),
            snapshot_root: snapshot_root.unwrap_or_else(default_snapshot_root),
            sprint_field_ids: Mutex::new(HashMap::new()),
        }
    }

    fn snapshot_path(&self, project_ref: &str) -> PathBuf {
        self.snapshot_root
            .join(format!("{}.json", slug_segment(project_ref)))
    }

    fn load_store(&self, project_ref: &str) -> Result<SnapshotStore, ScmError> {
        let path = self.snapshot_path(project_ref);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                ScmError::Transient(anyhow::anyhow!(
                    "jira snapshot parse {}: {e}",
                    path.display()
                ))
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SnapshotStore::default()),
            Err(err) => Err(ScmError::Transient(anyhow::anyhow!(
                "jira snapshot read {}: {err}",
                path.display()
            ))),
        }
    }

    fn save_store(&self, project_ref: &str, store: &SnapshotStore) -> Result<(), ScmError> {
        let path = self.snapshot_path(project_ref);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                ScmError::Transient(anyhow::anyhow!(
                    "jira snapshot mkdir {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let body = serde_json::to_vec_pretty(store).map_err(|e| {
            ScmError::Transient(anyhow::anyhow!(
                "jira snapshot serialize {}: {e}",
                path.display()
            ))
        })?;
        std::fs::write(&path, body).map_err(|err| {
            ScmError::Transient(anyhow::anyhow!(
                "jira snapshot write {}: {err}",
                path.display()
            ))
        })
    }

    fn resolve_target(&self, source: &EventSourceRef) -> Result<JiraProjectTarget, ScmError> {
        let project = match source {
            EventSourceRef::TrackerProject { tracker, project }
                if *tracker == IssueTracker::Jira =>
            {
                project.as_str()
            }
            _ => {
                return Err(ScmError::BadRequest {
                    message: "jira events polling only supports jira tracker_project sources"
                        .into(),
                });
            }
        };

        if let Some((site, project_key)) = project.rsplit_once('/') {
            if site.is_empty() || project_key.is_empty() {
                return Err(ScmError::BadRequest {
                    message: format!(
                        "invalid jira poll source `{}`; expected jira:<site>/<project>",
                        source.source_ref_text()
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
                    "jira polling source `{}` requires jira:<site>/<project> unless [scm.jira].base_url is configured",
                    source.source_ref_text()
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

    async fn fetch_sprint_field_id(
        &self,
        target: &JiraProjectTarget,
    ) -> Result<Option<String>, ScmError> {
        if let Some(cached) = self
            .sprint_field_ids
            .lock()
            .expect("mutex")
            .get(&target.site_key)
            .cloned()
        {
            return Ok(cached);
        }

        let url = format!("{}/rest/api/3/field", target.base_url.trim_end_matches('/'));
        let response = self
            .http
            .get(&url)
            .headers(self.request_headers()?)
            .send()
            .await
            .map_err(|err| ScmError::Network(anyhow::anyhow!("jira field list: {err}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify_jira_http_error(status.as_u16(), &body, &headers));
        }
        let fields: Vec<JiraFieldMeta> = serde_json::from_str(&body)
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("jira field parse: {e}")))?;
        let sprint = fields.into_iter().find_map(|field| {
            let is_sprint = field.name.eq_ignore_ascii_case("Sprint")
                || field
                    .schema
                    .as_ref()
                    .and_then(|schema| schema.custom.as_deref())
                    == Some("com.pyxis.greenhopper.jira:gh-sprint");
            is_sprint.then_some(field.id)
        });
        self.sprint_field_ids
            .lock()
            .expect("mutex")
            .insert(target.site_key.clone(), sprint.clone());
        Ok(sprint)
    }

    async fn fetch_project_issues(
        &self,
        target: &JiraProjectTarget,
        since: Option<DateTime<Utc>>,
        sprint_field_id: Option<&str>,
    ) -> Result<Vec<JiraIssueNode>, ScmError> {
        let mut next_page_token: Option<String> = None;
        let mut out = Vec::new();
        loop {
            let page = self
                .fetch_issue_page(target, since, sprint_field_id, next_page_token.as_deref())
                .await?;
            if page.issues.is_empty() {
                break;
            }
            out.extend(page.issues);
            if page.next_page_token.is_none() {
                break;
            }
            next_page_token = page.next_page_token;
        }
        Ok(out)
    }

    async fn fetch_issue_page(
        &self,
        target: &JiraProjectTarget,
        since: Option<DateTime<Utc>>,
        sprint_field_id: Option<&str>,
        next_page_token: Option<&str>,
    ) -> Result<JiraSearchResponse, ScmError> {
        let mut fields = vec![
            Value::String("created".into()),
            Value::String("updated".into()),
            Value::String("status".into()),
            Value::String("priority".into()),
            Value::String("project".into()),
        ];
        if let Some(sprint_field_id) = sprint_field_id {
            fields.push(Value::String(sprint_field_id.to_string()));
        }

        let mut body = json!({
            "jql": build_jql(&target.project_key, since),
            "fields": fields,
            "maxResults": 100,
        });
        if let Some(next_page_token) = next_page_token {
            body["nextPageToken"] = Value::String(next_page_token.to_string());
        }

        let url = format!(
            "{}/rest/api/3/search/jql",
            target.base_url.trim_end_matches('/')
        );
        let response = self
            .http
            .post(&url)
            .headers(self.request_headers()?)
            .json(&body)
            .send()
            .await
            .map_err(|err| ScmError::Network(anyhow::anyhow!("jira issue search: {err}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify_jira_http_error(status.as_u16(), &body, &headers));
        }
        serde_json::from_str(&body)
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("jira issue search parse: {e}")))
    }

    fn request_headers(&self) -> Result<HeaderMap, ScmError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("rupu-jira-event-connector"),
        );
        let auth = HeaderValue::from_str(&self.auth.authorization_header()).map_err(|err| {
            ScmError::BadRequest {
                message: format!("invalid jira auth header: {err}"),
            }
        })?;
        headers.insert(AUTHORIZATION, auth);
        Ok(headers)
    }
}

#[async_trait]
impl EventConnector for JiraEventConnector {
    async fn poll_events(
        &self,
        source: &EventSourceRef,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventPollResult, ScmError> {
        let target = self.resolve_target(source)?;
        let cursor = parse_cursor(cursor);
        let sprint_field_id = self.fetch_sprint_field_id(&target).await?;
        let mut store = self.load_store(&target.project_ref)?;

        if cursor.since.is_none() {
            for issue in self
                .fetch_project_issues(&target, None, sprint_field_id.as_deref())
                .await?
            {
                store.issues.insert(
                    issue.id.clone(),
                    IssueSnapshot::from_issue(&issue, sprint_field_id.as_deref()),
                );
            }
            self.save_store(&target.project_ref, &store)?;
            return Ok(EventPollResult {
                events: Vec::new(),
                next_cursor: encode_cursor(&Cursor {
                    since: Some(Utc::now()),
                }),
            });
        }

        let since = cursor.since.expect("checked above");
        let mut issues = self
            .fetch_project_issues(&target, Some(since), sprint_field_id.as_deref())
            .await?;
        if issues.is_empty() {
            return Ok(EventPollResult {
                events: Vec::new(),
                next_cursor: encode_cursor(&cursor),
            });
        }
        issues.sort_by_key(|issue| issue.fields.updated_at);

        let mut candidates = Vec::new();
        let mut refreshed_without_event = Vec::new();
        let mut max_seen = since;

        for issue in issues {
            max_seen = max_seen.max(issue.fields.updated_at);
            let current = IssueSnapshot::from_issue(&issue, sprint_field_id.as_deref());
            let Some(previous) = store.issues.get(&issue.id) else {
                if issue.fields.created_at > since {
                    candidates.push(candidate_opened_event(&target, source, &issue, current));
                } else {
                    refreshed_without_event.push(current);
                }
                continue;
            };

            if let Some(payload) = build_updated_payload(
                previous,
                &current,
                &issue,
                &target,
                sprint_field_id.as_deref(),
            ) {
                candidates.push(CandidateEvent {
                    delivery: format!(
                        "jira:{}:{}:{}",
                        target.project_ref,
                        issue.id,
                        issue.fields.updated_at.timestamp_millis()
                    ),
                    event_id: "jira.issue.updated".to_string(),
                    subject: issue_subject(&target, &issue),
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
        self.save_store(&target.project_ref, &store)?;

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

#[derive(Debug, Clone)]
struct JiraProjectTarget {
    base_url: String,
    site_key: String,
    project_key: String,
    project_ref: String,
}

#[derive(Debug, Clone)]
pub enum JiraAuth {
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

fn candidate_opened_event(
    target: &JiraProjectTarget,
    source: &EventSourceRef,
    issue: &JiraIssueNode,
    current: IssueSnapshot,
) -> CandidateEvent {
    CandidateEvent {
        delivery: format!(
            "jira:{}:{}:{}",
            source.source_ref_text(),
            issue.id,
            issue.fields.updated_at.timestamp_millis()
        ),
        event_id: "jira.issue.opened".to_string(),
        subject: issue_subject(target, issue),
        current,
        payload: normalized_issue_base_payload(issue, target),
    }
}

fn issue_subject(target: &JiraProjectTarget, issue: &JiraIssueNode) -> Option<EventSubjectRef> {
    let number = issue
        .key
        .rsplit_once('-')
        .and_then(|(_, number)| number.parse::<u64>().ok())?;
    Some(EventSubjectRef::Issue {
        issue: IssueRef {
            tracker: IssueTracker::Jira,
            project: target.project_ref.clone(),
            number,
        },
    })
}

fn build_updated_payload(
    previous: &IssueSnapshot,
    current: &IssueSnapshot,
    issue: &JiraIssueNode,
    target: &JiraProjectTarget,
    sprint_field_id: Option<&str>,
) -> Option<Value> {
    let mut payload = normalized_issue_base_payload(issue, target);
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
            snapshot_project_value(previous.project.as_ref()),
            snapshot_project_value(current.project.as_ref()),
            Some("project"),
        )?;
        changed = true;
    }
    if previous.sprint != current.sprint {
        payload["sprint"] = transition_object(
            snapshot_named_value(previous.sprint.as_ref()),
            snapshot_named_value(current.sprint.as_ref()),
            Some("sprint"),
        )?;
        changed = true;
    }
    if previous.priority != current.priority {
        payload["priority"] = transition_object(
            snapshot_named_value(previous.priority.as_ref()),
            snapshot_named_value(current.priority.as_ref()),
            None,
        )?;
        changed = true;
    }

    if let Some(sprint_field_id) = sprint_field_id {
        payload["context"]["sprint_field_id"] = Value::String(sprint_field_id.to_string());
    }

    changed.then_some(payload)
}

fn normalized_issue_base_payload(issue: &JiraIssueNode, target: &JiraProjectTarget) -> Value {
    json!({
        "vendor": "jira",
        "subject": {
            "kind": "issue",
            "id": issue.id,
            "ref": issue.key,
            "url": issue.self_url,
        },
        "tenant": {
            "base_url": target.base_url,
            "site": target.site_key,
        },
        "context": {
            "project": issue.fields.project,
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

fn snapshot_state_value(value: Option<&StateSnapshot>) -> Value {
    match value {
        Some(value) => json!({
            "id": value.id,
            "name": value.name,
        }),
        None => Value::Null,
    }
}

fn snapshot_project_value(value: Option<&ProjectSnapshot>) -> Value {
    match value {
        Some(value) => json!({
            "id": value.id,
            "key": value.key,
            "name": value.name,
        }),
        None => Value::Null,
    }
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

fn build_jql(project_key: &str, since: Option<DateTime<Utc>>) -> String {
    let escaped = project_key.replace('"', "\\\"");
    match since {
        Some(since) => format!(
            "project = \"{escaped}\" AND updated >= \"{}\" ORDER BY updated ASC",
            since.format("%Y-%m-%d %H:%M")
        ),
        None => format!("project = \"{escaped}\" ORDER BY updated ASC"),
    }
}

fn default_snapshot_root() -> PathBuf {
    rupu_home_dir()
        .unwrap_or_else(|| PathBuf::from(".rupu"))
        .join("cron-state")
        .join("jira-snapshots")
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

pub async fn try_build(
    resolver: &dyn rupu_auth::CredentialResolver,
    cfg: &rupu_config::Config,
) -> anyhow::Result<Option<Arc<dyn EventConnector>>> {
    let creds = match resolver
        .get("jira", Some(rupu_providers::AuthMode::ApiKey))
        .await
    {
        Ok((_mode, creds)) => creds,
        Err(_) => return Ok(None),
    };
    let auth = JiraAuth::from_credentials(creds)?;
    let base_url = cfg
        .scm
        .platforms
        .get("jira")
        .and_then(|platform| platform.base_url.clone());
    Ok(Some(Arc::new(JiraEventConnector::new(
        auth, base_url, None,
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
    key: String,
    self_url: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    state: Option<StateSnapshot>,
    priority: Option<NamedEntitySnapshot>,
    project: Option<ProjectSnapshot>,
    sprint: Option<NamedEntitySnapshot>,
}

impl IssueSnapshot {
    fn from_issue(issue: &JiraIssueNode, sprint_field_id: Option<&str>) -> Self {
        Self {
            id: issue.id.clone(),
            key: issue.key.clone(),
            self_url: issue.self_url.clone(),
            created_at: issue.fields.created_at,
            updated_at: issue.fields.updated_at,
            state: issue.fields.status.as_ref().map(|status| StateSnapshot {
                id: status.id.clone().unwrap_or_default(),
                name: status.name.clone().unwrap_or_default(),
            }),
            priority: issue
                .fields
                .priority
                .as_ref()
                .map(|priority| NamedEntitySnapshot {
                    id: priority.id.clone().unwrap_or_default(),
                    name: priority.name.clone().unwrap_or_default(),
                }),
            project: issue
                .fields
                .project
                .as_ref()
                .map(|project| ProjectSnapshot {
                    id: project.id.clone().unwrap_or_default(),
                    key: project.key.clone().unwrap_or_default(),
                    name: project.name.clone().unwrap_or_default(),
                }),
            sprint: sprint_field_id
                .and_then(|field_id| issue.fields.extra.get(field_id))
                .and_then(extract_sprint_value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StateSnapshot {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectSnapshot {
    id: String,
    key: String,
    name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NamedEntitySnapshot {
    id: String,
    name: String,
}

fn extract_sprint_value(value: &Value) -> Option<NamedEntitySnapshot> {
    match value {
        Value::Array(items) => items.iter().rev().find_map(extract_sprint_value),
        Value::Object(map) => Some(NamedEntitySnapshot {
            id: map.get("id").and_then(value_as_string).unwrap_or_default(),
            name: map
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        _ => None,
    }
}

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
struct JiraSearchResponse {
    #[serde(default)]
    issues: Vec<JiraIssueNode>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct JiraIssueNode {
    id: String,
    key: String,
    #[serde(rename = "self")]
    self_url: String,
    fields: JiraIssueFields,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct JiraIssueFields {
    #[serde(rename = "created")]
    created_at: DateTime<Utc>,
    #[serde(rename = "updated")]
    updated_at: DateTime<Utc>,
    status: Option<JiraNamedNode>,
    priority: Option<JiraNamedNode>,
    project: Option<JiraProjectNode>,
    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct JiraNamedNode {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct JiraProjectNode {
    id: Option<String>,
    key: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraFieldMeta {
    id: String,
    name: String,
    #[serde(default)]
    schema: Option<JiraFieldSchema>,
}

#[derive(Debug, Clone, Deserialize)]
struct JiraFieldSchema {
    #[serde(default)]
    custom: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::{GET, POST};
    use httpmock::MockServer;

    fn jira_source(project: &str) -> EventSourceRef {
        EventSourceRef::TrackerProject {
            tracker: IssueTracker::Jira,
            project: project.to_string(),
        }
    }

    fn connector(server: &MockServer, snapshot_root: &std::path::Path) -> JiraEventConnector {
        JiraEventConnector::new(
            JiraAuth::Basic {
                email: "matt@example.com".into(),
                token: "tok".into(),
            },
            Some(server.base_url()),
            Some(snapshot_root.to_path_buf()),
        )
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

    #[test]
    fn jira_source_with_site_parses() {
        let source = jira_source("acme.atlassian.net/ENG");
        let target = JiraEventConnector::new(
            JiraAuth::Basic {
                email: "matt@example.com".into(),
                token: "tok".into(),
            },
            None,
            None,
        )
        .resolve_target(&source)
        .expect("target");
        assert_eq!(target.site_key, "acme.atlassian.net");
        assert_eq!(target.project_key, "ENG");
        assert_eq!(target.project_ref, "acme.atlassian.net/ENG");
    }

    #[tokio::test]
    async fn first_poll_warms_snapshot_and_emits_no_events() {
        let server = MockServer::start_async().await;
        let _fields = server.mock(|when, then| {
            when.method(GET).path("/rest/api/3/field");
            then.status(200).json_body(json!([
                {
                    "id": "customfield_10020",
                    "name": "Sprint",
                    "schema": { "custom": "com.pyxis.greenhopper.jira:gh-sprint" }
                }
            ]));
        });
        let _search = server.mock(|when, then| {
            when.method(POST).path("/rest/api/3/search/jql");
            then.status(200).json_body(json!({
                "issues": [
                    {
                        "id": "10001",
                        "key": "ENG-1",
                        "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
                        "fields": {
                            "created": "2026-05-10T00:00:00.000+0000",
                            "updated": "2026-05-10T00:00:00.000+0000",
                            "status": { "id": "3", "name": "To Do" },
                            "priority": { "id": "2", "name": "Medium" },
                            "project": { "id": "10000", "key": "ENG", "name": "Engineering" },
                            "customfield_10020": [
                                { "id": 41, "name": "Sprint 41" }
                            ]
                        }
                    }
                ],
                "nextPageToken": null
            }));
        });
        let temp = tempfile::tempdir().unwrap();
        let connector = connector(&server, temp.path());

        let result = connector
            .poll_events(&jira_source("ENG"), None, 50)
            .await
            .unwrap();
        assert!(result.events.is_empty());
        assert!(result.next_cursor.contains("since:"));
        assert!(temp.path().join("127_0_0_1_eng.json").exists());
    }

    #[tokio::test]
    async fn update_poll_emits_normalized_state_priority_and_sprint_transition() {
        let server = MockServer::start_async().await;
        let _fields = server.mock(|when, then| {
            when.method(GET).path("/rest/api/3/field");
            then.status(200).json_body(json!([
                {
                    "id": "customfield_10020",
                    "name": "Sprint",
                    "schema": { "custom": "com.pyxis.greenhopper.jira:gh-sprint" }
                }
            ]));
        });
        let _search = server.mock(|when, then| {
            when.method(POST).path("/rest/api/3/search/jql");
            then.status(200).json_body(json!({
                "issues": [
                    {
                        "id": "10001",
                        "key": "ENG-1",
                        "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
                        "fields": {
                            "created": "2026-05-10T00:00:00.000+0000",
                            "updated": "2026-05-10T01:00:00.000+0000",
                            "status": { "id": "4", "name": "Ready For Review" },
                            "priority": { "id": "1", "name": "High" },
                            "project": { "id": "10000", "key": "ENG", "name": "Engineering" },
                            "customfield_10020": [
                                { "id": 42, "name": "Sprint 42" }
                            ]
                        }
                    }
                ],
                "nextPageToken": null
            }));
        });
        let temp = tempfile::tempdir().unwrap();
        let connector = connector(&server, temp.path());
        let target = connector.resolve_target(&jira_source("ENG")).unwrap();
        let mut store = SnapshotStore::default();
        store.issues.insert(
            "10001".into(),
            IssueSnapshot {
                id: "10001".into(),
                key: "ENG-1".into(),
                self_url: "https://acme.atlassian.net/rest/api/3/issue/10001".into(),
                created_at: DateTime::parse_from_rfc3339("2026-05-10T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                updated_at: DateTime::parse_from_rfc3339("2026-05-10T00:30:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                state: Some(StateSnapshot {
                    id: "3".into(),
                    name: "To Do".into(),
                }),
                priority: Some(NamedEntitySnapshot {
                    id: "2".into(),
                    name: "Medium".into(),
                }),
                project: Some(ProjectSnapshot {
                    id: "10000".into(),
                    key: "ENG".into(),
                    name: "Engineering".into(),
                }),
                sprint: Some(NamedEntitySnapshot {
                    id: "41".into(),
                    name: "Sprint 41".into(),
                }),
            },
        );
        connector.save_store(&target.project_ref, &store).unwrap();

        let result = connector
            .poll_events(&jira_source("ENG"), Some("since:2026-05-10T00:45:00Z"), 50)
            .await
            .unwrap();
        assert_eq!(result.events.len(), 1);
        let event = &result.events[0];
        assert_eq!(event.id, "jira.issue.updated");
        assert_eq!(
            event.subject.as_ref().unwrap(),
            &EventSubjectRef::Issue {
                issue: IssueRef {
                    tracker: IssueTracker::Jira,
                    project: "127.0.0.1/ENG".into(),
                    number: 1,
                }
            }
        );
        assert_eq!(event.payload["state"]["after"]["name"], "Ready For Review");
        assert_eq!(event.payload["priority"]["after"]["name"], "High");
        assert_eq!(event.payload["sprint"]["after"]["name"], "Sprint 42");
    }
}
