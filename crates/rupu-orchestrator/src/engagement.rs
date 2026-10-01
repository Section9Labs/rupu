//! Launch-time resolution of a workflow's engagement-profile selection.
//!
//! A workflow selects engagement profiles (asset families such as `binary`)
//! with `defaults.engagement_profiles`; a step may only NARROW that set with
//! its own `engagement_profiles`. Parsing stays pure — nothing here runs in
//! [`Workflow::parse`](crate::workflow::Workflow::parse). Whether an id exists,
//! whether a step narrows rather than widens, and whether a composite collides
//! with one of its own members all depend on the profile registry, so they are
//! checked at launch ([`crate::step_factory::DefaultStepFactory`]) and the
//! refusal is surfaced as a failed step, never a silent fall back to `code`.
//!
//! The native `code` path is the default: an empty selection, or exactly
//! `[code]`, resolves to `None` BEFORE the profile registry is built, so a
//! workflow that selects nothing is byte-identical to one that predates
//! engagement support and a stray unparseable file under a `profiles/`
//! directory can never break it (the same short-circuit `rupu run` applies).

use crate::workflow::{Step, Workflow};
use rupu_coverage::profile::{registry_with_overlay, ActiveSet, RegistryError, DEFAULT_PROFILE};
use std::path::Path;
use std::sync::Arc;

/// Why an engagement selection could not be resolved at launch.
#[derive(Debug, thiserror::Error)]
pub enum EngagementError {
    /// An unknown id, a bad profile file, a kind collision, or a step id the
    /// run's set does not contain (a widening).
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// A step selected engagement profiles but the run has none to narrow:
    /// the run is on the native `code` path.
    #[error("the run has no engagement set to narrow (it is on the native `code` path); a step may only narrow what `defaults.engagement_profiles` (or the agent's `engagementProfiles`) selects, not add to it")]
    NothingToNarrow,
}

