use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[derive(Default)]
struct Starter(Mutex<Vec<SessionStartRequest>>);
#[async_trait::async_trait]
impl crate::session_starter::SessionStarter for Starter {
    fn supports_session_tool_ceiling(&self) -> bool {
        true
    }
    fn supports_session_mcp(&self) -> bool {
        true
    }
    fn supports_session_tool_selection(&self) -> bool {
        true
    }
    fn supports_reasoning_effort(&self) -> bool {
        true
    }
    fn supports_session_capabilities(&self) -> bool {
        true
    }
    fn supports_builtin_harness(&self) -> bool {
        true
    }
    fn supports_idle_harness(&self) -> bool {
        true
    }
    async fn start(&self, request: SessionStartRequest) -> Result<String, SessionStartError> {
        self.0.lock().unwrap().push(request);
        Ok("ses_canonical".into())
    }
}
struct Launcher;
#[async_trait::async_trait]
impl crate::launcher::RunLauncher for Launcher {
    async fn launch(
        &self,
        _: crate::launcher::LaunchRequest,
    ) -> Result<String, crate::launcher::LaunchError> {
        panic!("session creation does not launch a workflow")
    }
}
fn setup() -> (tempfile::TempDir, AppState, String, Arc<Starter>) {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        global.join("config.toml"),
        "default_provider = 'openai'\ndefault_model = 'configured-model'\n",
    )
    .unwrap();
    let workspace = rupu_workspace::register(
        &rupu_workspace::WorkspaceStore {
            root: global.join("workspaces"),
        },
        &project,
    )
    .unwrap();
    let starter = Arc::new(Starter::default());
    let state = AppState::new(global, rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(Launcher)))
        .with_session_starter(Some(starter.clone()));
    (tmp, state, workspace.id, starter)
}
async fn post(s: AppState, id: &str, query: &str, body: Value) -> (StatusCode, Value) {
    let response = super::super::projects::routes()
        .with_state(s)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/projects/{id}/sessions{query}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn builtin_creation_requires_no_project_agent_and_freezes_the_first_turn_selection() {
    let (tmp, state, id, starter) = setup();
    let (status, response) = post(state, &id, "?host=local", json!({"prompt":"implement the request", "provider":"openai", "model":"uncatalogued-model", "mode":"bypass"})).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(
        response,
        json!({"session_id":"ses_canonical", "host_id":"local", "agent":"rupu-session", "provider":"openai", "model":"uncatalogued-model"})
    );
    let calls = starter.0.lock().unwrap();
    let request = &calls[0];
    assert_eq!(request.agent, SESSION_AGENT_NAME);
    assert_eq!(request.provider.as_deref(), Some("openai"));
    assert_eq!(request.model.as_deref(), Some("uncatalogued-model"));
    assert_eq!(request.workspace_id.as_deref(), Some(id.as_str()));
    assert_eq!(
        request.working_dir.as_deref(),
        tmp.path().join("project").canonicalize().unwrap().to_str()
    );
    assert_eq!(request.mode.as_deref(), Some("bypass"));
    assert!(request.target.is_none());
    assert!(!tmp.path().join("project/.rupu/agents").exists());
}

#[tokio::test]
async fn builtin_defaults_are_readonly_and_instance_owned() {
    let (tmp, state, id, starter) = setup();
    std::fs::create_dir_all(tmp.path().join(".rupu")).unwrap();
    std::fs::write(
        tmp.path().join(".rupu/config.toml"),
        "default_provider = 'wrong-ancestor'\n",
    )
    .unwrap();
    let (status, response) = post(state.clone(), &id, "", json!({"prompt":"inspect"})).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["provider"], "openai");
    assert_eq!(response["model"], "configured-model");
    let (status, selected) = post(
        state,
        &id,
        "",
        json!({"prompt":"inspect", "model":"manual-model"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{selected}");
    assert_eq!(selected["provider"], "openai");
    assert_eq!(selected["model"], "manual-model");
    assert_eq!(
        starter.0.lock().unwrap()[0].mode.as_deref(),
        Some("readonly")
    );
}

#[tokio::test]
async fn builtin_refuses_ambiguous_fields_remote_routing_and_permission_degradation() {
    let (_tmp, state, id, starter) = setup();
    for body in [
        json!({"prompt":"inspect", "agent":"specialist"}),
        json!({"prompt":"inspect", "host":"remote"}),
        json!({"prompt":"inspect", "working_dir":"/tmp"}),
        json!({"prompt":"inspect", "credentials":"ignored?"}),
        json!({"prompt":"inspect", "provider":"openai"}),
        json!({"prompt":"inspect", "provider":"unknown", "model":"m"}),
        json!({"prompt":"inspect", "mode":"ask"}),
        json!({"prompt":"inspect", "mode":"yolo"}),
        json!({"prompt":"inspect", "model":" leading-space"}),
        json!({"prompt":"   "}),
    ] {
        let (status, response) = post(state.clone(), &id, "", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {response}");
    }
    assert_eq!(
        post(
            state.clone(),
            &id,
            "?host=remote",
            json!({"prompt":"inspect"})
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        post(state.clone(), &id, "?target=x", json!({"prompt":"inspect"}))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(state, "ws_missing", "", json!({"prompt":"inspect"}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert!(starter.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn builtin_capability_tracks_adapter_and_writable_deployment() {
    let (_tmp, state, id, starter) = setup();
    assert!(available(&state));
    let readonly = state.clone().with_launcher(None);
    assert!(!available(&readonly));
    assert_eq!(
        post(readonly, &id, "", json!({"prompt":"inspect"})).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    let missing = state.with_session_starter(None);
    assert!(!available(&missing));
    assert_eq!(
        post(missing, &id, "", json!({"prompt":"inspect"})).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert!(starter.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn host_info_advertises_builtin_creation_only_when_the_route_can_execute() {
    let (_tmp, state, _id, _starter) = setup();
    for (state, expected) in [
        (state.clone(), true),
        (state.with_session_starter(None), false),
    ] {
        let response = super::super::host_info::routes()
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/api/host/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            response["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == CAP_BUILTIN_HARNESS),
            expected
        );
        assert_eq!(
            response["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == CAP_IDLE_HARNESS),
            expected
        );
        assert_eq!(
            response["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "session.skill_preset.v1"),
            expected
        );
        assert!(response["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == super::super::session_capabilities::FEATURE));
    }
}

#[tokio::test]
async fn idle_creation_has_no_prompt_and_requires_the_explicit_flag() {
    let (_tmp, state, id, starter) = setup();
    let (status, response) = post(state.clone(), &id, "", json!({"idle":true})).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    {
        let calls = starter.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].idle);
        assert!(calls[0].prompt.is_none());
        assert!(calls[0].target.is_none());
        assert_eq!(calls[0].mode.as_deref(), Some("readonly"));
    }
    for body in [
        json!({}),
        json!({"idle":false}),
        json!({"idle":true,"prompt":"go"}),
        json!({"idle":true,"prompt":""}),
    ] {
        assert_eq!(
            post(state.clone(), &id, "", body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(starter.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn idle_creation_refuses_an_adapter_that_only_supports_first_turn_creation() {
    struct FirstTurnOnly;
    #[async_trait::async_trait]
    impl crate::session_starter::SessionStarter for FirstTurnOnly {
        fn supports_builtin_harness(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            panic!("idle creation must not call an unsupported adapter")
        }
    }
    let (_tmp, state, id, _) = setup();
    let state = state.with_session_starter(Some(Arc::new(FirstTurnOnly)));
    assert!(available(&state));
    assert!(!idle_available(&state));
    assert_eq!(
        post(state, &id, "", json!({"idle":true})).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn preset_selection_is_exact_and_preserved() {
    let (_tmp, state, id, starter) = setup();
    let body = json!({"idle":true,"skill_preset":"none","skills":["global:work-units"],"preload_skills":["project:notes"]});
    let (status, response) = post(state.clone(), &id, "", body).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    {
        let calls = starter.0.lock().unwrap();
        assert_eq!(calls[0].skill_preset.as_deref(), Some("none"));
        assert_eq!(calls[0].skills, ["global:work-units"]);
        assert_eq!(calls[0].preload_skills, ["project:notes"]);
    }
    for body in [
        json!({"idle":true,"skill_preset":"all"}),
        json!({"idle":true,"skills":["*"]}),
        json!({"idle":true,"skills":["global:*"]}),
        json!({"idle":true,"skills":["bare-name"]}),
        json!({"idle":true,"preload_skills":["project:*"]}),
        json!({"idle":true,"preload_skills":["global:../escape"]}),
    ] {
        assert_eq!(
            post(state.clone(), &id, "", body).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(starter.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn skill_options_cannot_be_silently_ignored_by_old_starters() {
    struct OldStarter;
    #[async_trait::async_trait]
    impl crate::session_starter::SessionStarter for OldStarter {
        fn supports_builtin_harness(&self) -> bool {
            true
        }
        fn supports_idle_harness(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            panic!("unsupported selection must be refused before starting");
        }
    }
    let (_tmp, state, id, _) = setup();
    let state = state.with_session_starter(Some(Arc::new(OldStarter)));
    for body in [
        json!({"idle":true,"skill_preset":"none"}),
        json!({"idle":true,"skills":["global:work-units"]}),
    ] {
        assert_eq!(
            post(state.clone(), &id, "", body).await.0,
            StatusCode::NOT_IMPLEMENTED
        );
    }
}

#[tokio::test]
async fn first_turn_tools_preserve_default_empty_and_explicit_selections() {
    let (_tmp, state, id, starter) = setup();
    for (body, expected) in [
        (json!({"prompt":"inspect"}), None),
        (json!({"prompt":"inspect", "tools":null}), None),
        (json!({"prompt":"inspect", "tools":[]}), Some(vec![])),
        (
            json!({"prompt":"inspect", "tools":["read_file", "scm.*"]}),
            Some(vec!["read_file".to_string(), "scm.*".to_string()]),
        ),
        (
            json!({"prompt":"inspect", "tools":["*"]}),
            Some(vec!["*".to_string()]),
        ),
    ] {
        let before = starter.0.lock().unwrap().len();
        let (status, response) = post(state.clone(), &id, "?host=local", body).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        let calls = starter.0.lock().unwrap();
        assert_eq!(
            calls.len(),
            before + 1,
            "one creation dispatch, no follow-up mutation"
        );
        assert_eq!(calls[before].tools, expected);
        assert_eq!(calls[before].mode.as_deref(), Some("readonly"));
        assert_eq!(calls[before].prompt.as_deref(), Some("inspect"));
        assert!(!calls[before].idle);
    }
}

#[tokio::test]
async fn first_turn_tool_validation_refuses_before_dispatch() {
    let (_tmp, state, id, starter) = setup();
    for tools in [
        json!(["unknown_tool"]),
        json!(["runs.list"]),
        json!(["--mode"]),
        json!(vec!["read_file"; 257]),
        json!("read_file"),
    ] {
        let (status, response) = post(
            state.clone(),
            &id,
            "",
            json!({"prompt":"inspect", "tools":tools}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
    }
    assert!(starter.0.lock().unwrap().is_empty());
}

#[derive(Default)]
struct LegacyToolStarter(Mutex<Vec<SessionStartRequest>>);
#[async_trait::async_trait]
impl crate::session_starter::SessionStarter for LegacyToolStarter {
    fn supports_builtin_harness(&self) -> bool {
        true
    }
    async fn start(&self, request: SessionStartRequest) -> Result<String, SessionStartError> {
        assert!(request.tools.is_none());
        self.0.lock().unwrap().push(request);
        Ok("ses_legacy".into())
    }
}

#[tokio::test]
async fn old_starters_refuse_explicit_tools_without_breaking_default_requests() {
    let (_tmp, state, id, _) = setup();
    let legacy = Arc::new(LegacyToolStarter::default());
    let state = state.with_session_starter(Some(legacy.clone()));
    assert!(!tool_selection_available(&state));
    for tools in [json!([]), json!(["read_file"])] {
        assert_eq!(
            post(
                state.clone(),
                &id,
                "",
                json!({"prompt":"inspect", "tools":tools})
            )
            .await
            .0,
            StatusCode::NOT_IMPLEMENTED
        );
    }
    assert!(legacy.0.lock().unwrap().is_empty());
    for body in [
        json!({"prompt":"inspect"}),
        json!({"prompt":"inspect", "tools":null}),
    ] {
        assert_eq!(post(state.clone(), &id, "", body).await.0, StatusCode::OK);
    }
    assert_eq!(legacy.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn first_turn_tool_feature_requires_the_matching_writable_starter() {
    let (_tmp, state, _id, _) = setup();
    for (state, expected) in [
        (state.clone(), true),
        (state.clone().with_launcher(None), false),
        (state.clone().with_session_starter(None), false),
        (
            state.with_session_starter(Some(Arc::new(LegacyToolStarter::default()))),
            false,
        ),
    ] {
        assert_eq!(tool_selection_available(&state), expected);
        let response = super::super::host_info::routes()
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/api/host/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            response["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == CAP_TOOL_SELECTION),
            expected
        );
    }
}

#[tokio::test]
async fn mcp_creation_dispatches_once_and_keeps_default_empty_and_exact_selection() {
    let (_tmp, state, id, starter) = setup();
    for (body, expected) in [
        (json!({"prompt":"inspect"}), None),
        (json!({"prompt":"inspect","mcp_tools":null}), None),
        (json!({"prompt":"inspect","mcp_tools":[]}), Some(vec![])),
        (
            json!({"prompt":"inspect","mcp_tools":[{"server":"docs","tool":"search"}],"mcp_catalog_revision":"opaque"}),
            Some(vec![rupu_runtime::session_mcp::ToolRef {
                server: "docs".into(),
                tool: "search".into(),
            }]),
        ),
    ] {
        let count = starter.0.lock().unwrap().len();
        let (status, response) = post(state.clone(), &id, "?host=local", body).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        let calls = starter.0.lock().unwrap();
        assert_eq!(calls.len(), count + 1);
        assert_eq!(calls[count].mcp_tools, expected);
        assert_eq!(calls[count].mode.as_deref(), Some("readonly"));
    }
}
#[tokio::test]
async fn mcp_creation_requires_supported_starter_and_maps_catalog_conflict() {
    struct Conflict;
    #[async_trait::async_trait]
    impl crate::session_starter::SessionStarter for Conflict {
        fn supports_builtin_harness(&self) -> bool {
            true
        }
        fn supports_session_mcp(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            Err(SessionStartError::Conflict("catalog changed".into()))
        }
    }
    let (_tmp, state, id, _) = setup();
    let state = state.with_session_starter(Some(Arc::new(Conflict)));
    assert_eq!(
        post(state, &id, "", json!({"prompt":"inspect","mcp_tools":[]}))
            .await
            .0,
        StatusCode::CONFLICT
    );
    struct Old;
    #[async_trait::async_trait]
    impl crate::session_starter::SessionStarter for Old {
        fn supports_builtin_harness(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            panic!("unsupported MCP request cannot dispatch")
        }
    }
    let (_tmp, state, id, _) = setup();
    let state = state.with_session_starter(Some(Arc::new(Old)));
    assert_eq!(
        post(state, &id, "", json!({"prompt":"inspect","mcp_tools":[]}))
            .await
            .0,
        StatusCode::NOT_IMPLEMENTED
    );
}

#[tokio::test]
async fn first_turn_reasoning_is_forwarded_and_unsupported_levels_are_rejected() {
    let (_tmp, state, id, starter) = setup();
    let (status, _) = post(
        state.clone(),
        &id,
        "",
        json!({"prompt":"Review", "provider":"openai", "model":"gpt-6.1-sol", "effort":"high"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(starter.0.lock().unwrap()[0].effort.as_deref(), Some("high"));
    let (status, _) = post(
        state,
        &id,
        "",
        json!({"prompt":"Review", "provider":"openai", "model":"gpt-4.1", "effort":"high"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(starter.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn first_turn_reasoning_keeps_exact_tools_mcp_and_skills_in_one_dispatch() {
    let (_tmp, state, id, starter) = setup();
    let (status, response) = post(
        state,
        &id,
        "?host=local",
        json!({
            "prompt":"Review", "provider":"openai", "model":"gpt-6.1-sol", "effort":"high",
            "tools":[], "mcp_tools":[{"server":"docs","tool":"search"}],
            "mcp_catalog_revision":"opaque", "skill_preset":"none", "skills":["project:review"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    let calls = starter.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.effort.as_deref(), Some("high"));
    assert_eq!(call.tools, Some(vec![]));
    assert_eq!(
        call.mcp_tools,
        Some(vec![rupu_runtime::session_mcp::ToolRef {
            server: "docs".into(),
            tool: "search".into()
        }])
    );
    assert_eq!(call.mcp_catalog_revision.as_deref(), Some("opaque"));
    assert_eq!(call.skill_preset.as_deref(), Some("none"));
    assert_eq!(call.skills, vec!["project:review".to_string()]);
    assert_eq!(call.workspace_id.as_deref(), Some(id.as_str()));
    assert_eq!(call.mode.as_deref(), Some("readonly"));
}

#[tokio::test]
async fn mcp_tool_starter_without_reasoning_refuses_effort_before_dispatch() {
    struct NoReasoning;
    #[async_trait::async_trait]
    impl crate::session_starter::SessionStarter for NoReasoning {
        fn supports_builtin_harness(&self) -> bool {
            true
        }
        fn supports_session_tool_selection(&self) -> bool {
            true
        }
        fn supports_session_mcp(&self) -> bool {
            true
        }
        async fn start(&self, _: SessionStartRequest) -> Result<String, SessionStartError> {
            panic!("unsupported reasoning must not start a session")
        }
    }
    let (_tmp, state, id, _) = setup();
    let state = state.with_session_starter(Some(Arc::new(NoReasoning)));
    let (status, response) = post(state, &id, "", json!({
        "prompt":"Review", "provider":"openai", "model":"gpt-6.1-sol", "effort":"high", "tools":[], "mcp_tools":[]
    })).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{response}");
    assert!(response["error"]
        .as_str()
        .unwrap()
        .contains("reasoning effort"));
}

#[tokio::test]
async fn full_skill_preset_requires_independent_adapter_capability() {
    struct Full(Arc<Starter>);
    #[async_trait::async_trait]
    impl crate::session_starter::SessionStarter for Full {
        fn supports_builtin_harness(&self) -> bool {
            true
        }
        fn supports_session_capabilities(&self) -> bool {
            true
        }
        fn supports_full_skill_preset(&self) -> bool {
            true
        }
        async fn start(&self, request: SessionStartRequest) -> Result<String, SessionStartError> {
            crate::session_starter::SessionStarter::start(&*self.0, request).await
        }
    }
    let (_tmp, state, id, starter) = setup();
    let body = json!({"prompt":"inspect","skill_preset":"rupu-session-full-v1"});
    assert_eq!(
        post(state.clone(), &id, "", body.clone()).await.0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert!(starter.0.lock().unwrap().is_empty());
    let full = state
        .clone()
        .with_session_starter(Some(Arc::new(Full(starter.clone()))));
    assert_eq!(post(full.clone(), &id, "", body).await.0, StatusCode::OK);
    {
        let calls = starter.0.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].skill_preset.as_deref(),
            Some("rupu-session-full-v1")
        );
        assert_eq!(calls[0].mode.as_deref(), Some("readonly"));
        assert!(calls[0].tools.is_none() && calls[0].mcp_tools.is_none());
    }
    for (state, expected) in [
        (state, false),
        (full.clone().with_launcher(None), false),
        (full, true),
    ] {
        let response = super::super::host_info::routes()
            .with_state(state)
            .oneshot(
                Request::builder()
                    .uri("/api/host/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "session.skill_full_preset.v1"),
            expected
        );
    }
}

#[tokio::test]
async fn immutable_ceiling_creation_preserves_empty_and_exact_only_contract() {
    let (_tmp, state, id, starter) = setup();
    for ceiling in [json!([]), json!(["skills.load"])] {
        let (status, _) = post(
            state.clone(),
            &id,
            "",
            json!({"prompt":"inspect", "tool_ceiling":ceiling, "mcp_tools":[]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    {
        let calls = starter.0.lock().unwrap();
        assert_eq!(calls[0].tool_ceiling, Some(vec![]));
        assert_eq!(calls[1].tool_ceiling, Some(vec!["skills.load".into()]));
    }
    for ceiling in [
        json!(["*"]),
        json!(["skill_load"]),
        json!(["unknown"]),
        json!(["skills.load", "skills.load"]),
    ] {
        assert_eq!(
            post(
                state.clone(),
                &id,
                "",
                json!({"prompt":"inspect", "tool_ceiling":ceiling, "mcp_tools":[]})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(starter.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn old_adapter_cannot_silently_ignore_ceiling() {
    let (_tmp, state, id, _) = setup();
    let legacy = Arc::new(LegacyToolStarter::default());
    let state = state.with_session_starter(Some(legacy.clone()));
    for ceiling in [json!([]), json!(["skills.load"])] {
        assert_eq!(
            post(
                state.clone(),
                &id,
                "",
                json!({"prompt":"inspect", "tool_ceiling":ceiling})
            )
            .await
            .0,
            StatusCode::NOT_IMPLEMENTED
        );
    }
    assert!(legacy.0.lock().unwrap().is_empty());
    assert_eq!(
        post(state, &id, "", json!({"prompt":"inspect"})).await.0,
        StatusCode::OK
    );
}
