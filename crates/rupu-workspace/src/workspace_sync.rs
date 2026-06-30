//! Cross-host workspace sync codec (multi-host Slice 3c). Git mode for git
//! repos (T4), tar mode for everything else. Pack on the coordinator, stage +
//! collect_delta on the remote, apply_deltas back on the coordinator.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Git,
    Tar,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("workspace sync io: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace conflict on: {0:?}")]
    Conflict(Vec<String>),
    #[error("workspace sync git: {0}")]
    Git(String),
}

#[derive(Debug, Clone)]
pub struct Payload {
    pub mode: SyncMode,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Delta {
    pub mode: SyncMode,
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
    pub bytes: Vec<u8>,
}

/// Baseline captured at stage time so `collect_delta` can diff against it.
/// Tar: a path→sha256 manifest. Git: the staged commit oid (see T4).
#[derive(Debug, Clone)]
pub struct Baseline {
    pub mode: SyncMode,
    pub tar_manifest: BTreeMap<String, [u8; 32]>,
    pub git_commit: Option<String>,
}

/// `Git` when a git repo is found at/above `workspace_path`, else `Tar`.
pub fn detect_mode(workspace_path: &Path) -> SyncMode {
    match git2::Repository::discover(workspace_path) {
        Ok(_) => SyncMode::Git,
        Err(_) => SyncMode::Tar,
    }
}

pub fn pack(workspace_path: &Path) -> Result<Payload, SyncError> {
    match detect_mode(workspace_path) {
        SyncMode::Git => pack_git(workspace_path), // implemented in T4
        SyncMode::Tar => pack_tar(workspace_path),
    }
}

pub fn stage(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError> {
    match payload.mode {
        SyncMode::Git => stage_git(payload, scratch_dir), // T4
        SyncMode::Tar => stage_tar(payload, scratch_dir),
    }
}

pub fn collect_delta(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError> {
    match baseline.mode {
        SyncMode::Git => collect_delta_git(scratch_dir, baseline), // T4
        SyncMode::Tar => collect_delta_tar(scratch_dir, baseline),
    }
}

pub fn apply_deltas(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError> {
    if deltas.is_empty() {
        return Ok(());
    }
    match deltas[0].mode {
        SyncMode::Git => apply_deltas_git(workspace_path, deltas), // T4
        SyncMode::Tar => apply_deltas_tar(workspace_path, deltas),
    }
}

// ── tar mode ────────────────────────────────────────────────────────────────

fn pack_tar(workspace_path: &Path) -> Result<Payload, SyncError> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        // `ignore` walks the tree honoring .gitignore + global ignores.
        // `add_custom_ignore_filename` ensures `.gitignore` is read even when
        // `workspace_path` has no `.git` ancestor (non-git temp dirs in tests,
        // or freshly-cloned remote workspaces before first commit).
        for entry in ignore::WalkBuilder::new(workspace_path)
            .hidden(false)
            .add_custom_ignore_filename(".gitignore")
            .build()
        {
            let entry = entry.map_err(|e| SyncError::Io(std::io::Error::other(e)))?;
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let abs = entry.path();
            let rel = abs
                .strip_prefix(workspace_path)
                .map_err(|e| SyncError::Io(std::io::Error::other(e)))?;
            builder.append_path_with_name(abs, rel)?;
        }
        builder.finish()?;
    }
    Ok(Payload {
        mode: SyncMode::Tar,
        bytes: buf,
    })
}

fn stage_tar(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError> {
    fs::create_dir_all(scratch_dir)?;
    let mut ar = tar::Archive::new(payload.bytes.as_slice());
    ar.unpack(scratch_dir)?;
    Ok(Baseline {
        mode: SyncMode::Tar,
        tar_manifest: hash_tree(scratch_dir)?,
        git_commit: None,
    })
}

fn collect_delta_tar(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError> {
    let after = hash_tree(scratch_dir)?;
    let mut changed = Vec::new();
    let mut deleted = Vec::new();
    for (path, hash) in &after {
        match baseline.tar_manifest.get(path) {
            Some(old) if old == hash => {}
            _ => changed.push(path.clone()),
        }
    }
    for path in baseline.tar_manifest.keys() {
        if !after.contains_key(path) {
            deleted.push(path.clone());
        }
    }
    changed.sort();
    deleted.sort();
    // Pack only the changed files into a tar.
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        for rel in &changed {
            builder.append_path_with_name(scratch_dir.join(rel), rel)?;
        }
        builder.finish()?;
    }
    Ok(Delta {
        mode: SyncMode::Tar,
        changed,
        deleted,
        bytes: buf,
    })
}

fn apply_deltas_tar(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError> {
    // Conflict = the same path changed/deleted by more than one delta.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut conflicts = Vec::new();
    for d in deltas {
        for p in d.changed.iter().chain(d.deleted.iter()) {
            let n = seen.entry(p.clone()).or_insert(0);
            *n += 1;
            if *n == 2 {
                conflicts.push(p.clone());
            }
        }
    }
    if !conflicts.is_empty() {
        conflicts.sort();
        conflicts.dedup();
        return Err(SyncError::Conflict(conflicts));
    }
    // No overlap — apply each delta: extract changed files, remove deleted.
    for d in deltas {
        let mut ar = tar::Archive::new(d.bytes.as_slice());
        ar.unpack(workspace_path)?;
        for rel in &d.deleted {
            let p = workspace_path.join(rel);
            if p.exists() {
                fs::remove_file(p)?;
            }
        }
    }
    Ok(())
}

/// Map of repo-relative path → sha256 of file contents, for the whole tree
/// under `root` (used as the tar baseline manifest).
fn hash_tree(root: &Path) -> Result<BTreeMap<String, [u8; 32]>, SyncError> {
    let mut map = BTreeMap::new();
    for entry in walkdir_files(root)? {
        let rel = entry
            .strip_prefix(root)
            .map_err(|e| SyncError::Io(std::io::Error::other(e)))?
            .to_string_lossy()
            .replace('\\', "/");
        let mut hasher = Sha256::new();
        hasher.update(fs::read(&entry)?);
        map.insert(rel, hasher.finalize().into());
    }
    Ok(map)
}

/// Recursively list regular files under `root` (no ignore filtering — the
/// scratch tree was already filtered at pack time).
fn walkdir_files(root: &Path) -> Result<Vec<PathBuf>, SyncError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
            }
        }
    }
    Ok(out)
}

