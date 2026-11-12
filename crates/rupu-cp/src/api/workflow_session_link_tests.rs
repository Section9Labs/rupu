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

fn recorded_workflow(state: &AppState, project: &std::path::Path, id: &str) {
    let mut record = run_record(id, "demo", "ws_a");
    record.workspace_path = project.into();
    record.codename = Some("cobalt-harbor".into());
    state.run_store.create(record, VALID_YAML).unwrap();
}

#[tokio::test]
async fn session_workflow_attach_preserves_recorded_history_without_launching() {
    let (tmp, project, state, adapter) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_recorded");
    let session_path = tmp.path().join("sessions/ses_test/session.json");
    let run_path = tmp.path().join("runs/run_recorded/run.json");
    let session_before = std::fs::read(&session_path).unwrap();
    let run_before = std::fs::read(&run_path).unwrap();
    // The recorded graph remains authoritative after source deletion.
    std::fs::remove_file(project.path().join(".rupu/workflows/demo.yaml")).unwrap();
    let (code, receipt) = session_workflow_request(
        state.clone(),
        "POST",
        "/api/sessions/ses_test/workflows?host=local",
        serde_json::json!({"run_id":"run_recorded"}),
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK, "{receipt}");
    let link = &receipt["association"];
    assert_eq!(link["run_id"], "run_recorded");
    assert_eq!(link["origin"], "attached");
    assert!(link["source"].is_null(), "old revision must remain unknown");
    assert!(link["attached_at"].is_string());
    assert_eq!(link["status"], "completed");
    assert_eq!(std::fs::read(&session_path).unwrap(), session_before);
    assert_eq!(std::fs::read(&run_path).unwrap(), run_before);
    assert!(adapter.calls.lock().unwrap().is_empty());
    let (code, repeated) = session_workflow_request(
        state.clone(),
        "POST",
        "/api/sessions/ses_test/workflows",
        serde_json::json!({"run_id":"run_recorded"}),
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK);
    assert_eq!(repeated, receipt);
    state.run_store.archive("run_recorded").unwrap();
    assert_eq!(
        crate::api::session_workflows::list_records(&state, "ses_test").unwrap()["workflows"][0]
            ["status"],
        "completed"
    );
    let (code, graph) = session_workflow_request(
        state.clone(),
        "GET",
        "/api/runs/run_recorded/graph",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK, "{graph}");
    assert_eq!(graph["workflow"]["steps"][0]["id"], "one");
}

