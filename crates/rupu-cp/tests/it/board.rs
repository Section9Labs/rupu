//! The board endpoints: `GET /api/agentiflows/:id/board` (this host only) and
//! `GET /api/runs/:id/board[?host=]`, routed to a remote host through its
//! connector (`HostConnector::run_board`).

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use rupu_agentiflow::{agentiflow_dir, AgentiflowRecord};
use rupu_fleet::{Board, ClaimOutcome, Directive};
use rupu_orchestrator::runs::RunStatus;
use serde_json::{json, Value};

const RUN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0TBOARD";
const FLOW: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0TBOARD";

async fn serve(state: rupu_cp::state::AppState) -> SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn state(global: &Path) -> rupu_cp::state::AppState {
    rupu_cp::state::AppState::new(global.into(), rupu_config::PricingConfig::default())
}

async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// On the board at `space`: `auth-flow` held by `lynx#1`, `recon` claimed
/// and released by `heron#1`, a live directive and a retracted one.
fn seed_board(space: &Path) {
    let board = Board::new(space);
    match board
        .claim("auth-flow", "lynx#1", Duration::from_secs(3600))
        .unwrap()
    {
        // Held for the rest of the test: the guard would release it on drop.
        ClaimOutcome::Granted(guard) => std::mem::forget(guard),
        other => panic!("{other:?}"),
    }
    match board
        .claim("recon", "heron#1", Duration::from_secs(3600))
        .unwrap()
    {
        ClaimOutcome::Granted(guard) => drop(guard),
        other => panic!("{other:?}"),
    }
    let directive = |body: &str| Directive {
        id: String::new(),
        author: "lead".into(),
        ts: "2026-10-21T10:00:00Z".into(),
        body: body.into(),
        addressed_to: None,
    };
    board.put_directive(&directive("focus on login")).unwrap();
    let gone = board.put_directive(&directive("skip billing")).unwrap();
    board.retract_directive(&gone).unwrap();
}

/// `(key, owner, state)` of each claim, and `(body, retracted)` of each
/// directive, in the order served.
type Summary = (Vec<(String, String, String)>, Vec<(String, bool)>);

fn summary(view: &Value) -> Summary {
    let s = |v: &Value| v.as_str().unwrap().to_string();
    let claims = view["claims"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (s(&c["key"]), s(&c["owner"]), s(&c["state"])))
        .collect();
    let directives = view["directives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (s(&d["body"]), d["retracted"].as_bool().unwrap()))
        .collect();
    (claims, directives)
}

fn expected() -> Summary {
    let c = |k: &str, o: &str, st: &str| (k.to_string(), o.to_string(), st.to_string());
    (
        vec![
            c("auth-flow", "lynx#1", "held"),
            c("recon", "heron#1", "released"),
        ],
        vec![
            ("focus on login".to_string(), false),
            ("skip billing".to_string(), true),
        ],
    )
}

fn flow_record(id: &str) -> AgentiflowRecord {
    AgentiflowRecord {
        id: id.into(),
        name: "board-test".into(),
        engagement_profiles: vec![],
        trigger: rupu_runtime::RunTriggerSource::Agentiflow,
        status: "running".into(),
        stop_reason: None,
        rounds: 2,
        goals: vec![],
        started_at: "2026-10-21T10:00:00Z".parse().unwrap(),
        ended_at: None,
        codename: None,
        spent_usd: None,
        spent_tokens: 0,
        runner_pid: None,
    }
}

#[tokio::test]
async fn agentiflow_board_serves_claims_and_directives() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let dir = agentiflow_dir(g).join(FLOW);
    flow_record(FLOW).write(&dir).unwrap();
    seed_board(&dir);
    let addr = serve(state(g)).await;

    let (status, view) = get(addr, &format!("/api/agentiflows/{FLOW}/board")).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(summary(&view), expected());
    assert_eq!(view["journal"], json!(true));
    assert_eq!(view["claims"][0]["key_known"], json!(true));
    assert!(view["claims"][1]["ended_at"].is_string(), "{view}");

    let (status, body) = get(addr, &format!("/api/agentiflows/{FLOW}/board?host=peer")).await;
    assert_eq!(status, 501, "{body}");
    let (status, _) = get(addr, "/api/agentiflows/af_01NOSUCHFLOW/board").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn run_board_local() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    super::messages::workflow_run(g, RUN, RunStatus::Running);
    seed_board(&g.join("runs").join(RUN).join("messages"));
    let addr = serve(state(g)).await;

    let (status, view) = get(addr, &format!("/api/runs/{RUN}/board")).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(summary(&view), expected());
    let (status, local) = get(addr, &format!("/api/runs/{RUN}/board?host=local")).await;
    assert_eq!(status, 200);
    assert_eq!(local, view);

    // A run with no board yet is an empty view; an unknown run, or an id
    // that isn't a run's, is a 404.
    super::messages::workflow_run(g, "run_01J9ZQ3K4M5N6P7Q8R9S0TEMPTY", RunStatus::Running);
    let (status, empty) = get(addr, "/api/runs/run_01J9ZQ3K4M5N6P7Q8R9S0TEMPTY/board").await;
    assert_eq!(status, 200, "{empty}");
    assert_eq!(
        empty,
        json!({"claims": [], "directives": [], "journal": false})
    );
    assert_eq!(get(addr, "/api/runs/run_01NOSUCHRUN/board").await.0, 404);
    assert_eq!(get(addr, &format!("/api/runs/{FLOW}/board")).await.0, 404);
}

