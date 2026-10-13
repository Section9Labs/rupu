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
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "sess_a"}));
    });
    let restore_mock = server.mock(|when, then| {
        when.method("POST").path("/api/sessions/sess_a/restore");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "sess_a"}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE").path("/api/sessions/sess_a");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "sess_a"}));
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
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_a"}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE").path("/api/transcripts/run_a");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_a"}));
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
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_b"}));
    });
    let delete_mock = server.mock(|when, then| {
        when.method("DELETE")
            .path("/api/transcripts/run_b")
            .query_param("ignore_liveness", "true");
        then.status(200)
            .json_body(serde_json::json!({"ok": true, "id": "run_b"}));
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
        engagement_profiles: Vec::new(),
        skills: Vec::new(),
        preload_skills: Vec::new(),
        skill_bundle: None,
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

// ── engagement_profiles over HTTP ─────────────────────────────────────────────

fn engaged_agent_req(ids: &[&str]) -> rupu_cp::agent_launcher::AgentLaunchRequest {
    let mut req = profiled_agent_req(None);
    req.engagement_profiles = ids.iter().map(|s| s.to_string()).collect();
    req
}

/// End to end against a real remote CP: it advertises
/// `agent.engagement_profile`, takes the ids off the body and hands them to
/// its own agent launcher (which puts them on the `rupu run` argv).
#[tokio::test]
async fn launch_agent_delivers_the_engagement_to_a_real_remote_cp() {
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
        .launch_agent(engaged_agent_req(&["network", "web"]))
        .await
        .unwrap();
    assert_eq!(id, "run_REMOTE");
    let got = launcher
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("remote launched");
    assert_eq!(got.engagement_profiles, ["network", "web"]);
}

/// A remote predating the field would ignore the body key and run the unit on
/// the `code` path, so the connector refuses one that does not advertise it —
/// even when it advertises the findings-profile feature.
#[tokio::test]
async fn launch_agent_refuses_an_engagement_the_remote_does_not_advertise() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.81.0",
            "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []},
            "features": ["agent.findings_profile"]
        }));
    });
    let post = server.mock(|when, then| {
        when.method("POST").path("/api/agents/sec/run");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c
        .launch_agent(engaged_agent_req(&["network"]))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m) if m.contains("0.81.0") && m.contains("`network`")),
        "{err:?}"
    );
    post.assert_hits(0);
}

/// No engagement ⇒ the body carries no `engagement_profiles` key at all, so
/// an older remote sees exactly the request it always did.
#[tokio::test]
async fn launch_agent_without_an_engagement_posts_no_engagement_key() {
    let server = httpmock::MockServer::start_async().await;
    let post = server.mock(|when, then| {
        when.method("POST")
            .path("/api/agents/sec/run")
            .matches(|req| {
                let body: serde_json::Value =
                    serde_json::from_slice(req.body.as_deref().unwrap_or_default()).unwrap();
                body.get("engagement_profiles").is_none()
            });
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert_eq!(
        c.launch_agent(profiled_agent_req(None)).await.unwrap(),
        "run_X"
    );
    post.assert();
}

// ── workflow-step skills over HTTP ────────────────────────────────────────────

fn skilled_agent_req() -> rupu_cp::agent_launcher::AgentLaunchRequest {
    let mut req = profiled_agent_req(None);
    req.skills = vec!["pdf".into()];
    req.preload_skills = vec!["house-style".into()];
    req
}

/// End to end against a real remote CP: it advertises `agent.skills`, takes
/// the skills off the body and hands them to its own agent launcher (which
/// puts them on the `rupu run` argv).
#[tokio::test]
async fn launch_agent_delivers_the_skills_to_a_real_remote_cp() {
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
    let id = c.launch_agent(skilled_agent_req()).await.unwrap();
    assert_eq!(id, "run_REMOTE");
    let got = launcher
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("remote launched");
    assert_eq!(got.skills, ["pdf"]);
    assert_eq!(got.preload_skills, ["house-style"]);
}

/// A remote predating the field would ignore the body keys and run the unit
/// without its step's skills, so the connector refuses one that does not
/// advertise `agent.skills`, even when it advertises the other launch
/// features.
#[tokio::test]
async fn launch_agent_refuses_skills_the_remote_does_not_advertise() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.90.0",
            "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []},
            "features": ["agent.findings_profile", "agent.engagement_profile"]
        }));
    });
    let post = server.mock(|when, then| {
        when.method("POST").path("/api/agents/sec/run");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.launch_agent(skilled_agent_req()).await.unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m)
            if m.contains("0.90.0") && m.contains("skills") && m.contains("upgrade rupu")),
        "{err:?}"
    );
    post.assert_hits(0);
}

