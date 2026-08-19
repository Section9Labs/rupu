//! The API must never invent data. A Coarse record's unknown byte count
//! stays null through serialization; a missing ASN table is reported as
//! `asn_loaded: false` rather than silently producing empty enrichment.
//!
//! The end-to-end tests below also prove the ledger+transcript merge still
//! recovers a run whose ledger file could never be opened at all
//! (`netflow_sink::for_run` degrades to transcript-only capture rather than
//! break the run) — with one ledger file per run, attribution is by FILE,
//! not by the `ctx.run_id` field, so a flow with `run_id: None` (SCM/auth/
//! update/`Cp` origins) is already in the ledger read on a normal run; the
//! merge exists for the degraded one. See `rupu-cp/src/api/netflow.rs`'s
//! module doc for the full rationale.
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

/// As [`write_ledger`], but writes directly under `<global>/netflow/`
/// instead of `<workspace>/.rupu/netflow/` — the routing a run's ledger
/// falls back to whenever its workspace never got its own `.rupu/netflow/`
/// (see `rupu_netflow::netflow_dir`'s doc comment). On a fresh install
/// this is where EVERY run's ledger actually lands, so it's the shape the
/// Critical-fix regression tests below need to reproduce.
fn write_global_ledger(
    global: &std::path::Path,
    run_id: &str,
    lines: &[LedgerLine],
) -> NetflowPaths {
    let paths = NetflowPaths::for_run(&global.join("netflow"), run_id);
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
/// the ledger's finalized copy) and an SCM flow (C) that is deliberately
/// written to the transcript ONLY, standing in for a flow that landed in
/// this run's transcript but never made it into its ledger file (the
/// `netflow_sink::for_run` degrade case the merge exists for) — a ledger
/// read alone would never see it. Response must contain exactly A
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

    assert_eq!(body["dropped_total"], 3);

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
    assert_eq!(body["dropped_total"], 0);
}

// ── Project scope: must include a flow whose OWN `run_id` field is
//    `None`, from the same ledger directory as a normal run-scoped one ──
//
// `Origin::System`/`Update`/`Cp` traffic is NOT this case: every
// production construction site for those three wires its client to
// `Arc::new(NullSink)`, so none of it ever reaches a ledger at any scope
// (see `ScopeDisclosure.tsx`'s header comment for the full accounting) —
// asserting otherwise here would test a state production cannot produce,
// the exact defect a whole-branch review caught this file making (fix
// round 2). The REAL case: `Origin::Scm` traffic from
// `rupu_scm::Registry::discover` DOES reach a run's own per-run ledger
// file when a run is active, but — per this module doc and
// `rupu_netflow::ctx::FlowCtx`'s own doc comment — no production
// `FlowCtx` ever populates the `run_id` field itself; attribution is by
// which FILE a flow landed in, not by this field. Project scope unions
// every file in the directory regardless of that field, so this is the
// fixture that actually matches what a real ledger directory can contain.

#[tokio::test]
async fn project_netflow_includes_a_run_id_none_flow_from_the_same_ledger_directory() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let run_flow = e2e_flow(
        FlowId::new(),
        Some("run-a"),
        "api.anthropic.com",
        Origin::Provider("anthropic".into()),
    );
    let scm_flow = e2e_flow(
        FlowId::new(),
        None,
        "api.gitlab.example",
        Origin::Scm("gitlab".into()),
    );
    // Project scope unions EVERY `.jsonl` file under the workspace's
    // netflow directory, so which run id names this particular file is
    // incidental to what's under test here (unlike the run-scope test
    // above, where the filename IS the thing under test).
    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(run_flow.clone())),
            LedgerLine::Flow(Box::new(scm_flow.clone())),
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
        "both flows present regardless of the ctx.run_id field: {body}"
    );

    let hosts: std::collections::HashSet<&str> =
        flows.iter().map(|f| f["host"].as_str().unwrap()).collect();
    assert!(
        hosts.contains("api.gitlab.example"),
        "a run_id: None flow from the SAME ledger directory must survive project scope: {body}"
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

// ── Global scope: unions across every workspace, including a flow whose
//    OWN `run_id` field is `None` in a SEPARATE workspace's ledger ───────
//
// Same correction as the project-scope fixture above (fix round 2): the
// second flow here used to be `Origin::System`, which no production site
// ever writes to any ledger (every one of them is wired to `NullSink`).
// `Origin::Scm` from `Registry::discover` is the fixture that matches a
// real ledger directory's contents.

#[tokio::test]
async fn global_netflow_unions_every_workspace_including_a_run_id_none_flow() {
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
        "run-b",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            None,
            "api.gitlab.example",
            Origin::Scm("gitlab".into()),
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
        hosts.contains("api.gitlab.example"),
        "a run_id: None flow from a DIFFERENT workspace's ledger must survive global scope: {body}"
    );
}

// ── Regression guards: only a literal `flows.jsonl` stays excluded ────────
//
// IMPORTANT: `read_all_workspaces_sync` DOES read `<global_dir>/netflow/`
// now (the Critical fix — every run's ledger lands there on a fresh
// install, since `rupu init` never creates a project's own
// `.rupu/netflow/`; see `rupu_netflow::netflow_dir`'s doc comment). A REAL
// per-run file sitting there (`<run_id>.jsonl`) DOES surface at global
// scope — proved by `global_netflow_includes_a_run_that_fell_back_to_the_
// global_netflow_dir`, below. Do not "fix" these two tests back toward
// "global scope reads nothing outside a registered workspace" — that
// claim is false and reintroducing it is exactly the Critical this
// plan's own review caught.
//
// What's actually still excluded is narrower: `read_all_run_ledgers_in_dir`
// skips a file literally named `flows.jsonl` (the pre-plan shared-ledger
// filename — no real run id ever produces that name), so a leftover from
// before this migration can't be silently reinterpreted as a valid run's
// ledger. `global_netflow_ignores_a_legacy_daemon_wide_ledger_file` below
// proves exactly that filename exclusion, at global scope (the only scope
// that reads `<global_dir>/netflow/` at all).
// `project_netflow_does_not_leak_a_legacy_daemon_wide_ledger_file` proves
// something more basic, for a different reason: project scope never reads
// `<global_dir>/netflow/` in the first place (a separate, still-open gap —
// see `get_project_netflow`'s doc comment), so a file sitting there,
// `flows.jsonl` or not, was never going to appear in a project's own view.

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

