//! Per-model and per-agent USD pricing for token usage. Consumed by
//! `rupu usage` and `rupu workflow runs` to convert token counts into
//! a dollar figure.
//!
//! Layered into the global+project config under `[pricing]`. Three
//! lookup tiers (resolved in `rupu-cli::pricing`):
//!
//! 1. User-supplied `[pricing.<provider>."<model>"]` — wins.
//! 2. Built-in defaults table baked into the CLI for major models.
//! 3. User-supplied `[pricing.agents.<agent-name>]` — fallback when
//!    no model-level price is known. This is the hatch the user opens
//!    when they're running on a private / internal endpoint that has
//!    no public pricing.
//!
//! Prices are denominated in USD per million tokens — the format
//! Anthropic, OpenAI, and Google all publish on their pricing pages.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Top-level `[pricing]` section.
///
/// `models` is keyed first by provider id (`anthropic`, `openai`,
/// `google`, …) then by model id. `agents` is keyed by agent name and
/// is consulted only when no model-level entry matches the run's
/// `(provider, model)` pair.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PricingConfig {
    // `deny_unknown_fields` cannot coexist with `flatten` — provider
    // names are dynamic (`[pricing.anthropic.…]`, `[pricing.openai.…]`)
    // so we accept any top-level key under `[pricing]` and only reject
    // unknown fields inside the leaf `ModelPricing` structs.
    #[serde(flatten)]
    pub models: BTreeMap<String, BTreeMap<String, ModelPricing>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, ModelPricing>,
}

/// USD per million tokens for one model (or one agent's fallback
/// price). `cached_input_per_mtok` is optional — some vendors don't
/// charge separately for cache hits, so leaving it absent makes the
/// cost calculator treat cached tokens as fully-priced input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelPricing {
    /// USD per million input tokens.
    pub input_per_mtok: f64,
    /// USD per million output tokens.
    pub output_per_mtok: f64,
    /// USD per million cached-input tokens. When `None`, cached tokens
    /// are billed at the full `input_per_mtok` rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<f64>,
}

impl ModelPricing {
    /// Compute the USD cost for one (input, output, cached) tuple.
    ///
    /// `cached` is treated as a SUBSET of `input` — the convention all
    /// three major vendors use (Anthropic prompt caching, OpenAI
    /// `cached_tokens`, Gemini context-cache reads). Uncached input is
    /// `input - cached`, so the formula is:
    ///
    /// ```text
    /// cost = (input - cached) * input_per_mtok / 1e6
    ///      + cached           * cached_per_mtok / 1e6
    ///      + output           * output_per_mtok / 1e6
    /// ```
    ///
    /// When `cached_input_per_mtok` is unset, cached tokens fall back
    /// to the full input rate (still correct, just less favorable —
    /// over-estimates the bill rather than under-estimating).
    pub fn cost_usd(&self, input_tokens: u64, output_tokens: u64, cached_tokens: u64) -> f64 {
        let cached = cached_tokens.min(input_tokens) as f64;
        let uncached_input = (input_tokens.saturating_sub(cached_tokens)) as f64;
        let output = output_tokens as f64;
        let cached_rate = self
            .cached_input_per_mtok
            .unwrap_or(self.input_per_mtok);
        (uncached_input * self.input_per_mtok
            + cached * cached_rate
            + output * self.output_per_mtok)
            / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_zero_when_no_tokens() {
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.30),
        };
        assert_eq!(p.cost_usd(0, 0, 0), 0.0);
    }

    #[test]
    fn cost_uncached_input_full_rate() {
        // 1M input @ $3/Mtok + 0 output = $3.
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.30),
        };
        let c = p.cost_usd(1_000_000, 0, 0);
        assert!((c - 3.0).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn cost_cached_subset_uses_cached_rate() {
        // 1M total input, 800k of which is cached:
        // uncached = 200k @ $3/Mtok = $0.60
        // cached   = 800k @ $0.30/Mtok = $0.24
        // output   = 100k @ $15/Mtok = $1.50
        // total    = $2.34
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.30),
        };
        let c = p.cost_usd(1_000_000, 100_000, 800_000);
        assert!((c - 2.34).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn cost_cached_falls_back_to_input_rate_when_unset() {
        // Same call, cached_input_per_mtok unset → cached billed
        // at full input rate. 1M input @ $3 + 100k output @ $15 = $4.50.
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: None,
        };
        let c = p.cost_usd(1_000_000, 100_000, 800_000);
        assert!((c - 4.50).abs() < 1e-9, "got {c}");
    }

    #[test]
    fn cost_clamps_cached_above_input() {
        // Garbled input with cached > input shouldn't go negative on
        // uncached — clamp instead. (Defensive; real transcripts
        // shouldn't produce this.)
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 0.0,
            cached_input_per_mtok: Some(0.30),
        };
        // 100 input, 500 cached → uncached clamps to 0, cached clamps to 100.
        // cost = 100 * 0.30 / 1e6 = 0.00003
        let c = p.cost_usd(100, 0, 500);
        assert!((c - 0.000_03).abs() < 1e-12, "got {c}");
    }

    #[test]
    fn deserializes_pricing_section_with_models_and_agents() {
        let toml_text = r#"
[anthropic."claude-sonnet-4-6"]
input_per_mtok = 3.0
output_per_mtok = 15.0
cached_input_per_mtok = 0.30

[openai."gpt-5"]
input_per_mtok = 1.25
output_per_mtok = 10.0

[agents.security-reviewer]
input_per_mtok = 3.0
output_per_mtok = 15.0
"#;
        let cfg: PricingConfig = toml::from_str(toml_text).unwrap();
        let sonnet = cfg
            .models
            .get("anthropic")
            .and_then(|m| m.get("claude-sonnet-4-6"))
            .copied()
            .unwrap();
        assert_eq!(sonnet.input_per_mtok, 3.0);
        assert_eq!(sonnet.output_per_mtok, 15.0);
        assert_eq!(sonnet.cached_input_per_mtok, Some(0.30));

        let gpt5 = cfg.models.get("openai").and_then(|m| m.get("gpt-5")).copied().unwrap();
        assert_eq!(gpt5.cached_input_per_mtok, None);

        let agent = cfg.agents.get("security-reviewer").copied().unwrap();
        assert_eq!(agent.input_per_mtok, 3.0);
    }
}
