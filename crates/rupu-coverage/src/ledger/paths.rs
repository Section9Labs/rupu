use std::path::{Path, PathBuf};

/// Canonical layout of a target's coverage data on disk.
#[derive(Debug, Clone)]
pub struct CoveragePaths {
    /// The workspace the coverage data belongs to (artifact paths and
    /// evidence-claim files resolve against it).
    pub workspace: PathBuf,
    pub root: PathBuf,
    pub files: PathBuf,
    pub concerns: PathBuf,
    pub findings: PathBuf,
    pub catalog: PathBuf,
    pub runs: PathBuf,
    /// Append-only engagement asset graph (one `Asset` per line), folded on
    /// read. Absent for a code/no-engagement run.
    pub assets: PathBuf,
    /// Where this run's coverage is also streamed (`rupu run` only). `None`
    /// for every other writer — the coordinator's own ledgers never stream.
    pub run_stream: Option<crate::ledger::stream::RunStream>,
}

impl CoveragePaths {
    pub fn new(workspace: &Path, target_id: &str) -> Self {
        let root = workspace.join(".rupu").join("coverage").join(target_id);
        Self {
            workspace: workspace.to_path_buf(),
            files: root.join("files.jsonl"),
            concerns: root.join("concerns.jsonl"),
            findings: root.join("findings.jsonl"),
            catalog: root.join("catalog.yaml"),
            runs: root.join("runs.jsonl"),
            assets: root.join("assets.jsonl"),
            run_stream: None,
            root,
        }
    }

    pub fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }

    /// Attach (or clear) the run stream every append through
    /// [`crate::ledger::stream::append_record`] mirrors into.
    pub fn with_run_stream(mut self, stream: Option<crate::ledger::stream::RunStream>) -> Self {
        self.run_stream = stream;
        self
    }

    /// The workspace's tag log, carrying this path's run stream.
    pub fn tag_log(&self) -> crate::ledger::tags::TagLog {
        crate::ledger::tags::TagLog::for_workspace(&self.workspace)
            .with_run_stream(self.run_stream.clone())
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
        assert_eq!(paths.runs, paths.root.join("runs.jsonl"));
        assert_eq!(paths.assets, paths.root.join("assets.jsonl"));
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
