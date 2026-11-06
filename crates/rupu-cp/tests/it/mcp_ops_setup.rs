//! Plan 3a Package C over MCP: session stop/compact, skill installs, the
//! stock fleet, project init and engagement profiles. `setup_api.rs` covers
//! the same endpoints over plain HTTP; both use [`spawn_setup`].

// In-process test server client, not rupu egress (see sse.rs).
#![allow(clippy::disallowed_methods)]

use super::mcp::{assert_required_args, call_tool, json_of, rpc, text_of, NoopLauncher};
use rupu_cp::mcp::McpOpts;
use rupu_cp::server::{router_with, RouterOpts};
use rupu_cp::session_mutator::{SessionAction, SessionMutateError, SessionMutator};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(super) const LIVE: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1LIV";
pub(super) const BUSY: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1BSY";
pub(super) const SHELVED: &str = "ses_01J9ZQ3K4M5N6P7Q8R9S0T1SHV";
pub(super) const WS: &str = "ws_01J9ZQ3K4M5N6P7Q8R9S0T1PRJ";

/// Records every call it gets, as `"<action> <id>"` / `"compact <id>
/// <window> <log>"`.
#[derive(Default)]
pub(super) struct RecordingMutator(pub Mutex<Vec<String>>);

#[async_trait::async_trait]
impl SessionMutator for RecordingMutator {
    async fn mutate(&self, id: &str, action: SessionAction) -> Result<(), SessionMutateError> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{} {id}", action.as_str()));
        Ok(())
    }

    async fn compact(
        &self,
        id: &str,
        compaction_id: &str,
        window: Option<u32>,
        log: &Path,
    ) -> Result<u32, SessionMutateError> {
        assert!(compaction_id.starts_with("cmp_"), "{compaction_id}");
        self.0
            .lock()
            .unwrap()
            .push(format!("compact {id} {window:?} {}", log.display()));
        // No child: pid 0 is never alive, so the handed-over marker is stale.
        Ok(0)
    }
}

/// Records every send as `"send <session id>"`.
#[derive(Default)]
pub(super) struct RecordingSender(pub Mutex<Vec<String>>);

#[async_trait::async_trait]
impl rupu_cp::session_sender::SessionSender for RecordingSender {
    async fn send(
        &self,
        req: rupu_cp::session_sender::SendMessageRequest,
    ) -> Result<String, rupu_cp::session_sender::SendError> {
        self.0
            .lock()
            .unwrap()
            .push(format!("send {}", req.session_id));
        Ok("run_sent".into())
    }
}

/// A git fetcher with no network: every git source fails to fetch.
struct NoGit;

impl rupu_skills::Fetch for NoGit {
    fn fetch(
        &self,
        _src: &rupu_skills::GitSource,
    ) -> Result<rupu_skills::FetchedTree, rupu_skills::SourceError> {
        Err(rupu_skills::SourceError::Clone(
            "no network in tests".into(),
        ))
    }
}

pub(super) struct Setup {
    pub addr: SocketAddr,
    pub global: PathBuf,
    /// A registered project with a `.rupu/` (`WS`).
    pub project: PathBuf,
    /// A directory holding one valid skill, `demo-skill`.
    pub skill_src: PathBuf,
    pub mutator: Arc<RecordingMutator>,
    pub sender: Arc<RecordingSender>,
    _tmp: tempfile::TempDir,
}

fn write_session(global: &Path, dir: &str, id: &str, status: &str) {
    let d = global.join(dir).join(id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("session.json"),
        json!({"session_id": id, "status": status}).to_string(),
    )
    .unwrap();
}

pub(super) fn write_skill(dir: &Path, name: &str, description: &str) {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nDo the thing.\n"),
    )
    .unwrap();
}

/// A writable CP (`writable: false` drops the launcher, as a CP outside
/// `cp serve` has none) with the recording session mutator and, when
/// `installer`, the no-network skill installer; seeded with three sessions
/// (idle, running, archived) and one registered project.
pub(super) async fn spawn_setup(writable: bool, installer: bool) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("home");
    let project = tmp.path().join("project");
    let skill_src = tmp.path().join("skill-src");
    std::fs::create_dir_all(project.join(".rupu")).unwrap();
    write_skill(&skill_src, "demo-skill", "Demonstrates an install.");
    write_session(&global, "sessions", LIVE, "idle");
    write_session(&global, "sessions", BUSY, "running");
    write_session(&global, "sessions-archive", SHELVED, "idle");
    std::fs::create_dir_all(global.join("workspaces")).unwrap();
    std::fs::write(
        global.join("workspaces").join(format!("{WS}.toml")),
        format!(
            "id = \"{WS}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            project.display()
        ),
    )
    .unwrap();

    let mutator = Arc::new(RecordingMutator::default());
    let sender = Arc::new(RecordingSender::default());
    let mut state = rupu_cp::state::AppState::new(global.clone(), Default::default())
        .with_session_mutator(Some(mutator.clone()))
        .with_session_sender(Some(sender.clone()));
    if writable {
        state = state.with_launcher(Some(Arc::new(NoopLauncher)));
    }
    if installer {
        state = state.with_skill_installer(Some(Arc::new(NoGit)));
    }
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
    Setup {
        addr,
        global,
        project,
        skill_src,
        mutator,
        sender,
        _tmp: tmp,
    }
}

