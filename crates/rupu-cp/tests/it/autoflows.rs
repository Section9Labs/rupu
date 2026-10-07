//! Tests for GET /api/autoflows — autoflow *definitions* (workflows with
//! `autoflow.enabled = true`).  Distinct from `/api/runs/autoflows` in
//! run_streams.rs which returns execution history.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

async fn spawn_server(dir: &std::path::Path) -> std::net::SocketAddr {
    let state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Minimal valid workflow YAML with a manual trigger and NO autoflow block.
const MANUAL_WF_YAML: &str = r#"
name: manual-wf
steps:
  - id: step1
    agent: test-agent
    actions: []
    prompt: "do something"
"#;

/// Workflow with `autoflow.enabled = true` — classifies as autoflow-enabled.
const AUTOFLOW_WF_YAML: &str = r#"
name: cron-wf
autoflow:
  enabled: true
  entity: issue
steps:
  - id: step1
    agent: test-agent
    actions: []
    prompt: "do something autonomously"
"#;

#[tokio::test]
async fn list_autoflows_returns_only_autoflow_enabled_workflow() {
    let tmp = tempfile::tempdir().unwrap();
    let wf_dir = tmp.path().join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();

    std::fs::write(wf_dir.join("manual_wf.yaml"), MANUAL_WF_YAML).unwrap();
    std::fs::write(wf_dir.join("cron_wf.yaml"), AUTOFLOW_WF_YAML).unwrap();

    let addr = spawn_server(tmp.path()).await;
    let resp = reqwest::get(format!("http://{addr}/api/autoflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "expected 200 OK");

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");

    assert_eq!(
        arr.len(),
        1,
        "expected only the autoflow-enabled workflow; got {arr:?}"
    );

    let row = &arr[0];
    assert_eq!(row["name"], "cron-wf", "wrong workflow name");
    assert_eq!(row["scope"], "global", "scope should be global");
    // trigger kind for a workflow with no `trigger:` block defaults to "manual";
    // the autoflow predicate is `autoflow.enabled = true`, not trigger kind.
    assert!(
        row["trigger"].is_string(),
        "trigger field should be a string"
    );
}

#[tokio::test]
async fn list_autoflows_missing_dir_returns_empty_array() {
    let tmp = tempfile::tempdir().unwrap();
    // Do NOT create a workflows directory.
    let addr = spawn_server(tmp.path()).await;

    let resp = reqwest::get(format!("http://{addr}/api/autoflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    assert!(arr.is_empty(), "expected empty array for missing dir");
}

#[tokio::test]
async fn list_autoflows_skips_unparseable_yaml() {
    let tmp = tempfile::tempdir().unwrap();
    let wf_dir = tmp.path().join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();

    std::fs::write(wf_dir.join("bad.yaml"), "not: valid: yaml: {{{{").unwrap();
    std::fs::write(wf_dir.join("cron_wf.yaml"), AUTOFLOW_WF_YAML).unwrap();

    let addr = spawn_server(tmp.path()).await;
    let resp = reqwest::get(format!("http://{addr}/api/autoflows"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().expect("body should be a JSON array");
    // bad.yaml is skipped; only cron_wf survives
    assert_eq!(arr.len(), 1, "expected 1 autoflow, got {arr:?}");
    assert_eq!(arr[0]["name"], "cron-wf");
}

/// Any launcher: the CP's "this is `cp serve`" signal, which toggling needs.
struct NoopLauncher;

#[async_trait::async_trait]
impl rupu_cp::launcher::RunLauncher for NoopLauncher {
    async fn launch(
        &self,
        _req: rupu_cp::launcher::LaunchRequest,
    ) -> Result<String, rupu_cp::launcher::LaunchError> {
        Ok("run_unused".into())
    }
}

/// An autoflow saved as `.yml` is listed (the runtime runs it), so its toggle
/// and its workflow link must resolve it too — both used to 404.
#[tokio::test]
async fn a_yml_autoflow_toggles_and_links() {
    let tmp = tempfile::tempdir().unwrap();
    let wf = tmp.path().join("workflows");
    std::fs::create_dir_all(&wf).unwrap();
    let body = "name: nightly\nautoflow:\n  enabled: true\nsteps:\n  - id: s1\n    agent: ag\n    actions: []\n    prompt: p\n";
    std::fs::write(wf.join("nightly.yml"), body).unwrap();

    let state =
        rupu_cp::state::AppState::new(tmp.path().into(), rupu_config::PricingConfig::default())
            .with_launcher(Some(std::sync::Arc::new(NoopLauncher)));
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();

    let list: serde_json::Value = client
        .get(format!("http://{addr}/api/autoflows"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.to_string().contains("nightly"), "{list}");

    let resp = client
        .post(format!("http://{addr}/api/autoflows/nightly/disable"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let on_disk = std::fs::read_to_string(wf.join("nightly.yml")).unwrap();
    assert!(on_disk.contains("enabled: false"), "{on_disk}");

    let resp = client
        .get(format!("http://{addr}/api/workflows/nightly"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
