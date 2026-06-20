//! Token + dollar-cost aggregation for the control plane.
//!
//! Pure read-side composition of `rupu_transcript::aggregate` (tokens per
//! provider/model/agent) and `rupu_config::pricing` (USD price lookup +
//! `ModelPricing::cost_usd`). Cost is an estimate: when a model has no
//! resolvable price we report tokens with `priced = false` and never
//! fabricate a dollar figure.

use rupu_config::PricingConfig;
use rupu_orchestrator::runs::RunStore;
use rupu_transcript::TimeWindow;
use rupu_transcript::UsageRow;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// Aggregate the given transcript files and summarize the result.
pub fn summarize_paths(paths: &[PathBuf], pricing: &PricingConfig) -> UsageSummary {
    let rows = rupu_transcript::aggregate(paths, TimeWindow::default());
    summarize(&rows, pricing)
}

/// All transcript paths a run produced: one per step result, plus one per
/// fan-out / panel sub-unit. Missing files are tolerated by
/// `rupu_transcript::aggregate` (it skips unreadable paths).
pub fn run_transcript_paths(store: &RunStore, run_id: &str) -> Vec<PathBuf> {
    let records = store.read_step_results(run_id).unwrap_or_default();
    let mut paths = Vec::new();
    for record in &records {
        paths.push(record.transcript_path.clone());
        for item in &record.items {
            paths.push(item.transcript_path.clone());
        }
    }
    paths
}

/// Token + cost summary for a single run.
pub fn summarize_run(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> UsageSummary {
    summarize_paths(&run_transcript_paths(store, run_id), pricing)
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
        out.runs += s.runs;
        if let Some(c) = s.cost_usd {
            any_cost = true;
            cost_acc += c;
        }
        if !s.priced {
            out.priced = false;
        }
    }
    out.total_tokens = out.input_tokens + out.output_tokens;
    out.cost_usd = if any_cost { Some(cost_acc) } else { None };
    out
}

/// Dimension for the overview breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Provider,
    Model,
    Agent,
}

impl GroupBy {
    /// Parse the `group_by` query param; defaults to `Model` on anything else.
    pub fn parse(s: &str) -> Self {
        match s {
            "provider" => GroupBy::Provider,
            "agent" => GroupBy::Agent,
            _ => GroupBy::Model,
        }
    }
}

/// One grouped line for the overview breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageBreakdownRow {
    pub provider: String,
    pub model: String,
    pub agent: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
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
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            total_tokens: 0,
            cost_usd: None,
            priced: true,
            runs: 0,
        });
        entry.input_tokens += row.input_tokens;
        entry.output_tokens += row.output_tokens;
        entry.cached_tokens += row.cached_tokens;
        entry.runs += row.runs;
        match rupu_config::pricing::lookup(pricing, &row.provider, &row.model, &row.agent) {
            Some(price) => {
                let c = price.cost_usd(row.input_tokens, row.output_tokens, row.cached_tokens);
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

    #[test]
    fn summarize_run_reads_a_transcript() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rupu-cp-usage-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tpath = dir.join("t.jsonl");
        let mut f = std::fs::File::create(&tpath).unwrap();
        writeln!(f, r#"{{"type":"run_start","data":{{"run_id":"r1","workspace_id":"w","agent":"a","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-01-01T00:00:00Z","mode":"ask"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1000000,"output_tokens":0,"cached_tokens":0}}}}"#).unwrap();
        drop(f);

        let s = summarize_paths(&[tpath], &PricingConfig::default());
        assert_eq!(s.input_tokens, 1_000_000);
        assert!(s.priced);
        assert!((s.cost_usd.unwrap() - 3.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollup_sums_and_propagates_unpriced() {
        let priced = UsageSummary { input_tokens: 10, output_tokens: 5, cached_tokens: 0, total_tokens: 15, cost_usd: Some(2.0), priced: true, runs: 1 };
        let unpriced = UsageSummary { input_tokens: 20, output_tokens: 0, cached_tokens: 0, total_tokens: 20, cost_usd: None, priced: false, runs: 1 };
        let r = rollup([priced, unpriced].into_iter());
        assert_eq!(r.input_tokens, 30);
        assert_eq!(r.output_tokens, 5);
        assert_eq!(r.total_tokens, 35);
        assert_eq!(r.runs, 2);
        assert!(!r.priced);
        assert!((r.cost_usd.unwrap() - 2.0).abs() < 1e-9);
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
}
