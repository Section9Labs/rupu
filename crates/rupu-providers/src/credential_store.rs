//! CredentialStore — concrete implementation of CredentialSource.
//!
//! Backs credentials with `cortex/auth.json` (source of truth) and
//! provider health state with `cortex/auth_status.json`. Uses fs2 file
//! locking for atomic writes and notify for fsnotify watching.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use fs2::FileExt;
// Note: fsnotify-based automatic reload is a future enhancement.
// Currently, reload() must be called explicitly (by the router on
// provider exhaustion, or by a supervisor). The watcher needs Arc<Self>
// which requires construction changes.
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::auth::{AuthCredentials, AuthFile};
use crate::credential_source::{CredentialSource, ProviderAuthStatus};
use crate::error::ProviderError;
use crate::provider_id::ProviderId;

/// Persisted invalidation entry for auth_status.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedInvalidation {
    status: String,
    reason: String,
    since: DateTime<Utc>,
    retry_after_secs: Option<u64>,
}

/// In-memory invalidation state (includes Instant for retry_after timing).
#[derive(Debug, Clone)]
struct InvalidationEntry {
    reason: String,
    since: Instant,
    since_utc: DateTime<Utc>,
    retry_after: Option<Duration>,
}

/// Hash credentials to a fingerprint for change detection on reload.
/// Uses a simple hash to avoid storing a second copy of secrets in memory.
fn cred_fingerprint(creds: &AuthCredentials) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let json = serde_json::to_string(creds).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The unified credential store. Implements `CredentialSource`.
///
/// Disk is always the source of truth. In-memory caches are synced via
/// fsnotify or explicit `reload()`. File locking prevents concurrent
/// write corruption across multiple cell processes.
pub struct CredentialStore {
    auth_path: PathBuf,
    status_path: PathBuf,
    credentials: RwLock<HashMap<ProviderId, AuthCredentials>>,
    fingerprints: RwLock<HashMap<ProviderId, String>>,
    invalidations: RwLock<HashMap<ProviderId, InvalidationEntry>>,
}

type CredentialStoreLoadResult = Result<
    (
        HashMap<ProviderId, AuthCredentials>,
        HashMap<ProviderId, String>,
    ),
    ProviderError,
>;

