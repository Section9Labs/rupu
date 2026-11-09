use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;
struct Starter;
#[async_trait::async_trait]
impl crate::session_starter::SessionStarter for Starter {
    fn supports_builtin_harness(&self) -> bool {
        true
    }
    fn supports_session_mcp(&self) -> bool {
        true
    }
    async fn start(
        &self,
        _: crate::session_starter::SessionStartRequest,
    ) -> Result<String, crate::session_starter::SessionStartError> {
        panic!("metadata/settings must not launch")
    }
}
struct Launcher;
#[async_trait::async_trait]
impl crate::launcher::RunLauncher for Launcher {
    async fn launch(
        &self,
        _: crate::launcher::LaunchRequest,
    ) -> Result<String, crate::launcher::LaunchError> {
        panic!("metadata/settings must not launch")
    }
}
fn setup() -> (tempfile::TempDir, AppState, Vec<u8>) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("sessions/ses_fixture");
    std::fs::create_dir_all(&dir).unwrap();
    let record = json!({"version":5,"session_id":"ses_fixture","agent_name":"rupu-session","provider_name":"fixture","model":"fixture","agent_system_prompt":"private fixture prompt","agent_tools":["read_file"],"max_turns":3,"permission_mode":"readonly","no_stream":true,"workspace_id":"ws_fixture","workspace_path":"/fixture","transcripts_dir":"/fixture/transcripts","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","status":"running","mcp_tools":{"tools":[{"identity":{"server":"docs","tool":"search"},"fingerprint":format!("sha256:{}","a".repeat(64))}]}});
    let bytes = serde_json::to_vec(&record).unwrap();
    std::fs::write(dir.join("session.json"), &bytes).unwrap();
    let state = AppState::new(tmp.path().into(), Default::default())
        .with_launcher(Some(Arc::new(Launcher)))
        .with_session_starter(Some(Arc::new(Starter)));
    (tmp, state, bytes)
}
async fn request(state: AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = super::super::sessions::routes()
        .with_state(state)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"error":String::from_utf8_lossy(&bytes)})),
    )
}