#[tokio::test]
async fn launch_agent_refuses_skills_when_the_remote_has_no_host_info() {
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
    let err = c.launch_agent(skilled_agent_req()).await.unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m)
            if m.contains("skills") && m.contains("upgrade rupu")),
        "{err:?}"
    );
    post.assert_hits(0);
}

/// No skills ⇒ no feature check and no skills keys in the body, so an older
/// remote sees exactly the request it always did.
#[tokio::test]
async fn launch_agent_without_skills_posts_no_skills_keys() {
    let server = httpmock::MockServer::start_async().await;
    let info = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(404);
    });
    let post = server.mock(|when, then| {
        when.method("POST")
            .path("/api/agents/sec/run")
            .matches(|req| {
                let body: serde_json::Value =
                    serde_json::from_slice(req.body.as_deref().unwrap_or_default()).unwrap();
                body.get("skills").is_none() && body.get("preload_skills").is_none()
            });
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

// ── W6: supplied run id + codename over HTTP ──────────────────────────────────

// ── session skills over HTTP ──────────────────────────────────────────────────

struct CapturingSessions {
    started: std::sync::Mutex<Option<rupu_cp::session_starter::SessionStartRequest>>,
    sent: std::sync::Mutex<Option<rupu_cp::session_sender::SendMessageRequest>>,
}

impl CapturingSessions {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            started: std::sync::Mutex::new(None),
            sent: std::sync::Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl rupu_cp::session_starter::SessionStarter for CapturingSessions {
    async fn start(
        &self,
        req: rupu_cp::session_starter::SessionStartRequest,
    ) -> Result<String, rupu_cp::session_starter::SessionStartError> {
        *self.started.lock().unwrap() = Some(req);
        Ok("ses_REMOTE".into())
    }
}

#[async_trait::async_trait]
impl rupu_cp::session_sender::SessionSender for CapturingSessions {
    async fn send(
        &self,
        req: rupu_cp::session_sender::SendMessageRequest,
    ) -> Result<String, rupu_cp::session_sender::SendError> {
        *self.sent.lock().unwrap() = Some(req);
        Ok("run_REMOTE".into())
    }
}

/// A real remote CP serving sessions through `sessions`, with an active
/// session `ses_1` on disk.
async fn serve_session_cp(
    tmp: &tempfile::TempDir,
    sessions: std::sync::Arc<CapturingSessions>,
) -> std::net::SocketAddr {
    let dir = tmp.path().join("sessions").join("ses_1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("session.json"),
        r#"{"session_id":"ses_1","status":"active"}"#,
    )
    .unwrap();
    serve_cp(
        cp_state(tmp)
            .with_session_starter(Some(sessions.clone()))
            .with_session_sender(Some(sessions)),
    )
    .await
}

fn skilled_session_start() -> rupu_cp::session_starter::SessionStartRequest {
    rupu_cp::session_starter::SessionStartRequest {
        agent: "triage".into(),
        prompt: Some("hi".into()),
        mode: None,
        target: None,
        working_dir: None,
        skills: vec!["pdf".into()],
        preload_skills: vec!["house-style".into()],
    }
}

fn skill_only_send(skills: &[&str]) -> rupu_cp::session_sender::SendMessageRequest {
    rupu_cp::session_sender::SendMessageRequest {
        session_id: "ses_1".into(),
        prompt: None,
        skills: skills.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn send_session_turn_delivers_skills_to_a_real_remote_cp() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = CapturingSessions::new();
    let addr = serve_session_cp(&tmp, sessions.clone()).await;

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let id = c
        .send_session_turn(skill_only_send(&["pdf"]))
        .await
        .unwrap();
    assert_eq!(id, "run_REMOTE");
    let got = sessions.sent.lock().unwrap().clone().expect("remote sent");
    assert_eq!(got.session_id, "ses_1");
    assert_eq!(got.prompt, None);
    assert_eq!(got.skills, ["pdf"]);
}

#[tokio::test]
async fn start_session_delivers_skills_to_a_real_remote_cp() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = CapturingSessions::new();
    let addr = serve_session_cp(&tmp, sessions.clone()).await;

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let id = c.start_session(skilled_session_start()).await.unwrap();
    assert_eq!(id, "ses_REMOTE");
    let got = sessions
        .started
        .lock()
        .unwrap()
        .clone()
        .expect("remote started");
    assert_eq!(got.agent, "triage");
    assert_eq!(got.skills, ["pdf"]);
    assert_eq!(got.preload_skills, ["house-style"]);
}

/// A remote predating `session.skills` would drop the keys and run the turn
/// without the skill, so the connector refuses it and nothing reaches the
/// peer. Through the coordinator's own API, the refusal is a 501 telling
/// the operator to upgrade the remote.
#[tokio::test]
#[allow(clippy::disallowed_methods)] // a test client calling the coordinator's API
async fn session_skills_are_refused_by_a_remote_without_session_skills() {
    let remote = httpmock::MockServer::start_async().await;
    remote.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.90.0",
            "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []},
            "features": ["agent.findings_profile", "agent.engagement_profile", "agent.skills"]
        }));
    });
    let send = remote.mock(|when, then| {
        when.method("POST").path("/api/sessions/ses_1/send");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let start = remote.mock(|when, then| {
        when.method("POST").path("/api/agents/triage/session");
        then.status(200)
            .json_body(serde_json::json!({"session_id": "ses_X"}));
    });

    let c = HttpHostConnector::new(remote.base_url(), None);
    for err in [
        c.send_session_turn(skill_only_send(&["pdf"]))
            .await
            .unwrap_err(),
        c.start_session(skilled_session_start()).await.unwrap_err(),
    ] {
        assert!(
            matches!(&err, HostConnectorError::Unsupported(m)
                if m.contains("0.90.0") && m.contains("skills") && m.contains("upgrade rupu")),
            "{err:?}"
        );
    }

    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    let host = state
        .hosts
        .add_host("old-remote", &remote.base_url(), None)
        .unwrap();
    let addr = serve_cp(state).await;
    let http = reqwest::Client::new();
    for (url, body) in [
        (
            format!("http://{addr}/api/sessions/ses_1/send?host={}", host.id),
            serde_json::json!({ "prompt": "", "skills": ["pdf"] }),
        ),
        (
            format!("http://{addr}/api/agents/triage/session"),
            serde_json::json!({ "host": host.id, "prompt": "hi", "skills": ["pdf"] }),
        ),
    ] {
        let resp = http.post(&url).json(&body).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::NOT_IMPLEMENTED, "{url}");
        let text = resp.text().await.unwrap();
        assert!(text.contains("upgrade rupu"), "{url}: {text}");
    }

    send.assert_hits(0);
    start.assert_hits(0);
}

