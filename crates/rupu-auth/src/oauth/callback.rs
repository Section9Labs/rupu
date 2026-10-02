//! PKCE browser-callback OAuth flow. Anthropic, OpenAI, Gemini, GitLab.
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
use crate::oauth::providers::{
    provider_oauth, OAuthClient, OAuthFlow, TokenBodyFormat, EXTRA_CLIENT_ID, EXTRA_TOKEN_URL,
};
use crate::stored::StoredCredential;

const CALLBACK_TIMEOUT_SECS: u64 = 300;

/// Branded post-callback landing page served once the redirect lands.
/// Inlined at compile time so the OAuth flow has no external file deps.
const LANDING_PAGE: &str = include_str!("landing.html");

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    /// OpenAI's ChatGPT grant returns an ID token next to the access token;
    /// stored (`extra.id_token`) as codex-rs stores it, so the Codex client
    /// can take its account id from it when the access token has no claim.
    /// Stored for OpenAI only ([`stored_credential_for`]): Gemini's
    /// (`openid` + `email` scopes) carries the user's email and is dropped.
    #[serde(default)]
    id_token: Option<String>,
    /// Anthropic-shaped account block (uuid, email, …). Optional — other
    /// OAuth providers don't return this. We capture the uuid only; other
    /// fields are intentionally ignored to avoid storing extra PII.
    #[serde(default)]
    account: Option<AccountInfo>,
    /// Anthropic-shaped organization block (uuid, name). Same rationale.
    #[serde(default)]
    organization: Option<OrganizationInfo>,
}

#[derive(Debug, Deserialize)]
struct AccountInfo {
    #[serde(default)]
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OrganizationInfo {
    #[serde(default)]
    uuid: Option<String>,
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub async fn run(provider: ProviderId) -> Result<StoredCredential> {
    run_with_client(provider, None).await
}

/// [`run`] as `client` — an application chosen at login (a GitLab account's
/// configured one, or a self-managed instance's) — instead of the
/// provider's built-in one. The credential records the chosen application's
/// id and token endpoint ([`EXTRA_CLIENT_ID`], [`EXTRA_TOKEN_URL`]) so its
/// refreshes go there; a chosen application is a public client, sent no
/// secret, and its endpoint is never redirected by
/// `RUPU_OAUTH_TOKEN_URL_OVERRIDE` (the built-in endpoints' test seam).
pub async fn run_with_client(
    provider: ProviderId,
    client: Option<OAuthClient>,
) -> Result<StoredCredential> {
    let oauth =
        provider_oauth(provider).ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    if oauth.flow != OAuthFlow::Callback {
        anyhow::bail!("provider {provider} does not use the callback flow");
    }
    let chosen = client.is_some();
    let app = client.unwrap_or_else(|| OAuthClient {
        client_id: oauth.client_id.to_string(),
        authorize_url: oauth.authorize_url.to_string(),
        token_url: std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| oauth.token_url.to_string()),
    });
    let client_secret = if chosen { None } else { oauth.client_secret };

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
    // Anthropic's OAuth server appears to validate that the state on
    // the authorize URL matches the verifier produced for the code
    // challenge — pi-mono's anthropic.ts mirrors this. Other providers
    // use an independent random state nonce.
    let state = if oauth.state_is_verifier {
        pkce.verifier.clone()
    } else {
        random_state()
    };

    // For tests, expose the state so the test driver can craft the redirect:
    // in this process's env (an in-process driver), or in a file (a driver
    // running the `rupu` binary as a child, which can't read its env).
    if std::env::var_os("RUPU_OAUTH_SKIP_BROWSER").is_some() {
        // SAFETY: test-only seam; single-threaded in integration tests.
        std::env::set_var("RUPU_OAUTH_LAST_STATE", &state);
        if let Ok(path) = std::env::var("RUPU_OAUTH_STATE_FILE") {
            std::fs::write(&path, &state).with_context(|| format!("write state file {path}"))?;
        }
    }

    // Bind the listener. Some IdPs (notably OpenAI Hydra) only allow
    // specific pre-registered ports; honor `fixed_ports` if set,
    // otherwise let the OS assign one. We always bind on 127.0.0.1
    // even when the redirect URI advertises "localhost" — that's a
    // hostname-in-the-URL question, not a bind question, and 127.0.0.1
    // is portable across name-resolution oddities.
    let server = bind_listener(oauth.fixed_ports)?;

    // Discover the bound port via tiny_http's ListenAddr.
    let bound_port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow!("listener bound to unexpected address type"))?
        .port();

