//! `config_paths` — which config files one load layers (shared by the CLI
//! and the CP). Moved from `rupu-cli`'s `paths.rs` unit tests.

use rupu_workspace::{config_paths, CustomerStore, NewCustomer, ProjectRef};
use std::path::{Path, PathBuf};

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join(".rupu")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    (tmp, home, project)
}

fn assign(home: &Path, project: &Path) {
    let store = CustomerStore::new(home);
    store
        .create(
            "acme",
            &NewCustomer {
                name: "Acme".into(),
                ..NewCustomer::default()
            },
        )
        .unwrap();
    store.assign("acme", ProjectRef::Path(project)).unwrap();
}

#[test]
fn config_paths_includes_the_customer_of_the_project_root() {
    let (_t, home, project) = setup();
    assign(&home, &project);
    let p = config_paths(&home, Some(&project), Path::new("/")).unwrap();
    assert_eq!(p.global, home.join("config.toml"));
    assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
    assert_eq!(p.project, Some(project.join(".rupu/config.toml")));
}

#[test]
fn the_global_dir_is_never_also_the_project_layer() {
    // `project_root_for` resolves a repo without its own `.rupu/` to
    // `$HOME`, whose `.rupu` is the global dir: its config.toml must not
    // be loaded a second time as the project layer.
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join(".rupu");
    std::fs::create_dir_all(&global).unwrap();
    let p = config_paths(&global, Some(tmp.path()), tmp.path()).unwrap();
    assert_eq!(p.global, global.join("config.toml"));
    assert_eq!(p.project, None);
    // Also through a non-canonical spelling of the same directory.
    let dotted = tmp.path().join("sub/..");
    std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    let p = config_paths(&global, Some(&dotted), tmp.path()).unwrap();
    assert_eq!(p.project, None);
    // A root whose `.rupu` is a symlink to the global dir (canonical
    // compare, not a textual one).
    #[cfg(unix)]
    {
        let alias = tmp.path().join("alias");
        std::fs::create_dir_all(&alias).unwrap();
        std::os::unix::fs::symlink(&global, alias.join(".rupu")).unwrap();
        let p = config_paths(&global, Some(&alias), &alias).unwrap();
        assert_eq!(p.project, None);
    }
    // A normal project root still yields its own config.
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join(".rupu")).unwrap();
    let p = config_paths(&global, Some(&proj), &proj).unwrap();
    assert_eq!(p.project, Some(proj.join(".rupu/config.toml")));
}

#[test]
fn config_paths_falls_back_to_the_run_dir_without_a_project_root() {
    let (_t, home, project) = setup();
    assign(&home, &project);
    let p = config_paths(&home, None, &project).unwrap();
    assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
    assert_eq!(p.project, None);
}

#[test]
fn config_paths_looks_up_from_the_run_dir_before_the_project_root() {
    // `project_root_for` resolves to $HOME (which has `~/.rupu`) for any
    // repo without its own `.rupu/`; the repo's own assignment must win.
    let (_t, home, fake_home) = setup();
    let repo = fake_home.join("code/repo");
    std::fs::create_dir_all(&repo).unwrap();
    assign(&home, &repo);
    let p = config_paths(&home, Some(&fake_home), &repo).unwrap();
    assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
    assert_eq!(p.project, Some(fake_home.join(".rupu/config.toml")));

    // Even when the project root has a customer of its own.
    let store = CustomerStore::new(&home);
    store
        .create(
            "homeco",
            &NewCustomer {
                name: "Home".into(),
                ..NewCustomer::default()
            },
        )
        .unwrap();
    store
        .assign("homeco", ProjectRef::Path(&fake_home))
        .unwrap();
    let p = config_paths(&home, Some(&fake_home), &repo).unwrap();
    assert_eq!(p.customer, Some(home.join("customers/acme/config.toml")));
    // An unassigned run dir falls back to the project root's customer.
    let elsewhere = fake_home.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let p = config_paths(&home, Some(&fake_home), &elsewhere).unwrap();
    assert_eq!(p.customer, Some(home.join("customers/homeco/config.toml")));
}

#[test]
fn strict_errors_on_a_dangling_assignment() {
    let (_t, home, project) = setup();
    assign(&home, &project);
    std::fs::remove_dir_all(home.join("customers/acme")).unwrap();
    let err = config_paths(&home, Some(&project), &project).unwrap_err();
    // Launch failures print `{}` — the actionable cause must be in it.
    let shown = err.to_string();
    assert!(shown.contains("acme"), "{shown}");
    assert!(shown.contains("does not exist"), "{shown}");
}

#[test]
fn config_paths_reports_the_customer_slug() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let store = rupu_workspace::CustomerStore::new(&home);
    store
        .create(
            "acme",
            &rupu_workspace::NewCustomer {
                name: "Acme".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .assign("acme", rupu_workspace::ProjectRef::Path(&repo))
        .unwrap();
    let p = rupu_workspace::config_paths(&home, None, &repo).unwrap();
    assert_eq!(p.customer_slug.as_deref(), Some("acme"));
    assert_eq!(p.customer, Some(store.config_path("acme")));
}

#[test]
fn config_paths_for_customer_uses_the_slug_and_refuses_an_unknown_one() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let store = rupu_workspace::CustomerStore::new(&home);
    store
        .create(
            "acme",
            &rupu_workspace::NewCustomer {
                name: "Acme".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let p = rupu_workspace::config_paths_for_customer(&home, Some("acme"), None).unwrap();
    assert_eq!(p.customer, Some(store.config_path("acme")));
    assert_eq!(p.customer_slug.as_deref(), Some("acme"));
    let none = rupu_workspace::config_paths_for_customer(&home, None, None).unwrap();
    assert_eq!(none.customer, None);
    assert!(matches!(
        rupu_workspace::config_paths_for_customer(&home, Some("gone"), None),
        Err(rupu_workspace::CustomerError::NotFound(_))
    ));
}