/// A remote that takes session skills but whose own CLI refuses one answers
/// 400 with the CLI's message; through the coordinator that stays a 400 with
/// that message, not a gateway error wrapping the remote's JSON.
#[tokio::test]
#[allow(clippy::disallowed_methods)] // a test client calling the coordinator's API
async fn a_remote_refusing_a_session_skill_is_a_400_with_its_message() {
    let remote = httpmock::MockServer::start_async().await;
    remote.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.95.0",
            "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []},
            "features": ["agent.skills", "session.skills"]
        }));
    });
    let refusal = serde_json::json!({ "error": "unknown skill \"pdff\" (did you mean pdf?)" });
    remote.mock(|when, then| {
        when.method("POST").path("/api/sessions/ses_1/send");
        then.status(400).json_body(refusal.clone());
    });
    remote.mock(|when, then| {
        when.method("POST").path("/api/agents/triage/session");
        then.status(400).json_body(refusal.clone());
    });

    let tmp = tempfile::tempdir().unwrap();
    let state = cp_state(&tmp);
    let host = state
        .hosts
        .add_host("mini", &remote.base_url(), None)
        .unwrap();
    let addr = serve_cp(state).await;
    let http = reqwest::Client::new();
    for (url, body) in [
        (
            format!("http://{addr}/api/sessions/ses_1/send?host={}", host.id),
            serde_json::json!({ "skills": ["pdff"] }),
        ),
        (
            format!("http://{addr}/api/agents/triage/session"),
            serde_json::json!({ "host": host.id, "prompt": "hi", "skills": ["pdff"] }),
        ),
    ] {
        let resp = http.post(&url).json(&body).send().await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST, "{url}");
        let got: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            got["error"], "unknown skill \"pdff\" (did you mean pdf?)",
            "{url}"
        );
    }
}

