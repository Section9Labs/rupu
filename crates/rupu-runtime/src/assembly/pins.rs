//! The agent's frontmatter pins, as they apply to the run (moved here from
//! `rupu run` so every origin follows the same rule).

use rupu_agent::{AgentPins, AgentSpec};

use crate::model_limits::LimitOverrides;

/// The agent's model- and provider-specific settings, as they apply to a run
/// that may override its provider or model: a model other than the agent's
/// drops its limit pins and its model-specific Anthropic settings —
/// `contextWindow`, `anthropicSpeed`, `anthropicTaskBudget`,
/// `anthropicContextManagement` (each exists only on some models — the rule
/// a fallback hop follows; `effort` and `thinkingDisplay` are
/// provider-generic and stay); a provider other than the agent's drops its
/// `auth:`.
#[derive(Debug, Clone, Default)]
pub struct PinRule {
    pub pins: AgentPins,
    pub limits: LimitOverrides,
    pub auth: Option<rupu_providers::AuthMode>,
}

impl PinRule {
    pub fn for_run(spec: &AgentSpec, other_provider: bool, other_model: bool) -> Self {
        Self {
            pins: AgentPins {
                effort: spec.effort,
                thinking_display: spec.thinking_display,
                context_window: spec.context_window.filter(|_| !other_model),
                output_format: spec.output_format,
                output_schema: spec.output_schema.clone(),
                anthropic_task_budget: spec.anthropic_task_budget.filter(|_| !other_model),
                anthropic_context_management: spec
                    .anthropic_context_management
                    .filter(|_| !other_model),
                anthropic_speed: spec.anthropic_speed.filter(|_| !other_model),
            },
            limits: if other_model {
                LimitOverrides::default()
            } else {
                LimitOverrides::from_spec(spec)
            },
            auth: if other_provider { None } else { spec.auth },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_spec() -> rupu_agent::AgentSpec {
        rupu_agent::AgentSpec::parse(
            "---\nname: pinned\nprovider: anthropic\nmodel: claude-sonnet-4-6\nauth: api-key\n\
             contextWindow: 1m\nanthropicSpeed: fast\nanthropicTaskBudget: 40000\n\
             anthropicContextManagement: tool_clearing\neffort: high\nmaxTokens: 777\ncontextWindowTokens: 9000\n\
             ---\nhi\n",
        )
        .unwrap()
    }

    #[test]
    fn pins_follow_the_agents_own_model_and_provider() {
        let spec = pinned_spec();
        let own = PinRule::for_run(&spec, false, false);
        assert_eq!(own.limits.max_tokens, Some(777));
        assert_eq!(own.limits.context_window_tokens, Some(9000));
        assert!(own.pins.context_window.is_some());
        assert_eq!(
            own.pins.anthropic_speed,
            Some(rupu_providers::types::Speed::Fast)
        );
        assert_eq!(own.pins.anthropic_task_budget, Some(40_000));
        assert_eq!(
            own.pins.anthropic_context_management,
            Some(rupu_providers::types::ContextManagement::ToolClearing)
        );
        assert_eq!(own.auth, Some(rupu_providers::AuthMode::ApiKey));

        let other_model = PinRule::for_run(&spec, false, true);
        assert_eq!(other_model.limits.max_tokens, None);
        assert_eq!(other_model.limits.context_window_tokens, None);
        assert_eq!(other_model.pins.context_window, None);
        assert_eq!(
            other_model.pins.anthropic_speed, None,
            "fast mode exists only on some models"
        );
        assert_eq!(other_model.pins.anthropic_task_budget, None);
        assert_eq!(other_model.pins.anthropic_context_management, None);
        assert_eq!(
            other_model.pins.effort, spec.effort,
            "effort is provider-generic: it stays"
        );
        assert_eq!(other_model.auth, Some(rupu_providers::AuthMode::ApiKey));

        let other_provider = PinRule::for_run(&spec, true, false);
        assert_eq!(other_provider.auth, None);
        assert_eq!(other_provider.limits.max_tokens, Some(777));
        assert_eq!(
            other_provider.pins.anthropic_speed,
            Some(rupu_providers::types::Speed::Fast),
            "the same model on another provider keeps its pins"
        );
        assert_eq!(other_provider.pins.anthropic_task_budget, Some(40_000));
        assert!(other_provider.pins.anthropic_context_management.is_some());
    }
}
