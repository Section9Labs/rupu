use rupu_orchestrator::{annotate_event_payload, candidate_event_ids};
use rupu_runtime::{WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeSource};
use rupu_scm::{EventSourceRef, EventSubjectRef, IssueRef, IssueTracker, Platform, PolledEvent};
use rupu_webhook::{WebhookEvent, WebhookSource};
use sha2::{Digest, Sha256};

pub fn wake_requests_from_webhook(event: &WebhookEvent) -> Option<Vec<WakeEnqueueRequest>> {
    let repo_ref = repo_ref_from_webhook_event(event)?;
    let issue_ref = issue_ref_from_webhook_event(event, &repo_ref);
    let entity = issue_ref
        .clone()
        .map(|issue_ref| WakeEntity {
            kind: WakeEntityKind::Issue,
            ref_text: issue_ref,
        })
        .unwrap_or_else(|| WakeEntity {
            kind: WakeEntityKind::Repo,
            ref_text: repo_ref.clone(),
        });
    let payload = normalized_webhook_payload(event);
    let candidate_ids = candidate_event_ids(&event.event_id, &event.payload);
    Some(
        candidate_ids
            .into_iter()
            .map(|matched_id| WakeEnqueueRequest {
                source: WakeSource::Webhook,
                repo_ref: repo_ref.clone(),
                entity: entity.clone(),
                event: WakeEvent {
                    id: matched_id.clone(),
                    delivery_id: event.delivery_id.clone(),
                    dedupe_key: Some(dedupe_key_from_webhook(event, &repo_ref, &matched_id)),
                },
                payload: Some(annotate_event_payload(
                    &payload,
                    &event.event_id,
                    &matched_id,
                )),
                received_at: chrono::Utc::now().to_rfc3339(),
                not_before: chrono::Utc::now().to_rfc3339(),
            })
            .collect(),
    )
}

pub fn wake_request_from_webhook(event: &WebhookEvent) -> Option<WakeEnqueueRequest> {
    wake_requests_from_webhook(event).and_then(|mut requests| requests.drain(..).next())
}

pub fn wake_requests_from_polled_event(event: &PolledEvent) -> Vec<WakeEnqueueRequest> {
    let Some(repo_ref) = repo_ref_from_polled_event(event) else {
        return Vec::new();
    };
    let repo = event
        .source
        .repo()
        .expect("repo_ref_from_polled_event only returns Some for repo sources");
    let issue_ref = extract_issue_ref_from_polled_event(event);
    let entity = issue_ref
        .clone()
        .map(|issue_ref| WakeEntity {
            kind: WakeEntityKind::Issue,
            ref_text: issue_ref,
        })
        .unwrap_or_else(|| WakeEntity {
            kind: WakeEntityKind::Repo,
            ref_text: repo_ref.clone(),
        });
    let payload = normalized_polled_payload(event);
    candidate_event_ids(&event.id, &event.payload)
        .into_iter()
        .map(|matched_id| WakeEnqueueRequest {
            source: WakeSource::CronPoll,
            repo_ref: repo_ref.clone(),
            entity: entity.clone(),
            event: WakeEvent {
                id: matched_id.clone(),
                delivery_id: Some(event.delivery.clone()),
                dedupe_key: Some(format!(
                    "cron_poll:{}:{}:{}:{}:{}",
                    repo.platform.as_str(),
                    repo.owner,
                    repo.repo,
                    event.delivery,
                    matched_id
                )),
            },
            payload: Some(annotate_event_payload(&payload, &event.id, &matched_id)),
            received_at: chrono::Utc::now().to_rfc3339(),
            not_before: chrono::Utc::now().to_rfc3339(),
        })
        .collect()
}

pub fn wake_request_from_polled_event(event: &PolledEvent) -> WakeEnqueueRequest {
    wake_requests_from_polled_event(event)
        .into_iter()
        .next()
        .expect("candidate_event_ids always includes canonical id")
}

pub fn extract_issue_ref_from_polled_event(event: &PolledEvent) -> Option<String> {
    if let Some(EventSubjectRef::Issue { issue }) = &event.subject {
        return Some(format_issue_ref(issue));
    }
    let repo = event.source.repo()?;
    let tracker = match repo.platform {
        Platform::Github => IssueTracker::Github,
        Platform::Gitlab => IssueTracker::Gitlab,
    };
    let project = format!("{}/{}", repo.owner, repo.repo);
    let number = match repo.platform {
        Platform::Github => {
            if !event.id.starts_with("github.issue.") {
                return None;
            }
            event
                .payload
                .get("payload")
                .and_then(|payload| payload.get("issue"))
                .and_then(|issue| issue.get("number"))
                .and_then(json_u64)
        }
        Platform::Gitlab => {
            if event.id.starts_with("gitlab.issue.") {
                event
                    .payload
                    .get("target_iid")
                    .and_then(json_u64)
                    .or_else(|| {
                        event
                            .payload
                            .get("object_attributes")
                            .and_then(|obj| obj.get("iid"))
                            .and_then(json_u64)
                    })
                    .or_else(|| {
                        event
                            .payload
                            .get("issue")
                            .and_then(|issue| issue.get("iid"))
                            .and_then(json_u64)
                    })
            } else if event.id == "gitlab.comment"
                && event.payload.get("target_type").and_then(|v| v.as_str()) == Some("Issue")
            {
                event.payload.get("target_iid").and_then(json_u64)
            } else {
                None
            }
        }
    }?;
    Some(format!("{tracker}:{project}/issues/{number}"))
}