/// No skills ⇒ no feature check and no skills key, so an older remote sees
/// exactly the send it always did.
#[tokio::test]
async fn a_session_send_without_skills_posts_no_skills_key() {
    let server = httpmock::MockServer::start_async().await;
    let info = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(404);
    });
    let send = server.mock(|when, then| {
        when.method("POST")
            .path("/api/sessions/ses_1/send")
            .json_body(serde_json::json!({ "prompt": "hi" }));
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let start = server.mock(|when, then| {
        when.method("POST")
            .path("/api/agents/triage/session")
            .matches(|req| {
                let body: serde_json::Value =
                    serde_json::from_slice(req.body.as_deref().unwrap_or_default()).unwrap();
                body.get("skills").is_none() && body.get("preload_skills").is_none()
            });
        then.status(200)
            .json_body(serde_json::json!({"session_id": "ses_X"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let mut turn = skill_only_send(&[]);
    turn.prompt = Some("hi".into());
    assert_eq!(c.send_session_turn(turn).await.unwrap(), "run_X");
    let mut req = skilled_session_start();
    req.skills.clear();
    req.preload_skills.clear();
    assert_eq!(c.start_session(req).await.unwrap(), "ses_X");
    info.assert_hits(0);
    send.assert();
    start.assert();
}

fn placed_agent_req() -> rupu_cp::agent_launcher::AgentLaunchRequest {
    let mut req = profiled_agent_req(None);
    req.run_id = Some("run_01COORD".into());
    req.codename = Some("cobalt-harbor/heron#412".into());
    req
}

/// A remote advertising `run.supplied_run_id` and `run.codename` hands both
/// to its own launcher, and the connector then says it honours the id.
#[tokio::test]
async fn launch_agent_delivers_the_run_id_and_codename_to_a_real_remote_cp() {
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
    assert!(
        !c.honours_supplied_run_id(),
        "unknown before the remote is read"
    );
    c.launch_agent(placed_agent_req()).await.unwrap();
    let got = launcher
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("remote launched");
    assert_eq!(got.run_id.as_deref(), Some("run_01COORD"));
    assert_eq!(got.codename.as_deref(), Some("cobalt-harbor/heron#412"));
    assert!(c.honours_supplied_run_id());
}

/// An older remote gets neither field (it would ignore them anyway), still
/// launches — a codename is only a display name — and the id it reports
/// is the one used.
#[tokio::test]
async fn launch_agent_to_an_older_remote_omits_the_identity_and_reads_its_id_back() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.70.0",
            "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []},
            "features": [rupu_cp::node::protocol::CAP_AGENT_FINDINGS_PROFILE]
        }));
    });
    let post = server.mock(|when, then| {
        when.method("POST")
            .path("/api/agents/sec/run")
            .matches(|req| {
                let body: serde_json::Value =
                    serde_json::from_slice(req.body.as_deref().unwrap_or_default()).unwrap();
                body.get("run_id").is_none() && body.get("codename").is_none()
            });
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_MINTED_THERE"}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    assert_eq!(
        c.launch_agent(placed_agent_req()).await.unwrap(),
        "run_MINTED_THERE"
    );
    post.assert();
    assert!(!c.honours_supplied_run_id());
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

// ── Remote findings Plan B, Task 3: artifact blobs over HTTP ─────────────────

fn store_blob(global: &std::path::Path, sha: &str, body: &[u8]) {
    let p = rupu_coverage::report::ArtifactStore::new(global.join("findings").join("artifacts"))
        .blob_path(sha);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// [`serve_cp`] over a CP whose global dir is `global`.
async fn serve_cp_at(global: &std::path::Path) -> std::net::SocketAddr {
    serve_cp(rupu_cp::state::AppState::new(
        global.to_path_buf(),
        rupu_config::PricingConfig::default(),
    ))
    .await
}

/// End to end against a real remote CP: the connector sees the feature on
/// `/api/host/info`, streams the blob into `dest`, and refuses a blob that
/// outgrows its recorded size or is absent.
#[tokio::test]
async fn pull_finding_artifact_streams_a_real_remotes_blob() {
    let remote = tempfile::tempdir().unwrap();
    let sha = "ab".repeat(32);
    store_blob(remote.path(), &sha, b"remote poc");
    let addr = serve_cp_at(remote.path()).await;

    let c = HttpHostConnector::new(format!("http://{addr}"), None);
    let dest = tempfile::tempdir().unwrap();
    let out = dest.path().join("pulled");
    c.pull_finding_artifact(&sha, &out, 10).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"remote poc");

    // Over the recorded size: refused mid-stream.
    assert!(matches!(
        c.pull_finding_artifact(&sha, &out, 3).await,
        Err(HostConnectorError::Invalid(_))
    ));
    // Absent on the remote: an actionable reason naming the blob and the
    // host, not the bare request URL.
    let absent = "cd".repeat(32);
    let err = c
        .pull_finding_artifact(&absent, &out, 10)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::NotFound(m)
            if *m == format!("artifact {absent} is not in host http://{addr}'s store")),
        "{err:?}"
    );
    // A malformed digest never reaches the wire.
    assert!(matches!(
        c.pull_finding_artifact("../x", &out, 10).await,
        Err(HostConnectorError::Invalid(_))
    ));
}

