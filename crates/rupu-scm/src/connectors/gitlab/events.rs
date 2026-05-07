//! GitLab `EventConnector` impl.
//!
//! Polls `GET /projects/:id/events`. GitLab's events API uses
//! `target_type` + `action_name` to disambiguate; we map both to the
//! shared rupu vocabulary (`gitlab.<noun>.<verb>`).
//!
//! Cursor format: `since:<rfc3339>`. GitLab doesn't expose ETag the
//! way GitHub does — we just track the last `created_at` we processed.
//! `:id` is URL-encoded `<group>/<repo>` (GitLab accepts URL-encoded
//! project paths in lieu of numeric ids).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use tracing::debug;

use crate::error::{classify_scm_error, ScmError};
use crate::event_connector::{EventConnector, EventPollResult, PolledEvent};
use crate::platform::Platform;
use crate::types::RepoRef;

pub struct GitlabEventConnector {
    http: reqwest::Client,
    token: String,
    base_url: String,
}

impl GitlabEventConnector {
    pub fn new(token: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| "https://gitlab.com".to_string());
        Self {
            http: reqwest::Client::builder()
                .build()
                .expect("reqwest client build"),
            token,
            base_url,
        }
    }
}

#[async_trait]
impl EventConnector for GitlabEventConnector {
    async fn poll_events(
        &self,
        repo: &RepoRef,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventPollResult, ScmError> {
        let cursor = parse_cursor(cursor);

        // First poll: emit nothing, set since = now.
        if cursor.since.is_none() {
            let now = Utc::now();
            return Ok(EventPollResult {
                events: vec![],
                next_cursor: encode_cursor(&Cursor { since: Some(now) }),
            });
        }
        let since = cursor.since.unwrap();

        let project_id = format!("{}/{}", repo.owner, repo.repo);
        let project_id_enc = urlencode(&project_id);

        // GitLab events endpoint accepts `?after=YYYY-MM-DD` for date
        // filtering. We pull a generous window (last 7 days from
        // `since`) and then refine by `created_at` client-side, so we
        // don't miss anything when ticks are sparse.
        let after_date = (since - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string();
        let url = format!(
            "{}/api/v4/projects/{}/events?after={}&per_page=100&sort=asc",
            self.base_url.trim_end_matches('/'),
            project_id_enc,
            after_date,
        );

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(USER_AGENT, HeaderValue::from_static("rupu/0"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|e| ScmError::Transient(anyhow::anyhow!("invalid token: {e}")))?,
        );

        let resp = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ScmError::Network(anyhow::anyhow!("gitlab events GET {url}: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let resp_headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_scm_error(
                Platform::Gitlab,
                status.as_u16(),
                &body,
                &resp_headers,
            ));
        }

        let raw: Vec<Value> = resp
            .json()
            .await
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("gitlab events parse: {e}")))?;

        // Filter strictly newer than since; events come oldest-first
        // already (we asked for sort=asc).
        let mut filtered: Vec<&Value> = raw
            .iter()
            .filter(|ev| {
                ev.get("created_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc) > since)
                    .unwrap_or(false)
            })
            .collect();
        filtered.truncate(limit as usize);

        let mut events: Vec<PolledEvent> = Vec::with_capacity(filtered.len());
        let mut last_created_at: Option<DateTime<Utc>> = None;
        for raw_ev in &filtered {
            let Some(id) = map_gitlab_event(raw_ev) else {
                continue;
            };
            let delivery = raw_ev
                .get("id")
                .and_then(|v| v.as_i64())
                .map(|n| n.to_string())
                .unwrap_or_default();
            if delivery.is_empty() {
                debug!(?raw_ev, "gitlab event missing id; skipping");
                continue;
            }
            if let Some(s) = raw_ev.get("created_at").and_then(|v| v.as_str()) {
                if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
                    last_created_at = Some(dt.with_timezone(&Utc));
                }
            }
            events.push(PolledEvent {
                id,
                delivery,
                repo: repo.clone(),
                payload: (*raw_ev).clone(),
            });
        }

