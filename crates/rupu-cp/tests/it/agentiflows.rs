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

#[tokio::test]
async fn children_endpoint_returns_the_flow_launch_tree() {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;

    let (status, body) = get(addr, &format!("/api/agentiflows/{DONE}/children")).await;
    assert_eq!(status, 200);
    let kids = body["children"].as_array().unwrap();
    let ids: Vec<&str> = kids
        .iter()
        .map(|c| c["run_id"].as_str().unwrap())
        .collect();
    // Both legacy `units/` mirrors surface as children of the flow.
    assert!(ids.contains(&UNIT_A) && ids.contains(&UNIT_B), "got {ids:?}");

    let a = kids.iter().find(|c| c["run_id"] == UNIT_A).unwrap();
    assert_eq!(a["parent_run_id"], DONE);
    assert_eq!(a["kind"], "agent");
    assert_eq!(a["status"], "done");
    assert_eq!(a["legacy"], true);
    // The codename UNIT_A's own transcript `RunStart` recorded.
    assert_eq!(a["codename"], "topaz-pass/ferret");

    let b = kids.iter().find(|c| c["run_id"] == UNIT_B).unwrap();
    assert_eq!(b["status"], "failed");
    assert_eq!(b["error"], "killed");

    // An unknown id is a 404, like the other per-run endpoints.
    assert_eq!(get(addr, "/api/agentiflows/af_missing/children").await.0, 404);
}

/// Append a children-ledger `started` line for a W7 unit under the flow dir.
fn ledger_unit_line(run_dir: &Path, run_id: &str) {
    rupu_launch::ledger::append(
        run_dir,
        &rupu_launch::ledger::LedgerLine::Started(rupu_launch::ledger::Started::new(
            &format!("ch_{run_id}"),
            rupu_tools::ChildKind::Agent,
            "recon",
            run_id,
            1,
            1,
        )),
    );
}

/// Append a `finished` line (done) for that unit, with or without a recorded
/// answer.
fn ledger_finished(run_dir: &Path, run_id: &str, output: Option<&str>) {
    rupu_launch::ledger::append(
        run_dir,
        &rupu_launch::ledger::LedgerLine::Finished(rupu_launch::ledger::Finished {
            success: Some(true),
            output: output.map(str::to_string),
            ..rupu_launch::ledger::Finished::new(
                &format!("ch_{run_id}"),
                rupu_tools::ChildState::Done,
                None,
            )
        }),
    );
}

#[tokio::test]
async fn unit_output_rides_the_ledger_and_falls_back_to_the_transcript() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let af = "af_01LAZYOUTPUT000000000000000";
    let run = agentiflow_dir(global).join(af);
    record(af, "2026-10-14T00:00:00Z", "completed")
        .write(&run)
        .unwrap();

    // Unit A: its `finished` ledger line carries the answer, so it rides along
    // in the list for free — no transcript read.
    let a = "run_01LAZYUNITA0000000000000000";
    ledger_unit_line(&run, a);
    ledger_finished(&run, a, Some("answer from the ledger"));

    // Unit B: finished, but its ledger line records no answer; its transcript
    // has one. The list omits it; it is served on demand from the transcript.
    let b = "run_01LAZYUNITB0000000000000000";
    ledger_unit_line(&run, b);
    ledger_finished(&run, b, None);
    let transcripts = global.join("transcripts");
    std::fs::create_dir_all(&transcripts).unwrap();
    std::fs::write(
        transcripts.join(format!("{b}.jsonl")),
        "{\"type\":\"run_start\",\"data\":{\"run_id\":\"u\",\"workspace_id\":\"ws\",\
         \"agent\":\"recon\",\"provider\":\"mock\",\"model\":\"m\",\
         \"started_at\":\"2026-10-14T00:00:01Z\",\"mode\":\"bypass\"}}\n\
         {\"type\":\"assistant_message\",\"data\":{\"content\":\"answer from the transcript\"}}\n",
    )
    .unwrap();
    let addr = spawn_server(global).await;

    let (_, detail) = get(addr, &format!("/api/agentiflows/{af}")).await;
    let units = detail["units"].as_array().unwrap();
    let ua = units.iter().find(|u| u["unit_id"] == a).unwrap();
    let ub = units.iter().find(|u| u["unit_id"] == b).unwrap();
    // A: the ledger answer rides the list.
    assert_eq!(ua["status"]["output"], "answer from the ledger");
    // B: no ledger answer → the list omits it (fetched on demand instead).
    assert!(
        ub["status"].get("output").is_none() || ub["status"]["output"].is_null(),
        "a unit with no ledger output must be omitted from the list: {ub}"
    );

    // On demand, B's answer comes from its transcript.
    let (st, body) = get(addr, &format!("/api/agentiflows/{af}/units/{b}/output")).await;
    assert_eq!(st, 200);
    assert_eq!(body["output"], "answer from the transcript");

    // A unit id that is not under this flow is a 404.
    assert_eq!(
        get(addr, &format!("/api/agentiflows/{af}/units/run_nope/output"))
            .await
            .0,
        404
    );
}

