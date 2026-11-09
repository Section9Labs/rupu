//! OpenAI's device auth: Codex CLI's `--device-auth`
//! (openai/codex `codex-rs/login/src/device_code_auth.rs`). Not RFC 8628:
//! a pair of accounts endpoints hands out a user code, the login polls
//! until the user approves it on `{issuer}/codex/device`, and the
//! approval is an authorization code with a server-generated PKCE
//! verifier, exchanged at `{issuer}/oauth/token` as the browser flow's
//! code is.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer};

use crate::backend::ProviderId;
use crate::oauth::common::{self, CodeExchange};
use crate::oauth::providers::{provider_oauth, OAuthClient};
use crate::stored::StoredCredential;

/// OpenAI's accounts issuer.
pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";

/// How long a login waits for the user to approve its code, counted from
/// the first poll (Codex's 15 minutes).
const BUDGET: Duration = Duration::from_secs(15 * 60);

const NOT_ENABLED: &str = "device login is not enabled for this ChatGPT account. Turn it on in \
     ChatGPT settings → Security (a workspace admin does it under workspace permissions), then \
     retry; or log in from a desktop with --mode sso";

/// Where a device login goes and how long it waits.
#[derive(Debug, Clone)]
pub struct Options {
    /// The accounts issuer; a trailing `/` is ignored.
    pub issuer: String,
    /// The OAuth client the login is for (Codex CLI's).
    pub client_id: String,
    /// How long to poll for approval before giving up.
    pub budget: Duration,
}

impl Options {
    /// [`DEFAULT_ISSUER`] unless `RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE` is
    /// set, Codex CLI's client and a 15-minute budget.
    pub fn from_env() -> Self {
        let issuer = std::env::var("RUPU_OPENAI_DEVICE_ISSUER_OVERRIDE")
            .unwrap_or_else(|_| DEFAULT_ISSUER.to_string());
        let client_id = provider_oauth(ProviderId::Openai)
            .map(|oauth| oauth.client_id)
            .unwrap_or_default();
        Self {
            issuer: issuer.trim_end_matches('/').to_string(),
            client_id: client_id.to_string(),
            budget: BUDGET,
        }
    }
}

/// The page the operator opens: `{issuer}/codex/device`.
pub fn verification_url(issuer: &str) -> String {
    format!("{}/codex/device", issuer.trim_end_matches('/'))
}

#[derive(Debug, Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "interval_secs")]
    interval: u64,
}

/// The poll interval, which OpenAI sends as a string (`"5"`); a number is
/// accepted too, and `null` is 0.
fn interval_secs<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Interval {
        Number(u64),
        Text(String),
    }
    match Option::<Interval>::deserialize(d)? {
        None => Ok(0),
        Some(Interval::Number(n)) => Ok(n),
        Some(Interval::Text(s)) => s.trim().parse().map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Deserialize)]
struct Approval {
    authorization_code: String,
    code_verifier: String,
}

/// Runs the login with [`Options::from_env`].
pub async fn run() -> Result<StoredCredential> {
    run_with(Options::from_env()).await
}

/// Runs the login: requests a user code, shows it, polls until the user
/// approves it (or `opts.budget` runs out) and exchanges the approval for
/// tokens, stored as the browser flow stores them (with the ID token).
pub async fn run_with(opts: Options) -> Result<StoredCredential> {
    let issuer = opts.issuer.trim_end_matches('/');
    let api_base = format!("{issuer}/api/accounts");
    let client = common::http_client()?;

    let resp = client
        .post(format!("{api_base}/deviceauth/usercode"))
        .json(&serde_json::json!({ "client_id": opts.client_id }))
        .send()
        .await
        .context("device login request")?;
    let status = resp.status();
    if status == StatusCode::NOT_FOUND {
        bail!("{NOT_ENABLED}");
    }
    if !status.is_success() {
        bail!("device login request failed with status {status}");
    }
    let code: UserCodeResponse = resp.json().await.context("device login request json")?;

    eprintln!(
        "Visit {} and enter code: {}",
        verification_url(issuer),
        code.user_code
    );
    eprintln!("Only continue if you started this login yourself. The code expires in 15 minutes.");
    eprintln!("Waiting for approval…");

    let approval = poll(&client, &api_base, &code, opts.budget).await?;

    let oauth =
        provider_oauth(ProviderId::Openai).ok_or_else(|| anyhow!("no oauth config for openai"))?;
    let app = OAuthClient {
        client_id: opts.client_id.clone(),
        authorize_url: format!("{issuer}/oauth/authorize"),
        token_url: format!("{issuer}/oauth/token"),
        device_url: None,
    };
    let redirect_uri = format!("{issuer}/deviceauth/callback");
    let token = common::exchange_code(
        &client,
        CodeExchange {
            oauth: &oauth,
            app: &app,
            client_secret: oauth.client_secret,
            code: &approval.authorization_code,
            redirect_uri: &redirect_uri,
            code_verifier: &approval.code_verifier,
            state: "",
        },
    )
    .await?;
    Ok(common::stored_credential_for(ProviderId::Openai, token))
}

/// Polls `{api_base}/deviceauth/token` until the code is approved. A 403
/// or 404 is "not yet"; `budget` from the first poll ends the wait.
async fn poll(
    client: &reqwest_middleware::ClientWithMiddleware,
    api_base: &str,
    code: &UserCodeResponse,
    budget: Duration,
) -> Result<Approval> {
    let url = format!("{api_base}/deviceauth/token");
    let body = serde_json::json!({
        "device_auth_id": code.device_auth_id,
        "user_code": code.user_code,
    });
    let interval = Duration::from_secs(code.interval);
    let started = Instant::now();
    loop {
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("device login poll")?;
        let status = resp.status();
        if status.is_success() {
            return resp.json().await.context("device login poll json");
        }
        if status != StatusCode::FORBIDDEN && status != StatusCode::NOT_FOUND {
            bail!("device login failed with status {status}");
        }
        let Some(remaining) = budget
            .checked_sub(started.elapsed())
            .filter(|r| !r.is_zero())
        else {
            bail!("device login timed out");
        };
        tokio::time::sleep(interval.min(remaining)).await;
    }
}
