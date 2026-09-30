//! Token + dollar-cost aggregation for the control plane.
//!
//! **A run's usage is ledger-first** (spec 2026-09-29 §4): the sum of its
//! usage-ledger rows (`<runs>/<id>/usage.jsonl`, one row per LLM call made
//! anywhere in the run, dispatch children included), plus — for every known
//! transcript (step results ∪ live events ∪ dispatched sub-runs) that has no
//! ledger row — a fold of that transcript file, resolved through the host
//! mirror rules. A transcript that should exist but can't be read anywhere
//! marks the result `partial`. That one definition lives in
//! [`crate::usage_index`] (incremental, cached process-wide); this module's
//! [`run_usage`] / [`transcripts_usage`] are its entry points, and
//! [`summarize_run`] / [`run_metrics`] / [`run_transcript_paths`] are built
//! on it.
//!
//! Pricing is `rupu_config::pricing` (USD price lookup +
//! `ModelPricing::cost_usd`), applied here, never stored. Cost is an
//! estimate: when a model has no resolvable price we report tokens with
//! `priced = false` and never fabricate a dollar figure.

use crate::usage_index::{RunUsage, UsageIndex};
use rupu_config::PricingConfig;
use rupu_orchestrator::runs::RunStore;
use rupu_transcript::UsageRow;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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

/// Run a synchronous usage computation on tokio's blocking pool. The fold
/// reads files while holding a per-run (or per-file) `std::sync::Mutex`, so
/// async handlers must not run it on an executor thread. Usage accounting
/// never fails a request: if `f` panics the failure is logged and
/// `fallback()` is returned instead.
pub async fn usage_blocking<T: Send + 'static>(
    what: &'static str,
    f: impl FnOnce() -> T + Send + 'static,
    fallback: impl FnOnce() -> T,
) -> T {
    match tokio::task::spawn_blocking(f).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(what, error = %e, "usage computation failed; reporting it as partial");
            fallback()
        }
    }
}

/// An empty result that says it is incomplete — what a failed fold reports.
pub fn unknown_usage() -> Arc<RunUsage> {
    Arc::new(RunUsage {
        partial: true,
        ..RunUsage::default()
    })
}

/// [`run_usage`] off the async executor (see [`usage_blocking`]). Never
/// fails: a panicked fold reads as an empty, `partial` result.
pub async fn run_usage_blocking(store: Arc<RunStore>, run_id: String) -> Arc<RunUsage> {
    usage_blocking(
        "run_usage",
        move || run_usage(&store, &run_id),
        unknown_usage,
    )
    .await
}

/// [`transcripts_usage`] off the async executor (see [`usage_blocking`]).
/// Never fails: a panicked fold reads as an empty, `partial` result.
pub async fn transcripts_usage_blocking(labeled: Vec<(String, PathBuf)>) -> Arc<RunUsage> {
    usage_blocking(
        "transcripts_usage",
        move || transcripts_usage(&labeled),
        unknown_usage,
    )
    .await
}

/// [`summarize_run`] for each of `run_ids`, in order, off the async executor
/// (see [`usage_blocking`]). Never fails: a panicked fold prices every run as
/// an empty, `partial` result.
pub async fn summarize_runs_blocking(
    store: Arc<RunStore>,
    run_ids: Vec<String>,
    pricing: PricingConfig,
) -> Vec<UsageSummary> {
    let n = run_ids.len();
    let fallback_pricing = pricing.clone();
    usage_blocking(
        "summarize_runs",
        move || {
            run_ids
                .iter()
                .map(|id| summarize_run(&store, id, &pricing))
                .collect()
        },
        move || vec![summarize_run_usage(&unknown_usage(), &fallback_pricing); n],
    )
    .await
}

/// [`run_metrics`] for each of `run_ids`, in order, off the async executor
/// (see [`usage_blocking`]). Never fails: a panicked fold reports every run
/// as an empty, `partial` result.
pub async fn run_metrics_blocking(
    store: Arc<RunStore>,
    run_ids: Vec<String>,
    pricing: PricingConfig,
) -> Vec<RunMetrics> {
    let n = run_ids.len();
    let fallback_pricing = pricing.clone();
    usage_blocking(
        "run_metrics",
        move || {
            run_ids
                .iter()
                .map(|id| run_metrics(&store, id, &pricing))
                .collect()
        },
        move || {
            let usage = summarize_run_usage(&unknown_usage(), &fallback_pricing);
            vec![
                RunMetrics {
                    usage,
                    ..RunMetrics::default()
                };
                n
            ]
        },
    )
    .await
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
/// `host::transcript_paths::cache_path`) or the agent mirror path when that
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
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_cost { Some(cost_acc) } else { None };
    out
}

