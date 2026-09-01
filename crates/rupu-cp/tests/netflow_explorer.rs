//! End-to-end tests for `GET /api/netflow/explorer` and the cross-filter
//! params on the flows-list routes — the wiring the explorer surface
//! stands on. Aggregation SEMANTICS (facet skip-own-dimension, dense
//! buckets, observed-only byte sums, unknown grouping) are unit-tested in
//! `rupu_netflow::ledger::explorer`; what these prove is the read-side
//! composition: ledger flows reach the aggregates tagged with the right
//! run/workflow attribution, the same scope resolution as the flows
//! routes, and the same whole-history `dropped_total` / server `window`
//! echo contracts (mirrors `tests/netflow_api.rs`).

// Throwaway in-process HTTP client hitting our own spawned test server,
// not rupu's own egress (mirrors `tests/netflow_api.rs`).
#![allow(clippy::disallowed_methods)]

use rupu_netflow::{
    Fidelity, FlowCtx, FlowId, FlowRecord, LedgerLine, NetflowPaths, Origin, Outcome,
};
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};

fn flow(ts_secs: i64, host: &str, ok: bool) -> FlowRecord {
    FlowRecord {
        id: FlowId::new(),
        ts: chrono::DateTime::from_timestamp(ts_secs, 0).unwrap(),
        ctx: FlowCtx {
            run_id: None,
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        fidelity: Fidelity::Http,
        method: "POST".into(),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: "/v1/messages".into(),
        peer_ip: None,
        resolved_ips: vec![],
        http_version: None,
        status: Some(if ok { 200 } else { 500 }),
        outcome: if ok { Outcome::Ok } else { Outcome::HttpError },
        error: None,
        bytes_out: Some(10),
        bytes_in: Some(20),
        body_complete: true,
        ttfb_ms: None,
        duration_ms: Some(30),
    }
}

fn write_ledger(workspace: &std::path::Path, run_id: &str, flows: &[FlowRecord]) {
    let paths = NetflowPaths::for_run(&workspace.join(".rupu/netflow"), run_id);
    paths.ensure_dir().unwrap();
    let body: String = flows
        .iter()
        .map(|f| {
            format!(
                "{}\n",
                serde_json::to_string(&LedgerLine::Flow(Box::new(f.clone()))).unwrap()
            )
        })
        .collect();
    std::fs::write(&paths.flows, body).unwrap();
}

fn seed_run(id: &str, workflow: &str, workspace: &std::path::Path) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: workflow.into(),
        status: RunStatus::Completed,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: "ws_explorer".into(),
        workspace_path: workspace.to_path_buf(),
        transcript_dir: workspace.join(".rupu/transcripts"),
        started_at: chrono::DateTime::from_timestamp(50, 0).unwrap(),
        finished_at: Some(chrono::DateTime::from_timestamp(2_000, 0).unwrap()),
        error_message: None,
        awaiting: Vec::new(),
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        resume_gate_id: None,
        resume_approver: None,
        reject_cleanup_pending: None,
        permission_mode: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
        backend_id: None,
        worker_id: None,
        artifact_manifest_path: None,
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
        final_output: None,
        loop_progress: Default::default(),
    }
}

fn new_state(global_dir: &std::path::Path) -> rupu_cp::state::AppState {
    rupu_cp::state::AppState::new(global_dir.into(), rupu_config::PricingConfig::default())
}

async fn serve(state: rupu_cp::state::AppState) -> std::net::SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// One project with: two flows in `run-a`'s ledger (its run record names
/// workflow `review-wf`) and one flow in `orphan-run`'s ledger (no run
/// record anywhere). Returns `(global tempdir, project tempdir)`; the
/// caller keeps both alive.
fn seed_project(global: &tempfile::TempDir, project: &tempfile::TempDir) {
    write_ledger(
        project.path(),
        "run-a",
        &[
            flow(100, "api.anthropic.com", true),
            flow(900, "api.anthropic.com", false),
        ],
    );
    write_ledger(
        project.path(),
        "orphan-run",
        &[flow(500, "iptoasn.com", true)],
    );

    let run_store = RunStore::new(global.path().join("runs"));
    run_store
        .create(
            seed_run("run-a", "review-wf", project.path()),
            "name: review-wf\n",
        )
        .unwrap();

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    rupu_workspace::upsert(&ws_store, project.path()).unwrap();
}

