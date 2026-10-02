//! Integration tests for `HttpHostConnector` using an in-process `MockServer`.

use futures_util::StreamExt as _;
use rupu_cp::host::{
    connector::{HostConnector, HostConnectorError, RunKind, RunListQuery},
    dashboard_summary::DashboardRange,
    http::HttpHostConnector,
};
use rupu_cp::launcher::LaunchRequest;

/// A fresh in-process CP state rooted in `tmp` (default pricing, no launchers).
fn cp_state(tmp: &tempfile::TempDir) -> rupu_cp::state::AppState {
    rupu_cp::state::AppState::new(
        tmp.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    )
}

/// Serve `state`'s real router (no bearer) on an ephemeral loopback port and
/// return the address; the server task lives for the rest of the test.
async fn serve_cp(state: rupu_cp::state::AppState) -> std::net::SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

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
async fn http_archive_restore_delete_run_round_trip() {
    let server = httpmock::MockServer::start_async().await;
    let archive_mock = server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_a/archive");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_a", "archived": true}));
    });
    let restore_mock = server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_a/restore");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_a", "archived": false}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE").path("/api/runs/run_a");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_a", "deleted": true}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    c.archive_run("run_a").await.unwrap();
    c.restore_run("run_a").await.unwrap();
    c.delete_run("run_a").await.unwrap();
    archive_mock.assert();
    restore_mock.assert();
    delete_mock.assert();
}

#[tokio::test]
async fn http_delete_run_not_found_maps_to_error() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("DELETE").path("/api/runs/run_ghost");
        then.status(404);
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert!(matches!(
        c.delete_run("run_ghost").await,
        Err(HostConnectorError::NotFound(_))
    ));
}

#[tokio::test]
async fn http_archive_run_non_terminal_maps_to_remote_conflict() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("POST").path("/api/runs/run_running/archive");
        then.status(409).body("run run_running is not terminal");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert!(matches!(
        c.archive_run("run_running").await,
        Err(HostConnectorError::Remote(409, _))
    ));
}

#[tokio::test]
async fn http_archive_restore_delete_session_round_trip() {
    let server = httpmock::MockServer::start_async().await;
    let archive_mock = server.mock(|when, then| {
        when.method("POST").path("/api/sessions/sess_a/archive");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "sess_a"}));
    });
    let restore_mock = server.mock(|when, then| {
        when.method("POST").path("/api/sessions/sess_a/restore");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "sess_a"}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE").path("/api/sessions/sess_a");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "sess_a"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    c.archive_session("sess_a").await.unwrap();
    c.restore_session("sess_a").await.unwrap();
    c.delete_session("sess_a").await.unwrap();
    archive_mock.assert();
    restore_mock.assert();
    delete_mock.assert();
}

