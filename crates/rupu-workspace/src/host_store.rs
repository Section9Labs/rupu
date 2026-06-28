//! Host registry. Lives at `~/.rupu/hosts/`.
//!
//! Stores connection metadata for named rupu hosts (local + remote CP instances).
//! Tokens are kept in the system keychain via `keyring`; only transport metadata
//! lives on disk.

use crate::repo_store::sanitize_component;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tracing::warn;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum HostStoreError {
    #[error("io {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// How rupu connects to a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostTransport {
    /// The local machine (no network hop).
    Local,
    /// Remote rupu-cp instance reachable over HTTP.
    HttpCp { base_url: String },
}

/// Runtime-derived liveness indicator (not persisted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Offline,
    Stale,
}

/// Persisted host record (token lives in keychain, not here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub transport: HostTransport,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

impl Host {
    /// The implicit localhost host (always host[0]).
    pub fn local() -> Self {
        Self {
            id: "local".into(),
            name: "local".into(),
            transport: HostTransport::Local,
            created_at: Utc::now().to_rfc3339(),
            last_seen_at: None,
        }
    }
}

// ---------------------------------------------------------------------------
// HostStore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HostStore {
    pub root: PathBuf,
}

impl HostStore {
    fn ensure_root(&self) -> Result<(), HostStoreError> {
        std::fs::create_dir_all(&self.root).map_err(|e| HostStoreError::Io {
            action: format!("create_dir_all {}", self.root.display()),
            source: e,
        })
    }

    fn record_path(&self, host_id: &str) -> PathBuf {
        self.root
            .join(format!("{}.toml", sanitize_component(host_id)))
    }

    pub fn save(&self, host: &Host) -> Result<(), HostStoreError> {
        self.ensure_root()?;
        let path = self.record_path(&host.id);
        let body = toml::to_string(host)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, body).map_err(|e| HostStoreError::Io {
            action: format!("write {}", tmp.display()),
            source: e,
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| HostStoreError::Io {
            action: format!("rename {} -> {}", tmp.display(), path.display()),
            source: e,
        })?;
        Ok(())
    }

    pub fn load(&self, host_id: &str) -> Result<Option<Host>, HostStoreError> {
        let path = self.record_path(host_id);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| HostStoreError::Io {
            action: format!("read {}", path.display()),
            source: e,
        })?;
        let host = toml::from_str(&text).map_err(|e| HostStoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(host))
    }

    pub fn list(&self) -> Result<Vec<Host>, HostStoreError> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|e| HostStoreError::Io {
            action: format!("read_dir {}", self.root.display()),
            source: e,
        })? {
            let entry = entry.map_err(|e| HostStoreError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping unreadable host record");
                    continue;
                }
            };
            let host: Host = match toml::from_str(&text) {
                Ok(h) => h,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping corrupt host record");
                    continue;
                }
            };
            out.push(host);
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub fn delete(&self, host_id: &str) -> Result<(), HostStoreError> {
        let path = self.record_path(host_id);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| HostStoreError::Io {
                action: format!("remove_file {}", path.display()),
                source: e,
            })?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Keyring helpers
// ---------------------------------------------------------------------------

const KEYRING_SERVICE: &str = "rupu-host";

/// Store a token for `host_id` in the system keychain.
pub fn set_host_token(host_id: &str, token: &str) -> Result<(), HostStoreError> {
    keyring::Entry::new(KEYRING_SERVICE, host_id)?.set_password(token)?;
    Ok(())
}

/// Retrieve the token for `host_id` from the system keychain.
/// Returns `Ok(None)` when no entry exists.
pub fn get_host_token(host_id: &str) -> Result<Option<String>, HostStoreError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, host_id)?;
    match entry.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(HostStoreError::Keyring(e)),
    }
}

/// Delete the token for `host_id` from the system keychain.
/// Silently succeeds if no entry exists.
pub fn delete_host_token(host_id: &str) -> Result<(), HostStoreError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, host_id)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(HostStoreError::Keyring(e)),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn http_host(id: &str) -> Host {
        Host {
            id: id.into(),
            name: format!("host {id}"),
            transport: HostTransport::HttpCp { base_url: "https://h:8787".into() },
            created_at: "2026-06-28T00:00:00Z".into(),
            last_seen_at: None,
        }
    }

    #[test]
    fn save_load_list_delete_roundtrip() {
        let dir = tempdir().unwrap();
        let store = HostStore { root: dir.path().join("hosts") };
        assert!(store.list().unwrap().is_empty());
        store.save(&http_host("host_a")).unwrap();
        store.save(&http_host("host_b")).unwrap();
        assert_eq!(store.list().unwrap().len(), 2);
        let a = store.load("host_a").unwrap().unwrap();
        assert!(matches!(a.transport, HostTransport::HttpCp { .. }));
        store.delete("host_a").unwrap();
        assert!(store.load("host_a").unwrap().is_none());
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn local_host_is_local_transport() {
        assert_eq!(Host::local().id, "local");
        assert!(matches!(Host::local().transport, HostTransport::Local));
    }
}
