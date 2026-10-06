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
