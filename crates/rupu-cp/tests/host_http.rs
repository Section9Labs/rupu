//! Integration tests for `HttpHostConnector` using an in-process `MockServer`.

use futures_util::StreamExt as _;
use rupu_cp::host::{
    connector::{HostConnector, HostConnectorError, RunKind, RunListQuery},
    http::HttpHostConnector,
};
use rupu_cp::launcher::LaunchRequest;

// ── From the brief (verbatim) ─────────────────────────────────────────────────

#[tokio::test]
async fn launch_run_posts_with_bearer_and_returns_run_id() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("POST")
            .path("/api/workflows/wf/run")
            .header("authorization", "Bearer tok");
        then.status(200)
            .json_body(serde_json::json!({"run_id":"run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), Some("tok".into()));
    let id = c
        .launch_run(LaunchRequest {
            workflow: "wf".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(id, "run_X");
    m.assert();
}

#[tokio::test]
async fn info_unreachable_does_not_error() {
    let c = HttpHostConnector::new("http://127.0.0.1:9".into(), None); // closed port
    let info = c.info().await.unwrap();
    assert!(!info.reachable);
}

#[tokio::test]
async fn unauthorized_maps_to_error() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/runs/run_x");
        then.status(401);
    });
    let c = HttpHostConnector::new(server.base_url(), Some("bad".into()));
    assert!(matches!(
        c.get_run("run_x").await,
        Err(HostConnectorError::Unauthorized)
    ));
}

// ── Additional tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn list_runs_all_forwards_offset_and_limit() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET")
            .path("/api/runs")
            .query_param("offset", "0")
            .query_param("limit", "20");
        then.status(200)
            .json_body(serde_json::json!([{"id": "r1", "workflow_name": "wf"}]));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let runs = c
        .list_runs(RunListQuery {
            kind: RunKind::All,
            offset: 0,
            limit: 20,
            lifecycle: None,
        })
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["id"], "r1");
    m.assert();
}

#[tokio::test]
async fn list_runs_workflow_hits_workflows_path() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET").path("/api/runs/workflows");
        then.status(200).json_body(serde_json::json!([]));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let runs = c
        .list_runs(RunListQuery {
            kind: RunKind::Workflow,
            offset: 0,
            limit: 10,
            lifecycle: None,
        })
        .await
        .unwrap();
    assert!(runs.is_empty());
    m.assert();
}

#[tokio::test]
async fn cancel_run_posts_to_cancel_endpoint() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_c/cancel");
        then.status(200)
            .json_body(serde_json::json!({"run": {"id": "run_c", "status": "cancelled"}}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    c.cancel_run("run_c").await.unwrap();
    m.assert();
}

#[tokio::test]
async fn stream_run_events_smoke() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET")
            .path("/api/events/stream")
            .query_param("run", "run_z");
        then.status(200)
            .header("content-type", "text/event-stream")
            .body("data: {\"type\":\"done\"}\n\n");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let stream = c.stream_run_events("run_z").await.unwrap();
    let chunks: Vec<_> = stream.collect().await;
    assert!(!chunks.is_empty());
    m.assert();
}

#[tokio::test]
async fn not_found_maps_to_error() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/runs/ghost");
        then.status(404);
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert!(matches!(
        c.get_run("ghost").await,
        Err(HostConnectorError::NotFound(_))
    ));
}

#[tokio::test]
async fn server_error_maps_to_remote() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/runs/run_bad");
        then.status(500).body("internal error");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert!(matches!(
        c.get_run("run_bad").await,
        Err(HostConnectorError::Remote(500, _))
    ));
}
