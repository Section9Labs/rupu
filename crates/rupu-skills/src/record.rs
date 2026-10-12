//! Where an installed skill came from (spec §7.3), kept outside the skill's
//! own directory so it never moves the skill's fingerprints and is never
//! readable through a run's read roots.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    pub version: u32,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<PathBuf>,
    pub content_hash: String,
    pub installed_at: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

pub fn record_path(skills_dir: &Path, name: &str) -> PathBuf {
    skills_dir.join(".sources").join(format!("{name}.json"))
}

pub fn write_record(skills_dir: &Path, name: &str, rec: &SourceRecord) -> std::io::Result<()> {
    let path = record_path(skills_dir, name);
    std::fs::create_dir_all(path.parent().expect("record path has a parent"))?;
    let tmp = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(rec).expect("record serializes"),
    )?;
    std::fs::rename(&tmp, &path)
}

pub fn read_record(skills_dir: &Path, name: &str) -> Result<Option<SourceRecord>, RecordError> {
    let path = record_path(skills_dir, name);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(RecordError::Io { path, source }),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| RecordError::Json { path, source })
}

/// Names of installed skills that have a record. Orphaned records (the
/// skill directory was deleted) are ignored.
pub fn list_records(skills_dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(skills_dir.join(".sources")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .filter(|n| skills_dir.join(n).join("SKILL.md").is_file())
        .collect();
    names.sort();
    names
}

/// Whether a project layer's record may name the local directory `local`.
/// Project records are files a repository can carry: one naming a
/// directory outside the project would let a cloned repository copy any
/// directory on the machine into itself on `update`.
pub(crate) fn inside_project(skills_dir: &Path, local: &Path) -> std::io::Result<bool> {
    let local = local.canonicalize()?;
    let project = skills_dir
        .parent()
        .and_then(Path::parent)
        .and_then(|p| p.canonicalize().ok());
    Ok(project.is_some_and(|p| local.starts_with(p)))
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
