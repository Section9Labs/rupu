use sha2::{Digest, Sha256};
use std::path::Path;

/// Stable identifier for a coverage target.
///
/// Inputs: the workspace path (canonicalized when possible) and a
/// scope_name. The scope_name is the workflow name, agent name, or
/// session_id depending on which surface initiated the run.
///
/// Returns a 16-character lowercase hex prefix of the SHA-256 hash —
/// short enough for human-readable directory names, long enough to
/// avoid collisions in practice.
pub fn target_id(workspace: &Path, scope_name: &str) -> String {
    let canonical = workspace.canonicalize().unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.display().to_string().as_bytes());
    hasher.update(b"::");
    hasher.update(scope_name.as_bytes());
    let digest = hasher.finalize();
    hex_short(&digest)
}

fn hex_short(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in &bytes[..8] {
        use std::fmt::Write;
        write!(&mut out, "{byte:02x}").unwrap();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_id_is_deterministic() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = target_id(tmp.path(), "security-review");
        let b = target_id(tmp.path(), "security-review");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn target_id_differs_for_different_scopes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let a = target_id(tmp.path(), "security-review");
        let b = target_id(tmp.path(), "perf-review");
        assert_ne!(a, b);
    }

    #[test]
    fn target_id_differs_for_different_workspaces() {
        let tmp1 = tempfile::TempDir::new().unwrap();
        let tmp2 = tempfile::TempDir::new().unwrap();
        let a = target_id(tmp1.path(), "x");
        let b = target_id(tmp2.path(), "x");
        assert_ne!(a, b);
    }
}