        let new_since = last_created_at.or(Some(since));
        Ok(EventPollResult {
            events,
            next_cursor: encode_cursor(&Cursor { since: new_since }),
        })
    }
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
        if let Some((k, v)) = part.split_once(':') {
            fields.insert(k, v);
        }
    }
    let since = fields
        .get("since")
        .and_then(|s| {
            // Reconstruct in case the rfc3339 timestamp itself contained a `:`
            // (it always does — re-fetch from the original by joining everything
            // after the first `:` of the `since=` field).
            raw.find("since:")
                .and_then(|i| raw.get(i + "since:".len()..))
                .and_then(|after| DateTime::parse_from_rfc3339(after).ok())
                .or_else(|| DateTime::parse_from_rfc3339(s).ok())
        })
        .map(|dt| dt.with_timezone(&Utc));
    Cursor { since }
}

fn encode_cursor(c: &Cursor) -> String {
    match c.since {
        Some(s) => format!("since:{}", s.to_rfc3339()),
        None => String::new(),
    }
}

/// Map raw GitLab event onto the rupu vocabulary. GitLab's events
/// endpoint exposes `target_type` (Issue / MergeRequest / Note / ...)
/// and `action_name` (`opened`, `closed`, `pushed to`, ...).
fn map_gitlab_event(ev: &Value) -> Option<String> {
    let target_type = ev.get("target_type").and_then(|v| v.as_str());
    let action_name = ev.get("action_name").and_then(|v| v.as_str());
    Some(
        match (target_type, action_name) {
            (Some("Issue"), Some("opened")) => "gitlab.issue.opened",
            (Some("Issue"), Some("closed")) => "gitlab.issue.closed",
            (Some("Issue"), Some("reopened")) => "gitlab.issue.reopened",
            (Some("MergeRequest"), Some("opened")) => "gitlab.mr.opened",
            (Some("MergeRequest"), Some("closed")) => "gitlab.mr.closed",
            (Some("MergeRequest"), Some("merged")) => "gitlab.mr.merged",
            (Some("MergeRequest"), Some("reopened")) => "gitlab.mr.reopened",
            (Some("Note"), _) => "gitlab.comment",
            (None, Some(action)) if action.starts_with("pushed") => "gitlab.push",
            _ => {
                debug!(?target_type, ?action_name, "gitlab events: unrecognized");
                return None;
            }
        }
        .to_string(),
    )
}

fn urlencode(s: &str) -> String {
    // Hand-rolled minimal urlencode — only need slashes and a few
    // safe chars. Avoids pulling in `url::form_urlencoded` here.
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match *b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Discovery helper.
pub async fn try_build(
    resolver: &dyn rupu_auth::CredentialResolver,
    cfg: &rupu_config::Config,
) -> anyhow::Result<Option<std::sync::Arc<dyn EventConnector>>> {
    let creds = match resolver.get("gitlab", None).await {
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
        .get("gitlab")
        .and_then(|p| p.base_url.clone());
    let connector: std::sync::Arc<dyn EventConnector> =
        std::sync::Arc::new(GitlabEventConnector::new(token, base_url));
    Ok(Some(connector))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    fn rr() -> RepoRef {
        RepoRef {
            platform: Platform::Gitlab,
            owner: "my-group".into(),
            repo: "my-project".into(),
        }
    }

    #[test]
    fn map_event_handles_known_kinds() {
        use serde_json::json;
        let issue_opened = json!({"target_type":"Issue","action_name":"opened"});
        assert_eq!(
            map_gitlab_event(&issue_opened).as_deref(),
            Some("gitlab.issue.opened")
        );
        let mr_merged = json!({"target_type":"MergeRequest","action_name":"merged"});
        assert_eq!(
            map_gitlab_event(&mr_merged).as_deref(),
            Some("gitlab.mr.merged")
        );
        let push = json!({"action_name":"pushed to"});
        assert_eq!(map_gitlab_event(&push).as_deref(), Some("gitlab.push"));
        let unknown = json!({"target_type":"Snippet","action_name":"created"});
        assert!(map_gitlab_event(&unknown).is_none());
    }

    #[tokio::test]
    async fn first_poll_returns_empty_with_warmup_cursor() {
        let c = GitlabEventConnector::new("fake".into(), Some("http://127.0.0.1:1".into()));
        let r = c.poll_events(&rr(), None, 50).await.unwrap();
        assert_eq!(r.events.len(), 0);
        assert!(r.next_cursor.contains("since:"));
    }

    #[test]
    fn cursor_round_trip() {
        let c = Cursor {
            since: Some(Utc::now()),
        };
        let encoded = encode_cursor(&c);
        let parsed = parse_cursor(Some(&encoded));
        assert_eq!(
            parsed.since.unwrap().timestamp(),
            c.since.unwrap().timestamp()
        );
    }
}
