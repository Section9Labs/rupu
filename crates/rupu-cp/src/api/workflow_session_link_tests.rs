// Included in workflows::tests: uses its disposable project/source fixtures.
struct SessionWorkflowLauncher {
    global: std::path::PathBuf,
    calls: Mutex<Vec<(String, LaunchRequest)>>,
    fail: bool,
}
#[async_trait::async_trait]
impl RunLauncher for SessionWorkflowLauncher {
    async fn launch(&self, _: LaunchRequest) -> Result<String, LaunchError> {
        panic!("a linked launch must use its reserved run identity")
    }
    fn supports_session_workflows(&self) -> bool {
        true
    }
    async fn launch_identified(
        &self,
        request: LaunchRequest,
        run_id: &str,
        _watch: crate::launcher::LaunchWatch,
    ) -> Result<(), LaunchError> {
        let path = self
            .global
            .join("sessions/ses_test/workflow-runs")
            .join(format!("{run_id}.json"));
        let durable: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let durable = &durable["association"];
        assert_eq!(durable["session_id"], "ses_test");
        assert_eq!(durable["run_id"], run_id);
        assert_eq!(durable["status"], "starting");
        assert_eq!(durable["source"]["revision"], revision(VALID_YAML));
        assert_eq!(
            std::fs::read_to_string(request.file.as_ref().unwrap()).unwrap(),
            VALID_YAML
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        self.calls.lock().unwrap().push((run_id.into(), request));
        if self.fail {
            Err(LaunchError::Spawn("test spawn refused".into()))
        } else {
            Ok(())
        }
    }
}
fn session_workflow_fixture(
    fail: bool,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    AppState,
    Arc<SessionWorkflowLauncher>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    register_workspace(&tmp, "ws_a", project.path());
    let workflows = project.path().join(".rupu/workflows");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::write(workflows.join("demo.yaml"), VALID_YAML).unwrap();
    let dir = tmp.path().join("sessions/ses_test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("session.json"),
        serde_json::json!({
            "session_id":"ses_test", "workspace_id":"ws_a", "workspace_path":project.path(),
            "status":"idle", "runs":[], "active_run_id":null,
        })
        .to_string(),
    )
    .unwrap();
    let adapter = Arc::new(SessionWorkflowLauncher {
        global: tmp.path().into(),
        calls: Mutex::new(Vec::new()),
        fail,
    });
    let state = test_state(&tmp).with_launcher(Some(adapter.clone()));
    (tmp, project, state, adapter)
}
fn session_workflow_body() -> LaunchBody {
    LaunchBody {
        session_id: Some("ses_test".into()),
        workspace_id: Some("ws_a".into()),
        expected_revision: Some(revision(VALID_YAML)),
        scope_kind: Some("project".into()),
        scope_id: Some("ws_a".into()),
        mode: Some("readonly".into()),
        ..Default::default()
    }
}
async fn session_workflow_request(
    state: AppState,
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let app = routes()
        .merge(crate::api::sessions::routes())
        .merge(crate::api::host_info::routes())
        .merge(crate::api::graph::routes())
        .with_state(state);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(if method == "GET" {
                    axum::body::Body::empty()
                } else {
                    axum::body::Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let code = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (code, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn session_workflow_link_is_durable_before_spawn_and_survives_reload_and_source_deletion() {
    let (tmp, project, state, adapter) = session_workflow_fixture(false);
    let session_path = tmp.path().join("sessions/ses_test/session.json");
    let original = std::fs::read(&session_path).unwrap();
    let (status, receipt) = session_workflow_request(state.clone(), "POST", "/api/workflows/demo/run", serde_json::json!({
        "session_id":"ses_test", "workspace_id":"ws_a", "expected_revision":revision(VALID_YAML),
        "scope_kind":"project", "scope_id":"ws_a", "mode":"readonly",
    })).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{receipt}");
    assert_eq!(receipt["session_id"], "ses_test");
    let link = &receipt["association"];
    assert_eq!(link["run_id"], receipt["run_id"]);
    assert_eq!(link["host_id"], "local");
    assert_eq!(link["workspace_id"], "ws_a");
    assert_eq!(link["workflow_name"], "demo");
    assert_eq!(link["status"], "starting");
    assert_eq!(
        adapter.calls.lock().unwrap()[0].0,
        receipt["run_id"].as_str().unwrap()
    );
    assert_eq!(
        std::fs::read(&session_path).unwrap(),
        original,
        "no fabricated turn or ancestry"
    );
    std::fs::remove_file(project.path().join(".rupu/workflows/demo.yaml")).unwrap();
    let reloaded = test_state(&tmp);
    let (status, list) = session_workflow_request(
        reloaded.clone(),
        "GET",
        "/api/sessions/ses_test/workflows?host=local",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(list["workflows"], serde_json::json!([link]));
    std::fs::create_dir_all(tmp.path().join("sessions-archive")).unwrap();
    std::fs::rename(
        tmp.path().join("sessions/ses_test"),
        tmp.path().join("sessions-archive/ses_test"),
    )
    .unwrap();
    let (_, archived) = session_workflow_request(
        reloaded,
        "GET",
        "/api/sessions/ses_test/workflows",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(archived["workflows"], list["workflows"]);
}
#[tokio::test]
async fn session_workflow_link_guards_reject_before_any_snapshot_or_spawn() {
    for case in [
        "missing",
        "archived",
        "identity",
        "project",
        "path",
        "unguarded",
        "scope",
        "workspace",
        "remote",
        "stale",
        "target",
        "traversal",
    ] {
        let (tmp, _project, state, adapter) = session_workflow_fixture(false);
        let dir = tmp.path().join("sessions/ses_test");
        let mut body = session_workflow_body();
        let expected = match case {
            "missing" => {
                body.session_id = Some("ses_missing".into());
                404
            }
            "archived" => {
                std::fs::create_dir_all(tmp.path().join("sessions-archive")).unwrap();
                std::fs::rename(&dir, tmp.path().join("sessions-archive/ses_test")).unwrap();
                409
            }
            "identity" | "project" | "path" => {
                let mut record: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(dir.join("session.json")).unwrap())
                        .unwrap();
                match case {
                    "identity" => record["session_id"] = serde_json::json!("ses_other"),
                    "project" => record["workspace_id"] = serde_json::json!("ws_other"),
                    _ => record["workspace_path"] = serde_json::json!(tmp.path()),
                }
                std::fs::write(dir.join("session.json"), record.to_string()).unwrap();
                409
            }
            "unguarded" => {
                body.expected_revision = None;
                400
            }
            "scope" => {
                body.scope_kind = None;
                400
            }
            "workspace" => {
                body.workspace_id = None;
                400
            }
            "remote" => {
                body.host = Some("unknown-host".into());
                501
            }
            "stale" => {
                body.expected_revision = Some("stale".into());
                409
            }
            "target" => {
                body.target = Some("somewhere-else".into());
                400
            }
            "traversal" => {
                body.session_id = Some("../ses_test".into());
                400
            }
            _ => unreachable!(),
        };
        let error = launch_run(State(state.clone()), Path("demo".into()), Some(Json(body)))
            .await
            .unwrap_err();
        assert_eq!(error.0.as_u16(), expected, "{case}: {}", error.1);
        assert!(adapter.calls.lock().unwrap().is_empty(), "{case}");
        assert!(!yaml_launch_dir(&state).exists(), "{case}");
        assert!(!dir.join("workflow-runs").exists(), "{case}");
    }
}
#[tokio::test]
async fn session_workflow_link_failed_spawn_retains_exact_identity_without_retry() {
    let (_tmp, _project, state, adapter) = session_workflow_fixture(true);
    let error = launch_run(
        State(state.clone()),
        Path("demo".into()),
        Some(Json(session_workflow_body())),
    )
    .await
    .unwrap_err();
    assert_eq!(error.0, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let list = crate::api::session_workflows::list_records(&state, "ses_test").unwrap();
    let calls = adapter.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(list["workflows"][0]["run_id"], calls[0].0);
    assert_eq!(list["workflows"][0]["status"], "launch_failed");
}
#[tokio::test]
async fn session_workflow_link_feature_is_only_advertised_by_a_capable_writable_adapter() {
    let (tmp, _project, state, _adapter) = session_workflow_fixture(false);
    let (_, info) =
        session_workflow_request(state, "GET", "/api/host/info", serde_json::Value::Null).await;
    assert!(info["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("workflow.session_link")));
    let adapter = Arc::new(MockLauncher {
        last: Mutex::new(None),
        run_id: "should_not_launch".into(),
    });
    for s in [
        test_state(&tmp),
        test_state(&tmp).with_launcher(Some(adapter.clone())),
    ] {
        let (_, info) =
            session_workflow_request(s.clone(), "GET", "/api/host/info", serde_json::Value::Null)
                .await;
        assert!(!info["features"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("workflow.session_link")));
        let error = launch_run(
            State(s),
            Path("demo".into()),
            Some(Json(session_workflow_body())),
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
    assert!(adapter.last.lock().unwrap().is_none());
}
#[tokio::test]
async fn session_workflow_link_read_rejects_remote_and_corrupt_ownership() {
    let (tmp, _project, state, _adapter) = session_workflow_fixture(false);
    let (status, _) = session_workflow_request(
        state.clone(),
        "GET",
        "/api/sessions/ses_test/workflows?host=other",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_IMPLEMENTED);
    let receipt = launch_run(
        State(state.clone()),
        Path("demo".into()),
        Some(Json(session_workflow_body())),
    )
    .await
    .unwrap()
    .0;
    let mut link = receipt["association"].clone();
    link["session_id"] = serde_json::json!("ses_other");
    let path = tmp
        .path()
        .join("sessions/ses_test/workflow-runs")
        .join(format!("{}.json", receipt["run_id"].as_str().unwrap()));
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["association"] = link;
    std::fs::write(path, stored.to_string()).unwrap();
    assert_eq!(
        crate::api::session_workflows::list_records(&state, "ses_test")
            .unwrap_err()
            .0,
        axum::http::StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn session_workflow_link_reads_actual_status_and_original_graph_after_definition_changes() {
    let (_tmp, project, state, _adapter) = session_workflow_fixture(false);
    let receipt = launch_run(
        State(state.clone()),
        Path("demo".into()),
        Some(Json(session_workflow_body())),
    )
    .await
    .unwrap()
    .0;
    let id = receipt["run_id"].as_str().unwrap();
    let record = run_record(id, "demo", "ws_a");
    assert!(record.parent_run_id.is_none());
    state.run_store.create(record, VALID_YAML).unwrap();
    std::fs::write(
        project.path().join(".rupu/workflows/demo.yaml"),
        VALID_YAML.replace("one", "changed"),
    )
    .unwrap();
    let (code, graph) = session_workflow_request(
        state.clone(),
        "GET",
        &format!("/api/runs/{id}/graph"),
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK);
    assert_eq!(graph["workflow"]["steps"][0]["id"], "one");
    let link = crate::api::session_workflows::list_records(&state, "ses_test").unwrap()
        ["workflows"][0]
        .clone();
    assert_eq!(link["status"], "completed");
    assert_eq!(
        link["source"]["revision"],
        receipt["association"]["source"]["revision"]
    );
    state.run_store.archive(id).unwrap();
    assert_eq!(
        crate::api::session_workflows::list_records(&state, "ses_test").unwrap()["workflows"][0]
            ["status"],
        "completed"
    );
}

#[tokio::test]
async fn session_workflow_link_storage_failure_prevents_spawn() {
    for blocked in ["sessions/ses_test/workflow-runs", "runs"] {
        let (tmp, _project, state, adapter) = session_workflow_fixture(false);
        std::fs::write(tmp.path().join(blocked), "not a directory").unwrap();
        assert!(launch_run(
            State(state.clone()),
            Path("demo".into()),
            Some(Json(session_workflow_body()))
        )
        .await
        .is_err());
        assert!(adapter.calls.lock().unwrap().is_empty());
        if blocked == "runs" {
            assert!(
                crate::api::session_workflows::list_records(&state, "ses_test").unwrap()
                    ["workflows"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "known storage failure must not publish a starting link"
            );
        }
    }
}

#[tokio::test]
async fn session_workflow_link_supports_global_source_in_the_session_project() {
    let (_tmp, _project, state, adapter) = session_workflow_fixture(false);
    std::fs::create_dir_all(workflows_dir(&state)).unwrap();
    std::fs::write(workflows_dir(&state).join("demo.yaml"), VALID_YAML).unwrap();
    let mut body = session_workflow_body();
    body.scope_kind = Some("global".into());
    body.scope_id = None;
    let receipt = launch_run(State(state), Path("demo".into()), Some(Json(body)))
        .await
        .unwrap()
        .0;
    assert_eq!(receipt["association"]["source"]["scope_kind"], "global");
    assert_eq!(receipt["association"]["workspace_id"], "ws_a");
    assert_eq!(adapter.calls.lock().unwrap().len(), 1);
}

#[test]
fn session_workflow_link_never_recreates_a_session_archived_during_preflight() {
    let (tmp, _project, state, _adapter) = session_workflow_fixture(false);
    let dir = crate::api::session_workflows::validate_attachment(
        &state,
        "ses_test",
        "ws_a",
        _project.path(),
    )
    .unwrap();
    std::fs::create_dir(tmp.path().join("sessions-archive")).unwrap();
    std::fs::rename(&dir, tmp.path().join("sessions-archive/ses_test")).unwrap();
    assert!(crate::api::session_workflows::create_association_dir(&dir).is_err());
    assert!(
        !dir.exists(),
        "archive must not be shadowed by an orphan active shell"
    );
}

#[tokio::test]
async fn session_workflow_link_records_exact_early_exit_and_bounded_stderr_after_archive() {
    use std::os::unix::process::ExitStatusExt;
    let (tmp, _project, state, _adapter) = session_workflow_fixture(false);
    let receipt = launch_run(
        State(state.clone()),
        Path("demo".into()),
        Some(Json(session_workflow_body())),
    )
    .await
    .unwrap()
    .0;
    let id = receipt["run_id"].as_str().unwrap();
    let stderr = tmp.path().join("startup.log");
    std::fs::write(
        &stderr,
        format!("unknown engagement profile: bad\n{}", "x".repeat(8000)),
    )
    .unwrap();
    std::fs::create_dir(tmp.path().join("sessions-archive")).unwrap();
    std::fs::rename(
        tmp.path().join("sessions/ses_test"),
        tmp.path().join("sessions-archive/ses_test"),
    )
    .unwrap();
    crate::api::session_workflows::observe_exit(
        &state,
        "ses_test",
        id,
        Ok(std::process::ExitStatus::from_raw(7 << 8)),
        &stderr,
    )
    .unwrap();
    let link = crate::api::session_workflows::list_records(&state, "ses_test").unwrap()
        ["workflows"][0]
        .clone();
    assert_eq!(link["status"], "launch_failed");
    let error = link["error"].as_str().unwrap();
    assert!(error.contains("unknown engagement profile: bad"));
    assert!(error.len() < 4300);
    assert!(!tmp.path().join("sessions/ses_test").exists());
}

#[tokio::test]
async fn session_workflow_link_monitor_loss_is_unknown_and_actual_run_metadata_wins() {
    use std::os::unix::process::ExitStatusExt;
    let (tmp, _project, state, _adapter) = session_workflow_fixture(false);
    let receipt = launch_run(
        State(state.clone()),
        Path("demo".into()),
        Some(Json(session_workflow_body())),
    )
    .await
    .unwrap()
    .0;
    let id = receipt["run_id"].as_str().unwrap();
    let path = tmp
        .path()
        .join("sessions/ses_test/workflow-runs")
        .join(format!("{id}.json"));
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["monitor_pid"] = serde_json::json!(u32::MAX);
    stored["monitor_start"] = serde_json::Value::Null;
    std::fs::write(&path, stored.to_string()).unwrap();
    assert_eq!(
        crate::api::session_workflows::list_records(&state, "ses_test").unwrap()["workflows"][0]
            ["status"],
        "unknown"
    );
    state
        .run_store
        .create(run_record(id, "demo", "ws_a"), VALID_YAML)
        .unwrap();
    crate::api::session_workflows::observe_exit(
        &state,
        "ses_test",
        id,
        Ok(std::process::ExitStatus::from_raw(7 << 8)),
        &tmp.path().join("absent.log"),
    )
    .unwrap();
    let link = crate::api::session_workflows::list_records(&state, "ses_test").unwrap()
        ["workflows"][0]
        .clone();
    assert_eq!(link["status"], "completed");
    assert!(link["error"].is_null());
}
