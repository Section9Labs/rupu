//! A session turn the operator invoked skills in (spec 2026-10-12 §5.2):
//! each skill's body goes into the turn's message ahead of the prompt, and
//! the run records it as an operator load.

use crate::runner::OperatorLoad;
use rupu_skills::render::render_operator_injection;
use rupu_skills::{FrozenSkill, ResolvedSkills, TreeError};

/// The turn's message, `invoked`'s skills injected ahead of `prompt` (an
/// empty prompt adds nothing), and the loads that record them. Each skill
/// is taken from `skills`, the turn's set, at its current version; one the
/// set can't give (it no longer loads) is left out, since the turn reports
/// it missing.
///
/// Hashes each skill's files: blocking work an async caller runs on
/// `spawn_blocking`.
pub fn operator_turn(
    skills: Option<&ResolvedSkills>,
    invoked: &[FrozenSkill],
    prompt: &str,
) -> Result<(String, Vec<OperatorLoad>), TreeError> {
    let mut parts = Vec::new();
    let mut loads = Vec::new();
    for f in invoked {
        let Some(g) = skills
            .and_then(|s| s.get(&f.name))
            .filter(|g| g.skill.layer == f.layer)
        else {
            continue;
        };
        let version = g.skill.content_hash()?;
        parts.push(render_operator_injection(
            g.name(),
            &g.skill.root,
            &version,
            &g.skill.body,
        ));
        loads.push(OperatorLoad {
            name: g.name().to_owned(),
            layer: g.skill.layer.as_str().to_owned(),
            content_hash: version.hex(),
            base_dir: g.skill.root.display().to_string(),
        });
    }
    if !prompt.is_empty() {
        parts.push(prompt.to_owned());
    }
    Ok((parts.join("\n\n"), loads))
}