/// Per-entity rollup: summed usage + run count + most-recent activity.
#[derive(Debug, Clone, Serialize)]
pub struct EntityRollup {
    pub usage: UsageSummary,
    pub run_count: u64,
    /// Most-recent contributing run timestamp (ISO-8601), if any.
    pub last_active: Option<String>,
}

/// An empty rollup is PRICED, matching `rollup(empty)`: `add_spend` ANDs
/// `priced`, so a derived `Default` (`priced: false`) would latch every
/// `rollup_by` entity to unpriced no matter what its runs cost.
impl Default for EntityRollup {
    fn default() -> Self {
        Self {
            usage: UsageSummary {
                priced: true,
                ..UsageSummary::default()
            },
            run_count: 0,
            last_active: None,
        }
    }
}

impl EntityRollup {
    /// Fold one run's usage + timestamp into the rollup.
    pub fn add(&mut self, usage: &UsageSummary, at: Option<String>) {
        self.add_spend(usage, at);
        self.run_count += 1;
    }

    /// Fold spend + activity that is not a run-store run (a standalone agent
    /// run or session turn): usage and `last_active` move, `run_count` —
    /// the entity's workflow-run count — does not.
    pub fn add_spend(&mut self, usage: &UsageSummary, at: Option<String>) {
        self.usage = rollup([self.usage.clone(), usage.clone()].into_iter());
        if let Some(at) = at {
            match &self.last_active {
                Some(cur) if *cur >= at => {}
                _ => self.last_active = Some(at),
            }
        }
    }
}

/// Group every run's usage by a caller-chosen key, computing per-key rollups
/// in a single pass over the store. `key_of` returns `None` to skip a run.
pub fn rollup_by(
    store: &RunStore,
    runs: &[rupu_orchestrator::RunRecord],
    pricing: &PricingConfig,
    key_of: impl Fn(&rupu_orchestrator::RunRecord) -> Option<String>,
) -> BTreeMap<String, EntityRollup> {
    let mut out: BTreeMap<String, EntityRollup> = BTreeMap::new();
    for run in runs {
        let Some(key) = key_of(run) else { continue };
        let usage = summarize_run(store, &run.id, pricing);
        let at = Some(run.started_at.to_rfc3339());
        out.entry(key).or_default().add(&usage, at);
    }
    out
}

/// Dimension for the overview breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Provider,
    Model,
    Agent,
    /// Needs a UsageRow -> RunRecord join; see `breakdown_joined`.
    Workflow,
    Host,
    Project,
}

impl GroupBy {
    /// Parse the `group_by` query param.
    ///
    /// Returns `None` on anything unknown. Deliberately NOT infallible: the
    /// previous `_ => GroupBy::Model` fallthrough meant a typo silently
    /// returned a model breakdown and the caller never learned their pivot was
    /// ignored.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "provider" => Some(GroupBy::Provider),
            "model" => Some(GroupBy::Model),
            "agent" => Some(GroupBy::Agent),
            "workflow" => Some(GroupBy::Workflow),
            "host" => Some(GroupBy::Host),
            "project" => Some(GroupBy::Project),
            _ => None,
        }
    }

    /// Dimensions resolvable from a `UsageRow` alone, with no run join.
    pub fn is_intrinsic(&self) -> bool {
        matches!(self, GroupBy::Provider | GroupBy::Model | GroupBy::Agent)
    }

    /// The wire form of this dimension — the exact string [`Self::parse`]
    /// accepts for it. Used to forward the resolved `group_by` to a remote
    /// host during `/api/usage` fan-out, so every host groups identically
    /// even when the query omitted `group_by` (defaulted to `Model` locally).
    pub fn as_str(&self) -> &'static str {
        match self {
            GroupBy::Provider => "provider",
            GroupBy::Model => "model",
            GroupBy::Agent => "agent",
            GroupBy::Workflow => "workflow",
            GroupBy::Host => "host",
            GroupBy::Project => "project",
        }
    }
}

/// One grouped line for the overview breakdown.
///
/// `Deserialize` is derived for the same reason as [`UsageSummary`]: a
/// remote CP's `breakdown` array is parsed straight into this type during
/// `/api/usage` host fan-out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageBreakdownRow {
    pub provider: String,
    pub model: String,
    pub agent: String,
    /// Present when grouping by a joined dimension; empty otherwise.
    #[serde(default)]
    pub workflow: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub workspace_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    /// Cache writes — a subset of `input_tokens`. Defaulted so a remote CP
    /// that predates it still parses.
    #[serde(default)]
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub priced: bool,
    pub runs: u64,
}

