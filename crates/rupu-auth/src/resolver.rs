//! CredentialResolver: the runtime's single point of truth for "which
//! credential should this provider call use right now?"

use anyhow::Result;
use async_trait::async_trait;

use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;

/// Buffer (seconds) before expiry at which we proactively refresh.
pub const EXPIRY_REFRESH_BUFFER_SECS: i64 = 60;

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Resolve credentials for `provider`. `hint` may force a specific
    /// auth mode; if None, applies SSO > API-key precedence.
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)>;

    /// Force-refresh credentials. Used when an adapter sees a 401 mid-request.
    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials>;
}

// ── KeychainResolver ─────────────────────────────────────────────────────────

use crate::backend::ProviderId;
use crate::keychain_layout::{key_for, legacy_key_for, KeychainKey};
use crate::stored::StoredCredential;

/// Production resolver: reads/writes [`StoredCredential`] JSON to the OS
/// keychain at `rupu/<provider>/<mode>` entries.
///
/// On SSO entries whose access token is within [`EXPIRY_REFRESH_BUFFER_SECS`]
/// of expiry, [`KeychainResolver::get`] performs a silent token refresh via
/// the standard OAuth refresh-token grant before returning credentials.
pub struct KeychainResolver {
    service: String,
}

impl KeychainResolver {
    pub fn new() -> Self {
        Self {
            service: "rupu".to_string(),
        }
    }

    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, key: &KeychainKey) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &key.account)
            .map_err(|e| anyhow::anyhow!("keychain entry: {e}"))
    }

    pub async fn store(&self, p: ProviderId, mode: AuthMode, sc: &StoredCredential) -> Result<()> {
        let key = key_for(p, mode);
        let entry = self.entry(&key)?;
        let json = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        entry
            .set_password(&json)
            .map_err(|e| anyhow::anyhow!("keychain set: {e}"))?;
        Ok(())
    }

    pub async fn forget(&self, p: ProviderId, mode: AuthMode) -> Result<()> {
        let key = key_for(p, mode);
        let entry = self.entry(&key)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow::anyhow!("keychain delete: {e}")),
        }
    }

    fn read(&self, p: ProviderId, mode: AuthMode) -> Result<Option<StoredCredential>> {
        let key = key_for(p, mode);
        match self.entry(&key)?.get_password() {
            Ok(s) => Ok(Some(parse_stored_credential(&s, mode)?)),
            Err(keyring::Error::NoEntry) => {
                // Slice A legacy fallback: try the old single-key shape
                // (treated as api-key). Only relevant for ApiKey lookups.
                if mode == AuthMode::ApiKey {
                    let lk = legacy_key_for(p);
                    match self.entry(&lk)?.get_password() {
                        Ok(s) => Ok(Some(StoredCredential::api_key(s))),
                        Err(keyring::Error::NoEntry) => Ok(None),
                        Err(e) => Err(anyhow::anyhow!("keychain legacy read: {e}")),
                    }
                } else {
                    Ok(None)
                }
            }
            Err(e) => Err(anyhow::anyhow!("keychain read: {e}")),
        }
    }

    fn parse_provider(name: &str) -> Result<ProviderId> {
        match name {
            "anthropic" => Ok(ProviderId::Anthropic),
            "openai" => Ok(ProviderId::Openai),
            "gemini" => Ok(ProviderId::Gemini),
            "copilot" => Ok(ProviderId::Copilot),
            "local" => Ok(ProviderId::Local),
            other => anyhow::bail!("unknown provider: {other}"),
        }
    }

    /// Returns true if a credential entry exists for the given provider/mode.
    pub async fn peek(&self, p: ProviderId, mode: AuthMode) -> bool {
        self.read(p, mode).map(|o| o.is_some()).unwrap_or(false)
    }

    /// Returns a human-readable expiry string for an SSO token, or `None`
    /// if no SSO credential exists for the provider.
    pub async fn peek_sso(&self, p: ProviderId) -> Option<String> {
        let sc = self.read(p, AuthMode::Sso).ok().flatten()?;
        let exp = sc.expires_at?;
        let now = chrono::Utc::now();
        let dur = exp.signed_duration_since(now);
        if dur.num_seconds() <= 0 {
            Some("expired — re-login".into())
        } else if dur.num_days() >= 1 {
            Some(format!("expires in {}d", dur.num_days()))
        } else {
            Some(format!("expires in {}h", dur.num_hours().max(1)))
        }
    }

    async fn refresh_inner(
        &self,
        p: ProviderId,
        _mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<StoredCredential> {
        let oauth = crate::oauth::providers::provider_oauth(p)
            .ok_or_else(|| anyhow::anyhow!("no oauth config for {p}"))?;
        let refresh_token = sc.refresh_token.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "{p} SSO token expired and no refresh token stored. \
                 Run: rupu auth login --provider {p} --mode sso"
            )
        })?;
        // Provider-agnostic refresh: standard OAuth refresh-token grant.
        let token_url = std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| oauth.token_url.to_string());
        let client = reqwest::Client::new();
        let resp = client
            .post(&token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", oauth.client_id),
            ])
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("refresh request: {e}"))?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "refresh failed for {p}: HTTP {}. Run: rupu auth login --provider {p} --mode sso",
                resp.status()
            );
        }
        #[derive(serde::Deserialize)]
        struct R {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            expires_in: Option<i64>,
        }
        let r: R = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("refresh json: {e}"))?;
        // Preserve `extra` (account_uuid, organization_uuid, etc.) from
        // the prior credential — refresh-token responses generally don't
        // re-emit the account block, but those identifiers don't change
        // for the lifetime of the OAuth grant, so carrying them forward
        // keeps `metadata.user_id.account_uuid` populated post-refresh.
        let prior_extra = match &sc.credentials {
            rupu_providers::auth::AuthCredentials::OAuth { extra, .. } => extra.clone(),
            _ => Default::default(),
        };
        Ok(StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: r.access_token.clone(),
                refresh: r
                    .refresh_token
                    .clone()
                    .unwrap_or_else(|| refresh_token.to_string()),
                expires: r.expires_in.unwrap_or(0) as u64,
                extra: prior_extra,
            },
            refresh_token: Some(r.refresh_token.unwrap_or_else(|| refresh_token.to_string())),
            expires_at: r
                .expires_in
                .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s)),
        })
    }
}

