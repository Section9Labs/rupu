use assert_fs::prelude::*;
use rupu_workspace::{upsert, WorkspaceStore};

#[test]
fn first_upsert_creates_record_with_new_id() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore {
        root: store_dir.path().to_path_buf(),
    };

    let ws = upsert(&store, project.path()).unwrap();
    assert!(ws.id.starts_with("ws_"));
    assert_eq!(
        std::path::Path::new(&ws.path).canonicalize().unwrap(),
        project.path().canonicalize().unwrap()
    );

    // The record file exists at <store_dir>/<id>.toml
    let recorded = store_dir.child(format!("{}.toml", ws.id));
    recorded.assert(predicates::path::is_file());
}

#[test]
fn second_upsert_in_same_path_returns_same_id() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore {
        root: store_dir.path().to_path_buf(),
    };

    let ws1 = upsert(&store, project.path()).unwrap();
    let ws2 = upsert(&store, project.path()).unwrap();
    assert_eq!(ws1.id, ws2.id);
}

#[test]
fn second_upsert_updates_last_run_at() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore {
        root: store_dir.path().to_path_buf(),
    };

    let ws1 = upsert(&store, project.path()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let ws2 = upsert(&store, project.path()).unwrap();
    assert_ne!(
        ws1.last_run_at, ws2.last_run_at,
        "last_run_at should advance"
    );
}
