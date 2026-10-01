//! CredentialResolver: the runtime's single point of truth for "which
//! credential should this provider call use right now?"

use anyhow::Result;
use async_trait::async_trait;

use rupu_providers::auth::AuthCredentials;
use rupu_providers::AuthMode;

/// Buffer (seconds) before expiry at which we proactively refresh.
pub const EXPIRY_REFRESH_BUFFER_SECS: i64 = 60;

/// Bound on one SSO token refresh (the whole HTTP exchange). The refresh
/// holds the account's refresh lock, so a stalled token endpoint must fail
/// rather than block every later `get` for that account.
pub const REFRESH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Bound on waiting for the auth file's lock (`auth.json.lock`). A holder
/// that never lets go — a SIGSTOPped process, a hung NFS lockd, a co-tenant
/// flocking the file — must fail the operation with a clear error, not hang
/// every refresh, login and logout (and, through them, the runtime's
/// shutdown).
pub const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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

    /// The refresher a provider client built from `provider`'s OAuth
    /// credentials should hand its token refreshes to, so the rotation is
    /// persisted in this store (under its lock) instead of kept in memory.
    /// `kind` is the vendor the caller resolved from config (`"anthropic"`,
    /// `"openai"`, `"gemini"`, …), so a credential stored under a name this
    /// resolver was never told about still gets a refresher. `None` — the
    /// default — leaves the client refreshing on its own.
    fn oauth_refresher(
        &self,
        provider: &str,
        kind: &str,
    ) -> Option<std::sync::Arc<dyn rupu_providers::credential_writes::OAuthRefresher>> {
        let _ = (provider, kind);
        None
    }
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
    /// [`REFRESH_TIMEOUT`]; tests shorten it.
    refresh_timeout: std::time::Duration,
    /// [`LOCK_TIMEOUT`]; tests shorten it.
    lock_timeout: std::time::Duration,
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
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
        }
    }

    /// Bound each SSO token refresh by `timeout` instead of
    /// [`REFRESH_TIMEOUT`].
    pub fn with_refresh_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.refresh_timeout = timeout;
        self
    }

    /// Bound each wait for the auth file's lock by `timeout` instead of
    /// [`LOCK_TIMEOUT`].
    pub fn with_lock_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.lock_timeout = timeout;
        self
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

    /// Write the whole map through a private temp file + rename
    /// ([`crate::private_file::write_private_atomic`]): a reader (in any
    /// process) never sees a half-written file, and no byte of it is ever
    /// visible with a mode looser than 0600. Callers hold the
    /// [`AuthFileLock`] around their read-modify-write.
    fn write_file_map(
        path: &std::path::Path,
        map: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let body =
            serde_json::to_string_pretty(map).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        crate::private_file::write_private_atomic(path, body.as_bytes())
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The file the credentials actually live in: `self.path` followed
    /// through any symlink. Resolved per operation (the link can appear or
    /// change between two calls), so a write replaces the link's target —
    /// never the link — and the lock sits next to that target.
    fn auth_file(&self) -> PathBuf {
        crate::private_file::resolve_symlink(&self.path)
    }

    /// Run `f` on the auth file under its [`AuthFileLock`], on the blocking
    /// pool (the lock waits — bounded — while another holder, in this or
    /// another process, is mid-way through its read-modify-write or
    /// refresh).
    async fn with_file_lock<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&std::path::Path) -> Result<T> + Send + 'static,
    {
        let path = self.auth_file();
        let lock_timeout = self.lock_timeout;
        tokio::task::spawn_blocking(move || {
            let _lock = AuthFileLock::acquire(&path, lock_timeout)?;
            f(&path)
        })
        .await
        .map_err(|e| anyhow::anyhow!("auth file task failed: {e}"))?
    }

    /// Insert one entry. The caller holds the [`AuthFileLock`].
    fn write_account_at(path: &std::path::Path, account: &str, payload: &str) -> Result<()> {
        let mut map = Self::read_file_map(path)?;
        map.insert(account.to_string(), payload.to_string());
        Self::write_file_map(path, &map)?;
        Ok(())
    }

    /// Remove one entry, under the [`AuthFileLock`].
    async fn delete_account(&self, account: &str) -> Result<()> {
        let account = account.to_string();
        self.with_file_lock(move |path| {
            let mut map = Self::read_file_map(path)?;
            if map.remove(&account).is_some() {
                Self::write_file_map(path, &map)?;
            }
            Ok(())
        })
        .await
    }

    pub async fn store(&self, p: ProviderId, mode: AuthMode, sc: &StoredCredential) -> Result<()> {
        let account = account_for(p, mode);
        let payload = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        self.with_file_lock(move |path| Self::write_account_at(path, &account, &payload))
            .await
    }

    pub async fn forget(&self, p: ProviderId, mode: AuthMode) -> Result<()> {
        self.delete_account(&account_for(p, mode)).await
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
        Self::read_account_at(&self.auth_file(), account_base, legacy_base, mode)
    }

    fn read_account_at(
        path: &std::path::Path,
        account_base: &str,
        legacy_base: Option<&str>,
        mode: AuthMode,
    ) -> Result<Option<StoredCredential>> {
        let account = format!("{account_base}/{}", mode.as_str());
        let map = Self::read_file_map(path)?;
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

    /// The Slice-A legacy bare-key fallback applies only to canonical
    /// vendor names. A declared account name (e.g. `anthropic-work`) was
    /// never written under a bare key, so extending the fallback to it
    /// would mean silently reading a different account's credential.
    fn legacy_base(provider: &str) -> Option<&str> {
        if Self::parse_provider(provider).is_ok() {
            Some(provider)
        } else {
            None
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
        let (name, sc) = (name.to_string(), sc.clone());
        self.with_file_lock(move |path| Self::store_named_at(path, &name, mode, &sc))
            .await
    }

    /// The caller holds the [`AuthFileLock`].
    fn store_named_at(
        path: &std::path::Path,
        name: &str,
        mode: AuthMode,
        sc: &StoredCredential,
    ) -> Result<()> {
        let account = format!("{name}/{}", mode.as_str());
        let payload = serde_json::to_string(sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        Self::write_account_at(path, &account, &payload)
    }

    /// Refresh `account`'s stored `mode` credential and persist it — as a
    /// tracked task ([`rupu_providers::credential_writes`], which the binary
    /// drains before exit), holding the auth file's [`AuthFileLock`] (and a
    /// process-local per-account lock in front of it) across re-read → token
    /// request → write.
    ///
    /// Cancel-safe: the OAuth server rotates the refresh token, so a refresh
    /// abandoned between its response and the write (the caller dropped — a
    /// pause, a listing timeout) would leave only a dead token behind. Here
    /// the caller only waits; the task finishes and persists.
    ///
    /// Single-flight across processes: the flock serializes every refresher
    /// and writer of the file — other `rupu` processes, and other clients in
    /// this one — and the stored credential is re-read under it, so a holder
    /// that finds it already rotated by someone else adopts it and sends no
    /// token request (one carrying the rotated-out refresh token would get
    /// `invalid_grant`, and reuse detection can revoke the whole grant). See
    /// [`RefreshWhen`] for when a request is still made.
    async fn refresh_and_store_at(
        path: PathBuf,
        account: String,
        kind: ProviderId,
        mode: AuthMode,
        timeout: std::time::Duration,
        lock_timeout: std::time::Duration,
        when: RefreshWhen,
    ) -> Result<StoredCredential> {
        let legacy = Self::legacy_base(&account).map(str::to_string);
        let job = rupu_providers::credential_writes::spawn(async move {
            let local = refresh_lock(&path, &account);
            let _local = local.lock().await;
            let _file = {
                let path = path.clone();
                tokio::task::spawn_blocking(move || AuthFileLock::acquire(&path, lock_timeout))
                    .await
                    .map_err(|e| anyhow::anyhow!("auth file lock task failed: {e}"))??
            };
            let sc = Self::read_account_at(&path, &account, legacy.as_deref(), mode)?
                .ok_or_else(|| anyhow::anyhow!("no stored credential for {account}/{mode:?}"))?;
            if when.already_satisfied(&sc) {
                return Ok(sc);
            }
            let new = Self::refresh_inner(&account, kind, &sc, timeout).await?;
            if let Err(e) = Self::store_named_at(&path, &account, mode, &new) {
                report_unpersisted_rotation(&path, &account, &e);
            }
            Ok(new)
        });
        job.await
            .map_err(|e| anyhow::anyhow!("credential refresh task failed: {e}"))?
    }

    async fn refresh_and_store(
        &self,
        account: &str,
        kind: ProviderId,
        mode: AuthMode,
        when: RefreshWhen,
    ) -> Result<StoredCredential> {
        Self::refresh_and_store_at(
            self.auth_file(),
            account.to_string(),
            kind,
            mode,
            self.refresh_timeout,
            self.lock_timeout,
            when,
        )
        .await
    }

    /// Forget a named credential. No-op if absent.
    pub async fn forget_named(&self, name: &str, mode: AuthMode) -> Result<()> {
        let account = format!("{name}/{}", mode.as_str());
        self.delete_account(&account).await
    }

    /// Delete every stored credential, returning how many entries were
    /// removed.
    ///
    /// Used by `rupu auth logout --all`. A fixed sweep over the builtin
    /// `ProviderId` list (the pre-multi-account implementation) can only
    /// ever see built-in vendor names — it has no way to know about a
    /// declared account like `anthropic-work`, so `--all` would report
    /// success while leaving named accounts' credentials behind. This
    /// clears whatever keys are actually on disk instead. A no-op
    /// (returns `0`, writes nothing) when the store is empty or absent,
    /// so `--all` on a fresh install doesn't create an empty auth.json.
    pub async fn forget_all(&self) -> Result<usize> {
        self.with_file_lock(|path| {
            let map = Self::read_file_map(path)?;
            let count = map.len();
            if count > 0 {
                Self::write_file_map(path, &Default::default())?;
            }
            Ok(count)
        })
        .await
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

    /// True if `RUPU_<ACCOUNT>_API_KEY` is set (and non-empty) for `account`.
    /// Reuses [`Self::env_api_key`]'s name-mangling transform so `rupu auth
    /// status` cannot drift from what `get_named` actually resolves.
    pub fn has_env_api_key(account: &str) -> bool {
        Self::env_api_key(account).is_some()
    }

    /// Resolve a provider name that isn't in `self.accounts` and isn't a
    /// bare vendor name either: `get`'s fallthrough for an
    /// `openai-compatible` account (which can only ever have an api-key)
    /// AND for any name the caller genuinely forgot to declare.
    ///
    /// Tries `auth.json["<name>/sso"]`, then `auth.json["<name>/api-key"]`
    /// (or legacy `["<name>"]`), then `RUPU_<UPPER_NAME>_API_KEY` —
    /// matching `get`'s SSO > API-key precedence, so an SSO credential
    /// stored under an undeclared name is still readable rather than
    /// silently shadowed by this being the api-key-only path it used to
    /// be. Unlike `get`, this path never refreshes a near-expiry SSO
    /// credential: refresh needs a `ProviderId` (`refresh_inner` takes a
    /// `kind`), which by definition doesn't exist for a name that isn't
    /// in `self.accounts` and isn't a vendor name — so a near-expiry
    /// token here is returned as-is, not refreshed.
    ///
    /// `hint` applies the same guard `get` applies on the declared/vendor
    /// path: an explicit `Some(Sso)` must never be silently satisfied by
    /// an API key, stored or env. `None` and `Some(ApiKey)` are
    /// unaffected — both still try stored SSO first, exactly as before
    /// this parameter existed, since callers passing `Some(ApiKey)` today
    /// rely on that same SSO-then-api-key-then-env precedence.
    async fn get_named(
        &self,
        provider: &str,
        hint: Option<AuthMode>,
    ) -> Result<(AuthMode, AuthCredentials)> {
        if let Some(sc) = self.read_account(provider, Some(provider), AuthMode::Sso)? {
            return Ok((AuthMode::Sso, sc.credentials));
        }
        if hint == Some(AuthMode::Sso) {
            anyhow::bail!(
                "no SSO credentials for '{provider}'. Run: rupu auth login --account {provider} \
                 --mode sso"
            )
        }
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

    /// Human-readable expiry string for a stored SSO credential. Always
    /// `Some`: a `None` `expires_at` (e.g. GitHub device-code grants that
    /// never carry an explicit expiry) renders as `Some("no expiry")` so
    /// the status row still shows ✓ rather than nothing.
    fn expiry_string(sc: &StoredCredential) -> Option<String> {
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

    /// Returns a human-readable expiry string for an SSO token, or `None`
    /// if no SSO credential exists for the provider.
    pub async fn peek_sso(&self, p: ProviderId) -> Option<String> {
        self.peek_sso_named(p.as_str()).await
    }

    /// `peek_sso` for an account name rather than a built-in vendor.
    /// Byte-identical lookup to `peek_sso(p)` when `name == p.as_str()`,
    /// since `legacy_account_for(p) == p.as_str()` (see `account_key.rs`).
    pub async fn peek_sso_named(&self, name: &str) -> Option<String> {
        let sc = self.read_account(name, Some(name), AuthMode::Sso).ok()??;
        Self::expiry_string(&sc)
    }

    /// The OAuth request itself is keyed on **vendor** (`kind`): two
    /// accounts of the same kind refresh against the same OAuth config.
    /// `account` is carried separately, only to name the right identity
    /// in user-facing error text — `kind` alone would tell a user with
    /// `anthropic-work` and `anthropic-personal` to re-authenticate the
    /// wrong one.
    async fn refresh_inner(
        account: &str,
        kind: ProviderId,
        sc: &StoredCredential,
        timeout: std::time::Duration,
    ) -> Result<StoredCredential> {
        let oauth = crate::oauth::providers::provider_oauth(kind)
            .ok_or_else(|| anyhow::anyhow!("no oauth config for {kind}"))?;
        let refresh_token = sc.refresh_token.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "SSO token for '{account}' expired and no refresh token stored. \
                 Re-authenticate this account (mode: sso) to continue."
            )
        })?;
        // The standard OAuth refresh-token grant, in the shape the vendor's
        // own client sends it (`ProviderOAuth::refresh_body_format`,
        // `client_secret`). Gemini's client pair follows the credential's
        // variant: a token issued to the Antigravity client must be
        // refreshed as that client.
        let (client_id, client_secret) = match kind {
            ProviderId::Gemini => {
                let hint = match &sc.credentials {
                    AuthCredentials::OAuth { extra, .. } => {
                        extra.get("variant").and_then(|v| v.as_str())
                    }
                    AuthCredentials::ApiKey { .. } => None,
                };
                let variant =
                    rupu_providers::google_gemini::GeminiVariant::from_credential_hint(hint);
                (variant.client_id(), Some(variant.client_secret()))
            }
            _ => (oauth.client_id, oauth.client_secret),
        };
        let mut params: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ];
        if let Some(secret) = client_secret {
            params.push(("client_secret", secret));
        }
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
            // Bounded: the caller holds the account's refresh lock.
            reqwest::Client::builder().timeout(timeout),
            std::sync::Arc::new(rupu_netflow::NullSink),
        )?;
        let request = match oauth.refresh_body_format {
            crate::oauth::providers::TokenBodyFormat::Form => client.post(&token_url).form(&params),
            crate::oauth::providers::TokenBodyFormat::Json => {
                let json: std::collections::BTreeMap<&str, &str> = params.into_iter().collect();
                client.post(&token_url).json(&json)
            }
        };
        let resp = request
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("refresh request: {e}"))?;
        if !resp.status().is_success() {
            anyhow::bail!(
                "refresh failed for '{account}': HTTP {}. Re-authenticate this account (mode: sso) to continue.",
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

/// The token endpoint has already rotated the refresh token when the write
/// of the new credential fails (a read-only directory, a full disk): the
/// credential in hand is the only live one. It is handed back so the run
/// continues, and the loss is reported loudly — the next process will find
/// the dead refresh token in the file and need `rupu auth login`.
fn report_unpersisted_rotation(path: &std::path::Path, account: &str, error: &anyhow::Error) {
    tracing::error!(
        path = %path.display(),
        account,
        error = %error,
        "the refreshed OAuth token could not be persisted; this process keeps using it, the \
         next one will need `rupu auth login`"
    );
    eprintln!(
        "rupu: the '{account}' token was refreshed but could not be written to {}: {error}. This \
         process keeps using the new token; if the next rupu command cannot authenticate, run: \
         rupu auth login --account {account} --mode sso",
        path.display()
    );
}

/// The process-wide lock serializing refreshes of one account's stored
/// credentials, keyed by auth file and account. Every `KeychainResolver` in
/// the process shares it: the CLI builds a fresh resolver per command and
/// per step.
fn refresh_lock(path: &std::path::Path, account: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    type Locks = std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>;
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<Locks>> = std::sync::OnceLock::new();
    let key = format!("{}#{account}", path.display());
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.entry(key).or_default().clone()
}

/// An exclusive advisory lock (flock) on `<auth file>.lock`, released when
/// dropped (the fd closes). Every writer of the auth file and every
/// refresher holds it across its whole read-modify-write — a refresher
/// across re-read → token request → write — so concurrent `rupu` processes
/// never clobber each other's writes or refresh the same token twice. flock
/// locks belong to an open file description, so two holders in one process
/// exclude each other too. The lock file is created private (0600) like
/// the auth file itself.
struct AuthFileLock {
    _file: std::fs::File,
}

impl AuthFileLock {
    fn path_for(auth_file: &std::path::Path) -> PathBuf {
        let mut s = auth_file.as_os_str().to_owned();
        s.push(".lock");
        s.into()
    }

    /// Blocks (the calling thread) until the lock is held, for at most
    /// `timeout`: a non-blocking `flock` polled with a short backoff, so a
    /// holder that never lets go yields an error instead of a hang.
    fn acquire(auth_file: &std::path::Path, timeout: std::time::Duration) -> Result<Self> {
        use fs2::FileExt;
        let path = Self::path_for(auth_file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("mkdir {}: {e}", parent.display()))?;
        }
        let file = crate::private_file::open_private_for_lock(&path)
            .map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
        let deadline = std::time::Instant::now() + timeout;
        let contended = fs2::lock_contended_error();
        let mut backoff = std::time::Duration::from_millis(5);
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(e)
                    if e.kind() == contended.kind()
                        && e.raw_os_error() == contended.raw_os_error() =>
                {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        anyhow::bail!(
                            "credential store is locked by another process ({}); retry, or \
                             find and stop the process holding it",
                            path.display()
                        );
                    }
                    std::thread::sleep(backoff.min(deadline - now));
                    backoff = (backoff * 2).min(std::time::Duration::from_millis(50));
                }
                Err(e) => anyhow::bail!("lock {}: {e}", path.display()),
            }
        }
    }
}

