//! Build the findings write options every CLI entry point hands to tools.

use rupu_coverage::ActiveSet;
pub use rupu_runtime::findings_opts::base_options;
use std::path::Path;
use std::sync::Arc;

/// Resolve the selected engagement-profile ids into an active set, overlaying
/// discovered profiles (`<global>/profiles`, `<workspace>/.rupu/profiles`) on
/// the built-ins. An empty selection is the native code path (`None`). A bad
/// id or an unloadable profile fails loudly — never a silent default.
pub fn resolve_engagement(
    global: &Path,
    workspace: &Path,
    ids: &[String],
) -> anyhow::Result<Option<Arc<ActiveSet>>> {
    if ids.is_empty() {
        return Ok(None);
    }
    let registry = rupu_coverage::registry_with_overlay(
        &global.join("profiles"),
        &workspace.join(".rupu").join("profiles"),
    )
    .map_err(|e| anyhow::anyhow!("engagement profiles: {e}"))?;
    let set = registry
        .active_set(ids)
        .map_err(|e| anyhow::anyhow!("engagement profiles: {e}"))?;
    Ok(Some(Arc::new(set)))
}

/// The engagement a RESUMED workflow run continues under: the profile ids it
/// was launched with (`RunRecord.engagement_profiles`), re-resolved against
/// the run's own workspace exactly as the launch resolved them. A recorded
/// profile that no longer resolves fails the resume rather than continuing
/// on the `code` path; a run recorded without one (or before the field
/// existed) is `None`, the code path it launched on.
pub fn recorded_engagement(
    global: &Path,
    record: &rupu_orchestrator::RunRecord,
) -> anyhow::Result<Option<Arc<ActiveSet>>> {
    resolve_engagement(global, &record.workspace_path, &record.engagement_profiles).map_err(|e| {
        anyhow::anyhow!(
            "resume run {} under its recorded engagement profile(s) `{}`: {e}",
            record.id,
            record.engagement_profiles.join(",")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_engagement_empty_is_code_path_and_unknown_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let g = tmp.path();
        let ws = tmp.path();
        // empty selection = the native code path
        assert!(resolve_engagement(g, ws, &[]).unwrap().is_none());
        // a built-in resolves (profile dirs need not exist)
        let set = resolve_engagement(g, ws, &["network".to_string()])
            .unwrap()
            .expect("network resolves");
        assert!(set.profile_for_kind("network:service").is_some());
        // an unknown id fails loudly
        assert!(resolve_engagement(g, ws, &["nope".to_string()]).is_err());
    }
}
