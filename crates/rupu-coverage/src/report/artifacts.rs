//! Content-addressed store for finding artifacts (PoC scripts, outputs,
//! harnesses, binaries).
//!
//! Layout: `<root>/<first two hex chars>/<sha256>`. Identical content is
//! stored once across runs and projects. Files over the size cap are hashed
//! but not copied, and recorded as `external`.

use crate::report::types::{ArtifactKind, ArtifactRef, ArtifactStorage};
use crate::report::validate::rel_path_problem;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact `{path}`: {reason}")]
    Path { path: String, reason: &'static str },
    #[error("artifact `{path}` does not exist in the workspace")]
    Missing { path: String },
    #[error("artifact `{path}` resolves outside the workspace")]
    Escapes { path: String },
    #[error("artifact `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the report lists artifacts but no artifact store is configured for this run")]
    NoStore,
}

pub struct ArtifactStore {
    root: PathBuf,
}

const CHUNK: usize = 64 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file's contents, streamed.
pub(crate) fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn sniff_kind(p: &Path) -> std::io::Result<ArtifactKind> {
    let mut f = File::open(p)?;
    let mut buf = vec![0u8; 8192];
    let n = f.read(&mut buf)?;
    let head = &buf[..n];
    let text = !head.contains(&0) && std::str::from_utf8(head).is_ok();
    Ok(if text {
        ArtifactKind::Text
    } else {
        ArtifactKind::Binary
    })
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn blob_path(&self, sha256: &str) -> PathBuf {
        self.root.join(&sha256[..2.min(sha256.len())]).join(sha256)
    }

    /// Resolve every requested path (expanding directories), then hash and
    /// either copy (≤ `max_bytes`) or record as external. Agent-supplied
    /// metadata on `requested` is ignored; only `path` is read.
    pub fn ingest(
        &self,
        workspace: &Path,
        requested: &[ArtifactRef],
        max_bytes: u64,
    ) -> Result<Vec<ArtifactRef>, ArtifactError> {
        let ws_canon = fs::canonicalize(workspace).map_err(|source| ArtifactError::Io {
            path: workspace.display().to_string(),
            source,
        })?;
        let mut out = Vec::new();
        for r in requested {
            if let Some(reason) = rel_path_problem(&r.path) {
                return Err(ArtifactError::Path {
                    path: r.path.clone(),
                    reason,
                });
            }
            let abs = workspace.join(&r.path);
            if fs::symlink_metadata(&abs).is_err() {
                return Err(ArtifactError::Missing {
                    path: r.path.clone(),
                });
            }
            let canon = fs::canonicalize(&abs).map_err(|source| ArtifactError::Io {
                path: r.path.clone(),
                source,
            })?;
            if !canon.starts_with(&ws_canon) {
                return Err(ArtifactError::Escapes {
                    path: r.path.clone(),
                });
            }
            if canon.is_dir() {
                let mut files = Vec::new();
                collect_files(&canon, &mut files).map_err(|source| ArtifactError::Io {
                    path: r.path.clone(),
                    source,
                })?;
                files.sort();
                for f in files {
                    let rel = f
                        .strip_prefix(&ws_canon)
                        .expect("collected under the workspace")
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push(self.ingest_file(&rel, &f, max_bytes)?);
                }
            } else {
                out.push(self.ingest_file(&r.path, &canon, max_bytes)?);
            }
        }
        Ok(out)
    }

    fn ingest_file(
        &self,
        rel: &str,
        abs: &Path,
        max_bytes: u64,
    ) -> Result<ArtifactRef, ArtifactError> {
        let io = |source| ArtifactError::Io {
            path: rel.to_string(),
            source,
        };
        let size = fs::metadata(abs).map_err(io)?.len();
        let kind = sniff_kind(abs).map_err(io)?;
        let (sha256, stored) = if size > max_bytes {
            (sha256_file(abs).map_err(io)?, ArtifactStorage::External)
        } else {
            (self.copy_hashing(abs).map_err(io)?, ArtifactStorage::Copied)
        };
        Ok(ArtifactRef {
            path: rel.to_string(),
            sha256,
            size,
            kind: Some(kind),
            stored: Some(stored),
            host: None,
        })
    }

    /// Stream `src` into a temp file in the store while hashing, then move it
    /// to its content address (or drop it when that blob already exists).
    fn copy_hashing(&self, src: &Path) -> std::io::Result<String> {
        fs::create_dir_all(&self.root)?;
        let tmp = self.root.join(format!("{}.tmp", ulid::Ulid::new()));
        let mut input = File::open(src)?;
        let mut output = File::create(&tmp)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            output.write_all(&buf[..n])?;
        }
        output.flush()?;
        drop(output);
        let sha = hex(&h.finalize());
        let dest = self.blob_path(&sha);
        if dest.exists() {
            fs::remove_file(&tmp)?;
        } else {
            fs::create_dir_all(dest.parent().expect("blob path has a parent"))?;
            fs::rename(&tmp, &dest)?;
        }
        Ok(sha)
    }
}

