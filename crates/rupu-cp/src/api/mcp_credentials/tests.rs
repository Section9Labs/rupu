use super::*;
use axum::{body::Body, http::Request};
use std::sync::Arc;
use tower::ServiceExt;
const GEN: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OP: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const NEXT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
struct Launcher;
#[async_trait::async_trait]
impl crate::launcher::RunLauncher for Launcher {
    async fn launch(
        &self,
        _: crate::launcher::LaunchRequest,
    ) -> Result<String, crate::launcher::LaunchError> {
        panic!("credential setup must never launch")
    }
}
fn state(tmp: &tempfile::TempDir, writable: bool) -> AppState {
    AppState::new(tmp.path().into(), Default::default())
        .with_launcher(
            writable.then(|| Arc::new(Launcher) as Arc<dyn crate::launcher::RunLauncher>),
        )
        .with_token_set(true)
}
fn registry(tmp: &tempfile::TempDir, generation: &str) -> String {
    std::fs::write(tmp.path().join("mcp.toml"), format!("version=1\n[servers.fixture]\nlabel='Fixture'\nurl='https://fixture.invalid/mcp'\nenabled=false\nbearer_store='{generation}'\n")).unwrap();
    store::read(tmp.path()).unwrap().revision
}
async fn request(
    state: AppState,
    method: &str,
    path: &str,
    value: Value,
    middleware: bool,
    bearer: bool,
) -> (StatusCode, Value) {
    let app = routes().with_state(state);
    let app = if middleware {
        app.layer(axum::middleware::from_fn_with_state(
            crate::auth::Token::new("fixture-cp-token".into()),
            crate::auth::require_token,
        ))
    } else {
        app
    };
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if bearer {
        builder = builder.header("authorization", "Bearer fixture-cp-token");
    }
    let response = app
        .oneshot(
            builder
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(value.to_string())
                })
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
fn enroll_body(revision: &str) -> Value {
    json!({"operation_id":OP,"registry_revision":revision,"binding_generation":GEN,"credential_revision":"absent","token":"fixture-mcp-token-never-return"})
}
#[tokio::test]
async fn actual_verified_auth_is_required_not_flags_or_supplied_header_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let revision = registry(&tmp, GEN);
    for (middleware, bearer) in [(false, false), (false, true), (true, false)] {
        let (status, _) = request(
            state(&tmp, true),
            "PUT",
            "/api/mcp/servers/fixture/credential?host=local",
            enroll_body(&revision),
            middleware,
            bearer,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!tmp.path().join("mcp-credentials.json").exists());
    }
}
#[tokio::test]
async fn local_and_writable_gates_precede_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let revision = registry(&tmp, GEN);
    for (writable, host) in [(false, "local"), (true, "remote-fixture")] {
        let (status, _) = request(
            state(&tmp, writable),
            "PUT",
            &format!("/api/mcp/servers/fixture/credential?host={host}"),
            enroll_body(&revision),
            true,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    }
    assert!(!tmp.path().join("mcp-credentials.json").exists());
}
#[tokio::test]
async fn enrollment_status_and_lost_response_receipt_never_return_token() {
    let tmp = tempfile::tempdir().unwrap();
    let revision = registry(&tmp, GEN);
    let (status, accepted) = request(
        state(&tmp, true),
        "PUT",
        "/api/mcp/servers/fixture/credential",
        enroll_body(&revision),
        true,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["receipt"]["operation_id"], OP);
    assert_eq!(accepted["receipt"]["action"], "enroll");
    // Simulate lost response: a fresh client only inspects the durable op ID.
    let (status, recovered) = request(
        state(&tmp, true),
        "GET",
        &format!("/api/mcp/credential-operations/{OP}"),
        Value::Null,
        true,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered, accepted);
    let (_, metadata) = request(
        state(&tmp, true),
        "GET",
        "/api/mcp/servers/fixture/credential",
        Value::Null,
        true,
        true,
    )
    .await;
    assert_eq!(metadata["credential"]["present"], true);
    assert!(!accepted
        .to_string()
        .contains("fixture-mcp-token-never-return"));
    assert!(!metadata
        .to_string()
        .contains("fixture-mcp-token-never-return"));
    let (status, replay) = request(
        state(&tmp, true),
        "PUT",
        "/api/mcp/servers/fixture/credential",
        enroll_body(&revision),
        true,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(replay["code"], "operation_id_used");
    assert_eq!(replay["mutation_outcome"], "unknown");
}
#[tokio::test]
async fn stale_registry_or_binding_is_definite_no_write() {
    let tmp = tempfile::tempdir().unwrap();
    let revision = registry(&tmp, GEN);
    for field in ["registry_revision", "binding_generation"] {
        let mut body = enroll_body(&revision);
        body[field] = json!("stale");
        let (status, error) = request(
            state(&tmp, true),
            "PUT",
            "/api/mcp/servers/fixture/credential",
            body,
            true,
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["code"], "precondition_failed");
        assert_eq!(error["mutation_outcome"], "not_applied");
    }
    assert!(!tmp.path().join("mcp-credentials.json").exists());
}
#[tokio::test]
async fn recreation_does_not_inherit_old_token_and_orphan_remove_is_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let revision = registry(&tmp, GEN);
    let (_, accepted) = request(
        state(&tmp, true),
        "PUT",
        "/api/mcp/servers/fixture/credential",
        enroll_body(&revision),
        true,
        true,
    )
    .await;
    let next_revision = registry(&tmp, NEXT);
    let (_, metadata) = request(
        state(&tmp, true),
        "GET",
        "/api/mcp/servers/fixture/credential",
        Value::Null,
        true,
        true,
    )
    .await;
    assert_eq!(metadata["credential"]["present"], false);
    assert_eq!(metadata["credential"]["revision"], "absent");
    let (_, orphans) = request(
        state(&tmp, true),
        "GET",
        "/api/mcp/credential-orphans?limit=1&offset=0",
        Value::Null,
        true,
        true,
    )
    .await;
    assert_eq!(orphans["orphans"].as_array().unwrap().len(), 1);
    assert_eq!(orphans["orphans"][0]["binding"]["generation"], GEN);
    let body = json!({"operation_id":NEXT,"registry_revision":next_revision,"binding":accepted["receipt"]["binding"],"credential_revision":accepted["receipt"]["credential_revision"]});
    let (status, removed) = request(
        state(&tmp, true),
        "DELETE",
        &format!("/api/mcp/credential-orphans/{GEN}"),
        body,
        true,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed["receipt"]["present"], false);
    let (_, orphans) = request(
        state(&tmp, true),
        "GET",
        "/api/mcp/credential-orphans",
        Value::Null,
        true,
        true,
    )
    .await;
    assert_eq!(orphans["orphans"].as_array().unwrap().len(), 0);
}
#[tokio::test]
async fn invalid_body_errors_do_not_echo_token_and_unknown_operation_is_not_success() {
    let tmp = tempfile::tempdir().unwrap();
    registry(&tmp, GEN);
    let (status, error) = request(
        state(&tmp, true),
        "PUT",
        "/api/mcp/servers/fixture/credential",
        json!({"token":"fixture-secret-sentinel","unexpected":true}),
        true,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!error.to_string().contains("fixture-secret-sentinel"));
    let (status, _) = request(
        state(&tmp, true),
        "GET",
        &format!("/api/mcp/credential-operations/{OP}"),
        Value::Null,
        true,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