    let redirect_uri = format!(
        "http://{}:{bound_port}{}",
        oauth.redirect_host, oauth.redirect_path
    );

    // Test seam: write the port to a file so the test harness can discover it.
    if let Ok(path) = std::env::var("RUPU_OAUTH_PORT_FILE") {
        std::fs::write(&path, bound_port.to_string())
            .with_context(|| format!("write port file {path}"))?;
    }

    // Build authorize URL.
    let authorize = build_authorize_url(&oauth, &app, &pkce.challenge, &state, &redirect_uri)?;
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

            let resp = tiny_http::Response::from_string(LANDING_PAGE).with_header(
                tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                )
                .unwrap(),
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
    let token_url = app.token_url.clone();

    // `Arc::new(NullSink)`, deliberately: auth/OAuth traffic is out of
    // netflow's scope by matt's explicit ruling — "I do not care about
    // update or login" — not a stopgap pending further wiring. See
    // `resolver.rs`'s `refresh_inner` doc comment for the same ruling
    // stated at its call site; there is no further wiring planned here.
    let client = rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System),
        reqwest::Client::builder(),
        std::sync::Arc::new(rupu_netflow::NullSink),
    )?;
    let mut params: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".into()),
        ("code", code.clone()),
        ("client_id", app.client_id.clone()),
        ("redirect_uri", redirect_uri.clone()),
        ("code_verifier", pkce.verifier.clone()),
    ];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret.into()));
    }
    if oauth.include_state_in_token_body {
        params.push(("state", state.clone()));
    }

    let request = match oauth.token_body_format {
        TokenBodyFormat::Form => client.post(&token_url).form(&params),
        TokenBodyFormat::Json => {
            let json: std::collections::BTreeMap<&str, String> = params.into_iter().collect();
            client.post(&token_url).json(&json)
        }
    };

    let token: TokenResponse = request
        .send()
        .await
        .context("token exchange request")?
        .error_for_status()
        .context("token exchange status")?
        .json()
        .await
        .context("token exchange json")?;

    let mut stored = stored_credential_for(provider, token);
    if chosen {
        if let AuthCredentials::OAuth { extra, .. } = &mut stored.credentials {
            extra.insert(EXTRA_CLIENT_ID.into(), app.client_id.into());
            extra.insert(EXTRA_TOKEN_URL.into(), app.token_url.into());
        }
    }
    Ok(stored)
}

/// What of a token response is stored for `provider`: the tokens and their
/// expiry, plus the Anthropic account/organization UUIDs (so the Anthropic
/// adapter can send `metadata.user_id.account_uuid`, binding traffic to
/// the user's Pro/Max quota pool) and — for OpenAI only — the ID token the
/// Codex client takes its account id from. Any other provider's ID token
/// (Gemini's carries the user's email) is not stored. A Gemini credential
/// records the OAuth client it was issued to (`variant`: the Gemini CLI's);
/// its Code Assist project is set up once it is stored
/// ([`crate::oauth::gemini::set_up_code_assist`]).
fn stored_credential_for(provider: ProviderId, token: TokenResponse) -> StoredCredential {
    let expires_at = token
        .expires_in
        .map(|s| Utc::now() + chrono::Duration::seconds(s));

    let expires_ms = expires_at.map(|d| d.timestamp_millis() as u64).unwrap_or(0);

    let mut extra: std::collections::HashMap<String, serde_json::Value> = Default::default();
    if let Some(uuid) = token.account.as_ref().and_then(|a| a.uuid.clone()) {
        extra.insert("account_uuid".into(), serde_json::Value::String(uuid));
    }
    if let Some(uuid) = token.organization.as_ref().and_then(|o| o.uuid.clone()) {
        extra.insert("organization_uuid".into(), serde_json::Value::String(uuid));
    }
    if provider == ProviderId::Openai {
        if let Some(id_token) = token.id_token {
            extra.insert("id_token".into(), serde_json::Value::String(id_token));
        }
    }
    if provider == ProviderId::Gemini {
        let variant = rupu_providers::google_gemini::GeminiVariant::GeminiCli;
        if let Some(hint) = variant.credential_hint() {
            extra.insert(
                rupu_providers::google_gemini::code_assist::EXTRA_VARIANT.into(),
                hint.into(),
            );
        }
    }

    StoredCredential {
        credentials: AuthCredentials::OAuth {
            access: token.access_token,
            refresh: token.refresh_token.clone().unwrap_or_default(),
            expires: expires_ms,
            extra,
        },
        refresh_token: token.refresh_token,
        expires_at,
    }
}

