//! RFC 8628 device-code OAuth flow: GitHub and Copilot (the built-in
//! application) and GitLab (`POST {instance}/oauth/authorize_device`,
//! GitLab 17.9 or later, as the application the login chose).

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rupu_providers::auth::AuthCredentials;
use serde::Deserialize;

use crate::backend::ProviderId;
use crate::oauth::common::{self, TokenResponse};
use crate::oauth::providers::{provider_oauth, HeadlessFlow, OAuthClient, OAuthFlow};
use crate::stored::StoredCredential;

/// What a `slow_down` reply adds to the polling interval when it names no
/// new one (RFC 8628 §3.5).
const SLOW_DOWN_STEP_SECS: u64 = 5;

/// The shortest polling interval: a reply's `interval` of 0 never makes
/// the loop poll with no wait.
const MIN_INTERVAL_SECS: u64 = 1;

const GITLAB_WITHOUT_DEVICE_LOGIN: &str = "this GitLab does not offer device login (it needs \
     GitLab 17.9 or later); log in from a desktop with --mode sso, or use --mode api-key with a \
     personal access token";

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    interval: Option<u64>,
}

/// A token-endpoint reply while polling: the token, or an RFC 8628 error.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PollReply {
    Token(TokenResponse),
    Error {
        error: String,
        #[serde(default)]
        interval: Option<u64>,
    },
}

/// What the loop does after a reply.
#[derive(Debug)]
enum Poll {
    /// Poll again after this many seconds.
    Again(u64),
    Done(TokenResponse),
}

/// The device-code flow as `provider`'s built-in application.
pub async fn run(provider: ProviderId) -> Result<StoredCredential> {
    run_with_client(provider, None).await
}

