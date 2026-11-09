use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use std::sync::{Arc, Barrier};
use tower::ServiceExt;

struct Launcher;
#[async_trait::async_trait]
impl crate::launcher::RunLauncher for Launcher {
    async fn launch(
        &self,
        _: crate::launcher::LaunchRequest,
    ) -> Result<String, crate::launcher::LaunchError> {
        unreachable!()
    }
}
fn fixture() -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("checkout");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(root.path().join("workspaces")).unwrap();
    std::fs::write(
        root.path().join("workspaces/ws_one.toml"),
        format!(
            "id = 'ws_one'\npath = '{}'\ncreated_at = '2026-01-01T00:00:00Z'\n",
            project.display()
        ),
    )
    .unwrap();
    let customers = CustomerStore::new(root.path());
    customers
        .create(
            "acme",
            &rupu_workspace::NewCustomer {
                name: "Acme".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let state = AppState::new(root.path().into(), rupu_config::PricingConfig::default())
        .with_launcher(Some(Arc::new(Launcher)));
    (root, state)
}
fn raw(text: &str) -> ConfigWriteBody {
    ConfigWriteBody {
        raw: Some(text.into()),
        patch: None,
    }
}
async fn request(s: AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = super::super::routes()
        .with_state(s)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
    )
}
#[tokio::test]
async fn scoped_get_is_exact_read_only_and_empty_is_not_missing() {
    let (tmp, s) = fixture();
    let target = tmp.path().join("checkout/.rupu/config.toml");
    let (status, first) = request(
        s.clone(),
        "GET",
        "/api/config/scoped/project/ws_one",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        first["scope"],
        json!({"kind":"project","id":"ws_one","host_id":"local"})
    );
    assert_eq!(first["exists"], false);
    assert!(!target.parent().unwrap().exists());
    assert!(!tmp.path().join("config.lock").exists());
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();
    let (_, second) = request(s, "GET", "/api/config/scoped/project/ws_one", Value::Null).await;
    assert_eq!(second["exists"], true);
    assert_ne!(first["revision"], second["revision"]);
}
#[tokio::test]
async fn required_revision_raw_patch_and_owner_receipt() {
    let (_tmp, s) = fixture();
    let route = "/api/config/scoped/customer/acme";
    let (_, first) = request(s.clone(), "GET", route, Value::Null).await;
    let (missing, _) = request(s.clone(), "PUT", route, json!({"raw":""})).await;
    assert_eq!(missing, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, saved) = request(
        s.clone(),
        "PUT",
        route,
        json!({"expected_revision":first["revision"],"raw":"# keep\ndefault_model = 'one'\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["scope"], first["scope"]);
    let (status, patched) = request(
        s.clone(),
        "PUT",
        route,
        json!({"expected_revision":saved["revision"],"patch":{"default_model":"two"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(patched["raw"].as_str().unwrap().contains("# keep"));
    let (_, readback) = request(s.clone(), "GET", route, Value::Null).await;
    assert_eq!(readback["revision"], patched["revision"]);
    let (stale, _) = request(
        s.clone(),
        "PUT",
        route,
        json!({"expected_revision":first["revision"],"raw":""}),
    )
    .await;
    assert_eq!(stale, StatusCode::CONFLICT);
    let (both, _) = request(
        s,
        "PUT",
        route,
        json!({"expected_revision":patched["revision"],"raw":"","patch":{}}),
    )
    .await;
    assert_eq!(both, StatusCode::BAD_REQUEST);
}
#[test]
fn concurrent_guarded_writers_have_one_winner_and_no_lost_update() {
    let (_tmp, s) = fixture();
    let revision = Capture::read(&s, Kind::Project, "ws_one")
        .unwrap()
        .revision();
    let barrier = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|scope| {
        (0..2)
            .map(|index| {
                let s = s.clone();
                let revision = revision.clone();
                let barrier = barrier.clone();
                scope.spawn(move || {
                    barrier.wait();
                    transact(
                        &s,
                        Kind::Project,
                        "ws_one",
                        &raw(&format!("default_model = 'model{index}'\n")),
                        Some(&revision),
                    )
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| r
                .as_ref()
                .err()
                .is_some_and(|e| e.0 == StatusCode::CONFLICT))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(
        Capture::read(&s, Kind::Project, "ws_one")
            .unwrap()
            .response()["revision"],
        winner["revision"]
    );
}
#[test]
fn concurrent_legacy_patches_merge_under_the_same_lock() {
    let (_tmp, s) = fixture();
    let barrier = Arc::new(Barrier::new(2));
    std::thread::scope(|scope| {
        for patch in [
            json!({"default_model":"chosen"}),
            json!({"autoflow.max_active":3}),
        ] {
            let s = s.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                transact(
                    &s,
                    Kind::Project,
                    "ws_one",
                    &ConfigWriteBody {
                        raw: None,
                        patch: Some(patch),
                    },
                    None,
                )
                .unwrap();
            });
        }
    });
    let text = Capture::read(&s, Kind::Project, "ws_one")
        .unwrap()
        .selected
        .unwrap();
    let parsed: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(parsed["default_model"].as_str(), Some("chosen"));
    assert_eq!(parsed["autoflow"]["max_active"].as_integer(), Some(3));
}
#[test]
fn inherited_global_customer_and_assignment_changes_invalidate_revision() {
    let (tmp, s) = fixture();
    let store = CustomerStore::new(tmp.path());
    std::fs::write(tmp.path().join("workspaces/ws_one.customer"), "acme\n").unwrap();
    let first = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    crate::config_write::write_atomic(
        &tmp.path().join("config.toml"),
        "default_model = 'changed'\n",
    )
    .unwrap();
    assert_eq!(
        transact(
            &s,
            Kind::Project,
            "ws_one",
            &raw(""),
            Some(&first.revision())
        )
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    let second = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    crate::config_write::write_atomic(&store.config_path("acme"), "default_model = 'customer'\n")
        .unwrap();
    assert_eq!(
        transact(
            &s,
            Kind::Project,
            "ws_one",
            &raw(""),
            Some(&second.revision())
        )
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    let third = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    std::fs::remove_file(tmp.path().join("workspaces/ws_one.customer")).unwrap();
    assert_eq!(
        transact(
            &s,
            Kind::Project,
            "ws_one",
            &raw(""),
            Some(&third.revision())
        )
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
}
#[test]
fn revision_is_bound_to_exact_scope_even_for_identical_bytes() {
    let (_tmp, s) = fixture();
    let customer = Capture::read(&s, Kind::Customer, "acme").unwrap();
    assert_eq!(
        transact(
            &s,
            Kind::Project,
            "ws_one",
            &raw(""),
            Some(&customer.revision())
        )
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
}
#[test]
fn current_disk_policy_and_merged_provider_validation_precede_replacement() {
    let (tmp, s) = fixture();
    std::fs::write(
        tmp.path().join("config.toml"),
        "[policy]\nlock = ['default_model']\n",
    )
    .unwrap();
    // AppState predates the disk policy; validation must read the locked file.
    let captured = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    assert_eq!(
        transact(
            &s,
            Kind::Project,
            "ws_one",
            &raw("default_model = 'denied'\n"),
            Some(&captured.revision())
        )
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(!captured.owner.target.exists());
    std::fs::write(tmp.path().join("config.toml"), "[providers.corp]\nkind = 'openai-compatible'\nbase_url = 'https://example.invalid/v1'\ndefault_model = 'model'\n").unwrap();
    let captured = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    assert!(transact(
        &s,
        Kind::Project,
        "ws_one",
        &raw("[providers.corp]\ndefault_model = ''\n"),
        Some(&captured.revision())
    )
    .is_err());
    assert!(!captured.owner.target.exists());
}
#[tokio::test]
async fn legacy_global_wire_is_unchanged_and_guarded_routes_are_local() {
    let (_tmp, s) = fixture();
    let (status, saved) = request(
        s.clone(),
        "PUT",
        "/api/config/global",
        json!({"raw":"default_model = 'global'\n"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["ok"], true);
    assert!(saved.get("revision").is_none());
    assert_eq!(
        request(
            s.clone(),
            "GET",
            "/api/config/scoped/project/ws_one?host=remote",
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_IMPLEMENTED
    );
    assert_eq!(
        request(s, "GET", "/api/config/scoped/project/missing", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
#[test]
fn malformed_selected_layer_can_be_repaired_but_bad_patch_preserves_bytes() {
    let (_tmp, s) = fixture();
    let owner = Owner::resolve(&s, Kind::Project, "ws_one").unwrap();
    std::fs::create_dir_all(owner.target.parent().unwrap()).unwrap();
    std::fs::write(&owner.target, "broken = [").unwrap();
    let captured = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    assert!(transact(
        &s,
        Kind::Project,
        "ws_one",
        &ConfigWriteBody {
            raw: None,
            patch: Some(json!({"default_model":"x"}))
        },
        Some(&captured.revision())
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(&owner.target).unwrap(),
        "broken = ["
    );
    transact(
        &s,
        Kind::Project,
        "ws_one",
        &raw("default_model = 'fixed'\n"),
        Some(&captured.revision()),
    )
    .unwrap();
}

#[test]
fn project_relocation_invalidates_owner_revision() {
    let (tmp, s) = fixture();
    let before = Capture::read(&s, Kind::Project, "ws_one").unwrap();
    let replacement = tmp.path().join("relocated");
    std::fs::create_dir_all(&replacement).unwrap();
    std::fs::write(
        tmp.path().join("workspaces/ws_one.toml"),
        format!(
            "id = 'ws_one'\npath = '{}'\ncreated_at = '2026-01-01T00:00:00Z'\n",
            replacement.display()
        ),
    )
    .unwrap();
    assert_eq!(
        transact(
            &s,
            Kind::Project,
            "ws_one",
            &raw(""),
            Some(&before.revision())
        )
        .unwrap_err()
        .0,
        StatusCode::CONFLICT
    );
    assert!(!before.owner.target.exists());
    assert!(!replacement.join(".rupu/config.toml").exists());
}

#[tokio::test]
async fn unsupported_writes_and_oversized_reads_do_not_replace_configuration() {
    let (tmp, _) = fixture();
    let s = AppState::new(tmp.path().into(), rupu_config::PricingConfig::default());
    let route = "/api/config/scoped/project/ws_one";
    let (_, first) = request(s.clone(), "GET", route, Value::Null).await;
    let (status, _) = request(
        s.clone(),
        "PUT",
        route,
        json!({"expected_revision":first["revision"],"raw":""}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    let target = tmp.path().join("checkout/.rupu/config.toml");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, vec![b'#'; MAX_LAYER_BYTES as usize + 1]).unwrap();
    assert_eq!(
        request(s, "GET", route, Value::Null).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::metadata(target).unwrap().len(),
        MAX_LAYER_BYTES + 1
    );
}