#[tokio::test]
async fn http_archive_delete_transcript_round_trip() {
    let server = httpmock::MockServer::start_async().await;
    let archive_mock = server.mock(|when, then| {
        when.method("POST").path("/api/transcripts/run_a/archive");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "run_a"}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE").path("/api/transcripts/run_a");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "run_a"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    c.archive_transcript("run_a", false).await.unwrap();
    c.delete_transcript("run_a", false).await.unwrap();
    archive_mock.assert();
    delete_mock.assert();
}

// PID-reuse escape hatch: `ignore_liveness: true` must be forwarded as
// `?ignore_liveness=true` on both the archive and delete requests.
#[tokio::test]
async fn http_archive_delete_transcript_ignore_liveness_query_param() {
    let server = httpmock::MockServer::start_async().await;
    let archive_mock = server.mock(|when, then| {
        when.method("POST")
            .path("/api/transcripts/run_b/archive")
            .query_param("ignore_liveness", "true");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "run_b"}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE")
            .path("/api/transcripts/run_b")
            .query_param("ignore_liveness", "true");
        then.status(200).json_body(serde_json::json!({"ok": true, "id": "run_b"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    c.archive_transcript("run_b", true).await.unwrap();
    c.delete_transcript("run_b", true).await.unwrap();
    archive_mock.assert();
    delete_mock.assert();
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
        "active_longest": {
            "run_id": "run_remote_1",
            "workflow_name": "triage-wf",
            "age_ms": 45_000
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
        "throughput_buckets": [
            {
                "ts": "2026-07-15T00:00:00Z",
                "manual": 2,
                "cron": 1,
                "event": 0
            }
        ],
        "cycles": {
            "total": 4,
            "clean": 3,
            "with_failures": 1
        },
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
    assert_eq!(summary.throughput_buckets.len(), 1);
    assert_eq!(summary.throughput_buckets[0].manual, 2);
    assert_eq!(
        summary.active_longest.as_ref().map(|a| a.run_id.as_str()),
        Some("run_remote_1")
    );
    assert_eq!(summary.cycles.total, 4);
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
        "cycles_partial": false,
        "active": {"running": 0, "awaiting_approval": 0, "paused": 0, "pending": 0},
        "terminal_buckets": [],
        "throughput_buckets": [],
        "cycles": {"total": 0, "clean": null, "with_failures": null},
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

// ── findings_profile over HTTP ────────────────────────────────────────────────

fn profiled_agent_req(
    profile: Option<rupu_coverage::FindingProfile>,
) -> rupu_cp::agent_launcher::AgentLaunchRequest {
    rupu_cp::agent_launcher::AgentLaunchRequest {
        agent: "sec".into(),
        prompt: Some("audit".into()),
        mode: None,
        target: None,
        working_dir: None,
        run_id: None,
        findings_profile: profile,
        codename: None,
    }
}

struct CapturingAgentLauncher {
    last: std::sync::Mutex<Option<rupu_cp::agent_launcher::AgentLaunchRequest>>,
}

#[async_trait::async_trait]
impl rupu_cp::agent_launcher::AgentLauncher for CapturingAgentLauncher {
    async fn launch(
        &self,
        req: rupu_cp::agent_launcher::AgentLaunchRequest,
    ) -> Result<String, rupu_cp::agent_launcher::AgentLaunchError> {
        *self.last.lock().unwrap() = Some(req);
        Ok("run_REMOTE".into())
    }
}

/// End to end against a real remote CP: the coordinator's connector sees the
/// remote advertise the feature on `/api/host/info`, posts the profile, and
/// the remote hands it to its own agent launcher (which puts it on the
/// `rupu run` argv — `cp_agent_launcher`'s tests cover that last hop).
#[tokio::test]
async fn launch_agent_delivers_the_findings_profile_to_a_real_remote_cp() {
    let tmp = tempfile::tempdir().unwrap();
    let launcher = std::sync::Arc::new(CapturingAgentLauncher {
        last: std::sync::Mutex::new(None),
    });
    let state = rupu_cp::state::AppState::new(
        tmp.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    )
    .with_agent_launcher(Some(launcher.clone()));
    let addr = serve_cp(state).await;

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let id = c
        .launch_agent(profiled_agent_req(Some(
            rupu_coverage::FindingProfile::Summary,
        )))
        .await
        .unwrap();
    assert_eq!(id, "run_REMOTE");
    let got = launcher
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("remote launched");
    assert_eq!(
        got.findings_profile,
        Some(rupu_coverage::FindingProfile::Summary)
    );
    assert_eq!(got.agent, "sec");
}

/// A remote predating the field ignores unknown body keys, so the connector
/// must not post a profile to one that does not advertise the feature.
#[tokio::test]
async fn launch_agent_refuses_a_profile_when_the_remote_does_not_advertise_it() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.70.0",
            "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []}
        }));
    });
    let post = server.mock(|when, then| {
        when.method("POST").path("/api/agents/sec/run");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c
        .launch_agent(profiled_agent_req(Some(
            rupu_coverage::FindingProfile::Full,
        )))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m) if m.contains("0.70.0") && m.contains("`full`")),
        "{err:?}"
    );
    post.assert_hits(0);
}