/// Every regular file under `dir`. Symlinks are skipped, not followed: a link
/// inside a PoC directory must not pull in files from elsewhere.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_files(&entry.path(), out)?;
        } else if ft.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::types::{ArtifactKind, ArtifactRef, ArtifactStorage};
    use std::fs;

    fn req(p: &str) -> ArtifactRef {
        ArtifactRef {
            path: p.into(),
            sha256: String::new(),
            size: 0,
            kind: None,
            stored: None,
            host: None,
        }
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir) {
        (
            tempfile::TempDir::new().unwrap(),
            tempfile::TempDir::new().unwrap(),
        )
    }

    #[test]
    fn copies_small_file_and_fills_metadata() {
        let (ws, store) = setup();
        fs::create_dir_all(ws.path().join("pocs")).unwrap();
        fs::write(ws.path().join("pocs/result.txt"), "rc=0\n").unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s
            .ingest(ws.path(), &[req("pocs/result.txt")], 1024)
            .unwrap();
        assert_eq!(out.len(), 1);
        let a = &out[0];
        assert_eq!(a.path, "pocs/result.txt");
        assert_eq!(a.size, 5);
        assert_eq!(a.kind, Some(ArtifactKind::Text));
        assert_eq!(a.stored, Some(ArtifactStorage::Copied));
        assert_eq!(a.sha256.len(), 64);
        assert_eq!(fs::read(s.blob_path(&a.sha256)).unwrap(), b"rc=0\n");
    }

    #[test]
    fn identical_content_is_stored_once() {
        let (ws, store) = setup();
        fs::write(ws.path().join("a.bin"), [0u8, 1, 2, 3]).unwrap();
        fs::write(ws.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s
            .ingest(ws.path(), &[req("a.bin"), req("b.bin")], 1024)
            .unwrap();
        assert_eq!(out[0].sha256, out[1].sha256);
        assert_eq!(out[0].kind, Some(ArtifactKind::Binary));
        let blobs: Vec<_> = walk(store.path())
            .into_iter()
            .filter(|p| !p.ends_with(".tmp"))
            .collect();
        assert_eq!(blobs.len(), 1, "{blobs:?}");
    }

    #[test]
    fn over_cap_file_is_recorded_external_and_not_copied() {
        let (ws, store) = setup();
        fs::write(ws.path().join("big.img"), vec![7u8; 2048]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s.ingest(ws.path(), &[req("big.img")], 1024).unwrap();
        assert_eq!(out[0].stored, Some(ArtifactStorage::External));
        assert_eq!(out[0].size, 2048);
        assert_eq!(out[0].sha256.len(), 64);
        assert!(!s.blob_path(&out[0].sha256).exists());
    }

    #[test]
    fn directory_expands_to_its_files_in_sorted_order() {
        let (ws, store) = setup();
        fs::create_dir_all(ws.path().join("pocs/sub")).unwrap();
        fs::write(ws.path().join("pocs/b.txt"), "b").unwrap();
        fs::write(ws.path().join("pocs/sub/a.txt"), "a").unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s.ingest(ws.path(), &[req("pocs")], 1024).unwrap();
        let paths: Vec<_> = out.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(paths, vec!["pocs/b.txt", "pocs/sub/a.txt"]);
    }

    #[test]
    fn missing_file_is_an_error() {
        let (ws, store) = setup();
        let err = ArtifactStore::new(store.path())
            .ingest(ws.path(), &[req("nope.txt")], 1024)
            .unwrap_err();
        assert!(matches!(err, ArtifactError::Missing { .. }), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_workspace_is_refused() {
        let (ws, store) = setup();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), ws.path().join("link")).unwrap();
        let err = ArtifactStore::new(store.path())
            .ingest(ws.path(), &[req("link")], 1024)
            .unwrap_err();
        assert!(matches!(err, ArtifactError::Escapes { .. }), "{err}");
    }

    #[test]
    fn absolute_path_is_refused() {
        let (ws, store) = setup();
        let err = ArtifactStore::new(store.path())
            .ingest(ws.path(), &[req("/etc/hosts")], 1024)
            .unwrap_err();
        assert!(matches!(err, ArtifactError::Path { .. }), "{err}");
    }

    fn walk(dir: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        for e in fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.to_string_lossy().into_owned());
            }
        }
        out
    }
}