/// Group token rows by the chosen dimension, price each group, and return
/// rows sorted by total tokens descending. The non-grouped identity fields
/// carry the first row's value (or an empty string) — the UI labels by the
/// grouped dimension.
pub fn breakdown(
    rows: &[UsageRow],
    pricing: &PricingConfig,
    group_by: GroupBy,
) -> Vec<UsageBreakdownRow> {
    let mut groups: BTreeMap<String, UsageBreakdownRow> = BTreeMap::new();
    for row in rows {
        let key = match group_by {
            GroupBy::Provider => row.provider.clone(),
            GroupBy::Model => row.model.clone(),
            GroupBy::Agent => row.agent.clone(),
            GroupBy::Workflow => row.workflow.clone(),
            GroupBy::Host => row.host_id.clone(),
            GroupBy::Project => row.workspace_id.clone(),
        };
        let entry = groups.entry(key).or_insert_with(|| UsageBreakdownRow {
            provider: if group_by == GroupBy::Provider {
                row.provider.clone()
            } else {
                String::new()
            },
            model: if group_by == GroupBy::Model {
                row.model.clone()
            } else {
                String::new()
            },
            agent: if group_by == GroupBy::Agent {
                row.agent.clone()
            } else {
                String::new()
            },
            workflow: if group_by == GroupBy::Workflow {
                row.workflow.clone()
            } else {
                String::new()
            },
            host_id: if group_by == GroupBy::Host {
                row.host_id.clone()
            } else {
                String::new()
            },
            workspace_id: if group_by == GroupBy::Project {
                row.workspace_id.clone()
            } else {
                String::new()
            },
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_write_tokens: 0,
            total_tokens: 0,
            cost_usd: None,
            priced: true,
            runs: 0,
        });
        entry.input_tokens += row.input_tokens;
        entry.output_tokens += row.output_tokens;
        entry.cached_tokens += row.cached_tokens;
        entry.cache_write_tokens += row.cache_write_tokens;
        entry.runs += row.runs;
        match rupu_config::pricing::lookup(pricing, &row.provider, &row.model, &row.agent) {
            Some(price) => {
                let c = price.cost_usd(
                    row.input_tokens,
                    row.output_tokens,
                    row.cached_tokens,
                    row.cache_write_tokens,
                );
                entry.cost_usd = Some(entry.cost_usd.unwrap_or(0.0) + c);
            }
            None => entry.priced = false,
        }
    }
    let mut out: Vec<UsageBreakdownRow> = groups
        .into_values()
        .map(|mut r| {
            r.total_tokens = r.input_tokens + r.output_tokens;
            r
        })
        .collect();
    out.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.model.cmp(&b.model))
    });
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