/// Trim, drop blanks, and de-duplicate (first occurrence wins, order kept).
fn clean(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        let id = id.trim();
        if !id.is_empty() && !out.iter().any(|seen| seen == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// Whether a (cleaned) selection is the native default: empty, or only `code`.
fn is_native(ids: &[String]) -> bool {
    ids.iter().all(|id| id == DEFAULT_PROFILE)
}

/// The active set for a run-level selection, or `None` for the native `code`
/// path. Only a real (non-`code`) selection builds the overlay registry —
/// built-ins < `<global>/profiles` < `<project>/.rupu/profiles` — and an
/// unknown id or a bad profile file is an error.
pub(crate) fn resolve_run_set(
    global: &Path,
    project_root: Option<&Path>,
    ids: &[String],
) -> Result<Option<Arc<ActiveSet>>, EngagementError> {
    let ids = clean(ids);
    if is_native(&ids) {
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

/// A step's effective set: `run` narrowed by the step's own selection.
///
/// * No step selection ⇒ the step inherits `run` unchanged.
/// * Exactly `[code]` ⇒ the native path (`None`): the step opts back out of
///   the engagement. That grants nothing the run did not already allow.
/// * Anything else must be a subset of `run` (an origin reachable from a
///   selected composite counts) — a widening is an error, as is selecting
///   anything when the run itself is on the native path.
pub(crate) fn narrow_to_step(
    run: Option<&Arc<ActiveSet>>,
    step_ids: &[String],
) -> Result<Option<Arc<ActiveSet>>, EngagementError> {
    let ids = clean(step_ids);
    if ids.is_empty() {
        return Ok(run.cloned());
    }
    if is_native(&ids) {
        return Ok(None);
    }
    match run {
        None => Err(EngagementError::NothingToNarrow),
        Some(set) => Ok(Some(Arc::new(set.narrow(&ids)?))),
    }
}

/// Why a remote (`host:` / `distribute:`) unit must be refused for `step`, or
/// `None` when it may run.
///
/// Remote engagement delivery is deferred: `UnitDispatch` and every host
/// connector carry a findings PROFILE but no engagement set, so a non-default
/// selection would silently run on the host under the native `code` rules —
/// exactly the "wrong domain" the engagement model must never allow. The
/// selection judged is the step's own `engagement_profiles`, else
/// `defaults.engagement_profiles` ([`Workflow::action_engagement`]); `[code]`
/// or nothing is the default and dispatches as before.
pub(crate) fn remote_unit_refusal(workflow: &Workflow, step: &Step) -> Option<String> {
    let ids = clean(&workflow.action_engagement(step));
    if is_native(&ids) {
        return None;
    }
    Some(format!(
        "step `{}` runs on a remote host but selects engagement profile(s) [{}]; remote and \
         placed execution cannot deliver an engagement set yet, so it is refused rather than \
         run under the native `{DEFAULT_PROFILE}` rules. Run the step locally (drop `host:` / \
         `distribute:`), or narrow it with `engagement_profiles: [{DEFAULT_PROFILE}]`",
        step.id,
        ids.join(", "),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn run_set(selected: &[&str]) -> Arc<ActiveSet> {
        let home = tempfile::tempdir().unwrap();
        resolve_run_set(home.path(), None, &ids(selected))
            .unwrap()
            .expect("a real engagement")
    }

    /// A project holding a profile file that cannot parse: any call that
    /// builds the overlay registry fails on it, so `Ok(..)` proves it never
    /// was built.
    fn project_with_broken_profile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join(".rupu/profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("broken.toml"), "this is = not [valid").unwrap();
        dir
    }

    #[test]
    fn native_selections_short_circuit_before_the_overlay() {
        let home = tempfile::tempdir().unwrap();
        let project = project_with_broken_profile();
        for sel in [
            ids(&[]),
            ids(&["code"]),
            ids(&["code", "code"]),
            ids(&[" "]),
        ] {
            let got = resolve_run_set(home.path(), Some(project.path()), &sel).unwrap();
            assert!(got.is_none(), "{sel:?}");
        }
    }

    #[test]
    fn a_real_selection_builds_the_set_and_surfaces_overlay_errors() {
        let home = tempfile::tempdir().unwrap();
        let project = project_with_broken_profile();
        let err = resolve_run_set(home.path(), Some(project.path()), &ids(&["binary"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("broken.toml"), "{err}");
        let err = resolve_run_set(home.path(), None, &ids(&["nope"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn duplicate_ids_are_collapsed_not_collided() {
        let set = run_set(&["binary", "binary", " binary "]);
        assert_eq!(set.ids(), vec!["binary"]);
    }

    #[test]
    fn narrowing_inherits_subsets_and_drops_to_native() {
        let run = run_set(&["code", "binary"]);
        // No step selection: inherit.
        let same = narrow_to_step(Some(&run), &[]).unwrap().unwrap();
        assert_eq!(same.ids(), vec!["code", "binary"]);
        // A subset.
        let only = narrow_to_step(Some(&run), &ids(&["binary"]))
            .unwrap()
            .unwrap();
        assert_eq!(only.ids(), vec!["binary"]);
        assert!(only.profile_for_kind("code:file").is_none());
        // `[code]` opts the step out entirely, even when `code` is not in the
        // run's set.
        assert!(narrow_to_step(Some(&run_set(&["binary"])), &ids(&["code"]))
            .unwrap()
            .is_none());
        assert!(narrow_to_step(None, &ids(&["code"])).unwrap().is_none());
        assert!(narrow_to_step(None, &[]).unwrap().is_none());
    }

    #[test]
    fn narrowing_cannot_widen() {
        let run = run_set(&["binary"]);
        let err = narrow_to_step(Some(&run), &ids(&["web"])).unwrap_err();
        assert!(matches!(
            err,
            EngagementError::Registry(RegistryError::WidenNotAllowed(ref id)) if id == "web"
        ));
        // `code` mixed with others is not the native opt-out: it must be in
        // the run's set like any other id.
        assert!(narrow_to_step(Some(&run), &ids(&["binary", "code"])).is_err());
        // A run on the native path has nothing to narrow.
        let err = narrow_to_step(None, &ids(&["binary"])).unwrap_err();
        assert!(matches!(err, EngagementError::NothingToNarrow));
    }

    #[test]
    fn remote_refusal_judges_the_step_then_defaults_selection() {
        let wf = Workflow::parse(
            "name: w\ndefaults:\n  engagement_profiles: [binary]\nsteps:\n  - id: inherits\n    agent: a\n    prompt: p\n    host: h\n  - id: native\n    agent: a\n    prompt: p\n    host: h\n    engagement_profiles: [code]\n",
        )
        .unwrap();
        let msg = remote_unit_refusal(&wf, &wf.steps[0]).expect("defaults select binary");
        assert!(msg.contains("inherits") && msg.contains("binary"), "{msg}");
        assert!(remote_unit_refusal(&wf, &wf.steps[1]).is_none());
        let plain = Workflow::parse(
            "name: w\nsteps:\n  - id: a\n    agent: a\n    prompt: p\n    host: h\n",
        )
        .unwrap();
        assert!(remote_unit_refusal(&plain, &plain.steps[0]).is_none());
    }
}
