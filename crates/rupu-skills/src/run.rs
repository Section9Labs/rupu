//! The skills one run is given (spec §2.4). Every request a run carries
//! (the agent's keys, a workflow step's, a session's, an operator's) is
//! resolved in ONE `resolve_grants` call over their union, so a name bound
//! from both layers across requests is caught; preload is then marked by
//! membership and the reason is the first request that names the skill.

use crate::catalog::SkillCatalog;
use crate::grant::{parse_grants, resolve_grants, GrantParseError, ResolveError};
use crate::render::{render_skills_section, IndexItem, PreloadItem};
use crate::skill::{Layer, Skill};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillReason {
    Agent,
    Step,
    Session,
    Operator,
}

impl SkillReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillReason::Agent => "agent",
            SkillReason::Step => "step",
            SkillReason::Session => "session",
            SkillReason::Operator => "operator",
        }
    }
}

pub struct SkillRequest<'a> {
    pub reason: SkillReason,
    pub grants: &'a [String],
    pub preload: &'a [String],
}

#[derive(Debug, Clone)]
pub struct GrantedSkill {
    pub skill: Arc<Skill>,
    pub preload: bool,
    pub indexed: bool,
    pub reason: SkillReason,
}

impl GrantedSkill {
    pub fn name(&self) -> &str {
        self.skill.name.as_str()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolvedSkills {
    entries: Vec<GrantedSkill>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunSkillsError {
    #[error(transparent)]
    Parse(#[from] GrantParseError),
    #[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Resolve(Vec<ResolveError>),
}

impl RunSkillsError {
    /// The I/O errors behind the skills that failed to load.
    pub fn io_errors(&self) -> impl Iterator<Item = &std::io::Error> {
        let resolve = match self {
            RunSkillsError::Resolve(errors) => errors.as_slice(),
            RunSkillsError::Parse(_) => &[],
        };
        resolve.iter().filter_map(|e| match e {
            ResolveError::Invalid { error, .. } => error.0.io_error(),
            _ => None,
        })
    }
}

type Bound = BTreeMap<(String, Layer), Arc<Skill>>;

fn bind(entries: &[String], catalog: &SkillCatalog) -> Result<Bound, RunSkillsError> {
    if entries.is_empty() {
        return Ok(Bound::new());
    }
    let skills =
        resolve_grants(&parse_grants(entries)?, catalog).map_err(RunSkillsError::Resolve)?;
    Ok(skills
        .into_iter()
        .map(|s| ((s.name.as_str().to_owned(), s.layer), s))
        .collect())
}

pub fn resolve_run_skills(
    catalog: &SkillCatalog,
    requests: &[SkillRequest<'_>],
) -> Result<ResolvedSkills, RunSkillsError> {
    let all: Vec<String> = requests
        .iter()
        .flat_map(|r| r.grants.iter().chain(r.preload.iter()))
        .cloned()
        .collect();
    bind(&all, catalog)?;
    let mut granted: BTreeMap<(String, Layer), GrantedSkill> = BTreeMap::new();
    for r in requests {
        let grants = bind(r.grants, catalog)?.into_iter().map(|b| (b, false));
        let preloads = bind(r.preload, catalog)?.into_iter().map(|b| (b, true));
        for ((key, skill), preload) in grants.chain(preloads) {
            let g = granted.entry(key).or_insert_with(|| GrantedSkill {
                skill,
                preload: false,
                indexed: false,
                reason: r.reason,
            });
            g.preload |= preload;
        }
    }
    let entries = granted
        .into_values()
        .map(|mut g| {
            g.indexed = !g.preload && !g.skill.frontmatter.disable_model_invocation;
            g
        })
        .collect();
    Ok(ResolvedSkills { entries })
}

impl ResolvedSkills {
    pub fn entries(&self) -> &[GrantedSkill] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&GrantedSkill> {
        self.entries.iter().find(|g| g.name() == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(GrantedSkill::name).collect()
    }

    /// Every granted skill's canonical root: the directories the fs read
    /// tools may read outside the workspace (spec §3.4).
    pub fn read_roots(&self) -> Vec<PathBuf> {
        self.entries.iter().map(|g| g.skill.root.clone()).collect()
    }

    /// The system prompt's skills section: preloaded bodies, then the index.
    pub fn prompt_section(&self) -> Option<String> {
        let preloads: Vec<PreloadItem<'_>> = self
            .entries
            .iter()
            .filter(|g| g.preload)
            .map(|g| PreloadItem {
                name: g.name(),
                base_dir: &g.skill.root,
                body: &g.skill.body,
            })
            .collect();
        let index: Vec<IndexItem<'_>> = self
            .entries
            .iter()
            .filter(|g| g.indexed)
            .map(|g| IndexItem {
                name: g.name(),
                description: &g.skill.frontmatter.description,
            })
            .collect();
        render_skills_section(&preloads, &index)
    }
}
