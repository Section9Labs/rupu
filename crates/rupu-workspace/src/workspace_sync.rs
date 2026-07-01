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
    #[error("workspace sync mixed-mode deltas (git + tar in one batch)")]
    MixedMode,
    #[error("workspace sync invalid path (absolute or traversal): {0}")]
    InvalidPath(String),
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
    // Homogeneity: every delta in a batch must share one mode. A mixed batch
    // would otherwise be silently routed to the first delta's handler.
    if deltas.iter().any(|d| d.mode != deltas[0].mode) {
        return Err(SyncError::MixedMode);
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
    // Reject hostile paths arriving over the wire before touching disk.
    for d in deltas {
        guard_delta_paths(d)?;
    }
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

// ── shared guards ───────────────────────────────────────────────────────────

/// Reject a single relative path that is absolute or escapes the workspace via
/// a `..` component. These paths arrive over the wire from a remote host.
fn guard_rel_path(p: &str) -> Result<(), SyncError> {
    let path = Path::new(p);
    use std::path::Component;
    for comp in path.components() {
        match comp {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SyncError::InvalidPath(p.to_string()));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate every `changed`/`deleted` path on a delta.
fn guard_delta_paths(d: &Delta) -> Result<(), SyncError> {
    for p in d.changed.iter().chain(d.deleted.iter()) {
        guard_rel_path(p)?;
    }
    Ok(())
}

// ── git mode ────────────────────────────────────────────────────────────────

fn git_err(e: git2::Error) -> SyncError {
    SyncError::Git(e.to_string())
}

/// Snapshot the current working tree (tracked + modified + untracked-not-ignored)
/// into a tree object WITHOUT persisting the user's index to disk: `add_all`
/// mutates only the in-memory index, and `write_tree` writes the tree to the
/// ODB (it does not touch the on-disk index file).
fn snapshot_tree(repo: &git2::Repository) -> Result<git2::Oid, SyncError> {
    let mut index = repo.index().map_err(git_err)?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(git_err)?;
    index.write_tree().map_err(git_err)
}

fn pack_git(workspace_path: &Path) -> Result<Payload, SyncError> {
    let repo = git2::Repository::discover(workspace_path).map_err(git_err)?;
    let tree_oid = snapshot_tree(&repo)?;
    let tree = repo.find_tree(tree_oid).map_err(git_err)?;
    let sig = git2::Signature::now("rupu-sync", "sync@rupu").map_err(git_err)?;
    // Parent = current HEAD if any, so the snapshot carries base history.
    let parent = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|oid| repo.find_commit(oid).ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let snap_oid = repo
        .commit(None, &sig, &sig, "rupu-sync snapshot", &tree, &parents)
        .map_err(git_err)?;
    // Pack the snapshot commit + its reachable tree/blobs into a packfile.
    let mut pb = repo.packbuilder().map_err(git_err)?;
    pb.insert_commit(snap_oid).map_err(git_err)?;
    let mut packbuf: Vec<u8> = Vec::new();
    pb.foreach(|chunk| {
        packbuf.extend_from_slice(chunk);
        true
    })
    .map_err(git_err)?;
    // Self-describing header: 40-byte snapshot oid hex, then the packfile.
    let mut bytes = snap_oid.to_string().into_bytes();
    bytes.extend_from_slice(&packbuf);
    Ok(Payload {
        mode: SyncMode::Git,
        bytes,
    })
}

fn stage_git(payload: &Payload, scratch_dir: &Path) -> Result<Baseline, SyncError> {
    if payload.bytes.len() < 40 {
        return Err(SyncError::Git("git payload too short".into()));
    }
    let oid_hex = std::str::from_utf8(&payload.bytes[..40])
        .map_err(|e| SyncError::Git(e.to_string()))?
        .to_string();
    let pack = &payload.bytes[40..];
    fs::create_dir_all(scratch_dir)?;
    let repo = git2::Repository::init(scratch_dir).map_err(git_err)?;
    {
        let odb = repo.odb().map_err(git_err)?;
        let mut writer = odb.packwriter().map_err(git_err)?;
        std::io::Write::write_all(&mut writer, pack)?;
        writer.commit().map_err(git_err)?;
    }
    let oid = git2::Oid::from_str(&oid_hex).map_err(git_err)?;
    let commit = repo.find_commit(oid).map_err(git_err)?;
    repo.branch("rupu-sync", &commit, true).map_err(git_err)?;
    repo.set_head("refs/heads/rupu-sync").map_err(git_err)?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(git_err)?;
    Ok(Baseline {
        mode: SyncMode::Git,
        tar_manifest: Default::default(),
        git_commit: Some(oid_hex),
    })
}

fn collect_delta_git(scratch_dir: &Path, baseline: &Baseline) -> Result<Delta, SyncError> {
    let repo = git2::Repository::open(scratch_dir).map_err(git_err)?;
    let oid = git2::Oid::from_str(
        baseline
            .git_commit
            .as_ref()
            .ok_or_else(|| SyncError::Git("git baseline missing commit oid".into()))?,
    )
    .map_err(git_err)?;
    let tree = repo
        .find_commit(oid)
        .map_err(git_err)?
        .tree()
        .map_err(git_err)?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))
        .map_err(git_err)?;
    let mut changed = Vec::new();
    let mut deleted = Vec::new();
    for d in diff.deltas() {
        let path = d
            .new_file()
            .path()
            .or_else(|| d.old_file().path())
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        if let Some(p) = path {
            if d.status() == git2::Delta::Deleted {
                deleted.push(p);
            } else {
                changed.push(p);
            }
        }
    }
    // Render a unified-diff patch. For +/-/context lines, libgit2's
    // `line.content()` omits the leading marker, so prepend `line.origin()`;
    // file/hunk-header lines already carry their full text.
    let mut patch: Vec<u8> = Vec::new();
    diff.print(git2::DiffFormat::Patch, |_d, _h, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin() as u8);
        }
        patch.extend_from_slice(line.content());
        true
    })
    .map_err(git_err)?;
    changed.sort();
    deleted.sort();
    Ok(Delta {
        mode: SyncMode::Git,
        changed,
        deleted,
        bytes: patch,
    })
}

