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
    pub path: PathBuf,
}

impl ProbeCache {
    /// Read the cached backend choice, if any. Returns `None` if the
    /// file is absent or cannot be parsed.
    pub fn read(&self) -> Option<BackendChoice> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write `c` as the cached choice. Creates parent directories as
    /// needed.
    pub fn write(&self, c: BackendChoice) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string(&c).unwrap())
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

/// Probe the OS keychain (or read the cached choice if present) and
/// return a boxed `AuthBackend` ready for use.
///
/// On a fresh probe the keychain is preferred; if [`KeyringBackend::probe`]
/// fails, falls back to a [`JsonFileBackend`] at `fallback_path` with a
/// `tracing::warn!` explaining the fallback. The choice is cached at
/// `cache.path` so subsequent invocations skip the probe.
pub fn select_backend(cache: &ProbeCache, fallback_path: PathBuf) -> Box<dyn AuthBackend> {
    let choice = cache.read().unwrap_or_else(|| {
        let chosen = match KeyringBackend::probe() {
            Ok(()) => BackendChoice::Keyring,
            Err(e) => {
                warn!(
                    error = %e,
                    fallback = %fallback_path.display(),
                    "OS keychain unavailable; falling back to chmod-600 JSON file"
                );
                BackendChoice::JsonFile
            }
        };
        if let Err(e) = cache.write(chosen) {
            warn!(
                error = %e,
                cache = %cache.path.display(),
                "failed to write probe cache; will re-probe next run"
            );
        }
        chosen
    });

    match choice {
        BackendChoice::Keyring => Box::new(KeyringBackend::new()),
        BackendChoice::JsonFile => Box::new(JsonFileBackend {
            path: fallback_path,
        }),
    }
}
