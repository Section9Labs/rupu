use super::*;
use std::{fs, sync::Mutex};
#[derive(Default)]
struct Spy(Mutex<Vec<(AgentLaunchRequest, Binding)>>);
#[async_trait::async_trait]
impl AgentLauncher for Spy {
    async fn launch(&self, _: AgentLaunchRequest) -> Result<String, AgentLaunchError> {
        panic!("name lookup must not be used")
    }
    fn supports_exact_global_agent(&self) -> bool {
        true
    }
    async fn launch_exact_global(
        &self,
        req: AgentLaunchRequest,
        binding: Binding,
    ) -> Result<String, AgentLaunchError> {
        self.0.lock().unwrap().push((req, binding));
        Ok("run_exact".into())
    }
}
fn fixture() -> (tempfile::TempDir, AppState, Arc<Spy>) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(global.join("agents")).unwrap();
    fs::create_dir_all(global.join("workspaces")).unwrap();
    let project = global.join("project");
    fs::create_dir(&project).unwrap();
    fs::write(
        global.join("agents/file.md"),
        "---\nname: analyst\n---\nExact instructions\n",
    )
    .unwrap();
    let ws = rupu_workspace::Workspace {
        id: "ws_chosen".into(),
        path: project.to_str().unwrap().into(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-11-09T00:00:00Z".into(),
        last_run_at: None,
    };
    fs::write(
        global.join("workspaces/ws_chosen.toml"),
        toml::to_string(&ws).unwrap(),
    )
    .unwrap();
    let spy = Arc::new(Spy::default());
    let mut s = AppState::new(global, rupu_config::PricingConfig::default());
    s.agent_launcher = Some(spy.clone());
    (tmp, s, spy)
}
fn query() -> ReviewQuery {
    ReviewQuery {
        host: Some("local".into()),
        file_key: "file.md".into(),
        ws_id: "ws_chosen".into(),
    }
}
fn body(binding: Binding) -> Body {
    serde_json::from_value(json!({"binding":binding,"prompt":"Review","host":"local"})).unwrap()
}
#[tokio::test]
async fn review_is_read_only_and_launch_keeps_exact_file_project_and_readonly_default() {
    let (_tmp, s, spy) = fixture();
    let before = fs::read(s.global_dir.join("workspaces/ws_chosen.toml")).unwrap();
    let Json(v) = review(State(s.clone()), Query(query())).await.unwrap();
    assert!(spy.0.lock().unwrap().is_empty());
    let binding: Binding = serde_json::from_value(v["binding"].clone()).unwrap();
    let Json(receipt) = launch(State(s.clone()), Json(body(binding.clone())))
        .await
        .unwrap();
    assert_eq!(receipt["binding"], v["binding"]);
    assert_eq!(receipt["run_id"], "run_exact");
    let calls = spy.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0.agent, "analyst");
    assert_eq!(calls[0].0.mode.as_deref(), Some("readonly"));
    assert_eq!(
        calls[0].0.working_dir.as_deref(),
        Some(binding.project_path.as_str())
    );
    assert!(calls[0].0.target.is_none());
    assert_eq!(
        before,
        fs::read(s.global_dir.join("workspaces/ws_chosen.toml")).unwrap()
    );
}
#[tokio::test]
async fn changed_review_and_foreign_scope_refuse_before_dispatch() {
    let (_tmp, s, spy) = fixture();
    let binding = load(&s, "file.md", "ws_chosen").unwrap().review.binding;
    fs::write(
        s.global_dir.join("agents/file.md"),
        "---\nname: analyst\n---\nChanged\n",
    )
    .unwrap();
    assert_eq!(
        launch(State(s.clone()), Json(body(binding)))
            .await
            .err()
            .unwrap()
            .0
            .as_u16(),
        409
    );
    let mut q = query();
    q.host = Some("remote".into());
    assert!(review(State(s.clone()), Query(q)).await.is_err());
    let mut q = query();
    q.ws_id = "ws_other".into();
    assert!(review(State(s), Query(q)).await.is_err());
    assert!(spy.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn missing_adapter_never_advertises_or_emulates_exact_launch() {
    let (_tmp, mut s, spy) = fixture();
    s.agent_launcher = None;
    assert!(launcher(&s).is_err());
    assert!(review(State(s), Query(query())).await.is_err());
    assert!(spy.0.lock().unwrap().is_empty());
}
#[test]
fn alternate_paths_continuation_and_session_fields_are_not_accepted() {
    let (_tmp, s, _) = fixture();
    let binding = load(&s, "file.md", "ws_chosen").unwrap().review.binding;
    for field in [
        "working_dir",
        "target",
        "continue_from",
        "scope_id",
        "session_id",
    ] {
        let mut v = json!({"binding":binding,"prompt":"x"});
        v[field] = json!("other");
        assert!(serde_json::from_value::<Body>(v).is_err());
    }
}
