use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn login_api_key_with_inline_key_succeeds() {
    // Default credential backend is now the chmod-600 JSON file —
    // point it at a per-test temp file so this test doesn't write
    // to the developer's real `~/.rupu/auth.json` and doesn't
    // trigger the macOS keychain (which would hang `cargo test` on
    // an "Always Allow" prompt).
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_file = tmp.path().join("auth.json");
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_AUTH_BACKEND", "file")
        .env("RUPU_AUTH_FILE", &auth_file)
        .args([
            "auth",
            "login",
            "--provider",
            "anthropic",
            "--mode",
            "api-key",
            "--key",
            "sk-test-flag-only",
        ])
        .assert()
        .success();
}

#[test]
fn login_sso_without_browser_errors_on_headless_linux() {
    // Only meaningful on Linux without DISPLAY; skipped elsewhere.
    if std::env::var_os("DISPLAY").is_some() || cfg!(not(target_os = "linux")) {
        return;
    }
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env_remove("DISPLAY").env_remove("BROWSER").args([
        "auth",
        "login",
        "--provider",
        "anthropic",
        "--mode",
        "sso",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("requires a desktop browser"));
}

/// The stored credentials in `<home>/auth.json`, keyed `<account>/<mode>`.
fn stored_keys(home: &std::path::Path) -> Vec<String> {
    let raw = std::fs::read_to_string(home.join("auth.json")).unwrap();
    let map: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&raw).unwrap();
    map.into_keys().collect()
}

/// A GitHub device-code endpoint pair on `server` that grants at once.
fn github_device_mocks(server: &httpmock::MockServer) -> (httpmock::Mock<'_>, httpmock::Mock<'_>) {
    let device = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/login/device/code")
            .body_contains("client_id=178c6fc778ccc68e1d6a");
        then.status(200).json_body(serde_json::json!({
            "device_code": "dev-code",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 0,
        }));
    });
    let token = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/login/oauth/access_token")
            .body_contains("device_code=dev-code");
        then.status(200).json_body(serde_json::json!({
            "access_token": "ghp-test",
            "token_type": "bearer",
            "scope": "repo",
        }));
    });
    (device, token)
}

/// `rupu auth login --account github --mode device` against `server`.
fn github_device_login(home: &std::path::Path, server: &httpmock::MockServer) -> Command {
    let mut cmd = Command::cargo_bin("rupu").unwrap();
    cmd.env("RUPU_HOME", home)
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .env(
            "RUPU_DEVICE_DEVICE_URL_OVERRIDE",
            server.url("/login/device/code"),
        )
        .env(
            "RUPU_DEVICE_TOKEN_URL_OVERRIDE",
            server.url("/login/oauth/access_token"),
        )
        .env("RUPU_DEVICE_FAST_POLL", "1")
        .timeout(std::time::Duration::from_secs(30))
        .args(["auth", "login", "--account", "github", "--mode", "device"]);
    cmd
}

#[test]
fn login_device_for_a_vendor_without_a_headless_flow_refuses() {
    let tmp = assert_fs::TempDir::new().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .timeout(std::time::Duration::from_secs(30))
        .args(["auth", "login", "--account", "linear", "--mode", "device"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "linear has no device login; use --mode api-key",
        ));
    assert!(!tmp.path().join("auth.json").exists());
}

#[test]
fn login_device_for_an_openai_compatible_account_refuses_before_any_device_flow() {
    let tmp = assert_fs::TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.toml"),
        "[providers.oai-local]\nkind = \"openai-compatible\"\n\
         base_url = \"http://127.0.0.1:9\"\ndefault_model = \"m\"\n",
    )
    .unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .timeout(std::time::Duration::from_secs(30))
        .args([
            "auth",
            "login",
            "--account",
            "oai-local",
            "--mode",
            "device",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "openai-compatible providers only support --mode api-key",
        ));
    assert!(!tmp.path().join("auth.json").exists());
}

