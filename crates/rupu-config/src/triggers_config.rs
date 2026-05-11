//! `[triggers]` section of `config.toml`.
//!
//! Drives the polled-events tier of `rupu cron tick`. Empty by
//! default — rupu doesn't surprise users with API calls they didn't
//! ask for. See `docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md`,
//! §9.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PollSourceEntry {
    Source(String),
    Detailed(PollSourceSpec),
}

impl PollSourceEntry {
    pub fn source(&self) -> &str {
        match self {
            Self::Source(source) => source,
            Self::Detailed(spec) => &spec.source,
        }
    }

    pub fn poll_interval(&self) -> Option<&str> {
        match self {
            Self::Source(_) => None,
            Self::Detailed(spec) => spec.poll_interval.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PollSourceSpec {
    /// Source to poll. Today shipped poll connectors accept repo sources
    /// like `<platform>:<owner>/<repo>` and tracker-native sources
    /// such as `linear:<team-id>`. Jira support remains future work.
    pub source: String,
    /// Optional per-source cadence override like `1m`, `5m`, `1h`.
    /// When unset, the source is eligible on every `rupu cron tick`
    /// event pass.
    pub poll_interval: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TriggersConfig {
    /// Sources to poll for event-triggered workflows. Repo-backed
    /// examples: `github:Section9Labs/rupu`, `gitlab:group/project`.
    /// Tracker-native examples: `linear:<team-id>`. Jira remains
    /// future work.
    ///
    /// Each tick: rupu queries the corresponding `EventConnector`
    /// for events since the last persisted cursor.
    ///
    /// Empty by default. Project file shadows global per the
    /// existing array-replace layering rule.
    pub poll_sources: Vec<PollSourceEntry>,

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

    pub fn poll_source(&self, repo_ref: &str) -> Option<&PollSourceEntry> {
        self.poll_sources
            .iter()
            .find(|entry| entry.source() == repo_ref)
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
        assert_eq!(cfg.poll_sources.len(), 1);
        assert_eq!(cfg.poll_sources[0].source(), "github:Section9Labs/rupu");
        assert_eq!(cfg.poll_sources[0].poll_interval(), None);
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
    fn parses_inline_table_poll_source() {
        let toml_str = r#"
            poll_sources = [
              { source = "github:foo/bar", poll_interval = "5m" },
              "gitlab:baz/qux",
            ]
        "#;
        let cfg: TriggersConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.poll_sources.len(), 2);
        assert_eq!(cfg.poll_sources[0].source(), "github:foo/bar");
        assert_eq!(cfg.poll_sources[0].poll_interval(), Some("5m"));
        assert_eq!(cfg.poll_sources[1].source(), "gitlab:baz/qux");
        assert_eq!(cfg.poll_sources[1].poll_interval(), None);
    }

    #[test]
    fn finds_poll_source_by_repo_ref() {
        let toml_str = r#"
            poll_sources = [
              { source = "github:foo/bar", poll_interval = "5m" },
              "gitlab:baz/qux",
            ]
        "#;
        let cfg: TriggersConfig = toml::from_str(toml_str).unwrap();
        let github = cfg.poll_source("github:foo/bar").unwrap();
        assert_eq!(github.poll_interval(), Some("5m"));
        assert!(cfg.poll_source("github:nope/missing").is_none());
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
