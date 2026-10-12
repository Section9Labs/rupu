//! `[coverage]` section — how a run's concern catalog is rendered.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CoverageConfig {
    /// Most concerns a catalog may hold for its `auto` includes to render in
    /// full; a larger catalog renders them in index mode. Explicit
    /// `mode: full` / `mode: index` includes are unaffected. Default 80
    /// (`rupu_coverage::DEFAULT_FULL_MODE_THRESHOLD`).
    pub full_mode_max_concerns: Option<usize>,
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn parses_coverage_section() {
        let cfg: Config = toml::from_str("[coverage]\nfull_mode_max_concerns = 12\n").unwrap();
        assert_eq!(cfg.coverage.full_mode_max_concerns, Some(12));
        assert_eq!(Config::default().coverage.full_mode_max_concerns, None);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[coverage]\nbogus = 1\n").is_err());
    }
}
