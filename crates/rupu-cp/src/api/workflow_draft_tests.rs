// Included in workflows::tests; all records are disposable fixture data.
struct DraftLauncher {
    global: std::path::PathBuf,
    calls: Mutex<Vec<(String, LaunchRequest)>>,
    fail: bool,
}
#[async_trait::async_trait]
impl RunLauncher for DraftLauncher {
    async fn launch(&self, _: LaunchRequest) -> Result<String, LaunchError> {
        panic!("draft must reserve its session identity")
    }
    fn supports_session_workflows(&self) -> bool {
        true
    }
    async fn launch_identified(
        &self,
        request: LaunchRequest,
        id: &str,
        _: crate::launcher::LaunchWatch,
    ) -> Result<(), LaunchError> {
        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                self.global
                    .join("sessions/ses_test/workflow-runs")
                    .join(format!("{id}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        let yaml = std::fs::read_to_string(request.file.as_ref().unwrap()).unwrap();
        assert_eq!(
            raw["association"]["source"]["draft_revision"],
            revision(&yaml)
        );
        assert_eq!(
            raw["association"]["source"]["revision"],
            revision(VALID_YAML)
        );
        assert_eq!(raw["association"]["run_id"], id);
        assert_eq!(raw["association"]["status"], "starting");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(request.file.as_ref().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        self.calls.lock().unwrap().push((id.into(), request));
        if self.fail {
            Err(LaunchError::Spawn("fixture lost spawn result".into()))
        } else {
            Ok(())
        }
    }
}
fn draft_fixture(
    fail: bool,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    AppState,
    Arc<DraftLauncher>,
) {
    let (tmp, project, state, _) = session_workflow_fixture(false);
    let launcher = Arc::new(DraftLauncher {
        global: tmp.path().into(),
        calls: Mutex::new(Vec::new()),
        fail,
    });
    (
        tmp,
        project,
        state.with_launcher(Some(launcher.clone())),
        launcher,
    )
}
fn draft_body() -> serde_json::Value {
    let yaml = VALID_YAML.replace("hi", "draft change");
    serde_json::json!({"yaml":yaml,"host":"local","inputs":{},"draft":{
        "workspace_id":"ws_a","session_id":"ses_test","draft_revision":revision(&yaml),
        "source":{"name":"demo","scope_kind":"project","scope_id":"ws_a","revision":revision(VALID_YAML)}
    }})
}
#[tokio::test]
async fn draft_preview_is_readonly_and_projects_actual_unsaved_source() {
    let (_tmp, project, state, launcher) = draft_fixture(false);
    let state = state.with_launcher(None);
    let body = draft_body();
    let (status, result) = session_workflow_request(
        state.clone(),
        "POST",
        "/api/workflows/draft/preview",
        body.clone(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(result["workflow"]["name"], "demo");
    assert_eq!(result["workspace_id"], "ws_a");
    assert_eq!(result["session_id"], "ses_test");
    assert_eq!(
        result["source"]["draft_revision"],
        body["draft"]["draft_revision"]
    );
    assert_eq!(result["source"]["revision"], revision(VALID_YAML));
    assert!(!yaml_launch_dir(&state).exists());
    assert!(!project
        .path()
        .join(".rupu/workflows/.workflow-definitions.lock")
        .exists());
    assert!(launcher.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn draft_launch_retains_baseline_and_pins_actual_source_readonly_by_default() {
    let (tmp, project, state, launcher) = draft_fixture(false);
    let body = draft_body();
    let (status, reply) =
        session_workflow_request(state.clone(), "POST", "/api/workflows/run", body.clone()).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        reply["source"]["draft_revision"],
        body["draft"]["draft_revision"]
    );
    assert_eq!(reply["association"]["source"], reply["source"]);
    let calls = launcher.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.mode.as_deref(), Some("readonly"));
    assert_eq!(
        calls[0].1.working_dir.as_deref(),
        project.path().canonicalize().unwrap().to_str()
    );
    assert_eq!(
        std::fs::read_to_string(calls[0].1.file.as_ref().unwrap()).unwrap(),
        body["yaml"].as_str().unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join(".rupu/workflows/demo.yaml")).unwrap(),
        VALID_YAML
    );
    assert!(tmp.path().join("sessions/ses_test/workflow-runs").exists());
}
#[tokio::test]
async fn draft_refuses_stale_wrong_scope_session_hash_name_and_mode_before_snapshot() {
    for case in 0..8 {
        let (_tmp, _project, state, launcher) = draft_fixture(false);
        let mut body = draft_body();
        match case {
            0 => body["draft"]["source"]["revision"] = "stale".into(),
            1 => body["draft"]["source"]["scope_id"] = "ws_other".into(),
            2 => body["draft"]["session_id"] = "ses_other".into(),
            3 => body["draft"]["draft_revision"] = "sha256:wrong".into(),
            4 => {
                body["yaml"] = VALID_YAML.replace("name: demo", "name: other").into();
                body["draft"]["draft_revision"] = revision(body["yaml"].as_str().unwrap()).into();
            }
            5 => body["mode"] = "ask".into(),
            6 => body["working_dir"] = "/tmp".into(),
            _ => body["host"] = "other".into(),
        }
        let (status, _) =
            session_workflow_request(state.clone(), "POST", "/api/workflows/run", body).await;
        assert!(!status.is_success(), "case {case}");
        assert!(!yaml_launch_dir(&state).exists(), "case {case}");
        assert!(launcher.calls.lock().unwrap().is_empty());
    }
}
#[tokio::test]
async fn draft_requires_actual_session_project_and_source_bounds() {
    let (tmp, _project, state, launcher) = draft_fixture(false);
    let path = tmp.path().join("sessions/ses_test/session.json");
    let mut session: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    session["workspace_id"] = "ws_other".into();
    std::fs::write(path, session.to_string()).unwrap();
    let (status, _) =
        session_workflow_request(state.clone(), "POST", "/api/workflows/run", draft_body()).await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT);
    assert!(!yaml_launch_dir(&state).exists());
    assert!(launcher.calls.lock().unwrap().is_empty());
    let (_tmp, _project, state, _) = draft_fixture(false);
    let mut body = draft_body();
    body["yaml"] = "x".repeat(256 * 1024 + 1).into();
    body["draft"]["draft_revision"] = revision(body["yaml"].as_str().unwrap()).into();
    let (status, _) =
        session_workflow_request(state.clone(), "POST", "/api/workflows/run", body).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(!yaml_launch_dir(&state).exists());
}
#[tokio::test]
async fn draft_readonly_control_plane_refuses_launch_and_does_not_advertise() {
    let (_tmp, _project, state, _) = draft_fixture(false);
    let state = state.with_launcher(None);
    let (status, _) =
        session_workflow_request(state.clone(), "POST", "/api/workflows/run", draft_body()).await;
    assert_eq!(status, axum::http::StatusCode::NOT_IMPLEMENTED);
    assert!(!yaml_launch_dir(&state).exists());
    let (_, info) =
        session_workflow_request(state, "GET", "/api/host/info", serde_json::Value::Null).await;
    assert!(!info["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(DRAFT_FEATURE)));
    let (_tmp, _project, state, _) = draft_fixture(false);
    let (_, info) =
        session_workflow_request(state, "GET", "/api/host/info", serde_json::Value::Null).await;
    assert!(info["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!(DRAFT_FEATURE)));
}
#[tokio::test]
async fn draft_spawn_failure_retains_reserved_identity_and_actual_source() {
    let (tmp, _project, state, launcher) = draft_fixture(true);
    let (status, _) =
        session_workflow_request(state.clone(), "POST", "/api/workflows/run", draft_body()).await;
    assert!(!status.is_success());
    let calls = launcher.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            tmp.path()
                .join("sessions/ses_test/workflow-runs")
                .join(format!("{}.json", calls[0].0)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        record["association"]["source"]["draft_revision"],
        draft_body()["draft"]["draft_revision"]
    );
    assert!(calls[0].1.file.as_ref().unwrap().exists());
}
#[tokio::test]
async fn draft_global_baseline_can_only_use_registered_session_execution_project() {
    let (_tmp, _project, state, launcher) = draft_fixture(false);
    std::fs::create_dir_all(workflows_dir(&state)).unwrap();
    std::fs::write(wf_path(&state, "demo"), VALID_YAML).unwrap();
    let mut body = draft_body();
    body["draft"]["source"]["scope_kind"] = "global".into();
    body["draft"]["source"]["scope_id"] = serde_json::Value::Null;
    body["mode"] = "bypass".into();
    let (status, reply) = session_workflow_request(state, "POST", "/api/workflows/run", body).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(reply["source"]["scope_kind"], "global");
    assert_eq!(
        launcher.calls.lock().unwrap()[0].1.mode.as_deref(),
        Some("bypass")
    );
}
#[tokio::test]
async fn draft_saved_baseline_is_bounded_before_parsing() {
    let (_tmp, project, state, launcher) = draft_fixture(false);
    let huge = format!("{}\n#{}", VALID_YAML, "x".repeat(256 * 1024));
    std::fs::write(project.path().join(".rupu/workflows/demo.yaml"), &huge).unwrap();
    let mut body = draft_body();
    body["draft"]["source"]["revision"] = revision(&huge).into();
    let (status, _) =
        session_workflow_request(state.clone(), "POST", "/api/workflows/run", body).await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(!yaml_launch_dir(&state).exists());
    assert!(launcher.calls.lock().unwrap().is_empty());
}
#[test]
fn draft_metadata_is_optional_for_legacy_saved_associations() {
    let raw = serde_json::json!({"scope_kind":"project","scope_id":"ws_a","revision":revision(VALID_YAML)});
    let source: super::super::session_workflows::Source =
        serde_json::from_value(raw.clone()).unwrap();
    assert!(source.draft_revision.is_none());
    assert_eq!(serde_json::to_value(source).unwrap(), raw);
}

#[tokio::test]
async fn draft_refuses_host_and_distribute_before_preview_or_source_pin() {
    for source in [
        VALID_YAML.replace("    prompt: hi", "    prompt: hi\n    host: worker-1"),
        VALID_YAML.replace(
            "    prompt: hi",
            "    prompt: hi\n    for_each: only\n    distribute:\n      hosts: [worker-1]",
        ),
    ] {
        assert!(Workflow::parse(&source).unwrap().has_placed_step());
        for route in ["/api/workflows/draft/preview", "/api/workflows/run"] {
            let (tmp, _project, state, launcher) = draft_fixture(false);
            let mut body = draft_body();
            body["draft"]["draft_revision"] = revision(&source).into();
            body["yaml"] = source.clone().into();
            let (status, _) = session_workflow_request(state.clone(), "POST", route, body).await;
            assert_eq!(status, axum::http::StatusCode::NOT_IMPLEMENTED);
            assert!(!yaml_launch_dir(&state).exists());
            assert!(!tmp.path().join("sessions/ses_test/workflow-runs").exists());
            assert!(launcher.calls.lock().unwrap().is_empty());
        }
    }
}
