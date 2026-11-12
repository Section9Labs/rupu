use super::*;

const VALID: &str = "name: demo\nsteps:\n  - id: one\n    agent: missing-agent\n    prompt: hi\n";
fn request(raw: &str) -> Request {
    Request {
        schema_version: 1,
        nonce: "draft_1-Nonce".into(),
        raw: raw.into(),
    }
}
fn value(raw: &str) -> serde_json::Value {
    serde_json::from_slice(&project(request(raw)).unwrap()).unwrap()
}

#[test]
fn complete_projection_matches_owning_serializer_and_exact_utf8_identity() {
    let raw = VALID.replace("hi\n", "'héllo 🌱'\r\n");
    let raw = raw.trim_end_matches(['\r', '\n']);
    let result = value(raw);
    assert_eq!(result.as_object().unwrap().len(), 8);
    assert_eq!(result["schema"], FEATURE);
    assert_eq!(result["schema_version"], 1);
    assert_eq!(result["parser_contract"], "rupu.workflow.v1");
    assert_eq!(result["nonce"], "draft_1-Nonce");
    assert_eq!(result["source_bytes"], raw.len());
    assert_eq!(
        result["source_sha256"],
        format!("{:x}", Sha256::digest(raw.as_bytes()))
    );
    assert_eq!(
        result["workflow"],
        serde_json::to_value(Workflow::parse(raw).unwrap()).unwrap()
    );
    assert_ne!(
        result["source_sha256"],
        value(&format!("{raw}\n"))["source_sha256"]
    );
}

#[test]
fn integer_default_above_double_precision_is_not_rounded() {
    let raw =
        format!("inputs:\n  count:\n    type: int\n    default: 18446744073709551615\n{VALID}");
    let bytes = project(request(&raw)).unwrap();
    assert!(std::str::from_utf8(&bytes)
        .unwrap()
        .contains("18446744073709551615"));
    let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        result["workflow"]["inputs"]["count"]["default"].as_u64(),
        Some(u64::MAX)
    );
}

#[test]
fn thousand_step_workflow_is_complete_and_never_a_page() {
    let mut raw = String::from("name: many\nsteps:\n");
    for index in 0..1025 {
        raw.push_str(&format!(
            "  - id: s{index}\n    agent: absent\n    prompt: hi\n"
        ));
    }
    let result = value(&raw);
    let steps = result["workflow"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 1025);
    assert_eq!(steps[1024]["id"], "s1024");
}

#[test]
fn schema_nonce_and_unknown_request_fields_fail_closed() {
    for nonce in ["", "bad space", "é", "slash/value"] {
        let mut req = request(VALID);
        req.nonce = nonce.into();
        assert_eq!(project(req).unwrap_err(), Failure::Invalid);
    }
    let mut req = request(VALID);
    req.nonce = "x".repeat(129);
    assert_eq!(project(req).unwrap_err(), Failure::Invalid);
    let mut req = request(VALID);
    req.schema_version = 2;
    assert_eq!(project(req).unwrap_err(), Failure::Invalid);
    assert!(serde_json::from_value::<Request>(serde_json::json!({
        "schema_version":1,"nonce":"one","raw":VALID,"workspace":"arbitrary"
    }))
    .is_err());
}

#[test]
fn malformed_and_invalid_workflows_do_not_project() {
    for raw in [
        "[",
        "name: empty\nsteps: []",
        "name: multi\n---\nname: second",
    ] {
        assert_eq!(project(request(raw)).unwrap_err(), Failure::Invalid);
    }
}

#[test]
fn unsupported_yaml_values_are_distinct_from_invalid_source() {
    for raw in [
        "x: .nan",
        "x: .inf",
        "x: !custom tagged",
        "42: value",
        "? [a, b]\n: value",
    ] {
        assert_eq!(check_yaml(raw), Err(Failure::Unsupported), "{raw}");
    }
}

#[test]
fn source_depth_collection_and_expanded_alias_budgets_are_finite() {
    assert_eq!(
        project(request(&" ".repeat(MAX_SOURCE + 1))).unwrap_err(),
        Failure::Bound
    );
    let nested = format!(
        "{}0{}",
        "[".repeat(MAX_DEPTH + 2),
        "]".repeat(MAX_DEPTH + 2)
    );
    assert_eq!(check_yaml(&nested), Err(Failure::Bound));
    let collection = format!("[{}]", vec!["0"; MAX_COLLECTION + 1].join(","));
    assert_eq!(check_yaml(&collection), Err(Failure::Bound));
    // Small input with expanded scalar work over 1 MiB.
    let alias = format!(
        "x: &x '{}'\ny: [{}]\n",
        "a".repeat(1024),
        vec!["*x"; 1024].join(",")
    );
    assert_eq!(check_yaml(&alias), Err(Failure::Bound));
    // Each individual collection fits; total expanded node work does not.
    let rows = format!(
        "[{}]",
        vec![format!("[{}]", vec!["0"; 1024].join(",")); 33].join(",")
    );
    assert_eq!(check_yaml(&rows), Err(Failure::Bound));
}

