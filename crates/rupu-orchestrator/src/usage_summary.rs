//! A run's priced usage: the fold [`crate::usage_index`] keeps, summed and
//! priced. **A run's usage is ledger-first** (spec 2026-09-29 §4): the sum of
//! its usage-ledger rows (`<runs>/<id>/usage.jsonl`, one row per LLM call
//! made anywhere in the run, dispatch children included), plus — for every
//! known transcript (step results ∪ live events ∪ dispatched sub-runs) that
//! has no ledger row — a fold of that transcript file, resolved through the
//! host mirror rules ([`crate::transcript_paths`]). A transcript that should
//! exist but can't be read anywhere marks the result `partial`.
//!
//! [`run_usage`] / [`transcripts_usage`] are the index's entry points, and
//! [`summarize_run`] / [`run_metrics`] / [`run_transcript_paths`] are built
//! on it. The control plane (`rupu_cp::usage`, which adds its async and
//! per-customer pricing helpers on top) and the autoflow views
//! (`rupu_autoflow`) read the same process-wide index through here.
//!
//! Pricing is `rupu_config::pricing` (USD price lookup +
//! `ModelPricing::cost_usd`), applied here, never stored. Cost is an
//! estimate: when a model has no resolvable price we report tokens with
//! `priced = false` and never fabricate a dollar figure.

use crate::runs::RunStore;
use crate::usage_index::{RunUsage, UsageIndex};
use rupu_config::PricingConfig;
use rupu_transcript::UsageRow;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Token + cost summary for a run, or any rollup of runs.
///
/// `Deserialize` is derived so `/api/usage`'s host fan-out can parse a
/// remote CP's `summary` field straight off the wire and fold it into
/// [`rollup`] alongside the local summary — the same struct is both the
/// producer's and the aggregator's type, so there is no separate wire DTO to
/// drift out of sync.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cache reads — a subset of `input_tokens`.
    pub cached_tokens: u64,
    /// Cache writes — a subset of `input_tokens`, like `cached_tokens`.
    /// Defaulted so a remote CP that predates it still parses.
    #[serde(default)]
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    /// `None` when no contributing row was priced. Otherwise the sum of the
    /// priced rows' cost (a partial total when `priced == false`).
    pub cost_usd: Option<f64>,
    /// `false` when at least one contributing model lacked a price.
    pub priced: bool,
    /// Distinct runs contributing (for rollups).
    pub runs: u64,
    /// `true` when a transcript that should exist could not be read anywhere
    /// (a finished step's file is gone, or a mirrored run's transcript never
    /// arrived), so the totals are a lower bound. ORed by [`rollup`].
    #[serde(default)]
    pub partial: bool,
    /// Set when some contributing work was priced at the GLOBAL rates because
    /// its customer's config layer does not resolve (a malformed
    /// `customers/<slug>/config.toml`): the cost may be wrong, and this says
    /// why (naming the customer). Distinct from `partial`, which is about
    /// missing token counts. Folded by [`rollup`] (the first one wins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_error: Option<String>,
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
        out.cache_write_tokens += row.cache_write_tokens;
        out.runs += row.runs;
        match rupu_config::pricing::lookup(pricing, &row.provider, &row.model, &row.agent) {
            Some(price) => {
                any_priced = true;
                cost_acc += price.cost_usd(
                    row.input_tokens,
                    row.output_tokens,
                    row.cached_tokens,
                    row.cache_write_tokens,
                );
            }
            None => out.priced = false,
        }
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_priced { Some(cost_acc) } else { None };
    out
}

/// A run's usage — the one definition (ledger-first, fallback over known
/// transcripts without ledger rows). Incremental and cached process-wide in
/// [`UsageIndex::global`].
pub fn run_usage(store: &RunStore, run_id: &str) -> Arc<RunUsage> {
    UsageIndex::global().run_usage(store, run_id)
}

