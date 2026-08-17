//! The API must never invent data. A Coarse record's unknown byte count
//! stays null through serialization; a missing ASN table is reported as
//! `asn_loaded: false` rather than silently producing empty enrichment.
//!
//! The end-to-end tests below also prove Plan 3's ledger+transcript merge
//! correction over the real HTTP route: only *provider* flows carry
//! `ctx.run_id`, so a run's SCM/auth/update calls are `run_id: None` in the
//! ledger and only recoverable from the run's own transcript. See
//! `rupu-cp/src/api/netflow.rs`'s module doc for the full rationale.
//!
//! ## One ledger file per run, not one shared `flows.jsonl`
//!
//! `NetflowPaths::for_run(netflow_dir, run_id)` names each run's ledger
//! `<netflow_dir>/<run_id>.jsonl` (the netflow-per-run plan). `write_ledger`
//! below always writes to one such file; most fixtures below call it once
//! per run they want to exist so a run-scoped read only ever sees its own
//! file, matching production. `run_scope_filters_to_that_run_only` is the
//! one exception — it exercises `filter_by_run` (a pure function, still
//! exported, no longer called by any production read path) directly against
//! a hand-mixed in-memory set, so it deliberately writes multiple runs into
//! one file; that is NOT a claim about how production lays ledgers out.

// Throwaway in-process HTTP client hitting our own spawned test server, not
// rupu's own egress (mirrors `tests/run_graph.rs`).
#![allow(clippy::disallowed_methods)]

use rupu_netflow::{
    Fidelity, FlowCtx, FlowId, FlowRecord, LedgerLine, NetflowPaths, Origin, Outcome,
};

fn flow(run: Option<&str>, host: &str, fidelity: Fidelity, peer: Option<&str>) -> FlowRecord {
    FlowRecord {
        id: FlowId::new(),
        ts: chrono::Utc::now(),
        ctx: FlowCtx {
            run_id: run.map(str::to_string),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        fidelity,
        method: "POST".into(),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: "/v1/messages".into(),
        peer_ip: peer.map(|p| p.parse().unwrap()),
        resolved_ips: vec![],
        http_version: None,
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: if fidelity == Fidelity::Coarse {
            None
        } else {
            Some(10)
        },
        bytes_in: if fidelity == Fidelity::Coarse {
            None
        } else {
            Some(20)
        },
        body_complete: true,
        ttfb_ms: None,
        duration_ms: Some(30),
    }
}

/// Write `lines` to `run_id`'s own ledger file under `workspace`'s netflow
/// directory (`NetflowPaths::for_run`), creating the directory (and its
/// self-ignoring `.gitignore`) first. Returns the `NetflowPaths` so callers
/// don't have to re-derive the same path a second time to read it back.
fn write_ledger(workspace: &std::path::Path, run_id: &str, lines: &[LedgerLine]) -> NetflowPaths {
    let paths = NetflowPaths::for_run(&workspace.join(".rupu/netflow"), run_id);
    paths.ensure_dir().unwrap();
    let body: String = lines
        .iter()
        .map(|l| format!("{}\n", serde_json::to_string(l).unwrap()))
        .collect();
    std::fs::write(&paths.flows, body).unwrap();
    paths
}

#[test]
fn run_scope_filters_to_that_run_only() {
    // See the module doc: this exercises `filter_by_run` directly against a
    // hand-mixed set, not the per-run-file routing a real run-scoped read
    // relies on today (that's `run_netflow_route_merges_ledger_and_
    // transcript_dedupes_and_excludes_other_runs`, below). The run id
    // passed to `write_ledger` here is therefore just a label for this one
    // fixture file, not a claim that production ever mixes runs together.
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = write_ledger(
        tmp.path(),
        "mixed-fixture",
        &[
            LedgerLine::Flow(Box::new(flow(
                Some("run-a"),
                "api.anthropic.com",
                Fidelity::Http,
                Some("1.1.1.1"),
            ))),
            LedgerLine::Flow(Box::new(flow(
                Some("run-b"),
                "api.github.com",
                Fidelity::Http,
                None,
            ))),
            LedgerLine::Flow(Box::new(flow(None, "iptoasn.com", Fidelity::Http, None))),
        ],
    );

    let all = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    let scoped = rupu_cp::api::netflow::filter_by_run(&all, "run-a");

    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].host, "api.anthropic.com");
}

