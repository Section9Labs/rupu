//! Resolve the engagement profiles a CLI entry point hands to the findings
//! write path (`FindingWriteOptions::engagement`).

use anyhow::Result;
use rupu_coverage::profile::{
    registry_with_overlay, ActiveSet, EngagementProfile, DEFAULT_PROFILE,
};
use std::path::Path;
use std::sync::Arc;

/// The active engagement set for a run, or `None` for the native `code` path.
///
/// `selected` is the operator's `--engagement-profile(s)` selection and
/// `agent_profiles` the agent's `engagementProfiles` frontmatter; the first
/// non-empty one wins whole (see [`EngagementProfile::resolve`] — the CLI
/// selection occupies its non-agent slot, since it is a list), falling back to
/// the built-in `code` default.
///
/// An empty resolution or exactly `["code"]` returns `Ok(None)` BEFORE the
/// profile registry is built: the default path is byte-identical to a run with
/// no engagement support, and a stray unparseable file under `profiles/` can
/// never break it. Only a real (non-`code`) engagement builds the overlay
/// registry — built-ins < `<global>/profiles` < `<project>/.rupu/profiles` —
/// and an unknown id or a bad profile file is an error, never a silent fall
/// back to `code`.
pub fn active_set(
    global: &Path,
    project_root: Option<&Path>,
    selected: &[String],
    agent_profiles: &[String],
) -> Result<Option<Arc<ActiveSet>>> {
    let mut ids: Vec<String> = Vec::new();
    for id in EngagementProfile::resolve(None, selected, agent_profiles) {
        let id = id.trim().to_string();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.iter().all(|id| id == DEFAULT_PROFILE) {
        return Ok(None);
    }
    let global_profiles = global.join("profiles");
    // No project => scan the global directory twice; discovery is by id, so
    // the repeat is a no-op.
    let project_profiles = project_root
        .map(|p| p.join(".rupu").join("profiles"))
        .unwrap_or_else(|| global_profiles.clone());
    let registry = registry_with_overlay(&global_profiles, &project_profiles)?;
    Ok(Some(Arc::new(registry.active_set(&ids)?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A project with a profile file that cannot parse: any call that builds
    /// the overlay registry fails on it, so `Ok(..)` proves it was never built.
    fn project_with_broken_profile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join(".rupu/profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("broken.toml"), "this is = not [valid").unwrap();
        dir
    }

    #[test]
    fn no_selection_is_none_without_touching_the_overlay() {
        let home = tempfile::tempdir().unwrap();
        let project = project_with_broken_profile();
        let got = active_set(home.path(), Some(project.path()), &[], &[]).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn code_only_is_none_without_touching_the_overlay() {
        let home = tempfile::tempdir().unwrap();
        let project = project_with_broken_profile();
        for (cli, agent) in [
            (ids(&["code"]), ids(&[])),
            (ids(&[]), ids(&["code"])),
            (ids(&["code", "code"]), ids(&[])),
        ] {
            let got = active_set(home.path(), Some(project.path()), &cli, &agent).unwrap();
            assert!(got.is_none(), "cli={cli:?} agent={agent:?}");
        }
    }

    #[test]
    fn a_non_code_selection_builds_the_set() {
        let home = tempfile::tempdir().unwrap();
        let set = active_set(home.path(), None, &ids(&["binary"]), &[])
            .unwrap()
            .expect("binary is a real engagement");
        assert_eq!(
            set.profile_for_kind("binary:function").unwrap().id,
            "binary"
        );
    }

    #[test]
    fn the_agent_frontmatter_applies_when_the_cli_is_silent() {
        let home = tempfile::tempdir().unwrap();
        let set = active_set(home.path(), None, &[], &ids(&["binary"]))
            .unwrap()
            .expect("agent frontmatter selects binary");
        assert_eq!(set.ids(), vec!["binary"]);
    }

    #[test]
    fn the_cli_selection_beats_the_agent_frontmatter() {
        let home = tempfile::tempdir().unwrap();
        // CLI says `code`, so the agent's `binary` is overridden: native path.
        let got = active_set(home.path(), None, &ids(&["code"]), &ids(&["binary"])).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn an_unknown_id_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let err = active_set(home.path(), None, &ids(&["nope"]), &[]).unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn a_broken_overlay_profile_fails_a_non_code_selection() {
        let home = tempfile::tempdir().unwrap();
        let project = project_with_broken_profile();
        let err =
            active_set(home.path(), Some(project.path()), &ids(&["binary"]), &[]).unwrap_err();
        assert!(err.to_string().contains("broken.toml"), "{err}");
    }

    #[test]
    fn a_project_profile_is_selectable() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let profiles = project.path().join(".rupu/profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(
            profiles.join("firmware.toml"),
            "id = \"firmware\"\nname = \"Firmware\"\n\n[[asset_kinds]]\nid = \"image\"\ncoordinates = [\"sha256\"]\nlabel = \"{sha256}\"\n",
        )
        .unwrap();
        let set = active_set(home.path(), Some(project.path()), &ids(&["firmware"]), &[])
            .unwrap()
            .expect("a project profile is a real engagement");
        assert_eq!(
            set.profile_for_kind("firmware:image").unwrap().id,
            "firmware"
        );
    }
}
