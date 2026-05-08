//! Repo registry store. Lives at `~/.rupu/repos/`.

use crate::repo_record::TrackedRepo;
use chrono::Utc;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum RepoStoreError {
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
    #[error("repo `{repo_ref}` is not tracked")]
    NotTracked { repo_ref: String },
    #[error("path is not valid UTF-8: {path}")]
    NonUtf8Path { path: String },
}

#[derive(Debug, Clone)]
pub struct RepoRegistryStore {
    pub root: PathBuf,
}

impl RepoRegistryStore {
    fn ensure_root(&self) -> Result<(), RepoStoreError> {
        std::fs::create_dir_all(&self.root).map_err(|e| RepoStoreError::Io {
            action: format!("create_dir_all {}", self.root.display()),
            source: e,
        })
    }

    fn record_path(&self, repo_ref: &str) -> PathBuf {
        self.root.join(format!("{}.toml", repo_ref_key(repo_ref)))
    }

    pub fn load(&self, repo_ref: &str) -> Result<Option<TrackedRepo>, RepoStoreError> {
        let path = self.record_path(repo_ref);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| RepoStoreError::Io {
            action: format!("read {}", path.display()),
            source: e,
        })?;
        let rec: TrackedRepo = toml::from_str(&text).map_err(|e| RepoStoreError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Some(rec))
    }

    pub fn list(&self) -> Result<Vec<TrackedRepo>, RepoStoreError> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|e| RepoStoreError::Io {
            action: format!("read_dir {}", self.root.display()),
            source: e,
        })? {
            let entry = entry.map_err(|e| RepoStoreError::Io {
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
                    warn!(path = %path.display(), error = %e, "skipping unreadable repo record");
                    continue;
                }
            };
            let rec: TrackedRepo = match toml::from_str(&text) {
                Ok(r) => r,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping corrupt repo record");
                    continue;
                }
            };
            out.push(rec);
        }
        out.sort_by(|a, b| a.repo_ref.cmp(&b.repo_ref));
        Ok(out)
    }

    pub fn upsert(
        &self,
        repo_ref: &str,
        path: &Path,
        origin_url: Option<&str>,
        default_branch: Option<&str>,
    ) -> Result<TrackedRepo, RepoStoreError> {
        self.ensure_root()?;
        let canonical = canonical_path_string(path)?;
        let now = Utc::now().to_rfc3339();
        let mut rec = self.load(repo_ref)?.unwrap_or(TrackedRepo {
            repo_ref: repo_ref.to_string(),
            preferred_path: canonical.clone(),
            known_paths: vec![],
            origin_urls: vec![],
            default_branch: default_branch.map(ToOwned::to_owned),
            last_seen_at: now.clone(),
        });

        if !rec.known_paths.iter().any(|p| p == &canonical) {
            rec.known_paths.push(canonical.clone());
            rec.known_paths.sort();
        }
        if rec.preferred_path.is_empty() {
            rec.preferred_path = canonical.clone();
        }
        if let Some(origin_url) = origin_url {
            if !origin_url.is_empty() && !rec.origin_urls.iter().any(|u| u == origin_url) {
                rec.origin_urls.push(origin_url.to_string());
                rec.origin_urls.sort();
            }
        }
        if let Some(default_branch) = default_branch {
            if !default_branch.is_empty() {
                rec.default_branch = Some(default_branch.to_string());
            }
        }
        rec.last_seen_at = now;
        write_record(&self.record_path(repo_ref), &rec)?;
        Ok(rec)
    }

    pub fn set_preferred_path(
        &self,
        repo_ref: &str,
        path: &Path,
    ) -> Result<TrackedRepo, RepoStoreError> {
        let canonical = canonical_path_string(path)?;
        let mut rec = self
            .load(repo_ref)?
            .ok_or_else(|| RepoStoreError::NotTracked {
                repo_ref: repo_ref.to_string(),
            })?;
        if !rec.known_paths.iter().any(|p| p == &canonical) {
            rec.known_paths.push(canonical.clone());
            rec.known_paths.sort();
        }
        rec.preferred_path = canonical;
        rec.last_seen_at = Utc::now().to_rfc3339();
        write_record(&self.record_path(repo_ref), &rec)?;
        Ok(rec)
    }

    pub fn forget_repo(&self, repo_ref: &str) -> Result<bool, RepoStoreError> {
        let path = self.record_path(repo_ref);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|e| RepoStoreError::Io {
            action: format!("remove_file {}", path.display()),
            source: e,
        })?;
        Ok(true)
    }

    pub fn forget_path(&self, repo_ref: &str, path: &Path) -> Result<bool, RepoStoreError> {
        let canonical = normalize_path_string(path)?;
        let mut rec = self
            .load(repo_ref)?
            .ok_or_else(|| RepoStoreError::NotTracked {
                repo_ref: repo_ref.to_string(),
            })?;
        let before = rec.known_paths.len();
        rec.known_paths.retain(|p| p != &canonical);
        if rec.known_paths.len() == before {
            return Ok(false);
        }
        if rec.known_paths.is_empty() {
            let _ = self.forget_repo(repo_ref)?;
            return Ok(true);
        }
        if rec.preferred_path == canonical {
            rec.preferred_path = rec.known_paths[0].clone();
        }
        rec.last_seen_at = Utc::now().to_rfc3339();
        write_record(&self.record_path(repo_ref), &rec)?;
        Ok(true)
    }
}

