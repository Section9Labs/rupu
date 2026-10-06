use assert_fs::prelude::*;
use rupu_workspace::{
    upsert, CustomerError, CustomerStore, MetaPatch, NewCustomer, ProjectRef, WorkspaceStore,
};

fn home() -> assert_fs::TempDir {
    assert_fs::TempDir::new().unwrap()
}

fn acme() -> NewCustomer {
    NewCustomer {
        name: "Acme Corp".into(),
        notes: None,
        contact: Some("ops@acme.example".into()),
        color: Some("#3366cc".into()),
    }
}

fn ws_store(home: &std::path::Path) -> WorkspaceStore {
    WorkspaceStore {
        root: home.join("workspaces"),
    }
}

#[test]
fn slug_grammar_is_enforced() {
    for ok in ["acme", "a", "acme-2", "0day"] {
        rupu_workspace::validate_slug(ok).unwrap();
    }
    for bad in ["", "-acme", "Acme", "ac me", "acme_corp", &"a".repeat(64)] {
        assert!(
            matches!(
                rupu_workspace::validate_slug(bad),
                Err(CustomerError::InvalidSlug(_))
            ),
            "{bad:?} should be rejected"
        );
    }
}

/// `none` is the `?customer=` filter's "no customer": `create` refuses it
/// with a reason, while an existing `none` customer (made before the slug
/// was reserved) still reads.
#[test]
fn create_refuses_the_reserved_slug_none_but_get_and_list_tolerate_one() {
    let h = home();
    let store = CustomerStore::new(h.path());
    let err = store.create("none", &acme()).unwrap_err();
    assert!(matches!(err, CustomerError::ReservedSlug(ref s) if s == "none"), "{err:?}");
    assert!(err.to_string().contains("reserved"), "{err}");
    assert!(matches!(store.get("none"), Err(CustomerError::NotFound(_))));
    // Written directly, the way a store from before the reservation holds it.
    store.create("acme", &acme()).unwrap();
    let acme_dir = store.config_path("acme").parent().unwrap().to_path_buf();
    let none_dir = acme_dir.with_file_name("none");
    std::fs::create_dir_all(&none_dir).unwrap();
    std::fs::copy(acme_dir.join("customer.toml"), none_dir.join("customer.toml")).unwrap();
    assert_eq!(store.get("none").unwrap().slug, "none");
    assert!(store.list(true).unwrap().iter().any(|c| c.slug == "none"));
}

#[test]
fn create_then_get_round_trips_and_writes_an_empty_layer() {
    let h = home();
    let store = CustomerStore::new(h.path());
    let c = store.create("acme", &acme()).unwrap();
    assert_eq!(c.slug, "acme");
    assert_eq!(c.meta.name, "Acme Corp");
    assert!(!c.meta.archived);
    assert_eq!(store.get("acme").unwrap(), c);
    h.child("customers/acme/customer.toml")
        .assert(predicates::path::is_file());
    // The layer file exists so `rupu customer edit` has something to open,
    // and it parses as an empty config.
    let layer = std::fs::read_to_string(store.config_path("acme")).unwrap();
    let v: toml::Value = toml::from_str(&layer).unwrap();
    assert!(v.as_table().unwrap().is_empty());
}

#[test]
fn create_refuses_duplicates_bad_colors_and_empty_names() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    assert!(matches!(
        store.create("acme", &acme()),
        Err(CustomerError::Exists(_))
    ));
    let mut bad = acme();
    bad.color = Some("blue".into());
    assert!(matches!(
        store.create("globex", &bad),
        Err(CustomerError::InvalidColor(_))
    ));
    let mut empty = acme();
    empty.name = "  ".into();
    assert!(matches!(
        store.create("globex", &empty),
        Err(CustomerError::EmptyName)
    ));
}

