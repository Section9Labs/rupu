//! Provider/model resolution precedence — the single source of truth shared by
//! `rupu run`, `rupu session`, and the workflow `StepFactory`.
//!
//! Regression cover for ISSUES.md I-1 (`default_provider` was dead config) and
//! I-2 (`default_model` was skipped on the workflow path).

use rupu_runtime::provider_factory::{resolve_model, resolve_provider_name};

// ---------------------------------------------------------------- provider

#[test]
fn provider_prefers_agent_frontmatter_over_config_default() {
    assert_eq!(
        resolve_provider_name(Some("openai"), Some("oracle")),
        "openai"
    );
}

/// I-1: the whole point. A config-declared `default_provider` must actually be
/// honored when the agent pins none.
#[test]
fn provider_falls_back_to_config_default() {
    assert_eq!(resolve_provider_name(None, Some("oracle")), "oracle");
}

#[test]
fn provider_falls_back_to_anthropic_when_nothing_set() {
    assert_eq!(resolve_provider_name(None, None), "anthropic");
}

// ------------------------------------------------------------------- model

#[test]
fn model_prefers_agent_frontmatter_over_everything() {
    assert_eq!(
        resolve_model(Some("claude-opus-4-7"), Some("gpt-5"), Some("glm-5.2")),
        "claude-opus-4-7"
    );
}

/// I-2: the workflow path previously skipped `cfg.default_model` entirely.
#[test]
fn model_falls_back_to_config_default() {
    assert_eq!(resolve_model(None, Some("gpt-5"), None), "gpt-5");
}

/// Preserves the pre-existing `rupu run` order: the global `default_model` is
/// consulted before the provider-scoped one. See ISSUES.md I-3 — this ordering
/// is questionable but is deliberately locked here as current behavior so a
/// future change to it is a conscious, visible edit.
#[test]
fn model_config_default_wins_over_provider_scoped_default() {
    assert_eq!(resolve_model(None, Some("gpt-5"), Some("glm-5.2")), "gpt-5");
}

#[test]
fn model_falls_back_to_provider_scoped_default() {
    assert_eq!(resolve_model(None, None, Some("glm-5.2")), "glm-5.2");
}

#[test]
fn model_falls_back_to_hardcoded_when_nothing_set() {
    assert_eq!(resolve_model(None, None, None), "claude-sonnet-4-6");
}

// ------------------------------------------------------- empty-string guards

/// An empty `default_model = ""` in a `[providers.<name>]` block is what
/// `openai_compatible_params` produces when the key is absent
/// (`p.default_model.clone().unwrap_or_default()`), so an empty string must be
/// treated as "unset" rather than propagated as a model name.
#[test]
fn empty_provider_scoped_default_is_treated_as_unset() {
    assert_eq!(resolve_model(None, None, Some("")), "claude-sonnet-4-6");
}

#[test]
fn empty_agent_and_config_values_are_treated_as_unset() {
    assert_eq!(resolve_model(Some(""), Some(""), None), "claude-sonnet-4-6");
    assert_eq!(resolve_provider_name(Some(""), Some("")), "anthropic");
}
