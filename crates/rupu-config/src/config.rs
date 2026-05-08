//! Configuration types. See the Slice A spec for semantics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::pricing_config::PricingConfig;
use crate::provider_config::ProviderConfig;
use crate::scm_config::{IssuesSection, ScmSection};
use crate::triggers_config::TriggersConfig;

/// Top-level rupu configuration. Loaded from `~/.rupu/config.toml`
/// (global) and optionally overridden by `<repo>/.rupu/config.toml`
/// (project) — see [`crate::layer`] for layering rules.
///
/// All fields are optional so that a missing value at one layer can be
/// supplied by another. Defaults are applied at the consumer (e.g.,
/// `rupu-agent` substitutes `permission_mode = "ask"` if absent).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub permission_mode: Option<String>,
    pub log_level: Option<String>,
    pub bash: BashConfig,
    pub retry: RetryConfig,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub scm: ScmSection,
    #[serde(default)]
    pub issues: IssuesSection,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub triggers: TriggersConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
}

/// Terminal-output rendering preferences. Consumed by
/// `rupu agent show` / `rupu workflow show` (and any future
/// commands that print syntax-highlighted output).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// `auto` (default — color when stdout is a tty and `NO_COLOR`
    /// is unset), `always`, or `never`.
    pub color: Option<String>,
    /// syntect theme name. Defaults to `base16-ocean.dark`.
    pub theme: Option<String>,
    /// `auto` (default — page when stdout is a tty and the output
    /// exceeds one screen), `always`, or `never`.
    pub pager: Option<String>,
}

/// Bash tool configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BashConfig {
    /// Timeout for a single `bash` invocation. Defaults to 120 seconds
    /// at the consumer if absent.
    pub timeout_secs: Option<u64>,
    /// Environment variables (beyond the always-allowed PATH/HOME/USER/
    /// TERM/LANG) that are forwarded into the bash subprocess.
    pub env_allowlist: Option<Vec<String>>,
}

/// Provider-call retry configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetryConfig {
    pub max_attempts: Option<u32>,
    pub initial_delay_ms: Option<u64>,
}
