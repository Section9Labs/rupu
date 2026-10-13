//! Agent loader. Walks `<global>/agents/*.md` and (if provided)
//! `<project>/agents/*.md`. Project-local agents shadow globals by
//! name (no merging — same `name:` means project replaces global).
//!
//! [`load_agent`] is what a run loads its agent with: it also checks the
//! agent's `tools:` against the tool catalog ([`AgentSpec::validate_tools`])
//! and that every skill its `skills:` / `preloadSkills:` name is installed,
//! so an unknown name fails the launch instead of silently leaving the agent
//! without it. Listings
//! ([`load_agents`], [`find_agent`]) parse only, so one bad file never hides
//! the others; they show the error per agent.
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
    /// The agent's `skills:` / `preloadSkills:` name skills that aren't
    /// installed ([`load_agent`]) or don't resolve ([`check_agent_files`]),
    /// or the skills directories can't be read. `key` is `preloadSkills`
    /// when the failure is that key's alone, else `skills`. `looked_in` is
    /// the layers searched, so an unknown skill's message names where to
    /// install it.
    #[error("agent `{agent}`: {key}: {source}{}", source.install_hint(looked_in))]
    Skill {
        agent: String,
        key: &'static str,
        #[source]
        source: SkillCheckError,
        looked_in: Box<rupu_skills::LayerDirs>,
    },
}

#[derive(Debug, Error)]
pub enum SkillCheckError {
    #[error(transparent)]
    Discover(#[from] rupu_skills::DiscoverError),
    #[error(transparent)]
    Resolve(#[from] rupu_skills::RunSkillsError),
}

impl SkillCheckError {
    /// For a skill that is not installed: where the run looked, and the fix.
    pub fn install_hint(&self, dirs: &rupu_skills::LayerDirs) -> String {
        let SkillCheckError::Resolve(rupu_skills::RunSkillsError::Resolve(errors)) = self else {
            return String::new();
        };
        if !errors
            .iter()
            .any(|e| matches!(e, rupu_skills::ResolveError::Unknown { .. }))
        {
            return String::new();
        }
        let looked: Vec<String> = [&dirs.global, &dirs.project]
            .into_iter()
            .flatten()
            .map(|d| d.display().to_string())
            .collect();
        format!(
            " (looked in {}; install it with `rupu skill install`)",
            looked.join(" and ")
        )
    }
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
/// `tools:` check ([`AgentLoadError::UnknownTool`]) and a check that every
/// skill it names is installed ([`AgentLoadError::Skill`]). The check reads
/// only the skills directories' listings: the run assembler resolves the
/// skills in full. Returns `NotFound` if neither layer has it.
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
    check_skills(&spec, global, project, &mut None, SkillCheck::Names)?;
    Ok(spec)
}

/// How far [`check_skills`] goes.
#[derive(Clone, Copy)]
enum SkillCheck {
    /// Every name is installed: directory listings only.
    Names,
    /// Every skill loads and the set binds each name once.
    Resolve,
}

/// Check an agent's skill keys against the installed skills. The catalog
/// is discovered on first need and reused across calls that share `catalog`.
fn check_skills(
    spec: &AgentSpec,
    global: &Path,
    project: Option<&Path>,
    catalog: &mut Option<rupu_skills::SkillCatalog>,
    check: SkillCheck,
) -> Result<(), AgentLoadError> {
    if !spec.declares_skills() {
        return Ok(());
    }
    let grants = spec.skills.as_deref().unwrap_or_default();
    let preload = spec.preload_skills.as_deref().unwrap_or_default();
    let dirs = rupu_skills::LayerDirs::for_layers(global, project);
    let err = |key, source| AgentLoadError::Skill {
        agent: spec.name.clone(),
        key,
        source,
        looked_in: Box::new(dirs.clone()),
    };
    let catalog = match catalog {
        Some(c) => c,
        None => {
            let found = rupu_skills::SkillCatalog::discover(&dirs).map_err(|e| {
                let key = if grants.is_empty() {
                    "preloadSkills"
                } else {
                    "skills"
                };
                err(key, e.into())
            })?;
            catalog.insert(found)
        }
    };
    let check = |grants: &[String], preload: &[String]| match check {
        SkillCheck::Names => check_names(grants, preload, catalog),
        SkillCheck::Resolve => rupu_skills::resolve_run_skills(
            catalog,
            &[rupu_skills::SkillRequest {
                reason: rupu_skills::SkillReason::Agent,
                grants,
                preload,
            }],
        )
        .map(|_| ()),
    };
    check(grants, preload).map_err(|e| {
        let preload_only =
            check(grants, &[]).is_ok() && check(&[], preload).err().as_ref() == Some(&e);
        let key = if preload_only {
            "preloadSkills"
        } else {
            "skills"
        };
        err(key, e.into())
    })
}

/// Both keys' names, checked against the listing; their errors in key order.
fn check_names(
    grants: &[String],
    preload: &[String],
    catalog: &rupu_skills::SkillCatalog,
) -> Result<(), rupu_skills::RunSkillsError> {
    use rupu_skills::RunSkillsError;
    let mut errors = Vec::new();
    for key in [grants, preload] {
        match rupu_skills::check_grant_names(key, catalog) {
            Ok(()) => {}
            Err(RunSkillsError::Resolve(found)) => {
                for e in found {
                    if !errors.contains(&e) {
                        errors.push(e);
                    }
                }
            }
            Err(other) => return Err(other),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(RunSkillsError::Resolve(errors))
    }
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
/// `<project>/agents/*.md`), shadowed or not: each must parse, its
/// `tools:` must name known tools and its skill keys must resolve. Sorted by
/// layer, then path. A missing `agents/` directory contributes nothing; an
/// unreadable one is an `Io` error on a check for the directory itself.
pub fn check_agent_files(global: &Path, project: Option<&Path>) -> Vec<AgentFileCheck> {
    let mut out = Vec::new();
    let mut catalog = None;
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
                    let err = match spec.validate_tools() {
                        Err(source) => Some(AgentLoadError::UnknownTool {
                            agent: spec.name.clone(),
                            source,
                        }),
                        Ok(()) => {
                            check_skills(&spec, global, project, &mut catalog, SkillCheck::Resolve)
                                .err()
                        }
                    };
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
