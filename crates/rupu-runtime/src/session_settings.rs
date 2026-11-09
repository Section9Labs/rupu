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
    /// None inherits the frozen agent grant; [] explicitly declares no tools (ambient runtime grants still apply).
    pub tools: Option<Vec<String>>,
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
        if let Some(tools) = &self.tools {
            anyhow::ensure!(tools.len() <= 256, "too many tool grants");
            rupu_tools::ToolCatalog::builtin().validate_names(tools)?;
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
