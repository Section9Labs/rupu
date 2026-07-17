//! Integration tests for `HttpHostConnector` using an in-process `MockServer`.

use futures_util::StreamExt as _;
use rupu_cp::host::{
    connector::{HostConnector, HostConnectorError, RunKind, RunListQuery},
    dashboard_summary::DashboardRange,
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
            .query_param("limit", "20")
            .query_param("host", "local");
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
        when.method("GET")
            .path("/api/runs/workflows")
            .query_param("host", "local");
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
async fn http_pause_resume_round_trip() {
    let server = httpmock::MockServer::start_async().await;
    let pause_mock = server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_p/pause");
        then.status(200)
            .json_body(serde_json::json!({"run": {"id": "run_p", "status": "paused"}}));
    });
    let resume_mock = server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_p/resume");
        then.status(200)
            .json_body(serde_json::json!({"run": {"id": "run_p", "status": "running"}}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    c.pause_run("run_p").await.unwrap();
    c.resume_run("run_p").await.unwrap();
    pause_mock.assert();
    resume_mock.assert();
}

#[tokio::test]
async fn resume_run_surfaces_launcher_gated_501() {
    // A read-only remote deploy (no `RunLauncher`) answers `/resume` with a
    // 501 — must surface as a `Remote` error, not a silent success.
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_ro/resume");
        then.status(501)
            .body("resuming a paused run requires `rupu cp serve`");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert!(matches!(
        c.resume_run("run_ro").await,
        Err(HostConnectorError::Remote(501, _))
    ));
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

#[tokio::test]
async fn info_missing_endpoint_returns_reachable_true() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(404);
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let info = c.info().await.unwrap();
    assert!(info.reachable);
    assert!(info.version.is_none());
}

#[tokio::test]
async fn proxy_get_json_forwards_path_with_bearer() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET")
            .path("/api/runs/agents")
            .query_param("limit", "5")
            .header("authorization", "Bearer tok");
        then.status(200)
            .json_body(serde_json::json!([{"run_id":"r1"}]));
    });
    let c = HttpHostConnector::new(server.base_url(), Some("tok".into()));
    let v = c.proxy_get_json("/api/runs/agents?limit=5").await.unwrap();
    assert_eq!(v[0]["run_id"], "r1");
    m.assert();
}

#[tokio::test]
async fn info_parses_version_and_capabilities() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "9.9.9",
            "capabilities": {
                "backends": ["local_worktree"],
                "scm_hosts": ["github"],
                "permission_modes": ["ask"]
            }
        }));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let info = c.info().await.unwrap();
    assert!(info.reachable);
    assert_eq!(info.version, Some("9.9.9".to_string()));
    assert_eq!(info.capabilities.backends, vec!["local_worktree"]);
    assert_eq!(info.capabilities.scm_hosts, vec!["github"]);
    assert_eq!(info.capabilities.permission_modes, vec!["ask"]);
    m.assert();
}

// ── dashboard_summary ─────────────────────────────────────────────────────────
//
// NOTE: `GET /api/dashboard` does not yet accept `?range=`/`?host=`, nor does
// it serve the `DashboardSummary` shape (Task 7 wires both) — so these tests
// stub the endpoint with `httpmock` rather than round-tripping through a real
// second `rupu cp serve` instance, following the pattern already used above
// (`launch_run_posts_with_bearer_and_returns_run_id`,
// `proxy_get_json_forwards_path_with_bearer`) for every other HTTP-connector
// method in this file.

/// A `DashboardSummary`-shaped JSON body, standing in for what Task 7's
/// `/api/dashboard` will eventually serve.
fn stub_dashboard_summary_body(captured_at: &str) -> serde_json::Value {
    serde_json::json!({
        "active": {
            "running": 2,
            "awaiting_approval": 1,
            "paused": 0,
            "pending": 0
        },
        "terminal_buckets": [
            {
                "ts": "2026-07-15T00:00:00Z",
                "completed": 3,
                "failed": 1,
                "rejected": 0,
                "cancelled": 0
            }
        ],
        "active_runs": [
            {
                "run_id": "run_remote_1",
                "workflow_name": "triage-wf",
                "status": "running",
                "started_at": "2026-07-16T00:00:00Z",
                "trigger": "manual",
                "cycle_id": null
            }
        ],
        "cycles": [],
        "recent_manual": [
            {
                "id": "run_remote_1",
                "workflow_name": "triage-wf",
                "status": "running",
                "started_at": "2026-07-16T00:00:00Z",
                "finished_at": null,
                "trigger": "manual"
            }
        ],
        "findings_open": 4,
        "captured_at": captured_at
    })
}

