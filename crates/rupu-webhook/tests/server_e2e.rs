//! End-to-end test: spin up the receiver on a free port, fire a
//! signed GitHub webhook at it, and assert the dispatcher saw the
//! workflow it expected.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rupu_orchestrator::Workflow;
use rupu_webhook::{serve, DispatchOutcome, WebhookConfig, WorkflowDispatcher};
use sha2::Sha256;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct RecordingDispatcher {
    calls: Mutex<Vec<(String, serde_json::Value)>>,
}
#[async_trait]
impl WorkflowDispatcher for RecordingDispatcher {
    async fn dispatch(
        &self,
        name: &str,
        event: &serde_json::Value,
    ) -> anyhow::Result<DispatchOutcome> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_string(), event.clone()));
        Ok(DispatchOutcome::default())
    }
}

fn pick_free_port() -> u16 {
    // Bind to port 0, ask the kernel which port we got, drop the
    // socket. Tiny race between drop + the receiver bind, but fine
    // for a test.
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn parse(s: &str) -> Workflow {
    Workflow::parse(s).expect("workflow parse")
}

fn pr_opened_workflow() -> (String, Workflow) {
    let yaml = r#"name: review-pr
trigger:
  on: event
  event: github.pr.opened
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
"#;
    ("review-pr".into(), parse(yaml))
}

fn issue_state_workflow() -> (String, Workflow) {
    let yaml = r#"name: linear-state
trigger:
  on: event
  event: issue.entered_workflow_state
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
"#;
    ("linear-state".into(), parse(yaml))
}

fn jira_state_workflow() -> (String, Workflow) {
    let yaml = r#"name: jira-state
trigger:
  on: event
  event: issue.entered_workflow_state
steps:
  - id: a
    agent: a
    actions: []
    prompt: hi
"#;
    ("jira-state".into(), parse(yaml))
}

fn sign_github(secret: &[u8], body: &[u8]) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap();
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(digest))
}

fn sign_linear(secret: &[u8], body: &[u8]) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn sign_jira(secret: &[u8], body: &[u8]) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret).unwrap();
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(digest))
}

