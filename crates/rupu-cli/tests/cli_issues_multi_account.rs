//! Integration test: two GitHub accounts configured plus an owner rule,
//! and `rupu issues list --repo` reaches the RIGHT account's connector —
//! Arc 2's headline multi-account SCM case (spec §6.2's "Targeted"
//! shape), the exact scenario Task 4 exists to migrate `issues.rs` onto
//! (`registry.issues_for`, not the old `registry.issues(tracker)` shim
//! that silently picked the lexicographically-first account).
//!
//! Two accounts, two independent mock GitHub API servers — rather than
//! one shared server plus header inspection — so "gh-work's connector
//! served this request" is provable by which server actually received
//! the HTTP call. That is a stronger assertion than an exit-0 check,
//! which would pass identically whether the right account answered or
//! the wrong one did (or neither, on a config that happened not to
//! error). `personal_mock.assert_hits(0)` is the actual proof: the
//! wrong account's credential/connector was never touched.
//!
//! Uses a real subprocess (`assert_cmd`), not the in-process
//! `rupu_cli::run` helper other tests in this crate use, specifically
//! so the child's cwd is an isolated tempdir — `issues list` walks up
//! from cwd looking for a project `.rupu/` directory, and this test
//! binary's own process cwd sits inside the real rupu checkout.
//!
//! **Seeding credentials mutates the process-global `RUPU_HOME`, so every
//! test here locks `ENV_LOCK` for its whole body.** This is not tidiness:
//! `KeychainResolver::with_service` captures its auth-file path at
//! CONSTRUCTION from `RUPU_HOME` (`crates/rupu-auth/src/resolver.rs`), and
//! these are `#[tokio::test]`s sharing one binary's thread pool. Without
//! the lock, one test's restore landing between another's set and its
//! `KeychainResolver::new()` would seed `gh-work-token` into the
//! DEVELOPER'S REAL `~/.rupu/auth.json`. All three tests do their
//! `MockServer` setup first, so they converge on that window together.
//! `EnvVarGuard` restores the prior value rather than blindly unsetting,
//! and does so on panic too. Mirrors the convention `accounts_sso_e2e.rs`
//! already established. The child process's `RUPU_HOME` (passed via
//! `.env()`) was never at risk — only the parent's seeding step.

use assert_cmd::Command;
use httpmock::prelude::*;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// RAII guard: sets an env var for the test's duration and restores
/// whatever value (if any) was already there on drop, even on panic.
/// The `set` counterpart to `accounts_sso_e2e.rs`'s `unset` guard.
struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let prior = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prior }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

#[tokio::test]
async fn issues_list_with_owner_rule_reaches_the_matching_account() {
    let _env_guard = ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let work_server = MockServer::start_async().await;
    let personal_server = MockServer::start_async().await;

    let work_mock = work_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;
    let personal_mock = personal_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;

    // Two named GitHub accounts, distinguished only by which mock
    // server their `base_url` points at, plus an owner rule that must
    // route `acme/*` to `gh-work` and never to `gh-personal`.
    let config = format!(
        r#"
[scm.gh-work]
kind = "github"
base_url = "{work_url}"

[scm.gh-personal]
kind = "github"
base_url = "{personal_url}"

[[scm.rules]]
owner = "acme/*"
account = "gh-work"
"#,
        work_url = work_server.base_url(),
        personal_url = personal_server.base_url(),
    );
    std::fs::write(home.join("config.toml"), config).unwrap();

    // Store a distinct API-key credential under each account name,
    // matching exactly what `rupu auth login --account <name> --kind
    // github --mode api-key --key <token>` writes.
    let _home_env = EnvVarGuard::set("RUPU_HOME", &home);
    let resolver = rupu_auth::KeychainResolver::new();
    resolver
        .store_named(
            "gh-work",
            rupu_providers::AuthMode::ApiKey,
            &rupu_auth::StoredCredential::api_key("gh-work-token"),
        )
        .await
        .unwrap();
    resolver
        .store_named(
            "gh-personal",
            rupu_providers::AuthMode::ApiKey,
            &rupu_auth::StoredCredential::api_key("gh-personal-token"),
        )
        .await
        .unwrap();

    // Isolated cwd with no ancestor `.rupu/` — see module doc.
    let workdir = tmp.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&workdir)
        .env("RUPU_HOME", &home)
        .args(["issues", "list", "--repo", "github:acme/api"])
        .assert()
        .success();

    work_mock.assert();
    personal_mock.assert_hits(0);
}

