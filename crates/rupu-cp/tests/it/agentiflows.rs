//! `GET /api/agentiflows` and `GET /api/agentiflows/:id`: read-only views of
//! the run directories `rupu agentiflow run` lays out under
//! `<global>/agentiflows/`.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use rupu_agentiflow::{agentiflow_dir, AgentiflowRecord, GoalStatus};
use rupu_runtime::RunTriggerSource;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;

const DONE: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1V2W";
const LIVE: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1V2X";
const UNIT_A: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V01";
const UNIT_B: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0T1V02";

const DEF: &str = r#"
name: itest
description: A fixture flow.
lead: lead
engagement_profiles: [code]
goals:
  - id: issues
    objective: "  Find 3 issues.  "
    target: { findings: {}, count_gte: 3 }
budget: { usd: 5.0, wall_clock: 30m, rounds: 6, soft_at: 0.8 }
scope: { authorized: true }
pool: { agents: [lead, reviewer] }
round: { lead_max_turns: 40, ceiling: { rounds: 6 } }
"#;

async fn spawn_server(dir: &Path) -> SocketAddr {
    spawn_server_with(dir, false).await
}

/// Any launcher: the CP's "this is `cp serve`" signal, which steering needs.
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

async fn spawn_server_with(dir: &Path, writable: bool) -> SocketAddr {
    let mut state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    if writable {
        state = state.with_launcher(Some(std::sync::Arc::new(NoopLauncher)));
    }
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn record(id: &str, started: &str, status: &str) -> AgentiflowRecord {
    AgentiflowRecord {
        id: id.into(),
        name: "itest".into(),
        engagement_profiles: vec!["code".into()],
        trigger: RunTriggerSource::Agentiflow,
        status: status.into(),
        stop_reason: (status == "completed").then(|| "goals_met".into()),
        rounds: 1,
        goals: vec![
            GoalStatus {
                id: "issues".into(),
                met: true,
                current: 3,
                target: 3,
            },
            GoalStatus {
                id: "extra".into(),
                met: false,
                current: 0,
                target: 1,
            },
        ],
        started_at: started.parse().unwrap(),
        ended_at: None,
        codename: None,
        spent_usd: Some(1.25),
        spent_tokens: 4_200,
        runner_pid: None,
    }
}

fn write_unit(run_dir: &Path, id: &str, agent: &str, status: Value) {
    let dir = run_dir.join("units").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("unit.json"),
        json!({
            "run_id": id, "agent": agent, "kind": "agent",
            "participant": format!("{agent}#1"), "pgid": 77,
            "started_at": "2026-10-07T01:00:00+00:00", "status": status,
        })
        .to_string(),
    )
    .unwrap();
}

/// A finished run with a definition snapshot, events, two units (one with a
/// transcript on disk whose `RunStart` stored a codename) and two lead rounds.
fn seed(global: &Path) {
    let root = agentiflow_dir(global);
    let run = root.join(DONE);
    record(DONE, "2026-10-07T00:00:00Z", "completed")
        .write(&run)
        .unwrap();
    std::fs::write(run.join("agentiflow.yaml"), DEF).unwrap();
    std::fs::write(
        run.join("events.jsonl"),
        [
            json!({"ts": "2026-10-07T00:00:00.000Z", "kind": "run_started", "id": DONE,
                   "name": "itest", "goals": 1, "engagement_profiles": ["code"]}),
            json!({"ts": "2026-10-07T00:01:00.000Z", "kind": "round", "round": 0,
                   "budget": "soft", "converge": false, "goals_met": 0, "goals_total": 1,
                   "steering": 0, "outcome": "yielded", "spent_usd": 1.25,
                   "spent_tokens": 4200}),
            json!({"ts": "2026-10-07T00:02:00.000Z", "kind": "run_stopped",
                   "stop_reason": "goals_met", "detail": "d", "rounds": 1, "goals": [],
                   "summary": "s", "spent_usd": 1.25, "spent_tokens": 4200}),
        ]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect::<String>(),
    )
    .unwrap();
    std::fs::create_dir_all(run.join("lead")).unwrap();
    for name in ["transcript.r1.jsonl", "transcript.r0.jsonl"] {
        std::fs::write(run.join("lead").join(name), "").unwrap();
    }
    write_unit(
        &run,
        UNIT_A,
        "reviewer",
        json!({"state": "done", "success": true, "output": "found it"}),
    );
    write_unit(
        &run,
        UNIT_B,
        "reviewer",
        json!({"state": "failed", "error": "killed"}),
    );
    // UNIT_A ran as a real `rupu run`: its transcript's RunStart stored a name.
    let transcripts = global.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::write(
        transcripts.join(format!("{UNIT_A}.jsonl")),
        json!({"type": "run_start", "data": {
            "run_id": UNIT_A, "workspace_id": "ws_x", "agent": "reviewer",
            "provider": "mock", "model": "m", "started_at": "2026-10-07T01:00:01Z",
            "mode": "bypass", "codename": "topaz-pass/ferret"}})
        .to_string()
            + "\n",
    )
    .unwrap();

    // A still-`running` record, newer than DONE, with no definition snapshot.
    record(LIVE, "2026-10-07T05:00:00Z", "running")
        .write(&root.join(LIVE))
        .unwrap();
}

