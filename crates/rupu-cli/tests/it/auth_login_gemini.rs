//! `rupu auth login --provider gemini --mode sso` stores the sign-in, then
//! sets up Gemini Code Assist with the new token and records the account's
//! Google Cloud project in the stored credential.
//!
//! Drives the real binary through the browser flow with no browser: the
//! listener's port and state come from the `RUPU_OAUTH_PORT_FILE` /
//! `RUPU_OAUTH_STATE_FILE` seams, the token exchange and Code Assist go to
//! an httpmock server, and the redirect is a raw GET to the listener.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The contents of `path` once something non-empty is written there.
fn wait_for_file(path: &Path, what: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
        assert!(
            Instant::now() < deadline,
            "the login never wrote its {what}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn a_gemini_sso_login_stores_the_code_assist_project() {
    let server = httpmock::MockServer::start();
    let token = server.mock(|when, then| {
        when.method(httpmock::Method::POST).path("/token");
        then.status(200).json_body(serde_json::json!({
            "access_token": "g-access",
            "refresh_token": "g-refresh",
            "expires_in": 3600,
            "token_type": "Bearer",
        }));
    });
    let load = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1internal:loadCodeAssist")
            .header("authorization", "Bearer g-access");
        then.status(200).json_body(serde_json::json!({
            "currentTier": { "id": "free-tier" },
            "cloudaicompanionProject": "managed-cli",
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let port_file = tmp.path().join("oauth-port");
    let state_file = tmp.path().join("oauth-state");

    let child = Command::new(assert_cmd::cargo::cargo_bin("rupu"))
        .env("RUPU_HOME", tmp.path())
        .env_remove("RUPU_AUTH_FILE")
        .env_remove("GOOGLE_CLOUD_PROJECT")
        .env_remove("GOOGLE_CLOUD_PROJECT_ID")
        .env("RUPU_OAUTH_SKIP_BROWSER", "1")
        .env("RUPU_OAUTH_PORT_FILE", &port_file)
        .env("RUPU_OAUTH_STATE_FILE", &state_file)
        .env("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"))
        .env("RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE", server.url(""))
        .args(["auth", "login", "--provider", "gemini", "--mode", "sso"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Killed on any early return or failed assertion below.
    struct KillOnDrop(Option<std::process::Child>);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
            }
        }
    }
    let mut child = KillOnDrop(Some(child));

    let port: u16 = wait_for_file(&port_file, "port").parse().unwrap();
    let state = wait_for_file(&state_file, "state");
    let mut redirect = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        redirect,
        "GET /callback?code=stub-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut landing = String::new();
    let _ = redirect.read_to_string(&mut landing);

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the login never finished");
        std::thread::sleep(Duration::from_millis(25));
    };
    let mut stderr = String::new();
    child
        .0
        .take()
        .unwrap()
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();

    assert!(status.success(), "{stderr}");
    token.assert_hits(1);
    load.assert_hits(1);
    assert!(
        stderr.contains("Gemini Code Assist project: managed-cli"),
        "{stderr}"
    );
    let auth: std::collections::HashMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join("auth.json")).unwrap())
            .unwrap();
    let stored: serde_json::Value = serde_json::from_str(&auth["gemini/sso"]).unwrap();
    assert_eq!(
        stored["credentials"]["project_id"], "managed-cli",
        "{stored}"
    );
    assert_eq!(stored["credentials"]["variant"], "gemini-cli", "{stored}");
}
