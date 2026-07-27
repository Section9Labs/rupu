//! Configuration types. See the Slice A spec for semantics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::autoflow_config::AutoflowConfig;
use crate::pricing_config::PricingConfig;
use crate::provider_config::ProviderConfig;
use crate::scm_config::{IssuesSection, ScmSection};
use crate::storage_config::StorageConfig;
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
    /// **Deprecated, inert, and never read.** `[retry]` (`max_attempts` /
    /// `initial_delay_ms`) parsed since Slice A and drove nothing; per-provider
    /// `[providers.<name>].max_retries` supersedes it (ISSUES.md I-13).
    ///
    /// The field survives the deletion ONLY as a migration shim. `Config` is
    /// `deny_unknown_fields`, so an existing `config.toml` carrying the section
    /// would otherwise fail to deserialize — and seven CLI paths plus `rupu-cp`
    /// load config with `.unwrap_or_default()`, which converts that parse error
    /// into the *silent loss of every other key the user set*. Accepting the
    /// key as an opaque no-op keeps the rest of the config intact; the warning
    /// in [`Config::warn_deprecated_keys`] tells the user to delete it.
    ///
    /// `skip_serializing` keeps it out of `/api/config` and out of anything
    /// that round-trips `Config` back to TOML, so the key disappears the first
    /// time a config is rewritten. Remove the field one release after v0.68.
    #[serde(default, skip_serializing)]
    pub retry: Option<toml::Value>,
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
    pub autoflow: AutoflowConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub policy: crate::policy_config::PolicyConfig,
    #[serde(default)]
    pub cp: crate::policy_config::CpConfig,
    #[serde(default)]
    pub update: crate::update_config::UpdateConfig,
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
    /// Shared overall theme knob. When set, consumers treat this as the
    /// simple "use one named theme across syntax + palette" selector,
    /// unless a more specific `[ui.syntax]` or `[ui.palette]` override
    /// is present.
    pub theme: Option<String>,
    #[serde(default)]
    pub syntax: UiSyntaxConfig,
    #[serde(default)]
    pub palette: UiPaletteConfig,
    /// Default live renderer mode for interactive/event-driven views:
    /// `focused` (default), `compact`, or `full`.
    pub live_view: Option<String>,
    /// `auto` (default — page when stdout is a tty and the output
    /// exceeds one screen), `always`, or `never`.
    pub pager: Option<String>,
    /// External editor command used by `agent edit`/`create` and
    /// `workflow edit`/`create`. May include flags (e.g. `code --wait`,
    /// `vim -p`). Resolves between `--editor` (flag) and `$VISUAL`.
    pub editor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSyntaxConfig {
    /// syntect theme name. Defaults to `base16-ocean.dark`.
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiPaletteConfig {
    /// Named rupu UI palette theme. Defaults to `rupu-dark`.
    pub theme: Option<String>,
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

/// Provider names reserved for built-in providers. A `[providers.<name>]`
/// entry with `kind = "openai-compatible"` may not reuse one of these —
/// mirrors the built-ins in `rupu_runtime::provider_factory::is_builtin_provider`
/// and `rupu-auth`'s `parse_provider`. Keep in sync if built-ins change.
const RESERVED_PROVIDER_NAMES: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "copilot",
    "local",
    "github",
    "gitlab",
    "linear",
    "jira",
];

impl Config {
    /// Warn about keys that still parse but no longer do anything.
    ///
    /// Called from every load path (`layer_files`, `layer_files_locked` /
    /// `resolve`) via [`Config::validate`], so a deprecated key is surfaced
    /// wherever config is read rather than only on one command.
    pub fn warn_deprecated_keys(&self) {
        if self.retry.is_some() {
            tracing::warn!(
                key = "retry",
                "config.toml still declares a `[retry]` section (max_attempts / \
                 initial_delay_ms). It has never been read by anything and is now \
                 formally deprecated: it is accepted as a no-op so the rest of your \
                 config still loads. Delete the `[retry]` section; set \
                 `[providers.<name>].max_retries` instead. A future release will \
                 reject it."
            );
        }
        if self.cp.agent_authoring_ui.is_some() {
            tracing::warn!(
                key = "cp.agent_authoring_ui",
                "config.toml still declares `[cp].agent_authoring_ui`. The CP web \
                 app's classic agent-authoring UI has been deleted; the \"next\" UI \
                 is now the only UI and this key is accepted as a no-op so the rest \
                 of your config still loads. Delete `[cp].agent_authoring_ui`. A \
                 future release will reject it."
            );
        }
        if self.cp.workflow_editor_ui.is_some() {
            tracing::warn!(
                key = "cp.workflow_editor_ui",
                "config.toml still declares `[cp].workflow_editor_ui`. The CP web \
                 app's classic workflow-editor UI has been deleted; the \"next\" UI \
                 is now the only UI and this key is accepted as a no-op so the rest \
                 of your config still loads. Delete `[cp].workflow_editor_ui`. A \
                 future release will reject it."
            );
        }
    }

    /// Validate cross-field invariants not expressible in serde.
    ///
    /// Also emits [`Config::warn_deprecated_keys`] — this is the one function
    /// every load path already calls.
    pub fn validate(&self) -> Result<(), crate::layer::LayerError> {
        self.warn_deprecated_keys();
        for (name, p) in &self.providers {
            if p.kind.as_deref() == Some("openai-compatible") {
                if RESERVED_PROVIDER_NAMES.contains(&name.as_str()) {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "provider '{name}': \"openai-compatible\" cannot reuse the reserved \
                         built-in provider name '{name}'; choose a distinct name"
                    )));
                }
                if p.base_url.is_none() {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "provider '{name}': kind=\"openai-compatible\" requires base_url"
                    )));
                }
                if p.default_model.as_deref().is_none_or(|m| m.is_empty()) {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "provider '{name}': kind=\"openai-compatible\" requires default_model"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_openai_compatible_without_base_url() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "oracle".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("base_url"));
    }

    #[test]
    fn validate_accepts_openai_compatible_with_base_url() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "oracle".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                default_model: Some("llama3".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_rejects_openai_compatible_reserved_name() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "openai".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                default_model: Some("gpt-4".into()),
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("openai"));
    }

    #[test]
    fn validate_rejects_openai_compatible_without_default_model() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "my-llm".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err();
        assert!(err.to_string().contains("default_model"));
    }
}