#[test]
fn login_device_for_anthropic_reads_the_code_from_a_pipe() {
    let server = httpmock::MockServer::start();
    // A bare code: the test can't see the login's state, so the exchange is
    // matched on everything else.
    let exchange = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/token")
            .json_body_partial(r#"{"grant_type": "authorization_code"}"#)
            .json_body_partial(r#"{"code": "abc"}"#)
            .json_body_partial(
                r#"{"redirect_uri": "https://platform.claude.com/oauth/code/callback"}"#,
            );
        then.status(200).json_body(serde_json::json!({
            "access_token": "sk-ant-oat01-x",
            "refresh_token": "rt",
            "expires_in": 28800,
            "account": { "uuid": "acc" },
            "organization": { "uuid": "org" },
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .env("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"))
        .write_stdin("abc\n")
        .timeout(std::time::Duration::from_secs(30))
        .args([
            "auth",
            "login",
            "--account",
            "anthropic",
            "--mode",
            "device",
        ])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("claude.ai/oauth/authorize")
                .and(predicate::str::contains("Paste the code"))
                .and(predicate::str::contains("sk-ant-oat01-x").not()),
        )
        .stdout(predicate::str::contains("stored anthropic sso credential"));

    exchange.assert_hits(1);
    assert_eq!(stored_keys(tmp.path()), ["anthropic/sso"]);
}

#[test]
fn login_device_for_github_runs_the_device_code_flow_and_stores_sso() {
    let server = httpmock::MockServer::start();
    let (device, token) = github_device_mocks(&server);
    let tmp = assert_fs::TempDir::new().unwrap();

    github_device_login(tmp.path(), &server)
        .assert()
        .success()
        .stderr(
            predicate::str::contains("enter code: ABCD-1234")
                .and(predicate::str::contains("Waiting for approval")),
        )
        .stdout(
            predicate::str::contains("stored github sso credential")
                .and(predicate::str::contains("enter code").not()),
        );

    device.assert_hits(1);
    token.assert_hits(1);
    assert_eq!(stored_keys(tmp.path()), ["github/sso"]);
}

#[test]
fn logout_device_removes_the_sso_entry() {
    let server = httpmock::MockServer::start();
    let _mocks = github_device_mocks(&server);
    let tmp = assert_fs::TempDir::new().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .args([
            "auth",
            "login",
            "--account",
            "anthropic",
            "--mode",
            "api-key",
            "--key",
            "sk-x",
        ])
        .assert()
        .success();
    github_device_login(tmp.path(), &server).assert().success();
    assert_eq!(stored_keys(tmp.path()), ["anthropic/api-key", "github/sso"]);

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .args(["auth", "logout", "--account", "github", "--mode", "device"])
        .assert()
        .success();
    assert_eq!(stored_keys(tmp.path()), ["anthropic/api-key"]);
}

#[test]
fn login_sso_headless_refusal_names_device_mode() {
    // Only meaningful on Linux without DISPLAY; skipped elsewhere.
    if std::env::var_os("DISPLAY").is_some() || cfg!(not(target_os = "linux")) {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("DISPLAY")
        .env_remove("BROWSER")
        .env_remove("RUPU_OAUTH_SKIP_BROWSER")
        .args(["auth", "login", "--account", "anthropic", "--mode", "sso"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Run with --mode device to log in from any browser, or --mode api-key.",
        ));
}

#[test]
fn login_device_for_openai_stores_an_sso_credential() {
    let server = httpmock::MockServer::start();
    let usercode = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/api/accounts/deviceauth/usercode")
            .json_body(serde_json::json!({ "client_id": "app_EMoamEEZ73f0CkXaXp7hrann" }));
        then.status(200).json_body(serde_json::json!({
            "device_auth_id": "da-1",
            "user_code": "ABCD-1234",
            "interval": "0",
        }));
    });
    let poll = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/api/accounts/deviceauth/token")
            .json_body(serde_json::json!({ "device_auth_id": "da-1", "user_code": "ABCD-1234" }));
        then.status(200).json_body(serde_json::json!({
            "authorization_code": "ac-1",
            "code_verifier": "srv-verifier",
            "code_challenge": "srv-challenge",
        }));
    });
    let exchange = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/oauth/token")
            .body_contains("code=ac-1")
            .body_contains("code_verifier=srv-verifier");
        then.status(200).json_body(serde_json::json!({
            "access_token": "at",
            "refresh_token": "rt",
            "id_token": "idt",
            "expires_in": 3600,
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();

    Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("RUPU_AUTH_BACKEND")
        .env("RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE", server.base_url())
        .timeout(std::time::Duration::from_secs(30))
        .args(["auth", "login", "--account", "openai", "--mode", "device"])
        .assert()
        .success()
        .stderr(
            predicate::str::contains(format!("{}/codex/device", server.base_url()))
                .and(predicate::str::contains("enter code: ABCD-1234"))
                .and(predicate::str::contains("da-1").not())
                .and(predicate::str::contains("ac-1").not()),
        )
        .stdout(predicate::str::contains("stored openai sso credential"));

    usercode.assert_hits(1);
    poll.assert_hits(1);
    exchange.assert_hits(1);
    assert_eq!(stored_keys(tmp.path()), ["openai/sso"]);
    let auth: std::collections::HashMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("auth.json")).unwrap())
            .unwrap();
    let stored: serde_json::Value = serde_json::from_str(&auth["openai/sso"]).unwrap();
    assert_eq!(stored["credentials"]["id_token"], "idt", "{stored}");
}
