//! Autoflow claim store. Lives under `~/.rupu/autoflows/claims/`.

use crate::autoflow_claim::AutoflowClaimRecord;
use crate::repo_store::sanitize_component;
use chrono::{DateTime, Utc};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum ClaimStoreError {
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
    #[error("claim for `{issue_ref}` is already actively locked")]
    AlreadyLocked { issue_ref: String },
    #[error("claim for `{issue_ref}` is not tracked")]
    NotFound { issue_ref: String },
}

#[derive(Debug, Clone)]
pub struct AutoflowClaimStore {
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActiveLockRecord {
    pub owner: String,
    pub acquired_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub lease_expires_at: Option<String>,
}

#[derive(Debug)]
pub struct ClaimLockGuard {
    path: PathBuf,
    _file: File,
}

impl Drop for ClaimLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl AutoflowClaimStore {
    fn ensure_root(&self) -> Result<(), ClaimStoreError> {
        std::fs::create_dir_all(&self.root).map_err(|e| ClaimStoreError::Io {
            action: format!("create_dir_all {}", self.root.display()),
            source: e,
        })
    }

    fn issue_dir(&self, issue_ref: &str) -> PathBuf {
        self.root.join(issue_key(issue_ref))
    }

    fn claim_path(&self, issue_ref: &str) -> PathBuf {
        self.issue_dir(issue_ref).join("claim.toml")
    }

    fn lock_path(&self, issue_ref: &str) -> PathBuf {
        self.issue_dir(issue_ref).join(".lock")
    }

    pub fn save(&self, rec: &AutoflowClaimRecord) -> Result<(), ClaimStoreError> {
        self.ensure_root()?;
        let issue_dir = self.issue_dir(&rec.issue_ref);
        std::fs::create_dir_all(&issue_dir).map_err(|e| ClaimStoreError::Io {
            action: format!("create_dir_all {}", issue_dir.display()),
            source: e,
        })?;
        let body = toml::to_string(rec)?;
        let path = self.claim_path(&rec.issue_ref);
        let tmp = issue_dir.join("claim.toml.tmp");
        std::fs::write(&tmp, body).map_err(|e| ClaimStoreError::Io {
            action: format!("write {}", tmp.display()),
            source: e,
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| ClaimStoreError::Io {
            action: format!("rename {} -> {}", tmp.display(), path.display()),
            source: e,
        })?;
        Ok(())
    }

    pub fn load(&self, issue_ref: &str) -> Result<Option<AutoflowClaimRecord>, ClaimStoreError> {
        let path = self.claim_path(issue_ref);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| ClaimStoreError::Io {
            action: format!("read {}", path.display()),
            source: e,
        })?;
        let rec = toml::from_str(&text).map_err(|e| ClaimStoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(rec))
    }

