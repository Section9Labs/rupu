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

/// `--mode device` logs in as the account's own application on its own
/// instance, and records it, as `--mode sso` does.
#[test]
fn device_login_for_a_self_managed_account_uses_its_application() {
    let server = httpmock::MockServer::start();
    let device = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/oauth/authorize_device")
            .body_contains("client_id=corp-app")
            .body_contains("api");
        then.status(200).json_body(serde_json::json!({
            "device_code": "dc",
            "user_code": "0A44L90H",
            "verification_uri": "https://gitlab.example.com/oauth/device",
            "expires_in": 300,
            "interval": 0,
        }));
    });
    let token = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/oauth/token")
            .body_contains("device_code=dc")
            .body_contains("client_id=corp-app");
        then.status(200).json_body(serde_json::json!({
            "access_token": "at",
            "token_type": "Bearer",
            "expires_in": 7200,
            "refresh_token": "rt",
            "created_at": 1,
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        format!(
            "[scm.gl-corp]\nkind = \"gitlab\"\nbase_url = \"{}\"\noauth_client_id = \"corp-app\"\n",
            server.url("/api/v4")
        ),
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .env("RUPU_DEVICE_FAST_POLL", "1")
        .timeout(Duration::from_secs(30))
        .args(["auth", "login", "--account", "gl-corp", "--mode", "device"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("enter code: 0A44L90H")
                .and(predicate::str::contains("stored gl-corp sso credential")),
        );

    device.assert_hits(1);
    token.assert_hits(1);
    let auth: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("auth.json")).unwrap())
            .unwrap();
    // Each entry is the stored credential's own JSON.
    let stored: serde_json::Value =
        serde_json::from_str(auth["gl-corp/sso"].as_str().expect("an sso entry")).unwrap();
    assert_eq!(stored["credentials"]["oauth_client_id"], "corp-app");
    assert_eq!(
        stored["credentials"]["oauth_token_url"],
        server.url("/oauth/token")
    );
    assert_eq!(stored["refresh_token"], "rt");
}

/// The device grant needs the instance's own application as much as the
/// browser flow does, so the refusal is the same — before any request.
#[test]
fn device_login_for_a_self_managed_account_without_a_client_id_says_what_to_register() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        "[scm.gl-corp]\nkind = \"gitlab\"\nbase_url = \"https://gitlab.example.com/api/v4\"\n",
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .env("RUPU_DEVICE_FAST_POLL", "1")
        .timeout(Duration::from_secs(20))
        .args(["auth", "login", "--account", "gl-corp", "--mode", "device"])
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

/// An unreadable global config can't become gitlab.com and glab's
/// application for a device login either.
#[test]
fn device_login_for_gitlab_with_an_unreadable_global_config_is_refused() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        "[scm.gitlab]\nbase_url = \"https://gitlab.example.com/api/v4\"\noauth_client_id = \n",
    )
    .unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .env("RUPU_DEVICE_FAST_POLL", "1")
        .timeout(Duration::from_secs(20))
        .args(["auth", "login", "--account", "gitlab", "--mode", "device"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("config.toml").and(predicate::str::contains(
                "A GitLab login takes its instance",
            )),
        );
    assert!(!tmp.path().join("auth.json").exists(), "nothing was stored");
}
