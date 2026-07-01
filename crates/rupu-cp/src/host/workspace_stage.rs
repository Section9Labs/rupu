//! Shared workspace stage/collect core used by every HostConnector that
//! stages locally (Local, the remote `rupu __workspace` helper, HttpCp). One
//! implementation so the transports can't diverge.

use std::path::{Path, PathBuf};

use ulid::Ulid;

use crate::host::connector::{
    decode_payload, deserialize_baseline, encode_delta, serialize_baseline, HostConnectorError,
    MAX_WORKSPACE_BYTES,
};

/// Stage a packed workspace under `<cache_root>/workspace-sync/<ulid>/work`,
/// persisting the baseline sidecar one level up. Returns the `work` path.
pub fn stage_to_dir(payload: &[u8], cache_root: &Path) -> Result<String, HostConnectorError> {
    if payload.len() > MAX_WORKSPACE_BYTES {
        return Err(HostConnectorError::Invalid(format!(
            "workspace payload {} bytes exceeds limit {MAX_WORKSPACE_BYTES}",
            payload.len()
        )));
    }
    let decoded = decode_payload(payload)?;
    let base = cache_root
        .join("workspace-sync")
        .join(Ulid::new().to_string());
    let work = base.join("work");
    let baseline = rupu_workspace::stage(&decoded, &work)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    std::fs::write(base.join("baseline.json"), serialize_baseline(&baseline)?)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    Ok(work.to_string_lossy().into_owned())
}

/// Reload the baseline, diff the working dir, return the encoded delta, and
/// remove the scratch. `working_dir` is confined under `<cache_root>/workspace-sync`.
/// The scratch `base` dir is removed unconditionally once resolved, whether
/// the delta build succeeds or fails, so a bad baseline / collect error never
/// leaks the scratch dir.
pub fn collect_from_dir(
    working_dir: &str,
    cache_root: &Path,
) -> Result<Vec<u8>, HostConnectorError> {
    let sync_root = cache_root.join("workspace-sync");
    let base = resolve_confined_base(working_dir, &sync_root)?;
    let result = (|| {
        let baseline_bytes = std::fs::read(base.join("baseline.json"))
            .map_err(|e| HostConnectorError::Invalid(format!("baseline missing: {e}")))?;
        let baseline = deserialize_baseline(&baseline_bytes)?;
        let work = base.join("work");
        let delta = rupu_workspace::collect_delta(&work, &baseline)
            .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
        Ok(encode_delta(&delta))
    })();
    let _ = std::fs::remove_dir_all(&base);
    result
}

/// Best-effort discard of a staged workspace scratch dir. Used when the unit
/// that consumed the staged tree failed *between* stage and collect (launch
/// failure, poll timeout) so `collect_from_dir` never ran and the scratch
/// would otherwise leak forever. `working_dir` is confined under
/// `<cache_root>/workspace-sync`, same as [`collect_from_dir`].
pub fn discard_from_dir(working_dir: &str, cache_root: &Path) -> Result<(), HostConnectorError> {
    let sync_root = cache_root.join("workspace-sync");
    let base = resolve_confined_base(working_dir, &sync_root)?;
    let _ = std::fs::remove_dir_all(&base);
    Ok(())
}

/// Confine `working_dir` under `sync_root` and return its parent (the
/// per-request scratch `base` dir holding `work/` and `baseline.json`).
/// Shared by [`collect_from_dir`] and [`discard_from_dir`].
fn resolve_confined_base(
    working_dir: &str,
    sync_root: &Path,
) -> Result<PathBuf, HostConnectorError> {
    let work = confine(Path::new(working_dir), sync_root)?;
    work.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| HostConnectorError::Invalid("invalid working dir".into()))
}

