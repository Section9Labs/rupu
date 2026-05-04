//! End-to-end test: spin up the receiver on a free port, fire a
//! signed GitHub webhook at it, and assert the dispatcher saw the
//! workflow it expected.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use rupu_orchestrator::Workflow;
use rupu_webhook::{serve, WebhookConfig, WorkflowDispatcher};
use sha2::Sha256;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct RecordingDispatcher {
    calls: Mutex<Vec<String>>,
}
#[async_trait]
impl WorkflowDispatcher for RecordingDispatcher {
    async fn dispatch(&self, name: &str) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push(name.to_string());
        Ok(())
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

fn sign_github(secret: &[u8], body: &[u8]) -> String {
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
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher: dispatcher_handle,
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
    assert_eq!(calls, vec!["review-pr"]);
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
        workflow_loader: Arc::new(move || workflows.clone()),
        dispatcher,
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
