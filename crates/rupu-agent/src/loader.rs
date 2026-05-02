//! Agent loader. Walks `<global>/agents/*.md` and (if provided)
//! `<project>/agents/*.md`. Project-local agents shadow globals by
//! name (no merging — same `name:` means project replaces global).

use crate::spec::{AgentSpec, AgentSpecParseError};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// Errors that can occur while loading agents from disk.
#[derive(Debug, Error)]
pub enum AgentLoadError {
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("io reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: AgentSpecParseError,
    },
}

/// Load every agent under `<global>/agents/*.md` and (if `project` is
/// `Some`) `<project>/agents/*.md`. Project entries shadow globals by
/// name. Missing `agents/` dir at either layer is OK (returns those
/// entries that do exist).
pub fn load_agents(
    global: &Path,
    project: Option<&Path>,
) -> Result<Vec<AgentSpec>, AgentLoadError> {
    let mut by_name: BTreeMap<String, AgentSpec> = BTreeMap::new();
    load_dir_into(&global.join("agents"), &mut by_name)?;
    if let Some(p) = project {
        load_dir_into(&p.join("agents"), &mut by_name)?;
    }
    Ok(by_name.into_values().collect())
}

fn load_dir_into(dir: &Path, into: &mut BTreeMap<String, AgentSpec>) -> Result<(), AgentLoadError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(|e| AgentLoadError::Io {
        path: dir.display().to_string(),
        source: e,
    })? {
        let entry = entry.map_err(|e| AgentLoadError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let spec = AgentSpec::parse_file(&path).map_err(|source| AgentLoadError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        into.insert(spec.name.clone(), spec);
    }
    Ok(())
}

/// Look up a single agent by name. Returns `NotFound` if neither
/// layer has it.
pub fn load_agent(
    global: &Path,
    project: Option<&Path>,
    name: &str,
) -> Result<AgentSpec, AgentLoadError> {
    let agents = load_agents(global, project)?;
    agents
        .into_iter()
        .find(|a| a.name == name)
        .ok_or_else(|| AgentLoadError::NotFound(name.to_string()))
}