fn dedupe_key_from_webhook(event: &WebhookEvent, repo_ref: &str, event_id: &str) -> String {
    if let Some(delivery_id) = &event.delivery_id {
        return format!("webhook:{repo_ref}:{event_id}:{delivery_id}");
    }
    let payload = serde_json::to_vec(&event.payload).unwrap_or_default();
    let digest = Sha256::digest(&payload);
    format!("webhook:{repo_ref}:{event_id}:{}", hex::encode(digest))
}

fn normalized_webhook_payload(event: &WebhookEvent) -> serde_json::Value {
    let (vendor, repo) = match event.source {
        WebhookSource::Github => (
            "github",
            event.payload.get("repository").map(|repository| {
                let owner = repository
                    .get("owner")
                    .and_then(|owner| owner.get("login").or_else(|| owner.get("name")))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let name = repository
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                serde_json::json!({
                    "full_name": format!("{owner}/{name}"),
                    "owner": owner,
                    "name": name,
                })
            }),
        ),
        WebhookSource::Gitlab => (
            "gitlab",
            event.payload.get("project").map(|project| {
                let full_name = project
                    .get("path_with_namespace")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                let (owner, name) = full_name.split_once('/').unwrap_or(("", full_name));
                serde_json::json!({
                    "full_name": full_name,
                    "owner": owner,
                    "name": name,
                })
            }),
        ),
        WebhookSource::Linear => ("linear", None),
    };
    serde_json::json!({
        "id": event.event_id,
        "vendor": vendor,
        "delivery": event.delivery_id,
        "repo": repo.unwrap_or_else(|| serde_json::json!({})),
        "payload": event.payload,
    })
}

fn normalized_polled_payload(event: &PolledEvent) -> serde_json::Value {
    let (vendor, repo_payload, source_payload) = match &event.source {
        EventSourceRef::Repo { repo } => (
            repo.platform.as_str(),
            serde_json::json!({
                "full_name": format!("{}/{}", repo.owner, repo.repo),
                "owner": repo.owner,
                "name": repo.repo,
            }),
            serde_json::json!({
                "kind": "repo",
                "vendor": repo.platform.as_str(),
                "ref": format!("{}:{}/{}", repo.platform.as_str(), repo.owner, repo.repo),
            }),
        ),
        EventSourceRef::TrackerProject { tracker, project } => (
            tracker.as_str(),
            serde_json::json!({}),
            serde_json::json!({
                "kind": "tracker_project",
                "vendor": tracker.as_str(),
                "project": project,
                "ref": format!("{}:{project}", tracker.as_str()),
            }),
        ),
    };
    let mut base = match event.payload.clone() {
        serde_json::Value::Object(map) => serde_json::Value::Object(map),
        other => serde_json::json!({ "payload": other }),
    };
    let object = base.as_object_mut().expect("object after normalization");
    object.insert("id".into(), serde_json::Value::String(event.id.clone()));
    object.insert(
        "vendor".into(),
        serde_json::Value::String(vendor.to_string()),
    );
    object.insert(
        "delivery".into(),
        serde_json::Value::String(event.delivery.clone()),
    );
    object.insert("repo".into(), repo_payload);
    object.insert("source".into(), source_payload);
    object
        .entry("payload")
        .or_insert_with(|| event.payload.clone());
    base
}

fn repo_ref_from_polled_event(event: &PolledEvent) -> Option<String> {
    let repo = event.source.repo()?;
    Some(format!(
        "{}:{}/{}",
        repo.platform.as_str(),
        repo.owner,
        repo.repo
    ))
}

fn format_issue_ref(issue: &IssueRef) -> String {
    format!(
        "{}:{}/issues/{}",
        issue.tracker.as_str(),
        issue.project,
        issue.number
    )
}

fn repo_ref_from_webhook_event(event: &WebhookEvent) -> Option<String> {
    let (platform, owner, repo) = match event.source {
        WebhookSource::Github => {
            let repo = event.payload.get("repository")?;
            let owner = repo
                .get("owner")
                .and_then(|owner| owner.get("login").or_else(|| owner.get("name")))
                .and_then(|value| value.as_str())?;
            let repo_name = repo.get("name").and_then(|value| value.as_str())?;
            (Platform::Github, owner.to_string(), repo_name.to_string())
        }
        WebhookSource::Gitlab => {
            let project = event.payload.get("project")?;
            let path = project
                .get("path_with_namespace")
                .and_then(|value| value.as_str())?;
            let (owner, repo_name) = path.split_once('/')?;
            (Platform::Gitlab, owner.to_string(), repo_name.to_string())
        }
        WebhookSource::Linear => return None,
    };
    Some(format!("{}:{owner}/{repo}", platform.as_str()))
}

