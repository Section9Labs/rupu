//! Local worker registry. Lives at `~/.rupu/autoflows/workers/`.

use crate::repo_store::sanitize_component;
use rupu_runtime::WorkerRecord;
use std::path::PathBuf;
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum WorkerStoreError {
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
}

#[derive(Debug, Clone)]
pub struct WorkerStore {
    pub root: PathBuf,
}

impl WorkerStore {
    fn ensure_root(&self) -> Result<(), WorkerStoreError> {
        std::fs::create_dir_all(&self.root).map_err(|e| WorkerStoreError::Io {
            action: format!("create_dir_all {}", self.root.display()),
            source: e,
        })
    }

    fn record_path(&self, worker_id: &str) -> PathBuf {
        self.root
            .join(format!("{}.toml", sanitize_component(worker_id)))
    }

    pub fn save(&self, record: &WorkerRecord) -> Result<(), WorkerStoreError> {
        self.ensure_root()?;
        let path = self.record_path(&record.worker_id);
        let body = toml::to_string(record)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, body).map_err(|e| WorkerStoreError::Io {
            action: format!("write {}", tmp.display()),
            source: e,
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| WorkerStoreError::Io {
            action: format!("rename {} -> {}", tmp.display(), path.display()),
            source: e,
        })?;
        Ok(())
    }

    pub fn load(&self, worker_id: &str) -> Result<Option<WorkerRecord>, WorkerStoreError> {
        let path = self.record_path(worker_id);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| WorkerStoreError::Io {
            action: format!("read {}", path.display()),
            source: e,
        })?;
        let rec = toml::from_str(&text).map_err(|e| WorkerStoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(rec))
    }

    pub fn list(&self) -> Result<Vec<WorkerRecord>, WorkerStoreError> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|e| WorkerStoreError::Io {
            action: format!("read_dir {}", self.root.display()),
            source: e,
        })? {
            let entry = entry.map_err(|e| WorkerStoreError::Io {
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
                    warn!(path = %path.display(), error = %e, "skipping unreadable worker record");
                    continue;
                }
            };
            let rec: WorkerRecord = match toml::from_str(&text) {
                Ok(r) => r,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping corrupt worker record");
                    continue;
                }
            };
            out.push(rec);
        }
        out.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_runtime::{WorkerCapabilities, WorkerKind};

    #[test]
    fn save_and_load_worker_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = WorkerStore {
            root: tmp.path().join("workers"),
        };
        let worker = WorkerRecord {
            version: WorkerRecord::VERSION,
            worker_id: "worker_local_team-mini_cli".into(),
            kind: WorkerKind::Cli,
            name: "team-mini".into(),
            host: "team-mini.local".into(),
            capabilities: WorkerCapabilities {
                backends: vec!["local_worktree".into()],
                scm_hosts: vec!["github".into()],
                permission_modes: vec!["bypass".into()],
            },
            registered_at: "2026-05-09T16:00:00Z".into(),
            last_seen_at: "2026-05-09T16:10:00Z".into(),
        };

        store.save(&worker).unwrap();
        let loaded = store.load(&worker.worker_id).unwrap().unwrap();
        assert_eq!(loaded, worker);
        assert_eq!(store.list().unwrap(), vec![worker]);
    }
}
