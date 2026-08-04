use std::path::Path;

/// A coverage target found under `.rupu/coverage/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTarget {
    pub target_id: String,
    /// Number of concern assertions on disk (cheap signal of activity).
    pub assertion_lines: usize,
    pub has_catalog: bool,
}

/// List all coverage targets under `<workspace>/.rupu/coverage/`.
/// Returns an empty vec if the directory doesn't exist.
pub fn discover_targets(workspace: &Path) -> std::io::Result<Vec<DiscoveredTarget>> {
    let root = workspace.join(".rupu").join("coverage");
    if !root.is_dir() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let target_id = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        let assertion_lines = std::fs::read_to_string(dir.join("concerns.jsonl"))
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);
        let has_catalog = dir.join("catalog.yaml").exists();
        out.push(DiscoveredTarget { target_id, assertion_lines, has_catalog });
    }
    out.sort_by(|a, b| a.target_id.cmp(&b.target_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::paths::CoveragePaths;

    #[test]
    fn discover_empty_when_no_coverage_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(discover_targets(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn discover_lists_target_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "abc123");
        paths.ensure_dir().unwrap();
        std::fs::write(&paths.concerns, "{}\n{}\n").unwrap();
        std::fs::write(&paths.catalog, "name: x\n").unwrap();
        let targets = discover_targets(tmp.path()).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target_id, "abc123");
        assert_eq!(targets[0].assertion_lines, 2);
        assert!(targets[0].has_catalog);
    }
}