/// When [`KeychainResolver::refresh_and_store_at`] still makes a token
/// request after re-reading the stored credential under the lock.
#[derive(Debug, Clone)]
enum RefreshWhen {
    /// `get`: only if the stored credential is near expiry (another holder's
    /// rotation makes it fresh).
    NearExpiry,
    /// The forced `refresh` (a 401 mid-request): always, from the latest
    /// stored refresh token.
    Always,
    /// A provider client's refresh: unless another holder already rotated
    /// the stored credential away from `stale_refresh` (the refresh token
    /// the client holds) and it is not near expiry.
    RotatedFrom(Option<String>),
}

impl RefreshWhen {
    fn already_satisfied(&self, stored: &StoredCredential) -> bool {
        let fresh = !stored.is_near_expiry(chrono::Utc::now(), EXPIRY_REFRESH_BUFFER_SECS);
        match self {
            RefreshWhen::NearExpiry => fresh,
            RefreshWhen::Always => false,
            RefreshWhen::RotatedFrom(stale) => {
                fresh && stored_refresh_token(stored) != stale.as_deref()
            }
        }
    }
}

fn stored_refresh_token(sc: &StoredCredential) -> Option<&str> {
    match &sc.credentials {
        AuthCredentials::OAuth { refresh, .. } => Some(refresh.as_str()),
        AuthCredentials::ApiKey { .. } => sc.refresh_token.as_deref(),
    }
}

