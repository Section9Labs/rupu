//! `rupu auth backend` survives as a reporting command, but the keychain
//! is gone. Asking for it must fail loudly: silently writing to a
//! plaintext file while the user believes they selected an OS keystore
//! would be a security-relevant lie.

use assert_cmd::Command;

/// Isolate `RUPU_HOME` so these never touch the developer's real
/// `~/.rupu` (the reporting path would otherwise delete a real probe
/// cache file).
fn rupu(home: &assert_fs::TempDir) -> Command {
    let mut cmd = Command::cargo_bin("rupu").expect("rupu binary builds");
    cmd.env("RUPU_HOME", home.path());
    cmd
}

#[test]
fn requesting_the_keychain_is_an_explicit_error() {
    let home = assert_fs::TempDir::new().unwrap();
    rupu(&home)
        .args(["auth", "backend", "--use", "keychain"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no longer supported"));
}

/// Every alias the old command accepted must fail the same way — a user
/// who typed `--use keyring` deserves the same answer as `--use keychain`.
#[test]
fn every_keychain_alias_is_refused() {
    for alias in ["keyring", "keychain", "os", "os-keychain"] {
        let home = assert_fs::TempDir::new().unwrap();
        rupu(&home)
            .args(["auth", "backend", "--use", alias])
            .assert()
            .failure()
            .stderr(predicates::str::contains("no longer supported"));
    }
}

#[test]
fn requesting_the_file_backend_succeeds() {
    let home = assert_fs::TempDir::new().unwrap();
    rupu(&home)
        .args(["auth", "backend", "--use", "file"])
        .assert()
        .success();
}

#[test]
fn an_unknown_backend_names_the_only_valid_value() {
    let home = assert_fs::TempDir::new().unwrap();
    rupu(&home)
        .args(["auth", "backend", "--use", "vault"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("only supported value is: file"));
}

#[test]
fn reporting_the_backend_still_works() {
    let home = assert_fs::TempDir::new().unwrap();
    rupu(&home)
        .args(["auth", "backend"])
        .assert()
        .success()
        .stdout(predicates::str::contains("file"));
}

/// The probe cache chose between backends that no longer both exist.
/// Reporting the backend must clean it up rather than leave a file that
/// implies a choice is still being made.
#[test]
fn a_stale_probe_cache_is_removed() {
    let home = assert_fs::TempDir::new().unwrap();
    let cache = home.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let cache_file = cache.join("auth-backend.json");
    std::fs::write(&cache_file, r#""keyring""#).unwrap();

    rupu(&home).args(["auth", "backend"]).assert().success();

    assert!(
        !cache_file.exists(),
        "the stale probe cache should have been removed"
    );
}
