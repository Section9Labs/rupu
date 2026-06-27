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
}