#[test]
fn coarse_unknown_bytes_serialize_as_null_not_zero() {
    let f = flow(Some("r"), "api.github.com", Fidelity::Coarse, None);
    let view = rupu_cp::api::netflow::FlowView::from_flow(f, None);
    let json = serde_json::to_value(&view).unwrap();

    assert_eq!(json["fidelity"], "coarse");
    assert!(
        json["bytes_in"].is_null(),
        "unknown must stay null — 0 would read as a fact"
    );
    assert!(json["asn"].is_null());
}

#[test]
fn enrichment_attaches_asn_when_the_table_resolves_the_peer() {
    use std::io::Cursor;
    let table = rupu_netflow::AsnTable::compact_from_tsv(Cursor::new(
        "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n",
    ))
    .unwrap();

    let f = flow(
        Some("r"),
        "api.anthropic.com",
        Fidelity::Http,
        Some("1.0.0.7"),
    );
    let view = rupu_cp::api::netflow::FlowView::from_flow(f, Some(&table));

    let asn = view.asn.unwrap();
    assert_eq!(asn.asn, 13335);
    assert_eq!(asn.org, "CLOUDFLARENET");
}

#[test]
fn a_flow_with_no_peer_ip_gets_no_asn() {
    use std::io::Cursor;
    let table = rupu_netflow::AsnTable::compact_from_tsv(Cursor::new(
        "1.0.0.0\t1.0.0.255\t13335\tUS\tCLOUDFLARENET\n",
    ))
    .unwrap();

    let f = flow(Some("r"), "api.github.com", Fidelity::Coarse, None);
    let view = rupu_cp::api::netflow::FlowView::from_flow(f, Some(&table));
    assert!(view.asn.is_none());
}

#[test]
fn dropped_lines_are_reported_not_swallowed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = write_ledger(
        tmp.path(),
        "run-with-drops",
        &[LedgerLine::Dropped {
            count: 12,
            ts: chrono::Utc::now(),
        }],
    );
    assert_eq!(
        rupu_netflow::ledger::read_dropped_total(&paths.flows).unwrap(),
        12
    );
}

// ── End-to-end: the real HTTP route ─────────────────────────────────────

