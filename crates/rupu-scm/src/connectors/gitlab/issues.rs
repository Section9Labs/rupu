//! GitlabIssueConnector — implements rupu_scm::IssueConnector.

use async_trait::async_trait;

use crate::connectors::IssueConnector;
use crate::error::ScmError;
use crate::platform::IssueTracker;
use crate::types::{Comment, CreateIssue, Issue, IssueFilter, IssueRef, IssueState};

use super::client::GitlabClient;

pub struct GitlabIssueConnector {
    client: GitlabClient,
}

impl GitlabIssueConnector {
    pub fn new(client: GitlabClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl IssueConnector for GitlabIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Gitlab
    }

    async fn list_issues(
        &self,
        project: &str,
        filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(project)?;
        let mut path = format!("/projects/{id}/issues?per_page=100");
        if let Some(state) = filter.state {
            let s = match state {
                IssueState::Open => "opened",
                IssueState::Closed => "closed",
            };
            path.push_str(&format!("&state={s}"));
        }
        if !filter.labels.is_empty() {
            path.push_str(&format!(
                "&labels={}",
                urlencode_value(&filter.labels.join(","))
            ));
        }
        if let Some(author) = filter.author.as_ref() {
            path.push_str(&format!("&author_username={author}"));
        }
        let body = self.client.get_json(&path).await?;
        let arr = body.as_array().ok_or_else(|| ScmError::BadRequest {
            message: "expected array from /issues".into(),
        })?;
        arr.iter()
            .map(|v| translate_issue(project.to_string(), v))
            .collect()
    }

    async fn get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&i.project)?;
        let body = self
            .client
            .get_json(&format!("/projects/{id}/issues/{}", i.number))
            .await?;
        translate_issue(i.project.clone(), &body)
    }

    async fn comment_issue(&self, i: &IssueRef, body: &str) -> Result<Comment, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&i.project)?;
        let payload = serde_json::json!({ "body": body });
        let resp = self
            .client
            .write_json(
                reqwest::Method::POST,
                &format!("/projects/{id}/issues/{}/notes", i.number),
                payload,
            )
            .await?;
        translate_note(&resp)
    }

    async fn create_issue(&self, project: &str, opts: CreateIssue) -> Result<Issue, ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(project)?;
        let payload = serde_json::json!({
            "title": opts.title,
            "description": opts.body,
            "labels": opts.labels.join(","),
        });
        let resp = self
            .client
            .write_json(
                reqwest::Method::POST,
                &format!("/projects/{id}/issues"),
                payload,
            )
            .await?;
        translate_issue(project.to_string(), &resp)
    }

    async fn update_issue_state(&self, i: &IssueRef, state: IssueState) -> Result<(), ScmError> {
        let _permit = self.client.permit().await;
        let id = encode_project_id(&i.project)?;
        let event = match state {
            IssueState::Closed => "close",
            IssueState::Open => "reopen",
        };
        let payload = serde_json::json!({ "state_event": event });
        let _ = self
            .client
            .write_json(
                reqwest::Method::PUT,
                &format!("/projects/{id}/issues/{}", i.number),
                payload,
            )
            .await?;
        Ok(())
    }
}

fn encode_project_id(project: &str) -> Result<String, ScmError> {
    if project.is_empty() {
        return Err(ScmError::BadRequest {
            message: "project must not be empty".into(),
        });
    }
    Ok(project.replace('/', "%2F"))
}

fn urlencode_value(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn translate_issue(project: String, v: &serde_json::Value) -> Result<Issue, ScmError> {
    let iid = v
        .get("iid")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| ScmError::BadRequest {
            message: "issue missing iid".into(),
        })?;
    let state_str = v.get("state").and_then(|x| x.as_str()).unwrap_or("opened");
    let state = match state_str {
        "opened" => IssueState::Open,
        _ => IssueState::Closed,
    };
    let title = v
        .get("title")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let body = v
        .get("description")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let author = v
        .get("author")
        .and_then(|a| a.get("username"))
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let labels: Vec<String> = v
        .get("labels")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    // GitLab v4 /issues responses return labels as plain strings.
                    // v3 used objects; handle both for safety.
                    if let Some(s) = l.as_str() {
                        Some(s.to_string())
                    } else {
                        l.get("name").and_then(|n| n.as_str()).map(String::from)
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let created_at = v
        .get("created_at")
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let updated_at = v
        .get("updated_at")
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    Ok(Issue {
        r: IssueRef {
            tracker: IssueTracker::Gitlab,
            project,
            number: iid,
        },
        title,
        body,
        state,
        labels,
        // GitLab's `GET /projects/:id/issues` returns `labels: [string]`
        // (just names) — no embedded hex. Fetching colors requires a
        // separate `GET /projects/:id/labels` round-trip per list call.
        // Leaving empty here; the renderer falls back to a hash-based
        // chip color which is visually fine. Wiring real GitLab label
        // colors is a follow-up if users ask.
        label_colors: std::collections::BTreeMap::new(),
        author,
        created_at,
        updated_at,
    })
}

fn translate_note(v: &serde_json::Value) -> Result<Comment, ScmError> {
    let id = v
        .get("id")
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
        .to_string();
    let body = v
        .get("body")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let author = v
        .get("author")
        .and_then(|a| a.get("username"))
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let created_at = v
        .get("created_at")
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    Ok(Comment {
        id,
        author,
        body,
        created_at,
    })
}