pub fn repo_ref_key(repo_ref: &str) -> String {
    sanitize_component(repo_ref)
}

fn canonical_path_string(path: &Path) -> Result<String, RepoStoreError> {
    let canonical = path.canonicalize().map_err(|e| RepoStoreError::Io {
        action: format!("canonicalize {}", path.display()),
        source: e,
    })?;
    canonical
        .to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| RepoStoreError::NonUtf8Path {
            path: canonical.display().to_string(),
        })
}

fn normalize_path_string(path: &Path) -> Result<String, RepoStoreError> {
    match path.canonicalize() {
        Ok(canonical) => {
            canonical
                .to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| RepoStoreError::NonUtf8Path {
                    path: canonical.display().to_string(),
                })
        }
        Err(_) => {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map_err(|e| RepoStoreError::Io {
                        action: "current_dir".into(),
                        source: e,
                    })?
                    .join(path)
            };

            let mut suffix = Vec::new();
            let mut existing = absolute.as_path();
            while !existing.exists() {
                let name = existing.file_name().ok_or_else(|| RepoStoreError::Io {
                    action: format!("resolve missing path {}", absolute.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "missing path has no existing ancestor",
                    ),
                })?;
                suffix.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| RepoStoreError::Io {
                    action: format!("resolve missing path {}", absolute.display()),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "missing path has no parent",
                    ),
                })?;
            }

            let mut normalized = existing.canonicalize().map_err(|e| RepoStoreError::Io {
                action: format!("canonicalize {}", existing.display()),
                source: e,
            })?;
            for component in suffix.into_iter().rev() {
                normalized.push(component);
            }

            normalized
                .to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| RepoStoreError::NonUtf8Path {
                    path: normalized.display().to_string(),
                })
        }
    }
}

fn write_record(path: &Path, rec: &TrackedRepo) -> Result<(), RepoStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RepoStoreError::Io {
            action: format!("create_dir_all {}", parent.display()),
            source: e,
        })?;
    }
    let body = toml::to_string(rec)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, body).map_err(|e| RepoStoreError::Io {
        action: format!("write {}", tmp.display()),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| RepoStoreError::Io {
        action: format!("rename {} -> {}", tmp.display(), path.display()),
        source: e,
    })?;
    Ok(())
}

pub(crate) fn sanitize_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;
    for ch in input.chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        if keep {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push_str("--");
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_round_trips_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RepoRegistryStore {
            root: tmp.path().join("repos"),
        };
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();

        let rec = store
            .upsert(
                "github:Section9Labs/rupu",
                &repo_root,
                Some("git@github.com:Section9Labs/rupu.git"),
                Some("main"),
            )
            .unwrap();
        assert_eq!(
            rec.preferred_path,
            repo_root.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(rec.known_paths.len(), 1);
        assert_eq!(rec.default_branch.as_deref(), Some("main"));

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].repo_ref, "github:Section9Labs/rupu");
    }

    #[test]
    fn set_preferred_path_adds_unknown_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RepoRegistryStore {
            root: tmp.path().join("repos"),
        };
        let repo_a = tmp.path().join("repo-a");
        let repo_b = tmp.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();
        store
            .upsert("github:Section9Labs/rupu", &repo_a, None, None)
            .unwrap();

        let rec = store
            .set_preferred_path("github:Section9Labs/rupu", &repo_b)
            .unwrap();
        assert_eq!(
            rec.preferred_path,
            repo_b.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(rec.known_paths.len(), 2);
    }

    #[test]
    fn forget_path_deletes_record_when_last_path_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RepoRegistryStore {
            root: tmp.path().join("repos"),
        };
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        store
            .upsert("github:Section9Labs/rupu", &repo_root, None, None)
            .unwrap();

        let removed = store
            .forget_path("github:Section9Labs/rupu", &repo_root)
            .unwrap();
        assert!(removed);
        assert!(store.load("github:Section9Labs/rupu").unwrap().is_none());
    }

    #[test]
    fn forget_path_handles_missing_checkout_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RepoRegistryStore {
            root: tmp.path().join("repos"),
        };
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        store
            .upsert("github:Section9Labs/rupu", &repo_root, None, None)
            .unwrap();
        std::fs::remove_dir_all(&repo_root).unwrap();

        let removed = store
            .forget_path("github:Section9Labs/rupu", &repo_root)
            .unwrap();
        assert!(removed);
        assert!(store.load("github:Section9Labs/rupu").unwrap().is_none());
    }

    #[test]
    fn repo_ref_key_sanitizes_delimiters() {
        assert_eq!(
            repo_ref_key("github:Section9Labs/rupu"),
            "github--Section9Labs--rupu"
        );
    }
}