#[tokio::test]
async fn signed_pr_opened_dispatches_matching_workflow() {
    let secret = b"test-secret-for-rupu-webhook-receiver";
    let port = pick_free_port();
    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, port).into();

    let dispatcher = Arc::new(RecordingDispatcher {
        calls: Mutex::new(Vec::new()),
    });
    let dispatcher_handle = dispatcher.clone();

    let workflows = vec![pr_opened_workflow()];
    let config = WebhookConfig {
        addr,
        github_secret: Some(secret.to_vec()),
        gitlab_token: None,
        linear_secret: None,
        jira_secret: None,
        github_projects_hydrator: None,
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher: dispatcher_handle,
        observer: None,
    };
    let server = tokio::spawn(async move {
        let _ = serve(config).await;
    });

    // Wait for the listener to come up. Poll /healthz with a short
    // backoff rather than sleep.
    let url_health = format!("http://{addr}/healthz");
    for _ in 0..50 {
        if reqwest::get(&url_health).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let body = serde_json::json!({
        "action": "opened",
        "pull_request": { "number": 42, "merged": false },
        "repository": { "name": "rupu" }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let sig = sign_github(secret, &body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/github"))
        .header("x-github-event", "pull_request")
        .header("x-hub-signature-256", &sig)
        .header("content-type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["event"], "github.pr.opened");
    assert_eq!(json["fired"][0]["name"], "review-pr");
    assert_eq!(json["fired"][0]["fired"], true);

    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "review-pr");
    // The dispatcher must receive the verbatim vendor JSON so step
    // prompts and `when:` filters can resolve `{{event.*}}`.
    assert_eq!(calls[0].1["pull_request"]["number"], 42);
    assert_eq!(calls[0].1["repository"]["name"], "rupu");
    server.abort();
}

#[tokio::test]
async fn unsigned_request_returns_401() {
    let port = pick_free_port();
    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, port).into();
    let dispatcher = Arc::new(RecordingDispatcher {
        calls: Mutex::new(Vec::new()),
    });
    let workflows = vec![pr_opened_workflow()];
    let config = WebhookConfig {
        addr,
        github_secret: Some(b"k".to_vec()),
        gitlab_token: None,
        linear_secret: None,
        jira_secret: None,
        github_projects_hydrator: None,
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher,
        observer: None,
    };
    let server = tokio::spawn(async move {
        let _ = serve(config).await;
    });
    for _ in 0..50 {
        if reqwest::get(format!("http://{addr}/healthz")).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/github"))
        .header("x-github-event", "pull_request")
        // intentionally no x-hub-signature-256
        .body("{}")
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 401);
    server.abort();
}

#[tokio::test]
async fn signed_linear_issue_update_dispatches_matching_workflow() {
    let secret = b"linear-secret";
    let port = pick_free_port();
    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, port).into();

    let dispatcher = Arc::new(RecordingDispatcher {
        calls: Mutex::new(Vec::new()),
    });
    let dispatcher_handle = dispatcher.clone();

    let workflows = vec![issue_state_workflow()];
    let config = WebhookConfig {
        addr,
        github_secret: None,
        gitlab_token: None,
        linear_secret: Some(secret.to_vec()),
        jira_secret: None,
        github_projects_hydrator: None,
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher: dispatcher_handle,
        observer: None,
    };
    let server = tokio::spawn(async move {
        let _ = serve(config).await;
    });

    let url_health = format!("http://{addr}/healthz");
    for _ in 0..50 {
        if reqwest::get(&url_health).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let body = serde_json::json!({
        "action": "update",
        "type": "Issue",
        "url": "https://linear.app/acme/issue/ENG-123",
        "data": {
            "id": "issue-1",
            "identifier": "ENG-123",
            "stateId": "state-in-progress",
            "projectId": "project-core",
            "cycleId": "cycle-42",
            "teamId": "team-1"
        },
        "updatedFrom": {
            "stateId": "state-todo"
        },
        "webhookTimestamp": ts,
        "webhookId": "delivery-1"
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let sig = sign_linear(secret, &body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/linear"))
        .header("linear-event", "Issue")
        .header("linear-signature", &sig)
        .header("content-type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["event"], "linear.issue.updated");
    assert_eq!(json["fired"][0]["name"], "linear-state");
    assert_eq!(json["fired"][0]["fired"], true);

    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "linear-state");
    assert_eq!(calls[0].1["subject"]["ref"], "ENG-123");
    assert_eq!(calls[0].1["state"]["category"], "workflow_state");
    assert_eq!(calls[0].1["state"]["before"]["id"], "state-todo");
    assert_eq!(calls[0].1["state"]["after"]["id"], "state-in-progress");
    server.abort();
}

#[tokio::test]
async fn signed_jira_issue_update_dispatches_matching_workflow() {
    let secret = b"jira-secret";
    let port = pick_free_port();
    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, port).into();

    let dispatcher = Arc::new(RecordingDispatcher {
        calls: Mutex::new(Vec::new()),
    });
    let dispatcher_handle = dispatcher.clone();

    let workflows = vec![jira_state_workflow()];
    let config = WebhookConfig {
        addr,
        github_secret: None,
        gitlab_token: None,
        linear_secret: None,
        jira_secret: Some(secret.to_vec()),
        github_projects_hydrator: None,
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher: dispatcher_handle,
        observer: None,
    };
    let server = tokio::spawn(async move {
        let _ = serve(config).await;
    });

    let url_health = format!("http://{addr}/healthz");
    for _ in 0..50 {
        if reqwest::get(&url_health).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let body = serde_json::json!({
        "timestamp": 1731430163000u64,
        "webhookEvent": "jira:issue_updated",
        "user": { "accountId": "user-1", "displayName": "Matt" },
        "issue": {
            "id": "10001",
            "self": "https://acme.atlassian.net/rest/api/3/issue/10001",
            "key": "ENG-123",
            "fields": {
                "project": { "id": "10000", "key": "ENG", "name": "Engineering" },
                "issuetype": { "id": "10004", "name": "Task" }
            }
        },
        "changelog": {
            "items": [
                {
                    "field": "status",
                    "fieldId": "status",
                    "from": "3",
                    "fromString": "To Do",
                    "to": "4",
                    "toString": "Ready For Review"
                }
            ]
        }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let sig = sign_jira(secret, &body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/jira"))
        .header("x-atlassian-webhook-event", "jira:issue_updated")
        .header("x-atlassian-webhook-identifier", "delivery-1")
        .header("x-hub-signature", &sig)
        .header("content-type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["event"], "jira.issue.updated");
    assert_eq!(json["fired"][0]["name"], "jira-state");
    assert_eq!(json["fired"][0]["fired"], true);

    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "jira-state");
    assert_eq!(calls[0].1["subject"]["ref"], "ENG-123");
    assert_eq!(calls[0].1["state"]["category"], "workflow_state");
    assert_eq!(calls[0].1["state"]["before"]["name"], "To Do");
    assert_eq!(calls[0].1["state"]["after"]["name"], "Ready For Review");
    server.abort();
}

#[tokio::test]
async fn signed_github_projects_item_dispatches_matching_workflow() {
    let secret = b"test-secret-for-rupu-webhook-receiver";
    let port = pick_free_port();
    let addr: SocketAddr = (Ipv4Addr::LOCALHOST, port).into();

    let dispatcher = Arc::new(RecordingDispatcher {
        calls: Mutex::new(Vec::new()),
    });
    let dispatcher_handle = dispatcher.clone();

    let workflows = vec![issue_state_workflow()];
    let config = WebhookConfig {
        addr,
        github_secret: Some(secret.to_vec()),
        gitlab_token: None,
        linear_secret: None,
        jira_secret: None,
        github_projects_hydrator: None,
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher: dispatcher_handle,
        observer: None,
    };
    let server = tokio::spawn(async move {
        let _ = serve(config).await;
    });

    let url_health = format!("http://{addr}/healthz");
    for _ in 0..50 {
        if reqwest::get(&url_health).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let body = serde_json::json!({
        "action": "edited",
        "organization": { "login": "Section9Labs" },
        "projects_v2": { "node_id": "PVT_kwDOA", "title": "Delivery" },
        "projects_v2_item": {
            "id": "PVTI_lADOA",
            "project_node_id": "PVT_kwDOA",
            "content_type": "Issue",
            "content": {
                "__typename": "Issue",
                "node_id": "I_kwDOA",
                "number": 42,
                "html_url": "https://github.com/Section9Labs/rupu/issues/42",
                "repository": { "full_name": "Section9Labs/rupu" }
            },
            "field_value": {
                "field_type": "single_select",
                "optionId": "opt-ready",
                "name": "Ready For Review",
                "field": { "name": "Status" }
            }
        },
        "changes": {
            "field_value": {
                "field_type": "single_select",
                "optionId": "opt-progress",
                "name": "In Progress",
                "field": { "name": "Status" }
            }
        }
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let sig = sign_github(secret, &body_bytes);

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/webhook/github"))
        .header("x-github-event", "projects_v2_item")
        .header("x-github-delivery", "delivery-projects-1")
        .header("x-hub-signature-256", sig)
        .header("content-type", "application/json")
        .body(body_bytes)
        .send()
        .await
        .expect("post");
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(json["event"], "github.project_item.updated");
    assert_eq!(json["fired"][0]["name"], "linear-state");
    assert_eq!(json["fired"][0]["fired"], true);

    let calls = dispatcher.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "linear-state");
    assert_eq!(
        calls[0].1["subject"]["ref"],
        "github:Section9Labs/rupu/issues/42"
    );
    assert_eq!(calls[0].1["state"]["category"], "workflow_state");
    assert_eq!(calls[0].1["state"]["before"]["name"], "In Progress");
    assert_eq!(calls[0].1["state"]["after"]["name"], "Ready For Review");
    server.abort();
}