#[tokio::test]
async fn launch_agent_refuses_a_profile_when_the_remote_has_no_host_info() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(404);
    });
    let post = server.mock(|when, then| {
        when.method("POST").path("/api/agents/sec/run");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c
        .launch_agent(profiled_agent_req(Some(
            rupu_coverage::FindingProfile::Summary,
        )))
        .await
        .unwrap_err();
    assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err:?}");
    post.assert_hits(0);
}

/// No profile ⇒ no pre-check: an old remote keeps working exactly as before.
#[tokio::test]
async fn launch_agent_without_a_profile_skips_the_feature_check() {
    let server = httpmock::MockServer::start_async().await;
    let info = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(404);
    });
    let post = server.mock(|when, then| {
        when.method("POST").path("/api/agents/sec/run");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert_eq!(
        c.launch_agent(profiled_agent_req(None)).await.unwrap(),
        "run_X"
    );
    info.assert_hits(0);
    post.assert();
}

// ── Remote findings Plan A, Task 6: coverage stream over HTTP ────────────────

/// A real remote CP serves a run's stream; the connector fetches it.
#[tokio::test]
async fn unit_coverage_fetches_the_remote_runs_stream() {
    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    let p = rupu_coverage::stream_path(&state.run_store.root, "run_H1");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        b"{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"run_H1\"}\n",
    )
    .unwrap();
    let addr = serve_cp(state).await;

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let read = c.unit_coverage("run_H1").await.unwrap();
    assert!(read.bytes.starts_with(b"{\"ledger\":\"begin\""));
    assert!(read.complete, "the remote serves its own file: complete");
    assert!(c.unit_coverage("run_NONE").await.unwrap().bytes.is_empty());
}

/// An older remote answers unknown /api paths with the SPA (200 HTML), so the
/// connector must check the feature first rather than trust the status.
#[tokio::test]
async fn unit_coverage_refuses_a_remote_without_the_feature() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(serde_json::json!({"version": "0.70.0", "features": []}));
    });
    let spa = server.mock(|when, then| {
        when.method("GET").path("/api/runs/run_H1/coverage");
        then.status(200).body("<!doctype html>");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.unit_coverage("run_H1").await.unwrap_err();
    assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err:?}");
    spa.assert_hits(0);
}

#[tokio::test]
async fn unmatched_api_paths_are_a_json_404_not_the_spa() {
    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    let addr = serve_cp(state).await;
    #[allow(clippy::disallowed_methods)]
    let resp = reqwest::get(format!("http://{addr}/api/definitely/not/a/route"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("/api/definitely"));
}

/// The `/api` boundary: the bare prefix and its trailing-slash form are API
/// paths (JSON 404); a look-alike sibling such as `/apiary` is not, and still
/// gets the SPA fallback.
#[tokio::test]
async fn api_prefix_boundary_is_a_json_404_but_a_lookalike_path_gets_the_spa() {
    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    let addr = serve_cp(state).await;
    for path in ["/api", "/api/"] {
        #[allow(clippy::disallowed_methods)]
        let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
        assert_eq!(resp.status(), 404, "{path} must be a JSON 404");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("no API route"),
            "{path}: {body}"
        );
    }
    #[allow(clippy::disallowed_methods)]
    let resp = reqwest::get(format!("http://{addr}/apiary")).await.unwrap();
    assert_eq!(resp.status(), 200, "/apiary is a client route, not the API");
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.starts_with("text/html"), "SPA fallback, got {ct}");
}