fn apply_deltas_git(workspace_path: &Path, deltas: &[Delta]) -> Result<(), SyncError> {
    for d in deltas {
        guard_delta_paths(d)?;
    }
    let repo = git2::Repository::discover(workspace_path).map_err(git_err)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| SyncError::Git("bare repo has no workdir".into()))?
        .to_path_buf();

    // Common ancestor for the 3-way merge: a snapshot of the coordinator's
    // current working tree. Each remote staged from this same base, so its
    // patch applies cleanly onto `base_tree`.
    let base_oid = snapshot_tree(&repo)?;
    let base_tree = repo.find_tree(base_oid).map_err(git_err)?;

    // Fold each unit's patch into `ours_tree` via a 3-way tree merge. Disjoint
    // hunks in the same file both land; same-line edits surface as a conflict.
    let mut ours_tree = repo.find_tree(base_oid).map_err(git_err)?;
    for d in deltas {
        let diff = git2::Diff::from_buffer(&d.bytes).map_err(git_err)?;
        let mut theirs_index = repo
            .apply_to_tree(&base_tree, &diff, None)
            .map_err(git_err)?;
        let theirs_oid = theirs_index.write_tree_to(&repo).map_err(git_err)?;
        let theirs_tree = repo.find_tree(theirs_oid).map_err(git_err)?;

        let mut merged = repo
            .merge_trees(&base_tree, &ours_tree, &theirs_tree, None)
            .map_err(git_err)?;
        if merged.has_conflicts() {
            let mut paths: Vec<String> = Vec::new();
            if let Ok(conflicts) = merged.conflicts() {
                for c in conflicts.flatten() {
                    let entry = c.our.or(c.their).or(c.ancestor);
                    if let Some(e) = entry {
                        paths.push(String::from_utf8_lossy(&e.path).replace('\\', "/"));
                    }
                }
            }
            paths.sort();
            paths.dedup();
            if paths.is_empty() {
                paths = d.changed.clone();
            }
            return Err(SyncError::Conflict(paths));
        }
        let merged_oid = merged.write_tree_to(&repo).map_err(git_err)?;
        ours_tree = repo.find_tree(merged_oid).map_err(git_err)?;
    }

    // Materialize the merged tree into the workdir: write changed blobs, remove
    // deletions. Done by diffing base→merged so we touch only what changed.
    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&ours_tree), None)
        .map_err(git_err)?;
    for delta in diff.deltas() {
        if delta.status() == git2::Delta::Deleted {
            if let Some(p) = delta.old_file().path() {
                let abs = workdir.join(p);
                if abs.exists() {
                    fs::remove_file(abs)?;
                }
            }
            continue;
        }
        if let Some(p) = delta.new_file().path() {
            let blob = repo.find_blob(delta.new_file().id()).map_err(git_err)?;
            let abs = workdir.join(p);
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(abs, blob.content())?;
        }
    }
    Ok(())
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

    #[test]
    fn mixed_mode_deltas_rejected() {
        let ws = tempfile::tempdir().unwrap();
        let git = Delta {
            mode: SyncMode::Git,
            changed: vec!["a.txt".into()],
            deleted: vec![],
            bytes: vec![],
        };
        let tar = Delta {
            mode: SyncMode::Tar,
            changed: vec!["b.txt".into()],
            deleted: vec![],
            bytes: tar_one("b.txt", "B"),
        };
        let err = apply_deltas(ws.path(), &[git, tar]).unwrap_err();
        assert!(matches!(err, SyncError::MixedMode), "got {err:?}");
    }

    #[test]
    fn traversal_path_in_delta_rejected() {
        let ws = tempfile::tempdir().unwrap();
        let evil = Delta {
            mode: SyncMode::Tar,
            changed: vec![],
            deleted: vec!["../escape".into()],
            bytes: Vec::new(),
        };
        let err = apply_deltas_tar(ws.path(), &[evil]).unwrap_err();
        assert!(matches!(err, SyncError::InvalidPath(_)), "got {err:?}");
    }
}

