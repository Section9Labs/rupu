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
use rupu_transcript::{Event, JsonlReader};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

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

/// Token usage + turn count + duration for one run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunMetrics {
    pub usage: UsageSummary,
    /// Number of LLM turns (counted from `Usage` events).
    pub turns: u64,
    /// Wall-clock duration from the transcript's `RunComplete`, if present.
    pub duration_ms: Option<u64>,
}

/// Count turns (one per `Usage` event) and capture `RunComplete.duration_ms`
/// across the given transcripts. Tolerates unreadable/partial files.
fn turns_and_duration(paths: &[PathBuf]) -> (u64, Option<u64>) {
    let mut turns = 0u64;
    let mut duration_ms = None;
    for path in paths {
        let Ok(iter) = JsonlReader::iter(path) else {
            continue;
        };
        for ev in iter.flatten() {
            match ev {
                Event::Usage { .. } => turns += 1,
                Event::RunComplete { duration_ms: d, .. } => duration_ms = Some(d),
                _ => {}
            }
        }
    }
    (turns, duration_ms)
}

/// Full per-run metrics (usage + turns + duration) from transcript paths.
pub fn run_metrics_paths(paths: &[PathBuf], pricing: &PricingConfig) -> RunMetrics {
    let usage = summarize_paths(paths, pricing);
    let (turns, duration_ms) = turns_and_duration(paths);
    RunMetrics {
        usage,
        turns,
        duration_ms,
    }
}

/// Full per-run metrics for a run in the store.
pub fn run_metrics(store: &RunStore, run_id: &str, pricing: &PricingConfig) -> RunMetrics {
    run_metrics_paths(&run_transcript_paths(store, run_id), pricing)
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

/// Per-entity rollup: summed usage + run count + most-recent activity.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EntityRollup {
    pub usage: UsageSummary,
    pub run_count: u64,
    /// Most-recent contributing run timestamp (ISO-8601), if any.
    pub last_active: Option<String>,
}

impl EntityRollup {
    /// Fold one run's usage + timestamp into the rollup.
    pub fn add(&mut self, usage: &UsageSummary, at: Option<String>) {
        self.usage = rollup([self.usage.clone(), usage.clone()].into_iter());
        self.run_count += 1;
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
}

/// Build an ordered per-turn token series from labeled transcripts. Each
/// `Usage` event becomes one point; transcripts are read in the given order
/// and the `turn` counter is global across all of them. Unreadable/partial
/// files are skipped.
pub fn turn_series(labeled_paths: &[(String, PathBuf)]) -> Vec<TurnPoint> {
    let mut out: Vec<TurnPoint> = Vec::new();
    for (label, path) in labeled_paths {
        let Ok(iter) = JsonlReader::iter(path) else {
            continue;
        };
        for ev in iter.flatten() {
            if let Event::Usage {
                input_tokens,
                output_tokens,
                cached_tokens,
                ..
            } = ev
            {
                out.push(TurnPoint {
                    turn: out.len() as u64 + 1,
                    label: label.clone(),
                    tokens_in: u64::from(input_tokens),
                    tokens_out: u64::from(output_tokens),
                    tokens_cached: u64::from(cached_tokens),
                });
            }
        }
    }
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
        let priced = UsageSummary {
            input_tokens: 10,
            output_tokens: 5,
            cached_tokens: 0,
            total_tokens: 15,
            cost_usd: Some(2.0),
            priced: true,
            runs: 1,
        };
        let unpriced = UsageSummary {
            input_tokens: 20,
            output_tokens: 0,
            cached_tokens: 0,
            total_tokens: 20,
            cost_usd: None,
            priced: false,
            runs: 1,
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
    fn run_metrics_counts_turns_and_duration() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rupu-cp-metrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tpath = dir.join("t.jsonl");
        let mut f = std::fs::File::create(&tpath).unwrap();
        writeln!(f, r#"{{"type":"run_start","data":{{"run_id":"r1","workspace_id":"w","agent":"a","provider":"anthropic","model":"claude-sonnet-4-6","started_at":"2026-01-01T00:00:00Z","mode":"ask"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":1000,"output_tokens":200,"cached_tokens":0}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"claude-sonnet-4-6","input_tokens":800,"output_tokens":150,"cached_tokens":50}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"run_complete","data":{{"run_id":"r1","status":"ok","total_tokens":2150,"duration_ms":38000}}}}"#).unwrap();
        drop(f);
        let m = run_metrics_paths(&[tpath], &PricingConfig::default());
        assert_eq!(m.turns, 2);
        assert_eq!(m.duration_ms, Some(38000));
        assert_eq!(m.usage.input_tokens, 1800);
        assert_eq!(m.usage.output_tokens, 350);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entity_rollup_folds_usage_and_counts() {
        let mut r = EntityRollup::default();
        r.add(
            &UsageSummary {
                input_tokens: 10,
                output_tokens: 5,
                cached_tokens: 0,
                total_tokens: 15,
                cost_usd: Some(1.0),
                priced: true,
                runs: 1,
            },
            Some("2026-01-02T00:00:00Z".into()),
        );
        r.add(
            &UsageSummary {
                input_tokens: 20,
                output_tokens: 0,
                cached_tokens: 0,
                total_tokens: 20,
                cost_usd: Some(2.0),
                priced: true,
                runs: 1,
            },
            Some("2026-01-01T00:00:00Z".into()),
        );
        assert_eq!(r.run_count, 2);
        assert_eq!(r.usage.input_tokens, 30);
        assert_eq!(r.usage.total_tokens, 35);
        assert!((r.usage.cost_usd.unwrap() - 3.0).abs() < 1e-9);
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

    #[test]
    fn turn_series_aggregates_labeled_transcripts_in_order() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("rupu-cp-turnseries-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |name: &str, n: usize| {
            let p = dir.join(name);
            let mut f = std::fs::File::create(&p).unwrap();
            for i in 0..n {
                writeln!(f, r#"{{"type":"usage","data":{{"provider":"anthropic","model":"m","input_tokens":{},"output_tokens":1,"cached_tokens":0}}}}"#, 100 + i).unwrap();
            }
            p
        };
        let a = mk("a.jsonl", 2);
        let b = mk("b.jsonl", 1);
        let series = turn_series(&[("step1".into(), a), ("step2".into(), b)]);
        assert_eq!(series.len(), 3);
        assert_eq!(series[0].turn, 1);
        assert_eq!(series[0].label, "step1");
        assert_eq!(series[0].tokens_in, 100);
        assert_eq!(series[2].turn, 3);
        assert_eq!(series[2].label, "step2");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