async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let resp = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn list_is_newest_first_with_goal_counts_and_derived_codenames() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;

    let (status, body) = get(addr, "/api/agentiflows").await;
    assert_eq!(status, 200, "{body}");
    let rows = body["rows"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [LIVE, DONE]);

    let done = &rows[1];
    assert_eq!(done["name"], "itest");
    assert_eq!(done["status"], "completed");
    assert_eq!(done["stop_reason"], "goals_met");
    assert_eq!(done["rounds"], 1);
    assert_eq!(
        (done["goals_met"].as_u64(), done["goals_total"].as_u64()),
        (Some(1), Some(2))
    );
    assert_eq!(done["codename_derived"], true);
    assert!(!done["codename"].as_str().unwrap().is_empty());
    assert_eq!(done["spent_usd"], 1.25);
    assert_eq!(done["spent_tokens"], 4200);
    assert_eq!(done["engagement_profiles"], json!(["code"]));
    assert!(done["started_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(done["runner_alive"], Value::Null);
    // Slim: the detail-only keys are not on a row.
    assert!(done.get("events").is_none() && done.get("units").is_none());
}

#[tokio::test]
async fn list_with_no_agentiflows_dir_is_empty_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let addr = spawn_server(tmp.path()).await;
    let (status, body) = get(addr, "/api/agentiflows").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"rows": []}));
}

#[tokio::test]
async fn detail_carries_record_def_events_units_and_lead_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;

    let (status, d) = get(addr, &format!("/api/agentiflows/{DONE}")).await;
    assert_eq!(status, 200, "{d}");

    // record
    assert_eq!(d["record"]["id"], DONE);
    assert_eq!(d["record"]["trigger"], "agentiflow");
    assert_eq!(d["record"]["codename_derived"], true);
    assert_eq!(d["record"]["goals"][0]["id"], "issues");
    assert_eq!(d["record"]["goals"][0]["met"], true);
    assert_eq!(d["record"]["goals"][1]["met"], false);

    // def: the objective is trimmed, the predicate spelled out.
    assert_eq!(d["def"]["name"], "itest");
    assert_eq!(d["def"]["goals"][0]["objective"], "Find 3 issues.");
    assert_eq!(d["def"]["goals"][0]["predicate"], "findings, count >= 3");
    assert_eq!(d["def"]["goals"][0]["required"], true);
    assert_eq!(d["def"]["budget"]["usd"], 5.0);
    assert_eq!(d["def"]["budget"]["wall_clock"], "30m");
    assert_eq!(d["def"]["pool"]["agents"], json!(["lead", "reviewer"]));
    assert_eq!(d["def"]["round"]["ceiling"]["rounds"], 6);
    assert_eq!(d["def"]["scope"]["authorized"], true);
    assert_eq!(d["def"]["coverage"], Value::Null);

    // events + the budget state of the last round
    let kinds: Vec<&str> = d["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["run_started", "round", "run_stopped"]);
    assert_eq!(d["budget_state"], "soft");

    // units: ordered by id; a transcript's stored codename wins over the
    // derivation, and a unit with none is derived and flagged.
    let units = d["units"].as_array().unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[0]["unit_id"], UNIT_A);
    assert_eq!(units[0]["agent"], "reviewer");
    assert_eq!(units[0]["participant"], "reviewer#1");
    assert_eq!(units[0]["kind"], "agent");
    assert_eq!(units[0]["pgid"], 77);
    assert_eq!(units[0]["codename"], "topaz-pass/ferret");
    assert_eq!(units[0]["codename_derived"], false);
    assert_eq!(
        units[0]["status"],
        json!({"state": "done", "success": true, "output": "found it"})
    );
    assert!(units[0]["transcript_path"]
        .as_str()
        .unwrap()
        .ends_with(&format!("{UNIT_A}.jsonl")));
    assert_eq!(units[1]["unit_id"], UNIT_B);
    assert_eq!(units[1]["codename_derived"], true);
    assert_eq!(
        units[1]["status"],
        json!({"state": "failed", "error": "killed"})
    );
    assert_eq!(units[1]["transcript_path"], Value::Null);

    // lead transcripts, by round
    let rounds: Vec<u64> = d["lead_transcripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["round"].as_u64().unwrap())
        .collect();
    assert_eq!(rounds, [0, 1]);
}

