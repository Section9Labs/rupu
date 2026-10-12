//! Agent Skills (agentskills.io `SKILL.md` directories): the model, its
//! validation, discovery over rupu's global and project layers, version
//! fingerprints, the grant grammar and the prompt renderers. Spec:
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md`.

pub mod catalog;
pub mod frontmatter;
pub mod grant;
pub mod name;
pub mod skill;
pub mod tree;

pub use catalog::{CatalogEntry, DiscoverError, LayerDirs, SkillCatalog};
pub use frontmatter::{
    parse_skill_md, Frontmatter, FrontmatterError, MAX_COMPATIBILITY_LEN, MAX_DESCRIPTION_LEN,
};
pub use grant::{parse_grants, resolve_grants, GrantParseError, ResolveError, SkillGrant};
pub use name::{NameError, SkillName, MAX_NAME_LEN};
pub use skill::{load_skill, Layer, Skill, SkillError};
pub use tree::{ContentHash, Stamp, TreeEntry, TreeError};