#[tokio::test]
async fn session_workflow_attach_supports_archived_runs_and_marks_deleted_history_unavailable() {
    let (tmp, project, state, adapter) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_archived");
    state.run_store.archive("run_archived").unwrap();
    let (code, receipt) = session_workflow_request(
        state.clone(),
        "POST",
        "/api/sessions/ses_test/workflows",
        serde_json::json!({"run_id":"run_archived"}),
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK, "{receipt}");
    std::fs::remove_dir_all(tmp.path().join("runs-archive/run_archived")).unwrap();
    assert_eq!(
        crate::api::session_workflows::list_records(&state, "ses_test").unwrap()["workflows"][0]
            ["status"],
        "unavailable"
    );
    assert!(adapter.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn session_workflow_attach_refuses_cross_project_and_workspace_path_mismatches() {
    for mismatch in ["project", "path", "registered"] {
        let (tmp, project, state, adapter) = session_workflow_fixture(false);
        let other = tempfile::tempdir().unwrap();
        let mut record = run_record("run_other", "demo", "ws_a");
        record.workspace_path = project.path().into();
        match mismatch {
            "project" => {
                record.workspace_id = "ws_other".into();
                register_workspace(&tmp, "ws_other", project.path());
            }
            "path" => {
                let path = tmp.path().join("sessions/ses_test/session.json");
                let mut session: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                session["workspace_path"] = serde_json::json!(other.path());
                std::fs::write(path, session.to_string()).unwrap();
            }
            _ => register_workspace(&tmp, "ws_a", other.path()),
        }
        state.run_store.create(record, VALID_YAML).unwrap();
        let (code, receipt) = session_workflow_request(
            state.clone(),
            "POST",
            "/api/sessions/ses_test/workflows",
            serde_json::json!({"run_id":"run_other"}),
        )
        .await;
        assert_eq!(
            code,
            axum::http::StatusCode::CONFLICT,
            "{mismatch}: {receipt}"
        );
        assert!(!tmp.path().join("sessions/ses_test/workflow-runs").exists());
        assert!(adapter.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn session_workflow_attach_refuses_remote_readonly_archived_and_unknown_request_fields() {
    for scenario in [
        "remote",
        "readonly",
        "archived",
        "body",
        "query",
        "traversal",
        "missing",
    ] {
        let (tmp, project, mut state, adapter) = session_workflow_fixture(false);
        recorded_workflow(&state, project.path(), "run_recorded");
        let (mut uri, mut body) = (
            "/api/sessions/ses_test/workflows",
            serde_json::json!({"run_id":"run_recorded"}),
        );
        let expected = match scenario {
            "remote" => {
                uri = "/api/sessions/ses_test/workflows?host=remote";
                axum::http::StatusCode::NOT_IMPLEMENTED
            }
            "readonly" => {
                state.launcher = None;
                axum::http::StatusCode::NOT_IMPLEMENTED
            }
            "archived" => {
                std::fs::create_dir_all(tmp.path().join("sessions-archive")).unwrap();
                std::fs::rename(
                    tmp.path().join("sessions/ses_test"),
                    tmp.path().join("sessions-archive/ses_test"),
                )
                .unwrap();
                axum::http::StatusCode::CONFLICT
            }
            "body" => {
                body["workspace_id"] = serde_json::json!("ws_a");
                axum::http::StatusCode::BAD_REQUEST
            }
            "query" => {
                uri = "/api/sessions/ses_test/workflows?workspace_id=ws_a";
                axum::http::StatusCode::BAD_REQUEST
            }
            "traversal" => {
                body["run_id"] = serde_json::json!("../escape");
                axum::http::StatusCode::BAD_REQUEST
            }
            _ => {
                body["run_id"] = serde_json::json!("run_missing");
                axum::http::StatusCode::NOT_FOUND
            }
        };
        let (code, receipt) = session_workflow_request(state, "POST", uri, body).await;
        assert_eq!(code, expected, "{scenario}: {receipt}");
        assert!(adapter.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn session_workflow_history_scopes_paginates_and_includes_archives() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_active");
    recorded_workflow(&state, project.path(), "run_archived");
    state.run_store.archive("run_archived").unwrap();
    state
        .run_store
        .create(
            run_record("run_unrelated", "unrelated", "ws_other"),
            VALID_YAML,
        )
        .unwrap();
    let (code, first) = session_workflow_request(
        state.clone(),
        "GET",
        "/api/projects/ws_a/workflow-history?limit=1",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK, "{first}");
    assert_eq!(first["workspace_id"], "ws_a");
    assert_eq!(first["runs"].as_array().unwrap().len(), 1);
    assert_eq!(first["next_offset"], 1);
    let (_, second) = session_workflow_request(
        state.clone(),
        "GET",
        "/api/projects/ws_a/workflow-history?offset=1&limit=1",
        serde_json::Value::Null,
    )
    .await;
    assert_ne!(first["runs"][0]["run_id"], second["runs"][0]["run_id"]);
    assert!(second["next_offset"].is_null());
    let (_, searched) = session_workflow_request(
        state,
        "GET",
        "/api/projects/ws_a/workflow-history?q=run_archived",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(searched["runs"][0]["archived"], true);
    assert_eq!(searched["runs"][0]["codename"], "cobalt-harbor");
}

#[tokio::test]
async fn session_workflow_attach_keeps_original_launch_provenance() {
    let (_tmp, project, state, adapter) = session_workflow_fixture(false);
    let original = launch_run(
        State(state.clone()),
        Path("demo".into()),
        Some(Json(session_workflow_body())),
    )
    .await
    .unwrap()
    .0;
    let id = original["run_id"].as_str().unwrap();
    recorded_workflow(&state, project.path(), id);
    let (code, receipt) = session_workflow_request(
        state,
        "POST",
        "/api/sessions/ses_test/workflows",
        serde_json::json!({"run_id":id}),
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK, "{receipt}");
    assert_eq!(
        receipt["association"]["source"],
        original["association"]["source"]
    );
    assert_eq!(receipt["association"]["origin"], "launched");
    assert!(receipt["association"]["attached_at"].is_null());
    assert_eq!(
        adapter.calls.lock().unwrap().len(),
        1,
        "attachment never starts a second execution"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn session_workflow_attach_refuses_symlinked_run_storage() {
    let (tmp, project, state, adapter) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_recorded");
    let outside = tempfile::tempdir().unwrap();
    std::fs::rename(
        tmp.path().join("runs/run_recorded"),
        outside.path().join("run_recorded"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("run_recorded"),
        tmp.path().join("runs/run_recorded"),
    )
    .unwrap();
    let (code, _) = session_workflow_request(
        state,
        "POST",
        "/api/sessions/ses_test/workflows",
        serde_json::json!({"run_id":"run_recorded"}),
    )
    .await;
    assert!(!code.is_success());
    assert!(!tmp.path().join("sessions/ses_test/workflow-runs").exists());
    assert!(adapter.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn builtin_session_workflow_link_keeps_real_permission_modes() {
    for (stored, requested, expected) in [
        ("readonly", None, Some("readonly")),
        ("bypass", None, Some("bypass")),
        ("readonly", Some("bypass"), Some("bypass")),
        ("readonly", Some("ask"), None),
        ("ask", None, None),
        ("invalid", None, None),
    ] {
        let (tmp, _project, state, adapter) = session_workflow_fixture(false);
        let path = tmp.path().join("sessions/ses_test/session.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        record["builtin_harness"] = serde_json::json!(true);
        record["permission_mode"] = serde_json::json!(stored);
        std::fs::write(path, record.to_string()).unwrap();
        let mut body = session_workflow_body();
        body.mode = requested.map(str::to_string);
        let result = launch_run(State(state), Path("demo".into()), Some(Json(body))).await;
        if let Some(expected) = expected {
            let _ = result.unwrap();
            let calls = adapter.calls.lock().unwrap();
            assert_eq!(calls[0].1.mode.as_deref(), Some(expected));
            assert!(
                rupu_runtime::strict_approval::workflow_required(tmp.path(), &calls[0].0).unwrap()
            );
        } else {
            assert_eq!(result.unwrap_err().0, axum::http::StatusCode::BAD_REQUEST);
            assert!(adapter.calls.lock().unwrap().is_empty());
            assert!(!tmp.path().join("sessions/ses_test/workflow-runs").exists());
        }
    }
}

#[tokio::test]
async fn session_workflow_attach_accepts_recorded_path_from_before_a_project_move() {
    let (_tmp, project, state, adapter) = session_workflow_fixture(false);
    let mut record = run_record("run_before_move", "demo", "ws_a");
    record.workspace_path = project.path().join("no-longer-present");
    state.run_store.create(record, VALID_YAML).unwrap();
    let (code, receipt) = session_workflow_request(
        state,
        "POST",
        "/api/sessions/ses_test/workflows",
        serde_json::json!({"run_id":"run_before_move"}),
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::OK, "{receipt}");
    assert_eq!(receipt["association"]["run_id"], "run_before_move");
    assert!(adapter.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn session_workflow_history_reads_archived_events_and_transcript_without_restoring() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (tmp, project, state, _) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_archived_read");
    let event = rupu_orchestrator::executor::Event::StepStarted {
        run_id: "run_archived_read".into(),
        step_id: "one".into(),
        kind: rupu_orchestrator::runs::StepKind::Linear,
        agent: None,
        host: None,
        codename: None,
    };
    std::fs::write(
        state.run_store.events_path("run_archived_read"),
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();
    let path = state
        .run_store
        .root
        .join("run_archived_read")
        .join("agent.jsonl");
    let transcript = concat!(
        "{\"type\":\"assistant_message\",\"data\":{\"content\":\"Recorded answer\"}}\n",
        "{\"type\":\"run_complete\",\"data\":{\"run_id\":\"run_agent\",\"status\":\"ok\",\"total_tokens\":1,\"duration_ms\":1}}\n"
    );
    std::fs::write(&path, transcript).unwrap();
    state.run_store.archive("run_archived_read").unwrap();
    let app = crate::api::events::routes()
        .merge(crate::api::transcript::routes())
        .with_state(state);
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/events/stream?host=local&run=run_archived_read")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let data = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        response.into_body().collect(),
    )
    .await
    .unwrap()
    .unwrap()
    .to_bytes();
    let text = String::from_utf8(data.to_vec()).unwrap();
    assert!(text.contains("step_started"), "{text}");
    assert!(
        text.contains("event: end"),
        "recorded SSE must settle: {text}"
    );
    let path_query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("path", &path.to_string_lossy())
        .append_pair("host", "local")
        .append_pair("run", "run_agent")
        .finish();
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/transcript?{path_query}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let page: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(page["events"][0]["data"]["content"], "Recorded answer");
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/transcript/stream?{path_query}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let data = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        response.into_body().collect(),
    )
    .await
    .unwrap()
    .unwrap()
    .to_bytes();
    let text = String::from_utf8(data.to_vec()).unwrap();
    assert!(text.contains("Recorded answer"), "{text}");
    assert!(text.contains("event: end"), "{text}");
    assert!(!tmp.path().join("runs/run_archived_read").exists());
    assert_eq!(
        std::fs::read_to_string(
            tmp.path()
                .join("runs-archive/run_archived_read/agent.jsonl")
        )
        .unwrap(),
        transcript
    );
}

#[tokio::test]
async fn builtin_session_workflow_link_refuses_remote_approval_policy_loss() {
    let (tmp, project, state, adapter) = session_workflow_fixture(false);
    let path = tmp.path().join("sessions/ses_test/session.json");
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["builtin_harness"] = serde_json::json!(true);
    record["permission_mode"] = serde_json::json!("bypass");
    std::fs::write(path, record.to_string()).unwrap();
    let source = VALID_YAML.replace("    prompt: hi", "    prompt: hi\n    host: worker-1");
    std::fs::write(project.path().join(".rupu/workflows/demo.yaml"), &source).unwrap();
    let mut body = session_workflow_body();
    body.mode = None;
    body.expected_revision = Some(revision(&source));
    let error = launch_run(State(state), Path("demo".into()), Some(Json(body)))
        .await
        .unwrap_err();
    assert_eq!(
        error.0,
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "{}",
        error.1
    );
    assert!(error.1.contains("host/distribute"));
    assert!(adapter.calls.lock().unwrap().is_empty());
    assert!(!tmp.path().join("sessions/ses_test/workflow-runs").exists());
}

#[tokio::test]
async fn session_workflow_history_capabilities_match_read_and_write_availability() {
    let (_tmp, _project, mut state, _) = session_workflow_fixture(false);
    let (_, writable) = session_workflow_request(
        state.clone(),
        "GET",
        "/api/host/info",
        serde_json::Value::Null,
    )
    .await;
    assert!(writable["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("workflow.session_history")));
    assert!(writable["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("workflow.session_attach")));
    state.launcher = None;
    let (_, readonly) =
        session_workflow_request(state, "GET", "/api/host/info", serde_json::Value::Null).await;
    assert!(readonly["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("workflow.session_history")));
    assert!(!readonly["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("workflow.session_attach")));
}

async fn history_syntax_request(
    state: AppState,
    text: &str,
    suffix: &str,
) -> (axum::http::StatusCode, serde_json::Value) {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", text)
        .finish();
    session_workflow_request(
        state,
        "GET",
        &format!("/api/projects/ws_a/workflow-history?query_mode=syntax&{encoded}{suffix}"),
        serde_json::Value::Null,
    )
    .await
}

#[tokio::test]
async fn workflow_history_query_literal_default_is_unchanged() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_normal");
    let mut literal = run_record("run_literal", "status:failed", "ws_a");
    literal.codename = Some("Literal Quote".into());
    state.run_store.create(literal, VALID_YAML).unwrap();
    for mode in ["", "&query_mode=literal"] {
        let (code, response) = session_workflow_request(
            state.clone(),
            "GET",
            &format!("/api/projects/ws_a/workflow-history?q=STATUS%3AFAILED{mode}"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(code, axum::http::StatusCode::OK);
        assert_eq!(response["runs"].as_array().unwrap().len(), 1);
        assert_eq!(response["runs"][0]["run_id"], "run_literal");
        assert!(response.get("query_mode").is_none());
    }
    let (_, syntax) = history_syntax_request(state, "status:failed", "").await;
    assert!(syntax["runs"].as_array().unwrap().is_empty());
    assert_eq!(syntax["q"], "status:failed");
    assert_eq!(syntax["query_mode"], "syntax");
}

#[tokio::test]
async fn workflow_history_query_filters_before_paging_and_keeps_archived_receipts() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    for id in ["run_a", "run_b", "run_c"] {
        recorded_workflow(&state, project.path(), id);
    }
    state.run_store.archive("run_b").unwrap();
    state.run_store.archive("run_c").unwrap();
    let query = "status:completed workflow:demo host:local is:archived -id:run_a";
    let (code, first) = history_syntax_request(state.clone(), query, "&limit=1").await;
    assert_eq!(code, axum::http::StatusCode::OK, "{first}");
    assert_eq!(first["workspace_id"], "ws_a");
    assert_eq!(first["host_id"], "local");
    assert_eq!(first["runs"][0]["archived"], true);
    assert_eq!(first["next_offset"], 1);
    let (_, second) = history_syntax_request(state, query, "&limit=1&offset=1").await;
    assert_eq!(second["runs"][0]["archived"], true);
    assert_ne!(first["runs"][0]["run_id"], second["runs"][0]["run_id"]);
    assert!(second["next_offset"].is_null());
}

#[tokio::test]
async fn workflow_history_query_resolves_active_identity_before_filter_and_scope() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_duplicate");
    let mut archive = state.run_store.load("run_duplicate").unwrap();
    archive.status = rupu_orchestrator::RunStatus::Failed;
    archive.started_at += chrono::Duration::hours(1); // sorting must not prefer this archived copy
    rupu_orchestrator::RunStore::new(state.run_store.archive_root())
        .create(archive, VALID_YAML)
        .unwrap();
    for text in ["is:archived", "status:failed"] {
        let (_, response) = history_syntax_request(state.clone(), text, "").await;
        assert!(
            response["runs"].as_array().unwrap().is_empty(),
            "{response}"
        );
    }
    let (_, response) = history_syntax_request(state, "id:run_duplicate", "").await;
    assert_eq!(response["runs"].as_array().unwrap().len(), 1);
    assert_eq!(response["runs"][0]["archived"], false);
    assert_eq!(response["runs"][0]["status"], "completed");
}

#[tokio::test]
async fn workflow_history_query_excludes_worker_mirrors_and_foreign_projects_in_both_modes() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_local");
    let mut mirror = run_record("run_mirror", "demo", "ws_a");
    mirror.worker_id = Some("worker_remote".into());
    state.run_store.create(mirror, VALID_YAML).unwrap();
    state
        .run_store
        .create(run_record("run_foreign", "demo", "ws_other"), VALID_YAML)
        .unwrap();
    for suffix in ["", "?query_mode=syntax&q=host%3Alocal"] {
        let (code, response) = session_workflow_request(
            state.clone(),
            "GET",
            &format!("/api/projects/ws_a/workflow-history{suffix}"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(code, axum::http::StatusCode::OK);
        assert_eq!(response["runs"].as_array().unwrap().len(), 1);
        assert_eq!(response["runs"][0]["run_id"], "run_local");
    }
}

#[tokio::test]
async fn workflow_history_query_reports_shared_parser_errors_and_query_limits() {
    let (_tmp, _project, state, _) = session_workflow_fixture(false);
    let (code, response) = history_syntax_request(state.clone(), "é status:bogus", "").await;
    assert_eq!(code, axum::http::StatusCode::BAD_REQUEST);
    let expected =
        crate::query::ListQuery::parse(Some("é status:bogus"), crate::query::workflow_runs::FIELDS)
            .err()
            .unwrap();
    assert_eq!(response["code"], serde_json::json!(expected.code));
    assert_eq!(response["token"], serde_json::json!(expected.token));
    assert_eq!(response["start"], expected.start);
    assert_eq!(response["end"], expected.end);
    for text in ["x".repeat(4097), "demo ".repeat(129)] {
        let (code, _) = history_syntax_request(state.clone(), &text, "").await;
        assert_eq!(code, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }
    for suffix in ["?query_mode=unknown", "?query_mode=syntax&limit=101"] {
        let (code, _) = session_workflow_request(
            state.clone(),
            "GET",
            &format!("/api/projects/ws_a/workflow-history{suffix}"),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(code, axum::http::StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn workflow_history_query_uses_recorded_customer_and_time_trigger_fields() {
    let (_tmp, _project, state, _) = session_workflow_fixture(false);
    let mut record = run_record("run_customer", "demo", "ws_a");
    record.customer = Some(Some("acme".into()));
    record.started_at = "2026-01-15T12:00:00Z".parse().unwrap();
    record.codename = Some("Cobalt Harbor".into());
    state.run_store.create(record, VALID_YAML).unwrap();
    let (_, response) = history_syntax_request(
        state,
        "customer:acme trigger:manual since:2026-01-01 until:2026-02-01 codename:\"cobalt harbor\"",
        "",
    )
    .await;
    assert_eq!(response["runs"][0]["run_id"], "run_customer", "{response}");
}

#[tokio::test]
async fn workflow_history_query_keeps_empty_query_scope_and_readonly_capability() {
    let (_tmp, _project, mut state, _) = session_workflow_fixture(false);
    state.launcher = None;
    let (code, response) = history_syntax_request(state.clone(), "", "").await;
    assert_eq!(code, axum::http::StatusCode::OK);
    assert_eq!(response["q"], "");
    assert_eq!(response["workspace_id"], "ws_a");
    let (_, info) = session_workflow_request(
        state.clone(),
        "GET",
        "/api/host/info",
        serde_json::Value::Null,
    )
    .await;
    assert!(info["features"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("workflow.session_history.query.v1")));
    let (code, _) = session_workflow_request(
        state,
        "GET",
        "/api/projects/missing/workflow-history?query_mode=syntax",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(code, axum::http::StatusCode::NOT_FOUND);
}

fn local_worker_history_proof(state: &AppState, project: &std::path::Path, id: &str, kind: &str) {
    recorded_workflow(state, project, id);
    let mut record = state.run_store.load(id).unwrap();
    record.worker_id = Some("worker_fixture".into());
    record.backend_id = Some("local_worktree".into());
    state.run_store.update(&record).unwrap();
    let workers = state.global_dir.join("autoflows/workers");
    std::fs::create_dir_all(&workers).unwrap();
    let hostname = whoami::fallible::hostname().expect("fixture requires host identity");
    let worker = rupu_runtime::WorkerRecord {
        version: 1, worker_id: "worker_fixture".into(),
        kind: if kind == "cli" { rupu_runtime::WorkerKind::Cli } else { rupu_runtime::WorkerKind::AutoflowServe },
        name: "fixture".into(), host: hostname, capabilities: Default::default(),
        registered_at: "2020-01-01T00:00:00Z".into(), last_seen_at: "2020-01-01T00:00:00Z".into(),
    };
    std::fs::write(workers.join("worker_fixture.toml"), toml::to_string(&worker).unwrap()).unwrap();
    let envelope = serde_json::json!({"version":1,"run_id":id,"kind":"workflow_run",
        "workflow":{"name":"demo","source_path":"/fixture/demo.yaml","fingerprint":"fixture"},
        "repo":{"workspace_id":"ws_a","workspace_path":project},
        "trigger":{"source":"workflow_cli"},"execution":{"backend":"local_worktree","permission_mode":"readonly"},
        "worker":{"assigned_worker_id":"worker_fixture"}});
    std::fs::write(state.run_store.root.join(id).join("run_envelope.json"), envelope.to_string()).unwrap();
}

#[tokio::test]
async fn workflow_history_local_worker_cli_and_autoflow_have_explicit_provenance() {
    for (kind, origin) in [("cli", "local_cli"), ("autoflow_serve", "local_autoflow")] {
        let (_tmp, project, state, _) = session_workflow_fixture(false);
        local_worker_history_proof(&state, project.path(), "run_worker", kind);
        let (code, response) = history_syntax_request(state, "host:local id:run_worker", "").await;
        assert_eq!(code, axum::http::StatusCode::OK);
        assert_eq!(response["runs"][0]["worker_id"], "worker_fixture");
        assert_eq!(response["runs"][0]["execution_origin"], origin);
        assert_eq!(response["ownership"]["complete"], true);
    }
}

#[tokio::test]
async fn workflow_history_local_worker_missing_corrupt_or_imported_proof_is_unavailable() {
    for failure in ["missing_worker", "corrupt_worker", "foreign_host", "missing_envelope", "wrong_project", "wrong_worker", "wrong_backend"] {
        let (_tmp, project, state, _) = session_workflow_fixture(false);
        local_worker_history_proof(&state, project.path(), "run_worker", "cli");
        let worker = state.global_dir.join("autoflows/workers/worker_fixture.toml");
        let envelope = state.run_store.root.join("run_worker/run_envelope.json");
        match failure {
            "missing_worker" => std::fs::remove_file(&worker).unwrap(),
            "corrupt_worker" => std::fs::write(&worker, "invalid\0").unwrap(),
            "foreign_host" => {
                let mut w: rupu_runtime::WorkerRecord = toml::from_str(&std::fs::read_to_string(&worker).unwrap()).unwrap();
                w.host = "different-recorded-host.invalid".into();
                std::fs::write(&worker, toml::to_string(&w).unwrap()).unwrap();
            }
            "missing_envelope" => std::fs::remove_file(&envelope).unwrap(),
            other => {
                let mut e: serde_json::Value = serde_json::from_slice(&std::fs::read(&envelope).unwrap()).unwrap();
                match other {
                    "wrong_project" => e["repo"]["workspace_id"] = "ws_foreign".into(),
                    "wrong_worker" => e["worker"]["assigned_worker_id"] = "worker_foreign".into(),
                    _ => e["execution"]["backend"] = "remote".into(),
                }
                std::fs::write(&envelope, e.to_string()).unwrap();
            }
        }
        let (_, response) = history_syntax_request(state, "", "").await;
        assert_eq!(response["runs"].as_array().unwrap().len(), 0, "{failure}: {response}");
        assert_eq!(response["ownership"]["unavailable"], 1, "{failure}: {response}");
        assert_eq!(response["ownership"]["complete"], false);
    }
}

#[tokio::test]
async fn workflow_history_local_worker_remote_identity_collision_is_excluded() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    local_worker_history_proof(&state, project.path(), "run_worker", "cli");
    let hosts = state.global_dir.join("hosts");
    std::fs::create_dir_all(&hosts).unwrap();
    // Even malformed registered-host evidence prevents local reassignment.
    std::fs::write(hosts.join("worker_fixture.toml"), "malformed").unwrap();
    let (_, response) = history_syntax_request(state, "", "").await;
    assert!(response["runs"].as_array().unwrap().is_empty());
    assert_eq!(response["ownership"]["excluded"], 1);
    assert_eq!(response["ownership"]["unavailable"], 0);
}

#[tokio::test]
async fn workflow_history_local_worker_archive_and_active_precedence_preserved() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    local_worker_history_proof(&state, project.path(), "run_worker", "cli");
    state.run_store.archive("run_worker").unwrap();
    let (_, response) = history_syntax_request(state.clone(), "is:archived", "&limit=1").await;
    assert_eq!(response["runs"][0]["run_id"], "run_worker");
    assert_eq!(response["runs"][0]["archived"], true);
    assert_eq!(response["runs"][0]["execution_origin"], "local_cli");
    // An active record with unknown worker proof must not fall back to the
    // otherwise valid archived record for the same ID.
    let mut active = rupu_orchestrator::RunStore::new(state.run_store.archive_root()).load("run_worker").unwrap();
    active.worker_id = Some("worker_unproven".into());
    state.run_store.create(active, VALID_YAML).unwrap();
    let (_, response) = history_syntax_request(state, "is:archived", "").await;
    assert!(response["runs"].as_array().unwrap().is_empty());
    assert_eq!(response["ownership"]["unavailable"], 1);
}

#[tokio::test]
async fn workflow_history_local_worker_proof_bounds_and_special_files_fail_closed() {
    for failure in ["large_worker", "large_envelope", "symlink_worker"] {
        let (_tmp, project, state, _) = session_workflow_fixture(false);
        local_worker_history_proof(&state, project.path(), "run_worker", "cli");
        let worker = state.global_dir.join("autoflows/workers/worker_fixture.toml");
        match failure {
            "large_worker" => std::fs::write(&worker, vec![b'x'; 65537]).unwrap(),
            "large_envelope" => std::fs::write(state.run_store.root.join("run_worker/run_envelope.json"), vec![b'x'; 262145]).unwrap(),
            _ => {
                #[cfg(unix)] {
                    let target = worker.with_extension("saved");
                    std::fs::rename(&worker, &target).unwrap();
                    std::os::unix::fs::symlink(target, worker).unwrap();
                }
                #[cfg(not(unix))] { continue; }
            }
        }
        let (_, response) = history_syntax_request(state, "", "").await;
        assert!(response["runs"].as_array().unwrap().is_empty(), "{failure}");
        assert_eq!(response["ownership"]["unavailable"], 1);
    }
}

#[tokio::test]
async fn workflow_history_local_worker_prefix_alone_is_not_proof_and_no_worker_is_unchanged() {
    let (_tmp, project, state, _) = session_workflow_fixture(false);
    recorded_workflow(&state, project.path(), "run_stored");
    let mut fake = run_record("run_prefix", "demo", "ws_a");
    fake.worker_id = Some("worker_local_fixture_cli".into());
    state.run_store.create(fake, VALID_YAML).unwrap();
    let (_, response) = history_syntax_request(state.clone(), "", "").await;
    assert_eq!(response["runs"].as_array().unwrap().len(), 1);
    assert_eq!(response["runs"][0]["run_id"], "run_stored");
    assert_eq!(response["runs"][0]["execution_origin"], "local_recorded");
    assert!(response["runs"][0]["worker_id"].is_null());
    assert_eq!(response["ownership"]["unavailable"], 1);
    let (_, info) = session_workflow_request(state, "GET", "/api/host/info", serde_json::Value::Null).await;
    assert!(info["features"].as_array().unwrap().contains(&serde_json::json!("workflow.session_history.local_worker.v1")));
}


#[cfg(unix)]
#[tokio::test]
async fn workflow_history_local_worker_owner_root_alias_keeps_descendants_nofollow() {
    use std::os::unix::fs::symlink;
    let (_tmp, project, mut state, _) = session_workflow_fixture(false);
    local_worker_history_proof(&state, project.path(), "run_worker", "cli");
    let alias_parent = tempfile::tempdir().unwrap();
    let alias = alias_parent.path().join("configured-home");
    symlink(std::fs::canonicalize(&state.global_dir).unwrap(), &alias).unwrap();
    state.global_dir = alias.clone();
    state.run_store = std::sync::Arc::new(rupu_orchestrator::RunStore::new(alias.join("runs")));
    let (_, response) = history_syntax_request(state.clone(), "", "").await;
    assert_eq!(response["runs"][0]["execution_origin"], "local_cli", "{response}");
    state.run_store.archive("run_worker").unwrap();
    let (_, response) = history_syntax_request(state.clone(), "is:archived", "").await;
    assert_eq!(response["runs"][0]["execution_origin"], "local_cli", "{response}");
    let proof = state.run_store.archive_root().join("run_worker/run_envelope.json");
    let saved = proof.with_extension("saved");
    std::fs::rename(&proof, &saved).unwrap();
    symlink(&saved, &proof).unwrap();
    let (_, response) = history_syntax_request(state, "is:archived", "").await;
    assert!(response["runs"].as_array().unwrap().is_empty());
    assert_eq!(response["ownership"]["unavailable"], 1);
}
