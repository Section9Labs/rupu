//! `[findings]` section — limits and org-specific hints for recorded findings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FindingsConfig {
    /// Per-file cap for copying finding artifacts into the store. Larger
    /// files are recorded by hash only. Default 500 MB.
    pub artifact_max_bytes: Option<u64>,
    /// Most files one report's artifacts may expand to (a directory counts
    /// every file inside it). Default 500.
    pub artifact_max_files: Option<usize>,
    /// Most bytes one report's artifacts may add up to in the store: only
    /// files copied into it count, not files over `artifact_max_bytes`
    /// (which are recorded by reference). Default 2 GiB.
    pub artifact_total_max_bytes: Option<u64>,
    /// Serialized-size budget for one finding report. Default 256 KiB.
    pub report_max_bytes: Option<u64>,
    /// Patterns (regexes or URL prefixes) that identify existing tickets in
    /// this organisation; added to the agent guidance. Empty by default.
    pub ticket_patterns: Vec<String>,
    /// Display-number prefix for exported reports (default `SEC`).
    pub export_id_prefix: Option<String>,
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn parses_findings_section() {
        let cfg: Config = toml::from_str(
            r#"
            [findings]
            artifact_max_bytes = 1048576
            artifact_max_files = 50
            artifact_total_max_bytes = 10485760
            ticket_patterns = ["ABC-[0-9]+"]
            "#,
        )
        .unwrap();
        assert_eq!(cfg.findings.artifact_max_bytes, Some(1_048_576));
        assert_eq!(cfg.findings.artifact_max_files, Some(50));
        assert_eq!(cfg.findings.artifact_total_max_bytes, Some(10_485_760));
        assert_eq!(cfg.findings.report_max_bytes, None);
        assert_eq!(cfg.findings.ticket_patterns, vec!["ABC-[0-9]+".to_string()]);
    }

    #[test]
    fn parses_export_id_prefix() {
        let cfg: Config = toml::from_str("[findings]\nexport_id_prefix = \"VULN\"\n").unwrap();
        assert_eq!(cfg.findings.export_id_prefix.as_deref(), Some("VULN"));
        assert_eq!(Config::default().findings.export_id_prefix, None);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[findings]\nbogus = 1\n").is_err());
    }
}