pub(super) async fn http_get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let r = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn setup_tools_are_listed_with_their_required_arguments() {
    let s = spawn_setup(true, true).await;
    let table: &[(&str, &[&str])] = &[
        ("sessions.stop", &["id"]),
        ("sessions.compact", &["id"]),
        ("skills.install", &["source"]),
        ("skills.update", &[]),
        ("skills.validate", &[]),
        ("fleet.list", &[]),
        ("fleet.install", &[]),
        ("projects.init", &["path"]),
        ("engagement_profiles.list", &[]),
        ("engagement_profiles.get", &["id"]),
    ];
    assert_required_args(s.addr, table).await;
    let list = rpc(s.addr, "tools/list", json!({})).await["result"]["tools"].clone();
    let hint = |name: &str, key: &str| -> Value {
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap()["annotations"][key]
            .clone()
    };
    assert_eq!(hint("sessions.stop", "destructiveHint"), json!(true));
    assert_eq!(hint("skills.install", "destructiveHint"), json!(true));
    assert_eq!(hint("skills.install", "openWorldHint"), json!(true));
    assert_eq!(hint("skills.update", "openWorldHint"), json!(true));
    assert_eq!(hint("skills.validate", "readOnlyHint"), json!(true));
    assert_eq!(hint("fleet.install", "destructiveHint"), json!(true));
    assert_eq!(hint("fleet.list", "readOnlyHint"), json!(true));
    assert_eq!(hint("engagement_profiles.get", "readOnlyHint"), json!(true));
}

#[tokio::test]
async fn setup_reads_answer_what_the_api_answers() {
    let s = spawn_setup(true, true).await;
    let cases: &[(&str, Value, String)] = &[
        ("fleet.list", json!({}), "/api/fleet".into()),
        (
            "fleet.list",
            json!({"ws_id": WS}),
            format!("/api/fleet?ws_id={WS}"),
        ),
        (
            "engagement_profiles.list",
            json!({}),
            "/api/engagement-profiles".into(),
        ),
        (
            "engagement_profiles.get",
            json!({"id": "network"}),
            "/api/engagement-profiles/network".into(),
        ),
    ];
    for (tool, args, path) in cases {
        let r = call_tool(s.addr, tool, args.clone()).await;
        let (status, want) = http_get(s.addr, path).await;
        assert_eq!(status, 200, "{tool}: {path}: {want}");
        assert_ne!(r["isError"], json!(true), "{tool}: {r}");
        assert_eq!(json_of(&r), want, "{tool}");
    }
}

#[tokio::test]
async fn setup_writes_record_through_mcp() {
    let s = spawn_setup(true, true).await;

    let r = call_tool(s.addr, "fleet.install", json!({})).await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let created = json_of(&r)["created"].as_u64().unwrap();
    assert_eq!(created as usize, rupu_templates::FLEET_MANIFEST.len());
    assert!(s.global.join("agents/recon.md").is_file());

    let src = s.skill_src.display().to_string();
    let r = call_tool(s.addr, "skills.install", json!({"source": src})).await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert_eq!(json_of(&r)["installed"][0]["name"], "demo-skill", "{r}");
    assert!(s.global.join("skills/demo-skill/SKILL.md").is_file());

    let fresh = s.global.parent().unwrap().join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    let r = call_tool(
        s.addr,
        "projects.init",
        json!({"path": fresh.display().to_string()}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    assert!(fresh.join(".rupu/config.toml").is_file());

    let r = call_tool(s.addr, "sessions.stop", json!({"id": LIVE})).await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let r = call_tool(
        s.addr,
        "sessions.compact",
        json!({"id": LIVE, "window": 4096}),
    )
    .await;
    assert_ne!(r["isError"], json!(true), "{r}");
    let calls = s.mutator.0.lock().unwrap().clone();
    assert_eq!(calls[0], format!("stop {LIVE}"));
    assert!(
        calls[1].starts_with(&format!("compact {LIVE} Some(4096) ")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_refused_write_keeps_its_status_through_mcp() {
    let s = spawn_setup(true, true).await;
    let r = call_tool(s.addr, "sessions.compact", json!({"id": BUSY})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).contains("409"), "{r}");
    let r = call_tool(s.addr, "skills.install", json!({"source": "owner/repo"})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(text_of(&r).contains("502"), "{r}");
}

#[tokio::test]
async fn a_project_id_equal_to_init_never_initialises_anything() {
    // `POST /api/projects/init` sits beside `GET /api/projects/:ws_id`: a
    // `projects.get` naming `init` must not reach the init route.
    let s = spawn_setup(true, true).await;
    let r = call_tool(s.addr, "projects.get", json!({"ws_id": "init"})).await;
    assert_eq!(r["isError"], json!(true), "{r}");
    assert!(!s.global.join(".rupu").exists());
}
