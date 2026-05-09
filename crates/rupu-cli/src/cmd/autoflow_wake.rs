use anyhow::Context;
use rupu_scm::Platform;
use rupu_webhook::{WebhookEvent, WebhookSource};
use rupu_workspace::repo_dir_name;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredWebhookWakeEvent {
    pub delivery: String,
    pub event_id: String,
    pub repo_ref: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_ref: Option<String>,
    pub received_at: String,
}

pub fn store_webhook_wake_event(
    root: &Path,
    event: &StoredWebhookWakeEvent,
) -> anyhow::Result<PathBuf> {
    let dir = root.join(repo_dir_name(&event.repo_ref));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create webhook wake dir {}", dir.display()))?;
    let path = dir.join(format!("{}.json", repo_dir_name(&event.delivery)));
    if path.exists() {
        return Ok(path);
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(event)?;
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("persist webhook wake {}", path.display()))?;
    Ok(path)
}

pub fn drain_webhook_wake_events(
    root: &Path,
    repo_refs: &BTreeSet<String>,
) -> anyhow::Result<Vec<StoredWebhookWakeEvent>> {
    let mut out = Vec::new();
    for repo_ref in repo_refs {
        let dir = root.join(repo_dir_name(repo_ref));
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let record = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))
                .and_then(|body| {
                    serde_json::from_str::<StoredWebhookWakeEvent>(&body)
                        .with_context(|| format!("parse {}", path.display()))
                });
            match record {
                Ok(event) => out.push(event),
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "dropping invalid webhook wake event");
                }
            }
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(out)
}

pub fn wake_event_from_webhook(event: &WebhookEvent) -> Option<StoredWebhookWakeEvent> {
    let repo_ref = repo_ref_from_webhook_event(event)?;
    Some(StoredWebhookWakeEvent {
        delivery: delivery_key(event),
        event_id: event.event_id.clone(),
        issue_ref: issue_ref_from_webhook_event(event, &repo_ref),
        repo_ref,
        received_at: chrono::Utc::now().to_rfc3339(),
    })
}

fn delivery_key(event: &WebhookEvent) -> String {
    event.delivery_id.clone().unwrap_or_else(|| {
        format!(
            "{}-{}-{}",
            match event.source {
                WebhookSource::Github => "github",
                WebhookSource::Gitlab => "gitlab",
            },
            event.event_id,
            ulid::Ulid::new()
        )
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
    use serde_json::json;

    #[test]
    fn github_issue_webhook_maps_to_repo_and_issue_ref() {
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
        let stored = wake_event_from_webhook(&event).expect("stored event");
        assert_eq!(stored.repo_ref, "github:Section9Labs/rupu");
        assert_eq!(
            stored.issue_ref.as_deref(),
            Some("github:Section9Labs/rupu/issues/42")
        );
    }

    #[test]
    fn drain_webhook_wake_events_ignores_other_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        store_webhook_wake_event(
            root,
            &StoredWebhookWakeEvent {
                delivery: "delivery-123".into(),
                event_id: "github.issue.labeled".into(),
                repo_ref: "github:Section9Labs/rupu".into(),
                issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
                received_at: chrono::Utc::now().to_rfc3339(),
            },
        )
        .unwrap();

        let events =
            drain_webhook_wake_events(root, &BTreeSet::from(["github:Other/repo".into()])).unwrap();
        assert!(events.is_empty());

        let events =
            drain_webhook_wake_events(root, &BTreeSet::from(["github:Section9Labs/rupu".into()]))
                .unwrap();
        assert_eq!(events.len(), 1);
    }
}
