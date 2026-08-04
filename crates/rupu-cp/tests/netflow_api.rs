//! The API must never invent data. A Coarse record's unknown byte count
//! stays null through serialization; a missing ASN table is reported as
//! `asn_loaded: false` rather than silently producing empty enrichment.
//!
//! The end-to-end tests below also prove Plan 3's ledger+transcript merge
//! correction over the real HTTP route: only *provider* flows carry
//! `ctx.run_id`, so a run's SCM/auth/update calls are `run_id: None` in the
//! ledger and only recoverable from the run's own transcript. See
//! `rupu-cp/src/api/netflow.rs`'s module doc for the full rationale.

// Throwaway in-process HTTP client hitting our own spawned test server, not
// rupu's own egress (mirrors `tests/run_graph.rs`).
#![allow(clippy::disallowed_methods)]

use rupu_netflow::{Fidelity, FlowCtx, FlowId, FlowRecord, LedgerLine, Origin, Outcome};

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

fn write_ledger(workspace: &std::path::Path, lines: &[LedgerLine]) {
    let paths = rupu_netflow::NetflowPaths::new(workspace);
    paths.ensure_dir().unwrap();
    let body: String = lines
        .iter()
        .map(|l| format!("{}\n", serde_json::to_string(l).unwrap()))
        .collect();
    std::fs::write(&paths.flows, body).unwrap();
}

#[test]
fn run_scope_filters_to_that_run_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_ledger(
        tmp.path(),
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

    let paths = rupu_netflow::NetflowPaths::new(tmp.path());
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
    write_ledger(
        tmp.path(),
        &[LedgerLine::Dropped {
            count: 12,
            ts: chrono::Utc::now(),
        }],
    );
    let paths = rupu_netflow::NetflowPaths::new(tmp.path());
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

/// The full arc: the ledger has this run's finalized flow (A) and another
/// run's flow (must be excluded); the run's transcript has a stale
/// pre-completion copy of A (must lose to the ledger's finalized copy) and
/// a `run_id: None` SCM flow (C) that the ledger's `run_id` filter alone
/// would have dropped entirely. Response must contain exactly A (finalized)
/// + C, report the ledger's dropped-count, and roll `hosts` up from that
/// same merged set (not leaking the other run's host).
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

    write_ledger(
        project.path(),
        &[
            LedgerLine::Flow(Box::new(ledger_flow_a.clone())),
            LedgerLine::Flow(Box::new(ledger_flow_other)),
            LedgerLine::Dropped {
                count: 3,
                ts: chrono::Utc::now(),
            },
        ],
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
