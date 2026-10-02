//! Build the findings write options every CLI entry point hands to tools.

use rupu_coverage::{ActiveSet, FindingWriteOptions};
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

/// Artifact store under the global rupu dir plus the `[findings]` limits.
/// Profile is `full`; callers set the resolved profile.
pub fn base_options(global: &Path, cfg: &rupu_config::FindingsConfig) -> FindingWriteOptions {
    let d = FindingWriteOptions::default();
    FindingWriteOptions {
        artifact_root: Some(global.join("findings").join("artifacts")),
        artifact_max_bytes: cfg.artifact_max_bytes.unwrap_or(d.artifact_max_bytes),
        artifact_max_files: cfg.artifact_max_files.unwrap_or(d.artifact_max_files),
        artifact_total_max_bytes: cfg
            .artifact_total_max_bytes
            .unwrap_or(d.artifact_total_max_bytes),
        report_max_bytes: cfg
            .report_max_bytes
            .map(|b| usize::try_from(b).unwrap_or(usize::MAX))
            .unwrap_or(d.report_max_bytes),
        ticket_patterns: cfg.ticket_patterns.clone(),
        ..d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let g = Path::new("/tmp/rupu-home");
        let o = base_options(g, &rupu_config::FindingsConfig::default());
        assert_eq!(
            o.artifact_root.as_deref(),
            Some(Path::new("/tmp/rupu-home/findings/artifacts"))
        );
        assert_eq!(
            o.artifact_max_bytes,
            rupu_coverage::report::DEFAULT_ARTIFACT_MAX_BYTES
        );
        assert_eq!(
            o.artifact_max_files,
            rupu_coverage::report::DEFAULT_ARTIFACT_MAX_FILES
        );
        assert_eq!(
            o.artifact_total_max_bytes,
            rupu_coverage::report::DEFAULT_ARTIFACT_TOTAL_MAX_BYTES
        );
        let cfg = rupu_config::FindingsConfig {
            artifact_max_bytes: Some(10),
            artifact_max_files: Some(3),
            artifact_total_max_bytes: Some(99),
            ..Default::default()
        };
        let o = base_options(g, &cfg);
        assert_eq!(o.artifact_max_bytes, 10);
        assert_eq!(o.artifact_max_files, 3);
        assert_eq!(o.artifact_total_max_bytes, 99);
    }

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