#[tokio::test]
async fn detail_of_a_run_with_no_snapshot_or_events_is_still_served() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;
    let (status, d) = get(addr, &format!("/api/agentiflows/{LIVE}")).await;
    assert_eq!(status, 200, "{d}");
    assert_eq!(d["record"]["status"], "running");
    assert_eq!(d["def"], Value::Null);
    assert_eq!(d["events"], json!([]));
    assert_eq!(d["units"], json!([]));
    assert_eq!(d["budget_state"], Value::Null);
}

#[tokio::test]
async fn detail_404s_an_unknown_or_unsafe_id() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;
    for id in ["af_missing", "run_01J9ZQ3K4M5N6P7Q8R9S0T1V2W", "af_a%20b"] {
        let (status, _) = get(addr, &format!("/api/agentiflows/{id}")).await;
        assert_eq!(status, 404, "{id}");
    }
    // An encoded separator never reaches the handler: the router-level path
    // guard refuses it (`rupu_cp::path_guard`).
    let (status, _) = get(addr, "/api/agentiflows/af_..%2Fx").await;
    assert_eq!(status, 400);
}

/// A pid that existed and is gone: a child that ran to completion and was
/// reaped.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

async fn steer(addr: SocketAddr, id: &str) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/agentiflows/{id}/steer"))
        .json(&json!({ "message": "focus on auth" }))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn steer_needs_a_live_coordinator_and_cp_serve() {
    let tmp = tempfile::tempdir().unwrap();
    let root = agentiflow_dir(tmp.path());
    let live = root.join(LIVE);
    let mut rec = record(LIVE, "2026-10-07T00:00:00Z", "running");
    rec.runner_pid = Some(std::process::id());
    rec.write(&live).unwrap();

    // A read-only CP (no `cp serve` runtime) refuses the write.
    let ro = spawn_server_with(tmp.path(), false).await;
    assert_eq!(steer(ro, LIVE).await.0, 501);

    let addr = spawn_server_with(tmp.path(), true).await;
    let (status, body) = steer(addr, LIVE).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["queued"], true);
    assert_eq!(
        rupu_agentiflow::OperatorQueue::new(&live)
            .drain()
            .unwrap()
            .len(),
        1
    );

    // `running` on disk but the coordinator is dead: nothing would drain it.
    rec.runner_pid = Some(dead_pid());
    rec.write(&live).unwrap();
    let (status, body) = steer(addr, LIVE).await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("no longer running"));
    assert!(rupu_agentiflow::OperatorQueue::new(&live)
        .drain()
        .unwrap()
        .is_empty());

    // Finished, or unknown.
    seed(tmp.path());
    assert_eq!(steer(addr, DONE).await.0, 409);
    assert_eq!(steer(addr, "af_missing").await.0, 404);
}

#[tokio::test]
async fn events_feed_merges_coordinator_lifecycle_with_unit_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;

    let (status, body) = get(addr, &format!("/api/agentiflows/{DONE}/events")).await;
    assert_eq!(status, 200, "{body}");
    let evs = body["events"].as_array().unwrap();
    let count = |k: &str| evs.iter().filter(|e| e["kind"] == k).count();
    // 3 coordinator lifecycle lines + 2 units × (started + completed).
    assert_eq!(count("run_started"), 1);
    assert_eq!(count("round"), 1);
    assert_eq!(count("run_stopped"), 1);
    assert_eq!(count("af_unit_started"), 2);
    assert_eq!(count("af_unit_completed"), 2);

    // A completed unit carries its agent, verdict and output; UNIT_A's stored
    // transcript codename wins over a derived one.
    let done_a = evs
        .iter()
        .find(|e| e["kind"] == "af_unit_completed" && e["unit_id"] == UNIT_A)
        .unwrap();
    assert_eq!(done_a["agent"], "reviewer");
    assert_eq!(done_a["success"], true);
    assert_eq!(done_a["output"], "found it");
    assert_eq!(done_a["codename"], "topaz-pass/ferret");
    let failed_b = evs
        .iter()
        .find(|e| e["kind"] == "af_unit_completed" && e["unit_id"] == UNIT_B)
        .unwrap();
    // A failed unit never reported success (`success` is "done only"), so it is
    // absent → null here; its failure shows in `error`.
    assert_ne!(failed_b["success"], Value::Bool(true));
    assert_eq!(failed_b["error"], "killed");

    // Oldest-first: the feed is sorted by timestamp.
    let ts: Vec<&str> = evs.iter().map(|e| e["ts"].as_str().unwrap_or("")).collect();
    let mut sorted = ts.clone();
    sorted.sort_unstable();
    assert_eq!(ts, sorted, "events are oldest-first");

    // A run id that names no run is a 404.
    assert_eq!(get(addr, "/api/agentiflows/af_missing/events").await.0, 404);
}
