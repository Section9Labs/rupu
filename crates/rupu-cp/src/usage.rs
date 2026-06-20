//! Token + dollar-cost aggregation for the control plane.
//!
//! Pure read-side composition of `rupu_transcript::aggregate` (tokens per
//! provider/model/agent) and `rupu_config::pricing` (USD price lookup +
//! `ModelPricing::cost_usd`). Cost is an estimate: when a model has no
//! resolvable price we report tokens with `priced = false` and never
//! fabricate a dollar figure.

use rupu_config::PricingConfig;
use rupu_transcript::UsageRow;
use serde::Serialize;

/// Token + cost summary for a run, or any rollup of runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub total_tokens: u64,
    /// `None` when no contributing row was priced. Otherwise the sum of the
    /// priced rows' cost (a partial total when `priced == false`).
    pub cost_usd: Option<f64>,
    /// `false` when at least one contributing model lacked a price.
    pub priced: bool,
    /// Distinct runs contributing (for rollups).
    pub runs: u64,
}

/// Fold token rows into a single summary, pricing each row.
pub fn summarize(rows: &[UsageRow], pricing: &PricingConfig) -> UsageSummary {
    let mut out = UsageSummary {
        priced: true,
        ..UsageSummary::default()
    };
    let mut any_priced = false;
    let mut cost_acc = 0.0_f64;
    for row in rows {
        out.input_tokens += row.input_tokens;
        out.output_tokens += row.output_tokens;
        out.cached_tokens += row.cached_tokens;
        out.runs += row.runs;
        match rupu_config::pricing::lookup(pricing, &row.provider, &row.model, &row.agent) {
            Some(price) => {
                any_priced = true;
                cost_acc += price.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens);
            }
            None => out.priced = false,
        }
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_priced { Some(cost_acc) } else { None };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(provider: &str, model: &str, input: u64, output: u64, cached: u64) -> UsageRow {
        UsageRow {
            provider: provider.into(),
            model: model.into(),
            agent: "a".into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
            runs: 1,
        }
    }

    #[test]
    fn summarize_prices_a_known_model() {
        let pricing = PricingConfig::default();
        let s = summarize(
            &[row(
                "anthropic",
                "claude-sonnet-4-6",
                1_000_000,
                1_000_000,
                0,
            )],
            &pricing,
        );
        assert_eq!(s.input_tokens, 1_000_000);
        assert_eq!(s.output_tokens, 1_000_000);
        assert_eq!(s.total_tokens, 2_000_000);
        assert!(s.priced);
        // 1M*3.0 + 1M*15.0 = $18.00
        assert!((s.cost_usd.unwrap() - 18.0).abs() < 1e-9);
    }

    #[test]
    fn summarize_unpriced_model_yields_no_cost() {
        let pricing = PricingConfig::default();
        let s = summarize(
            &[row("internal-vllm", "llama-3-70b", 1000, 1000, 0)],
            &pricing,
        );
        assert_eq!(s.total_tokens, 2000);
        assert!(!s.priced);
        assert_eq!(s.cost_usd, None);
    }

    #[test]
    fn summarize_mixed_is_partial() {
        let pricing = PricingConfig::default();
        let s = summarize(
            &[
                row("anthropic", "claude-sonnet-4-6", 1_000_000, 0, 0), // $3.00
                row("internal-vllm", "llama-3-70b", 1000, 1000, 0),     // unpriced
            ],
            &pricing,
        );
        assert!(!s.priced);
        assert!((s.cost_usd.unwrap() - 3.0).abs() < 1e-9); // partial: priced rows only
        assert_eq!(s.total_tokens, 1_000_000 + 2000);
    }

    #[test]
    fn summarize_empty_is_zero_priced() {
        let s = summarize(&[], &PricingConfig::default());
        assert_eq!(s.total_tokens, 0);
        assert!(s.priced);
        assert_eq!(s.cost_usd, None);
    }
}
