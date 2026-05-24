use std::path::{Path, PathBuf};

/// Canonical layout of a target's coverage data on disk.
#[derive(Debug, Clone)]
pub struct CoveragePaths {
    pub root: PathBuf,
    pub files: PathBuf,
    pub concerns: PathBuf,
    pub findings: PathBuf,
    pub catalog: PathBuf,
}

impl CoveragePaths {
    pub fn new(workspace: &Path, target_id: &str) -> Self {
        let root = workspace.join(".rupu").join("coverage").join(target_id);
        Self {
            files: root.join("files.jsonl"),
            concerns: root.join("concerns.jsonl"),
            findings: root.join("findings.jsonl"),
            catalog: root.join("catalog.yaml"),
            root,
        }
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_layout_under_dotrupu_coverage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "abc123");
        assert_eq!(paths.root, tmp.path().join(".rupu/coverage/abc123"));
        assert_eq!(paths.files, paths.root.join("files.jsonl"));
        assert_eq!(paths.concerns, paths.root.join("concerns.jsonl"));
        assert_eq!(paths.findings, paths.root.join("findings.jsonl"));
        assert_eq!(paths.catalog, paths.root.join("catalog.yaml"));
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "abc");
        paths.ensure_dir().unwrap();
        paths.ensure_dir().unwrap(); // second call must not fail
        assert!(paths.root.is_dir());
    }
}
