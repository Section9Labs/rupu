use super::*;
use std::{fs, sync::Mutex};
#[derive(Default)]
struct Spy(Mutex<Vec<(SessionStartRequest, Binding)>>);
#[async_trait::async_trait]
impl SessionStarter for Spy {
    async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
        panic!("name-based start is forbidden")
    }
    fn supports_exact_global_conversation(&self) -> bool {
        true
    }
    fn supports_session_tool_ceiling(&self) -> bool {
        true
    }
    fn supports_session_tool_selection(&self) -> bool {
        true
    }
    fn supports_session_mcp(&self) -> bool {
        true
    }
    async fn start_exact_global(
        &self,
        req: SessionStartRequest,
        binding: Binding,
    ) -> Result<String, SessionStartError> {
        self.0.lock().unwrap().push((req, binding));
        Ok("ses_exact".into())
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
    s.session_starter = Some(spy.clone());
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
    serde_json::from_value(json!({"binding":binding,"prompt":"Review","mcp_tools":[]})).unwrap()
}
#[tokio::test]
async fn exact_conversation_review_and_receipt_preserve_separate_definition_and_project() {
    let (_tmp, s, spy) = fixture();
    let before = fs::read(s.global_dir.join("workspaces/ws_chosen.toml")).unwrap();
    let Json(r) = review(State(s.clone()), Query(query())).await.unwrap();
    assert!(spy.0.lock().unwrap().is_empty());
    let binding: Binding = serde_json::from_value(r["binding"].clone()).unwrap();
    let Json(receipt) = start(State(s.clone()), Json(body(binding.clone())))
        .await
        .unwrap();
    assert_eq!(receipt["schema"], FEATURE);
    assert_eq!(receipt["session_id"], "ses_exact");
    assert!(receipt.get("run_id").is_none());
    assert_eq!(receipt["binding"], r["binding"]);
    let calls = spy.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let req = &calls[0].0;
    assert_eq!(req.agent, "analyst");
    assert_eq!(req.workspace_id.as_deref(), Some("ws_chosen"));
    assert_eq!(
        req.working_dir.as_deref(),
        Some(binding.project_path.as_str())
    );
    assert_eq!(req.mode.as_deref(), Some("readonly"));
    assert_eq!(req.prompt.as_deref(), Some("Review"));
    assert!(!req.idle);
    assert!(req.target.is_none());
    assert_eq!(req.mcp_tools, Some(vec![]));
    assert_eq!(
        before,
        fs::read(s.global_dir.join("workspaces/ws_chosen.toml")).unwrap()
    );
}
#[tokio::test]
async fn source_and_registration_drift_never_dispatch() {
    for change_source in [true, false] {
        let (_tmp, s, spy) = fixture();
        let b = exact_run::load(&s, "file.md", "ws_chosen")
            .unwrap()
            .review
            .binding;
        let path = if change_source {
            s.global_dir.join("agents/file.md")
        } else {
            s.global_dir.join("workspaces/ws_chosen.toml")
        };
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("\n");
        fs::write(path, text).unwrap();
        assert_eq!(
            start(State(s), Json(body(b)))
                .await
                .err()
                .unwrap()
                .0
                .as_u16(),
            409
        );
        assert!(spy.0.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn remote_missing_adapter_ask_and_unknown_options_refuse_without_dispatch() {
    let (_tmp, s, spy) = fixture();
    let b = exact_run::load(&s, "file.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    let mut q = query();
    q.host = Some("remote".into());
    assert!(review(State(s.clone()), Query(q)).await.is_err());
    let mut missing = s.clone();
    missing.session_starter = None;
    assert_eq!(
        review(State(missing), Query(query()))
            .await
            .err()
            .unwrap()
            .0
            .as_u16(),
        501
    );
    for mode in ["ask", "invalid"] {
        let mut v = body(b.clone());
        v.mode = Some(mode.into());
        assert_eq!(
            start(State(s.clone()), Json(v))
                .await
                .err()
                .unwrap()
                .0
                .as_u16(),
            400
        );
    }
    for key in [
        "working_dir",
        "scope_kind",
        "idle",
        "target",
        "skill_preset",
        "approval_channel",
    ] {
        let mut v = json!({"binding":b,"prompt":"Review"});
        v[key] = json!("unexpected");
        assert!(serde_json::from_value::<Body>(v).is_err());
    }
    assert!(spy.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn invalid_first_turn_and_provider_pair_are_predispatch_errors() {
    let (_tmp, s, spy) = fixture();
    let b = exact_run::load(&s, "file.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    for prompt in [" ".into(), "x".repeat(32769)] {
        let mut v = body(b.clone());
        v.prompt = prompt;
        assert!(start(State(s.clone()), Json(v)).await.is_err());
    }
    let mut v = body(b);
    v.provider = Some("openai".into());
    assert!(start(State(s), Json(v)).await.is_err());
    assert!(spy.0.lock().unwrap().is_empty());
}
#[tokio::test]
async fn uncertain_starter_result_is_not_retried_or_replaced_by_name_lookup() {
    struct Uncertain(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl SessionStarter for Uncertain {
        fn supports_exact_global_conversation(&self) -> bool {
            true
        }
        fn supports_session_mcp(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            panic!("legacy path")
        }
        async fn start_exact_global(
            &self,
            _: SessionStartRequest,
            _: Binding,
        ) -> Result<String, SessionStartError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(SessionStartError::Spawn("receipt unavailable".into()))
        }
    }
    let (_tmp, mut s, _) = fixture();
    let b = exact_run::load(&s, "file.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    let spy = Arc::new(Uncertain(std::sync::atomic::AtomicUsize::new(0)));
    s.session_starter = Some(spy.clone());
    assert_eq!(
        start(State(s), Json(body(b)))
            .await
            .err()
            .unwrap()
            .0
            .as_u16(),
        500
    );
    assert_eq!(spy.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exact_conversation_preserves_unset_empty_and_custom_declarations() {
    for selected in [None, Some(vec![]), Some(vec!["read_file".into()])] {
        let (_tmp, s, spy) = fixture();
        let b = exact_run::load(&s, "file.md", "ws_chosen")
            .unwrap()
            .review
            .binding;
        let mut v = body(b);
        v.tools = selected.clone();
        v.skills = vec!["global:review".into()];
        v.mcp_tools = None;
        let _ = start(State(s), Json(v)).await.unwrap();
        let calls = spy.0.lock().unwrap();
        assert_eq!(calls[0].0.tools, selected);
        assert!(calls[0].0.mcp_tools.is_none());
        assert_eq!(calls[0].0.skills, vec!["global:review"]);
    }
}

#[tokio::test]
async fn exact_conversation_preserves_optional_ceiling_and_rejects_unknown_names() {
    for ceiling in [None, Some(vec![]), Some(vec!["skills.load".to_string()])] {
        let (_tmp, s, spy) = fixture();
        let binding = exact_run::load(&s, "file.md", "ws_chosen")
            .unwrap()
            .review
            .binding;
        let mut request = body(binding);
        request.tool_ceiling = ceiling.clone();
        let _ = start(State(s), Json(request)).await.unwrap();
        assert_eq!(spy.0.lock().unwrap()[0].0.tool_ceiling, ceiling);
    }
    for names in [
        vec!["*".into()],
        vec!["unknown.tool".into()],
        vec!["skill_load".into()],
    ] {
        let (_tmp, s, spy) = fixture();
        let binding = exact_run::load(&s, "file.md", "ws_chosen")
            .unwrap()
            .review
            .binding;
        let mut request = body(binding);
        request.tool_ceiling = Some(names);
        assert_eq!(
            start(State(s), Json(request))
                .await
                .err()
                .unwrap()
                .0
                .as_u16(),
            400
        );
        assert!(spy.0.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn exact_conversation_ceiling_refuses_legacy_starter_before_dispatch() {
    struct Legacy;
    #[async_trait::async_trait]
    impl SessionStarter for Legacy {
        fn supports_exact_global_conversation(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            panic!("must not dispatch")
        }
        async fn start_exact_global(
            &self,
            _: SessionStartRequest,
            _: Binding,
        ) -> Result<String, SessionStartError> {
            panic!("must not discard ceiling")
        }
    }
    let (_tmp, mut s, _) = fixture();
    let binding = exact_run::load(&s, "file.md", "ws_chosen")
        .unwrap()
        .review
        .binding;
    s.session_starter = Some(Arc::new(Legacy));
    let mut request = body(binding);
    request.mcp_tools = None;
    request.tool_ceiling = Some(vec![]);
    assert_eq!(
        start(State(s), Json(request))
            .await
            .err()
            .unwrap()
            .0
            .as_u16(),
        501
    );
}