// ── Critical-fix regression tests: global-fallback routing ────────────────
//
// `rupu init` now creates `<project>/.rupu/netflow/` for a NEW project
// (`crates/rupu-cli/src/cmd/init.rs`'s `ensure_netflow_dir`), but every
// project initialised before that change -- or never `rupu init`'d at all
// -- still has `<project>/.rupu/netflow/` missing, and there is no way to
// retroactively create it for old runs short of re-running `init` (which
// only helps runs dispatched AFTER it runs). These fixtures below
// deliberately don't call `init` at all, simulating exactly that
// pre-existing-project shape: EVERY run's ledger lands in
// `<global_dir>/netflow/<run_id>.jsonl`, not under any registered
// workspace's own directory. These prove the read side mirrors that
// write-side routing rule, rather than only ever finding project-rooted
// ledgers such a project never produces.

#[tokio::test]
async fn global_netflow_includes_a_run_that_fell_back_to_the_global_netflow_dir() {
    let global = tempfile::tempdir().unwrap();

    // Simulates the common case: this run's workspace never got its own
    // `.rupu/netflow/`, so its ledger landed here instead.
    write_global_ledger(
        global.path(),
        "run-fallback",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some("run-fallback"),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        1,
        "a run whose ledger fell back to <global_dir>/netflow/ must still surface at global scope: {body}"
    );
    assert_eq!(flows[0]["host"], "api.anthropic.com");
}

/// The default `RUPU_HOME=~/.rupu` shape: a workspace registered at
/// `$HOME` makes `<workspace>/.rupu/netflow/` and `<global_dir>/netflow/`
/// the SAME directory, because `global_dir` IS `<home>/.rupu`. This is
/// reachable, not theoretical -- `rupu run` executed from `$HOME` both
/// registers `$HOME` as a workspace (`cmd/run.rs`) and writes its ledger
/// to `~/.rupu/netflow` (`project_root_for` walks up and matches
/// `~/.rupu`). Without directory dedup, `read_all_workspaces_sync` would
/// read that one shared directory twice: every flow would be listed
/// twice (global scope has no `FlowId` dedup, unlike run scope's
/// `merge_with_transcript`) and `dropped` would double.
#[tokio::test]
async fn global_netflow_does_not_double_count_when_a_workspace_ledger_dir_is_the_global_one() {
    let home = tempfile::tempdir().unwrap();
    let global = home.path().join(".rupu");
    std::fs::create_dir_all(&global).unwrap();

    write_global_ledger(
        &global,
        "run-collision",
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some("run-collision"),
                "api.anthropic.com",
                Origin::Provider("anthropic".into()),
            ))),
            LedgerLine::Dropped {
                count: 4,
                ts: chrono::Utc::now(),
            },
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    rupu_workspace::upsert(&store, home.path()).unwrap();

    let addr = serve(new_state(&global)).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        1,
        "a workspace whose ledger dir IS the global one must not be read twice: {body}"
    );
    assert_eq!(
        body["dropped_total"], 4,
        "dropped must not double when the workspace and global ledger dirs collide: {body}"
    );
}

#[tokio::test]
async fn run_netflow_falls_back_to_the_global_netflow_dir_when_the_workspace_has_none() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_fallback_scoped";

    // Deliberately do NOT create `<project>/.rupu/netflow/` -- this is the
    // fresh-install shape the Critical fix exists for. The run's ledger
    // lives only at the global fallback path.
    write_global_ledger(
        global.path(),
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
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        1,
        "run scope must fall back to the global netflow dir when the workspace has none: {body}"
    );
    assert_eq!(flows[0]["host"], "api.anthropic.com");
}

/// Finding 5, whole-branch review: `cp serve` resumes a run via a detached
/// command running under the DAEMON's own cwd, which need not match the
/// run's original workspace. A run whose early steps wrote to the
/// workspace-local netflow dir can have its later steps' ledger land at
/// the global fallback instead (or vice versa) — one run id split across
/// both roots. The old `resolve_ledger_path` (singular) picked whichever
/// root it found first and stopped, silently losing every flow (and
/// `Dropped` line) that landed in the other one. `resolve_ledger_paths`
/// (plural) now reads both roots and unions the results.
#[tokio::test]
async fn run_netflow_unions_a_run_split_across_workspace_and_global_roots() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_split";

    // First half of this run's ledger: workspace-local root.
    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some(run_id),
                "api.anthropic.com",
                Origin::Provider("anthropic".into()),
            ))),
            LedgerLine::Dropped {
                count: 2,
                ts: chrono::Utc::now(),
            },
        ],
    );
    // Second half of the SAME run's ledger: global root.
    write_global_ledger(
        global.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some(run_id),
                "api.github.com",
                Origin::Scm("github".into()),
            ))),
            LedgerLine::Dropped {
                count: 3,
                ts: chrono::Utc::now(),
            },
        ],
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
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        2,
        "both halves of a run split across workspace-local and global roots must be read: {body}"
    );
    let hosts: std::collections::HashSet<&str> =
        flows.iter().map(|f| f["host"].as_str().unwrap()).collect();
    assert!(hosts.contains("api.anthropic.com"));
    assert!(hosts.contains("api.github.com"));
    assert_eq!(
        body["dropped_total"], 5,
        "dropped must sum across both halves of the split ledger: {body}"
    );
}

