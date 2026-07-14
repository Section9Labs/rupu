//! `[update]` section — release channel + passive-notice preference.

use serde::{Deserialize, Serialize};

/// `[update]` config: which release channel `rupu update` tracks, and whether
/// normal commands print a passive "update available" notice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    /// "stable" (default) or "beta".
    pub channel: Option<String>,
    /// Passive update notice on normal commands (default: on).
    pub check: Option<bool>,
}

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn parses_update_section() {
        let cfg: Config = toml::from_str(
            r#"
            [update]
            channel = "beta"
            check = false
            "#,
        )
        .unwrap();
        assert_eq!(cfg.update.channel.as_deref(), Some("beta"));
        assert_eq!(cfg.update.check, Some(false));
    }

    #[test]
    fn update_section_defaults_empty() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.update, crate::UpdateConfig::default());
    }

    #[test]
    fn rejects_unknown_update_key() {
        let err = toml::from_str::<Config>("[update]\nbogus = 1\n").unwrap_err();
        assert!(err.to_string().contains("bogus"), "got: {err}");
    }
}
