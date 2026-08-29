use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn status_renders_four_column_header() {
    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .args(["auth", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ACCOUNT"))
        .stdout(predicate::str::contains("KIND"))
        .stdout(predicate::str::contains("API KEY"))
        .stdout(predicate::str::contains("SSO"));
}

/// Structured on `--format json`'s `rows` array rather than loose stdout
/// substrings: with the KIND column added, a bare `predicate::str::contains
/// ("anthropic")` against table stdout can be satisfied by another row's
/// KIND cell (e.g. a declared `foo-work` account of kind `anthropic`)
/// without the actual built-in `anthropic` ACCOUNT row ever existing. This
/// asserts each vendor's own row is present by `account`, not just that
/// its name appears somewhere in the output.
#[test]
fn status_lists_all_four_providers() {
    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .args(["auth", "status", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "auth status --format json exited non-zero: {output:?}"
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("auth status --format json should emit valid JSON");
    let rows = report["rows"].as_array().expect("report has a rows array");

    for account in ["anthropic", "openai", "gemini", "copilot"] {
        assert!(
            rows.iter().any(|r| r["account"] == account),
            "expected a row for built-in account {account:?}, got: {rows:#?}"
        );
    }
}

/// Pre-populate the backend cache so the CLI never probes the real OS
/// keychain. `BackendChoice::JsonFile` serialises as `"json_file"`. Mirrors
/// `cli_auth.rs`'s helper of the same name, duplicated here because
/// integration test binaries don't share a `tests/common` module in this
/// crate yet.
fn force_json_backend(tmp: &assert_fs::TempDir) {
    let cache_path = tmp.path().join("cache/auth-backend.json");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    std::fs::write(&cache_path, r#""json_file""#).unwrap();
}

/// Covers the actual behavioral change of this task: `status()` used to
/// walk a hardcoded 8-item `ProviderId` list and then append only config
/// entries whose kind was literally `"openai-compatible"`. A
/// config-declared account with any other kind (like `anthropic`) was
/// silently dropped. This test is hermetic — unlike the two tests above,
/// it sets `RUPU_HOME` to a fresh tempdir via `Command::env` rather than
/// relying on the ambient environment.
#[test]
fn status_json_lists_a_declared_account_with_its_resolved_kind_and_dedups_builtins() {
    let tmp = assert_fs::TempDir::new().unwrap();
    force_json_backend(&tmp);

    // `foo-work` is not a recognized vendor name, so if `resolve_kind`
    // fell back to the name (as it does for undeclared built-ins) it
    // would resolve to `None` / "-", not "anthropic". Its presence with
    // kind "anthropic" can only come from reading the config declaration
    // `login` writes.
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .args([
            "auth",
            "login",
            "--account",
            "foo-work",
            "--kind",
            "anthropic",
            "--mode",
            "api-key",
            "--key",
            "sk-test",
        ])
        .assert()
        .success();

    // A built-in vendor name *also* declared in config (e.g. a user
    // pinning tuning under `[providers.anthropic]`) must collapse into
    // the single pre-seeded built-in row, not append a duplicate.
    // `login --account anthropic --kind anthropic` deliberately skips
    // writing this declaration (kind == account needs no config to
    // resolve), so this is written directly.
    let config_path = tmp.path().join("config.toml");
    let existing = std::fs::read_to_string(&config_path).unwrap_or_default();
    std::fs::write(
        &config_path,
        format!("{existing}\n[providers.anthropic]\nkind = \"anthropic\"\n"),
    )
    .unwrap();

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .args(["auth", "status", "--format", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "auth status --format json exited non-zero: {output:?}"
    );

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("auth status --format json should emit valid JSON");
    let rows = report["rows"].as_array().expect("report has a rows array");

    let foo_work: Vec<_> = rows.iter().filter(|r| r["account"] == "foo-work").collect();
    assert_eq!(
        foo_work.len(),
        1,
        "declared account foo-work should appear exactly once: {rows:#?}"
    );
    assert_eq!(foo_work[0]["kind"], "anthropic");
    assert_eq!(foo_work[0]["api_key"], true);

    let anthropic: Vec<_> = rows
        .iter()
        .filter(|r| r["account"] == "anthropic")
        .collect();
    assert_eq!(
        anthropic.len(),
        1,
        "a built-in name also declared in config must dedup to a single row: {rows:#?}"
    );
}
