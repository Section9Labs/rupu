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
    /// Named provider account → vendor kind, e.g. `openai-oracle` →
    /// `openai`, taken from `[providers.<name>].kind`. Runs record the
    /// ACCOUNT name as their provider, which is not a vendor key, so the
    /// price lookup follows it here to reach the vendor's table. Not part
    /// of the `[pricing]` TOML: `Config::attach_provider_kinds` fills it
    /// after every load. Accounts without a `kind` are the vendor
    /// themselves and are absent from this map.
    #[serde(skip)]
    pub provider_kinds: BTreeMap<String, String>,
}

/// USD per million tokens for one model (or one agent's fallback
/// price). `cached_input_per_mtok` and `cache_write_per_mtok` are
/// optional — some vendors don't charge separately for cache hits or
/// writes, so leaving either absent makes the cost calculator treat those
/// tokens as fully-priced input.
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
    /// USD per million cache-WRITE tokens (prompt-cache creation). When
    /// `None`, cache writes are billed at the full `input_per_mtok` rate.
    /// Anthropic is the vendor that bills writes at a premium (1.25x input
    /// for the 5-minute TTL); OpenAI and Gemini bill no separate write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<f64>,
}

impl ModelPricing {
    /// Compute the USD cost for one (input, output, cached, cache-write)
    /// tuple.
    ///
    /// `input_tokens` is the WHOLE prompt, and `cached_tokens` (cache
    /// reads) and `cache_write_tokens` (cache creations) are two disjoint
    /// SUBSETS of it — the convention all three major vendors use
    /// (Anthropic prompt caching, OpenAI `cached_tokens`, Gemini
    /// context-cache reads). Anthropic's wire format reports the three
    /// counts separately, but the provider layer normalizes them upstream
    /// so `input_tokens` already includes reads and writes (see
    /// `rupu_providers::anthropic::AnthropicWireUsage`). Uncached input is
    /// therefore `input - cached - cache_write`, and the formula is:
    ///
    /// ```text
    /// cost = (input - cached - cache_write) * input_per_mtok       / 1e6
    ///      + cached                         * cached_per_mtok      / 1e6
    ///      + cache_write                    * cache_write_per_mtok / 1e6
    ///      + output                         * output_per_mtok      / 1e6
    /// ```
    ///
    /// When `cached_input_per_mtok` or `cache_write_per_mtok` is unset,
    /// those tokens fall back to the full input rate: an over-estimate for
    /// unset reads, and the exact bill for a vendor with no separate write
    /// charge. (A user pricing Anthropic by hand should set both; the
    /// built-in Anthropic entries do.)
    ///
    /// Malformed reports are clamped, never negative: reads claim their
    /// share of `input` first (capped at `input`), writes then take at most
    /// what remains, and the uncached remainder is whatever is left.
    ///
    /// **`output_tokens` must already be the BILLABLE output** (I-48).
    ///
    /// Gemini reports "thinking" tokens (`thoughtsTokenCount`) *outside*
    /// `candidatesTokenCount`, but Google bills them at the output rate, and
    /// `gemini-2.5-pro` thinks by default — so ignoring them under-bills
    /// every default-config Gemini run, silently and with no `cost_partial`
    /// marker. The fold happens upstream, once, in `rupu-agent`'s runner
    /// (`billable_output_tokens = output_tokens + reasoning_tokens`), so the
    /// transcript's `output_tokens` and every downstream cost call already
    /// include reasoning.
    ///
    /// There is deliberately NO `cost_usd_with_reasoning` variant taking
    /// reasoning separately: every production caller sources `output_tokens`
    /// from the already-folded transcript, so such a function could only ever
    /// double-bill. The raw split stays visible on `Usage::reasoning_tokens`
    /// for anyone who needs it.
    pub fn cost_usd(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        cache_write_tokens: u64,
    ) -> f64 {
        let cached = cached_tokens.min(input_tokens);
        let write = cache_write_tokens.min(input_tokens - cached);
        let uncached = input_tokens - cached - write;
        let read_rate = self.cached_input_per_mtok.unwrap_or(self.input_per_mtok);
        let write_rate = self.cache_write_per_mtok.unwrap_or(self.input_per_mtok);
        (uncached as f64 * self.input_per_mtok
            + cached as f64 * read_rate
            + write as f64 * write_rate
            + output_tokens as f64 * self.output_per_mtok)
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
            cache_write_per_mtok: None,
        };
        assert_eq!(p.cost_usd(0, 0, 0, 0), 0.0);
    }

    #[test]
    fn cost_uncached_input_full_rate() {
        // 1M input @ $3/Mtok + 0 output = $3.
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: Some(0.30),
            cache_write_per_mtok: None,
        };
        let c = p.cost_usd(1_000_000, 0, 0, 0);
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
            cache_write_per_mtok: None,
        };
        let c = p.cost_usd(1_000_000, 100_000, 800_000, 0);
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
            cache_write_per_mtok: None,
        };
        let c = p.cost_usd(1_000_000, 100_000, 800_000, 0);
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
            cache_write_per_mtok: None,
        };
        // 100 input, 500 cached → uncached clamps to 0, cached clamps to 100.
        // cost = 100 * 0.30 / 1e6 = 0.00003
        let c = p.cost_usd(100, 0, 500, 0);
        assert!((c - 0.000_03).abs() < 1e-12, "got {c}");
    }

    #[test]
    fn cost_bills_reads_writes_and_uncached_separately() {
        let p = ModelPricing {
            input_per_mtok: 4.0,
            output_per_mtok: 20.0,
            cached_input_per_mtok: Some(0.20),
            cache_write_per_mtok: Some(5.0),
        };
        // 1M prompt = 700k read + 200k write + 100k uncached; 10k output.
        let c = p.cost_usd(1_000_000, 10_000, 700_000, 200_000);
        let want = 0.1 * 4.0 + 0.7 * 0.20 + 0.2 * 5.0 + 0.01 * 20.0;
        assert!((c - want).abs() < 1e-9, "{c} vs {want}");
    }

    #[test]
    fn write_rate_defaults_to_input_rate() {
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
        };
        assert!((p.cost_usd(1_000_000, 0, 0, 1_000_000) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn reads_plus_writes_exceeding_input_are_clamped() {
        let p = ModelPricing {
            input_per_mtok: 1.0,
            output_per_mtok: 0.0,
            cached_input_per_mtok: Some(0.1),
            cache_write_per_mtok: Some(1.25),
        };
        // Malformed report: read+write > input. Never negative uncached.
        let c = p.cost_usd(100, 0, 80, 80);
        assert!(c >= 0.0);
        // Reads claim their 80 first, writes get the remaining 20, and
        // nothing is left uncached: 80 * 0.1 + 20 * 1.25 = 33 (per 1e6).
        assert!((c - 33.0 / 1_000_000.0).abs() < 1e-15, "got {c}");
    }

    #[test]
    fn config_without_write_rate_still_deserializes_and_omits_it_on_write() {
        // User configs written before the cache-write field existed keep
        // parsing, and an unset write rate is not serialized back out.
        let p: ModelPricing =
            toml::from_str("input_per_mtok = 3.0\noutput_per_mtok = 15.0\n").unwrap();
        assert_eq!(p.cache_write_per_mtok, None);
        let out = toml::to_string(&p).unwrap();
        assert!(!out.contains("cache_write_per_mtok"), "{out}");

        let with: ModelPricing = toml::from_str(
            "input_per_mtok = 3.0\noutput_per_mtok = 15.0\ncache_write_per_mtok = 3.75\n",
        )
        .unwrap();
        assert_eq!(with.cache_write_per_mtok, Some(3.75));
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

        let gpt5 = cfg
            .models
            .get("openai")
            .and_then(|m| m.get("gpt-5"))
            .copied()
            .unwrap();
        assert_eq!(gpt5.cached_input_per_mtok, None);

        let agent = cfg.agents.get("security-reviewer").copied().unwrap();
        assert_eq!(agent.input_per_mtok, 3.0);
    }
}