/// `dashboard_summary` must GET `/api/dashboard` with `host=local` (so the
/// remote scopes to its own data and a host registered on both sides is not
/// double-counted) and `range=<wire form>`, then parse the response into a
/// `DashboardSummary` whose `captured_at` is the value the remote reported —
/// never re-synthesized locally.
#[tokio::test]
async fn http_dashboard_summary_scopes_to_host_local_and_preserves_captured_at() {
    let server = httpmock::MockServer::start_async().await;
    let captured_at = "2026-07-16T12:00:00Z";
    let m = server.mock(|when, then| {
        when.method("GET")
            .path("/api/dashboard")
            .query_param("host", "local")
            .query_param("range", "30d");
        then.status(200)
            .json_body(stub_dashboard_summary_body(captured_at));
    });

    let c = HttpHostConnector::new(server.base_url(), None);
    let summary = c
        .dashboard_summary(DashboardRange::Days30)
        .await
        .expect("http host must serve dashboard_summary");

    m.assert();
    assert_eq!(
        summary.captured_at,
        captured_at
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap(),
        "captured_at must come through unchanged from the remote's response"
    );
    assert_eq!(summary.active.running, 2);
    assert_eq!(summary.active.awaiting_approval, 1);
    assert_eq!(summary.terminal_buckets.len(), 1);
    assert_eq!(summary.terminal_buckets[0].completed, 3);
    assert_eq!(summary.active_runs.len(), 1);
    assert_eq!(summary.active_runs[0].run_id, "run_remote_1");
    assert_eq!(summary.findings_open, Some(4));
}

/// Each `DashboardRange` variant maps to its wire form (`as_str()`) in the
/// proxied query string, not a serde-derived spelling.
#[tokio::test]
async fn http_dashboard_summary_range_7d_maps_to_wire_form() {
    let server = httpmock::MockServer::start_async().await;
    let m = server.mock(|when, then| {
        when.method("GET")
            .path("/api/dashboard")
            .query_param("host", "local")
            .query_param("range", "7d");
        then.status(200)
            .json_body(stub_dashboard_summary_body("2026-07-16T00:00:00Z"));
    });

    let c = HttpHostConnector::new(server.base_url(), None);
    c.dashboard_summary(DashboardRange::Days7)
        .await
        .expect("range=7d must be served by the mock");
    m.assert();
}

/// A response that does not deserialize into `DashboardSummary` must surface
/// as `HostConnectorError::Invalid`, never panic or silently produce a
/// zeroed/default summary (per the trait doc: an unreadable host is not a
/// host with no runs).
#[tokio::test]
async fn http_dashboard_summary_bad_body_maps_to_invalid() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/dashboard");
        then.status(200)
            .json_body(serde_json::json!({"not": "a summary"}));
    });

    let c = HttpHostConnector::new(server.base_url(), None);
    assert!(matches!(
        c.dashboard_summary(DashboardRange::Days30).await,
        Err(HostConnectorError::Invalid(_))
    ));
}

/// The remote CP's own local host failed to report: it still answers 200
/// with an all-zero `DashboardSummary` + a fresh `captured_at` (the
/// no-host-reported fallback `api::dashboard::get_dashboard` produces when
/// nothing reported), but its `hosts[]` array records the true state —
/// `state: "offline"`, no `"ok"` entry anywhere.
///
/// Before the fix, `dashboard_summary` parsed the flattened body as a bare
/// `DashboardSummary` and discarded `hosts[]` entirely, so this all-zero body
/// came back as `Ok(summary)` — rendering on the outer CP as "ok, live, 0
/// runs" instead of surfacing the outage. It must instead return an error
/// carrying the remote's own reason.
#[tokio::test]
async fn http_dashboard_summary_rejects_a_zeroed_body_when_no_host_reports_ok() {
    let server = httpmock::MockServer::start_async().await;
    let body = serde_json::json!({
        "hosts": [
            {
                "host_id": "local",
                "name": "local",
                "transport_kind": "local",
                "state": "offline",
                "captured_at": null,
                "reason": "run store list failed: permission denied"
            }
        ],
        "findings_partial": false,
        "active": {"running": 0, "awaiting_approval": 0, "paused": 0, "pending": 0},
        "terminal_buckets": [],
        "active_runs": [],
        "cycles": [],
        "recent_manual": [],
        "findings_open": null,
        "captured_at": "2026-07-16T12:00:00Z"
    });
    server.mock(|when, then| {
        when.method("GET").path("/api/dashboard");
        then.status(200).json_body(body);
    });

    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c
        .dashboard_summary(DashboardRange::Days30)
        .await
        .expect_err(
            "a body whose hosts[] shows no ok state must never be accepted as a real summary",
        );
    match err {
        HostConnectorError::Unreachable(msg) | HostConnectorError::Unsupported(msg) => {
            assert!(
                msg.contains("permission denied"),
                "the remote's own reason must be carried through, got: {msg}"
            );
        }
        other => panic!("expected Unreachable/Unsupported carrying the reason, got {other:?}"),
    }
}

/// `hosts[]` present, with at least one `state == "ok"` entry, must still
/// parse normally — the check only rejects when NOTHING reported ok.
#[tokio::test]
async fn http_dashboard_summary_accepts_body_when_hosts_shows_ok() {
    let server = httpmock::MockServer::start_async().await;
    let mut body = stub_dashboard_summary_body("2026-07-16T12:00:00Z");
    body["hosts"] = serde_json::json!([
        {
            "host_id": "local",
            "name": "local",
            "transport_kind": "local",
            "state": "ok",
            "captured_at": "2026-07-16T12:00:00Z",
            "reason": null
        }
    ]);
    server.mock(|when, then| {
        when.method("GET").path("/api/dashboard");
        then.status(200).json_body(body);
    });

    let c = HttpHostConnector::new(server.base_url(), None);
    let summary = c
        .dashboard_summary(DashboardRange::Days30)
        .await
        .expect("a hosts[] entry reporting ok must parse normally");
    assert_eq!(summary.active.running, 2);
}