/// The DAG scheduler mints a fresh run id per dispatched step, so a
/// step's provider flows AND its `Dropped` line land in
/// `<netflow_dir>/<step_run_id>.jsonl`, not the parent workflow run's own
/// file (`Dropped` is a ledger-only line -- unlike a flow record, there
/// is no transcript fallback to recover it under a different id). A
/// run-scoped read that only opened the workflow's own ledger would
/// silently under-report loss for any run with a dispatched step -- proven
/// here by putting the workflow's own Dropped count in one file and a
/// step's in another, both of which must be summed.
#[tokio::test]
async fn run_netflow_sums_dropped_counts_across_a_dispatched_steps_own_ledger() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_with_step";
    let step_run_id = "run_step_own_id";

    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Dropped {
            count: 5,
            ts: chrono::Utc::now(),
        }],
    );
    let step_flow = e2e_flow(
        FlowId::new(),
        Some(step_run_id),
        "api.github.com",
        Origin::Scm("github".into()),
    );
    write_ledger(
        project.path(),
        step_run_id,
        &[
            LedgerLine::Flow(Box::new(step_flow)),
            LedgerLine::Dropped {
                count: 7,
                ts: chrono::Utc::now(),
            },
        ],
    );

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let step_transcript = project
        .path()
        .join(".rupu")
        .join("transcripts")
        .join("s1.jsonl");
    JsonlWriter::create(&step_transcript).unwrap();
    state
        .run_store
        .append_step_result(run_id, &seed_step(step_run_id, step_transcript))
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["dropped_total"], 12,
        "dropped must sum the workflow's own ledger (5) AND the dispatched \
         step's own ledger (7), not just the workflow's: {body}"
    );
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows.iter().any(|f| f["host"] == "api.github.com"),
        "the dispatched step's own flow must surface too: {body}"
    );
}

// ── Run scope: dispatched sub-agents (recursive) ───────────────────────────
//
// A `dispatch_agent` sub-run gets its OWN per-run ledger, written under the
// exact same netflow root as the dispatching run (`netflow_sink::for_run`
// in `rupu-cli`'s `cmd/dispatch.rs` is handed the same `global`/
// `project_root` the parent used) — but until `RunStore::sub_run_ids`/
// `sub_run_ids_recursive` existed, nothing on the read side ever looked
// for it: `run_and_unit_ids` only ever harvested ids from
// `step_results.jsonl`, which a sub-agent dispatch never writes to (that's
// the DAG-scheduled STEP path, a different mechanism). These tests use the
// real `RunStore::create_sub_run` (not a hand-rolled directory) so they
// exercise the actual on-disk layout dispatch produces.

#[tokio::test]
async fn run_netflow_includes_a_dispatched_sub_agents_flows_and_dropped_count() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_with_sub_agent";

    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Dropped {
            count: 5,
            ts: chrono::Utc::now(),
        }],
    );

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let (sub_run_id, _transcript) = state.run_store.create_sub_run(run_id, "fixer").unwrap();

    let sub_flow = e2e_flow(
        FlowId::new(),
        Some(&sub_run_id),
        "api.anthropic.com",
        Origin::Provider("anthropic".into()),
    );
    write_ledger(
        project.path(),
        &sub_run_id,
        &[
            LedgerLine::Flow(Box::new(sub_flow)),
            LedgerLine::Dropped {
                count: 7,
                ts: chrono::Utc::now(),
            },
        ],
    );

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["dropped_total"], 12,
        "must sum the run's own ledger (5) AND its dispatched sub-agent's own \
         ledger (7): {body}"
    );
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows.iter().any(|f| f["host"] == "api.anthropic.com"),
        "the sub-agent's own flow must surface at the parent's run scope: {body}"
    );
}

/// The Critical fix-round-1 case: a sub-agent dispatched from INSIDE a
/// fan-out unit. `for_each`/`parallel`/panel each mint a fresh unit run
/// id (`step_factory.rs` hands it down as `ToolContext.parent_run_id`),
/// so a `dispatch_agent` call made from within that unit's own agent run
/// calls `create_sub_run(unit_id, ...)`, not `create_sub_run(run_id, ...)`
/// — the sub-run lives at `<root>/<unit_id>/sub/`, a directory
/// `sub_run_ids_recursive(run_id)` alone never visits (it only starts
/// from the TOP-level run id). `run_and_unit_ids` must recurse from
/// EVERY id it already collects (the run id AND each step/item id), not
/// just the run id, or a sub-agent dispatched from a fan-out unit keeps
/// exactly the invisibility this task exists to remove.
#[tokio::test]
async fn run_netflow_includes_a_sub_agent_dispatched_from_a_fan_out_unit() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_with_fan_out_sub_agent";
    let unit_run_id = "run_fan_out_unit";

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let unit_transcript = project
        .path()
        .join(".rupu")
        .join("transcripts")
        .join("unit.jsonl");
    JsonlWriter::create(&unit_transcript).unwrap();
    state
        .run_store
        .append_step_result(run_id, &seed_step(unit_run_id, unit_transcript))
        .unwrap();

    // The unit's OWN agent run dispatches a sub-agent — parented on the
    // UNIT's id, exactly as `step_factory.rs`'s `ToolContext.parent_run_id`
    // wiring does for a real `for_each`/`parallel`/panel unit.
    let (sub_of_unit, _) = state
        .run_store
        .create_sub_run(unit_run_id, "fixer")
        .unwrap();

    write_ledger(
        project.path(),
        &sub_of_unit,
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some(&sub_of_unit),
                "api.fanout-sub.example",
                Origin::Provider("anthropic".into()),
            ))),
            LedgerLine::Dropped {
                count: 4,
                ts: chrono::Utc::now(),
            },
        ],
    );

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows.iter().any(|f| f["host"] == "api.fanout-sub.example"),
        "a sub-agent dispatched from inside a fan-out unit must surface at \
         the top-level run's netflow view: {body}"
    );
    assert_eq!(
        body["dropped_total"], 4,
        "its ledger-only Dropped count, which has no transcript fallback, \
         must also surface: {body}"
    );
}

