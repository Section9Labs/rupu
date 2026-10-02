//! Integration test for the PKCE callback flow.
//!
//! Drives the listener directly — no real browser — by:
//!   1. Setting test seam env vars before spawning.
//!   2. Polling the port file until the listener is ready.
//!   3. POSTing the redirect URL with a matching state directly to the listener.
//!   4. Letting the flow exchange the code against an httpmock'd token endpoint.
//!
//! The seams are process-wide env vars, so the tests are `#[serial]`.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::callback;
use rupu_auth::oauth::providers::OAuthClient;
use rupu_auth::stored::StoredCredential;
use serial_test::serial;

/// RAII guard: sets or clears an env var for the test's duration and
/// restores whatever was there on drop, even on panic.
struct EnvVarGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let prior = std::env::var(key).ok();
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
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

/// Run the callback flow for `provider` against a token endpoint mocked by
/// `token`, driving the redirect by hand. Returns the stored credential and
/// the mock (for hit assertions).
async fn run_flow(
    provider: ProviderId,
    token: impl FnOnce(httpmock::When, httpmock::Then),
) -> (StoredCredential, httpmock::Mock<'static>) {
    // Leaked on purpose: the mock's lifetime is tied to the server's, and
    // the caller asserts on it after the flow returns.
    let server: &'static MockServer = Box::leak(Box::new(MockServer::start()));
    let token_mock = server.mock(token);
    let stored = drive("/callback", &server.url("/token"), callback::run(provider)).await;
    (stored, token_mock)
}

/// Drive a started callback `flow` through its redirect: wait for its
/// listener, hit `redirect_path` with the state it exposed, and return what
/// it stored. `token_url_override` is the built-in endpoints' test seam.
async fn drive(
    redirect_path: &str,
    token_url_override: &str,
    flow: impl std::future::Future<Output = anyhow::Result<StoredCredential>> + Send + 'static,
) -> StoredCredential {
    // Each test run uses a unique suffix to avoid env-var collisions when
    // tests run in parallel.
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos()
        .to_string();

    let port_file = std::env::temp_dir().join(format!("rupu-oauth-test-port-{suffix}.txt"));
    let _ = std::fs::remove_file(&port_file);

    // ── Set all env vars before the flow spawns ──────────────────────────
    std::env::remove_var("RUPU_OAUTH_LAST_STATE");
    std::env::set_var("RUPU_OAUTH_SKIP_BROWSER", "1");
    std::env::set_var("RUPU_OAUTH_PORT_FILE", &port_file);

    std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", token_url_override);

    // ── Spawn the flow ───────────────────────────────────────────────────
    let flow_handle = tokio::spawn(flow);

    // ── Poll for the port file ───────────────────────────────────────────
    let mut bound_port = 0u16;
    for _ in 0..200 {
        if let Ok(s) = std::fs::read_to_string(&port_file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                bound_port = p;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(bound_port > 0, "listener never wrote port file");

    // ── Read the state the flow exposed ─────────────────────────────────
    // Small spin to let the flow write RUPU_OAUTH_LAST_STATE after the port file.
    let mut state = String::new();
    for _ in 0..40 {
        if let Ok(s) = std::env::var("RUPU_OAUTH_LAST_STATE") {
            if !s.is_empty() {
                state = s;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(!state.is_empty(), "flow never set RUPU_OAUTH_LAST_STATE");

    // ── POST the fake redirect ───────────────────────────────────────────
    let url = format!("http://127.0.0.1:{bound_port}{redirect_path}?code=stub-code&state={state}");
    let _resp = reqwest::get(&url)
        .await
        .expect("redirect GET should succeed");

    // ── Collect the result ───────────────────────────────────────────────
    let stored = flow_handle.await.unwrap().expect("flow should return Ok");

    // ── Cleanup ──────────────────────────────────────────────────────────
    std::env::remove_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE");
    std::env::remove_var("RUPU_OAUTH_SKIP_BROWSER");
    std::env::remove_var("RUPU_OAUTH_PORT_FILE");
    std::env::remove_var("RUPU_OAUTH_LAST_STATE");
    let _ = std::fs::remove_file(&port_file);
    stored
}

#[tokio::test]
#[serial]
async fn callback_completes_with_mocked_token_endpoint() {
    let (stored, token_mock) = run_flow(ProviderId::Anthropic, |when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "test-access",
                "refresh_token": "test-refresh",
                "expires_in": 3600,
                "token_type": "bearer",
            }));
    })
    .await;

    token_mock.assert();
    assert!(
        stored.refresh_token.is_some(),
        "refresh_token must be present"
    );
    assert!(stored.expires_at.is_some(), "expires_at must be present");
}

/// Google's token endpoint wants the installed app's `client_secret` on the
/// code exchange, next to the `client_id` (gemini-cli constructs its
/// `OAuth2Client` with both, and google-auth-library posts `client_secret`
/// form-encoded on `getToken`). The flow sends the Gemini CLI pair.
#[tokio::test]
#[serial]
async fn gemini_login_sends_the_client_secret_on_the_code_exchange() {
    let (stored, token_mock) = run_flow(ProviderId::Gemini, |when, then| {
        when.method(POST)
            .path("/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body_contains("grant_type=authorization_code")
            .body_contains("code=stub-code")
            .body_contains(
                "client_id=681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com",
            )
            .body_contains("client_secret=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");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "g-access",
                "refresh_token": "g-refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
            }));
    })
    .await;

    token_mock.assert();
    assert_eq!(stored.refresh_token.as_deref(), Some("g-refresh"));
}

