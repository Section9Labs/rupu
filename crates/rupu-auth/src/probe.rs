//! Backend probe + cached choice. Avoids re-probing the OS keychain on
//! every CLI invocation by caching the result at
//! `~/.rupu/cache/auth-backend.json`.
//!
//! Cache invalidation is explicit: callers (e.g., `rupu auth login`)
//! can call [`ProbeCache::invalidate`] to force the next
//! [`select_backend`] call to re-probe.

use crate::backend::AuthBackend;
use crate::json_file::JsonFileBackend;
use crate::keyring::KeyringBackend;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

/// Persisted choice of which backend to use; written to
/// `~/.rupu/cache/auth-backend.json` after a successful probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    Keyring,
    JsonFile,
}

/// Cache file location for the probe result.
#[derive(Debug, Clone)]
pub struct ProbeCache {
    path: PathBuf,
}

impl ProbeCache {
    /// Construct a cache pointing at `path` (typically
    /// `~/.rupu/cache/auth-backend.json`).
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Cache file path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Read the cached backend choice, if any. Returns `None` if the
    /// file is absent. A corrupt cache file emits a `tracing::warn!`
    /// before returning `None`, so silent re-probes are diagnosable.
    pub fn read(&self) -> Option<BackendChoice> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        match serde_json::from_str(&text) {
            Ok(c) => Some(c),
            Err(e) => {
                warn!(
                    path = %self.path.display(),
                    error = %e,
                    "auth-backend cache is corrupt; will re-probe"
                );
                None
            }
        }
    }

    /// Write `c` as the cached choice. Creates parent directories as
    /// needed.
    pub fn write(&self, c: BackendChoice) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string(&c)
            .expect("BackendChoice serialization is infallible (Copy enum, two unit variants)");
        std::fs::write(&self.path, body)
    }

    /// Remove the cache file. No-op if it doesn't exist.
    pub fn invalidate(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Environment variable that, when set to `file` or `keyring`,
/// overrides the cached probe result and forces the named backend.
/// Lets users escape the macOS-keychain "prompts on every signed-
/// binary update" UX without flipping a config flag (and lets CI /
/// headless contexts bypass the keychain probe entirely).
///
/// Unrecognized values are ignored with a `tracing::warn!`, so a
/// typo (`RUPU_AUTH_BACKEND=files`) doesn't silently degrade.
pub const ENV_BACKEND_OVERRIDE: &str = "RUPU_AUTH_BACKEND";

/// Resolve which credential backend to use and return a boxed
/// `AuthBackend` ready for use.
///
/// Selection order (matches [`crate::resolver::KeychainResolver`]):
///   1. **Env override** (`RUPU_AUTH_BACKEND=file|keyring`) —
///      session-scoped opt-in, wins over cache.
///   2. **Cached choice** at `cache.path` — persistent choice set
///      by `rupu auth backend --use ...`.
///   3. **Default = `JsonFile`** at `fallback_path`.
///
/// The default flipped from `Keyring` → `JsonFile` to address the
/// macOS-keychain "credentials vanish after binary update" UX:
/// the keychain ACL pins each trusted-app entry to the binary's
/// cdhash, which changes on every rebuild. Most CLI peers (`gh`,
/// `aws`, `gcloud`, `claude-cli`, etc.) don't use the OS keychain
/// for the same reason; the file backend at chmod-600 is the
/// industry-standard answer. Users who specifically want
/// OS-managed encryption can opt in via
/// `rupu auth backend --use keychain`.
pub fn select_backend(cache: &ProbeCache, fallback_path: PathBuf) -> Box<dyn AuthBackend> {
    if let Some(override_choice) = read_env_override() {
        return materialize(override_choice, fallback_path);
    }

    let choice = cache.read().unwrap_or(BackendChoice::JsonFile);
    materialize(choice, fallback_path)
}

/// Parse the `RUPU_AUTH_BACKEND` env var if set. Returns `None`
/// (with a `tracing::warn!`) for an unrecognized value rather than
/// silently picking one — typos shouldn't auth-bypass the keychain.
fn read_env_override() -> Option<BackendChoice> {
    let raw = std::env::var(ENV_BACKEND_OVERRIDE).ok()?;
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "file" | "json" | "json-file" | "json_file" => Some(BackendChoice::JsonFile),
        "keyring" | "keychain" | "os" | "os-keychain" => Some(BackendChoice::Keyring),
        "" => None,
        other => {
            warn!(
                env = ENV_BACKEND_OVERRIDE,
                value = %other,
                "unrecognized backend override; using cached / probed choice instead",
            );
            None
        }
    }
}

fn materialize(choice: BackendChoice, fallback_path: PathBuf) -> Box<dyn AuthBackend> {
    match choice {
        BackendChoice::Keyring => Box::new(KeyringBackend::new()),
        BackendChoice::JsonFile => Box::new(JsonFileBackend {
            path: fallback_path,
        }),
    }
}