    pub fn list(&self) -> Result<Vec<AutoflowClaimRecord>, ClaimStoreError> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|e| ClaimStoreError::Io {
            action: format!("read_dir {}", self.root.display()),
            source: e,
        })? {
            let entry = entry.map_err(|e| ClaimStoreError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let path = entry.path().join("claim.toml");
            if !path.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping unreadable claim record");
                    continue;
                }
            };
            let rec: AutoflowClaimRecord = match toml::from_str(&text) {
                Ok(r) => r,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping corrupt claim record");
                    continue;
                }
            };
            out.push(rec);
        }
        out.sort_by(|a, b| a.issue_ref.cmp(&b.issue_ref));
        Ok(out)
    }

    pub fn delete(&self, issue_ref: &str) -> Result<bool, ClaimStoreError> {
        let dir = self.issue_dir(issue_ref);
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir).map_err(|e| ClaimStoreError::Io {
            action: format!("remove_dir_all {}", dir.display()),
            source: e,
        })?;
        Ok(true)
    }

    pub fn try_acquire_active_lock(
        &self,
        issue_ref: &str,
        owner: &str,
        lease_expires_at: Option<&str>,
    ) -> Result<ClaimLockGuard, ClaimStoreError> {
        self.ensure_root()?;
        let issue_dir = self.issue_dir(issue_ref);
        std::fs::create_dir_all(&issue_dir).map_err(|e| ClaimStoreError::Io {
            action: format!("create_dir_all {}", issue_dir.display()),
            source: e,
        })?;
        let path = self.lock_path(issue_ref);
        let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if self.reap_expired_active_lock(issue_ref, Utc::now())? {
                    OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&path)
                        .map_err(|e| {
                            if e.kind() == std::io::ErrorKind::AlreadyExists {
                                ClaimStoreError::AlreadyLocked {
                                    issue_ref: issue_ref.to_string(),
                                }
                            } else {
                                ClaimStoreError::Io {
                                    action: format!("open {}", path.display()),
                                    source: e,
                                }
                            }
                        })?
                } else {
                    return Err(ClaimStoreError::AlreadyLocked {
                        issue_ref: issue_ref.to_string(),
                    });
                }
            }
            Err(err) => {
                return Err(ClaimStoreError::Io {
                    action: format!("open {}", path.display()),
                    source: err,
                });
            }
        };

        let lock = ActiveLockRecord {
            owner: owner.to_string(),
            acquired_at: Utc::now().to_rfc3339(),
            lease_expires_at: lease_expires_at.map(ToOwned::to_owned),
        };
        let body = toml::to_string(&lock)?;
        file.write_all(body.as_bytes())
            .map_err(|e| ClaimStoreError::Io {
                action: format!("write {}", path.display()),
                source: e,
            })?;
        Ok(ClaimLockGuard { path, _file: file })
    }

    pub fn read_active_lock(
        &self,
        issue_ref: &str,
    ) -> Result<Option<ActiveLockRecord>, ClaimStoreError> {
        self.reap_expired_active_lock(issue_ref, Utc::now())?;
        let path = self.lock_path(issue_ref);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| ClaimStoreError::Io {
            action: format!("read {}", path.display()),
            source: e,
        })?;
        let lock = toml::from_str(&text).map_err(|e| ClaimStoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(lock))
    }

    fn reap_expired_active_lock(
        &self,
        issue_ref: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, ClaimStoreError> {
        let path = self.lock_path(issue_ref);
        let Some(lock) = self.read_lock_file_if_present(&path)? else {
            return Ok(false);
        };
        if !lock_expired(&lock, now) {
            return Ok(false);
        }
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(err) => Err(ClaimStoreError::Io {
                action: format!("remove_file {}", path.display()),
                source: err,
            }),
        }
    }

    fn read_lock_file_if_present(
        &self,
        path: &PathBuf,
    ) -> Result<Option<ActiveLockRecord>, ClaimStoreError> {
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(path).map_err(|e| ClaimStoreError::Io {
            action: format!("read {}", path.display()),
            source: e,
        })?;
        let lock = toml::from_str(&text).map_err(|e| ClaimStoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(lock))
    }
}

pub fn issue_key(issue_ref: &str) -> String {
    sanitize_component(issue_ref)
}