#[test]
fn list_is_sorted_and_hides_archived_unless_asked() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("zeta", &acme()).unwrap();
    store.create("acme", &acme()).unwrap();
    store.set_archived("zeta", true).unwrap();
    let live: Vec<_> = store
        .list(false)
        .unwrap()
        .into_iter()
        .map(|c| c.slug)
        .collect();
    assert_eq!(live, vec!["acme"]);
    let all: Vec<_> = store
        .list(true)
        .unwrap()
        .into_iter()
        .map(|c| c.slug)
        .collect();
    assert_eq!(all, vec!["acme", "zeta"]);
}

#[test]
fn update_meta_patches_only_given_fields_and_empty_clears() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let c = store
        .update_meta(
            "acme",
            &MetaPatch {
                name: Some("ACME".into()),
                contact: Some(String::new()),
                ..MetaPatch::default()
            },
        )
        .unwrap();
    assert_eq!(c.meta.name, "ACME");
    assert_eq!(
        c.meta.contact, None,
        "empty string clears an optional field"
    );
    assert_eq!(c.meta.color.as_deref(), Some("#3366cc"), "untouched");
}

#[test]
fn assign_registers_an_unknown_path_without_bumping_last_run() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    assert!(ws.id.starts_with("ws_"));
    assert_eq!(ws.last_run_at, None, "assigning is not a run");
    assert_eq!(store.customer_of(&ws.id).unwrap().as_deref(), Some("acme"));
    let projects = store.projects_of("acme").unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, ws.id);
}

#[test]
fn assign_refuses_unknown_and_archived_customers() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    assert!(matches!(
        store.assign("nope", ProjectRef::Path(project.path())),
        Err(CustomerError::NotFound(_))
    ));
    store.create("acme", &acme()).unwrap();
    store.set_archived("acme", true).unwrap();
    assert!(matches!(
        store.assign("acme", ProjectRef::Path(project.path())),
        Err(CustomerError::Archived(_))
    ));
}

#[test]
fn assign_by_unknown_ws_id_is_an_error() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    assert!(matches!(
        store.assign("acme", ProjectRef::Id("ws_missing")),
        Err(CustomerError::NoProject(_))
    ));
}

#[test]
fn assignment_survives_an_upsert_after_it() {
    // `upsert` rewrites the whole workspace record on every run; the
    // assignment lives in a sidecar it never touches.
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    let again = upsert(&ws_store(h.path()), project.path()).unwrap();
    assert_eq!(again.id, ws.id);
    assert_eq!(store.customer_of(&ws.id).unwrap().as_deref(), Some("acme"));
}

#[test]
fn unassign_removes_the_sidecar_and_is_idempotent() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    store.unassign(ProjectRef::Id(&ws.id)).unwrap();
    assert_eq!(store.customer_of(&ws.id).unwrap(), None);
    store.unassign(ProjectRef::Id(&ws.id)).unwrap();
}

#[test]
fn delete_refuses_while_projects_are_assigned() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    match store.delete("acme") {
        Err(CustomerError::HasProjects { slug, projects }) => {
            assert_eq!(slug, "acme");
            assert_eq!(projects, vec![ws.path.clone()]);
        }
        other => panic!("expected HasProjects, got {other:?}"),
    }
    store.unassign(ProjectRef::Id(&ws.id)).unwrap();
    store.delete("acme").unwrap();
    assert!(matches!(store.get("acme"), Err(CustomerError::NotFound(_))));
    h.child("customers/acme")
        .assert(predicates::path::missing());
}

#[test]
fn lookup_walks_up_to_the_nearest_assigned_ancestor() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let sub = project.child("src/deep");
    sub.create_dir_all().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    // A run in a subdirectory has its own (unassigned) workspace record.
    upsert(&ws_store(h.path()), sub.path()).unwrap();

    assert_eq!(
        store.customer_for_dir(sub.path()).unwrap().as_deref(),
        Some("acme")
    );
    assert_eq!(
        store.customer_config_for_dir(sub.path()).unwrap(),
        Some(store.config_path("acme"))
    );
}