#[tokio::test]
async fn unit_output_falls_back_to_a_legacy_unit_json() {
    // A legacy `units/*/unit.json` records its answer directly; the endpoint
    // serves it even when the unit's transcript has no answer turn.
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let addr = spawn_server(tmp.path()).await;
    let (st, body) = get(addr, &format!("/api/agentiflows/{DONE}/units/{UNIT_A}/output")).await;
    assert_eq!(st, 200);
    assert_eq!(body["output"], "found it");
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
    assert_eq!(status, 202, "{body}");
    assert!(body["id"].as_str().unwrap().starts_with("msg_"), "{body}");
    let logged = || {
        rupu_fleet::Bus::open(&live, rupu_fleet::address::OPERATOR)
            .read_log(None, usize::MAX)
            .unwrap()
            .lines
    };
    match logged().as_slice() {
        [rupu_fleet::LogLine::Msg(m)] => {
            assert_eq!(m.kind, rupu_fleet::MessageKind::Operator);
            assert_eq!(m.body, "focus on auth");
            assert_eq!(m.via, Some(rupu_fleet::OperatorVia::Cp));
        }
        other => panic!("{other:?}"),
    }

    // `running` on disk but the coordinator is dead: nothing would drain it.
    rec.runner_pid = Some(dead_pid());
    rec.write(&live).unwrap();
    let (status, body) = steer(addr, LIVE).await;
    assert_eq!(status, 409, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("no longer running"));
    assert_eq!(logged().len(), 1, "nothing more was sent");

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

#[tokio::test]
async fn detail_asset_scope_resolves_to_the_engagement_coverage_target() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();

    // A registered workspace the lead ran in.
    let ws_path = tmp.path().join("proj");
    std::fs::create_dir_all(&ws_path).unwrap();
    std::fs::create_dir_all(global.join("workspaces")).unwrap();
    let ws_id = "ws_01SCOPETEST0000000000000000";
    std::fs::write(
        global.join("workspaces").join(format!("{ws_id}.toml")),
        format!(
            "id = {:?}\npath = {:?}\ncreated_at = {:?}\n",
            ws_id,
            ws_path.display().to_string(),
            "2026-10-13T00:00:00+00:00"
        ),
    )
    .unwrap();

    // An af whose lead RunStart names that workspace.
    let af = "af_01SCOPETEST0000000000000000";
    let run = agentiflow_dir(global).join(af);
    record(af, "2026-10-13T00:00:00Z", "running")
        .write(&run)
        .unwrap();
    std::fs::create_dir_all(run.join("lead")).unwrap();
    std::fs::write(
        run.join("lead").join("transcript.r0.jsonl"),
        json!({"type": "run_start", "data": {
            "run_id": "lead_1", "workspace_id": ws_id, "agent": "lead",
            "provider": "mock", "model": "m", "started_at": "2026-10-13T00:00:00Z",
            "mode": "bypass"}})
        .to_string()
            + "\n",
    )
    .unwrap();

    let addr = spawn_server(global).await;
    let (status, d) = get(addr, &format!("/api/agentiflows/{af}")).await;
    assert_eq!(status, 200, "{d}");
    // Scoped to the engagement's own coverage target, not every workspace's.
    assert_eq!(d["asset_scope"]["ws_id"], ws_id, "{d}");
    assert_eq!(
        d["asset_scope"]["target_id"].as_str().unwrap(),
        rupu_coverage::target_id(&ws_path, af),
        "{d}"
    );

    // No lead transcript → no scope at all (the tab says so; it never falls
    // back to showing every workspace's assets).
    let af2 = "af_01NOSCOPE00000000000000000";
    record(af2, "2026-10-13T00:00:00Z", "running")
        .write(&agentiflow_dir(global).join(af2))
        .unwrap();
    let (status2, d2) = get(addr, &format!("/api/agentiflows/{af2}")).await;
    assert_eq!(status2, 200, "{d2}");
    assert!(
        d2.get("asset_scope").is_none() || d2["asset_scope"].is_null(),
        "unscoped af must not carry an asset_scope: {d2}"
    );
}

