use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StandaloneRunMetadataError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandaloneRunMetadata {
    pub version: u32,
    pub run_id: String,
    pub workspace_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub project_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub issue_ref: Option<String>,
    pub backend_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_id: Option<String>,
    pub trigger_source: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace_strategy: Option<String>,
}

impl StandaloneRunMetadata {
    pub const VERSION: u32 = 1;
}

pub fn metadata_path_for_run(transcripts_dir: &Path, run_id: &str) -> PathBuf {
    transcripts_dir.join(format!("{run_id}.meta.json"))
}

pub fn write_metadata(
    path: &Path,
    metadata: &StandaloneRunMetadata,
) -> Result<(), StandaloneRunMetadataError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = serde_json::to_vec_pretty(metadata)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, payload)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn read_metadata(path: &Path) -> Result<StandaloneRunMetadata, StandaloneRunMetadataError> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = metadata_path_for_run(dir.path(), "run_01JXYZ");
        let metadata = StandaloneRunMetadata {
            version: StandaloneRunMetadata::VERSION,
            run_id: "run_01JXYZ".into(),
            workspace_path: PathBuf::from("/tmp/repo"),
            project_root: Some(PathBuf::from("/tmp/project")),
            repo_ref: Some("github:Section9Labs/rupu".into()),
            issue_ref: Some("github:Section9Labs/rupu/issues/42".into()),
            backend_id: "local_checkout".into(),
            worker_id: Some("worker_local_cli".into()),
            trigger_source: "run_cli".into(),
            target: Some("github:Section9Labs/rupu/issues/42".into()),
            workspace_strategy: Some("direct_checkout".into()),
        };

        write_metadata(&path, &metadata).unwrap();
        let loaded = read_metadata(&path).unwrap();
        assert_eq!(loaded, metadata);
    }
}
