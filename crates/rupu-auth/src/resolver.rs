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
#[cfg(not(target_os = "macos"))]
use crate::keychain_layout::KeychainKey;
use crate::keychain_layout::{key_for, legacy_key_for};
use crate::stored::StoredCredential;
use std::path::PathBuf;

/// Production resolver: reads/writes [`StoredCredential`] JSON to the OS
/// keychain at `rupu/<provider>/<mode>` entries — OR, when the
/// `RUPU_AUTH_BACKEND=file` env var is set, to `~/.rupu/auth.json`
/// (chmod 600).
///
/// The file backend is the escape hatch for the macOS keychain
/// dropping credentials between signed-binary updates: every new
/// build has a different cdhash, the keychain ACL pins the trusted
/// app to that cdhash, and the next build's read silently fails
/// in non-interactive contexts. Setting `RUPU_AUTH_BACKEND=file`
/// stores secrets in a normal chmod-600 JSON file that survives
/// updates because it's not bound to any signing identity.
///
/// On SSO entries whose access token is within [`EXPIRY_REFRESH_BUFFER_SECS`]
/// of expiry, [`KeychainResolver::get`] performs a silent token refresh via
/// the standard OAuth refresh-token grant before returning credentials.
pub struct KeychainResolver {
    storage: Storage,
}

/// Where credentials actually live for this resolver instance. The
/// keyring path is the default; the JSON-file path is the escape
/// hatch via `RUPU_AUTH_BACKEND=file`.
enum Storage {
    Keyring { service: String },
    JsonFile { path: PathBuf },
}

/// Resolve the global rupu directory, honoring `$RUPU_HOME` (set by
/// integration tests + by users who want a non-default location)
/// before falling back to `~/.rupu/`. Mirrors what
/// `rupu_cli::paths::global_dir()` does, kept in sync to avoid the
/// resolver and CLI looking at different directories.
fn rupu_home_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("RUPU_HOME") {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(".rupu"))
}

/// Default file path for the JSON-file backend's credentials.
/// Follows the same `RUPU_HOME` override as the rest of rupu, so
/// integration tests that redirect HOME also redirect the auth
/// store. Falls back to `./auth.json` only if HOME isn't resolvable
/// at all (extraordinary).
fn default_auth_json_path() -> PathBuf {
    if let Some(home) = rupu_home_dir() {
        return home.join("auth.json");
    }
    tracing::warn!("HOME not set; storing auth.json in current directory");
    PathBuf::from("./auth.json")
}

impl KeychainResolver {
    pub fn new() -> Self {
        Self::with_service("rupu")
    }