fn lock_expired(lock: &ActiveLockRecord, now: DateTime<Utc>) -> bool {
    let Some(lease_expires_at) = lock.lease_expires_at.as_deref() else {
        return false;
    };
    DateTime::parse_from_rfc3339(lease_expires_at)
        .map(|lease| lease.with_timezone(&Utc) <= now)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autoflow_claim::{AutoflowClaimRecord, ClaimStatus};

    #[test]
    fn claim_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let rec = AutoflowClaimRecord {
            issue_ref: "github:Section9Labs/rupu/issues/42".into(),
            repo_ref: "github:Section9Labs/rupu".into(),
            source_ref: Some("github:Section9Labs/rupu".into()),
            issue_display_ref: Some("42".into()),
            issue_title: Some("finish autoflow".into()),
            issue_url: Some("https://github.com/Section9Labs/rupu/issues/42".into()),
            issue_state_name: Some("open".into()),
            issue_tracker: Some("github".into()),
            workflow: "issue-supervisor-dispatch".into(),
            status: ClaimStatus::Claimed,
            worktree_path: Some("/tmp/ws".into()),
            branch: Some("rupu/issue-42".into()),
            last_run_id: Some("run_123".into()),
            last_error: None,
            last_summary: Some("phase 1 ready".into()),
            pr_url: Some("https://github.com/Section9Labs/rupu/pull/42".into()),
            artifacts: Some(serde_json::json!({
                "review_packet": "docs/reviews/issue-42.json"
            })),
            artifact_manifest_path: Some("/tmp/runs/run_123/artifact_manifest.json".into()),
            next_retry_at: None,
            claim_owner: Some("host:user:pid".into()),
            lease_expires_at: Some("2026-05-08T23:00:00Z".into()),
            pending_dispatch: None,
            contenders: vec![crate::autoflow_claim::AutoflowContender {
                workflow: "issue-supervisor-dispatch".into(),
                priority: 100,
                scope: Some("project".into()),
                selected: true,
            }],
            updated_at: "2026-05-08T20:00:00Z".into(),
        };
        store.save(&rec).unwrap();
        let loaded = store.load(&rec.issue_ref).unwrap().unwrap();
        assert_eq!(loaded, rec);
    }

    #[test]
    fn active_lock_blocks_second_acquire_until_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        let lease = (Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        let guard = store
            .try_acquire_active_lock(issue_ref, "owner-a", Some(&lease))
            .unwrap();
        let err = store
            .try_acquire_active_lock(issue_ref, "owner-b", Some(&lease))
            .unwrap_err();
        assert!(matches!(err, ClaimStoreError::AlreadyLocked { .. }));
        drop(guard);
        let _guard2 = store
            .try_acquire_active_lock(issue_ref, "owner-b", Some(&lease))
            .unwrap();
    }

    #[test]
    fn read_active_lock_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        let lease = (Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        let _guard = store
            .try_acquire_active_lock(issue_ref, "owner-a", Some(&lease))
            .unwrap();
        let lock = store.read_active_lock(issue_ref).unwrap().unwrap();
        assert_eq!(lock.owner, "owner-a");
        assert_eq!(lock.lease_expires_at.as_deref(), Some(lease.as_str()));
    }

    #[test]
    fn expired_active_lock_is_reaped_on_read() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        let issue_dir = store.root.join(issue_key(issue_ref));
        std::fs::create_dir_all(&issue_dir).unwrap();
        let path = issue_dir.join(".lock");
        let body = toml::to_string(&ActiveLockRecord {
            owner: "owner-a".into(),
            acquired_at: "2026-05-08T20:00:00Z".into(),
            lease_expires_at: Some("2000-01-01T00:00:00Z".into()),
        })
        .unwrap();
        std::fs::write(&path, body).unwrap();

        assert!(store.read_active_lock(issue_ref).unwrap().is_none());
        assert!(!path.exists());
    }

    #[test]
    fn expired_active_lock_can_be_reacquired() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AutoflowClaimStore {
            root: tmp.path().join("claims"),
        };
        let issue_ref = "github:Section9Labs/rupu/issues/42";
        let issue_dir = store.root.join(issue_key(issue_ref));
        std::fs::create_dir_all(&issue_dir).unwrap();
        let path = issue_dir.join(".lock");
        let body = toml::to_string(&ActiveLockRecord {
            owner: "owner-a".into(),
            acquired_at: "2026-05-08T20:00:00Z".into(),
            lease_expires_at: Some("2000-01-01T00:00:00Z".into()),
        })
        .unwrap();
        std::fs::write(&path, body).unwrap();

        let lease = (Utc::now() + chrono::Duration::hours(3)).to_rfc3339();
        let _guard = store
            .try_acquire_active_lock(issue_ref, "owner-b", Some(&lease))
            .unwrap();
        let lock = store.read_active_lock(issue_ref).unwrap().unwrap();
        assert_eq!(lock.owner, "owner-b");
        assert_eq!(lock.lease_expires_at.as_deref(), Some(lease.as_str()));
    }
}