fn issue_ref_from_webhook_event(event: &WebhookEvent, repo_ref: &str) -> Option<String> {
    let number = match event.source {
        WebhookSource::Github => {
            if event.event_id.starts_with("github.issue.")
                || event.event_id == "github.issue.commented"
            {
                event.payload.get("issue")?.get("number").and_then(json_u64)
            } else {
                None
            }
        }
        WebhookSource::Gitlab => {
            if event.event_id.starts_with("gitlab.issue.") {
                event
                    .payload
                    .get("object_attributes")
                    .and_then(|value| value.get("iid"))
                    .and_then(json_u64)
                    .or_else(|| {
                        event
                            .payload
                            .get("issue")
                            .and_then(|value| value.get("iid"))
                            .and_then(json_u64)
                    })
            } else if event.event_id == "gitlab.comment"
                && event
                    .payload
                    .get("target_type")
                    .and_then(|value| value.as_str())
                    == Some("Issue")
            {
                event.payload.get("target_iid").and_then(json_u64)
            } else {
                None
            }
        }
        WebhookSource::Linear => None,
    }?;
    Some(format!("{repo_ref}/issues/{number}"))
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| {
            value
                .as_i64()
                .and_then(|number| (number >= 0).then_some(number as u64))
        })
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_scm::RepoRef;
    use serde_json::json;

    #[test]
    fn github_issue_webhook_maps_to_wake_request() {
        let event = WebhookEvent {
            source: WebhookSource::Github,
            event_id: "github.issue.labeled".into(),
            delivery_id: Some("delivery-123".into()),
            payload: json!({
                "issue": { "number": 42 },
                "repository": {
                    "name": "rupu",
                    "owner": { "login": "Section9Labs" }
                }
            }),
        };
        let wake = wake_request_from_webhook(&event).expect("wake request");
        assert_eq!(wake.repo_ref, "github:Section9Labs/rupu");
        assert_eq!(wake.entity.kind, WakeEntityKind::Issue);
        assert_eq!(wake.entity.ref_text, "github:Section9Labs/rupu/issues/42");
        assert_eq!(wake.event.delivery_id.as_deref(), Some("delivery-123"));
    }

    #[test]
    fn semantic_alias_wakes_are_emitted_for_webhooks() {
        let event = WebhookEvent {
            source: WebhookSource::Github,
            event_id: "github.issue.labeled".into(),
            delivery_id: Some("delivery-123".into()),
            payload: json!({
                "issue": { "number": 42 },
                "repository": {
                    "name": "rupu",
                    "owner": { "login": "Section9Labs" }
                }
            }),
        };
        let wakes = wake_requests_from_webhook(&event).expect("wake requests");
        let ids = wakes
            .iter()
            .map(|wake| wake.event.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"github.issue.labeled"));
        assert!(ids.contains(&"github.issue.queue_changed"));
        assert!(ids.contains(&"issue.queue_entered"));
    }

    #[test]
    fn polled_issue_event_maps_to_issue_wake_request() {
        let event = PolledEvent {
            id: "github.issue.opened".into(),
            delivery: "evt-123".into(),
            source: RepoRef {
                platform: Platform::Github,
                owner: "Section9Labs".into(),
                repo: "rupu".into(),
            }
            .into(),
            subject: None,
            payload: json!({
                "payload": {
                    "issue": { "number": 42 }
                }
            }),
        };
        let wake = wake_request_from_polled_event(&event);
        assert_eq!(wake.repo_ref, "github:Section9Labs/rupu");
        assert_eq!(wake.entity.kind, WakeEntityKind::Issue);
        assert_eq!(wake.entity.ref_text, "github:Section9Labs/rupu/issues/42");
        assert_eq!(wake.event.id, "github.issue.opened");
    }

    #[test]
    fn semantic_alias_wakes_are_emitted_for_polled_events() {
        let event = PolledEvent {
            id: "github.issue.labeled".into(),
            delivery: "evt-123".into(),
            source: RepoRef {
                platform: Platform::Github,
                owner: "Section9Labs".into(),
                repo: "rupu".into(),
            }
            .into(),
            subject: None,
            payload: json!({
                "payload": {
                    "issue": { "number": 42 },
                    "action": "labeled"
                }
            }),
        };
        let wakes = wake_requests_from_polled_event(&event);
        let ids = wakes
            .iter()
            .map(|wake| wake.event.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"github.issue.labeled"));
        assert!(ids.contains(&"github.issue.queue_changed"));
        assert!(ids.contains(&"issue.queue_entered"));
    }

    #[test]
    fn tracker_scoped_polled_events_do_not_emit_repo_wakes() {
        let event = PolledEvent {
            id: "linear.issue.updated".into(),
            delivery: "evt-999".into(),
            source: EventSourceRef::TrackerProject {
                tracker: IssueTracker::Linear,
                project: "workspace-123".into(),
            },
            subject: None,
            payload: json!({
                "state": {
                    "before": { "id": "todo" },
                    "after": { "id": "in_progress" }
                }
            }),
        };
        assert!(wake_requests_from_polled_event(&event).is_empty());
    }
}
