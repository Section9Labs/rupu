use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::Value;
use tower::ServiceExt;

fn fixture() -> (tempfile::TempDir, AppState, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let global = temp.path().join("global/.rupu");
    let project = temp.path().join("checkout");
    std::fs::create_dir_all(global.join("workspaces")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    let state = AppState::new(global, rupu_config::PricingConfig::default());
    register(&state, &project);
    (temp, state, project)
}
fn register(state: &AppState, path: &Path) {
    std::fs::write(
        state.global_dir.join("workspaces/ws_setup.toml"),
        format!(
            "id = 'ws_setup'\npath = '{}'\ncreated_at = '2026-01-01T00:00:00Z'\n",
            path.display()
        ),
    )
    .unwrap();
}
async fn get(state: AppState, query: &str) -> (StatusCode, Value) {
    let response = routes()
        .with_state(state)
        .oneshot(
            Request::builder()
                .uri(format!("/api/fleet{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let data = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&data).unwrap())
}
#[tokio::test]
async fn missing_layer_has_exact_owner_evidence_without_creating_files() {
    let (_temp, state, project) = fixture();
    let (status, body) = get(state.clone(), "?ws_id=ws_setup&host=local").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "project_layer_missing");
    assert_eq!(body["schema_version"], 1);
    assert_eq!(body["scope"]["workspace_id"], "ws_setup");
    assert_eq!(body["scope"]["host_id"], "local");
    assert_eq!(
        body["scope"]["project_path"],
        project.to_string_lossy().as_ref()
    );
    assert_eq!(body["consistency"], "observed_not_locked");
    assert!(!project.join(".rupu").exists());
    assert!(!state.global_dir.join("config.toml").exists());
}
#[tokio::test]
async fn global_alias_without_config_is_never_reported_missing() {
    let (_temp, state, _) = fixture();
    register(&state, state.global_dir.parent().unwrap());
    let (status, body) = get(state.clone(), "?ws_id=ws_setup").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "project_layer_global_alias");
    assert!(!state.global_dir.join("config.toml").exists());
}
#[cfg(unix)]
#[tokio::test]
async fn symlinked_project_or_layer_alias_is_not_missing() {
    let (temp, state, project) = fixture();
    std::os::unix::fs::symlink(&state.global_dir, project.join(".rupu")).unwrap();
    let (_, body) = get(state.clone(), "?ws_id=ws_setup").await;
    assert_eq!(body["code"], "project_layer_global_alias");
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(state.global_dir.parent().unwrap(), &alias).unwrap();
    register(&state, &alias);
    let (_, body) = get(state, "?ws_id=ws_setup").await;
    assert_eq!(body["code"], "project_layer_global_alias");
}
#[cfg(unix)]
#[tokio::test]
async fn dangling_layer_symlink_is_unavailable_not_missing() {
    let (_temp, state, project) = fixture();
    std::os::unix::fs::symlink(project.join("absent"), project.join(".rupu")).unwrap();
    let (_, body) = get(state, "?ws_id=ws_setup").await;
    assert_eq!(body["code"], "project_layer_unavailable");
}
#[tokio::test]
async fn nondirectory_layer_and_removed_project_are_unavailable() {
    let (_temp, state, project) = fixture();
    std::fs::write(project.join(".rupu"), "not a directory").unwrap();
    let (_, body) = get(state.clone(), "?ws_id=ws_setup").await;
    assert_eq!(body["code"], "project_layer_unavailable");
    std::fs::remove_file(project.join(".rupu")).unwrap();
    std::fs::remove_dir(&project).unwrap();
    let (_, body) = get(state, "?ws_id=ws_setup").await;
    assert_eq!(body["code"], "project_layer_unavailable");
}
#[tokio::test]
async fn existing_project_and_global_only_success_wire_stay_unchanged() {
    let (_temp, state, project) = fixture();
    std::fs::create_dir(project.join(".rupu")).unwrap();
    let (status, body) = get(state.clone(), "?ws_id=ws_setup").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["entries"]
        .as_array()
        .is_some_and(|entries| !entries.is_empty()));
    assert_eq!(
        body["project_root"],
        project.join(".rupu").to_string_lossy().as_ref()
    );
    assert!(body.get("code").is_none());
    let (status, body) = get(state, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["project_root"], Value::Null);
}
#[tokio::test]
async fn unknown_workspace_and_remote_host_keep_existing_status() {
    let (_temp, state, _) = fixture();
    assert_eq!(
        get(state.clone(), "?ws_id=ws_unknown").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(state, "?ws_id=ws_setup&host=peer").await.0,
        StatusCode::NOT_IMPLEMENTED
    );
}