/// A `started` line in `dir`'s children ledger, as a lead round launches a
/// child: `round` and, for a workflow child, the file it was dispatched
/// from (`generated` when `workflows.generate` wrote it).
fn launched(
    dir: &Path,
    handle: &str,
    kind: rupu_tools::ChildKind,
    run_id: &str,
    depth: u32,
    round: Option<u32>,
    workflow: Option<(&Path, bool)>,
) {
    use rupu_launch::ledger::{LedgerLine, Request, Started};
    let request = workflow.map(|(file, generated)| {
        Box::new(Request {
            mode: rupu_tools::PermissionMode::Bypass,
            link: None,
            prompt: None,
            workflow: Some(file.to_path_buf()),
            inputs: Default::default(),
            generated,
        })
    });
    rupu_launch::ledger::append(
        dir,
        &LedgerLine::Started(Started {
            round,
            request,
            ..Started::new(handle, kind, "sweep", run_id, 1, depth)
        }),
    );
}

#[tokio::test]
async fn children_carry_round_generated_and_excerpt() {
    use rupu_tools::ChildKind;
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let af = "af_01ROUNDGENERATED00000000000";
    let run = agentiflow_dir(global).join(af);
    record(af, "2026-10-21T00:00:00Z", "running")
        .write(&run)
        .unwrap();
    let file = run.join("generated").join("auth-sweep-01X.yaml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "name: auth-sweep\nsteps: []\n").unwrap();
    // Round 2 dispatched the workflow the lead wrote; it ran a sub-agent.
    let wf = "run_01ROUNDGENWF000000000000000";
    let sub = "sub_01ROUNDGENSUB00000000000000";
    launched(
        &run,
        "ch_wf",
        ChildKind::Workflow,
        wf,
        1,
        Some(2),
        Some((&file, true)),
    );
    let answer = format!("  {}é{}  ", "a".repeat(239), "b".repeat(100));
    rupu_launch::ledger::append(
        &run,
        &rupu_launch::ledger::LedgerLine::Finished(rupu_launch::ledger::Finished {
            success: Some(true),
            output: Some(answer),
            ..rupu_launch::ledger::Finished::new("ch_wf", rupu_tools::ChildState::Done, None)
        }),
    );
    launched(
        &global.join("runs").join(wf),
        "ch_sub",
        ChildKind::Subagent,
        sub,
        2,
        None,
        None,
    );
    let addr = spawn_server(global).await;

    let (status, body) = get(addr, &format!("/api/agentiflows/{af}/children")).await;
    assert_eq!(status, 200, "{body}");
    let kids = body["children"].as_array().unwrap();
    let of = |id: &str| kids.iter().find(|c| c["run_id"] == id).unwrap().clone();
    let (w, s) = (of(wf), of(sub));
    assert_eq!((&w["round"], &w["generated"]), (&json!(2), &json!(true)));
    assert_eq!(
        w["output_excerpt"],
        json!(format!("{}é…", "a".repeat(239))),
        "240 chars, cut on a char boundary"
    );
    // The sub-agent sits under the round-2 child: its round is its root's.
    assert_eq!((&s["round"], &s["generated"]), (&json!(2), &json!(false)));
    assert!(s.get("output_excerpt").is_none(), "{s}");

    let (_, detail) = get(addr, &format!("/api/agentiflows/{af}")).await;
    let unit = detail["units"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["unit_id"] == wf)
        .unwrap()
        .clone();
    assert_eq!(
        (&unit["round"], &unit["generated"]),
        (&json!(2), &json!(true))
    );
}

#[tokio::test]
async fn children_without_round_infer_nothing() {
    use rupu_tools::ChildKind;
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let af = "af_01NOROUND000000000000000000";
    let run = agentiflow_dir(global).join(af);
    record(af, "2026-10-21T00:00:00Z", "completed")
        .write(&run)
        .unwrap();
    // A ledger from before rounds and `generated` were recorded.
    let wf = "run_01NOROUNDWF0000000000000000";
    let named = global.join("workflows").join("sweep.yaml");
    launched(
        &run,
        "ch_wf",
        ChildKind::Workflow,
        wf,
        1,
        None,
        Some((&named, false)),
    );
    launched(
        &global.join("runs").join(wf),
        "ch_sub",
        ChildKind::Subagent,
        "sub_01NOROUNDSUB000000000000000",
        2,
        None,
        None,
    );
    let addr = spawn_server(global).await;

    let (status, body) = get(addr, &format!("/api/agentiflows/{af}/children")).await;
    assert_eq!(status, 200, "{body}");
    let kids = body["children"].as_array().unwrap();
    assert_eq!(kids.len(), 2, "{body}");
    for c in kids {
        assert!(c.get("round").is_none(), "{c}");
        assert_eq!(c["generated"], json!(false), "{c}");
    }
    let (_, detail) = get(addr, &format!("/api/agentiflows/{af}")).await;
    let unit = &detail["units"][0];
    assert!(unit.get("round").is_none(), "{unit}");
    assert_eq!(unit["generated"], json!(false), "{unit}");
}

#[tokio::test]
async fn generated_definition_is_served_and_path_checked() {
    use rupu_tools::ChildKind;
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let af = "af_01GENERATEDDEF0000000000000";
    let run = agentiflow_dir(global).join(af);
    record(af, "2026-10-21T00:00:00Z", "running")
        .write(&run)
        .unwrap();
    let generated = run.join("generated");
    std::fs::create_dir_all(&generated).unwrap();
    let yaml = "name: auth-sweep\nsteps:\n  - id: scan\n    agent: recon\n";
    let file = generated.join("auth-sweep-01X.yaml");
    std::fs::write(&file, yaml).unwrap();
    launched(
        &run,
        "ch_gen",
        ChildKind::Workflow,
        "run_01GENA",
        1,
        Some(1),
        Some((&file, true)),
    );

    // Outside `generated/`: a file reached by `..`, and one by a symlink.
    let outside = global.join("secret.yaml");
    std::fs::write(&outside, "name: secret\n").unwrap();
    let dotdot = generated
        .join("..")
        .join("..")
        .join("..")
        .join("secret.yaml");
    launched(
        &run,
        "ch_dots",
        ChildKind::Workflow,
        "run_01GENB",
        1,
        Some(1),
        Some((&dotdot, true)),
    );
    let link = generated.join("link.yaml");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    launched(
        &run,
        "ch_link",
        ChildKind::Workflow,
        "run_01GENC",
        1,
        Some(1),
        Some((&link, true)),
    );
    // A directory under `generated/`: no definition, refused like one outside.
    let dir = generated.join("dir.yaml");
    std::fs::create_dir_all(&dir).unwrap();
    launched(
        &run,
        "ch_dir",
        ChildKind::Workflow,
        "run_01GENF",
        1,
        Some(1),
        Some((&dir, true)),
    );
    // A named pool workflow, and an agent child.
    launched(
        &run,
        "ch_named",
        ChildKind::Workflow,
        "run_01GEND",
        1,
        Some(1),
        Some((&outside, false)),
    );
    launched(
        &run,
        "ch_agent",
        ChildKind::Agent,
        "run_01GENE",
        1,
        Some(1),
        None,
    );
    let addr = spawn_server(global).await;

    let (status, body) = get(addr, &format!("/api/agentiflows/{af}/generated/ch_gen")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({
            "handle": "ch_gen",
            "name": "sweep",
            "file_name": "auth-sweep-01X.yaml",
            "yaml": yaml,
        })
    );
    for handle in ["ch_dots", "ch_link", "ch_dir"] {
        let (status, body) = get(addr, &format!("/api/agentiflows/{af}/generated/{handle}")).await;
        assert_eq!(status, 403, "{handle}: {body}");
    }
    for handle in ["ch_named", "ch_agent", "ch_nope"] {
        let (status, body) = get(addr, &format!("/api/agentiflows/{af}/generated/{handle}")).await;
        assert_eq!(status, 404, "{handle}: {body}");
    }
    let (status, _) = get(
        addr,
        &format!("/api/agentiflows/{af}/generated/ch_gen?host=peer"),
    )
    .await;
    assert_eq!(status, 501);
    let (status, _) = get(addr, "/api/agentiflows/af_01NOSUCHFLOW/generated/ch_gen").await;
    assert_eq!(status, 404);

    // A flow whose `generated/` is itself a symlink to a directory outside
    // it: what is reached through it is not the flow's.
    let af2 = "af_01GENERATEDLINK00000000000";
    let run2 = agentiflow_dir(global).join(af2);
    record(af2, "2026-10-21T00:00:00Z", "running")
        .write(&run2)
        .unwrap();
    let elsewhere = global.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("planted.yaml"), "name: planted\n").unwrap();
    std::os::unix::fs::symlink(&elsewhere, run2.join("generated")).unwrap();
    launched(
        &run2,
        "ch_planted",
        ChildKind::Workflow,
        "run_01GENF",
        1,
        Some(1),
        Some((&run2.join("generated").join("planted.yaml"), true)),
    );
    let (status, body) = get(
        addr,
        &format!("/api/agentiflows/{af2}/generated/ch_planted"),
    )
    .await;
    assert_eq!(status, 403, "{body}");
}
