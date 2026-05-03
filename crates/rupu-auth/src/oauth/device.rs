//! GitHub device-code OAuth flow. Used for Copilot.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rupu_providers::auth::AuthCredentials;
use serde::Deserialize;

use crate::backend::ProviderId;
use crate::oauth::providers::{provider_oauth, OAuthFlow};
use crate::stored::StoredCredential;

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

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AccessTokenResponse {
    Success {
        access_token: String,
        #[serde(default)]
        _token_type: Option<String>,
        #[serde(default)]
        _scope: Option<String>,
    },
    Pending {
        error: String,
        #[serde(default)]
        interval: Option<u64>,
    },
}

pub async fn run(provider: ProviderId) -> Result<StoredCredential> {
    let oauth =
        provider_oauth(provider).ok_or_else(|| anyhow!("no oauth config for {provider}"))?;
    if oauth.flow != OAuthFlow::Device {
        anyhow::bail!("provider {provider} does not use the device flow");
    }

    let device_url = std::env::var("RUPU_DEVICE_DEVICE_URL_OVERRIDE")
        .unwrap_or_else(|_| oauth.device_url.unwrap_or_default().to_string());
    let token_url = std::env::var("RUPU_DEVICE_TOKEN_URL_OVERRIDE")
        .unwrap_or_else(|_| oauth.token_url.to_string());

    let client = reqwest::Client::new();
    let dc: DeviceCodeResponse = client
        .post(&device_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", oauth.client_id),
            ("scope", &oauth.scopes.join(" ")),
        ])
        .send()
        .await
        .context("device-code request")?
        .error_for_status()
        .context("device-code status")?
        .json()
        .await
        .context("device-code json")?;

    println!(
        "Visit {} and enter code: {}",
        dc.verification_uri, dc.user_code
    );

    let mut interval = dc.interval.unwrap_or(5);
    if std::env::var_os("RUPU_DEVICE_FAST_POLL").is_some() {
        interval = 0; // poll as fast as the test wants
    }
    let deadline = Utc::now() + chrono::Duration::seconds(dc.expires_in.unwrap_or(900) as i64);

    loop {
        if interval > 0 {
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
        if Utc::now() >= deadline {
            anyhow::bail!("device-code flow timed out");
        }
        let resp: AccessTokenResponse = client
            .post(&token_url)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", oauth.client_id),
                ("device_code", dc.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .context("device-code poll request")?
            .json()
            .await
            .context("device-code poll json")?;
        match resp {
            AccessTokenResponse::Success { access_token, .. } => {
                return Ok(StoredCredential {
                    credentials: AuthCredentials::OAuth {
                        access: access_token.clone(),
                        refresh: String::new(),
                        expires: 0,
                        extra: Default::default(),
                    },
                    refresh_token: Some(access_token),
                    expires_at: None,
                });
            }
            AccessTokenResponse::Pending { error, interval: i } => {
                if error == "authorization_pending" || error == "slow_down" {
                    if let Some(new_interval) = i {
                        if std::env::var_os("RUPU_DEVICE_FAST_POLL").is_none() {
                            interval = new_interval;
                        }
                    }
                    continue;
                }
                anyhow::bail!("device-code flow error: {error}");
            }
        }
    }
}