/// The decisive nested case: a sub-agent that itself dispatches a
/// sub-agent (two levels deep). Both levels' flows AND drop counts must
/// surface at the top parent's run scope.
#[tokio::test]
async fn run_netflow_reaches_a_nested_sub_agents_sub_agent() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_with_nested_sub_agents";

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let (child_id, _) = state
        .run_store
        .create_sub_run(run_id, "researcher")
        .unwrap();
    let (grandchild_id, _) = state.run_store.create_sub_run(&child_id, "fixer").unwrap();

    write_ledger(
        project.path(),
        &child_id,
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some(&child_id),
                "api.github.com",
                Origin::Scm("github".into()),
            ))),
            LedgerLine::Dropped {
                count: 2,
                ts: chrono::Utc::now(),
            },
        ],
    );
    write_ledger(
        project.path(),
        &grandchild_id,
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some(&grandchild_id),
                "api.openai.com",
                Origin::Provider("openai".into()),
            ))),
            LedgerLine::Dropped {
                count: 3,
                ts: chrono::Utc::now(),
            },
        ],
    );

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows.iter().any(|f| f["host"] == "api.github.com"),
        "the first-level sub-agent's flow must surface: {body}"
    );
    assert!(
        flows.iter().any(|f| f["host"] == "api.openai.com"),
        "the SECOND-level (grandchild) sub-agent's flow must also surface: {body}"
    );
    assert_eq!(
        body["dropped_total"], 5,
        "must sum both levels' own dropped counts (2 + 3): {body}"
    );
}

/// Attribution safety: a sub-run dispatched under a DIFFERENT parent run
/// must never appear in this run's netflow view, even though both
/// ledgers live in the same netflow directory.
#[tokio::test]
async fn run_netflow_does_not_leak_a_sub_agent_belonging_to_a_different_parent() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_a = "run_a_with_sub_agent";
    let run_b = "run_b_with_sub_agent";

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_a, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    state
        .run_store
        .create(
            seed_run(run_b, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let (sub_of_a, _) = state.run_store.create_sub_run(run_a, "agent").unwrap();
    let (sub_of_b, _) = state.run_store.create_sub_run(run_b, "agent").unwrap();

    write_ledger(
        project.path(),
        &sub_of_a,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(&sub_of_a),
            "a.example",
            Origin::Provider("anthropic".into()),
        )))],
    );
    write_ledger(
        project.path(),
        &sub_of_b,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(&sub_of_b),
            "b.example",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_a}/netflow"))
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert!(flows.iter().any(|f| f["host"] == "a.example"));
    assert!(
        !flows.iter().any(|f| f["host"] == "b.example"),
        "run A's netflow view must never include run B's sub-agent's flow: {body}"
    );
}

// ── Project scope: id-driven global-fallback recovery ─────────────────────
//
// `get_project_netflow` used to read ONLY `<workspace>/.rupu/netflow/`
// (the whole-directory scan) -- so on every default install, where `rupu
// init` never created that directory, every run's ledger landed at the
// global fallback and project scope was empty for every project, forever
// (`ScopeDisclosure.tsx`'s old `netflowEmptyStateHint` said so explicitly).
// `project_scoped_flows_and_dropped` now ALSO runs an id-driven pass over
// the global fallback root, scoped to run ids this project's OWN run
// records name via their `workspace_path` field -- never inferred from
// scanning the global directory's file names. These tests prove both the
// recovery AND that it stays attribution-safe.

#[tokio::test]
async fn project_netflow_recovers_a_run_whose_ledger_fell_back_to_the_global_dir() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_fallback_project_scoped";

    // Deliberately do NOT create `<project>/.rupu/netflow/` -- the
    // fresh-install shape: this run's ledger lives only at the global
    // fallback path.
    write_global_ledger(
        global.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(run_id),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, project.path()).unwrap();

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        1,
        "project scope must recover a run whose ledger fell back to the global dir: {body}"
    );
    assert_eq!(flows[0]["host"], "api.anthropic.com");
}

/// The failure mode worse than the empty tab this replaces: a project
/// scope showing a flow that belongs to some OTHER project. Two projects
/// are registered; only project B has a run record (`workspace_path ==
/// project_b`) whose ledger fell back to the global dir. Project A's own
/// scope must stay empty -- attribution comes from B's run record naming
/// B's own workspace, never from A merely sharing the same global
/// fallback directory.
#[tokio::test]
async fn project_netflow_does_not_recover_a_different_projects_run_from_the_global_dir() {
    let global = tempfile::tempdir().unwrap();
    let project_a = tempfile::tempdir().unwrap();
    let project_b = tempfile::tempdir().unwrap();
    let run_id = "run_belongs_to_b";

    write_global_ledger(
        global.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(run_id),
            "api.b-only.example",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws_a = rupu_workspace::upsert(&ws_store, project_a.path()).unwrap();
    rupu_workspace::upsert(&ws_store, project_b.path()).unwrap();

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project_b.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws_a.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["flows"].as_array().unwrap().len(),
        0,
        "project A must never see project B's run just because both ledgers \
         share the same global fallback directory: {body}"
    );
}

