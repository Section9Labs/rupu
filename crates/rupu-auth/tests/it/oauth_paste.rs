//! The paste-the-code login (`--mode device` for Anthropic) against an
//! httpmock token endpoint: the authorize URL names Anthropic's hosted
//! callback page, the pasted `code#state` is exchanged with that redirect,
//! and a paste or exchange that goes wrong ends the login with no
//! credential. The token endpoint is the built-in app's
//! `RUPU_OAUTH_TOKEN_URL_OVERRIDE` seam, a process-wide env var, so the
//! tests are `#[serial]`.

use httpmock::prelude::*;
use rupu_auth::backend::ProviderId;
use rupu_auth::oauth::paste::{run_with_input, CodeInput};
use rupu_providers::auth::AuthCredentials;
use serial_test::serial;

const REDIRECT: &str = "https://platform.claude.com/oauth/code/callback";

/// A [`CodeInput`] over a closure: what the operator pastes, given the URL
/// shown and the state the login sent.
struct Paste<F>(F);

impl<F: FnMut(&str, &str) -> anyhow::Result<String>> CodeInput for Paste<F> {
    fn code(&mut self, authorize_url: &str, state: &str) -> anyhow::Result<String> {
        (self.0)(authorize_url, state)
    }
}

/// Points the built-in app's token endpoint at `server` for the test's
/// duration, restoring what was there on drop.
struct TokenUrl(Option<String>);

impl TokenUrl {
    fn at(server: &MockServer) -> Self {
        let prior = std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE").ok();
        std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", server.url("/token"));
        Self(prior)
    }
}

impl Drop for TokenUrl {
    fn drop(&mut self) {
        match &self.0 {
            Some(v) => std::env::set_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE", v),
            None => std::env::remove_var("RUPU_OAUTH_TOKEN_URL_OVERRIDE"),
        }
    }
}

/// Anthropic's exchange of code `abc` for a login that sent `state`: JSON,
/// the hosted redirect, the state as the PKCE verifier and in the body.
fn exchange_mock<'a>(server: &'a MockServer, state: &str) -> httpmock::Mock<'a> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": "abc",
        "client_id": "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        "redirect_uri": REDIRECT,
        "code_verifier": state,
        "state": state,
    });
    server.mock(move |when, then| {
        when.method(POST).path("/token").json_body(body);
        then.status(200).json_body(serde_json::json!({
            "access_token": "sk-ant-oat01-x",
            "refresh_token": "rt",
            "expires_in": 28800,
            "account": { "uuid": "acc" },
            "organization": { "uuid": "org" },
        }));
    })
}

#[tokio::test]
#[serial]
async fn anthropic_paste_login_exchanges_with_the_hosted_redirect() {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let mut exchange = None;
    let stored = {
        let mut input = Paste(|url: &str, state: &str| {
            assert!(
                url.starts_with("https://claude.ai/oauth/authorize?"),
                "{url}"
            );
            assert!(url.contains("code=true"), "{url}");
            assert!(
                url.contains(
                    "redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"
                ),
                "{url}"
            );
            exchange = Some(exchange_mock(&server, state));
            Ok(format!("abc#{state}"))
        });

        run_with_input(ProviderId::Anthropic, None, &mut input)
            .await
            .expect("login")
    };

    match &stored.credentials {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "sk-ant-oat01-x");
            assert_eq!(extra["account_uuid"], "acc");
            assert_eq!(extra["organization_uuid"], "org");
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
    assert_eq!(stored.refresh_token.as_deref(), Some("rt"));
    exchange.expect("the input ran").assert_hits(1);
}

#[tokio::test]
#[serial]
async fn a_mismatched_pasted_state_is_refused_before_any_exchange() {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .json_body(serde_json::json!({ "access_token": "at" }));
    });
    let mut input = Paste(|_: &str, _: &str| Ok("abc#not-the-state".to_string()));

    let err = run_with_input(ProviderId::Anthropic, None, &mut input)
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("pasted state does not match this login"),
        "{err}"
    );
    assert!(!err.contains("not-the-state"), "{err}");
    token.assert_hits(0);
}

/// A pasted redirect URL carrying its state in the fragment
/// (`…/callback?code=abc#state`) has that state compared too.
#[tokio::test]
#[serial]
async fn a_pasted_url_s_fragment_state_is_compared() {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(200)
            .json_body(serde_json::json!({ "access_token": "at" }));
    });
    let mut input = Paste(|_: &str, _: &str| {
        Ok("https://platform.claude.com/oauth/code/callback?code=abc#not-the-state".to_string())
    });

    let err = run_with_input(ProviderId::Anthropic, None, &mut input)
        .await
        .unwrap_err()
        .to_string();

    assert!(
        err.contains("pasted state does not match this login"),
        "{err}"
    );
    token.assert_hits(0);
}