/// An older remote answers an unknown /api path with the SPA, so the
/// connector must check the feature before it trusts a status.
#[tokio::test]
async fn pull_finding_artifact_refuses_a_remote_without_the_feature() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(serde_json::json!({"version": "0.70.0", "features": []}));
    });
    let blob = server.mock(|when, then| {
        when.method("GET")
            .path_matches(httpmock::Regex::new(r"^/api/findings/artifacts/").unwrap());
        then.status(200).body("<!doctype html>");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let dest = tempfile::tempdir().unwrap();
    let out = dest.path().join("x");
    let err = c
        .pull_finding_artifact(&"ab".repeat(32), &out, 10)
        .await
        .unwrap_err();
    assert!(matches!(err, HostConnectorError::Unsupported(_)), "{err:?}");
    blob.assert_hits(0);
    assert!(!out.exists(), "a refused pull must not create dest");
}

/// A remote that errors (not 404) surfaces as `Remote`, and leaves no claim
/// of success.
#[tokio::test]
async fn pull_finding_artifact_maps_a_remote_error_status() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.70.0",
            "features": ["findings.artifact_blob"]
        }));
    });
    let sha = "ab".repeat(32);
    server.mock(|when, then| {
        when.method("GET")
            .path(format!("/api/findings/artifacts/{sha}"));
        then.status(500).body("boom");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let dest = tempfile::tempdir().unwrap();
    let err = c
        .pull_finding_artifact(&sha, &dest.path().join("x"), 10)
        .await
        .unwrap_err();
    assert!(matches!(err, HostConnectorError::Remote(500, _)), "{err:?}");
}

/// The endpoint is a security surface: every success response is a
/// `nosniff` + `sandbox` octet-stream attachment; a malformed digest is 400
/// before any path is built; a directory where a blob should be is 404.
#[tokio::test]
async fn artifact_blob_endpoint_headers_and_validation() {
    let remote = tempfile::tempdir().unwrap();
    let sha = "ab".repeat(32);
    store_blob(remote.path(), &sha, b"<script>alert(1)</script>");
    // A directory squatting on a (never written) blob's path.
    let dir_sha = "ee".repeat(32);
    let squat =
        rupu_coverage::report::ArtifactStore::new(remote.path().join("findings").join("artifacts"))
            .blob_path(&dir_sha);
    std::fs::create_dir_all(&squat).unwrap();
    // A FIFO with no writer: a blocking open would park forever.
    let fifo_sha = "ff".repeat(32);
    let fifo =
        rupu_coverage::report::ArtifactStore::new(remote.path().join("findings").join("artifacts"))
            .blob_path(&fifo_sha);
    std::fs::create_dir_all(fifo.parent().unwrap()).unwrap();
    let have_fifo = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success());
    let addr = serve_cp_at(remote.path()).await;

    #[allow(clippy::disallowed_methods)]
    let ok = reqwest::get(format!("http://{addr}/api/findings/artifacts/{sha}"))
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let h = ok.headers().clone();
    assert_eq!(h["content-type"], "application/octet-stream");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["content-security-policy"], "sandbox");
    assert!(
        h["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment"),
        "{:?}",
        h["content-disposition"]
    );
    assert_eq!(
        ok.bytes().await.unwrap().as_ref(),
        b"<script>alert(1)</script>"
    );

    // Well-formed but absent: a JSON 404.
    #[allow(clippy::disallowed_methods)]
    let absent = reqwest::get(format!(
        "http://{addr}/api/findings/artifacts/{}",
        "cd".repeat(32)
    ))
    .await
    .unwrap();
    assert_eq!(absent.status(), 404);
    let body: serde_json::Value = absent.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("not in this host's store"));

    // Not a regular file: 404, never an open attempt that could hang.
    #[allow(clippy::disallowed_methods)]
    let squatted = reqwest::get(format!("http://{addr}/api/findings/artifacts/{dir_sha}"))
        .await
        .unwrap();
    assert_eq!(squatted.status(), 404);
    if have_fifo {
        #[allow(clippy::disallowed_methods)]
        let piped = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reqwest::get(format!("http://{addr}/api/findings/artifacts/{fifo_sha}")),
        )
        .await
        .expect("a FIFO in the store must not hang the handler")
        .unwrap();
        assert_eq!(piped.status(), 404);
    }

    for bad in [
        "AB".repeat(32),      // uppercase
        "a".repeat(63),       // short
        "a".repeat(65),       // long
        "..%2Fx".to_string(), // decodes to a traversal
    ] {
        #[allow(clippy::disallowed_methods)]
        let r = reqwest::get(format!("http://{addr}/api/findings/artifacts/{bad}"))
            .await
            .unwrap();
        assert_eq!(r.status(), 400, "{bad}");
    }
}