#[tokio::test]
async fn next_turn_edits_share_cas_preserve_mcp_on_builtin_put_and_reset_only_to_creation() {
    let (tmp, state, original) = setup();
    std::fs::write(
        tmp.path().join("mcp.toml"),
        "broken registry must not affect empty/reset",
    )
    .unwrap();
    let path = "/api/sessions/ses_fixture/mcp?host=local";
    let (status, initial) = request(state.clone(), "GET", path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["workspace_id"], "ws_fixture");
    assert_eq!(initial["tools"], Value::Null);
    assert_eq!(
        initial["effective"],
        json!([{"server":"docs","tool":"search"}])
    );
    let (status, empty) =
        request(state.clone(), "PUT", path, json!({"revision":0,"tools":[]})).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["revision"], 1);
    assert_eq!(empty["effective"], json!([]));
    let (status, builtin) = request(
        state.clone(),
        "PUT",
        "/api/sessions/ses_fixture/settings",
        json!({"revision":1,"tools":[],"provider":null,"model":null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{builtin}");
    assert_eq!(builtin["settings"]["revision"], 2);
    assert_eq!(builtin["settings"]["mcp"], json!({"tools":[]}));
    assert_eq!(
        request(
            state.clone(),
            "PUT",
            path,
            json!({"revision":1,"tools":null})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (status, reset) = request(state, "PUT", path, json!({"revision":2,"tools":null})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reset["effective"], initial["effective"]);
    assert_eq!(reset["revision"], 3);
    assert_eq!(
        std::fs::read(tmp.path().join("sessions/ses_fixture/session.json")).unwrap(),
        original
    );
}

#[tokio::test]
async fn old_sessions_keep_empty_defaults_and_missing_tools_field_cannot_reset() {
    let (tmp, state, _) = setup();
    let path = tmp.path().join("sessions/ses_fixture/session.json");
    let mut record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record.as_object_mut().unwrap().remove("mcp_tools");
    record["version"] = json!(4);
    std::fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    let endpoint = "/api/sessions/ses_fixture/mcp";
    let (status, result) = request(state.clone(), "GET", endpoint, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["effective"], json!([]));
    assert_eq!(
        request(state.clone(), "PUT", endpoint, json!({"revision":0}))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            state,
            "PUT",
            endpoint,
            json!({"revision":0,"tools":[],"endpoint":"https://invented"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(!tmp
        .path()
        .join("sessions/ses_fixture/settings.json")
        .exists());
}

#[tokio::test]
async fn wrong_host_archived_missing_and_unsupported_starter_refuse_without_mutation() {
    let (tmp, state, _) = setup();
    assert_eq!(
        request(
            state.clone(),
            "GET",
            "/api/mcp/tools?host=other",
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(
            state.clone(),
            "GET",
            "/api/sessions/ses_missing/mcp",
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            state.clone().with_session_starter(None),
            "GET",
            "/api/mcp/tools",
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(
            state.clone().with_launcher(None),
            "PUT",
            "/api/sessions/ses_fixture/mcp",
            json!({"revision":0,"tools":[]})
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    std::fs::create_dir_all(tmp.path().join("sessions-archive")).unwrap();
    std::fs::rename(
        tmp.path().join("sessions/ses_fixture"),
        tmp.path().join("sessions-archive/ses_fixture"),
    )
    .unwrap();
    assert_eq!(
        request(
            state,
            "PUT",
            "/api/sessions/ses_fixture/mcp",
            json!({"revision":0,"tools":[]})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn live_fixture_catalog_is_friendly_and_explicit_edit_freezes_exact_metadata() {
    use httpmock::{Method::POST, MockServer};
    let server = MockServer::start_async().await;
    for (method, id, result) in [
        (
            "initialize",
            1,
            json!({"protocolVersion":rupu_mcp_client::PROTOCOL_VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}),
        ),
        (
            "tools/list",
            2,
            json!({"tools":[{"name":"query","description":"Find records","inputSchema":{"type":"object"}}]}),
        ),
    ] {
        server.mock(|when, then| {
            when.method(POST)
                .json_body_partial(json!({"method":method}).to_string());
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"jsonrpc":"2.0","id":id,"result":result}));
        });
    }
    server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"notifications/initialized"}"#);
        then.status(202);
    });
    let calls = server.mock(|when, then| {
        when.method(POST)
            .json_body_partial(r#"{"method":"tools/call"}"#);
        then.status(500);
    });
    let (tmp, state, _) = setup();
    std::fs::write(
        tmp.path().join("mcp.toml"),
        format!(
            "version=1\n[servers.docs]\nlabel='Friendly docs'\nurl='{}'\nenabled=true",
            server.url("/mcp")
        ),
    )
    .unwrap();
    let (status, catalog) = request(
        state.clone(),
        "GET",
        "/api/mcp/tools?host=local",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{catalog}");
    assert_eq!(catalog["tools"][0]["server_label"], "Friendly docs");
    assert_eq!(catalog["tools"][0]["tool"], "query");
    assert_eq!(catalog["tools"][0]["effect"], "external");
    let body =
        json!({"revision":0,"tools":[{"server":"docs","tool":"query"}],"catalog_revision":"stale"});
    assert_eq!(
        request(
            state.clone(),
            "PUT",
            "/api/sessions/ses_fixture/mcp",
            body.clone()
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert!(!tmp
        .path()
        .join("sessions/ses_fixture/settings.json")
        .exists());
    let mut body = body;
    body["catalog_revision"] = catalog["revision"].clone();
    let (status, saved) = request(state, "PUT", "/api/sessions/ses_fixture/mcp", body).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["effective"],
        json!([{"server":"docs","tool":"query"}])
    );
    let settings = SessionSettings::read(&tmp.path().join("sessions/ses_fixture")).unwrap();
    assert_eq!(
        settings.mcp.unwrap().tools[0].fingerprint,
        catalog["tools"][0]["fingerprint"].as_str().unwrap()
    );
    calls.assert_hits(0);
}