#[tokio::test]
async fn coverage_endpoint_serves_ndjson_and_rejects_a_malformed_id() {
    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    let p = rupu_coverage::stream_path(&state.run_store.root, "run_H2");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(
        &p,
        b"{\"ledger\":\"begin\",\"v\":1,\"run_id\":\"run_H2\"}\n",
    )
    .unwrap();
    let addr = serve_cp(state).await;

    #[allow(clippy::disallowed_methods)]
    let ok = reqwest::get(format!("http://{addr}/api/runs/run_H2/coverage"))
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(
        ok.headers().get("content-type").unwrap(),
        "application/x-ndjson"
    );
    // A run that wrote none: 200, empty body.
    #[allow(clippy::disallowed_methods)]
    let none = reqwest::get(format!("http://{addr}/api/runs/run_NONE/coverage"))
        .await
        .unwrap();
    assert_eq!(none.status(), 200);
    assert!(none.bytes().await.unwrap().is_empty());
    #[allow(clippy::disallowed_methods)]
    let bad = reqwest::get(format!("http://{addr}/api/runs/not-a-run/coverage"))
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

/// A stream that exists but cannot be read is the server's problem, not the
/// client's: 500, not the 400 a malformed id earns.
#[tokio::test]
async fn coverage_endpoint_reports_an_unreadable_stream_as_a_server_error() {
    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    // `coverage.jsonl` is a directory: it exists, `read` fails, NotFound it is not.
    let p = rupu_coverage::stream_path(&state.run_store.root, "run_H3");
    std::fs::create_dir_all(&p).unwrap();
    let addr = serve_cp(state).await;

    #[allow(clippy::disallowed_methods)]
    let unreadable = reqwest::get(format!("http://{addr}/api/runs/run_H3/coverage"))
        .await
        .unwrap();
    assert_eq!(unreadable.status(), 500);
    #[allow(clippy::disallowed_methods)]
    let missing = reqwest::get(format!("http://{addr}/api/runs/run_NONE/coverage"))
        .await
        .unwrap();
    assert_eq!(
        missing.status(),
        200,
        "no stream written is empty, not an error"
    );
    #[allow(clippy::disallowed_methods)]
    let malformed = reqwest::get(format!("http://{addr}/api/runs/run_a.b/coverage"))
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
}

// ── Older-remote feature gate (Plan A review triage, G) ──────────────────────

/// A remote older than `/api/host/info` answers it with its SPA — HTML with a
/// 200. The feature gate must fail closed with `Unsupported`, never leak the
/// decode failure as `Remote(0, ..)`, and never go on to fetch the stream.
#[tokio::test]
async fn unit_coverage_refuses_a_remote_whose_host_info_is_the_spa() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .header("content-type", "text/html")
            .body("<!doctype html><html></html>");
    });
    let stream = server.mock(|when, then| {
        when.method("GET").path("/api/runs/run_H1/coverage");
        then.status(200).body("{}\n");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.unit_coverage("run_H1").await.unwrap_err();
    // The distinguishing text: this is the "answered with something that is
    // not host info" refusal (carrying the parse error), NOT the 404 branch's
    // "predates /api/host/info" one.
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m)
            if m.contains("did not answer /api/host/info with host info")
                && m.contains("expected value")
                && m.contains("this unit's coverage cannot be collected")
                && !m.contains("predates")),
        "{err:?}"
    );
    stream.assert_hits(0);
}

/// A 200 that IS JSON but not host info keeps its parse error in the refusal —
/// it must not be misreported as "not JSON".
#[tokio::test]
async fn unit_coverage_refusal_for_wrong_shaped_host_info_names_the_parse_error() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!([1, 2, 3]));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.unit_coverage("run_H1").await.unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m)
            if m.contains("did not answer /api/host/info with host info")
                && m.contains("invalid type")),
        "{err:?}"
    );
}

/// `info()` on a remote whose `/api/host/info` answers with its SPA (200 HTML)
/// is the same outcome as its 404: reachable, version unknown — not an error
/// (which `probe_remote` would render as "offline").
#[tokio::test]
async fn info_on_a_spa_host_info_reply_is_reachable_with_unknown_version() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .header("content-type", "text/html")
            .body("<!doctype html><html></html>");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let info = c.info().await.unwrap();
    assert!(info.reachable);
    assert!(info.version.is_none());
}

/// A remote with no `/api/host/info` route at all (404) fails the same way.
#[tokio::test]
async fn unit_coverage_refuses_a_remote_with_no_host_info_route() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(404);
    });
    let stream = server.mock(|when, then| {
        when.method("GET").path("/api/runs/run_H1/coverage");
        then.status(200).body("{}\n");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.unit_coverage("run_H1").await.unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m) if m.contains("predates /api/host/info")),
        "{err:?}"
    );
    stream.assert_hits(0);
}
