//! The agentiflow writes `rupu agentiflow` has: `POST /api/agentiflows`
//! (launch, detached, through the `AgentiflowLauncher` port),
//! `POST /api/agentiflows/:id/stop` and the `/api/agentiflow-defs`
//! definition CRUD and validate.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use rupu_agentiflow::{agentiflow_dir, AgentiflowRecord, OperatorQueue};
use rupu_cp::agentiflow_launcher::{
    AgentiflowLaunchError, AgentiflowLaunchRequest, AgentiflowLauncher, AgentiflowStart,
};
use rupu_runtime::RunTriggerSource;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A definition `AgentiflowDef::check` accepts against the built-in profiles.
pub(super) const DEF: &str = "name: itest
description: A fixture flow.
lead: lead
engagement_profiles: [code]
goals:
  - id: issues
    objective: Find 3 issues.
    target: { findings: {}, count_gte: 3 }
scope: { authorized: true }
pool: { agents: [lead, reviewer] }
";

/// What a launch should answer with, and every request it was handed.
#[derive(Clone)]
pub(super) struct FakeLauncher {
    pub(super) answer:
        Arc<dyn Fn() -> Result<AgentiflowStart, AgentiflowLaunchError> + Send + Sync>,
    pub(super) seen: Arc<Mutex<Vec<AgentiflowLaunchRequest>>>,
}

impl FakeLauncher {
    pub(super) fn started() -> Self {
        Self {
            answer: Arc::new(|| Ok(AgentiflowStart::Started)),
            seen: Arc::default(),
        }
    }

    pub(super) fn requests(&self) -> Vec<AgentiflowLaunchRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AgentiflowLauncher for FakeLauncher {
    async fn launch(
        &self,
        req: AgentiflowLaunchRequest,
    ) -> Result<AgentiflowStart, AgentiflowLaunchError> {
        self.seen.lock().unwrap().push(req);
        (self.answer)()
    }
}

/// Any run launcher: the CP's "this is `cp serve`" signal writes need.
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

/// The CP over `global`, started in `<global>/../ws` (no project above it),
/// writable when `writable`, with `launcher` as its agentiflow launcher.
pub(super) fn state(
    global: &Path,
    writable: bool,
    launcher: Option<FakeLauncher>,
) -> rupu_cp::state::AppState {
    let ws = workspace_dir(global);
    std::fs::create_dir_all(&ws).unwrap();
    let mut s = rupu_cp::state::AppState::new(global.into(), rupu_config::PricingConfig::default())
        .with_workspace_dir(ws)
        .with_agentiflow_launcher(launcher.map(|l| Arc::new(l) as Arc<dyn AgentiflowLauncher>));
    if writable {
        s = s.with_launcher(Some(Arc::new(NoopLauncher)));
    }
    s
}

/// The directory a launch with no scope or `working_dir` starts in.
pub(super) fn workspace_dir(global: &Path) -> PathBuf {
    global.parent().unwrap().join("ws")
}

async fn serve(state: rupu_cp::state::AppState) -> SocketAddr {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// A temp dir holding `home/` (the global dir) and `ws/`.
fn home() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    std::fs::create_dir_all(&global).unwrap();
    (tmp, global)
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

pub(super) fn register_workspace(global: &Path, id: &str, root: &Path) {
    write(
        &global.join("workspaces").join(format!("{id}.toml")),
        &format!(
            "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            root.display()
        ),
    );
}

async fn send(
    addr: SocketAddr,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(method, format!("http://{addr}{path}"));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    send(addr, reqwest::Method::GET, path, None).await
}

async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    send(addr, reqwest::Method::POST, path, Some(body)).await
}

// ---- launch ----------------------------------------------------------------

#[tokio::test]
async fn launch_mints_the_id_and_hands_the_launcher_the_definition_and_directory() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let launcher = FakeLauncher::started();
    let addr = serve(state(&global, true, Some(launcher.clone()))).await;

