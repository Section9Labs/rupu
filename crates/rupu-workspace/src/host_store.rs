//! Host registry. Lives at `~/.rupu/hosts/`.
//!
//! Stores connection metadata for named rupu hosts (local + remote CP instances).
//! Tokens are kept in the system keychain via `keyring`; only transport metadata
//! lives on disk. Tunnel hosts use token-hashed enrollment — the plaintext token
//! is returned once from [`enroll_node`] and never persisted.

use crate::repo_store::sanitize_component;
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tracing::warn;
use ulid::Ulid;

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
    /// Reverse tunnel: a remote node that initiated a WebSocket connection.
    /// The node is identified by its `node_id`; the server routes to the
    /// active connection held in the `NodeRegistry`.
    Tunnel { node_id: String },
}

/// Runtime-derived liveness indicator (not persisted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Offline,
    Stale,
}

/// Persisted host record (token lives in keychain, not here).
///
/// For `Tunnel` hosts an additional `token_hash` (SHA-256 hex of the node's
/// bearer token) is stored so the server can verify inbound connections without
/// keeping plaintext secrets on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub transport: HostTransport,
    /// SHA-256 hex digest of the node's enrollment token.
    /// Present only for `Tunnel` hosts; never the plaintext secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_hash: Option<String>,
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
            token_hash: None,
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
// Tunnel enrollment helpers
// ---------------------------------------------------------------------------

/// Hex-encode the SHA-256 digest of `input`.
fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// Enroll a new tunnel node.
///
/// Generates a `node_id` (`node_<ULID>`), a cryptographically random 32-byte
/// token (hex-encoded, 64 chars), stores a `Tunnel` `Host` whose `token_hash`
/// is `sha256_hex(token)`, and returns `(host, plaintext_token)`.
///
/// The plaintext token is returned **once** and never persisted.  Callers
/// should transmit it to the node over a secure channel (e.g. print it once
/// at enrollment time) and then forget it.
pub fn enroll_node(store: &HostStore, name: &str) -> Result<(Host, String), HostStoreError> {
    let node_id = format!("node_{}", Ulid::new());
    let mut token_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut token_bytes);
    let token = hex::encode(token_bytes);
    let token_hash = sha256_hex(&token);
    let host = Host {
        id: node_id.clone(),
        name: name.into(),
        transport: HostTransport::Tunnel { node_id },
        token_hash: Some(token_hash),
        created_at: Utc::now().to_rfc3339(),
        last_seen_at: None,
    };
    store.save(&host)?;
    Ok((host, token))
}

/// Verify a node's bearer token against the stored hash.
///
/// Uses a constant-time comparison (via `subtle`) so timing attacks cannot
/// leak prefix information about the stored hash.  Returns `false` when
/// `host.token_hash` is `None` (i.e. not a tunnel host).
pub fn verify_node_token(host: &Host, token: &str) -> bool {
    let Some(expected_hash) = &host.token_hash else {
        return false;
    };
    let computed = sha256_hex(token);
    let computed_bytes = computed.as_bytes();
    let expected_bytes = expected_hash.as_bytes();
    // SHA-256 hex is always 64 ASCII chars; lengths will always match for
    // well-formed hashes, but check defensively before the CT compare.
    if computed_bytes.len() != expected_bytes.len() {
        return false;
    }
    computed_bytes.ct_eq(expected_bytes).into()
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
            token_hash: None,
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

    // -----------------------------------------------------------------------
    // Tunnel / enrollment tests
    // -----------------------------------------------------------------------

    /// enroll_node returns a valid token and saves a Tunnel host whose
    /// token_hash matches sha256_hex(token).
    #[test]
    fn enroll_node_returns_token_and_saves_host() {
        let dir = tempdir().unwrap();
        let store = HostStore { root: dir.path().join("hosts") };

        let (host, token) = enroll_node(&store, "my-node").unwrap();

        // node_id is node_<ULID>
        assert!(host.id.starts_with("node_"), "id = {}", host.id);
        assert_eq!(host.name, "my-node");
        assert!(matches!(host.transport, HostTransport::Tunnel { .. }));
        if let HostTransport::Tunnel { ref node_id } = host.transport {
            assert_eq!(*node_id, host.id);
        }

        // token_hash must be sha256_hex(token)
        let expected_hash = sha256_hex(&token);
        assert_eq!(host.token_hash.as_deref(), Some(expected_hash.as_str()));

        // Host persisted to disk
        let loaded = store.load(&host.id).unwrap().unwrap();
        assert_eq!(loaded.token_hash, host.token_hash);
    }

    /// The TOML on disk must NOT contain the plaintext token.
    #[test]
    fn toml_does_not_contain_plaintext_token() {
        let dir = tempdir().unwrap();
        let store = HostStore { root: dir.path().join("hosts") };

        let (host, token) = enroll_node(&store, "secret-node").unwrap();

        // Read the raw TOML
        let toml_path = dir.path().join("hosts").join(format!("{}.toml", host.id));
        let toml_text = std::fs::read_to_string(&toml_path).unwrap();

        assert!(
            !toml_text.contains(&token),
            "TOML should not contain the plaintext token"
        );
    }

    /// verify_node_token: correct token → true, wrong token → false.
    #[test]
    fn verify_node_token_correct_and_wrong() {
        let dir = tempdir().unwrap();
        let store = HostStore { root: dir.path().join("hosts") };

        let (host, token) = enroll_node(&store, "node-a").unwrap();

        assert!(verify_node_token(&host, &token), "correct token must verify");
        assert!(!verify_node_token(&host, "wrong-token"), "wrong token must not verify");
        assert!(!verify_node_token(&host, ""), "empty token must not verify");
    }

    /// verify_node_token: returns false when token_hash is None.
    #[test]
    fn verify_node_token_none_hash_is_false() {
        let host = Host {
            id: "x".into(),
            name: "x".into(),
            transport: HostTransport::Local,
            token_hash: None,
            created_at: "2026-06-28T00:00:00Z".into(),
            last_seen_at: None,
        };
        assert!(!verify_node_token(&host, "any-token"));
    }

    /// Two enroll calls produce different node_ids and different tokens.
    #[test]
    fn enroll_node_is_unique() {
        let dir = tempdir().unwrap();
        let store = HostStore { root: dir.path().join("hosts") };

        let (h1, t1) = enroll_node(&store, "n1").unwrap();
        let (h2, t2) = enroll_node(&store, "n2").unwrap();

        assert_ne!(h1.id, h2.id);
        assert_ne!(t1, t2);
        assert_ne!(h1.token_hash, h2.token_hash);
    }

    /// Tunnel variant round-trips through TOML serde without data loss.
    #[test]
    fn tunnel_transport_serde_roundtrip() {
        let host = Host {
            id: "node_01J00000000000000000000000".into(),
            name: "test-node".into(),
            transport: HostTransport::Tunnel {
                node_id: "node_01J00000000000000000000000".into(),
            },
            token_hash: Some("abc123def456".into()),
            created_at: "2026-06-28T00:00:00Z".into(),
            last_seen_at: None,
        };
        let toml_str = toml::to_string(&host).unwrap();
        let decoded: Host = toml::from_str(&toml_str).unwrap();
        assert_eq!(host, decoded);
    }
}
