//! Content-addressed store for finding artifacts (PoC scripts, outputs,
//! harnesses, binaries).
//!
//! Layout: `<root>/<first two hex chars>/<sha256>`. Identical content is
//! stored once across runs and projects. Files over the size cap are hashed
//! but not copied, and recorded as `external`.
//!
//! One report's artifacts are bounded ([`IngestLimits`]): every requested
//! path is resolved and expanded first, and the whole set is refused if it
//! is too many files, or if the files that would be copied into the store add
//! up to too many bytes, before a single byte is copied. A file over the
//! per-file cap is only hashed and recorded by reference, so it never counts
//! toward the total and never causes a refusal.

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
    #[error("the requested artifacts expand to more than {max} files (`[findings].artifact_max_files` = {max}); list specific files instead of large directories")]
    TooManyFiles { max: usize },
    #[error("the artifacts to be copied into the store total {total} bytes across {files} files, over the {max} byte limit (`[findings].artifact_total_max_bytes`); files larger than `[findings].artifact_max_bytes` are recorded by reference and do not count; list specific files instead of large directories")]
    TooLarge { total: u64, files: usize, max: u64 },
}

/// Bounds on one report's artifacts. See [`crate::report::FindingWriteOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IngestLimits {
    /// Files up to this size are copied; larger ones are recorded external.
    pub max_file_bytes: u64,
    /// Most files the requested paths may expand to.
    pub max_files: usize,
    /// Most bytes the files that will be copied (those at or under
    /// `max_file_bytes`) may add up to. Files over `max_file_bytes` are
    /// recorded external and do not count.
    pub max_total_bytes: u64,
}

pub struct ArtifactStore {
    root: PathBuf,
}

const CHUNK: usize = 64 * 1024;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// RAII guard for temporary files: removes the file on drop unless disarmed.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Consume the guard without dropping the file (e.g., after successful rename).
    fn disarm(self) {
        std::mem::forget(self);
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// SHA-256 of a file's contents, streamed.
pub fn sha256_file(p: &Path) -> std::io::Result<String> {
    sha256_file_counted(p).map(|(sha, _)| sha)
}

/// SHA-256 of everything `r` yields from its current position to EOF, streamed.
/// Lets a caller hash the exact handle it will later serve, so the bytes served
/// are the bytes hashed (a path can be re-pointed between two opens; an open
/// handle cannot).
pub fn sha256_reader<R: Read>(r: &mut R) -> std::io::Result<String> {
    sha256_reader_counted(r).map(|(sha, _)| sha)
}

/// [`sha256_reader`] plus the number of bytes actually hashed.
fn sha256_reader_counted<R: Read>(r: &mut R) -> std::io::Result<(String, u64)> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex(&h.finalize()), total))
}

/// SHA-256 of a file's contents plus the number of bytes actually hashed.
fn sha256_file_counted(p: &Path) -> std::io::Result<(String, u64)> {
    sha256_reader_counted(&mut File::open(p)?)
}