    let (status, body) = post(addr, "/api/agentiflows", json!({"definition": "itest"})).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "started", "{body}");
    let id = body["id"].as_str().unwrap();
    assert!(id.starts_with("af_"), "{id}");

    let seen = launcher.requests();
    assert_eq!(
        seen,
        [AgentiflowLaunchRequest {
            definition: "itest".into(),
            run_id: id.into(),
            working_dir: workspace_dir(&global),
        }]
    );
}

#[tokio::test]
async fn a_launch_still_starting_is_pending_with_its_log() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let launcher = FakeLauncher {
        answer: Arc::new(|| {
            Ok(AgentiflowStart::Pending {
                log: "/x/detach.log".into(),
            })
        }),
        seen: Arc::default(),
    };
    let addr = serve(state(&global, true, Some(launcher))).await;
    let (status, body) = post(addr, "/api/agentiflows", json!({"definition": "itest"})).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["log"], "/x/detach.log");
}

#[tokio::test]
async fn a_project_scope_starts_the_run_in_that_workspace() {
    let (tmp, global) = home();
    let repo = tmp.path().join("repo");
    write(&repo.join(".rupu/agentiflows/itest.yaml"), DEF);
    register_workspace(&global, "ws_repo", &repo);
    let launcher = FakeLauncher::started();
    let addr = serve(state(&global, true, Some(launcher.clone()))).await;

    let (status, body) = post(
        addr,
        "/api/agentiflows",
        json!({"definition": "itest", "scope_kind": "project", "scope_id": "ws_repo"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(launcher.requests()[0].working_dir, repo);

    // The same scope without that definition is a 404, before any spawn.
    let (status, _) = post(
        addr,
        "/api/agentiflows",
        json!({"definition": "other", "scope_kind": "project", "scope_id": "ws_repo"}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(launcher.requests().len(), 1);
}

#[tokio::test]
async fn a_working_dir_must_be_an_absolute_directory_and_not_beside_a_scope() {
    let (tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let launcher = FakeLauncher::started();
    let addr = serve(state(&global, true, Some(launcher.clone()))).await;

    let dir = tmp.path().join("elsewhere");
    std::fs::create_dir_all(&dir).unwrap();
    let (status, _) = post(
        addr,
        "/api/agentiflows",
        json!({"definition": "itest", "working_dir": dir}),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(launcher.requests()[0].working_dir, dir);

    for body in [
        json!({"definition": "itest", "working_dir": "relative/dir"}),
        json!({"definition": "itest", "working_dir": tmp.path().join("missing")}),
        json!({"definition": "itest", "working_dir": dir, "scope_kind": "global"}),
        json!({"definition": "itest", "scope_kind": "nowhere"}),
    ] {
        let (status, resp) = post(addr, "/api/agentiflows", body.clone()).await;
        assert_eq!(status, 400, "{body}: {resp}");
    }
    assert_eq!(launcher.requests().len(), 1);
}

#[tokio::test]
async fn a_definition_is_found_and_checked_before_anything_is_spawned() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/broken.yaml"), "name: [\n");
    write(
        &global.join("agentiflows/unauthorized.yaml"),
        &DEF.replace("authorized: true", "authorized: false"),
    );
    let launcher = FakeLauncher::started();
    let addr = serve(state(&global, true, Some(launcher.clone()))).await;

    let (status, body) = post(addr, "/api/agentiflows", json!({"definition": "nope"})).await;
    assert_eq!(status, 404, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("not found"),
        "{body}"
    );

    let (status, body) = post(addr, "/api/agentiflows", json!({"definition": "broken"})).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("not loadable"),
        "{body}"
    );

    let (status, body) = post(
        addr,
        "/api/agentiflows",
        json!({"definition": "unauthorized"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("scope.authorized"),
        "{body}"
    );

    let (status, _) = post(addr, "/api/agentiflows", json!({"definition": "../x"})).await;
    assert_eq!(status, 400);
    assert!(launcher.requests().is_empty());
}

#[tokio::test]
async fn a_run_that_does_not_start_is_a_400_with_its_reason() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let launcher = FakeLauncher {
        answer: Arc::new(|| {
            Err(AgentiflowLaunchError::NotStarted(
                "no credential for anthropic".into(),
            ))
        }),
        seen: Arc::default(),
    };
    let addr = serve(state(&global, true, Some(launcher))).await;
    let (status, body) = post(addr, "/api/agentiflows", json!({"definition": "itest"})).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("no credential"),
        "{body}"
    );
}

#[tokio::test]
async fn launch_is_local_only_and_needs_cp_serve_and_a_launcher() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let def = json!({"definition": "itest"});

    let addr = serve(state(&global, true, Some(FakeLauncher::started()))).await;
    let (status, body) = post(addr, "/api/agentiflows?host=peer", def.clone()).await;
    assert_eq!(status, 501, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("local-only"),
        "{body}"
    );
    let (status, _) = post(addr, "/api/agentiflows?host=local", def.clone()).await;
    assert_eq!(status, 202);

    let addr = serve(state(&global, false, Some(FakeLauncher::started()))).await;
    let (status, _) = post(addr, "/api/agentiflows", def.clone()).await;
    assert_eq!(status, 501);

    let addr = serve(state(&global, true, None)).await;
    let (status, _) = post(addr, "/api/agentiflows", def).await;
    assert_eq!(status, 501);
}

// ---- stop --------------------------------------------------------------------

const LIVE: &str = "af_01J9ZQ3K4M5N6P7Q8R9S0T1STP";

/// A pid that existed and is gone.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

pub(super) fn seed_flow(global: &Path, id: &str, status: &str, runner_pid: Option<u32>) {
    AgentiflowRecord {
        id: id.into(),
        name: "itest".into(),
        engagement_profiles: vec!["code".into()],
        trigger: RunTriggerSource::Agentiflow,
        status: status.into(),
        stop_reason: None,
        rounds: 1,
        goals: vec![],
        started_at: "2026-10-20T00:00:00Z".parse().unwrap(),
        ended_at: None,
        codename: None,
        spent_usd: None,
        spent_tokens: 0,
        runner_pid,
    }
    .write(&agentiflow_dir(global).join(id))
    .unwrap();
}

/// Whether a stop is waiting on the flow's control queue.
pub(super) fn stop_queued(global: &Path, id: &str) -> bool {
    OperatorQueue::new(agentiflow_dir(global).join(id))
        .drain()
        .unwrap()
        .iter()
        .any(|c| matches!(c, rupu_agentiflow::Control::Stop { .. }))
}

#[tokio::test]
async fn a_graceful_stop_is_queued_for_a_live_flow() {
    let (_tmp, global) = home();
    seed_flow(&global, LIVE, "running", Some(std::process::id()));
    let addr = serve(state(&global, true, None)).await;
    let path = format!("/api/agentiflows/{LIVE}/stop");

    // Answered as `steer {stop: true}` answers.
    let (status, body) = post(addr, &path, json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({"queued": true}));
    assert!(stop_queued(&global, LIVE));

    // A bodyless POST is the graceful stop too.
    let (status, _) = send(addr, reqwest::Method::POST, &path, None).await;
    assert_eq!(status, 200);
    assert!(stop_queued(&global, LIVE));
}

#[tokio::test]
async fn a_stop_body_that_is_not_a_stop_request_is_a_400_not_a_graceful_stop() {
    let (_tmp, global) = home();
    seed_flow(&global, LIVE, "running", Some(std::process::id()));
    let addr = serve(state(&global, true, None)).await;
    let url = format!("http://{addr}/api/agentiflows/{LIVE}/stop");

    let (status, body) = post(
        addr,
        &format!("/api/agentiflows/{LIVE}/stop"),
        json!({"now": "true"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    // ...and with no content-type, the body is still read, not dropped.
    let r = reqwest::Client::new()
        .post(&url)
        .body(r#"{"now": 1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    assert!(!stop_queued(&global, LIVE));
    let record = AgentiflowRecord::read(&agentiflow_dir(&global).join(LIVE)).unwrap();
    assert_eq!(record.status, "running");
}

#[tokio::test]
async fn a_graceful_stop_needs_a_live_coordinator() {
    let (_tmp, global) = home();
    seed_flow(&global, LIVE, "running", Some(dead_pid()));
    let done = "af_01J9ZQ3K4M5N6P7Q8R9S0T1DNE";
    seed_flow(&global, done, "completed", None);
    let addr = serve(state(&global, true, None)).await;

    for id in [LIVE, done] {
        let (status, body) = post(addr, &format!("/api/agentiflows/{id}/stop"), json!({})).await;
        assert_eq!(status, 409, "{id}: {body}");
        assert!(!stop_queued(&global, id));
    }
}

#[tokio::test]
async fn a_hard_stop_records_the_run_failed() {
    let (_tmp, global) = home();
    // A coordinator that died without finalizing: the hard stop closes it
    // out (and signals nothing, so this test process is safe).
    seed_flow(&global, LIVE, "running", Some(dead_pid()));
    let addr = serve(state(&global, true, None)).await;
    let path = format!("/api/agentiflows/{LIVE}/stop");

    let (status, body) = post(addr, &path, json!({"now": true})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"stopped": true, "status": "failed", "stop_reason": "operator_stop:now"})
    );
    let record = AgentiflowRecord::read(&agentiflow_dir(&global).join(LIVE)).unwrap();
    assert_eq!(record.status, "failed");

    // Now it is not running: a second hard stop is a conflict.
    let (status, _) = post(addr, &path, json!({"now": true})).await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn stop_refuses_unknown_remote_and_read_only() {
    let (_tmp, global) = home();
    seed_flow(&global, LIVE, "running", Some(std::process::id()));
    let addr = serve(state(&global, true, None)).await;

    let (status, _) = post(
        addr,
        "/api/agentiflows/af_01J9ZQ3K4M5N6P7Q8R9S0T1NON/stop",
        json!({}),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = post(addr, "/api/agentiflows/run_x/stop", json!({})).await;
    assert_eq!(status, 404);
    let (status, body) = post(
        addr,
        &format!("/api/agentiflows/{LIVE}/stop?host=peer"),
        json!({"now": true}),
    )
    .await;
    assert_eq!(status, 501, "{body}");

    let addr = serve(state(&global, false, None)).await;
    let (status, _) = post(addr, &format!("/api/agentiflows/{LIVE}/stop"), json!({})).await;
    assert_eq!(status, 501);
    assert!(!stop_queued(&global, LIVE));
}

// ---- definitions -------------------------------------------------------------

fn named(name: &str) -> String {
    DEF.replace("name: itest", &format!("name: {name}"))
}

#[tokio::test]
async fn the_list_merges_layers_with_project_shadowing_and_lists_broken_files() {
    let (tmp, global) = home();
    let repo = tmp.path().join("repo");
    write(&global.join("agentiflows/alpha.yaml"), &named("alpha"));
    write(&global.join("agentiflows/shared.yaml"), &named("shared"));
    write(&global.join("agentiflows/broken.yaml"), "name: [\n");
    write(&global.join("agentiflows/notes.yml"), &named("notes"));
    // A run directory beside the definitions is not one.
    seed_flow(&global, LIVE, "completed", None);
    write(
        &repo.join(".rupu/agentiflows/shared.yaml"),
        &named("shared"),
    );
    register_workspace(&global, "ws_repo", &repo);
    let addr = serve(state(&global, false, None)).await;

    let (status, rows) = send(addr, reqwest::Method::GET, "/api/agentiflow-defs", None).await;
    assert_eq!(status, 200, "{rows}");
    let summary: Vec<(String, String)> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["scope_kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("alpha".to_string(), "global".to_string()),
            ("broken".to_string(), "global".to_string()),
            ("shared".to_string(), "project".to_string()),
        ]
    );
    let rows = rows.as_array().unwrap();
    assert_eq!(rows[0]["lead"], "lead");
    assert_eq!(rows[0]["engagement_profiles"], json!(["code"]));
    assert!(
        rows[1]["error"].as_str().unwrap().contains("parse"),
        "{}",
        rows[1]
    );
    assert_eq!(rows[2]["scope_id"], "ws_repo");
}

#[tokio::test]
async fn get_resolves_project_first_and_takes_an_explicit_scope() {
    let (tmp, global) = home();
    let repo = tmp.path().join("repo");
    write(&global.join("agentiflows/shared.yaml"), &named("shared"));
    write(
        &repo.join(".rupu/agentiflows/shared.yaml"),
        &named("shared").replace("A fixture flow.", "The project's."),
    );
    write(&global.join("agentiflows/broken.yaml"), "name: [\n");
    register_workspace(&global, "ws_repo", &repo);
    let addr = serve(state(&global, false, None)).await;

    let (status, body) = get(addr, "/api/agentiflow-defs/shared").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["scope_kind"], "project");
    assert_eq!(body["def"]["description"], "The project's.");
    assert!(body["yaml"].as_str().unwrap().contains("The project's."));

    let (status, body) = get(addr, "/api/agentiflow-defs/shared?scope_kind=global").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["scope_kind"], "global");
    assert_eq!(body["def"]["description"], "A fixture flow.");

    let (status, body) = get(addr, "/api/agentiflow-defs/broken").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["def"], Value::Null);
    assert!(body["error"].as_str().unwrap().contains("parse"), "{body}");

    let (status, _) = get(addr, "/api/agentiflow-defs/missing").await;
    assert_eq!(status, 404);
    let (status, _) = get(
        addr,
        "/api/agentiflow-defs/shared?scope_kind=project&scope_id=ws_none",
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn create_save_and_delete_write_the_layer_they_name() {
    let (tmp, global) = home();
    let repo = tmp.path().join("repo");
    write(&repo.join(".rupu/agentiflows/proj.yaml"), &named("proj"));
    register_workspace(&global, "ws_repo", &repo);
    let addr = serve(state(&global, true, None)).await;
    let global_file = global.join("agentiflows/fresh.yaml");

    // create: global, named by its `name:`
    let (status, body) = post(addr, "/api/agentiflow-defs", json!({"raw": named("fresh")})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["scope_kind"], "global");
    assert_eq!(
        std::fs::read_to_string(&global_file).unwrap(),
        named("fresh")
    );
    // ...once, in any layer
    for name in ["fresh", "proj"] {
        let (status, _) = post(addr, "/api/agentiflow-defs", json!({"raw": named(name)})).await;
        assert_eq!(status, 409, "{name}");
    }
    let (status, _) = post(addr, "/api/agentiflow-defs", json!({"raw": "name: [\n"})).await;
    assert_eq!(status, 400);

    // save: in place, project-first
    let edited = named("proj").replace("A fixture flow.", "Edited.");
    let (status, body) = send(
        addr,
        reqwest::Method::PUT,
        "/api/agentiflow-defs/proj",
        Some(json!({"raw": edited})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["scope_kind"], "project");
    assert_eq!(
        std::fs::read_to_string(repo.join(".rupu/agentiflows/proj.yaml")).unwrap(),
        edited
    );
    assert!(!global.join("agentiflows/proj.yaml").exists());
    // a mismatched name, a broken file, a scope without the file
    for (path, raw, want) in [
        ("/api/agentiflow-defs/proj", named("other"), 400),
        ("/api/agentiflow-defs/proj", "name: [\n".to_string(), 400),
        (
            "/api/agentiflow-defs/proj?scope_kind=global",
            named("proj"),
            404,
        ),
    ] {
        let (status, body) =
            send(addr, reqwest::Method::PUT, path, Some(json!({"raw": raw}))).await;
        assert_eq!(status, want, "{path}: {body}");
    }

    // delete: the layer named
    let (status, body) = send(
        addr,
        reqwest::Method::DELETE,
        "/api/agentiflow-defs/proj?scope_kind=project&scope_id=ws_repo",
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"deleted": true, "scope": "repo", "scope_kind": "project"})
    );
    assert!(!repo.join(".rupu/agentiflows/proj.yaml").exists());
    let (status, _) = send(
        addr,
        reqwest::Method::DELETE,
        "/api/agentiflow-defs/proj",
        None,
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn definition_routes_are_local_only() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let addr = serve(state(&global, true, None)).await;
    let fresh = json!({"raw": named("fresh")});
    let cases = [
        (reqwest::Method::GET, "/api/agentiflow-defs?host=peer", None),
        (
            reqwest::Method::GET,
            "/api/agentiflow-defs/itest?host=peer",
            None,
        ),
        (
            reqwest::Method::POST,
            "/api/agentiflow-defs?host=peer",
            Some(fresh.clone()),
        ),
        (
            reqwest::Method::PUT,
            "/api/agentiflow-defs/itest?host=peer",
            Some(json!({"raw": DEF})),
        ),
        (
            reqwest::Method::DELETE,
            "/api/agentiflow-defs/itest?host=peer",
            None,
        ),
        (
            reqwest::Method::POST,
            "/api/agentiflow-defs/validate?host=peer",
            Some(json!({"raw": DEF})),
        ),
    ];
    for (method, path, body) in cases {
        let (status, resp) = send(addr, method.clone(), path, body).await;
        assert_eq!(status, 501, "{method} {path}: {resp}");
        assert!(
            resp["error"].as_str().unwrap().contains("local-only"),
            "{resp}"
        );
    }
    assert!(global.join("agentiflows/itest.yaml").exists());
    assert!(!global.join("agentiflows/fresh.yaml").exists());
    let (status, _) = get(addr, "/api/agentiflow-defs?host=local").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_route_segment_cannot_name_a_definition() {
    let (_tmp, global) = home();
    let addr = serve(state(&global, true, None)).await;
    let (status, body) = post(
        addr,
        "/api/agentiflow-defs",
        json!({"raw": named("validate")}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("another route"),
        "{body}"
    );
    assert!(!global.join("agentiflows/validate.yaml").exists());
}

#[tokio::test]
async fn definition_writes_need_cp_serve() {
    let (_tmp, global) = home();
    write(&global.join("agentiflows/itest.yaml"), DEF);
    let addr = serve(state(&global, false, None)).await;
    let (status, _) = post(addr, "/api/agentiflow-defs", json!({"raw": named("fresh")})).await;
    assert_eq!(status, 501);
    let (status, _) = send(
        addr,
        reqwest::Method::PUT,
        "/api/agentiflow-defs/itest",
        Some(json!({"raw": DEF})),
    )
    .await;
    assert_eq!(status, 501);
    let (status, _) = send(
        addr,
        reqwest::Method::DELETE,
        "/api/agentiflow-defs/itest",
        None,
    )
    .await;
    assert_eq!(status, 501);
    assert!(global.join("agentiflows/itest.yaml").exists());
}

#[tokio::test]
async fn validate_runs_the_checks_agentiflow_run_runs() {
    let (tmp, global) = home();
    let addr = serve(state(&global, false, None)).await;
    let path = "/api/agentiflow-defs/validate";

    let (status, body) = post(addr, path, json!({"raw": DEF})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        json!({"ok": true, "name": "itest", "lead": "lead", "engagement_profiles": ["code"]})
    );

    let cases = [
        ("name: [\n".to_string(), "parse"),
        (
            DEF.replace("[code]", "[no-such-profile]"),
            "unknown engagement profile",
        ),
        (
            DEF.replace("lead: lead", "lead: ghost"),
            "not in pool.agents",
        ),
    ];
    for (raw, want) in cases {
        let (status, body) = post(addr, path, json!({"raw": raw})).await;
        assert_eq!(status, 400, "{want}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains(want),
            "{want}: {body}"
        );
    }

    // A project's profiles count when its id is given; an unknown id is a 404.
    let repo = tmp.path().join("repo");
    register_workspace(&global, "ws_repo", &repo);
    write(&repo.join(".rupu/profiles/broken.toml"), "not [ toml");
    let (status, body) = post(addr, path, json!({"raw": DEF, "scope_id": "ws_repo"})).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .starts_with("engagement profiles"),
        "{body}"
    );
    let (status, _) = post(addr, path, json!({"raw": DEF, "scope_id": "ws_none"})).await;
    assert_eq!(status, 404);
}