#[test]
fn lookup_without_any_assignment_is_none() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    assert_eq!(store.customer_config_for_dir(project.path()).unwrap(), None);
    upsert(&ws_store(h.path()), project.path()).unwrap();
    assert_eq!(store.customer_config_for_dir(project.path()).unwrap(), None);
}

#[test]
fn a_dangling_assignment_is_an_error_not_a_fallback() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    // The customer directory disappears behind the store's back.
    std::fs::remove_dir_all(h.path().join("customers/acme")).unwrap();
    match store.customer_config_for_dir(project.path()) {
        Err(CustomerError::Dangling { slug, .. }) => assert_eq!(slug, "acme"),
        other => panic!("expected Dangling, got {other:?}"),
    }
}

#[test]
fn lookup_from_a_dir_that_does_not_exist_uses_its_nearest_existing_ancestor() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    let gone = project.path().join("does/not/exist");
    assert_eq!(
        store.customer_for_dir(&gone).unwrap().as_deref(),
        Some("acme")
    );
}

/// A second workspace record for `ws`'s directory, as a hand-copied or
/// racing `register` would leave, assigned to `slug`.
fn duplicate_record(home: &std::path::Path, ws: &rupu_workspace::Workspace, id: &str, slug: &str) {
    let mut dup = ws.clone();
    dup.id = id.to_string();
    let root = home.join("workspaces");
    std::fs::write(
        root.join(format!("{id}.toml")),
        toml::to_string(&dup).unwrap(),
    )
    .unwrap();
    std::fs::write(root.join(format!("{id}.customer")), format!("{slug}\n")).unwrap();
}

#[test]
fn duplicate_records_for_one_dir_assigned_to_different_customers_are_an_error() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    store.create("globex", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    duplicate_record(h.path(), &ws, "ws_dup", "globex");
    match store.customer_for_dir(project.path()) {
        Err(CustomerError::ConflictingAssignments {
            first_slug,
            first_sidecar,
            second_slug,
            second_sidecar,
            ..
        }) => {
            let mut slugs = [first_slug, second_slug];
            slugs.sort();
            assert_eq!(slugs, ["acme".to_string(), "globex".to_string()]);
            let sidecars = format!("{first_sidecar} {second_sidecar}");
            assert!(sidecars.contains("ws_dup.customer"), "{sidecars}");
            assert!(
                sidecars.contains(&format!("{}.customer", ws.id)),
                "{sidecars}"
            );
        }
        other => panic!("expected ConflictingAssignments, got {other:?}"),
    }
}

#[test]
fn duplicate_records_for_one_dir_assigned_to_the_same_customer_are_fine() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    duplicate_record(h.path(), &ws, "ws_dup", "acme");
    assert_eq!(
        store.customer_for_dir(project.path()).unwrap().as_deref(),
        Some("acme")
    );
}

#[test]
fn a_corrupt_record_behind_a_sidecar_fails_closed() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let elsewhere = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = store
        .assign("acme", ProjectRef::Path(project.path()))
        .unwrap();
    std::fs::write(
        h.path().join(format!("workspaces/{}.toml", ws.id)),
        "this is = = not toml",
    )
    .unwrap();
    let sidecar = format!("{}.customer", ws.id);
    // Even a lookup for an unrelated dir refuses: the broken record could
    // be any directory's assignment.
    for dir in [project.path(), elsewhere.path()] {
        match store.customer_for_dir(dir) {
            Err(CustomerError::UnresolvableAssignment { sidecar: s, .. }) => {
                assert!(s.ends_with(&sidecar), "{s}")
            }
            other => panic!("expected UnresolvableAssignment, got {other:?}"),
        }
    }
    assert!(matches!(
        store.projects_of("acme"),
        Err(CustomerError::UnresolvableAssignment { .. })
    ));
    assert!(matches!(
        store.delete("acme"),
        Err(CustomerError::UnresolvableAssignment { .. })
    ));
    assert!(store.get("acme").is_ok(), "delete must not remove acme");

    // A sidecar whose record is gone is just as unresolvable.
    std::fs::remove_file(h.path().join(format!("workspaces/{}.toml", ws.id))).unwrap();
    assert!(matches!(
        store.customer_for_dir(project.path()),
        Err(CustomerError::UnresolvableAssignment { .. })
    ));
}

