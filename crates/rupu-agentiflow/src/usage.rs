//! The token fold: sums the usage ledgers (`usage.jsonl`) of an agentiflow's
//! lead and of every unit it launched into per-model and grand totals, and
//! [`LedgerUsageSource`], the budget enforcer's [`UsageSource`] built on it
//! (`budget.tokens` from the folded tokens, `budget.usd` from those tokens
//! priced through the layered `[pricing]` config).
//!
//! Each line is a [`rupu_orchestrator::usage_ledger::LedgerRow`]. The fold is
//! tolerant the way the ledger's other readers are: a missing file contributes
//! nothing, and a blank, non-JSON, non-UTF-8 or partially written line is
//! skipped rather than failing the fold (a ledger is appended to by live
//! processes, so the tail line can be mid-write). Rows are de-duplicated by
//! `LedgerRow.id` across ALL the files read, so a line seen twice (a re-read, a
//! mirror replay, one row present in two ledgers) counts once.
//!
//! # Known limitation: dispatched sub-agents are not counted
//!
//! A unit's own LLM calls are in its `usage.jsonl`, but the sub-agents a unit
//! launches through `dispatch_agent` are not: that dispatcher has no usage
//! ledger (a gap that predates the agentiflow fold). The fold therefore
//! UNDERCOUNTS the spend of any unit that dispatches sub-agents, and a
//! `budget.tokens` / `budget.usd` cap trips later than the true spend. See the
//! `dispatch_agent` entry in `TODO.md`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rupu_config::pricing::lookup;
use rupu_config::PricingConfig;
use rupu_orchestrator::usage_ledger::LedgerRow;

use crate::budget::UsageSource;
use crate::supervisor::FleetSupervisor;

/// A token count, in the ledger's own terms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input tokens, INCLUDING the cache reads and cache writes below.
    pub input: u64,
    /// Output tokens (already inclusive of any reasoning tokens the provider
    /// bills as output).
    pub output: u64,
    /// Cache reads: a subset of `input`.
    pub cached: u64,
    /// Cache writes: a subset of `input`.
    pub cache_write: u64,
}

impl Tokens {
    /// The tokens a `budget.tokens` cap counts: input plus output. `cached` and
    /// `cache_write` are subsets of `input`, so adding them would double count.
    pub fn billable(&self) -> u64 {
        self.input.saturating_add(self.output)
    }

    /// Add `other` into `self`, saturating rather than overflowing.
    pub fn add(&mut self, other: &Tokens) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cached = self.cached.saturating_add(other.cached);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}

impl std::ops::AddAssign<&Tokens> for Tokens {
    fn add_assign(&mut self, other: &Tokens) {
        self.add(other);
    }
}

/// The folded usage of a set of ledgers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenTotals {
    /// Tokens per `(provider, model)`.
    pub by_model: BTreeMap<(String, String), Tokens>,
    /// The sum over every model.
    pub total: Tokens,
}

impl TokenTotals {
    fn record(&mut self, row: &LedgerRow) {
        let t = Tokens {
            input: row.input_tokens,
            output: row.output_tokens,
            cached: row.cached_tokens,
            cache_write: row.cache_write_tokens,
        };
        self.by_model
            .entry((row.provider.clone(), row.model.clone()))
            .or_default()
            .add(&t);
        self.total.add(&t);
    }
}

/// Fold every `usage.jsonl` in `usage_jsonl_paths` into [`TokenTotals`].
///
/// Rows are de-duplicated by `LedgerRow.id` across all the files. A path that
/// does not exist (a unit that never called a model) or cannot be read
/// contributes nothing, and an unparseable line is skipped. See the module docs
/// for the `dispatch_agent` undercount.
pub fn fold_tokens(usage_jsonl_paths: &[PathBuf]) -> TokenTotals {
    let mut totals = TokenTotals::default();
    let mut seen: HashSet<String> = HashSet::new();
    for path in usage_jsonl_paths {
        fold_file(path, &mut seen, &mut totals);
    }
    totals
}

