use super::*;
const GENERATION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
const OP1: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const OP2: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAX";
fn binding() -> Binding {
    Binding {
        server_id: "fixture".into(),
        url: "https://fixture.invalid/mcp".into(),
        generation: GENERATION.into(),
    }
}
#[test]
fn store_is_dedicated_and_metadata_never_contains_token() {
    let tmp = tempfile::tempdir().unwrap();
    // A provider-store fixture is a sentinel, never opened by MCP storage.
    fs::write(
        tmp.path().join("auth.json"),
        b"provider-fixture-do-not-read",
    )
    .unwrap();
    let store = Store::for_global(tmp.path());
    let owner = binding();
    let receipt = store
        .mutate(&owner, "absent", OP1, Some("fixture-only-token-A"))
        .unwrap();
    assert!(receipt.present);
    assert_eq!(receipt.credential_revision.len(), 32);
    assert!(!serde_json::to_string(&receipt)
        .unwrap()
        .contains("fixture-only-token-A"));
    assert!(!format!("{:?}", store.status(&owner).unwrap()).contains("fixture-only-token-A"));
    assert_eq!(
        fs::read(tmp.path().join("auth.json")).unwrap(),
        b"provider-fixture-do-not-read"
    );
    assert_eq!(store.resolve(&owner).unwrap(), "fixture-only-token-A");
    assert_eq!(Store::for_global(tmp.path()).receipt(OP1).unwrap(), receipt);
}
#[test]
fn binding_checks_server_endpoint_and_generation_without_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::for_global(tmp.path());
    let owner = binding();
    store
        .mutate(&owner, "absent", OP1, Some("fixture-only-token"))
        .unwrap();
    for foreign in [
        Binding {
            server_id: "other".into(),
            ..owner.clone()
        },
        Binding {
            url: "https://other.invalid/mcp".into(),
            ..owner.clone()
        },
        Binding {
            generation: OP2.into(),
            ..owner.clone()
        },
    ] {
        assert!(store.resolve(&foreign).is_err());
    }
}
#[test]
fn revisions_and_operations_refuse_replay_and_preserve_exact_receipt() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::for_global(tmp.path());
    let owner = binding();
    let first = store
        .mutate(&owner, "absent", OP1, Some("fixture-A"))
        .unwrap();
    assert!(matches!(
        store.mutate(&owner, "absent", OP2, Some("fixture-B")),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        store.mutate(&owner, &first.credential_revision, OP1, Some("fixture-C")),
        Err(Error::OperationExists)
    ));
    assert_eq!(store.resolve(&owner).unwrap(), "fixture-A");
    assert_eq!(store.receipt(OP1).unwrap(), first);
    let removed = store
        .mutate(&owner, &first.credential_revision, OP2, None)
        .unwrap();
    assert!(!removed.present);
    assert_ne!(removed.credential_revision, first.credential_revision);
    assert!(store.resolve(&owner).is_err());
    assert!(matches!(
        store.mutate(
            &owner,
            "absent",
            "01ARZ3NDEKTSV4RRFFQ69G5FAY",
            Some("fixture-new")
        ),
        Err(Error::Conflict)
    ));
}
#[test]
fn concurrent_cas_has_one_winner() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::for_global(tmp.path());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = [OP1, OP2]
        .into_iter()
        .map(|operation| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.mutate(&binding(), "absent", operation, Some("fixture-concurrent"))
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
}
#[test]
fn malformed_store_is_not_replaced_and_errors_do_not_echo_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::for_global(tmp.path());
    fs::write(&store.path, b"{fixture-secret-not-json").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let error = store
        .mutate(&binding(), "absent", OP1, Some("fixture-token"))
        .unwrap_err();
    assert!(!error.to_string().contains("fixture-secret"));
    assert_eq!(fs::read(&store.path).unwrap(), b"{fixture-secret-not-json");
}
#[test]
fn invalid_tokens_never_create_store() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::for_global(tmp.path());
    for token in ["", "fixture\r\nheader", "fixture token"] {
        assert!(matches!(
            store.mutate(&binding(), "absent", OP1, Some(token)),
            Err(Error::Invalid)
        ));
    }
    assert!(!store.path.exists());
}
#[cfg(unix)]
#[test]
fn store_is_private_and_symlinks_are_refused() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::for_global(tmp.path());
    store
        .mutate(&binding(), "absent", OP1, Some("fixture-token"))
        .unwrap();
    assert_eq!(
        fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let linked = tempfile::tempdir().unwrap();
    symlink(&store.path, linked.path().join("mcp-credentials.json")).unwrap();
    assert!(matches!(
        Store::for_global(linked.path()).resolve(&binding()),
        Err(Error::Unavailable)
    ));
}
