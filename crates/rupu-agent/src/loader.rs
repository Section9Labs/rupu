//! Agent loader. Walks `<global>/agents/*.md` and (if provided)
//! `<project>/agents/*.md`. Project-local agents shadow globals by
//! name (no merging — same `name:` means project replaces global).
//!
//! [`load_agent`] is what a run loads its agent with: it also checks the
//! agent's `tools:` against the tool catalog ([`AgentSpec::validate_tools`]),
//! so an unknown tool name fails the launch instead of silently leaving the
//! agent without it. Listings ([`load_agents`], [`find_agent`]) parse only,
//! so one bad file never hides the others; they show the error per agent.
//! [`check_agent_files`] checks every file in both layers
//! (`rupu agent validate`).

use crate::spec::{AgentSpec, AgentSpecParseError};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    /// The agent's `tools:` names a tool no catalog tool answers to.
    #[error("agent `{agent}`: {source}")]
    UnknownTool {
        agent: String,
        #[source]
        source: rupu_tools::GrantError,
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

/// Look up a single agent by name, to run it: [`find_agent`], plus the
/// `tools:` check ([`AgentLoadError::UnknownTool`]). Returns `NotFound` if
/// neither layer has it.
pub fn load_agent(
    global: &Path,
    project: Option<&Path>,
    name: &str,
) -> Result<AgentSpec, AgentLoadError> {
    let spec = find_agent(global, project, name)?;
    spec.validate_tools()
        .map_err(|source| AgentLoadError::UnknownTool {
            agent: spec.name.clone(),
            source,
        })?;
    Ok(spec)
}

/// Look up a single agent by name without checking its `tools:` — for
/// showing an agent (and its errors), never for running one.
pub fn find_agent(
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

/// Which layer an agent file was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLayer {
    Global,
    Project,
}

/// The result of checking one agent file ([`check_agent_files`]).
#[derive(Debug)]
pub struct AgentFileCheck {
    pub path: PathBuf,
    pub layer: AgentLayer,
    /// The agent's frontmatter `name`, when the file parsed.
    pub name: Option<String>,
    /// Why the file can't be loaded to run, if it can't.
    pub error: Option<AgentLoadError>,
}

/// Check every agent file in both layers (`<global>/agents/*.md`, then
/// `<project>/agents/*.md`), shadowed or not: each must parse and its
/// `tools:` must name known tools. Sorted by layer, then path. A missing
/// `agents/` directory contributes nothing; an unreadable one is an `Io`
/// error on a check for the directory itself.
pub fn check_agent_files(global: &Path, project: Option<&Path>) -> Vec<AgentFileCheck> {
    let mut out = Vec::new();
    let layers = std::iter::once((global, AgentLayer::Global))
        .chain(project.map(|p| (p, AgentLayer::Project)));
    for (root, layer) in layers {
        let dir = root.join("agents");
        if !dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(source) => {
                out.push(AgentFileCheck {
                    path: dir.clone(),
                    layer,
                    name: None,
                    error: Some(AgentLoadError::Io {
                        path: dir.display().to_string(),
                        source,
                    }),
                });
                continue;
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md"))
            .collect();
        paths.sort();
        for path in paths {
            let (name, error) = match AgentSpec::parse_file(&path) {
                Ok(spec) => {
                    let err =
                        spec.validate_tools()
                            .err()
                            .map(|source| AgentLoadError::UnknownTool {
                                agent: spec.name.clone(),
                                source,
                            });
                    (Some(spec.name), err)
                }
                Err(source) => (
                    None,
                    Some(AgentLoadError::Parse {
                        path: path.display().to_string(),
                        source,
                    }),
                ),
            };
            out.push(AgentFileCheck {
                path,
                layer,
                name,
                error,
            });
        }
    }
    out
}
