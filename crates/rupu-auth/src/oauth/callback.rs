//! PKCE browser-callback OAuth flow. Anthropic, OpenAI, Gemini.
//!
//! 1. Generate PKCE pair + state nonce.
//! 2. Bind localhost listener (port 0 -> OS picks).
//! 3. Open browser to authorize URL with redirect_uri pointing at us.
//! 4. Receive redirect; validate state; exchange code at token URL.
//! 5. Return StoredCredential.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use rand::RngCore;
use rupu_providers::auth::AuthCredentials;
use serde::Deserialize;
use tracing::{debug, info};

use crate::backend::ProviderId;
use crate::oauth::pkce::PkcePair;
use crate::oauth::providers::{provider_oauth, OAuthFlow};
use crate::stored::StoredCredential;

const CALLBACK_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub async fn run(provider: ProviderId) -> Result<StoredCredential> {
    let oauth =
        provider_oauth(provider).ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    if oauth.flow != OAuthFlow::Callback {
        anyhow::bail!("provider {provider} does not use the callback flow");
    }

    // Headless detection: error early on Linux without DISPLAY/BROWSER.
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("BROWSER").is_none()
        && std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_none()
    {
        anyhow::bail!(
            "SSO requires a desktop browser. \
             Run with --mode api-key for headless setups."
        );
    }

    let pkce = PkcePair::generate();
    let state = random_state();

    // For tests, expose the state so the test driver can craft the redirect.
    if std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_some() {
        // SAFETY: test-only seam; single-threaded in integration tests.
        std::env::set_var("RUPU_OAUTH_LAST_STATE", &state);
    }

    // Bind the listener on port 0 (OS picks).
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow!("bind localhost listener: {e}"))?;

    // Discover the bound port via tiny_http's ListenAddr.
    let bound_port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("listener bound to unexpected address type"))?
        .port();

    let redirect_uri = format!("http://127.0.0.1:{bound_port}{}", oauth.redirect_path);

    // Test seam: write the port to a file so the test harness can discover it.
    if let Ok(path) = std::env::var("RUPU_OAUTH_PORT_FILE") {
        std::fs::write(&path, bound_port.to_string())
            .with_context(|| format!("write port file {path}"))?;
    }

    // Build authorize URL.
    let authorize = build_authorize_url(&oauth, &pkce.challenge, &state, &redirect_uri)?;
    if std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_none() {
        info!("opening browser to {}", authorize);
        if webbrowser::open(&authorize).is_err() {
            eprintln!(
                "Could not open a browser automatically. \
                 Open this URL in a browser to continue:\n\n  {authorize}\n"
            );
        }
    } else {
        debug!("RUPU_OAUTH_SKIP_BROWSER set; not launching browser");
    }

    // Wait for the redirect on a blocking task (tiny_http is sync).
    let server = Arc::new(server);
    let redirect_path = oauth.redirect_path.to_string();
    let recv = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
        loop {
            let req = server
                .recv_timeout(Duration::from_secs(CALLBACK_TIMEOUT_SECS))
                .map_err(|e| anyhow!("listener recv error: {e}"))?
                .ok_or_else(|| anyhow!("oauth callback timed out"))?;

            let url = req.url().to_string();

            // Accept both "/callback?..." and "/callback" (exact match edge case).
            let path_matches =
                url.starts_with(&format!("{}?", redirect_path)) || url == redirect_path;
            if !path_matches {
                let _ = req
                    .respond(tiny_http::Response::from_string("not found").with_status_code(404));
                continue;
            }

            let parsed = url::Url::parse(&format!("http://localhost{url}"))
                .map_err(|e| anyhow!("parse callback url: {e}"))?;

            let code = parsed
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.into_owned())
                .ok_or_else(|| anyhow!("no `code` in redirect"))?;

            let got_state = parsed
                .query_pairs()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.into_owned())
                .ok_or_else(|| anyhow!("no `state` in redirect"))?;

            let resp = tiny_http::Response::from_string(
                "<html><body>Authentication complete — return to your terminal.</body></html>",
            )
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap(),
            );
            let _ = req.respond(resp);
            return Ok((code, got_state));
        }
    });

    let (code, got_state) = recv.await??;

    if got_state != state {
        anyhow::bail!("state mismatch: possible replay or CSRF attempt");
    }

    // Exchange the code.
    let token_url = std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
        .unwrap_or_else(|_| oauth.token_url.to_string());

    let client = reqwest::Client::new();
    let body = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("client_id", oauth.client_id),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", pkce.verifier.as_str()),
    ];

    let token: TokenResponse = client
        .post(&token_url)
        .form(&body)
        .send()
        .await
        .context("token exchange request")?
        .error_for_status()
        .context("token exchange status")?
        .json()
        .await
        .context("token exchange json")?;

    let expires_at = token
        .expires_in
        .map(|s| Utc::now() + chrono::Duration::seconds(s));

    let expires_ms = expires_at.map(|d| d.timestamp_millis() as u64).unwrap_or(0);

    Ok(StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: token.access_token,
            refresh: token.refresh_token.clone().unwrap_or_default(),
            expires: expires_ms,
            extra: Default::default(),
        },
        refresh_token: token.refresh_token,
        expires_at,
    })
}

fn build_authorize_url(
    oauth: &crate::oauth::providers::ProviderOAuth,
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<String> {
    let mut url = url::Url::parse(oauth.authorize_url)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", oauth.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &oauth.scopes.join(" "))
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}
