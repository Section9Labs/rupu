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
use std::collections::BTreeSet;
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

type Bound = BTreeSet<(String, Layer)>;

fn bind(entries: &[String], catalog: &SkillCatalog) -> Result<Bound, RunSkillsError> {
    if entries.is_empty() {
        return Ok(Bound::new());
    }
    let skills =
        resolve_grants(&parse_grants(entries)?, catalog).map_err(RunSkillsError::Resolve)?;
    Ok(skills
        .iter()
        .map(|s| (s.name.as_str().to_owned(), s.layer))
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
    if all.is_empty() {
        return Ok(ResolvedSkills::default());
    }
    let skills = resolve_grants(&parse_grants(&all)?, catalog).map_err(RunSkillsError::Resolve)?;
    let per_request = requests
        .iter()
        .map(|r| {
            Ok((
                r.reason,
                bind(r.grants, catalog)?,
                bind(r.preload, catalog)?,
            ))
        })
        .collect::<Result<Vec<_>, RunSkillsError>>()?;
    let entries = skills
        .into_iter()
        .map(|skill| {
            let key = (skill.name.as_str().to_owned(), skill.layer);
            let preload = per_request.iter().any(|(_, _, p)| p.contains(&key));
            let reason = per_request
                .iter()
                .find(|(_, g, p)| g.contains(&key) || p.contains(&key))
                .map_or(SkillReason::Agent, |(reason, ..)| *reason);
            let indexed = !preload && !skill.frontmatter.disable_model_invocation;
            GrantedSkill {
                skill,
                preload,
                indexed,
                reason,
            }
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
