//! Configuration types. See the Slice A spec for semantics.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::autoflow_config::AutoflowConfig;
use crate::netflow_config::NetflowConfig;
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
    pub netflow: NetflowConfig,
    #[serde(default)]
    pub update: crate::update_config::UpdateConfig,
    #[serde(default)]
    pub workflow: crate::policy_config::WorkflowConfig,
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
    /// `[ui.cp]` — Control Plane web UI preferences.
    #[serde(default)]
    pub cp: UiCpConfig,
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

/// `[ui.cp]` — Control Plane web UI preferences.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiCpConfig {
    /// Web shell generation served by `rupu cp serve`: `"v1"` (default,
    /// classic sidebar shell) or `"v2"` (Shell v2 redesign). Any other
    /// value is treated as `"v1"` by the consumer.
    pub shell: Option<String>,
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

/// Vendor kinds a `[providers.<name>]` entry may declare, in addition to
/// `"openai-compatible"`. Kept in lockstep with
/// `rupu_auth::backend::ProviderId::from_vendor_str` — the full vendor
/// list `rupu auth login --kind` accepts, LLM providers and SCM/issue
/// connectors alike (`declare_account_in_config` in `rupu-cli` writes a
/// `[providers.<account>]` entry for any of them, and that write is
/// schema-validated through this list, so a gap here would reject a
/// legitimately declared second account for an SCM connector).
///
/// Deliberately broader than
/// `rupu_runtime::provider_factory::is_builtin_provider`, which is
/// LLM-only by design for its own (narrower) callers — see that
/// function's doc comment.
const BUILTIN_PROVIDER_KINDS: &[&str] = &[
    "anthropic",
    "openai",
    "openai_codex",
    "codex",
    "gemini",
    "google_gemini",
    "copilot",
    "github_copilot",
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
        if let Some(d) = self.scm.default.as_ref() {
            if d.owner.is_some() {
                tracing::warn!(
                    key = "scm.default.owner",
                    "config.toml still declares `[scm.default].owner`. It has never \
                     been read by anything — every command that resolves a single \
                     target repo requires it explicit, to avoid silently targeting \
                     the wrong repo — and is now formally deprecated: it is accepted \
                     as a no-op so the rest of your config still loads. Delete \
                     `[scm.default].owner`; pass `--repo <platform>:<owner>/<repo>` \
                     explicitly, or set `[issues.default].project` (paired with \
                     `.tracker`) if you want `rupu issues list` to default a repo. A \
                     future release will reject it."
                );
            }
            if d.repo.is_some() {
                tracing::warn!(
                    key = "scm.default.repo",
                    "config.toml still declares `[scm.default].repo`. It has never \
                     been read by anything — every command that resolves a single \
                     target repo requires it explicit, to avoid silently targeting \
                     the wrong repo — and is now formally deprecated: it is accepted \
                     as a no-op so the rest of your config still loads. Delete \
                     `[scm.default].repo`; pass `--repo <platform>:<owner>/<repo>` \
                     explicitly, or set `[issues.default].project` (paired with \
                     `.tracker`) if you want `rupu issues list` to default a repo. A \
                     future release will reject it."
                );
            }
        }
    }

    /// Validate cross-field invariants not expressible in serde.
    ///
    /// Also emits [`Config::warn_deprecated_keys`] — this is the one function
    /// every load path already calls.
    pub fn validate(&self) -> Result<(), crate::layer::LayerError> {
        self.warn_deprecated_keys();
        for (name, p) in &self.providers {
            match p.kind.as_deref() {
                // No kind: the account name itself is the vendor
                // (design spec §3.1). Nothing to validate here.
                None => {}
                Some("openai-compatible") => {
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
                Some(k) if BUILTIN_PROVIDER_KINDS.contains(&k) => {}
                Some(k) => {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "provider '{name}': unknown kind \"{k}\"; expected \
                         \"openai-compatible\" or one of: {}",
                        BUILTIN_PROVIDER_KINDS.join(", ")
                    )));
                }
            }
        }
        self.validate_scm_rules()?;
        Ok(())
    }

    /// Validate `[[scm.rules]]` (design spec §6.3). Makes true the claim
    /// `ScmRule`'s doc comment already made — "a rule with both, or
    /// neither, is a config error" — which this function is the first
    /// code to actually enforce.
    ///
    /// A rule naming an account with no matching `[scm.<name>]` table is
    /// deliberately a WARNING, not an error: the account may be
    /// credential-only (a bare vendor name like `"github"`/`"gitlab"`
    /// that resolves via the account-name-is-the-vendor fallback and so
    /// never needs a config table at all — see `Registry::discover`).
    /// Erroring there would reject a rule that resolves and works fine
    /// at runtime; a rule with an outright typo just never matches
    /// anything, which is already surfaced structurally (as
    /// `AccountError::NoRuleMatched`'s candidate list omitting the
    /// account) rather than needing a load-time hard stop.
    fn validate_scm_rules(&self) -> Result<(), crate::layer::LayerError> {
        for (idx, rule) in self.scm.rules.iter().enumerate() {
            match (&rule.owner, &rule.path) {
                (Some(_), Some(_)) => {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "[[scm.rules]] entry {idx} (account \"{}\"): both `owner` and `path` \
                         are set; exactly one is required",
                        rule.account
                    )));
                }
                (None, None) => {
                    return Err(crate::layer::LayerError::Invalid(format!(
                        "[[scm.rules]] entry {idx} (account \"{}\"): neither `owner` nor `path` \
                         is set; exactly one is required",
                        rule.account
                    )));
                }
                (Some(_), None) | (None, Some(_)) => {}
            }
            if !self.scm.platforms.contains_key(&rule.account) {
                tracing::warn!(
                    key = "scm.rules",
                    index = idx,
                    account = %rule.account,
                    "config.toml declares a `[[scm.rules]]` entry pointing at an account with \
                     no matching `[scm.<name>]` table. This is fine if the account is \
                     credential-only (e.g. the bare vendor name \"github\"/\"gitlab\", which \
                     needs no config table); if it's a typo the rule will simply never match and \
                     requests will fall through to the next tier (or to the ambiguity error). \
                     Run `rupu scm accounts` to see configured accounts."
                );
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

    #[test]
    fn ui_cp_shell_parses() {
        let cfg: Config = toml::from_str("[ui.cp]\nshell = \"v2\"\n").unwrap();
        assert_eq!(cfg.ui.cp.shell.as_deref(), Some("v2"));
    }

    #[test]
    fn ui_cp_defaults_to_absent() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.ui.cp.shell, None);
    }

    #[test]
    fn ui_cp_rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[ui.cp]\nshel = \"v2\"\n").is_err());
    }

    #[test]
    fn validate_accepts_builtin_kind() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic-work".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }

    /// A built-in kind needs no base_url / default_model — those are
    /// openai-compatible requirements only.
    #[test]
    fn validate_builtin_kind_does_not_require_base_url() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "openai-personal".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }

    /// `BUILTIN_PROVIDER_KINDS` grew from 9 to 13 entries when the four SCM
    /// kinds (`github`/`gitlab`/`linear`/`jira`) were added, but nothing
    /// pinned that they actually validate — the same shape of gap that let
    /// an earlier regression ship green. Pin all four: dropping any one of
    /// `gitlab`/`linear`/`jira` from `BUILTIN_PROVIDER_KINDS` while leaving
    /// only `github` covered would still pass a single-kind test, which is
    /// exactly the gap this test exists to close.
    #[test]
    fn validate_accepts_scm_kinds() {
        for kind in ["github", "gitlab", "linear", "jira"] {
            let mut cfg = Config::default();
            cfg.providers.insert(
                format!("{kind}-work"),
                crate::provider_config::ProviderConfig {
                    kind: Some(kind.to_string()),
                    ..Default::default()
                },
            );
            assert!(cfg.validate().is_ok(), "kind \"{kind}\" should validate");
        }
    }

    #[test]
    fn validate_rejects_unknown_kind() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "weird".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("not-a-vendor".into()),
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("not-a-vendor"), "got: {err}");
        assert!(
            err.contains("openai-compatible"),
            "should list valid kinds, got: {err}"
        );
    }

    /// The reserved-name rule stays scoped to openai-compatible: you may
    /// not name a custom endpoint "anthropic", but an account named
    /// "anthropic" with kind "anthropic" is the legacy default and fine.
    #[test]
    fn validate_still_rejects_reserved_name_for_openai_compatible() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("openai-compatible".into()),
                base_url: Some("http://host:8080".into()),
                default_model: Some("m".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_account_named_after_its_own_vendor() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "anthropic".into(),
            crate::provider_config::ProviderConfig {
                kind: Some("anthropic".into()),
                ..Default::default()
            },
        );
        assert!(cfg.validate().is_ok());
    }

    fn owner_rule(owner: &str, account: &str) -> crate::scm_config::ScmRule {
        crate::scm_config::ScmRule {
            owner: Some(owner.into()),
            path: None,
            account: account.into(),
        }
    }

    #[test]
    fn validate_rejects_scm_rule_with_both_owner_and_path() {
        let mut cfg = Config::default();
        cfg.scm.rules.push(crate::scm_config::ScmRule {
            owner: Some("acme/*".into()),
            path: Some("~/Code/work/*".into()),
            account: "gh-work".into(),
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("both `owner` and `path`"), "got: {err}");
        assert!(err.contains("gh-work"), "got: {err}");
    }

    #[test]
    fn validate_rejects_scm_rule_with_neither_owner_nor_path() {
        let mut cfg = Config::default();
        cfg.scm.rules.push(crate::scm_config::ScmRule {
            owner: None,
            path: None,
            account: "gh-work".into(),
        });
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("neither `owner` nor `path`"), "got: {err}");
    }

    /// The happy path: exactly-one-of-owner-or-path rules validate fine,
    /// even when the named account has no `[scm.<name>]` table — that's
    /// a warning (unit-tested separately via `should_warn`-style pure
    /// helpers elsewhere in the crate; this crate has no tracing-capture
    /// harness, so the assertion here is simply "does not error").
    #[test]
    fn validate_accepts_owner_only_and_path_only_rules_even_for_undeclared_accounts() {
        let mut cfg = Config::default();
        cfg.scm.rules.push(owner_rule("acme/*", "gh-work"));
        cfg.scm.rules.push(crate::scm_config::ScmRule {
            owner: None,
            path: Some("~/Code/work/*".into()),
            account: "gh-work".into(),
        });
        assert!(cfg.validate().is_ok());
    }

    /// A rule naming a declared `[scm.<name>]` account validates with no
    /// warning-worthy condition at all — the common, fully-declared case.
    #[test]
    fn validate_accepts_scm_rule_naming_a_declared_account() {
        let mut cfg = Config::default();
        cfg.scm.platforms.insert(
            "gh-work".into(),
            crate::scm_config::ScmPlatformConfig {
                kind: Some("github".into()),
                ..Default::default()
            },
        );
        cfg.scm.rules.push(owner_rule("acme/*", "gh-work"));
        assert!(cfg.validate().is_ok());
    }
}