/// [`rupu_providers::credential_writes::OAuthRefresher`] over the
/// `KeychainResolver` store: a provider client's refresh goes through the
/// same lock, re-read and persisted write as the resolver's own.
struct KeychainRefresher {
    path: PathBuf,
    account: String,
    kind: ProviderId,
    timeout: std::time::Duration,
    lock_timeout: std::time::Duration,
}

#[async_trait]
impl rupu_providers::credential_writes::OAuthRefresher for KeychainRefresher {
    async fn refresh(
        &self,
        stale: AuthCredentials,
    ) -> std::result::Result<AuthCredentials, rupu_providers::ProviderError> {
        let stale_refresh = match &stale {
            AuthCredentials::OAuth { refresh, .. } => Some(refresh.clone()),
            AuthCredentials::ApiKey { .. } => None,
        };
        KeychainResolver::refresh_and_store_at(
            self.path.clone(),
            self.account.clone(),
            self.kind,
            AuthMode::Sso,
            self.timeout,
            self.lock_timeout,
            RefreshWhen::RotatedFrom(stale_refresh),
        )
        .await
        .map(|sc| sc.credentials)
        .map_err(|e| rupu_providers::ProviderError::TokenRefreshFailed(e.to_string()))
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
            let legacy = Self::legacy_base(provider);
            for mode in modes {
                if let Some(mut sc) = self.read_account(provider, legacy, mode)? {
                    let now = chrono::Utc::now();
                    if mode == AuthMode::Sso && sc.is_near_expiry(now, EXPIRY_REFRESH_BUFFER_SECS) {
                        sc = self
                            .refresh_and_store(provider, kind, mode, RefreshWhen::NearExpiry)
                            .await?;
                    }
                    return Ok((mode, sc.credentials));
                }
            }
            // Only fall back to the env API key when the caller didn't
            // explicitly ask for SSO. An explicit `hint = Some(Sso)` (e.g.
            // an agent's `auth: sso` frontmatter) means the user chose SSO
            // on purpose; silently handing back an API key just because
            // `RUPU_<VENDOR>_API_KEY` happens to be set would violate that
            // choice (docs/providers.md's "no automatic fall-back to
            // API-key" invariant) and mask the real problem — no SSO
            // credential is stored.
            if hint != Some(AuthMode::Sso) {
                if let Some(creds) = Self::env_api_key(provider) {
                    return Ok((AuthMode::ApiKey, creds));
                }
            }
            anyhow::bail!(
                "no credentials configured for {provider}. \
                 Run: rupu auth login --account {provider} --mode <api-key|sso>"
            )
        }

