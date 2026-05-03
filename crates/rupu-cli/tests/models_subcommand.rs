use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn models_list_prints_table_header() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", dir.path())
        .env(
            "RUPU_CACHE_DIR_OVERRIDE",
            dir.path().join("cache/models").to_str().unwrap(),
        )
        .args(["models", "list", "--provider", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PROVIDER"))
        .stdout(predicate::str::contains("MODEL"))
        .stdout(predicate::str::contains("SOURCE"));
}

#[test]
fn models_list_copilot_shows_baked_in_entries_offline() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", dir.path())
        .env(
            "RUPU_CACHE_DIR_OVERRIDE",
            dir.path().join("cache/models").to_str().unwrap(),
        )
        .args(["models", "list", "--provider", "copilot"])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-4o"))
        .stdout(predicate::str::contains("baked-in"));
}
