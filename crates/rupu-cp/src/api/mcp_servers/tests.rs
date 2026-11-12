use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;
struct Launcher;
#[async_trait::async_trait]
impl crate::launcher::RunLauncher for Launcher {
    async fn launch(
        &self,
        _: crate::launcher::LaunchRequest,
    ) -> Result<String, crate::launcher::LaunchError> {
        panic!("registry edits never launch")
    }
}
fn state(tmp: &tempfile::TempDir, writable: bool) -> AppState {
    AppState::new(tmp.path().into(), Default::default()).with_launcher(
        writable.then(|| Arc::new(Launcher) as Arc<dyn crate::launcher::RunLauncher>),
    )
}
async fn request(state: AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = routes()
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
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn edit(revision: &Value) -> Value {
    json!({"revision":revision,"label":"Friendly fixture","url":"https://fixture.invalid/mcp","enabled":true,"credential":{"action":"set_reference","reference":"RUPU_MCP_PRIVATE_FIXTURE_REFERENCE"},"tool_effects":{"search":"read","update":"external"}})
}
#[tokio::test]
async fn missing_registry_is_readable_without_writes_and_readonly_or_wrong_host_refuses_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let s = state(&tmp, false);
    let (status, snapshot) = request(s.clone(), "GET", "/api/mcp/servers", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["servers"], json!([]));
    assert_eq!(snapshot["writable"], false);
    assert!(!tmp.path().join("mcp.toml").exists());
    assert!(!tmp.path().join("mcp.lock").exists());
    assert_eq!(
        request(
            s,
            "PUT",
            "/api/mcp/servers/fixture",
            edit(&snapshot["revision"])
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(
            state(&tmp, true),
            "PUT",
            "/api/mcp/servers/fixture?host=other",
            edit(&snapshot["revision"])
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert!(!tmp.path().join("mcp.toml").exists());
}
#[tokio::test]
async fn canonical_receipts_preserve_clear_and_replace_references_without_returning_them() {
    let tmp = tempfile::tempdir().unwrap();
    let s = state(&tmp, true);
    let (_, initial) = request(s.clone(), "GET", "/api/mcp/servers", Value::Null).await;
    let (status, created) = request(
        s.clone(),
        "PUT",
        "/api/mcp/servers/docs",
        edit(&initial["revision"]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["scope"], "trusted_host");
    assert_eq!(created["host_id"], "local");
    assert_eq!(
        created["mutation"],
        json!({"action":"upsert","server_id":"docs"})
    );
    assert_eq!(
        created["servers"][0]["credential"],
        json!({"kind":"bearer_env","configured":true})
    );
    assert!(!created
        .to_string()
        .contains("RUPU_MCP_PRIVATE_FIXTURE_REFERENCE"));
    assert_eq!(
        Registry::load(tmp.path()).unwrap().servers["docs"]
            .bearer_env
            .as_deref(),
        Some("RUPU_MCP_PRIVATE_FIXTURE_REFERENCE")
    );
    let (_, readback) = request(s.clone(), "GET", "/api/mcp/servers", Value::Null).await;
    assert_eq!(readback["revision"], created["revision"]);
    assert_eq!(readback["servers"], created["servers"]);
    let mut preserved = edit(&created["revision"]);
    preserved["credential"] = json!({"action":"preserve"});
    preserved["enabled"] = json!(false);
    let (status, saved) = request(s.clone(), "PUT", "/api/mcp/servers/docs", preserved).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        Registry::load(tmp.path()).unwrap().servers["docs"]
            .bearer_env
            .as_deref(),
        Some("RUPU_MCP_PRIVATE_FIXTURE_REFERENCE")
    );
    let mut clear = edit(&saved["revision"]);
    clear["credential"] = json!({"action":"clear"});
    let (status, cleared) = request(s.clone(), "PUT", "/api/mcp/servers/docs", clear).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["servers"][0]["credential"]["configured"], false);
    assert!(Registry::load(tmp.path()).unwrap().servers["docs"]
        .bearer_env
        .is_none());
    let (status, deleted) = request(
        s,
        "DELETE",
        "/api/mcp/servers/docs",
        json!({"revision":cleared["revision"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["servers"], json!([]));
    assert_eq!(
        deleted["mutation"],
        json!({"action":"delete","server_id":"docs"})
    );
    assert_eq!(
        store::read(tmp.path()).unwrap().revision,
        deleted["revision"].as_str().unwrap()
    );
}
#[tokio::test]
async fn stale_cas_and_unknown_or_risky_fields_leave_registry_bytes_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let s = state(&tmp, true);
    let (_, initial) = request(s.clone(), "GET", "/api/mcp/servers", Value::Null).await;
    let (_, saved) = request(
        s.clone(),
        "PUT",
        "/api/mcp/servers/docs",
        edit(&initial["revision"]),
    )
    .await;
    let path = tmp.path().join("mcp.toml");
    let original = std::fs::read(&path).unwrap();
    assert_eq!(
        request(
            s.clone(),
            "PUT",
            "/api/mcp/servers/other",
            edit(&initial["revision"])
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            s.clone(),
            "DELETE",
            "/api/mcp/servers/docs",
            json!({"revision":initial["revision"]})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    for (field, value) in [
        ("token", json!("DO_NOT_ECHO_SECRET")),
        ("auth_file", json!("/private/credential")),
        ("headers", json!({"Authorization":"DO_NOT_ECHO_SECRET"})),
        ("command", json!("sh")),
    ] {
        let mut bad = edit(&saved["revision"]);
        bad[field] = value;
        let (status, error) = request(s.clone(), "PUT", "/api/mcp/servers/docs", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!error.to_string().contains("DO_NOT_ECHO_SECRET"));
    }
    let mut oversized = edit(&saved["revision"]);
    oversized["label"] = json!("x".repeat(256 * 1024));
    assert_eq!(
        request(s.clone(), "PUT", "/api/mcp/servers/docs", oversized)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    for credential in [
        json!({"action":"DO_NOT_ECHO_SECRET"}),
        json!({"action":"set_reference","reference":"OPENAI_API_KEY"}),
        json!({"action":"set_reference","reference":"token secret"}),
    ] {
        let mut bad = edit(&saved["revision"]);
        bad["credential"] = credential;
        let (status, error) = request(s.clone(), "PUT", "/api/mcp/servers/docs", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!error.to_string().contains("DO_NOT_ECHO_SECRET"));
    }
    let mut bad = edit(&saved["revision"]);
    bad["url"] = json!("https://user:DO_NOT_ECHO_SECRET@example.invalid/mcp");
    let (status, error) = request(s.clone(), "PUT", "/api/mcp/servers/docs", bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!error.to_string().contains("DO_NOT_ECHO_SECRET"));
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert_eq!(
        request(
            s,
            "DELETE",
            "/api/mcp/servers/missing",
            json!({"revision":saved["revision"]})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn unknown_existing_fields_are_refused_without_exposure_or_silent_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let s = state(&tmp, true);
    let (_, initial) = request(s.clone(), "GET", "/api/mcp/servers", Value::Null).await;
    let text = "version=1\nunknown='DO_NOT_ECHO_SECRET'\n";
    std::fs::write(tmp.path().join("mcp.toml"), text).unwrap();
    for method in ["GET", "PUT"] {
        let path = if method == "GET" {
            "/api/mcp/servers"
        } else {
            "/api/mcp/servers/docs"
        };
        let (status, error) = request(s.clone(), method, path, edit(&initial["revision"])).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!error.to_string().contains("DO_NOT_ECHO_SECRET"));
    }
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("mcp.toml")).unwrap(),
        text
    );
}
#[test]
fn concurrent_writers_have_one_cas_winner_and_preserve_the_winner() {
    let tmp = tempfile::tempdir().unwrap();
    let initial = store::read(tmp.path()).unwrap().revision;
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let start = |id: &'static str| {
            let root = tmp.path();
            let revision = &initial;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                store::mutate(root, revision, |registry| {
                    registry.servers.insert(
                        id.into(),
                        ServerConfig {
                            label: id.into(),
                            url: "https://fixture.invalid/mcp".into(),
                            enabled: false,
                            bearer_env: None,
                            bearer_store: None,
                            tool_effects: Default::default(),
                        },
                    );
                    Ok(())
                })
                .is_ok()
            })
        };
        let a = start("first");
        let b = start("second");
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|v| **v).count(), 1);
    assert_eq!(Registry::load(tmp.path()).unwrap().servers.len(), 1);
}
#[cfg(unix)]
#[tokio::test]
async fn symlink_registry_is_refused_without_reading_or_writing_target() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("not_a_registry");
    std::fs::write(&outside, "DO_NOT_ECHO_SECRET").unwrap();
    std::os::unix::fs::symlink(&outside, tmp.path().join("mcp.toml")).unwrap();
    let (status, error) = request(state(&tmp, true), "GET", "/api/mcp/servers", Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(!error.to_string().contains("DO_NOT_ECHO_SECRET"));
    assert_eq!(
        std::fs::read_to_string(outside).unwrap(),
        "DO_NOT_ECHO_SECRET"
    );
}

#[tokio::test]
async fn host_store_binding_rotates_only_with_endpoint_or_mechanism_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let s = state(&tmp, true);
    let (_, mut snapshot) = request(s.clone(), "GET", "/api/mcp/servers", Value::Null).await;
    let mut generations = Vec::new();
    for (action, url, changes) in [
        ("use_host_store", "https://fixture.invalid/mcp", true),
        ("preserve", "https://FIXTURE.invalid:443/mcp", false),
        ("use_host_store", "https://fixture.invalid/mcp", false),
        ("preserve", "https://fixture.invalid/other", true),
        ("clear", "https://fixture.invalid/other", true),
        ("use_host_store", "https://fixture.invalid/other", true),
        ("set_reference", "https://fixture.invalid/other", true),
        ("use_host_store", "https://fixture.invalid/other", true),
    ] {
        let mut body = edit(&snapshot["revision"]);
        body["url"] = json!(url);
        body["credential"] = if action == "set_reference" {
            json!({"action":action,"reference":"RUPU_MCP_FIXTURE_REFERENCE"})
        } else {
            json!({"action":action})
        };
        let (status, saved) = request(s.clone(), "PUT", "/api/mcp/servers/fixture", body).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        let config = Registry::load(tmp.path()).unwrap();
        let generation = config.servers["fixture"].bearer_store.clone();
        if let Some(previous) = generations.last() {
            assert_eq!(generation != *previous, changes, "{action}: {url}");
        }
        if let Some(generation) = &generation {
            assert!(!saved.to_string().contains(generation));
            assert_eq!(
                saved["servers"][0]["credential"],
                json!({"kind":"host_store","configured":true})
            );
            assert!(config.servers["fixture"].bearer_env.is_none());
        }
        generations.push(generation);
        snapshot = saved;
    }
    let original = generations.last().unwrap().clone().unwrap();
    let (status, removed) = request(
        s.clone(),
        "DELETE",
        "/api/mcp/servers/fixture",
        json!({"revision":snapshot["revision"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut body = edit(&removed["revision"]);
    body["credential"] = json!({"action":"use_host_store"});
    let (status, _) = request(s, "PUT", "/api/mcp/servers/fixture", body).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        Registry::load(tmp.path()).unwrap().servers["fixture"]
            .bearer_store
            .as_deref(),
        Some(original.as_str())
    );
    assert!(!tmp.path().join("mcp-credentials.json").exists());
}

#[test]
fn credential_transaction_holds_registry_snapshot_lock_without_rewriting() {
    let tmp = tempfile::tempdir().unwrap();
    let revision = store::read(tmp.path()).unwrap().revision;
    store::with_snapshot_locked(tmp.path(), &revision, |snapshot| {
        assert_eq!(snapshot.revision, revision);
        assert!(
            store::mutate(tmp.path(), &revision, |_| panic!("lock must prevent edit")).is_err()
        );
        Ok(())
    })
    .unwrap();
    assert!(!tmp.path().join("mcp.toml").exists());
    assert!(
        store::with_snapshot_locked::<()>(tmp.path(), "stale", |_| panic!(
            "CAS must prevent credential write"
        ))
        .is_err()
    );
    assert!(store::mutate(tmp.path(), &revision, |_| Ok(())).is_ok());
}
