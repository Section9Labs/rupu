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
/// shutdown). The same bound every credential-file writer uses
/// ([`rupu_providers::private_file::LOCK_TIMEOUT`]).
pub const LOCK_TIMEOUT: std::time::Duration = rupu_providers::private_file::LOCK_TIMEOUT;

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
        Self::at(path)
    }

    /// The resolver over the auth file under `home`: `<home>/auth.json`,
    /// unless `RUPU_AUTH_FILE` names the file outright — the same override
    /// [`KeychainResolver::new`] honours. For a caller that already holds
    /// the rupu home (`$RUPU_HOME`, else `~/.rupu`, in production — the
    /// same directory `new` resolves; a temporary one in tests) and must
    /// not have the resolver look the home up again on its own.
    pub fn for_home(home: &std::path::Path) -> Self {
        let path = std::env::var("RUPU_AUTH_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("auth.json"));
        Self::at(path)
    }

    /// The resolver over exactly this auth file.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
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
    /// ([`rupu_providers::private_file::write_private_atomic`]): a reader (in any
    /// process) never sees a half-written file, and no byte of it is ever
    /// visible with a mode looser than 0600. Callers hold the
    /// [`AuthFileLock`] around their read-modify-write.
    fn write_file_map(
        path: &std::path::Path,
        map: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let body =
            serde_json::to_string_pretty(map).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        rupu_providers::private_file::write_private_atomic(path, body.as_bytes())
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// The file the credentials actually live in: `self.path` followed
    /// through any symlink. Resolved per operation (the link can appear or
    /// change between two calls), so a write replaces the link's target —
    /// never the link — and the lock sits next to that target.
    fn auth_file(&self) -> PathBuf {
        rupu_providers::private_file::resolve_symlink(&self.path)
    }

    /// Run `f` on the auth file under its locks — the process-wide
    /// [`file_mutex`] in front, then the [`AuthFileLock`] on the blocking
    /// pool (it waits — bounded — while another process is mid-way through
    /// its read-modify-write or refresh).
    async fn with_file_lock<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&std::path::Path) -> Result<T> + Send + 'static,
    {
        Self::with_lock_at(self.auth_file(), self.lock_timeout, f).await
    }

    /// [`Self::with_file_lock`] on the auth file at `path` (already resolved
    /// through any symlink), waiting at most `lock_timeout` for its lock.
    async fn with_lock_at<T, F>(path: PathBuf, lock_timeout: std::time::Duration, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&std::path::Path) -> Result<T> + Send + 'static,
    {
        let mutex = file_mutex(&path);
        let _in_process = mutex.lock().await;
        tokio::task::spawn_blocking(move || {
            let _lock = AuthFileLock::acquire(&path, lock_timeout)?;
            f(&path)
        })
        .await
        .map_err(|e| anyhow::anyhow!("auth file task failed: {e}"))?
    }

    /// Insert one entry. The caller holds the [`AuthFileLock`]. Rotations an
    /// earlier write could not land ([`unpersisted`]) ride along, and leave
    /// the overlay once the file holds them; this write supersedes any
    /// pending one for its own account.
    fn write_account_at(path: &std::path::Path, account: &str, payload: &str) -> Result<()> {
        let mut map = Self::read_file_map(path)?;
        let folded = unpersisted::fold_into(path, &mut map);
        map.insert(account.to_string(), payload.to_string());
        Self::write_file_map(path, &map)?;
        unpersisted::clear_folded(path, &folded);
        unpersisted::remove(path, account);
        Ok(())
    }

    /// Remove one entry, under the [`AuthFileLock`]. A pending rotation of
    /// that account ([`unpersisted`]) is dropped with it — the logout
    /// supersedes it — and the other accounts' pending ones ride along.
    async fn delete_account(&self, account: &str) -> Result<()> {
        let account = account.to_string();
        self.with_file_lock(move |path| {
            let mut map = Self::read_file_map(path)?;
            let removed = map.remove(&account).is_some();
            unpersisted::remove(path, &account);
            let folded = unpersisted::fold_into(path, &mut map);
            if removed || !folded.is_empty() {
                Self::write_file_map(path, &map)?;
                unpersisted::clear_folded(path, &folded);
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

    /// The file's raw entry for `key` (`<account>/<mode>`), exactly as
    /// stored — what [`unpersisted`] compares a rotation's base against.
    fn file_entry_at(path: &std::path::Path, key: &str) -> Result<Option<String>> {
        Ok(Self::read_file_map(path)?.remove(key))
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

    /// Merge `fields` into `account`'s stored SSO credential's `extra`, if it
    /// is still the grant `holder` (the credential a client holds) came from
    /// — the same refresh token, or with none, the same access token; `false`
    /// (nothing written) when it is not — a re-login or logout since. The
    /// credential's own fields are never overwritten (reserved keys are
    /// dropped, as every writer of the credential drops them). The caller
    /// holds the [`AuthFileLock`]. A rotation the file could not take
    /// ([`unpersisted`]) is the credential merged into, and lands with it.
    fn record_extra_at(
        path: &std::path::Path,
        account: &str,
        holder: &AuthCredentials,
        fields: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<bool> {
        let key = format!("{account}/{}", AuthMode::Sso.as_str());
        let file_entry = Self::file_entry_at(path, &key)?;
        let current = match unpersisted::get_based_on(path, &key, file_entry.as_deref()) {
            Some(pending) => pending,
            None => match file_entry {
                Some(entry) => entry,
                None => return Ok(false),
            },
        };
        let mut sc = parse_stored_credential(&current, AuthMode::Sso)?;
        if !same_grant(&sc.credentials, holder) {
            return Ok(false);
        }
        let AuthCredentials::OAuth { extra, .. } = &mut sc.credentials else {
            return Ok(false);
        };
        for (k, v) in fields {
            extra.insert(k.clone(), v.clone());
        }
        sc.credentials.sanitize_extra();
        let payload = serde_json::to_string(&sc).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
        Self::write_account_at(path, &key, &payload)?;
        Ok(true)
    }

    /// Refresh `account`'s stored `mode` credential and persist it — as a
    /// tracked task ([`rupu_providers::credential_writes`], which the binary
    /// drains before exit), holding the auth file's [`AuthFileLock`] (and the
    /// process-wide [`file_mutex`] in front of it) across re-read → token
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
    ///
    /// A rotation the file could not take (the write failed after the token
    /// endpoint answered) is the live credential, kept in [`unpersisted`]
    /// with the file's entry it started from: while the file still holds
    /// that entry, it is what this re-read finds, ahead of the file, and the
    /// persist is retried here first — so no later refresh in this process
    /// posts the dead token the file still holds. Once the file's entry
    /// differs (another process logged the account in or out, or rotated it
    /// itself), the overlay entry is dropped and the file is the truth.
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
            let mutex = file_mutex(&path);
            let _in_process = mutex.lock().await;
            let _file = {
                let path = path.clone();
                tokio::task::spawn_blocking(move || AuthFileLock::acquire(&path, lock_timeout))
                    .await
                    .map_err(|e| anyhow::anyhow!("auth file lock task failed: {e}"))??
            };
            let key = format!("{account}/{}", mode.as_str());
            // The overlay's rotation is the live credential only while the
            // file still holds the entry it started from; a login, logout
            // or rotation by another process since supersedes it.
            let file_entry = Self::file_entry_at(&path, &key)?;
            let sc = match unpersisted::get_based_on(&path, &key, file_entry.as_deref()) {
                Some(pending) => {
                    let sc = parse_stored_credential(&pending, mode)?;
                    // Land it now if the file will take it (the exact
                    // pending payload, so the file then holds what the
                    // overlay held); the entry goes with a successful write.
                    if let Err(e) = Self::write_account_at(&path, &key, &pending) {
                        tracing::warn!(
                            path = %path.display(),
                            account,
                            error = %e,
                            "the refreshed OAuth token still could not be persisted"
                        );
                    }
                    sc
                }
                None => Self::read_account_at(&path, &account, legacy.as_deref(), mode)?
                    .ok_or_else(|| {
                        anyhow::anyhow!("no stored credential for {account}/{mode:?}")
                    })?,
            };
            if when.already_satisfied(&sc) {
                return Ok(sc);
            }
            // The base this rotation starts from: the file's entry as it is
            // now, under the lock (the retry above may just have landed
            // the pending one). Read before the token request, so a read
            // failure costs nothing.
            let base = Self::file_entry_at(&path, &key)?;
            let new = Self::refresh_inner(&account, kind, &sc, timeout).await?;
            if let Err(e) = Self::store_named_at(&path, &account, mode, &new) {
                let payload =
                    serde_json::to_string(&new).map_err(|e| anyhow::anyhow!("serialize: {e}"))?;
                unpersisted::set(&path, &key, payload, base);
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
            // Pending rotations of this file go too: nothing is left to
            // persist them for.
            unpersisted::clear_file(path);
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

    /// Whether a credential exists for the account `name`: a stored
    /// api-key or SSO credential, or `RUPU_<ACCOUNT>_API_KEY` — the places
    /// [`CredentialResolver::get`] reads for a named account, and what a
    /// connector build (`Registry::discover`) needs before it registers the
    /// account. Existence only: nothing is refreshed or validated.
    ///
    /// Unlike [`Self::peek_named`], an unreadable store is an error rather
    /// than "absent", so a caller can tell "no credential" from "could not
    /// look".
    pub fn has_credential_named(&self, name: &str) -> Result<bool> {
        if Self::has_env_api_key(name) {
            return Ok(true);
        }
        for mode in [AuthMode::ApiKey, AuthMode::Sso] {
            if self.read_account(name, Some(name), mode)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
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
        // A GitLab credential that recorded the application it was issued
        // to (a login with a chosen application: the account's configured
        // one, or its self-managed instance's — `oauth::callback::
        // run_with_client`) refreshes as that public client, at that
        // endpoint, which the built-in endpoints' test seam never
        // redirects. Only GitLab logins record one; any other vendor's
        // free-form `extra` is never read for an endpoint.
        let recorded = |key: &str| match &sc.credentials {
            AuthCredentials::OAuth { extra, .. } if kind == ProviderId::Gitlab => {
                extra.get(key).and_then(|v| v.as_str())
            }
            _ => None,
        };
        let (client_id, client_secret) = match recorded(crate::oauth::providers::EXTRA_CLIENT_ID) {
            Some(id) => (id, None),
            None => (client_id, client_secret),
        };
        let mut params: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ];
        if let Some(secret) = client_secret {
            params.push(("client_secret", secret));
        }
        let token_url = match recorded(crate::oauth::providers::EXTRA_TOKEN_URL) {
            Some(url) => url.to_string(),
            None => std::env::var("RUPU_OAUTH_TOKEN_URL_OVERRIDE")
                .unwrap_or_else(|_| oauth.token_url.to_string()),
        };
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
            /// OpenAI's ChatGPT grant rotates the ID token with the access
            /// token; it is persisted (`extra.id_token`) as codex-rs does,
            /// since the Codex client takes its account id from it when
            /// the access token carries no claim. OpenAI's only: any other
            /// provider's (Gemini's carries the user's email) is dropped.
            #[serde(default)]
            id_token: Option<String>,
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
        let mut prior_extra = match &sc.credentials {
            rupu_providers::auth::AuthCredentials::OAuth { extra, .. } => extra.clone(),
            _ => Default::default(),
        };
        if kind == ProviderId::Openai {
            if let Some(id_token) = r.id_token {
                prior_extra.insert("id_token".into(), serde_json::Value::String(id_token));
            }
        } else {
            // Not stored for any other provider — and one a login stored
            // before this rule does not survive the refresh either.
            prior_extra.remove("id_token");
        }
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
/// continues and kept in [`unpersisted`] — every later refresh of the
/// account and every later write of the file in this process retry the
/// write — and the loss is reported loudly: the next process will find the
/// dead refresh token in the file and need `rupu auth login` unless a later
/// write in this one lands it first.
fn report_unpersisted_rotation(path: &std::path::Path, account: &str, error: &anyhow::Error) {
    tracing::error!(
        path = %path.display(),
        account,
        error = %error,
        "the refreshed OAuth token could not be persisted; this process keeps using it and \
         retries the write on its next write of the credential file or refresh of this account, \
         the next process will need `rupu auth login` unless one lands"
    );
    eprintln!(
        "rupu: the '{account}' token was refreshed but could not be written to {}: {error}. This \
         process keeps using the new token and retries the write on its next write of the \
         credential file or refresh of this account; if the next rupu command cannot \
         authenticate, run: rupu auth login --account {account} --mode sso",
        path.display()
    );
}

/// The process-wide lock serializing this process's holders of one auth
/// file, keyed by the resolved file, taken in front of the [`AuthFileLock`]
/// by every writer and refresher. Every `KeychainResolver` in the process
/// shares it (the CLI builds a fresh resolver per command and per step), so
/// in-process exclusion holds even where `flock` is per-process (an NFS
/// mount on Linux). A holder keeps it only for bounded work — the lock
/// wait, one token request, a write — so a wait on it is bounded too.
fn file_mutex(path: &std::path::Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    type Locks = std::collections::HashMap<PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>;
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<Locks>> = std::sync::OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    locks.entry(path.to_path_buf()).or_default().clone()
}

/// Rotated credentials the auth file could not take: the token endpoint had
/// already rotated the refresh token when the write failed (a read-only
/// directory, a full disk), so the credential in hand is the only live one
/// and the file keeps a dead refresh token. Keyed by the resolved auth file
/// and the file-map key (`<account>/<mode>`); process-wide, like the file
/// mutex, and only ever touched under the [`AuthFileLock`].
///
/// Each entry remembers its base: the file's entry for the key as it was
/// when the rotation started (what the dead refresh token is stored in).
/// The entry is the live credential only while the file still holds that
/// base. Every later refresh of the account reads it ahead of the file, and
/// every later write of the file folds it in — each after comparing the
/// file's current entry with the base: a file that moved on (another
/// process logged the account in or out, or rotated it itself) supersedes
/// the entry, which is dropped, so a stale rotation is never written over
/// a newer credential and never brings back a logged-out one. An entry also
/// leaves once the file holds it, or when the account is forgotten.
/// Without the overlay, every later refresh in the process re-read the
/// file and posted the dead token — `invalid_grant`, and reuse detection
/// can revoke the whole grant.
mod unpersisted {
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// A rotation the file could not take, and the file's entry for its
    /// key when it started (`None`: the file had none).
    struct Entry {
        payload: String,
        base: Option<String>,
    }

    type Overlay = HashMap<(PathBuf, String), Entry>;

    fn entries() -> MutexGuard<'static, Overlay> {
        static ENTRIES: OnceLock<Mutex<Overlay>> = OnceLock::new();
        ENTRIES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn superseded(path: &Path, key: &str) {
        tracing::info!(
            path = %path.display(),
            account = key,
            "dropping an unpersisted token rotation: the stored credential was rewritten by \
             another process since (a login, logout or its own refresh); using the file"
        );
    }

    /// The pending payload for `key` in `path`, if its base is still what
    /// the file holds (`file_entry`). An entry whose base the file no
    /// longer holds is superseded: dropped, and `None`.
    pub(super) fn get_based_on(path: &Path, key: &str, file_entry: Option<&str>) -> Option<String> {
        let mut entries = entries();
        let k = (path.to_path_buf(), key.to_string());
        let entry = entries.get(&k)?;
        if entry.base.as_deref() == file_entry {
            return Some(entry.payload.clone());
        }
        entries.remove(&k);
        drop(entries);
        superseded(path, key);
        None
    }

    /// Record a rotation the file could not take, started from `base` —
    /// the file's entry for `key` at that moment (a newer rotation replaces
    /// an older one for the same key).
    pub(super) fn set(path: &Path, key: &str, payload: String, base: Option<String>) {
        entries().insert(
            (path.to_path_buf(), key.to_string()),
            Entry { payload, base },
        );
    }

    /// Drop the pending rotation for `key`, if any (superseded by a write
    /// of that account, or by its logout).
    pub(super) fn remove(path: &Path, key: &str) {
        entries().remove(&(path.to_path_buf(), key.to_string()));
    }

    /// Put every pending rotation for `path` whose base `map` (the file,
    /// just read under the lock) still holds into `map` — they are newer
    /// than what the file holds. One whose base the file no longer holds is
    /// superseded and dropped instead. Returns what was folded, for
    /// [`clear_folded`] once the write succeeded.
    pub(super) fn fold_into(
        path: &Path,
        map: &mut BTreeMap<String, String>,
    ) -> Vec<(String, String)> {
        let mut folded = Vec::new();
        let mut stale = Vec::new();
        {
            let entries = entries();
            for ((p, key), entry) in entries.iter() {
                if p != path {
                    continue;
                }
                if map.get(key).map(String::as_str) == entry.base.as_deref() {
                    folded.push((key.clone(), entry.payload.clone()));
                } else {
                    stale.push(key.clone());
                }
            }
        }
        for key in stale {
            remove(path, &key);
            superseded(path, &key);
        }
        for (key, payload) in &folded {
            map.insert(key.clone(), payload.clone());
        }
        folded
    }

    /// Drop the entries `folded` that are still exactly what was folded —
    /// the file holds them now. A rotation recorded since stays.
    pub(super) fn clear_folded(path: &Path, folded: &[(String, String)]) {
        let mut entries = entries();
        for (key, payload) in folded {
            let k = (path.to_path_buf(), key.clone());
            if entries.get(&k).map(|e| &e.payload) == Some(payload) {
                entries.remove(&k);
            }
        }
    }

    /// Drop every pending rotation for `path` (`forget_all`).
    pub(super) fn clear_file(path: &Path) {
        entries().retain(|(p, _), _| p != path);
    }
}

/// The exclusive advisory lock (flock) on `<auth file>.lock` every writer
/// of the auth file and every refresher holds across its whole
/// read-modify-write — a refresher across re-read → token request → write
/// — so concurrent `rupu` processes never clobber each other's writes or
/// refresh the same token twice. The shared
/// [`rupu_providers::private_file::SidecarLock`]: created private, never
/// unlinked, and acquired with a bounded wait ([`LOCK_TIMEOUT`]).
type AuthFileLock = rupu_providers::private_file::SidecarLock;

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

/// Whether `stored` is the grant `holder` came from: the same refresh token
/// — or, for a grant issued without one, the same access token (two logins
/// without refresh tokens would otherwise look alike).
fn same_grant(stored: &AuthCredentials, holder: &AuthCredentials) -> bool {
    match (stored, holder) {
        (
            AuthCredentials::OAuth {
                access: stored_access,
                refresh: stored_refresh,
                ..
            },
            AuthCredentials::OAuth {
                access: holder_access,
                refresh: holder_refresh,
                ..
            },
        ) if holder_refresh.is_empty() => {
            stored_refresh.is_empty() && stored_access == holder_access
        }
        (
            AuthCredentials::OAuth {
                refresh: stored_refresh,
                ..
            },
            AuthCredentials::OAuth {
                refresh: holder_refresh,
                ..
            },
        ) => stored_refresh == holder_refresh,
        _ => false,
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

    /// As a tracked task ([`rupu_providers::credential_writes`], drained
    /// before exit), holding the process-wide [`file_mutex`] and the
    /// [`AuthFileLock`] across re-read → merge → write: a caller dropped
    /// mid-way only stops waiting, and never lets go of the mutex while the
    /// write still runs.
    async fn record_extra(
        &self,
        holder: AuthCredentials,
        fields: std::collections::HashMap<String, serde_json::Value>,
    ) -> std::result::Result<bool, rupu_providers::ProviderError> {
        let (path, account, lock_timeout) =
            (self.path.clone(), self.account.clone(), self.lock_timeout);
        let job = rupu_providers::credential_writes::spawn(async move {
            KeychainResolver::with_lock_at(path, lock_timeout, move |path| {
                KeychainResolver::record_extra_at(path, &account, &holder, &fields)
            })
            .await
        });
        job.await
            .map_err(|e| {
                rupu_providers::ProviderError::AuthConfig(format!(
                    "credential write task failed: {e}"
                ))
            })?
            .map_err(|e| rupu_providers::ProviderError::AuthConfig(e.to_string()))
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

    // `#[serial]`: sets `RUPU_AUTH_FILE`, which `new` / `with_service` /
    // `for_home` read.
    #[tokio::test]
    #[serial_test::serial]
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

    // `#[serial]`: sets `RUPU_AUTH_FILE` and an `RUPU_*_API_KEY`.
    #[tokio::test]
    #[serial_test::serial]
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

    /// `for_home` reads the auth file under that home and nowhere else:
    /// a credential stored at `<home>/auth.json` (through `at`) resolves
    /// there, and the same account under another home resolves to nothing
    /// — with no file created there, and no look-up of `$RUPU_HOME` or
    /// `~/.rupu`. `#[serial]`: `for_home` honours `RUPU_AUTH_FILE`, which
    /// other tests of this module set.
    #[tokio::test]
    #[serial_test::serial]
    async fn for_home_reads_the_auth_file_under_that_home_only() {
        let home = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let account = crate::account::AccountSpec::new("rooted-probe", "github");
        KeychainResolver::at(home.path().join("auth.json"))
            .with_accounts(vec![account.clone()])
            .store_named(
                "rooted-probe",
                AuthMode::ApiKey,
                &StoredCredential::api_key("home-token"),
            )
            .await
            .unwrap();
        assert!(home.path().join("auth.json").is_file());

        let (mode, creds) = KeychainResolver::for_home(home.path())
            .with_accounts(vec![account.clone()])
            .get("rooted-probe", None)
            .await
            .unwrap();
        assert_eq!(mode, AuthMode::ApiKey);
        assert!(matches!(creds, AuthCredentials::ApiKey { key } if key == "home-token"));

        let elsewhere = KeychainResolver::for_home(other.path())
            .with_accounts(vec![account])
            .get("rooted-probe", None)
            .await;
        assert!(
            elsewhere.is_err(),
            "another home holds no credential: {elsewhere:?}"
        );
        assert!(
            !other.path().join("auth.json").exists(),
            "a read creates nothing"
        );
    }

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
