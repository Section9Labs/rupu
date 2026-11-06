//! The findings write options a run, a CP import and the CLI start from.

use rupu_coverage::FindingWriteOptions;
use std::path::Path;

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

/// [`base_options`] from the GLOBAL config's `[findings]`: the limits an
/// import of finding reports writes under. An unreadable config gives the
/// defaults, with the warning to show.
pub fn global_options(global: &Path) -> (FindingWriteOptions, Option<String>) {
    let cfg_path = global.join("config.toml");
    let layers = rupu_config::LayerPaths::global_only(&cfg_path);
    let (cfg, warning) = match rupu_config::layer_files_locked(layers) {
        Ok(c) => (c.findings, None),
        Err(e) => (
            Default::default(),
            Some(format!(
                "cannot read {}: {e}; using the default findings limits",
                cfg_path.display()
            )),
        ),
    };
    (base_options(global, &cfg), warning)
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
}
