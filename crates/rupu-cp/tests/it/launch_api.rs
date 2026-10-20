//! Plan 3a package B over HTTP: continuing an agent run, retrying a workflow
//! run, the launch bodies' `model` / `provider` / `engagement_profiles`, the
//! YAML launch and agent validation.

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use rupu_cp::agent_launcher::{AgentLaunchError, AgentLaunchRequest, AgentLauncher};
use rupu_cp::launcher::{LaunchError, LaunchRequest, RetryRequest, RunLauncher};
use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use rupu_orchestrator::runs::{RunStatus, RunStore};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What the CP asked its launchers for.
#[derive(Default)]
pub(super) struct Recorded {
    pub(super) runs: Mutex<Vec<LaunchRequest>>,
    pub(super) retries: Mutex<Vec<RetryRequest>>,
    pub(super) agents: Mutex<Vec<AgentLaunchRequest>>,
}

struct Runs(Arc<Recorded>);

#[async_trait]
impl RunLauncher for Runs {
    async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError> {
        self.0.runs.lock().unwrap().push(req);
        Ok("run_01LAUNCHED".into())
    }
    async fn retry(&self, req: RetryRequest) -> Result<(), LaunchError> {
        self.0.retries.lock().unwrap().push(req);
        Ok(())
    }
}

struct Agents(Arc<Recorded>);

#[async_trait]
impl AgentLauncher for Agents {
    async fn launch(&self, req: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        self.0.agents.lock().unwrap().push(req);
        Ok("run_01CONTINUED".into())
    }
}

pub(super) const FAILED: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0FAIL";
pub(super) const PAUSED: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0PAUS";
pub(super) const DONE: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0DONE";
pub(super) const LIVE: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0LIVE";
/// A run left `Running` by a runner that is gone.
pub(super) const ORPHAN: &str = "run_01J9ZQ3K4M5N6P7Q8R9S0ORPH";
/// A standalone agent run that ended.
pub(super) const AGENT_DONE: &str = "run_01J9ZQ3K4M5N6P7Q8R9AGDONE";
/// A standalone agent run whose process is this test process.
pub(super) const AGENT_LIVE: &str = "run_01J9ZQ3K4M5N6P7Q8R9AGLIVE";
/// A sub-agent of the completed workflow run `DONE`.
pub(super) const SUB_DONE: &str = "sub_01J9ZQ3K4M5N6P7Q8R9SUBDONE";
/// A sub-agent of the live workflow run `LIVE`.
pub(super) const SUB_LIVE: &str = "sub_01J9ZQ3K4M5N6P7Q8R9SUBLIVE";

/// A workflow that parses, for the YAML launch.
pub(super) const ADHOC: &str =
    "name: adhoc\nsteps:\n  - id: one\n    agent: scout\n    actions: []\n    prompt: hi\n";

/// A pid no process has: above every platform's pid range.
const DEAD_PID: u32 = 99_999_999;

fn transcript(path: &Path, run_id: &str, agent: &str) {
    let start = rupu_transcript::Event::RunStart {
        run_id: run_id.into(),
        workspace_id: "ws_test".into(),
        agent: agent.into(),
        provider: "anthropic".into(),
        model: "opus".into(),
        started_at: chrono::Utc::now(),
        mode: rupu_transcript::RunMode::Bypass,
        schema: None,
        system_prompt: None,
        codename: None,
        customer: None,
        launch_skills: Vec::new(),
        launch_preload_skills: Vec::new(),
        launch_args: Vec::new(),
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(&start).unwrap() + "\n").unwrap();
}

/// The runs and agent runs package B's tests act on, plus one agent.
pub(super) fn seed(dir: &Path) {
    let store = RunStore::new(dir.join("runs"));
    let wf = "name: test\nsteps: []\n";
    for (id, status, pid) in [
        (FAILED, RunStatus::Failed, None),
        (PAUSED, RunStatus::Paused, None),
        (DONE, RunStatus::Completed, None),
        (LIVE, RunStatus::Running, Some(std::process::id())),
        (ORPHAN, RunStatus::Running, Some(DEAD_PID)),
    ] {
        let mut rec = super::sse::seed_run(id, status);
        rec.runner_pid = pid;
        rec.workspace_path = dir.to_path_buf();
        store.create(rec, wf).unwrap();
    }
    let project = dir.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let transcripts = dir.join("transcripts");
    for (id, pid) in [(AGENT_DONE, None), (AGENT_LIVE, Some(std::process::id()))] {
        transcript(&transcripts.join(format!("{id}.jsonl")), id, "scout");
        let meta = json!({"run_id": id, "workspace_path": project, "pid": pid});
        std::fs::write(
            transcripts.join(format!("{id}.meta.json")),
            meta.to_string(),
        )
        .unwrap();
    }
    for (parent, sub) in [(DONE, SUB_DONE), (LIVE, SUB_LIVE)] {
        let t = dir.join("runs").join(parent).join("sub").join(sub);
        transcript(&t.join("transcript.jsonl"), sub, "helper");
    }
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("scout.md"),
        "---\nname: scout\ndescription: d\n---\nLook.\n",
    )
    .unwrap();
    std::fs::write(
        agents.join("typo.md"),
        "---\nname: typo\ntools: [reed_file]\n---\nOops.\n",
    )
    .unwrap();
}

