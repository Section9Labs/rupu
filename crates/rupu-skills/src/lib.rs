//! Agent Skills (agentskills.io `SKILL.md` directories): the model, its
//! validation, discovery over rupu's global and project layers, version
//! fingerprints, the grant grammar and the prompt renderers. Spec:
//! `docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md`.

pub mod name;

pub use name::{NameError, SkillName, MAX_NAME_LEN};