impl Default for KeychainResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Deserialize a keychain entry's payload into a [`StoredCredential`].
///
/// Most entries hold the canonical JSON-serialized `StoredCredential`. For
/// ApiKey entries we additionally tolerate a raw plain-string payload —
/// pre-StoredCredential builds wrote api-keys that way under the new keyspace,
/// and the only way to recover from one of those entries (without surfacing a
/// confusing JSON-parse error to the user) is to treat the raw payload as a
/// legacy api-key. SSO entries cannot be recovered this way because the SSO
/// shape requires structured fields.
fn parse_stored_credential(s: &str, mode: AuthMode) -> Result<StoredCredential> {
    match serde_json::from_str::<StoredCredential>(s) {
        Ok(sc) => Ok(sc),
        Err(_) if mode == AuthMode::ApiKey => Ok(StoredCredential::api_key(s)),
        Err(e) => Err(anyhow::anyhow!(
            "keychain payload not StoredCredential JSON: {e}"
        )),
    }
}

#[async_trait]
impl CredentialResolver for KeychainResolver {
    async fn get(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)> {
        let p = Self::parse_provider(provider)?;
        let modes: Vec<AuthMode> = match hint {
            Some(m) => vec![m],
            None => vec![AuthMode::Sso, AuthMode::ApiKey],
        };
        for mode in modes {
            if let Some(mut sc) = self.read(p, mode)? {
                let now = chrono::Utc::now();
                if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                    let new = self.refresh_inner(p, mode, &sc).await?;
                    self.store(p, mode, &new).await?;
                    sc = new;
                }
                return Ok((mode, sc.credentials));
            }
        }
        anyhow::bail!(
            "no credentials configured for {provider}. \
             Run: rupu auth login --provider {provider} --mode <api-key|sso>"
        )
    }

    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let p = Self::parse_provider(provider)?;
        let sc = self
            .read(p, mode)?
            .ok_or_else(|| anyhow::anyhow!("no stored credential for {provider}/{mode:?}"))?;
        let new = self.refresh_inner(p, mode, &sc).await?;
        self.store(p, mode, &new).await?;
        Ok(new.credentials)
    }
}

#[cfg(test)]
mod parse_stored_credential_tests {
    use super::*;
    use rupu_providers::auth::AuthCredentials;

    #[test]
    fn json_payload_parses_as_stored_credential() {
        let json = r#"{"credentials":{"type":"api_key","key":"sk-test"}}"#;
        let sc = parse_stored_credential(json, AuthMode::ApiKey).expect("parse");
        match sc.credentials {
            AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-test"),
            _ => panic!("expected ApiKey credential"),
        }
    }

    #[test]
    fn raw_string_in_api_key_slot_falls_back_to_legacy_api_key() {
        // Legacy 0.1.5 builds wrote api-keys as raw strings under the new
        // keyspace. The resolver must recover instead of bubbling up a
        // confusing serde_json parse error to `rupu run`.
        let raw = "sk-ant-api03-legacy-plain-string";
        let sc = parse_stored_credential(raw, AuthMode::ApiKey).expect("legacy fallback");
        match sc.credentials {
            AuthCredentials::ApiKey { key } => assert_eq!(key, raw),
            _ => panic!("expected legacy api-key fallback"),
        }
        assert!(sc.refresh_token.is_none());
        assert!(sc.expires_at.is_none());
    }

    #[test]
    fn raw_string_in_sso_slot_returns_error() {
        // SSO requires structured fields (refresh_token, expires_at, etc.),
        // so a raw-string payload there really is unrecoverable garbage —
        // surface it rather than silently forging a half-broken credential.
        let raw = "not-a-real-oauth-token";
        let err = parse_stored_credential(raw, AuthMode::Sso).expect_err("should fail");
        let msg = format!("{err}");
        assert!(
            msg.contains("StoredCredential"),
            "expected typed error, got: {msg}"
        );
    }
}
