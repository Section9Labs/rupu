//! Resolve the engagement profiles a CLI entry point hands to the findings
//! write path (`FindingWriteOptions::engagement`).

use anyhow::{Context, Result};
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
    // Drop blank CLI ids BEFORE resolving: a purely-blank `--engagement-profile
    // ""` is "no CLI selection" and must fall through to the agent's
    // frontmatter, not win the precedence as `[""]` and then clean to nothing
    // (a silent downgrade to the native `code` path).
    let selected: Vec<String> = selected
        .iter()
        .filter(|id| !id.trim().is_empty())
        .cloned()
        .collect();
    let mut ids: Vec<String> = Vec::new();
    for id in EngagementProfile::resolve(None, &selected, agent_profiles) {
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

/// The findings base a WORKFLOW run's sub-agent dispatcher hands every agent
/// it dispatches (`dispatch_agent` from inside a step): `base` plus the
/// workflow-default engagement (`defaults.engagement_profiles`).
///
/// Without this a sub-agent under a `binary` workflow would record under the
/// native `code` rules while its parent step records under `binary` — a
/// silent downgrade. A workflow that selects nothing (or only `code`) returns
/// `base` untouched, and never builds the profile registry, so default
/// workflows stay byte-identical. An unknown id or a bad profile file is an
/// error, never a fall back to `code`.
///
/// Known limitation: this is the WORKFLOW-level set only. A step's own
/// `engagement_profiles` narrowing (and an agent's `engagementProfiles`
/// frontmatter) apply to that step's own agent but are not carried to the
/// sub-agents it dispatches — they inherit the broader workflow set.
pub fn workflow_dispatch_base(
    global: &Path,
    project_root: Option<&Path>,
    workflow: &rupu_orchestrator::Workflow,
    base: rupu_coverage::FindingWriteOptions,
) -> Result<rupu_coverage::FindingWriteOptions> {
    let engagement = active_set(
        global,
        project_root,
        &workflow.defaults.engagement_profiles,
        &[],
    )
    .with_context(|| {
        format!(
            "workflow `{}`: cannot resolve `defaults.engagement_profiles`",
            workflow.name
        )
    })?;
    Ok(base.with_engagement(engagement))
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
    fn a_blank_cli_value_falls_through_to_the_agent_not_a_downgrade() {
        let home = tempfile::tempdir().unwrap();
        for blank in [ids(&[""]), ids(&["  "]), ids(&["", " \t "])] {
            let set = active_set(home.path(), None, &blank, &ids(&["binary"]))
                .unwrap()
                .unwrap_or_else(|| panic!("blank cli {blank:?} must not drop the agent's binary"));
            assert_eq!(set.ids(), vec!["binary"], "cli={blank:?}");
        }
    }

    #[test]
    fn a_trailing_comma_still_selects_the_real_id() {
        let home = tempfile::tempdir().unwrap();
        // `--engagement-profiles binary,` splits to ["binary", ""].
        let set = active_set(home.path(), None, &ids(&["binary", ""]), &[])
            .unwrap()
            .expect("binary survives the trailing blank");
        assert_eq!(set.ids(), vec!["binary"]);
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

    fn workflow_with(defaults: &str) -> rupu_orchestrator::Workflow {
        let yaml = format!("name: wf\n{defaults}steps:\n  - id: s\n    agent: a\n    prompt: p\n");
        rupu_orchestrator::Workflow::parse(&yaml).unwrap()
    }

    fn base() -> rupu_coverage::FindingWriteOptions {
        rupu_coverage::FindingWriteOptions {
            artifact_max_bytes: 7,
            ..Default::default()
        }
    }

    #[test]
    fn a_workflow_without_engagement_leaves_the_dispatch_base_untouched() {
        let home = tempfile::tempdir().unwrap();
        let project = project_with_broken_profile();
        for defaults in [
            "",
            "defaults:\n  engagement_profiles: [code]\n",
            "defaults:\n  engagement_profiles: []\n",
        ] {
            let wf = workflow_with(defaults);
            // The broken project profile proves the overlay was never built.
            let got =
                workflow_dispatch_base(home.path(), Some(project.path()), &wf, base()).unwrap();
            assert!(got.engagement.is_none(), "{defaults:?}");
            assert_eq!(got.artifact_max_bytes, 7, "the rest of the base is kept");
        }
    }

    #[test]
    fn a_workflow_engagement_reaches_the_dispatch_base() {
        let home = tempfile::tempdir().unwrap();
        let wf = workflow_with("defaults:\n  engagement_profiles: [binary]\n");
        let got = workflow_dispatch_base(home.path(), None, &wf, base()).unwrap();
        let set = got.engagement.expect("binary is a real engagement");
        assert_eq!(set.ids(), vec!["binary"]);
        assert_eq!(got.artifact_max_bytes, 7);
    }

    #[test]
    fn an_unresolvable_workflow_engagement_is_an_error_not_a_downgrade() {
        let home = tempfile::tempdir().unwrap();
        let wf = workflow_with("defaults:\n  engagement_profiles: [nope]\n");
        let err = format!(
            "{:#}",
            workflow_dispatch_base(home.path(), None, &wf, base()).unwrap_err()
        );
        assert!(err.contains("defaults.engagement_profiles"), "{err}");
        assert!(err.contains("wf"), "{err}");
        assert!(err.contains("nope"), "the cause is kept: {err}");
    }
}
