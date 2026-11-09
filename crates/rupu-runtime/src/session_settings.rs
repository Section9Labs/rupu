//! Session-owned overrides, separate from the worker's conversation record.
//! Atomic replacement and revision checks prevent stale editors or worker
//! bookkeeping from losing settings. Workers take one snapshot per turn.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSettings {
    #[serde(default)]
    pub revision: u64,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// None inherits the frozen agent grant; [] explicitly declares no tools (ambient runtime grants still apply).
    pub tools: Option<Vec<String>>,
    /// None inherits the creation snapshot; Some(empty) disables MCP tools.
    /// Changed only through the explicit MCP selection endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<crate::session_mcp::FrozenSelection>,
}
/// Conservative adapter-supported controls. Unknown/custom models retain provider defaults.
pub fn reasoning_levels(provider: &str, model: &str) -> &'static [&'static str] {
    if matches!(provider, "openai" | "openai-codex")
        && rupu_providers::openai_codex::model_supports_reasoning(model)
    {
        &["auto", "low", "medium", "high"]
    } else if provider == "anthropic"
        && rupu_providers::anthropic::model_supports_adaptive_thinking(model)
    {
        &["auto", "low", "medium", "high", "max"]
    } else {
        &[]
    }
}
impl SessionSettings {
    pub fn read(dir: &Path) -> anyhow::Result<Self> {
        match fs::read(dir.join("settings.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_with_catalog(&rupu_tools::ToolCatalog::builtin())
    }
    /// Validate against the exact snapshot a caller will use for assembly.
    /// Existing HTTP/settings paths continue to use the builtin-only contract.
    pub fn validate_with_catalog(&self, catalog: &rupu_tools::ToolCatalog) -> anyhow::Result<()> {
        for (name, value) in [("provider", &self.provider), ("model", &self.model)] {
            if let Some(value) = value {
                anyhow::ensure!(
                    !value.trim().is_empty()
                        && value == value.trim()
                        && value.len() <= 256
                        && !value.chars().any(char::is_control),
                    "{name} must be a nonempty identifier of at most 256 bytes"
                );
            }
        }
        anyhow::ensure!(
            self.provider.is_none() || self.model.is_some(),
            "a provider override requires an explicit model"
        );
        if let Some(selection) = &self.mcp {
            selection.validate()?;
        }
        if let Some(effort) = &self.effort {
            anyhow::ensure!(reasoning_levels(self.provider.as_deref().unwrap_or(""), self.model.as_deref().unwrap_or("")).contains(&effort.as_str()),
                "reasoning effort is unsupported for this provider/model; choose an advertised level or use the default");
        }
        if let Some(tools) = &self.tools {
            anyhow::ensure!(tools.len() <= 256, "too many tool grants");
            catalog.validate_names(tools)?;
        }
        Ok(())
    }
    pub fn save(&self, dir: &Path) -> anyhow::Result<Self> {
        self.validate()?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("settings.lock"))?;
        lock.lock_exclusive()?;
        let current = Self::read(dir)?;
        anyhow::ensure!(
            current.revision == self.revision,
            "settings changed; reload before saving"
        );
        let mut saved = self.clone();
        saved.revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("settings revision overflow"))?;
        let temp = dir.join(format!("settings-{}.tmp", ulid::Ulid::new()));
        let result = (|| -> anyhow::Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?;
            file.write_all(&serde_json::to_vec_pretty(&saved)?)?;
            file.sync_all()?;
            fs::rename(&temp, dir.join("settings.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result?;
        Ok(saved)
    }
    /// Apply to a per-turn agent, never the saved agent definition.
    pub fn apply(&self, agent: &mut rupu_agent::AgentSpec) -> bool {
        let provider_changed = self
            .provider
            .as_ref()
            .is_some_and(|p| agent.provider.as_ref() != Some(p));
        let model_changed = self
            .model
            .as_ref()
            .is_some_and(|m| agent.model.as_ref() != Some(m));
        if let Some(provider) = &self.provider {
            agent.provider = Some(provider.clone());
        }
        if let Some(model) = &self.model {
            agent.model = Some(model.clone());
        }
        if let Some(tools) = &self.tools {
            agent.tools = Some(tools.clone());
        }
        if provider_changed {
            agent.auth = None;
            agent.anthropic_oauth_prefix = None;
            agent.anthropic_prompt_cache = None;
            agent.anthropic_task_budget = None;
            agent.anthropic_context_management = None;
            agent.anthropic_speed = None;
            agent.output_format = None;
            agent.output_schema = None;
        }
        if provider_changed || model_changed {
            agent.max_tokens = None;
            agent.context_window_tokens = None;
            agent.effort = None;
            agent.context_window = None;
            // Daybreak cyber is openai-codex-only and model-specific.
            agent.cyber = None;
        }
        if let Some(effort) = &self.effort {
            // validate() checks the advertised values before any save or launch.
            if let Ok(level) = serde_json::from_value(serde_json::Value::String(effort.clone())) {
                agent.effort = Some(level);
            }
        }
        if self.provider.is_some() || self.model.is_some() {
            // An override must not quietly fall back to the definition's model.
            // Pinning the original model is still an explicit selection.
            agent.fallbacks = Some(Vec::new());
        }
        provider_changed || model_changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reasoning_override_is_validated_persisted_and_applied_per_turn() {
        let dir = tempfile::tempdir().unwrap();
        let mut agent = rupu_agent::AgentSpec::parse(
            "---\nname: reviewer\nprovider: openai\nmodel: gpt-6.1-sol\n---\nReview.",
        )
        .unwrap();
        let original = agent.clone();
        let settings = SessionSettings {
            provider: Some("openai".into()),
            model: Some("gpt-6.1-sol".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        let saved = settings.save(dir.path()).unwrap();
        assert_eq!(
            SessionSettings::read(dir.path()).unwrap().effort.as_deref(),
            Some("high")
        );
        saved.apply(&mut agent);
        assert_eq!(
            agent.effort,
            Some(rupu_providers::model_tier::ThinkingLevel::High)
        );
        assert_eq!(original.effort, None);
        let reset = SessionSettings {
            revision: saved.revision,
            effort: Some("auto".into()),
            ..settings.clone()
        };
        reset.save(dir.path()).unwrap().apply(&mut agent);
        assert_eq!(
            agent.effort,
            Some(rupu_providers::model_tier::ThinkingLevel::Auto)
        );
        assert!(SessionSettings {
            model: Some("gpt-4.1".into()),
            ..settings.clone()
        }
        .validate()
        .is_err());
        assert!(SessionSettings {
            effort: Some("invented".into()),
            ..settings
        }
        .validate()
        .is_err());
        assert!(reasoning_levels("unrecognized", "gpt-6.1-sol").is_empty());
    }
    #[test]
    fn reasoning_and_mcp_settings_share_revision_without_dropping_either() {
        let dir = tempfile::tempdir().unwrap();
        let initial = SessionSettings {
            provider: Some("openai".into()),
            model: Some("gpt-6.1-sol".into()),
            effort: Some("high".into()),
            tools: Some(vec![]),
            ..Default::default()
        };
        let first = initial.save(dir.path()).unwrap();
        let selected = SessionSettings {
            mcp: Some(crate::session_mcp::FrozenSelection::default()),
            ..first.clone()
        }
        .save(dir.path())
        .unwrap();
        assert_eq!(selected.effort.as_deref(), Some("high"));
        assert_eq!(selected.tools, Some(vec![]));
        assert!(first.save(dir.path()).is_err());
        let changed = SessionSettings {
            effort: Some("low".into()),
            ..selected.clone()
        }
        .save(dir.path())
        .unwrap();
        let read = SessionSettings::read(dir.path()).unwrap();
        assert_eq!(read, changed);
        assert_eq!(read.mcp, selected.mcp);
        assert_eq!(read.revision, selected.revision + 1);
        assert!(SessionSettings {
            effort: Some("invalid".into()),
            ..read
        }
        .save(dir.path())
        .is_err());
        assert_eq!(SessionSettings::read(dir.path()).unwrap(), changed);
    }
    #[test]
    fn settings_are_atomic_revisioned_and_do_not_touch_history() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("session.json"), b"conversation").unwrap();
        let first = SessionSettings {
            model: Some("new-model".into()),
            tools: Some(vec![]),
            ..Default::default()
        };
        let saved = first.save(dir.path()).unwrap();
        assert_eq!(saved.revision, 1);
        assert!(first.save(dir.path()).is_err());
        assert_eq!(SessionSettings::read(dir.path()).unwrap(), saved);
        assert_eq!(
            fs::read(dir.path().join("session.json")).unwrap(),
            b"conversation"
        );
        let reset = SessionSettings {
            revision: 1,
            ..Default::default()
        }
        .save(dir.path())
        .unwrap();
        assert_eq!(reset.tools, None);
    }
    #[test]
    fn settings_reject_invalid_configuration() {
        assert!(SessionSettings {
            provider: Some("openai".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(SessionSettings {
            model: Some("\n".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(SessionSettings {
            tools: Some(vec!["invented_tool".into()]),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn explicit_original_model_disables_fallbacks_but_tools_only_preserves_them() {
        let original = rupu_agent::AgentSpec::parse(
            "---\nname: reviewer\nprovider: anthropic\nmodel: original\nmaxTokens: 2048\nfallbacks:\n  - model: fallback\n---\nReview carefully.",
        )
        .unwrap();
        let mut effective = original.clone();
        let settings = SessionSettings {
            model: original.model.clone(),
            ..Default::default()
        };
        assert!(
            !settings.apply(&mut effective),
            "the model identity is unchanged"
        );
        assert_eq!(effective.fallbacks, Some(Vec::new()));
        assert_eq!(effective.max_tokens, original.max_tokens);

        let mut tools_only = original.clone();
        assert!(!SessionSettings {
            tools: Some(vec![]),
            ..Default::default()
        }
        .apply(&mut tools_only));
        assert_eq!(tools_only.fallbacks, original.fallbacks);
    }
    #[test]
    fn override_changes_only_the_turn_agent_and_clears_incompatible_pins() {
        let original = rupu_agent::AgentSpec::parse("---\nname: reviewer\nprovider: anthropic\nmodel: original\ntools: [read_file]\n---\nReview carefully.").unwrap();
        let mut effective = original.clone();
        let settings = SessionSettings {
            provider: Some("openai".into()),
            model: Some("chosen".into()),
            tools: Some(vec!["agents.list".into()]),
            ..Default::default()
        };
        settings.validate().unwrap();
        assert!(settings.apply(&mut effective));
        assert_eq!(effective.provider.as_deref(), Some("openai"));
        assert_eq!(effective.model.as_deref(), Some("chosen"));
        assert_eq!(effective.tools, Some(vec!["agents.list".into()]));
        assert_eq!(effective.system_prompt, original.system_prompt);
        assert_eq!(effective.permission_mode, original.permission_mode);
        assert_eq!(original.model.as_deref(), Some("original"));
        assert!(!SessionSettings::default().apply(&mut original.clone()));
    }
}
