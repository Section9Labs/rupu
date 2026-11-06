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
