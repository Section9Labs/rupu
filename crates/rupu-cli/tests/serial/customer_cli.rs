//! `rupu customer` end to end: the store it writes is the one a run reads.

use crate::ENV_LOCK;
use assert_fs::prelude::*;

async fn rupu(args: &[&str]) -> std::process::ExitCode {
    let mut argv = vec!["rupu".to_string()];
    argv.extend(args.iter().map(|s| s.to_string()));
    // Boxed: `run`'s future is large in a debug build, and a test that awaits
    // it nine times would otherwise hold nine of them in its own frame.
    Box::pin(rupu_cli::run(argv)).await
}

/// `ExitCode` has no `PartialEq`; compare the way the other serial tests do.
fn ok(code: std::process::ExitCode) -> bool {
    format!("{code:?}") == format!("{:?}", std::process::ExitCode::from(0))
}

#[tokio::test(flavor = "multi_thread")]
async fn create_assign_show_and_delete_round_trip() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("proj");
    project.child(".rupu").create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();

    assert!(ok(rupu(&[
        "customer",
        "create",
        "acme",
        "--name",
        "Acme Corp"
    ])
    .await));
    assert!(
        !ok(rupu(&["customer", "create", "acme", "--name", "Again"]).await),
        "duplicate refused"
    );
    assert!(
        ok(rupu(&["customer", "assign", "acme"]).await),
        "defaults to the cwd project"
    );

    let store = rupu_workspace::CustomerStore::new(home.path());
    assert_eq!(
        store.customer_for_dir(project.path()).unwrap().as_deref(),
        Some("acme")
    );
    assert!(ok(rupu(&["customer", "show", "acme"]).await));
    assert!(ok(rupu(&["customer", "list"]).await));

    assert!(
        !ok(rupu(&["customer", "delete", "acme"]).await),
        "refused while assigned"
    );
    assert!(ok(rupu(&["customer", "unassign"]).await));
    assert!(ok(rupu(&["customer", "delete", "acme"]).await));
    assert!(store.list(true).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn set_archive_and_unarchive() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    assert!(ok(
        rupu(&["customer", "create", "acme", "--name", "Acme"]).await
    ));
    assert!(ok(rupu(&[
        "customer", "set", "acme", "--color", "#112233", "--notes", "retainer"
    ])
    .await));
    assert!(
        !ok(rupu(&["customer", "set", "acme", "--color", "red"]).await),
        "bad color refused"
    );
    assert!(
        !ok(rupu(&["customer", "set", "acme"]).await),
        "set with no flags refused"
    );
    assert!(ok(rupu(&["customer", "archive", "acme"]).await));
    let store = rupu_workspace::CustomerStore::new(home.path());
    let c = store.get("acme").unwrap();
    assert!(c.meta.archived);
    assert_eq!(c.meta.color.as_deref(), Some("#112233"));
    assert_eq!(c.meta.notes.as_deref(), Some("retainer"));
    assert!(ok(rupu(&["customer", "unarchive", "acme"]).await));
    assert!(!store.get("acme").unwrap().meta.archived);
}

#[tokio::test(flavor = "multi_thread")]
async fn assign_by_explicit_project_path() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("elsewhere");
    project.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    assert!(ok(
        rupu(&["customer", "create", "acme", "--name", "Acme"]).await
    ));
    let p = project.path().to_str().unwrap();
    assert!(ok(
        rupu(&["customer", "assign", "acme", "--project", p]).await
    ));
    let store = rupu_workspace::CustomerStore::new(home.path());
    assert_eq!(store.projects_of("acme").unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn assign_without_project_uses_cwd_not_home() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let repo = tmp.child("repo");
    repo.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(repo.path()).unwrap();

    assert!(ok(
        rupu(&["customer", "create", "acme", "--name", "Acme"]).await
    ));
    assert!(ok(rupu(&["customer", "assign", "acme"]).await));
    let store = rupu_workspace::CustomerStore::new(home.path());
    let projects = store.projects_of("acme").unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(
        std::fs::canonicalize(&projects[0].path).unwrap(),
        std::fs::canonicalize(repo.path()).unwrap(),
        "the cwd, not the parent holding the global .rupu"
    );
}

/// A malformed customer layer fails every launch, but the display commands
/// still work: `customer show` prints the customer with the layer's error in
/// place of the effective config (exit 0), and a UI-prefs-only command run
/// in an assigned project (`repos tracked`, `ui themes`) degrades to the
/// config without the customer layer instead of failing.
#[tokio::test(flavor = "multi_thread")]
async fn customer_with_a_malformed_layer_still_shows_and_display_commands_still_run() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    let project = tmp.child("proj");
    project.child(".rupu").create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(project.path()).unwrap();

    assert!(ok(
        rupu(&["customer", "create", "acme", "--name", "Acme"]).await
    ));
    assert!(ok(rupu(&["customer", "assign", "acme"]).await));
    let store = rupu_workspace::CustomerStore::new(home.path());
    std::fs::write(store.config_path("acme"), "default_provider = \n").unwrap();

    assert!(
        ok(rupu(&["customer", "show", "acme"]).await),
        "show degrades on a malformed layer"
    );
    assert!(ok(rupu(&["repos", "tracked"]).await), "repos tracked");
    assert!(ok(rupu(&["ui", "themes"]).await), "ui themes");
}

/// `none` is reserved (it is the `?customer=` filter's "no customer"):
/// `rupu customer create none` fails and creates nothing.
#[tokio::test(flavor = "multi_thread")]
async fn create_refuses_the_reserved_slug_none() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child(".rupu");
    home.create_dir_all().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_current_dir(tmp.path()).unwrap();

    assert!(
        !ok(rupu(&["customer", "create", "none", "--name", "Nobody"]).await),
        "the reserved slug is refused"
    );
    let store = rupu_workspace::CustomerStore::new(home.path());
    assert!(store.list(true).unwrap().is_empty());
    assert!(ok(rupu(&["customer", "create", "nonesuch", "--name", "Ok"]).await));
}
