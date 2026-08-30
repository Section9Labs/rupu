//! Integration test: the design spec §7 Arc 2 acceptance walkthrough
//! (Task 7 item 5), driven end-to-end through real `rupu` subprocesses —
//! `auth login` (writing the `[scm.<account>]` section — the Task 7
//! fix to `declare_account_in_config`'s section routing) and `scm bind`
//! (the new command this task adds, appending `[[scm.rules]]`), then
//! `repos list` (fan-out union tagged by account) and
//! `issues list --repo` (rule-routed to exactly one account).
//!
//! Unlike `cli_issues_multi_account.rs` (which hand-writes
//! `config.toml` and seeds credentials directly through
//! `KeychainResolver`), every account/rule declaration and credential
//! store here goes through the real CLI commands a user would actually
//! type. The only thing this test does by hand is point each declared
//! account's `base_url` at its own mock GitHub server — `auth login`
//! authenticates, it has no flag to configure endpoints, and a real
//! second-host user (GHES) would hand-edit exactly this field too.
//!
//! Two independent mock GitHub API servers, not one shared server plus
//! header inspection, so "gh-work's connector served this request" is
//! provable by which server actually received the HTTP call — a
//! stronger assertion than exit-0, which would pass identically whether
//! the right account answered, the wrong one did, or neither did.
//!
//! No `ENV_LOCK`/`EnvVarGuard` needed (unlike the sibling test): every
//! credential-touching operation here happens inside a child
//! subprocess with `RUPU_HOME` passed via `.env(..)`, never through an
//! in-process `KeychainResolver` reading the parent's process-global
//! env at construction.

use assert_cmd::Command;
use httpmock::prelude::*;
use predicates::prelude::*;

const WORK_REPO_BODY: &str = r#"[
  {
    "id": 1,
    "name": "api",
    "full_name": "acme/api",
    "owner": null,
    "private": true,
    "default_branch": "main",
    "url": "https://api.github.com/repos/acme/api",
    "clone_url": "https://github.com/acme/api.git",
    "ssh_url": "git@github.com:acme/api.git",
    "description": null
  }
]"#;

const PERSONAL_REPO_BODY: &str = r#"[
  {
    "id": 2,
    "name": "dots",
    "full_name": "MrBrutti/dots",
    "owner": null,
    "private": false,
    "default_branch": "main",
    "url": "https://api.github.com/repos/MrBrutti/dots",
    "clone_url": "https://github.com/MrBrutti/dots.git",
    "ssh_url": "git@github.com:MrBrutti/dots.git",
    "description": null
  }
]"#;

/// Insert `base_url = "<url>"` right after a just-declared
/// `[scm.<account>]\nkind = "github"` block. Pinned to the exact text
/// `declare_account_in_config` (`cmd/auth.rs`) is known to produce —
/// `cmd::auth::tests::declare_account_in_config_writes_scm_section_for_github`
/// pins that same shape from the other side, so a drift in one breaks
/// the other rather than this test silently matching nothing.
fn set_base_url(cfg_path: &std::path::Path, account: &str, url: &str) {
    let text = std::fs::read_to_string(cfg_path).unwrap();
    let marker = format!("[scm.{account}]\nkind = \"github\"\n");
    assert!(
        text.contains(&marker),
        "expected `{marker}` in config, got:\n{text}"
    );
    let replacement = format!("[scm.{account}]\nkind = \"github\"\nbase_url = \"{url}\"\n");
    let new_text = text.replacen(&marker, &replacement, 1);
    std::fs::write(cfg_path, new_text).unwrap();
}

