//! The skills one run is given (spec §2.4). Every request a run carries
//! (the agent's keys, a workflow step's, a session's, an operator's) is
//! resolved in ONE `resolve_grants` call over their union, so a name bound
//! from both layers across requests is caught; preload is then marked by
//! membership and the reason is the first request that names the skill.

use crate::catalog::SkillCatalog;
use crate::grant::{
    parse_grants, resolve_grants, EntryError, GrantParseError, ResolveError, SkillGrant,
};
use crate::render::{render_skills_section, IndexItem, PreloadItem};
use crate::skill::{Layer, Skill, SkillError};
use crate::tree::{ContentHash, Stamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillReason {
    Agent,
    /// Given by the launch itself: a workflow step's `skills:`, or
    /// `rupu run --skill` (an operator's, or a placed unit's carrying its
    /// step's).
    Launch,
    Session,
    Operator,
}

impl SkillReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkillReason::Agent => "agent",
            SkillReason::Launch => "launch",
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
    /// What a change on disk is judged against: the stamp the run's index
    /// recorded, which for a session turn is the one its first turn did.
    pub baseline_stamp: Stamp,
    /// The version of the body the system prompt carries: `Some` exactly
    /// for a preloaded skill, hashed when the run resolved it.
    pub content_hash: Option<ContentHash>,
}

impl GrantedSkill {
    pub fn name(&self) -> &str {
        self.skill.name.as_str()
    }

    /// Whether `skills.load` may hand this skill to the model (spec §3.2):
    /// a `disable-model-invocation` skill is reachable only by preload or an
    /// operator's invocation.
    pub fn model_loadable(&self) -> bool {
        self.indexed || self.preload || self.reason == SkillReason::Operator
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
    /// An operator invokes one skill; `*` and `project:*` name a set.
    #[error("skill entry {0:?} is a wildcard; invoke one skill by name")]
    Wildcard(String),
    /// A preloaded skill's files could not be read to version its body.
    #[error("skill {name:?} ({}) can't be read: {error}", layer.as_str())]
    Hash {
        name: String,
        layer: Layer,
        error: EntryError,
    },
}

impl RunSkillsError {
    /// The I/O errors behind the skills that failed to load.
    pub fn io_errors(&self) -> impl Iterator<Item = &std::io::Error> {
        let (resolve, hash) = match self {
            RunSkillsError::Resolve(errors) => (errors.as_slice(), None),
            RunSkillsError::Parse(_) | RunSkillsError::Wildcard(_) => (&[][..], None),
            RunSkillsError::Hash { error, .. } => (&[][..], Some(error)),
        };
        resolve
            .iter()
            .filter_map(|e| match e {
                ResolveError::Invalid { error, .. } => Some(error),
                _ => None,
            })
            .chain(hash)
            .filter_map(|e| e.0.io_error())
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
                baseline_stamp: skill.stamp,
                skill,
                preload: false,
                indexed: false,
                reason: r.reason,
                content_hash: None,
            });
            g.preload |= preload;
        }
    }
    let entries = granted
        .into_values()
        .map(|mut g| {
            g.indexed = !g.preload && !g.skill.frontmatter.disable_model_invocation;
            if g.preload {
                let hash = g.skill.content_hash().map_err(|e| RunSkillsError::Hash {
                    name: g.name().to_owned(),
                    layer: g.skill.layer,
                    error: EntryError(Arc::new(SkillError::Tree(e))),
                })?;
                g.content_hash = Some(hash);
            }
            Ok(g)
        })
        .collect::<Result<_, RunSkillsError>>()?;
    Ok(ResolvedSkills { entries })
}

/// One entry of a resolved set as a session stores it (spec §5.1): exactly
/// one skill in one layer, with the flags and versions its first turn had,
/// so a later turn rebuilds the same set without re-expanding `*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenSkill {
    pub name: String,
    pub layer: Layer,
    pub preload: bool,
    pub indexed: bool,
    pub reason: SkillReason,
    /// Hex; the baseline a later change is judged against.
    pub stamp: String,
    /// Hex; `Some` exactly for a preload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

/// A frozen entry a turn can't be given, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSkill {
    pub name: String,
    pub layer: Layer,
    pub reason: MissingReason,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MissingReason {
    #[error("removed")]
    Removed,
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("corrupt record: bad stamp")]
    BadStamp,
    #[error("corrupt record: bad hash")]
    BadHash,
    #[error("corrupt record: preload and content hash disagree")]
    PreloadHashMismatch,
    /// The set already holds the skill of that name from `held`; a run
    /// never binds one name from both layers.
    #[error("the set already holds the {} skill of that name", held.as_str())]
    Conflict { held: Layer },
}