/// The DAG scheduler mints a fresh id per dispatched step/unit, so its
/// flows land in `<id>.jsonl`, never the parent workflow run's own file
/// (see `run_and_unit_ids`). A project-scope recovery pass that only
/// looked up the workflow run's own id would miss it.
#[tokio::test]
async fn project_netflow_recovers_a_dispatched_steps_own_global_fallback_ledger() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_with_step_fallback";
    let step_run_id = "run_step_fallback_own_id";

    write_global_ledger(
        global.path(),
        step_run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(step_run_id),
            "api.github.com",
            Origin::Scm("github".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, project.path()).unwrap();

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let step_transcript = project
        .path()
        .join(".rupu")
        .join("transcripts")
        .join("s1.jsonl");
    JsonlWriter::create(&step_transcript).unwrap();
    state
        .run_store
        .append_step_result(run_id, &seed_step(step_run_id, step_transcript))
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows.iter().any(|f| f["host"] == "api.github.com"),
        "a dispatched step's own global-fallback ledger must surface at project scope: {body}"
    );
}

/// A run recorded in the PROJECT-LOCAL store (`<workspace>/.rupu/runs/`,
/// not the global one -- `resolve_run_location`'s `ProjectLocal` branch)
/// whose ledger still fell back to the global dir. Proves the recovery
/// pass enumerates both stores, not just the global one.
#[tokio::test]
async fn project_netflow_recovers_a_project_local_run_whose_ledger_fell_back_to_global() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_project_local_fallback";

    write_global_ledger(
        global.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(run_id),
            "api.project-local.example",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, project.path()).unwrap();

    let local_store =
        rupu_orchestrator::runs::RunStore::new(project.path().join(".rupu").join("runs"));
    local_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows
            .iter()
            .any(|f| f["host"] == "api.project-local.example"),
        "a project-local-store run's global-fallback ledger must surface too: {body}"
    );
}