#[tokio::test]
async fn arc2_walkthrough_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workdir = tmp.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();
    let cfg_path = home.join("config.toml");

    let work_server = MockServer::start_async().await;
    let personal_server = MockServer::start_async().await;

    let work_repos_mock = work_server
        .mock_async(|when, then| {
            when.method(GET).path("/user/repos");
            then.status(200)
                .header("content-type", "application/json")
                .body(WORK_REPO_BODY);
        })
        .await;
    let personal_repos_mock = personal_server
        .mock_async(|when, then| {
            when.method(GET).path("/user/repos");
            then.status(200)
                .header("content-type", "application/json")
                .body(PERSONAL_REPO_BODY);
        })
        .await;
    let work_issues_mock = work_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;
    let personal_issues_mock = personal_server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/api/issues");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        })
        .await;

    // Steps 1-2 (spec §7): `rupu auth login --account <name> --kind
    // github --mode <mode> ...`. `--mode api-key` here, not the
    // walkthrough's `--mode sso`: GitHub's SSO flow is an interactive
    // device-code browser flow with no headless path, but
    // `declare_account_in_config` doesn't branch on auth mode, so this
    // exercises the identical account-declaration code this task fixed.
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "auth",
            "login",
            "--account",
            "gh-work",
            "--kind",
            "github",
            "--mode",
            "api-key",
            "--key",
            "gh-work-token",
        ])
        .assert()
        .success();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "auth",
            "login",
            "--account",
            "gh-personal",
            "--kind",
            "github",
            "--mode",
            "api-key",
            "--key",
            "gh-personal-token",
        ])
        .assert()
        .success();

    // `auth login` writes `[scm.<account>] kind = "github"` with no
    // `base_url` — point each account at its mock server the same way a
    // real GHES / second-host user would.
    set_base_url(&cfg_path, "gh-work", &work_server.base_url());
    set_base_url(&cfg_path, "gh-personal", &personal_server.base_url());

    // Steps 3-4 (spec §7): `rupu scm bind --owner <glob> --account
    // <name>` — the new command this task adds.
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["scm", "bind", "--owner", "acme/*", "--account", "gh-work"])
        .assert()
        .success();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "scm",
            "bind",
            "--owner",
            "MrBrutti/*",
            "--account",
            "gh-personal",
        ])
        .assert()
        .success();

    // Step 5 (spec §7): `rupu repos list` -> BOTH accounts, one table,
    // tagged.
    let repos_assert = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(&workdir)
        .args(["repos", "list"])
        .assert()
        .success();
    let stdout = String::from_utf8(repos_assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("gh-work"), "missing gh-work row:\n{stdout}");
    assert!(
        stdout.contains("gh-personal"),
        "missing gh-personal row:\n{stdout}"
    );
    assert!(stdout.contains("acme/api"), "missing acme/api:\n{stdout}");
    assert!(
        stdout.contains("MrBrutti/dots"),
        "missing MrBrutti/dots:\n{stdout}"
    );
    work_repos_mock.assert();
    personal_repos_mock.assert();

    // Step 6 (spec §7): `rupu issues list --repo github:acme/api` ->
    // gh-work ONLY, via the owner rule — `personal_issues_mock`'s
    // `assert_hits(0)` is the proof the other account's connector was
    // never touched, not merely that the command exited 0.
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(&workdir)
        .args(["issues", "list", "--repo", "github:acme/api"])
        .assert()
        .success();
    work_issues_mock.assert();
    personal_issues_mock.assert_hits(0);
}

/// The ambiguity error (spec §6.4): a targeted repo with no matching
/// rule and two candidates must error naming both accounts and the fix
/// — never silently pick one. Same config as the walkthrough above
/// (two accounts, owner rules for `acme/*`/`MrBrutti/*`), but a repo
/// neither rule names.
#[tokio::test]
async fn arc2_walkthrough_ambiguity_error_names_candidates_and_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let workdir = tmp.path().join("work");
    std::fs::create_dir_all(&workdir).unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "auth",
            "login",
            "--account",
            "gh-work",
            "--kind",
            "github",
            "--mode",
            "api-key",
            "--key",
            "t1",
        ])
        .assert()
        .success();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "auth",
            "login",
            "--account",
            "gh-personal",
            "--kind",
            "github",
            "--mode",
            "api-key",
            "--key",
            "t2",
        ])
        .assert()
        .success();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["scm", "bind", "--owner", "acme/*", "--account", "gh-work"])
        .assert()
        .success();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "scm",
            "bind",
            "--owner",
            "MrBrutti/*",
            "--account",
            "gh-personal",
        ])
        .assert()
        .success();

    let assert = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .current_dir(&workdir)
        .args(["issues", "list", "--repo", "github:other/thing"])
        .assert()
        .failure();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("no account rule matches other/thing"),
        "got: {stderr}"
    );
    assert!(stderr.contains("gh-work"), "got: {stderr}");
    assert!(stderr.contains("gh-personal"), "got: {stderr}");
    assert!(
        stderr.contains("rupu scm bind --owner 'other/*' --account <name>"),
        "got: {stderr}"
    );
}