/// Gemini requests `openid` + `email`, so Google's token response carries
/// an ID token with the user's email in it. Only OpenAI's ID token is
/// persisted (the Codex client needs it for its account id); Gemini's is
/// not stored — the same PII rule that keeps Anthropic's account block
/// down to the uuid.
#[tokio::test]
#[serial]
async fn gemini_login_does_not_persist_the_id_token() {
    let (stored, token_mock) = run_flow(ProviderId::Gemini, |when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "g-access",
                "refresh_token": "g-refresh",
                "id_token": "eyJ.email-bearing.id-token",
                "expires_in": 3600,
                "token_type": "Bearer",
            }));
    })
    .await;

    token_mock.assert();
    match &stored.credentials {
        rupu_providers::auth::AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "g-access");
            assert!(
                !extra.contains_key("id_token"),
                "Gemini's ID token is not stored: {extra:?}"
            );
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    let json = serde_json::to_string(&stored).unwrap();
    assert!(!json.contains("id-token"), "{json}");
}

/// A Google token response, as the code exchange returns it.
fn google_token(when: httpmock::When, then: httpmock::Then) {
    when.method(POST).path("/token");
    then.status(200)
        .header("content-type", "application/json")
        .json_body(serde_json::json!({
            "access_token": "g-access",
            "refresh_token": "g-refresh",
            "expires_in": 3600,
            "token_type": "Bearer",
        }));
}

fn oauth_extra(stored: &StoredCredential) -> &std::collections::HashMap<String, serde_json::Value> {
    match &stored.credentials {
        rupu_providers::auth::AuthCredentials::OAuth { extra, .. } => extra,
        other => panic!("expected OAuth, got {other:?}"),
    }
}

/// The browser flow records which OAuth client a Gemini token belongs to
/// (`rupu auth login` signs in as the Gemini CLI client) and makes no Code
/// Assist call: the project is set up once the credential is stored.
#[tokio::test]
#[serial]
async fn a_gemini_login_records_the_variant_its_token_belongs_to() {
    let server: &'static MockServer = Box::leak(Box::new(MockServer::start()));
    let token = server.mock(google_token);
    let code_assist = server.mock(|when, then| {
        when.path_contains("/v1internal");
        then.status(500);
    });
    let _code_assist =
        EnvVarGuard::set("RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE", Some(&server.url("")));

    let stored = drive(
        "/callback",
        &server.url("/token"),
        callback::run(ProviderId::Gemini),
    )
    .await;

    token.assert();
    code_assist.assert_hits(0);
    let extra = oauth_extra(&stored);
    assert_eq!(extra["variant"], serde_json::json!("gemini-cli"));
    assert!(!extra.contains_key("project_id"), "{extra:?}");
}