/// Fix round 1, Important 1: the dispatched-step counterpart of the test
/// above. A run recorded in the PROJECT-LOCAL store dispatches a step
/// whose own id gets a SEPARATE global-fallback ledger — the parent run's
/// own id has no ledger at all, so this only passes if the step's id is
/// looked up via `run_and_unit_ids` against the PROJECT-LOCAL store's own
/// `step_results.jsonl`, not the global store's (which has no record of
/// this run and would silently resolve to nothing beyond the parent id).
#[tokio::test]
async fn project_netflow_recovers_a_project_local_runs_dispatched_step_global_fallback_ledger() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_project_local_with_step";
    let step_run_id = "run_project_local_step_own_id";

    // Deliberately NO ledger for `run_id` itself -- only the dispatched
    // step's own id has one. If the id-driven pass fell back to
    // resolving units against the wrong store, it would find zero units
    // for this run and this flow would never surface.
    write_global_ledger(
        global.path(),
        step_run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(step_run_id),
            "api.project-local-step.example",
            Origin::Scm("github".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, project.path()).unwrap();

    let local_store =
        rupu_orchestrator::runs::RunStore::new(project.path().join(".rupu").join("runs"));
    local_store
        .create(
            seed_run(run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();
    let step_transcript = project
        .path()
        .join(".rupu")
        .join("transcripts")
        .join("s1.jsonl");
    JsonlWriter::create(&step_transcript).unwrap();
    local_store
        .append_step_result(run_id, &seed_step(step_run_id, step_transcript))
        .unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert!(
        flows
            .iter()
            .any(|f| f["host"] == "api.project-local-step.example"),
        "a project-local run's dispatched step's own global-fallback ledger \
         must surface -- units must be resolved against the store the run id \
         actually came from: {body}"
    );
}

/// The two "correct absence" cases the recovery pass deliberately does NOT
/// fix, alongside proof that a real fallback flow for the same project
/// still surfaces: (1) a `run_id: None` flow sitting under an id with no
/// run record anywhere is unattributable and stays invisible at project
/// scope; (2) an orphaned ledger (its owning run's record has been
/// deleted) is likewise never resurrected. Neither is a bug -- there is no
/// run record to enumerate either id from.
#[tokio::test]
async fn project_netflow_does_not_recover_unattributable_or_orphaned_global_ledgers() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let real_run_id = "run_real_and_attributed";

    // A real, attributed run -- must surface.
    write_global_ledger(
        global.path(),
        real_run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(real_run_id),
            "api.attributed.example",
            Origin::Provider("anthropic".into()),
        )))],
    );
    // An orphan: a ledger file with no run record behind it at all (e.g.
    // the run was deleted, or a `run_id: None` system-origin flow that
    // was never tied to a run to begin with).
    write_global_ledger(
        global.path(),
        "run_no_record_at_all",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            None,
            "api.orphaned.example",
            Origin::Scm("github".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, project.path()).unwrap();

    let state = new_state(global.path());
    state
        .run_store
        .create(
            seed_run(real_run_id, project.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    let hosts: std::collections::HashSet<&str> =
        flows.iter().map(|f| f["host"].as_str().unwrap()).collect();
    assert!(
        hosts.contains("api.attributed.example"),
        "the real, attributed run must still surface: {body}"
    );
    assert!(
        !hosts.contains("api.orphaned.example"),
        "an orphaned/unattributable ledger with no run record must stay invisible \
         at project scope, not be swept in just because it's in the global dir: {body}"
    );
    assert_eq!(flows.len(), 1, "exactly the one attributed flow: {body}");
}

/// The `$HOME`/`RUPU_HOME` collision (mirrors `global_netflow_does_not_
/// double_count_when_a_workspace_ledger_dir_is_the_global_one`): when a
/// workspace is registered at `$HOME` and `global_dir` is the default
/// `~/.rupu`, `<workspace>/.rupu/netflow/` and `<global_dir>/netflow/`
/// are the SAME directory. The whole-directory scan already reads every
/// file in it; the id-driven fallback pass must not read it a second
/// time, or this flow (and its dropped count) would be double-counted.
#[tokio::test]
async fn project_netflow_does_not_double_count_when_workspace_and_global_netflow_dirs_collide() {
    let home = tempfile::tempdir().unwrap();
    let global = home.path().join(".rupu");
    std::fs::create_dir_all(&global).unwrap();
    let run_id = "run_home_collision";

    write_global_ledger(
        &global,
        run_id,
        &[
            LedgerLine::Flow(Box::new(e2e_flow(
                FlowId::new(),
                Some(run_id),
                "api.anthropic.com",
                Origin::Provider("anthropic".into()),
            ))),
            LedgerLine::Dropped {
                count: 4,
                ts: chrono::Utc::now(),
            },
        ],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, home.path()).unwrap();

    let state = new_state(&global);
    state
        .run_store
        .create(
            seed_run(run_id, home.path().to_path_buf()),
            "name: wf\nsteps: []\n",
        )
        .unwrap();

    let addr = serve(state).await;
    let resp = reqwest::get(format!("http://{addr}/api/projects/{}/netflow", ws.id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["flows"].as_array().unwrap().len(),
        1,
        "must not be double-counted when workspace-local and global netflow dirs collide: {body}"
    );
    assert_eq!(
        body["dropped_total"], 4,
        "dropped must not be doubled either: {body}"
    );
}

/// The graph endpoint's `project:` scope must recover global-fallback
/// flows too -- it shares `project_scoped_flows_and_dropped` with the
/// table endpoint precisely so the two never drift apart on this.
#[tokio::test]
async fn netflow_graph_project_scope_recovers_a_global_fallback_run() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_graph_fallback";

    write_global_ledger(
        global.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            Some(run_id),
            "api.anthropic.com",
            Origin::Provider("anthropic".into()),
        )))],
    );

    let ws_store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&ws_store, project.path()).unwrap();

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
        "http://{addr}/api/netflow/graph?scope=project:{}",
        ws.id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let nodes = body["nodes"].as_array().unwrap();
    assert!(
        nodes
            .iter()
            .any(|n| n["label"] == "api.anthropic.com" || n["id"] == "api.anthropic.com"),
        "graph project scope must include the recovered global-fallback flow's endpoint: {body}"
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

/// Finding 4, whole-branch review: `graph_view` used to derive a flow's
/// source id from `ctx.run_id`, a field no production `FlowCtx` ever
/// populates -- every graph, at every scope, collapsed to one node
/// literally called `system`. The read side now tags each flow with the
/// id of the LEDGER FILE it came from, so this test writes two SEPARATE
/// per-run ledger files (not one file with two differing `ctx.run_id`
/// values, which is what the pre-fix version of this test did) to prove
/// two distinct source nodes -- one per contributing file, regardless of
/// what `ctx.run_id` the flow inside each one happens to carry.
#[tokio::test]
async fn netflow_graph_project_scope_is_bipartite_with_one_source_per_ledger_file() {
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
    // A second run's own ledger file. `ctx.run_id: None` here on purpose
    // -- matches what a real Origin::Scm flow looks like -- to prove the
    // source id comes from the FILE, not this field.
    write_ledger(
        project.path(),
        "run-b",
        &[LedgerLine::Flow(Box::new(e2e_flow(
            FlowId::new(),
            None,
            "api.anthropic.com",
            Origin::Scm("github".into()),
        )))],
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
    assert_eq!(
        sources.len(),
        2,
        "one source per contributing ledger file: {body}"
    );
    assert_eq!(endpoints.len(), 1);
    assert!(
        sources.iter().any(|n| n["id"] == "run-a"),
        "run-a's ledger file must be its own source node: {body}"
    );
    assert!(
        sources.iter().any(|n| n["id"] == "run-b"),
        "run-b's ledger file must be its own source node, not merged into \
         a system fallback: {body}"
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

// ── Time-range filtering (`?from=`/`?to=`) ─────────────────────────────────
//
// `TimeRange`/`read_flows_in_range` (Task 1) already own the filtering
// semantics: `FlowRecord::ts` (header-arrival time, not request start or
// body completion), both bounds inclusive, `from > to` is an empty window
// rather than an error. These tests exercise the HTTP surface on top of
// that: every scope threads the parsed range into its read, a malformed
// bound is a 400 (never a silent unbounded fallback), and — the
// requirement that matters most — `dropped_total` always reports the
// WHOLE ledger file's loss, never narrowed by the window, and is named so
// an operator can't mistake it for "nothing was lost in this window".

fn flow_at(id: FlowId, host: &str, ts: chrono::DateTime<chrono::Utc>) -> FlowRecord {
    let mut f = e2e_flow(
        id,
        Some("run_tr"),
        host,
        Origin::Provider("anthropic".into()),
    );
    f.ts = ts;
    f
}

fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs, 0).unwrap()
}

/// Percent-encode an RFC 3339 timestamp for use as a query-string value —
/// `to_rfc3339()` on a UTC `DateTime` renders the `+00:00` offset form, and
/// an unescaped `:`/`+` sent through a real HTTP GET is decoded server-side
/// (`application/x-www-form-urlencoded` semantics, which is what Axum's
/// `Query` extractor uses) as a SPACE for the bare `+` — silently turning a
/// well-formed timestamp into a malformed one before it ever reaches
/// `parse_time_range`. Mirrors `rupu-cp/src/api/usage.rs`'s
/// `urlencoding_rfc3339` and `netflow.rs`'s own copy, added for the exact
/// same reason.
fn qs(dt: chrono::DateTime<chrono::Utc>) -> String {
    dt.to_rfc3339().replace('+', "%2B").replace(':', "%3A")
}

#[tokio::test]
async fn a_range_query_returns_only_flows_inside_the_window() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    let in_window = flow_at(FlowId::new(), "in.example", at(150));
    let before_window = flow_at(FlowId::new(), "before.example", at(50));
    let after_window = flow_at(FlowId::new(), "after.example", at(300));

    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(in_window.clone())),
            LedgerLine::Flow(Box::new(before_window)),
            LedgerLine::Flow(Box::new(after_window)),
        ],
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
        "http://{addr}/api/runs/{run_id}/netflow?from={}&to={}",
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(
        flows.len(),
        1,
        "only the in-window flow survives the filter: {body}"
    );
    assert_eq!(flows[0]["id"], in_window.id.to_string());
}

