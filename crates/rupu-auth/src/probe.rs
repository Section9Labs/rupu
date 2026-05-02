//! Backend probe + cached choice. Implemented in Task 15 of Plan 1.

use crate::backend::AuthBackend;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persisted choice of which backend to use; written to
/// `~/.rupu/cache/auth-backend.json` after a successful probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    Keyring,
    JsonFile,
}

/// Cache file location for the probe result.
pub struct ProbeCache {
    pub path: PathBuf,
}

/// Probe the OS keychain once (cached at `cache.path`) and return the
/// chosen backend. Falls back to a chmod-600 JSON file at
/// `fallback_path` when the keychain is unreachable.
pub fn select_backend(_cache: &ProbeCache, _fallback_path: PathBuf) -> Box<dyn AuthBackend> {
    todo!("select_backend lands in Task 15")
}
