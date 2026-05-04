//! Configuration types. See the Slice A spec for semantics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::provider_config::ProviderConfig;
use crate::scm_config::{IssuesSection, ScmSection};

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