/// A Gemini login end to end, as `rupu auth login` runs it: the flow, the
/// store, then gemini-cli's Code Assist setup with the new token — the
/// project lands in the stored credential, and a refresh keeps it.
#[tokio::test]
#[serial]
async fn a_gemini_login_stores_the_code_assist_project_which_survives_a_refresh() {
    let server: &'static MockServer = Box::leak(Box::new(MockServer::start()));
    let token = server.mock(google_token);
    let load = server.mock(|when, then| {
        when.method(POST)
            .path("/v1internal:loadCodeAssist")
            .header("authorization", "Bearer g-access")
            .json_body(serde_json::json!({
                "metadata": {
                    "ideType": "IDE_UNSPECIFIED",
                    "platform": "PLATFORM_UNSPECIFIED",
                    "pluginType": "GEMINI",
                },
            }));
        then.status(200).json_body(serde_json::json!({
            "currentTier": { "id": "free-tier" },
            "cloudaicompanionProject": "managed-123",
        }));
    });
    let tmp = assert_fs::TempDir::new().unwrap();
    let auth_path = tmp.path().join("auth.json");
    let _file = EnvVarGuard::set("RUPU_AUTH_FILE", auth_path.to_str());
    let _code_assist =
        EnvVarGuard::set("RUPU_CODE_ASSIST_ENDPOINT_OVERRIDE", Some(&server.url("")));
    let _project = EnvVarGuard::set("GOOGLE_CLOUD_PROJECT", None);
    let _project_id = EnvVarGuard::set("GOOGLE_CLOUD_PROJECT_ID", None);

    let stored = drive(
        "/callback",
        &server.url("/token"),
        callback::run(ProviderId::Gemini),
    )
    .await;
    let resolver = rupu_auth::resolver::KeychainResolver::new();
    resolver
        .store_named("gemini", rupu_providers::AuthMode::Sso, &stored)
        .await
        .unwrap();
    rupu_auth::oauth::gemini::set_up_code_assist(&resolver, "gemini", &stored).await;

    load.assert_hits(1);
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(saved.contains("managed-123"), "{saved}");

    let _url = EnvVarGuard::set("RUPU_OAUTH_TOKEN_URL_OVERRIDE", Some(&server.url("/token")));
    let refreshed = rupu_auth::resolver::CredentialResolver::refresh(
        &resolver,
        "gemini",
        rupu_providers::AuthMode::Sso,
    )
    .await
    .unwrap();
    token.assert_hits(2);
    match refreshed {
        rupu_providers::auth::AuthCredentials::OAuth { extra, .. } => {
            assert_eq!(extra["project_id"], serde_json::json!("managed-123"));
            assert_eq!(extra["variant"], serde_json::json!("gemini-cli"));
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    let saved = std::fs::read_to_string(&auth_path).unwrap();
    assert!(saved.contains("managed-123"), "{saved}");
}

/// A GitLab login with a chosen application — a self-managed instance's, or
/// a configured one — exchanges the code at that instance's token endpoint
/// as that application, and records both on the credential: a refresh
/// token is only good with the application that issued it, at the endpoint
/// that issued it.
#[tokio::test]
#[serial]
async fn a_gitlab_login_uses_and_records_the_chosen_application() {
    let server: &'static MockServer = Box::leak(Box::new(MockServer::start()));
    let token_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/oauth/token")
            .header("content-type", "application/x-www-form-urlencoded")
            .body_contains("grant_type=authorization_code")
            .body_contains("code=stub-code")
            .body_contains("client_id=corp-app")
            .body_contains("%2Fauth%2Fredirect");
        then.status(200)
            .header("content-type", "application/json")
            .json_body(serde_json::json!({
                "access_token": "gl-access",
                "refresh_token": "gl-refresh",
                "expires_in": 7200,
                "token_type": "Bearer",
            }));
    });
    let client = OAuthClient {
        client_id: "corp-app".into(),
        authorize_url: server.url("/oauth/authorize"),
        token_url: server.url("/oauth/token"),
    };
    // glab's application is registered on port 7171; any free port will do
    // here.
    std::env::set_var("RUPU_OAUTH_FORCE_PORT", "0");
    // The built-in endpoints' seam points elsewhere: the chosen
    // application's endpoint is the one that must be used.
    let stored = drive(
        "/auth/redirect",
        &server.url("/not-this-one"),
        callback::run_with_client(ProviderId::Gitlab, Some(client)),
    )
    .await;
    std::env::remove_var("RUPU_OAUTH_FORCE_PORT");

    token_mock.assert();
    let rupu_providers::auth::AuthCredentials::OAuth { access, extra, .. } = stored.credentials
    else {
        panic!("expected an OAuth credential");
    };
    assert_eq!(access, "gl-access");
    assert_eq!(extra["oauth_client_id"], "corp-app");
    assert_eq!(
        extra["oauth_token_url"],
        server.url("/oauth/token").as_str()
    );
}
