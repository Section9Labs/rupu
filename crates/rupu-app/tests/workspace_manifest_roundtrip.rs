//! End-to-end test: build a WorkspaceManifest, save it via storage::save,
//! load it back via storage::load, assert equality. Uses a TempDir +
//! XDG_CONFIG_HOME override so the test doesn't touch the real
//! ~/Library/Application Support tree.

use rupu_app::workspace::{
    manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest},
    storage,
};
use serial_test::serial;
use std::env;
use tempfile::TempDir;

#[test]
#[serial]
fn save_then_load_yields_identical_manifest() {
    // Sandbox: directories crate honors XDG_CONFIG_HOME on Linux but
    // NOT macOS — there we have to redirect HOME instead. We do both
    // so this test passes on both targets.
    let tmp = TempDir::new().expect("tempdir");
    env::set_var("HOME", tmp.path());
    env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));

    let original = WorkspaceManifest {
        id: format!("ws_{}", ulid::Ulid::new()),
        name: "test-workspace".into(),
        color: WorkspaceColor::Pink,
        path: "/tmp/test-project".into(),
        opened_at: chrono::Utc::now(),
        repos: vec![
            RepoBinding { r#ref: "github:acme/foo".into() },
            RepoBinding { r#ref: "gitlab:acme/bar".into() },
        ],
        attached_hosts: vec![AttachedHost::Local],
        ui: UiState::default(),
    };

    storage::save(&original).expect("save");
    let loaded = storage::load(&original.id).expect("load");

    assert_eq!(loaded, original);
}
