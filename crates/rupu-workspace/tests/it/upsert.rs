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

/// Every launch upserts its workspace, so runs of one project write the same
/// record concurrently. Each write must land whole: afterwards there is
/// exactly one record for the path and it parses.
#[test]
fn concurrent_upserts_of_one_project_never_tear_the_record() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore {
        root: store_dir.path().to_path_buf(),
    };
    let first = upsert(&store, project.path()).unwrap();

    let handles: Vec<_> = (0..16)
        .map(|_| {
            let store = store.clone();
            let path = project.path().to_path_buf();
            std::thread::spawn(move || {
                for _ in 0..20 {
                    upsert(&store, &path).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let records = store.list().unwrap();
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].id, first.id);
    assert!(store.load(&first.id).unwrap().is_some());
    let leftovers: Vec<_> = std::fs::read_dir(store_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".toml"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
}