/// A host that serves the blob endpoint says so on `/api/host/info`.
#[tokio::test]
async fn host_info_advertises_the_artifact_blob_feature() {
    let remote = tempfile::tempdir().unwrap();
    let addr = serve_cp_at(remote.path()).await;
    #[allow(clippy::disallowed_methods)]
    let info: serde_json::Value = reqwest::get(format!("http://{addr}/api/host/info"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let features: Vec<&str> = info["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    assert!(features.contains(&"findings.artifact_blob"), "{features:?}");
}

/// A remote that declares a length over the recorded size is refused before
/// anything is written.
#[tokio::test]
async fn pull_finding_artifact_refuses_a_declared_oversize_before_writing() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200).json_body(serde_json::json!({
            "version": "0.70.0",
            "features": ["findings.artifact_blob"]
        }));
    });
    let sha = "ab".repeat(32);
    server.mock(|when, then| {
        when.method("GET")
            .path(format!("/api/findings/artifacts/{sha}"));
        then.status(200).body("0123456789");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let dest = tempfile::tempdir().unwrap();
    let out = dest.path().join("x");
    let err = c.pull_finding_artifact(&sha, &out, 4).await.unwrap_err();
    assert!(matches!(err, HostConnectorError::Invalid(_)), "{err:?}");
    assert!(
        !out.exists(),
        "an oversize declaration must not create dest"
    );
}

// ── Shipped skills over HTTP ──────────────────────────────────────────────────

use rupu_cp::host::skill_ship::{manifest_of, ShipOutcome, SkillBundle};

/// Skills installed on the coordinator, under `dir`.
fn ship_bundle(dir: &std::path::Path, names: &[&str]) -> SkillBundle {
    SkillBundle::new(
        names
            .iter()
            .map(|n| {
                let root = dir.join(n);
                std::fs::create_dir_all(&root).unwrap();
                std::fs::write(
                    root.join("SKILL.md"),
                    format!("---\nname: {n}\ndescription: {n} skill.\n---\n\nDo {n}.\n"),
                )
                .unwrap();
                std::sync::Arc::new(
                    rupu_skills::load_skill(&root, rupu_skills::Layer::Global).unwrap(),
                )
            })
            .collect(),
    )
}

fn host_info_with(features: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "version": "0.95.0",
        "capabilities": {"backends": [], "scm_hosts": [], "permission_modes": []},
        "features": features,
    })
}