fn fold_file(path: &Path, seen: &mut HashSet<String>, totals: &mut TokenTotals) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "usage fold: cannot open ledger");
            }
            return;
        }
    };
    // Split on raw bytes so one non-UTF-8 line cannot fail the whole file.
    for line in std::io::BufReader::new(file).split(b'\n') {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "usage fold: read failed");
                return;
            }
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(row) = serde_json::from_slice::<LedgerRow>(&line) else {
            continue; // garbled, foreign or half-written line
        };
        if seen.insert(row.id.clone()) {
            totals.record(&row);
        }
    }
}

/// Whether `provider` / `model` has a USD price under `pricing` (a user
/// `[pricing.<provider>."<model>"]` entry or the built-in table).
///
/// This is the launch-time gate on `budget.usd`: a cap on spend that cannot be
/// priced would read `$0` forever, so it must not be treated as enforced. The
/// per-agent `[pricing.agents.*]` fallback is NOT consulted (here or in the
/// meter): the fold is keyed by `(provider, model)`, not by agent, so pricing
/// a private endpoint means a `[pricing.<provider>."<model>"]` entry.
pub(crate) fn usd_is_priceable(pricing: &PricingConfig, provider: &str, model: &str) -> bool {
    lookup(pricing, provider, model, "").is_some()
}

/// The ledger-backed [`UsageSource`] for a running agentiflow: what the lead
/// and every unit it has launched have spent, read from their `usage.jsonl`
/// ledgers on every query.
///
/// - the lead's ledger is `lead_usage` (`<run dir>/usage.jsonl`, written by the
///   lead driver's `on_usage` hook);
/// - each unit's is `<global>/runs/<unit id>/usage.jsonl`, found through
///   [`FleetSupervisor::launched_unit_run_dirs`] (a fleet-attached `rupu run`
///   writes it), so units that finished still count and a unit the supervisor
///   has not launched yet adds nothing.
///
/// Tokens are [`Tokens::billable`] (input + output; cache reads and writes are
/// subsets of input). USD prices each `(provider, model)` bucket's input /
/// output / cache counts with [`rupu_config::pricing::ModelPricing::cost_usd`];
/// a model with no price contributes `$0` (and is logged once, since its spend
/// is then invisible to a `budget.usd` cap).
///
/// The envelope queries each dimension once per round, and each query re-folds
/// the ledgers (the files are small and rounds are minutes apart); there is no
/// cache to go stale against a unit still writing.
pub struct LedgerUsageSource {
    lead_usage: PathBuf,
    supervisor: Arc<FleetSupervisor>,
    global: PathBuf,
    pricing: PricingConfig,
    /// `(provider, model)` pairs already warned about as unpriced.
    warned_unpriced: Mutex<BTreeSet<(String, String)>>,
}

impl LedgerUsageSource {
    pub fn new(
        lead_usage: PathBuf,
        supervisor: Arc<FleetSupervisor>,
        global: PathBuf,
        pricing: PricingConfig,
    ) -> Self {
        Self {
            lead_usage,
            supervisor,
            global,
            pricing,
            warned_unpriced: Mutex::new(BTreeSet::new()),
        }
    }

    /// Every ledger this run's spend lives in: the lead's, then each launched
    /// unit's (oldest first).
    fn ledger_paths(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.lead_usage.clone()];
        paths.extend(
            self.supervisor
                .launched_unit_run_dirs(&self.global)
                .into_iter()
                .map(|dir| dir.join("usage.jsonl")),
        );
        paths
    }

    /// Fold the lead's and every launched unit's ledger once. Both budget
    /// dimensions are derived from one of these.
    pub fn fold_all(&self) -> TokenTotals {
        fold_tokens(&self.ledger_paths())
    }

    /// The USD cost of `totals`: each `(provider, model)` bucket priced on its
    /// own, summed; an unpriced bucket adds `0`.
    fn price(&self, totals: &TokenTotals) -> f64 {
        let mut usd = 0.0;
        for ((provider, model), t) in &totals.by_model {
            match lookup(&self.pricing, provider, model, "") {
                Some(p) => usd += p.cost_usd(t.input, t.output, t.cached, t.cache_write),
                None if t.billable() > 0 => self.warn_unpriced_once(provider, model),
                None => {}
            }
        }
        usd
    }

    fn warn_unpriced_once(&self, provider: &str, model: &str) {
        let first = self
            .warned_unpriced
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((provider.to_string(), model.to_string()));
        if first {
            tracing::warn!(
                provider,
                model,
                "no price for this model: its spend is counted as $0 against budget.usd \
                 (budget.tokens still counts it); add a [pricing.<provider>.\"<model>\"] entry"
            );
        }
    }
}