/// Important 1 (Fix round 1 review): the bypass Task 3 found and fixed —
/// `run_scoped_flows_and_dropped` filtering the ledger read by range but
/// then unconditionally re-adding whatever `merge_with_transcript` recovers
/// — had NO regression test. This is the model fixture
/// (`run_netflow_route_merges_ledger_and_transcript_dedupes_and_excludes_
/// other_runs`) with a window applied and a transcript-only flow placed
/// OUTSIDE it: no ledger counterpart exists for this flow at all, so the
/// only way it could appear in the response is via the unfiltered merge
/// output — which is exactly the code path the `.filter(|f| range.
/// contains(f.ts))` at the end of `run_scoped_flows_and_dropped` guards.
/// Deleting that filter must turn this test red.
#[tokio::test]
async fn run_scope_time_range_excludes_an_out_of_window_transcript_only_flow() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr_merge";

    let id_in = FlowId::new();
    let ledger_flow_in_window = flow_at(id_in, "in.example", at(150));
    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(ledger_flow_in_window))],
    );

    // Transcript-only flow: no ledger counterpart anywhere, and its `ts` is
    // OUTSIDE the window this test queries for. If the merge's output were
    // ever re-exposed without a second filter pass, this is exactly the
    // flow that would leak back in.
    let id_out = FlowId::new();
    let transcript_only_out_of_window = {
        let mut f = e2e_flow(
            id_out,
            None,
            "recovered.example",
            Origin::Scm("github".into()),
        );
        f.ts = at(500);
        f
    };
    let transcript_path = project
        .path()
        .join(".rupu")
        .join("transcripts")
        .join("s1.jsonl");
    let mut w = JsonlWriter::create(&transcript_path).unwrap();
    w.write(&TxEvent::NetFlow {
        flow: Box::new(transcript_only_out_of_window),
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
    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/{run_id}/netflow?from={}&to={}",
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    let ids: std::collections::HashSet<String> = flows
        .iter()
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect();

    assert!(
        ids.contains(&id_in.to_string()),
        "the in-window ledger flow must still be present: {body}"
    );
    assert!(
        !ids.contains(&id_out.to_string()),
        "a transcript-only flow recovered by the merge, but outside the \
         requested window, must NOT leak into a filtered response: {body}"
    );
    assert_eq!(
        flows.len(),
        1,
        "exactly the in-window flow, nothing recovered-but-out-of-window: {body}"
    );
}

#[tokio::test]
async fn absent_bounds_return_everything() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "a.example", at(50)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "b.example", at(300)))),
        ],
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
    let resp = reqwest::get(format!("http://{addr}/api/runs/{run_id}/netflow"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["flows"].as_array().unwrap().len(),
        2,
        "no from/to must behave exactly as before the feature existed: {body}"
    );
    assert!(
        body["window"]["from"].is_null() && body["window"]["to"].is_null(),
        "no filter requested must echo a null window, not omit the key: {body}"
    );
}

/// Minor 4 (Task 3 review round 1): the applied window must be echoed back
/// verbatim so a caller has positive confirmation its filter was honoured,
/// distinct from `dropped_total`'s whole-file scope.
#[tokio::test]
async fn response_echoes_the_applied_window() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(flow_at(
            FlowId::new(),
            "a.example",
            at(150),
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
        "http://{addr}/api/runs/{run_id}/netflow?from={}&to={}",
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Compare parsed instants, not raw strings: chrono's serde
    // serialization renders a UTC `DateTime` with a `Z` suffix, while
    // `.to_rfc3339()` (used by `qs()` to build the request) renders
    // `+00:00` — both are valid RFC 3339 for the same instant, so a
    // string-literal comparison would be a false negative here.
    let got_from: chrono::DateTime<chrono::Utc> =
        body["window"]["from"].as_str().unwrap().parse().unwrap();
    let got_to: chrono::DateTime<chrono::Utc> =
        body["window"]["to"].as_str().unwrap().parse().unwrap();
    assert_eq!(got_from, at(100));
    assert_eq!(got_to, at(200));
}

#[tokio::test]
async fn a_malformed_timestamp_is_a_400_not_a_panic_and_not_a_silent_ignore() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(flow_at(
            FlowId::new(),
            "a.example",
            at(50),
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
        "http://{addr}/api/runs/{run_id}/netflow?from=not-a-date"
    ))
    .await
    .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "a malformed `from` must be rejected, not silently ignored \
         (which would show MORE data than the caller asked to see)"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let msg = body["error"].as_str().unwrap();
    assert!(
        msg.contains("from"),
        "the error must name the offending parameter: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("rfc 3339") || msg.to_lowercase().contains("rfc3339"),
        "the error must name the accepted format: {msg}"
    );
}

#[tokio::test]
async fn a_malformed_to_is_also_a_400() {
    let global = tempfile::tempdir().unwrap();
    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/whatever/netflow?to=garbage"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("to"));
}