use chrono::Utc;
use rupu_orchestrator::runs::{RunRecord, RunStatus, StepKind, StepResultRecord};
use rupu_transcript::{Event as TxEvent, JsonlWriter};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn seed_run(id: &str, workspace_path: PathBuf) -> RunRecord {
    RunRecord {
        id: id.into(),
        workflow_name: "wf".into(),
        status: RunStatus::Completed,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws".into(),
        transcript_dir: workspace_path.join(".rupu").join("transcripts"),
        workspace_path,
        started_at: Utc::now(),
        finished_at: Some(Utc::now()),
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

fn seed_step(run_id: &str, transcript_path: PathBuf) -> StepResultRecord {
    StepResultRecord {
        run_outcome: None,
        step_id: "s1".into(),
        run_id: run_id.into(),
        transcript_path,
        output: "done".into(),
        success: true,
        skipped: false,
        rendered_prompt: "prompt".into(),
        kind: StepKind::Linear,
        items: Vec::new(),
        findings: Vec::new(),
        iterations: 0,
        resolved: true,
        finished_at: Utc::now(),
        loop_iteration: None,
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

fn e2e_flow(id: FlowId, run: Option<&str>, host: &str, origin: Origin) -> FlowRecord {
    FlowRecord {
        id,
        ts: chrono::Utc::now(),
        ctx: FlowCtx {
            run_id: run.map(str::to_string),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin,
        },
        fidelity: Fidelity::Http,
        method: "POST".into(),
        scheme: "https".into(),
        host: host.into(),
        port: 443,
        path: "/x".into(),
        peer_ip: None,
        resolved_ips: vec![],
        http_version: None,
        status: Some(200),
        outcome: Outcome::Ok,
        error: None,
        bytes_out: Some(1),
        bytes_in: Some(2),
        body_complete: true,
        ttfb_ms: None,
        duration_ms: Some(5),
    }
}

/// The full arc: `run_id`'s own ledger file has this run's finalized flow
/// (A); `other_run_id` has its own SEPARATE ledger file with a different
/// run's flow that must never be read for this request at all (not merely
/// filtered out — proving the route only ever opens `run_id`'s own file).
/// The run's transcript has a stale pre-completion copy of A (must lose to
/// the ledger's finalized copy) and a `run_id: None` SCM flow (C) that a
/// ledger read alone would never see. Response must contain exactly A
/// (finalized) + C, report `run_id`'s own ledger's dropped-count, and roll
/// `hosts` up from that same merged set (not leaking the other run's host).
#[tokio::test]
async fn run_netflow_route_merges_ledger_and_transcript_dedupes_and_excludes_other_runs() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let run_id = "run_e2e";
    let other_run_id = "run_other";

    let id_a = FlowId::new();
    let ledger_flow_a = {
        let mut f = e2e_flow(
            id_a,
            Some(run_id),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        );
        f.bytes_in = Some(4096); // finalized (folded Complete)
        f
    };
    let ledger_flow_other = e2e_flow(
        FlowId::new(),
        Some(other_run_id),
        "api.other.example",
        Origin::Provider("anthropic".into()),
    );

    // `run_id`'s own ledger file: flow A plus this run's dropped-count.
    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(ledger_flow_a.clone())),
            LedgerLine::Dropped {
                count: 3,
                ts: chrono::Utc::now(),
            },
        ],
    );
    // `other_run_id`'s own, separate ledger file — the route under test
    // must never open this file for a `run_id` request.
    write_ledger(
        project.path(),
        other_run_id,
        &[LedgerLine::Flow(Box::new(ledger_flow_other))],
    );

    let id_c = FlowId::new();
    let stale_a = {
        let mut f = ledger_flow_a.clone();
        f.bytes_in = None;
        f.body_complete = false;
        f
    };
    let scm_flow_c = e2e_flow(id_c, None, "api.github.com", Origin::Scm("github".into()));

    let transcript_path = project
        .path()
        .join(".rupu")
        .join("transcripts")
        .join("s1.jsonl");
    let mut w = JsonlWriter::create(&transcript_path).unwrap();
    w.write(&TxEvent::NetFlow {
        flow: Box::new(stale_a),
    })
    .unwrap();
    w.write(&TxEvent::NetFlow {
        flow: Box::new(scm_flow_c),
    })
    .unwrap();
    w.flush().unwrap();

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    state
        .run_store
        .append_step_result(run_id, &seed_step(run_id, transcript_path))
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        2,
        "flow A (deduped) + transcript-only flow C; other-run flow excluded: {body}"
    );

    let ids: std::collections::HashSet<String> = flows
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&id_a.to_string()));
    assert!(ids.contains(&id_c.to_string()));

    let flow_a_out = flows
        .iter()
        .find(|f| f["id"].as_str() == Some(&id_a.to_string()))
        .unwrap();
    assert_eq!(
        flow_a_out["bytes_in"], 4096,
        "the ledger's finalized copy must win over the transcript's stale one"
    );

    assert_eq!(body["dropped"], 3);

    let hosts = body["hosts"].as_array().unwrap();
    assert!(hosts.iter().any(|h| h["host"] == "api.anthropic.com"));
    assert!(hosts.iter().any(|h| h["host"] == "api.github.com"));
    assert!(
        !hosts.iter().any(|h| h["host"] == "api.other.example"),
        "other run's flow must not leak into this run's host rollup: {hosts:?}"
    );
}

