//! Shared filesystem-safety helpers for the definition-editing endpoints
//! (agents `.md`, workflows `.yaml`). Both reuse the same name validation and
//! atomic-write primitives so a bad name can never escape the target directory
//! and a crashed write never leaves a corrupt definition.

use crate::error::ApiError;
use std::path::Path as FsPath;

/// Reject anything but a bare file stem: must start with an ASCII letter and
/// contain only `[A-Za-z0-9_-]`. Blocks `/`, `.`, `..`, spaces, and the empty
/// string so the name can never escape the target directory.
pub(crate) fn validate_name(name: &str) -> Result<(), ApiError> {
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if first_ok && rest_ok {
        Ok(())
    } else {
        Err(ApiError::bad_request("invalid name"))
    }
}

/// Write `bytes` to a sibling temp file then atomically rename it over `path`,
/// so a crashed/partial write never leaves a corrupt definition on disk.
pub(crate) fn write_atomic(path: &FsPath, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Confirm `path` (which must already exist) actually lives under `dir`
/// (which must already exist), both canonicalized so a symlink can't fake
/// containment.
///
/// This is defense in depth, not the primary guard, for the definition
/// delete endpoints (`delete_agent` / `delete_workflow`): the resolver that
/// produces `path` only ever joins a [`validate_name`]-checked identifier
/// (no `/`, `.`, or `..`) onto a trusted directory, so `path` can never
/// actually escape `dir` today. This check exists to catch a *future*
/// resolver bug — a bad join, a wrong directory passed in — before it can
/// ever delete a file outside the layer the caller believes it's operating
/// on, mirroring `api::config::project_config_path`'s `starts_with`
/// confinement guard for config writes.
pub(crate) fn validate_within(path: &FsPath, dir: &FsPath) -> Result<(), ApiError> {
    let dir_canon = dir
        .canonicalize()
        .map_err(|e| ApiError::internal(format!("could not resolve directory: {e}")))?;
    let path_canon = path
        .canonicalize()
        .map_err(|e| ApiError::internal(format!("could not resolve path: {e}")))?;
    if path_canon.starts_with(&dir_canon) {
        Ok(())
    } else {
        Err(ApiError::bad_request("path escapes expected directory"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_rejects_traversal_and_accepts_plain() {
        for bad in ["../evil", "a/b", ".", "", "..", " spaces", "1leading"] {
            assert!(validate_name(bad).is_err(), "should reject {bad:?}");
        }
        assert!(validate_name("code-reviewer").is_ok());
        assert!(validate_name("Agent_1").is_ok());
    }

    #[test]
    fn write_atomic_writes_exact_bytes_no_temp_left() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("demo.yaml");
        write_atomic(&path, b"hello").expect("write ok");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn validate_within_accepts_contained_rejects_escaped() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("agents");
        std::fs::create_dir_all(&dir).unwrap();
        let inside = dir.join("a.md");
        std::fs::write(&inside, "x").unwrap();
        assert!(validate_within(&inside, &dir).is_ok());

        let outside_dir = tmp.path().join("other");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let outside = outside_dir.join("b.md");
        std::fs::write(&outside, "x").unwrap();
        assert!(
            validate_within(&outside, &dir).is_err(),
            "a path outside `dir` must be rejected even though both exist"
        );
    }
}