#[tokio::test]
#[serial]
async fn a_bare_code_proceeds_with_rupu_s_own_state() {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let mut exchange = None;
    {
        let mut input = Paste(|_: &str, state: &str| {
            exchange = Some(exchange_mock(&server, state));
            Ok("abc\n".to_string())
        });
        run_with_input(ProviderId::Anthropic, None, &mut input)
            .await
            .expect("login");
    }

    exchange.expect("the input ran").assert_hits(1);
}

#[tokio::test]
#[serial]
async fn a_failed_exchange_surfaces_the_vendor_message_and_stores_nothing() {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let token = server.mock(|when, then| {
        when.method(POST).path("/token");
        then.status(400).json_body(serde_json::json!({
            "type": "error",
            "error": { "type": "invalid_grant", "message": "Invalid code" },
        }));
    });
    let mut input = Paste(|_: &str, state: &str| Ok(format!("abc#{state}")));

    let err = run_with_input(ProviderId::Anthropic, None, &mut input)
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("Invalid code"), "{err}");
    token.assert_hits(1);
}

const GEMINI_REDIRECT: &str = "https://codeassist.google.com/authcode";

#[tokio::test]
#[serial]
async fn gemini_paste_login_sends_consent_params_and_the_client_secret() {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let token = server.mock(|when, then| {
        when.method(POST)
            .path("/token")
            .x_www_form_urlencoded_tuple("grant_type", "authorization_code")
            .x_www_form_urlencoded_tuple("code", "4/0Acode")
            .x_www_form_urlencoded_tuple("redirect_uri", GEMINI_REDIRECT)
            .x_www_form_urlencoded_tuple(
                "client_secret",
                rupu_providers::google_gemini::GEMINI_CLI_CLIENT_SECRET,
            )
            .x_www_form_urlencoded_key_exists("code_verifier");
        then.status(200).json_body(serde_json::json!({
            "access_token": "ya29",
            "refresh_token": "1//rt",
            "expires_in": 3599,
            "id_token": "x",
        }));
    });
    let stored = {
        let mut input = Paste(|url: &str, _: &str| {
            assert!(
                url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"),
                "{url}"
            );
            assert!(url.contains("access_type=offline"), "{url}");
            assert!(url.contains("prompt=consent"), "{url}");
            assert!(
                url.contains("redirect_uri=https%3A%2F%2Fcodeassist.google.com%2Fauthcode"),
                "{url}"
            );
            assert!(url.contains("code_challenge_method=S256"), "{url}");
            Ok("4/0Acode".to_string())
        });
        run_with_input(ProviderId::Gemini, None, &mut input)
            .await
            .expect("login")
    };

    token.assert_hits(1);
    assert_eq!(stored.refresh_token.as_deref(), Some("1//rt"));
    match &stored.credentials {
        AuthCredentials::OAuth { access, extra, .. } => {
            assert_eq!(access, "ya29");
            assert_eq!(
                extra[rupu_providers::google_gemini::code_assist::EXTRA_VARIANT],
                "gemini-cli"
            );
            assert!(!extra.contains_key("id_token"), "{extra:?}");
        }
        other => panic!("expected OAuth, got {other:?}"),
    }
}

async fn gemini_login_with_reply(reply: serde_json::Value) -> String {
    let server = MockServer::start();
    let _url = TokenUrl::at(&server);
    let token = server.mock(move |when, then| {
        when.method(POST).path("/token");
        then.status(200).json_body(reply);
    });
    let mut input = Paste(|_: &str, _: &str| Ok("4/0Acode".to_string()));
    let err = run_with_input(ProviderId::Gemini, None, &mut input)
        .await
        .unwrap_err()
        .to_string();
    token.assert_hits(1);
    err
}

#[tokio::test]
#[serial]
async fn a_gemini_reply_without_a_refresh_token_is_refused() {
    let err =
        gemini_login_with_reply(serde_json::json!({"access_token": "ya29", "expires_in": 3599}))
            .await;
    assert_eq!(err, "Google returned no refresh token. Revoke \"Gemini CLI\" at https://myaccount.google.com/permissions and log in again, or use --mode api-key");
}

#[tokio::test]
#[serial]
async fn a_gemini_reply_with_an_empty_refresh_token_is_refused() {
    let err = gemini_login_with_reply(
        serde_json::json!({"access_token": "ya29", "refresh_token": "", "expires_in": 3599}),
    )
    .await;
    assert_eq!(err, "Google returned no refresh token. Revoke \"Gemini CLI\" at https://myaccount.google.com/permissions and log in again, or use --mode api-key");
    assert!(!err.contains("ya29"), "{err}");
}