#[tokio::test]
async fn run_netflow_unknown_run_is_404() {
    let global = tempfile::tempdir().unwrap();
    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/nope/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn run_netflow_missing_ledger_is_empty_not_an_error() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run("run_no_ledger", project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/run_no_ledger/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["flows"].as_array().unwrap().len(), 0);
    assert_eq!(body["dropped"], 0);
}

// ── Project scope: must include system-origin egress with no run_id ───────
//
// The updater, the ASN refresh and CP's own fleet traffic are all
// `run_id: None` (`Origin::System`/`Update`/`Cp`). Project scope is where
// that egress becomes visible — a filter that dropped it here would hide
// exactly the traffic this scope exists to show.

#[tokio::test]
async fn project_netflow_includes_run_scoped_and_system_origin_flows() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let run_flow = e2e_flow(
        FlowId::new(),
        Some("run-a"),
        "api.anthropic.com",
        Origin::Provider("anthropic".into()),
    );
    let system_flow = e2e_flow(FlowId::new(), None, "iptoasn.com", Origin::System);
    // Project scope unions EVERY `.jsonl` file under the workspace's
    // netflow directory, so which run id names this particular file is
    // incidental to what's under test here (unlike the run-scope test
    // above, where the filename IS the thing under test).
    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(run_flow.clone())),
            LedgerLine::Flow(Box::new(system_flow.clone())),
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        2,
        "run-scoped and system-origin flows both present: {body}"
    );

    let hosts: std::collections::HashSet<&str> =
        flows.iter().map(|f| f["host"].as_str().unwrap()).collect();
    assert!(
        hosts.contains("iptoasn.com"),
        "system-origin egress must survive project scope: {body}"
    );
}

#[tokio::test]
async fn project_netflow_unknown_project_is_404() {
    let global = tempfile::tempdir().unwrap();
    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/nope/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── Global scope: union across every workspace, including system egress ───

#[tokio::test]
async fn global_netflow_unions_every_workspace_including_system_egress() {
    let global = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();

    write_ledger(
        project_a.path(),
        "run-a",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some("run-a"),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );
    write_ledger(
        project_b.path(),
        "system",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            None,
            "iptoasn.com",
            Origin::System,
        )))],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    rupu_workspace::upsert(&store, project_a.path()).unwrap();
    rupu_workspace::upsert(&store, project_b.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 2, "both workspaces' flows present: {body}");
    let hosts: std::collections::HashSet<&str> =
        flows.iter().map(|f| f["host"].as_str().unwrap()).collect();
    assert!(hosts.contains("api.anthropic.com"));
    assert!(
        hosts.contains("iptoasn.com"),
        "system-origin egress must survive global scope: {body}"
    );
}

// ── Regression guards: no daemon-wide (non-workspace) ledger union ────────
//
// A prior revision of this API unioned a `cp serve`-daemon-wide ledger at
// `$RUPU_HOME/netflow/flows.jsonl` into global scope (Fix 1, netflow Plan 3
// review round 3) — that file no longer exists at all: per the netflow
// per-run plan, `cp serve`'s own fleet/ASN-refresh traffic is deliberately
// unrecorded (see `rupu-cli/src/cmd/cp.rs`'s deleted `http::init` call), and
// `read_all_workspaces_sync` no longer reads any path outside a registered
// workspace's own `.rupu/netflow/` directory. These two tests are now
// regression guards for that: a stray `.jsonl` sitting directly under
// `$RUPU_HOME/netflow/` (e.g. left over from an install that predates this
// plan) must stay invisible at both global and project scope, not silently
// start leaking again if a future change re-adds a shared-directory read.