impl UsageSource for LedgerUsageSource {
    fn spent_usd(&self) -> f64 {
        self.price(&self.fold_all())
    }

    fn spent_tokens(&self) -> u64 {
        self.fold_all().total.billable()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::usage_ledger::{LedgerKind, LEDGER_VERSION};
    use std::io::Write as _;

    #[allow(clippy::too_many_arguments)]
    fn row(
        id: &str,
        provider: &str,
        model: &str,
        input: u64,
        output: u64,
        cached: u64,
        cache_write: u64,
    ) -> LedgerRow {
        LedgerRow {
            v: LEDGER_VERSION,
            id: id.into(),
            at: Utc::now(),
            kind: LedgerKind::Turn,
            step_id: None,
            unit_index: None,
            unit_key: None,
            agent_run_id: "ar_1".into(),
            parent_agent_run_id: None,
            transcript: PathBuf::from("/t/x.jsonl"),
            agent: "recon".into(),
            provider: provider.into(),
            model: model.into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
            cache_write_tokens: cache_write,
        }
    }

    fn line(r: &LedgerRow) -> Vec<u8> {
        let mut v = serde_json::to_vec(r).unwrap();
        v.push(b'\n');
        v
    }

    fn write_ledger(path: &Path, rows: &[LedgerRow]) {
        let mut bytes = Vec::new();
        for r in rows {
            bytes.extend(line(r));
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn key(provider: &str, model: &str) -> (String, String) {
        (provider.to_string(), model.to_string())
    }

    #[test]
    fn folds_lead_and_unit_ledgers_per_model_and_in_total() {
        let dir = tempfile::tempdir().unwrap();
        let lead = dir.path().join("lead-usage.jsonl");
        let unit = dir.path().join("unit-usage.jsonl");
        write_ledger(
            &lead,
            &[
                row("01A", "anthropic", "opus", 1000, 100, 400, 50),
                row("01B", "anthropic", "opus", 2000, 200, 0, 0),
            ],
        );
        write_ledger(
            &unit,
            &[
                row("01C", "anthropic", "opus", 500, 50, 10, 5),
                row("01D", "openai", "gpt-x", 300, 30, 0, 0),
            ],
        );

        let got = fold_tokens(&[lead, unit]);

        assert_eq!(
            got.by_model[&key("anthropic", "opus")],
            Tokens {
                input: 3500,
                output: 350,
                cached: 410,
                cache_write: 55,
            }
        );
        assert_eq!(
            got.by_model[&key("openai", "gpt-x")],
            Tokens {
                input: 300,
                output: 30,
                cached: 0,
                cache_write: 0,
            }
        );
        assert_eq!(got.by_model.len(), 2);
        assert_eq!(
            got.total,
            Tokens {
                input: 3800,
                output: 380,
                cached: 410,
                cache_write: 55,
            }
        );
        // Cache tokens are subsets of input: billable is input + output only.
        assert_eq!(got.total.billable(), 3800 + 380);
    }

    #[test]
    fn a_duplicated_row_id_counts_once_across_and_within_files() {
        let dir = tempfile::tempdir().unwrap();
        let lead = dir.path().join("lead.jsonl");
        let unit = dir.path().join("unit.jsonl");
        let dup = row("01DUP", "anthropic", "opus", 700, 70, 0, 0);
        // Twice in the lead's file, and once more in the unit's.
        write_ledger(
            &lead,
            &[
                dup.clone(),
                row("01A", "anthropic", "opus", 100, 10, 0, 0),
                dup.clone(),
            ],
        );
        write_ledger(
            &unit,
            &[dup, row("01B", "anthropic", "opus", 200, 20, 0, 0)],
        );

        let got = fold_tokens(&[lead.clone(), unit.clone()]);
        assert_eq!(got.total.input, 700 + 100 + 200);
        assert_eq!(got.total.output, 70 + 10 + 20);
        assert_eq!(
            got.by_model[&key("anthropic", "opus")],
            got.total,
            "one model: its tokens are the total"
        );

        // Re-reading the same files (a re-listed path) changes nothing.
        let again = fold_tokens(&[lead.clone(), unit, lead]);
        assert_eq!(again, got);
    }

    #[test]
    fn a_missing_path_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("usage.jsonl");
        write_ledger(&real, &[row("01A", "anthropic", "opus", 10, 5, 0, 0)]);
        let missing = dir.path().join("runs").join("run_gone").join("usage.jsonl");

        let got = fold_tokens(&[missing.clone(), real, missing.clone()]);
        assert_eq!(got.total.billable(), 15);

        assert_eq!(fold_tokens(&[missing]), TokenTotals::default());
        assert_eq!(fold_tokens(&[]), TokenTotals::default());
    }

    #[test]
    fn junk_blank_and_non_utf8_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&line(&row("01A", "anthropic", "opus", 100, 10, 0, 0)))
            .unwrap();
        f.write_all(b"not json at all\n").unwrap();
        f.write_all(b"\n   \n").unwrap();
        f.write_all(b"{\"v\":1,\"id\":\"01X\"}\n").unwrap(); // JSON, but not a row
        f.write_all(&[0xff, 0xfe, 0xfd, b'\n']).unwrap(); // not UTF-8
        f.write_all(&line(&row("01B", "anthropic", "opus", 200, 20, 0, 0)))
            .unwrap();
        // A half-written tail line (no newline) from a live writer.
        let mut tail = serde_json::to_vec(&row("01C", "anthropic", "opus", 999, 99, 0, 0)).unwrap();
        tail.truncate(tail.len() / 2);
        f.write_all(&tail).unwrap();
        drop(f);

        let got = fold_tokens(&[path]);
        assert_eq!(got.total.input, 300);
        assert_eq!(got.total.output, 30);
    }