#[cfg(test)]
pub(crate) mod tests {
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
            ..UsageRow::default()
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
    fn summarize_and_breakdown_bill_cache_writes_at_the_write_rate() {
        // Sonnet 4.6 built-in: $3 in / $15 out, $0.30 read, $3.75 write.
        // 1M prompt = 500k read + 300k write + 200k uncached; 100k output.
        let pricing = PricingConfig::default();
        let rows = [UsageRow {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cached_tokens: 500_000,
            cache_write_tokens: 300_000,
            ..row("anthropic", "claude-sonnet-4-6", 0, 0, 0)
        }];
        let want = 0.2 * 3.0 + 0.5 * 0.30 + 0.3 * 3.75 + 0.1 * 15.0;

        let s = summarize(&rows, &pricing);
        assert!(
            (s.cost_usd.unwrap() - want).abs() < 1e-9,
            "{:?}",
            s.cost_usd
        );
        assert_eq!(s.cache_write_tokens, 300_000);

        let b = breakdown(&rows, &pricing, GroupBy::Model);
        assert_eq!(b.len(), 1);
        assert!((b[0].cost_usd.unwrap() - want).abs() < 1e-9);
        assert_eq!(b[0].cache_write_tokens, 300_000);
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

    #[test]
    fn transcripts_usage_prices_a_transcript() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let tpath = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&tpath).unwrap();
        writeln!(f, r#"{{"type":"run_start","data":{{"run_id":"r1","workspace_id":"w","agent":"a","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-01-01T00:00:00Z","mode":"ask"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1000000,"output_tokens":0,"cached_tokens":0}}}}"#).unwrap();
        drop(f);

        let u = transcripts_usage(&[("r1".into(), tpath)]);
        let s = summarize_run_usage(&u, &PricingConfig::default());
        assert_eq!(s.input_tokens, 1_000_000);
        assert!(s.priced);
        assert!(!s.partial);
        assert!((s.cost_usd.unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn rollup_sums_and_propagates_unpriced() {
        let priced = UsageSummary {
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: 0,
            cache_write_tokens: 0,
            total_tokens: 15,
            cost_usd: Some(2.0),
            priced: true,
            runs: 1,
            partial: false,
        };
        let unpriced = UsageSummary {
            input_tokens: 20,
            output_tokens: 0,
            cached_tokens: 0,
            cache_write_tokens: 0,
            total_tokens: 20,
            cost_usd: None,
            priced: false,
            runs: 1,
            partial: false,
        };
        let r = rollup([priced, unpriced].into_iter());
        assert_eq!(r.input_tokens, 30);
        assert_eq!(r.output_tokens, 5);
        assert_eq!(r.total_tokens, 35);
        assert_eq!(r.runs, 2);
        assert!(!r.priced);
        assert!((r.cost_usd.unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn rollup_ors_partial_and_summarize_run_usage_carries_it() {
        let whole = UsageSummary {
            priced: true,
            ..UsageSummary::default()
        };
        let short = UsageSummary {
            priced: true,
            partial: true,
            ..UsageSummary::default()
        };
        assert!(!rollup([whole.clone(), whole.clone()].into_iter()).partial);
        assert!(rollup([whole, short].into_iter()).partial);

        let u = crate::usage_index::RunUsage {
            rows: vec![row("anthropic", "claude-sonnet-4-6", 10, 5, 0)],
            partial: true,
            ..Default::default()
        };
        let s = summarize_run_usage(&u, &PricingConfig::default());
        assert!(s.partial);
        assert_eq!(s.total_tokens, 15);
    }

    #[test]
    fn summarize_and_rollup_carry_cache_write_tokens() {
        let pricing = PricingConfig::default();
        let mut a = row("anthropic", "claude-sonnet-4-6", 1000, 10, 600);
        a.cache_write_tokens = 30;
        let mut b = row("anthropic", "claude-sonnet-4-6", 500, 5, 0);
        b.cache_write_tokens = 5;
        let s = summarize(&[a, b], &pricing);
        assert_eq!(s.cache_write_tokens, 35);
        assert_eq!(s.cached_tokens, 600);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["cache_write_tokens"], serde_json::json!(35), "{v}");

        let r = rollup([s.clone(), s].into_iter());
        assert_eq!(r.cache_write_tokens, 70);
        let mut e = EntityRollup::default();
        e.add(&r, None);
        assert_eq!(e.usage.cache_write_tokens, 70);

        // A remote CP that predates the field (host fan-out) parses as 0.
        let old: UsageSummary = serde_json::from_value(serde_json::json!({
            "input_tokens": 1, "output_tokens": 1, "cached_tokens": 0,
            "total_tokens": 2, "cost_usd": null, "priced": false, "runs": 1
        }))
        .unwrap();
        assert_eq!(old.cache_write_tokens, 0);
    }

    #[test]
    fn breakdown_groups_by_model_and_prices() {
        let pricing = PricingConfig::default();
        let rows = vec![
            row("anthropic", "claude-sonnet-4-6", 1_000_000, 0, 0),
            row("anthropic", "claude-sonnet-4-6", 1_000_000, 0, 0),
            row("internal-vllm", "llama-3-70b", 5, 5, 0),
        ];
        let b = breakdown(&rows, &pricing, GroupBy::Model);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].model, "claude-sonnet-4-6");
        assert_eq!(b[0].input_tokens, 2_000_000);
        assert!(b[0].priced);
        assert!((b[0].cost_usd.unwrap() - 6.0).abs() < 1e-9); // 2M * 3.0
        assert_eq!(b[0].runs, 2);
        assert_eq!(b[1].model, "llama-3-70b");
        assert!(!b[1].priced);
        assert_eq!(b[1].cost_usd, None);
    }

    #[test]
    fn transcripts_usage_counts_turns_and_duration() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let tpath = dir.path().join("t.jsonl");
        let mut f = std::fs::File::create(&tpath).unwrap();
        writeln!(f, r#"{{"type":"run_start","data":{{"run_id":"r1","workspace_id":"w","agent":"a","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-01-01T00:00:00Z","mode":"ask"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1000,"output_tokens":200,"cached_tokens":0}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":800,"output_tokens":150,"cached_tokens":50}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"run_complete","data":{{"run_id":"r1","status":"ok","total_tokens":2150,"duration_ms":38000}}}}"#).unwrap();
        drop(f);
        let u = transcripts_usage(&[("r1".into(), tpath)]);
        let s = summarize_run_usage(&u, &PricingConfig::default());
        assert_eq!(u.turns, 2);
        assert_eq!(u.duration_ms, Some(38000));
        assert_eq!(s.input_tokens, 1800);
        assert_eq!(s.output_tokens, 350);
    }

    #[test]
    fn entity_rollup_folds_usage_and_counts() {
        let mut r = EntityRollup::default();
        r.add(
            &UsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cached_tokens: 0,
                cache_write_tokens: 0,
                total_tokens: 15,
                cost_usd: Some(1.0),
                priced: true,
                runs: 1,
                partial: false,
            },
            Some("2026-01-02T00:00:00Z".into()),
        );
        r.add(
            &UsageSummary {
                input_tokens: 20,
                output_tokens: 0,
                cached_tokens: 0,
                cache_write_tokens: 0,
                total_tokens: 20,
                cost_usd: Some(2.0),
                priced: true,
                runs: 1,
                partial: false,
            },
            Some("2026-01-01T00:00:00Z".into()),
        );
        assert_eq!(r.run_count, 2);
        assert_eq!(r.usage.input_tokens, 30);
        assert_eq!(r.usage.total_tokens, 35);
        assert!((r.usage.cost_usd.unwrap() - 3.0).abs() < 1e-9);
        assert!(r.usage.priced, "all-priced inputs keep the rollup priced");
        assert_eq!(r.last_active.as_deref(), Some("2026-01-02T00:00:00Z"));
    }

    #[test]
    fn group_by_parses_known_dimensions() {
        assert_eq!(GroupBy::parse("model"), Some(GroupBy::Model));
        assert_eq!(GroupBy::parse("provider"), Some(GroupBy::Provider));
        assert_eq!(GroupBy::parse("agent"), Some(GroupBy::Agent));
        assert_eq!(GroupBy::parse("workflow"), Some(GroupBy::Workflow));
        assert_eq!(GroupBy::parse("host"), Some(GroupBy::Host));
        assert_eq!(GroupBy::parse("project"), Some(GroupBy::Project));
    }

    #[test]
    fn group_by_as_str_round_trips_through_parse() {
        for g in [
            GroupBy::Provider,
            GroupBy::Model,
            GroupBy::Agent,
            GroupBy::Workflow,
            GroupBy::Host,
            GroupBy::Project,
        ] {
            assert_eq!(GroupBy::parse(g.as_str()), Some(g));
        }
    }

    #[test]
    fn group_by_rejects_unknown_rather_than_defaulting() {
        // A typo must not silently return a model breakdown — the caller would
        // never learn their pivot was ignored.
        assert_eq!(GroupBy::parse("workflw"), None);
        assert_eq!(GroupBy::parse(""), None);
    }

    #[test]
    fn breakdown_group_by_provider_merges_models() {
        let pricing = PricingConfig::default();
        let rows = vec![
            row("anthropic", "claude-sonnet-4-6", 1000, 0, 0),
            row("anthropic", "claude-haiku-4-5", 1000, 0, 0),
        ];
        let b = breakdown(&rows, &pricing, GroupBy::Provider);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].provider, "anthropic");
        assert_eq!(b[0].input_tokens, 2000);
    }

