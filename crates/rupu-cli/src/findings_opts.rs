//! Build the findings write options every CLI entry point hands to tools.

use rupu_coverage::FindingWriteOptions;
use std::path::Path;

/// Artifact store under the global rupu dir plus the `[findings]` limits.
/// Profile is `full`; callers set the resolved profile.
pub fn base_options(global: &Path, cfg: &rupu_config::FindingsConfig) -> FindingWriteOptions {
    let d = FindingWriteOptions::default();
    FindingWriteOptions {
        artifact_root: Some(global.join("findings").join("artifacts")),
        artifact_max_bytes: cfg.artifact_max_bytes.unwrap_or(d.artifact_max_bytes),
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
        let cfg = rupu_config::FindingsConfig {
            artifact_max_bytes: Some(10),
            ..Default::default()
        };
        assert_eq!(base_options(g, &cfg).artifact_max_bytes, 10);
    }
}
