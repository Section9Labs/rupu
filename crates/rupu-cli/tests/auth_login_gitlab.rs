//! `rupu auth login` for a GitLab account picks the account's OAuth
//! application from its `[scm.<account>]` table.

use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;

/// A self-managed instance has no default application (glab's exists only
/// on gitlab.com), so SSO is refused up front — before a browser opens or a
/// listener binds — with what to register and where to put its id.
#[test]
fn sso_for_a_self_managed_account_without_a_client_id_says_what_to_register() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        "[scm.gl-corp]\nkind = \"gitlab\"\nbase_url = \"https://gitlab.example.com/api/v4\"\n",
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        // Were the flow to start anyway: no browser, no port 7171, and a
        // bounded wait instead of the five-minute callback timeout.
        .env("RUPU_OAUTH_SKIP_BROWSER", "1")
        .env("RUPU_OAUTH_FORCE_PORT", "0")
        .timeout(Duration::from_secs(20))
        .args(["auth", "login", "--account", "gl-corp", "--mode", "sso"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("https://gitlab.example.com")
                .and(predicate::str::contains("[scm.gl-corp] oauth_client_id"))
                .and(predicate::str::contains(
                    "http://localhost:7171/auth/redirect",
                )),
        );
    assert!(!tmp.path().join("auth.json").exists(), "nothing was stored");
}

/// The instance and application come from the global config, so a config
/// that can't be read can't silently become gitlab.com and glab's
/// application — the token would be stored under the account's name for
/// the wrong instance.
#[test]
fn sso_for_gitlab_with_an_unreadable_global_config_is_refused() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        "[scm.gitlab]\nbase_url = \"https://gitlab.example.com/api/v4\"\noauth_client_id = \n",
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env("RUPU_OAUTH_SKIP_BROWSER", "1")
        .env("RUPU_OAUTH_FORCE_PORT", "0")
        .timeout(Duration::from_secs(20))
        .args(["auth", "login", "--account", "gitlab", "--mode", "sso"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("config.toml"));
    assert!(!tmp.path().join("auth.json").exists(), "nothing was stored");
}