    // ── Off-executor wrappers ──────────────────────────────────────────────

    #[tokio::test]
    async fn run_usage_blocking_matches_the_sync_fold() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _) = seed_remote_run(tmp.path());
        let cache = tmp.path().join("mirror/host_abc/transcripts/run_01A.jsonl");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(
            &cache,
            concat!(
                r#"{"type":"usage","data":{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":70,"output_tokens":7,"cached_tokens":0}}"#,
                "\n"
            ),
        )
        .unwrap();
        let sync = run_usage(&store, "run_01USAGE");
        let off = run_usage_blocking(std::sync::Arc::clone(&store), "run_01USAGE".into()).await;
        assert_eq!(off.rows, sync.rows);
        assert_eq!(off.turns, sync.turns);
        assert!(!off.partial);
    }

    #[tokio::test]
    async fn a_panicking_usage_computation_degrades_to_partial_never_an_error() {
        let u = usage_blocking(
            "test",
            || -> Arc<RunUsage> { panic!("fold blew up") },
            unknown_usage,
        )
        .await;
        assert!(u.partial, "a failed fold is reported incomplete");
        assert!(u.rows.is_empty());
        assert_eq!(u.turns, 0);
    }

    // ── `run_transcript_paths`' host-mirror fallback (spec §6.3) ──────────

    pub(crate) fn seed_remote_run(tmp: &std::path::Path) -> (std::sync::Arc<RunStore>, PathBuf) {
        let store = std::sync::Arc::new(RunStore::new(tmp.join("runs")));
        let mirror = crate::node::NodeMirror::new(std::sync::Arc::clone(&store));
        let spec = crate::node::protocol::RunSpec {
            kind: crate::node::protocol::RunSpecKind::Workflow,
            name: "wf".into(),
            inputs: Default::default(),
            prompt: None,
            mode: None,
            target: None,
            findings_profile: None,
        };
        mirror.create_run("run_01USAGE", "host_abc", &spec).unwrap();
        let recorded = PathBuf::from("/remote/proj/.rupu/transcripts/run_01A.jsonl");
        store
            .append_step_result(
                "run_01USAGE",
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "a".into(),
                    run_id: "run_01A".into(),
                    transcript_path: recorded.clone(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: Default::default(),
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                    host: None,
                    codename: None,
                },
            )
            .unwrap();
        (store, recorded)
    }

    #[test]
    fn paths_of_a_remote_run_resolve_to_the_cache_when_it_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, _recorded) = seed_remote_run(tmp.path());
        let cache = tmp
            .path()
            .join("mirror")
            .join("host_abc")
            .join("transcripts")
            .join("run_01A.jsonl");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "").unwrap();

        let got = run_transcript_paths(&store, "run_01USAGE");
        assert_eq!(got, vec![cache]);
    }

    #[test]
    fn paths_of_a_remote_run_stay_recorded_when_no_cache_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let (store, recorded) = seed_remote_run(tmp.path());

        let got = run_transcript_paths(&store, "run_01USAGE");
        assert_eq!(got, vec![recorded]);
    }

    #[test]
    fn paths_of_a_local_run_are_never_mapped_even_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = RunStore::new(tmp.path().join("runs"));
        let recorded = PathBuf::from("/does/not/exist/run_local.jsonl");
        let record = rupu_orchestrator::runs::RunRecord {
            id: "run_01LOCAL".into(),
            workflow_name: "wf".into(),
            status: rupu_orchestrator::runs::RunStatus::Completed,
            inputs: Default::default(),
            event: None,
            workspace_id: String::new(),
            workspace_path: PathBuf::from("."),
            transcript_dir: tmp.path().join("run_01LOCAL"),
            started_at: chrono::Utc::now(),
            finished_at: None,
            error_message: None,
            awaiting: Vec::new(),
            awaiting_step_id: None,
            approval_prompt: None,
            awaiting_since: None,
            expires_at: None,
            issue_ref: None,
            issue: None,
            parent_run_id: None,
            backend_id: None,
            worker_id: None,
            artifact_manifest_path: None,
            runner_pid: None,
            source_wake_id: None,
            active_step_id: None,
            active_step_kind: None,
            active_step_agent: None,
            active_step_transcript_path: None,
            resume_requested_at: None,
            resume_claimed_at: None,
            resume_claimed_by: None,
            resume_mode: None,
            resume_gate_id: None,
            resume_approver: None,
            reject_cleanup_pending: None,
            permission_mode: None,
            final_output: None,
            loop_progress: Default::default(),
            codename: None,
        };
        store.create(record, "").unwrap();
        store
            .append_step_result(
                "run_01LOCAL",
                &rupu_orchestrator::runs::StepResultRecord {
                    step_id: "a".into(),
                    run_id: "run_01LOCAL".into(),
                    transcript_path: recorded.clone(),
                    output: String::new(),
                    success: true,
                    skipped: false,
                    rendered_prompt: String::new(),
                    kind: Default::default(),
                    items: vec![],
                    findings: vec![],
                    iterations: 0,
                    resolved: true,
                    finished_at: chrono::Utc::now(),
                    loop_iteration: None,
                    run_outcome: None,
                    host: None,
                    codename: None,
                },
            )
            .unwrap();

        let got = run_transcript_paths(&store, "run_01LOCAL");
        assert_eq!(got, vec![recorded]);
    }
}