#[tokio::test]
async fn global_explorer_attributes_workflows_and_groups_unknowns() {
    let global = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    seed_project(&global, &project);

    let addr = serve(new_state(global.path())).await;
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/netflow/explorer"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Workflow attribution: the ledger FILE id `run-a` maps through the
    // run record to `review-wf`; `orphan-run` has no record and lands in
    // the explicit unknown bucket, never dropped.
    let workflows = body["sankey"]["workflows"].as_array().unwrap();
    let by_id = |id: &str| {
        workflows
            .iter()
            .find(|n| n["id"] == id)
            .unwrap_or_else(|| panic!("no {id} node in {body}"))
            .clone()
    };
    assert_eq!(by_id("review-wf")["calls"], 2, "{body}");
    assert_eq!(by_id("unknown")["calls"], 1, "{body}");
    assert_eq!(by_id("unknown")["label"], "No workflow");

    // Every fixture flow has `peer_ip: None`, so regardless of whether
    // the machine running this test happens to have a real ASN table at
    // its `RUPU_HOME` (deliberately not asserted — `asn_loaded` is
    // environment-dependent here, mirroring `tests/netflow_api.rs`'s
    // avoidance), every flow groups under the explicit unknown org.
    let orgs = body["sankey"]["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 1, "{body}");
    assert_eq!(orgs[0]["id"], "unknown");
    assert_eq!(orgs[0]["label"], "Unknown network");

    assert_eq!(body["kpis"]["flows"], 3);
    assert_eq!(body["kpis"]["errors"], 1);
    // Unfiltered request: the window echo is positively null on both
    // sides, and the histogram carries the server-chosen full bounds.
    assert!(body["window"]["from"].is_null());
    assert!(body["window"]["to"].is_null());
    assert!(body["histogram"]["from"].as_str().is_some(), "{body}");
    let bucket_calls: u64 = body["histogram"]["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["calls"].as_u64().unwrap())
        .sum();
    assert_eq!(bucket_calls, 3);

    // Timeline: one lane per endpoint, dense buckets, active-runs strip
    // sees the seeded run's span.
    let lanes = body["timeline"]["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 2, "{body}");
    assert_eq!(body["timeline"]["runs_in_window"], 1, "{body}");
}

#[tokio::test]
async fn explorer_window_narrows_kpis_but_never_the_histogram() {
    let global = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    seed_project(&global, &project);

    let addr = serve(new_state(global.path())).await;
    let body: serde_json::Value = reqwest::get(format!(
        "http://{addr}/api/netflow/explorer?from=1970-01-01T00:13:00Z&to=1970-01-01T00:16:00Z"
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(
        body["kpis"]["flows"], 1,
        "only the t=900 flow is inside the window: {body}"
    );
    let bucket_calls: u64 = body["histogram"]["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["calls"].as_u64().unwrap())
        .sum();
    assert_eq!(
        bucket_calls, 3,
        "the activity strip stays full-history for context: {body}"
    );
    assert_eq!(body["window"]["from"], "1970-01-01T00:13:00Z");
    // The sankey node universe is scope-wide; the out-of-window flows'
    // nodes stay present with zeroed calls for dimming, never removed.
    let workflows = body["sankey"]["workflows"].as_array().unwrap();
    let unknown = workflows.iter().find(|n| n["id"] == "unknown").unwrap();
    assert_eq!(unknown["calls"], 0, "{body}");
}

#[tokio::test]
async fn explorer_run_scope_reads_that_runs_ledger_only() {
    let global = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    seed_project(&global, &project);

    let addr = serve(new_state(global.path())).await;
    let body: serde_json::Value = reqwest::get(format!(
        "http://{addr}/api/netflow/explorer?scope=run:run-a"
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    assert_eq!(body["kpis"]["flows"], 2, "{body}");
    let workflows = body["sankey"]["workflows"].as_array().unwrap();
    assert_eq!(workflows.len(), 1, "only this run's workflow: {body}");
    assert_eq!(workflows[0]["id"], "review-wf");
    assert_eq!(
        body["timeline"]["runs_in_window"], 1,
        "the run's own span drives the strip: {body}"
    );
}

#[tokio::test]
async fn flows_list_cross_filters_server_side_and_attributes_rows() {
    let global = tempfile::TempDir::new().unwrap();
    let project = tempfile::TempDir::new().unwrap();
    seed_project(&global, &project);

    let addr = serve(new_state(global.path())).await;

    // Unfiltered: rows at global scope carry run + workflow attribution.
    let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 3, "{body}");
    let attributed = flows
        .iter()
        .find(|f| f["host"] == "api.anthropic.com")
        .unwrap();
    assert_eq!(attributed["run_id"], "run-a", "{body}");
    assert_eq!(attributed["workflow"], "review-wf", "{body}");
    let orphan = flows.iter().find(|f| f["host"] == "iptoasn.com").unwrap();
    assert!(
        orphan.get("run_id").is_none(),
        "unattributable rows omit the field rather than inventing one: {body}"
    );

    // workflow filter: server-side narrowing, hosts recomputed from the
    // RETAINED set only (never the unfiltered rollup).
    let body: serde_json::Value =
        reqwest::get(format!("http://{addr}/api/netflow?workflow=review-wf"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    assert_eq!(body["flows"].as_array().unwrap().len(), 2, "{body}");
    let hosts = body["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 1, "{body}");
    assert_eq!(hosts[0]["host"], "api.anthropic.com");
    assert_eq!(
        body["dropped_total"], 0,
        "dropped stays whole-history, untouched by filters: {body}"
    );

    // The explicit unknown key selects the unattributable rows.
    let body: serde_json::Value =
        reqwest::get(format!("http://{addr}/api/netflow?workflow=unknown"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 1, "{body}");
    assert_eq!(flows[0]["host"], "iptoasn.com");

    // host filter matches the exact `host:port` endpoint key.
    let body: serde_json::Value = reqwest::get(format!(
        "http://{addr}/api/netflow?host=api.anthropic.com:443"
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["flows"].as_array().unwrap().len(), 2, "{body}");
}