/// Migration: an account declared before the Task 7 `auth.rs` fix (or
/// by hand) sits under `[providers.<account>]` with a github kind --
/// invisible to `Registry::discover`, which only reads
/// `cfg.scm.platforms`. Re-running the identical `auth login` command
/// against that stale config must repair it rather than silently
/// re-store the credential and say nothing: this pins that the repair
/// actually lands on disk under `[scm.<account>]`, and that the account
/// is then genuinely visible to `rupu scm accounts`.
#[tokio::test]
async fn auth_login_self_heals_an_account_stranded_under_providers() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("config.toml");

    // Simulate a pre-fix declaration: exactly what the OLD
    // `declare_account_in_config` (unconditionally `[providers.*]`)
    // would have written for `auth login --account gh-work --kind
    // github`.
    std::fs::write(&cfg_path, "[providers.gh-work]\nkind = \"github\"\n").unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "auth",
            "login",
            "--account",
            "gh-work",
            "--kind",
            "github",
            "--mode",
            "api-key",
            "--key",
            "gh-work-token",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("migrated stale declaration"))
        .stdout(predicate::str::contains("[scm.gh-work]"));

    let text = std::fs::read_to_string(&cfg_path).unwrap();
    let v: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        v["scm"]["gh-work"]["kind"].as_str(),
        Some("github"),
        "migration did not land [scm.gh-work] in the config:\n{text}"
    );
    // The stale table is left in place (never deleted), so both must
    // now agree.
    assert_eq!(v["providers"]["gh-work"]["kind"].as_str(), Some("github"));

    // The account is now genuinely visible to `scm accounts`, not just
    // present in the config text.
    let assert = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["scm", "accounts"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("gh-work"), "got: {stdout}");
}

/// Arc 2 final review item 4: `scm bind` used to write a rule for a
/// typo'd `--account` with zero signal — the only feedback was a WARN
/// at the *next* config load, deep in an unrelated command's log
/// output. `warn_if_account_unknown` (`cmd/scm.rs`) now surfaces this
/// immediately on stderr. Non-blocking by design (see that function's
/// doc): the command still succeeds and the rule still lands, because
/// `scm bind` before `auth login` is a legitimate forward-declaration
/// order.
#[tokio::test]
async fn bind_warns_on_stderr_for_an_unknown_account_but_still_writes_the_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("config.toml");

    let assert = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args([
            "scm",
            "bind",
            "--owner",
            "acme/*",
            "--account",
            "gh-typo-account",
        ])
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("gh-typo-account") && stderr.contains("not a declared"),
        "expected an unknown-account warning on stderr, got:\n{stderr}"
    );

    // Non-blocking: the rule is still written despite the warning.
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        text.contains("gh-typo-account"),
        "rule must still be written:\n{text}"
    );
}

/// The bare-vendor-name and already-declared cases must NOT warn — a
/// single-account user binding `--account github` (today's back-compat
/// default) or a user re-binding a second rule at an already-declared
/// `[scm.gh-work]` account should see clean stderr.
#[tokio::test]
async fn bind_does_not_warn_for_a_bare_vendor_name() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    let assert = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", &home)
        .args(["scm", "bind", "--owner", "acme/*", "--account", "github"])
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("not a declared"),
        "bare vendor name must not warn, got:\n{stderr}"
    );
}