/// Usage of explicit transcript files (standalone runs, session turns),
/// each labelled by the caller, through the same per-file fold cache.
pub fn transcripts_usage(labeled: &[(String, PathBuf)]) -> Arc<RunUsage> {
    UsageIndex::global().transcripts_usage(labeled)
}

/// An empty result that says it is incomplete — what a failed fold reports.
pub fn unknown_usage() -> Arc<RunUsage> {
    Arc::new(RunUsage {
        partial: true,
        ..RunUsage::default()
    })
}

/// Price a [`RunUsage`]; carries its `partial` flag through.
pub fn summarize_run_usage(u: &RunUsage, pricing: &PricingConfig) -> UsageSummary {
    let mut s = summarize(&u.rows, pricing);
    s.partial = u.partial;
    s
}

/// All transcript paths a run produced, resolved to the file that serves
/// each one on this coordinator: the run's known transcripts (step results,
/// live events, dispatched sub-runs) plus every usage-ledger row's
/// transcript that exists. A recorded path that does not exist locally is
/// mapped to the executing host's mirror cache (`worker_id`,
/// `crate::transcript_paths::cache_path`) or the agent mirror path when that
/// file exists; otherwise the recorded path is kept unchanged (callers
/// tolerate unreadable paths, so a still-open or never-mirrored transcript
/// degrades to "no rows" rather than an error).
///
/// This is the K-resolution helper of [`crate::usage_index`]; usage totals
/// should come from [`run_usage`], not from re-aggregating these paths.
pub fn run_transcript_paths(store: &RunStore, run_id: &str) -> Vec<PathBuf> {
    UsageIndex::global().resolved_transcripts(store, run_id)
}

/// Token + cost summary for a single run.
pub fn summarize_run(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> UsageSummary {
    summarize_run_usage(&run_usage(store, run_id), pricing)
}

/// Token usage + turn count + duration for one run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunMetrics {
    pub usage: UsageSummary,
    /// Number of LLM turns (counted from `Usage` events).
    pub turns: u64,
    /// Wall-clock duration from the transcript's `RunComplete`, if present.
    pub duration_ms: Option<u64>,
}

/// Full per-run metrics for a run in the store: [`run_usage`]'s tokens and
/// turns; the duration is the run record's wall clock once it has finished,
/// else the longest transcript `RunComplete`.
pub fn run_metrics(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> RunMetrics {
    let (u, wall_clock) = UsageIndex::global().run_usage_and_wall_clock(store, run_id);
    RunMetrics {
        usage: summarize_run_usage(&u, pricing),
        turns: u.turns,
        duration_ms: wall_clock.or(u.duration_ms),
    }
}

/// Combine many summaries into one. Token fields add; `priced` ANDs across
/// inputs; `cost_usd` sums priced contributions (a `None` contributes 0 but
/// forces `priced = false` only if the input itself was unpriced). `runs` sums.
pub fn rollup(summaries: impl Iterator<Item = UsageSummary>) -> UsageSummary {
    let mut out = UsageSummary {
        priced: true,
        ..UsageSummary::default()
    };
    let mut any_cost = false;
    let mut cost_acc = 0.0_f64;
    for s in summaries {
        out.input_tokens += s.input_tokens;
        out.output_tokens += s.output_tokens;
        out.cached_tokens += s.cached_tokens;
        out.cache_write_tokens += s.cache_write_tokens;
        out.runs += s.runs;
        if let Some(c) = s.cost_usd {
            any_cost = true;
            cost_acc += c;
        }
        if !s.priced {
            out.priced = false;
        }
        out.partial |= s.partial;
        if out.pricing_error.is_none() {
            out.pricing_error = s.pricing_error;
        }
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_cost { Some(cost_acc) } else { None };
    out
}

/// One per-turn point for the usage timeline. `turn` is a 1-based global index
/// across all contributing transcripts (in order); `label` is the grouping key
/// (step id for a run, run id for a session).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TurnPoint {
    pub turn: u64,
    pub label: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cached: u64,
    #[serde(default)]
    pub tokens_cache_write: u64,
}