/// Canonicalize `path` and confirm it stays under `root`. Rejects `..`/absolute
/// escapes.
pub(crate) fn confine(path: &Path, root: &Path) -> Result<PathBuf, HostConnectorError> {
    let canon = path
        .canonicalize()
        .map_err(|e| HostConnectorError::Invalid(format!("path: {e}")))?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| HostConnectorError::Invalid(format!("root: {e}")))?;
    if !canon.starts_with(&root_canon) {
        return Err(HostConnectorError::Invalid(format!(
            "path escapes workspace-sync root: {}",
            canon.display()
        )));
    }
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn stage_then_collect_round_trips_tar() {
        // build a tar payload from a non-git workspace via rupu_workspace::pack
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = crate::host::connector::encode_payload(&payload); // wire form used by decode_payload

        let cache = tempfile::tempdir().unwrap();
        let work = stage_to_dir(&encoded, cache.path()).unwrap();
        // simulate a remote edit
        fs::write(std::path::Path::new(&work).join("a.txt"), "EDITED").unwrap();
        let delta_bytes = collect_from_dir(&work, cache.path()).unwrap();
        let delta = crate::host::connector::decode_delta(&delta_bytes).unwrap();
        assert!(delta.changed.iter().any(|p| p == "a.txt"));
        // scratch cleaned
        assert!(!std::path::Path::new(&work).exists());
    }

    /// Mirrors `git_init` in `rupu_workspace::workspace_sync`'s git tests: a
    /// minimal repo with one committed file, so `rupu_workspace::pack` detects
    /// git mode and the staged baseline carries a `git_commit: Some(..)`
    /// sidecar rather than the tar-mode snapshot.
    fn git_init(dir: &std::path::Path) {
        let repo = git2::Repository::init(dir).unwrap();
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
    fn stage_then_collect_round_trips_git() {
        // build a git payload via rupu_workspace::pack (auto-detects git mode
        // from the repo's baseline commit) so the shared core is exercised on
        // the `git_commit: Some(..)` sidecar path, not just tar.
        let ws = tempfile::tempdir().unwrap();
        git_init(ws.path());
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        assert_eq!(payload.mode, rupu_workspace::SyncMode::Git);
        let encoded = crate::host::connector::encode_payload(&payload);

        let cache = tempfile::tempdir().unwrap();
        let work = stage_to_dir(&encoded, cache.path()).unwrap();
        // simulate a remote edit
        fs::write(
            std::path::Path::new(&work).join("a.txt"),
            "line1\nEDITED\nline3\n",
        )
        .unwrap();
        let delta_bytes = collect_from_dir(&work, cache.path()).unwrap();
        let delta = crate::host::connector::decode_delta(&delta_bytes).unwrap();
        assert!(delta.changed.iter().any(|p| p == "a.txt"));
        // scratch cleaned
        assert!(!std::path::Path::new(&work).exists());
    }

    #[test]
    fn confine_rejects_traversal() {
        let root = tempfile::tempdir().unwrap();
        let escape = root
            .path()
            .join("workspace-sync")
            .join("..")
            .join("..")
            .join("etc");
        assert!(confine(&escape, &root.path().join("workspace-sync")).is_err());
    }

    /// `confine` must reject a `working_dir` that canonicalizes successfully
    /// but sits outside `root` — the real containment branch — not merely a
    /// path whose `root` fails to canonicalize because it doesn't exist yet.
    #[test]
    fn confine_rejects_real_sibling_outside_root() {
        let cache = tempfile::tempdir().unwrap();
        let root = cache.path().join("workspace-sync");
        fs::create_dir_all(&root).unwrap();
        // sibling of `root`, not a descendant: root itself canonicalizes fine,
        // so a failure here can only come from the `starts_with` check.
        assert!(matches!(
            confine(cache.path(), &root),
            Err(HostConnectorError::Invalid(_))
        ));

        let inside = root.join("some-ulid").join("work");
        fs::create_dir_all(&inside).unwrap();
        assert!(confine(&inside, &root).is_ok());
    }

    #[test]
    fn collect_rejects_working_dir_outside_cache() {
        let cache = tempfile::tempdir().unwrap();
        // ensure `<cache>/workspace-sync` exists so `confine`'s `root.canonicalize()`
        // succeeds and the rejection below actually comes from the `starts_with`
        // containment check, not a canonicalize failure on a missing root.
        fs::create_dir_all(cache.path().join("workspace-sync")).unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("work");
        std::fs::create_dir_all(&outside).unwrap();
        let err = collect_from_dir(outside.to_str().unwrap(), cache.path());
        assert!(matches!(err, Err(HostConnectorError::Invalid(_))));
    }

    /// Simulates a launch failure after a real stage: `stage_to_dir` succeeds,
    /// the "agent launch" fails before `collect_from_dir` ever runs, and
    /// `discard_from_dir` is called instead — the scratch must be removed.
    #[test]
    fn discard_removes_scratch_after_simulated_launch_failure() {
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = crate::host::connector::encode_payload(&payload);

        let cache = tempfile::tempdir().unwrap();
        let work = stage_to_dir(&encoded, cache.path()).unwrap();
        let base = std::path::Path::new(&work).parent().unwrap().to_path_buf();
        assert!(base.exists());

        // Simulated launch failure: collect_from_dir is never called; discard
        // instead.
        discard_from_dir(&work, cache.path()).unwrap();

        assert!(!base.exists(), "scratch base dir must be removed");
        assert!(!std::path::Path::new(&work).exists());
    }

    /// A `collect_from_dir` that fails partway through (here: the
    /// `baseline.json` sidecar is missing) must still remove the scratch
    /// `base` dir — otherwise every failed collect leaks a directory under
    /// `<cache_root>/workspace-sync` forever.
    #[test]
    fn collect_removes_scratch_even_on_error() {
        let ws = tempfile::tempdir().unwrap();
        fs::write(ws.path().join("a.txt"), "orig").unwrap();
        let payload = rupu_workspace::pack(ws.path()).unwrap();
        let encoded = crate::host::connector::encode_payload(&payload);

        let cache = tempfile::tempdir().unwrap();
        let work = stage_to_dir(&encoded, cache.path()).unwrap();
        let base = std::path::Path::new(&work).parent().unwrap().to_path_buf();
        assert!(base.exists());

        // Sabotage the baseline sidecar so collect_from_dir errors before it
        // ever gets to rupu_workspace::collect_delta.
        std::fs::remove_file(base.join("baseline.json")).unwrap();

        let err = collect_from_dir(&work, cache.path());
        assert!(matches!(err, Err(HostConnectorError::Invalid(_))));
        assert!(!base.exists(), "scratch base dir must be removed on error");
    }

    #[test]
    fn discard_rejects_working_dir_outside_cache() {
        let cache = tempfile::tempdir().unwrap();
        // same fix as `collect_rejects_working_dir_outside_cache`: the
        // workspace-sync root must exist so the rejection is proven to come
        // from the containment check rather than a canonicalize failure.
        fs::create_dir_all(cache.path().join("workspace-sync")).unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("work");
        std::fs::create_dir_all(&outside).unwrap();
        let err = discard_from_dir(outside.to_str().unwrap(), cache.path());
        assert!(matches!(err, Err(HostConnectorError::Invalid(_))));
    }
}
