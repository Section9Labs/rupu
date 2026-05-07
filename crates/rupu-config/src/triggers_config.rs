//! `[triggers]` section of `config.toml`.
//!
//! Drives the polled-events tier of `rupu cron tick`. Empty by
//! default — rupu doesn't surprise users with API calls they didn't
//! ask for. See `docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`,
//! §9.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TriggersConfig {
    /// Repos to poll for event-triggered workflows. Format:
    /// `<platform>:<owner>/<repo>` (e.g. `github:Section9Labs/rupu`).
    /// Each tick: rupu queries the corresponding `EventConnector`
    /// for events since the last persisted cursor.
    ///
    /// Empty by default. Project file shadows global per the
    /// existing array-replace layering rule.
    pub poll_sources: Vec<String>,

    /// Cap on events processed per source per tick. Default 50.
    /// Prevents a backlog (or a misconfigured filter) from chewing
    /// the rate-limit budget.
    pub max_events_per_tick: Option<u32>,
}

impl TriggersConfig {
    /// Resolved cap with the documented default applied.
    pub fn effective_max_events_per_tick(&self) -> u32 {
        self.max_events_per_tick.unwrap_or(50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let toml_str = r#"
            poll_sources = ["github:Section9Labs/rupu"]
        "#;
        let cfg: TriggersConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.poll_sources, vec!["github:Section9Labs/rupu"]);
        assert_eq!(cfg.effective_max_events_per_tick(), 50);
    }

    #[test]
    fn parses_with_cap_override() {
        let toml_str = r#"
            poll_sources = ["github:foo/bar", "gitlab:baz/qux"]
            max_events_per_tick = 20
        "#;
        let cfg: TriggersConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.poll_sources.len(), 2);
        assert_eq!(cfg.effective_max_events_per_tick(), 20);
    }

    #[test]
    fn rejects_unknown_field() {
        let toml_str = r#"
            poll_sources = []
            unknown_key = "boom"
        "#;
        let res: Result<TriggersConfig, _> = toml::from_str(toml_str);
        assert!(res.is_err());
    }

    #[test]
    fn defaults_when_section_missing() {
        let cfg = TriggersConfig::default();
        assert!(cfg.poll_sources.is_empty());
        assert_eq!(cfg.effective_max_events_per_tick(), 50);
    }
}