/// A writable CP over a seeded global dir, serving `/api` and `/mcp`, whose
/// launchers record what they're asked for.
pub(super) async fn spawn_launch() -> (SocketAddr, tempfile::TempDir, Arc<Recorded>) {
    let tmp = tempfile::tempdir().unwrap();
    seed(tmp.path());
    let rec = Arc::new(Recorded::default());
    let state = rupu_cp::state::AppState::new(tmp.path().to_path_buf(), Default::default())
        .with_launcher(Some(Arc::new(Runs(rec.clone()))))
        .with_agent_launcher(Some(Arc::new(Agents(rec.clone()))));
    let server = rupu_cp::mcp::build_server(
        &state,
        &McpOpts {
            catalog: None,
            tools: vec![],
        },
    )
    .unwrap();
    let app = router_with(
        state,
        None,
        RouterOpts {
            web: false,
            mcp: Some(server),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, tmp, rec)
}

pub(super) async fn post(addr: SocketAddr, path: &str, body: Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

pub(super) async fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

fn status_of(dir: &Path, id: &str) -> RunStatus {
    RunStore::new(dir.join("runs")).load(id).unwrap().status
}

// ── continue ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn continuing_an_agent_run_launches_rupu_run_continue_in_its_directory() {
    let (addr, tmp, rec) = spawn_launch().await;
    let (status, body) = post(
        addr,
        &format!("/api/runs/agents/{AGENT_DONE}/continue"),
        json!({"model": "claude-opus-5-5", "provider": "anthropic", "mode": "bypass"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(
        body,
        json!({"run_id": "run_01CONTINUED", "continues": AGENT_DONE, "host_id": "local"})
    );
    let agents = rec.agents.lock().unwrap();
    let [req] = agents.as_slice() else {
        panic!("{agents:?}")
    };
    assert_eq!(req.agent, "scout");
    assert_eq!(req.continue_from.as_deref(), Some(AGENT_DONE));
    assert_eq!(req.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(req.provider.as_deref(), Some("anthropic"));
    assert_eq!(req.mode.as_deref(), Some("bypass"));
    // Nothing else is set: the child restores the original launch's flags.
    assert_eq!((&req.prompt, &req.target), (&None, &None));
    let want = tmp.path().join("proj").display().to_string();
    assert_eq!(req.working_dir.as_deref(), Some(want.as_str()));
    // `rupu run scout --continue <id> --model … --provider …`.
    let args: Vec<String> = rupu_runtime::argv::RunArgv::Agent(req.agent_run("run_01NEW").unwrap())
        .to_args()
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect();
    assert_eq!(
        args,
        [
            "run",
            "scout",
            "--run-id",
            "run_01NEW",
            "--mode",
            "bypass",
            "--continue",
            AGENT_DONE,
            "--model=claude-opus-5-5",
            "--provider=anthropic"
        ]
    );
}

#[tokio::test]
async fn a_sub_agent_run_continues_too() {
    let (addr, _tmp, rec) = spawn_launch().await;
    let (status, body) = post(
        addr,
        &format!("/api/runs/agents/{SUB_DONE}/continue"),
        json!({}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let agents = rec.agents.lock().unwrap();
    assert_eq!(agents[0].agent, "helper");
    assert_eq!(agents[0].continue_from.as_deref(), Some(SUB_DONE));
}

#[tokio::test]
async fn continue_refuses_a_running_run_an_unknown_one_a_prompt_and_a_remote_host() {
    let (addr, _tmp, rec) = spawn_launch().await;
    for (id, body, want) in [
        (AGENT_LIVE, json!({}), 409),
        (SUB_LIVE, json!({}), 409),
        ("run_01J9ZQ3K4M5N6P7Q8R9NOSUCH", json!({}), 404),
        ("sub_01J9ZQ3K4M5N6P7Q8R9NOSUCH", json!({}), 404),
        (AGENT_DONE, json!({"prompt": "and then?"}), 400),
        (AGENT_DONE, json!({"mode": "yolo"}), 400),
    ] {
        let (status, err) = post(addr, &format!("/api/runs/agents/{id}/continue"), body).await;
        assert_eq!(status, want, "{id}: {err}");
    }
    let (status, err) = post(
        addr,
        &format!("/api/runs/agents/{AGENT_DONE}/continue?host=peer"),
        json!({}),
    )
    .await;
    assert_eq!(status, 501, "{err}");
    assert!(
        err["error"].as_str().unwrap().contains("local-only"),
        "{err}"
    );
    assert!(rec.agents.lock().unwrap().is_empty());
}

// ── retry ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn retrying_a_failed_run_spawns_an_operators_workflow_resume() {
    let (addr, _tmp, rec) = spawn_launch().await;
    let (status, body) = post(
        addr,
        &format!("/api/runs/{FAILED}/retry"),
        json!({"restart_interrupted": true}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(
        body,
        json!({"run_id": FAILED, "retrying": true, "restart_interrupted": true, "host_id": "local"})
    );
    let retries = rec.retries.lock().unwrap().clone();
    assert_eq!(
        retries,
        [RetryRequest {
            run_id: FAILED.into(),
            restart_interrupted: true
        }]
    );
    let args: Vec<String> = retries[0]
        .argv()
        .to_args()
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect();
    assert_eq!(
        args,
        ["workflow", "resume", FAILED, "--restart-interrupted"]
    );
}

#[tokio::test]
async fn a_run_left_running_by_a_dead_runner_is_reaped_then_retried() {
    let (addr, tmp, rec) = spawn_launch().await;
    let (status, body) = post(addr, &format!("/api/runs/{ORPHAN}/retry"), json!({})).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(status_of(tmp.path(), ORPHAN), RunStatus::Failed);
    assert_eq!(rec.retries.lock().unwrap()[0].run_id, ORPHAN);
    assert!(!rec.retries.lock().unwrap()[0].restart_interrupted);
}

#[tokio::test]
async fn retry_refuses_what_isnt_finished_or_failed() {
    let (addr, tmp, rec) = spawn_launch().await;
    for (id, want) in [
        (LIVE, 409),
        (PAUSED, 409),
        (DONE, 409),
        ("run_01J9ZQ3K4M5N6P7Q8R9S0NONE", 404),
    ] {
        let (status, err) = post(addr, &format!("/api/runs/{id}/retry"), json!({})).await;
        assert_eq!(status, want, "{id}: {err}");
    }
    let (status, err) = post(
        addr,
        &format!("/api/runs/{FAILED}/retry?host=peer"),
        json!({}),
    )
    .await;
    assert_eq!(status, 501, "{err}");
    assert!(rec.retries.lock().unwrap().is_empty());
    assert_eq!(status_of(tmp.path(), LIVE), RunStatus::Running);
}

// ── launch body gaps ────────────────────────────────────────────────────────

#[tokio::test]
async fn an_agent_launch_carries_model_and_provider() {
    let (addr, _tmp, rec) = spawn_launch().await;
    let (status, body) = post(
        addr,
        "/api/agents/scout/run",
        json!({"prompt": "go", "model": "gpt-6", "provider": "openai", "engagement_profiles": ["network"]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let req = rec.agents.lock().unwrap()[0].clone();
    assert_eq!(
        (req.model.as_deref(), req.provider.as_deref()),
        (Some("gpt-6"), Some("openai"))
    );
    assert_eq!(req.engagement_profiles, ["network"]);
    assert_eq!(req.continue_from, None);
    // A blank one is a 400; a remote launch can't carry either: 501.
    let (status, _) = post(addr, "/api/agents/scout/run", json!({"model": " "})).await;
    assert_eq!(status, 400);
    let (status, err) = post(
        addr,
        "/api/agents/scout/run",
        json!({"host": "peer", "provider": "openai"}),
    )
    .await;
    assert_eq!(status, 501, "{err}");
}

#[tokio::test]
async fn a_workflow_launch_carries_engagement_profiles() {
    let (addr, tmp, rec) = spawn_launch().await;
    let wfs = tmp.path().join("workflows");
    std::fs::create_dir_all(&wfs).unwrap();
    std::fs::write(wfs.join("audit.yaml"), "name: audit\nsteps: []\n").unwrap();
    let (status, body) = post(
        addr,
        "/api/workflows/audit/run",
        json!({"engagement_profiles": ["network", "web"]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let req = rec.runs.lock().unwrap()[0].clone();
    assert_eq!(req.engagement_profiles, ["network", "web"]);
    assert_eq!(req.file, None);
    let (status, _) = post(
        addr,
        "/api/workflows/audit/run",
        json!({"engagement_profiles": [""]}),
    )
    .await;
    assert_eq!(status, 400);
    let (status, err) = post(
        addr,
        "/api/workflows/audit/run",
        json!({"host": "peer", "engagement_profiles": ["web"]}),
    )
    .await;
    assert_eq!(status, 501, "{err}");
}

#[tokio::test]
async fn a_yaml_launch_validates_keeps_the_file_and_runs_it_with_file() {
    let (addr, tmp, rec) = spawn_launch().await;
    let yaml = ADHOC;
    let (status, body) = post(
        addr,
        "/api/workflows/run",
        json!({"yaml": yaml, "inputs": {"k": "v"}, "mode": "readonly", "engagement_profiles": ["web"]}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run_id"], json!("run_01LAUNCHED"));
    let file = std::path::PathBuf::from(body["file"].as_str().unwrap());
    assert!(
        file.starts_with(tmp.path().join("cp").join("workflow-launches")),
        "{file:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), yaml);
    let req = rec.runs.lock().unwrap()[0].clone();
    assert_eq!(req.file.as_deref(), Some(file.as_path()));
    let args: Vec<String> =
        rupu_runtime::argv::RunArgv::Workflow(req.workflow_run("run_X").unwrap())
            .to_args()
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
    let file_arg = file.display().to_string();
    assert_eq!(
        args,
        [
            "workflow",
            "run",
            "--file",
            file_arg.as_str(),
            "--run-id",
            "run_X",
            "--mode",
            "readonly",
            "--plain",
            "--input",
            "k=v",
            "--engagement-profile",
            "web"
        ]
    );
}

#[tokio::test]
async fn a_yaml_launch_refuses_bad_yaml_before_writing_and_a_remote_host() {
    let (addr, tmp, rec) = spawn_launch().await;
    let (status, err) = post(addr, "/api/workflows/run", json!({"yaml": "steps: ["})).await;
    assert_eq!(status, 400, "{err}");
    let (status, err) = post(
        addr,
        "/api/workflows/run",
        json!({"yaml": "name: a\nsteps: []\n", "host": "peer"}),
    )
    .await;
    assert_eq!(status, 501, "{err}");
    assert!(rec.runs.lock().unwrap().is_empty());
    assert!(!tmp.path().join("cp").exists());
}

// ── agent validation ────────────────────────────────────────────────────────

#[tokio::test]
async fn validating_an_agents_text_answers_ok_or_400() {
    let (addr, _tmp, _) = spawn_launch().await;
    let (status, body) = post(
        addr,
        "/api/agents/validate",
        json!({"raw": "---\nname: fresh\n---\nHi.\n"}),
    )
    .await;
    assert_eq!((status, body), (200, json!({"ok": true, "name": "fresh"})));
    for raw in [
        "no frontmatter",
        "---\nname: t\ntools: [reed_file]\n---\nx\n",
        "---\nname: t\nskills: [not-installed]\n---\nx\n",
    ] {
        let (status, err) = post(addr, "/api/agents/validate", json!({"raw": raw})).await;
        assert_eq!(status, 400, "{raw}: {err}");
    }
    let (status, _) = post(
        addr,
        "/api/agents/validate",
        json!({"raw": "---\nname: t\n---\nx\n", "ws_id": "ws_nope"}),
    )
    .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn validating_every_agent_file_lists_a_row_per_file() {
    let (addr, tmp, _) = spawn_launch().await;
    let (status, body) = get(addr, "/api/agents/validate").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], json!(false));
    let rows = body["rows"].as_array().unwrap();
    let row = |name: &str| rows.iter().find(|r| r["name"] == name).unwrap().clone();
    let agents = tmp.path().join("agents");
    assert_eq!(
        row("scout"),
        json!({"name": "scout", "layer": "global", "path": agents.join("scout.md"), "ok": true})
    );
    let typo = row("typo");
    assert_eq!(typo["ok"], json!(false));
    assert!(
        typo["error"].as_str().unwrap().contains("reed_file"),
        "{typo}"
    );
    assert_eq!(rows.len(), 2);
}
