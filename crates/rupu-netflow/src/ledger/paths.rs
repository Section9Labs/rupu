use std::path::{Path, PathBuf};

/// Canonical on-disk layout of a workspace's netflow ledger.
#[derive(Debug, Clone)]
pub struct NetflowPaths {
    pub root: PathBuf,
    pub flows: PathBuf,
}

impl NetflowPaths {
    pub fn new(workspace: &Path) -> Self {
        let root = workspace.join(".rupu").join("netflow");
        Self {
            flows: root.join("flows.jsonl"),
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
    fn paths_layout_under_dotrupu_netflow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        assert_eq!(paths.root, tmp.path().join(".rupu/netflow"));
        assert_eq!(paths.flows, paths.root.join("flows.jsonl"));
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = NetflowPaths::new(tmp.path());
        paths.ensure_dir().unwrap();
        paths.ensure_dir().unwrap();
        assert!(paths.root.is_dir());
    }
}