    #[test]
    fn a_row_without_cache_write_tokens_reads_as_zero() {
        // Ledgers written before `cache_write_tokens` existed still fold.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let mut v = serde_json::to_value(row("01A", "anthropic", "opus", 40, 4, 8, 0)).unwrap();
        v.as_object_mut().unwrap().remove("cache_write_tokens");
        v.as_object_mut().unwrap().remove("cached_tokens");
        std::fs::write(&path, format!("{v}\n")).unwrap();

        let got = fold_tokens(&[path]);
        assert_eq!(
            got.total,
            Tokens {
                input: 40,
                output: 4,
                cached: 0,
                cache_write: 0,
            }
        );
    }

    #[test]
    fn tokens_add_saturates() {
        let mut a = Tokens {
            input: u64::MAX,
            output: 1,
            cached: 0,
            cache_write: 0,
        };
        a += &Tokens {
            input: 5,
            output: 2,
            cached: 3,
            cache_write: 4,
        };
        assert_eq!(a.input, u64::MAX);
        assert_eq!(a.output, 3);
        assert_eq!(a.billable(), u64::MAX);
    }

    // ---- LedgerUsageSource ---------------------------------------------------

    use crate::budget::{Budget, BudgetEnforcer, BudgetStage};
    use crate::unit::{MockUnitLauncher, UnitKind, UnitSpec, UnitStatus};
    use rupu_config::ModelPricing;

    fn unit_spec(participant: &str) -> UnitSpec {
        UnitSpec {
            agent: "recon".into(),
            prompt: "scan".into(),
            engagement: vec![],
            participant: participant.into(),
            kind: UnitKind::Agent,
            inputs: vec![],
            workflow_file: None,
        }
    }

    /// `[pricing.priced."m"]`: $2 / Mtok in, $10 / Mtok out, $0.5 / Mtok cache
    /// reads, $4 / Mtok cache writes.
    fn test_pricing() -> PricingConfig {
        let mut cfg = PricingConfig::default();
        cfg.models.entry("priced".into()).or_default().insert(
            "m".into(),
            ModelPricing {
                input_per_mtok: 2.0,
                output_per_mtok: 10.0,
                cached_input_per_mtok: Some(0.5),
                cache_write_per_mtok: Some(4.0),
            },
        );
        cfg
    }

