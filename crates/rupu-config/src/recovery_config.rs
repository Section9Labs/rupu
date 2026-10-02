//! `[recovery]` section of `config.toml` — what to do when a model's reply
//! cannot be used as is: the fallback chain and the server-side fallback
//! opt-out. See `docs/superpowers/specs/2026-10-01-rupu-response-outcomes-design.md`.

use serde::{Deserialize, Serialize};

/// One rung of a fallback chain: a model, optionally on another provider.
/// A missing `provider` means "the provider the run is already on".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FallbackEntry {
    #[serde(default)]
    pub provider: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryConfig {
    /// Ordered fallback chain used when an agent declares no `fallbacks:`.
    pub fallbacks: Vec<FallbackEntry>,
    /// Ask Anthropic to fall back server-side on models that support it.
    /// Defaults to `true`.
    #[serde(default = "RecoveryConfig::default_true")]
    pub server_side_fallback: bool,
}

impl RecoveryConfig {
    fn default_true() -> bool {
        true
    }

    /// Agent frontmatter wins; else this table's chain; else empty.
    pub fn chain_for(&self, agent: Option<&[FallbackEntry]>) -> Vec<FallbackEntry> {
        match agent {
            Some(chain) => chain.to_vec(),
            None => self.fallbacks.clone(),
        }
    }
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            fallbacks: Vec::new(),
            server_side_fallback: Self::default_true(),
        }
    }
}

/// Splits a chain for a run on `current_provider`: rung 1 is the entries on the
/// same provider (or with no provider named), rung 2 is the others. Order is
/// kept within each rung.
pub fn split_chain(
    chain: &[FallbackEntry],
    current_provider: &str,
) -> (Vec<FallbackEntry>, Vec<FallbackEntry>) {
    chain
        .iter()
        .cloned()
        .partition(|e| e.provider.as_deref().is_none_or(|p| p == current_provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn entry(provider: Option<&str>, model: &str) -> FallbackEntry {
        FallbackEntry {
            provider: provider.map(str::to_string),
            model: model.to_string(),
        }
    }

    #[test]
    fn parses_through_config() {
        let toml = "[recovery]\nfallbacks = [{ model = \"claude-opus-4-8\" }, { provider = \"openai-codex\", model = \"gpt-5.6-cyber\" }]\n";
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            cfg.recovery.fallbacks,
            vec![
                entry(None, "claude-opus-4-8"),
                entry(Some("openai-codex"), "gpt-5.6-cyber"),
            ]
        );
    }

    #[test]
    fn server_side_fallback_defaults_true_and_can_be_disabled() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.recovery.server_side_fallback);
        assert!(cfg.recovery.fallbacks.is_empty());
        let cfg: Config = toml::from_str("[recovery]\nserver_side_fallback = false\n").unwrap();
        assert!(!cfg.recovery.server_side_fallback);
        // A table that sets only `fallbacks` keeps the default.
        let cfg: Config = toml::from_str("[recovery]\nfallbacks = [{ model = \"m\" }]\n").unwrap();
        assert!(cfg.recovery.server_side_fallback);
    }

    #[test]
    fn unknown_recovery_key_is_an_error() {
        assert!(toml::from_str::<Config>("[recovery]\nbogus = 1\n").is_err());
        assert!(toml::from_str::<Config>(
            "[recovery]\nfallbacks = [{ model = \"m\", bogus = 1 }]\n"
        )
        .is_err());
    }

    #[test]
    fn chain_for_prefers_agent_chain() {
        let cfg = RecoveryConfig {
            fallbacks: vec![entry(None, "table-model")],
            ..Default::default()
        };
        let x = entry(None, "agent-model");
        assert_eq!(cfg.chain_for(Some(std::slice::from_ref(&x))), vec![x]);
        assert_eq!(cfg.chain_for(None), vec![entry(None, "table-model")]);
        assert!(RecoveryConfig::default().chain_for(None).is_empty());
    }

    #[test]
    fn split_chain_partitions_by_provider_keeping_order() {
        let chain = [
            entry(None, "claude-opus-4-8"),
            entry(Some("openai-codex"), "gpt-5.6-cyber"),
            entry(Some("anthropic"), "claude-sonnet-4-8"),
        ];
        let (same, other) = split_chain(&chain, "anthropic");
        assert_eq!(
            same,
            vec![
                entry(None, "claude-opus-4-8"),
                entry(Some("anthropic"), "claude-sonnet-4-8")
            ]
        );
        assert_eq!(other, vec![entry(Some("openai-codex"), "gpt-5.6-cyber")]);
    }
}