fn build_authorize_url(
    oauth: &crate::oauth::providers::ProviderOAuth,
    app: &OAuthClient,
    challenge: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<String> {
    let mut url = url::Url::parse(&app.authorize_url)?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &app.client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &oauth.scopes.join(" "))
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        for (k, v) in oauth.extra_authorize_params {
            q.append_pair(k, v);
        }
    }
    Ok(url.to_string())
}

/// Bind the redirect listener. When `fixed_ports` is `Some`, walk the
/// list and use the first one that succeeds (mirrors Codex CLI's
/// 1455 → 1457 fallback). When `None`, bind to OS-assigned port 0.
fn bind_listener(fixed_ports: Option<&'static [u16]>) -> Result<tiny_http::Server> {
    // Allow tests to force a specific port by env var (legacy seam).
    if let Ok(p) = std::env::var("RUPU_OAUTH_FORCE_PORT") {
        if let Ok(port) = p.parse::<u16>() {
            return tiny_http::Server::http(format!("127.0.0.1:{port}"))
                .map_err(|e| anyhow!("bind 127.0.0.1:{port}: {e}"));
        }
    }
    match fixed_ports {
        Some(ports) => {
            let mut last_err: Option<String> = None;
            for &port in ports {
                match tiny_http::Server::http(format!("127.0.0.1:{port}")) {
                    Ok(s) => return Ok(s),
                    Err(e) => last_err = Some(format!("port {port}: {e}")),
                }
            }
            Err(anyhow!(
                "could not bind any of the required ports {ports:?} ({})",
                last_err.unwrap_or_else(|| "unknown".into())
            ))
        }
        None => {
            tiny_http::Server::http("127.0.0.1:0").map_err(|e| anyhow!("bind 127.0.0.1:0: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_with_id_token() -> TokenResponse {
        serde_json::from_value(serde_json::json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 3600,
            "id_token": "eyJ.id-token"
        }))
        .unwrap()
    }

    fn extra_of(sc: &StoredCredential) -> &std::collections::HashMap<String, serde_json::Value> {
        match &sc.credentials {
            AuthCredentials::OAuth { extra, .. } => extra,
            other => panic!("expected OAuth, got {other:?}"),
        }
    }

    /// OpenAI's ID token is stored (the Codex client takes its account id
    /// from it); no other provider's is.
    #[test]
    fn only_openai_s_id_token_is_stored() {
        let openai = stored_credential_for(ProviderId::Openai, token_with_id_token());
        assert_eq!(
            extra_of(&openai).get("id_token").and_then(|v| v.as_str()),
            Some("eyJ.id-token")
        );
        for provider in [ProviderId::Gemini, ProviderId::Anthropic] {
            let sc = stored_credential_for(provider, token_with_id_token());
            assert!(
                !extra_of(&sc).contains_key("id_token"),
                "{provider}: {:?}",
                extra_of(&sc)
            );
            assert_eq!(sc.refresh_token.as_deref(), Some("refresh"));
        }
    }

    /// A login with a chosen application (a GitLab account's configured
    /// one, or a self-managed instance's) sends the user to that
    /// application's consent screen, on that instance.
    #[test]
    fn the_authorize_url_names_the_chosen_application_on_its_instance() {
        let oauth = provider_oauth(ProviderId::Gitlab).unwrap();
        let client = OAuthClient {
            client_id: "corp-app".into(),
            authorize_url: "https://gitlab.example.com/oauth/authorize".into(),
            token_url: "https://gitlab.example.com/oauth/token".into(),
        };
        let url = build_authorize_url(
            &oauth,
            &client,
            "challenge",
            "state",
            "http://localhost:7171/auth/redirect",
        )
        .unwrap();
        let url = url::Url::parse(&url).unwrap();
        assert_eq!(url.host_str(), Some("gitlab.example.com"));
        assert_eq!(url.path(), "/oauth/authorize");
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "corp-app");
        assert_eq!(q["redirect_uri"], "http://localhost:7171/auth/redirect");
        assert_eq!(q["scope"], "openid profile read_user write_repository api");
        assert_eq!(q["code_challenge_method"], "S256");
    }
}