    struct Rig {
        _tmp: tempfile::TempDir,
        global: PathBuf,
        lead_usage: PathBuf,
        sup: Arc<FleetSupervisor>,
    }

    impl Rig {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let global = tmp.path().join("global");
            let run_dir = tmp.path().join("af_run");
            std::fs::create_dir_all(&global).unwrap();
            std::fs::create_dir_all(&run_dir).unwrap();
            let sup = Arc::new(FleetSupervisor::new(
                Arc::new(MockUnitLauncher::scripted(vec![UnitStatus::Running])),
                run_dir.clone(),
            ));
            Rig {
                lead_usage: run_dir.join("usage.jsonl"),
                _tmp: tmp,
                global,
                sup,
            }
        }

        fn source(&self, pricing: PricingConfig) -> LedgerUsageSource {
            LedgerUsageSource::new(
                self.lead_usage.clone(),
                self.sup.clone(),
                self.global.clone(),
                pricing,
            )
        }

        /// Launch a unit and seed its `usage.jsonl` under `<global>/runs/<id>/`.
        fn unit_with(&self, participant: &str, rows: &[LedgerRow]) -> String {
            let id = self.sup.dispatch(unit_spec(participant)).unwrap();
            let dir = self.global.join("runs").join(&id);
            std::fs::create_dir_all(&dir).unwrap();
            write_ledger(&dir.join("usage.jsonl"), rows);
            id
        }
    }

    #[test]
    fn the_source_sums_the_leads_and_every_launched_units_billable_tokens() {
        let rig = Rig::new();
        write_ledger(
            &rig.lead_usage,
            &[row("01A", "priced", "m", 1000, 100, 400, 50)],
        );
        rig.unit_with("recon#1", &[row("01B", "priced", "m", 500, 50, 10, 0)]);
        rig.unit_with("recon#2", &[row("01C", "other", "x", 300, 30, 0, 0)]);
        // A launched unit that never called a model has no ledger: it adds nothing.
        rig.sup.dispatch(unit_spec("recon#3")).unwrap();
        // A run dir the supervisor did not launch is not this flow's spend.
        let stray = rig.global.join("runs").join("run_stranger");
        std::fs::create_dir_all(&stray).unwrap();
        write_ledger(
            &stray.join("usage.jsonl"),
            &[row("01Z", "priced", "m", 9_000_000, 9_000_000, 0, 0)],
        );

        let src = rig.source(test_pricing());
        // input + output only: cached / cache_write are subsets of input.
        assert_eq!(src.spent_tokens(), (1000 + 100) + (500 + 50) + (300 + 30));
        assert_eq!(src.fold_all().total.billable(), src.spent_tokens());
    }

    #[test]
    fn the_source_prices_each_model_bucket_and_counts_unpriced_models_as_zero() {
        let rig = Rig::new();
        // Lead: 1M in (400k cache reads, 100k cache writes) + 200k out, priced.
        write_ledger(
            &rig.lead_usage,
            &[row(
                "01A", "priced", "m", 1_000_000, 200_000, 400_000, 100_000,
            )],
        );
        // A unit on the same model plus one on a model nothing prices.
        rig.unit_with("recon#1", &[row("01B", "priced", "m", 500_000, 0, 0, 0)]);
        rig.unit_with(
            "recon#2",
            &[row(
                "01C", "mystery", "no-price", 7_000_000, 7_000_000, 0, 0,
            )],
        );

        let src = rig.source(test_pricing());
        let price = test_pricing().models["priced"]["m"];
        let want =
            price.cost_usd(1_000_000, 200_000, 400_000, 100_000) + price.cost_usd(500_000, 0, 0, 0);
        assert!(want > 0.0);
        let got = src.spent_usd();
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
        // The unpriced model's tokens still count against budget.tokens.
        assert_eq!(
            src.spent_tokens(),
            1_000_000 + 200_000 + 500_000 + 14_000_000
        );
    }

    #[test]
    fn with_no_pricing_for_any_model_the_usd_spent_is_zero() {
        let rig = Rig::new();
        write_ledger(
            &rig.lead_usage,
            &[row(
                "01A", "mystery", "no-price", 1_000_000, 1_000_000, 0, 0,
            )],
        );
        let src = rig.source(PricingConfig::default());
        assert_eq!(src.spent_usd(), 0.0);
        assert_eq!(src.spent_tokens(), 2_000_000);
    }

    #[test]
    fn a_source_with_no_ledgers_yet_reports_nothing_spent() {
        let rig = Rig::new();
        let src = rig.source(test_pricing());
        assert_eq!(src.spent_tokens(), 0);
        assert_eq!(src.spent_usd(), 0.0);
    }

    #[test]
    fn the_source_re_reads_the_ledgers_on_every_query() {
        let rig = Rig::new();
        let src = rig.source(test_pricing());
        assert_eq!(src.spent_tokens(), 0);
        write_ledger(&rig.lead_usage, &[row("01A", "priced", "m", 10, 5, 0, 0)]);
        assert_eq!(src.spent_tokens(), 15);
        rig.unit_with("recon#1", &[row("01B", "priced", "m", 100, 50, 0, 0)]);
        assert_eq!(src.spent_tokens(), 15 + 150);
    }

    #[test]
    fn a_tokens_cap_below_the_folded_total_is_a_hard_stop() {
        let rig = Rig::new();
        write_ledger(
            &rig.lead_usage,
            &[row("01A", "priced", "m", 600, 100, 0, 0)],
        );
        rig.unit_with("recon#1", &[row("01B", "priced", "m", 300, 100, 0, 0)]);
        let src = rig.source(test_pricing());
        let started = Utc::now();
        let stage = |tokens: u64, soft_at: Option<f64>| {
            BudgetEnforcer::new(
                Budget {
                    tokens: Some(tokens),
                    soft_at,
                    ..Budget::default()
                },
                started,
            )
            .stage(&src, 0, started)
        };

        // 1100 billable tokens folded across the lead and the unit.
        assert_eq!(
            stage(1000, None),
            BudgetStage::Hard {
                dimension: "tokens".into()
            }
        );
        assert_eq!(
            stage(1100, None),
            BudgetStage::Hard {
                dimension: "tokens".into()
            }
        );
        assert_eq!(stage(1200, None), BudgetStage::Soft); // 1100/1200 >= 0.8
        assert_eq!(stage(1_000_000, None), BudgetStage::Ok);
    }

    #[test]
    fn a_usd_cap_trips_on_the_priced_spend() {
        let rig = Rig::new();
        // $2 in + $1 out = $3.00 on the lead.
        write_ledger(
            &rig.lead_usage,
            &[row("01A", "priced", "m", 1_000_000, 100_000, 0, 0)],
        );
        let src = rig.source(test_pricing());
        let started = Utc::now();
        let stage = |usd: f64| {
            BudgetEnforcer::new(
                Budget {
                    usd: Some(usd),
                    ..Budget::default()
                },
                started,
            )
            .stage(&src, 0, started)
        };
        assert_eq!(
            stage(3.0),
            BudgetStage::Hard {
                dimension: "usd".into()
            }
        );
        assert_eq!(stage(100.0), BudgetStage::Ok);
    }

    #[test]
    fn usd_priceability_follows_user_and_builtin_prices() {
        let none = PricingConfig::default();
        // The built-in table prices a known vendor model...
        assert!(usd_is_priceable(&none, "anthropic", "claude-sonnet-5-5"));
        // ...but not a mock / private model.
        assert!(!usd_is_priceable(&none, "mock", "mock-1"));
        assert!(!usd_is_priceable(&none, "priced", "m"));
        // A user entry prices it, a named account resolves to its vendor's table.
        assert!(usd_is_priceable(&test_pricing(), "priced", "m"));
        let mut acct = PricingConfig::default();
        acct.provider_kinds
            .insert("anthropic-work".into(), "anthropic".into());
        assert!(usd_is_priceable(
            &acct,
            "anthropic-work",
            "claude-sonnet-5-5"
        ));
        // An agent-level fallback does not make a MODEL priceable (the fold is
        // keyed by model, not agent).
        let mut agent_only = PricingConfig::default();
        agent_only
            .agents
            .insert("lead".into(), ModelPricing::default());
        assert!(!usd_is_priceable(&agent_only, "mock", "mock-1"));
    }
}
