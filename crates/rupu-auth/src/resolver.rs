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

use crate::account_key::{account_for, legacy_account_for};
use crate::backend::ProviderId;
use crate::stored::StoredCredential;
use std::path::PathBuf;

/// Production resolver: reads/writes [`StoredCredential`] JSON to a
/// chmod-600 file at `~/.rupu/auth.json`, overridable via `RUPU_HOME`
/// or `RUPU_AUTH_FILE`.
///
/// This is the only credential backend. The OS keychain was retired
/// because a bare CLI binary's keychain requirement is cdhash-bound:
/// every rebuild invalidates it and the next read silently fails, which
/// is how "my credentials vanished after an update" kept happening.
/// `gh`, `aws`, `gcloud`, `kubectl`, and `terraform` all store
/// credentials in files for the same reason.
///
/// On SSO entries whose access token is within [`EXPIRY_REFRESH_BUFFER_SECS`]
/// of expiry, [`KeychainResolver::get`] performs a silent token refresh via
/// the standard OAuth refresh-token grant before returning credentials.
pub struct KeychainResolver {
    /// Where credentials live: a chmod-600 JSON file. There is no
    /// second backend — see the type docs.
    path: PathBuf,
    /// Accounts declared in config. Empty means "built-in vendor names
    /// only", which is exactly the pre-multi-account behavior.
    accounts: Vec<crate::account::AccountSpec>,
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