    pub fn with_service(service: &str) -> Self {
        // Backend selection priority:
        //   1. `RUPU_AUTH_BACKEND` env var — session override for
        //      users who explicitly want the OS keychain (or for
        //      tests that need to swap out at runtime).
        //   2. Probe cache at `<HOME>/.rupu/cache/auth-backend.json`
        //      — persistent choice, set via `rupu auth backend --use`.
        //   3. Default = file (chmod-600 JSON at `~/.rupu/auth.json`).
        //      This matches what `gh`, `aws`, `gcloud`, `claude-cli`,
        //      `kubectl`, `terraform`, and most CLI peers do — none
        //      of them hit the OS keychain by default. The keychain
        //      is great for `.app` bundles whose designated
        //      requirement is bundle-ID-based (any binary signed
        //      under that bundle ID + Team ID matches), but for
        //      bare CLI binaries the requirement is cdhash-bound
        //      and breaks on every rebuild — leading to the
        //      "credentials vanished after update" UX rupu hit
        //      repeatedly.
        let env_choice = std::env::var(crate::ENV_BACKEND_OVERRIDE)
            .ok()
            .map(|s| s.trim().to_ascii_lowercase());
        let want_file = match env_choice.as_deref() {
            Some("file") | Some("json") | Some("json-file") | Some("json_file") => Some(true),
            Some("keyring") | Some("keychain") | Some("os") | Some("os-keychain") => Some(false),
            // Empty / unset → fall through to cache.
            _ => None,
        }
        .or_else(|| {
            // Cache lives at `<RUPU_HOME>/cache/auth-backend.json` —
            // same layout the CLI's `rupu auth backend` writes to,
            // and honors the same `RUPU_HOME` test override.
            let cache_path = rupu_home_dir()?.join("cache").join("auth-backend.json");
            let text = std::fs::read_to_string(&cache_path).ok()?;
            let choice: crate::BackendChoice = serde_json::from_str(&text).ok()?;
            Some(matches!(choice, crate::BackendChoice::JsonFile))
        })
        // Default to file when no env override and no cache.
        .unwrap_or(true);

        let storage = if want_file {
            let path = std::env::var("RUPU_AUTH_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| default_auth_json_path());
            tracing::info!(
                path = %path.display(),
                "credential backend = file (chmod-600 JSON); set RUPU_AUTH_BACKEND=keychain to override"
            );
            Storage::JsonFile { path }
        } else {
            Storage::Keyring {
                service: service.to_string(),
            }
        };
        Self { storage }
    }

    #[cfg(not(target_os = "macos"))]
    fn entry(&self, key: &KeychainKey) -> Result<keyring::Entry> {
        match &self.storage {
            Storage::Keyring { service } => keyring::Entry::new(service, &key.account)
                .map_err(|e| anyhow::anyhow!("keychain entry: {e}")),
            Storage::JsonFile { .. } => Err(anyhow::anyhow!(
                "keychain entry not used in file-backend mode (programmer error)"
            )),
        }
    }

    /// Read the chmod-600 JSON file as a flat key→value map. Missing
    /// file is not an error — returns an empty map. Invalid JSON
    /// surfaces as a hard error so a corrupt store doesn't silently
    /// drop credentials.
    fn read_file_map(path: &std::path::Path) -> Result<std::collections::BTreeMap<String, String>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
            Err(e) => return Err(anyhow::anyhow!("read {}: {e}", path.display())),
        };
        serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))
    }

    fn write_file_map(
        path: &std::path::Path,
        map: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("mkdir {}: {e}", parent.display()))?;
        }
        let body =
            serde_json::to_string_pretty(map).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        std::fs::write(path, body).map_err(|e| anyhow::anyhow!("write {}: {e}", path.display()))?;
        // Enforce 0600 on every write so a previous loose-mode file
        // gets tightened up next time the user logs in.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                if let Err(e) = std::fs::set_permissions(path, perms) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "could not enforce mode 0600 on auth.json"
                    );
                }
            }
        }
        Ok(())
    }

    pub async fn store(&self, p: ProviderId, mode: AuthMode, sc: &StoredCredential) -> Result<()> {
        let key = key_for(p, mode);
        let json = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        match &self.storage {
            Storage::Keyring { service } => {
                #[cfg(target_os = "macos")]
                {
                    rupu_keychain_acl::set_generic_password(service, &key.account, json.as_bytes())
                        .map_err(|e| anyhow::anyhow!("keychain set: {e}"))?;
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let entry = self.entry(&key)?;
                    entry
                        .set_password(&json)
                        .map_err(|e| anyhow::anyhow!("keychain set: {e}"))?;
                    crate::keyring::try_add_self_to_acl(&key.account);
                }
            }
            Storage::JsonFile { path } => {
                let mut map = Self::read_file_map(path)?;
                map.insert(key.account.clone(), json);
                Self::write_file_map(path, &map)?;
            }
        }
        Ok(())
    }

    pub async fn forget(&self, p: ProviderId, mode: AuthMode) -> Result<()> {
        let key = key_for(p, mode);
        match &self.storage {
            Storage::Keyring { service } => {
                #[cfg(target_os = "macos")]
                {
                    match rupu_keychain_acl::delete_generic_password(service, &key.account) {
                        Ok(()) | Err(rupu_keychain_acl::AclError::NotFound { .. }) => Ok(()),
                        Err(e) => Err(anyhow::anyhow!("keychain delete: {e}")),
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let entry = self.entry(&key)?;
                    match entry.delete_credential() {
                        Ok(()) => Ok(()),
                        Err(keyring::Error::NoEntry) => Ok(()),
                        Err(e) => Err(anyhow::anyhow!("keychain delete: {e}")),
                    }
                }
            }
            Storage::JsonFile { path } => {
                let mut map = Self::read_file_map(path)?;
                if map.remove(&key.account).is_some() {
                    Self::write_file_map(path, &map)?;
                }
                Ok(())
            }
        }
    }

    fn read(&self, p: ProviderId, mode: AuthMode) -> Result<Option<StoredCredential>> {
        let key = key_for(p, mode);
        match &self.storage {
            Storage::Keyring { service } => {
                #[cfg(target_os = "macos")]
                {
                    match rupu_keychain_acl::get_generic_password(service, &key.account) {
                        Ok(bytes) => {
                            let s = String::from_utf8(bytes)
                                .map_err(|e| anyhow::anyhow!("keychain read: {e}"))?;
                            Ok(Some(parse_stored_credential(&s, mode)?))
                        }
                        Err(rupu_keychain_acl::AclError::NotFound { .. }) => {
                            if mode == AuthMode::ApiKey {
                                let lk = legacy_key_for(p);
                                match rupu_keychain_acl::get_generic_password(service, &lk.account)
                                {
                                    Ok(bytes) => {
                                        let s = String::from_utf8(bytes).map_err(|e| {
                                            anyhow::anyhow!("keychain legacy read: {e}")
                                        })?;
                                        Ok(Some(StoredCredential::api_key(s)))
                                    }
                                    Err(rupu_keychain_acl::AclError::NotFound { .. }) => Ok(None),
                                    Err(e) => Err(anyhow::anyhow!("keychain legacy read: {e}")),
                                }
                            } else {
                                Ok(None)
                            }
                        }
                        Err(e) => Err(anyhow::anyhow!("keychain read: {e}")),
                    }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    match self.entry(&key)?.get_password() {
                        Ok(s) => Ok(Some(parse_stored_credential(&s, mode)?)),
                        Err(keyring::Error::NoEntry) => {
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
            }
            Storage::JsonFile { path } => {
                let map = Self::read_file_map(path)?;
                match map.get(&key.account) {
                    Some(s) => Ok(Some(parse_stored_credential(s, mode)?)),
                    None => {
                        // Same Slice-A legacy fallback as the keyring
                        // path: try the bare provider id for api-key.
                        if mode == AuthMode::ApiKey {
                            let lk = legacy_key_for(p);
                            if let Some(legacy) = map.get(&lk.account) {
                                return Ok(Some(StoredCredential::api_key(legacy.clone())));
                            }
                        }
                        Ok(None)
                    }
                }
            }
        }
    }

    fn parse_provider(name: &str) -> Result<ProviderId> {
        match name {
            "anthropic" => Ok(ProviderId::Anthropic),
            "openai" => Ok(ProviderId::Openai),
            "gemini" => Ok(ProviderId::Gemini),
            "copilot" => Ok(ProviderId::Copilot),
            "github" => Ok(ProviderId::Github),
            "gitlab" => Ok(ProviderId::Gitlab),
            "linear" => Ok(ProviderId::Linear),
            "jira" => Ok(ProviderId::Jira),
            "local" => Ok(ProviderId::Local),
            other => anyhow::bail!("unknown provider: {other}"),
        }
    }

    /// Returns true if a credential entry exists for the given provider/mode.
    pub async fn peek(&self, p: ProviderId, mode: AuthMode) -> bool {
        self.read(p, mode).map(|o| o.is_some()).unwrap_or(false)
    }

    /// Returns a human-readable expiry string for an SSO token, or `None`
    /// if no SSO credential exists for the provider. When a credential
    /// is stored but has no `expires_at` (e.g. GitHub device-code grants
    /// that never carry an explicit expiry), returns `Some("no expiry")`
    /// so the status row still renders ✓.
    pub async fn peek_sso(&self, p: ProviderId) -> Option<String> {
        let sc = self.read(p, AuthMode::Sso).ok().flatten()?;
        let Some(exp) = sc.expires_at else {
            return Some("no expiry".into());
        };
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
        // `credentials.expires` is the SAME field the provider crates'
        // `is_token_expired(expires_ms)` checks, and they all interpret
        // it as ABSOLUTE milliseconds-since-Unix-epoch (see e.g.
        // `rupu_providers::anthropic::refresh_anthropic_token` and
        // `rupu_auth::oauth::callback::*` which both store `now_ms +
        // expires_in*1000`). Storing the raw `expires_in` in seconds
        // here corrupted the field to a tiny number (~3600), which
        // `is_token_expired` then read as a Unix timestamp deep in the
        // past and concluded the token was expired — re-firing a
        // provider-side refresh on every call. Anthropic's OAuth
        // server rotates refresh tokens, so the second refresh would
        // race the first and surface as `invalid_grant`. Fix: convert
        // to absolute ms here, matching every other write site.
        let expires_ms = expires_in_secs_to_ms_epoch(r.expires_in);
        Ok(StoredCredential {
            credentials: rupu_providers::auth::AuthCredentials::OAuth {
                access: r.access_token.clone(),
                refresh: r
                    .refresh_token
                    .clone()
                    .unwrap_or_else(|| refresh_token.to_string()),
                expires: expires_ms,
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

/// Convert an OAuth `expires_in` (relative seconds, the wire-format
/// every standard token endpoint returns) into the ABSOLUTE
/// milliseconds-since-Unix-epoch shape that
/// `rupu_providers::auth::is_token_expired` expects. `None` →
/// `0`, matching the "no expiry / treat as valid" sentinel
/// `is_token_expired` already understands.
///
/// Pulled out into a free function so we can lock the conversion
/// behavior under a unit test without spinning up a token-endpoint
/// mock.
fn expires_in_secs_to_ms_epoch(expires_in: Option<i64>) -> u64 {
    match expires_in {
        Some(s) if s > 0 => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            now_ms + (s as u64) * 1000
        }
        _ => 0,
    }
}

#[cfg(test)]
mod expires_in_tests {
    use super::expires_in_secs_to_ms_epoch;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn one_hour_lands_within_one_hour_of_now() {
        // expires_in = 3600s → result must be (now ± a few ms) + 3.6e6 ms.
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let got = expires_in_secs_to_ms_epoch(Some(3600));
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(
            got >= before + 3_600_000 && got <= after + 3_600_000,
            "expected {}..{} (1h window), got {got}",
            before + 3_600_000,
            after + 3_600_000,
        );
    }

    #[test]
    fn none_returns_zero_no_expiry_sentinel() {
        // `is_token_expired(0)` short-circuits to "valid" — preserve
        // that contract for refresh responses that omit `expires_in`.
        assert_eq!(expires_in_secs_to_ms_epoch(None), 0);
    }

    #[test]
    fn zero_or_negative_returns_zero_sentinel() {
        // Pathological responses (negative / zero expiry) shouldn't
        // get encoded as "now" — that'd round-trip to "expired" and
        // cause the same refresh-loop the bug fix targets.
        assert_eq!(expires_in_secs_to_ms_epoch(Some(0)), 0);
        assert_eq!(expires_in_secs_to_ms_epoch(Some(-1)), 0);
    }

    #[test]
    fn result_is_compatible_with_is_token_expired() {
        // End-to-end shape check: a refresh that issues a 1h token
        // produces an `expires_ms` that `is_token_expired` reads as
        // "valid". Pre-fix this returned a tiny number (~3600) that
        // `is_token_expired` immediately classified as expired,
        // re-firing the refresh on every call.
        let expires_ms = expires_in_secs_to_ms_epoch(Some(3600));
        assert!(
            !rupu_providers::auth::is_token_expired(expires_ms),
            "fresh 1h token must not read as expired (got expires_ms={expires_ms})",
        );
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

    #[test]
    fn parse_provider_recognizes_github_and_gitlab() {
        // Regression: rupu repos list calls resolver.get("github", None);
        // pre-fix this errored "unknown provider: github" and silently
        // bubbled up to the SCM Registry as "no credentials configured".
        assert_eq!(
            KeychainResolver::parse_provider("github").unwrap(),
            ProviderId::Github,
        );
        assert_eq!(
            KeychainResolver::parse_provider("gitlab").unwrap(),
            ProviderId::Gitlab,
        );
        assert_eq!(
            KeychainResolver::parse_provider("linear").unwrap(),
            ProviderId::Linear,
        );
        assert_eq!(
            KeychainResolver::parse_provider("jira").unwrap(),
            ProviderId::Jira,
        );
    }
}
