use assert_fs::prelude::*;
use rupu_workspace::{find_by_path, upsert, WorkspaceStore};

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
        // The registration lock sidecar is an expected, inert artifact.
        .filter(|n| !n.ends_with(".toml") && n != ".registration.lock")
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
}

/// The real registration race (distinct from the tear test above, which
/// pre-seeds the record): with the store EMPTY, N launches upsert the same
/// new path at once. Before the registration lock each found an empty store
/// and minted its own id, leaving several records for one path — which then
/// double-counted that path's findings/coverage/usage everywhere a reader
/// walks the store. Exactly one id must ever be minted, and every caller must
/// get it back.
#[test]
fn concurrent_first_upserts_of_a_new_path_register_one_workspace() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore {
        root: store_dir.path().to_path_buf(),
    };

    const N: usize = 24;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(N));
    let handles: Vec<_> = (0..N)
        .map(|_| {
            let store = store.clone();
            let path = project.path().to_path_buf();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                // Release all threads into the find-or-create at once.
                barrier.wait();
                upsert(&store, &path).unwrap().id
            })
        })
        .collect();
    let ids: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let first = ids[0].clone();
    assert!(
        ids.iter().all(|id| *id == first),
        "every upsert must adopt one id, got {ids:?}"
    );

    let records = store.list().unwrap();
    assert_eq!(records.len(), 1, "one record per path: {records:?}");
    assert_eq!(records[0].id, first);

    // Exactly one record FILE on disk — proof no second id was minted (the
    // read-side merge would hide a second record from `list`, but not from
    // the directory).
    let toml_files: Vec<_> = std::fs::read_dir(store_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".toml"))
        .collect();
    assert_eq!(
        toml_files.len(),
        1,
        "a second id was minted: {toml_files:?}"
    );
}

/// Two records with different ids but the IDENTICAL path — the shape a lost
/// registration race leaves behind, observed in a real `~/.rupu/workspaces`.
/// `list` (and so every CP reader that walks it) must collapse them to a
/// single, deterministic winner so existing data stops double-counting.
#[test]
fn list_collapses_duplicate_path_records_to_the_min_id() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore {
        root: store_dir.path().to_path_buf(),
    };
    let canon = project.path().canonicalize().unwrap();
    let p = canon.to_str().unwrap();

    // The exact pair observed in the field: same millisecond, same path.
    let lo = "ws_01M4BF2F3WD1QW318WGNKS4KNM"; // lexicographically smaller
    let hi = "ws_01M4BF2F3WX073DADT87SJKP4Z"; // lexicographically greater
    for id in [hi, lo] {
        store_dir
            .child(format!("{id}.toml"))
            .write_str(&format!(
                "id = \"{id}\"\npath = \"{p}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n"
            ))
            .unwrap();
    }

    let records = store.list().unwrap();
    assert_eq!(
        records.len(),
        1,
        "duplicate-path records must collapse: {records:?}"
    );
    assert_eq!(
        records[0].id, lo,
        "the earliest (min-id ULID) record wins, deterministically"
    );

    // find_by_path agrees with the deduped listing.
    let found = find_by_path(&store, project.path()).unwrap().unwrap();
    assert_eq!(found.id, lo);
}