// ── git mode stubs (implemented in T4) ─────────────────────────────────────

fn pack_git(_p: &Path) -> Result<Payload, SyncError> {
    Err(SyncError::Git(
        "git mode not yet implemented (3c T4)".into(),
    ))
}
fn stage_git(_p: &Payload, _s: &Path) -> Result<Baseline, SyncError> {
    Err(SyncError::Git(
        "git mode not yet implemented (3c T4)".into(),
    ))
}
fn collect_delta_git(_s: &Path, _b: &Baseline) -> Result<Delta, SyncError> {
    Err(SyncError::Git(
        "git mode not yet implemented (3c T4)".into(),
    ))
}
fn apply_deltas_git(_w: &Path, _d: &[Delta]) -> Result<(), SyncError> {
    Err(SyncError::Git(
        "git mode not yet implemented (3c T4)".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    /// Helper: build a one-file tar payload (matches the delta bytes format).
    fn tar_one(path: &str, body: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            b.append_data(&mut header, path, body.as_bytes()).unwrap();
            b.finish().unwrap();
        }
        buf
    }

    /// Note: `detect_mode` uses `git2::Repository::discover` which walks upward.
    /// If the system temp dir is inside a git repo (unlikely but possible in
    /// some CI environments), this test will spuriously fail. Mark `#[ignore]`
    /// in that case and verify manually.
    #[test]
    fn tar_mode_detected_for_non_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.txt", "hello");
        assert_eq!(detect_mode(dir.path()), SyncMode::Tar);
    }

    /// Calls `pack_tar`/`stage_tar`/`collect_delta_tar`/`apply_deltas_tar`
    /// directly (crate-private, same module) so the test never depends on
    /// the temp dir's git ancestry.
    #[test]
    fn tar_round_trip_create_modify_delete() {
        // coordinator workspace
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), "keep.txt", "keep");
        write(ws.path(), "mod.txt", "before");
        write(ws.path(), "gone.txt", "remove me");

        // pack + stage to a remote scratch
        let payload = pack_tar(ws.path()).unwrap();
        assert_eq!(payload.mode, SyncMode::Tar);
        let scratch = tempfile::tempdir().unwrap();
        let baseline = stage_tar(&payload, scratch.path()).unwrap();

        // "remote agent" mutates the scratch tree
        write(scratch.path(), "mod.txt", "after");
        write(scratch.path(), "new.txt", "created");
        fs::remove_file(scratch.path().join("gone.txt")).unwrap();

        // collect the delta
        let delta = collect_delta_tar(scratch.path(), &baseline).unwrap();
        assert!(delta.changed.contains(&"mod.txt".to_string()));
        assert!(delta.changed.contains(&"new.txt".to_string()));
        assert!(delta.deleted.contains(&"gone.txt".to_string()));
        assert!(!delta.changed.contains(&"keep.txt".to_string()));

        // apply back to the coordinator workspace
        apply_deltas_tar(ws.path(), &[delta]).unwrap();
        assert_eq!(
            fs::read_to_string(ws.path().join("mod.txt")).unwrap(),
            "after"
        );
        assert_eq!(
            fs::read_to_string(ws.path().join("new.txt")).unwrap(),
            "created"
        );
        assert!(!ws.path().join("gone.txt").exists());
        assert_eq!(
            fs::read_to_string(ws.path().join("keep.txt")).unwrap(),
            "keep"
        );
    }

    /// Calls `pack_tar`/`stage_tar` directly to avoid git discovery.
    #[test]
    fn tar_pack_respects_gitignore() {
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), ".gitignore", "target/\n*.log\n");
        write(ws.path(), "src.rs", "code");
        write(ws.path(), "target/junk.o", "binary");
        write(ws.path(), "run.log", "noise");

        let payload = pack_tar(ws.path()).unwrap();
        let scratch = tempfile::tempdir().unwrap();
        stage_tar(&payload, scratch.path()).unwrap();
        assert!(scratch.path().join("src.rs").exists());
        assert!(!scratch.path().join("target/junk.o").exists());
        assert!(!scratch.path().join("run.log").exists());
    }

    #[test]
    fn tar_apply_disjoint_deltas_merges() {
        let ws = tempfile::tempdir().unwrap();
        write(ws.path(), "base", "x");
        let d1 = Delta {
            mode: SyncMode::Tar,
            changed: vec!["a.txt".into()],
            deleted: vec![],
            bytes: tar_one("a.txt", "AAA"),
        };
        let d2 = Delta {
            mode: SyncMode::Tar,
            changed: vec!["b.txt".into()],
            deleted: vec![],
            bytes: tar_one("b.txt", "BBB"),
        };
        apply_deltas_tar(ws.path(), &[d1, d2]).unwrap();
        assert_eq!(fs::read_to_string(ws.path().join("a.txt")).unwrap(), "AAA");
        assert_eq!(fs::read_to_string(ws.path().join("b.txt")).unwrap(), "BBB");
    }

    #[test]
    fn tar_apply_overlapping_deltas_conflicts() {
        let ws = tempfile::tempdir().unwrap();
        let d1 = Delta {
            mode: SyncMode::Tar,
            changed: vec!["shared.txt".into()],
            deleted: vec![],
            bytes: tar_one("shared.txt", "A"),
        };
        let d2 = Delta {
            mode: SyncMode::Tar,
            changed: vec!["shared.txt".into()],
            deleted: vec![],
            bytes: tar_one("shared.txt", "B"),
        };
        let err = apply_deltas_tar(ws.path(), &[d1, d2]).unwrap_err();
        match err {
            SyncError::Conflict(paths) => assert!(paths.contains(&"shared.txt".to_string())),
            other => panic!("expected Conflict, got {other:?}"),
        }
    }
}
