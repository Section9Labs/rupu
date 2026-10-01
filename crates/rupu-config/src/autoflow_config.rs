//! `[autoflow]` section of `config.toml`.
//!
//! This section carries machine-local and operational defaults for
//! autonomous workflow execution. Logical ownership and scheduling
//! policy stays in workflow YAML under `autoflow:`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoflowConfig {
    pub enabled: Option<bool>,
    pub repo: Option<String>,
    pub checkout: Option<AutoflowCheckout>,
    pub worktree_root: Option<String>,
    pub permission_mode: Option<String>,
    pub strict_templates: Option<bool>,
    pub max_active: Option<u32>,
    pub cleanup_after: Option<String>,
    /// Days of autoflow cycle/event history to keep under
    /// `<global>/autoflows/history`; `0` keeps it forever. Absent means
    /// [`AutoflowConfig::DEFAULT_HISTORY_RETENTION_DAYS`]. Read from the
    /// global config only — the history is machine-wide, not per project.
    pub history_retention_days: Option<u32>,
}

impl AutoflowConfig {
    pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 30;

    /// How many whole UTC days of history to keep, or `None` to keep it all.
    pub fn history_retention_days(&self) -> Option<u32> {
        match self
            .history_retention_days
            .unwrap_or(Self::DEFAULT_HISTORY_RETENTION_DAYS)
        {
            0 => None,
            days => Some(days),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoflowCheckout {
    Worktree,
    InPlace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_autoflow_config() {
        let cfg: AutoflowConfig = toml::from_str("").expect("parse");
        assert_eq!(cfg.enabled, None);
        assert_eq!(cfg.repo, None);
    }

    #[test]
    fn parses_full_autoflow_config() {
        let toml = r#"
            enabled = true
            repo = "github:Section9Labs/rupu"
            checkout = "worktree"
            worktree_root = "~/.rupu/autoflows/worktrees"
            permission_mode = "bypass"
            strict_templates = true
            max_active = 2
            cleanup_after = "7d"
            history_retention_days = 14
        "#;
        let cfg: AutoflowConfig = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.enabled, Some(true));
        assert_eq!(cfg.repo.as_deref(), Some("github:Section9Labs/rupu"));
        assert_eq!(cfg.checkout, Some(AutoflowCheckout::Worktree));
        assert_eq!(
            cfg.worktree_root.as_deref(),
            Some("~/.rupu/autoflows/worktrees")
        );
        assert_eq!(cfg.permission_mode.as_deref(), Some("bypass"));
        assert_eq!(cfg.strict_templates, Some(true));
        assert_eq!(cfg.max_active, Some(2));
        assert_eq!(cfg.cleanup_after.as_deref(), Some("7d"));
        assert_eq!(cfg.history_retention_days(), Some(14));
    }

    #[test]
    fn history_retention_defaults_to_30_days_and_zero_keeps_everything() {
        let unset: AutoflowConfig = toml::from_str("").expect("parse");
        assert_eq!(unset.history_retention_days(), Some(30));
        let forever: AutoflowConfig = toml::from_str("history_retention_days = 0").expect("parse");
        assert_eq!(forever.history_retention_days(), None);
    }

    #[test]
    fn rejects_unknown_field() {
        let toml = r#"
            enabled = true
            unknown_key = "boom"
        "#;
        let res: Result<AutoflowConfig, _> = toml::from_str(toml);
        assert!(res.is_err());
    }
}