/// The total-bytes bound over the files `resolve` found. Only files that
/// will be copied count toward it: a file over the per-file cap is hashed and
/// recorded external, so it consumes no store space and must not get the
/// finding rejected.
fn check_total(
    files: &[(String, PathBuf, u64)],
    limits: IngestLimits,
) -> Result<(), ArtifactError> {
    let mut total = 0u64;
    let mut copied = 0usize;
    for (_, _, size) in files {
        if *size <= limits.max_file_bytes {
            total = total.saturating_add(*size);
            copied += 1;
        }
    }
    if total > limits.max_total_bytes {
        return Err(ArtifactError::TooLarge {
            total,
            files: copied,
            max: limits.max_total_bytes,
        });
    }
    Ok(())
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

    /// Like `blob_path`, but only for a well-formed sha256 (64 lowercase hex).
    /// Use this for any caller-supplied digest (e.g. an HTTP path segment).
    pub fn blob_path_checked(&self, sha256: &str) -> Option<PathBuf> {
        let ok = sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        ok.then(|| self.blob_path(sha256))
    }

    /// Resolve every requested path (expanding directories), check the whole
    /// set against `limits`, then hash and either copy (≤
    /// `limits.max_file_bytes`) or record as external. The total-bytes bound
    /// covers only the files that will be copied; a file over the per-file
    /// cap is recorded by reference and never causes a refusal. Agent-supplied
    /// metadata on `requested` is ignored; only `path` is read.
    pub fn ingest(
        &self,
        workspace: &Path,
        requested: &[ArtifactRef],
        limits: IngestLimits,
    ) -> Result<Vec<ArtifactRef>, ArtifactError> {
        let files = self.resolve(workspace, requested, limits.max_files)?;
        check_total(&files, limits)?;
        files
            .iter()
            .map(|(rel, abs, size)| self.ingest_file(rel, abs, *size, limits.max_file_bytes))
            .collect()
    }

    /// What [`ingest`](Self::ingest) would refuse before it reads a byte:
    /// every requested path must exist inside the workspace and be a file or
    /// a directory, and the set they expand to must be within `limits`.
    /// Nothing is hashed, copied or created (the store need not exist), so a
    /// file that then cannot be read, or a store that cannot be written, is
    /// only found by a real `ingest`.
    pub fn check(
        &self,
        workspace: &Path,
        requested: &[ArtifactRef],
        limits: IngestLimits,
    ) -> Result<(), ArtifactError> {
        let files = self.resolve(workspace, requested, limits.max_files)?;
        check_total(&files, limits)
    }

    /// Every file the requested paths name, as `(workspace-relative path,
    /// canonical path, size)`, deduplicated, in request order (a directory's
    /// files sorted). Nothing is read or copied here; expansion stops as soon
    /// as it passes `max_files`.
    fn resolve(
        &self,
        workspace: &Path,
        requested: &[ArtifactRef],
        max_files: usize,
    ) -> Result<Vec<(String, PathBuf, u64)>, ArtifactError> {
        let ws_canon = fs::canonicalize(workspace).map_err(|source| ArtifactError::Io {
            path: workspace.display().to_string(),
            source,
        })?;
        let mut out: Vec<(String, PathBuf, u64)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut push = |rel: String, abs: PathBuf, out: &mut Vec<_>| -> Result<(), ArtifactError> {
            if !seen.insert(abs.clone()) {
                return Ok(());
            }
            if out.len() >= max_files {
                return Err(ArtifactError::TooManyFiles { max: max_files });
            }
            let size = fs::metadata(&abs)
                .map_err(|source| ArtifactError::Io {
                    path: rel.clone(),
                    source,
                })?
                .len();
            out.push((rel, abs, size));
            Ok(())
        };
        for r in requested {
            if let Some(reason) = rel_path_problem(&r.path) {
                return Err(ArtifactError::Path {
                    path: r.path.clone(),
                    reason,
                });
            }
            if names_workspace_root(&r.path) {
                return Err(ArtifactError::Path {
                    path: r.path.clone(),
                    reason: WORKSPACE_ROOT_REASON,
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
            // A path that resolves to the root (a symlink to `.`, say) is the
            // same request as `.`.
            if canon == ws_canon {
                return Err(ArtifactError::Path {
                    path: r.path.clone(),
                    reason: WORKSPACE_ROOT_REASON,
                });
            }
            if canon.is_dir() {
                let mut files = Vec::new();
                collect_files(&canon, &mut files, max_files).map_err(|source| {
                    ArtifactError::Io {
                        path: r.path.clone(),
                        source,
                    }
                })?;
                if files.len() > max_files {
                    return Err(ArtifactError::TooManyFiles { max: max_files });
                }
                files.sort();
                for f in files {
                    let rel = f
                        .strip_prefix(&ws_canon)
                        .expect("collected under the workspace")
                        .to_string_lossy()
                        .replace('\\', "/");
                    push(rel, f, &mut out)?;
                }
            } else if fs::metadata(&canon)
                .map_err(|source| ArtifactError::Io {
                    path: r.path.clone(),
                    source,
                })?
                .is_file()
            {
                push(r.path.clone(), canon, &mut out)?;
            } else {
                return Err(ArtifactError::Path {
                    path: r.path.clone(),
                    reason: "not a regular file or directory",
                });
            }
        }
        Ok(out)
    }

    fn ingest_file(
        &self,
        rel: &str,
        abs: &Path,
        listed_size: u64,
        max_bytes: u64,
    ) -> Result<ArtifactRef, ArtifactError> {
        let io = |source| ArtifactError::Io {
            path: rel.to_string(),
            source,
        };
        let kind = sniff_kind(abs).map_err(io)?;
        // `size` is what was actually read, not the earlier stat: the file
        // may have changed in between, and the hash covers what was read.
        let (sha256, size, stored) = if listed_size > max_bytes {
            let (sha, n) = sha256_file_counted(abs).map_err(io)?;
            (sha, n, ArtifactStorage::External)
        } else {
            let (sha, n) = self.copy_hashing(abs).map_err(io)?;
            (sha, n, ArtifactStorage::Copied)
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
    /// Returns the hash and the number of bytes copied. Cleans up the temp
    /// file on any error via RAII guard.
    fn copy_hashing(&self, src: &Path) -> std::io::Result<(String, u64)> {
        create_store_dir(&self.root)?;
        let tmp_path = self.root.join(format!("{}.tmp", ulid::Ulid::new()));
        let tmp = TempFile::new(tmp_path.clone());

        let mut input = File::open(src)?;
        let mut output = File::create(&tmp_path)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            output.write_all(&buf[..n])?;
            total += n as u64;
        }
        output.flush()?;
        // Durable before it becomes visible at its content address: a blob
        // that exists must hold the bytes its name promises.
        output.sync_all()?;
        drop(output);
        let sha = hex(&h.finalize());
        let dest = self.blob_path(&sha);
        if dest.exists() {
            fs::remove_file(&tmp_path)?;
            tmp.disarm();
        } else {
            fs::create_dir_all(dest.parent().expect("blob path has a parent"))?;
            fs::rename(&tmp_path, &dest)?;
            tmp.disarm();
        }
        Ok((sha, total))
    }
}

/// Create the store root (and its parents). On unix the root itself is
/// private to the user (0700): it holds proof-of-concept material from every
/// project. An existing directory's mode is left alone.
fn create_store_dir(root: &Path) -> std::io::Result<()> {
    if root.is_dir() {
        return Ok(());
    }
    if let Some(parent) = root.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(root)
}

const WORKSPACE_ROOT_REASON: &str =
    "names the workspace root; list the files or directories inside it that prove the finding";

/// `.`, `./`, `./.` and the like: a path whose every component is `.`.
pub(crate) fn names_workspace_root(p: &str) -> bool {
    Path::new(p)
        .components()
        .all(|c| matches!(c, std::path::Component::CurDir))
}

/// Every regular file under `dir`. Symlinks are skipped, not followed: a link
/// inside a PoC directory must not pull in files from elsewhere. Stops once
/// `out` holds more than `max` files, so an oversized directory is refused
/// without walking all of it.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, max: usize) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        if out.len() > max {
            return Ok(());
        }
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_files(&entry.path(), out, max)?;
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

    #[test]
    fn sha256_reader_hashes_from_the_current_position_and_matches_the_file_hash() {
        use std::io::{Seek, SeekFrom};
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("a.bin");
        fs::write(&p, b"hello world").unwrap();
        // Known SHA-256 of "hello world".
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        assert_eq!(sha256_file(&p).unwrap(), expected);

        let mut f = File::open(&p).unwrap();
        assert_eq!(sha256_reader(&mut f).unwrap(), expected);
        // The handle is now at EOF: a second pass hashes nothing (SHA-256 of
        // the empty input) until the caller rewinds.
        assert_eq!(
            sha256_reader(&mut f).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        f.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(sha256_reader(&mut f).unwrap(), expected);
        // Works on any reader, and across multiple CHUNK-sized reads.
        let big = vec![7u8; CHUNK * 2 + 5];
        fs::write(&p, &big).unwrap();
        assert_eq!(
            sha256_reader(&mut std::io::Cursor::new(&big)).unwrap(),
            sha256_file(&p).unwrap()
        );
    }

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

    fn limits(max_file_bytes: u64) -> IngestLimits {
        IngestLimits {
            max_file_bytes,
            max_files: crate::report::DEFAULT_ARTIFACT_MAX_FILES,
            max_total_bytes: crate::report::DEFAULT_ARTIFACT_TOTAL_MAX_BYTES,
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
            .ingest(ws.path(), &[req("pocs/result.txt")], limits(1024))
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

    #[cfg(unix)]
    #[test]
    fn a_new_store_root_is_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let (ws, parent) = setup();
        fs::write(ws.path().join("out.txt"), "rc=0\n").unwrap();
        let root = parent.path().join("findings").join("artifacts");
        ArtifactStore::new(&root)
            .ingest(ws.path(), &[req("out.txt")], limits(1024))
            .unwrap();
        let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "store root mode {mode:o}");
    }

    #[test]
    fn identical_content_is_stored_once() {
        let (ws, store) = setup();
        fs::write(ws.path().join("a.bin"), [0u8, 1, 2, 3]).unwrap();
        fs::write(ws.path().join("b.bin"), [0u8, 1, 2, 3]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s
            .ingest(ws.path(), &[req("a.bin"), req("b.bin")], limits(1024))
            .unwrap();
        assert_eq!(out[0].sha256, out[1].sha256);
        assert_eq!(out[0].kind, Some(ArtifactKind::Binary));
        let blobs: Vec<_> = walk(store.path());
        let tmp_files: Vec<_> = blobs.iter().filter(|p| p.ends_with(".tmp")).collect();
        assert!(
            tmp_files.is_empty(),
            "no temp files should remain; found: {:?}",
            tmp_files
        );
        let real_blobs: Vec<_> = blobs.into_iter().filter(|p| !p.ends_with(".tmp")).collect();
        assert_eq!(real_blobs.len(), 1);
    }

    #[test]
    fn over_cap_file_is_recorded_external_and_not_copied() {
        let (ws, store) = setup();
        fs::write(ws.path().join("big.img"), vec![7u8; 2048]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s
            .ingest(ws.path(), &[req("big.img")], limits(1024))
            .unwrap();
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
        let out = s.ingest(ws.path(), &[req("pocs")], limits(1024)).unwrap();
        let paths: Vec<_> = out.iter().map(|a| a.path.as_str()).collect();
        assert_eq!(paths, vec!["pocs/b.txt", "pocs/sub/a.txt"]);
    }

    #[test]
    fn missing_file_is_an_error() {
        let (ws, store) = setup();
        let err = ArtifactStore::new(store.path())
            .ingest(ws.path(), &[req("nope.txt")], limits(1024))
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
            .ingest(ws.path(), &[req("link")], limits(1024))
            .unwrap_err();
        assert!(matches!(err, ArtifactError::Escapes { .. }), "{err}");
    }

    #[test]
    fn absolute_path_is_refused() {
        let (ws, store) = setup();
        let err = ArtifactStore::new(store.path())
            .ingest(ws.path(), &[req("/etc/hosts")], limits(1024))
            .unwrap_err();
        assert!(matches!(err, ArtifactError::Path { .. }), "{err}");
    }

    #[test]
    fn temp_file_cleaned_up_on_error() {
        let (ws, store) = setup();
        fs::write(ws.path().join("src.txt"), "data").unwrap();

        // Compute the expected sha256 of "data"
        let expected_sha = {
            let mut h = sha2::Sha256::new();
            h.update(b"data");
            hex(&h.finalize())
        };

        // Pre-create a regular FILE at <root>/<first two hex chars>,
        // so create_dir_all in the blob path will fail.
        fs::create_dir_all(store.path()).unwrap();
        let shard = store.path().join(&expected_sha[..2]);
        fs::write(&shard, "obstacle").unwrap();

        let s = ArtifactStore::new(store.path());
        let err = s
            .ingest(ws.path(), &[req("src.txt")], limits(1024))
            .unwrap_err();

        // Error should be Io (from create_dir_all failure)
        assert!(matches!(err, ArtifactError::Io { .. }), "{err}");

        // Assert no .tmp files remain in the store
        let blobs: Vec<_> = walk(store.path());
        let tmp_files: Vec<_> = blobs.iter().filter(|p| p.ends_with(".tmp")).collect();
        assert!(
            tmp_files.is_empty(),
            "temp file should have been cleaned up; found: {:?}",
            tmp_files
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_refused_without_blocking() {
        let (ws, store) = setup();

        // Try to create a FIFO. If mkfifo is not available, skip gracefully.
        let fifo_path = ws.path().join("blocked.fifo");
        let mkfifo_result = std::process::Command::new("mkfifo")
            .arg(&fifo_path)
            .status();

        if mkfifo_result.is_err() || !mkfifo_result.unwrap().success() {
            // mkfifo not available or failed; skip test
            return;
        }

        let s = ArtifactStore::new(store.path());
        let err = s
            .ingest(ws.path(), &[req("blocked.fifo")], limits(1024))
            .unwrap_err();

        // Should be a Path error for "not a regular file or directory"
        assert!(
            matches!(err, ArtifactError::Path { reason, .. } if reason.contains("not a regular file")),
            "expected Path error, got: {err}"
        );
    }

    #[test]
    fn too_many_files_is_refused_before_anything_is_copied() {
        let (ws, store) = setup();
        fs::create_dir_all(ws.path().join("pocs")).unwrap();
        for n in 0..4 {
            fs::write(ws.path().join(format!("pocs/f{n}.txt")), format!("{n}")).unwrap();
        }
        let s = ArtifactStore::new(store.path());
        let tight = IngestLimits {
            max_files: 3,
            ..limits(1024)
        };
        let err = s.ingest(ws.path(), &[req("pocs")], tight).unwrap_err();
        assert!(
            matches!(err, ArtifactError::TooManyFiles { max: 3 }),
            "{err}"
        );
        assert!(err.to_string().contains("more than 3 files"), "{err}");
        // Across separately listed files too.
        let err = s
            .ingest(
                ws.path(),
                &[
                    req("pocs/f0.txt"),
                    req("pocs/f1.txt"),
                    req("pocs/f2.txt"),
                    req("pocs/f3.txt"),
                ],
                tight,
            )
            .unwrap_err();
        assert!(matches!(err, ArtifactError::TooManyFiles { .. }), "{err}");
        assert!(!store.path().exists() || walk(store.path()).is_empty());
        // Exactly at the cap is fine, and a file listed twice counts once.
        let ok = s
            .ingest(
                ws.path(),
                &[req("pocs/f0.txt"), req("pocs/f0.txt"), req("pocs/f1.txt")],
                IngestLimits {
                    max_files: 2,
                    ..limits(1024)
                },
            )
            .unwrap();
        assert_eq!(ok.len(), 2);
    }

    #[test]
    fn total_bytes_over_the_cap_is_refused_before_anything_is_copied() {
        let (ws, store) = setup();
        fs::write(ws.path().join("a.bin"), vec![1u8; 600]).unwrap();
        fs::write(ws.path().join("b.bin"), vec![2u8; 600]).unwrap();
        let s = ArtifactStore::new(store.path());
        let err = s
            .ingest(
                ws.path(),
                &[req("a.bin"), req("b.bin")],
                IngestLimits {
                    max_total_bytes: 1000,
                    ..limits(1024)
                },
            )
            .unwrap_err();
        assert!(
            matches!(
                err,
                ArtifactError::TooLarge {
                    total: 1200,
                    files: 2,
                    max: 1000
                }
            ),
            "{err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("1200 bytes across 2 files"), "{msg}");
        assert!(!store.path().exists() || walk(store.path()).is_empty());
    }

    #[test]
    fn a_file_over_both_caps_is_recorded_external_and_does_not_reject() {
        let (ws, store) = setup();
        fs::write(ws.path().join("huge.bin"), vec![7u8; 100]).unwrap();
        let s = ArtifactStore::new(store.path());
        let out = s
            .ingest(
                ws.path(),
                &[req("huge.bin")],
                IngestLimits {
                    max_file_bytes: 10,
                    max_total_bytes: 15,
                    ..limits(10)
                },
            )
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stored, Some(ArtifactStorage::External));
        assert_eq!(out[0].size, 100);
        assert_eq!(out[0].sha256.len(), 64);
        // Recorded by reference: nothing was copied into the store.
        assert!(!store.path().exists() || walk(store.path()).is_empty());
    }

    #[test]
    fn external_files_do_not_count_toward_the_total_but_copied_ones_do() {
        let (ws, store) = setup();
        // One file over the per-file cap, two under it.
        fs::write(ws.path().join("huge.bin"), vec![7u8; 100]).unwrap();
        fs::write(ws.path().join("a.bin"), vec![1u8; 8]).unwrap();
        fs::write(ws.path().join("b.bin"), vec![2u8; 8]).unwrap();
        let s = ArtifactStore::new(store.path());
        let tight = IngestLimits {
            max_file_bytes: 10,
            max_total_bytes: 15,
            ..limits(10)
        };
        // Two copyable files sum to 16 > 15: still refused, and the huge
        // external file is not in the count.
        let err = s
            .ingest(
                ws.path(),
                &[req("huge.bin"), req("a.bin"), req("b.bin")],
                tight,
            )
            .unwrap_err();
        assert!(
            matches!(
                err,
                ArtifactError::TooLarge {
                    total: 16,
                    files: 2,
                    max: 15
                }
            ),
            "{err}"
        );
        assert!(!store.path().exists() || walk(store.path()).is_empty());
        // Without the second copyable file the set fits, so the huge file is
        // recorded external alongside the one copied file.
        let out = s
            .ingest(ws.path(), &[req("huge.bin"), req("a.bin")], tight)
            .unwrap();
        assert_eq!(out[0].stored, Some(ArtifactStorage::External));
        assert_eq!(out[1].stored, Some(ArtifactStorage::Copied));
    }

    #[test]
    fn the_workspace_root_is_refused() {
        let (ws, store) = setup();
        fs::write(ws.path().join("a.txt"), "a").unwrap();
        let s = ArtifactStore::new(store.path());
        for p in [".", "./", "./."] {
            let err = s.ingest(ws.path(), &[req(p)], limits(1024)).unwrap_err();
            assert!(
                matches!(&err, ArtifactError::Path { reason, .. } if reason.contains("workspace root")),
                "{p}: {err}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_the_workspace_root_is_refused() {
        let (ws, store) = setup();
        std::os::unix::fs::symlink(ws.path(), ws.path().join("all")).unwrap();
        let err = ArtifactStore::new(store.path())
            .ingest(ws.path(), &[req("all")], limits(1024))
            .unwrap_err();
        assert!(
            matches!(&err, ArtifactError::Path { reason, .. } if reason.contains("workspace root")),
            "{err}"
        );
    }

    #[test]
    fn blob_path_checked_rejects_non_hex_and_wrong_length() {
        let s = ArtifactStore::new("/tmp/store");
        assert!(s.blob_path_checked("").is_none());
        assert!(s.blob_path_checked("../etc/passwd").is_none());
        assert!(s.blob_path_checked(&"A".repeat(64)).is_none()); // uppercase
        assert!(s.blob_path_checked(&"a".repeat(63)).is_none());
        let ok = "0123456789abcdef".repeat(4);
        assert_eq!(
            s.blob_path_checked(&ok).unwrap(),
            std::path::PathBuf::from("/tmp/store").join("01").join(&ok)
        );
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