/// The device-code flow for `provider`.
///
/// `client`: a chosen application (GitLab's configured one or a
/// self-managed instance's), whose `device_url`, `token_url` and
/// `client_id` are used as they are (no env override). `None`: the
/// built-in application with the `RUPU_DEVICE_*` seams.
pub async fn run_with_client(
    provider: ProviderId,
    client: Option<OAuthClient>,
) -> Result<StoredCredential> {
    let oauth =
        provider_oauth(provider).ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    if oauth.flow != OAuthFlow::Device && oauth.headless != Some(HeadlessFlow::DeviceCode) {
        anyhow::bail!("provider {provider} does not use the device flow");
    }

    let (client_id, device_url, token_url) = match &client {
        Some(app) => {
            let device_url = app.device_url.clone().ok_or_else(|| {
                anyhow!(
                    "the {provider} application {} has no device-authorization endpoint",
                    app.client_id
                )
            })?;
            (app.client_id.clone(), device_url, app.token_url.clone())
        }
        None => {
            let device_url = match std::env::var("RUPU_DEVICE_DEVICE_URL_OVERRIDE") {
                Ok(url) => url,
                Err(_) => oauth
                    .device_url
                    .ok_or_else(|| anyhow!("{provider} has no device-authorization endpoint"))?
                    .to_string(),
            };
            let token_url = std::env::var("RUPU_DEVICE_TOKEN_URL_OVERRIDE")
                .unwrap_or_else(|_| oauth.token_url.to_string());
            (oauth.client_id.to_string(), device_url, token_url)
        }
    };

    let http = common::http_client()?;
    let resp = http
        .post(&device_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id.as_str()),
            ("scope", &oauth.scopes.join(" ")),
        ])
        .send()
        .await
        .context("device-code request")?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND && provider == ProviderId::Gitlab {
        anyhow::bail!("{GITLAB_WITHOUT_DEVICE_LOGIN}");
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("{}", device_code_failure(status, &body));
    }
    let dc: DeviceCodeResponse = resp.json().await.context("device-code json")?;

    println!(
        "Visit {} and enter code: {}",
        dc.verification_uri, dc.user_code
    );
    println!("Waiting for approval…");

    // Poll as fast as the test wants.
    let fast_poll = std::env::var_os("RUPU_DEVICE_FAST_POLL").is_some();
    let mut interval = dc
        .interval
        .unwrap_or(SLOW_DOWN_STEP_SECS)
        .max(MIN_INTERVAL_SECS);
    let deadline = Utc::now() + chrono::Duration::seconds(dc.expires_in.unwrap_or(900) as i64);

    let token = loop {
        if !fast_poll {
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
        if Utc::now() >= deadline {
            anyhow::bail!("device-code flow timed out");
        }
        let reply: PollReply = http
            .post(&token_url)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client_id.as_str()),
                ("device_code", dc.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .context("device-code poll request")?
            .json()
            .await
            .context("device-code poll json")?;
        match after_reply(interval, reply)? {
            Poll::Again(next) => interval = next,
            Poll::Done(token) => break token,
        }
    };

    Ok(match provider {
        ProviderId::Gitlab => {
            let mut stored = common::stored_credential_for(ProviderId::Gitlab, token);
            if let Some(app) = &client {
                common::record_client(&mut stored, app);
            }
            stored
        }
        // GitHub's tokens don't expire and come with no refresh token; the
        // access token stands in for one, as it always has.
        _ => StoredCredential {
            credentials: AuthCredentials::OAuth {
                access: token.access_token.clone(),
                refresh: String::new(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some(token.access_token),
            expires_at: None,
        },
    })
}

/// What a refused device-code request reports: the vendor's RFC 6749
/// `error` (and `error_description`) when the body carries one — GitLab's
/// `invalid_client` for a misconfigured application —, else the status.
fn device_code_failure(status: reqwest::StatusCode, body: &str) -> String {
    let reply = serde_json::from_str::<serde_json::Value>(body).unwrap_or_default();
    let error = reply["error"].as_str().filter(|e| !e.is_empty());
    let description = reply["error_description"]
        .as_str()
        .filter(|d| !d.is_empty());
    match (error, description) {
        (Some(e), Some(d)) => format!("device-code request failed ({status}): {e}: {d}"),
        (Some(e), None) => format!("device-code request failed ({status}): {e}"),
        _ => format!("device-code request failed with status {status}"),
    }
}

/// RFC 8628 §3.5: `slow_down` with no new interval adds 5 s; a reply's
/// `interval` replaces the current one. Never below 1 s.
pub fn next_interval(current: u64, reply: Option<u64>) -> u64 {
    reply
        .unwrap_or(current + SLOW_DOWN_STEP_SECS)
        .max(MIN_INTERVAL_SECS)
}

/// What the loop polling at `interval` does after `reply`:
/// `authorization_pending` polls again as it was, `slow_down` slower, a
/// token ends the loop, and any other error ends the flow.
fn after_reply(interval: u64, reply: PollReply) -> Result<Poll> {
    match reply {
        PollReply::Token(token) => Ok(Poll::Done(token)),
        PollReply::Error { error, interval: i } => match error.as_str() {
            "authorization_pending" => Ok(Poll::Again(interval)),
            "slow_down" => Ok(Poll::Again(next_interval(interval, i))),
            _ => anyhow::bail!("device-code flow error: {error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(json: serde_json::Value) -> PollReply {
        serde_json::from_value(json).unwrap()
    }

    fn again(interval: u64, json: serde_json::Value) -> u64 {
        match after_reply(interval, reply(json)).unwrap() {
            Poll::Again(next) => next,
            Poll::Done(_) => panic!("expected another poll"),
        }
    }

    #[test]
    fn slow_down_without_an_interval_adds_five_seconds() {
        assert_eq!(next_interval(5, None), 10);
    }

    #[test]
    fn a_reply_interval_replaces_the_current_one() {
        assert_eq!(next_interval(5, Some(7)), 7);
    }

    /// A `slow_down` naming 0 never resets a grown interval to no wait.
    #[test]
    fn the_interval_is_never_below_one_second() {
        assert_eq!(next_interval(10, Some(0)), 1);
        assert_eq!(next_interval(0, Some(0)), 1);
        assert_eq!(
            again(
                10,
                serde_json::json!({ "error": "slow_down", "interval": 0 })
            ),
            1
        );
    }

    #[test]
    fn a_refused_device_code_request_reports_the_vendor_s_error_else_the_status() {
        let unauthorized = reqwest::StatusCode::UNAUTHORIZED;
        assert_eq!(
            device_code_failure(
                unauthorized,
                r#"{"error":"invalid_client","error_description":"Client authentication failed"}"#
            ),
            "device-code request failed (401 Unauthorized): invalid_client: \
             Client authentication failed"
        );
        assert_eq!(
            device_code_failure(unauthorized, r#"{"error":"invalid_client"}"#),
            "device-code request failed (401 Unauthorized): invalid_client"
        );
        for body in ["", "<html>nope</html>", "{}", r#"{"error":""}"#] {
            assert_eq!(
                device_code_failure(unauthorized, body),
                "device-code request failed with status 401 Unauthorized",
                "{body}"
            );
        }
    }

    /// GitLab's `slow_down` names no interval; GitHub's does.
    #[test]
    fn the_poll_loop_slows_down_as_the_reply_says() {
        assert_eq!(again(5, serde_json::json!({ "error": "slow_down" })), 10);
        assert_eq!(
            again(
                5,
                serde_json::json!({ "error": "slow_down", "interval": 7 })
            ),
            7
        );
        assert_eq!(
            again(
                5,
                serde_json::json!({ "error": "authorization_pending", "interval": 9 })
            ),
            5
        );
    }

    #[test]
    fn a_terminal_error_ends_the_flow_with_the_vendor_s_error() {
        for error in ["expired_token", "access_denied", "invalid_grant"] {
            let err = after_reply(5, reply(serde_json::json!({ "error": error })))
                .unwrap_err()
                .to_string();
            assert_eq!(err, format!("device-code flow error: {error}"));
        }
    }
}