#[test]
fn lookup_with_records_but_no_sidecars_is_none() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    upsert(&ws_store(h.path()), project.path()).unwrap();
    // A corrupt record without a sidecar is not an assignment: the lookup
    // never reads records when no sidecar exists.
    std::fs::write(h.path().join("workspaces/ws_junk.toml"), "= = =").unwrap();
    let store = CustomerStore::new(h.path());
    assert_eq!(store.customer_for_dir(project.path()).unwrap(), None);
}

#[test]
fn a_traversal_ws_id_is_refused_before_it_reaches_a_path() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    for bad in ["../../etc", "a/b", "", "ws id"] {
        assert!(
            matches!(
                store.assign("acme", ProjectRef::Id(bad)),
                Err(CustomerError::InvalidWsId(_))
            ),
            "{bad:?}"
        );
        assert!(matches!(
            store.customer_of(bad),
            Err(CustomerError::InvalidWsId(_))
        ));
        assert!(matches!(
            store.unassign(ProjectRef::Id(bad)),
            Err(CustomerError::InvalidWsId(_))
        ));
    }
    // Nothing was written anywhere under the home.
    assert!(!h.path().join("workspaces").exists());
    assert!(!h.path().join("etc.customer").exists());
    rupu_workspace::validate_ws_id("ws_01HXYZ-abc").unwrap();
}

#[test]
fn create_treats_an_empty_color_as_none() {
    let h = home();
    let store = CustomerStore::new(h.path());
    let mut c = acme();
    c.color = Some(String::new());
    let created = store.create("acme", &c).unwrap();
    assert_eq!(created.meta.color, None);
    assert_eq!(store.get("acme").unwrap().meta.color, None);
}

/// Every non-hidden-temp entry name in `dir`, and whether any temp file
/// (`.tmp*`, `*.tmp`) is left behind.
fn leftover_temp_files(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".tmp") || n.ends_with(".tmp"))
        .collect()
}

#[test]
fn concurrent_writers_never_tear_the_metadata_file() {
    let h = home();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                for j in 0..25 {
                    store
                        .update_meta(
                            "acme",
                            &MetaPatch {
                                notes: Some(format!("writer {i} pass {j} {}", "x".repeat(i * 50))),
                                ..MetaPatch::default()
                            },
                        )
                        // A reader may race a rename; only a torn file is a bug.
                        .ok();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let meta = store.get("acme").unwrap().meta;
    assert!(meta.notes.unwrap().starts_with("writer "));
    assert!(leftover_temp_files(&h.path().join("customers/acme")).is_empty());
}

#[test]
fn a_failed_rename_leaves_no_temp_file() {
    let h = home();
    let project = assert_fs::TempDir::new().unwrap();
    let store = CustomerStore::new(h.path());
    store.create("acme", &acme()).unwrap();
    let ws = upsert(&ws_store(h.path()), project.path()).unwrap();
    // A directory where the sidecar goes makes the final rename fail.
    let sidecar = h.path().join(format!("workspaces/{}.customer", ws.id));
    std::fs::create_dir_all(sidecar.join("blocker")).unwrap();
    assert!(matches!(
        store.assign("acme", ProjectRef::Id(&ws.id)),
        Err(CustomerError::Io { .. })
    ));
    assert!(leftover_temp_files(&h.path().join("workspaces")).is_empty());
}