#[test]
fn response_sink_never_grows_past_its_limit() {
    let mut sink = BoundedBytes(vec![0; MAX_RESPONSE - 1]);
    sink.write_all(b"a").unwrap();
    assert!(sink.write_all(b"b").is_err());
    assert_eq!(sink.0.len(), MAX_RESPONSE);
}

#[tokio::test]
async fn errors_never_include_untrusted_source_or_parser_diagnostics() {
    let raw = "name: private-fixture-marker\nsteps: []";
    let response = project(request(raw)).unwrap_err().into_response();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    assert!(body.contains("workflow_invalid"));
    assert!(!body.contains("private-fixture-marker"));
    for (failure, status) in [
        (Failure::Bound, 413),
        (Failure::Unsupported, 422),
        (Failure::Busy, 429),
    ] {
        assert_eq!(failure.into_response().status().as_u16(), status);
    }
}

#[tokio::test]
async fn cancelled_waiter_retains_admission_until_actual_parser_work_ends() {
    // Local leaked semaphore isolates this fixture from parallel route tests.
    let parsers: &'static Semaphore = Box::leak(Box::new(Semaphore::new(1)));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let task = tokio::spawn(respond(parsers, move || {
        started_tx.send(()).unwrap();
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        Ok(b"{}".to_vec())
    }));
    started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        respond(parsers, || panic!("busy must not dispatch"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    release_tx.send(()).unwrap();
    let permit = tokio::time::timeout(std::time::Duration::from_secs(5), parsers.acquire())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert_eq!(
        respond(parsers, || project(request(VALID))).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn http_projection_and_capability_need_no_execution_adapter_or_store() {
    use axum::{body::Body, http::Request as HttpRequest};
    use tower::ServiceExt;
    let temporary = tempfile::TempDir::new().unwrap();
    let absent = temporary.path().join("no-store");
    let state = crate::state::AppState::new(absent.clone(), rupu_config::PricingConfig::default());
    let router = crate::api::workflows::routes()
        .merge(crate::api::host_info::routes())
        .with_state(state);
    let response = router
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/workflows/validate/projection")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"schema_version":1,"nonce":"http","raw":VALID}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), MAX_RESPONSE)
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(result["workflow"]["steps"][0]["agent"], "missing-agent");
    let response = router
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/api/host/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), MAX_RESPONSE)
        .await
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(result["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == FEATURE));
    // The original validation contract stays {ok:true}, not this new DTO.
    let response = router
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/workflows/validate")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({"raw":VALID}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), MAX_RESPONSE)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({"ok":true})
    );
    assert!(
        !absent.exists(),
        "stateless parse must not materialize any store"
    );
}

#[tokio::test]
async fn http_rejects_large_or_unknown_envelopes_without_echoing_values() {
    use axum::{body::Body, http::Request as HttpRequest, routing::post, Router};
    use tower::ServiceExt;
    let router = Router::new().route(
        "/api/workflows/validate/projection",
        post(validate).layer(axum::extract::DefaultBodyLimit::max(MAX_REQUEST)),
    );
    for (body, status) in [
        (serde_json::json!({"schema_version":1,"nonce":"http","raw":VALID,"private-marker":"private-value"}).to_string(), 400),
        (format!("{{\"raw\":\"{}\"}}", "x".repeat(MAX_REQUEST)), 413),
    ] {
        let response = router.clone().oneshot(HttpRequest::builder().method("POST").uri("/api/workflows/validate/projection")
            .header("content-type", "application/json").body(Body::from(body)).unwrap()).await.unwrap();
        assert_eq!(response.status().as_u16(), status);
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(!std::str::from_utf8(&body).unwrap().contains("private-"));
    }
}

#[test]
fn standard_yaml_scalar_tags_keep_owning_parser_semantics() {
    assert_eq!(
        check_yaml("x: !!str text\ny: !!int 42\nz: !!bool true\n"),
        Ok(())
    );
    let raw = VALID.replace("prompt: hi", "prompt: !!str hi");
    assert_eq!(value(&raw)["workflow"], value(VALID)["workflow"]);
}