/// The frozen entry's skill as installed now, or why it can't be given.
fn thaw(catalog: &SkillCatalog, f: &FrozenSkill) -> Result<GrantedSkill, MissingReason> {
    let entry = catalog
        .get(f.layer, &f.name)
        .ok_or(MissingReason::Removed)?;
    let skill = entry
        .skill()
        .map_err(|e| MissingReason::Invalid(e.to_string()))?
        .clone();
    let baseline_stamp = Stamp::from_hex(&f.stamp).ok_or(MissingReason::BadStamp)?;
    let content_hash = match &f.content_hash {
        Some(h) => Some(ContentHash::from_hex(h).ok_or(MissingReason::BadHash)?),
        None => None,
    };
    if f.preload != content_hash.is_some() {
        return Err(MissingReason::PreloadHashMismatch);
    }
    Ok(GrantedSkill {
        skill,
        preload: f.preload,
        indexed: f.indexed,
        reason: f.reason,
        baseline_stamp,
        content_hash,
    })
}

/// Rebuilds a frozen set from the skills installed now. Returns the set and
/// the entries that can no longer be given: the caller reports them and
/// runs without them.
pub fn resolve_frozen(
    catalog: &SkillCatalog,
    frozen: &[FrozenSkill],
) -> (ResolvedSkills, Vec<MissingSkill>) {
    ResolvedSkills::default().with_frozen(catalog, frozen)
}

/// An operator's invocation of one installed skill (spec §5.2): any valid
/// skill in either layer, granted or not. The entry takes the grant
/// grammar's names and prefixes; a wildcard, an unknown name, a bare name
/// in both layers, or a skill whose name `held` (the session's set) binds
/// from the other layer is refused.
pub fn resolve_operator(
    catalog: &SkillCatalog,
    held: &[FrozenSkill],
    entry: &str,
) -> Result<FrozenSkill, RunSkillsError> {
    let grant: SkillGrant = entry.parse()?;
    if matches!(grant, SkillGrant::AllGlobal | SkillGrant::AllProject) {
        return Err(RunSkillsError::Wildcard(entry.to_owned()));
    }
    let skills = resolve_grants(&[grant], catalog).map_err(RunSkillsError::Resolve)?;
    let skill = skills.first().ok_or_else(|| {
        RunSkillsError::Resolve(vec![ResolveError::Unknown {
            entry: entry.to_owned(),
            suggestion: None,
        }])
    })?;
    let name = skill.name.as_str();
    if held
        .iter()
        .any(|h| h.name == name && h.layer != skill.layer)
    {
        return Err(RunSkillsError::Resolve(vec![ResolveError::BothLayers {
            name: name.to_owned(),
        }]));
    }
    Ok(FrozenSkill {
        name: name.to_owned(),
        layer: skill.layer,
        preload: false,
        indexed: false,
        reason: SkillReason::Operator,
        stamp: skill.stamp.hex(),
        content_hash: None,
    })
}

impl ResolvedSkills {
    /// The set as a session stores it.
    pub fn freeze(&self) -> Vec<FrozenSkill> {
        self.entries
            .iter()
            .map(|g| FrozenSkill {
                name: g.name().to_owned(),
                layer: g.skill.layer,
                preload: g.preload,
                indexed: g.indexed,
                reason: g.reason,
                stamp: g.baseline_stamp.hex(),
                content_hash: g.content_hash.map(|h| h.hex()),
            })
            .collect()
    }

    /// Appends frozen entries (an operator's invocations) the set lacks; an
    /// entry the set holds already, in the same layer, is skipped. Returns
    /// the set and the entries that can't be given, as `resolve_frozen`
    /// does: one whose name the set binds from the other layer is a
    /// [`MissingReason::Conflict`].
    pub fn with_frozen(
        mut self,
        catalog: &SkillCatalog,
        extra: &[FrozenSkill],
    ) -> (Self, Vec<MissingSkill>) {
        let mut missing = Vec::new();
        for f in extra {
            let thawed = match self.get(&f.name) {
                Some(g) if g.skill.layer == f.layer => continue,
                Some(g) => Err(MissingReason::Conflict {
                    held: g.skill.layer,
                }),
                None => thaw(catalog, f),
            };
            match thawed {
                Ok(g) => self.entries.push(g),
                Err(reason) => missing.push(MissingSkill {
                    name: f.name.clone(),
                    layer: f.layer,
                    reason,
                }),
            }
        }
        (self, missing)
    }

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

    /// Whether the model may load any of them: what offers `skills.load`.
    pub fn has_loadable(&self) -> bool {
        self.entries.iter().any(GrantedSkill::model_loadable)
    }

    /// The granted skills the model may load, by name.
    pub fn loadable_names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|g| g.model_loadable())
            .map(GrantedSkill::name)
            .collect()
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