#[tokio::test]
async fn ship_skills_downgrades_when_the_peer_lacks_the_feature() {
    let tmp = tempfile::tempdir().unwrap();
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(host_info_with(&["agent.skills"]));
    });
    let missing = server.mock(|when, then| {
        when.method("POST").path("/api/skills/bundle/missing");
        then.status(200)
            .json_body(serde_json::json!({"missing": []}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let out = c
        .ship_skills(&ship_bundle(tmp.path(), &["pdf"]))
        .await
        .unwrap();
    assert!(
        matches!(&out, ShipOutcome::Unsupported(m)
            if m.contains("shipped skills") && m.contains("0.95.0")),
        "{out:?}"
    );
    missing.assert_hits(0);
}

#[tokio::test]
async fn ship_skills_puts_only_what_the_peer_lacks_once() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = ship_bundle(tmp.path(), &["pdf", "sql"]);
    let packed = bundle.pack().unwrap();
    let lacking = packed[1].reference();
    let server = httpmock::MockServer::start_async().await;
    let info = server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(host_info_with(&["agent.skill_bundle"]));
    });
    let asked = serde_json::json!({
        "skills": packed.iter().map(|p| p.reference()).collect::<Vec<_>>()
    });
    let missing = server.mock(|when, then| {
        when.method("POST")
            .path("/api/skills/bundle/missing")
            .json_body(asked.clone());
        then.status(200)
            .json_body(serde_json::json!({"missing": [lacking.clone()]}));
    });
    let put = server.mock(|when, then| {
        when.method("PUT")
            .path(format!("/api/skills/bundle/{}", lacking.content_hash))
            .query_param("layer", "global")
            .query_param("name", "sql")
            .body(String::from_utf8_lossy(&packed[1].bytes).to_string());
        then.status(200)
            .json_body(serde_json::json!({"stored": lacking.content_hash.clone()}));
    });
    let c = HttpHostConnector::new(server.base_url(), None);

    let out = c.ship_skills(&bundle).await.unwrap();
    assert_eq!(out, ShipOutcome::Shipped(manifest_of(&packed)));
    put.assert_hits(1);

    assert_eq!(c.ship_skills(&bundle).await.unwrap(), out);
    info.assert_hits(1);
    missing.assert_hits(1);
    put.assert_hits(1);
}

