//! GitHub `EventConnector` impl.
//!
//! Polls `GET /repos/{owner}/{repo}/events`. The response includes an
//! `ETag` and a list of events ordered newest-first. We:
//!
//! 1. Send `If-None-Match: <etag>` from the prior cursor → 304 fast-path.
//! 2. On 200, parse events newest-first, walk OLDEST-FIRST (reverse),
//!    drop everything `<= since_iso`, map to rupu event ids.
//! 3. Cap at `limit`; advance `since_iso` to the last-emitted event's
//!    `created_at`. Update etag.
//!
//! Cursor format is `etag:<etag>|since:<rfc3339>`. `etag` may be empty
//! on first poll; `since` is always populated (set to "now" on the very
//! first call, per §15 of the spec).

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, IF_NONE_MATCH, USER_AGENT};
use serde_json::Value;
use tracing::debug;

use crate::error::{classify_scm_error, ScmError};
use crate::event_connector::{EventConnector, EventPollResult, PolledEvent};
use crate::platform::Platform;
use crate::types::RepoRef;

/// Wraps a single shared `reqwest::Client` + the user's GitHub token.
/// Multiple repos share one connector instance; each call is scoped to
/// the supplied `RepoRef`.
pub struct GithubEventConnector {
    http: reqwest::Client,
    token: String,
    /// API root, default `https://api.github.com`. Overrideable for
    /// GitHub Enterprise via `[scm.github].base_url` config.
    base_url: String,
}

impl GithubEventConnector {
    pub fn new(token: String, base_url: Option<String>) -> Self {
        let base_url = base_url.unwrap_or_else(|| "https://api.github.com".to_string());
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
impl EventConnector for GithubEventConnector {
    async fn poll_events(
        &self,
        repo: &RepoRef,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<EventPollResult, ScmError> {
        let cursor = parse_cursor(cursor);

        // First poll: emit nothing, set since = now. Avoids stampede
        // on the last 90 days of history.
        if cursor.since.is_none() && cursor.etag.is_empty() {
            let now = Utc::now();
            return Ok(EventPollResult {
                events: vec![],
                next_cursor: encode_cursor(&Cursor {
                    etag: String::new(),
                    since: Some(now),
                }),
            });
        }

        let url = format!(
            "{}/repos/{}/{}/events",
            self.base_url.trim_end_matches('/'),
            repo.owner,
            repo.repo
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("rupu/0"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|e| ScmError::Transient(anyhow::anyhow!("invalid token: {e}")))?,
        );
        if !cursor.etag.is_empty() {
            if let Ok(v) = HeaderValue::from_str(&cursor.etag) {
                headers.insert(IF_NONE_MATCH, v);
            }
        }

        let resp = self
            .http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| ScmError::Network(anyhow::anyhow!("github events GET {url}: {e}")))?;

        let status = resp.status();
        if status.as_u16() == 304 {
            // No change since last poll. Cursor unchanged except we
            // re-emit the same etag (the response also carries it but
            // there's no body to read on 304).
            return Ok(EventPollResult {
                events: vec![],
                next_cursor: encode_cursor(&cursor),
            });
        }
        if !status.is_success() {
            let resp_headers = resp.headers().clone();
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_scm_error(
                Platform::Github,
                status.as_u16(),
                &body,
                &resp_headers,
            ));
        }

        let new_etag = resp
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let raw: Vec<Value> = resp
            .json()
            .await
            .map_err(|e| ScmError::Transient(anyhow::anyhow!("github events parse: {e}")))?;

        // Reverse to oldest-first so the caller can stable-sort by arrival.
        let mut oldest_first: Vec<Value> = raw.into_iter().rev().collect();

        // Filter to events strictly newer than `since`.
        if let Some(since) = cursor.since {
            oldest_first.retain(|ev| {
                ev.get("created_at")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc) > since)
                    .unwrap_or(false)
            });
        }

        // Cap at `limit`.
        oldest_first.truncate(limit as usize);

