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
pub(crate) fn stage_to_dir(
    payload: &[u8],
    cache_root: &Path,
) -> Result<String, HostConnectorError> {
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
pub(crate) fn collect_from_dir(
    working_dir: &str,
    cache_root: &Path,
) -> Result<Vec<u8>, HostConnectorError> {
    let sync_root = cache_root.join("workspace-sync");
    let work = confine(Path::new(working_dir), &sync_root)?;
    let base = work
        .parent()
        .ok_or_else(|| HostConnectorError::Invalid("invalid working dir".into()))?;
    let baseline_bytes = std::fs::read(base.join("baseline.json"))
        .map_err(|e| HostConnectorError::Invalid(format!("baseline missing: {e}")))?;
    let baseline = deserialize_baseline(&baseline_bytes)?;
    let delta = rupu_workspace::collect_delta(&work, &baseline)
        .map_err(|e| HostConnectorError::Invalid(e.to_string()))?;
    let bytes = encode_delta(&delta);
    let _ = std::fs::remove_dir_all(base);
    Ok(bytes)
}

/// Best-effort discard of a staged workspace scratch dir. Used when the unit
/// that consumed the staged tree failed *between* stage and collect (launch
/// failure, poll timeout) so `collect_from_dir` never ran and the scratch
/// would otherwise leak forever. `working_dir` is confined under
/// `<cache_root>/workspace-sync`, same as [`collect_from_dir`].
pub(crate) fn discard_from_dir(
    working_dir: &str,
    cache_root: &Path,
) -> Result<(), HostConnectorError> {
    let sync_root = cache_root.join("workspace-sync");
    let work = confine(Path::new(working_dir), &sync_root)?;
    let base = work
        .parent()
        .ok_or_else(|| HostConnectorError::Invalid("invalid working dir".into()))?;
    let _ = std::fs::remove_dir_all(base);
    Ok(())
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

    #[test]
    fn collect_rejects_working_dir_outside_cache() {
        let cache = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("work");
        std::fs::create_dir_all(&outside).unwrap();
        let err = collect_from_dir(outside.to_str().unwrap(), cache.path());
        assert!(err.is_err());
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

    #[test]
    fn discard_rejects_working_dir_outside_cache() {
        let cache = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("work");
        std::fs::create_dir_all(&outside).unwrap();
        let err = discard_from_dir(outside.to_str().unwrap(), cache.path());
        assert!(err.is_err());
    }
}