        // Not a vendor and not declared: an openai-compatible entry, or a
        // typo. `get_named` produces the actionable error either way.
        self.get_named(provider, hint).await
    }

    async fn refresh(&self, provider: &str, mode: AuthMode) -> Result<AuthCredentials> {
        let kind = crate::account::resolve_provider_id(provider, &self.accounts)
            .ok_or_else(|| anyhow::anyhow!("unknown provider or account: {provider}"))?;
        let new = self
            .refresh_and_store(provider, kind, mode, RefreshWhen::Always)
            .await?;
        Ok(new.credentials)
    }

    fn oauth_refresher(
        &self,
        provider: &str,
        kind: &str,
    ) -> Option<std::sync::Arc<dyn rupu_providers::credential_writes::OAuthRefresher>> {
        // A declared account (or a vendor name) knows its own kind; an
        // undeclared name takes the kind the caller resolved from config.
        // Either way the kind must have an OAuth config to refresh against.
        let kind = crate::account::resolve_provider_id(provider, &self.accounts)
            .or_else(|| ProviderId::from_vendor_str(kind))?;
        crate::oauth::providers::provider_oauth(kind)?;
        Some(std::sync::Arc::new(KeychainRefresher {
            path: self.auth_file(),
            account: provider.to_string(),
            kind,
            timeout: self.refresh_timeout,
            lock_timeout: self.lock_timeout,
        }))
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
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
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
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
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

    /// Spec §3.1 regression guard: a user with a modern `anthropic/api-key`
    /// entry and NO declared accounts must resolve exactly as before. This
    /// does NOT exercise the Slice-A bare-key fallback (`read_account`'s
    /// `legacy_base` branch) -- `store()` always writes the modern
    /// `<provider>/<mode>` key. See
    /// `bare_legacy_key_resolves_with_no_declared_accounts` below for the
    /// actual legacy-key path.
    #[tokio::test]
    async fn modern_key_still_resolves_with_no_declared_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: Vec::new(),
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
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

    /// Spec §3.1's actual regression guard: a user with ONLY the Slice-A
    /// bare `"anthropic"` key (no mode suffix, the on-disk shape every
    /// pre-mode-suffix install has) and NO declared accounts must still
    /// resolve. This is what exercises `read_account`'s `legacy_base`
    /// fallback branch, which the test above does not reach.
    #[tokio::test]
    async fn bare_legacy_key_resolves_with_no_declared_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"anthropic":"legacy-bare-key"}"#).unwrap();
        let resolver = KeychainResolver {
            path: path.clone(),
            accounts: Vec::new(),
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
        };

        let (mode, creds) = resolver.get("anthropic", None).await.unwrap();
        assert_eq!(mode, AuthMode::ApiKey);
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "legacy-bare-key"));
    }

    /// An undeclared, non-vendor name is a typo, not an account.
    #[tokio::test]
    async fn undeclared_account_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = KeychainResolver {
            path: dir.path().join("auth.json"),
            accounts: Vec::new(),
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
        };
        let err = resolver.get("anthropic-typo", None).await.unwrap_err();
        assert!(
            err.to_string().contains("anthropic-typo"),
            "error should name the offending string, got: {err}"
        );
    }

    /// `get_named` is the fallthrough for any name not in `self.accounts`
    /// (a typo, or a name the caller genuinely forgot to declare). Before
    /// Task 7 it tried only `AuthMode::ApiKey`, so an SSO credential
    /// stored under an undeclared name was silently unreadable even
    /// though it was sitting right there in `auth.json`.
    #[tokio::test]
    async fn get_named_reads_sso_credential_under_an_undeclared_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let resolver = KeychainResolver {
            path: path.clone(),
            // Deliberately NOT declared, so `get` routes into `get_named`.
            accounts: Vec::new(),
            refresh_timeout: REFRESH_TIMEOUT,
            lock_timeout: LOCK_TIMEOUT,
        };

        let sso = StoredCredential {
            credentials: AuthCredentials::OAuth {
                access: "undeclared-token".into(),
                refresh: "undeclared-refresh".into(),
                expires: 0,
                extra: Default::default(),
            },
            refresh_token: Some("undeclared-refresh".into()),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::days(30)),
        };
        resolver
            .store_named("oracle-personal", AuthMode::Sso, &sso)
            .await
            .unwrap();

        let (mode, creds) = resolver.get("oracle-personal", None).await.unwrap();
        assert_eq!(mode, AuthMode::Sso);
        assert!(
            matches!(creds, AuthCredentials::OAuth { access, .. } if access == "undeclared-token")
        );
    }
}
