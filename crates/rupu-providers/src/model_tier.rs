use serde::{Deserialize, Serialize};

use crate::provider_id::ProviderId;

/// Unified reasoning/effort level across all providers.
/// Each provider translates this to its native format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Max,
}

/// Abstract model tier for provider-agnostic selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Fast,
    Default,
    DeepThink,
    Code,
    Local,
}

/// Metadata for a specific model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: ProviderId,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub supports_reasoning: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_streaming: bool,
}

/// Maps model tiers to concrete model IDs for a specific provider.
#[derive(Debug, Clone)]
pub struct ModelMap {
    provider: ProviderId,
    fast: &'static str,
    default: &'static str,
    deep_think: &'static str,
    code: &'static str,
}

impl ModelMap {
    /// Get the default model map for a provider.
    pub fn for_provider(id: ProviderId) -> Self {
        match id {
            ProviderId::Anthropic => Self {
                provider: id,
                fast: "claude-haiku-4-5",
                default: "claude-sonnet-4-6",
                deep_think: "claude-opus-4-6",
                code: "claude-sonnet-4-6",
            },
            ProviderId::OpenaiCodex => Self {
                provider: id,
                fast: "gpt-4.1-mini",
                default: "gpt-4.1",
                deep_think: "o3",
                code: "codex-mini",
            },
            ProviderId::GoogleGeminiCli | ProviderId::GoogleAntigravity => Self {
                provider: id,
                fast: "gemini-2.0-flash",
                default: "gemini-2.5-pro",
                deep_think: "gemini-2.5-pro",
                code: "gemini-2.5-pro",
            },
            ProviderId::GithubCopilot => Self {
                provider: id,
                fast: "gpt-4.1-mini",
                default: "claude-sonnet-4-6",
                deep_think: "o3",
                code: "claude-sonnet-4-6",
            },
        }
    }

    /// Resolve a tier to a concrete model ID.
    pub fn resolve(&self, tier: ModelTier) -> Option<&str> {
        match tier {
            ModelTier::Fast => Some(self.fast),
            ModelTier::Default => Some(self.default),
            ModelTier::DeepThink => Some(self.deep_think),
            ModelTier::Code => Some(self.code),
            ModelTier::Local => None,
        }
    }

    pub fn provider(&self) -> ProviderId {
        self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_level_serde_roundtrip() {
        let levels = [
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
            ThinkingLevel::Max,
        ];
        for level in &levels {
            let json = serde_json::to_string(level).unwrap();
            let parsed: ThinkingLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*level, parsed);
        }
    }

    #[test]
    fn test_thinking_level_snake_case() {
        assert_eq!(
            serde_json::to_string(&ThinkingLevel::Minimal).unwrap(),
            "\"minimal\""
        );
        assert_eq!(
            serde_json::to_string(&ThinkingLevel::Max).unwrap(),
            "\"max\""
        );
    }

    #[test]
    fn test_model_tier_serde_roundtrip() {
        let tiers = [
            ModelTier::Fast,
            ModelTier::Default,
            ModelTier::DeepThink,
            ModelTier::Code,
            ModelTier::Local,
        ];
        for tier in &tiers {
            let json = serde_json::to_string(tier).unwrap();
            let parsed: ModelTier = serde_json::from_str(&json).unwrap();
            assert_eq!(*tier, parsed);
        }
    }

    #[test]
    fn test_model_tier_deep_think_serde_name() {
        let json = serde_json::to_string(&ModelTier::DeepThink).unwrap();
        assert_eq!(json, "\"deep_think\"");
    }

    #[test]
    fn test_model_map_anthropic() {
        let map = ModelMap::for_provider(ProviderId::Anthropic);
        assert_eq!(map.resolve(ModelTier::Fast), Some("claude-haiku-4-5"));
        assert_eq!(map.resolve(ModelTier::Default), Some("claude-sonnet-4-6"));
        assert_eq!(map.resolve(ModelTier::DeepThink), Some("claude-opus-4-6"));
        assert_eq!(map.resolve(ModelTier::Code), Some("claude-sonnet-4-6"));
        assert_eq!(map.resolve(ModelTier::Local), None);
    }

    #[test]
    fn test_model_map_openai() {
        let map = ModelMap::for_provider(ProviderId::OpenaiCodex);
        assert_eq!(map.resolve(ModelTier::Fast), Some("gpt-4.1-mini"));
        assert_eq!(map.resolve(ModelTier::DeepThink), Some("o3"));
        assert_eq!(map.resolve(ModelTier::Code), Some("codex-mini"));
    }

    #[test]
    fn test_model_map_google() {
        let map = ModelMap::for_provider(ProviderId::GoogleGeminiCli);
        assert_eq!(map.resolve(ModelTier::Fast), Some("gemini-2.0-flash"));
        assert_eq!(map.resolve(ModelTier::Default), Some("gemini-2.5-pro"));
    }

    #[test]
    fn test_model_map_copilot() {
        let map = ModelMap::for_provider(ProviderId::GithubCopilot);
        assert_eq!(map.resolve(ModelTier::Default), Some("claude-sonnet-4-6"));
        assert_eq!(map.resolve(ModelTier::DeepThink), Some("o3"));
    }

    #[test]
    fn test_model_map_gemini_variants_same() {
        let cli = ModelMap::for_provider(ProviderId::GoogleGeminiCli);
        let anti = ModelMap::for_provider(ProviderId::GoogleAntigravity);
        assert_eq!(
            cli.resolve(ModelTier::Default),
            anti.resolve(ModelTier::Default)
        );
    }

    #[test]
    fn test_model_info_serde() {
        let info = ModelInfo {
            id: "claude-sonnet-4-6-20250514".into(),
            provider: ProviderId::Anthropic,
            context_window: 200_000,
            max_output_tokens: 64_000,
            supports_reasoning: true,
            supports_tools: true,
            supports_vision: true,
            supports_streaming: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        let parsed: ModelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "claude-sonnet-4-6-20250514");
        assert_eq!(parsed.provider, ProviderId::Anthropic);
        assert!(parsed.supports_reasoning);
    }
}
