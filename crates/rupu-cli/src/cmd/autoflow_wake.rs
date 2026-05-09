use rupu_runtime::{WakeEnqueueRequest, WakeEntity, WakeEntityKind, WakeEvent, WakeSource};
use rupu_scm::{IssueTracker, Platform, PolledEvent};
use rupu_webhook::{WebhookEvent, WebhookSource};
use sha2::{Digest, Sha256};

pub fn wake_request_from_webhook(event: &WebhookEvent) -> Option<WakeEnqueueRequest> {
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
    Some(WakeEnqueueRequest {
        source: WakeSource::Webhook,
        repo_ref: repo_ref.clone(),
        entity,
        event: WakeEvent {
            id: event.event_id.clone(),
            delivery_id: event.delivery_id.clone(),
            dedupe_key: Some(dedupe_key_from_webhook(event, &repo_ref)),
        },
        payload: Some(normalized_webhook_payload(event)),
        received_at: chrono::Utc::now().to_rfc3339(),
        not_before: chrono::Utc::now().to_rfc3339(),
    })
}

pub fn wake_request_from_polled_event(event: &PolledEvent) -> WakeEnqueueRequest {
    let repo_ref = format!(
        "{}:{}/{}",
        event.repo.platform.as_str(),
        event.repo.owner,
        event.repo.repo
    );
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
    WakeEnqueueRequest {
        source: WakeSource::CronPoll,
        repo_ref,
        entity,
        event: WakeEvent {
            id: event.id.clone(),
            delivery_id: Some(event.delivery.clone()),
            dedupe_key: Some(format!(
                "cron_poll:{}:{}:{}:{}",
                event.repo.platform.as_str(),
                event.repo.owner,
                event.repo.repo,
                event.delivery
            )),
        },
        payload: Some(normalized_polled_payload(event)),
        received_at: chrono::Utc::now().to_rfc3339(),
        not_before: chrono::Utc::now().to_rfc3339(),
    }
}

pub fn extract_issue_ref_from_polled_event(event: &PolledEvent) -> Option<String> {
    let tracker = match event.repo.platform {
        Platform::Github => IssueTracker::Github,
        Platform::Gitlab => IssueTracker::Gitlab,
    };
    let project = format!("{}/{}", event.repo.owner, event.repo.repo);
    let number = match event.repo.platform {
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

fn dedupe_key_from_webhook(event: &WebhookEvent, repo_ref: &str) -> String {
    if let Some(delivery_id) = &event.delivery_id {
        return format!("webhook:{repo_ref}:{}:{delivery_id}", event.event_id);
    }
    let payload = serde_json::to_vec(&event.payload).unwrap_or_default();
    let digest = Sha256::digest(&payload);
    format!(
        "webhook:{repo_ref}:{}:{}",
        event.event_id,
        hex::encode(digest)
    )
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
    serde_json::json!({
        "id": event.id,
        "vendor": event.repo.platform.as_str(),
        "delivery": event.delivery,
        "repo": {
            "full_name": format!("{}/{}", event.repo.owner, event.repo.repo),
            "owner": event.repo.owner,
            "name": event.repo.repo,
        },
        "payload": event.payload,
    })
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
    fn polled_issue_event_maps_to_issue_wake_request() {
        let event = PolledEvent {
            id: "github.issue.opened".into(),
            delivery: "evt-123".into(),
            repo: RepoRef {
                platform: Platform::Github,
                owner: "Section9Labs".into(),
                repo: "rupu".into(),
            },
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
}