#[tokio::test]
async fn from_and_to_are_each_independently_optional() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "early.example", at(50)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "late.example", at(300)))),
        ],
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

    // `from` alone: bounds only the lower side.
    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/{run_id}/netflow?from={}",
        qs(at(100))
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let hosts: std::collections::HashSet<&str> = body["flows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["host"].as_str().unwrap())
        .collect();
    assert_eq!(hosts, std::collections::HashSet::from(["late.example"]));

    // `to` alone: bounds only the upper side.
    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/{run_id}/netflow?to={}",
        qs(at(100))
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let hosts: std::collections::HashSet<&str> = body["flows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["host"].as_str().unwrap())
        .collect();
    assert_eq!(hosts, std::collections::HashSet::from(["early.example"]));
}

/// The requirement that matters most: a narrow window that excludes every
/// surviving flow must NOT make it look like nothing was lost. `dropped`
/// (renamed `dropped_total` on the wire) must still report the whole
/// file's loss, and the bare key `dropped` must be GONE — a caller reading
/// the old name would silently get `null`/`undefined`, which is a louder
/// failure than a wrong number.
#[tokio::test]
async fn dropped_total_is_whole_file_scoped_not_windowed_by_the_filter() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "a.example", at(50)))),
            LedgerLine::Dropped {
                count: 7,
                ts: at(50),
            },
        ],
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
    // A window that contains NO flows at all — the emptiest possible
    // response, exactly where a bare `dropped: 0` would be most
    // misleading if the field were window-scoped instead of file-scoped.
    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/{run_id}/netflow?from={}&to={}",
        qs(at(900)),
        qs(at(1000)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["flows"].as_array().unwrap().len(), 0);
    assert_eq!(
        body["dropped_total"], 7,
        "loss is reported for the whole file even though the window is \
         empty: {body}"
    );
    assert!(
        body.get("dropped").is_none(),
        "the bare, misreadable `dropped` key must not exist on the wire: {body}"
    );
}

#[tokio::test]
async fn project_scope_time_range_filters_flows() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "in.example", at(150)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "out.example", at(500)))),
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/projects/{}/netflow?from={}&to={}",
        ws.id,
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 1, "project scope respects the window: {body}");
    assert_eq!(flows[0]["host"], "in.example");
}

#[tokio::test]
async fn project_scope_malformed_from_is_400() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/projects/{}/netflow?from=nope",
        ws.id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn global_scope_time_range_filters_flows() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "in.example", at(150)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "out.example", at(500)))),
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/netflow?from={}&to={}",
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 1, "global scope respects the window: {body}");
    assert_eq!(flows[0]["host"], "in.example");
}

#[tokio::test]
async fn global_scope_malformed_to_is_400() {
    let global = tempfile::tempdir().unwrap();
    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow?to=nope"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn graph_scope_time_range_filters_edges() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "in.example", at(150)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "out.example", at(500)))),
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/netflow/graph?from={}&to={}",
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let nodes = body["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["id"] == "in.example:443"));
    assert!(
        !nodes.iter().any(|n| n["id"] == "out.example:443"),
        "the out-of-window endpoint must not appear as a node: {body}"
    );
}

/// "Also fold in if cheap" (Task 3 review round 1): the prior
/// `graph_scope_time_range_filters_edges` only exercised the global
/// fall-through (no `scope=` param) — the `run:`/`project:` sub-scopes
/// each have their own read path in `get_netflow_graph`/
/// `run_scoped_flows_for_graph`, so filtering there needs its own proof.
#[tokio::test]
async fn graph_scope_run_time_range_filters_edges() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_graph_tr";

    write_ledger(
        project.path(),
        run_id,
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "in.example", at(150)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "out.example", at(500)))),
        ],
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
        "http://{addr}/api/netflow/graph?scope=run:{run_id}&from={}&to={}",
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let nodes = body["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["id"] == "in.example:443"));
    assert!(
        !nodes.iter().any(|n| n["id"] == "out.example:443"),
        "run scope's graph must respect the window too: {body}"
    );
}

#[tokio::test]
async fn graph_scope_project_time_range_filters_edges() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    write_ledger(
        project.path(),
        "run-a",
        &[
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "in.example", at(150)))),
            LedgerLine::Flow(Box::new(flow_at(FlowId::new(), "out.example", at(500)))),
        ],
    );

    let store = rupu_workspace::WorkspaceStore {
        root: global.path().join("workspaces"),
    };
    let ws = rupu_workspace::upsert(&store, project.path()).unwrap();

    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!(
        "http://{addr}/api/netflow/graph?scope=project:{}&from={}&to={}",
        ws.id,
        qs(at(100)),
        qs(at(200)),
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let nodes = body["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["id"] == "in.example:443"));
    assert!(
        !nodes.iter().any(|n| n["id"] == "out.example:443"),
        "project scope's graph must respect the window too: {body}"
    );
}

/// "Also fold in if cheap": `an_inverted_range_contains_nothing` already
/// proves `from > to` is an empty window at the `TimeRange` unit-test
/// level — this is the HTTP-layer counterpart, proving the route doesn't
/// reject an inverted range as malformed (it's a well-formed pair of
/// timestamps, just an unsatisfiable window) nor silently swap/ignore it.
#[tokio::test]
async fn an_inverted_window_is_a_valid_empty_result_not_a_400() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let run_id = "run_tr";

    write_ledger(
        project.path(),
        run_id,
        &[LedgerLine::Flow(Box::new(flow_at(
            FlowId::new(),
            "a.example",
            at(150),
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
    // `from` (200) is AFTER `to` (100) — well-formed timestamps, inverted
    // order.
    let resp = reqwest::get(format!(
        "http://{addr}/api/runs/{run_id}/netflow?from={}&to={}",
        qs(at(200)),
        qs(at(100)),
    ))
    .await
    .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "an inverted range is a valid (empty) request, not a 400"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["flows"].as_array().unwrap().len(),
        0,
        "no timestamp can satisfy both bounds: {body}"
    );
}

#[tokio::test]
async fn graph_scope_malformed_from_is_400() {
    let global = tempfile::tempdir().unwrap();
    let addr = serve(new_state(global.path())).await;
    let resp = reqwest::get(format!("http://{addr}/api/netflow/graph?from=nope"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
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