#[tokio::test]
async fn global_netflow_ignores_a_legacy_daemon_wide_ledger_file() {
    let global = tempfile::tempdir().unwrap();

    // A leftover from the pre-plan shared-ledger model — NOT under any
    // registered workspace's `.rupu/netflow/`.
    let legacy_ledger_dir = global.path().join("netflow");
    std::fs::create_dir_all(&legacy_ledger_dir).unwrap();
    let cp_flow = e2e_flow(FlowId::new(), None, "api.other-cp.example", Origin::Cp);
    std::fs::write(
        legacy_ledger_dir.join("flows.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&LedgerLine::Flow(Box::new(cp_flow))).unwrap()
        ),
    )
    .unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["flows"].as_array().unwrap().len(),
        0,
        "a legacy daemon-wide ledger file must not surface at global scope: {body}"
    );
}

#[tokio::test]
async fn project_netflow_does_not_leak_a_legacy_daemon_wide_ledger_file() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let legacy_ledger_dir = global.path().join("netflow");
    std::fs::create_dir_all(&legacy_ledger_dir).unwrap();
    let cp_flow = e2e_flow(FlowId::new(), None, "api.other-cp.example", Origin::Cp);
    std::fs::write(
        legacy_ledger_dir.join("flows.jsonl"),
        format!(
            "{}\n",
            serde_json::to_string(&LedgerLine::Flow(Box::new(cp_flow))).unwrap()
        ),
    )
    .unwrap();

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["flows"].as_array().unwrap().len(),
        0,
        "a legacy daemon-wide ledger file is not this project's traffic: {body}"
    );
}

#[tokio::test]
async fn global_netflow_with_no_registered_workspaces_is_empty_not_an_error() {
    let global = tempfile::tempdir().unwrap();
    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["flows"].as_array().unwrap().len(), 0);
}

// ── Graph scope ─────────────────────────────────────────────────────────

#[tokio::test]
async fn netflow_graph_project_scope_is_bipartite_and_keeps_system_source() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some("run-a"),
                "api.anthropic.com",
                Origin::Provider("anthropic".into()),
            ))),
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                None,
                "api.anthropic.com",
                Origin::System,
            ))),
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/netflow/graph?scope=project:{}",
        ws.id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let nodes = body["nodes"].as_array().unwrap();

    let sources: Vec<_> = nodes.iter().filter(|n| n["side"] == "source").collect();
    let endpoints: Vec<_> = nodes.iter().filter(|n| n["side"] == "endpoint").collect();
    assert_eq!(sources.len(), 2, "run-a and system: {body}");
    assert_eq!(endpoints.len(), 1);
    assert!(
        sources.iter().any(|n| n["id"] == "system"),
        "unattributed egress keeps a system source node: {body}"
    );
}

#[tokio::test]
async fn netflow_graph_run_scope_matches_the_runs_netflow_route() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_graph";

    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(run_id),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/netflow/graph?scope=run:{run_id}"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let nodes = body["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["id"] == run_id));
    assert!(nodes.iter().any(|n| n["id"] == "api.anthropic.com:443"));
}

#[tokio::test]
async fn netflow_graph_defaults_to_global_scope_when_absent() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    write_ledger(
        project.path(),
        "run-a",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some("run-a"),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );
    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow/graph"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["id"] == "run-a"));
}

#[test]
fn a_stale_table_triggers_at_most_one_concurrent_refresh() {
    // The guard is a flag, not a lock — a second request while a
    // refresh is in flight must not spawn a second download.
    let guard = rupu_cp::api::netflow::RefreshGuard::default();
    assert!(guard.try_begin(), "first caller wins");
    assert!(!guard.try_begin(), "second caller is turned away");
    guard.finish();
    assert!(
        guard.try_begin(),
        "after finishing, a later caller may retry"
    );
}