/// `--account` explicitly overrides the rule engine entirely (spec
/// §6.2's "Explicit" shape) — even though the owner rule above would
/// route `acme/*` to `gh-work`, an explicit `--account gh-personal`
/// must win.
#[tokio::test]
async fn issues_list_explicit_account_overrides_the_owner_rule() {
    let _env_guard = ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let work_server = MockServer::start_async().await;
    let personal_server = MockServer::start_async().await;

    let work_mock = work_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;
    let personal_mock = personal_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;

    let config = format!(
        r#"
[scm.gh-work]
kind = "github"
base_url = "{work_url}"

[scm.gh-personal]
kind = "github"
base_url = "{personal_url}"

[[scm.rules]]
owner = "acme/*"
account = "gh-personal"
"#,
        work_url = work_server.base_url(),
        personal_url = personal_server.base_url(),
    );
    std::fs::write(home.join("config.toml"), config).unwrap();

    let _home_env = EnvVarGuard::set("RUPU_HOME", &home);
    let resolver = rupu_auth::KeychainResolver::new();
    resolver
        .store_named(
            "gh-work",
            rupu_providers::AuthMode::ApiKey,
            &rupu_auth::StoredCredential::api_key("gh-work-token"),
        )
        .await
        .unwrap();
    resolver
        .store_named(
            "gh-personal",
            rupu_providers::AuthMode::ApiKey,
            &rupu_auth::StoredCredential::api_key("gh-personal-token"),
        )
        .await
        .unwrap();

    let workdir = tmp.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();

    // The owner rule above routes `acme/*` to `gh-personal` — and,
    // not incidentally, `gh-personal` also sorts before `gh-work`
    // lexicographically. Both `resolve_account`'s owner tier and the
    // old `registry.issues(tracker)` shim's "pick the
    // lexicographically-first account" fallback would independently
    // land on `gh-personal` here, so this alone would not discriminate
    // migrated code from unmigrated code. `--account gh-work` is the
    // part that only the migrated `issues_for` path can honor: it must
    // win over BOTH the rule and the alphabetical fallback.
    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&workdir)
        .env("RUPU_HOME", &home)
        .args([
            "issues",
            "list",
            "--repo",
            "github:acme/api",
            "--account",
            "gh-work",
        ])
        .assert()
        .success();

    work_mock.assert();
    personal_mock.assert_hits(0);
}

/// `--account <typo>` must error clearly rather than silently falling
/// back to some other account (spec §6.4's `UnknownAccount`).
#[tokio::test]
async fn issues_list_unknown_explicit_account_errors_clearly() {
    let _env_guard = ENV_LOCK.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let work_server = MockServer::start_async().await;
    let work_mock = work_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;

    let config = format!(
        r#"
[scm.gh-work]
kind = "github"
base_url = "{work_url}"
"#,
        work_url = work_server.base_url(),
    );
    std::fs::write(home.join("config.toml"), config).unwrap();

    let _home_env = EnvVarGuard::set("RUPU_HOME", &home);
    let resolver = rupu_auth::KeychainResolver::new();
    resolver
        .store_named(
            "gh-work",
            rupu_providers::AuthMode::ApiKey,
            &rupu_auth::StoredCredential::api_key("gh-work-token"),
        )
        .await
        .unwrap();

    let workdir = tmp.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .current_dir(&workdir)
        .env("RUPU_HOME", &home)
        .args([
            "issues",
            "list",
            "--repo",
            "github:acme/api",
            "--account",
            "gh-wrok",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no such account: gh-wrok"));

    work_mock.assert_hits(0);
}