    /// The `service` argument is retained for source compatibility with
    /// callers written against the keychain era; it no longer selects
    /// anything, because there is only one backend.
    pub fn with_service(_service: &str) -> Self {
        let path = std::env::var("RUPU_AUTH_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_auth_json_path());
        tracing::debug!(path = %path.display(), "credential store");
        Self {
            path,
            accounts: Vec::new(),
        }
    }

    /// Declare the config's accounts so `get` / `refresh` can resolve a
    /// named account to its vendor kind.
    ///
    /// `rupu-auth` cannot read config itself (hexagonal rule 1), so the
    /// CLI resolves the list and passes it here. Leaving this unset
    /// keeps the pre-multi-account behavior exactly.
    pub fn with_accounts(mut self, accounts: Vec<crate::account::AccountSpec>) -> Self {
        self.accounts = accounts;
        self
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

    fn write_account(&self, account: &str, payload: &str) -> Result<()> {
        let mut map = Self::read_file_map(&self.path)?;
        map.insert(account.to_string(), payload.to_string());
        Self::write_file_map(&self.path, &map)?;
        Ok(())
    }

    fn delete_account(&self, account: &str) -> Result<()> {
        let mut map = Self::read_file_map(&self.path)?;
        if map.remove(account).is_some() {
            Self::write_file_map(&self.path, &map)?;
        }
        Ok(())
    }

    pub async fn store(&self, p: ProviderId, mode: AuthMode, sc: &StoredCredential) -> Result<()> {
        let account = account_for(p, mode);
        let payload = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        self.write_account(&account, &payload)
    }

    pub async fn forget(&self, p: ProviderId, mode: AuthMode) -> Result<()> {
        self.delete_account(&account_for(p, mode))
    }

    /// Read a credential by its account *base* string (e.g. `"oracle"` or a
    /// built-in's `as_str()`), composing the `<base>/<mode>` store key
    /// exactly like [`account_for`]. The `legacy_base` (if any) is the bare
    /// account tried for api-key entries written before the mode suffix.
    fn read_account(
        &self,
        account_base: &str,
        legacy_base: Option<&str>,
        mode: AuthMode,
    ) -> Result<Option<StoredCredential>> {
        let account = format!("{account_base}/{}", mode.as_str());
        let map = Self::read_file_map(&self.path)?;
        if let Some(s) = map.get(&account) {
            return Ok(Some(parse_stored_credential(s, mode)?));
        }
        if mode == AuthMode::ApiKey {
            if let Some(lb) = legacy_base {
                if let Some(legacy) = map.get(lb) {
                    return Ok(Some(StoredCredential::api_key(legacy.clone())));
                }
            }
        }
        Ok(None)
    }

    fn read(&self, p: ProviderId, mode: AuthMode) -> Result<Option<StoredCredential>> {
        self.read_account(p.as_str(), Some(&legacy_account_for(p)), mode)
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

    /// Store an api-key/SSO credential under an arbitrary provider *name*
    /// (used for config-declared OpenAI-compatible providers).
    pub async fn store_named(
        &self,
        name: &str,
        mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<()> {
        let account = format!("{name}/{}", mode.as_str());
        let payload = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        self.write_account(&account, &payload)
    }

    /// Forget a named credential. No-op if absent.
    pub async fn forget_named(&self, name: &str, mode: AuthMode) -> Result<()> {
        let account = format!("{name}/{}", mode.as_str());
        self.delete_account(&account)
    }

    /// True if a named credential exists for `name`/`mode`.
    pub async fn peek_named(&self, name: &str, mode: AuthMode) -> bool {
        self.read_account(name, Some(name), mode)
            .map(|o| o.is_some())
            .unwrap_or(false)
    }

    /// `RUPU_<UPPER_ACCOUNT>_API_KEY`. Non-alphanumeric characters in an
    /// account name map to `_` so `anthropic-work` reads
    /// `RUPU_ANTHROPIC_WORK_API_KEY`.
    fn env_api_key(account: &str) -> Option<AuthCredentials> {
        let upper: String = account
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        let key = std::env::var(format!("RUPU_{upper}_API_KEY")).ok()?;
        if key.is_empty() {
            return None;
        }
        Some(AuthCredentials::ApiKey { key })
    }

    /// Resolve a config-declared provider name to an api-key credential:
    /// `auth.json["<name>/api-key"]` (or legacy `["<name>"]`), then
    /// `RUPU_<UPPER_NAME>_API_KEY`.
    async fn get_named(&self, provider: &str) -> Result<(AuthMode, AuthCredentials)> {
        if let Some(sc) = self.read_account(provider, Some(provider), AuthMode::ApiKey)? {
            return Ok((AuthMode::ApiKey, sc.credentials));
        }
        if let Some(creds) = Self::env_api_key(provider) {
            return Ok((AuthMode::ApiKey, creds));
        }
        anyhow::bail!(
            "no credentials for '{provider}'. Run: rupu auth login --account {provider} \
             --mode api-key, or set the matching RUPU_*_API_KEY env var"
        )
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

    /// `peek_sso` for an account name rather than a built-in vendor.
    /// Same human-readable expiry strings.
    pub async fn peek_sso_named(&self, name: &str) -> Option<String> {
        let sc = self.read_account(name, Some(name), AuthMode::Sso).ok()??;
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

    /// Refresh against a **vendor**, not an account name. Two accounts of
    /// the same kind refresh against the same OAuth config but store
    /// under their own keys — that is what makes multi-account SSO work.
    async fn refresh_inner(
        &self,
        kind: ProviderId,
        sc: &StoredCredential,
    ) -> Result<StoredCredential> {
        let oauth = crate::oauth::providers::provider_oauth(kind)
            .ok_or_else(|| anyhow::anyhow!("no oauth config for {kind}"))?;
        let refresh_token = sc.refresh_token.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "{kind} SSO token expired and no refresh token stored. \
                 Run: rupu auth login --provider {kind} --mode sso"
            )
        })?;
        // Provider-agnostic refresh: standard OAuth refresh-token grant.
        let token_url = std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
            .unwrap_or_else(|_| oauth.token_url.to_string());
        // Deliberately `NullSink`, not a stopgap: matt's scope call for this
        // plan was explicit — "I do not care about update or login" — and a
        // token refresh is login traffic even when it fires mid-run (a
        // credential nearing expiry gets refreshed inline, on whatever
        // thread happens to need it next, which can easily be mid-run).
        // The alternative — threading the calling run's sink all the way
        // into `CredentialResolver::refresh` — was considered and rejected
        // on that scope call, not deferred; there is no further wiring
        // planned for this call site. It is named in the disclosure's
        // exclusion list (`ScopeDisclosure.tsx`) so the absence is honest,
        // not silent.
        let client = rupu_netflow::http::client_with(
            rupu_netflow::FlowCtx::system(rupu_netflow::Origin::System),
            reqwest::Client::builder(),
            std::sync::Arc::new(rupu_netflow::NullSink),
        )?;
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
                "refresh failed for {kind}: HTTP {}. Run: rupu auth login --provider {kind} --mode sso",
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
        let modes: Vec<AuthMode> = match hint {
            Some(m) => vec![m],
            None => vec![AuthMode::Sso, AuthMode::ApiKey],
        };

        // A declared account, or a bare vendor name. Both read
        // `<name>/<mode>`; `account_for(pid, mode)` and
        // `format!("{name}/{mode}")` produce byte-identical keys, so the
        // legacy bare-vendor path is a special case of this one — it just
        // additionally tolerates the Slice-A legacy key.
        if let Some(kind) = crate::account::resolve_provider_id(provider, &self.accounts) {
            let legacy = if Self::parse_provider(provider).is_ok() {
                Some(provider)
            } else {
                None
            };
            for mode in modes {
                if let Some(mut sc) = self.read_account(provider, legacy, mode)? {
                    let now = chrono::Utc::now();
                    if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                        let new = self.refresh_inner(kind, &sc).await?;
                        self.store_named(provider, mode, &new).await?;
                        sc = new;
                    }
                    return Ok((mode, sc.credentials));
                }
            }
            if let Some(creds) = Self::env_api_key(provider) {
                return Ok((AuthMode::ApiKey, creds));
            }
            anyhow::bail!(
                "no credentials configured for {provider}. \
                 Run: rupu auth login --account {provider} --mode <api-key|sso>"
            )
        }

        // Not a vendor and not declared: an openai-compatible entry, or a
        // typo. `get_named` produces the actionable error either way.
        self.get_named(provider).await
    }

    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let kind = crate::account::resolve_provider_id(provider, &self.accounts)
            .ok_or_else(|| anyhow::anyhow!("unknown provider or account: {provider}"))?;
        let legacy = if Self::parse_provider(provider).is_ok() {
            Some(provider)
        } else {
            None
        };
        let sc = self
            .read_account(provider, legacy, mode)?
            .ok_or_else(|| anyhow::anyhow!("no stored credential for {provider}/{mode:?}"))?;
        let new = self.refresh_inner(kind, &sc).await?;
        self.store_named(provider, mode, &new).await?;
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
mod resolver_named_tests {
    use super::*;
    use rupu_providers::auth::AuthCredentials;

    /// Serialise all env-mutating tests through a single lock so they
    /// cannot race each other over shared process-global env vars. This must
    /// be an async-aware mutex (not `std::sync::Mutex`): the critical section
    /// spans the `.await` on `KeychainResolver::get`, which itself reads the
    /// env vars set just before it, so the guard genuinely has to be held
    /// across the await point to keep the whole read serialized.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// RAII guard: removes the listed env vars on drop, even on panic.
    struct EnvGuard(Vec<&'static str>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for k in &self.0 {
                std::env::remove_var(k);
            }
        }
    }

    #[tokio::test]
    async fn named_provider_reads_from_json_file() {
        let _lock = ENV_LOCK.lock().await;
        let _guard = EnvGuard(vec!["RUPU_AUTH_FILE", "RUPU_AUTH_BACKEND"]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{ "oracle/api-key": "sk-oracle-123" }"#).unwrap();
        std::env::set_var("RUPU_AUTH_FILE", &path);
        std::env::set_var("RUPU_AUTH_BACKEND", "file");

        let r = KeychainResolver::new();
        let (mode, creds) = r.get("oracle", None).await.unwrap();
        assert_eq!(mode, rupu_providers::AuthMode::ApiKey);
        match creds {
            AuthCredentials::ApiKey { key } => {
                assert_eq!(key, "sk-oracle-123")
            }
            _ => panic!("expected api key"),
        }
    }

    #[tokio::test]
    async fn named_provider_falls_back_to_env() {
        let _lock = ENV_LOCK.lock().await;
        let _guard = EnvGuard(vec![
            "RUPU_AUTH_FILE",
            "RUPU_AUTH_BACKEND",
            "RUPU_ACME_API_KEY",
        ]);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::env::set_var("RUPU_AUTH_FILE", &path);
        std::env::set_var("RUPU_AUTH_BACKEND", "file");
        std::env::set_var("RUPU_ACME_API_KEY", "sk-env-456");

        let r = KeychainResolver::new();
        let (_mode, creds) = r.get("acme", None).await.unwrap();
        match creds {
            AuthCredentials::ApiKey { key } => {
                assert_eq!(key, "sk-env-456")
            }
            _ => panic!("expected api key"),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_providers::auth::AuthCredentials;

    /// Two accounts of the same vendor store and read back independently.
    /// This is the core capability the whole arc exists to deliver.
    #[tokio::test]
    async fn two_accounts_of_same_kind_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: vec![
                crate::account::AccountSpec::new("anthropic-work", "anthropic"),
                crate::account::AccountSpec::new("anthropic-personal", "anthropic"),
            ],
        };

        resolver
            .store_named(
                "anthropic-work",
                AuthMode::ApiKey,
                &StoredCredential::api_key("work-key"),
            )
            .await
            .unwrap();
        resolver
            .store_named(
                "anthropic-personal",
                AuthMode::ApiKey,
                &StoredCredential::api_key("personal-key"),
            )
            .await
            .unwrap();

        let (mode, creds) = resolver.get("anthropic-work", None).await.unwrap();
        assert_eq!(mode, AuthMode::ApiKey);
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "work-key"));

        let (_, creds) = resolver.get("anthropic-personal", None).await.unwrap();
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "personal-key"));
    }

    /// Named accounts must support SSO, not just api-key. Before this
    /// task `get_named` only ever tried api-key.
    #[tokio::test]
    async fn named_account_resolves_sso_and_prefers_it_over_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: vec![crate::account::AccountSpec::new(
                "anthropic-work",
                "anthropic",
            )],
        };

        resolver
            .store_named(
                "anthropic-work",
                AuthMode::ApiKey,
                &StoredCredential::api_key("the-key"),
            )
            .await
            .unwrap();

        let sso = StoredCredential {
            credentials: AuthCredentials::OAuth {
                access: "the-token".into(),
                refresh: "the-refresh".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("the-refresh".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
        };
        resolver
            .store_named("anthropic-work", AuthMode::Sso, &sso)
            .await
            .unwrap();

        let (mode, creds) = resolver.get("anthropic-work", None).await.unwrap();
        assert_eq!(mode, AuthMode::Sso, "SSO must win over api-key");
        assert!(matches!(creds, AuthCredentials::OAuth { access, .. } if access == "the-token"));
    }

    /// Spec §3.1 regression guard: a user with only the legacy bare key
    /// and NO declared accounts must resolve exactly as before.
    #[tokio::test]
    async fn bare_builtin_still_resolves_with_no_declared_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: Vec::new(),
        };
        resolver
            .store(
                ProviderId::Anthropic,
                AuthMode::ApiKey,
                &StoredCredential::api_key("legacy"),
            )
            .await
            .unwrap();

        let (mode, creds) = resolver.get("anthropic", None).await.unwrap();
        assert_eq!(mode, AuthMode::ApiKey);
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "legacy"));
    }

    /// An undeclared, non-vendor name is a typo, not an account.
    #[tokio::test]
    async fn undeclared_account_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = KeychainResolver {
            path: dir.path().join("auth.json"),
            accounts: Vec::new(),
        };
        let err = resolver.get("anthropic-typo", None).await.unwrap_err();
        assert!(
            err.to_string().contains("anthropic-typo"),
            "error should name the offending string, got: {err}"
        );
    }
}
