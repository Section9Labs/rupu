use crate::catalog::types::ConcernsBlock;
use crate::ledger::events::Surface;
use crate::ledger::paths::CoveragePaths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// The defining inputs of a coverage run, captured at run start so the run
/// can be described and (for agent runs) replayed. Appended one-per-run to
/// `runs.jsonl` alongside the three event ledgers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunManifest {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub surface: Surface,
    pub agent_name: String,
    pub provider: String,
    pub model: String,
    pub permission_mode: String,
    pub user_prompt: String,
    /// The resolved concerns block the run was configured with (record of
    /// the catalog at run time).
    pub concerns: ConcernsBlock,
    /// The scope name used to derive this run's `target_id`
    /// (agent name for agent runs, session id for session runs, etc.).
    pub scope_name: String,
    pub workspace_path: std::path::PathBuf,
}

/// Append a manifest row to `runs.jsonl` (creates the file if absent).
pub fn append_manifest(paths: &CoveragePaths, manifest: &RunManifest) -> std::io::Result<()> {
    if let Some(parent) = paths.runs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.runs)?;
    let line = serde_json::to_string(manifest)?;
    writeln!(file, "{line}")
}

/// Read all manifests from `runs.jsonl` (empty vec if the file is absent).
/// Malformed lines are skipped, matching the other ledger readers.
pub fn read_manifests(paths: &CoveragePaths) -> std::io::Result<Vec<RunManifest>> {
    if !paths.runs.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&paths.runs)?;
    Ok(raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<RunManifest>(l).ok())
        .collect())
}

/// Find the manifest for a specific run id, if present.
pub fn find_manifest(paths: &CoveragePaths, run_id: &str) -> std::io::Result<Option<RunManifest>> {
    Ok(read_manifests(paths)?
        .into_iter()
        .find(|m| m.run_id == run_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::types::{ConcernsEntry, IncludeDirective};

    fn sample(run_id: &str) -> RunManifest {
        RunManifest {
            run_id: run_id.to_string(),
            started_at: DateTime::<Utc>::from_timestamp(1000, 0).unwrap(),
            surface: Surface::Agent,
            agent_name: "reviewer".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            permission_mode: "bypass".to_string(),
            user_prompt: "Review for security issues.".to_string(),
            concerns: ConcernsBlock {
                entries: vec![ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: crate::catalog::types::CatalogMode::Auto,
                    filter: None,
                })],
            },
            scope_name: "reviewer".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp/repo"),
        }
    }

    #[test]
    fn append_then_read_round_trips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        append_manifest(&paths, &sample("run_a")).unwrap();
        append_manifest(&paths, &sample("run_b")).unwrap();
        let all = read_manifests(&paths).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].run_id, "run_a");
        assert_eq!(all[1].run_id, "run_b");
        assert_eq!(all[0], sample("run_a"));
    }

    #[test]
    fn find_manifest_by_run_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        append_manifest(&paths, &sample("run_a")).unwrap();
        append_manifest(&paths, &sample("run_b")).unwrap();
        assert_eq!(
            find_manifest(&paths, "run_b").unwrap().unwrap().run_id,
            "run_b"
        );
        assert!(find_manifest(&paths, "nope").unwrap().is_none());
    }

    #[test]
    fn read_absent_file_is_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        assert!(read_manifests(&paths).unwrap().is_empty());
    }
}
