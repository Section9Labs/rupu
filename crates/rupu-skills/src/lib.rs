//! Agent Skills (agentskills.io `SKILL.md` directories): the model, its
//! validation, discovery over rupu's global and project layers, version
//! fingerprints, the grant grammar and the prompt renderers. Spec:
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md`.

pub mod catalog;
pub mod frontmatter;
#[cfg(feature = "git")]
pub mod git;
pub mod grant;
pub mod install;
pub mod name;
pub mod record;
pub mod render;
pub mod run;
pub mod skill;
pub mod source;
pub mod tools_map;
pub mod tree;
pub mod update;

pub use catalog::{CatalogEntry, DiscoverError, LayerDirs, SkillCatalog};
pub use frontmatter::{
    parse_skill_md, Frontmatter, FrontmatterError, MAX_COMPATIBILITY_LEN, MAX_DESCRIPTION_LEN,
};
pub use grant::{
    check_grant_names, parse_grants, resolve_grants, EntryError, GrantParseError, ResolveError,
    SkillGrant,
};
pub use name::{NameError, SkillName, MAX_NAME_LEN};
pub use run::{
    resolve_frozen, resolve_operator, resolve_run_skills, FrozenSkill, GrantedSkill,
    ResolvedSkills, RunSkillsError, SkillReason, SkillRequest,
};
pub use skill::{load_skill, Layer, Skill, SkillError, MAX_SKILL_MD_BYTES};
pub use source::{parse_source, Fetch, FetchedTree, GitSource, Source, SourceError};
pub use tree::{ContentHash, Stamp, TreeEntry, TreeError};