#[tokio::test]
async fn run_board_remote_is_501_on_tunnel() {
    let tmp = tempfile::tempdir().unwrap();
    let g = tmp.path();
    let store = rupu_workspace::HostStore {
        root: g.join("hosts"),
    };
    let (tunnel, _token) = rupu_workspace::enroll_node(&store, "node-a").unwrap();
    let bucket_dir = tempfile::tempdir().unwrap();
    let bucket = rupu_workspace::add_bucket_host(
        &store,
        "drop",
        &format!("file://{}", bucket_dir.path().display()),
        None,
    )
    .unwrap();
    let addr = serve(state(g)).await;

    for (transport, host) in [("tunnel", &tunnel.id), ("bucket", &bucket.id)] {
        let (status, body) = get(addr, &format!("/api/runs/{RUN}/board?host={host}")).await;
        assert_eq!(status, 501, "{transport}: {body}");
        let reason = body["error"].as_str().unwrap();
        assert!(
            reason.contains(&format!("a {transport} host has no board transport")),
            "{reason}"
        );
    }
}

/// A peer CP advertising `run.board` serves its runs' boards through the
/// coordinator, as it serves them itself.
#[tokio::test]
async fn http_host_proxies_the_board() {
    let peer_dir = tempfile::tempdir().unwrap();
    super::messages::workflow_run(peer_dir.path(), RUN, RunStatus::Running);
    seed_board(&peer_dir.path().join("runs").join(RUN).join("messages"));
    let peer = serve(state(peer_dir.path())).await;

    let tmp = tempfile::tempdir().unwrap();
    let coordinator = state(tmp.path());
    let host = coordinator
        .hosts
        .add_host("peer", &format!("http://{peer}"), None)
        .unwrap()
        .id;
    let addr = serve(coordinator).await;

    let (status, direct) = get(peer, &format!("/api/runs/{RUN}/board")).await;
    assert_eq!(status, 200);
    let (status, proxied) = get(addr, &format!("/api/runs/{RUN}/board?host={host}")).await;
    assert_eq!(status, 200, "{proxied}");
    assert_eq!(proxied, direct);
    let (status, body) = get(
        addr,
        &format!("/api/runs/run_01NOSUCHRUN/board?host={host}"),
    )
    .await;
    assert_eq!(status, 404, "{body}");
}

/// A peer whose `/api/host/info` does not list `run.board` is never asked
/// for a board: the coordinator answers 501 with the reason.
#[tokio::test]
async fn http_without_run_board_is_unsupported() {
    let mock = httpmock::MockServer::start_async().await;
    let info = mock
        .mock_async(|when, then| {
            when.method("GET").path("/api/host/info");
            then.status(200).json_body(
                json!({ "version": "0.85.0", "features": ["run.list", "run.messages"] }),
            );
        })
        .await;
    let board = mock
        .mock_async(|when, then| {
            when.path(format!("/api/runs/{RUN}/board"));
            then.status(200)
                .json_body(json!({"claims": [], "directives": [], "journal": true}));
        })
        .await;
    let tmp = tempfile::tempdir().unwrap();
    let coordinator = state(tmp.path());
    let host = coordinator
        .hosts
        .add_host("old-peer", &mock.base_url(), None)
        .unwrap()
        .id;
    let addr = serve(coordinator).await;

    let (status, body) = get(addr, &format!("/api/runs/{RUN}/board?host={host}")).await;
    assert_eq!(status, 501, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("upgrade rupu there"),
        "{body}"
    );
    assert_eq!(board.hits_async().await, 0);
    assert_eq!(info.hits_async().await, 1);
}