        let mut events: Vec<PolledEvent> = Vec::with_capacity(oldest_first.len());
        let mut last_created_at: Option<DateTime<Utc>> = None;
        for raw_ev in &oldest_first {
            let Some(id) = map_github_event(raw_ev) else {
                continue;
            };
            let delivery = raw_ev
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if delivery.is_empty() {
                debug!(?raw_ev, "github event missing id field; skipping");
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
                payload: raw_ev.clone(),
            });
        }

        let new_since = last_created_at.or(cursor.since);
        let next_cursor = encode_cursor(&Cursor {
            etag: new_etag,
            since: new_since,
        });

        Ok(EventPollResult {
            events,
            next_cursor,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct Cursor {
    etag: String,
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
    let etag = fields.get("etag").copied().unwrap_or("").to_string();
    let since = fields
        .get("since")
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    Cursor { etag, since }
}

fn encode_cursor(c: &Cursor) -> String {
    match c.since {
        Some(s) => format!("etag:{}|since:{}", c.etag, s.to_rfc3339()),
        None => format!("etag:{}", c.etag),
    }
}

/// Map a raw GitHub events-API event onto the rupu vocabulary. The
/// shape is the `Event` object documented at
/// <https://docs.github.com/en/rest/activity/events>: top-level `type`
/// is the event class (`PushEvent`, `IssuesEvent`, ...); the `payload`
/// child carries the action (`opened`, `closed`, ...).
///
/// Returns `None` for events we don't (yet) recognize — those are
/// silently skipped per §6.2 of the spec.
fn map_github_event(ev: &Value) -> Option<String> {
    let kind = ev.get("type")?.as_str()?;
    let payload = ev.get("payload");
    let action = payload
        .and_then(|p| p.get("action"))
        .and_then(|v| v.as_str());

    Some(
        match (kind, action) {
            ("PushEvent", _) => "github.push",
            // Issue lifecycle.
            ("IssuesEvent", Some("opened")) => "github.issue.opened",
            ("IssuesEvent", Some("closed")) => "github.issue.closed",
            ("IssuesEvent", Some("reopened")) => "github.issue.reopened",
            ("IssuesEvent", Some("edited")) => "github.issue.edited",
            // Issue queue / categorization (the GitHub-Issues-as-queue surface).
            // Workflow authors filter by label name with `trigger.filter:
            // "{{ event.payload.label.name == 'triage' }}"`.
            ("IssuesEvent", Some("labeled")) => "github.issue.labeled",
            ("IssuesEvent", Some("unlabeled")) => "github.issue.unlabeled",
            ("IssuesEvent", Some("assigned")) => "github.issue.assigned",
            ("IssuesEvent", Some("unassigned")) => "github.issue.unassigned",
            ("IssuesEvent", Some("milestoned")) => "github.issue.milestoned",
            ("IssuesEvent", Some("demilestoned")) => "github.issue.demilestoned",
            // Comments.
            ("IssueCommentEvent", Some("created")) => "github.issue.commented",
            ("IssueCommentEvent", Some("edited")) => "github.issue.comment_edited",
            // PR lifecycle.
            ("PullRequestEvent", Some("opened")) => "github.pr.opened",
            ("PullRequestEvent", Some("reopened")) => "github.pr.reopened",
            ("PullRequestEvent", Some("edited")) => "github.pr.edited",
            ("PullRequestEvent", Some("synchronize")) => "github.pr.updated",
            ("PullRequestEvent", Some("ready_for_review")) => "github.pr.ready_for_review",
            // PR queue / review.
            ("PullRequestEvent", Some("labeled")) => "github.pr.labeled",
            ("PullRequestEvent", Some("unlabeled")) => "github.pr.unlabeled",
            ("PullRequestEvent", Some("assigned")) => "github.pr.assigned",
            ("PullRequestEvent", Some("unassigned")) => "github.pr.unassigned",
            ("PullRequestEvent", Some("review_requested")) => "github.pr.review_requested",
            ("PullRequestReviewEvent", Some("created")) => "github.pr.review_submitted",
            ("PullRequestEvent", Some("closed")) => {
                let merged = payload
                    .and_then(|p| p.get("pull_request"))
                    .and_then(|p| p.get("merged"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if merged {
                    "github.pr.merged"
                } else {
                    "github.pr.closed"
                }
            }
            _ => {
                debug!(kind, ?action, "github events: unrecognized type/action");
                return None;
            }
        }
        .to_string(),
    )
}

/// Discovery helper: build a `GithubEventConnector` from the same
/// resolver + config inputs as the Repo / Issue connectors. Returns
/// `Ok(None)` when no GitHub credential is stored.
pub async fn try_build(
    resolver: &dyn rupu_auth::CredentialResolver,
    cfg: &rupu_config::Config,
) -> anyhow::Result<Option<std::sync::Arc<dyn EventConnector>>> {
    let creds = match resolver.get("github", None).await {
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
        .get("github")
        .and_then(|p| p.base_url.clone());
    let connector: std::sync::Arc<dyn EventConnector> =
        std::sync::Arc::new(GithubEventConnector::new(token, base_url));
    Ok(Some(connector))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    fn rr() -> RepoRef {
        RepoRef {
            platform: Platform::Github,
            owner: "Section9Labs".into(),
            repo: "rupu".into(),
        }
    }

    #[test]
    fn first_poll_with_no_cursor_returns_empty_and_sets_since() {
        // Tested via the connector path in the integration test below;
        // here we just exercise cursor encoding.
        let c = Cursor {
            etag: "W/\"abc\"".into(),
            since: Some(Utc::now()),
        };
        let s = encode_cursor(&c);
        let parsed = parse_cursor(Some(&s));
        assert_eq!(parsed.etag, c.etag);
        assert_eq!(
            parsed.since.unwrap().timestamp(),
            c.since.unwrap().timestamp()
        );
    }

    #[test]
    fn map_event_handles_known_kinds() {
        use serde_json::json;
        let push = json!({"type":"PushEvent","payload":{}});
        assert_eq!(map_github_event(&push).as_deref(), Some("github.push"));

        let issue_opened = json!({"type":"IssuesEvent","payload":{"action":"opened"}});
        assert_eq!(
            map_github_event(&issue_opened).as_deref(),
            Some("github.issue.opened")
        );

        let pr_merged = json!({
            "type":"PullRequestEvent",
            "payload":{"action":"closed","pull_request":{"merged":true}}
        });
        assert_eq!(
            map_github_event(&pr_merged).as_deref(),
            Some("github.pr.merged")
        );

        let pr_closed_unmerged = json!({
            "type":"PullRequestEvent",
            "payload":{"action":"closed","pull_request":{"merged":false}}
        });
        assert_eq!(
            map_github_event(&pr_closed_unmerged).as_deref(),
            Some("github.pr.closed")
        );

        let unknown = json!({"type":"WatchEvent","payload":{"action":"started"}});
        assert!(map_github_event(&unknown).is_none());
    }

    #[test]
    fn map_event_handles_label_and_assign_actions() {
        use serde_json::json;
        let labeled = json!({
            "type":"IssuesEvent",
            "payload":{"action":"labeled","label":{"name":"triage"}}
        });
        assert_eq!(
            map_github_event(&labeled).as_deref(),
            Some("github.issue.labeled")
        );

        let unlabeled = json!({"type":"IssuesEvent","payload":{"action":"unlabeled"}});
        assert_eq!(
            map_github_event(&unlabeled).as_deref(),
            Some("github.issue.unlabeled")
        );

        let assigned = json!({"type":"IssuesEvent","payload":{"action":"assigned"}});
        assert_eq!(
            map_github_event(&assigned).as_deref(),
            Some("github.issue.assigned")
        );

        let pr_review_requested = json!({
            "type":"PullRequestEvent",
            "payload":{"action":"review_requested"}
        });
        assert_eq!(
            map_github_event(&pr_review_requested).as_deref(),
            Some("github.pr.review_requested")
        );

        let pr_ready = json!({
            "type":"PullRequestEvent",
            "payload":{"action":"ready_for_review"}
        });
        assert_eq!(
            map_github_event(&pr_ready).as_deref(),
            Some("github.pr.ready_for_review")
        );
    }

    #[tokio::test]
    async fn first_poll_returns_empty_with_warmup_cursor() {
        // No HTTP call: empty cursor short-circuits at the top of poll_events.
        let c = GithubEventConnector::new("fake".into(), Some("http://127.0.0.1:1".into()));
        let r = c.poll_events(&rr(), None, 50).await.unwrap();
        assert_eq!(r.events.len(), 0);
        // next_cursor includes a since= timestamp.
        assert!(r.next_cursor.contains("since:"));
    }
}