impl CredentialStore {
    /// Load the credential store from disk. Creates files if they don't exist.
    ///
    /// `reload()` must be called explicitly when external changes to auth.json
    /// need to be picked up (e.g., by the router on provider exhaustion).
    /// fsnotify-based automatic reload is a future enhancement.
    pub fn load(auth_path: PathBuf, status_path: PathBuf) -> Result<Self, ProviderError> {
        if let Some(parent) = auth_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ProviderError::AuthConfig(format!("cannot create dir: {e}")))?;
        }

        let (credentials, fingerprints) = Self::read_auth_file(&auth_path)?;
        let invalidations = Self::read_status_file(&status_path);

        info!(
            path = %auth_path.display(),
            providers = credentials.len(),
            "credential store loaded"
        );

        Ok(Self {
            auth_path,
            status_path,
            credentials: RwLock::new(credentials),
            fingerprints: RwLock::new(fingerprints),
            invalidations: RwLock::new(invalidations),
        })
    }

    fn read_auth_file(path: &Path) -> CredentialStoreLoadResult {
        let content = if path.exists() {
            fs::read_to_string(path).map_err(|e| {
                ProviderError::AuthConfig(format!("cannot read {}: {e}", path.display()))
            })?
        } else {
            "{}".to_string()
        };

        let raw: AuthFile = serde_json::from_str(&content)
            .map_err(|e| ProviderError::AuthConfig(format!("invalid auth.json: {e}")))?;

        let mut creds = HashMap::new();
        let mut fps = HashMap::new();
        for (key, val) in raw {
            if let Ok(id) = key.parse::<ProviderId>() {
                fps.insert(id, cred_fingerprint(&val));
                creds.insert(id, val);
            }
        }

        Ok((creds, fps))
    }

    fn read_status_file(path: &Path) -> HashMap<ProviderId, InvalidationEntry> {
        let mut result = HashMap::new();
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return result,
        };

        let raw: HashMap<String, PersistedInvalidation> = match serde_json::from_str(&content) {
            Ok(r) => r,
            Err(_) => return result,
        };

        for (key, entry) in raw {
            if entry.status != "invalidated" {
                continue;
            }
            if let Ok(id) = key.parse::<ProviderId>() {
                result.insert(
                    id,
                    InvalidationEntry {
                        reason: entry.reason,
                        since: Instant::now(),
                        since_utc: entry.since,
                        retry_after: entry.retry_after_secs.map(Duration::from_secs),
                    },
                );
            }
        }

        result
    }

    /// Write a specific provider's credentials to auth.json.
    /// Only writes the given provider — does not resurrect removed providers.
    /// Clones credentials before file I/O to avoid holding RwLock across blocking ops.
    fn write_provider_to_auth(
        &self,
        provider: ProviderId,
        creds: &AuthCredentials,
    ) -> Result<(), ProviderError> {
        let mut creds = creds.clone();
        creds.sanitize_extra();
        let creds_value =
            serde_json::to_value(&creds).map_err(|e| ProviderError::AuthConfig(e.to_string()))?;

        // File-locked read-modify-write (no RwLock held during I/O)
        let lock_path = self.auth_path.with_extension("lock");
        let lock_file = File::create(&lock_path)
            .map_err(|e| ProviderError::AuthConfig(format!("cannot create lock file: {e}")))?;
        lock_file
            .lock_exclusive()
            .map_err(|e| ProviderError::AuthConfig(format!("cannot acquire file lock: {e}")))?;

        // Read current disk state (another process may have updated)
        let disk_content = fs::read_to_string(&self.auth_path).unwrap_or_else(|_| "{}".into());
        let mut disk: serde_json::Value =
            serde_json::from_str(&disk_content).unwrap_or_else(|_| serde_json::json!({}));

        // Write only the specific provider
        disk[provider.auth_key()] = creds_value;

        // Write atomically: temp → fsync → rename
        // Use thread ID in temp name to avoid collisions under the file lock
        let tmp = self
            .auth_path
            .with_extension(format!("tmp.{:?}", std::thread::current().id()));
        let mut file = File::create(&tmp)
            .map_err(|e| ProviderError::AuthConfig(format!("cannot create temp file: {e}")))?;
        let json = serde_json::to_string_pretty(&disk)
            .map_err(|e| ProviderError::AuthConfig(e.to_string()))?;
        file.write_all(json.as_bytes())
            .map_err(|e| ProviderError::AuthConfig(format!("write failed: {e}")))?;
        file.sync_all()
            .map_err(|e| ProviderError::AuthConfig(format!("fsync failed: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                .map_err(|e| ProviderError::AuthConfig(format!("chmod failed: {e}")))?;
        }

        fs::rename(&tmp, &self.auth_path)
            .map_err(|e| ProviderError::AuthConfig(format!("rename failed: {e}")))?;

        drop(lock_file);
        let _ = fs::remove_file(&lock_path);

        info!(path = %self.auth_path.display(), provider = %provider, "auth.json updated");
        Ok(())
    }

    fn write_status_file(&self) -> Result<(), ProviderError> {
        let invalidations = self
            .invalidations
            .read()
            .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;

        let mut status: HashMap<String, PersistedInvalidation> = HashMap::new();
        for (id, entry) in invalidations.iter() {
            status.insert(
                id.auth_key().to_string(),
                PersistedInvalidation {
                    status: "invalidated".into(),
                    reason: entry.reason.clone(),
                    since: entry.since_utc,
                    retry_after_secs: entry.retry_after.map(|d| d.as_secs()),
                },
            );
        }

        let json = serde_json::to_string_pretty(&status)
            .map_err(|e| ProviderError::AuthConfig(e.to_string()))?;

        let tmp = self.status_path.with_extension("json.tmp");
        fs::write(&tmp, json.as_bytes())
            .map_err(|e| ProviderError::AuthConfig(format!("write failed: {e}")))?;
        fs::rename(&tmp, &self.status_path)
            .map_err(|e| ProviderError::AuthConfig(format!("rename failed: {e}")))?;

        Ok(())
    }

    fn get_expires_ms(&self, provider: ProviderId) -> Option<u64> {
        self.credentials
            .read()
            .ok()
            .and_then(|c| c.get(&provider).cloned())
            .and_then(|c| match c {
                AuthCredentials::OAuth { expires, .. } if expires > 0 => Some(expires),
                _ => None,
            })
    }
}

impl CredentialSource for CredentialStore {
    fn get(&self, provider: ProviderId) -> Option<AuthCredentials> {
        self.credentials
            .read()
            .ok()
            .and_then(|creds| creds.get(&provider).cloned())
    }

    fn update(&self, provider: ProviderId, creds: AuthCredentials) -> Result<(), ProviderError> {
        // Persist to disk FIRST (disk is source of truth), then update cache.
        // No RwLock held during file I/O.
        self.write_provider_to_auth(provider, &creds)?;

        // Update in-memory cache
        {
            let mut cache = self
                .credentials
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            let mut fps = self
                .fingerprints
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            fps.insert(provider, cred_fingerprint(&creds));
            cache.insert(provider, creds);
        }

        // Clear invalidation (successful update = recovery)
        {
            let mut inv = self
                .invalidations
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            if inv.remove(&provider).is_some() {
                info!(provider = %provider, "provider recovered via credential update");
            }
        }

        self.write_status_file()?;
        Ok(())
    }

    fn invalidate(
        &self,
        provider: ProviderId,
        reason: &str,
        retry_after: Option<Duration>,
    ) -> Result<(), ProviderError> {
        {
            let mut inv = self
                .invalidations
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            inv.insert(
                provider,
                InvalidationEntry {
                    reason: reason.to_string(),
                    since: Instant::now(),
                    since_utc: Utc::now(),
                    retry_after,
                },
            );
        }

        warn!(
            provider = %provider,
            reason,
            retry_after_secs = retry_after.map(|d| d.as_secs()),
            "provider invalidated"
        );

        self.write_status_file()?;
        Ok(())
    }

    fn available(&self) -> Vec<ProviderId> {
        let creds = match self.credentials.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let inv = match self.invalidations.read() {
            Ok(i) => i,
            Err(_) => return creds.keys().copied().collect(),
        };

        creds
            .keys()
            .filter(|id| match inv.get(id) {
                None => true,
                Some(entry) => {
                    // Auto-recover if retry_after window has passed
                    if let Some(retry_after) = entry.retry_after {
                        entry.since.elapsed() >= retry_after
                    } else {
                        false
                    }
                }
            })
            .copied()
            .collect()
    }

    fn status(&self, provider: ProviderId) -> ProviderAuthStatus {
        let has_creds = self
            .credentials
            .read()
            .ok()
            .map(|c| c.contains_key(&provider))
            .unwrap_or(false);

        if !has_creds {
            return ProviderAuthStatus::NotConfigured;
        }

        if let Some(entry) = self
            .invalidations
            .read()
            .ok()
            .and_then(|inv| inv.get(&provider).cloned())
        {
            // Check auto-recovery
            if let Some(retry_after) = entry.retry_after {
                if entry.since.elapsed() >= retry_after {
                    return ProviderAuthStatus::Available {
                        expires_ms: self.get_expires_ms(provider),
                    };
                }
            }
            return ProviderAuthStatus::Invalidated {
                reason: entry.reason,
                since: entry.since,
                retry_after: entry.retry_after,
            };
        }

        ProviderAuthStatus::Available {
            expires_ms: self.get_expires_ms(provider),
        }
    }

    fn reload(&self) -> Result<(), ProviderError> {
        let (new_creds, new_fps) = Self::read_auth_file(&self.auth_path)?;

        let old_fps = self
            .fingerprints
            .read()
            .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;

        let mut changed_providers = Vec::new();
        for (id, new_fp) in &new_fps {
            match old_fps.get(id) {
                Some(old_fp) if old_fp != new_fp => changed_providers.push(*id),
                None => changed_providers.push(*id),
                _ => {}
            }
        }
        drop(old_fps);

        {
            let mut creds = self
                .credentials
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            let mut fps = self
                .fingerprints
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            *creds = new_creds;
            *fps = new_fps;
        }

        if !changed_providers.is_empty() {
            let mut inv = self
                .invalidations
                .write()
                .map_err(|e| ProviderError::AuthConfig(format!("lock poisoned: {e}")))?;
            for id in &changed_providers {
                if inv.remove(id).is_some() {
                    info!(provider = %id, "provider recovered — new credentials detected on disk");
                }
            }
            drop(inv);
            self.write_status_file()?;
        }

        info!("credential store reloaded from disk");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn write_auth(dir: &Path, json: &str) {
        fs::write(dir.join("auth.json"), json).expect("write auth.json");
    }

    fn make_store(dir: &Path) -> CredentialStore {
        CredentialStore::load(dir.join("auth.json"), dir.join("auth_status.json"))
            .expect("load store")
    }

    #[test]
    fn test_load_and_get() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
        );
        let store = make_store(dir.path());
        assert!(store.get(ProviderId::Anthropic).is_some());
    }

    #[test]
    fn test_get_returns_none_for_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), r#"{}"#);
        let store = make_store(dir.path());
        assert!(store.get(ProviderId::Anthropic).is_none());
    }

    #[test]
    fn test_update_persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), r#"{}"#);
        let store = make_store(dir.path());

        store
            .update(
                ProviderId::Anthropic,
                AuthCredentials::ApiKey {
                    key: "sk-new".into(),
                },
            )
            .unwrap();

        let content = fs::read_to_string(dir.path().join("auth.json")).unwrap();
        assert!(content.contains("sk-new"));

        let creds = store.get(ProviderId::Anthropic).unwrap();
        match creds {
            AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-new"),
            _ => panic!("expected ApiKey"),
        }
    }

    #[test]
    fn test_update_clears_invalidation() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-old"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(ProviderId::Anthropic, "token revoked", None)
            .unwrap();
        assert!(!store.status(ProviderId::Anthropic).is_available());

        store
            .update(
                ProviderId::Anthropic,
                AuthCredentials::ApiKey {
                    key: "sk-new".into(),
                },
            )
            .unwrap();
        assert!(store.status(ProviderId::Anthropic).is_available());
    }

    #[test]
    fn test_invalidate_persists_to_status_file() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(ProviderId::Anthropic, "refresh failed", None)
            .unwrap();

        let content = fs::read_to_string(dir.path().join("auth_status.json")).unwrap();
        assert!(content.contains("refresh failed"));
    }

    #[test]
    fn test_available_excludes_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-a"},"openai-codex":{"type":"api_key","key":"sk-b"}}"#,
        );
        let store = make_store(dir.path());

        assert_eq!(store.available().len(), 2);

        store
            .invalidate(ProviderId::Anthropic, "down", None)
            .unwrap();
        let avail = store.available();
        assert_eq!(avail.len(), 1);
        assert_eq!(avail[0], ProviderId::OpenaiCodex);
    }

    #[test]
    fn test_available_auto_recovers_after_retry_window() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(
                ProviderId::Anthropic,
                "rate limited",
                Some(Duration::from_millis(1)),
            )
            .unwrap();

        std::thread::sleep(Duration::from_millis(10));
        assert!(store.available().contains(&ProviderId::Anthropic));
    }

    #[test]
    fn test_status_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), r#"{}"#);
        let store = make_store(dir.path());
        assert!(matches!(
            store.status(ProviderId::Anthropic),
            ProviderAuthStatus::NotConfigured
        ));
    }

    #[test]
    fn test_get_returns_creds_even_when_invalidated() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(ProviderId::Anthropic, "broken", None)
            .unwrap();
        assert!(store.get(ProviderId::Anthropic).is_some());
    }

    #[test]
    fn test_reload_picks_up_disk_changes() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), r#"{}"#);
        let store = make_store(dir.path());

        assert!(store.get(ProviderId::Anthropic).is_none());

        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-external"}}"#,
        );
        store.reload().unwrap();
        assert!(store.get(ProviderId::Anthropic).is_some());
    }

    #[test]
    fn test_reload_clears_invalidation_on_changed_creds() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-old"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(ProviderId::Anthropic, "broken", None)
            .unwrap();
        assert!(!store.status(ProviderId::Anthropic).is_available());

        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-fixed"}}"#,
        );
        store.reload().unwrap();
        assert!(store.status(ProviderId::Anthropic).is_available());
    }

    #[test]
    fn test_update_preserves_other_providers() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-a"},"openai-codex":{"type":"api_key","key":"sk-b"}}"#,
        );
        let store = make_store(dir.path());

        store
            .update(
                ProviderId::Anthropic,
                AuthCredentials::ApiKey {
                    key: "sk-new-a".into(),
                },
            )
            .unwrap();

        let content = fs::read_to_string(dir.path().join("auth.json")).unwrap();
        assert!(content.contains("sk-b"), "other provider must be preserved");
        assert!(
            content.contains("sk-new-a"),
            "updated provider must be written"
        );
    }

    #[test]
    fn test_file_locking_prevents_corruption() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-init"}}"#,
        );
        let store = Arc::new(make_store(dir.path()));

        let store1 = store.clone();
        let store2 = store.clone();
        let t1 = std::thread::spawn(move || {
            for i in 0..10 {
                store1
                    .update(
                        ProviderId::Anthropic,
                        AuthCredentials::ApiKey {
                            key: format!("sk-t1-{i}"),
                        },
                    )
                    .unwrap();
            }
        });
        let t2 = std::thread::spawn(move || {
            for i in 0..10 {
                store2
                    .update(
                        ProviderId::OpenaiCodex,
                        AuthCredentials::ApiKey {
                            key: format!("sk-t2-{i}"),
                        },
                    )
                    .unwrap();
            }
        });
        t1.join().unwrap();
        t2.join().unwrap();

        let content = fs::read_to_string(dir.path().join("auth.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.get("anthropic").is_some());
        assert!(parsed.get("openai-codex").is_some());
    }

    #[test]
    fn test_load_nonexistent_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        // Don't create auth.json — store should handle gracefully
        let store = make_store(dir.path());
        assert!(store.available().is_empty());
    }

    #[test]
    fn test_oauth_expires_ms_in_status() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"oauth","access":"tok","refresh":"ref","expires":9999999999999}}"#,
        );
        let store = make_store(dir.path());
        match store.status(ProviderId::Anthropic) {
            ProviderAuthStatus::Available { expires_ms } => {
                assert_eq!(expires_ms, Some(9999999999999));
            }
            other => panic!("expected Available, got {other:?}"),
        }
    }

    #[test]
    fn test_load_returns_error_for_malformed_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("auth.json"), b"not valid json {{{{").unwrap();
        let result = CredentialStore::load(
            dir.path().join("auth.json"),
            dir.path().join("auth_status.json"),
        );
        assert!(result.is_err(), "malformed auth.json must return Err");
    }

    #[test]
    fn test_status_invalidated_fields_are_populated() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(
                ProviderId::Anthropic,
                "quota exceeded",
                Some(Duration::from_secs(60)),
            )
            .unwrap();

        match store.status(ProviderId::Anthropic) {
            ProviderAuthStatus::Invalidated {
                reason,
                retry_after,
                ..
            } => {
                assert_eq!(reason, "quota exceeded");
                assert_eq!(retry_after, Some(Duration::from_secs(60)));
            }
            other => panic!("expected Invalidated, got {other:?}"),
        }
    }

    #[test]
    fn test_invalidation_survives_store_reload() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-test"}}"#,
        );

        {
            let store = make_store(dir.path());
            store
                .invalidate(ProviderId::Anthropic, "persistent failure", None)
                .unwrap();
        } // store dropped

        let fresh_store = make_store(dir.path());
        assert!(
            !fresh_store.status(ProviderId::Anthropic).is_available(),
            "invalidation must survive store restart"
        );
    }

    #[test]
    fn test_reload_does_not_clear_invalidation_when_creds_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-same"}}"#,
        );
        let store = make_store(dir.path());

        store
            .invalidate(ProviderId::Anthropic, "still broken", None)
            .unwrap();
        store.reload().unwrap();

        assert!(
            !store.status(ProviderId::Anthropic).is_available(),
            "reload with unchanged creds must NOT clear invalidation"
        );
    }

    #[test]
    fn test_reload_removes_provider_deleted_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            r#"{"anthropic":{"type":"api_key","key":"sk-a"},"openai-codex":{"type":"api_key","key":"sk-b"}}"#,
        );
        let store = make_store(dir.path());
        assert_eq!(store.available().len(), 2);

        write_auth(
            dir.path(),
            r#"{"openai-codex":{"type":"api_key","key":"sk-b"}}"#,
        );
        store.reload().unwrap();

        assert!(store.get(ProviderId::Anthropic).is_none());
        assert_eq!(store.available().len(), 1);
    }
}