#[cfg(test)]
mod git_sync_tests {
    use super::*;
    use std::fs;

    fn git_init(dir: &std::path::Path) {
        let repo = git2::Repository::init(dir).unwrap();
        // identity for commits
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "t").unwrap();
        cfg.set_str("user.email", "t@e").unwrap();
        fs::write(dir.join("a.txt"), "line1\nline2\nline3\n").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("a.txt")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
    }

    #[test]
    fn git_mode_detected_and_round_trips() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path());
        assert_eq!(detect_mode(ws.path()), SyncMode::Git);

        let payload = pack(ws.path()).unwrap();
        assert_eq!(payload.mode, SyncMode::Git);
        let scratch = tempfile::tempdir().unwrap();
        let baseline = stage(&payload, scratch.path()).unwrap();
        assert_eq!(
            fs::read_to_string(scratch.path().join("a.txt")).unwrap(),
            "line1\nline2\nline3\n"
        );

        // remote edits line2
        fs::write(scratch.path().join("a.txt"), "line1\nEDITED\nline3\n").unwrap();
        let delta = collect_delta(scratch.path(), &baseline).unwrap();
        assert!(delta.changed.contains(&"a.txt".to_string()));

        apply_deltas(ws.path(), &[delta]).unwrap();
        assert_eq!(
            fs::read_to_string(ws.path().join("a.txt")).unwrap(),
            "line1\nEDITED\nline3\n"
        );
    }

    #[test]
    fn git_non_overlapping_hunks_in_same_file_merge() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path()); // a.txt = line1/line2/line3

        let payload = pack(ws.path()).unwrap();
        // unit A edits line1; unit B edits line3 — same file, disjoint hunks.
        let sa = tempfile::tempdir().unwrap();
        let ba = stage(&payload, sa.path()).unwrap();
        fs::write(sa.path().join("a.txt"), "AAA\nline2\nline3\n").unwrap();
        let da = collect_delta(sa.path(), &ba).unwrap();

        let sb = tempfile::tempdir().unwrap();
        let bb = stage(&payload, sb.path()).unwrap();
        fs::write(sb.path().join("a.txt"), "line1\nline2\nBBB\n").unwrap();
        let db = collect_delta(sb.path(), &bb).unwrap();

        apply_deltas(ws.path(), &[da, db]).unwrap();
        assert_eq!(
            fs::read_to_string(ws.path().join("a.txt")).unwrap(),
            "AAA\nline2\nBBB\n"
        );
    }

    #[test]
    fn git_conflicting_hunks_error() {
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path());
        let payload = pack(ws.path()).unwrap();

        // both units edit line2 differently — a real conflict.
        let sa = tempfile::tempdir().unwrap();
        let ba = stage(&payload, sa.path()).unwrap();
        fs::write(sa.path().join("a.txt"), "line1\nAAA\nline3\n").unwrap();
        let da = collect_delta(sa.path(), &ba).unwrap();

        let sb = tempfile::tempdir().unwrap();
        let bb = stage(&payload, sb.path()).unwrap();
        fs::write(sb.path().join("a.txt"), "line1\nBBB\nline3\n").unwrap();
        let db = collect_delta(sb.path(), &bb).unwrap();

        let err = apply_deltas(ws.path(), &[da, db]).unwrap_err();
        assert!(matches!(err, SyncError::Conflict(_)), "got {err:?}");
    }
}