#[tokio::test]
async fn ship_skills_fails_on_a_put_the_peer_refuses_and_retries_later() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = ship_bundle(tmp.path(), &["pdf"]);
    let lacking = bundle.pack().unwrap()[0].reference();
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(host_info_with(&["agent.skill_bundle"]));
    });
    let missing = server.mock(|when, then| {
        when.method("POST").path("/api/skills/bundle/missing");
        then.status(200)
            .json_body(serde_json::json!({"missing": [lacking]}));
    });
    server.mock(|when, then| {
        when.method("PUT");
        then.status(400).body("hash mismatch");
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.ship_skills(&bundle).await.unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Remote(400, m)
            if m.contains("hash mismatch") && m.contains(&server.base_url())),
        "{err:?}"
    );
    let _ = c.ship_skills(&bundle).await;
    missing.assert_hits(2);
}

/// End to end against a real remote CP: the skill lands in its cache, and the
/// launch that follows hands the manifest to the remote's agent launcher.
#[tokio::test]
async fn shipped_skills_reach_a_real_remote_cp_and_ride_the_launch() {
    let coord = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let launcher = std::sync::Arc::new(CapturingAgentLauncher {
        last: std::sync::Mutex::new(None),
    });
    let addr = serve_cp(cp_state(&remote).with_agent_launcher(Some(launcher.clone()))).await;
    let c = HttpHostConnector::new(format!("http://{addr}"), None);

    let bundle = ship_bundle(coord.path(), &["pdf"]);
    let ShipOutcome::Shipped(manifest) = c.ship_skills(&bundle).await.unwrap() else {
        panic!("a current remote takes shipped skills")
    };
    let r = &manifest.skills[0];
    let cached = rupu_skills::ship::cached_path(&rupu_skills::ship::cache_dir(remote.path()), r);
    assert!(cached.join("SKILL.md").is_file(), "{}", cached.display());

    let mut req = profiled_agent_req(None);
    req.skill_bundle = Some(manifest.clone());
    assert_eq!(c.launch_agent(req).await.unwrap(), "run_REMOTE");
    let got = launcher
        .last
        .lock()
        .unwrap()
        .clone()
        .expect("remote launched");
    assert_eq!(got.skill_bundle, Some(manifest));
}

/// A remote predating the field would drop `skill_bundle` and run the unit
/// without its shipped skills, so the launch is refused.
#[tokio::test]
async fn launch_agent_refuses_a_skill_bundle_the_remote_does_not_advertise() {
    let server = httpmock::MockServer::start_async().await;
    server.mock(|when, then| {
        when.method("GET").path("/api/host/info");
        then.status(200)
            .json_body(host_info_with(&["agent.skills"]));
    });
    let post = server.mock(|when, then| {
        when.method("POST").path("/api/agents/sec/run");
        then.status(200)
            .json_body(serde_json::json!({"run_id": "run_X"}));
    });
    let mut req = profiled_agent_req(None);
    req.skill_bundle = Some(rupu_skills::ship::SkillBundleManifest {
        version: 1,
        skills: Vec::new(),
    });
    let c = HttpHostConnector::new(server.base_url(), None);
    let err = c.launch_agent(req).await.unwrap_err();
    assert!(
        matches!(&err, HostConnectorError::Unsupported(m) if m.contains("shipped skills")),
        "{err:?}"
    );
    post.assert_hits(0);
}
